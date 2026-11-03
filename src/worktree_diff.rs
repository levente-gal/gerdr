//! A worktree's diff, from the worktrees popup (Enter, or Ctrl+D): every
//! change of the worktree since it forked from what its branch merges
//! into, its commits and what isn't committed yet, in the repositories
//! panel's diff view (`diff_view`), alone across the panel. A main
//! worktree's is what isn't committed. Its keys, clicks, comments, search
//! and vim's edits are the diff's own; Esc closes it, back to the popup.
//! It is read again with every Git status read. Its review comments are
//! saved by worktree (see `review::Worktrees`), and go with it when it is
//! deleted.
//!
//! A commit's diff, from the repositories panel's Log tab, shows the same
//! way, read-only: what the commit changed against its first parent. Esc
//! goes back to the panel.
//!
//! A feature stage's file under review (`brief.md`, `plan.md`; see
//! `doc_review`) shows the same way too, as one file of added lines: its
//! comments work as on code, edits don't. Approve and Send back sit on its
//! header and footer.
//!
//! A feature's code review (see `feature_diff`) shows the same way too:
//! the feature's changes over every repository it touched, one section
//! each, read-only. Each file's checkbox says whether it was viewed; the
//! AI review's findings show as comments of their own; Approve and Send
//! back sit on its header.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{Density, DetailView, Hint, ReposButton, ReposHit, SheetView},
    diff::FileDiff,
    diff_view::{self, CheckOf, DiffView, Keys, NoteOf, Outcome, Section},
    doc_review::DocFile,
    feature::review_import::Decision,
    feature_diff::FeatureOf,
    git::Change,
    git_ops::Since,
    md_preview::PREVIEW_KEY,
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
    /// A commit's diff instead, opened from the panel's Log tab: read
    /// once, not edited, no review.
    pub commit: Option<CommitOf>,
    /// A feature stage's file under review instead: read again with
    /// every Git status read, edited while its agent isn't writing it.
    pub doc: Option<DocOf>,
    /// A feature's code review instead: its changes over every repository
    /// it touched, read again with every Git status read, not edited.
    pub feature: Option<FeatureOf>,
}

/// The feature stage's file a diff shows for review
/// (`WorktreeDiff::doc`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocOf {
    pub project: String,
    pub slug: String,
    /// Which of the feature's files it is.
    pub file: DocFile,
    /// The file: `<feature data dir>/brief.md`.
    pub path: PathBuf,
    /// Where its review comments are kept (`doc_review::comments_path`).
    pub comments: PathBuf,
    /// The feature's title.
    pub title: String,
    /// Where the stage stands, lowercase: `ready for review`.
    pub state: String,
    /// The file's text as last read: the same again isn't shown again.
    pub text: Option<String>,
    /// Opened from the feature panel, which Esc goes back to.
    pub back: bool,
    /// Why it can't be edited now (`doc_review::locked`): its agent is
    /// writing it, it's approved. None: the diff's edits change it.
    pub locked: Option<&'static str>,
}

impl DocOf {
    /// Verify's `evidence.md`, only read: no comments, no Approve or Send
    /// back.
    pub fn evidence(&self) -> bool {
        self.file == DocFile::Evidence
    }

    /// The file's name: `brief.md`.
    pub fn name(&self) -> String {
        let name = self.path.file_name().unwrap_or(self.path.as_os_str());
        name.to_string_lossy().into()
    }
}

/// Why a restore does nothing on a file under review: it shows as all
/// added, with nothing before it to restore.
pub const DOC_NO_RESTORE: &str = "There's nothing to restore in a file under review";
/// Why a comment, an edit, Approve or Send back do nothing on Verify's
/// evidence.
pub const EVIDENCE_READ_ONLY: &str = "The evidence is only read";
/// Why Alt+Shift+Down did nothing.
pub const ALL_VIEWED: &str = "Every file is viewed";
/// Why Send back does nothing yet.
pub const NO_COMMENT: &str = "Add a comment first";
/// Why Approve or Send back wait.
pub const DRAFT_OPEN: &str = "Save or discard the comment first";
/// Why an edit or a restore does nothing in a feature's code review.
pub const FEATURE_READ_ONLY: &str = "A feature's code review can't edit the code";
/// The keys approving the file under review and sending it back.
pub const APPROVE_KEY: &str = "Ctrl+Enter";
pub const SEND_BACK_KEY: &str = "Ctrl+Shift+Enter";
/// The key going to the next file not viewed in a feature's code review.
pub const NEXT_UNVIEWED_KEY: &str = "Alt+Shift+Down";

