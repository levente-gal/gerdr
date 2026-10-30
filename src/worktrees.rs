//! The worktrees popup (prefix, w; or a click on a workspace's worktree
//! count): the worktrees of the workspace's repository, or (Tab) of every
//! repository the workspaces are in, a section per repository, to review,
//! squash into the main worktree and delete, as agents leave them behind.
//!
//! Typing filters them by name, path and repository. Up/Down select, Enter
//! (or Ctrl+D) shows its diff against the main worktree's branch in place
//! of the popup, in the repositories panel's diff view (see
//! `worktree_diff`); Esc there comes back. Ctrl+K checks its branch out
//! in the main worktree, Ctrl+M squashes it in there (in a tab, for the
//! commit message and any conflicts), Delete
//! deletes it (twice when that loses changes or commits), Shift+Delete
//! deletes the repository's merged ones, Ctrl+O opens a tab in it (Zed
//! over a terminal), Ctrl+C copies its path, Esc clears the filter, then
//! closes.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Instant,
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, LineState, WorktreeButton, WorktreeEntry, WorktreeLine, WorktreesHit, WorktreesView,
    },
    fuzzy,
    git::{self, Untracked, Worktree},
    git_ops::Turn,
    input::Overlay,
    menu::MenuAction,
    paths,
    scroll::{Bounds, Motion, Scroll},
    text_field::{Edit, TextField},
    toast::Level,
    tooltip::Tooltip,
};

/// The worktrees popup, while open, and what is being done to worktrees.
#[derive(Default)]
pub struct Worktrees {
    popup: Option<Popup>,
    /// The worktrees being deleted or checked out, by path, with what
    /// their rows say meanwhile.
    busy: HashMap<PathBuf, &'static str>,
    /// The worktrees squashed in a tab, by path, until they read as merged.
    merging: HashSet<PathBuf>,
    /// Hands the deletes' and checkouts' results to the event loop.
    done: Option<channel::Sender<Done>>,
    /// The popup as it was when a worktree's diff took its place, to go
    /// back to.
    parked: Option<Popup>,
}

impl Worktrees {
    /// Stop the popup's list flung; whether it was.
    pub fn stop_fling(&mut self) -> bool {
        self.popup.as_mut().is_some_and(Popup::stop_fling)
    }

    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }

    pub fn close(&mut self) {
        self.popup = None;
        self.parked = None;
    }

    /// Put the popup aside while a worktree's diff takes its place.
    pub fn park(&mut self) {
        if let Some(popup) = self.popup.take() {
            self.parked = Some(popup);
        }
    }

    /// The popup back as it was put aside, if it was.
    pub fn unpark(&mut self) {
        if let Some(popup) = self.parked.take() {
            self.popup = Some(popup);
        }
    }

    /// The filter field, while the popup is open.
    pub fn field_mut(&mut self) -> Option<&mut TextField> {
        self.popup.as_mut().map(|p| &mut p.query)
    }
}

/// The popup's state: what is selected and scrolled to, by path, as the
/// list changes under it with every Git status read.
#[derive(Debug, Default)]
struct Popup {
    selected: Option<PathBuf>,
    /// Where the list stands, in worktrees: its first in view (by index
    /// into the list; its repository's label stays above it) and the part
    /// of that one scrolled past.
    scroll: Scroll,
    hover: Option<WorktreesHit>,
    armed: Option<Armed>,
    /// The main worktree of the repository listed first.
    focus: Option<PathBuf>,
    /// Where the selected worktree was in the list: what is selected once
    /// it is gone.
    selected_at: usize,
    /// What the list is filtered by.
    query: TextField,
    /// Every repository's worktrees, not just the focused one's.
    all: bool,
}

/// What a second press confirms.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Armed {
    /// Deleting the worktree at this path, with what it loses.
    Delete(PathBuf),
    /// Deleting the merged worktrees of the repository whose main worktree
    /// is here.
    CleanUp(PathBuf),
}

/// A worktree as the popup lists it.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    /// Its repository, by index into the names.
    repo: usize,
    /// The repository's main worktree, where Git runs for it.
    main: PathBuf,
    /// The git folder its worktrees share: whose turn (`git_ops::Turn`)
    /// a delete or checkout takes, so the repositories panel's fetches
    /// and batches don't run at once.
    common: PathBuf,
    /// What its branch merges into: the main worktree's branch, else its
    /// commit.
    target: String,
    /// The workspace its tabs open in.
    workspace: usize,
    wt: Worktree,
}

/// A line of the list: a repository's label, or a worktree by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Repo(usize),
    Item(usize),
}

/// What the popup asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Diff(PathBuf),
    /// Check the worktree's branch out in the main worktree.
    Checkout(PathBuf),
    Merge(PathBuf),
    Delete {
        path: PathBuf,
        force: bool,
    },
    /// The merged worktrees of the repository whose main worktree is here.
    CleanUp(PathBuf),
    Open(PathBuf),
    Copy(PathBuf),
}

/// What deleting worktrees came to: per worktree, its name and path, and
/// why it failed.
type Deleted = Vec<(String, PathBuf, Result<(), String>)>;

/// What a thread working on worktrees came to.
enum Done {
    Deleted(Deleted),
    /// Checking worktree `path`'s branch out in the main worktree, with
    /// what to say once it is, and how many untracked files came along
    /// with it.
    CheckedOut {
        path: PathBuf,
        said: String,
        result: Result<(), String>,
        brought: Result<usize, String>,
    },
}

/// The lines of `items`, a repository's label before its worktrees.
fn entries(items: &[Item]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (k, item) in items.iter().enumerate() {
        if k == 0 || items[k - 1].repo != item.repo {
            entries.push(Entry::Repo(item.repo));
        }
        entries.push(Entry::Item(k));
    }
    entries
}

/// How high `entry` is in the list.
fn height(entry: Entry) -> i32 {
    match entry {
        Entry::Repo(_) => chrome::WORKTREES_LABEL,
        Entry::Item(_) => chrome::WORKTREES_ROW,
    }
}

/// How high the list is with a view of `page` rows: all of it when it
/// fits, else the page, so it stays put as it scrolls.
fn list_height(items: &[Item], page: usize) -> i32 {
    let all = entries(items).into_iter().map(height).sum();
    room(page).min(all)
}

/// The room for a view of `page` rows.
fn room(page: usize) -> i32 {
    page.max(1) as i32 * chrome::WORKTREES_ROW
}

/// The entries in a view of `page` rows from worktree `first`: its
/// repository's label on top, whatever is scrolled, then what fits; never
/// a label with no worktree under it.
fn shown(items: &[Item], first: usize, page: usize) -> Vec<Entry> {
    let Some(top) = items.get(first) else {
        return Vec::new();
    };
    let room = room(page);
    let mut shown = vec![Entry::Repo(top.repo)];
    let mut h = chrome::WORKTREES_LABEL;
    for (k, item) in items.iter().enumerate().skip(first) {
        let label = k > first && items[k - 1].repo != item.repo;
        let need = chrome::WORKTREES_ROW + if label { chrome::WORKTREES_LABEL } else { 0 };
        if h + need > room && shown.len() > 1 {
            break;
        }
        if label {
            shown.push(Entry::Repo(item.repo));
        }
        shown.push(Entry::Item(k));
        h += need;
    }
    shown
}

/// The entries under a view of `page` rows showing `shown` of `items`
/// that show in part: those reaching into the room its last rows leave,
/// and as far as scrolling part of its first worktree brings up.
fn under(items: &[Item], shown: &[Entry], page: usize) -> Vec<Entry> {
    let (Some(&Entry::Item(first)), Some(&Entry::Item(last))) = (
        shown.iter().find(|e| matches!(e, Entry::Item(_))),
        shown.last(),
    ) else {
        return Vec::new();
    };
    let reach = room(page) + step(items, first);
    let mut h: i32 = shown.iter().copied().map(height).sum();
    let mut under = Vec::new();
    for (k, item) in items.iter().enumerate().skip(last + 1) {
        if h >= reach {
            break;
        }
        if item.repo != items[k - 1].repo {
            under.push(Entry::Repo(item.repo));
            h += chrome::WORKTREES_LABEL;
        }
        under.push(Entry::Item(k));
        h += chrome::WORKTREES_ROW;
    }
    under
}

