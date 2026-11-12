//! Reviewing a stage's file (`brief.md`, `plan.md`) in the diff view: the
//! file held as all added lines and shown as a plain page (the view's
//! `plain`), so line comments work as on code, with
//! Approve and Send back on its header (see `feature_agents`).
//!
//! It shows in the file's tab (see `doc_tab`), with the same diff a worktree
//! has (see `worktree_diff::DocOf`). The diff's vim edits change the file
//! while its agent isn't writing it (`locked`); a restore has nothing to put
//! back.
//! Sending it back tells the agent when the user edited it since it was
//! written (`artifacts::edited_since_written`). The file is read again with every
//! Git status read, so an agent writing it again shows; the comments find
//! their lines again, or say they are outdated. They are kept in the
//! feature's data dir (`<stage>-comments.json`), so closing its tab and
//! opening it again keeps them, and deleted once the file is approved or
//! sent back.
//!
//! Once its stage is done the file is only read: the header says so and
//! offers Reopen and going on to the current stage, or only going on once
//! the next stage has made something (`Feature::standing`, `stage_lock`).
//!
//! Verify's `evidence.md` shows in the same view, only read: no comments,
//! no Approve or Send back (`Gerdr::open_evidence`).

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    Gerdr,
    diff::{self, FileDiff, Hunk, Line, LineKind},
    feature::{
        Feature, Stage, Standing, Status, artifacts,
        fix_round::{self, FixPhase},
        page::Docs,
        store,
        verify::EVIDENCE,
    },
    features::{Job, lock_why},
    review::{self, Review},
    toast::Level,
    worktree_diff::{DocOf, EVIDENCE_READ_ONLY},
};

/// Which of a feature's files a review shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocFile {
    /// The file a stage's agent writes: `brief.md`, `plan.md`.
    Stage(Stage),
    /// Verify's `evidence.md`, only read: no comments, no Approve or Send
    /// back.
    Evidence,
    /// The fix round's plan, open or the last: reviewed as a stage's file
    /// is, and Make fixes where that has Approve.
    FixPlan,
    /// The final plan Plan review wrote, as changes to the plan: only read,
    /// as the evidence is.
    PlanChanges,
    /// Draft `k` of a stage's file, as changes to the draft before: only
    /// read, as the evidence is (see `feature::drafts`).
    DraftChanges(Stage, u32),
    /// Plan review's suggestions, flagged where they apply in Plan's
    /// `plan.md`: the plan is approved and only read, the suggestions are
    /// decided on it.
    Suggestions,
    /// Ship's plan of how to resolve conflicts with a base, of turn `.0`
    /// (`conflicts-<n>.md`): Approve resolves as planned, Send back has its
    /// agent write it again with the comments.
    ConflictPlan(u32),
}

impl DocFile {
    /// The stage it belongs to.
    pub fn stage(self) -> Stage {
        match self {
            DocFile::Stage(stage) => stage,
            DocFile::Evidence => Stage::Verify,
            DocFile::FixPlan => Stage::Build,
            DocFile::PlanChanges | DocFile::Suggestions => Stage::PlanReview,
            DocFile::ConflictPlan(_) => Stage::Ship,
            DocFile::DraftChanges(stage, _) => stage,
        }
    }

    /// Where its review comments are, under feature data dir `dir`.
    pub fn comments(self, dir: &Path) -> PathBuf {
        match self {
            DocFile::FixPlan => fix_round::comments_path(dir),
            // Its own: the final plan's review has Plan review's.
            DocFile::Suggestions => dir.join(SUGGESTIONS_COMMENTS),
            DocFile::ConflictPlan(n) => dir.join(format!("conflicts-{n}-comments.json")),
            _ => comments_path(dir, self.stage()),
        }
    }

