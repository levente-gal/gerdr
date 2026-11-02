//! Features in gerdr's window: the new feature dialog, the feature panel,
//! and the work they start off the UI thread (see `feature` for the record
//! and where it lives).
//!
//! The features of every project are held here, loaded at start and when
//! the projects' file is read again, and saved after every change.
//!
//! The new feature dialog: a title, where the feature goes (found off the
//! thread), and over several repositories which ones it changes. Enter
//! creates it, Tab and Shift+Tab (Up/Down) move between the title and the
//! repositories, Space ticks one, Esc clears the title, then closes.
//!
//! The feature panel: a feature's repositories and its eight stages. Up and
//! Down (Ctrl+J/K) move, Enter does the row's own thing (mark the current
//! stage done, ship it once Ship is the current one, reopen a done one,
//! touch a repository or open a tab in it), Left/Right go to the previous
//! or next feature, Ctrl+O opens a tab in the feature's folder,
//! Ctrl+Shift+O opens the pull request of the repository under the cursor,
//! Ctrl+C copies its path, Ctrl+N starts a new one, Del abandons it (or,
//! once done, removes its worktrees) after asking, Esc closes.
//!
//! The marks from Build on are bound to the code (see `feature::patch`):
//! marking one reads the feature's patch-id off the thread first, and the
//! patch watch probes the feature shown every few seconds, and every open
//! feature with a bound mark or a squash under way every half minute (see
//! `feature::ship::probe`). Code changed since sends it back to Build, a
//! squash merged makes it done.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, CheckLine, CheckTone, Dot, FeatureHit, FeaturePanelView, FeatureRepoRow,
        MAX_CHECK_LINES, NewFeatureHit, NewFeatureRepo, NewFeatureView, StageMark, StageRow,
    },
    feature::{
        self, Event, Feature, Pr, Stage, Status,
        hook::Guard,
        layout::{self, Created, Root},
        patch::{self, Reading},
        ship::{self, Action, Probe},
        store,
    },
    home_dir, paths, projects,
    tasks::Task,
    text_field::{Edit, TextField},
};

/// The features of every project, and the dialog and panel on them.
#[derive(Default)]
pub struct Features {
    /// By project id, oldest first.
    pub all: BTreeMap<String, Vec<Feature>>,
    pub dialog: Option<NewFeatureDialog>,
    pub panel: Option<FeaturePanel>,
    /// The feature last shown in the panel, by project: the features key
    /// opens it again.
    shown: BTreeMap<String, String>,
    /// What runs off the thread now, for the tasks panel.
    busy: Vec<Busy>,
    /// The latest probe of each feature, by project and slug.
    probes: BTreeMap<(String, String), Probed>,
    /// What the Ship sheet open or last open asked to do, by project and
    /// slug: what shipping does once confirmed.
    asked: BTreeMap<(String, String), Asked>,
    /// Whether the patch watch's timer runs.
    watching: bool,
    /// The serial of the last dialog opened: a root found for an older one
    /// is dropped.
    serial: u64,
    done: Option<channel::Sender<Done>>,
}

/// How often the patch watch looks: the feature shown, and one whose
/// squash is under way, are probed this often, the others every
/// `WATCH_EVERY`.
const PANEL_EVERY: Duration = Duration::from_secs(5);
const WATCH_EVERY: Duration = Duration::from_secs(30);

/// A feature's probes (see `ship::probe`).
#[derive(Debug, Default)]
struct Probed {
    /// The latest one that came back.
    last: Option<Probe>,
    /// When the latest one started.
    at: Option<Instant>,
    /// One runs now: only one at a time per feature.
    running: bool,
    /// Probe again once the one running is back: what it reads may be
    /// from before a change.
    again: bool,
    /// The reading error last logged: a new one only goes to the log.
    logged: Option<String>,
}

/// What the Ship sheet asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    actions: Vec<Action>,
    /// The patch-id the check was on, for Ship's mark.
    patch: Option<String>,
}

/// What a feature's work off the thread is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Create,
    Touch,
    Losses,
    Remove,
    /// Reading the patch-id to mark a stage done.
    Read,
    Ship,
}

/// Work on a feature running off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Busy {
    project: String,
    /// The feature's slug; for one being created, the slug it gets.
    slug: String,
    job: Job,
    /// The repository being touched, if that is the work.
    repo: Option<String>,
    /// What it does, for the tasks panel: "Creating “Bulk export”".
    what: String,
}

/// What the work off the thread came to.
enum Done {
    Found {
        serial: u64,
        result: Result<Root, String>,
    },
    Created {
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<Created, String>,
    },
    Touched {
        project: String,
        slug: String,
        repo: String,
        result: Result<feature::Repo, String>,
    },
    Losses {
        project: String,
        slug: String,
        losses: Vec<String>,
    },
    Removed {
        project: String,
        slug: String,
        abandon: bool,
        result: Result<(), String>,
    },
    Probed {
        project: String,
        slug: String,
        probe: Probe,
    },
    /// The patch-id read to mark `stage` done.
    Read {
        project: String,
        slug: String,
        stage: Stage,
        result: Result<Reading, String>,
    },
    Shipped {
        project: String,
        slug: String,
        /// The patch-id the Ship check was on.
        patch: Option<String>,
        /// The squash steps, to start in a tab once the code is found
        /// unchanged: then done once merged, not now.
        squash: Vec<Action>,
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
    },
}

/// Where a feature would go, as the dialog knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    Looking,
    Found(Root),
    Failed(String),
}

/// The new feature dialog's state, while it is open.
#[derive(Debug)]
pub struct NewFeatureDialog {
    pub project: String,
    pub serial: u64,
    pub title: TextField,
    pub root: Finding,
    /// Whether the feature changes each repository of the root.
    pub touched: Vec<bool>,
    /// The repository row with the keyboard; `None`: the title.
    pub focus: Option<usize>,
    /// The first repository shown, when not all fit.
    pub top: usize,
    pub error: Option<String>,
    pub hover: Option<NewFeatureHit>,
    /// The folder it was opened from: the repository ticked first.
    pub from: PathBuf,
}

/// What the new feature dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Cancel,
    /// Create a feature with this title in the root found, changing these
    /// repositories.
    Create {
        title: String,
        root: Root,
        touched: Vec<String>,
    },
}

impl NewFeatureDialog {
    fn new(project: String, serial: u64, from: PathBuf) -> Self {
        Self {
            project,
            serial,
            title: TextField::default(),
            root: Finding::Looking,
            touched: Vec::new(),
            focus: None,
            top: 0,
            error: None,
            hover: None,
            from,
        }
    }

    /// The root was found, or not: over several repositories the one it
    /// was opened from is ticked.
    fn found(&mut self, result: Result<Root, String>) {
        match result {
            Ok(root) => {
                self.touched = root
                    .repos
                    .iter()
                    .map(|r| !root.multi || self.from.starts_with(&r.main))
                    .collect();
                self.root = Finding::Found(root);
            }
            Err(why) => self.root = Finding::Failed(why),
        }
        self.error = None;
    }

    /// Whether its repositories can be ticked: over several only.
    fn multi(&self) -> bool {
        matches!(&self.root, Finding::Found(root) if root.multi)
    }

    fn repos(&self) -> usize {
        match &self.root {
            Finding::Found(root) => root.repos.len(),
            _ => 0,
        }
    }

    fn toggle(&mut self, i: usize) {
        if self.multi()
            && let Some(t) = self.touched.get_mut(i)
        {
            *t = !*t;
            self.error = None;
        }
    }

    /// What creating it would do, or why it can't, as a sentence.
    fn create(&self) -> Result<DialogOutcome, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Give the feature a title.".into());
        }
        let root = match &self.root {
            Finding::Looking => {
                return Err("Still finding the repositories; try again in a moment.".into());
            }
            Finding::Failed(why) => return Err(format!("No repositories to start from: {why}.")),
            Finding::Found(root) => root,
        };
        let touched: Vec<String> = root
            .repos
            .iter()
            .zip(&self.touched)
            .filter(|(_, t)| **t)
            .map(|(r, _)| r.name.clone())
            .collect();
        if touched.is_empty() {
            return Err("Tick at least one repository the feature changes.".into());
        }
        Ok(DialogOutcome::Create {
            title: title.to_owned(),
            root: root.clone(),
            touched,
        })
    }

    /// Move the keyboard between the title and the repositories.
    fn step(&mut self, back: bool) {
        let n = if self.multi() { self.repos() } else { 0 };
        let at = self.focus.map_or(0, |i| i + 1);
        let to = if back {
            (at + n) % (n + 1)
        } else {
            (at + 1) % (n + 1)
        };
        self.focus = to.checked_sub(1);
    }

    /// Scroll so the focused repository is among the `fit` shown.
    fn reveal(&mut self, fit: usize) {
        let fit = fit.max(1);
        if let Some(i) = self.focus {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + fit {
                self.top = i + 1 - fit;
            }
        }
        self.top = self.top.min(self.repos().saturating_sub(fit));
    }

    /// Handle a key, `fit` repositories shown at once: `sym` is the key's
    /// own symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        fit: usize,
    ) -> Option<DialogOutcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        match sym {
            Keysym::Escape => {
                // Back to the title, then the title cleared, then closed.
                if self.focus.is_some() {
                    self.focus = None;
                } else if !self.title.is_empty() {
                    self.title.clear();
                    self.error = None;
                } else {
                    return Some(DialogOutcome::Cancel);
                }
            }
            Keysym::Return | Keysym::KP_Enter => match self.create() {
                Ok(outcome) => return Some(outcome),
                Err(why) => self.error = Some(why),
            },
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::Down | Keysym::KP_Down => self.step(false),
            Keysym::Up | Keysym::KP_Up => self.step(true),
            Keysym::j | Keysym::J if ctrl => self.step(false),
            Keysym::k | Keysym::K if ctrl => self.step(true),
            Keysym::space if plain && self.focus.is_some() => {
                if let Some(i) = self.focus {
                    self.toggle(i);
                }
            }
            _ if self.focus.is_none() && self.title.key(mods, sym, typed) == Edit::Changed => {
                self.error = None;
            }
            _ => {}
        }
        self.reveal(fit);
        None
    }
}

/// The feature panel's state, while it is open.
#[derive(Debug, Default)]
pub struct FeaturePanel {
    pub project: String,
    pub slug: String,
    /// The row with the keyboard: the repositories', then the stages'.
    pub cursor: usize,
    /// The first repository shown, when not all fit.
    pub repo_top: usize,
    pub hover: Option<FeatureHit>,
}

