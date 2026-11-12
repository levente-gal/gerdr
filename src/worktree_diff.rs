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
//! `doc_review`) shows the same way too, as one file of added lines, but
//! flat in a tab of its own (see `doc_tab`): its comments work as on code,
//! and vim's edits change the file. Approve and Send back sit on its header
//! and footer.
//! Once its stage is done the file is only read: Reopen and going on to the
//! current stage take their place, or only going on once it is locked.
//!
//! A feature's code review (see `feature_diff`) shows the same way too:
//! the feature's changes over every repository it touched, one section
//! each, read-only, its files listed beside the diff when there is room
//! (`file_tree::FileTree`), and on its stage's page laid flat under the rail.
//! Each file's checkbox says whether it was viewed; the
//! AI review's findings show as comments of their own; its header has what
//! works at the stage the feature is at (`FeatureOf::buttons`).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Instant, SystemTime},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{Density, DetailView, Hint, ReposButton, ReposHit, SheetView, TreeTool},
    diff::FileDiff,
    diff_view::{self, CheckOf, DiffView, Keys, NoteOf, Outcome, Section},
    doc_review::DocFile,
    doc_tab::DocMode,
    feature::fix_round::FixPhase,
    feature::review_import::Decision,
    feature::review_items::ItemOf,
    feature::{Stage, Standing},
    feature_diff::{FeatureOf, Role},
    file_tree::{Asked, FileTree},
    git::Change,
    git_ops::Since,
    md_preview::PREVIEW_KEY,
    repos::{Grouping, diff_help_lines, help_title, key_name, scroll_lines, sheet_step},
    review::Review,
    scroll::Motion,
};

/// A read's stamp and the diff it read.
pub type Kept = (Stamp, Vec<FileDiff>);

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
    /// What the last read saw of its files and the diff it read then: a
    /// read seeing the same takes that diff again (see `stamp`).
    pub kept: Arc<Mutex<Option<Kept>>>,
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
    /// A feature's code review's list of its files, beside the diff when
    /// there is room for it (`Env::tree_share`).
    pub tree: Option<FileTree>,
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
    /// Why it can't be edited now (`doc_review::locked`): its agent is
    /// writing it, it's approved. None: the diff's edits change it.
    pub locked: Option<&'static str>,
    /// Whether its stage is done, and can still be reopened.
    pub standing: Standing,
    /// The stage the feature is at.
    pub current: Option<Stage>,
    /// What it shows changes against, as last read: the plan, for the final
    /// plan's changes (`DocFile::PlanChanges`); none, it shows all added.
    pub base: Option<String>,
    /// Plan review's suggestions flagged on the plan, as the record had them
    /// last (`DocFile::Suggestions`); none for any other file.
    pub suggestions: Option<crate::plan_doc::SuggestionsOf>,
}

/// What Plan review's suggestions flagged on the plan are called: the
/// stage page's button, the tab and its heading.
pub const REVIEW_IN_PLAN: &str = "Review in plan";

