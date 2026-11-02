//! Code review of a feature: its diff over every repository it touched,
//! shown across the repositories panel as a worktree's diff is (see
//! `worktree_diff`, `WorktreeDiff::feature`), read-only.
//!
//! Each file's header has a checkbox: viewed, not viewed, or changed since
//! viewed (see `feature::viewed`); Space ticks it. AI review's findings
//! show as comments of their own, on their line, else on their file's
//! header, else in the list of those not on a line, each with Fix and
//! Dismiss. The user's own comments are kept in the feature's data dir
//! (`code-review-comments.json`) until sent back to the Build agent.
//!
//! Approve, only at the Code review stage, wants every file viewed as it
//! is now, no comment left, nothing uncommitted and no finding holding up.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    Gerdr,
    chrome::Check,
    diff::{FileDiff, LineKind},
    diff_view::Section,
    feature::{
        Feature, Repo, Stage, Status,
        review_import::{Decision, Finding, FindingState},
        store,
        viewed::{self, Tick, Viewed},
    },
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
/// Why Send back does nothing.
pub const NO_OWN_COMMENT: &str = "Add a comment first: press c on a line";

/// The keys of a feature's code review, before the diff's own (F1).
pub const KEYS: [(&str, &str); 8] = [
    ("Esc", "close the code review"),
    ("F1", "list the keys"),
    ("Ctrl+Enter", "approve Code review"),
    (
        "Ctrl+Shift+Enter",
        "send the comments back to the Build agent",
    ),
    ("Space", "mark the file viewed, or not"),
    (
        "Alt+Shift+Down / Up",
        "go to the next or the last file not viewed",
    ),
    ("f", "fix the finding, or take that back"),
    ("Del", "dismiss the finding, saying why, or reopen it"),
];

