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
    feature::{Feature, Stage, Standing, Status, artifacts, page::Docs, store, verify::EVIDENCE},
    features::{Job, lock_why},
    projects,
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
}

impl DocFile {
    /// The stage it belongs to.
    pub fn stage(self) -> Stage {
        match self {
            DocFile::Stage(stage) => stage,
            DocFile::Evidence => Stage::Verify,
        }
    }

    /// Its name, if it has a file at all.
    pub fn name(self) -> Option<&'static str> {
        match self {
            DocFile::Stage(stage) => artifacts::name(stage),
            DocFile::Evidence => Some(EVIDENCE),
        }
    }

    /// Where it stands, as the review's header says it, `docs` the files
    /// last read; `running`: its checks run again.
    pub fn state(self, feature: Option<&Feature>, running: bool, docs: &Docs, now: u64) -> String {
        match self {
            DocFile::Stage(stage) => state(feature, stage, docs, now),
            DocFile::Evidence => evidence_state(feature, running),
        }
    }

    /// Why it can't be edited now. None: it can.
    pub fn locked(self, feature: Option<&Feature>, docs: &Docs) -> Option<&'static str> {
        match self {
            DocFile::Stage(stage) => locked(feature, stage, docs),
            DocFile::Evidence => Some(EVIDENCE_READ_ONLY),
        }
    }

    /// Whether it is closed for good and why. The evidence is only read
    /// however Verify stands, and has no actions to change.
    pub fn standing(self, feature: Option<&Feature>, docs: &Docs) -> Standing {
        match (self, feature) {
            (DocFile::Stage(stage), Some(feature)) => {
                feature.standing(stage, |s| docs.written.contains(&s))
            }
            _ => Standing::Open,
        }
    }

    /// Where it is, under feature data dir `dir`.
    pub fn path(self, dir: &Path) -> Option<PathBuf> {
        Some(dir.join(self.name()?))
    }
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

/// Why an edit, a comment or Send back do nothing on an approved file.
pub const APPROVED_FILE: &str = "An approved file can't be edited: reopen it first";
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
            None => format!("{word} {} by {}", projects::ago(now, mark.at), mark.by),
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
                    DocFile::Stage(_) => {
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
        let doc = DocOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            file,
            path,
            comments: comments_path(&dir, file.stage()),
            title,
            state: stands.state,
            text: None,
            locked: stands.locked,
            standing: stands.standing,
            current: stands.current,
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
        self.doc_stand();
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
        let count = wd.diff.review.comments.len();
        if count > 0 {
            let name = doc.name();
            let stage = doc.file.stage();
            return self.ask_approve_unsent(doc.project, doc.slug, stage, &name, count);
        }
        self.approve_doc_now(&doc.project, &doc.slug, doc.file.stage());
    }

    /// Approve `stage` of feature `slug`: the stage marked done, then its
    /// review comments deleted and its review closed if open. A refused
    /// approval keeps both.
    pub(crate) fn approve_doc_now(&mut self, project: &str, slug: &str, stage: Stage) {
        if self.approve_stage(project, slug, stage) {
            self.doc_done(project, slug, stage);
        }
    }

    /// Send the file under review back to its agent with `comments`
    /// (Markdown): its comments deleted, the review closed.
    pub(crate) fn doc_send_back(&mut self, comments: String) {
        let Some(doc) = self.doc_of().cloned() else {
            return;
        };
        // One no longer waiting is refused there: the comments stay.
        let waiting = self
            .feature(&doc.project, &doc.slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(doc.file.stage()));
        if waiting {
            self.doc_done(&doc.project, &doc.slug, doc.file.stage());
        }
        self.send_back_stage(&doc.project, &doc.slug, doc.file.stage(), comments);
    }

    /// `stage` of feature `slug` is approved or sent back: its comments
    /// deleted, in the file and in its tab if it has one, and that tab, if
    /// it shows, gives way to the feature's. The tab stays, for the next
    /// look at the file.
    fn doc_done(&mut self, project: &str, slug: &str, stage: Stage) {
        let dir = store::dir(None, project, slug);
        self.doc_write_review(&comments_path(&dir, stage), &Review::default());
        let file = DocFile::Stage(stage);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::agents::StageRun;

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
}
