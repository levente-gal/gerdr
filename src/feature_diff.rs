//! Code review of a feature: its diff over every repository it touched,
//! shown across the repositories panel as a worktree's diff is (see
//! `worktree_diff`, `WorktreeDiff::feature`), read-only.
//!
//! Each file's header has a checkbox: viewed, not viewed, or changed since
//! viewed (see `feature::viewed`); Space ticks it. AI review's findings
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
    collections::HashMap,
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
        store,
        viewed::{self, Tick, Viewed},
    },
    features::plural,
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

/// What Plan fixes waits for at AI review.
pub fn decide_first(undecided: usize) -> String {
    format!("Decide the {} left first", plural(undecided, "finding"))
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
    /// AI review's findings were read in.
    pub ready: bool,
    /// The open fix plan: its phase and how it stands, in words.
    pub round: Option<(FixPhase, String)>,
    /// Where the user's comments are kept (`COMMENTS`).
    pub comments: PathBuf,
    /// Opened from the feature panel, which Esc goes back to.
    pub back: bool,
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
            closed: feature.status != Status::Open,
            ready: feature.review.is_some() && run.is_some_and(|r| r.ready.is_some()),
            round: (feature.fix_round.as_ref()).map(|round| (round.phase, round.line())),
        }
    }
}

impl FeatureOf {
    /// What it is for now.
    pub fn role(&self) -> Role {
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
            Role::Review => vec![
                (B::AddComment, None),
                (B::PlanFixes, self.plan_refusal(own)),
                (B::Approve, None),
            ],
            Role::Round(FixPhase::Ready) => vec![(B::ReviewFixPlan, None)],
            _ => Vec::new(),
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
        let direct = !self.needs_plan(own);
        match direct && (!shift || self.count(FindingState::Open) > 0) {
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
            Role::Done => "Code review is done",
        }
    }

