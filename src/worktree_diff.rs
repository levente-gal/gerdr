//! A worktree's diff, from the worktrees popup (Enter, or Ctrl+D): every
//! change of the worktree since it forked from what its branch merges
//! into, its commits and what isn't committed yet, in the repositories
//! panel's diff view (`diff_view`), alone across the panel. A main
//! worktree's is what isn't committed. Its keys, clicks, comments, search
//! and vim's edits are the diff's own; Esc closes it, back to the popup.
//! It is read again with every Git status read. Its review comments are
//! saved by worktree (see `review::Worktrees`), and go with it when it is
//! deleted.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{Density, DetailView, Hint, ReposButton, ReposHit, SheetView},
    diff::FileDiff,
    diff_view::{self, DiffView, Keys, Section},
    git_ops::Since,
    repos::{diff_help_lines, help_title, key_name, scroll_lines, sheet_step},
    review::Review,
    scroll::Motion,
};

/// The worktree's diff while open.
#[derive(Debug, Default)]
pub struct WorktreeDiff {
    /// The worktree's top folder.
    pub root: PathBuf,
    /// Its repository's name, on the diff's header.
    pub label: String,
    pub branch: Option<String>,
    /// What its branch merges into; none for a main worktree, diffed
    /// against its HEAD.
    pub target: Option<String>,
    /// The workspace a Zed tab on it opens in.
    pub workspace: usize,
    /// The workspace whose checkout it is, whose Zed its files open in;
    /// none, and they open in a Zed tab on it.
    pub home: Option<usize>,
    /// What it changed since it forked, as last read.
    pub since: Since,
    /// Read once at least; and why the last read failed.
    pub read: bool,
    pub error: Option<String>,
    /// A read is under way; and another is wanted once it is in, as a
    /// file was edited from the diff meanwhile.
    pub reading: bool,
    pub again: bool,
    pub diff: DiffView,
    pub hover: Option<ReposHit>,
    /// The sheet asking whether to delete the review, while open: the
    /// button Enter presses, by index into `CLEAR_BUTTONS`.
    pub clearing: Option<usize>,
    /// That sheet deletes only the comments on lines no longer in the diff.
    pub clearing_outdated: bool,
    /// The keys sheet (F1), while open: the line it is scrolled to.
    pub help: Option<usize>,
}

/// The buttons of the sheet asking whether to delete the review (only the
/// comments on lines gone, when `outdated`): Cancel, which Enter presses
/// first, on the left.
fn clear_buttons(outdated: bool) -> [ReposButton; 2] {
    match outdated {
        true => [ReposButton::Cancel, ReposButton::DropOutdated],
        false => [ReposButton::Cancel, ReposButton::ClearReview],
    }
}

/// What the diff's views and keys need of the chrome.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    /// How many rows fit (`chrome::diff_only_page`).
    pub page: usize,
    /// How many characters a comment's row holds
    /// (`chrome::diff_only_cols`).
    pub cols: usize,
    pub density: Density,
    /// How many lines a sheet shows (`chrome::repos_sheet_rows`).
    pub sheet_rows: usize,
}

/// What a key or a click asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    Close,
    Diff(diff_view::Outcome),
}

impl WorktreeDiff {
    /// The diff of the worktree at `root`, of repository `label`, on
    /// `branch`, against `target`; its review comments `review`, kept
    /// from when it was shown last.
    pub fn new(
        root: PathBuf,
        label: String,
        branch: Option<String>,
        target: Option<String>,
        workspace: usize,
        review: Review,
    ) -> WorktreeDiff {
        let mut diff = DiffView::default();
        diff.review = review;
        WorktreeDiff {
            root,
            label,
            branch,
            target,
            workspace,
            diff,
            ..WorktreeDiff::default()
        }
    }

    /// Call `f` with the diff and its `Ctx`.
    fn with_diff<R>(&mut self, env: Env, f: impl FnOnce(&mut DiffView, &diff_view::Ctx) -> R) -> R {
        let lead = self.lead();
        let empty = self.empty();
        let WorktreeDiff {
            root,
            label,
            branch,
            since,
            diff,
            ..
        } = self;
        let sections = [Section {
            root,
            label,
            branch: branch.as_deref(),
            head: None,
            changes: &since.changes,
        }];
        f(diff, &ctx(&sections, env, &lead, empty))
    }

    /// The diff's one section: the worktree.
    fn sections(&self) -> [Section<'_>; 1] {
        [Section {
            root: &self.root,
            label: &self.label,
            branch: self.branch.as_deref(),
            head: None,
            changes: &self.since.changes,
        }]
    }