/// What the feature panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelOutcome {
    Close,
    /// Mark the current stage done.
    Tick,
    /// Ship it, after the Ship sheet: the current stage is Ship.
    Ship,
    /// Start this agent stage's agent (fresh: even if one runs).
    StartAgent(Stage, bool),
    /// Show this agent stage's agent.
    ShowAgent(Stage),
    /// Review this agent stage's file.
    ReviewDoc(Stage),
    /// Open this pull request's page.
    OpenPr(String),
    /// Clear this stage's mark and the later ones'.
    Untick(Stage),
    /// A later stage can't be marked yet: this one comes first.
    NotYet(Stage),
    /// Make this repository one the feature changes.
    Touch(String),
    /// Open a tab in this folder: the feature's, or a repository's
    /// worktree.
    Open(PathBuf),
    CopyPath,
    /// Abandon it, or once done remove its worktrees, after asking.
    Remove,
    New,
    /// Show the feature with this slug.
    Show(String),
}

/// A row of the feature panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelRow {
    Repo(usize),
    Stage(Stage),
}

/// The rows of the feature panel for `feature`: its repositories, then
/// its stages.
pub fn panel_rows(feature: &Feature) -> Vec<PanelRow> {
    (0..feature.repos.len())
        .map(PanelRow::Repo)
        .chain(Stage::ALL.into_iter().map(PanelRow::Stage))
        .collect()
}

/// Where `stage` stands in `feature`.
pub fn stage_mark(feature: &Feature, stage: Stage) -> StageMark {
    if feature.is_done(stage) {
        StageMark::Done
    } else if feature.current() == Some(stage) {
        StageMark::Current
    } else {
        StageMark::Later
    }
}

/// What Enter (or the row's button) does on `row` of `feature`.
pub fn row_action(feature: &Feature, row: PanelRow) -> Option<PanelOutcome> {
    match row {
        PanelRow::Repo(i) => {
            let repo = feature.repos.get(i)?;
            Some(if repo.touched {
                PanelOutcome::Open(repo.worktree.clone())
            } else {
                PanelOutcome::Touch(repo.name.clone())
            })
        }
        PanelRow::Stage(stage) => Some(match stage_mark(feature, stage) {
            StageMark::Done => PanelOutcome::Untick(stage),
            StageMark::Current if stage == Stage::Ship => PanelOutcome::Ship,
            StageMark::Current => PanelOutcome::Tick,
            StageMark::Later => PanelOutcome::NotYet(feature.current()?),
        }),
    }
}

/// What Enter does on `row`, in words for its hint.
fn row_hint(feature: &Feature, row: PanelRow) -> &'static str {
    match row_action(feature, row) {
        Some(PanelOutcome::Tick) => "mark done",
        Some(PanelOutcome::Ship) => "ship\u{2026}",
        Some(PanelOutcome::Untick(_)) => "reopen",
        Some(PanelOutcome::Touch(_)) => "touch",
        Some(PanelOutcome::Open(_)) => "open tab",
        _ => "",
    }
}

/// The label of `stage`'s button in `feature`: none for a later one.
fn stage_button(feature: &Feature, stage: Stage) -> Option<&'static str> {
    match row_action(feature, PanelRow::Stage(stage))? {
        PanelOutcome::Tick => Some("Mark done"),
        PanelOutcome::Ship => Some("Ship\u{2026}"),
        PanelOutcome::Untick(_) => Some("Reopen"),
        _ => None,
    }
}

/// The pull request `feature` opened for repository `repo`.
fn pr_of<'a>(feature: &'a Feature, repo: &str) -> Option<&'a Pr> {
    feature.shipping.prs.iter().find(|pr| pr.repo == repo)
}

/// What repository `i`'s button does: touch one the feature doesn't
/// change, open the pull request of one that has one.
pub fn repo_button(feature: &Feature, i: usize) -> Option<PanelOutcome> {
    let repo = feature.repos.get(i)?;
    if !repo.touched {
        return Some(PanelOutcome::Touch(repo.name.clone()));
    }
    pr_of(feature, &repo.name).map(|pr| PanelOutcome::OpenPr(pr.url.clone()))
}

/// The end of a pull request's address, as the panel shows it:
/// `api/pull/12` of `https://github.com/acme/api/pull/12`.
fn pr_tail(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').take(3).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// What Ship asks for, given the latest `probe`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ShipAsk {
    /// No check yet.
    Checking,
    /// What stops it: the toast's text.
    Blocked(String),
    /// It can ship, but nothing would.
    Nothing,
    Ready(Asked),
}

/// What shipping `title` asks for, given the latest `probe`.
fn ship_ask(title: &str, probe: Option<&Probe>) -> ShipAsk {
    let Some(probe) = probe else {
        return ShipAsk::Checking;
    };
    let Some(check) = &probe.ship else {
        return match &probe.reading {
            Err(why) => ShipAsk::Blocked(blocked_text(title, &[read_blocker(why)])),
            Ok(_) => ShipAsk::Checking,
        };
    };
    if !check.blockers.is_empty() {
        return ShipAsk::Blocked(blocked_text(title, &check.blockers));
    }
    if check.actions.is_empty() {
        return ShipAsk::Nothing;
    }
    ShipAsk::Ready(Asked {
        actions: check.actions.clone(),
        patch: probe.reading.as_ref().ok().map(|r| r.patch.clone()),
    })
}

/// What a feature's code that couldn't be read says in the Ship check.
fn read_blocker(why: &str) -> String {
    format!("Couldn't read the code: {why}")
}

/// The warning when `title` can't ship for `blockers`: the first, and how
/// many more.
fn blocked_text(title: &str, blockers: &[String]) -> String {
    let first = blockers.first().map_or("", String::as_str);
    match blockers.len() {
        0 | 1 => format!("Can't ship \u{201c}{title}\u{201d} yet: {first}"),
        n => format!(
            "Can't ship \u{201c}{title}\u{201d} yet: {first}, and {} more",
            n - 1
        ),
    }
}

/// The warning when the code of `title` changed, its marks from `stage`
/// on gone.
fn changed_text(title: &str, stage: Stage) -> String {
    format!("\u{201c}{title}\u{201d} changed: back to {}", stage.label())
}

/// The success toast once the pull requests of `title` are open:
/// `opened` new ones, and `updated` ones it had open already, which the
/// push updated.
fn opened_text(title: &str, opened: usize, updated: usize) -> String {
    let what = |n: usize| {
        if n == 1 {
            "pull request"
        } else {
            "pull requests"
        }
    };
    let title = format!("\u{201c}{title}\u{201d}");
    match (opened, updated) {
        (n, 0) => format!("Opened {n} {} for {title}", what(n)),
        (0, n) => format!("Updated {n} {} for {title}", what(n)),
        (o, u) => format!("Opened {o} and updated {u} {} for {title}", what(o + u)),
    }
}

/// `lines` as the panel has room for: past `MAX_CHECK_LINES`, the first
/// ones and how many more, in `tone`.
fn capped(lines: Vec<CheckLine>, tone: CheckTone) -> Vec<CheckLine> {
    if lines.len() <= MAX_CHECK_LINES {
        return lines;
    }
    let more = lines.len() - (MAX_CHECK_LINES - 1);
    let mut lines = lines;
    lines.truncate(MAX_CHECK_LINES - 1);
    lines.push(CheckLine {
        text: format!("and {more} more"),
        tone,
    });
    lines
}

/// The Ship check the panel shows under the Ship row of `feature`, from
/// the latest `probe`: only while Ship is its current stage. `shipping`:
/// its pull requests are being opened now.
fn check_lines(
    feature: &Feature,
    probe: Option<&Probe>,
    shipping: bool,
    now: u64,
) -> Vec<CheckLine> {
    if feature.status != Status::Open || feature.current() != Some(Stage::Ship) {
        return Vec::new();
    }
    let line = |text: String, tone| CheckLine { text, tone };
    if shipping {
        return vec![line("shipping\u{2026}".into(), CheckTone::Note)];
    }
    if let Some(at) = feature.shipping.squash_at {
        let when = projects::ago(now, at);
        let text = format!("Squash started {when}: done once it is merged");
        return vec![line(text, CheckTone::Note)];
    }
    let check = probe.and_then(|p| p.ship.as_ref());
    let Some(check) = check else {
        return match probe.map(|p| &p.reading) {
            Some(Err(why)) => vec![line(read_blocker(why), CheckTone::Blocker)],
            _ => vec![line("checking\u{2026}".into(), CheckTone::Note)],
        };
    };
    if !check.blockers.is_empty() {
        let lines = check
            .blockers
            .iter()
            .map(|b| line(b.clone(), CheckTone::Blocker))
            .collect();
        return capped(lines, CheckTone::Blocker);
    }
    let ready = line("Ready to ship".into(), CheckTone::Ready);
    let notes = check.notes.iter().map(|n| line(n.clone(), CheckTone::Note));
    capped(
        std::iter::once(ready).chain(notes).collect(),
        CheckTone::Note,
    )
}

/// Whether the patch watch keeps an eye on `feature`: open, with a mark
/// bound to its code or a squash under way.
fn watched(feature: &Feature) -> bool {
    feature.status == Status::Open
        && (feature.marks.keys().any(|s| s.bound()) || feature.shipping.squash_at.is_some())
}

/// How often the patch watch probes watched `feature` while the panel
/// shows another: as often as the one shown while its squash is under
/// way, as the user waits on it in its tab (and the panel closed to show
/// that tab), else every `WATCH_EVERY`.
fn watch_every(feature: &Feature) -> Duration {
    if feature.shipping.squash_at.is_some() {
        PANEL_EVERY
    } else {
        WATCH_EVERY
    }
}

/// The label of what removes `feature`'s worktrees: abandoning it while
/// open, removing them once done.
pub fn remove_label(feature: &Feature) -> &'static str {
    if feature.status == Status::Done {
        "Remove worktrees"
    } else {
        "Abandon"
    }
}

/// The features the panel lists, of a project's `all`: the open ones, and
/// the done ones whose worktrees are still there to remove.
pub fn listed(all: &[Feature]) -> Vec<&Feature> {
    all.iter()
        .filter(|f| match f.status {
            Status::Open => true,
            Status::Done => f.dir.exists(),
            Status::Abandoned => false,
        })
        .collect()
}

/// The features the sidebar shows, of a project's `all`: the open ones.
pub fn sidebar_features(all: &[Feature]) -> Vec<&Feature> {
    all.iter().filter(|f| f.status == Status::Open).collect()
}

/// A slug for a feature called `title`, unique among `all`.
pub fn new_slug(title: &str, all: &[Feature]) -> String {
    let taken: Vec<&str> = all.iter().map(|f| f.slug.as_str()).collect();
    projects::slugify(title, &taken)
}