    /// The key of `button` as it stands with `own` comments of the user's:
    /// empty for one not of its own.
    pub fn key_of(&self, button: ReposButton, own: usize) -> &'static str {
        use ReposButton as B;
        match button {
            B::AddComment => "Shift+C",
            B::FixAllFindings => "Shift+F",
            B::PlanFixes if self.role() == Role::Review || !self.needs_plan(own) => {
                crate::worktree_diff::SEND_BACK_KEY
            }
            B::MakeFixes | B::PlanFixes | B::ReviewFixPlan | B::Approve => {
                crate::worktree_diff::APPROVE_KEY
            }
            _ => "",
        }
    }

    /// Its keys as it stands with `own` comments of the user's, before the
    /// diff's own (F1).
    pub fn keys(&self, own: usize) -> Vec<(&'static str, &'static str)> {
        let mut keys = vec![("Esc", "close the diff"), ("F1", "list the keys")];
        match self.role() {
            Role::Deciding => keys.extend([
                match self.needs_plan(own) {
                    true => (
                        "Ctrl+Enter",
                        "plan the fixes, once every finding is decided",
                    ),
                    false => (
                        "Ctrl+Enter",
                        "make the fixes, once every finding is decided",
                    ),
                },
                ("Ctrl+Shift+Enter", "plan the fixes first"),
                ("Shift+C", "add a comment on no line or file"),
                ("Shift+F", "mark every undecided finding to fix"),
                ("f", "fix the finding, or take that back"),
                ("c", "on a finding, write or edit your comment on it"),
                ("Shift+E", "on a finding, edit its fix"),
                ("Del", "dismiss the finding, saying why, or reopen it"),
            ]),
            Role::Review => keys.extend([
                ("Ctrl+Enter", "approve Code review"),
                ("Ctrl+Shift+Enter", "plan fixes for the comments"),
                ("Shift+C", "add a comment on no line or file"),
            ]),
            Role::Round(FixPhase::Ready) => keys.push(("Ctrl+Enter", "review the fix plan")),
            _ => {}
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
        let mut parts = vec![format!("{} to fix", self.count(FindingState::Fix))];
        match self.count(FindingState::Dismissed) {
            0 => {}
            n => parts.push(format!("{n} dismissed")),
        }
        parts.push(format!("{} to decide", self.count(FindingState::Open)));
        let made = self.findings.iter().filter(|f| f.fix_made()).count();
        match self.count(FindingState::Sent) - made {
            0 => {}
            n => parts.push(format!("{n} sent to Build")),
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
        let head = match self.role() {
            Role::Deciding => self.decisions(),
            _ => self.state.clone(),
        };
        match line(&self.counts(own)) {
            Some(line) => format!("{head} \u{b7} {line}"),
            None => head,
        }
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
        self.findings.iter().filter(|f| f.state.holds_up()).count()
    }

    /// The findings as comments, placed in the diff read (`diffs`).
    pub fn finding_comments(&self, diffs: &HashMap<PathBuf, (u64, Vec<FileDiff>)>) -> Vec<Comment> {
        let single = (self.repos.len() == 1).then(|| &self.repos[0]);
        self.findings
            .iter()
            // Those an older gerdr dropped: gone from here, on the page until
            // deleted.
            .filter(|f| !(f.yours && f.state == FindingState::Dismissed))
            .flat_map(|f| {
                let repo = (self.repos.iter().find(|r| r.name == f.repo)).or(single);
                let root = repo.map(|r| r.root.clone()).unwrap_or_default();
                let file = diffs
                    .get(&root)
                    .and_then(|(_, files)| files.iter().find(|d| d.path == f.file));
                let comment = finding_comment(f, root, file);
                let note = finding_note(f, &comment);
                std::iter::once(comment).chain(note)
            })
            .collect()
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
}

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

impl WorktreeDiff {
    /// Feature code review `fe`, in workspace `workspace`; the user's
    /// comments `review`. Shown once read.
    pub fn of_feature(fe: FeatureOf, root: PathBuf, workspace: usize, review: Review) -> Self {
        let mut wd = WorktreeDiff {
            root,
            label: fe.title.clone(),
            workspace,
            feature: Some(fe),
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
        let Some(fe) = self.feature.as_mut() else {
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
                    files.push((root, read.files));
                }
                Err(why) => repo.error = Some(format!("{}: {why}", repo.shown())),
            }
        }
        self.read = true;
        self.error = fe.repos.iter().find_map(|r| r.error.clone());
        self.with_diff(env, |diff, ctx| {
            diff.refresh(ctx);
            let reads = diff.reads;
            for (root, files) in files {
                diff.set_diffs(root, reads, files, ctx);
            }
        });
        self.place_findings(env);
    }

    /// The findings, as the record has them now, as comments after the
    /// user's own; the cursor on the one to show first, once.
    pub fn place_findings(&mut self, env: Env) {
        let Some(fe) = self.feature.as_mut() else {
            return;
        };
        let comments = fe.finding_comments(&self.diff.diffs);
        let focus = fe.focus.clone();
        let focus_note = fe.focus_note.clone();
        self.diff.notes_open = fe.role() == Role::Deciding;
        let review = &mut self.diff.review;
        review.comments.retain(|c| c.flag.is_none());
        let first = review.comments.len();
        review.comments.extend(comments);
        let at = focus.and_then(|id| {
            let found = review.comments[first..]
                .iter()
                .position(|c| c.flag.as_ref().is_some_and(|f| f.id == id));
            found.map(|i| first + i)
        });
        let at = at.or_else(|| {
            let note = focus_note.as_ref()?;
            review.comments[..first].iter().position(|c| c == note)
        });
        let sections = self
            .feature
            .as_ref()
            .map(FeatureOf::sections)
            .unwrap_or_default();
        self.diff.rebuild(&sections);
        if let Some(at) = at
            && self.with_diff(env, |diff, ctx| diff.show_comment(at, ctx))
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
            ready: standing.ready,
            round: standing.round,
            comments: comments.clone(),
            back: (self.features.panel.as_ref())
                .is_some_and(|p| p.project == project && p.slug == slug),
            repos,
            viewed: feature.viewed.clone(),
            findings: (feature.review.as_ref())
                .map(|r| r.findings.clone())
                .unwrap_or_default(),
            focus: finding.map(str::to_owned),
            focus_note: None,
        };
        let root = feature.root.clone();
        // A Zed opened from it goes to the feature's own workspace.
        let w = (self.feature_workspace(project, slug)).unwrap_or(self.active_workspace);
        let review = review::load_file(&comments).without_findings();
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
        let wd = WorktreeDiff::of_feature(fe, root, w, review);
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
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
        let repos: Vec<Repo> = feature.touched().cloned().collect();
        let (standing, viewed) = (Standing::of(feature), feature.viewed.clone());
        let findings = (feature.review.as_ref())
            .map(|r| r.findings.clone())
            .unwrap_or_default();
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
            let repos = repos
                .iter()
                .map(|r| (r.key(), r.worktree.clone(), viewed::read(r)))
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
        let open = self.repos.worktree.as_mut().filter(|wd| {
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
        let Some(fe) = &wd.feature else {
            return;
        };
        let counts = fe.counts(wd.diff.review.own());
        let key = (fe.project.clone(), fe.slug.clone());
        self.code_reviews.insert(key, counts);
    }

    /// Tick file `path` of the repository at `root` viewed, or untick it:
    /// kept in the feature's record.
    pub(crate) fn feature_toggle(&mut self, root: &Path, path: &str) {
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
        if let Some((project, slug)) = self.feature_open() {
            self.send_findings(&project, &slug);
        }
    }

    /// Make fixes in the open diff, at AI review: the findings to fix go to
    /// the Build agent, or it says what they wait for.
    pub(crate) fn feature_make_fixes(&mut self) {
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
        let gate = Gate {
            stage,
            draft: wd.diff.editing.is_some(),
            read: wd.read && !wd.reading,
            errors: fe.repos.iter().filter_map(|r| r.error.clone()).collect(),
            dirty: (fe.repos.iter())
                .filter(|r| !r.dirty.is_empty())
                .map(|r| r.shown().to_owned())
                .collect(),
            unviewed: fe.unviewed(),
            comments: wd.diff.review.own(),
            holding: fe.holding(),
        };
        if let Some(why) = approve_refusal(&gate) {
            return self.show_warning(why);
        }
        let (project, slug, back) = (fe.project.clone(), fe.slug.clone(), fe.back);
        self.read_and_tick(&project, &slug, Stage::CodeReview);
        self.close_feature_diff(back);
    }

    /// Send the user's comments (`text`, Markdown) to the Build agent:
    /// once sent, they are deleted, the review closed and the agent shown.
    pub(crate) fn feature_send_back(&mut self, text: String) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug, path) = (fe.project.clone(), fe.slug.clone(), fe.comments.clone());
        let stage = self.feature(&project, &slug).and_then(Feature::current);
        if let Some(why) = stage_refusal(stage) {
            return self.show_warning(why);
        }
        if !self.send_to_build(&project, &slug, "review comments", text) {
            return;
        }
        // What is kept to give back must be all that was sent.
        self.feature_save_review();
        if let Some(copy) = self.keep_comments(&project, &slug) {
            let back = crate::feature::fix_round::GiveBack::Comments { copy };
            self.fix_round_gives_back(&project, &slug, back);
        }
        if let Err(err) = review::save_file(&path, &Review::default()) {
            tracing::warn!("deleting {}: {err}", path.display());
            self.toast(format!("Couldn't delete the review: {err}"), Level::Error);
        }
        if let Some(wd) = self.wd_mut() {
            wd.diff.review.comments.retain(|c| c.flag.is_some());
            wd.diff.review_dirty = false;
        }
        self.code_review_counted();
        self.close_feature_diff(false);
        self.show_build_agent(&project, &slug);
    }

    /// Save the user's comments in the open code review, if they changed:
    /// the findings left out; a failure says so.
    pub(crate) fn feature_save_review(&mut self) {
        let Some(wd) = self
            .repos
            .worktree
            .as_mut()
            .filter(|wd| wd.diff.review_dirty && wd.feature.is_some())
        else {
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
        assert_eq!(fe.check(api, "a.rs"), Check::Partial);
        assert_eq!(fe.note(api, "a.rs").as_deref(), Some("not committed"));
        assert_eq!(line(&Counts::default()), None);
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
            Some(("r1-1".to_owned(), "c".to_owned()))
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
            DetailRow::CommentHead { finding: Some(_), place, .. } if place == "finding 1 of 1"
        )));
        let none = mods(false, false);
        let right = Keysym::bracketright;
        wd.key(&none, right, right, POPUP_ENV);
        let on = wd.diff.cursor.as_ref().map(|c| c.row);
        wd.key(&none, right, right, POPUP_ENV);
        assert_eq!(wd.diff.cursor.as_ref().map(|c| c.row), on);
        // On the finding, no second Comment: the one there is on its card.
        let labels = |wd: &WorktreeDiff| -> Vec<String> {
            let view = wd.view(POPUP_ENV);
            view.row_buttons
                .iter()
                .map(|(_, h)| h.label.clone())
                .collect()
        };
        assert!(!labels(&wd).iter().any(|l| l == "Comment"));
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
            Some(("r1-1".to_owned(), Decision::Fix))
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
            Some(("r1-1".to_owned(), String::new()))
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
            Some(("r1-1".to_owned(), "Careful".to_owned()))
        );
        wd.key(&none, c, c, POPUP_ENV);
        wd.click(ReposHit::Button(ReposButton::DeleteComment), POPUP_ENV);
        assert_eq!(
            wd.take_error().noted,
            Some(("r1-1".to_owned(), String::new()))
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
        wd.key(&mods(false, true), Keysym::E, Keysym::E, POPUP_ENV);
        assert_eq!(wd.take_error().edit_finding.as_deref(), Some("r1-1"));
    }

    const POPUP_ENV: Env = Env {
        sheet_rows: 10,
        page: 20,
        cols: 60,
        density: crate::chrome::Density::Comfortable,
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
        };
        wd.set_feature(vec![("api".into(), "/w/api".into(), Ok(read))], POPUP_ENV);
        wd
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
            title.filter(|b| !matches!(b, ReposButton::PrevComment | ReposButton::NextComment));
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
        assert_eq!(past.2, nothing);
        assert_eq!(at(None, true, None).0, Role::Done);
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
            for shift in [false, true] {
                assert_eq!(
                    wd.key(&mods(true, shift), enter, enter, POPUP_ENV),
                    Some(Done::PlanFixes)
                );
            }
            let keys = wd.feature.as_ref().unwrap().keys(wd.diff.review.own());
            assert!(keys.contains(&(
                "Ctrl+Enter",
                "plan the fixes, once every finding is decided"
            )));
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
        assert!(text.contains("### General comment\n"), "{text}");
        assert!(text.contains("The whole of it reads badly"), "{text}");
        assert!(!text.contains("the whole file"), "{text}");
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
                DetailRow::CommentHead { finding: None, place, .. } if place == "comment 1 of 1"
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
        assert_eq!(taken.delete_own.as_deref(), Some("u1"));
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
            ..FeatureOf::default()
        };
        let mut wd = WorktreeDiff::of_feature(fe, "/w".into(), 0, Review::default());
        let read = viewed::Read {
            since: api.since.clone(),
            files: vec![crate::doc_review::added_file("a.rs", "let x = 1;\n")],
            blobs: api.blobs.clone(),
            dirty: Default::default(),
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
                if f.severity == "medium" && place == "finding 1 of 1"
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
}