    /// Over the diff, before its counts: what it is against.
    fn lead(&self) -> String {
        match &self.target {
            Some(target) => format!("Since {target}"),
            None => "Not committed".into(),
        }
    }

    /// What the diff says with nothing to show.
    fn empty(&self) -> &'static str {
        match (&self.error, self.read, &self.target) {
            (Some(_), ..) => "Its changes couldn't be read",
            (None, false, _) => "Reading…",
            (None, true, Some(_)) => "No changes since it forked",
            (None, true, None) => "No changes",
        }
    }

    /// The panel's heading: whose diff it is.
    pub fn heading(&self) -> String {
        let name = self.branch.clone().unwrap_or_else(|| {
            let dir = self.root.file_name().unwrap_or(self.root.as_os_str());
            dir.to_string_lossy().into()
        });
        format!("Worktree · {name}")
    }

    /// After the heading: its commits since it forked, or why it can't be
    /// read.
    pub fn summary(&self) -> String {
        if let Some(why) = &self.error {
            return why.clone();
        }
        if !self.read {
            return "reading…".into();
        }
        match (&self.target, self.since.commits) {
            (None, _) => "uncommitted changes".into(),
            (Some(target), 0) => format!("no commits of its own on {target}"),
            (Some(target), 1) => format!("1 commit on {target}"),
            (Some(target), n) => format!("{n} commits on {target}"),
        }
    }

    /// The diff, as the panel's detail shows it.
    pub fn view(&self, env: Env) -> DetailView {
        let sections = self.sections();
        let lead = self.lead();
        self.diff.view(&ctx(&sections, env, &lead, self.empty()))
    }

    /// Its changes read again (`git_ops::changes_since`), with their diff:
    /// shown where it stood.
    pub fn set_read(&mut self, read: Result<(Since, Vec<FileDiff>), String>, env: Env) {
        self.reading = false;
        let (since, files) = match read {
            Ok(read) => read,
            Err(why) => {
                self.error = Some(why);
                return;
            }
        };
        self.error = None;
        self.read = true;
        self.since = since;
        let root = self.root.clone();
        self.with_diff(env, |diff, ctx| {
            diff.refresh(ctx);
            let reads = diff.reads;
            diff.set_diffs(root, reads, files, ctx);
        });
    }

    /// A key; `None` for none of the diff's, Esc closing it. The sheet
    /// asking whether to delete the review takes them all while open.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        env: Env,
    ) -> Option<Done> {
        let buttons = clear_buttons(self.clearing_outdated);
        if let Some(choice) = self.clearing.as_mut() {
            let button = sheet_step(choice, &buttons, mods, sym)?;
            self.sheet_press(button, env);
            return None;
        }
        // The keys sheet scrolls; any other key closes it.
        if let Some(top) = self.help.as_mut() {
            let len = diff_help_lines().len();
            if !scroll_lines(top, len, env.sheet_rows, sym) {
                self.help = None;
            }
            return None;
        }
        let plain = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        if sym == Keysym::F1 && plain {
            self.help = Some(0);
            return None;
        }
        let outcome = self.with_diff(env, |diff, ctx| diff.key(mods, sym, typed, ctx));
        let done = match outcome {
            Some(outcome) => outcome.map(Done::Diff),
            None => (sym == Keysym::Escape && plain).then_some(Done::Close),
        };
        self.asked(done)
    }

    /// Do `button`, as the footer or a menu asks: a sheet's, the keys
    /// sheet, closing, or one of the diff's.
    pub fn press(&mut self, button: ReposButton, env: Env) -> Option<Done> {
        match button {
            ReposButton::Help => {
                self.help = Some(0);
                None
            }
            ReposButton::Close if self.help.is_some() => {
                self.help = None;
                None
            }
            ReposButton::Close => Some(Done::Close),
            button => self.click(ReposHit::Button(button), env),
        }
    }

    /// The footer's actions: writing a comment, the review copied or
    /// deleted while there is one, the keys and closing.
    pub fn hints(&self) -> Vec<Hint> {
        use ReposButton as B;
        let mut buttons = vec![B::Comment];
        if !self.diff.review.is_empty() {
            buttons.extend([B::CopyReview, B::ClearReview]);
        }
        buttons.extend([B::Help, B::Close]);
        buttons
            .into_iter()
            .map(|button| Hint {
                button,
                label: match button {
                    B::Comment => "comment on the lines".into(),
                    B::Close => "close".into(),
                    button => button.label().to_lowercase(),
                },
                key: key_name(button).into(),
                disabled: None,
            })
            .collect()
    }

    /// A click on `hit`, in the panel; the sheet takes them while open.
    pub fn click(&mut self, hit: ReposHit, env: Env) -> Option<Done> {
        if self.help.is_some() {
            if hit == ReposHit::Button(ReposButton::Close) {
                self.help = None;
            }
            return None;
        }
        if self.clearing.is_some() {
            if let ReposHit::Button(button) = hit {
                self.sheet_press(button, env);
            }
            return None;
        }
        let outcome = self.with_diff(env, |diff, ctx| diff.click(hit, ctx));
        self.asked(outcome.flatten().map(Done::Diff))
    }

    /// Answer the sheet asking whether to delete the review: it closes,
    /// the review deleted when `button` says so.
    fn sheet_press(&mut self, button: ReposButton, env: Env) {
        if !clear_buttons(self.clearing_outdated).contains(&button) {
            return;
        }
        self.clearing = None;
        match button {
            ReposButton::ClearReview => {
                self.with_diff(env, |diff, ctx| diff.clear_review(ctx.sections));
            }
            ReposButton::DropOutdated => {
                self.with_diff(env, |diff, ctx| diff.drop_outdated(ctx.sections));
            }
            _ => {}
        }
    }

    /// What a key or click on the diff asked for: deleting the review is
    /// asked about first, on a sheet.
    fn asked(&mut self, done: Option<Done>) -> Option<Done> {
        match done {
            Some(Done::Diff(diff_view::Outcome::Clear)) => {
                self.clearing = Some(0);
                self.clearing_outdated = false;
                None
            }
            Some(Done::Diff(diff_view::Outcome::ClearOutdated)) => {
                self.clearing = Some(0);
                self.clearing_outdated = true;
                None
            }
            done => done,
        }
    }

    /// The sheet over the diff, while open: its keys, or asking whether to
    /// delete the review.
    pub fn sheet(&self, env: Env) -> Option<SheetView> {
        if let Some(top) = self.help {
            let lines = diff_help_lines();
            let top = top.min(lines.len().saturating_sub(1));
            return Some(SheetView {
                title: help_title("Keys in a worktree's diff", top, lines.len()),
                lines: lines[top..].to_vec(),
                buttons: vec![ReposButton::Close],
                default: 0,
                danger: false,
            });
        }
        let choice = self.clearing?;
        let sections = self.sections();
        let lead = self.lead();
        let outdated = self.clearing_outdated;
        let (title, lines) = self
            .diff
            .clear_sheet(&ctx(&sections, env, &lead, self.empty()), outdated);
        let buttons = clear_buttons(outdated);
        Some(SheetView {
            title,
            lines,
            buttons: buttons.to_vec(),
            default: choice.min(buttons.len() - 1),
            danger: true,
        })
    }

    /// Shift+click on row `i` of the diff, or the pointer dragged on it.
    pub fn extend_row(&mut self, i: usize, env: Env) {
        self.with_diff(env, |diff, ctx| diff.extend_detail_row(i, ctx));
    }

    /// The pointer dragged past the diff's rows, `by` (up negative).
    pub fn drag_past_rows(&mut self, by: i64, env: Env) {
        self.with_diff(env, |diff, ctx| diff.drag_past_rows(by, ctx));
    }

    /// Whether a press on the diff, held, drags the cursor's range on.
    pub fn drags_rows(&mut self, env: Env) -> bool {
        self.with_diff(env, |diff, ctx| diff.drags_rows(ctx))
    }

    /// Open the file shown as row `i` in Zed, at that line.
    pub fn open_row(&mut self, i: usize, env: Env) -> Option<Done> {
        let outcome = self.with_diff(env, |diff, ctx| diff.open_detail_row(i, ctx));
        outcome.map(Done::Diff)
    }

    /// Ctrl+click on row `i`: the cursor there, and its code opened in
    /// Zed.
    pub fn goto_row(&mut self, i: usize, env: Env) -> Option<Done> {
        let outcome = self.with_diff(env, |diff, ctx| diff.goto_detail_row(i, ctx));
        outcome.map(Done::Diff)
    }

    /// Moved by `motion` at `now`.
    pub fn scroll(&mut self, motion: Motion, now: Instant, env: Env) {
        self.with_diff(env, |diff, ctx| diff.scroll(motion, now, ctx));
    }

    /// Moved on to `now`; whether it still moves.
    pub fn scroll_tick(&mut self, now: Instant, env: Env) -> bool {
        self.with_diff(env, |diff, ctx| diff.scroll_tick(now, ctx))
    }

    /// Stand at row `pos`, as the scrollbar's thumb held there puts it.
    pub fn stand_at(&mut self, pos: f64, env: Env) {
        self.with_diff(env, |diff, ctx| diff.stand_at(pos, ctx));
    }

    /// Whether it moves on its own or under the fingers at `now`.
    pub fn scrolling(&self, now: Instant) -> bool {
        self.diff.scroll.moving(now)
    }

    /// Why the last key or click did nothing, if it says.
    pub fn take_error(&mut self) -> Option<String> {
        self.diff.take_signals().error.flatten()
    }

    /// Whether `path` is this worktree's.
    pub fn is(&self, path: &Path) -> bool {
        self.root == path
    }
}

