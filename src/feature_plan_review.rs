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
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    Gerdr,
    ai_work::SendKind,
    confirm::Then,
    feature::{
        self, Feature, Stage, Status,
        artifacts::{self},
        patch::{self, Reading},
        plan_review::{self, Decision, Placed, PlanReview, Raw, State, Suggestion},
        review_items::{ItemOf, words},
        store,
    },
    feature_agents::Stamp,
    features::{CardNote, Done, plural},
    menu::MenuAction,
    prompts,
    review_cards::{self, ItemAct},
    text_field::TextField,
    toast::Level,
};

const STAGE: Stage = Stage::PlanReview;

/// What was read off the thread once the stage's agent rested.
#[derive(Debug)]
pub struct PlanRead {
    pub reading: Reading,
    /// The suggestions, their citations checked, while reviewing.
    pub raws: Option<Result<Vec<Raw>, String>>,
    /// Where each suggestion goes in the plan once the run is read in, by
    /// id: the earlier ones too, an old record's never placed among them.
    /// Empty when the plan couldn't be read: then they stay where they were.
    pub placed: BTreeMap<String, Placed>,
}

/// The repositories a citation may name: those of the feature, and those
/// it keeps as context, by key and folder.
pub(crate) fn citable(feature: &Feature) -> Vec<(String, PathBuf)> {
    let repos = (feature.repos.iter()).map(|r| (r.key(), r.worktree.clone()));
    let context = (feature.context.iter()).map(|c| (c.key(), c.main.clone()));
    repos.chain(context).collect()
}

/// Where a citation of a suggestion is: its checkout, the file, and the
/// line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CitationPlace {
    pub root: PathBuf,
    pub file: PathBuf,
    pub line: usize,
}

/// The file of `citation` of a feature with `feature`'s repositories: its
/// checkout and the file itself, or why not, a citation being the agent's
/// word. It only looks at the file system.
pub(crate) fn citation_place(
    feature: &Feature,
    citation: &plan_review::Evidence,
) -> Result<(PathBuf, PathBuf), String> {
    let repos = citable(feature);
    let Some(root) = plan_review::locate(citation, &repos) else {
        return Err(match citation.repo.as_str() {
            "" => format!("Can't tell which repository {} is in", citation.file),
            repo => format!("This feature has no repository \u{201c}{repo}\u{201d}"),
        });
    };
    let path = root.join(&citation.file);
    let (Ok(top), Ok(real)) = (root.canonicalize(), path.canonicalize()) else {
        return Err(format!("{} isn't there", citation.file));
    };
    if !real.starts_with(&top) {
        return Err(format!("{} is outside its repository", citation.file));
    }
    Ok((root.to_path_buf(), path))
}

/// Read what the code is and, while reviewing, the suggestions in `path`,
/// placed in the plan at `plan` as `review` comes to be once they are read
/// in as run `n`. Reads git and files: not for the UI thread.
fn read_plan(
    feature: &Feature,
    path: &Path,
    plan: Option<&Path>,
    repos: &[(String, PathBuf)],
    n: u32,
) -> Result<PlanRead, String> {
    let reading = patch::read(feature)?;
    let review = feature.plan_review.as_ref();
    let writing = review.is_some_and(|r| r.writing);
    let mut placed = BTreeMap::new();
    let raws = (!writing).then(|| {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", plan_review::FILE))?;
        let mut raws = plan_review::parse(&text).map_err(|lacks| lacks.join("; "))?;
        plan_review::check_citations(&mut raws, repos);
        match plan.map(|p| (p, std::fs::read_to_string(p))) {
            Some((_, Ok(plan_text))) => {
                placed = placements(review, n, &raws, &plan_text);
            }
            // Only less to go on: the cards and the plan doc say where.
            Some((p, Err(e))) => tracing::warn!("reading {}: {e}", p.display()),
            None => {}
        }
        Ok(raws)
    });
    Ok(PlanRead {
        reading,
        raws,
        placed,
    })
}

