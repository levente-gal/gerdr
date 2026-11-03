//! Reviewing a stage's file (`brief.md`, `plan.md`) in the diff view: the
//! file shown as all added lines, so line comments work as on code, with
//! Approve and Send back on its header (see `feature_agents`).
//!
//! It shows across the repositories panel, as a worktree's diff does (see
//! `worktree_diff::DocOf`). The diff's vim edits change the file while its
//! agent isn't writing it (`locked`); a restore has nothing to put back.
//! Sending it back tells the agent when the user edited it since it was
//! written (`artifacts::edited_since_written`). The file is read again with every
//! Git status read, so an agent writing it again shows; the comments find
//! their lines again, or say they are outdated. They are kept in the
//! feature's data dir (`<stage>-comments.json`), so closing and opening
//! the review again keeps them, and deleted once the file is approved or
//! sent back.
//!
//! Verify's `evidence.md` shows in the same view, only read: no comments,
//! no Approve or Send back (`Gerdr::open_evidence`).

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{
    Gerdr,
    diff::{self, FileDiff, Hunk, Line, LineKind},
    feature::{Feature, Stage, Status, store, verify::EVIDENCE},
    features::Job,
    review::{self, Review},
    toast::Level,
    worktree_diff::{DocOf, WorktreeDiff},
};

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

/// Where the review comments on `stage`'s file are kept, in feature data
/// dir `dir`: `discover-comments.json`.
pub fn comments_path(dir: &Path, stage: Stage) -> PathBuf {
    let stage = stage.label().to_lowercase().replace(' ', "-");
    dir.join(format!("{stage}-comments.json"))
}