/// The diff's `Ctx` over `sections`, the diff having the keyboard and no
/// file checked.
fn ctx<'a>(
    sections: &'a [Section<'a>],
    env: Env,
    lead: &'a str,
    empty: &'a str,
) -> diff_view::Ctx<'a> {
    diff_view::Ctx {
        sections,
        page: env.page,
        cols: env.cols,
        density: env.density,
        keys: Keys::Diff,
        lead,
        empty,
        count_sections: false,
        check: None,
        selected: None,
    }
}

impl crate::Gerdr {
    /// What the worktree diff's views and keys need of the chrome.
    fn worktree_env(&self) -> Env {
        let size = self.output_size();
        Env {
            page: crate::chrome::diff_only_page(size, self.density),
            cols: crate::chrome::diff_only_cols(size),
            density: self.density,
            sheet_rows: crate::chrome::repos_sheet_rows(size, true),
        }
    }

    /// Show the diff of the worktree at `root` (of repository `label`, on
    /// `branch`) against `target`, in place of the worktrees popup, which
    /// it goes back to when closed; its files open in the Zed of workspace
    /// `home`, whose checkout it is, else in a Zed tab on it in workspace
    /// `workspace`.
    pub fn open_worktree_diff(
        &mut self,
        root: PathBuf,
        label: String,
        branch: Option<String>,
        target: Option<String>,
        (workspace, home): (usize, Option<usize>),
    ) {
        self.switcher = None;
        self.project_menu = None;
        self.inbox.close();
        self.scrollback = None;
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.worktrees.park();
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
        self.repos.close_worktree();
        let review = self.worktree_reviews().get(&root, branch.as_deref());
        let mut wd = WorktreeDiff::new(root, label, branch, target, workspace, review);
        wd.home = home;
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The reviews of worktrees' diffs: read when first wanted, those of
    /// worktrees gone, on another branch or too old dropped.
    fn worktree_reviews(&mut self) -> &mut crate::review::Worktrees {
        if self.repos.worktree_reviews.is_none() {
            let mut reviews = crate::review::load_worktrees(None);
            if reviews.prune(crate::review::now(), Path::is_dir) {
                self.save_worktree_reviews(&reviews);
            }
            self.repos.worktree_reviews = Some(reviews);
        }
        self.repos.worktree_reviews.get_or_insert_default()
    }

    /// Write the worktrees' reviews, saying so when that fails.
    fn save_worktree_reviews(&mut self, reviews: &crate::review::Worktrees) {
        if let Err(err) = crate::review::save_worktrees(None, reviews) {
            tracing::warn!("saving the worktrees' reviews: {err}");
            self.toast(
                format!("Couldn't save the review: {err}"),
                crate::toast::Level::Error,
            );
        }
    }

    /// Save the worktree diff's review, if it changed: with the others,
    /// pruned.
    fn worktree_save_review(&mut self) {
        let Some(wd) = self
            .repos
            .worktree
            .as_mut()
            .filter(|wd| wd.diff.review_dirty)
        else {
            return;
        };
        wd.diff.review_dirty = false;
        let (root, branch, review) = (wd.root.clone(), wd.branch.clone(), wd.diff.review.clone());
        let now = crate::review::now();
        let reviews = self.worktree_reviews();
        reviews.put(&root, branch.as_deref(), &review, now);
        reviews.prune(now, Path::is_dir);
        let reviews = reviews.clone();
        self.save_worktree_reviews(&reviews);
    }

    /// Drop the reviews of the worktrees at `paths`, deleted.
    pub(crate) fn forget_worktree_reviews(&mut self, paths: &[PathBuf]) {
        let reviews = self.worktree_reviews();
        let mut gone = false;
        for path in paths {
            gone |= reviews.forget(path);
        }
        if gone {
            let reviews = reviews.clone();
            self.save_worktree_reviews(&reviews);
        }
    }

    /// Close the worktree's diff, back to the worktrees popup.
    pub(crate) fn close_worktree_diff(&mut self) {
        self.worktree_save_review();
        self.repos.close_worktree();
        self.worktrees.unpark();
        self.refresh_pointer_focus();
    }

    /// Read the worktree's changes and their diff again, off the event
    /// loop, unless a read is under way.
    pub(crate) fn worktree_diff_read(&mut self) {
        let Some(wd) = self.repos.worktree.as_ref().filter(|wd| !wd.reading) else {
            return;
        };
        let (root, target) = (wd.root.clone(), wd.target.clone());
        let Some(tx) = self.repos_sender() else {
            return;
        };
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.reading = true;
        }
        std::thread::spawn(move || {
            let read = crate::git_ops::changes_since(&root, target.as_deref()).map(|since| {
                let files =
                    crate::git_ops::repo_diff_from(&root, &since.changes, Some(&since.base));
                (since, files)
            });
            let _ = tx.send(crate::repos::RepoEvent::WorktreeDiff { root, read });
        });
    }