impl FeaturePanel {
    /// Handle a key over `feature`, one of `slugs` listed.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        feature: &Feature,
        slugs: &[String],
    ) -> Option<PanelOutcome> {
        let rows = panel_rows(feature);
        let last = rows.len().saturating_sub(1);
        self.cursor = self.cursor.min(last);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        match sym {
            Keysym::Escape => return Some(PanelOutcome::Close),
            Keysym::Up | Keysym::KP_Up => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Down | Keysym::KP_Down => self.cursor = (self.cursor + 1).min(last),
            Keysym::k | Keysym::K if ctrl => self.cursor = self.cursor.saturating_sub(1),
            Keysym::j | Keysym::J if ctrl => self.cursor = (self.cursor + 1).min(last),
            Keysym::Home | Keysym::KP_Home => self.cursor = 0,
            Keysym::End | Keysym::KP_End => self.cursor = last,
            Keysym::Return | Keysym::KP_Enter => return row_action(feature, rows[self.cursor]),
            Keysym::Left | Keysym::KP_Left | Keysym::Right | Keysym::KP_Right if plain => {
                let back = matches!(sym, Keysym::Left | Keysym::KP_Left);
                return switch(slugs, &self.slug, back).map(PanelOutcome::Show);
            }
            Keysym::o | Keysym::O if ctrl && mods.shift => {
                return match rows[self.cursor] {
                    PanelRow::Repo(i) => {
                        repo_button(feature, i).filter(|out| matches!(out, PanelOutcome::OpenPr(_)))
                    }
                    PanelRow::Stage(_) => None,
                };
            }
            Keysym::o | Keysym::O if ctrl => return Some(PanelOutcome::Open(feature.dir.clone())),
            Keysym::c | Keysym::C if ctrl => return Some(PanelOutcome::CopyPath),
            Keysym::n | Keysym::N if ctrl => return Some(PanelOutcome::New),
            Keysym::Delete | Keysym::KP_Delete if plain => return Some(PanelOutcome::Remove),
            _ => {}
        }
        None
    }

    /// Scroll so the cursor's repository is among the `fit` shown, of
    /// `repos`.
    fn reveal(&mut self, repos: usize, fit: usize) {
        let fit = fit.max(1);
        if self.cursor < repos {
            if self.cursor < self.repo_top {
                self.repo_top = self.cursor;
            } else if self.cursor >= self.repo_top + fit {
                self.repo_top = self.cursor + 1 - fit;
            }
        }
        self.repo_top = self.repo_top.min(repos.saturating_sub(fit));
    }
}

/// The slug before (`back`) or after `slug` in `slugs`, round the ends;
/// `None` with only one.
fn switch(slugs: &[String], slug: &str, back: bool) -> Option<String> {
    if slugs.len() < 2 {
        return None;
    }
    let at = slugs.iter().position(|s| s == slug).unwrap_or(0);
    let n = slugs.len();
    let to = if back { (at + n - 1) % n } else { (at + 1) % n };
    Some(slugs[to].clone())
}

/// The feature the features key shows, of `slugs` listed: the one open,
/// else the one last shown, else the newest.
fn pick_feature(slugs: &[String], open: Option<&str>, last: Option<&str>) -> Option<String> {
    [open, last]
        .into_iter()
        .flatten()
        .find(|s| slugs.iter().any(|l| l == s))
        .map(str::to_owned)
        .or_else(|| slugs.last().cloned())
}

/// The cursor a feature's panel starts at: its current stage, else its
/// last.
fn start_cursor(feature: &Feature) -> usize {
    let stage = feature.current().unwrap_or(Stage::Ship);
    let at = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
    feature.repos.len() + at
}

/// A folder as the chrome shows it, from the home folder as `~`.
fn tilde(path: &Path) -> String {
    paths::display(path, home_dir().as_deref(), 60)
}

/// What a done stage's row says of its mark.
fn mark_detail(mark: &feature::Mark, now: u64) -> String {
    let when = projects::ago(now, mark.at);
    if mark.note.is_empty() {
        format!("done {when} by {}", mark.by)
    } else {
        format!("done {when} by {} \u{b7} {}", mark.by, mark.note)
    }
}

impl Gerdr {
    // ------------------------------------------------------------ state

    /// Read the features of every project from their store, as at start
    /// and when the projects' file is read again. A project whose features
    /// can't be read keeps those it had, and says so.
    pub fn load_features(&mut self) {
        let ids: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let mut all = BTreeMap::new();
        for (id, name) in ids {
            match store::load_all(None, &id) {
                Ok(features) => {
                    all.insert(id, features);
                }
                Err(why) => {
                    tracing::warn!("reading the features of {id}: {why}");
                    self.show_error(format!("Couldn't read the features of {name}: {why}"));
                    if let Some(kept) = self.features.all.remove(&id) {
                        all.insert(id, kept);
                    }
                }
            }
        }
        self.features.all = all;
        let all = &self.features.all;
        let kept = |(project, slug): &(String, String)| {
            all.get(project)
                .is_some_and(|fs| fs.iter().any(|f| f.slug == *slug))
        };
        self.features.probes.retain(|key, _| kept(key));
        self.features.asked.retain(|key, _| kept(key));
        self.watch_features();
        // A panel on a feature gone closes.
        if let Some(panel) = &self.features.panel
            && self.feature(&panel.project, &panel.slug).is_none()
        {
            self.features.panel = None;
            self.refresh_pointer_focus();
        }
    }

    /// Feature `slug` of project `project`.
    pub fn feature(&self, project: &str, slug: &str) -> Option<&Feature> {
        self.features
            .all
            .get(project)?
            .iter()
            .find(|f| f.slug == slug)
    }

    fn feature_mut(&mut self, project: &str, slug: &str) -> Option<&mut Feature> {
        self.features
            .all
            .get_mut(project)?
            .iter_mut()
            .find(|f| f.slug == slug)
    }