/// How far scrolling past worktree `first` of `items` moves the list:
/// the next one comes up to its place, with its repository's label when it
/// has one.
fn step(items: &[Item], first: usize) -> i32 {
    match (items.get(first), items.get(first + 1)) {
        (Some(this), Some(next)) if next.repo != this.repo => {
            chrome::WORKTREES_ROW + chrome::WORKTREES_LABEL
        }
        _ => chrome::WORKTREES_ROW,
    }
}

/// How far down a list of `items` standing at `pos` (in worktrees, to the
/// fraction) is scrolled, in logical pixels: each worktree scrolled past
/// as far as it moves the list (see `step`).
fn scrolled_px(items: &[Item], pos: f64) -> f64 {
    let first = pos.max(0.0).floor() as usize;
    let past: i32 = (0..first.min(items.len())).map(|i| step(items, i)).sum();
    past as f64 + pos.max(0.0).fract() * step(items, first) as f64
}

/// Where a list of `items` scrolled `px` logical pixels down stands, in
/// worktrees (see `scrolled_px`).
fn standing_at(items: &[Item], px: f64) -> f64 {
    let mut down = 0.0;
    for i in 0..items.len() {
        let h = step(items, i) as f64;
        if px < down + h {
            return i as f64 + (px - down).max(0.0) / h;
        }
        down += h;
    }
    items.len() as f64
}

/// The furthest worktree a view of `page` rows scrolls to: the first from
/// which the rest all fit.
fn last_first(items: &[Item], page: usize) -> usize {
    let Some(last) = items.len().checked_sub(1) else {
        return 0;
    };
    (0..items.len())
        .find(|&f| shown(items, f, page).last() == Some(&Entry::Item(last)))
        .unwrap_or(last)
}

/// What can be done to `wt`, as its row's buttons.
fn buttons(wt: &Worktree) -> Vec<WorktreeButton> {
    let mut buttons = Vec::new();
    if !wt.gone {
        buttons.push(WorktreeButton::Diff);
    }
    if can_checkout(wt) {
        buttons.push(WorktreeButton::Checkout);
    }
    if can_merge(wt) {
        buttons.push(WorktreeButton::Merge);
    }
    if !wt.main {
        buttons.push(WorktreeButton::Delete);
    }
    buttons
}

fn can_merge(wt: &Worktree) -> bool {
    can_checkout(wt) && !wt.merged && !wt.fresh
}

fn can_checkout(wt: &Worktree) -> bool {
    !wt.main && !wt.gone && wt.branch.is_some()
}

/// Deleting `wt` loses nothing: it is clean and merged or without a
/// commit of its own, or its folder is gone.
fn safe_to_delete(wt: &Worktree) -> bool {
    done_with(wt) || (wt.fresh && !wt.dirty)
}

/// `wt` is done with, for a clean-up: merged and clean, or its folder
/// gone. Not a new one with no commit yet: work may be about to start in
/// it.
fn done_with(wt: &Worktree) -> bool {
    wt.gone || (wt.merged && !wt.dirty)
}

/// What deleting `wt` loses, for the second press to confirm.
fn loses(wt: &Worktree) -> String {
    let mut what = Vec::new();
    if wt.dirty {
        what.push("its changes");
    }
    if !wt.merged && !wt.fresh {
        what.push("unmerged commits");
    }
    format!("Delete again to delete, with {}", what.join(" and "))
}

/// The merged worktrees of the repository whose main worktree is at
/// `main`, which a clean-up deletes.
fn merged_ones<'a>(items: &'a [Item], main: &Path) -> Vec<&'a Item> {
    items
        .iter()
        .filter(|i| i.main == main && !i.wt.main && done_with(&i.wt))
        .collect()
}

/// What the popup lists of `items` (with their repositories' `names`):
/// only the worktrees of the repository whose main worktree is `focus`
/// unless `all`, and those `query` matches by name, path or repository,
/// the best matches first within each repository.
fn filter(
    items: Vec<Item>,
    names: &[String],
    focus: Option<&Path>,
    all: bool,
    query: &str,
) -> Vec<Item> {
    let mut kept: Vec<(Reverse<i32>, Item)> = items
        .into_iter()
        .filter(|i| all || focus.is_none_or(|f| i.main == f))
        .filter_map(|i| {
            let path = shown_path(&i.wt.path, &i.main);
            let text = format!("{} {path} {}", i.wt.name(), names[i.repo]);
            let (score, _) = fuzzy::score(query, &text)?;
            Some((Reverse(score), i))
        })
        .collect();
    kept.sort_by_key(|(score, i)| (i.repo, *score));
    kept.into_iter().map(|(_, i)| i).collect()
}

/// `text` quoted for a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

impl Popup {
    /// Stop the list flung; whether it was.
    pub fn stop_fling(&mut self) -> bool {
        self.scroll.stop_fling()
    }

    /// The selected worktree, by index into `items`.
    fn selected(&self, items: &[Item]) -> Option<usize> {
        let path = self.selected.as_ref()?;
        items.iter().position(|i| &i.wt.path == path)
    }

    /// Select worktree `k` of `items`, scrolling it into a view of `page`
    /// entries.
    fn select(&mut self, items: &[Item], k: usize, page: usize) {
        let Some(item) = items.get(k) else { return };
        self.selected = Some(item.wt.path.clone());
        self.selected_at = k;
        let mut first = self.first_shown(items, page);
        // The first worktree, partly scrolled past, is shown whole.
        if k <= first {
            self.scroll.jump(k as f64);
            return;
        }
        while !shown(items, first, page).contains(&Entry::Item(k)) {
            first += 1;
        }
        self.scroll.jump_row(first);
    }

    /// With the selected worktree gone from `items` (deleted), select the
    /// one that took its place.
    fn keep_selection(&mut self, items: &[Item], page: usize) {
        if self.selected(items).is_none() && !items.is_empty() {
            self.select(items, self.selected_at.min(items.len() - 1), page);
        }
    }

    /// The filter changed, and with it the list, now `items`: the best
    /// match selected while there is a query, else the selection kept when
    /// still listed.
    fn refilter(&mut self, items: &[Item], page: usize) {
        self.armed = None;
        self.scroll.jump(0.0);
        let kept = self.selected(items).filter(|_| self.query.is_empty());
        if let Some(k) = kept.or((!items.is_empty()).then_some(0)) {
            self.select(items, k, page);
        }
    }

    /// The list as it stands for `motion`s in a view of `page` rows of
    /// `items`: in worktrees, a notch one, as high as from its first to
    /// the next.
    fn bounds(&self, items: &[Item], page: usize) -> Bounds {
        let first = self.first_shown(items, page);
        let unit = step(items, first) as f64;
        Bounds {
            max: last_first(items, page) as f64,
            notch: 1.0,
            unit,
            next: step(items, first + 1) as f64,
            prev: first.checked_sub(1).map_or(unit, |i| step(items, i) as f64),
            view: list_height(items, page) as f64,
        }
    }

    /// The wheel or the touchpad moved the list of `items` at `now`.
    fn scroll_by(&mut self, motion: Motion, now: Instant, items: &[Item], page: usize) {
        let bounds = self.bounds(items, page);
        self.scroll.apply(motion, now, bounds);
        self.calm(now);
    }

    /// Stand the list of `items` `px` pixels down, as its scrollbar's
    /// thumb held there puts it.
    fn stand_at(&mut self, px: f64, items: &[Item], page: usize) {
        let max = last_first(items, page) as f64;
        self.scroll.jump(standing_at(items, px).clamp(0.0, max));
    }

