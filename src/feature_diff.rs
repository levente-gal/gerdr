//! Code review of a feature: its diff over every repository it touched,
//! shown across the repositories panel as a worktree's diff is (see
//! `worktree_diff`, `WorktreeDiff::feature`), read-only, with a list of its
//! files beside it (`file_tree::FileTree`).
//!
//! While the feature waits on Code review it is that stage's page, under the
//! rail on the feature tab (`inline_review`, `FeaturePanel::review`): Esc gives
//! the rail the keys, Enter, Tab or a press there gives them back. From anywhere
//! else, with a fix plan open, or for a feature another machine writes, it is a
//! popup over the app.
//!
//! Each file's header and its line in the list have a checkbox: viewed, not
//! viewed, or changed since viewed (see `feature::viewed`), one record behind
//! both; Space ticks it. A folder's or a repository's ticks the files under it
//! that can be, and says how many it left out. AI review's findings
//! show as comments of their own, on their line, else on their file's
//! header, else in the list of those not on a line, each with Fix and
//! Dismiss; `]` and `[` go through them. The user's own comments are kept
//! in the feature's data dir (`code-review-comments.json`) until they go to
//! the Build agent, those on no line or file listed first, as General
//! comments. Each card's heading says whose it is.
//!
//! It opens at any stage, and shows only what works at that one
//! (`FeatureOf::role`, `FeatureOf::buttons`): at AI review, Make fixes once
//! every finding is decided, or only Plan fixes when the user wrote anything
//! of their own; at Code review, Plan fixes for the comments and
//! Approve, which wants every file viewed as it is now, no comment left,
//! nothing uncommitted and no finding holding up.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use crate::{
    Gerdr,
    chrome::Check,
    chrome::ReposButton,
    diff::{FileDiff, LineKind},
    diff_view::Section,
    doc_review::DocFile,
    feature::{
        Feature, Repo, Stage, Status,
        fix_round::FixPhase,
        review_import::{Decision, Finding, FindingState},
        review_items::ItemOf,
        store,
        viewed::{self, Tick, Viewed},
    },
    features::plural,
    file_tree::FileTree,
    git_ops::Since,
    review::{self, Comment, Flag, Quoted, Review},
    toast::Level,
    worktree_diff::{DRAFT_OPEN, Env, WorktreeDiff},
};

/// The file a feature's code review comments are kept in, in its data
/// dir.
pub const COMMENTS: &str = "code-review-comments.json";
/// Why Approve waits on the diff being read.
pub const STILL_READING: &str = "Still reading the diff";
/// Why Plan fixes does nothing at Code review: comments are all it sends.
pub const NO_OWN_COMMENT: &str = "Add a comment first";
/// Why a done Code review's diff takes no comments or viewed ticks.
pub const REVIEW_DONE: &str = "Code review is done: reopen it first";
/// Why a closed feature's diff takes no comments or viewed ticks.
pub const FEATURE_CLOSED: &str = "The feature is closed";
/// Why a fix round's diff takes no comments or viewed ticks.
pub const ROUND_ONLY_READ: &str = "This shows what a fix round changed, only to read";
/// Under a file's diff since the last review: what it shows.
pub const SINCE_VIEWED: &str = "Only what changed since you viewed it: All changes shows the rest";

/// What Plan fixes waits for at AI review.
pub fn decide_first(undecided: usize) -> String {
    format!("Decide the {} left first", plural(undecided, "finding"))
}

/// Whether Code review sent `feature` comments to fix, kept since or in a round
/// open now.
pub fn has_rounds(feature: &Feature) -> bool {
    !feature.comment_fixes.is_empty()
        || (feature.fix_round.as_ref()).is_some_and(|round| round.stage == Stage::CodeReview)
}

/// Whether `feature`'s Code review page goes between its code, laid into the
/// page rather than opened over the app, and the fixes it asked for: while the
/// stage is at hand, once it sent any back, wherever the feature went for
/// them, and once it is done, to read as it was worked. Another machine
/// writing the feature keeps the popup, its page saying what it can.
pub fn review_views(feature: &Feature) -> bool {
    (feature.current() == Some(Stage::CodeReview)
        || has_rounds(feature)
        || feature.is_done(Stage::CodeReview))
        && feature.writer.is_none()
        && feature.status == Status::Open
        && feature.touched().next().is_some()
}

/// Whether `panel` shows `feature`'s Code review page with its review on it.
pub fn review_page(panel: &crate::features::FeaturePanel, feature: &Feature) -> bool {
    !panel.keys
        && panel.page_stage(feature) == Stage::CodeReview
        && review_views(feature)
        && panel.review_view_of(feature) == crate::chrome::ReviewView::Code
}

/// What a feature's diff is for, as the feature stands now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Its code, on its way to AI review: only read.
    Early,
    /// AI review runs: its findings aren't in yet.
    Reading,
    /// AI review's findings wait to be decided, then planned.
    Deciding,
    /// A fix plan is open, wherever the feature is.
    Round(FixPhase),
    /// Code review: approved, or comments sent to be fixed.
    Review,
    /// Past Code review, or the feature closed.
    Done,
    /// What one fix round changed (`FeatureOf::span`): only read.
    Span,
}

/// The fix round whose code a feature's diff shows instead of all the
/// feature's (`FeatureOf::span`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpanOf {
    /// `FixRound::n`.
    pub n: u32,
    /// What it is called: "Manual test 2".
    pub label: String,
    /// How it stands, lowercase: "fixes made 12m ago".
    pub state: String,
    /// Per repository by `Repo::key`: the HEAD before its fixes, and after
    /// them, none while they go on (up to HEAD).
    pub ends: BTreeMap<String, (String, Option<String>)>,
}

/// A touched repository as its code review last read it.
#[derive(Clone, Debug, Default)]
pub struct RepoOf {
    /// `Repo::key`: the viewed keys' first part and the findings' repo.
    pub name: String,
    /// Its header's label (`PerProject::label`); empty: `name`.
    pub label: String,
    /// Its worktree for the feature.
    pub root: PathBuf,
    pub branch: Option<String>,
    pub since: Since,
    /// By path: the committed blob (see `viewed::Read`).
    pub blobs: std::collections::BTreeMap<String, String>,
    /// Paths with changes not committed.
    pub dirty: std::collections::BTreeSet<String>,
    /// Its files' diffs since its base, in the file list's order.
    pub files: Vec<FileDiff>,
    /// By path: what a file viewed before changed since (`viewed::Read`).
    pub since_viewed: BTreeMap<String, FileDiff>,
    /// Why its last read failed.
    pub error: Option<String>,
}

impl RepoOf {
    /// What it is called on screen.
    fn shown(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

/// A feature's code review (`WorktreeDiff::feature`).
#[derive(Clone, Debug, Default)]
pub struct FeatureOf {
    pub project: String,
    pub slug: String,
    pub title: String,
    /// Where Code review stands, lowercase: `ready for review`.
    pub state: String,
    /// The stage the feature is at: none once all are done.
    pub stage: Option<Stage>,
    /// The feature isn't open any more.
    pub closed: bool,
    /// Another machine writes it: nothing can be left on its code from here.
    pub seen: bool,
    /// AI review's findings were read in.
    pub ready: bool,
    /// The open fix plan: its phase and how it stands, in words.
    pub round: Option<(FixPhase, String)>,
    /// Where the user's comments are kept (`COMMENTS`).
    pub comments: PathBuf,
    /// Opened from the feature panel, which Esc goes back to.
    pub back: bool,
    /// Laid into its Code review page: Esc gives the rail the keys.
    pub inline: bool,
    /// Its touched repositories, in name order.
    pub repos: Vec<RepoOf>,
    /// The viewed ticks, as the record has them.
    pub viewed: Viewed,
    /// Its AI review's findings, as the record has them.
    pub findings: Vec<Finding>,
    /// The finding to put the cursor on once read.
    pub focus: Option<String>,
    /// The comment of the user's to go to, if any (`focus` is for findings).
    pub focus_note: Option<Comment>,
    /// The comments that went to Build in Code review's latest round: only
    /// to see, its comments file holding them no longer.
    pub sent_notes: Vec<Comment>,
    /// Only what this fix round changed, to read: no ticks, findings or
    /// comments.
    pub span: Option<SpanOf>,
    /// The review shows what changed since the user's last review: of a
    /// file viewed before and changed since, only that (`RepoOf::since_viewed`),
    /// the files viewed and unchanged folded. Else all the changes.
    pub since: bool,
    /// What its reviews want goes to GitHub with the review the user
    /// submits, not to a Build agent (`Feature::posts_review`).
    pub posts: bool,
}

/// What code review counts of a feature, for its panel row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    pub viewed: usize,
    pub changed: usize,
    pub comments: usize,
}

/// The counts of every feature's code review read since gerdr started,
/// by project and slug.
pub type Lines = HashMap<(String, String), Counts>;

/// What a read off the UI thread brings back (`RepoEvent::FeatureDiff`).
#[derive(Debug)]
pub struct Read {
    pub project: String,
    pub slug: String,
    /// Per touched repository: its name, worktree and what was read.
    pub repos: Vec<(String, PathBuf, Result<viewed::Read, String>)>,
}

/// What a feature's diff keeps of where the feature stands.
struct Standing {
    state: String,
    stage: Option<Stage>,
    closed: bool,
    ready: bool,
    round: Option<(FixPhase, String)>,
}

impl Standing {
    fn of(feature: &Feature) -> Standing {
        let run = feature.runs.get(&Stage::AiReview);
        Standing {
            state: state(feature),
            stage: feature.current(),
            // What's written elsewhere is only looked at here, as a closed one.
            closed: feature.status != Status::Open || feature.writer.is_some(),
            ready: feature.review.is_some() && run.is_some_and(|r| r.ready.is_some()),
            round: (feature.fix_round.as_ref()).map(|round| (round.phase, round.line())),
        }
    }
}

impl FeatureOf {
    /// What it is for now.
    pub fn role(&self) -> Role {
        if self.span.is_some() {
            return Role::Span;
        }
        if self.closed {
            return Role::Done;
        }
        if let Some((phase, _)) = &self.round {
            return Role::Round(*phase);
        }
        match self.stage {
            Some(Stage::CodeReview) => Role::Review,
            Some(Stage::AiReview) if self.ready => Role::Deciding,
            Some(Stage::AiReview) => Role::Reading,
            Some(stage) if stage < Stage::AiReview => Role::Early,
            _ => Role::Done,
        }
    }

    /// Its heading: what the feature is at.
    pub fn heading(&self) -> String {
        if let Some(span) = &self.span {
            return format!("{} of \u{201c}{}\u{201d}", span.label, self.title);
        }
        let what = match self.stage.filter(|_| !self.closed) {
            Some(Stage::AiReview) => "AI review",
            Some(stage) if stage < Stage::AiReview => "Code",
            _ => "Code review",
        };
        format!("{what} of \u{201c}{}\u{201d}", self.title)
    }

    /// How many findings stand at `state`.
    fn count(&self, state: FindingState) -> usize {
        self.findings.iter().filter(|f| f.state == state).count()
    }

    /// The buttons it has now, in the title row's order, with `own` comments
    /// of the user's; each with why it can't be pressed yet, if so.
    pub fn buttons(&self, own: usize) -> Vec<(ReposButton, Option<String>)> {
        use ReposButton as B;
        let (open, fix) = (
            self.count(FindingState::Open),
            self.count(FindingState::Fix),
        );
        match self.role() {
            // Wanted ones wait for the review to be submitted: nothing to send.
            Role::Deciding if self.posts => vec![(B::AddComment, None)],
            Role::Review if self.posts => vec![(B::AddComment, None), (B::Approve, None)],
            Role::Deciding => {
                let mut buttons = vec![(B::AddComment, None)];
                if open > 0 {
                    buttons.push((B::FixAllFindings, None));
                }
                if open > 0 || fix > 0 {
                    let direct = !self.needs_plan(own);
                    if direct {
                        buttons.push((B::MakeFixes, self.plan_refusal(own)));
                    }
                    // Greyed, one says what they wait for, and the row is full
                    // on a small output.
                    if !direct || open == 0 {
                        buttons.push((B::PlanFixes, self.plan_refusal(own)));
                    }
                }
                buttons
            }
            Role::Review => {
                let scope = self.scope_button().map(|b| (b, None));
                (scope.into_iter())
                    .chain([
                        (B::AddComment, None),
                        (B::PlanFixes, self.plan_refusal(own)),
                        (B::Approve, None),
                    ])
                    .collect()
            }
            Role::Round(FixPhase::Ready) => vec![(B::ReviewFixPlan, None)],
            // Done with, it only reads: going on, or back to it.
            Role::Done if !self.closed => {
                let mut buttons = Vec::new();
                if self.stage.is_some() {
                    buttons.push((B::GoOn, None));
                }
                buttons.push((B::Reopen, None));
                buttons
            }
            _ => Vec::new(),
        }
    }

    /// The button switching what the review shows: All changes while it
    /// shows those since the last review, back to them while there are
    /// some to show; none with none.
    pub fn scope_button(&self) -> Option<ReposButton> {
        let some = (self.repos.iter()).any(|r| !r.since_viewed.is_empty());
        match (self.since, some && self.span.is_none()) {
            (true, _) => Some(ReposButton::AllChanges),
            (false, true) => Some(ReposButton::SinceReview),
            (false, false) => None,
        }
    }

    /// The diffs of `repo` the review shows: since the last review, a file
    /// viewed before and changed since as what changed since, unless
    /// `comments` has one of the user's own on it (`own`) on lines that diff
    /// hasn't, which the whole diff then shows rather than taking it for
    /// outdated.
    pub fn shown_files(&self, repo: &RepoOf, comments: &[Comment]) -> Vec<FileDiff> {
        let since = |file: &FileDiff| {
            let part = repo.since_viewed.get(&file.path).filter(|_| self.since)?;
            let lost = (comments.iter())
                .filter(|c| c.flag.is_none() && c.root == repo.root && c.path == file.path)
                .filter(|c| !c.lines.is_empty())
                .any(|c| crate::review::locate(part, c).is_none());
            let mut part = part.clone();
            part.notes.push(SINCE_VIEWED.to_owned());
            (!lost).then_some(part)
        };
        (repo.files.iter())
            .map(|file| since(file).unwrap_or_else(|| file.clone()))
            .collect()
    }

    /// Why its code takes no comments or viewed ticks, if it doesn't: another
    /// machine writes it, Code review is done, or the feature closed.
    pub fn locked(&self) -> Option<&'static str> {
        if self.span.is_some() {
            return Some(ROUND_ONLY_READ);
        }
        if self.seen {
            return Some(crate::doc_review::WRITTEN_ELSEWHERE);
        }
        match self.role() {
            Role::Done if self.closed => Some(FEATURE_CLOSED),
            Role::Done => Some(REVIEW_DONE),
            _ => None,
        }
    }

    /// What Go on is called: going to the stage the feature is at.
    pub fn go_on_label(&self) -> String {
        match self.stage {
            Some(stage) => format!("Go to {}", stage.label()),
            None => ReposButton::GoOn.label().into(),
        }
    }

    /// Whether AI review's fixes need a plan, with `own` comments of the
    /// user's in the diff (`review_import::needs_plan`).
    pub fn needs_plan(&self, own: usize) -> bool {
        crate::feature::review_import::needs_plan(&self.findings, own)
    }

    /// What Ctrl+Enter (`shift`: Ctrl+Shift+Enter) presses while deciding,
    /// with `own` comments of the user's: the button that is there, for the
    /// host to say what it waits for.
    pub fn deciding_button(&self, shift: bool, own: usize) -> ReposButton {
        // Ctrl+Enter makes them, Ctrl+Shift+Enter plans them, as on every
        // page; with findings left, Make fixes says to decide them first.
        let direct = !self.needs_plan(own);
        match !shift || direct && self.count(FindingState::Open) > 0 {
            true => ReposButton::MakeFixes,
            false => ReposButton::PlanFixes,
        }
    }

    /// Why Plan fixes can't be pressed yet, with `own` comments of the
    /// user's: at AI review it goes by the findings alone.
    pub fn plan_refusal(&self, own: usize) -> Option<String> {
        match self.role() {
            Role::Deciding => match self.count(FindingState::Open) {
                0 => None,
                n => Some(decide_first(n)),
            },
            Role::Review if own == 0 => Some(NO_OWN_COMMENT.into()),
            Role::Review => None,
            _ => Some(self.not_now().into()),
        }
    }