/// A touched repository as its code review last read it.
#[derive(Clone, Debug, Default)]
pub struct RepoOf {
    /// `Repo::name`, its header's label and the viewed keys' first part.
    pub name: String,
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

impl FeatureOf {
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
                label: &r.name,
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
        match line(&self.counts(own)) {
            Some(line) => format!("{} \u{b7} {line}", self.state),
            None => self.state.clone(),
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
            .map(|f| {
                let repo = (self.repos.iter().find(|r| r.name == f.repo)).or(single);
                let root = repo.map(|r| r.root.clone()).unwrap_or_default();
                let file = diffs
                    .get(&root)
                    .and_then(|(_, files)| files.iter().find(|d| d.path == f.file));
                finding_comment(f, root, file)
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
    let tag = match f.state {
        FindingState::Open => String::new(),
        FindingState::Fix => " \u{b7} to fix".into(),
        FindingState::Sent => " \u{b7} sent to fix".into(),
        FindingState::Fixed => " \u{b7} fixed".into(),
        FindingState::Dismissed => format!(" \u{b7} dismissed: {}", f.reason),
    };
    let mut text = format!("{} \u{b7} {}{tag}", f.severity.label(), f.title);
    if let (Some(n), None) = (f.line, on) {
        text.push_str(&format!("\nline {n}"));
    }
    if !f.body.is_empty() {
        text.push('\n');
        text.push_str(&f.body);
    }
    if !f.fix.is_empty() {
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
        }),
    }
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
        1 => return Some("1 comment left: send it back or delete it".into()),
        n => return Some(format!("{n} comments left: send them back or delete them")),
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
    match feature.current() {
        Some(Stage::CodeReview) => "ready for review".into(),
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
                Err(why) => repo.error = Some(format!("{}: {why}", repo.name)),
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
        let repos = feature
            .touched()
            .map(|r| RepoOf {
                name: r.name.clone(),
                root: r.worktree.clone(),
                branch: Some(r.branch.clone().unwrap_or_else(|| branch.clone())),
                ..RepoOf::default()
            })
            .collect();
        let dir = store::dir(None, project, slug);
        let comments = dir.join(COMMENTS);
        let fe = FeatureOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            title: feature.title.clone(),
            state: state(feature),
            stage: feature.current(),
            comments: comments.clone(),
            back: (self.features.panel.as_ref())
                .is_some_and(|p| p.project == project && p.slug == slug),
            repos,
            viewed: feature.viewed.clone(),
            findings: (feature.review.as_ref())
                .map(|r| r.findings.clone())
                .unwrap_or_default(),
            focus: finding.map(str::to_owned),
        };
        let root = feature.root.clone();
        let review = review::load_file(&comments).without_findings();
        // It covers the other panels: they close, the repositories panel
        // kept as it was.
        self.switcher = None;
        self.project_menu = None;
        self.features.panel = None;
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
        if self.repos.worktree.is_some() {
            self.worktree_save_review();
            self.repos.close_worktree();
        }
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
        let wd = WorktreeDiff::of_feature(fe, root, self.active_workspace, review);
        self.repos.worktree = Some(wd);
        self.worktree_diff_read();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The record of `project`'s `slug` changed: its code review, if
    /// open, reads where its stage stands again.
    pub(crate) fn feature_diff_changed(&mut self, project: &str, slug: &str) {
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        if !(wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug) {
            return;
        }
        wd.again |= wd.reading;
        self.worktree_diff_read();
    }

    /// The feature code review open, if one is.
    fn feature_of(&self) -> Option<&FeatureOf> {
        self.repos.worktree.as_ref()?.feature.as_ref()
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
        let (state, stage, viewed) = (state(feature), feature.current(), feature.viewed.clone());
        let findings = (feature.review.as_ref())
            .map(|r| r.findings.clone())
            .unwrap_or_default();
        let Some(tx) = self.repos_sender() else {
            return;
        };
        let env = self.worktree_env();
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.reading = true;
            if let Some(fe) = wd.feature.as_mut() {
                let changed = fe.findings != findings;
                (fe.state, fe.stage, fe.viewed, fe.findings) = (state, stage, viewed, findings);
                if changed {
                    wd.place_findings(env);
                }
            }
        }
        std::thread::spawn(move || {
            let repos = repos
                .iter()
                .map(|r| (r.name.clone(), r.worktree.clone(), viewed::read(r)))
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
        let Some(wd) = self.repos.worktree.as_mut() else {
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
        let held = (self.repos.worktree.as_mut()).map(|wd| wd.diff.take_held());
        for (mods, sym, typed) in held.unwrap_or_default() {
            if let Some(outcome) = self.repos_key(&mods, sym, typed) {
                self.apply_repos(outcome);
            }
        }
    }

    /// Keep the open code review's counts for the feature panel.
    fn code_review_counted(&mut self) {
        let Some(wd) = self.repos.worktree.as_ref().filter(|wd| wd.read) else {
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
        if let Err(why) = feature.viewed.toggle(&key, blob.as_deref(), dirty) {
            return self.show_warning(why.into());
        }
        let viewed = feature.viewed.clone();
        self.save_feature(&project, &slug);
        if let Some(fe) = (self.repos.worktree.as_mut()).and_then(|wd| wd.feature.as_mut()) {
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
        self.decide_finding(&project, &slug, &id, decision);
        let findings = (self.feature(&project, &slug))
            .and_then(|f| f.review.as_ref())
            .map(|r| r.findings.clone())
            .unwrap_or_default();
        let env = self.worktree_env();
        if let Some(wd) = self.repos.worktree.as_mut()
            && let Some(fe) = wd.feature.as_mut()
        {
            fe.findings = findings;
            wd.place_findings(env);
        }
    }

    /// Approve Code review, unless something holds it up (said why).
    pub(crate) fn feature_approve(&mut self) {
        let Some(wd) = self.repos.worktree.as_ref() else {
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
                .map(|r| r.name.clone())
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
    /// once sent, they are deleted and the review closed.
    pub(crate) fn feature_send_back(&mut self, text: String) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug, back, path) = (
            fe.project.clone(),
            fe.slug.clone(),
            fe.back,
            fe.comments.clone(),
        );
        let stage = self.feature(&project, &slug).and_then(Feature::current);
        if let Some(why) = stage_refusal(stage) {
            return self.show_warning(why);
        }
        if !self.send_to_build(&project, &slug, "review comments", text) {
            return;
        }
        if let Err(err) = review::save_file(&path, &Review::default()) {
            tracing::warn!("deleting {}: {err}", path.display());
            self.toast(format!("Couldn't delete the review: {err}"), Level::Error);
        }
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.diff.review.comments.retain(|c| c.flag.is_some());
            wd.diff.review_dirty = false;
        }
        self.code_review_counted();
        self.close_feature_diff(back);
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

    /// Close the open code review: back to the feature panel when `back`.
    pub(crate) fn close_feature_diff(&mut self, back: bool) {
        let Some(fe) = self.feature_of() else {
            return;
        };
        let (project, slug) = (fe.project.clone(), fe.slug.clone());
        self.close_worktree_diff();
        if back {
            self.open_feature(&project, &slug);
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
        diff::{Hunk, Line},
        feature::review_import::Severity,
        git::Change,
    };

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
            Some("2 comments left: send them back or delete them")
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
        assert_eq!(c.text, "high \u{b7} Leaks\nIt leaks.\nFix: Close it.");
        assert_eq!(c.flag.as_ref().map(|f| f.id.as_str()), Some("r1-1"));
        // Its line not in the diff: on the file, saying the line.
        f.line = Some(9);
        f.state = FindingState::Dismissed;
        f.reason = "on purpose".into();
        let c = finding_comment(&f, "/w/api".into(), Some(&file));
        assert!(c.lines.is_empty());
        assert!(
            c.text
                .starts_with("high \u{b7} Leaks \u{b7} dismissed: on purpose\nline 9\n")
        );
    }

    #[test]
    fn its_view_is_read_only_with_viewed_ticks_and_finding_cards() {
        use crate::{
            chrome::{Density, ReposButton, ReposHit},
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
        // Its code isn't edited.
        let refused = Some(Done::Diff(Outcome::Notice(
            crate::worktree_diff::FEATURE_READ_ONLY,
        )));
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
}
