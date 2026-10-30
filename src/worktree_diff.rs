//! A worktree's diff, from the worktrees popup (Enter, or Ctrl+D): every
//! change of the worktree since it forked from what its branch merges
//! into, its commits and what isn't committed yet, in the repositories
//! panel's diff view (`diff_view`), alone across the panel. A main
//! worktree's is what isn't committed. Its keys, clicks, comments, search
//! and vim's edits are the diff's own; Esc closes it, back to the popup.
//! It is read again with every Git status read.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{Density, DetailView, ReposHit},
    diff::FileDiff,
    diff_view::{self, DiffView, Keys, Section},
    git_ops::Since,
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
    /// The workspace its files open in.
    pub workspace: usize,
    /// What it changed since it forked, as last read.
    pub since: Since,
    /// Read once at least; and why the last read failed.
    pub read: bool,
    pub error: Option<String>,
    /// A read is under way.
    pub reading: bool,
    pub diff: DiffView,
    pub hover: Option<ReposHit>,
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
            changes: &since.changes,
        }];
        f(diff, &ctx(&sections, env, &lead, empty))
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
        match (&self.target, self.since.commits) {
            (None, _) => "uncommitted changes".into(),
            (Some(target), 0) => format!("no commits of its own on {target}"),
            (Some(target), 1) => format!("1 commit on {target}"),
            (Some(target), n) => format!("{n} commits on {target}"),
        }
    }

    /// The diff, as the panel's detail shows it.
    pub fn view(&self, env: Env) -> DetailView {
        let sections = [Section {
            root: &self.root,
            label: &self.label,
            branch: self.branch.as_deref(),
            changes: &self.since.changes,
        }];
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

    /// A key; `None` for none of the diff's, Esc closing it.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        env: Env,
    ) -> Option<Done> {
        let outcome = self.with_diff(env, |diff, ctx| diff.key(mods, sym, typed, ctx));
        match outcome {
            Some(outcome) => outcome.map(Done::Diff),
            None => {
                let plain = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
                (sym == Keysym::Escape && plain).then_some(Done::Close)
            }
        }
    }

    /// A click on `hit`, in the panel.
    pub fn click(&mut self, hit: ReposHit, env: Env) -> Option<Done> {
        let outcome = self.with_diff(env, |diff, ctx| diff.click(hit, ctx));
        outcome.flatten().map(Done::Diff)
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
        }
    }

    /// Show the diff of the worktree at `root` (of repository `label`, on
    /// `branch`) against `target`, in place of the worktrees popup, which
    /// it goes back to when closed; its files open in workspace
    /// `workspace`.
    pub fn open_worktree_diff(
        &mut self,
        root: PathBuf,
        label: String,
        branch: Option<String>,
        target: Option<String>,
        workspace: usize,
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
        let review = self
            .repos
            .worktree_reviews
            .remove(&root)
            .unwrap_or_default();
        let wd = WorktreeDiff::new(root, label, branch, target, workspace, review);
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the worktree's diff, back to the worktrees popup.
    pub(crate) fn close_worktree_diff(&mut self) {
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
            hints: Vec::new(),
            detail: wd.view(env),
            sheet: None,
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
        let error = self
            .repos
            .worktree
            .as_mut()
            .and_then(WorktreeDiff::take_error);
        if let Some(why) = error {
            self.show_notice(why);
        }
        match done? {
            Done::Close => Some(crate::repos::Outcome::Close),
            Done::Diff(outcome) => Some(crate::repos::from_diff(outcome)),
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
    pub(crate) fn worktree_diff_hit(&mut self, hit: ReposHit, double: bool, shift: bool) {
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        let mut drags = false;
        let done = match (hit, shift, double) {
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
                "a",
                "b",
                "file new.rs None",
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
        let Some(Done::Diff(Outcome::OpenInZed { root, paths })) = opened else {
            panic!("{opened:?}");
        };
        assert_eq!((root, paths), (wd.root.clone(), vec!["a.rs:1".to_owned()]));
        // `/` searches it; Esc closes the search, then the diff.
        key(&mut wd, none(), Keysym::slash);
        assert!(wd.diff.search_focused());
        assert_eq!(key(&mut wd, none(), Keysym::Escape), None);
        assert!(!wd.diff.search_focused());
        assert_eq!(key(&mut wd, none(), Keysym::Escape), Some(Done::Close));
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
}