/// The commit a diff shows (`WorktreeDiff::commit`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitOf {
    pub oid: String,
    /// Its line in the log: `1a2b3c4 subject · author, 2 hours ago`.
    pub line: String,
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
    /// Approve the file under review as it is.
    Approve,
    /// Send it back to its agent with these comments, as Markdown
    /// (`review::doc_markdown`).
    SendBack(String),
    /// Show the file under review rendered (see `md_preview`).
    Preview,
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

    /// The diff of `commit` of the repository at `root` (called `label`,
    /// in workspace `workspace`), read-only.
    pub fn of_commit(root: PathBuf, label: String, commit: CommitOf, workspace: usize) -> Self {
        WorktreeDiff {
            root,
            label,
            workspace,
            home: Some(workspace),
            commit: Some(commit),
            ..WorktreeDiff::default()
        }
    }

    /// Feature stage file `doc` for review, in workspace `workspace`
    /// (where a Zed tab on it opens); its review comments `review`, kept
    /// from when it was shown last. Its text is shown once `set_doc` has
    /// it.
    pub fn of_doc(doc: DocOf, workspace: usize, review: Review) -> Self {
        let mut diff = DiffView::default();
        diff.review = review;
        let root = doc.path.parent().map(Path::to_path_buf).unwrap_or_default();
        WorktreeDiff {
            root,
            label: doc.title.clone(),
            workspace,
            diff,
            doc: Some(doc),
            ..WorktreeDiff::default()
        }
    }

    /// The panel showing this diff alone across it.
    pub fn repos_view(&self, env: Env) -> crate::chrome::ReposView {
        crate::chrome::ReposView::diff_only(
            self.heading(),
            self.summary(),
            self.hover,
            self.hints(),
            self.view(env),
            self.sheet(env),
            env.density,
        )
    }

    /// The file under review read again (`Err`: why it couldn't be): shown
    /// as all added lines, where it stood; the same text as before left
    /// as it is.
    pub fn set_doc(&mut self, read: Result<String, String>, env: Env) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let text = match read {
            Ok(text) => text,
            Err(why) => {
                doc.text = None;
                return self.set_read(Err(why), env);
            }
        };
        if doc.text.as_ref() == Some(&text) && self.error.is_none() {
            return;
        }
        let name = doc.name();
        let file = crate::doc_review::added_file(&name, &text);
        doc.text = Some(text);
        let since = Since {
            base: String::new(),
            commits: 0,
            changes: vec![Change {
                path: name,
                orig: None,
                code: 'A',
            }],
        };
        self.set_read(Ok((since, vec![file])), env);
    }

    /// The file under review as the diff has it, if read.
    fn doc_file(&self) -> Option<&FileDiff> {
        self.diff.diffs.get(&self.root)?.1.first()
    }

    /// The review comments as Markdown for the agent that wrote the file
    /// under review: those whose lines are gone say so.
    pub fn doc_markdown(&self) -> String {
        let name = self.doc.as_ref().map(DocOf::name).unwrap_or_default();
        let file = self.doc_file();
        let outdated = |c: &crate::review::Comment| {
            !c.lines.is_empty() && file.is_none_or(|f| crate::review::locate(f, c).is_none())
        };
        crate::review::doc_markdown(&self.diff.review, &name, outdated)
    }

    /// Whether it shows Verify's evidence.
    fn evidence(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.evidence())
    }

    /// Whether its lines take no comments: a commit's, the evidence.
    fn read_only(&self) -> bool {
        self.commit.is_some() || self.evidence()
    }

    /// Approve the file under review, unless a comment is being written.
    /// A feature's code review is checked further by its host.
    fn approve(&self) -> Option<Done> {
        if self.evidence() {
            return Some(Done::Diff(Outcome::Notice(EVIDENCE_READ_ONLY)));
        }
        if self.diff.editing.is_some() {
            return Some(Done::Diff(Outcome::Notice(DRAFT_OPEN)));
        }
        Some(Done::Approve)
    }

    /// Send the file under review back with its comments: there must be
    /// one, and none being written.
    fn send_back(&self, env: Env) -> Option<Done> {
        if self.evidence() {
            return Some(Done::Diff(Outcome::Notice(EVIDENCE_READ_ONLY)));
        }
        if self.diff.editing.is_some() {
            return Some(Done::Diff(Outcome::Notice(DRAFT_OPEN)));
        }
        if self.feature.is_some() {
            return Some(match self.with_ctx(env, |ctx| self.diff.copy_review(ctx)) {
                Ok(Outcome::Copy { text, .. }) => Done::SendBack(text),
                _ => Done::Diff(Outcome::Notice(crate::feature_diff::NO_OWN_COMMENT)),
            });
        }
        if self.diff.review.is_empty() {
            return Some(Done::Diff(Outcome::Notice(NO_COMMENT)));
        }
        Some(Done::SendBack(self.doc_markdown()))
    }

    /// Whether it is reviewed: a file under review, not the evidence, or
    /// a feature's code.
    fn reviewed(&self) -> bool {
        (self.doc.is_some() && !self.evidence()) || self.feature.is_some()
    }

    /// What the diff asked for, on a file under review: edits only while
    /// it isn't locked, no restore, and the review copied as the agent is
    /// sent it.
    fn doc_outcome(&self, outcome: Option<Outcome>) -> Option<Outcome> {
        if self.feature.is_some() {
            return match outcome {
                Some(o) if o.edits() => Some(Outcome::Notice(FEATURE_READ_ONLY)),
                outcome => outcome,
            };
        }
        if self.doc.is_none() {
            return outcome;
        }
        let refused = |o: &Outcome| o.edits() || *o == Outcome::Notice(diff_view::READ_ONLY);
        let restores = |o: &Outcome| matches!(o, Outcome::Cut(e) if e.cut.restored().is_some());
        // A yank takes nothing from the file: it works however it stands.
        let changes = |o: &Outcome| o.edits() && !matches!(o, Outcome::Yank(_));
        let locked = self.doc.as_ref().and_then(|d| d.locked);
        match outcome {
            Some(o) if self.evidence() && refused(&o) => Some(Outcome::Notice(EVIDENCE_READ_ONLY)),
            Some(o) if restores(&o) => Some(Outcome::Notice(DOC_NO_RESTORE)),
            Some(o) if changes(&o) && locked.is_some() => locked.map(Outcome::Notice),
            Some(Outcome::Copy { count, .. }) => Some(Outcome::Copy {
                text: self.doc_markdown(),
                count,
            }),
            outcome => outcome,
        }
    }

    /// The keys sheet's lines: a file under review's own keys first.
    fn help_lines(&self) -> Vec<(String, String, crate::chrome::Tone)> {
        // A read-only diff refuses the keys that edit the files.
        let edits = |what: &str| {
            what.contains("delete from the file")
                || what.contains("yank")
                || what.contains("restore the change")
        };
        // Evidence takes no comments either.
        let comments = |what: &str| what.contains("comment");
        // A file under review is edited while unlocked, never restored.
        let restore = |what: &str| what.contains("restore the change");
        let locked = self.doc.as_ref().is_some_and(|d| d.locked.is_some());
        let read_only =
            self.commit.is_some() || self.feature.is_some() || self.evidence() || locked;
        let evidence = self.evidence();
        let diff = diff_help_lines().into_iter().filter(|(_, what, _)| {
            !(read_only && edits(what))
                && !(self.doc.is_some() && restore(what))
                && !(evidence && comments(what))
        });
        if self.doc.is_none() && self.feature.is_none() {
            return diff.collect();
        }
        let own: &[(&str, &str)] = match (evidence, &self.feature) {
            (_, Some(_)) => &crate::feature_diff::KEYS,
            (true, _) => &[
                ("Esc", "close the evidence"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "show or hide it rendered, beside the review"),
            ],
            (false, _) => &[
                ("Esc", "close the review"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "show or hide it rendered, beside the review"),
                (APPROVE_KEY, "approve the file as it is"),
                (SEND_BACK_KEY, "send it back to its agent with the comments"),
            ],
        };
        let own = own.iter().map(|(key, what)| {
            (
                key.to_string(),
                what.to_string(),
                crate::chrome::Tone::Normal,
            )
        });
        // The diff's, but its Esc and F1.
        own.chain(diff.skip(2)).collect()
    }

    /// Call `f` with the diff and its `Ctx`.
    pub(crate) fn with_diff<R>(
        &mut self,
        env: Env,
        f: impl FnOnce(&mut DiffView, &diff_view::Ctx) -> R,
    ) -> R {
        let lead = self.lead();
        let empty = self.empty();
        let read_only = self.read_only();
        let WorktreeDiff {
            root,
            label,
            branch,
            since,
            diff,
            feature,
            ..
        } = self;
        let feature = feature.as_ref();
        let sections = sections(root, label, branch.as_deref(), since, feature);
        let check =
            |root: &Path, path: &str| feature.map(|f| f.check(root, path)).unwrap_or_default();
        let note = |root: &Path, path: &str| feature.and_then(|f| f.note(root, path));
        let marks: (Option<CheckOf>, Option<NoteOf>) = match feature {
            Some(_) => (Some(&check), Some(&note)),
            None => (None, None),
        };
        let mut ctx = ctx(&sections, env, &lead, empty, read_only);
        (ctx.check, ctx.note) = marks;
        ctx.count_sections = feature.is_some_and(|f| f.repos.len() > 1);
        f(diff, &ctx)
    }

    /// Call `f` with the diff's `Ctx`, to look.
    fn with_ctx<R>(&self, env: Env, f: impl FnOnce(&diff_view::Ctx) -> R) -> R {
        let lead = self.lead();
        let feature = self.feature.as_ref();
        let sections = self.sections();
        let check =
            |root: &Path, path: &str| feature.map(|f| f.check(root, path)).unwrap_or_default();
        let note = |root: &Path, path: &str| feature.and_then(|f| f.note(root, path));
        let marks: (Option<CheckOf>, Option<NoteOf>) = match feature {
            Some(_) => (Some(&check), Some(&note)),
            None => (None, None),
        };
        let mut ctx = ctx(&sections, env, &lead, self.empty(), self.read_only());
        (ctx.check, ctx.note) = marks;
        ctx.count_sections = feature.is_some_and(|f| f.repos.len() > 1);
        f(&ctx)
    }

    /// The diff's sections: the worktree, or a feature's repositories.
    fn sections(&self) -> Vec<Section<'_>> {
        let branch = self.branch.as_deref();
        sections(
            &self.root,
            &self.label,
            branch,
            &self.since,
            self.feature.as_ref(),
        )
    }

    /// Over the diff, before its counts: what it is against.
    fn lead(&self) -> String {
        if self.feature.is_some() {
            return "Since its base".into();
        }
        if self.commit.is_some() {
            return "This commit".into();
        }
        if let Some(doc) = &self.doc {
            return doc.name();
        }
        match &self.target {
            Some(target) => format!("Since {target}"),
            None => "Not committed".into(),
        }
    }

    /// What the diff says with nothing to show.
    fn empty(&self) -> &'static str {
        if let Some(feature) = &self.feature {
            return feature.empty(self.read);
        }
        match (&self.error, self.read, &self.target) {
            (Some(_), ..) if self.doc.is_some() => "The file couldn't be read",
            (Some(_), ..) => "Its changes couldn't be read",
            (None, false, _) => "Reading…",
            (None, true, _) if self.commit.is_some() => "No changes in this commit",
            (None, true, _) if self.doc.is_some() => "The file is empty",
            (None, true, Some(_)) => "No changes since it forked",
            (None, true, None) => "No changes",
        }
    }

    /// The panel's heading: whose diff it is.
    pub fn heading(&self) -> String {
        if let Some(feature) = &self.feature {
            return format!("Code review of \u{201c}{}\u{201d}", feature.title);
        }
        if self.commit.is_some() {
            return format!("Commit · {}", self.label);
        }
        if let Some(doc) = &self.doc {
            if doc.evidence() {
                return format!("Evidence of \u{201c}{}\u{201d}", doc.title);
            }
            return format!("{} · {}", doc.file.stage().label(), doc.name());
        }
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
        if let Some(feature) = &self.feature {
            return feature.summary(self.diff.review.own());
        }
        if let Some(doc) = &self.doc {
            if doc.evidence() {
                return doc.state.clone();
            }
            return format!("\u{201c}{}\u{201d} · {}", doc.title, doc.state);
        }
        if let Some(commit) = &self.commit {
            return commit.line.clone();
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
        let mut view = self.with_ctx(env, |ctx| self.diff.view(ctx));
        if self.doc.is_some() || self.feature.is_some() {
            self.doc_view(&mut view);
        }
        view
    }

    /// A file under review's view: its lines counted in the title, no
    /// Restore buttons, Approve and Send back on the title row.
    fn doc_view(&self, view: &mut DetailView) {
        if let (Some(doc), Some(file)) = (
            &self.doc,
            self.doc_file().filter(|_| self.feature.is_none()),
        ) {
            view.title = match file.added {
                1 => format!("{} · 1 line", doc.name()),
                n => format!("{} · {n} lines", doc.name()),
            };
        }
        (view.row_buttons).retain(|(_, hint)| hint.button != ReposButton::Restore);
        let hint = |button: ReposButton, key: &str| Hint {
            button,
            label: button.label().into(),
            key: key.into(),
            disabled: None,
        };
        if self.evidence() {
            view.title_buttons
                .push(hint(ReposButton::Preview, PREVIEW_KEY));
            return;
        }
        view.title_buttons.extend([
            hint(ReposButton::Preview, PREVIEW_KEY),
            hint(ReposButton::SendBack, SEND_BACK_KEY),
            hint(ReposButton::Approve, APPROVE_KEY),
        ]);
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
        let len = self.help_lines().len();
        if let Some(top) = self.help.as_mut() {
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
        // Approve and Send back, unless a comment or the search has the
        // keys.
        let enter = matches!(sym, Keysym::Return | Keysym::KP_Enter);
        let typing = self.diff.editing.is_some() || self.diff.search_focused();
        let keyed = self.doc.is_some() || self.feature.is_some();
        if keyed && enter && mods.ctrl && !mods.alt && !mods.logo && !typing {
            return match mods.shift {
                false => self.approve(),
                true => self.send_back(env),
            };
        }
        let v = matches!(sym, Keysym::v | Keysym::V);
        if self.doc.is_some() && v && mods.ctrl && mods.shift && !mods.alt && !mods.logo && !typing
        {
            return Some(Done::Preview);
        }
        // Alt+Shift+Down and Up: the next file not viewed, and the one
        // before.
        let up_down = match sym {
            Keysym::Down | Keysym::KP_Down => Some(1),
            Keysym::Up | Keysym::KP_Up => Some(-1),
            _ => None,
        };
        if let Some(by) = up_down.filter(|_| self.feature.is_some() && !typing)
            && mods.alt
            && mods.shift
            && !(mods.ctrl || mods.logo)
        {
            let found = self.with_diff(env, |diff, ctx| diff.jump_unchecked(by, ctx));
            return (!found).then_some(Done::Diff(Outcome::Notice(ALL_VIEWED)));
        }
        let outcome = self.with_diff(env, |diff, ctx| diff.key(mods, sym, typed, ctx));
        let done = match outcome {
            Some(outcome) => self.doc_outcome(outcome).map(Done::Diff),
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
        let mut buttons = Vec::new();
        if !self.read_only() {
            buttons.push(B::Comment);
        }
        if self.feature.is_some() {
            buttons.extend([B::MarkViewed, B::NextUnviewed]);
        }
        if self.reviewed() {
            buttons.extend([B::Approve, B::SendBack]);
        }
        if self.doc.is_some() {
            buttons.push(B::Preview);
        }
        if self.diff.review.own() > 0 {
            buttons.extend([B::CopyReview, B::ClearReview]);
        }
        buttons.extend([B::Help, B::Close]);
        buttons
            .into_iter()
            .map(|button| Hint {
                button,
                label: match button {
                    B::Comment => "comment on the lines".into(),
                    B::Close if self.commit.is_some() => "back".into(),
                    B::Close if self.feature.as_ref().is_some_and(|f| f.back) => "back".into(),
                    B::Close => "close".into(),
                    button => button.label().to_lowercase(),
                },
                key: match button {
                    B::Approve => APPROVE_KEY,
                    B::SendBack => SEND_BACK_KEY,
                    B::Preview => PREVIEW_KEY,
                    B::MarkViewed => "Space",
                    B::NextUnviewed => NEXT_UNVIEWED_KEY,
                    button => key_name(button),
                }
                .into(),
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
        if self.feature.is_some() {
            match hit {
                ReposHit::Button(ReposButton::Approve) => return self.approve(),
                ReposHit::Button(ReposButton::SendBack) => return self.send_back(env),
                ReposHit::Button(ReposButton::MarkViewed) => {
                    return self
                        .with_diff(env, |diff, ctx| diff.toggle_current(ctx))
                        .map(Done::Diff);
                }
                ReposHit::Button(ReposButton::NextUnviewed) => {
                    let found = self.with_diff(env, |diff, ctx| diff.jump_unchecked(1, ctx));
                    return (!found).then_some(Done::Diff(Outcome::Notice(ALL_VIEWED)));
                }
                ReposHit::DiffRestore(_) => {
                    return Some(Done::Diff(Outcome::Notice(FEATURE_READ_ONLY)));
                }
                _ => {}
            }
        }
        if self.doc.is_some() {
            match hit {
                ReposHit::Button(ReposButton::Preview) => return Some(Done::Preview),
                ReposHit::Button(ReposButton::Approve) => return self.approve(),
                ReposHit::Button(ReposButton::SendBack) => return self.send_back(env),
                ReposHit::DiffRestore(_) if self.evidence() => {
                    return Some(Done::Diff(Outcome::Notice(EVIDENCE_READ_ONLY)));
                }
                ReposHit::DiffRestore(_) => {
                    return Some(Done::Diff(Outcome::Notice(DOC_NO_RESTORE)));
                }
                _ => {}
            }
        }
        let outcome = self.with_diff(env, |diff, ctx| diff.click(hit, ctx));
        let outcome = self.doc_outcome(outcome.flatten());
        self.asked(outcome.map(Done::Diff))
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
            let lines = self.help_lines();
            let top = top.min(lines.len().saturating_sub(1));
            return Some(SheetView {
                title: help_title(
                    match (&self.commit, &self.doc) {
                        _ if self.feature.is_some() => "Keys in a feature's code review",
                        (Some(_), _) => "Keys in a commit's diff",
                        (_, Some(doc)) if doc.evidence() => "Keys in the evidence",
                        (_, Some(_)) => "Keys in a file under review",
                        (None, None) => "Keys in a worktree's diff",
                    },
                    top,
                    lines.len(),
                ),
                lines: lines[top..].to_vec(),
                buttons: vec![ReposButton::Close],
                default: 0,
                danger: false,
            });
        }
        let choice = self.clearing?;
        let outdated = self.clearing_outdated;
        let (title, lines) = self.with_ctx(env, |ctx| self.diff.clear_sheet(ctx, outdated));
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

    /// Why the last key or click did nothing, if it says; and what was
    /// decided of an AI review finding.
    pub fn take_error(&mut self) -> (Option<String>, Option<(String, Decision)>) {
        let signals = self.diff.take_signals();
        (signals.error.flatten(), signals.decided)
    }

    /// Whether `path` is this worktree's.
    pub fn is(&self, path: &Path) -> bool {
        self.root == path
    }
}

/// The diff's sections: the worktree at `root` (repository `label`, on
/// `branch`) with its changes `since`; or `feature`'s repositories.
fn sections<'a>(
    root: &'a Path,
    label: &'a str,
    branch: Option<&'a str>,
    since: &'a Since,
    feature: Option<&'a FeatureOf>,
) -> Vec<Section<'a>> {
    match feature {
        Some(feature) => feature.sections(),
        None => vec![Section {
            root,
            label,
            branch,
            head: None,
            changes: &since.changes,
        }],
    }
}

/// The diff's `Ctx` over `sections`, the diff having the keyboard and no
/// file checked; a commit's `read_only`.
fn ctx<'a>(
    sections: &'a [Section<'a>],
    env: Env,
    lead: &'a str,
    empty: &'a str,
    read_only: bool,
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
        note: None,
        selected: None,
        read_only,
    }
}