    /// A read of a worktree's diff came back: shown if it is still the
    /// one open, the keys typed meanwhile after a cut done now.
    pub(crate) fn worktree_diff_event(
        &mut self,
        root: PathBuf,
        read: Result<(Since, Vec<FileDiff>), String>,
    ) {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut().filter(|wd| wd.is(&root)) else {
            return;
        };
        // Read from before the edit: the read after it is the one to show.
        if std::mem::take(&mut wd.again) {
            wd.reading = false;
            return self.worktree_diff_read();
        }
        wd.set_read(read, env);
        let held = wd.diff.take_held();
        for (mods, sym, typed) in held {
            if let Some(outcome) = self.repos_key(&mods, sym, typed) {
                self.apply_repos(outcome);
            }
        }
    }

    /// What the worktree's diff shows, while open: the panel with the
    /// diff alone across it.
    pub(crate) fn worktree_diff_view(&self) -> Option<crate::chrome::ReposView> {
        let wd = self.repos.worktree.as_ref()?;
        let env = self.worktree_env();
        Some(crate::chrome::ReposView {
            title: wd.heading(),
            summary: wd.summary(),
            lines: Vec::new(),
            part: 0.0,
            over: 0.0,
            first: 0,
            total: 0,
            selected: None,
            hover: wd.hover,
            field: Default::default(),
            field_focused: false,
            placeholder: String::new(),
            error: None,
            result: None,
            hints: wd.hints(),
            detail: wd.view(env),
            sheet: wd.sheet(env),
            empty: String::new(),
            branch: Default::default(),
            density: self.density,
            tree_share: 0.0,
            grouping: Default::default(),
            diff_only: true,
        })
    }