    /// Its name, if it has a file at all.
    pub fn name(self) -> Option<&'static str> {
        match self {
            DocFile::Stage(stage) => artifacts::name(stage),
            DocFile::Evidence => Some(EVIDENCE),
            DocFile::FixPlan => Some(fix_round::FILE),
            DocFile::PlanChanges => artifacts::name(Stage::PlanReview),
            DocFile::DraftChanges(stage, _) => artifacts::name(stage),
            DocFile::Suggestions => artifacts::name(Stage::Plan),
            // Named by its turn: its path says it.
            DocFile::ConflictPlan(_) => None,
        }
    }

    /// Where it stands, as the review's header says it, `docs` the files
    /// last read; `running`: its checks run again.
    pub fn state(self, feature: Option<&Feature>, running: bool, docs: &Docs, now: u64) -> String {
        match self {
            DocFile::Stage(stage) => state(feature, stage, docs, now),
            DocFile::Evidence => evidence_state(feature, running),
            DocFile::FixPlan => fix_plan_state(feature, now),
            DocFile::DraftChanges(_, k) => format!("draft {k} against draft {}", k - 1),
            DocFile::PlanChanges => format!(
                "against {} \u{b7} {}",
                artifacts::name(Stage::Plan).unwrap_or("the plan"),
                state(feature, Stage::PlanReview, docs, now)
            ),
            DocFile::Suggestions => suggestions_state(feature),
            DocFile::ConflictPlan(n) => conflict_plan_state(feature, n),
        }
    }

    /// Why it can't be edited now. None: it can.
    pub fn locked(self, feature: Option<&Feature>, docs: &Docs) -> Option<&'static str> {
        if feature.is_some_and(|f| f.writer.is_some()) {
            return Some(WRITTEN_ELSEWHERE);
        }
        match self {
            DocFile::Stage(stage) => locked(feature, stage, docs),
            DocFile::Evidence => Some(EVIDENCE_READ_ONLY),
            DocFile::PlanChanges => Some(PLAN_CHANGES_READ_ONLY),
            DocFile::DraftChanges(..) => Some(DRAFT_CHANGES_READ_ONLY),
            DocFile::Suggestions => Some(SUGGESTIONS_READ_ONLY),
            DocFile::FixPlan => fix_plan_locked(feature),
            DocFile::ConflictPlan(n) => (conflict_plan_open(feature, n).is_none())
                .then_some(CONFLICT_PLAN_PAST),
        }
    }

    /// Whether it is closed for good and why. The evidence is only read
    /// however Verify stands, and has no actions to change.
    pub fn standing(self, feature: Option<&Feature>, docs: &Docs) -> Standing {
        // Written elsewhere, it is only read here, whichever stage it is at.
        if !(self == DocFile::Evidence || self.shows_changes())
            && let Some(feature) = feature.filter(|f| f.writer.is_some())
        {
            return Standing::Locked(feature.current().unwrap_or(Stage::Ship));
        }
        match (self, feature) {
            (DocFile::Stage(stage), Some(feature)) => {
                feature.standing(stage, |s| docs.written.contains(&s))
            }
            // Its suggestions are decided only while the page can decide
            // them; else it is a record.
            (DocFile::Suggestions, Some(feature))
                if !crate::review_cards::can_decide(feature, Stage::PlanReview) =>
            {
                match feature.standing(Stage::PlanReview, |s| docs.written.contains(&s)) {
                    Standing::Open => Standing::Locked(Stage::PlanReview),
                    standing => standing,
                }
            }
            _ => Standing::Open,
        }
    }

    /// Where it is, under feature data dir `dir`.
    pub fn path(self, dir: &Path) -> Option<PathBuf> {
        match self {
            DocFile::DraftChanges(stage, k) => crate::feature::drafts::path(dir, stage, k),
            DocFile::ConflictPlan(n) => Some(dir.join(format!("conflicts-{n}.md"))),
            _ => Some(dir.join(self.name()?)),
        }
    }

    /// Whether it shows a file as changes to another, only to read: the
    /// final plan's to the plan, a draft's to the one before.
    pub fn shows_changes(self) -> bool {
        matches!(self, DocFile::PlanChanges | DocFile::DraftChanges(..))
    }
}

/// Why a plan of conflicts no agent waits on any more takes no comment.
pub const CONFLICT_PLAN_PAST: &str = "This plan of the conflicts is past: only read";

/// The conflicts feature `feature` takes on while their plan of turn `n` is
/// written and waits for the user; none once that turn is past.
fn conflict_plan_open(feature: Option<&Feature>, n: u32) -> Option<&crate::feature::Resolving> {
    (feature?.shipping.resolving.as_ref()).filter(|r| r.n == n && r.planning)
}

/// How a plan of conflicts stands, as its review's header says it.
fn conflict_plan_state(feature: Option<&Feature>, n: u32) -> String {
    match conflict_plan_open(feature, n) {
        Some(r) => format!("{}: {} with {}", r.repo, r.files.join(", "), r.onto),
        None => "past".to_owned(),
    }
}

/// Where the comments of `DocFile::Suggestions` would be kept: it takes
/// none, as it is only read, but it has a file of its own.
const SUGGESTIONS_COMMENTS: &str = "suggestions-comments.json";

/// Why the plan shown with its suggestions takes no edit or comment.
pub const SUGGESTIONS_READ_ONLY: &str =
    "This shows the approved plan with the suggestions on it, only to read";

/// How Plan review's suggestions stand, as the header of
/// `DocFile::Suggestions` says it: "2 to decide · 1 accepted", or why they
/// are only to read.
fn suggestions_state(feature: Option<&Feature>) -> String {
    use crate::feature::plan_review::State;
    let Some(feature) = feature else {
        return "feature gone".into();
    };
    let Some(review) = feature.plan_review.as_ref() else {
        return "no suggestions yet".into();
    };
    if feature.is_done(Stage::PlanReview) {
        return "Plan review done \u{b7} only to read".into();
    }
    if review.writing {
        return "the final plan is being written from these".into();
    }
    if crate::plan_doc::frozen(Some(feature)).is_some() {
        return "only to read".into();
    }
    let open = review.open();
    let accepted = review.count(State::Accepted);
    let rejected = review.count(State::Rejected);
    let mut said = vec![format!("{open} to decide"), format!("{accepted} accepted")];
    if rejected > 0 {
        said.push(format!("{rejected} rejected"));
    }
    said.join(" \u{b7} ")
}

/// The user's comments in `review`, as a draft's card lists them: where each
/// is, by its first line quoted, and what it says.
pub fn drafts_comments(review: &Review) -> Vec<crate::feature::fix_round::Asked> {
    (review.comments.iter())
        .filter(|c| c.flag.is_none())
        .enumerate()
        .map(|(i, c)| crate::feature::fix_round::Asked {
            id: format!("C{}", i + 1),
            title: (c.lines.first())
                .map(|q| format!("line {}: {}", q.number, q.text.trim()))
                .unwrap_or_default(),
            note: c.text.clone(),
            shots: 0,
        })
        .collect()
}

/// Why a file can't be opened: what is said about it, and how loudly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failed {
    Warning(String),
    Error(String),
}

/// `text`, the file `name`, as a diff adding every line of it: one hunk,
/// lines numbered from 1, coloured as its language. An empty file has no
/// hunk.
pub fn added_file(name: &str, text: &str) -> FileDiff {
    let lines: Vec<Line> = text
        .lines()
        .enumerate()
        .map(|(i, raw)| {
            let text = diff::expand(raw);
            Line {
                kind: LineKind::Added,
                raw: (text != raw).then(|| raw.to_owned()),
                text,
                number: i + 1,
                old: 0,
                words: Vec::new(),
                syntax: Vec::new(),
            }
        })
        .collect();
    let added = lines.len();
    let hunks = match lines.is_empty() {
        true => Vec::new(),
        false => vec![Hunk {
            context: String::new(),
            lines,
        }],
    };
    let mut file = FileDiff {
        path: name.to_owned(),
        code: 'A',
        hunks,
        added,
        ..FileDiff::default()
    };
    diff::colour_file(&mut file);
    file
}