    /// Why a button it doesn't have does nothing now.
    pub fn not_now(&self) -> &'static str {
        match self.role() {
            Role::Early => "This code isn't up for review yet",
            Role::Reading => "AI review's findings aren't in yet",
            Role::Deciding => "No finding left to decide",
            Role::Round(_) => "A fix plan is open: make the fixes or drop it first",
            Role::Review => "Code review has no findings to decide",
            Role::Done if self.closed => FEATURE_CLOSED,
            Role::Done => REVIEW_DONE,
            Role::Span => ROUND_ONLY_READ,
        }
    }

    /// The key of `button` as it stands with `own` comments of the user's:
    /// empty for one not of its own.
    pub fn key_of(&self, button: ReposButton, _own: usize) -> &'static str {
        use ReposButton as B;
        match button {
            B::AddComment => "Shift+C",
            B::FixAllFindings => "Shift+F",
            B::AllChanges | B::SinceReview => "Shift+A",
            B::PlanFixes => crate::worktree_diff::SEND_BACK_KEY,
            B::MakeFixes | B::ReviewFixPlan | B::Approve | B::GoOn => {
                crate::worktree_diff::APPROVE_KEY
            }
            B::Reopen => crate::worktree_diff::SEND_BACK_KEY,
            _ => "",
        }
    }

    /// Its keys as it stands with `own` comments of the user's, before the
    /// diff's own (F1).
    pub fn keys(&self, _own: usize) -> Vec<(&'static str, &'static str)> {
        let esc = match self.inline {
            true => "back to the stages",
            false => "close the diff",
        };
        let mut keys = vec![("Esc", esc), ("F1", "list the keys")];
        match self.role() {
            Role::Deciding => keys.extend([
                (
                    "Ctrl+Enter",
                    "make the fixes, once every finding is decided and nothing of yours needs a plan",
                ),
                ("Ctrl+Shift+Enter", "plan the fixes first"),
                ("Shift+C", "add a comment on no line or file"),
                ("Shift+F", "mark every undecided finding to fix"),
                ("f", "fix the finding, or take that back"),
                ("c", "on a finding, write or edit your comment on it"),
                ("Shift+E", "on a finding, edit its fix"),
                ("Del", "dismiss the finding, saying why, or reopen it"),
            ]),
            Role::Review => {
                keys.extend([
                    ("Ctrl+Enter", "approve Code review"),
                    ("Ctrl+Shift+Enter", "plan fixes for the comments"),
                    ("Shift+C", "add a comment on no line or file"),
                ]);
                if self.scope_button().is_some() {
                    keys.push(("Shift+A", "all changes, or only those since your last review"));
                }
                keys.push(("Shift+O", "on a comment whose fix is made, show its commit"));
            }
            Role::Round(FixPhase::Ready) => keys.push(("Ctrl+Enter", "review the fix plan")),
            Role::Done if !self.closed => {
                if self.stage.is_some() {
                    keys.push(("Ctrl+Enter", "go to the stage the feature is at"));
                }
                keys.push(("Ctrl+Shift+Enter", "reopen Code review"));
            }
            _ => {}
        }
        if self.locked().is_some() {
            return keys;
        }
        keys.extend([
            ("Space", "mark the file viewed, or not"),
            (
                "Alt+Shift+Down / Up",
                "go to the next or the previous file not viewed",
            ),
        ]);
        keys
    }

    /// Where its findings stand, as AI review's summary says it.
    fn decisions(&self) -> String {
        let wanted = crate::feature::review_items::words_in(ItemOf::Finding, self.posts).wanted;
        let mut parts = vec![format!("{} {wanted}", self.count(FindingState::Fix))];
        match self.count(FindingState::Dismissed) {
            0 => {}
            n => parts.push(format!("{n} dismissed")),
        }
        parts.push(format!("{} to decide", self.count(FindingState::Open)));
        let made = self.findings.iter().filter(|f| f.fix_made()).count();
        let planning = (self.round.as_ref()).is_some_and(|(phase, _)| {
            matches!(
                phase,
                FixPhase::Compacting | FixPhase::Planning | FixPhase::Ready
            )
        });
        match self.count(FindingState::Sent) - made {
            0 => {}
            n => parts.push(format!(
                "{n} {}",
                crate::feature::review_import::sent_word(false, planning)
            )),
        }
        if made > 0 {
            parts.push(crate::feature::review_import::fixes_made_words(made));
        }
        parts.join(" \u{b7} ")
    }

    /// Take where the feature stands now.
    fn stands(&mut self, standing: Standing) {
        self.state = standing.state;
        self.stage = standing.stage;
        self.closed = standing.closed;
        self.ready = standing.ready;
        self.round = standing.round;
    }

    /// The repository at `root`.
    fn repo(&self, root: &Path) -> Option<&RepoOf> {
        self.repos.iter().find(|r| r.root == root)
    }

    /// The tick of file `path` of the repository at `root`.
    pub fn tick(&self, root: &Path, path: &str) -> Tick {
        let Some(repo) = self.repo(root) else {
            return Tick::No;
        };
        let blob = repo.blobs.get(path).map(String::as_str);
        let dirty = repo.dirty.contains(path) || blob.is_none();
        self.viewed
            .tick(&viewed::key(&repo.name, path), blob, dirty)
    }

    /// The checkbox of file `path` of the repository at `root`.
    pub fn check(&self, root: &Path, path: &str) -> Check {
        match self.tick(root, path) {
            Tick::Yes => Check::On,
            Tick::Changed => Check::Partial,
            Tick::No => Check::Off,
        }
    }

    /// What the header of file `path` of the repository at `root` says.
    pub fn note(&self, root: &Path, path: &str) -> Option<String> {
        let repo = self.repo(root)?;
        if repo.dirty.contains(path) || !repo.blobs.contains_key(path) {
            return Some("not committed".into());
        }
        (self.tick(root, path) == Tick::Changed).then(|| "changed since viewed".into())
    }

    /// The diff's sections: one per repository.
    pub fn sections(&self) -> Vec<Section<'_>> {
        self.repos
            .iter()
            .map(|r| Section {
                root: &r.root,
                label: r.shown(),
                branch: r.branch.as_deref(),
                head: None,
                changes: &r.since.changes,
            })
            .collect()
    }

    /// What the diff says with no changes, read once when `read`.
    pub fn empty(&self, read: bool) -> &'static str {
        match (read, self.repos.iter().any(|r| r.error.is_some())) {
            (false, _) => "Reading…",
            (true, true) => "Its changes couldn't be read",
            (true, false) if self.span.is_some() => "Its fixes changed no file",
            (true, false) => "No changes since its base",
        }
    }

    /// Its counts, with `own` comments of the user's.
    pub fn counts(&self, own: usize) -> Counts {
        let mut counts = Counts {
            comments: own,
            ..Counts::default()
        };
        for repo in &self.repos {
            for change in &repo.since.changes {
                counts.files += 1;
                match self.tick(&repo.root, &change.path) {
                    Tick::Yes => counts.viewed += 1,
                    Tick::Changed => counts.changed += 1,
                    Tick::No => {}
                }
            }
        }
        counts
    }

    /// The header's summary: where the stage stands, then its counts.
    pub fn summary(&self, own: usize) -> String {
        if let Some(span) = &self.span {
            return self.span_summary(span);
        }
        let head = match self.role() {
            Role::Deciding => self.decisions(),
            _ => self.state.clone(),
        };
        // The files viewed are counted over the file list (`tree_note`) or
        // on the page's line: not again here.
        let counts = self.counts(own);
        let mut parts = vec![head];
        if counts.comments > 0 {
            parts.push(plural(counts.comments, "comment"));
        }
        if counts.changed > 0 {
            parts.push(format!("{} changed since viewed", counts.changed));
        }
        parts.join(" \u{b7} ")
    }

    /// The header's summary of a fix round's diff: how the round stands,
    /// its commits and files.
    fn span_summary(&self, span: &SpanOf) -> String {
        let commits: usize = self.repos.iter().map(|r| r.since.commits).sum();
        let files: usize = self.repos.iter().map(|r| r.since.changes.len()).sum();
        let mut parts = vec![span.state.clone(), plural(commits, "commit")];
        if files > 0 {
            parts.push(plural(files, "file"));
        }
        parts.join(" \u{b7} ")
    }

    /// What the file list's title says once read: how many files are left.
    pub fn tree_note(&self, read: bool) -> Option<(String, crate::chrome::Tone)> {
        use crate::chrome::Tone;
        if self.span.is_some() {
            return None;
        }
        let counts = self.counts(0);
        if !read || counts.files == 0 {
            return None;
        }
        let left = counts.files - counts.viewed;
        Some(match left {
            0 => (format!("All {} viewed", plural(counts.files, "file")), Tone::Success),
            _ => (format!("{left} of {} left", plural(counts.files, "file")), Tone::Warning),
        })
    }

    /// The files not viewed as they are now, as `<repo>/<path>`.
    pub fn unviewed(&self) -> Vec<String> {
        let mut all = Vec::new();
        for repo in &self.repos {
            for change in &repo.since.changes {
                if self.tick(&repo.root, &change.path) != Tick::Yes {
                    all.push(viewed::key(&repo.name, &change.path));
                }
            }
        }
        all
    }

    /// How many findings hold Code review up.
    pub fn holding(&self) -> usize {
        (self.findings.iter())
            .filter(|f| f.state.holds_up() && !(self.posts && f.state == FindingState::Fix))
            .count()
    }

    /// Whether the finding at `i` is dismissed and only repeats another one
    /// listed (the same title on the same file): raised again run after run,
    /// it would pile up in the diff. The one gone to Build, else the latest
    /// dismissed, stands for them; the one to show stays.
    fn repeats(&self, i: usize) -> bool {
        let f = &self.findings[i];
        let same = |g: &Finding| g.repo == f.repo && g.file == f.file && g.title == f.title;
        f.state == FindingState::Dismissed
            && self.focus.as_deref() != Some(f.id.as_str())
            && (self.findings.iter().enumerate()).any(|(j, g)| {
                j != i && !g.yours && same(g) && (g.state != FindingState::Dismissed || j > i)
            })
    }

    /// The findings as comments, placed in the diff read (`diffs`).
    pub fn finding_comments(&self, diffs: &HashMap<PathBuf, (u64, Vec<FileDiff>)>) -> Vec<Comment> {
        let single = (self.repos.len() == 1).then(|| &self.repos[0]);
        (self.findings.iter().enumerate())
            // Those an older gerdr dropped: gone from here, on the page until
            // deleted.
            .filter(|(_, f)| !(f.yours && f.state == FindingState::Dismissed))
            .filter(|&(i, _)| !self.repeats(i))
            .flat_map(|(_, f)| {
                let repo = (self.repos.iter().find(|r| r.name == f.repo)).or(single);
                let root = repo.map(|r| r.root.clone()).unwrap_or_default();
                let file = diffs
                    .get(&root)
                    .and_then(|(_, files)| files.iter().find(|d| d.path == f.file));
                let mut comment = finding_comment(f, root, file);
                let mut note = finding_note(f, &comment);
                let planning = (self.round.as_ref()).is_some_and(|(phase, _)| {
                    matches!(
                        phase,
                        FixPhase::Compacting | FixPhase::Planning | FixPhase::Ready
                    )
                });
                for c in std::iter::once(&mut comment).chain(note.as_mut()) {
                    if let Some(flag) = c.flag.as_mut() {
                        flag.pr = self.posts;
                        flag.planning = planning;
                    }
                }
                std::iter::once(comment).chain(note)
            })
            .collect()
    }
}

/// The files of `fe` to fold so its review opens on what changed since the
/// user's last review: those viewed and unchanged, once some file viewed
/// before changed since (a fix round's work); none otherwise, or on a fix
/// round's own diff.
pub(crate) fn since_review(fe: &FeatureOf) -> Vec<(PathBuf, String)> {
    if fe.span.is_some() {
        return Vec::new();
    }
    let files = (fe.repos.iter())
        .flat_map(|r| r.since.changes.iter().map(move |c| (r.root.clone(), c.path.clone())));
    let mut viewed = Vec::new();
    let mut changed = false;
    for (root, path) in files {
        match fe.tick(&root, &path) {
            Tick::Yes => viewed.push((root, path)),
            Tick::Changed => changed = true,
            Tick::No => {}
        }
    }
    match changed {
        true => viewed,
        false => Vec::new(),
    }
}

/// Finding `f` as a comment on the repository at `root`: on its line
/// when `file` (its diff) shows it, else on the whole file.
pub fn finding_comment(f: &Finding, root: PathBuf, file: Option<&FileDiff>) -> Comment {
    let on = f.line.and_then(|n| {
        let lines = file?.hunks.iter().flat_map(|h| &h.lines);
        lines
            .into_iter()
            .find(|l| l.number == n && l.kind != LineKind::Removed)
    });
    // Its severity and where it stands are on the card's heading.
    let mut text = f.title.clone();
    if f.state == FindingState::Dismissed && !f.reason.is_empty() {
        text.push_str(&format!("\nDismissed: {}", f.reason));
    }
    if let (Some(n), None) = (f.line, on) {
        text.push_str(&format!("\nline {n}"));
    }
    if !f.body.is_empty() {
        text.push('\n');
        text.push_str(&f.body);
    }
    if let Some(fix) = &f.my_fix {
        text.push_str("\nYour fix: ");
        text.push_str(fix);
    } else if !f.fix.is_empty() {
        text.push_str("\nFix: ");
        text.push_str(&f.fix);
    }
    Comment {
        root,
        path: f.file.clone(),
        lines: on.map(Quoted::of).into_iter().collect(),
        text,
        flag: Some(Flag {
            id: f.id.clone(),
            severity: f.severity,
            state: f.state,
            made: f.fix_made(),
            yours: f.yours,
            note: false,
            sent: false,
            of: ItemOf::Finding,
            frozen: None,
            unanswered: false,
            question: false,
            pr: false,
            planning: false,
            fixed_in: None,
        }),
    }
}

/// The user's comment on finding `f`, shown as `comment` is, right under it.
fn finding_note(f: &Finding, comment: &Comment) -> Option<Comment> {
    (!f.note.is_empty() && !f.yours).then(|| Comment {
        root: comment.root.clone(),
        path: comment.path.clone(),
        lines: comment.lines.clone(),
        text: f.note.clone(),
        flag: Some(Flag {
            id: f.id.clone(),
            severity: f.severity,
            state: f.state,
            made: f.fix_made(),
            yours: true,
            note: true,
            sent: false,
            of: ItemOf::Finding,
            frozen: None,
            unanswered: false,
            question: false,
            pr: false,
            planning: false,
            fixed_in: None,
        }),
    })
}

/// The panel row's line of `counts`: "3 of 7 files viewed · 2 comments
/// · 1 changed since viewed"; zero parts left out, none for no files.
pub fn line(counts: &Counts) -> Option<String> {
    if counts.files == 0 {
        return None;
    }
    let mut parts = vec![match counts.files {
        1 => format!("{} of 1 file viewed", counts.viewed),
        n => format!("{} of {n} files viewed", counts.viewed),
    }];
    match counts.comments {
        0 => {}
        1 => parts.push("1 comment".into()),
        n => parts.push(format!("{n} comments")),
    }
    if counts.changed > 0 {
        parts.push(format!("{} changed since viewed", counts.changed));
    }
    Some(parts.join(" \u{b7} "))
}

/// Why Approve or Send back don't fit the stage the feature is at.
pub fn stage_refusal(stage: Option<Stage>) -> Option<String> {
    match stage {
        Some(Stage::CodeReview) => None,
        Some(stage) if stage < Stage::CodeReview => {
            Some(format!("Code review waits for {}", stage.label()))
        }
        _ => Some("Code review is done".into()),
    }
}

/// What Approve looks at.
#[derive(Clone, Debug, Default)]
pub struct Gate {
    pub stage: Option<Stage>,
    /// A comment is being written.
    pub draft: bool,
    /// The diff was read.
    pub read: bool,
    /// Why repositories couldn't be read.
    pub errors: Vec<String>,
    /// The repositories with changes not committed.
    pub dirty: Vec<String>,
    /// The files not viewed as they are, as `<repo>/<path>`.
    pub unviewed: Vec<String>,
    /// The user's comments left.
    pub comments: usize,
    /// The findings holding it up.
    pub holding: usize,
    /// The comments go to GitHub with the review the user submits, someone
    /// else's pull request's (`FeatureOf::posts`): they don't hold it up.
    pub posts: bool,
}

/// Why comments of someone else's pull request's review aren't sent to an
/// agent.
pub const POSTED_LATER: &str =
    "On someone else's pull request, comments go to GitHub when you submit the review";

/// Why Approve is refused, in the order to fix it; none to approve.
pub fn approve_refusal(gate: &Gate) -> Option<String> {
    if let Some(why) = stage_refusal(gate.stage) {
        return Some(why);
    }
    if gate.draft {
        return Some(DRAFT_OPEN.into());
    }
    if !gate.read {
        return Some(STILL_READING.into());
    }
    if let Some(why) = gate.errors.first() {
        return Some(why.clone());
    }
    if !gate.dirty.is_empty() {
        return Some(format!(
            "Commit the changes in {} first",
            gate.dirty.join(", ")
        ));
    }
    if !gate.unviewed.is_empty() {
        let n = gate.unviewed.len();
        let shown: Vec<&str> = gate.unviewed.iter().take(3).map(String::as_str).collect();
        let more = if n > 3 { ", …" } else { "" };
        let files = if n == 1 { "file" } else { "files" };
        return Some(format!(
            "{n} {files} not viewed: {}{more}",
            shown.join(", ")
        ));
    }
    match gate.comments {
        _ if gate.posts => {}
        0 => {}
        1 => return Some("1 comment left: plan fixes for it or delete it".into()),
        n => {
            return Some(format!(
                "{n} comments left: plan fixes for them or delete them"
            ));
        }
    }
    match gate.holding {
        0 => None,
        1 => Some("1 finding still open".into()),
        n => Some(format!("{n} findings still open")),
    }
}

/// Where Code review of `feature` stands, as its header says it.
pub fn state(feature: &Feature) -> String {
    if feature.writer.is_some() {
        return "written on another machine".into();
    }
    if feature.status != Status::Open {
        return "feature closed".into();
    }
    if let Some(round) = &feature.fix_round {
        return round.line();
    }
    match feature.current() {
        Some(Stage::CodeReview) => "ready for review".into(),
        Some(Stage::AiReview) => "waits for AI review's findings".into(),
        Some(stage) if stage < Stage::CodeReview => {
            format!("waits for {}", stage.label())
        }
        _ => "approved".into(),
    }
}

