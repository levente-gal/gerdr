//! Plan review, gerdr's side: reading the reviewer's suggestions in, the
//! decisions on them, and the final plan (see `feature::plan_review`).
//!
//! The stage has two phases and one run at a time. While reviewing, its run
//! is the reviewer's; it is never `ready`, so the watch goes on looking at
//! its file and a rewrite is read in whenever it comes. What waits for the
//! user then is the review in the record. Once the decisions are made, the
//! reviewer's run is parked in the record and a fresh agent writes the final
//! plan: that goes as Plan's file does, up for review, approve or send back.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    Gerdr,
    confirm::Then,
    feature::{
        self, Feature, Stage, Status,
        artifacts::{self},
        patch::{self, Reading},
        plan_review::{self, Decision, PlanReview, Raw},
        store,
    },
    feature_agents::Stamp,
    features::{Done, plural},
    prompts,
};

const STAGE: Stage = Stage::PlanReview;

/// What was read off the thread once the stage's agent rested.
#[derive(Debug)]
pub struct PlanRead {
    pub reading: Reading,
    /// The suggestions, their citations checked, while reviewing.
    pub raws: Option<Result<Vec<Raw>, String>>,
}

/// The repositories a citation may name: those of the feature, and those
/// it keeps as context, by key and folder.
fn citable(feature: &Feature) -> Vec<(String, PathBuf)> {
    let repos = (feature.repos.iter()).map(|r| (r.key(), r.worktree.clone()));
    let context = (feature.context.iter()).map(|c| (c.key(), c.main.clone()));
    repos.chain(context).collect()
}

/// Read what the code is and, while reviewing, the suggestions in `path`.
/// Reads git and files: not for the UI thread.
fn read_plan(
    feature: &Feature,
    path: &Path,
    repos: &[(String, PathBuf)],
    writing: bool,
) -> Result<PlanRead, String> {
    let reading = patch::read(feature)?;
    let raws = (!writing).then(|| {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", plan_review::FILE))?;
        let mut raws = plan_review::parse(&text).map_err(|lacks| lacks.join("; "))?;
        plan_review::check_citations(&mut raws, repos);
        Ok(raws)
    });
    Ok(PlanRead { reading, raws })
}

/// What `reading` shows the agent changed: uncommitted work anywhere, or
/// commits since the review started (`baseline`; none: it couldn't be read
/// then, and commits can't be told apart from older ones).
fn changed_code(reading: &Reading, baseline: Option<&str>) -> Vec<String> {
    let mut changed: Vec<String> = reading.dirty.clone();
    changed.extend(reading.stray.iter().cloned());
    if baseline.is_some_and(|b| b != reading.patch) {
        let commits = reading.commits.iter().filter(|(_, n)| **n > 0);
        changed.extend(commits.map(|(repo, _)| repo.clone()));
    }
    changed.sort();
    changed.dedup();
    changed
}

/// Whether `feature` has suggestions waiting to be decided, or decided and
/// waiting for the final plan.
pub(crate) fn deciding(feature: &Feature) -> Option<&PlanReview> {
    (feature.plan_review.as_ref()).filter(|r| r.stamp.is_some() && !r.writing)
}

impl Gerdr {
    /// Stands in until the sheet is built.
    pub(crate) fn open_plan_sheet(&mut self, _project: &str, _slug: &str) {}