pub use crate::feature::artifacts::comments_path;

/// Why the final plan's changes take no edit or comment.
pub const PLAN_CHANGES_READ_ONLY: &str =
    "This shows what the final plan changed in the plan, only to read";

/// The plan as approved, under feature data dir `dir`: what the final plan's
/// changes are against.
pub fn read_base(dir: &Path) -> Result<String, Failed> {
    let Some(path) = artifacts::path(dir, Stage::Plan) else {
        return Err(Failed::Warning("Plan has no file".into()));
    };
    std::fs::read_to_string(&path).map_err(|err| {
        tracing::warn!("reading {}: {err}", path.display());
        Failed::Error(format!("Couldn't read the plan: {err}"))
    })
}

/// Why a draft's changes take no edit or comment.
pub const DRAFT_CHANGES_READ_ONLY: &str = "This shows what a draft changed, only to read";

/// Draft `k` of `stage`'s file, under feature data dir `dir`.
fn read_draft(dir: &Path, stage: Stage, k: u32) -> Result<String, Failed> {
    let Some(path) = crate::feature::drafts::path(dir, stage, k) else {
        return Err(Failed::Warning(format!("{} has no file", stage.label())));
    };
    std::fs::read_to_string(&path).map_err(|err| match err.kind() {
        ErrorKind::NotFound => Failed::Warning(format!("Draft {k} wasn't kept")),
        _ => {
            tracing::warn!("reading {}: {err}", path.display());
            Failed::Error(format!("Couldn't read draft {k}: {err}"))
        }
    })
}

/// The lines `new` has against `old`, the file `name`, as one diff: every line
/// of `new` in one hunk, those not in `old` added, those gone from it removed,
/// as the longest run of lines in common leaves them. Past `MAX_CELLS` of
/// work, `old` is all removed and `new` all added.
pub fn changed_file(name: &str, old: &str, new: &str) -> FileDiff {
    const MAX_CELLS: usize = 4_000_000;
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut ops: Vec<(char, &str)> = Vec::with_capacity(n + m);
    if n.saturating_mul(m) <= MAX_CELLS {
        // lcs[i][j]: the longest run in common of a[i..] and b[j..].
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[at(i, j)] = match a[i] == b[j] {
                    true => lcs[at(i + 1, j + 1)] + 1,
                    false => lcs[at(i + 1, j)].max(lcs[at(i, j + 1)]),
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[i] == b[j] {
                ops.push((' ', a[i]));
                (i, j) = (i + 1, j + 1);
            } else if lcs[at(i + 1, j)] >= lcs[at(i, j + 1)] {
                ops.push(('-', a[i]));
                i += 1;
            } else {
                ops.push(('+', b[j]));
                j += 1;
            }
        }
        ops.extend(a[i..].iter().map(|l| ('-', *l)));
        ops.extend(b[j..].iter().map(|l| ('+', *l)));
    } else {
        ops.extend(a.iter().map(|l| ('-', *l)));
        ops.extend(b.iter().map(|l| ('+', *l)));
    }
    let mut text = format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n");
    if !ops.is_empty() {
        text.push_str(&format!("@@ -1,{n} +1,{m} @@\n"));
        for (op, line) in ops {
            text.push(op);
            text.push_str(line);
            text.push('\n');
        }
    }
    let mut file = diff::parse(&text).pop().unwrap_or_default();
    file.path = name.to_owned();
    file.code = 'M';
    diff::colour_file(&mut file);
    file
}

/// Why an edit, a comment or Send back do nothing on an approved file.
pub const APPROVED_FILE: &str = "An approved file can't be edited: reopen it first";
/// Why those do nothing on a file of a feature another machine writes.
pub const WRITTEN_ELSEWHERE: &str = "This feature is written on another machine: take it over on its page to change it";
/// Why those, and Reopen, do nothing on a locked one.
pub const LOCKED_FILE: &str = "This file is locked: start over the stages after it first";

/// Where `stage` of `feature` stands, as the review's header says it:
/// lowercase, `ready for review`; `docs` is what its files were last read
/// as.
pub fn state(feature: Option<&Feature>, stage: Stage, docs: &Docs, now: u64) -> String {
    let Some(feature) = feature else {
        return "feature gone".into();
    };
    if let Some(mark) = feature.marks.get(&stage) {
        let word = match stage {
            Stage::Discover | Stage::Plan | Stage::PlanReview => "approved",
            _ => "done",
        };
        let mut state = match feature.locked_by(stage, |s| docs.written.contains(&s)) {
            Some(by) => format!("{word} \u{b7} locked: {}", lock_why(feature, by)),
            None => format!("{word} {} by {}", crate::clock::ago(now, mark.at), mark.by),
        };
        if docs.changed.contains(&stage) {
            let name = artifacts::name(stage).unwrap_or("its file");
            state.push_str(&format!(" \u{b7} {name} changed since"));
        }
        return state;
    }
    let Some(run) = feature.runs.get(&stage) else {
        return "ready for review".into();
    };
    let state = match run.ready {
        Some(_) => "ready for review",
        None => "agent still working",
    };
    match run.sent_back {
        0 => state.into(),
        1 => format!("{state} · sent back once"),
        n => format!("{state} · sent back {n} times"),
    }
}