/// What to do next on Code review's page with `gate`, as its toolbar says it,
/// and whether that is Approve. The review has `total` files, `changed` of
/// them changed since viewed.
pub fn next_step(gate: &Gate, total: usize, changed: usize) -> (String, bool) {
    let all = || format!("All {} viewed", plural(total, "file"));
    let text = if let Some(why) = stage_refusal(gate.stage) {
        why
    } else if !gate.read {
        "Reading the diff\u{2026}".into()
    } else if let Some(why) = gate.errors.first() {
        why.clone()
    } else if !gate.dirty.is_empty() {
        format!("Commit the changes in {} first", gate.dirty.join(", "))
    } else if !gate.unviewed.is_empty() {
        let left = gate.unviewed.len();
        let files = plural(total.max(left), "file");
        match changed {
            0 => format!("{left} of {files} left"),
            n => format!("{left} of {files} left \u{b7} {n} changed since viewed"),
        }
    } else if gate.comments > 0 && !gate.posts {
        let them = if gate.comments == 1 { "it" } else { "them" };
        format!(
            "{} \u{b7} {}: Plan fixes sends {them} to Build",
            all(),
            plural(gate.comments, "comment")
        )
    } else if gate.holding > 0 {
        let open = match gate.holding {
            1 => "1 finding still open".to_owned(),
            n => format!("{n} findings still open"),
        };
        format!("{} \u{b7} {open}", all())
    } else if gate.draft {
        DRAFT_OPEN.into()
    } else {
        let next = Stage::CodeReview.next().map_or("the next stage", Stage::label);
        // Its comments wait for the review on GitHub.
        let kept = match gate.comments {
            0 => String::new(),
            n if gate.posts => format!(" \u{b7} {} for GitHub", plural(n, "comment")),
            _ => String::new(),
        };
        return (format!("{}{kept} \u{b7} Approve to go on to {next}", all()), true);
    };
    (text, false)
}

impl WorktreeDiff {
    /// What Approve looks at, for the feature at `stage`: none for a diff of
    /// no feature.
    pub fn approve_gate(&self, stage: Option<Stage>) -> Option<Gate> {
        let fe = self.feature.as_ref()?;
        Some(Gate {
            stage,
            draft: self.diff.editing.is_some(),
            read: self.read && !self.reading,
            errors: fe.repos.iter().filter_map(|r| r.error.clone()).collect(),
            dirty: (fe.repos.iter())
                .filter(|r| !r.dirty.is_empty())
                .map(|r| r.shown().to_owned())
                .collect(),
            unviewed: fe.unviewed(),
            comments: self.diff.review.own(),
            holding: fe.holding(),
            posts: fe.posts,
        })
    }

    /// Feature code review `fe`, in workspace `workspace`; the user's
    /// comments `review`. Shown once read.
    pub fn of_feature(fe: FeatureOf, root: PathBuf, workspace: usize, review: Review) -> Self {
        let mut wd = WorktreeDiff {
            root,
            label: fe.title.clone(),
            workspace,
            feature: Some(fe),
            tree: Some(FileTree::default()),
            ..WorktreeDiff::default()
        };
        wd.diff.review = review;
        wd
    }

    /// The touched repositories read again: shown where the diff stood,
    /// the findings placed again.
    pub fn set_feature(
        &mut self,
        repos: Vec<(String, PathBuf, Result<viewed::Read, String>)>,
        env: Env,
    ) {
        self.reading = false;
        let (Some(fe), Some(tree)) = (self.feature.as_mut(), self.tree.as_mut()) else {
            return;
        };
        let mut files = Vec::new();
        for (name, root, read) in repos {
            let Some(repo) = fe.repos.iter_mut().find(|r| r.root == root) else {
                continue;
            };
            repo.name = name;
            match read {
                Ok(read) => {
                    repo.error = None;
                    repo.since = read.since;
                    repo.blobs = read.blobs;
                    repo.dirty = read.dirty;
                    repo.since_viewed = read.since_viewed;
                    // In the file list's order.
                    let mut read_files = read.files;
                    let rank = tree.rank(&repo.since.changes);
                    read_files
                        .sort_by_key(|f| rank.get(f.path.as_str()).copied().unwrap_or(usize::MAX));
                    repo.files = read_files;
                    files.push(root);
                }
                Err(why) => repo.error = Some(format!("{}: {why}", repo.shown())),
            }
        }
        let first = !self.read;
        self.read = true;
        self.error = fe.repos.iter().find_map(|r| r.error.clone());
        // Back after fixes changed files viewed before: what changed since
        // the last review shows, of those files only what changed since
        // they were viewed, the files viewed and unchanged folded (All
        // changes brings back all changes).
        if first {
            let fold = since_review(fe);
            let some = !fold.is_empty() || fe.repos.iter().any(|r| !r.since_viewed.is_empty());
            fe.since = some && fe.role() == Role::Review;
            self.diff.folded.extend(fold);
        }
        let shown: Vec<(PathBuf, Vec<FileDiff>)> = (files.into_iter())
            .filter_map(|root| {
                let repo = fe.repos.iter().find(|r| r.root == root)?;
                Some((root, fe.shown_files(repo, &self.diff.review.comments)))
            })
            .collect();
        self.with_diff(env, |diff, ctx| {
            diff.refresh(ctx);
            let reads = diff.reads;
            for (root, files) in shown {
                diff.set_diffs(root, reads, files, ctx);
            }
        });
        self.place_findings(env);
        if let (Some(tree), Some(fe)) = (self.tree.as_mut(), self.feature.as_ref()) {
            tree.settle(&fe.sections(), env.tree_page);
        }
        self.follow_diff(env);
    }

    /// The findings, as the record has them now, as comments after the
    /// user's own; the cursor on the one to show first, once.
    pub fn place_findings(&mut self, env: Env) {
        let Some(fe) = self.feature.as_mut() else {
            return;
        };
        let mut comments = fe.finding_comments(&self.diff.diffs);
        comments.extend(fe.sent_notes.iter().cloned());
        let focus = fe.focus.clone();
        let focus_note = fe.focus_note.clone();
        self.diff.notes_open = fe.role() == Role::Deciding;
        if self.lay_flags(comments, focus.as_deref(), focus_note.as_ref(), env)
            && let Some(fe) = self.feature.as_mut()
        {
            fe.focus = None;
            fe.focus_note = None;
        }
    }
}