    /// Move the list on to `now`; whether it still moves.
    fn tick(&mut self, now: Instant, items: &[Item], page: usize) -> bool {
        let bounds = self.bounds(items, page);
        let moving = self.scroll.tick(now, bounds);
        self.calm(now);
        moving
    }

    /// Nothing is under the pointer on the entries while they move under
    /// it: their highlight would flicker.
    fn calm(&mut self, now: Instant) {
        if self.scroll.moving(now)
            && matches!(
                self.hover,
                Some(WorktreesHit::Entry(_) | WorktreesHit::Button(..))
            )
        {
            self.hover = None;
        }
    }

    /// The first worktree in a view of `page` rows, as scrolled.
    fn first_shown(&self, items: &[Item], page: usize) -> usize {
        self.scroll.first().min(last_first(items, page))
    }

    /// The entries a view of `page` rows shows, then those under it shown
    /// in part, with the part of its first worktree scrolled past.
    /// Selection and keys keep to the view's own entries.
    fn in_view(&self, items: &[Item], page: usize) -> (Vec<Entry>, f64) {
        let first = self.first_shown(items, page);
        let mut shown = shown(items, first, page);
        // None once the list shows its end.
        let part = if first < last_first(items, page) {
            self.scroll.part()
        } else {
            0.0
        };
        let under = under(items, &shown, page);
        shown.extend(under);
        (shown, part)
    }

    /// A key for the filter, if it takes it: what it did to the filter
    /// (`Edit::Changed`: the list is to be filtered again). It edits the
    /// query; Tab flips the scope; Esc clears the query.
    fn filter_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<Edit> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        match self.query.key(mods, sym, typed) {
            Edit::Ignored => {}
            edit => {
                self.armed = None;
                return Some(edit);
            }
        }
        match sym {
            Keysym::Tab | Keysym::ISO_Left_Tab if plain && self.focus.is_some() => {
                self.all = !self.all;
            }
            Keysym::Escape if !self.query.is_empty() => self.query.clear(),
            _ => return None,
        }
        Some(Edit::Changed)
    }

    /// A key, over `items` in a view of `page` entries; what to do, if
    /// anything. Any key but the one confirming it takes back what was
    /// armed.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        items: &[Item],
        page: usize,
    ) -> Option<Outcome> {
        let armed = self.armed.take();
        if mods.alt || mods.logo {
            return None;
        }
        let ctrl = mods.ctrl;
        self.keep_selection(items, page);
        let k = self.selected(items);
        let item = k.map(|k| &items[k]);
        let step = |by: isize| {
            let n = items.len() as isize;
            match k {
                _ if n == 0 => None,
                Some(k) => Some((k as isize + by).clamp(0, n - 1) as usize),
                None => Some(0),
            }
        };
        let path =
            |when: fn(&Worktree) -> bool| item.filter(|i| when(&i.wt)).map(|i| i.wt.path.clone());
        match sym {
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::Down | Keysym::KP_Down => {
                let k = step(1)?;
                self.select(items, k, page);
            }
            Keysym::Up | Keysym::KP_Up => {
                let k = step(-1)?;
                self.select(items, k, page);
            }
            Keysym::Page_Down | Keysym::KP_Page_Down => {
                let k = step(page.max(1) as isize)?;
                self.select(items, k, page);
            }
            Keysym::Page_Up | Keysym::KP_Page_Up => {
                let k = step(-(page.max(1) as isize))?;
                self.select(items, k, page);
            }
            Keysym::Home | Keysym::KP_Home if !items.is_empty() => self.select(items, 0, page),
            Keysym::End | Keysym::KP_End if !items.is_empty() => {
                self.select(items, items.len() - 1, page);
            }
            Keysym::Return | Keysym::KP_Enter => {
                return path(|wt| !wt.gone).map(Outcome::Diff);
            }
            Keysym::d | Keysym::D if ctrl => return path(|wt| !wt.gone).map(Outcome::Diff),
            Keysym::k | Keysym::K if ctrl => return path(can_checkout).map(Outcome::Checkout),
            Keysym::m | Keysym::M if ctrl => return path(can_merge).map(Outcome::Merge),
            Keysym::o | Keysym::O if ctrl => return path(|wt| !wt.gone).map(Outcome::Open),
            Keysym::c | Keysym::C if ctrl => return path(|_| true).map(Outcome::Copy),
            Keysym::Delete | Keysym::KP_Delete if !ctrl && !mods.shift => {
                return self.delete(item?, armed);
            }
            Keysym::Delete | Keysym::KP_Delete if !ctrl => {
                let main = item?.main.clone();
                if merged_ones(items, &main).is_empty() {
                    return None;
                }
                if armed == Some(Armed::CleanUp(main.clone())) {
                    return Some(Outcome::CleanUp(main));
                }
                self.armed = Some(Armed::CleanUp(main));
            }
            _ => {}
        }
        None
    }

    /// Delete `item`: at once when that loses nothing, else once asked
    /// twice (`armed` says what the press before asked).
    fn delete(&mut self, item: &Item, armed: Option<Armed>) -> Option<Outcome> {
        if item.wt.main {
            return None;
        }
        let path = item.wt.path.clone();
        if safe_to_delete(&item.wt) {
            return Some(Outcome::Delete { path, force: false });
        }
        if armed == Some(Armed::Delete(path.clone())) {
            return Some(Outcome::Delete { path, force: true });
        }
        self.armed = Some(Armed::Delete(path));
        None
    }

    /// A click on `hit`, over `items` in a view of `page` entries.
    fn click(&mut self, hit: WorktreesHit, items: &[Item], page: usize) -> Option<Outcome> {
        let (shown, _) = self.in_view(items, page);
        let at = |i: usize| match shown.get(i) {
            Some(Entry::Item(k)) => Some(*k),
            _ => None,
        };
        match hit {
            WorktreesHit::Panel
            | WorktreesHit::Query
            | WorktreesHit::Scope(_)
            | WorktreesHit::Hint(_) => {
                self.armed = None;
                None
            }
            WorktreesHit::Entry(i) => {
                let k = at(i)?;
                let keep = self.selected(items) == Some(k);
                if !keep {
                    self.armed = None;
                }
                self.select(items, k, page);
                None
            }
            WorktreesHit::Button(i, button) => {
                let k = at(i)?;
                self.select(items, k, page);
                let ctrl = ModifiersState {
                    ctrl: true,
                    ..ModifiersState::default()
                };
                let (mods, sym) = match button {
                    WorktreeButton::Diff => (ModifiersState::default(), Keysym::Return),
                    WorktreeButton::Checkout => (ctrl, Keysym::k),
                    WorktreeButton::Merge => (ctrl, Keysym::m),
                    WorktreeButton::Delete => (ModifiersState::default(), Keysym::Delete),
                };
                self.key(&mods, sym, items, page)
            }
        }
    }

    /// What it shows of `items` in a view of `page` entries, with the
    /// repositories' `names`, the worktrees being worked on `busy`.
    fn view(
        &self,
        items: &[Item],
        names: &[String],
        busy: &HashMap<PathBuf, &'static str>,
        page: usize,
    ) -> WorktreesView {
        let (shown, part) = self.in_view(items, page);
        let selected = self.selected(items);
        let entries = shown
            .iter()
            .map(|entry| match *entry {
                Entry::Repo(r) => WorktreeEntry::Repo(names[r].clone()),
                Entry::Item(k) => {
                    let item = &items[k];
                    let wt = &item.wt;
                    let state = if let Some(doing) = busy.get(&wt.path) {
                        LineState::Busy((*doing).into())
                    } else if self.armed == Some(Armed::Delete(wt.path.clone())) {
                        LineState::Armed(loses(wt))
                    } else if self.armed == Some(Armed::CleanUp(item.main.clone()))
                        && !wt.main
                        && done_with(wt)
                    {
                        LineState::Armed("Shift+Delete again to delete the merged ones".into())
                    } else if wt.needs_attention() {
                        LineState::Attention
                    } else {
                        LineState::Settled
                    };
                    WorktreeEntry::Worktree(WorktreeLine {
                        name: wt.name(),
                        path: shown_path(&wt.path, &item.main),
                        dirty: wt.dirty,
                        tag: wt.tag().into(),
                        state,
                        buttons: buttons(wt),
                    })
                }
            })
            .collect();
        let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
        let mut hints = vec![
            hint("Enter", "diff"),
            hint("Ctrl+K", "check out in main"),
            hint("Ctrl+M", "squash merge"),
            hint("Del", "delete"),
            hint("Shift+Del", "delete merged"),
            hint("Ctrl+O", "open in a tab"),
            hint("Ctrl+C", "copy path"),
        ];
        if self.focus.is_some() {
            let scope = if self.all {
                "this workspace"
            } else {
                "all workspaces"
            };
            hints.push(hint("Tab", scope));
        }
        hints.push(hint(
            "Esc",
            if self.query.is_empty() {
                "close"
            } else {
                "clear"
            },
        ));
        let empty = if !self.query.is_empty() {
            "No worktrees match"
        } else {
            "No worktrees besides the main ones"
        };
        WorktreesView {
            entries,
            list_height: list_height(items, page),
            selected: shown
                .iter()
                .position(|e| selected.is_some_and(|k| *e == Entry::Item(k))),
            hover: self.hover,
            hints,
            query: self.query.line(),
            all: self.all,
            repo: self.focus.as_ref().map(|f| {
                f.file_name()
                    .map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into())
            }),
            empty: empty.into(),
            part,
            overscroll: self.scroll.overscroll(),
            bar: {
                let max = last_first(items, page);
                (max > 0).then(|| {
                    let down = scrolled_px(items, self.scroll.pos().clamp(0.0, max as f64));
                    let total = scrolled_px(items, max as f64) + list_height(items, page) as f64;
                    chrome::ListBar { down, total }
                })
            },
        }
    }
}