    /// Save feature `slug` of `project` as it is now; a failure says so.
    fn save_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if let Err(err) = store::save(None, feature) {
            tracing::warn!("saving feature {project}/{slug}: {err}");
            let title = feature.title.clone();
            self.show_error(format!("Couldn't save \u{201c}{title}\u{201d}: {err}"));
        }
    }

    /// The slugs of the features the panel lists for `project`.
    fn listed_slugs(&self, project: &str) -> Vec<String> {
        self.features
            .all
            .get(project)
            .map(|all| listed(all).into_iter().map(|f| f.slug.clone()).collect())
            .unwrap_or_default()
    }

    /// Where the threads send what they came to; set up the first time.
    fn feature_sender(&mut self) -> Option<channel::Sender<Done>> {
        if self.features.done.is_none() {
            let (tx, rx) = channel::channel::<Done>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg(done) => state.feature_done(done),
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.features.done = Some(tx),
                Err(err) => {
                    tracing::warn!("working on features: {err}");
                    self.show_error(format!("Couldn't start the work on features: {err}"));
                    return None;
                }
            }
        }
        self.features.done.clone()
    }

    /// Run `work` off the thread, its result sent back as a `Done`; `busy`
    /// shows in the tasks panel meanwhile.
    fn feature_work(&mut self, busy: Option<Busy>, work: impl FnOnce() -> Done + Send + 'static) {
        let Some(tx) = self.feature_sender() else {
            return;
        };
        self.features.busy.extend(busy);
        std::thread::spawn(move || {
            // It fails only once the loop is gone: gerdr is quitting, and
            // nothing is left to show the result in.
            let _ = tx.send(work());
        });
    }

    /// The end of work on a feature, the panel open or not: what it came
    /// to is kept and said.
    fn feature_done(&mut self, done: Done) {
        match done {
            Done::Found { serial, result } => {
                if let Err(why) = &result {
                    tracing::warn!("finding where a feature goes: {why}");
                }
                // Dropped when the dialog it was for closed: a new one
                // looks again.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.found(result);
                }
            }
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Create);
                self.feature_created(project, title, slug, root, result);
            }
            Done::Touched {
                project,
                slug,
                repo,
                result,
            } => {
                self.features.busy.retain(|b| {
                    !(b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo))
                });
                self.feature_touched(&project, &slug, &repo, result);
            }
            Done::Losses {
                project,
                slug,
                losses,
            } => {
                self.unbusy(&project, &slug, Job::Losses);
                self.ask_remove_feature(project, slug, losses);
            }
            Done::Removed {
                project,
                slug,
                abandon,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Remove);
                self.feature_removed(&project, &slug, abandon, result);
            }
            Done::Probed {
                project,
                slug,
                probe,
            } => self.feature_probed(&project, &slug, probe),
            Done::Read {
                project,
                slug,
                stage,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Read);
                self.feature_read(&project, &slug, stage, result);
            }
            Done::Shipped {
                project,
                slug,
                patch,
                squash,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.feature_shipped(&project, &slug, patch.as_deref(), &squash, result);
            }
        }
    }

    /// `job` on feature `slug` of `project` is over.
    fn unbusy(&mut self, project: &str, slug: &str, job: Job) {
        self.features
            .busy
            .retain(|b| !(b.project == project && b.slug == slug && b.job == job));
    }

    /// Whether feature `slug` of `project` has work running off the thread.
    fn feature_busy(&self, project: &str, slug: &str) -> bool {
        self.features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug)
    }

    // ------------------------------------------------------ patch watch

    /// Start the patch watch, once: every `PANEL_EVERY` it probes the
    /// feature shown, and the watched ones not probed for as long as
    /// `watch_every` says.
    fn watch_features(&mut self) {
        if self.features.watching {
            return;
        }
        let timer = Timer::from_duration(PANEL_EVERY);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.feature_watch_tick();
            TimeoutAction::ToDuration(PANEL_EVERY)
        });
        match inserted {
            Ok(_) => self.features.watching = true,
            Err(err) => tracing::warn!("watching the features' code: {err}"),
        }
    }

    /// Probe what is due.
    fn feature_watch_tick(&mut self) {
        let mut due = Vec::new();
        if let Some(panel) = &self.features.panel {
            due.push((panel.project.clone(), panel.slug.clone(), PANEL_EVERY));
        }
        for (project, all) in &self.features.all {
            for feature in all.iter().filter(|f| watched(f)) {
                due.push((project.clone(), feature.slug.clone(), watch_every(feature)));
            }
        }
        let now = Instant::now();
        // The timer's own jitter doesn't skip a round.
        let slack = Duration::from_secs(1);
        for (project, slug, every) in due {
            let probed = self.features.probes.get(&(project.clone(), slug.clone()));
            let fresh = probed
                .and_then(|p| p.at)
                .is_some_and(|at| now.duration_since(at) + slack < every);
            if !fresh && !probed.is_some_and(|p| p.running) {
                self.probe_feature(&project, &slug);
            }
        }
    }

    /// Probe open feature `slug` of `project` off the thread (see
    /// `ship::probe`); one running already goes again once it is back.
    fn probe_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if feature.status != Status::Open || self.feature_sender().is_none() {
            return;
        }
        let mode = self.projects.project(project).and_then(|p| p.workflow.ship);
        let probed = self
            .features
            .probes
            .entry((project.to_owned(), slug.to_owned()))
            .or_default();
        if probed.running {
            probed.again = true;
            return;
        }
        probed.running = true;
        probed.at = Some(Instant::now());
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move || Done::Probed {
            probe: ship::probe(&feature, mode),
            project,
            slug,
        });
    }

    /// A probe of feature `slug` of `project` is back: its code changed
    /// sends it back, its squash merged makes it done.
    fn feature_probed(&mut self, project: &str, slug: &str, probe: Probe) {
        let key = (project.to_owned(), slug.to_owned());
        let probed = self.features.probes.entry(key).or_default();
        probed.running = false;
        let again = std::mem::take(&mut probed.again);
        match &probe.reading {
            Err(why) if probed.logged.as_ref() != Some(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                probed.logged = Some(why.clone());
            }
            Err(_) => {}
            Ok(_) => probed.logged = None,
        }
        let patch = probe.reading.as_ref().ok().map(|r| r.patch.clone());
        let merged = probe.merged;
        probed.last = Some(probe);
        if let Some(patch) = &patch {
            self.feature_code_is(project, slug, patch);
        }
        if merged {
            self.feature_merged(project, slug, patch.as_deref());
        }
        if again {
            self.probe_feature(project, slug);
        }
    }

    /// The code of feature `slug` is `patch`'s now: the marks that no
    /// longer hold go, and the toast says so.
    fn feature_code_is(&mut self, project: &str, slug: &str, patch: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.invalidate(patch, feature::now()) else {
            return;
        };
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_warning(changed_text(&title, stage));
    }

    /// The squash of feature `slug` is merged: Ship is done.
    fn feature_merged(&mut self, project: &str, slug: &str, patch: Option<&str>) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let squashing = feature.status == Status::Open
            && feature.current() == Some(Stage::Ship)
            && feature.shipping.squash_at.is_some();
        if !squashing {
            return;
        }
        feature.tick(feature::now(), "gerdr", patch);
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_success(format!("\u{201c}{title}\u{201d} is merged: done"));
    }

    /// Mark bound stage `stage` of feature `slug` done once its code is
    /// read off the thread (see `feature_read`).
    fn read_and_tick(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Read,
            repo: None,
            what: format!("Checking \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || Done::Read {
            result: patch::read(&feature),
            project,
            slug,
            stage,
        });
    }

    /// The code of feature `slug` is read to mark `stage` done: done on
    /// that code, unless it has changes not committed, or `stage` is no
    /// longer the current one.
    fn feature_read(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        result: Result<Reading, String>,
    ) {
        let reading = match result {
            Ok(reading) => reading,
            Err(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't mark {} done: {why}", stage.label()));
            }
        };
        if !reading.dirty.is_empty() {
            return self.show_warning(format!(
                "Commit the changes in {} first",
                reading.dirty.join(", ")
            ));
        }
        self.feature_code_is(project, slug, &reading.patch);
        let still = self
            .feature(project, slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage));
        if still {
            self.tick_feature(project, slug, Some(&reading.patch));
        }
        self.probe_feature(project, slug);
    }

    /// Mark the current stage of feature `slug` done by you, on the code
    /// whose patch-id is `patch`, and say so.
    fn tick_feature(&mut self, project: &str, slug: &str, patch: Option<&str>) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.tick(feature::now(), "you", patch) else {
            return self.show_warning("Nothing left to mark done".into());
        };
        let shipped = feature.status == Status::Done;
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.show_success(if shipped {
            format!("\u{201c}{title}\u{201d} is done")
        } else {
            format!("{} marked done", stage.label())
        });
        self.feature_cursor_to_current_of(project, slug);
    }

    /// The tasks running on features, for the tasks panel.
    pub fn feature_tasks(&self) -> Vec<Task> {
        self.features
            .busy
            .iter()
            .map(|b| Task {
                title: b.what.clone(),
                chip: "feature".into(),
                detail: self
                    .projects
                    .project(&b.project)
                    .map_or_else(|| b.project.clone(), |p| p.name.clone()),
                dot: Dot::Working,
                actions: Vec::new(),
            })
            .collect()
    }

    // ----------------------------------------------------------- dialog

    /// Open the new feature dialog for the active project, from the
    /// folder of workspace `at`, else the active one's.
    pub fn open_new_feature(&mut self, at: Option<usize>) {
        let Some(id) = self.project.clone() else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let Some(project) = self.projects.project(&id) else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let workspace_dirs: Vec<PathBuf> =
            project.workspaces.iter().map(|w| w.dir.clone()).collect();
        let ignore = project.workflow.ignore.clone();
        let bases: Vec<(String, String)> = project
            .workflow
            .base
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let from = at
            .and_then(|i| self.workspaces.get(i))
            .or_else(|| self.workspace())
            .map(|ws| ws.dir.clone())
            .or_else(|| workspace_dirs.first().cloned());
        let Some(from) = from else {
            return self.show_warning("Add a workspace with a folder to start a feature".into());
        };
        self.features.serial += 1;
        let serial = self.features.serial;
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.dialog = Some(NewFeatureDialog::new(id, serial, from.clone()));
        self.feature_work(None, move || Done::Found {
            serial,
            result: layout::find_root(&from, &workspace_dirs, &ignore, &bases),
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// How many repositories the dialog shows at once.
    fn new_feature_fit(&self) -> usize {
        let Some(dialog) = &self.features.dialog else {
            return 1;
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .rows
            .len()
    }

    /// A key while the new feature dialog has the keyboard.
    pub fn new_feature_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<DialogOutcome> {
        let fit = self.new_feature_fit();
        self.features.dialog.as_mut()?.key(mods, sym, typed, fit)
    }

    /// Do what the new feature dialog asked: create the feature off the
    /// thread, shown in the tasks panel meanwhile.
    pub fn apply_new_feature(&mut self, outcome: DialogOutcome) {
        let Some(dialog) = self.features.dialog.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        let DialogOutcome::Create {
            title,
            root,
            touched,
        } = outcome
        else {
            return;
        };
        let project = dialog.project;
        let all = self
            .features
            .all
            .get(&project)
            .map_or(&[][..], Vec::as_slice);
        let slug = new_slug(&title, all);
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        self.feature_work(Some(busy), move || {
            let result = layout::create(&root, &slug, &touched);
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            }
        });
    }

    /// A feature's worktrees were made, or not: keep it, show it, and say
    /// what was changed outside its folder.
    fn feature_created(
        &mut self,
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<Created, String>,
    ) {
        let created = match result {
            Ok(created) => created,
            Err(why) => {
                tracing::warn!("creating feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't create \u{201c}{title}\u{201d}: {why}"));
            }
        };
        let text = created_text(&title, &root.path, &created.ignores);
        let now = feature::now();
        let feature = Feature {
            version: feature::VERSION,
            slug: slug.clone(),
            title: title.clone(),
            project: project.clone(),
            created: now,
            root: root.path,
            dir: created.dir,
            multi: root.multi,
            repos: created.repos,
            status: Status::Open,
            marks: BTreeMap::new(),
            history: vec![Event {
                at: now,
                what: "created".into(),
            }],
            shipping: Default::default(),
            runs: Default::default(),
        };
        self.features
            .all
            .entry(project.clone())
            .or_default()
            .push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        self.show_success(text);
    }

    /// A left click while the new feature dialog is open.
    pub fn new_feature_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
        let outcome = match layout.hit(dialog.top, pos) {
            Some(NewFeatureHit::Button(chrome::CREATE)) => match dialog.create() {
                Ok(outcome) => Some(outcome),
                Err(why) => {
                    dialog.error = Some(why);
                    None
                }
            },
            Some(NewFeatureHit::Button(_)) => Some(DialogOutcome::Cancel),
            Some(NewFeatureHit::Repo(i)) => {
                if dialog.multi() {
                    dialog.focus = Some(i);
                    dialog.toggle(i);
                }
                None
            }
            Some(NewFeatureHit::Field) => {
                dialog.focus = None;
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            // Off it, it stays: a click away never throws the title away.
            None => None,
        };
        if let Some(outcome) = outcome {
            self.apply_new_feature(outcome);
        }
    }

    /// The pointer moved while the new feature dialog is open.
    pub fn new_feature_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
            dialog.hover = layout.hit(dialog.top, pos);
        }
    }

    /// Scroll the dialog's repositories `by` rows.
    pub fn new_feature_scroll(&mut self, by: isize) {
        let fit = self.new_feature_fit();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let most = dialog.repos().saturating_sub(fit.max(1));
            dialog.top = dialog.top.saturating_add_signed(by).min(most);
        }
    }

    /// The dialog's repository rows shown, for the wheel's pitch.
    pub fn new_feature_rows(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some()).rows
    }

    /// What the pointer can press in the new feature dialog.
    pub fn new_feature_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .controls()
    }

    pub fn new_feature_view(&self) -> Option<NewFeatureView> {
        let dialog = self.features.dialog.as_ref()?;
        let project = self
            .projects
            .project(&dialog.project)
            .map_or_else(|| dialog.project.clone(), |p| p.name.clone());
        Some(new_feature_view(dialog, project))
    }

    // ------------------------------------------------------------ panel

    /// The features key: the panel on the active project's features, or
    /// the new feature dialog when it has none.
    pub fn open_features(&mut self) {
        let Some(project) = self.project.clone() else {
            return self.show_warning("Open a project to see its features".into());
        };
        let slugs = self.listed_slugs(&project);
        let open = self.features.panel.as_ref().map(|p| p.slug.as_str());
        let last = self.features.shown.get(&project).map(String::as_str);
        match pick_feature(&slugs, open, last) {
            Some(slug) => self.open_feature(&project, &slug),
            None => self.open_new_feature(None),
        }
    }

    /// Open the feature panel on feature `slug` of `project`. The other
    /// panels close: it would cover them.
    pub fn open_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let cursor = start_cursor(feature);
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.features
            .shown
            .insert(project.to_owned(), slug.to_owned());
        self.features.panel = Some(FeaturePanel {
            project: project.to_owned(),
            slug: slug.to_owned(),
            cursor,
            ..FeaturePanel::default()
        });
        self.reveal_feature_row();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
        self.probe_feature(project, slug);
    }

    /// Close the feature panel.
    pub fn close_feature_panel(&mut self) {
        self.features.panel = None;
        self.refresh_pointer_focus();
    }

    /// The feature the panel is on.
    fn panel_feature(&self) -> Option<&Feature> {
        let panel = self.features.panel.as_ref()?;
        self.feature(&panel.project, &panel.slug)
    }

    /// Scroll the panel's repositories to the cursor.
    fn reveal_feature_row(&mut self) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(&view, self.output_size());
        if let Some(panel) = self.features.panel.as_mut() {
            panel.reveal(view.repos.len(), fit);
        }
    }

    /// A key while the feature panel has the keyboard.
    pub fn feature_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
    ) -> Option<PanelOutcome> {
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let Some(feature) = self.feature(&panel.project, &panel.slug).cloned() else {
            return Some(PanelOutcome::Close);
        };
        let out = self
            .features
            .panel
            .as_mut()?
            .key(mods, sym, &feature, &slugs);
        self.reveal_feature_row();
        out
    }

    /// Do what the feature panel asked.
    pub fn apply_feature_panel(&mut self, outcome: PanelOutcome) {
        let Some((project, slug)) = self
            .features
            .panel
            .as_ref()
            .map(|p| (p.project.clone(), p.slug.clone()))
        else {
            return;
        };
        let now = feature::now();
        match outcome {
            PanelOutcome::Close => self.close_feature_panel(),
            PanelOutcome::Tick => match self.feature(&project, &slug).and_then(Feature::current) {
                Some(stage) if stage.bound() => self.read_and_tick(&project, &slug, stage),
                Some(_) => {
                    self.tick_feature(&project, &slug, None);
                    self.probe_feature(&project, &slug);
                }
                None => self.show_warning("Nothing left to mark done".into()),
            },
            PanelOutcome::Ship => self.ask_ship_feature(&project, &slug),
            PanelOutcome::StartAgent(stage, fresh) => {
                self.start_stage_agent(&project, &slug, stage, fresh)
            }
            PanelOutcome::ShowAgent(_) => todo!("features: show the stage's agent"),
            PanelOutcome::ReviewDoc(stage) => self.open_doc_review(&project, &slug, stage),
            PanelOutcome::OpenPr(url) => {
                self.close_feature_panel();
                let w = self.active_workspace;
                self.open_page(w, url);
            }
            PanelOutcome::Untick(stage) => {
                let Some(feature) = self.feature_mut(&project, &slug) else {
                    return;
                };
                if feature.untick(stage, now) {
                    self.save_feature(&project, &slug);
                    self.show_success(format!("{} reopened", stage.label()));
                    self.probe_feature(&project, &slug);
                } else {
                    self.show_warning(format!("{} isn't done", stage.label()));
                }
            }
            PanelOutcome::NotYet(first) => {
                self.show_warning(format!("Mark {} done first", first.label()));
            }
            PanelOutcome::Touch(repo) => self.touch_repo(&project, &slug, repo),
            PanelOutcome::Open(dir) => {
                if !dir.exists() {
                    return self.show_error(format!("{} is gone", tilde(&dir)));
                }
                self.close_feature_panel();
                let w = self.active_workspace;
                self.new_project_tab(w, &dir);
            }
            PanelOutcome::CopyPath => {
                if let Some(dir) = self.feature(&project, &slug).map(|f| f.dir.clone()) {
                    self.copy_text(dir.display().to_string());
                    self.show_success(format!("Copied {}", tilde(&dir)));
                }
            }
            PanelOutcome::Remove => self.check_remove_feature(&project, &slug),
            PanelOutcome::New => self.open_new_feature(None),
            PanelOutcome::Show(to) => {
                if let Some(feature) = self.feature(&project, &to) {
                    let cursor = start_cursor(feature);
                    self.features.shown.insert(project.clone(), to.clone());
                    if let Some(panel) = self.features.panel.as_mut() {
                        panel.slug = to.clone();
                        panel.cursor = cursor;
                        panel.repo_top = 0;
                    }
                    self.reveal_feature_row();
                    self.probe_feature(&project, &to);
                }
            }
        }
    }

    /// Put the panel's cursor on the current stage of feature `slug`,
    /// when the panel shows it.
    fn feature_cursor_to_current_of(&mut self, project: &str, slug: &str) {
        let on_it = self
            .features
            .panel
            .as_ref()
            .is_some_and(|p| p.project == project && p.slug == slug);
        let Some(cursor) = self.panel_feature().map(start_cursor) else {
            return;
        };
        if let Some(panel) = self.features.panel.as_mut().filter(|_| on_it) {
            panel.cursor = cursor;
        }
    }

    /// Make `repo` one feature `slug` changes, off the thread.
    fn touch_repo(&mut self, project: &str, slug: &str, repo: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let doing = self
            .features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo));
        if doing {
            return self.show_warning(format!("{repo} is being touched already"));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Touch,
            repo: Some(repo.clone()),
            what: format!("Touching {repo} for \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || {
            let result = layout::touch(&feature, &repo);
            Done::Touched {
                project,
                slug,
                repo,
                result,
            }
        });
    }

    fn feature_touched(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        result: Result<feature::Repo, String>,
    ) {
        let touched = match result {
            Ok(touched) => touched,
            Err(why) => {
                tracing::warn!("touching {repo} for feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't touch {repo}: {why}"));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let branch = touched.branch.clone().unwrap_or_else(|| feature.branch());
        match feature.repos.iter_mut().find(|r| r.name == repo) {
            Some(r) => *r = touched,
            None => feature.repos.push(touched),
        }
        feature.log(feature::now(), format!("touched {repo}"));
        self.save_feature(project, slug);
        self.show_success(format!("{repo} is on {branch} now"));
        self.probe_feature(project, slug);
    }

    /// Look at what removing feature `slug`'s worktrees loses, off the
    /// thread, then ask (see `ask_remove_feature`).
    fn check_remove_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        if let Some(user) = self.folder_in_use(&feature.dir) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has {user} in it; close it first",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Losses,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || Done::Losses {
            losses: layout::losses(&feature),
            project,
            slug,
        });
    }

    /// Ship feature `slug` of `project` as its last Ship check said, once
    /// the Ship sheet is confirmed: open its pull requests off the thread,
    /// or start its squash in a tab.
    pub fn ship_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let key = (project.to_owned(), slug.to_owned());
        let asked = self.features.asked.remove(&key).or_else(|| {
            let probe = self.features.probes.get(&key)?.last.as_ref();
            match ship_ask(&title, probe) {
                ShipAsk::Ready(asked) => Some(asked),
                _ => None,
            }
        });
        let Some(Asked { actions, patch }) = asked else {
            return self.show_warning(format!("Nothing to ship in \u{201c}{title}\u{201d}"));
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let (squash, prs): (Vec<Action>, Vec<Action>) = actions
            .into_iter()
            .partition(|a| matches!(a, Action::Squash { .. }));
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: None,
            what: format!("Shipping \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // The code is read again first: a commit since the Ship check
        // would ship what no gate saw.
        self.feature_work(Some(busy), move || {
            let result = still_checked(&feature, patch.as_deref())
                .map_err(|why| (why, Vec::new()))
                .and_then(|()| {
                    if prs.is_empty() {
                        Ok(Vec::new())
                    } else {
                        ship::open_prs(&feature, &prs, "gh")
                    }
                });
            Done::Shipped {
                result,
                project,
                slug,
                patch,
                squash,
            }
        });
    }

    /// Start the squash of feature `slug` (`squash`, its Squash steps) in a
    /// tab: done once its branches read as merged.
    fn start_squash(&mut self, project: &str, slug: &str, squash: &[Action]) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let (dir, command) = ship::squash_command(feature, squash);
        if command.trim().is_empty() {
            tracing::warn!("shipping feature {project}/{slug}: no squash command");
            return self.show_error(format!(
                "Couldn't ship \u{201c}{title}\u{201d}: no squash command for it"
            ));
        }
        if !dir.exists() {
            return self.show_error(format!(
                "Couldn't ship \u{201c}{title}\u{201d}: {} is gone",
                tilde(&dir)
            ));
        }
        self.close_feature_panel();
        let w = self.active_workspace;
        self.new_tab_running(w, &dir, command);
        if let Some(feature) = self.feature_mut(project, slug) {
            let now = feature::now();
            feature.shipping.squash_at = Some(now);
            feature.log(now, "Squash started");
        }
        self.save_feature(project, slug);
    }

    /// Ship was asked for on feature `slug`: say what stops it, or ask
    /// with what shipping does, as its latest Ship check has it.
    fn ask_ship_feature(&mut self, project: &str, slug: &str) {
        let Some((title, branch)) = self
            .feature(project, slug)
            .map(|f| (f.title.clone(), f.branch()))
        else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let key = (project.to_owned(), slug.to_owned());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        match ship_ask(&title, probe) {
            ShipAsk::Checking => {
                self.show_warning(format!(
                    "Still checking \u{201c}{title}\u{201d}; try again in a moment"
                ));
                self.probe_feature(project, slug);
            }
            ShipAsk::Blocked(why) => self.show_warning(why),
            ShipAsk::Nothing => {
                self.show_warning(format!("Nothing to ship in \u{201c}{title}\u{201d}"));
            }
            ShipAsk::Ready(asked) => {
                let steps: Vec<String> = asked.actions.iter().map(Action::label).collect();
                self.features.asked.insert(key, asked);
                self.ask_ship(project.to_owned(), slug.to_owned(), &title, &branch, &steps);
            }
        }
    }

    /// The pull requests of feature `slug` are open, or some are and the
    /// rest failed: they are kept, and once all are open, Ship is done
    /// (on the code whose patch-id is `patch`), unless a squash goes on.
    fn feature_shipped(
        &mut self,
        project: &str,
        slug: &str,
        patch: Option<&str>,
        squash: &[Action],
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
    ) {
        if result.is_ok() && !squash.is_empty() {
            self.start_squash(project, slug, squash);
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let now = feature::now();
        let (prs, failed) = match result {
            Ok(prs) => (prs, None),
            Err((why, prs)) => (prs, Some(why)),
        };
        let n = prs.len();
        // Those it had open already, the push updated.
        let updated = prs
            .iter()
            .filter(|pr| feature.shipping.prs.contains(pr))
            .count();
        for pr in prs {
            match feature.shipping.prs.iter_mut().find(|p| p.repo == pr.repo) {
                Some(old) => *old = pr,
                None => feature.shipping.prs.push(pr),
            }
        }
        if let Some(why) = failed {
            feature.log(now, format!("Shipping stopped: {why}"));
            self.save_feature(project, slug);
            tracing::warn!("shipping feature {project}/{slug}: {why}");
            self.show_error(format!("Couldn't ship \u{201c}{title}\u{201d}: {why}"));
            return self.probe_feature(project, slug);
        }
        if n == 0 {
            return self.probe_feature(project, slug);
        }
        let text = opened_text(&title, n - updated, updated);
        feature.log(now, text.clone());
        if squash.is_empty()
            && feature.status == Status::Open
            && feature.current() == Some(Stage::Ship)
        {
            feature.tick(now, "gerdr", patch);
        }
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_success(text);
        self.probe_feature(project, slug);
    }

    /// Ask before removing feature `slug`'s worktrees, naming what goes
    /// with them.
    fn ask_remove_feature(&mut self, project: String, slug: String, losses: Vec<String>) {
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let title = feature.title.clone();
        let abandon = feature.status != Status::Done;
        let (question, go) = if abandon {
            (format!("Abandon \u{201c}{title}\u{201d}?"), "Abandon")
        } else {
            (
                format!("Remove the worktrees of \u{201c}{title}\u{201d}?"),
                "Remove worktrees",
            )
        };
        let body = removal_body(&losses);
        let force = !losses.is_empty();
        let then = crate::confirm::Then::RemoveFeature {
            project,
            slug,
            abandon,
            force,
        };
        self.ask_remove_worktrees(question, body, go, then);
    }

    /// Remove feature `slug`'s worktrees off the thread (`force`: with
    /// what they lose), then mark it abandoned if `abandon`.
    pub fn remove_feature(&mut self, project: &str, slug: &str, abandon: bool, force: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Remove,
            repo: None,
            what: format!(
                "Removing the worktrees of \u{201c}{}\u{201d}",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move || Done::Removed {
            result: layout::remove(&feature, force),
            project,
            slug,
            abandon,
        });
    }

    fn feature_removed(
        &mut self,
        project: &str,
        slug: &str,
        abandon: bool,
        result: Result<(), String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("removing the worktrees of feature {project}/{slug}: {why}");
            return self.show_error(format!(
                "Couldn't remove the worktrees of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        let now = feature::now();
        if abandon {
            feature.status = Status::Abandoned;
            feature.log(now, "abandoned");
        } else {
            feature.log(now, "worktrees removed");
        }
        self.save_feature(project, slug);
        // The panel moves on to another feature, or closes.
        let on_it = self
            .features
            .panel
            .as_ref()
            .is_some_and(|p| p.project == project && p.slug == slug);
        if on_it {
            match self.listed_slugs(project).into_iter().find(|s| s != slug) {
                Some(next) => self.apply_feature_panel(PanelOutcome::Show(next)),
                None => self.close_feature_panel(),
            }
        }
        self.show_success(if abandon {
            format!("Abandoned \u{201c}{title}\u{201d}")
        } else {
            format!("Removed the worktrees of \u{201c}{title}\u{201d}")
        });
    }

    /// A left click while the feature panel is open: off it closes it.
    pub fn feature_panel_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let Some(feature) = self.panel_feature().cloned() else {
            return;
        };
        let layout = chrome::feature_panel_layout(&view, self.output_size());
        let rows = panel_rows(&feature);
        let repos = feature.repos.len();
        let stage = |i: usize| PanelRow::Stage(Stage::ALL[i.min(Stage::ALL.len() - 1)]);
        let outcome = match layout.hit(view.repo_top, pos) {
            None => Some(PanelOutcome::Close),
            Some(FeatureHit::Panel) => None,
            Some(FeatureHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i).cloned() {
                    self.press_hint(crate::input::Overlay::Features, &keys, pos);
                }
                None
            }
            Some(FeatureHit::Prev | FeatureHit::Next) => {
                let slugs = self.listed_slugs(&feature.project);
                let back = layout.hit(view.repo_top, pos) == Some(FeatureHit::Prev);
                switch(&slugs, &feature.slug, back).map(PanelOutcome::Show)
            }
            Some(FeatureHit::Repo(i) | FeatureHit::Stage(i)) => {
                let at = match layout.hit(view.repo_top, pos) {
                    Some(FeatureHit::Stage(_)) => repos + i,
                    _ => i,
                };
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.cursor = at.min(rows.len().saturating_sub(1));
                }
                None
            }
            Some(FeatureHit::RepoButton(i)) => repo_button(&feature, i),
            Some(FeatureHit::StageButton(i)) => row_action(&feature, stage(i)),
            Some(FeatureHit::Button(0)) => Some(PanelOutcome::Open(feature.dir.clone())),
            Some(FeatureHit::Button(1)) => Some(PanelOutcome::CopyPath),
            Some(FeatureHit::Button(2)) => Some(PanelOutcome::New),
            Some(FeatureHit::Button(_)) => Some(PanelOutcome::Remove),
        };
        if let Some(outcome) = outcome {
            self.apply_feature_panel(outcome);
        }
    }

    /// The pointer moved while the feature panel is open.
    pub fn feature_panel_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = self.feature_panel_view().and_then(|view| {
            chrome::feature_panel_layout(&view, self.output_size()).hit(view.repo_top, pos)
        });
        if let Some(panel) = self.features.panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel over the feature panel: its repositories `by` rows on.
    pub fn feature_panel_scroll(&mut self, by: isize) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(&view, self.output_size());
        if let Some(panel) = self.features.panel.as_mut() {
            let most = view.repos.len().saturating_sub(fit);
            panel.repo_top = panel.repo_top.saturating_add_signed(by).min(most);
        }
    }

    /// The panel's repository rows shown, for the wheel's pitch.
    pub fn feature_repo_rows(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.output_size())
                .repos
                .into_iter()
                .map(|(row, _)| row)
                .collect()
        })
    }

    /// What the pointer can press in the feature panel.
    pub fn feature_panel_controls(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.output_size()).controls()
        })
    }

    pub fn feature_panel_view(&self) -> Option<FeaturePanelView> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let slugs = self.listed_slugs(&panel.project);
        let mine = || {
            self.features
                .busy
                .iter()
                .filter(|b| b.project == panel.project && b.slug == panel.slug)
        };
        let busy: Vec<&str> = mine().filter_map(|b| b.repo.as_deref()).collect();
        let shipping = mine().any(|b| b.job == Job::Ship);
        let key = (panel.project.clone(), panel.slug.clone());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        let state = PanelState {
            busy: &busy,
            probe,
            shipping,
        };
        Some(panel_view(panel, feature, &slugs, &state, feature::now()))
    }

    /// The sidebar's rows for the shown project's open features.
    pub fn feature_rows(&self) -> Vec<chrome::FeatureRowView> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let Some(all) = self.features.all.get(project) else {
            return Vec::new();
        };
        sidebar_features(all)
            .into_iter()
            .map(|f| chrome::FeatureRowView {
                title: f.title.clone(),
                marks: Stage::ALL.iter().map(|s| stage_mark(f, *s)).collect(),
            })
            .collect()
    }

    /// How many feature rows the sidebar shows.
    pub fn sidebar_feature_count(&self) -> usize {
        let all = self.project.as_ref().and_then(|p| self.features.all.get(p));
        all.map_or(0, |all| sidebar_features(all).len())
    }

    /// Feature `i` of the sidebar's rows.
    pub fn sidebar_feature(&self, i: usize) -> Option<&Feature> {
        let all = self.features.all.get(self.project.as_ref()?)?;
        sidebar_features(all).get(i).copied()
    }
}