    /// A plan review's agent is starting: remember what the code is now, so
    /// that what it changes shows. Its prompt isn't sent yet, so nothing it
    /// does can be taken for the code it started from.
    pub(crate) fn plan_review_starts(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let review = feature.plan_review.get_or_insert_with(PlanReview::default);
        if review.writing || review.baseline.is_some() {
            return;
        }
        let feature = feature.clone();
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::PlanReviewBaseline {
            result: patch::read(&feature).map(|r| r.patch),
            project,
            slug,
        });
    }

    /// The code a plan review started from was read.
    pub(crate) fn plan_review_baseline(
        &mut self,
        project: &str,
        slug: &str,
        result: Result<String, String>,
    ) {
        let patch = match result {
            Ok(patch) => patch,
            Err(why) => {
                // Only less to go on: uncommitted changes still show.
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_warning(format!(
                    "Couldn't read the code before the review, so commits it makes won't show: {why}"
                ));
            }
        };
        let Some(review) = self
            .feature_mut(project, slug)
            .and_then(|f| f.plan_review.as_mut())
        else {
            return;
        };
        review.baseline = Some(patch);
        self.save_feature(project, slug);
    }

    /// Read the code of feature `slug` and, while reviewing, its suggestions
    /// off the thread, for run `n` whose file has `stamp`.
    pub(crate) fn check_plan_review_code(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let writing = feature.plan_review.as_ref().is_some_and(|r| r.writing);
        let repos = citable(&feature);
        let path = store::dir(None, project, slug).join(plan_review::FILE);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::PlanReviewRead {
            result: read_plan(&feature, &path, &repos, writing).map(Box::new),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// Run `n`'s file at `stamp` can't be taken, as `why` says for the
    /// panel; `text` is told once until it changes.
    fn refuse_plan_review(
        &mut self,
        project: &str,
        slug: &str,
        stamp: Stamp,
        why: String,
        text: String,
    ) {
        let key = (project.to_owned(), slug.to_owned(), STAGE);
        let Some(watch) = self.features.agents.get_mut(&key) else {
            return;
        };
        let again = watch.refused.as_ref() == Some(&(stamp, why.clone()));
        watch.refused = Some((stamp, why.clone()));
        if again {
            return;
        }
        tracing::warn!("feature {project}/{slug}: Plan review not read: {why}");
        if let Some(feature) = self.feature_mut(project, slug) {
            feature.log(feature::now(), format!("Plan review not read: {why}"));
            self.save_feature(project, slug);
        }
        self.show_warning(text);
    }

    /// What run `n` left was read. The agent must have left the code as it
    /// was; then its suggestions are read in, or its final plan goes up for
    /// review.
    pub(crate) fn plan_review_read(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Box<PlanRead>, String>,
    ) {
        let key = (project.to_owned(), slug.to_owned(), STAGE);
        let Some(watch) = self.features.agents.get_mut(&key).filter(|w| w.n == n) else {
            return;
        };
        watch.checking = None;
        watch.refused_at = Some(Instant::now());
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let running = feature.status == Status::Open
            && feature.current() == Some(STAGE)
            && feature.runs.get(&STAGE).is_some_and(|r| r.n == n);
        if !running {
            return;
        }
        let title = feature.title.clone();
        let read = match result {
            Ok(read) => read,
            Err(why) => {
                let text = format!("Couldn't read the code of \u{201c}{title}\u{201d}: {why}");
                let why = format!("couldn't read the code: {why}");
                return self.refuse_plan_review(project, slug, stamp, why, text);
            }
        };
        let review = feature.plan_review.as_ref();
        let changed = changed_code(&read.reading, review.and_then(|r| r.baseline.as_deref()));
        if !changed.is_empty() {
            let repos = changed.join(", ");
            let text = format!(
                "The Plan review agent changed the code in {repos}; undo its changes for its work to be read, or start a fresh agent"
            );
            let why = format!("the agent changed the code in {repos}");
            return self.refuse_plan_review(project, slug, stamp, why, text);
        }
        let Some(raws) = read.raws else {
            if let Some(watch) = self.features.agents.get_mut(&key) {
                watch.refused = None;
            }
            return self.stage_ready(project, slug, STAGE);
        };
        let raws = match raws {
            Ok(raws) => raws,
            Err(why) => {
                let text = format!("Couldn't read the suggestions of the plan review: {why}");
                let why = format!("couldn't read {}: {why}", plan_review::FILE);
                return self.refuse_plan_review(project, slug, stamp, why, text);
            }
        };
        if let Some(watch) = self.features.agents.get_mut(&key) {
            watch.refused = None;
        }
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let review = f.plan_review.get_or_insert_with(PlanReview::default);
        let first = review.stamp.is_none();
        let got = plan_review::import(review, n, raws);
        review.stamp = Some(stamp);
        let open = review.open();
        let not_found = review.not_found();
        let total = review.suggestions.len();
        let pane = f.runs.get_mut(&STAGE).and_then(|r| {
            r.nudged = false;
            r.pane.clone()
        });
        f.log(
            now,
            format!(
                "Plan review run {n} read: {} new, {} raised again, {} left out",
                got.new, got.again, got.dropped
            ),
        );
        self.save_feature(project, slug);
        let summary = match (first, open, total) {
            (_, _, 0) => format!("Plan review of \u{201c}{title}\u{201d} found nothing to change"),
            (_, 0, _) => format!("Plan review of \u{201c}{title}\u{201d} read again: all decided"),
            _ => format!(
                "Plan review of \u{201c}{title}\u{201d}: {} to decide",
                plural(open, "suggestion")
            ),
        };
        if first {
            self.notify_feature(project, slug, pane, &summary);
        }
        self.show_success(summary);
        if not_found > 0 {
            self.show_warning(format!(
                "{} of the review not found in the code",
                plural(not_found, "citation")
            ));
        }
    }

    /// Decide suggestion `id`, and save; why not when it can't be.
    pub(crate) fn decide_suggestion(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        decision: Decision,
    ) -> Result<(), String> {
        let review = self
            .feature_mut(project, slug)
            .and_then(|f| f.plan_review.as_mut())
            .ok_or("That review is gone")?;
        plan_review::decide(review, id, decision)?;
        self.save_feature(project, slug);
        Ok(())
    }

    /// Set suggestion `id`'s note, and save.
    pub(crate) fn note_suggestion(&mut self, project: &str, slug: &str, id: &str, note: &str) {
        let noted = self
            .feature_mut(project, slug)
            .and_then(|f| f.plan_review.as_mut())
            .is_some_and(|r| plan_review::set_note(r, id, note));
        if noted {
            self.save_feature(project, slug);
        }
    }

    /// Add the user's own suggestion, and save.
    pub(crate) fn add_suggestion(&mut self, project: &str, slug: &str, text: &str) {
        let added = self
            .feature_mut(project, slug)
            .and_then(|f| f.plan_review.as_mut())
            .map(|r| plan_review::add_own(r, text));
        if added.is_some() {
            self.save_feature(project, slug);
        }
    }

    /// Send the notes on the suggestions to the reviewer's chat, or to a
    /// fresh reviewer once its tab is gone.
    pub(crate) fn ask_reviewer(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let Some(review) = deciding(&feature) else {
            return self.show_warning("The suggestions aren't up for discussion now".into());
        };
        let notes = plan_review::notes(review);
        if notes.is_empty() {
            return self.show_warning("Nothing to ask: write a note on a suggestion first".into());
        }
        let Some(run) = feature.runs.get(&STAGE).cloned() else {
            self.launch_stage_agent(project, slug, STAGE, &[("comments", notes)]);
            return;
        };
        let pane = (run.pane.clone()).filter(|p| self.herdr_pane_place(p).is_some());
        let Some(pane) = pane.filter(|_| !run.held) else {
            self.launch_stage_agent(project, slug, STAGE, &[("comments", notes)]);
            return;
        };
        let data = store::dir(None, project, slug);
        let back = run.sent_back + 1;
        let file = (data.join("prompts")).join(format!("plan-review-{}-back-{back}.md", run.n));
        let names = self.projects.names(&feature);
        let mut values = crate::feature_agents::vars(&feature, &names, &data, STAGE);
        values.insert("comments.path".into(), file.display().to_string());
        let sent = std::fs::create_dir_all(data.join("prompts"))
            .and_then(|()| std::fs::write(&file, &notes))
            .map_err(|e| format!("writing {}: {e}", file.display()))
            .and_then(|()| {
                prompts::render_builtin("plan-review-back", &values).map_err(|e| e.to_string())
            })
            .and_then(|line| self.tell_agent_pane(&pane, line.trim()));
        if let Err(why) = sent {
            tracing::warn!("asking the reviewer of {project}/{slug}: {why}");
            return self.show_error(format!("Couldn't reach the reviewer: {why}"));
        }
        if let Some(f) = self.feature_mut(project, slug) {
            if let Some(r) = f.runs.get_mut(&STAGE) {
                r.sent_back = back;
            }
            f.log(feature::now(), "Notes sent to the Plan review agent");
            self.save_feature(project, slug);
        }
        self.show_success(format!("Sent the notes to {}", run.agent));
    }

    /// The suggestions are decided: a fresh agent writes the final plan, or,
    /// with nothing accepted, the plan stands as it is. The reviewer's run
    /// is kept, for going back.
    pub(crate) fn write_final_plan(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(review) = deciding(feature) else {
            return self.show_warning("The suggestions aren't read yet".into());
        };
        let open = review.open();
        if open > 0 {
            let text = format!("{} still to decide", plural(open, "suggestion"));
            return self.show_warning(text);
        }
        if !review.any_accepted() {
            return self.finish_without_final(project, slug, "the plan stands as it is");
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let parked = f.runs.get(&STAGE).cloned();
        f.forget_run(STAGE);
        if let Some(review) = f.plan_review.as_mut() {
            review.reviewer = parked.clone();
            review.writing = true;
        }
        f.log(feature::now(), "Suggestions decided: final plan to write");
        self.save_feature(project, slug);
        if !self.launch_stage_agent(project, slug, STAGE, &[]) {
            // Said why already. The decisions are still to act on.
            if let Some(f) = self.feature_mut(project, slug) {
                if let Some(run) = parked {
                    f.runs.insert(STAGE, run);
                }
                if let Some(review) = f.plan_review.as_mut() {
                    review.reviewer = None;
                    review.writing = false;
                }
                self.save_feature(project, slug);
            }
        }
    }

    /// Plan review is done with no final plan: Build reads the plan.
    fn finish_without_final(&mut self, project: &str, slug: &str, note: &str) {
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if f.current() != Some(STAGE) {
            return;
        }
        f.tick(now, "you", None);
        if let Some(mark) = f.marks.get_mut(&STAGE) {
            note.clone_into(&mut mark.note);
        }
        f.log(now, format!("Plan review done: {note}"));
        self.save_feature(project, slug);
        let key = (project.to_owned(), slug.to_owned(), STAGE);
        self.features.agents.remove(&key);
        self.feature_cursor_to_current_of(project, slug);
        self.show_success(format!("Plan review done: {note}"));
    }

    /// The panes of this review's agents still there: the stage's run, and
    /// the reviewer parked while the final plan is written.
    fn plan_review_panes(&self, project: &str, slug: &str) -> Vec<String> {
        let Some(feature) = self.feature(project, slug) else {
            return Vec::new();
        };
        let parked = feature.plan_review.as_ref().and_then(|r| r.reviewer.as_ref());
        (feature.runs.get(&STAGE).into_iter().chain(parked))
            .filter_map(|run| run.pane.clone())
            .filter(|pane| self.herdr_pane_place(pane).is_some())
            .collect()
    }

    /// Close the reviewer parked while the final plan was written.
    pub(crate) fn close_parked_reviewer(&mut self, project: &str, slug: &str) {
        let parked = self
            .feature(project, slug)
            .and_then(|f| f.plan_review.as_ref()?.reviewer.as_ref()?.pane.clone());
        if let Some(pane) = parked.filter(|p| self.herdr_pane_place(p).is_some()) {
            self.close_herdr_pane(&pane);
        }
    }

    /// Skip Plan review: at once when nothing would be lost, else after
    /// asking (an agent at work, decisions made).
    pub(crate) fn ask_skip_plan_review(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let decided = feature.plan_review.as_ref().is_some_and(|r| {
            (r.suggestions.iter()).any(|s| s.state != plan_review::State::Open)
        });
        let live = !self.plan_review_panes(project, slug).is_empty();
        if !decided && !live {
            return self.skip_plan_review_now(project, slug);
        }
        let mut body = String::from("Build reads the plan as it was approved.");
        if decided {
            body.push_str(" Your decisions stay in the feature's record but aren't used.");
        }
        if live {
            body.push_str(" Its agent stops.");
        }
        let then = Then::SkipPlanReview {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        self.ask("Skip Plan review?".to_owned(), body, "Skip", then);
    }

    /// Skip Plan review: what the agents wrote goes, so that Build can't
    /// take a final plan nobody approved.
    pub(crate) fn skip_plan_review_now(&mut self, project: &str, slug: &str) {
        let dir = store::dir(None, project, slug);
        if let Err(why) = artifacts::archive_final_plan(&dir) {
            tracing::warn!("archiving {project}/{slug}'s final plan: {why}");
            return self.show_error(format!("Couldn't skip Plan review: {why}"));
        }
        for pane in self.plan_review_panes(project, slug) {
            self.close_herdr_pane(&pane);
        }
        self.forget_doc_tabs(project, slug, STAGE);
        if let Some(f) = self.feature_mut(project, slug) {
            f.forget_run(STAGE);
            if let Some(review) = f.plan_review.as_mut() {
                review.reviewer = None;
                review.writing = false;
            }
        }
        self.finish_without_final(project, slug, "skipped");
    }

    /// Go back from the final plan to the decisions, after asking when its
    /// agent is at work.
    pub(crate) fn ask_change_decisions(&mut self, project: &str, slug: &str) {
        let Some(run) = self.feature(project, slug).and_then(|f| f.runs.get(&STAGE)) else {
            return self.change_decisions_now(project, slug);
        };
        let live = (run.pane.as_deref()).is_some_and(|p| self.herdr_pane_place(p).is_some());
        if !live {
            return self.change_decisions_now(project, slug);
        }
        let then = Then::ChangeDecisions {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        self.ask(
            "Change the decisions?".to_owned(),
            "The final plan moves to the archive folder and its agent stops.".to_owned(),
            "Change decisions",
            then,
        );
    }

    /// Back to the decisions: the final plan moved to the archive, its agent
    /// closed, the reviewer's run back in place.
    pub(crate) fn change_decisions_now(&mut self, project: &str, slug: &str) {
        let writing = (self.feature(project, slug))
            .is_some_and(|f| f.plan_review.as_ref().is_some_and(|r| r.writing));
        if !writing {
            return self.show_warning("There is no final plan to go back from".into());
        }
        let dir = store::dir(None, project, slug);
        if let Err(why) = artifacts::archive_final_plan(&dir) {
            tracing::warn!("archiving {project}/{slug}'s final plan: {why}");
            return self.show_error(format!("Couldn't go back to the decisions: {why}"));
        }
        let writer = (self.feature(project, slug))
            .and_then(|f| f.runs.get(&STAGE)?.pane.clone())
            .filter(|pane| self.herdr_pane_place(pane).is_some());
        if let Some(pane) = writer {
            self.close_herdr_pane(&pane);
        }
        self.forget_doc_tabs(project, slug, STAGE);
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        f.forget_run(STAGE);
        if let Some(review) = f.plan_review.as_mut() {
            review.writing = false;
            if let Some(run) = review.reviewer.take() {
                f.runs.insert(STAGE, run);
            }
        }
        f.log(feature::now(), "Final plan set aside: decisions open again");
        self.save_feature(project, slug);
        let key = (project.to_owned(), slug.to_owned(), STAGE);
        self.features.agents.remove(&key);
        self.show_success("Decisions open again; the final plan is in the archive folder".into());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn reading(patch: &str) -> Reading {
        Reading {
            patch: patch.into(),
            commits: BTreeMap::from([("api".to_owned(), 2), ("web".to_owned(), 0)]),
            ..Reading::default()
        }
    }

    #[test]
    fn the_code_counts_as_changed_only_past_where_the_review_began() {
        // A Build started over keeps its commits: those aren't the agent's.
        assert!(changed_code(&reading("p1"), Some("p1")).is_empty());
        assert_eq!(changed_code(&reading("p2"), Some("p1")), ["api"]);
        // Not known where it began: commits can't be told apart.
        assert!(changed_code(&reading("p2"), None).is_empty());
        let mut dirty = reading("p1");
        dirty.dirty = vec!["web".into()];
        dirty.stray = vec!["docs".into(), "web".into()];
        assert_eq!(changed_code(&dirty, Some("p1")), ["docs", "web"]);
        assert_eq!(changed_code(&dirty, None), ["docs", "web"]);
    }

    #[test]
    fn suggestions_wait_only_once_read_and_until_the_final_plan_starts() {
        let mut f = crate::features::tests::feature(
            "a",
            vec![crate::features::tests::repo("api", true)],
            2,
        );
        assert!(deciding(&f).is_none());
        f.plan_review = Some(PlanReview::default());
        assert!(deciding(&f).is_none(), "the agent hasn't written yet");
        f.plan_review.as_mut().unwrap().stamp = Some((1, 1));
        assert!(deciding(&f).is_some());
        f.plan_review.as_mut().unwrap().writing = true;
        assert!(deciding(&f).is_none());
    }

    #[test]
    fn a_citation_is_checked_before_it_is_read_in() {
        let dir = std::env::temp_dir().join(format!("gerdr-plan-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn real() {}\n").unwrap();
        let text = r#"{"suggestions": [{"kind": "gap", "plan": "p", "title": "t", "why": "w",
            "change": "c", "evidence": [
              {"file": "a.rs", "line": 1, "quote": "fn real()"},
              {"file": "a.rs", "line": 1, "quote": "fn made_up()"}]}]}"#;
        let mut raws = plan_review::parse(text).unwrap();
        plan_review::check_citations(&mut raws, &[("api".to_owned(), dir.clone())]);
        let mut review = PlanReview::default();
        plan_review::import(&mut review, 1, raws);
        let found: Vec<_> = review.suggestions[0].evidence.iter().map(|e| e.found).collect();
        assert_eq!(found, [Some(true), Some(false)]);
        assert_eq!(review.not_found(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