/// Why `stage`'s file of `feature` can't be edited in its review now:
/// its agent is writing it, it's approved or locked, the feature is closed.
/// None: it can.
pub fn locked(feature: Option<&Feature>, stage: Stage, docs: &Docs) -> Option<&'static str> {
    let Some(feature) = feature else {
        return Some("That feature is gone");
    };
    if feature.is_done(stage) {
        return Some(
            match feature.locked_by(stage, |s| docs.written.contains(&s)) {
                Some(_) => LOCKED_FILE,
                None => APPROVED_FILE,
            },
        );
    }
    if feature.status != Status::Open {
        return Some("The feature is closed");
    }
    match feature.runs.get(&stage) {
        Some(run) if run.ready.is_none() => Some("The agent is still writing the file"),
        _ => None,
    }
}

/// Where the fix plan of `feature` stands at `now`, as its review's header
/// says it: which round's it is first.
pub fn fix_plan_state(feature: Option<&Feature>, now: u64) -> String {
    let Some(feature) = feature else {
        return "feature gone".into();
    };
    // Which round, and who asked it: the Build agent makes every round's
    // fixes, whichever stage's page it is followed on.
    let label = |round: &fix_round::FixRound| {
        let name = fix_round::label(round.n);
        match fix_round::asker(round.n, &feature.verify_fixes, &feature.fix_asked) {
            Some(stage) => format!("{name} \u{b7} asked by {}", stage.label()),
            None => name,
        }
    };
    let Some(round) = &feature.fix_round else {
        return match &feature.last_fix_round {
            Some(round) => {
                let mut said = vec![label(round), round.made()];
                let ago = |at| crate::clock::ago(now, at);
                said.extend(round.ended.map(|at| format!("finished {}", ago(at))));
                said.join(" \u{b7} ")
            }
            None => "no fix plan open".into(),
        };
    };
    let state = match round.phase {
        FixPhase::Fixing if round.stalled.is_some() => STOPPED,
        FixPhase::Planning if round.stalled.is_some() => "fix plan stalled",
        FixPhase::Ready => "ready for review",
        FixPhase::CompactingToFix => "agent compacting its chat before the fixes",
        FixPhase::Fixing => "agent making the fixes",
        FixPhase::Compacting | FixPhase::Planning => "agent still working",
    };
    let mut said = vec![label(round), state.to_owned()];
    match round.sent_back {
        0 => {}
        1 => said.push("sent back once".to_owned()),
        n => said.push(format!("sent back {n} times")),
    }
    if let Some(at) = round.ready.filter(|_| round.phase == FixPhase::Ready) {
        said.push(format!("written {}", crate::clock::ago(now, at)));
    }
    said.join(" \u{b7} ")
}

/// Why a fix plan can't be edited while its fixes are being made.
pub const FIXING_THEM: &str = "The agent is making these fixes";
/// Why the plan of a round that is over can't be edited.
pub const FIXES_MADE: &str =
    "These fixes are made: for more, add a comment or a finding where the feature is";
/// How a plan whose agent stopped with fixes left stands: Make fixes sends it
/// on, and it can't go back to be planned again.
pub const STOPPED: &str = "agent stopped with fixes left";

/// Why the fix plan of `feature` can't be edited in its review now.
fn fix_plan_locked(feature: Option<&Feature>) -> Option<&'static str> {
    let Some(feature) = feature else {
        return Some("That feature is gone");
    };
    match feature.fix_round.as_ref().map(|r| r.phase) {
        Some(FixPhase::Ready) => None,
        Some(FixPhase::Fixing)
            if feature
                .fix_round
                .as_ref()
                .is_some_and(|r| r.fixes_stopped()) =>
        {
            None
        }
        Some(FixPhase::Compacting | FixPhase::Planning) => {
            Some("The agent is still writing the file")
        }
        Some(FixPhase::CompactingToFix | FixPhase::Fixing) => Some(FIXING_THEM),
        None if feature.last_fix_round.is_some() => Some(FIXES_MADE),
        None => Some("No fix plan is open"),
    }
}

/// Where Verify of `feature` stands, as its evidence's header says it;
/// `running` while its checks run again.
pub fn evidence_state(feature: Option<&Feature>, running: bool) -> String {
    let Some(feature) = feature else {
        return "feature gone".into();
    };
    let Some(run) = &feature.verify else {
        return "no run yet".into();
    };
    if running {
        let shown = run.n.saturating_sub(1);
        return format!("checks running again \u{b7} this is run {shown}");
    }
    match run.passed {
        Some(true) => format!("run {} passed", run.n),
        Some(false) => format!("run {} failed", run.n),
        None => format!("run {}", run.n),
    }
}

/// The file `path` read, or why it couldn't be, as the review says it.
fn read(path: &Path, name: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| match err.kind() {
        ErrorKind::NotFound => format!("{name} is gone"),
        _ => {
            tracing::warn!("reading {}: {err}", path.display());
            format!("Couldn't read {name}: {err}")
        }
    })
}

/// How a feature's file stands, as its tab shows it (see `DocOf`).
struct DocStands {
    state: String,
    locked: Option<&'static str>,
    standing: Standing,
    current: Option<Stage>,
}

impl Gerdr {
    /// How `file` of feature `slug` stands now, by the latest probe of its
    /// files.
    fn doc_stands(&self, project: &str, slug: &str, file: DocFile) -> DocStands {
        let feature = self.feature(project, slug);
        let none = Docs::default();
        let docs = self.feature_docs(project, slug).unwrap_or(&none);
        let running = self.busy_with(project, slug, Job::Verify).is_some();
        DocStands {
            state: file.state(feature, running, docs, crate::feature::now()),
            locked: file.locked(feature, docs),
            standing: file.standing(feature, docs),
            current: feature.and_then(Feature::current),
        }
    }