impl crate::Gerdr {
    /// The diff on show: the keys, clicks and views of the panel act on
    /// it.
    pub(crate) fn wd(&self) -> Option<&WorktreeDiff> {
        self.repos.worktree.as_ref()
    }

    /// `wd`, to change.
    pub(crate) fn wd_mut(&mut self) -> Option<&mut WorktreeDiff> {
        self.repos.worktree.as_mut()
    }

    /// What the worktree diff's views and keys need of the chrome.
    pub(crate) fn worktree_env(&self) -> Env {
        let size = self.repos_rect();
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

    /// Show what commit `oid` (its log line `line`) of the repository at
    /// `root` changed, across the repositories panel, which it goes back
    /// to when closed.
    pub(crate) fn open_commit_diff(&mut self, root: PathBuf, oid: String, line: String) {
        let Some(item) = self.repos.items.iter().find(|i| i.root == root) else {
            return;
        };
        let (label, workspace) = (item.label.clone(), item.workspace);
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let commit = CommitOf { oid, line };
        let wd = WorktreeDiff::of_commit(root, label, commit, workspace);
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
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
    /// pruned; a file under review's in its own file.
    pub(crate) fn worktree_save_review(&mut self) {
        if self.wd().is_some_and(|wd| wd.doc.is_some()) {
            return self.doc_save_review();
        }
        if self.wd().is_some_and(|wd| wd.feature.is_some()) {
            return self.feature_save_review();
        }
        // A commit's diff has no review.
        let Some(wd) = self
            .repos
            .worktree
            .as_mut()
            .filter(|wd| wd.diff.review_dirty && wd.commit.is_none())
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

    /// Close the worktree's diff, back to the worktrees popup; a
    /// commit's, back to the repositories panel.
    pub(crate) fn close_worktree_diff(&mut self) {
        self.worktree_save_review();
        // The next review shows its preview beside it again.
        self.previews.undocked = false;
        // A commit's diff, a file under review and a feature's code review
        // weren't opened from the worktrees popup.
        let other = self
            .wd()
            .is_some_and(|wd| wd.commit.is_some() || wd.doc.is_some() || wd.feature.is_some());
        // Edits to a file under review can't be undone once it closes:
        // its agent may write it again meanwhile.
        let doc = self
            .wd()
            .filter(|wd| wd.doc.is_some())
            .map(|wd| wd.root.clone());
        if let Some(root) = doc {
            self.repos.cuts.retain(|d| d.root != root);
            self.repos.undone.retain(|d| d.root != root);
        }
        self.repos.close_worktree();
        if !other {
            self.worktrees.unpark();
        }
        self.refresh_pointer_focus();
    }

    /// Read the worktree's changes and their diff again, off the event
    /// loop, unless a read is under way.
    pub(crate) fn worktree_diff_read(&mut self) {
        let Some(wd) = self.wd().filter(|wd| !wd.reading) else {
            return;
        };
        if wd.doc.is_some() {
            return self.doc_review_read();
        }
        if wd.feature.is_some() {
            return self.feature_diff_read();
        }
        if let Some(commit) = &wd.commit {
            // A commit doesn't change: read once.
            if wd.read {
                return;
            }
            return self.commit_diff_read(wd.root.clone(), commit.oid.clone());
        }
        let (root, target) = (wd.root.clone(), wd.target.clone());
        let Some(tx) = self.repos_sender() else {
            return;
        };
        if let Some(wd) = self.wd_mut() {
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

    /// Read commit `oid`'s diff of the repository at `root`, off the
    /// event loop.
    fn commit_diff_read(&mut self, root: PathBuf, oid: String) {
        let Some(tx) = self.repos_sender() else {
            return;
        };
        if let Some(wd) = self.wd_mut() {
            wd.reading = true;
        }
        std::thread::spawn(move || {
            let read = crate::git_ops::commit_diff(&root, &oid).map(|(changes, files)| {
                let since = Since {
                    base: format!("{oid}^"),
                    commits: 1,
                    changes,
                };
                (since, files)
            });
            if let Err(why) = &read {
                tracing::warn!("reading commit {oid} in {}: {why}", root.display());
            }
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
        let Some(wd) = self.wd_mut().filter(|wd| wd.is(&root)) else {
            return;
        };
        // Another commit's, opened before this one: not what is shown.
        if let (Some(commit), Ok((since, _))) = (&wd.commit, &read)
            && since.base != format!("{}^", commit.oid)
        {
            return;
        }
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
        Some(self.wd()?.repos_view(self.worktree_env()))
    }

    /// What a key or click on the worktree's diff asked for, as the
    /// panel asks it; why it did nothing said briefly.
    fn worktree_done(&mut self, done: Option<Done>) -> Option<crate::repos::Outcome> {
        self.worktree_save_review();
        let (error, decided) = self
            .wd_mut()
            .map(WorktreeDiff::take_error)
            .unwrap_or_default();
        if let Some(why) = error {
            self.show_warning(why);
        }
        if let Some((id, decision)) = decided {
            self.feature_decide(id, decision);
        }
        let commit = self.wd().is_some_and(|wd| wd.commit.is_some());
        let doc = self.wd().is_some_and(|wd| wd.doc.is_some());
        let feature = self.wd().is_some_and(|wd| wd.feature.is_some());
        match done? {
            Done::Close if feature => {
                self.close_feature_review();
                None
            }
            Done::Diff(Outcome::Toggle { root, path }) if feature => {
                self.feature_toggle(&root, &path);
                None
            }
            Done::Approve if feature => {
                self.feature_approve();
                None
            }
            Done::SendBack(comments) if feature => {
                self.feature_send_back(comments);
                None
            }
            // Back to the panel it was opened from.
            Done::Close if commit => {
                self.close_worktree_diff();
                None
            }
            Done::Close if doc => {
                self.close_doc_review();
                None
            }
            Done::Close => Some(crate::repos::Outcome::Close),
            Done::Diff(outcome) => Some(crate::repos::from_diff(outcome)),
            Done::Approve => {
                self.doc_approve();
                None
            }
            Done::SendBack(comments) => {
                self.doc_send_back(comments);
                None
            }
            Done::Preview => {
                self.toggle_doc_preview();
                None
            }
        }
    }

    /// Do `button` in the worktree's diff, as its footer or a menu asks.
    pub(crate) fn worktree_diff_press(&mut self, button: ReposButton) {
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut() else {
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
        let done = self.wd_mut()?.key(mods, sym, typed, env);
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
        let Some(wd) = self.wd_mut() else {
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
        let Some(wd) = self.wd_mut() else {
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
        if let Some(wd) = self.wd_mut() {
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
        if let Some(wd) = self.wd_mut() {
            wd.stand_at(pos, env);
        }
    }

    /// Move the worktree's diff on to `now` if it moves on its own;
    /// whether it still does.
    pub(crate) fn worktree_diff_tick(&mut self, now: Instant) -> bool {
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut().filter(|wd| wd.scrolling(now)) else {
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
        let key =
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
    fn a_commits_diff_is_read_only() {
        let commit = CommitOf {
            oid: "c1".into(),
            line: "c1 Read the log · lgal, 2 hours ago".into(),
        };
        let mut wd = WorktreeDiff::of_commit("/src/gerdr".into(), "gerdr".into(), commit, 3);
        assert_eq!(wd.heading(), "Commit · gerdr");
        assert_eq!(wd.summary(), "reading…");
        let since = Since {
            base: "c1^".into(),
            commits: 1,
            changes: vec![change("a.rs", 'M')],
        };
        wd.set_read(Ok((since, vec![added("a.rs", 'M', &["a", "b"])])), ENV);
        assert_eq!(wd.summary(), "c1 Read the log · lgal, 2 hours ago");
        assert_eq!((wd.workspace, wd.home), (3, Some(3)));
        assert_eq!(texts(&wd)[1], "file a.rs None");
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let refused = Some(Done::Diff(Outcome::Notice(diff_view::READ_ONLY)));
        // On a line: no comment, delete, yank or restore; no buttons for them.
        wd.click(ReposHit::DiffRow(3), ENV);
        assert_eq!(texts(&wd)[3], "a");
        assert_eq!(key(&mut wd, Keysym::c), refused);
        assert!(wd.diff.editing.is_none());
        key(&mut wd, Keysym::d);
        assert_eq!(key(&mut wd, Keysym::d), refused);
        key(&mut wd, Keysym::y);
        assert_eq!(key(&mut wd, Keysym::y), refused);
        assert_eq!(key(&mut wd, Keysym::R), refused);
        assert_eq!(wd.click(ReposHit::DiffRestore(2), ENV), refused);
        assert!(wd.view(ENV).row_buttons.is_empty());
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert_eq!(hints, [ReposButton::Help, ReposButton::Close]);
        assert_eq!(wd.hints()[1].label, "back");
        // Searching and opening in Zed still work; Esc closes it.
        key(&mut wd, Keysym::g);
        assert!(matches!(
            key(&mut wd, Keysym::space),
            Some(Done::Diff(Outcome::OpenInZed { .. }))
        ));
        assert_eq!(key(&mut wd, Keysym::Escape), Some(Done::Close));
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
        assert!(wd.take_error().0.is_some());
        // A file header's chevron folds it.
        wd.click(ReposHit::DiffFold(1), ENV);
        assert_eq!(texts(&wd)[1..3], ["file a.rs None", "file new.rs None"]);
    }

    #[test]
    fn clearing_its_review_asks_first() {
        let mut wd = read();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
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

    /// Discover's brief of feature "Log in", read: three lines.
    fn doc() -> WorktreeDiff {
        let doc = DocOf {
            project: "p".into(),
            slug: "login".into(),
            file: DocFile::Stage(crate::feature::Stage::Discover),
            path: "/data/features/p/login/brief.md".into(),
            comments: "/data/features/p/login/discover-comments.json".into(),
            title: "Log in".into(),
            state: "ready for review".into(),
            text: None,
            back: false,
            locked: None,
        };
        let mut wd = WorktreeDiff::of_doc(doc, 2, Review::default());
        wd.set_doc(Ok("# Brief\nUsers log in.\nWith a password.\n".into()), ENV);
        wd
    }

    #[test]
    fn a_stage_file_shows_as_added_lines_to_review() {
        let mut wd = doc();
        assert_eq!(wd.heading(), "Discover · brief.md");
        assert_eq!(wd.summary(), "\u{201c}Log in\u{201d} · ready for review");
        assert_eq!(wd.root, PathBuf::from("/data/features/p/login"));
        let rows = texts(&wd);
        assert!(rows.contains(&"file brief.md None".to_owned()), "{rows:?}");
        assert!(rows.ends_with(&[
            "# Brief".into(),
            "Users log in.".into(),
            "With a password.".into()
        ]));
        let buttons: Vec<ReposButton> = wd
            .view(ENV)
            .title_buttons
            .iter()
            .map(|h| h.button)
            .collect();
        assert_eq!(
            buttons,
            [
                ReposButton::Preview,
                ReposButton::SendBack,
                ReposButton::Approve
            ]
        );
        assert!(wd.view(ENV).row_buttons.is_empty());
        let hints: Vec<(ReposButton, String)> =
            wd.hints().into_iter().map(|h| (h.button, h.key)).collect();
        assert_eq!(
            hints[..4],
            [
                (ReposButton::Comment, "c".into()),
                (ReposButton::Approve, APPROVE_KEY.into()),
                (ReposButton::SendBack, SEND_BACK_KEY.into()),
                (ReposButton::Preview, PREVIEW_KEY.into()),
            ]
        );
        let help = wd.help_lines();
        assert_eq!(help[2].0, PREVIEW_KEY);
        assert_eq!(help[3].0, APPROVE_KEY);
        assert_eq!(help[4].0, SEND_BACK_KEY);
        // Ctrl+Shift+V and the button show it rendered; not while a
        // comment is written.
        let ctrl_shift = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        let v = Keysym::V;
        assert_eq!(wd.key(&ctrl_shift, v, v, ENV), Some(Done::Preview));
        let preview = ReposHit::Button(ReposButton::Preview);
        assert_eq!(wd.click(preview, ENV), Some(Done::Preview));
        assert_eq!(wd.press(ReposButton::Preview, ENV), Some(Done::Preview));
        // Its edits are listed; the restore it refuses isn't.
        let keys: Vec<&str> = help.iter().map(|(key, ..)| key.as_str()).collect();
        assert!(keys.contains(&"c / Del Del"), "{keys:?}");
        for edit in ["x X D dd dw de d$ 3dd", "yy yw y$ Y, p / P"] {
            assert!(keys.contains(&edit), "{edit} not in {keys:?}");
        }
        assert!(!keys.contains(&"Shift+R"), "Shift+R in {keys:?}");
    }

    #[test]
    fn its_caret_and_comments_are_found_by_line() {
        let mut wd = doc();
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let enter = Keysym::Return;
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        // Line 2 of the file is the row "Users log in.".
        let row = texts(&wd)
            .iter()
            .position(|t| t == "Users log in.")
            .unwrap();
        assert_eq!(wd.diff.row_of_line(2), Some(row));
        assert_eq!(wd.diff.row_of_line(9), None);
        assert_eq!(wd.diff.cursor_line(), None);
        wd.click(ReposHit::DiffRow(row), ENV);
        assert_eq!(wd.diff.cursor_line(), Some(2));
        // A comment there is told by its line.
        key(&mut wd, Keysym::c);
        key(&mut wd, Keysym::o);
        wd.key(&ctrl, enter, enter, ENV);
        assert_eq!(wd.diff.comment_lines(), [(2, "o".to_owned())]);
        // The caret goes where a preview's block starts.
        let third = wd.diff.row_of_line(3).unwrap();
        wd.with_diff(ENV, |diff, ctx| diff.caret_to(third, 0, ctx));
        assert_eq!(wd.diff.cursor_line(), Some(3));
    }

    #[test]
    fn it_stands_at_a_line_of_its_file() {
        let mut wd = doc();
        let text: String = (1..=80).map(|n| format!("Line {n}.\n")).collect();
        wd.set_doc(Ok(text), ENV);
        let top = |wd: &WorktreeDiff| wd.diff.top_line().unwrap();
        // At the start, the rows above the first line stand for the
        // line before it.
        assert_eq!(top(&wd), 0.0);
        let first = wd.diff.row_of_line(1).unwrap();
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(0.5, ctx));
        assert_eq!(wd.diff.scroll.pos(), first as f64 / 2.0);
        assert!((top(&wd) - 0.5).abs() < 1e-9, "{}", top(&wd));
        // A preview's top line, to the fraction, is the review's too.
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(30.25, ctx));
        assert!((top(&wd) - 30.25).abs() < 1e-9, "{}", top(&wd));
        let row = wd.diff.row_of_line(30).unwrap();
        assert_eq!(wd.diff.scroll.first(), row);
        // Before the first line, the review stands at its start.
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(-4.0, ctx));
        assert_eq!(wd.diff.scroll.pos(), 0.0);
        // Past the end, as far as it scrolls.
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(500.0, ctx));
        assert!(top(&wd) < 80.0, "{}", top(&wd));
    }

    #[test]
    fn its_keys_approve_or_send_back() {
        let mut wd = doc();
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let ctrl_shift = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        let enter = Keysym::Return;
        assert_eq!(wd.key(&ctrl, enter, enter, ENV), Some(Done::Approve));
        // Sending back needs a comment.
        let none_yet = Some(Done::Diff(Outcome::Notice(NO_COMMENT)));
        assert_eq!(wd.key(&ctrl_shift, enter, enter, ENV), none_yet);
        let send = ReposHit::Button(ReposButton::SendBack);
        assert_eq!(wd.click(send, ENV), none_yet);
        // A comment on line 2.
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let row = texts(&wd)
            .iter()
            .position(|t| t == "Users log in.")
            .unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        key(&mut wd, Keysym::c);
        assert!(wd.diff.editing.is_some());
        // While it is written, Ctrl+Enter saves it, not approves.
        key(&mut wd, Keysym::o);
        assert_eq!(
            wd.click(send, ENV),
            Some(Done::Diff(Outcome::Notice(DRAFT_OPEN)))
        );
        wd.key(&ctrl, enter, enter, ENV);
        assert!(wd.diff.editing.is_none());
        assert_eq!(wd.diff.review.comments.len(), 1);
        let Some(Done::SendBack(text)) = wd.key(&ctrl_shift, enter, enter, ENV) else {
            panic!("not sent back");
        };
        assert!(
            text.starts_with("Review comments on brief.md (1 comment)."),
            "{text}"
        );
        assert!(text.contains("### `brief.md:2`\n"), "{text}");
        assert!(text.contains("Users log in."), "{text}");
        assert_eq!(
            wd.click(ReposHit::Button(ReposButton::Approve), ENV),
            Some(Done::Approve)
        );
    }

    #[test]
    fn a_file_under_review_is_edited_but_not_restored() {
        let mut wd = doc();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let no_restore = Some(Done::Diff(Outcome::Notice(DOC_NO_RESTORE)));
        let row = texts(&wd).iter().position(|t| t == "# Brief").unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        key(&mut wd, Keysym::d);
        let Some(Done::Diff(Outcome::Cut(edit))) = key(&mut wd, Keysym::d) else {
            panic!("dd didn't cut");
        };
        assert_eq!(edit.root, Path::new("/data/features/p/login"));
        assert_eq!(edit.path, "brief.md");
        assert!(matches!(
            key(&mut wd, Keysym::x),
            Some(Done::Diff(Outcome::Cut(_)))
        ));
        key(&mut wd, Keysym::y);
        assert!(matches!(
            key(&mut wd, Keysym::y),
            Some(Done::Diff(Outcome::Yank(_)))
        ));
        assert!(matches!(
            key(&mut wd, Keysym::u),
            Some(Done::Diff(Outcome::UndoCut(_)))
        ));
        assert_eq!(key(&mut wd, Keysym::R), no_restore);
        assert_eq!(wd.click(ReposHit::DiffRestore(row), ENV), no_restore);
        assert_eq!(texts(&wd)[row], "# Brief");
        key(&mut wd, Keysym::c);
        assert!(wd.diff.editing.is_some());
        assert_eq!(key(&mut wd, Keysym::Escape), None);
        assert_eq!(key(&mut wd, Keysym::Escape), Some(Done::Close));
        // The keys sheet lists the edits, not the restore.
        let lines = wd.help_lines();
        assert!(
            lines
                .iter()
                .any(|(_, what, _)| what.contains("delete from the file"))
        );
        assert!(
            lines
                .iter()
                .all(|(_, what, _)| !what.contains("restore the change"))
        );
    }

    #[test]
    fn a_locked_file_under_review_says_why_it_isnt_edited() {
        let mut wd = doc();
        let why = "The agent is still writing the file";
        if let Some(doc) = wd.doc.as_mut() {
            doc.locked = Some(why);
        }
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let refused = Some(Done::Diff(Outcome::Notice(why)));
        let row = texts(&wd).iter().position(|t| t == "# Brief").unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        key(&mut wd, Keysym::d);
        assert_eq!(key(&mut wd, Keysym::d), refused);
        assert_eq!(key(&mut wd, Keysym::x), refused);
        assert_eq!(key(&mut wd, Keysym::u), refused);
        // A yank takes nothing from it.
        key(&mut wd, Keysym::y);
        assert!(matches!(
            key(&mut wd, Keysym::y),
            Some(Done::Diff(Outcome::Yank(_)))
        ));
        let lines = wd.help_lines();
        assert!(
            lines
                .iter()
                .all(|(_, what, _)| !what.contains("delete from the file"))
        );
    }

    fn evidence() -> WorktreeDiff {
        let doc = DocOf {
            project: "p".into(),
            slug: "login".into(),
            file: DocFile::Evidence,
            path: "/data/features/p/login/evidence.md".into(),
            comments: "/data/features/p/login/verify-comments.json".into(),
            title: "Log in".into(),
            state: "run 2 failed".into(),
            text: None,
            back: true,
            locked: None,
        };
        let mut wd = WorktreeDiff::of_doc(doc, 2, Review::default());
        wd.set_doc(Ok("# Evidence\napi: npm test failed\n".into()), ENV);
        wd
    }

    #[test]
    fn the_evidence_is_only_read() {
        let mut wd = evidence();
        assert_eq!(wd.heading(), "Evidence of \u{201c}Log in\u{201d}");
        assert_eq!(wd.summary(), "run 2 failed");
        let refused = Some(Done::Diff(Outcome::Notice(EVIDENCE_READ_ONLY)));
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let row = texts(&wd).iter().position(|t| t == "# Evidence").unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        // No comment, edit or restore.
        assert_eq!(key(&mut wd, Keysym::c), refused);
        assert!(wd.diff.editing.is_none());
        key(&mut wd, Keysym::d);
        assert_eq!(key(&mut wd, Keysym::d), refused);
        assert_eq!(key(&mut wd, Keysym::R), refused);
        assert_eq!(wd.click(ReposHit::DiffRestore(row), ENV), refused);
        // No Approve or Send back, by key or button.
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let ctrl_shift = ModifiersState {
            shift: true,
            ..ctrl
        };
        let enter = Keysym::Return;
        assert_eq!(wd.key(&ctrl, enter, enter, ENV), refused);
        assert_eq!(wd.key(&ctrl_shift, enter, enter, ENV), refused);
        let approve = ReposHit::Button(ReposButton::Approve);
        assert_eq!(wd.click(approve, ENV), refused);
        // Nor buttons, hints or keys sheet lines for them.
        // It shows rendered as any file under review does.
        let view = wd.view(ENV);
        let buttons: Vec<_> = view.title_buttons.iter().map(|h| h.button).collect();
        assert_eq!(buttons, [ReposButton::Preview]);
        assert!(view.row_buttons.is_empty());
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert_eq!(
            hints,
            [ReposButton::Preview, ReposButton::Help, ReposButton::Close]
        );
        let lines = wd.help_lines();
        assert_eq!(lines[0].1, "close the evidence");
        assert!(
            lines.iter().all(|(key, what, _)| !what.contains("comment")
                && key != APPROVE_KEY
                && key != SEND_BACK_KEY),
            "{lines:?}"
        );
        assert_eq!(key(&mut wd, Keysym::Escape), Some(Done::Close));
    }

    #[test]
    fn its_comments_follow_the_file_written_again() {
        let mut wd = doc();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let row = texts(&wd)
            .iter()
            .position(|t| t == "With a password.")
            .unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        for sym in [Keysym::c, Keysym::o, Keysym::Return] {
            key(&mut wd, sym);
        }
        assert_eq!(wd.diff.review.comments.len(), 1);
        // A line in front: the comment still finds its line.
        wd.set_doc(
            Ok("# Brief\nIntro.\nUsers log in.\nWith a password.\n".into()),
            ENV,
        );
        assert!(
            wd.doc_markdown().contains("### `brief.md:3`\n"),
            "{}",
            wd.doc_markdown()
        );
        // Its line gone: it says so.
        wd.set_doc(Ok("# Brief\nUsers log in.\n".into()), ENV);
        assert!(
            wd.doc_markdown().contains("(the file changed since)"),
            "{}",
            wd.doc_markdown()
        );
        // Gone: it says why.
        wd.set_doc(Err("brief.md is gone".into()), ENV);
        assert_eq!(wd.summary(), "brief.md is gone");
    }
}