/// What the toast says once feature `title` is made under `root`, naming
/// the files `.features` was added to (`ignores`) from the root: whole
/// paths are long enough to cut the last ones off.
fn created_text(title: &str, root: &Path, ignores: &[PathBuf]) -> String {
    if ignores.is_empty() {
        return format!("Created \u{201c}{title}\u{201d}");
    }
    let names: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| tilde(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "Created \u{201c}{title}\u{201d} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        names.join(", ")
    )
}

/// Whether `feature`'s code is still the code whose patch-id is `patch`,
/// the one its Ship check was on, with nothing uncommitted; else why not.
/// Runs git: off the UI thread.
fn still_checked(feature: &Feature, patch: Option<&str>) -> Result<(), String> {
    let reading = patch::read(feature)?;
    if !reading.dirty.is_empty() {
        return Err(format!(
            "{} has uncommitted changes",
            reading.dirty.join(", ")
        ));
    }
    if patch != Some(reading.patch.as_str()) {
        return Err("its code changed since the check".into());
    }
    Ok(())
}

/// What a confirm dialog about removing a feature's worktrees says of
/// `losses`, what goes with them.
fn removal_body(losses: &[String]) -> String {
    match losses {
        [] => "Its worktrees go; nothing in them is lost".into(),
        [one] => format!("Its worktrees go, and with them {one}"),
        more => format!("Its worktrees go, and with them {}", more.join(", ")),
    }
}