    /// Bring what the shown file's tab says of its stage up to date: the
    /// next stage may have just locked it.
    pub(crate) fn doc_stand(&mut self) {
        let Some(doc) = self.doc_of() else {
            return;
        };
        let stands = self.doc_stands(&doc.project, &doc.slug, doc.file);
        if let Some(doc) = self.wd_mut().and_then(|wd| wd.doc.as_mut()) {
            doc.state = stands.state;
            doc.locked = stands.locked;
            doc.standing = stands.standing;
            doc.current = stands.current;
        }
    }

    /// `file` of feature `slug` read, as a review shows it.
    pub(crate) fn read_doc(
        &self,
        project: &str,
        slug: &str,
        file: DocFile,
    ) -> Result<(DocOf, String), Failed> {
        let dir = store::dir(None, project, slug);
        let Some(path) = file.path(&dir) else {
            let stage = file.stage().label();
            return Err(Failed::Warning(format!("{stage} has no file to review")));
        };
        let Some(feature) = self.feature(project, slug) else {
            return Err(Failed::Warning("That feature is gone".into()));
        };
        let title = feature.title.clone();
        let name = file.name().unwrap_or_default();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(match file {
                    DocFile::Evidence => {
                        Failed::Warning("No evidence yet: Run checks first".into())
                    }
                    DocFile::PlanChanges => {
                        Failed::Warning(format!("No final plan yet for \u{201c}{title}\u{201d}"))
                    }
                    DocFile::Suggestions => {
                        Failed::Warning(format!("No plan yet for \u{201c}{title}\u{201d}"))
                    }
                    DocFile::DraftChanges(_, k) => {
                        Failed::Warning(format!("Draft {k} of {name} wasn't kept"))
                    }
                    DocFile::ConflictPlan(_) => {
                        Failed::Warning("The plan isn't written yet".into())
                    }
                    DocFile::Stage(_) | DocFile::FixPlan => {
                        Failed::Error(format!("No {name} yet for \u{201c}{title}\u{201d}"))
                    }
                });
            }
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                return Err(Failed::Error(format!("Couldn't read {name}: {err}")));
            }
        };
        let stands = self.doc_stands(project, slug, file);
        let base = match file {
            DocFile::PlanChanges => Some(read_base(&dir)?),
            DocFile::DraftChanges(stage, k) => Some(read_draft(&dir, stage, k - 1)?),
            _ => None,
        };
        let doc = DocOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            file,
            path,
            comments: file.comments(&dir),
            title,
            state: stands.state,
            text: None,
            locked: stands.locked,
            standing: stands.standing,
            current: stands.current,
            base,
            // Laid on the plan once it is read (`place_suggestions`).
            suggestions: (file == DocFile::Suggestions)
                .then(|| crate::plan_doc::SuggestionsOf::of(Some(feature), None)),
        };
        Ok((doc, text))
    }

    /// Say why a file couldn't be opened.
    pub(crate) fn show_failed(&mut self, failed: Failed) {
        match failed {
            Failed::Warning(text) => self.show_warning(text),
            Failed::Error(text) => self.show_error(text),
        }
    }

    /// Show feature `slug`'s `evidence.md` (see `feature::verify`) in the
    /// same view, read-only, without Approve or Send back.
    pub fn open_evidence(&mut self, project: &str, slug: &str) {
        self.open_doc_tab(project, slug, DocFile::Evidence);
    }

    /// The file under review, if one is open.
    fn doc_of(&self) -> Option<&DocOf> {
        self.wd()?.doc.as_ref()
    }

    /// Read the file under review again, and where its stage stands: a
    /// new text is shown where it stood, its comments found again.
    pub(crate) fn doc_review_read(&mut self) {
        let Some(doc) = self.doc_of() else {
            return;
        };
        let read = read(&doc.path, &doc.name());
        // The plan the changes are against may have changed too.
        let base = (doc.file == DocFile::PlanChanges).then(|| {
            let dir = store::dir(None, &doc.project, &doc.slug);
            read_base(&dir).ok()
        });
        self.doc_stand();
        if let Some(Some(base)) = base
            && let Some(doc) = self.wd_mut().and_then(|wd| wd.doc.as_mut())
            && doc.base.as_ref() != Some(&base)
        {
            doc.base = Some(base);
            doc.text = None;
        }
        let env = self.worktree_env();
        let Some(wd) = self.wd_mut() else {
            return;
        };
        wd.set_doc(read, env);
        // Its preview shows an edit at once.
        self.refresh_previews();
    }

    /// Save the review comments on the file under review, if they
    /// changed; a failure says so.
    pub(crate) fn doc_save_review(&mut self) {
        let Some(wd) = self
            .wd_mut()
            .filter(|wd| wd.diff.review_dirty && wd.doc.is_some())
        else {
            return;
        };
        wd.diff.review_dirty = false;
        let path = wd.doc.as_ref().map(|d| d.comments.clone());
        let review = wd.diff.review.clone();
        if let Some(path) = path {
            self.doc_write_review(&path, &review);
        }
    }

    /// Write `review` to `path` (an empty one deletes it), saying so when
    /// that fails.
    fn doc_write_review(&mut self, path: &Path, review: &Review) {
        if let Err(err) = review::save_file(path, review) {
            tracing::warn!("saving {}: {err}", path.display());
            let what = match review.is_empty() {
                true => "delete",
                false => "save",
            };
            self.toast(format!("Couldn't {what} the review: {err}"), Level::Error);
        }
    }

    /// Approve the file under review: asked first when comments on it
    /// would go unsent.
    pub(crate) fn doc_approve(&mut self) {
        let Some(wd) = self.wd() else {
            return;
        };
        let Some(doc) = wd.doc.clone() else {
            return;
        };
        if doc.file == DocFile::FixPlan {
            let count = wd.diff.review.comments.len();
            if count > 0 {
                let back = !doc.state.contains(STOPPED);
                return self.ask_make_fixes_unsent(doc.project, doc.slug, count, back);
            }
            return self.make_fixes_now(&doc.project, &doc.slug);
        }
        // The plan is approved already: what its suggestions come to is the
        // final plan.
        if doc.file == DocFile::Suggestions {
            return self.write_final_plan(&doc.project, &doc.slug);
        }
        // A plan of conflicts approved: resolved as planned.
        if let DocFile::ConflictPlan(_) = doc.file {
            return self.approve_conflict_plan(&doc.project, &doc.slug, doc.file);
        }
        self.approve_doc(&doc.project, &doc.slug, doc.file.stage());
    }

    /// Approve `stage`'s file of feature `slug`, from its review or from
    /// the stage's page: asked first when comments on it would go unsent,
    /// refused while its tab has one being written.
    pub(crate) fn approve_doc(&mut self, project: &str, slug: &str, stage: Stage) {
        let file = DocFile::Stage(stage);
        let is_it = |d: &DocOf| d.project == project && d.slug == slug && d.file == file;
        let tab = (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .filter_map(|tab| tab.review())
            .map(|r| &r.wd)
            .find(|wd| wd.doc.as_ref().is_some_and(is_it));
        if tab.is_some_and(|wd| wd.diff.editing.is_some()) {
            return self.show_warning(crate::worktree_diff::DRAFT_OPEN.into());
        }
        let count = match tab {
            Some(wd) => wd.diff.review.comments.len(),
            None => {
                let dir = store::dir(None, project, slug);
                review::load_file(&file.comments(&dir)).comments.len()
            }
        };
        if count > 0 {
            let name = artifacts::name(stage).unwrap_or("its file");
            let (project, slug) = (project.to_owned(), slug.to_owned());
            return self.ask_approve_unsent(project, slug, stage, name, count);
        }
        self.approve_doc_now(project, slug, stage);
    }

    /// Approve `stage` of feature `slug`: the stage marked done, then its
    /// review comments deleted and its review closed if open. A refused
    /// approval keeps both.
    pub(crate) fn approve_doc_now(&mut self, project: &str, slug: &str, stage: Stage) {
        if self.approve_stage(project, slug, stage) {
            self.doc_done(project, slug, DocFile::Stage(stage));
        }
    }

    /// The fix plan of feature `slug` is approved: the fixes go, then its
    /// comments are deleted and its review closed; the agent shows only if
    /// it waits for its prompt (`show_build_agent`). A refusal keeps them all.
    pub(crate) fn make_fixes_now(&mut self, project: &str, slug: &str) {
        if let Some(why) = self.checks_refusal(project, slug) {
            return self.show_warning(why);
        }
        if self.make_fixes(project, slug) {
            self.doc_done(project, slug, DocFile::FixPlan);
            self.close_done_doc_tabs(project, slug, |file| file == DocFile::FixPlan);
            self.show_build_agent(project, slug);
        }
    }

    /// Send the file under review back to its agent with `comments`
    /// (Markdown): its comments deleted, the review closed.
    pub(crate) fn doc_send_back(&mut self, comments: String) {
        let Some(doc) = self.doc_of().cloned() else {
            return;
        };
        if doc.file == DocFile::FixPlan {
            return self.fix_plan_send_back(&doc.project, &doc.slug, comments);
        }
        // The suggestions' notes go back to their reviewer.
        if doc.file == DocFile::Suggestions {
            return self.ask_reviewer(&doc.project, &doc.slug);
        }
        if let DocFile::ConflictPlan(_) = doc.file {
            return self.send_back_conflict_plan(&doc.project, &doc.slug, doc.file, comments);
        }
        // What goes back is kept with the draft it is on, for its card.
        let sent: Vec<crate::feature::fix_round::Asked> = (self.wd())
            .map(|wd| drafts_comments(&wd.diff.review))
            .unwrap_or_default();
        // One no longer waiting is refused there: the comments stay.
        let waiting = self
            .feature(&doc.project, &doc.slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(doc.file.stage()));
        if waiting {
            self.doc_done(&doc.project, &doc.slug, doc.file);
        }
        self.send_back_stage(&doc.project, &doc.slug, doc.file.stage(), comments);
        let stage = doc.file.stage();
        if let Some(f) = self.feature_mut(&doc.project, &doc.slug)
            && let Some(draft) = crate::feature::drafts::latest(&mut f.drafts, stage)
                .filter(|d| d.sent_back.is_some() && d.comments.is_empty())
        {
            draft.comments = sent;
            self.save_feature(&doc.project, &doc.slug);
        }
    }

    /// `file` of feature `slug` is approved or sent back: its comments
    /// deleted, in the file and in its tab if it has one, and that tab, if
    /// it shows, gives way to the feature's. The tab stays, for the next
    /// look at the file.
    pub(crate) fn doc_done(&mut self, project: &str, slug: &str, file: DocFile) {
        let dir = store::dir(None, project, slug);
        self.doc_write_review(&file.comments(&dir), &Review::default());
        let is_it = |d: &DocOf| d.project == project && d.slug == slug && d.file == file;
        let tabs = self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs);
        for wd in tabs.filter_map(|tab| tab.review_mut()).map(|r| &mut r.wd) {
            if wd.doc.as_ref().is_some_and(is_it) {
                wd.diff.review = Review::default();
                wd.diff.review_dirty = false;
            }
        }
        if self.doc_of().is_some_and(is_it) {
            self.doc_review_read();
            self.show_feature_tab(project, slug);
        }
    }

    /// Read `file` of feature `slug` again if its tab shows.
    pub(crate) fn doc_reread_shown(&mut self, project: &str, slug: &str, file: DocFile) {
        let is_it = |d: &DocOf| d.project == project && d.slug == slug && d.file == file;
        if self.doc_of().is_some_and(is_it) {
            self.doc_review_read();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::agents::StageRun;

    #[test]
    fn the_suggestions_show_on_the_plan_only_read_decided_while_they_can_be() {
        use crate::feature::plan_review::{PlanReview, State, Suggestion};
        let file = DocFile::Suggestions;
        let docs = Docs::default();
        assert_eq!(file.stage(), Stage::PlanReview);
        assert_eq!(file.name(), Some("plan.md"));
        let dir = Path::new("/d");
        assert_eq!(file.path(dir), Some(PathBuf::from("/d/plan.md")));
        assert_eq!(file.comments(dir), dir.join(SUGGESTIONS_COMMENTS));
        let mut f = crate::features::tests::feature("login", Vec::new(), 2);
        // The plan is approved: never edited here.
        assert_eq!(file.locked(Some(&f), &docs), Some(SUGGESTIONS_READ_ONLY));
        assert_eq!(file.state(Some(&f), false, &docs, 0), "no suggestions yet");
        let one = |id: &str, state| Suggestion {
            id: id.into(),
            state,
            ..Suggestion::default()
        };
        f.plan_review = Some(PlanReview {
            n: 1,
            stamp: Some((1, 1)),
            suggestions: vec![
                one("r1-1", State::Open),
                one("r1-2", State::Accepted),
                one("r1-3", State::Rejected),
            ],
            ..PlanReview::default()
        });
        // Open, and said how they stand, only while the page decides them.
        let open = crate::review_cards::can_decide(&f, Stage::PlanReview);
        assert_eq!(file.standing(Some(&f), &docs) == Standing::Open, open);
        let state = file.state(Some(&f), false, &docs, 0);
        match open {
            true => assert_eq!(state, "1 to decide \u{b7} 1 accepted \u{b7} 1 rejected"),
            false => assert_eq!(state, "only to read"),
        }
        f.plan_review.as_mut().unwrap().writing = true;
        assert_eq!(
            file.standing(Some(&f), &docs),
            Standing::Locked(Stage::PlanReview)
        );
        assert_eq!(
            file.state(Some(&f), false, &docs, 0),
            "the final plan is being written from these"
        );
        let mut done = crate::features::tests::feature("login", Vec::new(), 3);
        done.plan_review = f.plan_review.clone();
        assert_ne!(file.standing(Some(&done), &docs), Standing::Open);
        assert_eq!(
            file.state(Some(&done), false, &docs, 0),
            "Plan review done \u{b7} only to read"
        );
    }

    #[test]
    fn the_last_fix_plan_is_only_read() {
        let mut f = crate::feature::tests::feature("a", 1);
        assert_eq!(fix_plan_state(Some(&f), 100), "no fix plan open");
        assert_eq!(fix_plan_locked(Some(&f)), Some("No fix plan is open"));
        f.last_fix_round = Some(crate::feature::fix_round::FixRound {
            n: 1,
            what: "1 finding".into(),
            input: "/d/a.md".into(),
            stage: Stage::Verify,
            phase: FixPhase::Fixing,
            sent_back: 0,
            asked: 1,
            ready: None,
            stalled: None,
            give_back: Default::default(),
            items: Vec::new(),
            done: 0,
            ended: None,
            trail: Default::default(),
        });
        assert_eq!(
            fix_plan_state(Some(&f), 100),
            "Fix round 1 \u{b7} fixes made"
        );
        f.last_fix_round.as_mut().unwrap().items = vec!["One".into(), "Two".into()];
        f.last_fix_round.as_mut().unwrap().done = 2;
        f.last_fix_round.as_mut().unwrap().ended = Some(880);
        assert_eq!(
            fix_plan_state(Some(&f), 1_000),
            "Fix round 1 \u{b7} 2 of 2 made \u{b7} finished 2m ago"
        );
        assert_eq!(fix_plan_locked(Some(&f)), Some(FIXES_MADE));
    }

    #[test]
    fn a_file_shows_as_one_hunk_of_added_lines() {
        let file = added_file("brief.md", "# Brief\n\n\tTabbed\n");
        assert_eq!(
            (file.path.as_str(), file.code, file.added),
            ("brief.md", 'A', 3)
        );
        assert_eq!(file.hunks.len(), 1);
        let lines = &file.hunks[0].lines;
        assert!(
            lines
                .iter()
                .all(|l| l.kind == LineKind::Added && l.old == 0)
        );
        let numbers: Vec<usize> = lines.iter().map(|l| l.number).collect();
        assert_eq!(numbers, [1, 2, 3]);
        assert_eq!(lines[2].text, "    Tabbed");
        assert_eq!(lines[2].raw(), "\tTabbed");
        assert_eq!(lines[0].raw, None);
        // Nothing in it: no hunk.
        let empty = added_file("plan.md", "");
        assert!(empty.hunks.is_empty() && empty.added == 0);
    }

    #[test]
    fn comments_are_kept_per_stage_in_the_feature_dir() {
        let dir = Path::new("/data/features/p/login");
        assert_eq!(
            comments_path(dir, Stage::Discover),
            dir.join("discover-comments.json")
        );
        assert_eq!(
            comments_path(dir, Stage::Plan),
            dir.join("plan-comments.json")
        );
    }

    #[test]
    fn the_header_says_where_the_stage_stands() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 0);
        let docs = Docs::default();
        let state = |f: Option<&Feature>, docs: &Docs| state(f, Stage::Discover, docs, 0);
        assert_eq!(state(None, &docs), "feature gone");
        assert_eq!(state(Some(&f), &docs), "ready for review");
        let run = StageRun {
            n: 1,
            ..StageRun::default()
        };
        f.runs.insert(Stage::Discover, run);
        assert_eq!(state(Some(&f), &docs), "agent still working");
        let run = f.runs.get_mut(&Stage::Discover).unwrap();
        run.ready = Some(5);
        run.sent_back = 2;
        assert_eq!(
            state(Some(&f), &docs),
            "ready for review · sent back 2 times"
        );
    }

    #[test]
    fn an_approved_file_says_so_and_what_holds_it() {
        let f = crate::features::tests::feature("login", Vec::new(), 1);
        let by = &f.marks[&Stage::Discover].by;
        let state = |docs: &Docs| state(Some(&f), Stage::Discover, docs, 0);
        let mut docs = Docs::default();
        assert!(state(&docs).starts_with("approved "));
        assert!(state(&docs).ends_with(&format!(" by {by}")));
        assert_eq!(
            DocFile::Stage(Stage::Discover).standing(Some(&f), &docs),
            Standing::Approved
        );
        assert_eq!(
            locked(Some(&f), Stage::Discover, &docs),
            Some(APPROVED_FILE)
        );
        docs.changed.insert(Stage::Discover);
        assert!(state(&docs).ends_with("\u{b7} brief.md changed since"));
        docs.changed.clear();
        docs.written.insert(Stage::Plan);
        assert_eq!(state(&docs), "approved \u{b7} locked: Plan has plan.md");
        assert_eq!(
            DocFile::Stage(Stage::Discover).standing(Some(&f), &docs),
            Standing::Locked(Stage::Plan)
        );
        assert_eq!(locked(Some(&f), Stage::Discover, &docs), Some(LOCKED_FILE));
        // The evidence has no actions to close.
        assert_eq!(DocFile::Evidence.standing(Some(&f), &docs), Standing::Open);
        // What another machine writes is only read here, at any stage.
        let mut seen = f.clone();
        seen.writer = Some("m1".into());
        let file = DocFile::Stage(Stage::Discover);
        assert!(matches!(file.standing(Some(&seen), &docs), Standing::Locked(_)));
        assert_eq!(file.locked(Some(&seen), &docs), Some(WRITTEN_ELSEWHERE));
    }

    #[test]
    fn the_file_is_edited_only_while_its_agent_isnt_writing_it() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 0);
        let docs = Docs::default();
        let locked = |f: Option<&Feature>| locked(f, Stage::Discover, &docs);
        assert_eq!(locked(None), Some("That feature is gone"));
        assert_eq!(locked(Some(&f)), None);
        let run = StageRun {
            n: 1,
            ..StageRun::default()
        };
        f.runs.insert(Stage::Discover, run);
        let writing = Some("The agent is still writing the file");
        assert_eq!(locked(Some(&f)), writing);
        f.runs.get_mut(&Stage::Discover).unwrap().ready = Some(5);
        assert_eq!(locked(Some(&f)), None);
        f.status = Status::Abandoned;
        assert_eq!(locked(Some(&f)), Some("The feature is closed"));
    }

    #[test]
    fn the_evidence_header_says_how_the_run_went() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 4);
        assert_eq!(evidence_state(None, false), "feature gone");
        assert_eq!(evidence_state(Some(&f), false), "no run yet");
        f.verify = Some(crate::feature::verify::VerifyRun {
            n: 2,
            passed: Some(false),
            ..Default::default()
        });
        assert_eq!(evidence_state(Some(&f), false), "run 2 failed");
        assert_eq!(
            evidence_state(Some(&f), true),
            "checks running again \u{b7} this is run 1"
        );
        f.verify.as_mut().unwrap().passed = Some(true);
        assert_eq!(evidence_state(Some(&f), false), "run 2 passed");
    }

    #[test]
    fn the_final_plans_changes_show_against_the_plan() {
        let old = "# Plan\nKeep\nGone\nAlso kept\n";
        let new = "# Plan\nKeep\nNew line\nAlso kept\nAt the end\n";
        let file = changed_file("final-plan.md", old, new);
        let kinds: Vec<(LineKind, &str)> = (file.hunks.iter())
            .flat_map(|h| &h.lines)
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (LineKind::Context, "# Plan"),
                (LineKind::Context, "Keep"),
                (LineKind::Removed, "Gone"),
                (LineKind::Added, "New line"),
                (LineKind::Context, "Also kept"),
                (LineKind::Added, "At the end"),
            ]
        );
        assert_eq!((file.added, file.removed), (2, 1));
        assert_eq!(file.path, "final-plan.md");
        // The same text has no change to show.
        let same = changed_file("final-plan.md", old, old);
        assert_eq!((same.added, same.removed), (0, 0));
    }

    #[test]
    fn a_drafts_changes_are_against_the_draft_before_only_to_read() {
        let file = DocFile::DraftChanges(Stage::Plan, 3);
        let dir = Path::new("/d");
        assert_eq!(file.path(dir), Some(PathBuf::from("/d/drafts/plan-3.md")));
        assert_eq!(file.name(), Some("plan.md"));
        assert_eq!(file.stage(), Stage::Plan);
        assert!(file.shows_changes() && DocFile::PlanChanges.shows_changes());
        assert!(!DocFile::Stage(Stage::Plan).shows_changes());
        assert_eq!(
            file.locked(None, &Docs::default()),
            Some(DRAFT_CHANGES_READ_ONLY)
        );
        assert_eq!(
            file.state(None, false, &Docs::default(), 0),
            "draft 3 against draft 2"
        );
        // What goes back is listed by where it is and what it says.
        let review = Review {
            comments: vec![review::Comment {
                lines: vec![review::Quoted {
                    number: 4,
                    text: "  Run nightly ".into(),
                    ..Default::default()
                }],
                text: "Hourly?".into(),
                ..Default::default()
            }],
            ..Review::default()
        };
        let sent = drafts_comments(&review);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (sent[0].id.as_str(), sent[0].title.as_str()),
            ("C1", "line 4: Run nightly")
        );
        assert_eq!(sent[0].note, "Hourly?");
    }
}