/// Where `stage` of `feature` stands, as the review's header says it:
/// lowercase, `ready for review`.
pub fn state(feature: Option<&Feature>, stage: Stage) -> String {
    let Some(feature) = feature else {
        return "feature gone".into();
    };
    if feature.marks.contains_key(&stage) {
        return "approved".into();
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
/// its agent is writing it, it's approved, the feature is closed. None:
/// it can.
pub fn locked(feature: Option<&Feature>, stage: Stage) -> Option<&'static str> {
    let Some(feature) = feature else {
        return Some("That feature is gone");
    };
    if feature.marks.contains_key(&stage) {
        return Some("An approved file can't be edited");
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

impl Gerdr {
    /// Show feature `slug`'s `evidence.md` (see `feature::verify`) in the
    /// same view, read-only, without Approve or Send back.
    pub fn open_evidence(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        let running = self.busy_with(project, slug, Job::Verify).is_some();
        let state = evidence_state(Some(feature), running);
        let dir = store::dir(None, project, slug);
        let path = dir.join(EVIDENCE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return self.show_warning("No evidence yet: Run checks first".into());
            }
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                return self.show_error(format!("Couldn't read {EVIDENCE}: {err}"));
            }
        };
        let doc = DocOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            stage: Stage::Verify,
            path,
            comments: comments_path(&dir, Stage::Verify),
            title,
            state,
            text: None,
            back: false,
            evidence: true,
            locked: None,
        };
        self.show_doc(doc, text, Review::default());
    }

    /// Show `stage`'s file of feature `slug` for review.
    pub fn open_doc_review(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(name) = crate::feature::artifacts::name(stage) else {
            let text = format!("{} has no file to review", stage.label());
            return self.show_warning(text);
        };
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        let state = state(Some(feature), stage);
        let locked = locked(Some(feature), stage);
        let dir = store::dir(None, project, slug);
        let path = dir.join(name);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                let text = format!("No {name} yet for \u{201c}{title}\u{201d}");
                return self.show_error(text);
            }
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                return self.show_error(format!("Couldn't read {name}: {err}"));
            }
        };
        let comments = comments_path(&dir, stage);
        let review = review::load_file(&comments);
        let doc = DocOf {
            project: project.to_owned(),
            slug: slug.to_owned(),
            stage,
            path,
            comments,
            title,
            state,
            text: None,
            back: false,
            evidence: false,
            locked,
        };
        self.show_doc(doc, text, review);
    }

    /// Show `doc`, read as `text`, its review comments `review`: back to
    /// the feature panel on close when that is open on its feature.
    fn show_doc(&mut self, mut doc: DocOf, text: String, review: Review) {
        doc.back = (self.features.panel.as_ref())
            .is_some_and(|p| p.project == doc.project && p.slug == doc.slug);
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
        if self.repos.worktree.is_some() {
            self.worktree_save_review();
            self.repos.close_worktree();
        }
        if let Some(panel) = self.repos.panel.take() {
            self.repos.kept = Some((self.project.clone(), panel));
        }
        let w = (self.feature_workspace(&doc.project, &doc.slug)).unwrap_or(self.active_workspace);
        let wd = WorktreeDiff::of_doc(doc, w, review);
        self.repos.worktree = Some(wd);
        let env = self.worktree_env();
        if let Some(wd) = self.repos.worktree.as_mut() {
            wd.set_doc(Ok(text), env);
        }
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The file under review, if one is open.
    fn doc_of(&self) -> Option<&DocOf> {
        self.repos.worktree.as_ref()?.doc.as_ref()
    }

    /// Read the file under review again, and where its stage stands: a
    /// new text is shown where it stood, its comments found again.
    pub(crate) fn doc_review_read(&mut self) {
        let Some(doc) = self.doc_of() else {
            return;
        };
        let read = read(&doc.path, &doc.name());
        let feature = self.feature(&doc.project, &doc.slug);
        let locked = locked(feature, doc.stage);
        let state = match doc.evidence {
            true => {
                let running = self
                    .busy_with(&doc.project, &doc.slug, Job::Verify)
                    .is_some();
                evidence_state(feature, running)
            }
            false => state(feature, doc.stage),
        };
        let env = self.worktree_env();
        let Some(wd) = self.repos.worktree.as_mut() else {
            return;
        };
        if let Some(doc) = wd.doc.as_mut() {
            doc.state = state;
            doc.locked = locked;
        }
        wd.set_doc(read, env);
    }

    /// Save the review comments on the file under review, if they
    /// changed; a failure says so.
    pub(crate) fn doc_save_review(&mut self) {
        let Some(wd) =
            (self.repos.worktree.as_mut()).filter(|wd| wd.diff.review_dirty && wd.doc.is_some())
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

    /// Close the file under review: back to the feature panel when it
    /// was opened from there.
    pub(crate) fn close_doc_review(&mut self) {
        let back = self.doc_of().filter(|d| d.back).cloned();
        self.close_worktree_diff();
        if let Some(doc) = back {
            self.open_feature(&doc.project, &doc.slug);
        }
    }

    /// Approve the file under review: asked first when comments on it
    /// would go unsent.
    pub(crate) fn doc_approve(&mut self) {
        let Some(wd) = self.repos.worktree.as_ref() else {
            return;
        };
        let Some(doc) = wd.doc.clone() else {
            return;
        };
        let count = wd.diff.review.comments.len();
        if count > 0 {
            let name = doc.name();
            return self.ask_approve_unsent(doc.project, doc.slug, doc.stage, &name, count);
        }
        self.approve_doc_now(&doc.project, &doc.slug, doc.stage);
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
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(doc.stage));
        if waiting {
            self.doc_done(&doc.project, &doc.slug, doc.stage);
        }
        self.send_back_stage(&doc.project, &doc.slug, doc.stage, comments);
    }

    /// `stage` of feature `slug` is approved or sent back: its comments
    /// deleted, and its review, if open, closed, back to the feature panel
    /// when it was opened from there.
    fn doc_done(&mut self, project: &str, slug: &str, stage: Stage) {
        let dir = store::dir(None, project, slug);
        self.doc_write_review(&comments_path(&dir, stage), &Review::default());
        let open = self
            .doc_of()
            .filter(|d| d.project == project && d.slug == slug && d.stage == stage)
            .map(|d| d.back);
        if let Some(back) = open {
            if let Some(wd) = self.repos.worktree.as_mut() {
                wd.diff.review_dirty = false;
            }
            self.close_worktree_diff();
            if back {
                self.open_feature(project, slug);
            }
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
        assert_eq!(state(None, Stage::Discover), "feature gone");
        assert_eq!(state(Some(&f), Stage::Discover), "ready for review");
        let run = StageRun {
            n: 1,
            ..StageRun::default()
        };
        f.runs.insert(Stage::Discover, run);
        assert_eq!(state(Some(&f), Stage::Discover), "agent still working");
        let run = f.runs.get_mut(&Stage::Discover).unwrap();
        run.ready = Some(5);
        run.sent_back = 2;
        assert_eq!(
            state(Some(&f), Stage::Discover),
            "ready for review · sent back 2 times"
        );
        let f = crate::features::tests::feature("login", Vec::new(), 1);
        assert_eq!(state(Some(&f), Stage::Discover), "approved");
    }

    #[test]
    fn the_file_is_edited_only_while_its_agent_isnt_writing_it() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 0);
        assert_eq!(locked(None, Stage::Discover), Some("That feature is gone"));
        assert_eq!(locked(Some(&f), Stage::Discover), None);
        let run = StageRun {
            n: 1,
            ..StageRun::default()
        };
        f.runs.insert(Stage::Discover, run);
        let writing = Some("The agent is still writing the file");
        assert_eq!(locked(Some(&f), Stage::Discover), writing);
        f.runs.get_mut(&Stage::Discover).unwrap().ready = Some(5);
        assert_eq!(locked(Some(&f), Stage::Discover), None);
        f.status = Status::Abandoned;
        assert_eq!(
            locked(Some(&f), Stage::Discover),
            Some("The feature is closed")
        );
        let f = crate::features::tests::feature("login", Vec::new(), 1);
        let approved = Some("An approved file can't be edited");
        assert_eq!(locked(Some(&f), Stage::Discover), approved);
    }

    #[test]
    fn the_evidence_header_says_how_the_run_went() {
        let mut f = crate::features::tests::feature("login", Vec::new(), 3);
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