/// Where each suggestion of `review`, once `raws` are read in as run `n`,
/// goes in the plan reading `plan_text`, by id.
fn placements(
    review: Option<&PlanReview>,
    n: u32,
    raws: &[Raw],
    plan_text: &str,
) -> BTreeMap<String, Placed> {
    let mut review = review.cloned().unwrap_or_default();
    plan_review::import(&mut review, n, raws.to_vec());
    plan_review::place_all(&mut review, plan_text);
    (review.suggestions.into_iter())
        .filter_map(|s| Some((s.id, s.placed?)))
        .collect()
}

/// Give the suggestions of `review` their places of `placed`; one not
/// there (added since the read) keeps its own.
fn take_places(review: &mut PlanReview, placed: &BTreeMap<String, Placed>) {
    for s in &mut review.suggestions {
        if let Some(at) = placed.get(&s.id) {
            s.placed = Some(*at);
        }
    }
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

/// `feature`'s suggestions, when they can be decided, commented on and
/// added to now (`review_cards::can_decide`): at Plan review, read, and no
/// final plan being written. Else why not, for a toast, as the page says it.
pub(crate) fn decidable(feature: &Feature) -> Result<&PlanReview, String> {
    let before = feature.current().is_none_or(|stage| stage < STAGE);
    if feature.status != Status::Open || before {
        let title = &feature.title;
        return Err(format!("\u{201c}{title}\u{201d} isn't at Plan review"));
    }
    match deciding(feature).filter(|_| review_cards::can_decide(feature, STAGE)) {
        Some(review) => Ok(review),
        None => {
            let why = review_cards::why_not_decide(feature, STAGE);
            Err(why.unwrap_or("The suggestions aren't read yet").to_owned())
        }
    }
}

/// Why something taken out of `feature`'s suggestions can't be put back:
/// they can't be decided any more.
fn why_not_restore(feature: &Feature) -> Option<String> {
    decidable(feature).err()
}

/// What a toast says of suggestion `id` once `decision` was made of it,
/// where it stood at `was`.
fn decided_text(id: &str, was: State, decision: &Decision) -> String {
    let w = words(ItemOf::Suggestion);
    let word = match (decision, was) {
        (Decision::Accept, _) => w.wanted,
        (Decision::Reject(_), _) => w.refused,
        (Decision::Clear, State::Accepted) => "no longer accepted",
        (Decision::Clear, _) => "reopened",
    };
    format!("Suggestion {id} {word}")
}

/// What a toast says of `n` questions accepting all left open: they wait
/// for an answer.
fn waiting_text(n: usize) -> String {
    match n {
        1 => "1 question waits for an answer".to_owned(),
        n => format!("{n} questions wait for an answer"),
    }
}

/// What a toast says once nothing of `review` is left to decide.
fn all_decided_text(review: &PlanReview) -> String {
    match review.count(State::Accepted) {
        0 => "All suggestions decided: none accepted".to_owned(),
        n => format!("All suggestions decided: {n} accepted"),
    }
}

impl Gerdr {
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
        let repos = citable(&feature);
        let dir = store::dir(None, project, slug);
        let path = dir.join(plan_review::FILE);
        let plan = artifacts::path(&dir, Stage::Plan);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let there = self.machine_dirt(&project, &slug);
        self.feature_work(None, move |_| Done::PlanReviewRead {
            result: read_plan(&feature, &path, plan.as_deref(), &repos, n).map(|mut read| {
                crate::machine_work::with_dirt(&feature, &mut read.reading, &there);
                Box::new(read)
            }),
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
        // The cards come and go around the one the user is on: it stays theirs.
        let selected = self.selected_review_card();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let review = f.plan_review.get_or_insert_with(PlanReview::default);
        let first = review.stamp.is_none();
        let got = plan_review::import(review, n, raws);
        take_places(review, &read.placed);
        review.stamp = Some(stamp);
        let reviewer = f.runs.get(&STAGE);
        let record = plan_review::RunRecord {
            n,
            at: now,
            agent: reviewer.map(|r| r.agent.clone()).unwrap_or_default(),
            model: reviewer.and_then(|r| r.model.clone()).unwrap_or_default(),
            raised: got.ids.clone(),
            new: got.new,
            again: got.again,
            dropped: got.dropped,
        };
        let review = f.plan_review.get_or_insert_with(PlanReview::default);
        // The same run's file rewritten, as after Ask reviewer: read again.
        let again = review.runs.iter().any(|r| r.n == n);
        plan_review::keep_run(review, record);
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
                "Plan review run {n} read{}: {} new, {} raised again, {} left out",
                if again { " again" } else { "" },
                got.new,
                got.again,
                got.dropped
            ),
        );
        self.save_feature(project, slug);
        self.reselect_review_card(selected);
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

    /// Change feature `slug`'s suggestions by `change` and keep them:
    /// `change` says what it did, for the log, else why it can't. Why not
    /// is said, as when they can't be changed now (`decidable`). Whether it
    /// did.
    fn change_plan_review(
        &mut self,
        project: &str,
        slug: &str,
        change: impl FnOnce(&mut PlanReview) -> Result<String, String>,
    ) -> bool {
        let changed = (|| {
            let f = (self.feature_mut(project, slug)).ok_or("That feature is gone")?;
            decidable(f).map(|_| ())?;
            let review = (f.plan_review.as_mut()).ok_or("The suggestions aren't read yet")?;
            let what = change(review)?;
            f.log(feature::now(), what);
            Ok::<(), String>(())
        })();
        match changed {
            Ok(()) => {
                self.save_feature(project, slug);
                true
            }
            Err(why) => {
                self.show_warning(why);
                false
            }
        }
    }

    /// Suggestion `id` of feature `slug`, as it is now.
    fn suggestion_of(&self, project: &str, slug: &str, id: &str) -> Option<&Suggestion> {
        self.feature(project, slug)?
            .plan_review
            .as_ref()?
            .suggestion(id)
    }

    /// Decide suggestion `id`, log it, save and say so, offering what comes
    /// next once nothing is left to decide; why not when it can't be.
    /// Whether it was.
    fn decide_suggestion_said(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        decision: Decision,
    ) -> bool {
        let was = (self.suggestion_of(project, slug, id)).map_or(State::Open, |s| s.state);
        let done = decided_text(id, was, &decision);
        let logged = match &decision {
            Decision::Reject(why) => format!("{done}: {}", why.trim()),
            _ => done.clone(),
        };
        let decided = self.change_plan_review(project, slug, |review| {
            plan_review::decide(review, id, decision)?;
            Ok(logged)
        });
        if decided {
            self.after_suggestion_decision(project, slug, done, None);
        }
        decided
    }

    /// Reject suggestion `id` for `why`, from the dialog asking it: the
    /// keyboard goes on to the next card left to decide.
    pub(crate) fn reject_suggestion(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        why: &str,
    ) -> bool {
        let done = self.decide_suggestion_said(project, slug, id, Decision::Reject(why.to_owned()));
        if done {
            self.select_next_open_item(ItemOf::Suggestion, id);
        }
        done
    }

    /// What comes of a change to feature `slug`'s suggestions, said in
    /// `done`, with `undo` on its toast; once nothing is left to decide, the
    /// final plan is offered instead. An offer has its own button, so only
    /// the toast that says `done` can undo (as AI review's `after_decision`).
    fn after_suggestion_decision(
        &mut self,
        project: &str,
        slug: &str,
        done: String,
        undo: Option<MenuAction>,
    ) {
        let Some(review) = self
            .feature(project, slug)
            .and_then(|f| f.plan_review.as_ref())
        else {
            return;
        };
        if review.open() == 0 && !review.suggestions.is_empty() && undo.is_none() {
            let text = all_decided_text(review);
            return self.offer_final_plan(project, slug, text);
        }
        let button = undo.map(|undo| ("Undo".to_owned(), undo));
        self.toast_with(done, Level::Success, button);
    }

    /// Say `text`, with a button for what comes next now that nothing of
    /// feature `slug`'s suggestions is left to decide: write the final plan,
    /// or with nothing accepted go on with the plan as it is. Nothing starts
    /// by itself.
    fn offer_final_plan(&mut self, project: &str, slug: &str, text: String) {
        let accepted = (self.feature(project, slug))
            .and_then(|f| f.plan_review.as_ref())
            .is_some_and(PlanReview::any_accepted);
        let label = match accepted {
            true => "Write final plan",
            false => "Use the plan as it is",
        };
        let then = MenuAction::WriteFinalPlan(project.to_owned(), slug.to_owned());
        self.toast_with(text, Level::Success, Some((label.to_owned(), then)));
    }

    /// Accept every open suggestion of feature `slug` but the questions
    /// still to answer and those a later run left out; its toast's Undo
    /// opens them again.
    pub(crate) fn accept_all_suggestions(&mut self, project: &str, slug: &str) {
        let mut accepted = Vec::new();
        let done = self.change_plan_review(project, slug, |review| {
            accepted = plan_review::accept_all(review);
            match (accepted.len(), review.waiting_answer()) {
                (0, 0) => Err("No suggestion left to decide".to_owned()),
                (0, waiting) => Err(waiting_text(waiting)),
                (n, _) => Ok(format!("Accepted all open: {n}")),
            }
        });
        let Some(review) = (self.feature(project, slug))
            .and_then(|f| f.plan_review.as_ref())
            .filter(|_| done)
        else {
            return;
        };
        let mut text = format!("{} accepted", plural(accepted.len(), "suggestion"));
        if let waiting @ 1.. = review.waiting_answer() {
            text = format!("{text}; {}", waiting_text(waiting));
        }
        // The page offers what comes next: this toast only undoes.
        let undo = MenuAction::ReopenSuggestions {
            project: project.to_owned(),
            slug: slug.to_owned(),
            ids: accepted,
        };
        self.toast_with(text, Level::Success, Some(("Undo".to_owned(), undo)));
    }

    /// Undo `accept_all_suggestions`: open again those of `ids` still
    /// accepted.
    pub(crate) fn reopen_suggestions(&mut self, project: &str, slug: &str, ids: &[String]) {
        let mut n = 0;
        let done = self.change_plan_review(project, slug, |review| {
            for id in ids {
                if review.suggestion(id).is_some_and(|s| s.state == State::Accepted) {
                    plan_review::decide(review, id, Decision::Clear)?;
                    n += 1;
                }
            }
            match n {
                0 => Err("Nothing to undo: they were decided again since".to_owned()),
                n => Ok(format!("Accept all open undone: {n}")),
            }
        });
        if done {
            self.show_success(format!("{} open again", plural(n, "suggestion")));
        }
    }

    /// Set suggestion `id`'s note, a question's answer; empty deletes it.
    /// Whether it was set: why not is said.
    pub(crate) fn note_suggestion(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        note: &str,
    ) -> bool {
        // Left as it was: nothing to save or log.
        let s = self.suggestion_of(project, slug, id);
        if s.is_some_and(|s| s.note == note.trim()) {
            return true;
        }
        let answered =
            s.is_some_and(|s| s.kind == plan_review::Kind::Question && s.state == State::Accepted);
        if note.trim().is_empty() && answered {
            self.show_warning(
                "An accepted question keeps its answer: take the decision back first".into(),
            );
            return false;
        }
        self.change_plan_review(project, slug, |review| {
            let what = plan_review::set_note(review, id, note)?;
            Ok(format!("Suggestion {id}: {what}"))
        })
    }

    /// Keep what the user wrote on suggestion `id`'s card, and accept it
    /// when they wrote it to (`accept`: a question answered to accept it).
    pub(crate) fn keep_suggestion_note(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        note: &str,
        accept: bool,
    ) {
        // An answer to an open question is what accepting it asks for:
        // answered, it is decided, not left counted to decide.
        let question = (self.suggestion_of(project, slug, id))
            .is_some_and(|s| s.kind == plan_review::Kind::Question && s.state == State::Open);
        let accept = accept || (question && !note.trim().is_empty());
        if self.note_suggestion(project, slug, id, note) && accept {
            self.decide_suggestion_said(project, slug, id, Decision::Accept);
        }
    }

    /// Delete the user's comment on suggestion `id`: the toast can bring it
    /// back (checklist C4).
    pub(crate) fn delete_suggestion_comment(&mut self, project: &str, slug: &str, id: &str) {
        let note = (self.suggestion_of(project, slug, id)).map(|s| s.note.clone());
        let Some(note) = note.filter(|note| !note.is_empty()) else {
            return;
        };
        if !self.note_suggestion(project, slug, id, "") {
            return;
        }
        let undo = MenuAction::RestoreItemComment {
            project: project.to_owned(),
            slug: slug.to_owned(),
            of: ItemOf::Suggestion,
            id: id.to_owned(),
            note,
        };
        let what = match self.suggestion_of(project, slug, id) {
            Some(s) if s.kind == plan_review::Kind::Question => "Answer",
            _ => "Comment",
        };
        self.toast_with(
            format!("{what} on {id} deleted"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// Undo `delete_suggestion_comment`: refused, and why, once the
    /// suggestions can't be decided any more.
    pub(crate) fn restore_suggestion_comment(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        note: &str,
    ) {
        let why = self.feature(project, slug).map(why_not_restore);
        if let Some(why) = why.flatten() {
            return self.show_warning(format!("Can't put the comment on {id} back: {why}"));
        }
        self.note_suggestion(project, slug, id, note);
    }

    /// Add the user's own suggestion saying `text`, and select it; say why
    /// when it can't be. Whether it was added.
    pub(crate) fn add_suggestion(&mut self, project: &str, slug: &str, text: &str) -> bool {
        let mut added = String::new();
        let done = self.change_plan_review(project, slug, |review| {
            added = plan_review::add_own(review, text)?;
            Ok(format!("You added suggestion {added}"))
        });
        if done {
            // It lands at the top of the page's list.
            self.select_panel_item(ItemOf::Suggestion, &added);
            self.show_success(format!(
                "Suggestion {added} added, to go into the final plan"
            ));
        }
        done
    }

    /// Reword suggestion `id`'s change, accepting it unless it is a question
    /// still to answer. Whether it was.
    pub(crate) fn edit_suggestion_change(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        text: &str,
    ) -> bool {
        let done = self.change_plan_review(project, slug, |review| {
            let what = plan_review::edit_change(review, id, text)?;
            Ok(format!("Suggestion {id}: {what}"))
        });
        let Some(review) = (self.feature(project, slug))
            .and_then(|f| f.plan_review.as_ref())
            .filter(|_| done)
        else {
            return done;
        };
        let accepted = review
            .suggestion(id)
            .is_some_and(|s| s.state == State::Accepted);
        let text = match accepted {
            true => format!("Suggestion {id} edited and accepted"),
            false => format!("Suggestion {id} edited"),
        };
        match review.open() {
            0 => self.offer_final_plan(project, slug, text),
            _ => self.show_success(text),
        }
        true
    }

    /// Rewrite suggestion `id` of the user's own. Whether it was.
    pub(crate) fn edit_own_suggestion(
        &mut self,
        project: &str,
        slug: &str,
        id: &str,
        text: &str,
    ) -> bool {
        let done = self.change_plan_review(project, slug, |review| {
            let what = plan_review::edit_own(review, id, text)?;
            Ok(format!("Your suggestion {id}: {what}"))
        });
        if done {
            self.show_success(format!("Suggestion {id} edited"));
        }
        done
    }

    /// Delete suggestion `id` of the user's own: it goes for good, not
    /// rejected. The toast can bring it back (checklist C4).
    fn delete_own_suggestion_said(&mut self, project: &str, slug: &str, id: &str) {
        let selected = self.selected_review_card();
        let mut gone = None;
        let done = self.change_plan_review(project, slug, |review| {
            gone = Some(plan_review::delete_own(review, id)?);
            Ok(format!("You deleted suggestion {id}"))
        });
        let Some((at, suggestion)) = gone.filter(|_| done) else {
            return;
        };
        self.reselect_review_card(selected);
        let undo = MenuAction::RestoreSuggestion {
            project: project.to_owned(),
            slug: slug.to_owned(),
            at,
            suggestion: Box::new(suggestion),
        };
        self.after_suggestion_decision(
            project,
            slug,
            format!("Suggestion {id} deleted"),
            Some(undo),
        );
    }

    /// Undo removing one of the user's own suggestions: refused, and why,
    /// once the suggestions can't be decided any more.
    pub(crate) fn restore_suggestion(
        &mut self,
        project: &str,
        slug: &str,
        at: usize,
        suggestion: Suggestion,
    ) {
        let id = suggestion.id.clone();
        let why = self.feature(project, slug).map(why_not_restore);
        if let Some(why) = why.flatten() {
            return self.show_warning(format!("Can't put {id} back: {why}"));
        }
        let selected = self.selected_review_card();
        let done = self.change_plan_review(project, slug, |review| {
            match plan_review::restore(review, at, suggestion) {
                true => Ok(format!("You brought back suggestion {id}")),
                false => Err(format!("Can't put {id} back: its id is taken")),
            }
        });
        if done {
            self.reselect_review_card(selected);
        }
    }

    /// Where citation `i` of suggestion `id` of feature `slug` is; else why
    /// it can't be opened.
    pub(crate) fn suggestion_citation(
        &self,
        project: &str,
        slug: &str,
        id: &str,
        i: usize,
    ) -> Result<CitationPlace, String> {
        let feature = self.feature(project, slug).ok_or("That feature is gone")?;
        let citation = (feature.plan_review.as_ref())
            .and_then(|r| r.suggestion(id))
            .and_then(|s| s.evidence.get(i))
            .ok_or("This suggestion cites no code")?;
        let (root, file) = citation_place(feature, citation)?;
        Ok(CitationPlace {
            root,
            file,
            line: citation.line,
        })
    }

    /// Open `place`, a citation of feature `slug`'s plan review, in Zed: in
    /// the feature's own workspace, as its diff's Zed is.
    pub(crate) fn open_citation(&mut self, project: &str, slug: &str, place: CitationPlace) {
        let w = (self.feature_workspace(project, slug)).unwrap_or(self.active_workspace);
        self.open_line_in_zed(w, &place.root, &place.file, place.line);
    }

    /// Open citation `i` of suggestion `id` in Zed, or say why not.
    pub(crate) fn open_suggestion_code(&mut self, project: &str, slug: &str, id: &str, i: usize) {
        match self.suggestion_citation(project, slug, id, i) {
            Ok(place) => self.open_citation(project, slug, place),
            Err(why) => self.show_warning(why),
        }
    }

    /// Do `act` to suggestion `id` of feature `slug`, from its card or its
    /// keys.
    pub(crate) fn apply_suggestion(&mut self, project: &str, slug: &str, id: &str, act: ItemAct) {
        match act {
            ItemAct::Decide(decision) => {
                let decision = Decision::from_item(decision);
                // A question is accepted by answering it: the field opens.
                let unanswered =
                    (self.suggestion_of(project, slug, id)).is_some_and(Suggestion::unanswered);
                if decision == Decision::Accept && unanswered {
                    return self.write_suggestion_comment(project, slug, id, true);
                }
                self.decide_suggestion_said(project, slug, id, decision);
            }
            ItemAct::AskRefuse => self.ask_reject_suggestion(project, slug, id),
            ItemAct::Edit => self.ask_edit_suggestion(project, slug, id),
            ItemAct::Comment => self.write_suggestion_comment(project, slug, id, false),
            // A question with no answer: accepted once its answer is saved.
            ItemAct::Answer => self.write_suggestion_comment(project, slug, id, true),
            ItemAct::DeleteComment => self.delete_suggestion_comment(project, slug, id),
            ItemAct::DeleteOwn => self.delete_own_suggestion_said(project, slug, id),
            ItemAct::OpenCode => self.open_suggestion_code(project, slug, id, 0),
            ItemAct::Show => self.show_in_plan(project, slug, id),
        }
    }

    /// Write or edit the user's comment on suggestion `id`, a question's
    /// answer, on its card on the page: the keyboard goes to it there.
    /// `accept`: saving it accepts the suggestion too.
    fn write_suggestion_comment(&mut self, project: &str, slug: &str, id: &str, accept: bool) {
        let Some(s) = self.item_to_ask(project, slug, ItemOf::Suggestion, id) else {
            return;
        };
        if let Err(why) =
            crate::feature::review_items::note_editable(ItemOf::Suggestion, id, s.standing())
        {
            return self.show_warning(why);
        }
        let field = TextField::multiline(s.note());
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let on = CardNote::Item {
            of: ItemOf::Suggestion,
            id: id.to_owned(),
            accept,
        };
        panel.note = Some((on, field));
        self.stop_key_repeat();
        self.select_panel_item(ItemOf::Suggestion, id);
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
        // The live reviewer is told the notes, and what was rejected and why,
        // not to be raised again; a fresh one reads the rejected in its prompt
        // (`plan_review.rejected`), so only the notes go with it.
        let notes = plan_review::back_notes(review);
        if notes.is_empty() {
            return self.show_warning(
                "Nothing to ask: comment on a suggestion or reject one first".into(),
            );
        }
        let fresh = vec![("comments", plan_review::notes(review))];
        let Some(run) = feature.runs.get(&STAGE).cloned() else {
            self.launch_stage_agent(project, slug, STAGE, &fresh);
            return;
        };
        let alive = (run.pane.as_deref()).is_some_and(|p| self.pane_there(p));
        if !alive || run.held {
            self.launch_stage_agent(project, slug, STAGE, &fresh);
            return;
        }
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
            .and_then(|line| {
                self.tell_stage_agent(project, slug, STAGE, SendKind::PlanReviewBack, line.trim())
            });
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
        // Done by one press, with nothing asked: Undo reopens it, as Reopen does.
        let undo = MenuAction::ReopenStage(project.to_owned(), slug.to_owned(), STAGE);
        self.toast_with(
            format!("Plan review done: {note}"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// The panes of this review's agents still there: the stage's run, and
    /// the reviewer parked while the final plan is written.
    fn plan_review_panes(&self, project: &str, slug: &str) -> Vec<String> {
        let Some(feature) = self.feature(project, slug) else {
            return Vec::new();
        };
        let parked = feature
            .plan_review
            .as_ref()
            .and_then(|r| r.reviewer.as_ref());
        (feature.runs.get(&STAGE).into_iter().chain(parked))
            .filter_map(|run| run.pane.clone())
            .filter(|pane| self.pane_there(pane))
            .collect()
    }

    /// Close the reviewer parked while the final plan was written.
    pub(crate) fn close_parked_reviewer(&mut self, project: &str, slug: &str) {
        let parked = self
            .feature(project, slug)
            .and_then(|f| f.plan_review.as_ref()?.reviewer.as_ref()?.pane.clone());
        if let Some(pane) = parked.filter(|p| self.pane_there(p)) {
            self.close_herdr_pane(&pane);
        }
    }

    /// Skip Plan review: at once when nothing would be lost, else after
    /// asking (an agent at work, decisions made).
    pub(crate) fn ask_skip_plan_review(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let decided = feature
            .plan_review
            .as_ref()
            .is_some_and(|r| (r.suggestions.iter()).any(|s| s.state != plan_review::State::Open));
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
        let live = (run.pane.as_deref()).is_some_and(|p| self.pane_there(p));
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
            .filter(|pane| self.pane_there(pane));
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

    fn at_plan_review() -> Feature {
        crate::features::tests::feature("a", vec![crate::features::tests::repo("api", true)], 2)
    }

    #[test]
    fn suggestions_are_decided_only_at_plan_review_once_read_and_not_while_writing() {
        let mut f = at_plan_review();
        assert_eq!(f.current(), Some(STAGE));
        assert!(decidable(&f).unwrap_err().contains("decide once it's read"));
        f.plan_review = Some(PlanReview {
            stamp: Some((1, 1)),
            ..PlanReview::default()
        });
        assert!(decidable(&f).is_ok());
        f.plan_review.as_mut().unwrap().writing = true;
        assert_eq!(
            decidable(&f).unwrap_err(),
            review_cards::SUGGESTIONS_WRITING
        );
        f.plan_review.as_mut().unwrap().writing = false;
        f.tick(2, "you", None);
        assert_eq!(decidable(&f).unwrap_err(), review_cards::SUGGESTIONS_DONE);
        assert_eq!(why_not_restore(&f).unwrap(), review_cards::SUGGESTIONS_DONE);
        let api = crate::features::tests::repo("api", true);
        let early = crate::features::tests::feature("b", vec![api], 1);
        assert_eq!(early.current(), Some(Stage::Plan));
        assert!(
            decidable(&early)
                .unwrap_err()
                .contains("isn't at Plan review")
        );
    }

    #[test]
    fn a_decision_is_said_in_a_suggestions_words() {
        let reject = Decision::Reject("no".into());
        assert_eq!(
            decided_text("r1-2", State::Open, &Decision::Accept),
            "Suggestion r1-2 accepted"
        );
        assert_eq!(
            decided_text("r1-2", State::Open, &reject),
            "Suggestion r1-2 rejected"
        );
        assert_eq!(
            decided_text("r1-2", State::Accepted, &Decision::Clear),
            "Suggestion r1-2 no longer accepted"
        );
        assert_eq!(
            decided_text("r1-2", State::Rejected, &Decision::Clear),
            "Suggestion r1-2 reopened"
        );
        let mut review = PlanReview::default();
        plan_review::add_own(&mut review, "Mine").unwrap();
        assert_eq!(
            all_decided_text(&review),
            "All suggestions decided: 1 accepted"
        );
        review.suggestions[0].state = State::Rejected;
        assert_eq!(
            all_decided_text(&review),
            "All suggestions decided: none accepted"
        );
        for text in [
            decided_text("r1-2", State::Accepted, &Decision::Clear),
            all_decided_text(&review),
        ] {
            assert!(!text.ends_with('.'), "{text}");
            assert!(!text.to_lowercase().contains("fix"), "{text}");
        }
    }

    #[test]
    fn questions_left_open_wait_for_an_answer() {
        assert_eq!(waiting_text(1), "1 question waits for an answer");
        assert_eq!(waiting_text(2), "2 questions wait for an answer");
    }

    #[test]
    fn a_run_read_in_places_every_suggestion_an_old_one_too() {
        let plan = "# Plan\n## Storage\nKeep rows in SQLite.\n## Sync\nPush on save.\n";
        let mut review = PlanReview {
            n: 1,
            ..PlanReview::default()
        };
        // An old record's: never placed.
        review.suggestions.push(Suggestion {
            id: "r1-1".into(),
            plan: "Sync".into(),
            run: 1,
            ..Suggestion::default()
        });
        let text = r#"{"suggestions": [{"kind": "question", "plan": "Storage", "title": "t",
            "why": "w", "change": "c", "plan_quote": "keep rows in **SQLite**"}]}"#;
        let raws = plan_review::parse(text).unwrap();
        let placed = placements(Some(&review), 2, &raws, plan);
        assert_eq!(placed.get("r1-1"), Some(&Placed::Heading(4)));
        assert_eq!(placed.get("r2-1"), Some(&Placed::Quote(3)));
        // The record takes them as it is when read in; one added since keeps
        // its own.
        plan_review::add_own(&mut review, "Mine").unwrap();
        plan_review::import(&mut review, 2, raws);
        take_places(&mut review, &placed);
        let at = |id: &str| review.suggestion(id).unwrap().placed;
        assert_eq!(at("r1-1"), Some(Placed::Heading(4)));
        assert_eq!(at("r2-1"), Some(Placed::Quote(3)));
        assert_eq!(at("u1"), None);
    }

    #[test]
    fn a_citation_opens_only_inside_its_repository() {
        let dir = std::env::temp_dir().join(format!("gerdr-plan-citation-{}", std::process::id()));
        let repo = dir.join("api");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.join("secret"), "x").unwrap();
        let mut f = crate::features::tests::feature(
            "a",
            vec![crate::features::tests::repo("api", true)],
            2,
        );
        f.repos[0].worktree = repo.clone();
        let cite = |file: &str, repo: &str| plan_review::Evidence {
            repo: repo.into(),
            file: file.into(),
            line: 1,
            quote: "q".into(),
            found: None,
        };
        let (root, path) = citation_place(&f, &cite("src/a.rs", "")).unwrap();
        assert_eq!((root, path), (repo.clone(), repo.join("src/a.rs")));
        assert!(
            citation_place(&f, &cite("src/none.rs", ""))
                .unwrap_err()
                .contains("isn't there")
        );
        assert!(
            citation_place(&f, &cite("../secret", ""))
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            citation_place(&f, &cite("src/a.rs", "other"))
                .unwrap_err()
                .contains("other")
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let found: Vec<_> = review.suggestions[0]
            .evidence
            .iter()
            .map(|e| e.found)
            .collect();
        assert_eq!(found, [Some(true), Some(false)]);
        assert_eq!(review.not_found(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