    /// What a key or click on the worktree's diff asked for, as the
    /// panel asks it; why it did nothing said briefly.
    fn worktree_done(&mut self, done: Option<Done>) -> Option<crate::repos::Outcome> {
        self.worktree_save_review();
        let error = self
            .repos
            .worktree
            .as_mut()
            .and_then(WorktreeDiff::take_error);
        if let Some(why) = error {
            self.show_warning(why);
        }
        match done? {
            Done::Close => Some(crate::repos::Outcome::Close),
            Done::Diff(outcome) => Some(crate::repos::from_diff(outcome)),
        }
    }

    /// Do `button` in the worktree's diff, as its footer or a menu asks.
    pub(crate) fn worktree_diff_press(&mut self, button: ReposButton) {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        let done = wd.press(button, env);
        if let Some(outcome) = self.worktree_done(done) {
            self.apply_repos(outcome);
        }
    }

    /// A key while the worktree's diff is open.
    pub(crate) fn worktree_diff_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<crate::repos::Outcome> {
        let env = self.worktree_env();
        let done = self.repos.worktree.as_mut()?.key(mods, sym, typed, env);
        self.worktree_done(done)
    }

    /// A click on `hit` in the worktree's diff, the second of a double
    /// click when `double`: the press, held, drags the cursor's range.
    /// With Shift it reaches the range there; with Ctrl it opens the code
    /// there in Zed.
    pub(crate) fn worktree_diff_hit(
        &mut self,
        hit: ReposHit,
        double: bool,
        shift: bool,
        ctrl: bool,
    ) {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        let mut drags = false;
        let done = match (hit, shift, double) {
            (ReposHit::Button(button), ..) => wd.press(button, env),
            (ReposHit::DiffRow(i), _, _) if ctrl => wd.goto_row(i, env),
            (ReposHit::DiffRow(i), true, false) => {
                wd.extend_row(i, env);
                drags = wd.drags_rows(env);
                None
            }
            (ReposHit::DiffRow(i), _, true) => {
                wd.click(hit, env);
                wd.open_row(i, env)
            }
            _ => {
                let done = wd.click(hit, env);
                drags = matches!(hit, ReposHit::DiffRow(_)) && wd.drags_rows(env);
                done
            }
        };
        self.repos_rows_drag = drags;
        if let Some(outcome) = self.worktree_done(done) {
            self.apply_repos(outcome);
        }
    }