impl DocOf {
    /// What its tab is called.
    pub fn tab_title(&self) -> &'static str {
        match self.file {
            DocFile::Evidence => "Evidence",
            DocFile::FixPlan => "Fix plan",
            DocFile::PlanChanges => "Plan changes",
            DocFile::DraftChanges(..) => "Draft changes",
            // One name for the view: its button's, tab's and heading's.
            DocFile::Suggestions => REVIEW_IN_PLAN,
            DocFile::ConflictPlan(_) => "Conflicts plan",
            DocFile::Stage(Stage::Plan) => "Plan",
            DocFile::Stage(Stage::PlanReview) => "Final plan",
            DocFile::Stage(_) => "Brief",
        }
    }

    /// Verify's `evidence.md`, only read: no comments, no Approve or Send
    /// back.
    pub fn evidence(&self) -> bool {
        self.file == DocFile::Evidence
    }

    /// Whether its stage is done: the file is only read, and has its own
    /// actions.
    pub fn closed(&self) -> bool {
        self.standing != Standing::Open
    }

    /// Why it changes nothing once closed: the suggestions' own reason when
    /// it shows them, else why it can't be edited.
    pub fn why_closed(&self) -> &'static str {
        (self.suggestions.as_ref().and_then(|s| s.frozen))
            .or(self.locked)
            .unwrap_or(crate::doc_review::APPROVED_FILE)
    }

    /// The file's name: `brief.md`, also for a draft kept as `brief-2.md`.
    pub fn name(&self) -> String {
        if let Some(name) = self.file.name() {
            return name.into();
        }
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
/// Why going to the current stage does nothing.
pub const ALL_DONE: &str = "Every stage is done";
/// Why Alt+Shift+Down or Up did nothing.
pub const ALL_VIEWED: &str = "Every file is viewed";
/// Why Shift+A does nothing: no file viewed before changed since.
pub const NOTHING_SINCE: &str = "No file you viewed changed since: all its changes show";
/// Why Send back does nothing yet.
pub const NO_COMMENT: &str = "Add a comment first";
/// Why Approve or Send back wait.
pub const DRAFT_OPEN: &str = "Save or discard the comment first";
/// Why an edit or a restore does nothing in a feature's code review.
pub const FEATURE_READ_ONLY: &str = "A feature's code review can't edit the code";
/// The keys approving the file under review and sending it back.
/// Why Ctrl+Enter makes no fixes while what the user wrote needs a plan.
pub const PLAN_FIRST: &str = "What you wrote needs a plan first: Ctrl+Shift+Enter plans the fixes";
pub const APPROVE_KEY: &str = "Ctrl+Enter";
pub const SEND_BACK_KEY: &str = "Ctrl+Shift+Enter";
/// The key going to the next file not viewed in a feature's code review.
pub const NEXT_UNVIEWED_KEY: &str = "Alt+Shift+Down / Up";

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

/// What the diff asked of its host since it was last asked.
#[derive(Debug, Default)]
pub struct Taken {
    /// Why the last key did nothing.
    pub error: Option<String>,
    /// Each by the kind of item it is on (`diff_view::Signals`).
    pub decided: Option<(ItemOf, String, Decision)>,
    pub delete_own: Option<(ItemOf, String)>,
    pub edit_finding: Option<(ItemOf, String)>,
    /// An item's kind and id, and what the user's comment on it now says.
    pub noted: Option<(ItemOf, String, String)>,
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
    /// How many lines of a file list fit (`chrome::diff_tree_page`).
    pub tree_page: usize,
    /// A file list's share of the width: none, no list.
    pub tree_share: f64,
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
    /// Show the next mode of the file's tab (see `doc_tab`).
    CycleMode,
    /// Reopen the approved file's stage.
    Reopen,
    /// Show the stage the feature is at, from an approved file.
    GoOn,
    /// A feature's diff: send what is to fix to a fix plan.
    PlanFixes,
    /// The same with no plan (`Gerdr::make_finding_fixes`).
    MakeFixes,
    /// A feature's diff: mark every undecided finding to fix.
    FixAll,
    /// A feature's diff: ask for a comment on no line or file.
    AddComment,
    /// A feature's diff: show the fix plan waiting for the user.
    ReviewFixPlan,
    /// The files are listed this way now, for every list of them.
    Grouping(Grouping),
    /// A feature's diff: tick these files of the repository at `root` viewed,
    /// or untick them (a folder's or a repository's checkbox).
    TickFiles {
        root: PathBuf,
        paths: Vec<String>,
    },
}

/// What a worktree's diff is read from: its changes since its base and
/// each changed file's size and time; `None` when its files can't be told
/// apart from its top (`root` isn't one).
pub type Stamp = Option<(Since, Vec<Option<(u64, SystemTime)>>)>;

/// The `Stamp` of the worktree at `root` with changes `since`. An
/// untracked folder counts all it holds; `None` (read again) past
/// `WALK_MAX` entries in one.
fn stamp(root: &Path, since: &Since) -> Stamp {
    if !root.join(".git").exists() {
        return None;
    }
    let mut files = Vec::with_capacity(since.changes.len());
    for change in &since.changes {
        let path = root.join(&change.path);
        let meta = std::fs::symlink_metadata(&path).ok();
        files.push(match meta {
            Some(meta) if meta.is_dir() => Some(walk_meta(&path)?),
            Some(meta) => meta.modified().ok().map(|at| (meta.len(), at)),
            None => None,
        });
    }
    Some((since.clone(), files))
}

/// Most entries `walk_meta` looks at in one folder before giving up.
const WALK_MAX: usize = 20_000;

/// The size of all under folder `dir` and its newest time, its folders'
/// times too: a file added deep inside changes its own folder's. `None`
/// past `WALK_MAX` entries.
fn walk_meta(dir: &Path) -> Option<(u64, SystemTime)> {
    let at = |meta: &std::fs::Metadata| meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let mut newest = at(&std::fs::symlink_metadata(dir).ok()?);
    let (mut size, mut seen, mut todo) = (0, 0, vec![dir.to_path_buf()]);
    while let Some(folder) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > WALK_MAX {
                return None;
            }
            // Not followed: a link counts as itself.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                todo.push(entry.path());
            }
            size += meta.len();
            newest = newest.max(at(&meta));
        }
    }
    Some((size, newest))
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
        // Changes show as changes; a file under review reads as a page.
        diff.plain = !doc.file.shows_changes();
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
        let mut view = crate::chrome::ReposView::diff_only(
            self.heading(),
            self.summary(),
            self.hover,
            self.hints(),
            self.view(env),
            self.sheet(env),
            env.density,
        );
        // An approved file says so at a look, wherever it is scrolled to.
        if self.closed() {
            view.summary_tone = crate::chrome::Tone::Success;
        }
        if let (Some(tree), Some(fe), true) = (&self.tree, &self.feature, env.tree_share > 0.0) {
            view.diff_tree = true;
            view.tree_share = env.tree_share;
            view.empty = fe.empty(self.read).into();
            // Laid into its page, the line over it counts the files.
            view.tree_note = fe.tree_note(self.read).filter(|_| !fe.inline);
            let counts = fe.counts(0);
            // A fix round's files are only read: no ticks to go by.
            view.unchecked = fe.span.is_some();
            view.tree_nav = (!view.unchecked).then_some(counts.viewed < counts.files);
            let check = |root: &Path, path: &str| fe.check(root, path);
            tree.fill(&mut view, &fe.sections(), &self.diff, &check, env.tree_page);
        }
        view
    }

    /// The file list, if it shows at `env`.
    fn tree_shows(&self, env: Env) -> bool {
        self.tree.is_some() && self.feature.is_some() && env.tree_share > 0.0
    }

    /// Follow what the diff did: where its cursor went shows in the file list,
    /// and it took the keyboard from the list.
    pub fn follow_diff(&mut self, env: Env) {
        let (moved, focus) = self.diff.take_followed();
        let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref()) else {
            return;
        };
        if focus {
            tree.focused = false;
        }
        if let Some(moved) = moved {
            tree.follow(moved, &fe.sections(), env.tree_page);
        }
    }

    /// The files listed by `grouping`, and the diff in that order.
    pub fn set_grouping(&mut self, grouping: Grouping, env: Env) {
        let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref()) else {
            return;
        };
        if tree.grouping == grouping {
            return;
        }
        let sections = fe.sections();
        tree.set_grouping(grouping, &sections, env.tree_page);
        for section in &sections {
            let rank = tree.rank(section.changes);
            let of = |path: &str| rank.get(path).copied().unwrap_or(usize::MAX);
            self.diff.order_files(section.root, of);
        }
        self.diff.rebuild(&sections);
    }

    /// The file list's toolbar pressed, as its keys press it.
    fn tree_tool(&mut self, tool: TreeTool, env: Env) -> Option<Done> {
        let grouping = match tool {
            TreeTool::List => Grouping::List,
            TreeTool::Folders => Grouping::Folders,
            TreeTool::Expand | TreeTool::Collapse => {
                let (tree, fe) = (self.tree.as_mut()?, self.feature.as_ref()?);
                let open = tool == TreeTool::Expand;
                tree.fold_all(open, &fe.sections(), env.tree_page);
                return None;
            }
            TreeTool::PrevUnviewed | TreeTool::NextUnviewed => {
                return self.jump_unviewed(tool == TreeTool::NextUnviewed, env);
            }
        };
        if self.tree.as_ref()?.grouping == grouping {
            return None;
        }
        self.set_grouping(grouping, env);
        Some(Done::Grouping(grouping))
    }

    /// Go to the next file not viewed, else the one before: the diff at its
    /// header, its line selected in the file list, the keyboard left where
    /// it was. A toast says when every file is viewed.
    fn jump_unviewed(&mut self, next: bool, env: Env) -> Option<Done> {
        let by = if next { 1 } else { -1 };
        let found = self.with_diff(env, |diff, ctx| diff.jump_unchecked(by, ctx));
        (!found).then_some(Done::Diff(Outcome::Notice(ALL_VIEWED)))
    }

    /// What the file list asked: the diff goes to its line, or a file is
    /// ticked as its header's checkbox ticks it.
    fn tree_asked(&mut self, asked: Option<Asked>) -> Option<Done> {
        let tree = self.tree.as_mut()?;
        match asked? {
            Asked::Show => {
                if let Some(place) = tree.place() {
                    self.diff.show_with_cursor(place);
                }
                None
            }
            Asked::ToDiff => {
                tree.focused = false;
                if let Some(place) = tree.place() {
                    self.diff.show_with_cursor(place);
                }
                None
            }
            Asked::Tick { root, mut paths } if paths.len() == 1 => {
                let path = paths.pop()?;
                Some(Done::Diff(Outcome::Toggle { root, path }))
            }
            Asked::Tick { root, paths } => Some(Done::TickFiles { root, paths }),
        }
    }

    /// A key for the file list, unless the diff is typing: Tab goes between
    /// it and the diff, and its toolbar's keys work from either. Whether it
    /// took the key, and what that asks.
    fn tree_key(&mut self, mods: &ModifiersState, sym: Keysym, env: Env) -> Option<Option<Done>> {
        if !self.tree_shows(env) {
            return None;
        }
        let alt = mods.alt && !(mods.ctrl || mods.shift || mods.logo);
        match sym {
            Keysym::Tab | Keysym::ISO_Left_Tab if !(mods.ctrl || mods.alt || mods.logo) => {
                let tree = self.tree.as_mut()?;
                tree.focused = !tree.focused;
                return Some(None);
            }
            Keysym::f | Keysym::F if alt => {
                let tool = match self.tree.as_ref()?.grouping {
                    Grouping::List => TreeTool::Folders,
                    Grouping::Folders => TreeTool::List,
                };
                return Some(self.tree_tool(tool, env));
            }
            Keysym::Left | Keysym::KP_Left if alt => {
                return Some(self.tree_tool(TreeTool::Collapse, env));
            }
            Keysym::Right | Keysym::KP_Right if alt => {
                return Some(self.tree_tool(TreeTool::Expand, env));
            }
            _ => {}
        }
        let (tree, fe) = (self.tree.as_mut()?, self.feature.as_ref()?);
        if !tree.focused {
            return None;
        }
        let asked = tree.key(mods, sym, &fe.sections(), env.tree_page)?;
        Some(self.tree_asked(asked))
    }

    /// A click on the file list: whether it was, and what that asks.
    fn tree_click(&mut self, hit: ReposHit, env: Env) -> Option<Option<Done>> {
        if !self.tree_shows(env) {
            return None;
        }
        if let ReposHit::Tool(tool) = hit {
            return Some(self.tree_tool(tool, env));
        }
        let page = env.tree_page;
        let (tree, fe) = (self.tree.as_mut()?, self.feature.as_ref()?);
        let sections = fe.sections();
        let asked = match hit {
            ReposHit::Line(i) => tree.press_line(i, &sections, page),
            ReposHit::Check(i) => tree.press_check(i, &sections, page),
            ReposHit::Expander(i) => {
                tree.press_expander(i, &sections, page);
                None
            }
            _ => return None,
        };
        Some(self.tree_asked(asked))
    }

    /// The file on line `i` of the file list opened in Zed, as a double click
    /// does: a folder's or a repository's line opens nothing.
    pub fn open_tree_line(&mut self, i: usize, env: Env) -> Option<Done> {
        let (tree, fe) = (self.tree.as_ref()?, self.feature.as_ref()?);
        let (root, path) = tree.file_at(i, &fe.sections(), env.tree_page)?;
        Some(Done::Diff(Outcome::OpenInZed {
            root,
            paths: vec![path],
            at: None,
        }))
    }

    /// The file list moved by `motion` at `now`.
    pub fn scroll_tree(&mut self, motion: Motion, now: Instant, env: Env) {
        if let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref()) {
            let bounds = tree.bounds(&fe.sections(), env.tree_page, env.density);
            tree.scroll.apply(motion, now, bounds);
        }
    }

    /// Stand the file list at line `pos`, as its thumb held there puts it.
    pub fn stand_tree_at(&mut self, pos: f64, env: Env) {
        if let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref()) {
            let bounds = tree.bounds(&fe.sections(), env.tree_page, env.density);
            tree.scroll.jump(pos.clamp(0.0, bounds.max.max(0.0)));
        }
    }

    /// The file under review read again (`Err`: why it couldn't be): held
    /// as all added lines (saved comments quote them so) and shown as a
    /// page, where it stood; the same text as before left as it is.
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
        let flagged = doc.suggestions.is_some();
        let name = doc.name();
        let file = match &doc.base {
            Some(base) => crate::doc_review::changed_file(&name, base, &text),
            None => crate::doc_review::added_file(&name, &text),
        };
        doc.text = Some(text);
        let since = Since {
            base: String::new(),
            commits: 0,
            changes: vec![Change {
                path: name,
                orig: None,
                code: file.code,
            }],
        };
        self.set_read(Ok((since, vec![file])), env);
        // Where the suggestions go follows the text.
        if flagged {
            self.place_suggestions(env);
        }
    }

    /// Lay `comments`, the items flagged (AI review's findings, Plan
    /// review's suggestions), in place of those laid before, after the
    /// user's own; the cursor on the item `focus` (by id), else on
    /// `focus_note` (one of the user's own), shown. Whether one was.
    pub fn lay_flags(
        &mut self,
        comments: Vec<crate::review::Comment>,
        focus: Option<&str>,
        focus_note: Option<&crate::review::Comment>,
        env: Env,
    ) -> bool {
        let review = &mut self.diff.review;
        review.comments.retain(|c| c.flag.is_none());
        let first = review.comments.len();
        review.comments.extend(comments);
        let at = focus.and_then(|id| {
            let found = review.comments[first..]
                .iter()
                .position(|c| c.flag.as_ref().is_some_and(|f| f.id == id && !f.note));
            found.map(|i| first + i)
        });
        let at = at.or_else(|| {
            review.comments[..first]
                .iter()
                .position(|c| Some(c) == focus_note)
        });
        let branch = self.branch.as_deref();
        let feature = self.feature.as_ref();
        let sections = sections(&self.root, &self.label, branch, &self.since, feature);
        self.diff.rebuild(&sections);
        at.is_some_and(|at| self.with_diff(env, |diff, ctx| diff.show_comment(at, ctx)))
    }

    /// The file under review as the diff has it, if read.
    pub(crate) fn doc_file(&self) -> Option<&FileDiff> {
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

    /// Whether it shows a file only to read whatever its stage: Verify's
    /// evidence, the final plan's changes.
    fn evidence(&self) -> bool {
        (self.doc.as_ref()).is_some_and(|d| d.evidence() || d.file.shows_changes())
    }

    /// Whether it shows a file whose stage is done.
    fn closed(&self) -> bool {
        self.doc.as_ref().is_some_and(DocOf::closed)
    }

    /// Why a file whose stage is done changes nothing.
    fn closed_notice(&self) -> Option<Done> {
        let doc = self.doc.as_ref().filter(|d| d.closed())?;
        Some(Done::Diff(Outcome::Notice(doc.why_closed())))
    }

    /// Whether it shows Plan review's suggestions on the plan: Write final
    /// plan and Ask reviewer where others have Approve and Send back.
    fn suggestions(&self) -> Option<&crate::plan_doc::SuggestionsOf> {
        self.doc.as_ref()?.suggestions.as_ref()
    }

    /// Whether it shows the fix plan: Make fixes where others have Approve.
    fn fix_plan(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.file == DocFile::FixPlan)
    }

    /// Whether its lines take no comments: a commit's, the evidence, a
    /// file whose stage is done.
    fn read_only(&self) -> bool {
        self.commit.is_some() || self.evidence() || self.closed() || self.locked().is_some()
    }

    /// Why it shows a feature's code only to read (`FeatureOf::locked`).
    fn locked(&self) -> Option<&'static str> {
        self.feature.as_ref().and_then(FeatureOf::locked)
    }

    /// Whether the keys changing files are refused though comments aren't:
    /// a feature's code, a file its agent is writing.
    fn no_edits(&self) -> bool {
        self.feature.is_some() || self.doc.as_ref().is_some_and(|d| d.locked.is_some())
    }

    /// Reopen the file's stage, unless it is locked.
    fn reopen(&self) -> Option<Done> {
        let doc = self.doc.as_ref()?;
        match doc.standing {
            Standing::Approved => Some(Done::Reopen),
            _ => self.closed_notice(),
        }
    }

    /// Go to the stage the feature is at, from a file whose stage is done.
    fn go_on(&self) -> Option<Done> {
        match self.doc.as_ref()?.current {
            Some(_) => Some(Done::GoOn),
            None => Some(Done::Diff(Outcome::Notice(ALL_DONE))),
        }
    }

    /// Approve the file under review, unless a comment is being written.
    /// A feature's code review is checked further by its host.
    fn approve(&self) -> Option<Done> {
        if self.evidence() {
            return Some(Done::Diff(Outcome::Notice(EVIDENCE_READ_ONLY)));
        }
        if let Some(closed) = self.closed_notice() {
            return Some(closed);
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
        if let Some(closed) = self.closed_notice() {
            return Some(closed);
        }
        if self.diff.editing.is_some() {
            return Some(Done::Diff(Outcome::Notice(DRAFT_OPEN)));
        }
        if self.feature.is_some() {
            return Some(match self.with_ctx(env, |ctx| self.diff.send_review(ctx)) {
                Ok(Outcome::Copy { text, .. }) => Done::SendBack(text),
                _ => Done::Diff(Outcome::Notice(crate::feature_diff::NO_OWN_COMMENT)),
            });
        }
        // The comments on the suggestions are in their record: the host
        // asks the reviewer about them.
        if self.suggestions().is_some() {
            return Some(Done::SendBack(String::new()));
        }
        if self.diff.review.is_empty() {
            return Some(Done::Diff(Outcome::Notice(NO_COMMENT)));
        }
        Some(Done::SendBack(self.doc_markdown()))
    }

    /// `button` of a feature's diff (`FeatureOf::buttons`): one it doesn't
    /// have now says why, as does one greyed once its host looks.
    fn feature_press(&self, button: ReposButton, env: Env) -> Option<Done> {
        use ReposButton as B;
        let fe = self.feature.as_ref()?;
        let has = (fe.buttons(self.diff.review.own()).iter()).any(|(b, _)| *b == button);
        if !has {
            let own = self.diff.review.own();
            let plan_first = button == B::MakeFixes
                && fe.role() == Role::Deciding
                && fe.needs_plan(own)
                && fe.buttons(own).iter().any(|(b, _)| *b == B::PlanFixes);
            return Some(Done::Diff(Outcome::Notice(match plan_first {
                true => PLAN_FIRST,
                false => fe.not_now(),
            })));
        }
        if self.diff.editing.is_some() {
            return Some(Done::Diff(Outcome::Notice(DRAFT_OPEN)));
        }
        match button {
            B::Approve => self.approve(),
            B::PlanFixes if fe.role() == Role::Review => self.send_back(env),
            B::PlanFixes => Some(Done::PlanFixes),
            B::MakeFixes => Some(Done::MakeFixes),
            B::FixAllFindings => Some(Done::FixAll),
            B::AddComment => Some(Done::AddComment),
            B::ReviewFixPlan => Some(Done::ReviewFixPlan),
            B::GoOn => Some(Done::GoOn),
            B::Reopen => Some(Done::Reopen),
            _ => None,
        }
    }

    /// Whether it is a file under review: not the evidence, nor one whose
    /// stage is done.
    fn reviewed(&self) -> bool {
        self.doc.is_some() && !self.evidence() && !self.closed()
    }

    /// What the diff asked for, on a file under review: edits only while
    /// it isn't locked, no restore, and the review copied as the agent is
    /// sent it.
    fn doc_outcome(&self, outcome: Option<Outcome>) -> Option<Outcome> {
        if self.feature.is_some() {
            return match outcome {
                Some(Outcome::Notice(diff_view::READ_ONLY)) if self.locked().is_some() => {
                    self.locked().map(Outcome::Notice)
                }
                Some(o) if o.edits() => Some(Outcome::Notice(FEATURE_READ_ONLY)),
                outcome => outcome,
            };
        }
        if self.doc.is_none() {
            return outcome;
        }
        let refused = |o: &Outcome| o.edits() || *o == Outcome::Notice(diff_view::READ_ONLY);
        let restores = |o: &Outcome| matches!(o, Outcome::Cut(e) if e.cut.restored().is_some());
        let locked = self.doc.as_ref().and_then(|d| d.locked);
        match outcome {
            Some(o) if self.evidence() && refused(&o) => {
                Some(Outcome::Notice(locked.unwrap_or(EVIDENCE_READ_ONLY)))
            }
            // Its comments and edits are refused as read-only: it says why.
            Some(Outcome::Notice(diff_view::READ_ONLY)) if self.closed() => {
                let frozen = self.suggestions().and_then(|s| s.frozen);
                frozen.or(locked).map(Outcome::Notice)
            }
            Some(o) if restores(&o) => Some(Outcome::Notice(DOC_NO_RESTORE)),
            Some(o) if o.edits() && locked.is_some() => locked.map(Outcome::Notice),
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
            what.contains("delete from the file") || what.contains("restore the change")
        };
        let yank = |what: &str| what.contains("yank");
        // Evidence takes no comments either.
        let comments = |what: &str| what.contains("comment");
        // A file under review is edited while unlocked, never restored.
        let restore = |what: &str| what.contains("restore the change");
        let locked = self.doc.as_ref().is_some_and(|d| d.locked.is_some());
        let read_only = self.commit.is_some() || self.feature.is_some() || self.evidence();
        let evidence = self.evidence();
        let closed = self.closed();
        // A file under review is a page: no other file to go to, no header.
        let other_file = |what: &str| what.contains("the file before or after");
        let on_page = self.doc.is_some();
        let flagged = self.suggestions().is_some();
        let diff = (diff_help_lines().into_iter())
            .filter(|(_, what, _)| {
                !(read_only && edits(what))
                    && !(locked && edits(what))
                    && !(on_page && (restore(what) || other_file(what)))
                    && !((evidence || closed) && comments(what))
            })
            // Yanks work on any diff, pastes only where it can be edited.
            .map(
                |(key, what, tone)| match (read_only || locked) && yank(&what) {
                    true => (
                        key.replace(", p / P", ""),
                        what.replace("; paste after, before", ""),
                        tone,
                    ),
                    false => (key, what, tone),
                },
            )
            .map(|(key, what, tone)| match on_page {
                true => (
                    key.replace(" { }", ""),
                    (what.replace(" (on a file's header, the file)", ""))
                        .replace("; on a file's name, the file", "")
                        .replace("find its lines, paths, hunks", "find in the file")
                        .replace(
                            "the top, the end, the hunk before or after,",
                            "go to the top, the end,",
                        )
                        .replace(
                            ": the selection, a line, a file's path, a hunk",
                            ": the selection or the line",
                        )
                        .replace("in the diff, ", "in the review, "),
                    tone,
                ),
                false => (key, what, tone),
            })
            // On the plan, comments go on the suggestions.
            .map(|(key, what, tone)| match flagged {
                true => (
                    key,
                    (what.replace("finding or comment", "suggestion")).replace(
                        "comment on the lines or edit the comment; delete it",
                        "comment on the suggestion, or answer its question; delete the comment",
                    ),
                    tone,
                ),
                false => (key, what, tone),
            });
        if self.doc.is_none() && self.feature.is_none() {
            return diff.collect();
        }
        let feature_keys;
        let own: &[(&str, &str)] = match (evidence, &self.feature) {
            (_, Some(fe)) => {
                feature_keys = fe.keys(self.diff.review.own());
                &feature_keys
            }
            (false, _) if closed => &[
                ("Esc", "back to the feature"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "go to the next of Preview, Review and Split"),
                (APPROVE_KEY, "go to the stage the feature is at"),
                (SEND_BACK_KEY, "reopen the file, unless it is locked"),
            ],
            (true, _) => &[
                ("Esc", "back to the feature"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "go to the next of Preview, Review and Split"),
            ],
            (false, _) if flagged => &[
                ("Esc", "back to the feature"),
                ("F1", "list the keys"),
                (APPROVE_KEY, "write the final plan from what you decided"),
                (SEND_BACK_KEY, "ask the reviewer about your comments"),
                ("f", "accept the suggestion, or take that back"),
                ("Del", "reject it, saying why, or reopen it"),
                ("Shift+E", "write its change in your own words"),
            ],
            (false, _) if self.fix_plan() => &[
                ("Esc", "back to the feature"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "go between Review and Split"),
                (APPROVE_KEY, "make the fixes as the file says"),
                (SEND_BACK_KEY, "send it back to its agent with the comments"),
                ("Shift+C", "in the review, comment on the whole file"),
            ],
            (false, _) => &[
                ("Esc", "back to the feature"),
                ("F1", "list the keys"),
                (PREVIEW_KEY, "go between Review and Split"),
                (APPROVE_KEY, "approve the file as it is"),
                (SEND_BACK_KEY, "send it back to its agent with the comments"),
                ("Shift+C", "in the review, comment on the whole file"),
            ],
        };
        let list: &[(&str, &str)] = match self.tree.is_some() {
            true => &[
                ("Tab", "go between the file list and the diff"),
                (
                    "Space",
                    "in the file list, mark the line's files viewed, or not",
                ),
                (
                    "Enter",
                    "in the file list, fold the line, or go to the file's diff",
                ),
                ("Alt+F", "list the files flat, or by folder"),
                ("Alt+Left / Right", "fold or unfold the whole file list"),
            ],
            false => &[],
        };
        let on_page: &[(&str, &str)] = match self.feature.as_ref().is_some_and(|fe| fe.inline) {
            true => &[(
                "Alt+1 / Alt+2",
                "show the code, or the fixes the comments asked for",
            )],
            false => &[],
        };
        let list = list.iter().chain(on_page);
        let own = own.iter().chain(list).map(|(key, what)| {
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
        let no_edits = self.no_edits();
        let WorktreeDiff {
            root,
            label,
            branch,
            since,
            diff,
            feature,
            tree,
            ..
        } = self;
        let feature = feature.as_ref();
        let tree = tree.as_ref().filter(|_| env.tree_share > 0.0);
        let sections = sections(root, label, branch.as_deref(), since, feature);
        let check =
            |root: &Path, path: &str| feature.map(|f| f.check(root, path)).unwrap_or_default();
        let note = |root: &Path, path: &str| feature.and_then(|f| f.note(root, path));
        let marks: (Option<CheckOf>, Option<NoteOf>) = match feature {
            Some(fe) if fe.span.is_none() => (Some(&check), Some(&note)),
            _ => (None, None),
        };
        let mut ctx = ctx(&sections, env, &lead, empty, read_only);
        ctx.no_edits = no_edits;
        (ctx.check, ctx.note) = marks;
        ctx.count_sections = feature.is_some_and(|f| f.repos.len() > 1);
        if let Some(tree) = tree {
            ctx.selected = tree.selected_file();
            if tree.focused {
                ctx.keys = Keys::Other;
            }
        }
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
            Some(fe) if fe.span.is_none() => (Some(&check), Some(&note)),
            _ => (None, None),
        };
        let mut ctx = ctx(&sections, env, &lead, self.empty(), self.read_only());
        ctx.no_edits = self.no_edits();
        (ctx.check, ctx.note) = marks;
        ctx.count_sections = feature.is_some_and(|f| f.repos.len() > 1);
        if let Some(tree) = self.tree.as_ref().filter(|_| env.tree_share > 0.0) {
            ctx.selected = tree.selected_file();
            if tree.focused {
                ctx.keys = Keys::Other;
            }
        }
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
    pub(crate) fn lead(&self) -> String {
        if let Some(fe) = &self.feature {
            return match (&fe.span, fe.since) {
                (Some(_), _) => "What its fixes changed".into(),
                (None, true) => "Since last review".into(),
                (None, false) => "Since its base".into(),
            };
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
            return feature.heading();
        }
        if self.commit.is_some() {
            return format!("Commit · {}", self.label);
        }
        if let Some(doc) = &self.doc {
            if doc.evidence() {
                return format!("Evidence of \u{201c}{}\u{201d}", doc.title);
            }
            if doc.file == DocFile::Suggestions {
                return format!("{REVIEW_IN_PLAN} \u{b7} {}", doc.name());
            }
            if doc.file == DocFile::PlanChanges {
                return format!("Plan review \u{b7} what {} changed", doc.name());
            }
            if let DocFile::DraftChanges(stage, k) = doc.file {
                return format!(
                    "{} \u{b7} what draft {k} of {} changed",
                    stage.label(),
                    doc.name()
                );
            }
            // A fix round's plan is the Build agent's, for the stage that
            // asked: its state after the heading names both.
            if doc.file == DocFile::FixPlan {
                return format!("Build agent's fix plan · {}", doc.name());
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
        crate::chrome::mark_primary(&mut view.title_buttons);
        view
    }

    /// A file under review's view: its lines counted in the title, no
    /// Restore buttons, Approve and Send back on the title row.
    fn doc_view(&self, view: &mut DetailView) {
        if let (Some(doc), Some(file)) = (
            &self.doc,
            self.doc_file().filter(|_| self.feature.is_none()),
        ) && self.diff.mode_title(false).is_none()
        {
            view.title = match (doc.file.shows_changes(), file.added) {
                // Changes to another file: what was added and taken out.
                (true, _) => format!("{} · +{} \u{2212}{}", doc.name(), file.added, file.removed),
                (false, 1) => format!("{} · 1 line", doc.name()),
                (false, n) => format!("{} · {n} lines", doc.name()),
            };
        }
        (view.row_buttons).retain(|(_, hint)| hint.button != ReposButton::Restore);
        let hint = |button: ReposButton, key: &str| Hint {
            button,
            label: button.label().into(),
            key: key.into(),
            disabled: None,
            style: Default::default(),
        };
        if self.evidence() {
            return;
        }
        if let Some(fe) = &self.feature {
            // Laid into its page, the review's actions are on the page's line
            // over it (`ReviewSwitch::buttons`).
            if !fe.inline {
                view.title_buttons.extend(self.feature_hints());
                // A popup over the feature's page: a way out by the mouse too.
                view.title_buttons.push(hint(ReposButton::Close, "Esc"));
            }
            // Findings are decided at AI review alone: past it their buttons
            // would only refuse.
            if fe.role() != Role::Deciding {
                view.row_buttons.retain(|(_, hint)| {
                    !matches!(
                        hint.button,
                        ReposButton::FixFinding
                            | ReposButton::DismissFinding
                            | ReposButton::ReopenFinding
                    )
                });
            }
            return;
        }
        if let Some(doc) = self.doc.as_ref().filter(|d| d.closed()) {
            if doc.standing == Standing::Approved {
                view.title_buttons
                    .push(hint(ReposButton::Reopen, SEND_BACK_KEY));
            }
            if let Some(current) = doc.current {
                view.title_buttons.push(Hint {
                    label: format!("Go to {}", current.label()),
                    ..hint(ReposButton::GoOn, APPROVE_KEY)
                });
            }
            return;
        }
        // The suggestions come to the final plan, or go back to the reviewer.
        // Both stay, greyed with why while they can't be pressed: as on the
        // stage's page, the final plan waits until nothing is to decide.
        if let Some(of) = self.suggestions() {
            view.title_buttons.push(Hint {
                label: crate::plan_doc::ASK_REVIEWER.into(),
                disabled: (!of.asks())
                    .then(|| "Comment on a suggestion or reject one first".to_owned()),
                ..hint(ReposButton::SendBack, SEND_BACK_KEY)
            });
            view.title_buttons.push(Hint {
                label: of.approve_label().into(),
                disabled: of.to_decide(),
                ..hint(ReposButton::Approve, APPROVE_KEY)
            });
            return;
        }
        // Going to its agent already, or over: these would only refuse.
        let fixing = |d: &DocOf| {
            matches!(
                d.locked,
                Some(crate::doc_review::FIXING_THEM | crate::doc_review::FIXES_MADE)
            )
        };
        if self.fix_plan() && self.doc.as_ref().is_some_and(fixing) {
            return;
        }
        // Its agent stopped with fixes left: they go on, not back to be planned.
        let stopped = |d: &DocOf| d.state.contains(crate::doc_review::STOPPED);
        let stopped = self.fix_plan() && self.doc.as_ref().is_some_and(stopped);
        let go = match self.fix_plan() {
            true => Hint {
                label: "Make fixes".to_owned(),
                ..hint(ReposButton::Approve, APPROVE_KEY)
            },
            false => hint(ReposButton::Approve, APPROVE_KEY),
        };
        view.title_buttons
            .push(hint(ReposButton::CommentFile, "Shift+C"));
        // The one to press goes rightmost, filled: Send back once a comment
        // is written, else Approve.
        let back = (!stopped).then(|| hint(ReposButton::SendBack, SEND_BACK_KEY));
        match back {
            Some(back) if self.diff.review.own() > 0 => view.title_buttons.extend([go, back]),
            back => view.title_buttons.extend(back.into_iter().chain([go])),
        }
    }

    /// A feature's buttons as it stands, with their keys; Approve with what
    /// holds it up.
    pub(crate) fn feature_hints(&self) -> Vec<Hint> {
        let Some(fe) = self.feature.as_ref() else {
            return Vec::new();
        };
        (fe.buttons(self.diff.review.own()).into_iter())
            .map(|(button, disabled)| Hint {
                button,
                label: match button {
                    ReposButton::GoOn => fe.go_on_label(),
                    _ => button.label().into(),
                },
                key: fe.key_of(button, self.diff.review.own()).into(),
                disabled: match button {
                    ReposButton::Approve => (self.approve_gate(fe.stage))
                        .and_then(|gate| crate::feature_diff::approve_refusal(&gate)),
                    _ => disabled,
                },
                style: Default::default(),
            })
            .collect()
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
        let keyed = self.doc.is_some();
        if self.closed() && enter && mods.ctrl && !mods.alt && !mods.logo && !typing {
            return match mods.shift {
                false => self.go_on(),
                true => self.reopen(),
            };
        }
        if keyed && enter && mods.ctrl && !mods.alt && !mods.logo && !typing {
            return match mods.shift {
                false => self.approve(),
                true => self.send_back(env),
            };
        }
        if let Some(fe) = self
            .feature
            .as_ref()
            .filter(|_| !typing && !mods.alt && !mods.logo)
        {
            use ReposButton as B;
            let button = match sym {
                _ if enter && mods.ctrl => Some(match (mods.shift, fe.role()) {
                    (shift, Role::Deciding) => fe.deciding_button(shift, self.diff.review.own()),
                    (false, Role::Done) => B::GoOn,
                    (true, Role::Done) => B::Reopen,
                    (true, _) => B::PlanFixes,
                    (false, Role::Round(FixPhase::Ready)) => B::ReviewFixPlan,
                    (false, _) => B::Approve,
                }),
                Keysym::c | Keysym::C if mods.shift && !mods.ctrl => Some(B::AddComment),
                Keysym::f | Keysym::F if mods.shift && !mods.ctrl => Some(B::FixAllFindings),
                Keysym::a | Keysym::A if mods.shift && !mods.ctrl => {
                    return self.toggle_scope(env);
                }
                _ => None,
            };
            if let Some(button) = button {
                return self.feature_press(button, env);
            }
        }
        if !typing && crate::repos::clear_review_key(mods, sym) && self.diff.review.own() > 0 {
            return self.click(ReposHit::Button(ReposButton::ClearReview), env);
        }
        let v = matches!(sym, Keysym::v | Keysym::V);
        if self.doc.is_some() && v && mods.ctrl && mods.shift && !mods.alt && !mods.logo && !typing
        {
            return Some(Done::CycleMode);
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
            return self.jump_unviewed(by > 0, env);
        }
        if !typing && let Some(done) = self.tree_key(mods, sym, env) {
            return done;
        }
        // The list has no field to type in: a key it doesn't take is the
        // diff's, which gets the keyboard with it. Esc still closes from
        // the list.
        let listed = !typing
            && sym != Keysym::Escape
            && self.tree_shows(env)
            && self.tree.as_ref().is_some_and(|tree| tree.focused);
        if let Some(tree) = self.tree.as_mut().filter(|_| listed) {
            tree.focused = false;
        }
        let outcome = self.with_diff(env, |diff, ctx| diff.key(mods, sym, typed, ctx));
        if let Some(tree) = self.tree.as_mut().filter(|_| listed && outcome.is_none()) {
            tree.focused = true;
        }
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
        let flagged = self.suggestions();
        // On the plan, comments go on the suggestions alone.
        let on_flag = self.diff.on_finding();
        if !self.read_only() && (flagged.is_none() || on_flag) {
            buttons.push(B::Comment);
        }
        buttons.push(B::CopyLines);
        if self.feature.is_some() && self.locked().is_none() {
            buttons.extend([B::MarkViewed, B::NextUnviewed]);
        }
        if self.diff.comments_to_visit().is_some() {
            buttons.push(B::NextComment);
        }
        if self.reviewed() {
            buttons.push(B::Approve);
            if flagged.is_none_or(|of| of.asks()) {
                buttons.push(B::SendBack);
            }
        }
        let feature_hints = self.feature_hints();
        buttons.extend(feature_hints.iter().map(|hint| hint.button));
        if let Some(doc) = self.doc.as_ref().filter(|d| d.closed()) {
            if doc.standing == Standing::Approved {
                buttons.push(B::Reopen);
            }
            if doc.current.is_some() {
                buttons.push(B::GoOn);
            }
        }
        if self.diff.review.own() > 0 {
            buttons.extend([B::CopyReview, B::ClearReview]);
        }
        buttons.extend([B::Help, B::Close]);
        let mut hints: Vec<Hint> = buttons
            .into_iter()
            .map(|button| Hint {
                button,
                label: match button {
                    // With no cursor on a page, the key takes the whole file.
                    B::Comment if self.diff.plain && self.diff.cursor.is_none() => {
                        "comment on the file".into()
                    }
                    B::Comment if flagged.is_some() && self.diff.on_yours() => {
                        "edit your suggestion".into()
                    }
                    B::Comment if flagged.is_some() => "comment on the suggestion".into(),
                    B::Comment if self.diff.on_yours() => "edit the comment".into(),
                    B::Comment if self.diff.on_finding() => "edit the finding".into(),
                    B::Comment => "comment on the lines".into(),
                    B::Close if self.commit.is_some() || self.doc.is_some() => "back".into(),
                    B::Close if self.feature.as_ref().is_some_and(|f| f.inline) => "back".into(),
                    B::Close if self.feature.as_ref().is_some_and(|f| f.back) => "back".into(),
                    B::Close => "close".into(),
                    B::NextUnviewed => "next, previous unviewed".into(),
                    B::NextComment => match self.diff.comments_to_visit() {
                        Some(noun @ ("finding" | "suggestion")) => {
                            format!("next, previous {noun}")
                        }
                        _ => "next, previous comment".into(),
                    },
                    B::GoOn => match (self.doc.as_ref().and_then(|d| d.current))
                        .or_else(|| self.feature.as_ref().and_then(|f| f.stage))
                    {
                        Some(current) => format!("go to {}", current.label()),
                        None => "go on".into(),
                    },
                    B::Approve if self.fix_plan() => "make fixes".into(),
                    B::Approve if flagged.is_some() => (flagged)
                        .map(|of| of.approve_label().to_lowercase())
                        .unwrap_or_default(),
                    B::SendBack if flagged.is_some() => {
                        crate::plan_doc::ASK_REVIEWER.to_lowercase()
                    }
                    // As its button over the diff says it: "copy 2 comments".
                    B::CopyReview => match self.diff.review.own() {
                        1 => "copy 1 comment".into(),
                        n => format!("copy {n} comments"),
                    },
                    button => button.label().to_lowercase(),
                },
                key: match button {
                    B::Approve | B::GoOn => APPROVE_KEY,
                    B::SendBack | B::Reopen => SEND_BACK_KEY,
                    B::MarkViewed => "Space",
                    B::NextUnviewed => NEXT_UNVIEWED_KEY,
                    B::NextComment => "] / [",
                    button => match self.feature.as_ref() {
                        Some(fe) if !fe.key_of(button, self.diff.review.own()).is_empty() => {
                            fe.key_of(button, self.diff.review.own())
                        }
                        _ => key_name(button),
                    },
                }
                .into(),
                disabled: (feature_hints.iter())
                    .find(|hint| hint.button == button)
                    .and_then(|hint| hint.disabled.clone()),
                style: Default::default(),
            })
            .collect();
        // On a finding or suggestion, what its buttons decide, by their
        // words ("f accept", "Del reject…"), after the comment's hint.
        let decide = (self.diff.finding_buttons().into_iter())
            .filter(|h| h.button != B::Comment)
            .map(|h| Hint {
                label: h.label.to_lowercase(),
                ..h
            });
        let at = hints
            .iter()
            .position(|h| h.button == B::Comment)
            .map_or(0, |i| i + 1);
        hints.splice(at..at, decide);
        hints
    }

    /// A feature's code review switched between what changed since the
    /// last review and all its changes (`FeatureOf::since`): the files viewed
    /// and unchanged fold with the first and open with the second.
    fn toggle_scope(&mut self, env: Env) -> Option<Done> {
        if !self.read {
            let still = crate::feature_diff::STILL_READING;
            return Some(Done::Diff(Outcome::Notice(still)));
        }
        let fe = self.feature.as_mut()?;
        if fe.scope_button().is_none() {
            return Some(Done::Diff(Outcome::Notice(NOTHING_SINCE)));
        }
        fe.since = !fe.since;
        let fold = crate::feature_diff::since_review(fe);
        let shown: Vec<(PathBuf, Vec<FileDiff>)> = (fe.repos.iter())
            .filter(|r| r.error.is_none())
            .map(|r| {
                (
                    r.root.clone(),
                    fe.shown_files(r, &self.diff.review.comments),
                )
            })
            .collect();
        match fe.since {
            true => self.diff.folded.extend(fold),
            false => {
                for file in &fold {
                    self.diff.folded.remove(file);
                }
            }
        }
        self.with_diff(env, |diff, ctx| {
            let reads = diff.reads;
            for (root, files) in shown {
                diff.set_diffs(root, reads, files, ctx);
            }
        });
        self.place_findings(env);
        None
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
        if let Some(done) = self.tree_click(hit, env) {
            return done;
        }
        if self.feature.is_some() {
            match hit {
                ReposHit::Button(ReposButton::AllChanges | ReposButton::SinceReview) => {
                    return self.toggle_scope(env);
                }
                ReposHit::Button(
                    button @ (ReposButton::Approve
                    | ReposButton::PlanFixes
                    | ReposButton::MakeFixes
                    | ReposButton::FixAllFindings
                    | ReposButton::AddComment
                    | ReposButton::ReviewFixPlan
                    | ReposButton::GoOn
                    | ReposButton::Reopen),
                ) => return self.feature_press(button, env),
                ReposHit::Button(ReposButton::MarkViewed) => {
                    return self
                        .with_diff(env, |diff, ctx| diff.toggle_current(ctx))
                        .map(Done::Diff);
                }
                ReposHit::Button(ReposButton::NextUnviewed) => {
                    return self.jump_unviewed(true, env);
                }
                ReposHit::DiffRestore(_) => {
                    return Some(Done::Diff(Outcome::Notice(FEATURE_READ_ONLY)));
                }
                _ => {}
            }
        }
        if self.doc.is_some() {
            match hit {
                ReposHit::Button(ReposButton::Approve) => return self.approve(),
                ReposHit::Button(ReposButton::SendBack) => return self.send_back(env),
                ReposHit::Button(ReposButton::Reopen) => return self.reopen(),
                ReposHit::Button(ReposButton::GoOn) => return self.go_on(),
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
                        (_, Some(doc)) if doc.suggestions.is_some() => {
                            "Keys in the suggestions on the plan"
                        }
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

    /// A press on row `i` of the diff, if it is on the comment being
    /// written: see `DiffView::press_editor`.
    pub fn press_editor(&mut self, i: usize, extend: bool, count: u32, env: Env) -> bool {
        self.with_diff(env, |diff, ctx| diff.press_editor(i, extend, count, ctx))
    }

    /// A press on the comment being written dragged to char `col` of row
    /// `i` of the diff: see `DiffView::drag_editor`.
    pub fn drag_editor(&mut self, i: usize, col: usize, env: Env) {
        self.with_diff(env, |diff, ctx| diff.drag_editor(i, col, ctx));
    }

    /// A press on the comment being written dragged past the diff's rows:
    /// see `DiffView::drag_editor_past`.
    pub fn drag_editor_past(&mut self, by: i64, col: usize, env: Env) {
        self.with_diff(env, |diff, ctx| diff.drag_editor_past(by, col, ctx));
    }

    /// The press held past the diff's rows, `by` rows on (up negative).
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
        let mut moving = self.with_diff(env, |diff, ctx| diff.scroll_tick(now, ctx));
        if let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref())
            && tree.scroll.moving(now)
        {
            let bounds = tree.bounds(&fe.sections(), env.tree_page, env.density);
            moving |= tree.scroll.tick(now, bounds);
        }
        moving
    }

    /// Stand at row `pos`, as the scrollbar's thumb held there puts it.
    pub fn stand_at(&mut self, pos: f64, env: Env) {
        self.with_diff(env, |diff, ctx| diff.stand_at(pos, ctx));
    }

    /// Whether it moves on its own or under the fingers at `now`.
    pub fn scrolling(&self, now: Instant) -> bool {
        self.diff.scroll.moving(now) || self.tree.as_ref().is_some_and(|t| t.scroll.moving(now))
    }

    /// Why the last key or click did nothing, if it says; and what was
    /// decided of an AI review finding, and the finding to edit.
    pub fn take_error(&mut self) -> Taken {
        let signals = self.diff.take_signals();
        Taken {
            error: signals.error.flatten(),
            decided: signals.decided,
            delete_own: signals.delete_own,
            edit_finding: signals.edit_finding,
            noted: signals.noted,
        }
    }

    /// Whether `path` is this worktree's.
    pub fn is(&self, path: &Path) -> bool {
        self.root == path
    }

    /// Whether `path` is a checkout it shows: its worktree, or one of its
    /// feature's repositories, each in a worktree of its own.
    pub fn shows(&self, path: &Path) -> bool {
        self.is(path)
            || (self.feature.as_ref())
                .is_some_and(|fe| fe.repos.iter().any(|repo| repo.root == path))
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
        no_edits: false,
    }
}

impl crate::Gerdr {
    /// The diff on show: the keys, clicks and views of the panel act on
    /// it.
    pub(crate) fn wd(&self) -> Option<&WorktreeDiff> {
        if let Some(wd) = &self.repos.worktree {
            return Some(wd);
        }
        if self.repos.panel.is_some() {
            return None;
        }
        match self.review_tab() {
            Some(tab) => Some(&tab.wd),
            None => self.inline_wd(),
        }
    }

    /// `wd`, to change.
    pub(crate) fn wd_mut(&mut self) -> Option<&mut WorktreeDiff> {
        if self.repos.worktree.is_some() {
            return self.repos.worktree.as_mut();
        }
        if self.repos.panel.is_some() {
            return None;
        }
        if self.review_tab().is_some() {
            return Some(&mut self.review_tab_mut()?.wd);
        }
        self.inline_wd_mut()
    }

    /// What the worktree diff's views and keys need of the chrome.
    pub(crate) fn worktree_env(&self) -> Env {
        if !self.repos.is_open()
            && let Some(tab) = self.review_tab()
        {
            return self.doc_env(tab.mode);
        }
        let list = self.wd().is_some_and(|wd| wd.tree.is_some());
        self.env_in(self.repos_rect(), list)
    }

    /// What a diff in `size`, with a file list or not, needs of the chrome.
    pub(crate) fn env_in(&self, size: crate::chrome::Rect, list: bool) -> Env {
        let tree_share = match list && size.size.w >= crate::chrome::REVIEW_TREE_MIN_PANEL {
            true => (self.repos.review_tree_share).unwrap_or(crate::chrome::REVIEW_TREE_SHARE),
            false => 0.0,
        };
        Env {
            page: crate::chrome::diff_only_page(size, self.density),
            cols: crate::chrome::diff_tree_cols(size, tree_share),
            density: self.density,
            sheet_rows: crate::chrome::repos_sheet_rows(size, true),
            tree_page: crate::chrome::diff_tree_page(size, self.density),
            tree_share,
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

    /// Show commit `oid` of a feature's repository at `root`, where a comment's
    /// fix was made, read-only over the feature's review: closing it goes
    /// back to the feature's page.
    fn open_fix_commit(&mut self, root: PathBuf, oid: String) {
        let label = (self.wd().and_then(|wd| wd.feature.as_ref()))
            .and_then(|fe| fe.repos.iter().find(|r| r.root == root))
            .map_or_else(|| root.display().to_string(), |r| r.name.clone());
        if self.repos.worktree.is_some() {
            self.close_worktree_diff();
        }
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        let line = format!("{oid} \u{b7} where the fix was made");
        let commit = CommitOf { oid, line };
        let wd = WorktreeDiff::of_commit(root, label, commit, self.active_workspace);
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        self.refresh_pointer_focus();
    }

    /// The reviews of worktrees' diffs: read when first wanted, those of
    /// worktrees gone, on another branch or too old dropped.
    fn worktree_reviews(&mut self) -> &mut crate::review::Worktrees {
        if self.repos.worktree_reviews.is_none() {
            let mut reviews = crate::review::load_worktrees(None);
            if reviews.prune(crate::clock::now_secs(), Path::is_dir) {
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
        let now = crate::clock::now_secs();
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
        if self.repos.worktree.is_none() {
            return;
        }
        self.worktree_save_review();
        // A commit's diff and a feature's code review weren't opened from
        // the worktrees popup.
        let other = self
            .wd()
            .is_some_and(|wd| wd.commit.is_some() || wd.feature.is_some());
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
        let (root, target, kept) = (wd.root.clone(), wd.target.clone(), wd.kept.clone());
        let Some(tx) = self.repos_sender() else {
            return;
        };
        if let Some(wd) = self.wd_mut() {
            wd.reading = true;
        }
        std::thread::spawn(move || {
            let read = crate::git_ops::changes_since(&root, target.as_deref()).map(|since| {
                let stamp = stamp(&root, &since);
                let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                let files = match kept.as_ref() {
                    Some((was, files)) if stamp.is_some() && *was == stamp => files.clone(),
                    _ => {
                        let files = crate::git_ops::repo_diff_from(
                            &root,
                            &since.changes,
                            Some(&since.base),
                        );
                        *kept = Some((stamp, files.clone()));
                        files
                    }
                };
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
        if let Some(wd) = &self.repos.worktree {
            return Some(wd.repos_view(self.worktree_env()));
        }
        self.repos.panel.is_none().then(|| self.doc_tab_view())?
    }

    /// What a key or click on the worktree's diff asked for, as the
    /// panel asks it; why it did nothing said briefly.
    fn worktree_done(&mut self, done: Option<Done>) -> Option<crate::repos::Outcome> {
        self.worktree_save_review();
        let env = self.worktree_env();
        if let Some(wd) = self.wd_mut() {
            wd.follow_diff(env);
        }
        let Taken {
            error,
            decided,
            delete_own,
            edit_finding,
            noted,
        } = self
            .wd_mut()
            .map(WorktreeDiff::take_error)
            .unwrap_or_default();
        if let Some(why) = error {
            self.show_warning(why);
        }
        // A finding is the code review's; a suggestion, the plan's.
        match decided {
            Some((ItemOf::Finding, id, decision)) => self.feature_decide(id, decision),
            Some((ItemOf::Suggestion, id, decision)) => self.plan_doc_decide(&id, decision),
            None => {}
        }
        match delete_own {
            Some((ItemOf::Finding, id)) => self.feature_delete_own(&id),
            Some((ItemOf::Suggestion, id)) => self.plan_doc_delete_own(&id),
            None => {}
        }
        match edit_finding {
            Some((ItemOf::Finding, id)) => self.feature_edit_finding(&id),
            Some((ItemOf::Suggestion, id)) => self.plan_doc_edit(&id),
            None => {}
        }
        match noted {
            Some((ItemOf::Finding, id, text)) => self.feature_note_finding(&id, &text),
            Some((ItemOf::Suggestion, id, text)) => self.plan_doc_note(&id, &text),
            None => {}
        }
        let commit = self.wd().is_some_and(|wd| wd.commit.is_some());
        let doc = self.wd().is_some_and(|wd| wd.doc.is_some());
        let feature = self.wd().is_some_and(|wd| wd.feature.is_some());
        match done? {
            // On the Code review page there is nothing to close: the rail has
            // the keys, to go to another page.
            Done::Close if feature && self.on_inline_review() => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.rail_keys = true;
                }
                None
            }
            Done::Close if feature => {
                self.close_feature_review();
                None
            }
            Done::Diff(Outcome::Toggle { root, path }) if feature => {
                self.feature_toggle(&root, &path);
                None
            }
            Done::Diff(Outcome::ShowCommit { root, oid }) if feature => {
                self.open_fix_commit(root, oid);
                None
            }
            Done::TickFiles { root, paths } => {
                self.feature_toggle_many(&root, &paths);
                None
            }
            Done::Grouping(grouping) => Some(crate::repos::Outcome::Grouping(grouping)),
            Done::Approve if feature => {
                self.feature_approve();
                None
            }
            Done::SendBack(comments) if feature => {
                self.feature_send_back(comments);
                None
            }
            Done::PlanFixes => {
                self.feature_plan_fixes();
                None
            }
            Done::MakeFixes => {
                self.feature_make_fixes();
                None
            }
            Done::FixAll => {
                self.feature_fix_all();
                None
            }
            Done::AddComment => {
                self.feature_ask_comment();
                None
            }
            Done::ReviewFixPlan => {
                self.feature_review_fix_plan();
                None
            }
            // Back to the panel it was opened from.
            Done::Close if commit => {
                self.close_worktree_diff();
                None
            }
            // Back to the feature's tab, and this one closes: its comments
            // are kept with the file, and the page opens it again. One with a
            // comment being written stays, not to lose it.
            Done::Close if doc => {
                let of = self.wd().and_then(|wd| wd.doc.clone());
                let writing = self.wd().is_some_and(|wd| wd.diff.editing.is_some());
                let id = self.tab().map(|tab| tab.id).filter(|_| !writing);
                if let Some(of) = of {
                    self.show_feature_tab(&of.project, &of.slug);
                }
                if let Some((w, t)) = id.and_then(|id| self.tab_place(id)) {
                    self.close_tab_in(w, t);
                }
                None
            }
            // Comments are made in the review: it comes to show.
            Done::Diff(Outcome::Notice(NO_COMMENT))
                if self
                    .review_tab()
                    .is_some_and(|r| r.mode == DocMode::Preview) =>
            {
                self.set_doc_mode(DocMode::Review);
                self.show_warning(NO_COMMENT.into());
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
            Done::CycleMode => {
                self.cycle_doc_mode();
                None
            }
            Done::Reopen => {
                let of = self.wd().and_then(|wd| wd.doc.clone());
                if let Some(of) = of {
                    self.reopen_stage(&of.project, &of.slug, of.file.stage());
                } else if let Some(fe) = self.feature_of() {
                    let (project, slug) = (fe.project.clone(), fe.slug.clone());
                    self.reopen_stage(&project, &slug, Stage::CodeReview);
                }
                None
            }
            Done::GoOn => {
                let of = (self.wd().and_then(|wd| wd.doc.clone()))
                    .map(|of| (of.project, of.slug))
                    .or_else(|| (self.feature_of()).map(|f| (f.project.clone(), f.slug.clone())));
                // A feature's popup makes way for the page it goes to.
                if self.feature_of().is_some_and(|f| !f.inline) {
                    self.close_feature_diff(false);
                }
                if let Some((project, slug)) = of {
                    self.show_feature_tab(&project, &slug);
                    self.feature_cursor_to_current_of(&project, &slug);
                }
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
        // On the feature's page, Ctrl+PgUp and PgDn still go to another
        // feature.
        let only_ctrl = mods.ctrl && !(mods.alt || mods.shift || mods.logo);
        let pages = matches!(
            sym,
            Keysym::Page_Up | Keysym::KP_Page_Up | Keysym::Page_Down | Keysym::KP_Page_Down
        );
        if only_ctrl && pages && self.on_inline_review() {
            if let Some(outcome) = self.feature_panel_key(mods, sym, typed) {
                self.apply_feature_panel(outcome);
            }
            return None;
        }
        // On Code review's page, Alt+1 and Alt+2 go between its code and its
        // fixes; text being written takes them as any key.
        let only_alt = mods.alt && !(mods.ctrl || mods.shift || mods.logo);
        let views = matches!(sym, Keysym::_1 | Keysym::KP_1 | Keysym::_2 | Keysym::KP_2);
        if only_alt
            && views
            && self.on_inline_review()
            && self.wd().is_some_and(|wd| wd.diff.editing.is_none())
        {
            if let Some(outcome) = self.feature_panel_key(mods, sym, typed) {
                self.apply_feature_panel(outcome);
            }
            return None;
        }
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
        let count = self.press_count.max(1);
        let Some(wd) = self.wd_mut() else {
            return;
        };
        let (mut drags, mut editor_drag) = (false, false);
        let done = match (hit, shift, double) {
            (ReposHit::Button(button), ..) => wd.press(button, env),
            (ReposHit::DiffRow(i), _, _) if ctrl => wd.goto_row(i, env),
            // On the comment being written, the caret goes there.
            (ReposHit::DiffRow(i), _, _) if wd.press_editor(i, shift, count, env) => {
                editor_drag = count == 1;
                None
            }
            // Twice on a file of the list, it opens in Zed.
            (ReposHit::Line(i), _, true) => {
                wd.click(hit, env);
                wd.open_tree_line(i, env)
            }
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
        self.repos_editor_drag = editor_drag;
        if let Some(outcome) = self.worktree_done(done) {
            self.apply_repos(outcome);
        }
    }

    /// The pointer dragged from a press on the worktree's diff: over row
    /// `row` (by index into those shown); or the press held past them,
    /// `by` rows on.
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
        let over_tree = (self.repos_view())
            .map(|view| crate::chrome::repos_layout(&view, self.repos_rect()))
            .is_some_and(|layout| layout.tree.to_f64().contains(self.pointer));
        if let Some(wd) = self.wd_mut() {
            match over_tree {
                true => wd.scroll_tree(motion, now, env),
                false => wd.scroll(motion, now, env),
            }
            if wd.scrolling(now) {
                wd.hover = None;
            }
        }
    }

    /// Stand the worktree's diff at row `pos`, as its scrollbar's thumb
    /// held there puts it.
    pub(crate) fn worktree_diff_scroll_to(&mut self, pos: f64, detail: bool) {
        let env = self.worktree_env();
        if let Some(wd) = self.wd_mut() {
            match detail {
                true => wd.stand_at(pos, env),
                false => wd.stand_tree_at(pos, env),
            }
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
pub(crate) mod tests {
    use super::*;
    use crate::{
        chrome::{DetailRow, ReposButton},
        diff_edit::Register,
        diff_view::{DiffRow, Outcome},
        git::Change,
    };

    #[test]
    fn a_stamp_changes_with_a_changed_file() {
        let dir = std::env::temp_dir().join(format!("gerdr-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let since = Since {
            changes: vec![Change {
                path: "a.txt".into(),
                orig: None,
                code: 'M',
            }],
            ..Since::default()
        };
        // Not a worktree's top: never taken as the same.
        assert_eq!(stamp(&dir, &since), None);
        std::fs::create_dir(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        let first = stamp(&dir, &since);
        assert!(first.is_some());
        assert_eq!(stamp(&dir, &since), first);
        std::fs::write(dir.join("a.txt"), "two!").unwrap();
        assert_ne!(stamp(&dir, &since), first);
        // An untracked folder: a file added deep inside counts.
        let since = Since {
            changes: vec![Change {
                path: "new/".into(),
                orig: None,
                code: '?',
            }],
            ..Since::default()
        };
        std::fs::create_dir_all(dir.join("new/deep")).unwrap();
        std::fs::write(dir.join("new/deep/a"), "a").unwrap();
        let first = stamp(&dir, &since);
        assert!(first.is_some());
        assert_eq!(stamp(&dir, &since), first);
        std::fs::write(dir.join("new/deep/b"), "b").unwrap();
        assert_ne!(stamp(&dir, &since), first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(crate) const ENV: Env = Env {
        sheet_rows: 10,
        page: 20,
        cols: 60,
        density: Density::Comfortable,
        tree_page: 20,
        tree_share: 0.25,
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
        let all = [
            B::Comment,
            B::CopyLines,
            B::NextComment,
            B::CopyReview,
            B::ClearReview,
            B::Help,
            B::Close,
        ];
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
        assert_eq!(wd.diff.mode_title(true).as_deref(), Some("g"));
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
        assert_eq!(wd.diff.mode_title(true), None);
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

    /// A commit's diff of two files with tabs and removed lines, in `git
    /// diff`'s words.
    fn commit_of_two() -> WorktreeDiff {
        let commit = CommitOf {
            oid: "c1".into(),
            line: "c1 Tabs".into(),
        };
        let mut wd = WorktreeDiff::of_commit("/src/gerdr".into(), "gerdr".into(), commit, 3);
        let text = "diff --git a/a.go b/a.go\n@@ -1,2 +1,2 @@\n keep\n-\told\n+\tnew\n\
                    diff --git a/b.go b/b.go\n@@ -1 +1 @@\n-gone\n+here\n";
        let mut files = crate::diff::parse(text);
        files[0].path = "a.go".into();
        files[1].path = "b.go".into();
        let since = Since {
            base: "c1^".into(),
            commits: 1,
            changes: vec![change("a.go", 'M'), change("b.go", 'M')],
        };
        wd.set_read(Ok((since, files)), ENV);
        wd
    }

    /// Type `typed`, as `key` hears it; what the last key did.
    fn typed(wd: &mut WorktreeDiff, typed: &str) -> Option<Done> {
        let mut last = None;
        for c in typed.chars() {
            let sym = Keysym::from(c as u32);
            last = wd.key(&none(), sym, sym, ENV);
        }
        last
    }

    fn yank_of(done: Option<Done>) -> (String, bool) {
        match done {
            Some(Done::Diff(Outcome::Yank(Register { text, lines }))) => (text, lines),
            other => panic!("not a yank: {other:?}"),
        }
    }

    #[test]
    fn a_yank_takes_what_the_diff_shows_of_any_line() {
        let mut wd = commit_of_two();
        let row = |wd: &WorktreeDiff, text: &str| {
            let at = texts(wd).iter().position(|t| t == text);
            at.unwrap_or_else(|| panic!("no {text} in {:?}", texts(wd)))
        };
        let (old, new, gone) = (row(&wd, "    old"), row(&wd, "    new"), row(&wd, "gone"));
        // A removed line, with its tab; however the motion goes.
        wd.click(ReposHit::DiffRow(old), ENV);
        assert_eq!(yank_of(typed(&mut wd, "yy")), ("\told\n".into(), true));
        assert_eq!(yank_of(typed(&mut wd, "Y")), ("\told\n".into(), true));
        assert_eq!(yank_of(typed(&mut wd, "y$")), ("\told".into(), false));
        assert_eq!(
            yank_of(typed(&mut wd, "2yy")),
            ("\told\n\tnew\n".into(), true)
        );
        // By lines over two files, without their headers.
        wd.click(ReposHit::DiffRow(old), ENV);
        typed(&mut wd, "V");
        for _ in old..gone {
            typed(&mut wd, "j");
        }
        assert_eq!(
            yank_of(typed(&mut wd, "y")),
            ("\told\n\tnew\ngone\n".into(), true)
        );
        assert!(wd.diff.visual.is_none());
        // By chars, on to the next line: a shown char of a tab takes it.
        wd.click(ReposHit::DiffRow(old), ENV);
        typed(&mut wd, "$hvj");
        assert_eq!(yank_of(typed(&mut wd, "y")), ("ld\n\tne".into(), false));
        // Ctrl+C takes the line, or what is selected, and leaves it at that.
        let ctrl = ModifiersState {
            ctrl: true,
            ..none()
        };
        wd.click(ReposHit::DiffRow(new), ENV);
        let copied = wd.key(&ctrl, Keysym::c, Keysym::c, ENV);
        assert_eq!(yank_of(copied), ("\tnew\n".into(), true));
        // As does the button on the row.
        let chip = ReposHit::Button(ReposButton::CopyLines);
        assert_eq!(yank_of(wd.click(chip, ENV)), ("\tnew\n".into(), true));
    }

    #[test]
    fn a_yank_off_a_line_takes_what_the_row_holds() {
        let mut wd = commit_of_two();
        let ctrl = ModifiersState {
            ctrl: true,
            ..none()
        };
        // No cursor: it says where to go.
        assert_eq!(
            wd.click(ReposHit::Button(ReposButton::CopyLines), ENV),
            Some(Done::Diff(Outcome::Notice(diff_view::NOWHERE)))
        );
        let on = |wd: &mut WorktreeDiff, row: DiffRow| {
            let r = wd.diff.rows.iter().position(|&x| x == row).unwrap();
            wd.diff.set_cursor(r, None);
        };
        // A file's header, its path; a hunk's, its lines.
        on(&mut wd, DiffRow::File(0, 1));
        assert_eq!(yank_of(typed(&mut wd, "yy")), ("b.go".into(), false));
        on(&mut wd, DiffRow::Hunk(0, 0, 0));
        let hunk = wd.key(&ctrl, Keysym::c, Keysym::c, ENV);
        assert_eq!(yank_of(hunk), ("keep\n\told\n\tnew\n".into(), true));
        // A comment, what it says.
        let mut wd = read();
        let row = texts(&wd).iter().position(|t| t == "a").unwrap();
        wd.click(ReposHit::DiffRow(row), ENV);
        typed(&mut wd, "chi");
        wd.key(&none(), Keysym::Return, Keysym::Return, ENV);
        let head = (wd.diff.rows.iter())
            .position(|r| matches!(r, DiffRow::CommentHead { .. }))
            .unwrap();
        wd.diff.set_cursor(head, None);
        assert_eq!(yank_of(typed(&mut wd, "yy")), ("hi".into(), false));
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
        // On a line: no comment, delete or restore, no buttons for them;
        // a yank takes what the diff shows.
        wd.click(ReposHit::DiffRow(3), ENV);
        assert_eq!(texts(&wd)[3], "a");
        assert_eq!(key(&mut wd, Keysym::c), refused);
        assert!(wd.diff.editing.is_none());
        key(&mut wd, Keysym::d);
        assert_eq!(key(&mut wd, Keysym::d), refused);
        key(&mut wd, Keysym::y);
        assert_eq!(
            key(&mut wd, Keysym::y),
            Some(Done::Diff(Outcome::Yank(Register {
                text: "a\n".into(),
                lines: true,
            })))
        );
        assert_eq!(key(&mut wd, Keysym::p), refused);
        assert_eq!(key(&mut wd, Keysym::R), refused);
        assert_eq!(wd.click(ReposHit::DiffRestore(2), ENV), refused);
        let chips: Vec<ReposButton> = (wd.view(ENV).row_buttons.iter())
            .map(|(_, h)| h.button)
            .collect();
        assert_eq!(chips, [ReposButton::CopyLines]);
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert_eq!(
            hints,
            [
                ReposButton::CopyLines,
                ReposButton::Help,
                ReposButton::Close
            ]
        );
        assert_eq!(wd.hints()[2].label, "back");
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
        assert!(wd.take_error().error.is_some());
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
        // One comment: nowhere to go on to, so no Previous / Next.
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
    pub(crate) fn doc() -> WorktreeDiff {
        let doc = DocOf {
            project: "p".into(),
            slug: "login".into(),
            file: DocFile::Stage(crate::feature::Stage::Discover),
            path: "/data/features/p/login/brief.md".into(),
            comments: "/data/features/p/login/discover-comments.json".into(),
            title: "Log in".into(),
            state: "ready for review".into(),
            text: None,
            locked: None,
            standing: Standing::Open,
            current: Some(Stage::Discover),
            base: None,
            suggestions: None,
        };
        let mut wd = WorktreeDiff::of_doc(doc, 2, Review::default());
        wd.set_doc(Ok("# Brief\nUsers log in.\nWith a password.\n".into()), ENV);
        wd
    }

    #[test]
    fn a_draft_shows_by_its_file_name_and_counts_both_ways() {
        let doc = DocOf {
            project: "p".into(),
            slug: "login".into(),
            file: DocFile::DraftChanges(Stage::Discover, 2),
            path: "/data/features/p/login/drafts/brief-2.md".into(),
            comments: "/data/features/p/login/discover-comments.json".into(),
            title: "Log in".into(),
            state: "draft 2 against draft 1".into(),
            text: None,
            locked: None,
            standing: Standing::Open,
            current: Some(Stage::Discover),
            base: Some("# Brief\nOld.\nGone.\n".into()),
            suggestions: None,
        };
        let mut wd = WorktreeDiff::of_doc(doc, 2, Review::default());
        wd.set_doc(Ok("# Brief\nNew.\n".into()), ENV);
        assert_eq!(
            wd.heading(),
            "Discover \u{b7} what draft 2 of brief.md changed"
        );
        assert_eq!(wd.view(ENV).title, "brief.md · +1 \u{2212}2");
    }

    #[test]
    fn a_stage_file_shows_as_a_page_to_review() {
        let mut wd = doc();
        assert_eq!(wd.heading(), "Discover · brief.md");
        assert_eq!(wd.summary(), "\u{201c}Log in\u{201d} · ready for review");
        assert_eq!(wd.root, PathBuf::from("/data/features/p/login"));
        // Only its lines, in one column of numbers.
        assert_eq!(texts(&wd), ["# Brief", "Users log in.", "With a password."]);
        let view = wd.view(ENV);
        assert!(view.plain);
        assert_eq!(view.gutter, 2);
        let buttons: Vec<ReposButton> = wd
            .view(ENV)
            .title_buttons
            .iter()
            .map(|h| h.button)
            .collect();
        assert_eq!(
            buttons,
            [
                ReposButton::CommentFile,
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
                (ReposButton::CopyLines, "Ctrl+C".into()),
                (ReposButton::Approve, APPROVE_KEY.into()),
                (ReposButton::SendBack, SEND_BACK_KEY.into()),
            ]
        );
        let help = wd.help_lines();
        assert_eq!(help[2].0, PREVIEW_KEY);
        assert_eq!(help[3].0, APPROVE_KEY);
        assert_eq!(help[4].0, SEND_BACK_KEY);
        // Ctrl+Shift+V moves its tab on to the next mode.
        let ctrl_shift = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        let v = Keysym::V;
        assert_eq!(wd.key(&ctrl_shift, v, v, ENV), Some(Done::CycleMode));
        // Its edits are listed; the restore it refuses isn't.
        let keys: Vec<&str> = help.iter().map(|(key, ..)| key.as_str()).collect();
        assert!(keys.contains(&"c / Del Del"), "{keys:?}");
        for edit in ["x X D dd dw de d$ 3dd", "yy yw y$ Y, p / P"] {
            assert!(keys.contains(&edit), "{edit} not in {keys:?}");
        }
        assert!(!keys.contains(&"Shift+R"), "Shift+R in {keys:?}");
        assert!(keys.contains(&"Shift+C"), "{keys:?}");
        // No header, no other file, no hunks to go to.
        assert!(!keys.contains(&"Alt+Up / Down"), "{keys:?}");
        assert!(keys.contains(&"gg G Ctrl+U Ctrl+D"), "{keys:?}");
        let says = |text: &str| help.iter().any(|(_, what, _)| what.contains(text));
        assert!(!says("header") && !says("hunk") && !says("a file's name"));
    }

    /// `n` words, each its number: a line over many rows of `ENV`'s.
    fn long(n: usize) -> String {
        let words: Vec<String> = (0..n).map(|i| format!("word{i}")).collect();
        words.join(" ")
    }

    /// A file of a short line, a line over many rows, and a short line.
    fn wrapped() -> WorktreeDiff {
        let mut wd = doc();
        let text = format!("Short.\n{}\nLast.\n", long(40));
        wd.set_doc(Ok(text), ENV);
        wd
    }

    /// The rows shown of the line `n`'s, by index into the view.
    fn parts_of(wd: &WorktreeDiff, n: usize) -> std::ops::Range<usize> {
        let first = wd.diff.row_of_line(n).unwrap();
        let next = wd.diff.row_of_line(n + 1).unwrap();
        first..next
    }

    #[test]
    fn a_long_line_goes_on_over_more_rows() {
        let wd = wrapped();
        let rows = texts(&wd);
        let parts = parts_of(&wd, 2);
        assert!(parts.len() > 1, "{rows:?}");
        // Every char of it, once; the number only on the first.
        assert_eq!(rows[parts.clone()].concat(), long(40));
        let view = wd.view(ENV);
        let numbers: Vec<usize> = (view.rows[parts.clone()].iter())
            .map(|row| match row {
                DetailRow::Line(line) => line.number,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(numbers[0], 2);
        assert!(numbers[1..].iter().all(|&n| n == 0), "{numbers:?}");
        // None is wider than a row holds, the caret past its end included.
        let room = crate::chrome::diff_line_room(ENV.cols, view.gutter, true);
        assert!(rows.iter().all(|t| t.chars().count() < room), "{rows:?}");
    }

    #[test]
    fn a_line_wraps_whatever_its_chars() {
        let mut wd = doc();
        let word = "é→日本語".repeat(40);
        for text in [word.clone(), format!("a {word} b"), format!("  {word}  ")] {
            wd.set_doc(Ok(format!("{text}\nEnd.\n")), ENV);
            let rows = texts(&wd);
            let n = rows.len() - 1;
            assert!(n > 1, "{rows:?}");
            assert_eq!(rows[..n].concat(), text);
        }
    }

    #[test]
    fn the_caret_shows_on_the_part_it_is_in() {
        let mut wd = wrapped();
        let parts = parts_of(&wd, 2);
        let rows = texts(&wd);
        let first = rows[parts.start].chars().count();
        // Three chars into the second part.
        wd.with_diff(ENV, |diff, ctx| diff.caret_to(parts.start, first + 3, ctx));
        assert_eq!(wd.diff.cursor_line(), Some(2));
        assert_eq!(wd.view(ENV).caret, Some((parts.start + 1, 3)));
        // Moving on, it stays on its line; j goes to the next line whole.
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        key(&mut wd, Keysym::j);
        assert_eq!(wd.diff.cursor_line(), Some(3));
        key(&mut wd, Keysym::k);
        assert_eq!(wd.diff.cursor_line(), Some(2));
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), Some(parts.start));
    }

    #[test]
    fn a_click_on_a_later_part_lands_on_the_char_under_it() {
        let mut wd = wrapped();
        let parts = parts_of(&wd, 2);
        let rows = texts(&wd);
        let first = rows[parts.start].chars().count();
        let row = parts.start + 1;
        wd.diff.pointer_char = Some((row, 3));
        wd.click(ReposHit::DiffRow(row), ENV);
        assert_eq!(wd.diff.caret(), Some((parts.start, first + 3)));
        // Past the part's end, its last char: not the next part's first.
        let len = rows[row].chars().count();
        wd.diff.pointer_char = Some((row, len + 50));
        wd.click(ReposHit::DiffRow(row), ENV);
        assert_eq!(wd.diff.caret(), Some((parts.start, first + len - 1)));
    }

    #[test]
    fn a_selection_covers_the_parts_of_its_lines() {
        let mut wd = wrapped();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let parts = parts_of(&wd, 2);
        // By lines, the band reaches its last row.
        wd.click(ReposHit::DiffRow(parts.start), ENV);
        key(&mut wd, Keysym::V);
        let marks = wd.view(ENV).marks;
        assert!(parts.clone().all(|r| marks[r].selected), "{marks:?}");
        assert!(!marks[parts.end].selected);
        key(&mut wd, Keysym::Escape);
        // By chars, each row what falls on it.
        wd.with_diff(ENV, |diff, ctx| diff.caret_to(parts.start, 2, ctx));
        key(&mut wd, Keysym::v);
        let first = texts(&wd)[parts.start].chars().count();
        wd.with_diff(ENV, |diff, ctx| diff.caret_to(parts.start, first + 2, ctx));
        assert_eq!(
            wd.view(ENV).chosen,
            [(parts.start, 2..first), (parts.start + 1, 0..3)]
        );
    }

    #[test]
    fn deleting_a_wrapped_line_cuts_the_whole_of_it() {
        let mut wd = wrapped();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let parts = parts_of(&wd, 2);
        wd.click(ReposHit::DiffRow(parts.end - 1), ENV);
        key(&mut wd, Keysym::d);
        let Some(Done::Diff(Outcome::Cut(edit))) = key(&mut wd, Keysym::d) else {
            panic!("no cut");
        };
        assert_eq!(edit.path, "brief.md");
        assert_eq!(edit.cut, crate::diff_edit::Cut::Lines(vec![2]));
    }

    #[test]
    fn a_comment_on_a_wrapped_line_quotes_it_whole_under_its_last_part() {
        let mut wd = wrapped();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        let parts = parts_of(&wd, 2);
        wd.click(ReposHit::DiffRow(parts.start + 1), ENV);
        key(&mut wd, Keysym::c);
        key(&mut wd, Keysym::o);
        key(&mut wd, Keysym::Return);
        let comment = &wd.diff.review.comments[0];
        assert_eq!(comment.lines.len(), 1);
        assert_eq!(comment.lines[0].text, long(40));
        assert_eq!(comment.lines[0].number, 2);
        assert!(matches!(
            wd.diff.rows[parts.end],
            crate::diff_view::DiffRow::CommentHead { .. }
        ));
    }

    #[test]
    fn a_narrower_page_keeps_its_top_line() {
        let mut wd = doc();
        let lines = (1..=60).map(|n| format!("{n} {}\n", long(30)));
        wd.set_doc(Ok(lines.collect()), ENV);
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(30.5, ctx));
        let top = wd.diff.top_line().unwrap();
        assert!((top - 30.5).abs() < 1e-9, "{top}");
        // Into a line's parts and back, to the fraction.
        let row = wd.diff.row_of_line(30).unwrap();
        assert!(wd.diff.scroll.first() > row);
        let before = wd.diff.rows.len();
        let narrow = Env { cols: 30, ..ENV };
        wd.with_diff(narrow, |diff, ctx| diff.rewrap(ctx));
        assert!(wd.diff.rows.len() > before);
        let top = wd.diff.top_line().unwrap();
        assert!((top - 30.5).abs() < 0.2, "{top}");
    }

    #[test]
    fn the_review_comes_back_where_it_was_when_read_again() {
        let mut wd = doc();
        let text = |first: &str| {
            let rest: String = (2..=80).map(|n| format!("Line {n}.\n")).collect();
            format!("{first}\n{rest}")
        };
        wd.set_doc(Ok(text("One")), ENV);
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(30.0, ctx));
        wd.click(ReposHit::DiffRow(3), ENV);
        assert_eq!(wd.diff.cursor_line(), Some(33));
        let scrolled = wd.diff.scroll.pos();
        wd.set_doc(Ok(text("Two")), ENV);
        assert_eq!(wd.diff.scroll.pos(), scrolled);
        assert_eq!(wd.diff.cursor_line(), Some(33));
    }

    #[test]
    fn a_page_has_nothing_to_fold_or_go_on_to() {
        let mut wd = doc();
        let before = wd.diff.rows.clone();
        let alt = ModifiersState {
            alt: true,
            ..ModifiersState::default()
        };
        for sym in [Keysym::Left, Keysym::Right] {
            wd.key(&none(), sym, sym, ENV);
            assert_eq!(wd.diff.rows, before);
        }
        wd.key(&alt, Keysym::Down, Keysym::Down, ENV);
        assert_eq!(wd.diff.rows, before);
    }

    #[test]
    fn enter_opens_a_page_at_the_line_with_no_caret() {
        let mut wd = doc();
        let enter = Keysym::Return;
        let Some(Done::Diff(Outcome::OpenInZed { paths, .. })) = wd.key(&none(), enter, enter, ENV)
        else {
            panic!("not opened");
        };
        assert_eq!(paths, ["brief.md"]);
    }

    #[test]
    fn a_file_under_review_is_not_called_a_diff() {
        let wd = doc();
        let lines = wd.help_lines();
        assert!(!lines.is_empty());
        for (key, what, _) in &lines {
            assert!(!what.contains("diff"), "{key}: {what}");
        }
        assert!(
            lines
                .iter()
                .any(|(_, what, _)| what == "in the review, go to the top, the end, half a page")
        );
    }

    #[test]
    fn the_comment_hint_says_what_the_key_will_take() {
        let label = |wd: &WorktreeDiff| {
            let hints = wd.hints();
            let comment = hints.iter().find(|h| h.button == ReposButton::Comment);
            comment.map(|h| h.label.clone()).unwrap_or_default()
        };
        let mut wd = doc();
        assert_eq!(label(&wd), "comment on the file");
        wd.click(ReposHit::DiffRow(1), ENV);
        assert_eq!(label(&wd), "comment on the lines");
    }

    #[test]
    fn shift_c_comments_on_the_whole_file_as_input_sends_it() {
        let mut wd = doc();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        wd.key(&shift, Keysym::c, Keysym::C, ENV);
        assert!(wd.diff.editing.is_some());
    }

    #[test]
    fn a_click_on_the_comment_being_written_moves_its_caret() {
        let mut wd = doc();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        wd.key(&shift, Keysym::c, Keysym::C, ENV);
        typed(&mut wd, "hello world");
        let editor = |wd: &WorktreeDiff| {
            let rows = wd.view(ENV).rows;
            let i = (rows.iter()).position(|r| matches!(r, DetailRow::Editor { .. }));
            let i = i.expect("an editor row");
            let DetailRow::Editor {
                caret, selection, ..
            } = rows[i].clone()
            else {
                unreachable!()
            };
            (i, caret, selection)
        };
        let (row, caret, _) = editor(&wd);
        assert_eq!(caret, Some(11));
        // On a char, the caret goes before it; typing goes there.
        wd.diff.pointer_char = Some((row, 5));
        assert!(wd.press_editor(row, false, 1, ENV));
        assert_eq!(editor(&wd).1, Some(5));
        typed(&mut wd, ",");
        assert_eq!(editor(&wd).1, Some(6));
        // Twice, the word there; past the end, the end.
        wd.diff.pointer_char = Some((row, 2));
        wd.press_editor(row, false, 2, ENV);
        assert_eq!(editor(&wd).2, 0..5);
        wd.diff.pointer_char = Some((row, 99));
        wd.press_editor(row, false, 1, ENV);
        assert_eq!(editor(&wd).1, Some(12));
        // Pressed and dragged, the selection reaches the char there.
        wd.diff.pointer_char = Some((row, 1));
        wd.press_editor(row, false, 1, ENV);
        wd.drag_editor(row, 4, ENV);
        assert_eq!(editor(&wd).2, 1..4);
        assert_eq!(editor(&wd).1, Some(4));
        wd.drag_editor(row, 99, ENV);
        assert_eq!(editor(&wd).2, 1..12);
        // Off it (its hint under it), nothing taken.
        assert!(!wd.press_editor(row + 1, false, 1, ENV));
    }

    #[test]
    fn dragging_the_comment_past_the_diff_scrolls_it_and_selects_on() {
        let mut wd = doc();
        let shift = ModifiersState {
            shift: true,
            ..Default::default()
        };
        wd.key(&shift, Keysym::c, Keysym::C, ENV);
        // Twice the page's rows of it.
        for n in 0..40 {
            if n > 0 {
                wd.key(&shift, Keysym::Return, Keysym::Return, ENV);
            }
            typed(&mut wd, &format!("l{n:02}"));
        }
        // Where its rows are in view, and the one with the caret.
        let shown = |wd: &WorktreeDiff| {
            let rows = wd.view(ENV).rows;
            let at = |f: &dyn Fn(&DetailRow) -> bool| rows.iter().position(f);
            let first = at(&|r| matches!(r, DetailRow::Editor { first: true, .. }));
            let last = at(&|r| matches!(r, DetailRow::Editor { last: true, .. }));
            let caret = rows.iter().find_map(|r| match r {
                DetailRow::Editor {
                    text,
                    caret: Some(c),
                    ..
                } => Some((text.clone(), *c)),
                _ => None,
            });
            (first, last, caret)
        };
        let (first, last, _) = shown(&wd);
        assert_eq!(first, None, "its start is scrolled out of view");
        assert!(last.is_some());
        // Pressed on a row of it in view, then held above the diff: a row
        // a move, until its first is in view.
        let row = last.unwrap() - 5;
        wd.diff.pointer_char = Some((row, 1));
        assert!(wd.press_editor(row, false, 1, ENV));
        let pressed = shown(&wd).2.unwrap();
        wd.drag_editor_past(-1, 2, ENV);
        let (_, _, caret) = shown(&wd);
        assert_ne!(caret, Some(pressed.clone()), "one row on");
        for _ in 0..60 {
            wd.drag_editor_past(-1, 2, ENV);
        }
        let (first, _, caret) = shown(&wd);
        assert!(first.is_some(), "its start in view");
        assert_eq!(caret, Some(("l00".into(), 2)));
        // Below it, on to its end.
        for _ in 0..60 {
            wd.drag_editor_past(1, 99, ENV);
        }
        let (_, last, caret) = shown(&wd);
        assert!(last.is_some(), "its end in view");
        assert_eq!(caret, Some(("l39".into(), 3)));
        // Held far past, many rows a frame: as far as there are.
        wd.drag_editor_past(-5, 0, ENV);
        let (_, _, five) = shown(&wd);
        wd.drag_editor_past(-1000, 0, ENV);
        assert_eq!(shown(&wd).2, Some(("l00".into(), 0)));
        assert_ne!(
            five,
            Some(("l00".into(), 0)),
            "five rows up is not the start"
        );
    }

    #[test]
    fn a_whole_file_comment_goes_first() {
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        // By c with no cursor; Shift+C with one; the title row's button.
        for route in 0..3 {
            let mut wd = doc();
            match route {
                0 => {
                    wd.key(&none(), Keysym::c, Keysym::c, ENV);
                }
                1 => {
                    wd.click(ReposHit::DiffRow(1), ENV);
                    wd.key(&shift, Keysym::C, Keysym::C, ENV);
                }
                _ => {
                    wd.click(ReposHit::Button(ReposButton::CommentFile), ENV);
                }
            }
            assert!(wd.diff.editing.is_some());
            wd.key(&none(), Keysym::o, Keysym::o, ENV);
            wd.key(&none(), Keysym::Return, Keysym::Return, ENV);
            let comments = &wd.diff.review.comments;
            assert_eq!(comments.len(), 1);
            assert!(comments[0].lines.is_empty());
            assert_eq!(comments[0].path, "brief.md");
            let rows = texts(&wd);
            let line = rows.iter().position(|t| t == "# Brief").unwrap();
            assert!(line > 0, "{rows:?}");
            assert_eq!(wd.diff.row_of_line(1), Some(line));
        }
    }

    #[test]
    fn an_empty_file_says_it_is_empty() {
        let mut wd = doc();
        wd.set_doc(Ok(String::new()), ENV);
        assert_eq!(texts(&wd), ["The file is empty"]);
    }

    #[test]
    fn a_long_file_shows_whole() {
        let mut wd = doc();
        let text: String = (1..=2500).map(|n| format!("Line {n}.\n")).collect();
        wd.set_doc(Ok(text), ENV);
        assert_eq!(wd.diff.rows.len(), 2500);
        assert_eq!(wd.diff.row_of_line(2500), Some(2499));
    }

    /// What the search finds of `what`, typed in.
    fn find(wd: &mut WorktreeDiff, what: &str) {
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        key(wd, Keysym::slash);
        for c in what.chars() {
            let sym = Keysym::from_char(c);
            wd.key(&none(), sym, sym, ENV);
        }
    }

    #[test]
    fn search_finds_on_the_part_a_word_is_in_and_once_in_a_comment() {
        let mut wd = wrapped();
        let parts = parts_of(&wd, 2);
        let rows = texts(&wd);
        // A word on the second part.
        let word = rows[parts.start + 1].split(' ').nth(1).unwrap().to_owned();
        let at = rows[parts.start + 1].find(&word).unwrap();
        find(&mut wd, &word);
        let found = wd.view(ENV).found;
        assert!(found[parts.start].is_empty(), "{found:?}");
        let start = found[parts.start + 1][0].0.start;
        assert_eq!(start, at);
        assert_eq!(wd.view(ENV).ticks.len(), 1);
        // A comment on the first line, and on the whole file, count once.
        let mut wd = doc();
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        wd.click(ReposHit::DiffRow(0), ENV);
        key(&mut wd, Keysym::c);
        key(&mut wd, Keysym::z);
        key(&mut wd, Keysym::Return);
        wd.click(ReposHit::Button(ReposButton::CommentFile), ENV);
        key(&mut wd, Keysym::z);
        key(&mut wd, Keysym::Return);
        find(&mut wd, "z");
        assert_eq!(wd.view(ENV).ticks.len(), 2);
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
        // At the start, the line before its first: the preview's top.
        assert_eq!(top(&wd), 0.0);
        wd.with_diff(ENV, |diff, ctx| diff.stand_at_line(0.5, ctx));
        assert_eq!(wd.diff.scroll.pos(), 0.0);
        // A comment on the whole file is rows above the first line: they
        // stand for the line before it.
        wd.click(ReposHit::Button(ReposButton::CommentFile), ENV);
        for sym in [Keysym::o, Keysym::Return] {
            wd.key(&none(), sym, sym, ENV);
        }
        let first = wd.diff.row_of_line(1).unwrap();
        assert!(first > 0);
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
        // A yank takes nothing from it, and the title offers only that.
        key(&mut wd, Keysym::y);
        assert!(matches!(
            key(&mut wd, Keysym::y),
            Some(Done::Diff(Outcome::Yank(_)))
        ));
        key(&mut wd, Keysym::v);
        assert_eq!(wd.view(ENV).title, "-- VISUAL -- y yank · Esc stop");
        key(&mut wd, Keysym::Escape);
        let lines = wd.help_lines();
        assert!(
            lines
                .iter()
                .all(|(_, what, _)| !what.contains("delete from the file"))
        );
    }

    /// The review of the brief with its stage done, the feature at Plan.
    fn closed_doc(standing: Standing) -> WorktreeDiff {
        let mut wd = doc();
        let it = wd.doc.as_mut().unwrap();
        (it.standing, it.current) = (standing, Some(Stage::Plan));
        it.state = "approved 1m ago by you".into();
        it.locked = Some(match standing {
            Standing::Locked(_) => crate::doc_review::LOCKED_FILE,
            _ => crate::doc_review::APPROVED_FILE,
        });
        wd
    }

    #[test]
    fn an_approved_file_can_be_read_reopened_and_left() {
        let mut wd = closed_doc(Standing::Approved);
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let ctrl_shift = ModifiersState {
            shift: true,
            ..ctrl
        };
        let enter = Keysym::Return;
        let why = crate::doc_review::APPROVED_FILE;
        let refused = Some(Done::Diff(Outcome::Notice(why)));
        // Its own buttons, not those of a file under review.
        let shown = |wd: &WorktreeDiff| -> Vec<(ReposButton, String)> {
            let view = wd.view(ENV);
            (view.title_buttons.iter())
                .map(|h| (h.button, h.label.clone()))
                .collect()
        };
        assert_eq!(
            shown(&wd),
            [
                (ReposButton::Reopen, "Reopen".to_owned()),
                (ReposButton::GoOn, "Go to Plan".to_owned()),
            ]
        );
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert!(hints.contains(&ReposButton::Reopen) && hints.contains(&ReposButton::GoOn));
        assert!(!hints.contains(&ReposButton::Approve) && !hints.contains(&ReposButton::SendBack));
        // The keys and buttons agree.
        assert_eq!(wd.key(&ctrl, enter, enter, ENV), Some(Done::GoOn));
        assert_eq!(wd.key(&ctrl_shift, enter, enter, ENV), Some(Done::Reopen));
        let press = |b| ReposHit::Button(b);
        assert_eq!(
            wd.click(press(ReposButton::Reopen), ENV),
            Some(Done::Reopen)
        );
        assert_eq!(wd.click(press(ReposButton::GoOn), ENV), Some(Done::GoOn));
        // Nothing that would change it, each saying why.
        assert_eq!(wd.click(press(ReposButton::Approve), ENV), refused);
        assert_eq!(wd.click(press(ReposButton::SendBack), ENV), refused);
        let key = |wd: &mut WorktreeDiff, sym: Keysym| wd.key(&none(), sym, sym, ENV);
        wd.click(ReposHit::DiffRow(1), ENV);
        assert_eq!(key(&mut wd, Keysym::c), refused);
        assert!(wd.diff.editing.is_none());
        key(&mut wd, Keysym::d);
        assert_eq!(key(&mut wd, Keysym::d), refused);
        // A yank takes nothing from it.
        key(&mut wd, Keysym::y);
        let yanked = key(&mut wd, Keysym::y);
        assert!(
            matches!(yanked, Some(Done::Diff(Outcome::Yank(_)))),
            "{yanked:?}"
        );
        assert_eq!(key(&mut wd, Keysym::p), refused);
        // The keys sheet says the same.
        let lines = wd.help_lines();
        let says = |what: &str| lines.iter().any(|(_, w, _)| w.contains(what));
        assert!(says("go to the stage the feature is at"));
        assert!(says("reopen the file"));
        assert!(says("yank"));
        assert!(!says("paste"));
        assert!(!says("comment"));
        assert!(!says("approve"));
        assert!(!says("delete from the file"));
        assert_eq!(key(&mut wd, Keysym::Escape), Some(Done::Close));
    }

    #[test]
    fn a_locked_file_only_goes_on_and_says_what_frees_it() {
        let mut wd = closed_doc(Standing::Locked(Stage::Plan));
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let ctrl_shift = ModifiersState {
            shift: true,
            ..ctrl
        };
        let enter = Keysym::Return;
        let why = crate::doc_review::LOCKED_FILE;
        let refused = Some(Done::Diff(Outcome::Notice(why)));
        let view = wd.view(ENV);
        let buttons: Vec<ReposButton> = view.title_buttons.iter().map(|h| h.button).collect();
        assert_eq!(buttons, [ReposButton::GoOn]);
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert!(!hints.contains(&ReposButton::Reopen));
        assert_eq!(wd.key(&ctrl, enter, enter, ENV), Some(Done::GoOn));
        assert_eq!(wd.key(&ctrl_shift, enter, enter, ENV), refused);
        let reopen = ReposHit::Button(ReposButton::Reopen);
        assert_eq!(wd.click(reopen, ENV), refused);
        // Every stage done: nowhere to go on to, and it says so.
        wd.doc.as_mut().unwrap().current = None;
        let view = wd.view(ENV);
        assert!(view.title_buttons.is_empty());
        let all_done = Some(Done::Diff(Outcome::Notice(ALL_DONE)));
        assert_eq!(wd.key(&ctrl, enter, enter, ENV), all_done);
    }

    #[test]
    fn a_closed_file_says_so_in_the_success_colour() {
        let open = doc();
        let view = open.repos_view(ENV);
        assert_eq!(view.summary_tone, crate::chrome::Tone::Muted);
        let closed = closed_doc(Standing::Approved);
        let view = closed.repos_view(ENV);
        assert_eq!(view.summary_tone, crate::chrome::Tone::Success);
        assert!(
            view.summary.contains("approved 1m ago by you"),
            "{}",
            view.summary
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
            locked: None,
            standing: Standing::Open,
            current: Some(Stage::Verify),
            base: None,
            suggestions: None,
        };
        let mut wd = WorktreeDiff::of_doc(doc, 2, Review::default());
        wd.set_doc(Ok("# Evidence\napi: npm test failed\n".into()), ENV);
        wd
    }

    #[test]
    fn the_evidence_is_only_read() {
        let mut wd = evidence();
        // It is copied as any other: only its edits are refused.
        let mut copied = evidence();
        let row = texts(&copied)
            .iter()
            .position(|t| t == "# Evidence")
            .unwrap();
        copied.click(ReposHit::DiffRow(row), ENV);
        assert_eq!(
            yank_of(typed(&mut copied, "yy")),
            ("# Evidence\n".into(), true)
        );
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
        let view = wd.view(ENV);
        assert!(view.title_buttons.is_empty());
        assert!(
            (view.row_buttons.iter()).all(|(_, h)| h.button == ReposButton::CopyLines),
            "{:?}",
            view.row_buttons
        );
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        assert_eq!(
            hints,
            [
                ReposButton::CopyLines,
                ReposButton::Help,
                ReposButton::Close
            ]
        );
        let lines = wd.help_lines();
        assert_eq!(lines[0].1, "back to the feature");
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