/// What the new feature dialog shows of `dialog`, in project `project`.
fn new_feature_view(dialog: &NewFeatureDialog, project: String) -> NewFeatureView {
    let (root, root_failed, repos) = match &dialog.root {
        Finding::Looking => ("finding repositories\u{2026}".to_owned(), false, Vec::new()),
        Finding::Failed(why) => (format!("No repositories found: {why}"), true, Vec::new()),
        Finding::Found(root) => {
            let n = root.repos.len();
            let line = if root.multi {
                format!("In {} \u{b7} {n} repositories", tilde(&root.path))
            } else {
                format!("In {}", tilde(&root.path))
            };
            let repos = root
                .repos
                .iter()
                .zip(&dialog.touched)
                .map(|(r, t)| NewFeatureRepo {
                    name: r.name.clone(),
                    base: r.base.clone(),
                    touched: *t,
                    fixed: !root.multi,
                })
                .collect();
            (line, false, repos)
        }
    };
    let mut keys = if dialog.multi() {
        vec![("Tab", "next"), ("Space", "tick")]
    } else {
        Vec::new()
    };
    // Esc steps back as `NewFeatureDialog::key` has it.
    keys.push(if dialog.focus.is_some() {
        ("Esc", "back")
    } else if !dialog.title.is_empty() {
        ("Esc", "clear")
    } else {
        ("Esc", "close")
    });
    NewFeatureView {
        project,
        title: dialog.title.line(),
        focus: dialog.focus,
        root,
        root_failed,
        repos,
        top: dialog.top,
        error: dialog.error.clone(),
        hover: dialog.hover,
        keys,
    }
}

/// What the feature panel shows of a feature besides the feature itself.
#[derive(Default)]
struct PanelState<'a> {
    /// The repositories being touched.
    busy: &'a [&'a str],
    /// Its latest probe.
    probe: Option<&'a Probe>,
    /// Its pull requests are being opened.
    shipping: bool,
}

/// The line under repository `repo` of `feature`: its branch, its pull
/// request, and a push not guarded as `probe` found; for one it doesn't
/// change, its base.
fn repo_line(feature: &Feature, repo: &feature::Repo, probe: Option<&Probe>) -> String {
    if !repo.touched {
        return format!("context, at {}", repo.base);
    }
    let mut line = repo.branch.clone().unwrap_or_else(|| feature.branch());
    if let Some(pr) = pr_of(feature, &repo.name) {
        line.push_str(&format!(" \u{b7} PR {}", pr_tail(&pr.url)));
    }
    if let Some(Guard::Off(why)) = probe.and_then(|p| p.guards.get(&repo.name)) {
        line.push_str(&format!(" \u{b7} push not guarded: {why}"));
    }
    line
}