    /// The pointer dragged from a press on the worktree's diff: over row
    /// `row` (by index into those shown), or `by` rows past them.
    pub(crate) fn worktree_diff_drag(&mut self, by: i64, row: Option<usize>) {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        match (by, row) {
            (0, Some(i)) => wd.extend_row(i, env),
            (0, None) => {}
            (by, _) => wd.drag_past_rows(by, env),
        }
    }

    /// The wheel or the touchpad moved the worktree's diff.
    pub(crate) fn worktree_diff_scroll(&mut self, motion: Motion) {
        let now = Instant::now();
        let env = self.worktree_env();
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.scroll(motion, now, env);
            if wd.scrolling(now) {
                wd.hover = None;
            }
        }
    }

    /// Stand the worktree's diff at row `pos`, as its scrollbar's thumb
    /// held there puts it.
    pub(crate) fn worktree_diff_scroll_to(&mut self, pos: f64) {
        let env = self.worktree_env();
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.stand_at(pos, env);
        }
    }

    /// Move the worktree's diff on to `now` if it moves on its own;
    /// whether it still does.
    pub(crate) fn worktree_diff_tick(&mut self, now: Instant) -> bool {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut().filter(|wd| wd.scrolling(now)) else {
            return false;
        };
        let moving = wd.scroll_tick(now, env);
        if moving {
            wd.hover = None;
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chrome::{DetailRow, ReposButton},
        diff_view::Outcome,
        git::Change,
    };

    const ENV: Env = Env {
        sheet_rows: 10,
        page: 20,
        cols: 60,
        density: Density::Comfortable,
    };

    fn none() -> ModifiersState {
        ModifiersState::default()
    }

    fn change(path: &str, code: char) -> Change {
        Change {
            path: path.into(),
            orig: None,
            code,
        }
    }

    /// `path`'s diff adding `lines`.
    fn added(path: &str, code: char, lines: &[&str]) -> FileDiff {
        let mut text = String::from("diff --git a/f b/f\n@@ -0,0 +1 @@\n");
        for line in lines {
            text.push_str(&format!("+{line}\n"));
        }
        let mut file = crate::diff::parse(&text).pop().unwrap();
        file.path = path.into();
        file.code = code;
        file
    }

    /// The diff of worktree `wt` of repository gerdr on feat/x against
    /// main, read: two commits, a.rs adding a and b, new.rs untracked.
    fn read() -> WorktreeDiff {
        let root = PathBuf::from("/src/gerdr/.wt/x");
        let mut wd = WorktreeDiff::new(
            root,
            "gerdr".into(),
            Some("feat/x".into()),
            Some("main".into()),
            0,
            Review::default(),
        );
        assert_eq!(wd.view(ENV).rows.len(), 1, "{:?}", wd.view(ENV).rows);
        let since = Since {
            base: "abc".into(),
            commits: 2,
            changes: vec![change("a.rs", 'M'), change("new.rs", '?')],
        };
        let files = vec![
            added("a.rs", 'M', &["a", "b"]),
            added("new.rs", '?', &["n"]),
        ];
        wd.set_read(Ok((since, files)), ENV);
        wd
    }

    fn texts(wd: &WorktreeDiff) -> Vec<String> {
        wd.view(ENV)
            .rows
            .iter()
            .map(|row| match row {
                DetailRow::Repo(repo) => format!("repo {}", repo.label),
                DetailRow::File(file) => format!("file {} {:?}", file.path, file.check),
                DetailRow::Line(line) => line.text.clone(),
                DetailRow::Text(line) => line.text.clone(),
                DetailRow::Hunk { context, .. } => format!("@ {context}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn it_shows_what_the_worktree_changed_since_it_forked() {
        let mut wd = WorktreeDiff::new(
            "/src/gerdr/.wt/x".into(),
            "gerdr".into(),
            Some("feat/x".into()),
            Some("main".into()),
            0,
            Review::default(),
        );
        assert_eq!(texts(&wd), ["Reading…"]);
        wd.set_read(Err("It has no commit in common with main".into()), ENV);
        assert_eq!(wd.summary(), "It has no commit in common with main");
        let wd = read();
        assert_eq!(wd.heading(), "Worktree · feat/x");
        assert_eq!(wd.summary(), "2 commits on main");
        // Its files have no checkbox: nothing is committed from here.
        assert_eq!(
            texts(&wd),
            [
                "repo gerdr",
                "file a.rs None",
                "@ ",
                "a",
                "b",
                "file new.rs None",
                "@ ",
                "n"
            ]
        );
        assert_eq!(wd.view(ENV).title, "Since main · 2 files · +3 −0");
        // Nothing left to show, it says so.
        let mut wd = wd;
        wd.set_read(Ok((Since::default(), Vec::new())), ENV);
        assert_eq!(texts(&wd), ["No changes since it forked"]);
    }

    #[test]
    fn its_keys_are_the_diffs_and_esc_closes_it() {
        let mut wd = read();
        let mut key =
            |wd: &mut WorktreeDiff, mods: ModifiersState, sym: Keysym| wd.key(&mods, sym, sym, ENV);
        // Space checks nothing: there is nothing to check here.
        assert_eq!(key(&mut wd, none(), Keysym::Down), None);
        assert_eq!(key(&mut wd, none(), Keysym::space), None);
        // A comment on a line, then copied for an agent.
        key(&mut wd, none(), Keysym::Down);
        key(&mut wd, none(), Keysym::Down);
        key(&mut wd, none(), Keysym::Down);
        key(&mut wd, none(), Keysym::c);
        for sym in [Keysym::o, Keysym::k] {
            key(&mut wd, none(), sym);
        }
        key(&mut wd, none(), Keysym::Return);
        assert_eq!(wd.diff.review.comments.len(), 1);
        let copy = ModifiersState {
            ctrl: true,
            shift: true,
            ..none()
        };
        let copied = key(&mut wd, copy, Keysym::C);
        assert!(
            matches!(copied, Some(Done::Diff(Outcome::Copy { count: 1, .. }))),
            "{copied:?}"
        );
        // Enter opens the cursor's line in Zed, in the worktree.
        let opened = key(&mut wd, none(), Keysym::Return);
        let Some(Done::Diff(Outcome::OpenInZed { root, paths, at })) = opened else {
            panic!("{opened:?}");
        };
        assert_eq!((root, paths), (wd.root.clone(), vec!["a.rs".to_owned()]));
        assert_eq!(at.map(|at| at.line), Some(1));
        // `/` searches it; Esc closes the search, then the diff.
        key(&mut wd, none(), Keysym::slash);
        assert!(wd.diff.search_focused());
        assert_eq!(key(&mut wd, none(), Keysym::Escape), None);
        assert!(!wd.diff.search_focused());
        // F1 lists its keys, scrolled by the arrows; Esc closes the list,
        // then the diff.
        assert_eq!(key(&mut wd, none(), Keysym::F1), None);
        let sheet = wd.sheet(ENV).expect("the keys");
        assert_eq!(sheet.lines[0].0, "Esc");
        assert!(sheet.lines.iter().any(|(k, ..)| k == "c / Del Del"));
        key(&mut wd, none(), Keysym::Down);
        assert_eq!(wd.sheet(ENV).expect("the keys").lines[0].0, "F1");
        assert_eq!(key(&mut wd, none(), Keysym::Escape), None);
        assert!(wd.sheet(ENV).is_none());
        // Its footer has the review's actions (a comment is written),
        // the keys and closing.
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        use ReposButton as B;
        let all = [B::Comment, B::CopyReview, B::ClearReview, B::Help, B::Close];
        assert_eq!(hints, all);
        assert_eq!(wd.press(ReposButton::Help, ENV), None);
        assert!(wd.sheet(ENV).is_some());
        wd.press(ReposButton::Close, ENV);
        assert!(wd.sheet(ENV).is_none());
        assert_eq!(key(&mut wd, none(), Keysym::Escape), Some(Done::Close));
    }

    /// What opening asked for: the paths, and where in the first.
    fn opened(done: Option<Done>) -> (Vec<String>, Option<diff_view::Position>) {
        match done {
            Some(Done::Diff(Outcome::OpenInZed { paths, at, .. })) => (paths, at),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn g_space_and_ctrl_click_open_the_code_at_the_caret() {
        let mut wd = read();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let at = |line, col| Some(diff_view::Position { line, col });
        // On a file's header: the file, Zed putting it where it was.
        wd.click(ReposHit::DiffRow(1), ENV);
        assert_eq!(key(&mut wd, Keysym::g), None);
        assert_eq!(wd.diff.mode_title().as_deref(), Some("g"));
        let (paths, place) = opened(key(&mut wd, Keysym::space));
        assert_eq!((paths, place), (vec!["a.rs".to_owned()], None));
        // On a line, its line at the caret's char.
        key(&mut wd, Keysym::Down);
        key(&mut wd, Keysym::Down);
        key(&mut wd, Keysym::Down);
        key(&mut wd, Keysym::g);
        assert_eq!(opened(key(&mut wd, Keysym::space)).1, at(2, Some(0)));
        // `g` and another key: what `g` takes is said, and Space is its
        // own again.
        key(&mut wd, Keysym::g);
        let said = "After g: Space opens in Zed, g goes to the top";
        assert_eq!(
            key(&mut wd, Keysym::k),
            Some(Done::Diff(Outcome::Notice(said)))
        );
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), Some(4));
        assert_eq!(wd.diff.mode_title(), None);
        assert_eq!(key(&mut wd, Keysym::space), None);
        // Ctrl+click: the caret on the char pressed, there in Zed.
        wd.diff.pointer_char = Some((3, 0));
        let (paths, place) = opened(wd.goto_row(3, ENV));
        assert_eq!((paths, place), (vec!["a.rs".to_owned()], at(1, Some(0))));
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), Some(3));
        // On a comment, its line, the comment not edited.
        key(&mut wd, Keysym::c);
        key(&mut wd, Keysym::o);
        key(&mut wd, Keysym::Return);
        assert_eq!(wd.diff.review.comments.len(), 1);
        assert_eq!(opened(wd.goto_row(4, ENV)).1, at(1, None));
        assert!(wd.diff.editing.is_none());
    }

    #[test]
    fn a_removed_line_opens_at_its_line_alone() {
        let mut wd = read();
        let mut file = crate::diff::parse("diff --git a/f b/f\n@@ -1,2 +1 @@\n-gone\n kept\n")
            .pop()
            .unwrap();
        file.path = "a.rs".into();
        file.code = 'M';
        let since = Since {
            changes: vec![change("a.rs", 'M')],
            ..Since::default()
        };
        wd.set_read(Ok((since, vec![file])), ENV);
        assert_eq!(texts(&wd)[3], "gone");
        let (_, place) = opened(wd.goto_row(3, ENV));
        assert_eq!(place.map(|at| at.col), Some(None));
    }

    #[test]
    fn its_clicks_are_the_diffs() {
        let mut wd = read();
        // A click on a line puts the cursor there; on the title's button
        // with no comment, it says why.
        assert_eq!(wd.click(ReposHit::DiffRow(2), ENV), None);
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), Some(2));
        let copy = ReposHit::Button(ReposButton::CopyReview);
        assert_eq!(wd.click(copy, ENV), None);
        assert!(wd.take_error().is_some());
        // A file header's chevron folds it.
        wd.click(ReposHit::DiffFold(1), ENV);
        assert_eq!(texts(&wd)[1..3], ["file a.rs None", "file new.rs None"]);
    }

    #[test]
    fn clearing_its_review_asks_first() {
        let mut wd = read();
        let mut key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        for sym in [
            Keysym::Down,
            Keysym::Down,
            Keysym::Down,
            Keysym::Down,
            Keysym::c,
            Keysym::o,
        ] {
            key(&mut wd, sym);
        }
        key(&mut wd, Keysym::Return);
        let buttons: Vec<ReposButton> = wd
            .view(ENV)
            .title_buttons
            .iter()
            .map(|h| h.button)
            .collect();
        assert_eq!(buttons, [ReposButton::CopyReview, ReposButton::ClearReview]);
        // Asked on a sheet, which takes the keys: Esc keeps it.
        let clear = ReposHit::Button(ReposButton::ClearReview);
        assert_eq!(wd.click(clear, ENV), None);
        let sheet = wd.sheet(ENV).unwrap();
        assert_eq!(sheet.title, "Delete the review's 1 comment for good?");
        assert_eq!(sheet.lines[0].1, "a.rs: 1 comment");
        assert_eq!(key(&mut wd, Keysym::Escape), None);
        assert!(wd.sheet(ENV).is_none() && wd.diff.review.comments.len() == 1);
        // Enter cancels first; on Clear review it deletes it, to be saved.
        wd.click(clear, ENV);
        assert_eq!(key(&mut wd, Keysym::Return), None);
        assert!(wd.sheet(ENV).is_none() && wd.diff.review.comments.len() == 1);
        wd.click(clear, ENV);
        wd.diff.review_dirty = false;
        assert_eq!(key(&mut wd, Keysym::Right), None);
        assert_eq!(key(&mut wd, Keysym::Return), None);
        assert!(wd.sheet(ENV).is_none() && wd.diff.review.is_empty());
        assert!(wd.diff.review_dirty);
        assert!(wd.view(ENV).title_buttons.is_empty());
    }
}