impl Gerdr {
    /// Show feature `slug`'s diff across the repositories panel, as a
    /// worktree's diff shows; Esc goes back to the feature panel when it
    /// was opened from there. With `finding`, the cursor goes to that AI
    /// review finding.
    pub fn open_feature_diff(&mut self, project: &str, slug: &str, finding: Option<&str>) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.touched().next().is_none() {
            let text = format!(
                "\u{201c}{}\u{201d} hasn't changed any code yet",
                feature.title
            );
            return self.show_warning(text);
        }
        // Elsewhere on the tab the popup leaves the page where it is.
        let on_page = (self.features.panel.as_ref()).is_some_and(|panel| {
            panel.project == project
                && panel.slug == slug
                && panel.page_stage(feature) == Stage::CodeReview
        });
        let at_hand = feature.current() == Some(Stage::CodeReview) || on_page;
        if at_hand && review_views(feature) && self.show_inline_review(project, slug, finding) {
            return;
        }
        let Some(wd) = self.new_feature_diff(project, slug, finding) else {
            return;
        };
        self.show_feature_diff(wd);
    }

    /// Show what fix round `n` of feature `slug` changed, across the
    /// repositories panel as its code review shows, only to read: from where
    /// each repository's code stood before its fixes to after them, or to now
    /// while they go on. Esc goes back to the feature panel.
    pub fn open_round_diff(&mut self, project: &str, slug: &str, n: u32) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let found = (feature.fix_round.iter())
            .chain(&feature.last_fix_round)
            .chain(feature.past_rounds.iter().rev())
            .find(|r| r.n == n)
            .cloned();
        let label = crate::feature::fix_round::label(n);
        let Some(round) = found else {
            return self.show_warning(format!("{label} is gone"));
        };
        if round.trail.went.is_none() {
            return self.show_warning(format!(
                "The fixes of {label} haven't gone to the agent yet"
            ));
        }
        if round.trail.from.is_empty() {
            return self.show_warning(format!(
                "gerdr didn't read the code before the fixes of {label}: it can't tell what they changed"
            ));
        }
        let ends: BTreeMap<String, (String, Option<String>)> =
            (crate::feature::fix_round::span(&round).into_iter())
                .map(|(key, from, to)| (key, (from, to)))
                .collect();
        if ends.is_empty() {
            return self.show_warning(format!("The fixes of {label} changed no code"));
        }
        let state = crate::features::round_state_words(&round, crate::feature::now());
        let Some(mut wd) = self.new_feature_diff(project, slug, None) else {
            return;
        };
        wd.diff.review = Review::default();
        if let Some(fe) = wd.feature.as_mut() {
            fe.findings.clear();
            fe.viewed = Viewed::default();
            fe.comments = PathBuf::new();
            fe.repos.retain(|r| ends.contains_key(&r.name));
            fe.span = Some(SpanOf {
                n,
                label,
                state,
                ends,
            });
        }
        self.show_feature_diff(wd);
    }

    /// Show `wd`, a feature's diff, across the repositories panel.
    fn show_feature_diff(&mut self, mut wd: WorktreeDiff) {
        wd.tree = Some(FileTree::new(self.projects.repo_grouping));
        // It covers the other panels: they close, the repositories panel
        // kept as it was.
        self.switcher = None;
        self.project_menu = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.scrollback = None;
        self.worktrees.close();
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        if self.wd().is_some() {
            self.worktree_save_review();
            self.repos.close_worktree();
        }
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Show feature `slug`'s Code review page with the keys on its review,
    /// the cursor on finding `finding` if any; whether it shows.
    fn show_inline_review(&mut self, project: &str, slug: &str, finding: Option<&str>) -> bool {
        self.close_panels_for_tab();
        if !self.show_feature_tab(project, slug) {
            return false;
        }
        self.sync_feature_panel();
        let Some(feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        let Some(panel) = (self.features.panel.as_mut())
            .filter(|panel| panel.project == project && panel.slug == slug)
        else {
            return false;
        };
        match feature.current() == Some(Stage::CodeReview) {
            true => panel.unpick(),
            false => panel.pick(&feature, Stage::CodeReview),
        }
        panel.keys = false;
        panel.rail_keys = false;
        panel.review_view = Some(crate::chrome::ReviewView::Code);
        self.sync_inline_review();
        let env = self.worktree_env();
        let Some(wd) = self.inline_wd_mut() else {
            return false;
        };
        if let Some(fe) = wd.feature.as_mut() {
            fe.focus = finding.map(str::to_owned);
        }
        // Once read it goes there now; before, when it is.
        if finding.is_some() && wd.read {
            wd.place_findings(env);
        }
        self.refresh_pointer_focus();
        true
    }

    /// The diff of feature `slug`, not read yet.
    fn new_feature_diff(
        &self,
        project: &str,
        slug: &str,
        finding: Option<&str>,
    ) -> Option<WorktreeDiff> {
        let feature = self.feature(project, slug)?;
        let branch = feature.branch();
        let names = self.projects.names(feature);
        let repos = feature
            .touched()
            .map(|r| RepoOf {
                name: r.key(),
                label: names.label(r),
                root: r.worktree.clone(),
                branch: Some(r.branch.clone().unwrap_or_else(|| branch.clone())),
                ..RepoOf::default()
            })
            .collect();
        let dir = store::dir(None, project, slug);
        let comments = dir.join(COMMENTS);
        let standing = Standing::of(feature);
        let fe = FeatureOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            title: feature.title.clone(),
            state: standing.state,
            stage: standing.stage,
            closed: standing.closed,
            seen: feature.writer.is_some(),
            ready: standing.ready,
            round: standing.round,
            comments: comments.clone(),
            back: (self.features.panel.as_ref())
                .is_some_and(|p| p.project == project && p.slug == slug),
            inline: false,
            repos,
            viewed: feature.viewed.clone(),
            findings: (feature.review.as_ref())
                .map(|r| r.findings.clone())
                .unwrap_or_default(),
            focus: finding.map(str::to_owned),
            focus_note: None,
            sent_notes: Vec::new(),
            span: None,
            since: false,
            posts: feature.posts_review(),
        };
        let root = feature.root.clone();
        // A Zed opened from it goes to the feature's own workspace.
        let w = (self.feature_workspace(project, slug)).unwrap_or(self.active_workspace);
        let review = review::load_file(&comments).without_findings();
        Some(WorktreeDiff::of_feature(fe, root, w, review))
    }

    /// The code review on the feature tab's Code review page, while that
    /// page shows.
    pub(crate) fn inline_wd(&self) -> Option<&WorktreeDiff> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let shows = self.tab().is_some_and(crate::Tab::is_feature) && review_page(panel, feature);
        shows.then_some(panel.review.as_deref())?
    }

    /// `inline_wd`, to change.
    pub(crate) fn inline_wd_mut(&mut self) -> Option<&mut WorktreeDiff> {
        self.inline_wd()?;
        self.features.panel.as_mut()?.review.as_deref_mut()
    }

    /// Whether the diff the keys and clicks go to is the one on the page.
    pub(crate) fn on_inline_review(&self) -> bool {
        !self.repos.is_open() && self.inline_wd().is_some()
    }

    /// Bring the feature tab's review up to date, every frame: made when its
    /// page first shows, read when it comes to show, as what was over it or
    /// another page took its reads, and laid out where the tab leaves room.
    /// What it paints, while its page shows.
    pub(crate) fn sync_inline_review(&mut self) -> Option<crate::chrome::ReposView> {
        let panel = self.features.panel.as_ref()?;
        let (project, slug) = (panel.project.clone(), panel.slug.clone());
        let feature = self.feature(&project, &slug)?;
        let at_it = review_views(feature);
        let wanted = review_page(panel, feature) && self.tab().is_some_and(crate::Tab::is_feature);
        let made = panel.review.is_some();
        if !wanted {
            let panel = self.features.panel.as_mut()?;
            panel.review_shown = false;
            // The feature moved on: it starts anew if it comes back.
            if !at_it {
                panel.review = None;
            }
            return None;
        }
        if !made {
            let mut wd = self.new_feature_diff(&project, &slug, None)?;
            wd.tree = Some(FileTree::new(self.projects.repo_grouping));
            if let Some(fe) = wd.feature.as_mut() {
                fe.inline = true;
            }
            self.features.panel.as_mut()?.review = Some(Box::new(wd));
        }
        // Laid out anew only as it comes to show or the tab's size changes:
        // this runs every frame, and the tab's view isn't cheap.
        let area = self.content_rect();
        let panel = self.features.panel.as_ref()?;
        if !panel.review_shown || panel.review_area != Some(area) {
            let (_, layout) = self.feature_tab_parts()?;
            let panel = self.features.panel.as_mut()?;
            panel.review_rect = layout.review;
            panel.review_area = Some(area);
        }
        let open = self.repos.is_open();
        let panel = self.features.panel.as_mut()?;
        panel.focus = crate::chrome::TabArea::Stages;
        let fresh = !std::mem::replace(&mut panel.review_shown, !open);
        let grouping = self.projects.repo_grouping;
        let workspace = self.active_workspace;
        let env = self.inline_env();
        let sent = self.sent_notes(&project, &slug);
        let wd = self.inline_wd_mut()?;
        wd.workspace = workspace;
        wd.set_grouping(grouping, env);
        wd.with_diff(env, |diff, ctx| diff.rewrap(ctx));
        if fresh && !open {
            // A read that came back to nothing, as this one wasn't on show.
            (wd.reading, wd.again) = (false, false);
            // What was left on its code from elsewhere meanwhile.
            if !wd.diff.review_dirty {
                let path = wd.feature.as_ref().map(|fe| fe.comments.clone());
                if let Some(path) = path {
                    wd.diff.review = review::load_file(&path).without_findings();
                }
                if let Some(fe) = wd.feature.as_mut() {
                    fe.sent_notes = sent;
                }
                if wd.read {
                    wd.place_findings(env);
                }
            }
            self.worktree_diff_read();
        }
        self.inline_review_view()
    }

    /// Show `view` of Code review's page, the tab's own keys and all going to
    /// it.
    pub(crate) fn show_review_view(&mut self, view: crate::chrome::ReviewView) {
        if let Some(panel) = self.features.panel.as_mut() {
            panel.review_view = Some(view);
            panel.keys = false;
            panel.rail_keys = false;
        }
        self.refresh_pointer_focus();
    }

    /// What the feature tab's review needs of the chrome, wherever the keys
    /// are.
    pub(crate) fn inline_env(&self) -> Env {
        let area = (self.features.panel.as_ref())
            .and_then(|panel| panel.review_rect)
            .unwrap_or_else(|| self.content_rect());
        self.env_in(area, true)
    }

    /// What the feature tab's review paints.
    pub(crate) fn inline_review_view(&self) -> Option<crate::chrome::ReposView> {
        let wd = self.inline_wd()?;
        let mut view = wd.repos_view(self.inline_env());
        view.flat = true;
        // The rail has the keys, or a panel over it does.
        let rail = self.features.panel.as_ref().is_some_and(|p| p.rail_keys);
        view.detail.focused &= !rail && !self.repos.is_open();
        Some(view)
    }

    /// The record of `project`'s `slug` changed: its code review, if
    /// open, reads where its stage stands again.
    pub(crate) fn feature_diff_changed(&mut self, project: &str, slug: &str) {
        let Some(wd) = self.wd_mut() else {
            return;
        };
        if !(wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug) {
            return;
        }
        wd.again |= wd.reading;
        self.worktree_diff_read();
    }

    /// The record of `project`'s `slug` was saved: its open diff, if any,
    /// goes by where the feature stands now, with no reading.
    pub(crate) fn feature_diff_stands(&mut self, project: &str, slug: &str) {
        let Some(standing) = self.feature(project, slug).map(Standing::of) else {
            return;
        };
        if let Some(fe) = (self.wd_mut().and_then(|wd| wd.feature.as_mut()))
            .filter(|fe| fe.project == project && fe.slug == slug)
        {
            fe.stands(standing);
        }
    }

    /// The feature code review open, if one is.
    pub(crate) fn feature_of(&self) -> Option<&FeatureOf> {
        self.wd()?.feature.as_ref()
    }

    /// Read the feature's touched repositories again, off the event
    /// loop; where its stage stands, its ticks and findings, from the
    /// record now.
    pub(crate) fn feature_diff_read(&mut self) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let span = fe.span.as_ref().map(|span| span.ends.clone());
        let repos: Vec<Repo> = (feature.touched())
            .filter(|r| span.as_ref().is_none_or(|ends| ends.contains_key(&r.key())))
            .cloned()
            .collect();
        let (standing, mut viewed) = (Standing::of(feature), feature.viewed.clone());
        let mut findings = (feature.review.as_ref())
            .map(|r| r.findings.clone())
            .unwrap_or_default();
        // A fix round's diff has none of the review's.
        if span.is_some() {
            (viewed, findings) = (Viewed::default(), Vec::new());
        }
        // The blobs each repository's files were viewed on, by path.
        let kept = |key: &str| -> BTreeMap<String, String> {
            let prefix = format!("{key}/");
            (viewed.0.iter())
                .filter_map(|(k, blob)| Some((k.strip_prefix(&prefix)?.to_owned(), blob.clone())))
                .collect()
        };
        let kept: BTreeMap<String, BTreeMap<String, String>> =
            (repos.iter()).map(|r| (r.key(), kept(&r.key()))).collect();
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let env = self.worktree_env();
        if let Some(wd) = self.wd_mut() {
            wd.reading = true;
            if let Some(fe) = wd.feature.as_mut() {
                let changed = fe.findings != findings;
                fe.stands(standing);
                (fe.viewed, fe.findings) = (viewed, findings);
                wd.diff.notes_open = fe.role() == Role::Deciding;
                if changed {
                    wd.place_findings(env);
                }
            }
        }
        std::thread::spawn(move || {
            let read = |r: &Repo| match span.as_ref().and_then(|ends| ends.get(&r.key())) {
                Some((from, to)) => viewed::read_span(&r.worktree, from, to.as_deref()),
                None => viewed::read(r, kept.get(&r.key()).unwrap_or(&BTreeMap::new())),
            };
            let repos = (repos.iter())
                .map(|r| (r.key(), r.worktree.clone(), read(r)))
                .collect();
            let read = Read {
                project,
                slug,
                repos,
            };
            let _ = tx.send(crate::repos::RepoEvent::FeatureDiff(Box::new(read)));
        });
    }

    /// A read of a feature's repositories came back: shown if its code
    /// review is still the one open.
    pub(crate) fn feature_diff_event(&mut self, read: Read) {
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut() else {
            return;
        };
        let open =
            (wd.feature.as_ref()).is_some_and(|f| f.project == read.project && f.slug == read.slug);
        if !open {
            return;
        }
        if std::mem::take(&mut wd.again) {
            wd.reading = false;
            return self.worktree_diff_read();
        }
        wd.set_feature(read.repos, env);
        self.code_review_counted();
        let held = self.wd_mut().map(|wd| wd.diff.take_held());
        for (mods, sym, typed) in held.unwrap_or_default() {
            if let Some(outcome) = self.repos_key(&mods, sym, typed) {
                self.apply_repos(outcome);
            }
        }
    }

    /// Keep the open code review's counts for the feature panel.
    /// Comments of feature `slug`'s Code review are back from a dropped fix
    /// plan, `own` the user's own now: in its review if it is open, and in
    /// its count.
    pub(crate) fn code_review_comments_back(
        &mut self,
        project: &str,
        slug: &str,
        back: Vec<review::Comment>,
        own: usize,
    ) {
        let open = self.wd_mut().filter(|wd| {
            (wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug)
        });
        if let Some(wd) = open {
            wd.diff.review.comments.extend(back);
        }
        let key = (project.to_owned(), slug.to_owned());
        if let Some(counts) = self.code_reviews.get_mut(&key) {
            counts.comments = own;
        }
    }

    fn code_review_counted(&mut self) {
        let Some(wd) = self.wd().filter(|wd| wd.read) else {
            return;
        };
        let Some(fe) = wd.feature.as_ref().filter(|fe| fe.span.is_none()) else {
            return;
        };
        let counts = fe.counts(wd.diff.review.own());
        let key = (fe.project.clone(), fe.slug.clone());
        self.code_reviews.insert(key, counts);
    }

    /// Tick file `path` of the repository at `root` viewed, or untick it:
    /// kept in the feature's record.
    pub(crate) fn feature_toggle(&mut self, root: &Path, path: &str) {
        let locked = (self.wd().and_then(|wd| wd.feature.as_ref())).and_then(FeatureOf::locked);
        if let Some(why) = locked {
            return self.show_warning(why.to_owned());
        }
        let Some(fe) = self.feature_of() else {
            return;
        };
        let Some(repo) = fe.repo(root) else {
            return;
        };
        let key = viewed::key(&repo.name, path);
        let blob = repo.blobs.get(path).cloned();
        let dirty = repo.dirty.contains(path);
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        let Some(feature) = self.feature_mut(&project, &slug) else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.is_done(Stage::CodeReview) {
            return self.show_warning("Code review is done: reopen it first".into());
        }
        if let Err(why) = feature.viewed.toggle(&key, blob.as_deref(), dirty) {
            return self.show_warning(why.into());
        }
        let viewed = feature.viewed.clone();
        self.save_feature(&project, &slug);
        if let Some(fe) = self.wd_mut().and_then(|wd| wd.feature.as_mut()) {
            fe.viewed = viewed;
        }
        self.code_review_counted();
    }

    /// Tick files `paths` of the repository at `root` viewed, or untick them,
    /// as a folder's or a repository's checkbox does: kept in the feature's
    /// record, and said when some have changes not committed.
    pub(crate) fn feature_toggle_many(&mut self, root: &Path, paths: &[String]) {
        let locked = (self.wd().and_then(|wd| wd.feature.as_ref())).and_then(FeatureOf::locked);
        if let Some(why) = locked {
            return self.show_warning(why.to_owned());
        }
        let Some(fe) = self.feature_of() else {
            return;
        };
        let Some(repo) = fe.repo(root) else {
            return;
        };
        let files: Vec<viewed::ManyFile> = (paths.iter())
            .map(|path| {
                let key = viewed::key(&repo.name, path);
                (
                    key,
                    repo.blobs.get(path).cloned(),
                    repo.dirty.contains(path),
                )
            })
            .collect();
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        let Some(feature) = self.feature_mut(&project, &slug) else {
            return self.show_warning("That feature is gone".into());
        };
        if feature.is_done(Stage::CodeReview) {
            return self.show_warning("Code review is done: reopen it first".into());
        }
        let left = match feature.viewed.set_many(&files) {
            Ok(left) => left,
            Err(why) => return self.show_warning(why.into()),
        };
        let viewed = feature.viewed.clone();
        self.save_feature(&project, &slug);
        if let Some(fe) = self.wd_mut().and_then(|wd| wd.feature.as_mut()) {
            fe.viewed = viewed;
        }
        self.code_review_counted();
        match left {
            0 => {}
            1 => self.show_warning("1 file left out: commit it first".into()),
            n => self.show_warning(format!("{n} files left out: commit them first")),
        }
    }

    /// Finding `id` decided in the open code review: kept in the record,
    /// shown again as the record has it.
    pub(crate) fn feature_decide(&mut self, id: String, decision: Decision) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        // Decided, the cursor goes on to what is left to decide.
        let next = (!matches!(decision, Decision::Reopen))
            .then(|| self.wd()?.diff.next_undecided(&id))
            .flatten();
        self.decide_finding(&project, &slug, &id, decision);
        if let Some(fe) = self.wd_mut().and_then(|wd| wd.feature.as_mut()) {
            fe.focus = next;
        }
        self.feature_findings_changed(&project, &slug);
    }

    /// The project and slug of the feature whose diff is open.
    fn feature_open(&self) -> Option<(String, String)> {
        let fe = self.feature_of()?;
        Some((fe.project.clone(), fe.slug.clone()))
    }

    /// Plan fixes in the open diff, at AI review: the findings to fix go to
    /// a fix plan, or it says what they wait for.
    pub(crate) fn feature_plan_fixes(&mut self) {
        if self.feature_of().is_some_and(|fe| fe.posts) {
            return self.show_warning(POSTED_LATER.into());
        }
        if let Some((project, slug)) = self.feature_open() {
            self.send_findings(&project, &slug);
        }
    }

    /// Make fixes in the open diff, at AI review: the findings to fix go to
    /// the Build agent, or it says what they wait for.
    pub(crate) fn feature_make_fixes(&mut self) {
        if self.feature_of().is_some_and(|fe| fe.posts) {
            return self.show_warning(POSTED_LATER.into());
        }
        if let Some((project, slug)) = self.feature_open() {
            self.make_finding_fixes(&project, &slug);
        }
    }

    /// Mark every undecided finding of the open diff to fix.
    pub(crate) fn feature_fix_all(&mut self) {
        if let Some((project, slug)) = self.feature_open() {
            self.fix_all_findings(&project, &slug);
        }
    }

    /// Ask for a comment on no line or file, in the open diff.
    pub(crate) fn feature_ask_comment(&mut self) {
        if let Some((project, slug)) = self.feature_open() {
            self.ask_add_comment(&project, &slug);
        }
    }

    /// Leave the open diff for the fix plan waiting for the user.
    pub(crate) fn feature_review_fix_plan(&mut self) {
        if let Some((project, slug)) = self.feature_open() {
            self.close_feature_diff(false);
            self.open_doc_tab(&project, &slug, DocFile::FixPlan);
        }
    }

    /// A comment of the user's on no line or file of feature `slug`, kept
    /// with their other comments to go with the fixes they plan at Code
    /// review; shown first in its diff if open.
    pub(crate) fn add_general_comment(&mut self, project: &str, slug: &str, text: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(root) = feature.touched().next().map(|r| r.worktree.clone()) else {
            return self.show_warning("Nothing was changed yet to comment on".into());
        };
        let comment = Comment {
            root,
            path: String::new(),
            lines: Vec::new(),
            text: text.trim().to_owned(),
            flag: None,
        };
        let env = self.worktree_env();
        let open = self.wd_mut().filter(|wd| {
            (wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug)
        });
        if let Some(wd) = open {
            wd.diff.review.comments.push(comment);
            wd.diff.review_dirty = true;
            wd.place_findings(env);
            let at = wd.diff.review.own() - 1;
            wd.with_diff(env, |diff, ctx| diff.show_comment(at, ctx));
            self.feature_save_review();
        } else {
            let path = store::dir(None, project, slug).join(COMMENTS);
            let mut own = review::load_file(&path);
            own.comments.push(comment);
            if let Err(err) = review::save_file(&path, &own) {
                tracing::warn!("saving {}: {err}", path.display());
                return self.show_error(format!("Couldn't save the comment: {err}"));
            }
            let key = (project.to_owned(), slug.to_owned());
            if let Some(counts) = self.code_reviews.get_mut(&key) {
                counts.comments = own.own();
            }
            // With no diff open its file is what counts them.
            self.probe_feature(project, slug);
        }
        self.show_success("Comment added, to go to Build when you plan fixes".into());
    }

    /// The findings of `project`'s `slug` changed: the open code review, if
    /// it is of that feature, shows them as the record has them.
    pub(crate) fn feature_findings_changed(&mut self, project: &str, slug: &str) {
        let findings = (self.feature(project, slug))
            .and_then(|f| f.review.as_ref())
            .map(|r| r.findings.clone())
            .unwrap_or_default();
        let env = self.worktree_env();
        if let Some(wd) = self.wd_mut()
            && let Some(fe) =
                (wd.feature.as_mut()).filter(|fe| fe.project == project && fe.slug == slug)
        {
            fe.findings = findings;
            wd.place_findings(env);
        }
    }

    /// Set the user's comment on finding `id` of the open code review; empty
    /// deletes it.
    pub(crate) fn feature_note_finding(&mut self, id: &str, text: &str) {
        let Some((project, slug)) = self.feature_open() else {
            return;
        };
        match text.is_empty() {
            true => self.delete_finding_comment(&project, &slug, id),
            false => {
                self.note_finding(&project, &slug, id, text);
            }
        }
    }

    /// Delete the user's own item `id` of the open code review.
    pub(crate) fn feature_delete_own(&mut self, id: &str) {
        let Some((project, slug)) = self.feature_open() else {
            return;
        };
        self.delete_own_finding(&project, &slug, id);
    }

    /// Edit finding `id` of the open code review, in the finding dialog.
    pub(crate) fn feature_edit_finding(&mut self, id: &str) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        self.ask_edit_finding(&project, &slug, id);
    }

    /// Approve Code review, unless something holds it up (said why).
    pub(crate) fn feature_approve(&mut self) {
        let Some(wd) = self.wd() else {
            return;
        };
        let Some(fe) = wd.feature.as_ref() else {
            return;
        };
        let stage = self
            .feature(&fe.project, &fe.slug)
            .and_then(Feature::current);
        let Some(gate) = wd.approve_gate(stage) else {
            return;
        };
        if let Some(why) = approve_refusal(&gate) {
            return self.show_warning(why);
        }
        let (project, slug, back) = (fe.project.clone(), fe.slug.clone(), fe.back);
        self.read_and_tick(&project, &slug, Stage::CodeReview);
        self.close_feature_diff(back);
    }

    /// Send the user's comments (`text`, Markdown) to the Build agent:
    /// once sent, they are deleted and the review closed. The agent shows only
    /// if it waits for its prompt (`show_build_agent`).
    pub(crate) fn feature_send_back(&mut self, text: String) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        if fe.posts {
            return self.show_warning(POSTED_LATER.into());
        }
        let (project, slug, path) = (fe.project.clone(), fe.slug.clone(), fe.comments.clone());
        let stage = self.feature(&project, &slug).and_then(Feature::current);
        if let Some(why) = stage_refusal(stage) {
            return self.show_warning(why);
        }
        // What is kept to give back must be all that was sent. Saved now: once
        // the round is open the review on the page is no longer `wd`.
        self.feature_save_review();
        if !self.send_to_build(&project, &slug, Stage::CodeReview, "review comments", text) {
            return;
        }
        let kept = self.keep_comments(&project, &slug);
        let opened = kept.is_some();
        if let Some(copy) = kept {
            let ids = (0..review::load_file(&copy).comments.len())
                .map(review::comment_id)
                .collect();
            let back = crate::feature::fix_round::GiveBack::Comments { copy, ids };
            self.fix_round_gives_back(&project, &slug, back);
        }
        self.code_review_round_opened(&project, &slug, opened);
        if let Err(err) = review::save_file(&path, &Review::default()) {
            tracing::warn!("deleting {}: {err}", path.display());
            self.toast(format!("Couldn't delete the review: {err}"), Level::Error);
        }
        self.sent_comments_gone(&project, &slug);
        self.close_feature_diff(false);
        self.show_build_agent(&project, &slug);
    }

    /// Feature `slug`'s comments went to Build: they are gone from its diff
    /// wherever that is, the one on its page included, which `wd` can't reach
    /// with the round open, and from what counts them.
    fn sent_comments_gone(&mut self, project: &str, slug: &str) {
        let of = |wd: &mut crate::worktree_diff::WorktreeDiff| {
            (wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug)
        };
        let popup = self.repos.worktree.as_mut();
        let page = (self.features.panel.as_mut()).and_then(|panel| panel.review.as_deref_mut());
        let open = (popup.into_iter().chain(page)).filter_map(|wd| of(wd).then_some(wd));
        for wd in open {
            wd.diff.review.comments.retain(|c| c.flag.is_some());
            wd.diff.review_dirty = false;
        }
        self.notes_are(project, slug, Vec::new());
    }

    /// Whether a diff of feature `slug` is open, over the app or on its page:
    /// only then does `code_reviews` know more than its file does.
    pub(crate) fn diff_open_of(&self, project: &str, slug: &str) -> bool {
        let of = |wd: &crate::worktree_diff::WorktreeDiff| {
            (wd.feature.as_ref())
                .is_some_and(|fe| fe.project == project && fe.slug == slug && fe.span.is_none())
        };
        self.repos.worktree.as_ref().is_some_and(of)
            || (self.features.panel.as_ref())
                .and_then(|panel| panel.review.as_deref())
                .is_some_and(of)
    }

    /// Feature `slug`'s Code review just started a fix round, its comments
    /// `kept` for the page to show: the page goes to its fixes whatever view was
    /// picked last, and they show at once.
    fn code_review_round_opened(&mut self, project: &str, slug: &str, kept: bool) {
        if kept && let Some(f) = self.feature_mut(project, slug) {
            let round = f.fix_rounds;
            f.comment_fixes.push(crate::feature::fix_round::CommentFix {
                round,
                made: Vec::new(),
            });
            self.save_feature(project, slug);
        }
        if let Some(panel) = (self.features.panel.as_mut())
            .filter(|panel| panel.project == project && panel.slug == slug)
        {
            panel.review_view = None;
        }
        self.probe_feature(project, slug);
    }

    /// Save the user's comments in the open code review, if they changed:
    /// the findings left out; a failure says so.
    pub(crate) fn feature_save_review(&mut self) {
        let Some(wd) = self.wd_mut().filter(|wd| {
            wd.diff.review_dirty && (wd.feature.as_ref()).is_some_and(|fe| fe.span.is_none())
        }) else {
            return;
        };
        wd.diff.review_dirty = false;
        let path = wd.feature.as_ref().map(|f| f.comments.clone());
        let review = wd.diff.review.without_findings();
        if let Some(path) = path
            && let Err(err) = review::save_file(&path, &review)
        {
            tracing::warn!("saving {}: {err}", path.display());
            self.toast(format!("Couldn't save the review: {err}"), Level::Error);
        }
        self.code_review_counted();
    }

    /// Close the open code review: back to the page of the feature panel it
    /// was opened from when `back`.
    pub(crate) fn close_feature_diff(&mut self, back: bool) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        self.close_worktree_diff();
        if back {
            self.back_to_feature(&project, &slug);
        }
    }

    /// Close the open code review as Esc does.
    pub(crate) fn close_feature_review(&mut self) {
        let back = self.feature_of().is_some_and(|f| f.back);
        self.close_feature_diff(back);
    }

    /// What the feature panel's Code review row says of feature `slug`:
    /// "3 of 7 files viewed · 2 comments", once its diff was read; before,
    /// how many files were viewed, if any.
    pub fn code_review_line(&self, project: &str, slug: &str) -> Option<String> {
        if let Some(counts) = self
            .code_reviews
            .get(&(project.to_owned(), slug.to_owned()))
        {
            return line(counts);
        }
        match self.feature(project, slug)?.viewed.0.len() {
            0 => None,
            1 => Some("1 file viewed".into()),
            n => Some(format!("{n} files viewed")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chrome::{Hint, ReposHit},
        diff::{Hunk, Line},
        feature::review_import::Severity,
        git::Change,
        worktree_diff::Done,
    };
    use smithay::input::keyboard::Keysym;

    fn repo(name: &str, paths: &[&str]) -> RepoOf {
        RepoOf {
            name: name.into(),
            root: PathBuf::from(format!("/w/{name}")),
            since: Since {
                changes: (paths.iter())
                    .map(|p| Change {
                        path: (*p).into(),
                        orig: None,
                        code: 'M',
                    })
                    .collect(),
                ..Since::default()
            },
            blobs: paths
                .iter()
                .map(|p| ((*p).into(), format!("b-{p}")))
                .collect(),
            ..RepoOf::default()
        }
    }

    #[test]
    fn files_tick_on_their_blob_and_count() {
        let mut fe = FeatureOf {
            repos: vec![repo("api", &["a.rs", "b.rs"]), repo("web", &["c.ts"])],
            ..FeatureOf::default()
        };
        fe.viewed.0.insert("api/a.rs".into(), "b-a.rs".into());
        fe.viewed.0.insert("web/c.ts".into(), "old".into());
        let api = Path::new("/w/api");
        assert_eq!(fe.check(api, "a.rs"), Check::On);
        assert_eq!(fe.check(api, "b.rs"), Check::Off);
        assert_eq!(fe.check(Path::new("/w/web"), "c.ts"), Check::Partial);
        assert_eq!(
            fe.note(Path::new("/w/web"), "c.ts").as_deref(),
            Some("changed since viewed")
        );
        assert_eq!(fe.unviewed(), ["api/b.rs", "web/c.ts"]);
        let counts = fe.counts(2);
        assert_eq!(
            line(&counts).as_deref(),
            Some("1 of 3 files viewed \u{b7} 2 comments \u{b7} 1 changed since viewed")
        );
        // Not committed: can't be viewed.
        fe.repos[0].dirty.insert("a.rs".into());
        assert_eq!(fe.check(api, "a.rs"), Check::Off);
        assert_eq!(fe.note(api, "a.rs").as_deref(), Some("not committed"));
        assert_eq!(line(&Counts::default()), None);
    }

    #[test]
    fn since_the_last_review_a_changed_file_shows_only_what_changed_since() {
        use crate::{doc_review::added_file, review::Quoted};
        let mut api = repo("api", &["a.rs"]);
        api.files = vec![added_file("a.rs", "one\ntwo\nthree\n")];
        api.since_viewed = BTreeMap::from([("a.rs".to_owned(), added_file("a.rs", "three\n"))]);
        let mut fe = FeatureOf {
            stage: Some(Stage::CodeReview),
            repos: vec![api],
            ..FeatureOf::default()
        };
        let lines = |files: Vec<FileDiff>| files[0].hunks[0].lines.len();
        // All changes until it is asked for; one click away either way.
        assert_eq!(fe.scope_button(), Some(ReposButton::SinceReview));
        assert_eq!(lines(fe.shown_files(&fe.repos[0], &[])), 3);
        fe.since = true;
        assert_eq!(fe.scope_button(), Some(ReposButton::AllChanges));
        assert_eq!(fe.buttons(0)[0], (ReposButton::AllChanges, None));
        let shown = fe.shown_files(&fe.repos[0], &[]);
        assert_eq!(lines(shown.clone()), 1);
        assert_eq!(shown[0].notes, [SINCE_VIEWED]);
        // A comment of the user's on a line it hasn't: the whole diff, so
        // the comment isn't taken for outdated.
        let comment = Comment {
            root: PathBuf::from("/w/api"),
            path: "a.rs".into(),
            lines: vec![Quoted {
                kind: LineKind::Added,
                number: 1,
                text: "one".into(),
            }],
            text: "Why?".into(),
            flag: None,
        };
        assert_eq!(lines(fe.shown_files(&fe.repos[0], &[comment])), 3);
        // Nothing viewed changed since: no scope to switch.
        fe.since = false;
        fe.repos[0].since_viewed.clear();
        assert_eq!(fe.scope_button(), None);
    }

    #[test]
    fn a_review_after_fixes_opens_on_what_changed_since_viewed() {
        let mut fe = FeatureOf {
            repos: vec![repo("api", &["a.rs", "b.rs", "c.rs"])],
            ..FeatureOf::default()
        };
        // Nothing viewed changed yet: all of it shows.
        fe.viewed.0.insert("api/a.rs".into(), "b-a.rs".into());
        assert!(since_review(&fe).is_empty());
        // Edits not committed to a viewed file aren't a change since viewed.
        fe.viewed.0.insert("api/c.rs".into(), "b-c.rs".into());
        fe.repos[0].dirty.insert("c.rs".into());
        assert!(since_review(&fe).is_empty());
        assert_eq!(
            fe.note(Path::new("/w/api"), "c.rs").as_deref(),
            Some("not committed")
        );
        // A fix changed b.rs, viewed before: a.rs, viewed and the same, folds.
        fe.viewed.0.insert("api/b.rs".into(), "old".into());
        assert_eq!(since_review(&fe), [(PathBuf::from("/w/api"), "a.rs".to_owned())]);
        // A fix round's own diff is only read: nothing folds.
        fe.span = Some(SpanOf::default());
        assert!(since_review(&fe).is_empty());
    }

    #[test]
    fn approve_says_what_holds_it_up_first() {
        let ok = Gate {
            stage: Some(Stage::CodeReview),
            read: true,
            ..Gate::default()
        };
        assert_eq!(approve_refusal(&ok), None);
        let at = |stage| {
            approve_refusal(&Gate {
                stage,
                ..ok.clone()
            })
        };
        assert_eq!(
            at(Some(Stage::AiReview)).as_deref(),
            Some("Code review waits for AI review")
        );
        assert_eq!(
            at(Some(Stage::Ship)).as_deref(),
            Some("Code review is done")
        );
        assert_eq!(at(None).as_deref(), Some("Code review is done"));
        let unviewed = Gate {
            unviewed: ["api/a.rs", "api/b.rs", "web/c.ts", "web/d.ts"]
                .map(String::from)
                .to_vec(),
            comments: 2,
            ..ok.clone()
        };
        assert_eq!(
            approve_refusal(&unviewed).as_deref(),
            Some("4 files not viewed: api/a.rs, api/b.rs, web/c.ts, …")
        );
        let dirty = Gate {
            dirty: vec!["api".into(), "web".into()],
            ..unviewed.clone()
        };
        assert_eq!(
            approve_refusal(&dirty).as_deref(),
            Some("Commit the changes in api, web first")
        );
        let comments = Gate {
            unviewed: vec![],
            ..unviewed
        };
        assert_eq!(
            approve_refusal(&comments).as_deref(),
            Some("2 comments left: plan fixes for them or delete them")
        );
        let holding = Gate {
            holding: 1,
            ..ok.clone()
        };
        assert_eq!(
            approve_refusal(&holding).as_deref(),
            Some("1 finding still open")
        );
        let reading = Gate {
            read: false,
            holding: 1,
            ..ok
        };
        assert_eq!(approve_refusal(&reading).as_deref(), Some(STILL_READING));
    }

    #[test]
    fn a_findings_card_shows_the_users_words_and_an_item_of_theirs_shows_until_dropped() {
        let f = Finding {
            id: "r1-1".into(),
            file: "src/a.rs".into(),
            title: "Off".into(),
            fix: "Reviewer's".into(),
            my_fix: Some("Mine".into()),
            note: "Careful".into(),
            ..Finding::default()
        };
        let c = finding_comment(&f, "/w/api".into(), None);
        assert!(c.text.contains("Your fix: Mine"), "{}", c.text);
        assert!(!c.text.contains("Careful"), "{}", c.text);
        assert!(!c.text.contains("Reviewer's"), "{}", c.text);
        let fe = FeatureOf {
            repos: vec![RepoOf {
                name: "api".into(),
                root: "/w/api".into(),
                ..RepoOf::default()
            }],
            findings: vec![
                f,
                Finding {
                    id: "u1".into(),
                    yours: true,
                    ..Finding::default()
                },
            ],
            ..FeatureOf::default()
        };
        let comments = fe.finding_comments(&HashMap::new());
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0].flag.as_ref().unwrap().id, "r1-1");
        // The user's comment on it, right under it.
        let note = comments[1].flag.as_ref().unwrap();
        assert!(note.note && note.yours && note.id == "r1-1");
        assert_eq!(comments[1].text, "Careful");
        assert_eq!(comments[1].lines, comments[0].lines);
        let yours = comments[2].flag.as_ref().unwrap();
        assert!(yours.yours && !yours.note && review::is_general(&comments[2]));
        let mut fe = fe;
        fe.findings[1].state = FindingState::Dismissed;
        assert_eq!(fe.finding_comments(&HashMap::new()).len(), 2);
    }

    /// `wd` after the record's comment on its first finding became `note`.
    fn noted(wd: &mut WorktreeDiff, note: &str) {
        let fe = wd.feature.as_mut().unwrap();
        fe.findings[0].note = note.to_owned();
        wd.place_findings(POPUP_ENV);
    }

    #[test]
    fn a_comment_on_a_finding_is_written_under_it_and_goes_to_the_record() {
        use crate::chrome::DetailRow;
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        let none = mods(false, false);
        let right = Keysym::bracketright;
        wd.key(&none, right, right, POPUP_ENV);
        let labels = |wd: &WorktreeDiff| -> Vec<String> {
            let view = wd.view(POPUP_ENV);
            view.row_buttons
                .iter()
                .map(|(_, h)| h.label.clone())
                .collect()
        };
        let shown = labels(&wd);
        for label in ["Comment", "Edit fix\u{2026}", "Fix", "Dismiss\u{2026}"] {
            assert!(shown.iter().any(|l| l == label), "{label} in {shown:?}");
        }
        // The editor opens under the finding, inside its card.
        let c = Keysym::c;
        wd.key(&none, c, c, POPUP_ENV);
        assert!(wd.diff.editing.is_some());
        assert_eq!(wd.take_error().edit_finding, None);
        let view = wd.view(POPUP_ENV);
        let finding_last = view.rows.iter().find_map(|row| match row {
            DetailRow::Comment {
                last,
                tone: Some(_),
                ..
            } => Some(*last),
            _ => None,
        });
        assert_eq!(finding_last, Some(false));
        assert!(view.rows.iter().any(|row| matches!(
            row,
            DetailRow::Editor {
                within: Some(_),
                ..
            }
        )));
        // A first comment has nothing to delete.
        assert!(!labels(&wd).iter().any(|l| l == "Delete"));
        // Nothing written, nothing said.
        let enter = Keysym::Return;
        wd.key(&none, enter, enter, POPUP_ENV);
        assert_eq!(wd.take_error().noted, None);
        wd.key(&none, c, c, POPUP_ENV);
        wd.key(&none, c, c, POPUP_ENV);
        wd.key(&none, enter, enter, POPUP_ENV);
        assert_eq!(
            wd.take_error().noted,
            Some((ItemOf::Finding, "r1-1".to_owned(), "c".to_owned()))
        );
        // It decides nothing, and none of it is saved with the user's own.
        assert_eq!(wd.diff.review.own(), 0);
    }

    #[test]
    fn a_comment_on_a_finding_shows_inside_its_card_with_what_can_be_done_to_it() {
        use crate::chrome::DetailRow;
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        noted(&mut wd, "Careful");
        let view = wd.view(POPUP_ENV);
        let kinds: Vec<(bool, bool)> = (view.rows.iter())
            .filter_map(|row| match row {
                DetailRow::CommentHead {
                    finding, within, ..
                } => Some((finding.is_some(), within.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, [(true, false), (false, true)]);
        // Of the finding's rows the last isn't the card's; the comment's is.
        let ends: Vec<(String, bool)> = (view.rows.iter())
            .filter_map(|row| match row {
                DetailRow::Comment { text, last, .. } => Some((text.clone(), *last)),
                _ => None,
            })
            .collect();
        assert_eq!(
            ends,
            [
                ("Finding r1-1".to_owned(), false),
                ("Careful".to_owned(), true),
            ]
        );
        // It is no stop of its own, and isn't counted.
        assert!(view.title.ends_with("1 finding, open"), "{}", view.title);
        assert!(view.rows.iter().any(|row| matches!(
            row,
            DetailRow::CommentHead { finding: Some(_), place, .. } if place == "r1-1 \u{b7} finding 1 of 1"
        )));
        let none = mods(false, false);
        let right = Keysym::bracketright;
        wd.key(&none, right, right, POPUP_ENV);
        let on = wd.diff.cursor.as_ref().map(|c| c.row);
        wd.key(&none, right, right, POPUP_ENV);
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), on);
        // On the finding, Comment's place says Edit comment.
        let labels = |wd: &WorktreeDiff| -> Vec<String> {
            let view = wd.view(POPUP_ENV);
            view.row_buttons
                .iter()
                .map(|(_, h)| h.label.clone())
                .collect()
        };
        assert!(labels(&wd).iter().any(|l| l == "Edit comment"));
        // Off the finding, on its comment.
        let down = Keysym::Down;
        wd.key(&none, down, down, POPUP_ENV);
        wd.key(&none, down, down, POPUP_ENV);
        assert!(wd.diff.on_finding());
        assert_eq!(labels(&wd), ["Edit", "Delete"]);
        // f decides the finding it is on.
        let f = Keysym::f;
        wd.key(&none, f, f, POPUP_ENV);
        assert_eq!(
            wd.take_error().decided,
            Some((ItemOf::Finding, "r1-1".to_owned(), Decision::Fix))
        );
        // Del arms, Del deletes, and the finding isn't dismissed.
        let del = Keysym::Delete;
        wd.key(&none, del, del, POPUP_ENV);
        let taken = wd.take_error();
        assert_eq!((taken.noted, taken.decided), (None, None));
        assert!(wd.diff.editing.is_none());
        wd.key(&none, del, del, POPUP_ENV);
        assert_eq!(
            wd.take_error().noted,
            Some((ItemOf::Finding, "r1-1".to_owned(), String::new()))
        );
        // The cursor stays with the finding once the record says so.
        noted(&mut wd, "");
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), on);
    }

    #[test]
    fn a_comment_on_a_finding_is_edited_in_place_and_emptied_it_goes() {
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        noted(&mut wd, "Careful");
        let none = mods(false, false);
        let right = Keysym::bracketright;
        wd.key(&none, right, right, POPUP_ENV);
        // From the finding, `c` goes to the comment it has.
        let c = Keysym::c;
        wd.key(&none, c, c, POPUP_ENV);
        let enter = Keysym::Return;
        let view = wd.view(POPUP_ENV);
        assert!(view.row_buttons.iter().any(|(_, h)| h.label == "Delete"));
        // Left as it was: said again, which changes nothing.
        wd.key(&none, enter, enter, POPUP_ENV);
        assert_eq!(
            wd.take_error().noted,
            Some((ItemOf::Finding, "r1-1".to_owned(), "Careful".to_owned()))
        );
        wd.key(&none, c, c, POPUP_ENV);
        wd.click(ReposHit::Button(ReposButton::DeleteComment), POPUP_ENV);
        assert_eq!(
            wd.take_error().noted,
            Some((ItemOf::Finding, "r1-1".to_owned(), String::new()))
        );
    }

    #[test]
    fn a_findings_comment_changes_only_while_ai_review_decides_and_it_can_change() {
        let none = mods(false, false);
        let (c, right) = (Keysym::c, Keysym::bracketright);
        // Past AI review: said why, no editor, no buttons for it.
        let mut wd = popup(
            Stage::CodeReview,
            vec![finding("r1-1", FindingState::Dismissed)],
            Review::default(),
        );
        noted(&mut wd, "Careful");
        wd.key(&none, right, right, POPUP_ENV);
        wd.key(&none, c, c, POPUP_ENV);
        assert!(wd.diff.editing.is_none());
        assert!(wd.take_error().error.is_some());
        let view = wd.view(POPUP_ENV);
        let shows = |l: &str| view.row_buttons.iter().any(|(_, h)| h.label == l);
        assert!(!shows("Comment") && !shows("Edit") && !shows("Edit fix\u{2026}"));
        // At AI review a dismissed one keeps its comment open, but its fix
        // waits for it to be reopened.
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Dismissed)],
            Review::default(),
        );
        wd.key(&none, right, right, POPUP_ENV);
        let view = wd.view(POPUP_ENV);
        let shows = |l: &str| view.row_buttons.iter().any(|(_, h)| h.label == l);
        assert!(shows("Comment") && !shows("Edit fix\u{2026}"));
        wd.key(&none, c, c, POPUP_ENV);
        assert!(wd.diff.editing.is_some());
    }

    #[test]
    fn shift_e_asks_for_a_findings_fix() {
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        let right = Keysym::bracketright;
        wd.key(&mods(false, false), right, right, POPUP_ENV);
        // As input sends it: the letter's own sym, the capital as typed.
        wd.key(&mods(false, true), Keysym::e, Keysym::E, POPUP_ENV);
        assert_eq!(
            wd.take_error().edit_finding,
            Some((ItemOf::Finding, "r1-1".to_owned()))
        );
    }

    #[test]
    fn g_space_opens_the_code_in_the_repository_it_is_in() {
        use crate::diff_view::Outcome;
        let mut wd = listed(&["top.rs"]);
        press(&mut wd, plain(), Keysym::Down);
        press(&mut wd, plain(), Keysym::Down);
        press(&mut wd, plain(), Keysym::g);
        let done = press(&mut wd, plain(), Keysym::space);
        let Some(Done::Diff(Outcome::OpenInZed { root, paths, .. })) = done else {
            panic!("{done:?}");
        };
        assert_eq!(root, Path::new("/w/api"));
        assert_eq!(paths, ["top.rs"]);
        // Not the feature's own folder: Zed still finds where to open it.
        assert!(wd.shows(&root) && !wd.is(&root));
        assert!(wd.shows(Path::new("/w")));
        assert!(!wd.shows(Path::new("/elsewhere")));
    }

    #[test]
    fn a_key_the_list_does_not_take_is_the_diffs_and_the_keyboard_goes_with_it() {
        use crate::diff_view::Outcome;
        let mut wd = listed(&["src/a.rs", "top.rs"]);
        let focused = |wd: &WorktreeDiff| wd.tree.as_ref().unwrap().focused;
        wd.tree.as_mut().unwrap().focused = true;
        // The list's own stay its own.
        press(&mut wd, plain(), Keysym::Down);
        assert!(focused(&wd));
        // None's key: it stays where it was.
        assert_eq!(press(&mut wd, plain(), Keysym::z), None);
        assert!(focused(&wd));
        // Esc closes from the list, as before.
        assert_eq!(press(&mut wd, plain(), Keysym::Escape), Some(Done::Close));
        assert!(focused(&wd));
        // `g` Space opens the code there, and doesn't tick the file.
        press(&mut wd, plain(), Keysym::g);
        assert!(!focused(&wd));
        assert!(matches!(
            press(&mut wd, plain(), Keysym::space),
            Some(Done::Diff(Outcome::OpenInZed { .. }))
        ));
        // A comment started from the list is written in the diff.
        for _ in 0..3 {
            press(&mut wd, plain(), Keysym::j);
        }
        wd.tree.as_mut().unwrap().focused = true;
        press(&mut wd, plain(), Keysym::c);
        assert!(!focused(&wd));
        assert!(wd.diff.editing.is_some());
    }

    const POPUP_ENV: Env = Env {
        sheet_rows: 10,
        page: 20,
        cols: 60,
        density: crate::chrome::Density::Comfortable,
        tree_page: 20,
        tree_share: 0.25,
    };

    fn finding(id: &str, state: FindingState) -> Finding {
        Finding {
            id: id.into(),
            repo: "api".into(),
            file: "a.rs".into(),
            line: Some(1),
            title: format!("Finding {id}"),
            state,
            ..Finding::default()
        }
    }

    #[test]
    fn a_finding_dismissed_again_and_again_shows_once() {
        let same = |id: &str, state| Finding {
            title: "Same".into(),
            ..finding(id, state)
        };
        let ids = |findings: Vec<Finding>, focus: Option<&str>| -> Vec<String> {
            let fe = FeatureOf {
                findings,
                focus: focus.map(str::to_owned),
                ..FeatureOf::default()
            };
            (fe.finding_comments(&HashMap::new()).into_iter())
                .filter_map(|c| c.flag.map(|f| f.id))
                .collect()
        };
        // The fixed one stands for the repeats dismissed since.
        let fixed = vec![
            same("r1-1", FindingState::Fixed),
            same("r3-1", FindingState::Dismissed),
            same("r4-1", FindingState::Dismissed),
            finding("r4-2", FindingState::Dismissed),
        ];
        assert_eq!(ids(fixed.clone(), None), ["r1-1", "r4-2"]);
        // Shown on purpose, a repeat stays.
        assert_eq!(ids(fixed, Some("r3-1")), ["r1-1", "r3-1", "r4-2"]);
        // Only dismissed: the latest says it.
        let dismissed = vec![
            same("r3-1", FindingState::Dismissed),
            same("r4-1", FindingState::Dismissed),
        ];
        assert_eq!(ids(dismissed, None), ["r4-1"]);
    }

    /// A feature's diff at `stage`, its findings read, with `own` comments.
    fn popup(stage: Stage, findings: Vec<Finding>, own: Review) -> WorktreeDiff {
        let mut api = repo("api", &["a.rs"]);
        api.since.changes[0].code = 'A';
        let fe = FeatureOf {
            title: "Login".into(),
            stage: Some(stage),
            ready: true,
            repos: vec![api.clone()],
            findings,
            ..FeatureOf::default()
        };
        let mut wd = WorktreeDiff::of_feature(fe, "/w".into(), 0, own);
        let read = viewed::Read {
            since: api.since.clone(),
            files: vec![crate::doc_review::added_file("a.rs", "let x = 1;\n")],
            blobs: api.blobs.clone(),
            dirty: Default::default(),
            since_viewed: Default::default(),
        };
        wd.set_feature(vec![("api".into(), "/w/api".into(), Ok(read))], POPUP_ENV);
        wd
    }

    #[test]
    fn on_its_page_esc_says_it_goes_to_the_stages() {
        use crate::chrome::ReposButton;
        let mut wd = listed(&["a.rs"]);
        let close = |wd: &WorktreeDiff| {
            let hints = wd.hints();
            let hint = hints.iter().find(|h| h.button == ReposButton::Close);
            hint.map(|h| h.label.clone()).unwrap_or_default()
        };
        assert_eq!(close(&wd), "close");
        wd.feature.as_mut().unwrap().inline = true;
        assert_eq!(close(&wd), "back");
        let keys = wd.feature.as_ref().unwrap().keys(0);
        assert_eq!(keys[0], ("Esc", "back to the stages"));
    }

    /// A feature's diff at Code review over these files of one repository,
    /// read, each committed.
    fn listed(paths: &[&str]) -> WorktreeDiff {
        let api = repo("api", paths);
        let fe = FeatureOf {
            title: "Login".into(),
            stage: Some(Stage::CodeReview),
            repos: vec![api.clone()],
            ..FeatureOf::default()
        };
        let mut wd = WorktreeDiff::of_feature(fe, "/w".into(), 0, Review::default());
        wd.tree = Some(FileTree::new(crate::repos::Grouping::Folders));
        let files = paths
            .iter()
            .map(|p| crate::doc_review::added_file(p, "x\n"));
        let read = viewed::Read {
            since: api.since.clone(),
            files: files.collect(),
            blobs: api.blobs.clone(),
            dirty: Default::default(),
            since_viewed: Default::default(),
        };
        wd.set_feature(vec![("api".into(), "/w/api".into(), Ok(read))], POPUP_ENV);
        wd
    }

    /// The file list's lines, as text.
    fn tree_texts(wd: &WorktreeDiff, env: Env) -> Vec<String> {
        use crate::chrome::TreeLine;
        (wd.repos_view(env).lines.iter())
            .map(|line| match line {
                TreeLine::Repo(r) => format!("{} {:?}", r.label, r.check),
                TreeLine::Dir(d) => format!("{}/ {:?}", d.name, d.check),
                TreeLine::File(f) => format!("{} {:?}", f.path, f.check),
                TreeLine::Clean(_) => "clean".into(),
            })
            .collect()
    }

    fn press(
        wd: &mut WorktreeDiff,
        mods: smithay::input::keyboard::ModifiersState,
        sym: Keysym,
    ) -> Option<Done> {
        let done = wd.key(&mods, sym, sym, POPUP_ENV);
        wd.follow_diff(POPUP_ENV);
        done
    }

    fn plain() -> smithay::input::keyboard::ModifiersState {
        smithay::input::keyboard::ModifiersState::default()
    }

    fn alt() -> smithay::input::keyboard::ModifiersState {
        smithay::input::keyboard::ModifiersState {
            alt: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_list_shows_each_files_viewed_tick_as_its_header_does() {
        let mut wd = listed(&["src/a.rs", "src/b.rs", "top.rs"]);
        assert_eq!(
            tree_texts(&wd, POPUP_ENV),
            ["api Off", "src/ Off", "a.rs Off", "b.rs Off", "top.rs Off"]
        );
        let root = PathBuf::from("/w/api");
        let header =
            |wd: &WorktreeDiff, path: &str| wd.feature.as_ref().unwrap().check(&root, path);
        let record = |wd: &mut WorktreeDiff, path: &str| {
            let fe = wd.feature.as_mut().unwrap();
            let key = viewed::key("api", path);
            let blob = format!("b-{path}");
            fe.viewed.0.insert(key, blob);
        };
        // The one record both go by: a file's tick shows in the list, a folder
        // takes after its files.
        record(&mut wd, "src/a.rs");
        assert_eq!(header(&wd, "src/a.rs"), Check::On);
        assert_eq!(
            tree_texts(&wd, POPUP_ENV),
            [
                "api Partial",
                "src/ Partial",
                "a.rs On",
                "b.rs Off",
                "top.rs Off"
            ]
        );
        record(&mut wd, "src/b.rs");
        record(&mut wd, "top.rs");
        assert_eq!(
            tree_texts(&wd, POPUP_ENV),
            ["api On", "src/ On", "a.rs On", "b.rs On", "top.rs On"]
        );
        let counts = wd.feature.as_ref().unwrap().counts(0);
        assert_eq!((counts.files, counts.viewed), (3, 3));
    }

    #[test]
    fn a_tick_in_the_list_asks_for_what_the_headers_tick_asks() {
        let mut wd = listed(&["src/a.rs", "src/b.rs", "top.rs"]);
        let root = PathBuf::from("/w/api");
        // Its checkbox, on a file's line.
        assert_eq!(
            wd.click(ReposHit::Check(2), POPUP_ENV),
            Some(Done::Diff(crate::diff_view::Outcome::Toggle {
                root: root.clone(),
                path: "src/a.rs".into(),
            }))
        );
        assert!(wd.tree.as_ref().unwrap().focused);
        // On a folder's and a repository's: all under it.
        assert_eq!(
            wd.click(ReposHit::Check(1), POPUP_ENV),
            Some(Done::TickFiles {
                root: root.clone(),
                paths: vec!["src/a.rs".into(), "src/b.rs".into()],
            })
        );
        assert_eq!(
            press(&mut wd, plain(), Keysym::Up),
            None,
            "moves to the repository's line"
        );
        assert_eq!(
            press(&mut wd, plain(), Keysym::space),
            Some(Done::TickFiles {
                root: root.clone(),
                paths: vec!["src/a.rs".into(), "src/b.rs".into(), "top.rs".into()],
            })
        );
        // Space on a file's line.
        press(&mut wd, plain(), Keysym::End);
        assert_eq!(
            press(&mut wd, plain(), Keysym::space),
            Some(Done::Diff(crate::diff_view::Outcome::Toggle {
                root,
                path: "top.rs".into(),
            }))
        );
    }

    #[test]
    fn tab_goes_between_the_list_and_the_diff_and_esc_still_closes() {
        let mut wd = listed(&["src/a.rs", "top.rs"]);
        let focused = |wd: &WorktreeDiff| wd.tree.as_ref().unwrap().focused;
        assert!(!focused(&wd));
        assert!(wd.view(POPUP_ENV).focused);
        press(&mut wd, plain(), Keysym::Tab);
        assert!(focused(&wd));
        assert!(!wd.view(POPUP_ENV).focused);
        assert_eq!(press(&mut wd, plain(), Keysym::Escape), Some(Done::Close));
        press(&mut wd, plain(), Keysym::Tab);
        assert!(!focused(&wd));
        assert_eq!(press(&mut wd, plain(), Keysym::Escape), Some(Done::Close));
    }

    #[test]
    fn a_line_takes_the_diff_to_its_file_and_the_diff_takes_the_list_along() {
        let mut wd = listed(&["src/a.rs", "src/b.rs", "top.rs"]);
        let at = |wd: &WorktreeDiff| wd.diff.scroll.pos();
        let selected = |wd: &WorktreeDiff| {
            wd.tree
                .as_ref()
                .unwrap()
                .selected_file()
                .map(|(_, p)| p.to_owned())
        };
        assert_eq!(wd.click(ReposHit::Line(3), POPUP_ENV), None);
        assert_eq!(selected(&wd).as_deref(), Some("src/b.rs"));
        let b = at(&wd);
        assert!(b > 0.0);
        // What shows has the keys: Space ticks that file, not the first.
        wd.tree.as_mut().unwrap().focused = false;
        assert_eq!(
            press(&mut wd, plain(), Keysym::space),
            Some(Done::Diff(crate::diff_view::Outcome::Toggle {
                root: "/w/api".into(),
                path: "src/b.rs".into(),
            }))
        );
        // The diff going to the next file not viewed selects its line.
        let mods = smithay::input::keyboard::ModifiersState {
            shift: true,
            ..alt()
        };
        press(&mut wd, mods, Keysym::Down);
        assert_eq!(selected(&wd).as_deref(), Some("top.rs"));
    }

    #[test]
    fn the_lists_toolbar_and_its_keys_fold_and_regroup() {
        use crate::{chrome::TreeTool, repos::Grouping};
        let mut wd = listed(&["src/a.rs", "top.rs"]);
        assert_eq!(wd.repos_view(POPUP_ENV).grouping, Grouping::Folders);
        assert_eq!(
            press(&mut wd, alt(), Keysym::f),
            Some(Done::Grouping(Grouping::List))
        );
        assert_eq!(
            tree_texts(&wd, POPUP_ENV),
            ["api Off", "src/a.rs Off", "top.rs Off"]
        );
        // Already so: nothing to save.
        assert_eq!(wd.click(ReposHit::Tool(TreeTool::List), POPUP_ENV), None);
        assert_eq!(
            wd.click(ReposHit::Tool(TreeTool::Folders), POPUP_ENV),
            Some(Done::Grouping(Grouping::Folders))
        );
        press(&mut wd, alt(), Keysym::Left);
        assert_eq!(tree_texts(&wd, POPUP_ENV), ["api Off"]);
        wd.click(ReposHit::Tool(TreeTool::Expand), POPUP_ENV);
        assert_eq!(tree_texts(&wd, POPUP_ENV).len(), 4);
    }

    #[test]
    fn the_lists_toolbar_goes_to_the_next_and_previous_file_not_viewed() {
        use crate::chrome::TreeTool;
        let mut wd = listed(&["src/a.rs", "src/b.rs", "top.rs"]);
        let selected = |wd: &WorktreeDiff| {
            wd.tree
                .as_ref()
                .unwrap()
                .selected_file()
                .map(|(_, p)| p.to_owned())
        };
        assert_eq!(wd.repos_view(POPUP_ENV).tree_nav, Some(true));
        // From the list, which keeps the keyboard.
        assert_eq!(wd.click(ReposHit::Line(2), POPUP_ENV), None);
        assert_eq!(selected(&wd).as_deref(), Some("src/a.rs"));
        let next = wd.click(ReposHit::Tool(TreeTool::NextUnviewed), POPUP_ENV);
        assert_eq!(next, None);
        wd.follow_diff(POPUP_ENV);
        assert_eq!(selected(&wd).as_deref(), Some("src/b.rs"));
        assert!(wd.tree.as_ref().unwrap().focused);
        wd.click(ReposHit::Tool(TreeTool::PrevUnviewed), POPUP_ENV);
        wd.follow_diff(POPUP_ENV);
        assert_eq!(selected(&wd).as_deref(), Some("src/a.rs"));
        // Round past the start, as Alt+Shift+Up goes.
        let up = smithay::input::keyboard::ModifiersState {
            shift: true,
            ..alt()
        };
        press(&mut wd, up, Keysym::Up);
        assert_eq!(selected(&wd).as_deref(), Some("top.rs"));
        assert!(wd.tree.as_ref().unwrap().focused);
    }

    #[test]
    fn with_no_room_for_it_the_list_is_gone_and_the_diff_keeps_the_keys() {
        let mut wd = listed(&["src/a.rs", "top.rs"]);
        let narrow = Env {
            tree_share: 0.0,
            ..POPUP_ENV
        };
        let view = wd.repos_view(narrow);
        assert!(!view.diff_tree && view.lines.is_empty());
        assert_eq!(wd.key(&plain(), Keysym::Tab, Keysym::Tab, narrow), None);
        assert!(!wd.tree.as_ref().unwrap().focused);
        assert!(wd.view(narrow).focused);
        assert_eq!(wd.click(ReposHit::Line(1), narrow), None);
        assert!(!wd.tree.as_ref().unwrap().focused);
        let wide = wd.repos_view(POPUP_ENV);
        assert!(wide.diff_tree && !wide.lines.is_empty());
    }

    fn mods(ctrl: bool, shift: bool) -> smithay::input::keyboard::ModifiersState {
        smithay::input::keyboard::ModifiersState {
            ctrl,
            shift,
            ..Default::default()
        }
    }

    /// What its title row and footer offer, by button.
    fn offered(wd: &WorktreeDiff) -> (Vec<ReposButton>, Vec<ReposButton>) {
        let title = wd
            .view(POPUP_ENV)
            .title_buttons
            .into_iter()
            .map(|h| h.button);
        let title =
            title.filter(|b| {
                !matches!(
                    b,
                    ReposButton::PrevComment | ReposButton::NextComment | ReposButton::Close
                )
            });
        let hints = wd.hints().into_iter().map(|h| h.button).collect();
        (title.collect(), hints)
    }

    #[test]
    fn it_shows_only_what_works_at_the_stage_the_feature_is_at() {
        use ReposButton as B;
        let at = |stage, ready, round| {
            let fe = FeatureOf {
                title: "Login".into(),
                stage,
                ready,
                round,
                findings: vec![finding("r1-1", FindingState::Open)],
                ..FeatureOf::default()
            };
            (fe.role(), fe.heading(), fe.buttons(0))
        };
        let nothing: Vec<(ReposButton, Option<String>)> = Vec::new();
        let early = at(Some(Stage::Verify), false, None);
        assert_eq!(early.0, Role::Early);
        assert_eq!(early.1, "Code of \u{201c}Login\u{201d}");
        assert_eq!(early.2, nothing);
        let reading = at(Some(Stage::AiReview), false, None);
        assert_eq!(reading.0, Role::Reading);
        assert_eq!(reading.1, "AI review of \u{201c}Login\u{201d}");
        assert_eq!(reading.2, nothing);
        let deciding = at(Some(Stage::AiReview), true, None);
        assert_eq!(deciding.0, Role::Deciding);
        assert_eq!(
            deciding.2,
            vec![
                (B::AddComment, None),
                (B::FixAllFindings, None),
                (
                    B::MakeFixes,
                    Some("Decide the 1 finding left first".to_owned())
                ),
            ]
        );
        let planning = Some((FixPhase::Planning, "planning fixes".to_owned()));
        let round = at(Some(Stage::AiReview), true, planning);
        assert_eq!(round.0, Role::Round(FixPhase::Planning));
        assert_eq!(round.2, nothing);
        let ready = Some((FixPhase::Ready, "fix plan ready".to_owned()));
        assert_eq!(
            at(Some(Stage::AiReview), true, ready).2,
            vec![(B::ReviewFixPlan, None)]
        );
        let review = at(Some(Stage::CodeReview), true, None);
        assert_eq!(review.0, Role::Review);
        assert_eq!(review.1, "Code review of \u{201c}Login\u{201d}");
        assert_eq!(
            review.2,
            vec![
                (B::AddComment, None),
                (B::PlanFixes, Some(NO_OWN_COMMENT.to_owned())),
                (B::Approve, None),
            ]
        );
        let past = at(Some(Stage::ManualTest), true, None);
        assert_eq!(past.0, Role::Done);
        assert_eq!(past.1, "Code review of \u{201c}Login\u{201d}");
        assert_eq!(past.2, vec![(B::GoOn, None), (B::Reopen, None)]);
        assert_eq!(at(None, true, None).0, Role::Done);
    }

    #[test]
    fn a_done_code_review_reads_as_worked_and_changes_nothing() {
        use crate::{diff_view::Outcome, worktree_diff::Done};
        use ReposButton as B;
        let enter = Keysym::Return;
        let mut wd = popup(Stage::ManualTest, Vec::new(), Review::default());
        let fe = wd.feature.as_ref().unwrap();
        assert_eq!(fe.role(), Role::Done);
        assert_eq!(fe.locked(), Some(REVIEW_DONE));
        // Going on, or back to it: no comment, no viewed tick.
        let (title, hints) = offered(&wd);
        assert_eq!(title, [B::GoOn, B::Reopen]);
        for dead in [B::Comment, B::AddComment, B::MarkViewed, B::Approve, B::PlanFixes] {
            assert!(!title.contains(&dead) && !hints.contains(&dead), "{dead:?}");
        }
        let go = (wd.feature_hints().into_iter()).find(|h| h.button == B::GoOn);
        assert_eq!(go.map(|h| h.label), Some("Go to Manual test".to_owned()));
        // Its keys and buttons go to its host.
        assert_eq!(
            wd.key(&mods(true, false), enter, enter, POPUP_ENV),
            Some(Done::GoOn)
        );
        assert_eq!(
            wd.key(&mods(true, true), enter, enter, POPUP_ENV),
            Some(Done::Reopen)
        );
        assert_eq!(
            wd.click(ReposHit::Button(B::Reopen), POPUP_ENV),
            Some(Done::Reopen)
        );
        // A comment is refused, saying why.
        let c = Keysym::c;
        let none = mods(false, false);
        assert_eq!(
            wd.key(&none, c, c, POPUP_ENV),
            Some(Done::Diff(Outcome::Notice(REVIEW_DONE)))
        );
        assert!(wd.diff.editing.is_none());
        let upper_c = Keysym::C;
        assert_eq!(
            wd.key(&mods(false, true), upper_c, upper_c, POPUP_ENV),
            Some(Done::Diff(Outcome::Notice(REVIEW_DONE)))
        );
        // Closed, it has nothing to go on to.
        wd.feature.as_mut().unwrap().closed = true;
        assert_eq!(offered(&wd).0, Vec::<ReposButton>::new());
        assert_eq!(wd.feature.as_ref().unwrap().locked(), Some(FEATURE_CLOSED));
    }

    #[test]
    fn ai_review_goes_by_its_findings_and_never_asks_for_a_comment() {
        use crate::worktree_diff::Done;
        use ReposButton as B;
        let enter = Keysym::Return;
        let mut wd = popup(
            Stage::AiReview,
            vec![
                finding("r1-1", FindingState::Fix),
                finding("r1-2", FindingState::Open),
                finding("r1-3", FindingState::Dismissed),
            ],
            Review::default(),
        );
        let (title, hints) = offered(&wd);
        assert_eq!(title, [B::AddComment, B::FixAllFindings, B::MakeFixes]);
        for dead in [B::Approve, B::SendBack, B::CommentFile] {
            assert!(!title.contains(&dead) && !hints.contains(&dead), "{dead:?}");
        }
        // Greyed in both places, saying what it waits for.
        let why = |hints: Vec<Hint>| {
            hints
                .into_iter()
                .find(|h| h.button == B::MakeFixes)
                .unwrap()
        };
        let waits = Some("Decide the 1 finding left first".to_owned());
        assert_eq!(why(wd.view(POPUP_ENV).title_buttons).disabled, waits);
        assert_eq!(why(wd.hints()).disabled, waits);
        assert_eq!(why(wd.hints()).key, "Ctrl+Enter");
        // The summary says where they stand.
        let summary = wd.summary();
        assert!(
            summary.starts_with("1 to fix \u{b7} 1 dismissed \u{b7} 1 to decide"),
            "{summary}"
        );
        // Both keys go to its host, which says what they wait for.
        assert_eq!(
            wd.key(&mods(true, false), enter, enter, POPUP_ENV),
            Some(Done::MakeFixes)
        );
        assert_eq!(
            wd.key(&mods(true, true), enter, enter, POPUP_ENV),
            Some(Done::MakeFixes)
        );
        assert_eq!(
            wd.click(ReposHit::Button(B::MakeFixes), POPUP_ENV),
            Some(Done::MakeFixes)
        );
        let upper_f = Keysym::F;
        assert_eq!(
            wd.key(&mods(false, true), upper_f, upper_f, POPUP_ENV),
            Some(Done::FixAll)
        );
        let upper_c = Keysym::C;
        assert_eq!(
            wd.key(&mods(false, true), upper_c, upper_c, POPUP_ENV),
            Some(Done::AddComment)
        );
        // All decided: Plan fixes can be pressed, and Fix all open is gone.
        let mut wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Fix)],
            Review::default(),
        );
        assert_eq!(offered(&wd).0, [B::AddComment, B::MakeFixes, B::PlanFixes]);
        assert_eq!(why(wd.view(POPUP_ENV).title_buttons).disabled, None);
        assert_eq!(
            wd.key(&mods(false, true), upper_f, upper_f, POPUP_ENV),
            Some(Done::Diff(crate::diff_view::Outcome::Notice(
                "No finding left to decide"
            )))
        );
    }

    #[test]
    fn what_the_user_wrote_at_ai_review_goes_through_a_plan() {
        use crate::worktree_diff::Done;
        use ReposButton as B;
        let enter = Keysym::Return;
        let noted = Finding {
            note: "Careful".into(),
            ..finding("r1-1", FindingState::Fix)
        };
        let comment = Review {
            comments: vec![Comment {
                root: "/w/api".into(),
                path: "src/a.rs".into(),
                text: "This name is wrong".into(),
                ..Comment::default()
            }],
            ..Review::default()
        };
        let plain = || vec![finding("r1-1", FindingState::Fix)];
        for mut wd in [
            popup(Stage::AiReview, vec![noted], Review::default()),
            popup(Stage::AiReview, plain(), comment),
        ] {
            let title = offered(&wd).0;
            assert!(title.contains(&B::PlanFixes), "{title:?}");
            assert!(!title.contains(&B::MakeFixes), "{title:?}");
            // Ctrl+Shift+Enter plans them, as on every page; Ctrl+Enter
            // makes fixes only, so here it says a plan comes first.
            assert_eq!(
                wd.key(&mods(true, true), enter, enter, POPUP_ENV),
                Some(Done::PlanFixes)
            );
            assert_eq!(
                wd.key(&mods(true, false), enter, enter, POPUP_ENV),
                Some(Done::Diff(crate::diff_view::Outcome::Notice(
                    crate::worktree_diff::PLAN_FIRST
                )))
            );
            let hint = |b| wd.hints().into_iter().find(|h| h.button == b).unwrap();
            assert_eq!(hint(B::PlanFixes).key, "Ctrl+Shift+Enter");
        }
        // With nothing of theirs the plan is a choice, on the other chord.
        let mut wd = popup(Stage::AiReview, plain(), Review::default());
        assert_eq!(
            wd.key(&mods(true, true), enter, enter, POPUP_ENV),
            Some(Done::PlanFixes)
        );
        assert_eq!(
            wd.click(ReposHit::Button(B::MakeFixes), POPUP_ENV),
            Some(Done::MakeFixes)
        );
        let hint = |b| wd.hints().into_iter().find(|h| h.button == b).unwrap();
        assert_eq!(hint(B::MakeFixes).key, "Ctrl+Enter");
        assert_eq!(hint(B::PlanFixes).key, "Ctrl+Shift+Enter");
    }

    #[test]
    fn code_review_plans_fixes_only_for_comments_and_approves() {
        use crate::worktree_diff::Done;
        use ReposButton as B;
        let enter = Keysym::Return;
        let mut wd = popup(Stage::CodeReview, Vec::new(), Review::default());
        assert_eq!(offered(&wd).0, [B::AddComment, B::PlanFixes, B::Approve]);
        assert_eq!(
            wd.key(&mods(true, true), enter, enter, POPUP_ENV),
            Some(Done::Diff(crate::diff_view::Outcome::Notice(
                NO_OWN_COMMENT
            )))
        );
        assert_eq!(
            wd.key(&mods(true, false), enter, enter, POPUP_ENV),
            Some(Done::Approve)
        );
        let hint = |wd: &WorktreeDiff, b| wd.hints().into_iter().find(|h| h.button == b).unwrap();
        assert_eq!(hint(&wd, B::PlanFixes).key, "Ctrl+Shift+Enter");
        assert_eq!(hint(&wd, B::Approve).key, "Ctrl+Enter");
        let own = Review {
            comments: vec![Comment {
                root: "/w/api".into(),
                text: "The whole of it reads badly".into(),
                ..Comment::default()
            }],
            ..Review::default()
        };
        let mut wd = popup(Stage::CodeReview, Vec::new(), own);
        assert_eq!(hint(&wd, B::PlanFixes).disabled, None);
        let sent = wd.key(&mods(true, true), enter, enter, POPUP_ENV);
        let Some(Done::SendBack(text)) = sent else {
            panic!("{sent:?}");
        };
        assert!(text.contains("### C1: General comment\n"), "{text}");
        assert!(text.contains("The whole of it reads badly"), "{text}");
        assert!(!text.contains("the whole file"), "{text}");
    }

    #[test]
    fn approve_says_what_holds_it_up_where_it_shows() {
        use ReposButton as B;
        let mut wd = popup(Stage::CodeReview, Vec::new(), Review::default());
        let gate = wd.approve_gate(Some(Stage::CodeReview)).unwrap();
        let why = approve_refusal(&gate);
        assert!(why.as_deref().is_some_and(|w| w.contains("not viewed")), "{why:?}");
        // The popup's title row and the footer both grey it, saying why.
        let approve = |hints: Vec<Hint>| hints.into_iter().find(|h| h.button == B::Approve);
        assert_eq!(approve(wd.view(POPUP_ENV).title_buttons).unwrap().disabled, why);
        assert_eq!(approve(wd.hints()).unwrap().disabled, why);
        // Laid into its page, they leave the title row for the page's line;
        // the footer keeps their keys.
        wd.feature.as_mut().unwrap().inline = true;
        let (title, hints) = offered(&wd);
        for gone in [B::AddComment, B::PlanFixes, B::Approve] {
            assert!(!title.contains(&gone), "{gone:?} in {title:?}");
            assert!(hints.contains(&gone), "{gone:?} not in {hints:?}");
        }
    }

    #[test]
    fn the_page_line_says_what_comes_next() {
        let ready = Gate {
            stage: Some(Stage::CodeReview),
            read: true,
            ..Gate::default()
        };
        let next = |gate: &Gate| next_step(gate, 8, 1);
        assert_eq!(
            next(&ready),
            (
                "All 8 files viewed \u{b7} Approve to go on to Manual test".to_owned(),
                true
            )
        );
        let with = |f: &dyn Fn(&mut Gate)| {
            let mut gate = ready.clone();
            f(&mut gate);
            next(&gate)
        };
        let not = |text: &str| (text.to_owned(), false);
        assert_eq!(
            with(&|g| g.unviewed = vec!["api/a.rs".into(), "api/b.rs".into()]),
            not("2 of 8 files left \u{b7} 1 changed since viewed")
        );
        assert_eq!(
            with(&|g| g.comments = 2),
            not("All 8 files viewed \u{b7} 2 comments: Plan fixes sends them to Build")
        );
        assert_eq!(
            with(&|g| g.comments = 1),
            not("All 8 files viewed \u{b7} 1 comment: Plan fixes sends it to Build")
        );
        assert_eq!(
            with(&|g| g.holding = 1),
            not("All 8 files viewed \u{b7} 1 finding still open")
        );
        assert_eq!(
            with(&|g| g.dirty = vec!["api".into()]),
            not("Commit the changes in api first")
        );
        assert_eq!(with(&|g| g.read = false), not("Reading the diff\u{2026}"));
        assert_eq!(with(&|g| g.draft = true), not(DRAFT_OPEN));
        assert_eq!(
            with(&|g| g.stage = Some(Stage::ManualTest)),
            not("Code review is done")
        );
        // Whatever it says, Approve is ready exactly when nothing refuses it.
        for gate in [ready.clone(), Gate { comments: 1, ..ready.clone() }] {
            assert_eq!(next(&gate).1, approve_refusal(&gate).is_none());
        }
    }

    #[test]
    fn a_comment_that_went_to_build_shows_in_the_diff_and_stays_as_it_was() {
        use crate::diff_view::SENT_COMMENT;
        let mut wd = popup(Stage::CodeReview, Vec::new(), Review::default());
        let note = Comment {
            root: wd.feature.as_ref().unwrap().repos[0].root.clone(),
            path: "src/a.rs".into(),
            text: "Rename it".into(),
            flag: Some(Flag {
                id: "C1".into(),
                state: FindingState::Sent,
                yours: true,
                sent: true,
                ..Flag::default()
            }),
            ..Comment::default()
        };
        wd.feature.as_mut().unwrap().sent_notes = vec![note.clone()];
        wd.feature.as_mut().unwrap().focus = Some("C1".into());
        wd.place_findings(POPUP_ENV);
        assert!(wd.diff.review.comments.contains(&note));
        assert!(!wd.diff.on_yours());
        // Never saved or sent as the user's.
        assert_eq!(wd.diff.review.own(), 0);
        let none = mods(false, false);
        for sym in [Keysym::f, Keysym::Delete] {
            wd.key(&none, sym, sym, POPUP_ENV);
            let said = wd.take_error().error;
            assert_eq!(said.as_deref(), Some(SENT_COMMENT), "{sym:?}");
        }
        assert!(wd.diff.review.comments.contains(&note));
    }

    #[test]
    fn a_comment_whose_fix_is_made_opens_the_commit_it_was_made_in() {
        use crate::diff_view::{NO_FIX_COMMIT, Outcome, SHOW_FIX};
        let mut wd = popup(Stage::CodeReview, Vec::new(), Review::default());
        let root = wd.feature.as_ref().unwrap().repos[0].root.clone();
        let note = Comment {
            root: root.clone(),
            path: "src/a.rs".into(),
            text: "Rename it".into(),
            flag: Some(Flag {
                id: "C1".into(),
                state: FindingState::Sent,
                yours: true,
                sent: true,
                made: true,
                fixed_in: Some("1a2b3c4".into()),
                ..Flag::default()
            }),
            ..Comment::default()
        };
        let o = Keysym::O;
        let shift = mods(false, true);
        assert_eq!(
            wd.key(&shift, Keysym::o, o, POPUP_ENV),
            Some(Done::Diff(Outcome::Notice(NO_FIX_COMMIT)))
        );
        wd.feature.as_mut().unwrap().sent_notes = vec![note];
        wd.feature.as_mut().unwrap().focus = Some("C1".into());
        wd.place_findings(POPUP_ENV);
        let shown = wd.view(POPUP_ENV).row_buttons;
        assert!(shown.iter().any(|(_, h)| h.label == SHOW_FIX), "{shown:?}");
        let commit = Some(Done::Diff(Outcome::ShowCommit {
            root,
            oid: "1a2b3c4".into(),
        }));
        assert_eq!(wd.key(&shift, Keysym::o, o, POPUP_ENV), commit);
    }

    #[test]
    fn a_general_comment_is_listed_first_gone_to_edited_and_deleted() {
        use crate::{chrome::DetailRow, diff_view::DiffRow};
        let own = Review {
            comments: vec![Comment {
                root: "/w/api".into(),
                text: "The name reads badly".into(),
                ..Comment::default()
            }],
            ..Review::default()
        };
        let mut wd = popup(Stage::CodeReview, Vec::new(), own);
        assert!(
            matches!(wd.diff.rows[0], DiffRow::General),
            "{:?}",
            wd.diff.rows
        );
        assert!(matches!(wd.diff.rows[1], DiffRow::CommentHead { c: 0, .. }));
        assert_eq!(wd.diff.outdated_count(), 0, "it isn't on a line gone");
        let view = wd.view(POPUP_ENV);
        assert!(view.rows.iter().any(|row| matches!(
            row,
            DetailRow::Hunk { context, .. } if context == "General comments"
        )));
        // ] goes to it.
        let none = mods(false, false);
        let right = Keysym::bracketright;
        assert_eq!(wd.key(&none, right, right, POPUP_ENV), None);
        let on = wd.diff.cursor.as_ref().map(|c| wd.diff.rows[c.row]);
        assert!(
            matches!(on, Some(DiffRow::CommentHead { c: 0, .. })),
            "{on:?}"
        );
        // c edits it, in its place.
        let c = Keysym::c;
        assert_eq!(wd.key(&none, c, c, POPUP_ENV), None);
        assert_eq!(wd.diff.editing.as_ref().and_then(|e| e.at), Some(0));
        assert!(
            matches!(wd.diff.rows[1], DiffRow::Editor { .. }),
            "{:?}",
            wd.diff.rows
        );
        // Ctrl+Enter keeps what is written, and approves nothing.
        let enter = Keysym::Return;
        let kept = wd.key(&mods(true, false), enter, enter, POPUP_ENV);
        assert!(
            !matches!(kept, Some(crate::worktree_diff::Done::Approve)),
            "{kept:?}"
        );
        assert_eq!(wd.diff.editing.as_ref().map(|e| e.at), None);
        // Del arms, Del deletes: gone with its heading.
        assert_eq!(wd.key(&none, right, right, POPUP_ENV), None);
        let del = Keysym::Delete;
        assert_eq!(wd.key(&none, del, del, POPUP_ENV), None);
        assert_eq!(wd.diff.review.own(), 1);
        assert_eq!(wd.key(&none, del, del, POPUP_ENV), None);
        assert_eq!(wd.diff.review.own(), 0);
        assert!(!matches!(wd.diff.rows.first(), Some(DiffRow::General)));
    }

    #[test]
    fn an_item_of_the_users_says_whose_it_is_and_is_edited_or_dropped() {
        use crate::chrome::DetailRow;
        let mine = Finding {
            id: "u1".into(),
            title: "Rename it".into(),
            state: FindingState::Fix,
            yours: true,
            ..Finding::default()
        };
        let mut wd = popup(
            Stage::AiReview,
            vec![mine, finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        let none = mods(false, false);
        let right = Keysym::bracketright;
        assert_eq!(wd.key(&none, right, right, POPUP_ENV), None);
        assert!(wd.diff.on_yours());
        let view = wd.view(POPUP_ENV);
        assert!(
            view.rows.iter().any(|row| matches!(
                row,
                DetailRow::CommentHead { finding: None, place, .. } if place == "u1 \u{b7} comment 1 of 1"
            )),
            "{:?}",
            view.rows
        );
        let labels: Vec<&str> = view
            .row_buttons
            .iter()
            .map(|(_, h)| h.label.as_str())
            .collect();
        assert_eq!(labels, ["Edit\u{2026}", "Delete"]);
        // f says why not; Del goes at once, with nothing to say.
        let f = Keysym::f;
        wd.key(&none, f, f, POPUP_ENV);
        let crate::worktree_diff::Taken { error, decided, .. } = wd.take_error();
        assert_eq!(error.as_deref(), Some(crate::diff_view::YOUR_COMMENT));
        assert_eq!(decided, None);
        let del = Keysym::Delete;
        wd.key(&none, del, del, POPUP_ENV);
        let taken = wd.take_error();
        assert_eq!(taken.decided, None);
        assert_eq!(taken.delete_own, Some((ItemOf::Finding, "u1".to_owned())));
        assert!(wd.diff.editing.is_none());
    }

    #[test]
    fn a_finding_can_be_decided_only_at_ai_review() {
        let decides = |stage| {
            let mut wd = popup(
                stage,
                vec![finding("r1-1", FindingState::Dismissed)],
                Review::default(),
            );
            let none = mods(false, false);
            let right = Keysym::bracketright;
            assert_eq!(wd.key(&none, right, right, POPUP_ENV), None);
            assert!(wd.diff.on_finding());
            let view = wd.view(POPUP_ENV);
            (view.row_buttons.iter()).any(|(_, h)| h.button == ReposButton::FixFinding)
        };
        assert!(decides(Stage::AiReview));
        assert!(!decides(Stage::CodeReview));
        assert!(!decides(Stage::Verify));
    }

    #[test]
    fn deciding_goes_on_to_what_is_left() {
        let wd = popup(
            Stage::AiReview,
            vec![
                finding("r1-1", FindingState::Open),
                finding("r1-2", FindingState::Dismissed),
                finding("r1-3", FindingState::Open),
            ],
            Review::default(),
        );
        assert_eq!(wd.diff.next_undecided("r1-1").as_deref(), Some("r1-3"));
        // Round past the last.
        assert_eq!(wd.diff.next_undecided("r1-3").as_deref(), Some("r1-1"));
        let wd = popup(
            Stage::AiReview,
            vec![finding("r1-1", FindingState::Open)],
            Review::default(),
        );
        assert_eq!(wd.diff.next_undecided("r1-1"), None);
    }

    #[test]
    fn its_keys_list_what_it_offers() {
        let keys = |stage| {
            let fe = FeatureOf {
                stage: Some(stage),
                ready: true,
                ..FeatureOf::default()
            };
            fe.keys(0)
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>()
        };
        let ai = keys(Stage::AiReview);
        assert!(ai.contains(&"Shift+F") && ai.contains(&"f"));
        assert!(ai.contains(&"Ctrl+Shift+Enter"));
        let code = keys(Stage::CodeReview);
        assert!(code.contains(&"Ctrl+Shift+Enter") && code.contains(&"Shift+C"));
        assert!(!code.contains(&"Shift+F") && !code.contains(&"f"));
        assert!(!keys(Stage::Build).contains(&"Ctrl+Enter"));
    }

    #[test]
    fn findings_sit_on_their_line_else_on_their_file() {
        let line = |number, kind, text: &str| Line {
            kind,
            raw: None,
            text: text.into(),
            number,
            old: 0,
            words: Vec::new(),
            syntax: Vec::new(),
        };
        let file = FileDiff {
            path: "a.rs".into(),
            hunks: vec![Hunk {
                context: String::new(),
                lines: vec![
                    line(4, LineKind::Removed, "old"),
                    line(4, LineKind::Added, "new"),
                ],
            }],
            ..FileDiff::default()
        };
        let mut f = Finding {
            id: "r1-1".into(),
            severity: Severity::High,
            repo: "api".into(),
            file: "a.rs".into(),
            line: Some(4),
            title: "Leaks".into(),
            body: "It leaks.".into(),
            fix: "Close it.".into(),
            ..Finding::default()
        };
        let c = finding_comment(&f, "/w/api".into(), Some(&file));
        assert_eq!(c.lines.len(), 1);
        assert_eq!(
            (c.lines[0].kind, c.lines[0].text.as_str()),
            (LineKind::Added, "new")
        );
        // Its severity and state are on its heading (`review::Flag`).
        assert_eq!(c.text, "Leaks\nIt leaks.\nFix: Close it.");
        assert_eq!(c.flag.as_ref().map(|f| f.id.as_str()), Some("r1-1"));
        // Its line not in the diff: on the file, saying the line.
        f.line = Some(9);
        f.state = FindingState::Dismissed;
        f.reason = "on purpose".into();
        let c = finding_comment(&f, "/w/api".into(), Some(&file));
        assert!(c.lines.is_empty());
        assert!(c.text.starts_with("Leaks\nDismissed: on purpose\nline 9\n"));
    }

    #[test]
    fn its_view_is_read_only_with_viewed_ticks_and_finding_cards() {
        use crate::{
            chrome::{Density, DetailRow, ReposButton, ReposHit},
            diff_view::Outcome,
            worktree_diff::Done,
        };
        use smithay::input::keyboard::{Keysym, ModifiersState};
        const ENV: Env = Env {
            sheet_rows: 10,
            page: 20,
            cols: 60,
            density: Density::Comfortable,
            tree_page: 20,
            tree_share: 0.25,
        };
        let mut api = repo("api", &["a.rs"]);
        api.since.changes[0].code = 'A';
        let fe = FeatureOf {
            stage: Some(Stage::CodeReview),
            repos: vec![RepoOf {
                error: None,
                ..api.clone()
            }],
            findings: vec![Finding {
                id: "r1-1".into(),
                repo: "api".into(),
                file: "a.rs".into(),
                line: Some(1),
                title: "Leaks".into(),
                ..Finding::default()
            }],
            focus: Some("r1-1".into()),
            focus_note: None,
            sent_notes: Vec::new(),
            ..FeatureOf::default()
        };
        let mut wd = WorktreeDiff::of_feature(fe, "/w".into(), 0, Review::default());
        let read = viewed::Read {
            since: api.since.clone(),
            files: vec![crate::doc_review::added_file("a.rs", "let x = 1;\n")],
            blobs: api.blobs.clone(),
            dirty: Default::default(),
            since_viewed: Default::default(),
        };
        wd.set_feature(vec![("api".into(), "/w/api".into(), Ok(read))], ENV);
        // The finding sits on its line, as a card of its own.
        assert_eq!(wd.diff.review.comments.len(), 1);
        assert_eq!(wd.diff.review.comments[0].lines.len(), 1);
        assert_eq!(wd.diff.review.own(), 0);
        assert_eq!(wd.feature.as_ref().and_then(|f| f.focus.clone()), None);
        // Footer: viewed, next unviewed, Approve and Send back.
        let hints: Vec<ReposButton> = wd.hints().iter().map(|h| h.button).collect();
        for b in [
            ReposButton::MarkViewed,
            ReposButton::NextUnviewed,
            ReposButton::Approve,
        ] {
            assert!(hints.contains(&b), "{b:?}");
        }
        // The viewed button toggles the file at the cursor.
        let toggled = wd.click(ReposHit::Button(ReposButton::MarkViewed), ENV);
        assert!(matches!(toggled, Some(Done::Diff(Outcome::Toggle { .. }))));
        // Alt+Shift+Down goes to it, not viewed; viewed, it says so.
        let mods = ModifiersState {
            alt: true,
            shift: true,
            ..ModifiersState::default()
        };
        let down = Keysym::Down;
        assert_eq!(wd.key(&mods, down, down, ENV), None);
        if let Some(fe) = wd.feature.as_mut() {
            fe.viewed.0.insert("api/a.rs".into(), "b-a.rs".into());
        }
        let all = Some(Done::Diff(Outcome::Notice(
            crate::worktree_diff::ALL_VIEWED,
        )));
        assert_eq!(wd.key(&mods, down, down, ENV), all);
        // Its finding is gone to from anywhere, and the title counts it.
        wd.diff.cursor = None;
        let right = Keysym::bracketright;
        assert_eq!(wd.key(&ModifiersState::default(), right, right, ENV), None);
        let view = wd.view(ENV);
        assert!(view.title.ends_with("1 finding, open"), "{}", view.title);
        assert!(view.rows.iter().any(|row| matches!(
            row,
            DetailRow::CommentHead { finding: Some(f), place, .. }
                if f.severity == "medium" && place == "r1-1 \u{b7} finding 1 of 1"
        )));
        let hint = wd
            .hints()
            .into_iter()
            .find(|h| h.button == ReposButton::NextComment);
        let hint = hint.expect("the footer goes to it too");
        assert_eq!(
            (hint.key.as_str(), hint.label.as_str()),
            ("] / [", "next, previous finding")
        );
        // The title's button goes to it too, from elsewhere.
        wd.diff.cursor = None;
        let next = ReposHit::Button(ReposButton::NextComment);
        assert_eq!(wd.click(next, ENV), None);
        let on = wd.diff.cursor.as_ref().map(|c| wd.diff.rows[c.row]);
        assert!(
            matches!(on, Some(crate::diff_view::DiffRow::CommentHead { .. })),
            "{on:?}"
        );
        // Its code isn't edited, but a yank takes what it shows.
        let refused = Some(Done::Diff(Outcome::Notice(
            crate::worktree_diff::FEATURE_READ_ONLY,
        )));
        let none = ModifiersState::default();
        wd.key(&none, Keysym::y, Keysym::y, ENV);
        assert!(matches!(
            wd.key(&none, Keysym::y, Keysym::y, ENV),
            Some(Done::Diff(Outcome::Yank(_)))
        ));
        assert_eq!(wd.click(ReposHit::DiffRestore(0), ENV), refused);
        assert!(
            wd.view(ENV)
                .row_buttons
                .iter()
                .all(|(_, h)| h.button != ReposButton::Restore)
        );
    }

    #[test]
    fn a_feature_s_findings_find_their_repository() {
        let fe = FeatureOf {
            repos: vec![repo("api", &["a.rs"])],
            findings: vec![Finding {
                file: "a.rs".into(),
                ..Finding::default()
            }],
            ..FeatureOf::default()
        };
        // One repository: a finding naming none is on it.
        let comments = fe.finding_comments(&HashMap::new());
        assert_eq!(comments[0].root, Path::new("/w/api"));
        assert_eq!(fe.holding(), 1);
    }

    #[test]
    fn a_linked_projects_repository_ticks_and_finds_by_its_key() {
        let mut ui = repo("admin:ui", &["a.rs"]);
        ui.label = "Admin: ui".into();
        let mut fe = FeatureOf {
            repos: vec![repo("ui", &["a.rs"]), ui],
            findings: vec![Finding {
                repo: "admin:ui".into(),
                file: "a.rs".into(),
                ..Finding::default()
            }],
            ..FeatureOf::default()
        };
        fe.viewed.0.insert("admin:ui/a.rs".into(), "b-a.rs".into());
        assert_eq!(fe.check(Path::new("/w/admin:ui"), "a.rs"), Check::On);
        assert_eq!(fe.check(Path::new("/w/ui"), "a.rs"), Check::Off);
        assert_eq!(fe.unviewed(), ["ui/a.rs"]);
        let labels: Vec<&str> = fe.sections().iter().map(|s| s.label).collect();
        assert_eq!(labels, ["ui", "Admin: ui"]);
        let comments = fe.finding_comments(&HashMap::new());
        assert_eq!(comments[0].root, Path::new("/w/admin:ui"));
    }

    #[test]
    fn a_fix_rounds_diff_only_reads_and_says_what_round_it_is() {
        let mut fe = FeatureOf {
            title: "Export".into(),
            stage: Some(Stage::ManualTest),
            repos: vec![RepoOf {
                name: "api".into(),
                root: "/w/api".into(),
                since: Since {
                    base: "a1".into(),
                    commits: 2,
                    changes: vec![crate::git::Change {
                        path: "a.rs".into(),
                        orig: None,
                        code: 'M',
                    }],
                },
                ..RepoOf::default()
            }],
            round: Some((FixPhase::Ready, "fix plan ready".into())),
            span: Some(SpanOf {
                n: 2,
                label: "Manual test 2".into(),
                state: "fixes made 12m ago".into(),
                ends: BTreeMap::from([("api".into(), ("a1".into(), Some("a3".into())))]),
            }),
            ..FeatureOf::default()
        };
        assert_eq!(fe.role(), Role::Span);
        assert_eq!(fe.heading(), "Manual test 2 of \u{201c}Export\u{201d}");
        assert_eq!(
            fe.summary(0),
            "fixes made 12m ago \u{b7} 2 commits \u{b7} 1 file"
        );
        assert_eq!(fe.locked(), Some(ROUND_ONLY_READ));
        assert!(fe.buttons(0).is_empty());
        assert_eq!(fe.tree_note(true), None);
        // Only Esc and F1 of its own: no ticks.
        assert_eq!(fe.keys(0).len(), 2);
        fe.span = None;
        assert_eq!(fe.role(), Role::Round(FixPhase::Ready));
    }
}

#[cfg(test)]
mod posts_tests {
    use super::*;
    use crate::chrome::ReposButton as B;

    fn finding(id: &str, state: FindingState) -> Finding {
        Finding {
            id: id.into(),
            repo: "api".into(),
            file: "a.rs".into(),
            title: format!("Finding {id}"),
            state,
            ..Finding::default()
        }
    }

    #[test]
    fn someone_elses_pull_request_sends_nothing_to_fix() {
        let fe = |stage, posts| FeatureOf {
            stage: Some(stage),
            ready: true,
            posts,
            findings: vec![
                finding("r1-1", FindingState::Fix),
                finding("r1-2", FindingState::Open),
            ],
            ..FeatureOf::default()
        };
        let buttons = |fe: &FeatureOf| fe.buttons(0).into_iter().map(|(b, _)| b).collect::<Vec<_>>();
        assert_eq!(buttons(&fe(Stage::AiReview, true)), vec![B::AddComment]);
        assert!(buttons(&fe(Stage::AiReview, false)).contains(&B::MakeFixes));
        assert_eq!(
            buttons(&fe(Stage::CodeReview, true)),
            vec![B::AddComment, B::Approve]
        );
        // One to comment waits for the review, not for AI review.
        assert_eq!(fe(Stage::AiReview, true).holding(), 1);
        assert_eq!(fe(Stage::AiReview, false).holding(), 2);
        assert!(fe(Stage::AiReview, true).decisions().starts_with("1 to comment"));
        let comments = fe(Stage::AiReview, true).finding_comments(&HashMap::new());
        assert!(comments.iter().all(|c| c.flag.as_ref().is_some_and(|f| f.pr)));
    }

    #[test]
    fn its_comments_wait_for_github_not_for_approve() {
        let gate = Gate {
            stage: Some(Stage::CodeReview),
            read: true,
            comments: 2,
            ..Gate::default()
        };
        assert!(approve_refusal(&gate).is_some_and(|why| why.contains("plan fixes")));
        let posts = Gate { posts: true, ..gate };
        assert_eq!(approve_refusal(&posts), None);
        let (text, ready) = next_step(&posts, 3, 0);
        assert!(ready);
        assert_eq!(text, "All 3 files viewed \u{b7} 2 comments for GitHub \u{b7} Approve to go on to Manual test");
    }
}