/// What the feature panel shows of `feature`, one of `slugs`, as `state`
/// has it.
fn panel_view(
    panel: &FeaturePanel,
    feature: &Feature,
    slugs: &[String],
    state: &PanelState,
    now: u64,
) -> FeaturePanelView {
    let repos = feature
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| FeatureRepoRow {
            name: r.name.clone(),
            line: repo_line(feature, r, state.probe),
            touched: r.touched,
            busy: state.busy.contains(&r.name.as_str()),
            button: match repo_button(feature, i) {
                Some(PanelOutcome::Touch(_)) => Some("Touch"),
                Some(PanelOutcome::OpenPr(_)) => Some("Open PR"),
                _ => None,
            },
        })
        .collect();
    let stages = Stage::ALL
        .iter()
        .map(|s| {
            let mark = stage_mark(feature, *s);
            StageRow {
                label: s.label().to_owned(),
                detail: match feature.marks.get(s) {
                    Some(m) => mark_detail(m, now),
                    None => s.about().to_owned(),
                },
                mark,
                button: stage_button(feature, *s),
            }
        })
        .collect();
    let rows = panel_rows(feature);
    let row = rows[panel.cursor.min(rows.len() - 1)];
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    let mut hints = vec![hint("Up / Down", "move")];
    let enter = row_hint(feature, row);
    if !enter.is_empty() {
        hints.push(hint("Enter", enter));
    }
    if let PanelRow::Repo(i) = row
        && let Some(PanelOutcome::OpenPr(_)) = repo_button(feature, i)
    {
        hints.push(hint("Ctrl+Shift+O", "open PR"));
    }
    if slugs.len() > 1 {
        hints.push(hint("Left / Right", "other feature"));
    }
    hints.extend([
        hint("Ctrl+O", "open tab"),
        hint("Ctrl+C", "copy path"),
        hint("Ctrl+N", "new"),
        hint("Del", &remove_label(feature).to_lowercase()),
        hint("Esc", "close"),
    ]);
    let which = slugs
        .iter()
        .position(|s| *s == feature.slug)
        .map(|i| (i, slugs.len()));
    let mut sub = format!("{} \u{b7} {}", feature.slug, tilde(&feature.dir));
    if feature.status == Status::Done {
        sub = format!("done \u{b7} {sub}");
    }
    FeaturePanelView {
        title: feature.title.clone(),
        sub,
        which,
        repos,
        stages,
        cursor: panel.cursor,
        repo_top: panel.repo_top,
        remove: remove_label(feature),
        hover: panel.hover,
        hints,
        check: check_lines(feature, state.probe, state.shipping, now),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::feature::{Mark, Repo, ship::ShipCheck};
    use layout::Found;

    pub(crate) fn repo(name: &str, touched: bool) -> Repo {
        Repo {
            name: name.into(),
            main: PathBuf::from(format!("/work/{name}")),
            base: "main".into(),
            base_commit: "abc123".into(),
            worktree: PathBuf::from(format!("/work/.features/bulk/{name}")),
            touched,
            branch: touched.then(|| "feature/bulk".into()),
        }
    }

    /// A feature over `repos`, its first `done` stages marked.
    pub(crate) fn feature(slug: &str, repos: Vec<Repo>, done: usize) -> Feature {
        Feature {
            version: feature::VERSION,
            slug: slug.into(),
            title: format!("Feature {slug}"),
            project: "acme".into(),
            created: 1_000,
            root: "/work".into(),
            dir: format!("/work/.features/{slug}").into(),
            multi: repos.len() > 1,
            repos,
            status: Status::Open,
            marks: Stage::ALL
                .iter()
                .take(done)
                .map(|s| {
                    let mark = Mark {
                        at: 2_000,
                        by: "you".into(),
                        note: String::new(),
                        patch: None,
                    };
                    (*s, mark)
                })
                .collect(),
            history: Vec::new(),
            shipping: Default::default(),
            runs: Default::default(),
        }
    }

    fn key(sym: Keysym) -> (ModifiersState, Keysym) {
        (ModifiersState::default(), sym)
    }

    fn ctrl(sym: Keysym) -> (ModifiersState, Keysym) {
        let mods = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        (mods, sym)
    }

    fn press(
        panel: &mut FeaturePanel,
        f: &Feature,
        (mods, sym): (ModifiersState, Keysym),
    ) -> Option<PanelOutcome> {
        let slugs = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        panel.key(&mods, sym, f, &slugs)
    }

    fn root(names: &[&str]) -> Root {
        Root {
            path: "/work".into(),
            multi: names.len() > 1,
            repos: names
                .iter()
                .map(|n| Found {
                    name: (*n).into(),
                    main: PathBuf::from(format!("/work/{n}")),
                    base: "main".into(),
                })
                .collect(),
        }
    }

    fn type_into(d: &mut NewFeatureDialog, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            d.key(&ModifiersState::default(), sym, sym, 8);
        }
    }

    fn dkey(
        d: &mut NewFeatureDialog,
        (mods, sym): (ModifiersState, Keysym),
    ) -> Option<DialogOutcome> {
        d.key(&mods, sym, sym, 8)
    }

    #[test]
    fn esc_goes_back_to_the_title_then_clears_it_then_closes() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
        d.found(Ok(root(&["api", "web"])));
        type_into(&mut d, "Bulk");
        assert_eq!(&*d.title, "Bulk");
        dkey(&mut d, key(Keysym::Tab));
        assert_eq!(d.focus, Some(0));
        assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
        assert_eq!(d.focus, None);
        assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
        assert!(d.title.is_empty());
        assert_eq!(
            dkey(&mut d, key(Keysym::Escape)),
            Some(DialogOutcome::Cancel)
        );
    }

    #[test]
    fn enter_creates_with_the_ticked_repositories_or_says_what_is_missing() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api/src".into());
        assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
        assert_eq!(d.error.as_deref(), Some("Give the feature a title."));
        type_into(&mut d, "Bulk export");
        // Typing clears the error; still looking, Enter says so.
        dkey(&mut d, key(Keysym::Return));
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Still finding"))
        );
        d.found(Ok(root(&["api", "web"])));
        // The repository it was opened from is ticked.
        assert_eq!(d.touched, [true, false]);
        // Space ticks the focused one; in the title it types.
        dkey(&mut d, key(Keysym::Down));
        dkey(&mut d, key(Keysym::Down));
        assert_eq!(d.focus, Some(1));
        dkey(&mut d, key(Keysym::space));
        dkey(&mut d, ctrl(Keysym::k));
        dkey(&mut d, key(Keysym::space));
        assert_eq!(d.touched, [false, true]);
        let out = dkey(&mut d, key(Keysym::Return));
        assert_eq!(
            out,
            Some(DialogOutcome::Create {
                title: "Bulk export".into(),
                root: root(&["api", "web"]),
                touched: vec!["web".into()],
            })
        );
        dkey(&mut d, key(Keysym::Down));
        dkey(&mut d, key(Keysym::space));
        assert_eq!(d.touched, [false, false]);
        assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Tick at least"))
        );
        // Shift+Tab from the title goes round to the last repository.
        d.focus = None;
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        dkey(&mut d, (shift, Keysym::ISO_Left_Tab));
        assert_eq!(d.focus, Some(1));
    }

    #[test]
    fn a_single_repository_is_ticked_and_fixed() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, "/elsewhere".into());
        d.found(Ok(root(&["api"])));
        assert_eq!(d.touched, [true]);
        dkey(&mut d, key(Keysym::Tab));
        assert_eq!(d.focus, None, "nothing to move to");
        d.toggle(0);
        assert_eq!(d.touched, [true]);
        let view = new_feature_view(&d, "Acme".into());
        assert!(view.repos[0].fixed);
        assert_eq!(view.keys, [("Esc", "close")]);
    }

    #[test]
    fn panel_keys_ask_for_the_rows_and_the_features_actions() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 2);
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        // It starts on the current stage: Plan done, Build next.
        assert_eq!(panel.cursor, 2 + 2);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Tick)
        );
        press(&mut panel, &f, key(Keysym::Up));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Untick(Stage::Plan))
        );
        press(&mut panel, &f, ctrl(Keysym::j));
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::NotYet(Stage::Build))
        );
        press(&mut panel, &f, key(Keysym::Home));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Open("/work/.features/bulk/api".into()))
        );
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Touch("web".into()))
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::o)),
            Some(PanelOutcome::Open("/work/.features/b".into()))
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::c)),
            Some(PanelOutcome::CopyPath)
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::n)),
            Some(PanelOutcome::New)
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Left)),
            Some(PanelOutcome::Show("a".into()))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Right)),
            Some(PanelOutcome::Show("c".into()))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Escape)),
            Some(PanelOutcome::Close)
        );
        // Ctrl+N and Ctrl+P never move.
        let at = panel.cursor;
        press(&mut panel, &f, ctrl(Keysym::p));
        assert_eq!(panel.cursor, at);
    }

    #[test]
    fn the_panel_shows_repositories_then_stages_and_the_right_hints() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
        let panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["b".to_owned()];
        let state = PanelState {
            busy: &["web"],
            ..PanelState::default()
        };
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(view.repos[0].line, "feature/bulk");
        assert_eq!(view.repos[1].line, "context, at main");
        assert!(view.repos[1].busy);
        let marks: Vec<StageMark> = view.stages.iter().map(|s| s.mark).collect();
        assert_eq!(
            &marks[..4],
            [
                StageMark::Done,
                StageMark::Done,
                StageMark::Done,
                StageMark::Current
            ]
        );
        assert_eq!(view.stages[0].detail, "done 1m ago by you");
        assert_eq!(view.stages[3].detail, Stage::Verify.about());
        assert_eq!(view.which, Some((0, 1)));
        let keys: Vec<&str> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "Up / Down",
                "Enter",
                "Ctrl+O",
                "Ctrl+C",
                "Ctrl+N",
                "Del",
                "Esc"
            ]
        );
        assert_eq!(view.hints[1].1, "mark done");
        assert_eq!(view.remove, "Abandon");
    }

    fn probe(ship: Option<ShipCheck>) -> Probe {
        Probe {
            reading: Ok(Reading {
                patch: "p1".into(),
                ..Reading::default()
            }),
            guards: BTreeMap::new(),
            ship,
            merged: false,
        }
    }

    fn check(blockers: &[&str], actions: Vec<Action>, notes: &[&str]) -> Option<ShipCheck> {
        Some(ShipCheck {
            blockers: blockers.iter().map(|b| (*b).to_owned()).collect(),
            actions,
            notes: notes.iter().map(|n| (*n).to_owned()).collect(),
        })
    }

    fn texts(lines: &[CheckLine]) -> Vec<(&str, CheckTone)> {
        lines.iter().map(|l| (l.text.as_str(), l.tone)).collect()
    }

    #[test]
    fn ship_is_asked_for_not_ticked_and_bound_stages_tick_as_before() {
        let shipping = feature("s", vec![repo("api", true)], 7);
        let ship = PanelRow::Stage(Stage::Ship);
        assert_eq!(row_action(&shipping, ship), Some(PanelOutcome::Ship));
        assert_eq!(row_hint(&shipping, ship), "ship\u{2026}");
        assert_eq!(stage_button(&shipping, Stage::Ship), Some("Ship\u{2026}"));
        assert_eq!(stage_button(&shipping, Stage::Build), Some("Reopen"));
        let building = feature("b", vec![repo("api", true)], 2);
        let build = PanelRow::Stage(Stage::Build);
        assert_eq!(row_action(&building, build), Some(PanelOutcome::Tick));
        assert_eq!(row_hint(&building, build), "mark done");
        assert_eq!(stage_button(&building, Stage::Build), Some("Mark done"));
        assert_eq!(stage_button(&building, Stage::Ship), None);
        // Enter on the Ship row of the panel asks to ship.
        let mut panel = FeaturePanel {
            cursor: 1 + Stage::ALL.len() - 1,
            ..FeaturePanel::default()
        };
        let slugs = vec!["s".to_owned()];
        let (mods, sym) = key(Keysym::Return);
        assert_eq!(
            panel.key(&mods, sym, &shipping, &slugs),
            Some(PanelOutcome::Ship)
        );
    }

    #[test]
    fn a_repository_with_a_pull_request_opens_it_by_button_and_key() {
        let mut f = feature(
            "d",
            vec![repo("api", true), repo("web", true), repo("ui", false)],
            8,
        );
        f.status = Status::Done;
        let url = "https://github.com/acme/api/pull/12";
        f.shipping.prs.push(Pr {
            repo: "api".into(),
            url: url.into(),
        });
        assert_eq!(repo_button(&f, 0), Some(PanelOutcome::OpenPr(url.into())));
        assert_eq!(repo_button(&f, 1), None);
        assert_eq!(repo_button(&f, 2), Some(PanelOutcome::Touch("ui".into())));
        let slugs = vec!["d".to_owned()];
        let mut panel = FeaturePanel::default();
        let shift_ctrl = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs),
            Some(PanelOutcome::OpenPr(url.into()))
        );
        // Ctrl+O still opens a tab; on a row without a PR, nothing.
        let (mods, sym) = ctrl(Keysym::o);
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs),
            Some(PanelOutcome::Open(f.dir.clone()))
        );
        panel.cursor = 1;
        assert_eq!(panel.key(&shift_ctrl, Keysym::O, &f, &slugs), None);
        // The view: the button, the line, and the hint on that row only.
        let state = PanelState::default();
        panel.cursor = 0;
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(view.repos[0].button, Some("Open PR"));
        assert_eq!(view.repos[0].line, "feature/bulk \u{b7} PR api/pull/12");
        assert_eq!(view.repos[1].button, None);
        assert_eq!(view.repos[2].button, Some("Touch"));
        assert!(
            view.hints
                .contains(&("Ctrl+Shift+O".into(), "open PR".into()))
        );
        assert!(view.check.is_empty());
        panel.cursor = 1;
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert!(!view.hints.iter().any(|(k, _)| k == "Ctrl+Shift+O"));
    }

    #[test]
    fn a_push_not_guarded_shows_on_its_repository() {
        let f = feature("g", vec![repo("api", true), repo("web", true)], 3);
        let mut p = probe(None);
        p.guards.insert("api".into(), Guard::On);
        p.guards
            .insert("web".into(), Guard::Off("hooks path is set".into()));
        assert_eq!(repo_line(&f, &f.repos[0], Some(&p)), "feature/bulk");
        assert_eq!(
            repo_line(&f, &f.repos[1], Some(&p)),
            "feature/bulk \u{b7} push not guarded: hooks path is set"
        );
    }

    #[test]
    fn the_ship_check_shows_what_stops_it_or_that_it_is_ready() {
        let f = feature("s", vec![repo("api", true)], 7);
        let none = check_lines(&f, None, false, 2_060);
        assert_eq!(texts(&none), [("checking\u{2026}", CheckTone::Note)]);
        let blocked = probe(check(&["api is dirty", "web is behind"], Vec::new(), &[]));
        assert_eq!(
            texts(&check_lines(&f, Some(&blocked), false, 2_060)),
            [
                ("api is dirty", CheckTone::Blocker),
                ("web is behind", CheckTone::Blocker)
            ]
        );
        let many = probe(check(&["a", "b", "c", "d", "e", "f"], Vec::new(), &[]));
        let lines = check_lines(&f, Some(&many), false, 2_060);
        assert_eq!(lines.len(), MAX_CHECK_LINES);
        assert_eq!(lines[MAX_CHECK_LINES - 1].text, "and 3 more");
        let push = Action::Push { repo: "api".into() };
        let ready = probe(check(&[], vec![push.clone()], &["api pushes to origin"]));
        assert_eq!(
            texts(&check_lines(&f, Some(&ready), false, 2_060)),
            [
                ("Ready to ship", CheckTone::Ready),
                ("api pushes to origin", CheckTone::Note)
            ]
        );
        let unread = Probe {
            reading: Err("no git".into()),
            ..probe(None)
        };
        assert_eq!(
            texts(&check_lines(&f, Some(&unread), false, 2_060)),
            [("Couldn't read the code: no git", CheckTone::Blocker)]
        );
        assert_eq!(
            texts(&check_lines(&f, Some(&ready), true, 2_060)),
            [("shipping\u{2026}", CheckTone::Note)]
        );
        let mut squashing = f.clone();
        squashing.shipping.squash_at = Some(2_000);
        let lines = check_lines(&squashing, Some(&ready), false, 2_060);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.starts_with("Squash started"));
        // Only while Ship is the current stage.
        let building = feature("b", vec![repo("api", true)], 2);
        assert!(check_lines(&building, Some(&ready), false, 2_060).is_empty());
    }

    #[test]
    fn shipping_asks_with_the_last_check_or_says_why_not() {
        assert_eq!(ship_ask("T", None), ShipAsk::Checking);
        assert_eq!(ship_ask("T", Some(&probe(None))), ShipAsk::Checking);
        let blocked = probe(check(&["a", "b", "c"], Vec::new(), &[]));
        assert_eq!(
            ship_ask("T", Some(&blocked)),
            ShipAsk::Blocked("Can't ship \u{201c}T\u{201d} yet: a, and 2 more".into())
        );
        let one = probe(check(&["a"], Vec::new(), &[]));
        assert_eq!(
            ship_ask("T", Some(&one)),
            ShipAsk::Blocked("Can't ship \u{201c}T\u{201d} yet: a".into())
        );
        assert_eq!(
            ship_ask("T", Some(&probe(check(&[], Vec::new(), &[])))),
            ShipAsk::Nothing
        );
        let actions = vec![Action::Push { repo: "api".into() }, Action::LinkPrs];
        assert_eq!(
            ship_ask("T", Some(&probe(check(&[], actions.clone(), &[])))),
            ShipAsk::Ready(Asked {
                actions,
                patch: Some("p1".into())
            })
        );
    }

    #[test]
    fn the_toasts_name_the_feature_and_what_happened() {
        assert_eq!(
            changed_text("Bulk", Stage::Build),
            "\u{201c}Bulk\u{201d} changed: back to Build"
        );
        assert_eq!(
            opened_text("Bulk", 1, 0),
            "Opened 1 pull request for \u{201c}Bulk\u{201d}"
        );
        assert_eq!(
            opened_text("Bulk", 2, 0),
            "Opened 2 pull requests for \u{201c}Bulk\u{201d}"
        );
        assert_eq!(
            opened_text("Bulk", 0, 2),
            "Updated 2 pull requests for \u{201c}Bulk\u{201d}"
        );
        assert_eq!(
            opened_text("Bulk", 1, 1),
            "Opened 1 and updated 1 pull requests for \u{201c}Bulk\u{201d}"
        );
        assert_eq!(
            pr_tail("https://github.com/acme/api/pull/12/"),
            "api/pull/12"
        );
        assert_eq!(pr_tail("12"), "12");
    }

    #[test]
    fn the_watch_looks_at_open_features_bound_to_their_code() {
        assert!(!watched(&feature("a", vec![repo("api", true)], 2)));
        assert!(watched(&feature("b", vec![repo("api", true)], 3)));
        let mut squashing = feature("c", vec![repo("api", true)], 1);
        squashing.shipping.squash_at = Some(1);
        assert!(watched(&squashing));
        assert_eq!(watch_every(&squashing), PANEL_EVERY);
        assert_eq!(
            watch_every(&feature("b", vec![repo("api", true)], 3)),
            WATCH_EVERY
        );
        let mut done = feature("d", vec![repo("api", true)], 8);
        done.status = Status::Done;
        assert!(!watched(&done));
    }

    #[test]
    fn sidebar_and_panel_list_the_right_features() {
        let mut open = feature("a", vec![repo("api", true)], 0);
        open.title = "Open".into();
        let mut abandoned = feature("b", vec![repo("api", true)], 1);
        abandoned.status = Status::Abandoned;
        let mut done = feature("c", vec![repo("api", true)], 8);
        done.status = Status::Done;
        let all = vec![open, abandoned, done];
        let shown: Vec<&str> = sidebar_features(&all)
            .iter()
            .map(|f| f.slug.as_str())
            .collect();
        assert_eq!(shown, ["a"]);
        // A done feature's folder is gone here: nothing to remove.
        let listed: Vec<&str> = listed(&all).iter().map(|f| f.slug.as_str()).collect();
        assert_eq!(listed, ["a"]);
        assert_eq!(remove_label(&all[2]), "Remove worktrees");
    }

    #[test]
    fn slugs_are_unique_in_the_project() {
        let all = vec![
            feature("bulk-export", vec![], 0),
            feature("bulk-export-2", vec![], 0),
        ];
        assert_eq!(new_slug("Bulk export!", &all), "bulk-export-3");
        assert_eq!(new_slug("Other thing", &all), "other-thing");
    }

    #[test]
    fn the_features_key_shows_the_open_then_the_last_shown_then_the_newest() {
        let slugs = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(pick_feature(&slugs, Some("a"), Some("b")), Some("a".into()));
        assert_eq!(pick_feature(&slugs, None, Some("a")), Some("a".into()));
        // One gone (abandoned) is passed over.
        assert_eq!(pick_feature(&slugs, None, Some("gone")), Some("b".into()));
        assert_eq!(pick_feature(&[], None, Some("a")), None);
    }

    #[test]
    fn the_created_toast_names_the_ignore_files_from_the_root() {
        let root = Path::new("/w/multi");
        let ignores = vec![
            root.join(".cursorignore"),
            root.join("pyrightconfig.json"),
            PathBuf::from("/elsewhere/.git/info/exclude"),
        ];
        assert_eq!(
            created_text("Bulk export", root, &ignores),
            "Created \u{201c}Bulk export\u{201d} \u{b7} added .features to .cursorignore, \
             pyrightconfig.json, /elsewhere/.git/info/exclude"
        );
        assert_eq!(created_text("X", root, &[]), "Created \u{201c}X\u{201d}");
    }

    #[test]
    fn removing_names_what_is_lost() {
        assert_eq!(
            removal_body(&[]),
            "Its worktrees go; nothing in them is lost"
        );
        assert_eq!(
            removal_body(&["2 changed files in api".into()]),
            "Its worktrees go, and with them 2 changed files in api"
        );
    }

    #[test]
    fn switching_goes_round_and_needs_two() {
        let slugs: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(switch(&slugs, "b", false).as_deref(), Some("a"));
        assert_eq!(switch(&slugs, "a", true).as_deref(), Some("b"));
        assert_eq!(switch(&slugs[..1], "a", true), None);
    }
}