/// Where worktree `path` is, from its repository's main worktree at `main`
/// (the main one by its own place).
fn shown_path(path: &Path, main: &Path) -> String {
    match path.strip_prefix(main) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => paths::display(path, crate::home_dir().as_deref(), 60),
    }
}

impl Gerdr {
    /// The worktrees of the repositories the workspaces are in, the main
    /// worktree first, then those needing a look; the repository of
    /// `focus` (a main worktree) first. With the repositories' names. Their
    /// tabs open in the active workspace, else the repository's first.
    fn worktree_items(&self, focus: Option<&Path>) -> (Vec<String>, Vec<Item>) {
        let mut repos: Vec<(PathBuf, PathBuf, usize, Vec<Worktree>)> = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            let Some(status) = self.git.get(&ws.dir) else {
                continue;
            };
            let Some(main) = status.main_path() else {
                continue;
            };
            if let Some(repo) = repos.iter_mut().find(|(m, ..)| m == main) {
                // The active workspace is where its tabs open, when it is in
                // the repository.
                if w == self.active_workspace {
                    repo.2 = w;
                }
                continue;
            }
            let mut all: Vec<Worktree> = status.all_worktrees().into_iter().cloned().collect();
            all.sort_by_key(|wt| (!wt.main, !wt.needs_attention()));
            let common = status.common_dir.clone().unwrap_or_else(|| main.to_owned());
            repos.push((main.to_path_buf(), common, w, all));
        }
        repos.sort_by_key(|(main, ..)| Some(main.as_path()) != focus);
        let mut names = Vec::new();
        let mut items = Vec::new();
        for (repo, (main, common, workspace, all)) in repos.into_iter().enumerate() {
            names.push(main.file_name().map_or_else(
                || main.display().to_string(),
                |n| n.to_string_lossy().into(),
            ));
            let target = all
                .iter()
                .find(|wt| wt.main)
                .and_then(|wt| wt.branch.clone().or_else(|| wt.head.clone()))
                .unwrap_or_else(|| "HEAD".into());
            for wt in all {
                items.push(Item {
                    repo,
                    main: main.clone(),
                    common: common.clone(),
                    target: target.clone(),
                    workspace,
                    wt,
                });
            }
        }
        (names, items)
    }

    /// What the open popup lists, as filtered; with no popup, every
    /// worktree.
    fn popup_items(&self) -> (Vec<String>, Vec<Item>) {
        let Some(popup) = self.worktrees.popup.as_ref() else {
            return self.worktree_items(None);
        };
        let focus = popup.focus.as_deref();
        let (names, items) = self.worktree_items(focus);
        let items = filter(items, &names, focus, popup.all, &popup.query);
        (names, items)
    }

    /// The popup's filter changed: list what it keeps.
    fn worktrees_refilter(&mut self) {
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        if let Some(popup) = self.worktrees.popup.as_mut() {
            popup.refilter(&items, page);
        }
    }

    /// Show every repository's worktrees when `all`, else only the
    /// focused one's.
    pub fn worktrees_scope(&mut self, all: bool) {
        let Some(popup) = self.worktrees.popup.as_mut() else {
            return;
        };
        if popup.focus.is_none() || popup.all == all {
            return;
        }
        popup.all = all;
        self.worktrees_refilter();
    }

    fn worktrees_page(&self) -> usize {
        chrome::worktrees_page(self.output_size())
    }

    /// Open the worktrees popup, the repository of the workspace at `dir`
    /// (else the active one's) first, with worktree `dir` selected when it
    /// is one.
    pub fn open_worktrees(&mut self, dir: Option<PathBuf>) {
        self.switcher = None;
        self.repos.close();
        self.inbox.close();
        self.scrollback = None;
        self.project_menu = None;
        self.stop_key_repeat();
        self.close_menu();
        // The count's list would linger beside it, and go stale.
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let ws_dir = dir
            .clone()
            .or_else(|| self.workspace().map(|ws| ws.dir.clone()));
        let status = ws_dir.as_ref().and_then(|d| {
            self.git.get(d).or_else(|| {
                self.git
                    .values()
                    .find(|g| g.worktrees.iter().any(|w| &w.path == d))
            })
        });
        let focus = status.and_then(|s| s.main_path()).map(Path::to_path_buf);
        let mut popup = Popup {
            focus: focus.clone(),
            all: focus.is_none(),
            ..Popup::default()
        };
        let (names, items) = self.worktree_items(focus.as_deref());
        let items = filter(items, &names, focus.as_deref(), popup.all, "");
        let page = self.worktrees_page();
        // The worktree asked for, else the first needing a look.
        let pick = items
            .iter()
            .position(|i| Some(&i.wt.path) == dir.as_ref() && !i.wt.main)
            .or_else(|| {
                items
                    .iter()
                    .position(|i| !i.wt.main && i.wt.needs_attention())
            })
            .or_else(|| items.iter().position(|i| !i.wt.main))
            .or((!items.is_empty()).then_some(0));
        if let Some(k) = pick {
            popup.select(&items, k, page);
        }
        self.worktrees.popup = Some(popup);
        self.refresh_git();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// A key while the worktrees popup has the keyboard; what to do once
    /// the keyboard is free again, if anything.
    /// `sym` is the key's own symbol, `typed` the one with Shift and the
    /// layout applied. The filter takes the keys that edit it first; Tab
    /// shows every repository's worktrees, or the focused one's again.
    pub fn worktrees_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        match self.worktrees.popup.as_mut()?.filter_key(mods, sym, typed) {
            Some(Edit::Changed) => {
                self.worktrees_refilter();
                return None;
            }
            Some(_) => return None,
            None => {}
        }
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        self.worktrees.popup.as_mut()?.key(mods, sym, &items, page)
    }

    /// A left click while the worktrees popup is open: outside it closes it.
    pub fn worktrees_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.worktrees_view() else {
            return;
        };
        let layout = chrome::worktrees_layout(&view, self.output_size());
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        let outcome = match chrome::worktrees_hit(&view, &layout, pos) {
            Some(WorktreesHit::Scope(all)) => {
                self.worktrees_scope(all);
                None
            }
            // A key hint: its key, pressed.
            Some(WorktreesHit::Hint(i)) => {
                return self.press_hint(Overlay::Worktrees, &view.hints[i].0, pos);
            }
            Some(hit) => match self.worktrees.popup.as_mut() {
                Some(popup) => popup.click(hit, &items, page),
                None => None,
            },
            None => Some(Outcome::Close),
        };
        if let Some(outcome) = outcome {
            self.apply_worktrees(outcome);
        }
    }

    /// The pointer moved while the worktrees popup is open.
    pub fn worktrees_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.worktrees_view()?;
        let layout = chrome::worktrees_layout(&view, self.output_size());
        let mut hit = chrome::worktrees_hit(&view, &layout, pos);
        if let Some(popup) = self.worktrees.popup.as_mut() {
            popup.hover = hit;
            popup.calm(Instant::now());
            hit = popup.hover;
        }
        // A row's marks say what they mean.
        let Some(WorktreesHit::Entry(i)) = hit else {
            return None;
        };
        let WorktreeEntry::Worktree(line) = view.entries.get(i)? else {
            return None;
        };
        let (rect, buttons) = layout.entries.get(i)?;
        chrome::worktree_marks(line, *rect, buttons)
            .into_iter()
            .find(|(r, _)| r.to_f64().contains(pos))
            .map(|(r, text)| Tooltip::new(text, r))
    }

    /// The wheel or the touchpad moved over the popup.
    pub fn worktrees_scroll(&mut self, motion: Motion) {
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        if let Some(popup) = self.worktrees.popup.as_mut() {
            popup.scroll_by(motion, Instant::now(), &items, page);
        }
    }

    /// Stand the popup's list `px` pixels down, as its scrollbar's thumb
    /// held there puts it.
    pub fn worktrees_scroll_to(&mut self, px: f64) {
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        if let Some(popup) = self.worktrees.popup.as_mut() {
            popup.stand_at(px, &items, page);
        }
    }

    /// Move the worktrees popup's list on to `now` if it moves on its own;
    /// whether it still moves (see `Scroll::moving`).
    pub fn worktrees_scroll_tick(&mut self, now: Instant) -> bool {
        // At rest, it is left to stand where it is.
        match &self.worktrees.popup {
            Some(popup) if popup.scroll.moving(now) || popup.scroll.overscroll() != 0.0 => {}
            _ => return false,
        }
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        self.worktrees
            .popup
            .as_mut()
            .is_some_and(|popup| popup.tick(now, &items, page))
    }

    /// What the worktrees popup shows, while it is open.
    pub fn worktrees_view(&self) -> Option<WorktreesView> {
        let popup = self.worktrees.popup.as_ref()?;
        let (names, items) = self.popup_items();
        Some(popup.view(&items, &names, &self.worktrees.busy, self.worktrees_page()))
    }

    /// Do what the worktrees popup asked.
    pub fn apply_worktrees(&mut self, outcome: Outcome) {
        let (_, items) = self.popup_items();
        let find = |path: &Path| items.iter().find(|i| i.wt.path == path).cloned();
        match outcome {
            Outcome::Close => self.close_worktrees(),
            Outcome::Diff(path) => {
                let Some(item) = find(&path) else { return };
                let w = self.worktree_workspace(&item);
                let (names, _) = self.popup_items();
                let label = names.get(item.repo).cloned().unwrap_or_default();
                let target = (!item.wt.main).then(|| item.target.clone());
                let home = self.worktree_home(&item);
                let at = (w, home);
                self.open_worktree_diff(item.wt.path, label, item.wt.branch, target, at);
            }
            Outcome::Checkout(path) => {
                let Some(item) = find(&path) else { return };
                let main = self
                    .worktree_items(None)
                    .1
                    .into_iter()
                    .find(|i| i.wt.path == item.main);
                let Some(main) = main else { return };
                if let Err(why) = git::checkout_check(&main.wt, &item.wt) {
                    return self.toast(why, Level::Warning);
                }
                let files = git::untracked(&item.wt.path);
                if files.is_empty() {
                    self.checkout_worktree(item, main.wt.name(), Vec::new());
                } else {
                    self.ask_checkout(item.wt, &item.main, main.wt.name(), files);
                }
            }
            Outcome::Merge(path) => {
                let Some(item) = find(&path) else { return };
                if let Err(why) = git::squash_check(&item.main, &item.wt) {
                    return self.toast(why, Level::Warning);
                }
                let Some(branch) = item.wt.branch.clone() else {
                    return;
                };
                self.close_worktrees();
                self.worktrees.merging.insert(item.wt.path.clone());
                let command = format!("git merge --squash {} && git commit", quote(&branch));
                self.new_tab_running(item.workspace, &item.main, command);
            }
            Outcome::Delete { path, force } => {
                if let Some(item) = find(&path) {
                    self.delete_worktrees(vec![item], force);
                }
            }
            Outcome::CleanUp(main) => {
                let merged = merged_ones(&items, &main).into_iter().cloned().collect();
                self.delete_worktrees(merged, false);
            }
            Outcome::Open(path) => {
                let Some(item) = find(&path) else { return };
                self.close_worktrees();
                let w = self.worktree_workspace(&item);
                self.new_project_tab(w, &item.wt.path);
            }
            Outcome::Copy(path) => {
                self.copy_text(path.display().to_string());
                self.toast("Copied the path".into(), Level::Info);
            }
        }
    }

    fn close_worktrees(&mut self) {
        self.worktrees.popup = None;
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
    }

    /// The toast's "Delete worktree", after a squash: delete the worktree
    /// at `path` if that still loses nothing.
    pub fn delete_merged_worktree(&mut self, path: &Path) {
        let (_, items) = self.worktree_items(None);
        if let Some(item) = items.into_iter().find(|i| i.wt.path == path) {
            if safe_to_delete(&item.wt) {
                self.delete_worktrees(vec![item], false);
            } else {
                self.toast(
                    format!("{} has changes now", item.wt.name()),
                    Level::Warning,
                );
            }
        }
    }

    /// The workspace a tab in worktree `item` opens in: one in the worktree
    /// itself, else the repository's (see `worktree_items`).
    fn worktree_workspace(&self, item: &Item) -> usize {
        let own = |ws: &crate::Workspace| !item.wt.main && ws.dir.starts_with(&item.wt.path);
        if self.workspace().is_some_and(own) {
            return self.active_workspace;
        }
        self.workspaces
            .iter()
            .position(own)
            .unwrap_or(item.workspace)
    }

    /// The workspace whose checkout worktree `item` is (the worktree its
    /// folder is in), the active one first.
    fn worktree_home(&self, item: &Item) -> Option<usize> {
        let own = |ws: &crate::Workspace| {
            let here = self.git.get(&ws.dir).and_then(|s| s.here.as_ref());
            here.is_some_and(|here| here.path == item.wt.path)
        };
        if self.workspace().is_some_and(own) {
            return Some(self.active_workspace);
        }
        self.workspaces.iter().position(own)
    }

    /// Check worktree `path`'s branch out in its repository's main
    /// worktree, called `main_name`, bringing untracked `files` of it
    /// along, as the checkout dialog asked.
    pub fn checkout_worktree_at(&mut self, path: &Path, main_name: String, files: Vec<Untracked>) {
        let (_, items) = self.worktree_items(None);
        if let Some(item) = items.into_iter().find(|i| i.wt.path == path) {
            self.checkout_worktree(item, main_name, files);
        }
    }

    /// Check worktree `item`'s branch out in its repository's main worktree,
    /// called `main_name`, off the event loop, then move its untracked
    /// `files` there.
    fn checkout_worktree(&mut self, item: Item, main_name: String, files: Vec<Untracked>) {
        if self.worktrees.busy.contains_key(&item.wt.path) {
            return;
        }
        let Some(tx) = self.done_sender() else {
            return;
        };
        let Some(branch) = item.wt.branch.clone() else {
            return;
        };
        // Whoever is in the worktree stays on its commit; their next
        // commits miss the branch.
        let said = match self.in_use(&item.wt) {
            Some(user) => format!(
                "Checked out {branch} in {main_name}; {user} in its worktree is detached now"
            ),
            None => format!("Checked out {branch} in {main_name}"),
        };
        self.worktrees
            .busy
            .insert(item.wt.path.clone(), "checking out…");
        std::thread::spawn(move || {
            let (result, brought) = {
                let _turn = Turn::take(&item.common, "a worktree checkout");
                let result = git::checkout_in_main(&item.main, &item.wt);
                let brought = match result {
                    Ok(()) => git::bring_untracked(&item.wt.path, &item.main, &files),
                    Err(_) => Ok(0),
                };
                (result, brought)
            };
            let _ = tx.send(Done::CheckedOut {
                path: item.wt.path,
                said,
                result,
                brought,
            });
        });
    }

    /// The pane or workspace still in worktree `wt`, by its name, if any:
    /// an agent may be working there.
    fn in_use(&self, wt: &Worktree) -> Option<String> {
        if let Some(ws) = self
            .workspaces
            .iter()
            .find(|ws| ws.dir.starts_with(&wt.path))
        {
            return Some(format!("workspace {}", ws.name));
        }
        let mut panes = self.herdr_tabs.values().flat_map(|t| &t.panes);
        panes
            .find(|pane| {
                let dir = pane.foreground_cwd.as_ref().or(pane.cwd.as_ref());
                dir.is_some_and(|d| d.starts_with(&wt.path))
            })
            .map(|_| "a terminal".to_owned())
    }

    /// Delete `items`' worktrees, and their branches, off the event loop:
    /// losing their changes and commits when `force`. Those a pane is in
    /// are kept.
    fn delete_worktrees(&mut self, items: Vec<Item>, force: bool) {
        let mut doomed = Vec::new();
        for item in items {
            if self.worktrees.busy.contains_key(&item.wt.path) {
                continue;
            }
            if let Some(user) = self.in_use(&item.wt).filter(|_| !item.wt.gone) {
                let name = item.wt.name();
                self.toast(format!("Kept {name}: {user} is in it"), Level::Warning);
                continue;
            }
            doomed.push(item);
        }
        if doomed.is_empty() {
            return;
        }
        let Some(tx) = self.done_sender() else {
            return;
        };
        for item in &doomed {
            self.worktrees
                .busy
                .insert(item.wt.path.clone(), "deleting…");
        }
        std::thread::spawn(move || {
            let deleted = doomed
                .into_iter()
                .map(|item| {
                    let result = {
                        let _turn = Turn::take(&item.common, "a worktree delete");
                        git::remove_worktree(&item.main, &item.wt, force)
                    };
                    (item.wt.name(), item.wt.path, result)
                })
                .collect();
            let _ = tx.send(Done::Deleted(deleted));
        });
    }

    /// Where the deletes' and checkouts' threads send what they came to;
    /// set up the first time.
    fn done_sender(&mut self) -> Option<channel::Sender<Done>> {
        if self.worktrees.done.is_none() {
            let (tx, rx) = channel::channel::<Done>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg(Done::Deleted(deleted)) => state.worktrees_deleted(deleted),
                    channel::Event::Msg(Done::CheckedOut {
                        path,
                        said,
                        result,
                        brought,
                    }) => state.worktree_checked_out(&path, said, result, brought),
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.worktrees.done = Some(tx),
                Err(err) => {
                    tracing::warn!("working on worktrees: {err}");
                    return None;
                }
            }
        }
        self.worktrees.done.clone()
    }

    /// A worktree's branch was checked out in the main worktree, or failed
    /// to be.
    fn worktree_checked_out(
        &mut self,
        path: &Path,
        said: String,
        result: Result<(), String>,
        brought: Result<usize, String>,
    ) {
        self.worktrees.busy.remove(path);
        match (result, brought) {
            (Err(why), _) => self.toast(format!("Couldn't check it out: {why}"), Level::Error),
            (Ok(()), Ok(0)) => self.toast(said, Level::Success),
            (Ok(()), Ok(n)) => {
                let files = if n == 1 { "file" } else { "files" };
                self.toast(
                    format!("{said}; brought {n} untracked {files} along"),
                    Level::Success,
                );
            }
            (Ok(()), Err(why)) => self.toast(
                format!("{said}, but not all its untracked files: {why}"),
                Level::Warning,
            ),
        }
        self.refresh_git();
    }

    /// Worktrees were deleted, or failed to be.
    fn worktrees_deleted(&mut self, deleted: Deleted) {
        let (mut gone, mut paths) = (Vec::new(), Vec::new());
        for (name, path, result) in deleted {
            self.worktrees.busy.remove(&path);
            match result {
                Ok(()) => {
                    gone.push(name);
                    paths.push(path);
                }
                Err(why) => self.toast(format!("Couldn't delete {name}: {why}"), Level::Error),
            }
        }
        match gone.as_slice() {
            [] => {}
            [name] => self.toast(format!("Deleted {name}"), Level::Success),
            names => self.toast(format!("Deleted {} worktrees", names.len()), Level::Success),
        }
        // Their review comments go with them.
        self.forget_worktree_reviews(&paths);
        self.refresh_git();
    }

    /// A Git status read came in: a worktree squashed in a tab that reads
    /// as merged now is offered for deleting.
    pub fn worktrees_read(&mut self) {
        let (_, items) = self.popup_items();
        let page = self.worktrees_page();
        if let Some(popup) = self.worktrees.popup.as_mut() {
            popup.keep_selection(&items, page);
        }
        // All of them: the filter can't hide a squash coming through.
        let (_, items) = self.worktree_items(None);
        let merging = std::mem::take(&mut self.worktrees.merging);
        for path in merging {
            match items.iter().find(|i| i.wt.path == path) {
                Some(item) if item.wt.merged => {
                    let name = item.wt.name();
                    let delete = MenuAction::DeleteWorktree(path);
                    self.toast_with(
                        format!("Squashed {name} in"),
                        Level::Success,
                        Some(("Delete worktree".into(), delete)),
                    );
                }
                Some(_) => {
                    self.worktrees.merging.insert(path);
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(repo: usize, name: &str, main: bool, merged: bool, dirty: bool) -> Item {
        Item {
            repo,
            main: "/src/r".into(),
            common: "/src/r/.git".into(),
            target: "main".into(),
            workspace: 0,
            wt: Worktree {
                path: if main {
                    "/src/r".into()
                } else {
                    format!("/src/r/.wt/{name}").into()
                },
                branch: Some(name.into()),
                main,
                merged,
                dirty,
                ..Worktree::default()
            },
        }
    }

    fn items() -> Vec<Item> {
        vec![
            item(0, "main", true, false, false),
            item(0, "wip", false, false, true),
            item(0, "done", false, true, false),
        ]
    }

    fn press(popup: &mut Popup, sym: Keysym, items: &[Item]) -> Option<Outcome> {
        popup.key(&ModifiersState::default(), sym, items, 10)
    }

    fn ctrl() -> ModifiersState {
        ModifiersState {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_delete_losing_work_is_asked_twice() {
        let items = items();
        let mut popup = Popup::default();
        popup.select(&items, 1, 10);
        assert_eq!(press(&mut popup, Keysym::Delete, &items), None);
        assert_eq!(popup.armed, Some(Armed::Delete("/src/r/.wt/wip".into())));
        assert_eq!(
            press(&mut popup, Keysym::Delete, &items),
            Some(Outcome::Delete {
                path: "/src/r/.wt/wip".into(),
                force: true
            })
        );
        // Any other key takes it back.
        press(&mut popup, Keysym::Delete, &items);
        press(&mut popup, Keysym::Down, &items);
        assert_eq!(popup.armed, None);
        // A merged, clean one goes at once.
        assert_eq!(
            press(&mut popup, Keysym::Delete, &items),
            Some(Outcome::Delete {
                path: "/src/r/.wt/done".into(),
                force: false
            })
        );
        // The main one stays.
        popup.select(&items, 0, 10);
        assert_eq!(press(&mut popup, Keysym::Delete, &items), None);
        assert_eq!(popup.armed, None);
    }

    #[test]
    fn a_new_worktree_goes_at_once_but_not_in_a_clean_up() {
        let mut items = items();
        let mut new = item(0, "new", false, false, false);
        new.wt.fresh = true;
        items.push(new);
        let mut popup = Popup::default();
        popup.select(&items, 3, 10);
        assert_eq!(
            press(&mut popup, Keysym::Delete, &items),
            Some(Outcome::Delete {
                path: "/src/r/.wt/new".into(),
                force: false
            })
        );
        let cleaned: Vec<_> = merged_ones(&items, Path::new("/src/r"))
            .iter()
            .map(|i| i.wt.name())
            .collect();
        assert_eq!(cleaned, ["done"]);
        // With changes, it asks twice, for those only.
        items[3].wt.dirty = true;
        assert_eq!(press(&mut popup, Keysym::Delete, &items), None);
        assert_eq!(
            loses(&items[3].wt),
            "Delete again to delete, with its changes"
        );
    }

    #[test]
    fn keys_ask_for_what_the_worktree_allows() {
        let items = items();
        let mut popup = Popup::default();
        popup.select(&items, 0, 10);
        assert_eq!(popup.key(&ctrl(), Keysym::m, &items, 10), None);
        assert_eq!(popup.key(&ctrl(), Keysym::k, &items, 10), None);
        assert_eq!(
            press(&mut popup, Keysym::Return, &items),
            Some(Outcome::Diff("/src/r".into()))
        );
        assert_eq!(
            popup.key(&ctrl(), Keysym::d, &items, 10),
            Some(Outcome::Diff("/src/r".into()))
        );
        // Letters alone are for the filter.
        assert_eq!(press(&mut popup, Keysym::d, &items), None);
        press(&mut popup, Keysym::Down, &items);
        assert_eq!(
            popup.key(&ctrl(), Keysym::m, &items, 10),
            Some(Outcome::Merge("/src/r/.wt/wip".into()))
        );
        assert_eq!(
            popup.key(&ctrl(), Keysym::k, &items, 10),
            Some(Outcome::Checkout("/src/r/.wt/wip".into()))
        );
        press(&mut popup, Keysym::End, &items);
        assert_eq!(popup.key(&ctrl(), Keysym::m, &items, 10), None);
        // A merged one is checked out still.
        assert_eq!(
            popup.key(&ctrl(), Keysym::k, &items, 10),
            Some(Outcome::Checkout("/src/r/.wt/done".into()))
        );
        assert_eq!(
            press(&mut popup, Keysym::Escape, &items),
            Some(Outcome::Close)
        );
        // Shift+Delete twice deletes the merged ones.
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        popup.select(&items, 1, 10);
        assert_eq!(popup.key(&shift, Keysym::Delete, &items, 10), None);
        assert_eq!(popup.armed, Some(Armed::CleanUp("/src/r".into())));
        assert_eq!(
            popup.key(&shift, Keysym::Delete, &items, 10),
            Some(Outcome::CleanUp("/src/r".into()))
        );
        // The deleted one's place goes to the next.
        let rest = [items[0].clone(), items[2].clone()];
        assert_eq!(
            popup.key(&ctrl(), Keysym::c, &rest, 10),
            Some(Outcome::Copy("/src/r/.wt/done".into()))
        );
    }

    #[test]
    fn the_view_has_a_label_per_repository_and_scrolls() {
        let mut items = items();
        let mut other = item(1, "other", true, false, false);
        (other.main, other.wt.path) = ("/src/s".into(), "/src/s".into());
        items.push(other);
        assert_eq!(
            entries(&items),
            vec![
                Entry::Repo(0),
                Entry::Item(0),
                Entry::Item(1),
                Entry::Item(2),
                Entry::Repo(1),
                Entry::Item(3),
            ]
        );
        let names = vec!["r".to_owned(), "s".to_owned()];
        let mut popup = Popup::default();
        let view = |popup: &Popup| popup.view(&items, &names, &HashMap::new(), 3);
        let height = view(&popup).list_height;
        assert_eq!(height, 3 * chrome::WORKTREES_ROW);
        // Scrolled to the second worktree, its repository's label stays on top.
        popup.scroll.jump(1.0);
        let scrolled = view(&popup);
        assert_eq!(scrolled.entries[0], WorktreeEntry::Repo("r".into()));
        assert!(matches!(&scrolled.entries[1], WorktreeEntry::Worktree(l) if l.name == "wip"));
        assert_eq!(scrolled.list_height, height);
        // A label never comes without a worktree under it.
        assert!(matches!(
            scrolled.entries.last(),
            Some(WorktreeEntry::Worktree(_))
        ));
        popup.select(&items, 3, 3);
        let v = view(&popup);
        assert_eq!(v.entries[0], WorktreeEntry::Repo("s".into()));
        assert_eq!(v.selected, Some(1));
        assert_eq!(v.list_height, height);
        popup.select(&items, 0, 3);
        let v = view(&popup);
        assert_eq!(v.entries[0], WorktreeEntry::Repo("r".into()));
        assert_eq!(v.selected, Some(1));
    }

    #[test]
    fn the_scrollbar_thumb_stands_the_list_where_it_is_dragged() {
        let mut items = items();
        items.push(item(0, "a", false, false, false));
        items.push(item(0, "b", false, false, false));
        let mut other = item(1, "other", true, false, false);
        (other.main, other.wt.path) = ("/src/s".into(), "/src/s".into());
        items.push(other);
        let names = vec!["r".to_owned(), "s".to_owned()];
        let view = |popup: &Popup| popup.view(&items, &names, &HashMap::new(), 3);
        let (row, label) = (chrome::WORKTREES_ROW as f64, chrome::WORKTREES_LABEL as f64);
        // Past "b", the next repository's label comes up with "other".
        assert_eq!(scrolled_px(&items, 5.0), 5.0 * row + label);
        assert_eq!(scrolled_px(&items, 4.5), 4.0 * row + (row + label) / 2.0);
        assert_eq!(standing_at(&items, 4.0 * row + (row + label) / 2.0), 4.5);
        let mut popup = Popup::default();
        popup.select(&items, 0, 3);
        assert_eq!(view(&popup).bar.unwrap().down, 0.0);
        // Part of a row down.
        popup.stand_at(1.5 * row, &items, 3);
        let v = view(&popup);
        assert_eq!((v.part, v.bar.unwrap().down), (0.5, 1.5 * row));
        // No further than the end.
        popup.stand_at(1e6, &items, 3);
        let bar = view(&popup).bar.unwrap();
        assert_eq!(bar.down, bar.total - view(&popup).list_height as f64);
        // All shown, there is none.
        let few = &items[..2];
        let fits = popup.view(few, &names, &HashMap::new(), 3);
        assert_eq!(fits.bar, None);
    }

    #[test]
    fn a_touchpad_scrolls_by_parts_of_a_row() {
        let mut items = items();
        items.push(item(0, "a", false, false, false));
        items.push(item(0, "b", false, false, false));
        let mut other = item(1, "other", true, false, false);
        (other.main, other.wt.path) = ("/src/s".into(), "/src/s".into());
        items.push(other);
        let names = vec!["r".to_owned(), "s".to_owned()];
        let view = |popup: &Popup| popup.view(&items, &names, &HashMap::new(), 3);
        let shown = |popup: &Popup| -> Vec<String> {
            view(popup)
                .entries
                .iter()
                .map(|entry| match entry {
                    WorktreeEntry::Repo(name) => format!("# {name}"),
                    WorktreeEntry::Worktree(line) => line.name.clone(),
                })
                .collect()
        };
        let key = |popup: &mut Popup, sym: Keysym| {
            popup.key(&ModifiersState::default(), sym, &items, 3);
        };
        let (t, row) = (Instant::now(), chrome::WORKTREES_ROW as f64);
        let finger = |popup: &mut Popup, rows: f64, time: u32| {
            let motion = Motion::Finger {
                px: rows * row,
                time,
            };
            popup.scroll_by(motion, t, &items, 3);
        };
        let mut popup = Popup::default();
        popup.select(&items, 0, 3);
        // Under the page, already at rest, what shows in its last rows'
        // room and as it scrolls part of a row.
        assert_eq!(shown(&popup), ["# r", "main", "wip", "done", "a"]);
        assert_eq!(view(&popup).part, 0.0);
        finger(&mut popup, 1.5, 0);
        assert_eq!(view(&popup).part, 0.5);
        assert_eq!(shown(&popup), ["# r", "wip", "done", "a", "b"]);
        assert_eq!(view(&popup).list_height, 3 * chrome::WORKTREES_ROW);
        // The next repository's label and worktree come with it.
        popup.scroll.jump(4.0);
        assert_eq!(shown(&popup), ["# r", "b", "# s", "other"]);
        // Up past the top holds the list whole, stretched down; so does
        // its end, stretched up.
        finger(&mut popup, -9.0, 8);
        assert_eq!(view(&popup).part, 0.0);
        assert!(view(&popup).overscroll > 0.0);
        assert_eq!(shown(&popup), ["# r", "main", "wip", "done", "a"]);
        popup.scroll.jump(0.0);
        finger(&mut popup, 9.5, 16);
        assert_eq!(view(&popup).part, 0.0);
        assert!(view(&popup).overscroll < 0.0);
        assert_eq!(shown(&popup), ["# s", "other"]);
        // Keys moving the list, or selecting the worktree scrolled past
        // in part, drop the part.
        popup.scroll.jump(2.5);
        assert_eq!(view(&popup).part, 0.5);
        key(&mut popup, Keysym::End);
        assert_eq!(view(&popup).part, 0.0);
        popup.scroll.jump(2.5);
        assert_eq!(view(&popup).part, 0.5);
        key(&mut popup, Keysym::Home);
        assert_eq!(view(&popup).part, 0.0);
        assert_eq!(shown(&popup), ["# r", "main", "wip", "done", "a"]);
        // Keys keep to the page: a worktree under it is scrolled in.
        key(&mut popup, Keysym::Down);
        key(&mut popup, Keysym::Down);
        assert_eq!(popup.first_shown(&items, 3), 1);
        assert_eq!(view(&popup).selected, Some(2));
        // So is one clicked.
        key(&mut popup, Keysym::Home);
        finger(&mut popup, 0.5, 24);
        popup.click(WorktreesHit::Entry(3), &items, 3);
        assert_eq!(popup.selected(&items), Some(2));
        assert_eq!(popup.first_shown(&items, 3), 1);
        assert_eq!(view(&popup).part, 0.0);
    }

    #[test]
    fn a_moving_list_glides_settles_and_shows_no_hover_meanwhile() {
        let mut items = items();
        items.push(item(0, "a", false, false, false));
        let mut other = item(1, "other", true, false, false);
        (other.main, other.wt.path) = ("/src/s".into(), "/src/s".into());
        items.push(other);
        let mut popup = Popup {
            hover: Some(WorktreesHit::Entry(2)),
            ..Popup::default()
        };
        // From the last worktree of a repository, a notch scrolls to the
        // next one's first, its label and all.
        popup.scroll.jump(3.0);
        assert_eq!(
            popup.bounds(&items, 3).unit,
            (chrome::WORKTREES_ROW + chrome::WORKTREES_LABEL) as f64
        );
        let t = Instant::now();
        popup.scroll_by(Motion::Notches(1.0), t, &items, 3);
        assert_eq!(popup.hover, None);
        let mut now = t;
        let mut moving = true;
        for _ in 0..60 {
            now += std::time::Duration::from_millis(8);
            moving = popup.tick(now, &items, 3);
            if !moving {
                break;
            }
            assert!(popup.scroll.part() > 0.0);
        }
        assert!(!moving);
        assert_eq!((popup.scroll.first(), popup.scroll.part()), (4, 0.0));
        // At rest, the pointer finds the rows again.
        popup.hover = Some(WorktreesHit::Entry(1));
        popup.calm(now);
        assert_eq!(popup.hover, Some(WorktreesHit::Entry(1)));
    }

    /// Two repositories: gerdr at /src/r (main, wip, done) and herdr at
    /// /src/s (its main, s, and feature).
    fn two_repos() -> (Vec<String>, Vec<Item>) {
        let mut items = items();
        for (name, main) in [("s", true), ("feature", false)] {
            let mut i = item(1, name, main, false, false);
            i.main = "/src/s".into();
            i.wt.path = if main {
                "/src/s".into()
            } else {
                format!("/src/s/.wt/{name}").into()
            };
            items.push(i);
        }
        (vec!["gerdr".into(), "herdr".into()], items)
    }

    fn names_of(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| i.wt.name()).collect()
    }

    #[test]
    fn the_filter_scopes_and_searches() {
        let (names, items) = two_repos();
        let r = Some(Path::new("/src/r"));
        let scoped = filter(items.clone(), &names, r, false, "");
        assert_eq!(names_of(&scoped), ["main", "wip", "done"]);
        let all = filter(items.clone(), &names, r, true, "");
        assert_eq!(all.len(), 5);
        assert_eq!(
            names_of(&filter(items.clone(), &names, r, true, "wip")),
            ["wip"]
        );
        // The repository's name matches its rows.
        let by_repo = filter(items.clone(), &names, r, true, "herdr");
        assert_eq!(names_of(&by_repo), ["s", "feature"]);
        // Scoped, the other repository stays out whatever is typed.
        assert!(filter(items.clone(), &names, r, false, "feature").is_empty());
        // With no repository of its own, everything is listed.
        assert_eq!(filter(items, &names, None, false, "").len(), 5);
    }

    #[test]
    fn typing_filters_tab_scopes_and_esc_clears_then_closes() {
        let mut popup = Popup {
            focus: Some("/src/r".into()),
            ..Popup::default()
        };
        let none = ModifiersState::default();
        let typed = |popup: &mut Popup, c: char| {
            let sym = Keysym::from_char(c);
            popup.filter_key(&none, sym, sym)
        };
        assert_eq!(typed(&mut popup, 'w'), Some(Edit::Changed));
        assert_eq!(&*popup.query, "w");
        // Ctrl with a letter is left to the list.
        assert_eq!(popup.filter_key(&ctrl(), Keysym::m, Keysym::m), None);
        let tab = popup.filter_key(&none, Keysym::Tab, Keysym::Tab);
        assert_eq!(tab, Some(Edit::Changed));
        assert!(popup.all);
        let esc = popup.filter_key(&none, Keysym::Escape, Keysym::Escape);
        assert_eq!(esc, Some(Edit::Changed));
        assert!(popup.query.is_empty());
        // Esc with nothing typed is the list's: it closes.
        assert_eq!(
            popup.filter_key(&none, Keysym::Escape, Keysym::Escape),
            None
        );
        // Without a repository of its own there is no scope to flip.
        popup.focus = None;
        assert_eq!(popup.filter_key(&none, Keysym::Tab, Keysym::Tab), None);
    }

    #[test]
    fn a_new_filter_selects_the_best_match() {
        let (names, items) = two_repos();
        let mut popup = Popup::default();
        popup.select(&items, 1, 10);
        popup.query.set("feat");
        let kept = filter(items.clone(), &names, None, true, &popup.query);
        popup.refilter(&kept, 10);
        assert_eq!(
            popup.selected.as_deref(),
            Some(Path::new("/src/s/.wt/feature"))
        );
        // Cleared, the selection stays where it is.
        popup.query.clear();
        popup.refilter(&items, 10);
        assert_eq!(
            popup.selected.as_deref(),
            Some(Path::new("/src/s/.wt/feature"))
        );
    }
}
