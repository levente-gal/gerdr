//! The AI review stage of features, run by gerdr (inc 5, agent A): the
//! reviewer agent, its `review.json` read in as findings (see
//! `feature::review_import`), the user's Fix and Dismiss, their edits and
//! their own comments, and the fixes sent to the Build agent. All of it can
//! be done on the stage's page (`features::finding_cards`), the deciding in
//! the feature's diff too.
//!
//! Verify passing starts the reviewer, an agent other than Build's (see
//! `feature_agents::pick_reviewer`). Once it is done, its findings are read
//! in against the code Verify passed: code that changed since sends the
//! feature back. A review with no finding that holds up marks AI review
//! done; else its gate waits for the user to decide each one, then to send
//! those to fix to the Build agent: straight away when the user only took
//! them as they are, through a fix plan they read first when they wrote
//! anything of their own. Fixed code goes through Build and Verify again, and
//! a new review checks the fixes.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    Gerdr,
    feature::{
        self, Feature, Stage, Status, artifacts,
        fix_round::GiveBack,
        review_import::{self, AiReview, CodeRead, Decision, Finding, FindingState},
        store,
    },
    feature_agents::Stamp,
    feature_diff::{COMMENTS, decide_first},
    features::{CardNote, Done, plural},
    menu::MenuAction,
    review,
    text_field::TextField,
    toast::Level,
};

/// The values the AI review prompt gets of `feature` besides the stage's
/// own: per touched repository the diff to review (all of the change the
/// first time, since the last review after), and the earlier findings,
/// sent, dismissed and still open.
pub(crate) fn review_vars(feature: &Feature) -> Vec<(String, String)> {
    let review = feature.review.as_ref();
    let diffs = feature
        .touched()
        .map(|repo| {
            let whole = format!("git diff {}...HEAD", repo.base_commit);
            let at = repo.worktree.display();
            let key = repo.key();
            match review.and_then(|r| r.reviewed.get(&key)) {
                Some(sha) => format!(
                    "   - {key}, in {at}: `git diff {sha}..HEAD` (what changed since the last review; `{whole}` shows the whole change)"
                ),
                None => format!("   - {key}, in {at}: `{whole}`"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let list = |states: &[FindingState]| {
        review
            .map(|r| {
                r.findings
                    .iter()
                    .filter(|f| states.contains(&f.state))
                    .map(|f| finding_line(f, feature.many_repos()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    vec![
        ("review.diffs".into(), diffs),
        ("review.sent".into(), list(&[FindingState::Sent])),
        ("review.dismissed".into(), list(&[FindingState::Dismissed])),
        (
            "review.open".into(),
            list(&[FindingState::Open, FindingState::Fix]),
        ),
    ]
}

/// The user's comments in `own` to follow the fixes sent to Build, on
/// repositories `roots` (their top folders and labels). At AI review the
/// code is as they read it, so none is marked outdated.
fn own_comments_markdown(own: &review::Review, roots: &[(PathBuf, String)]) -> String {
    let order: Vec<PathBuf> = roots.iter().map(|(root, _)| root.clone()).collect();
    let label = |root: &Path| {
        (roots.iter().find(|(r, _)| r == root))
            .map_or_else(|| root.display().to_string(), |(_, label)| label.clone())
    };
    format!(
        "\n---\n\n{}",
        review::markdown(own, &order, label, |_| false)
    )
}

/// A finding as the reviewer's prompt lists it: "- r1-2 (high,
/// src/a.rs:12): Off by one", with the reason a dismissed one has.
fn finding_line(f: &Finding, multi: bool) -> String {
    let mut line = if f.yours {
        format!("- {} (added by the user): {}", f.id, f.title)
    } else {
        format!(
            "- {} ({}, {}): {}",
            f.id,
            f.severity.label(),
            review_import::location(f, multi),
            f.title
        )
    };
    let one_line = |text: &str| text.replace('\n', " ");
    if f.yours && !f.body.is_empty() {
        line.push_str(&format!(". {}", one_line(&f.body)));
    }
    if let Some(fix) = &f.my_fix {
        line.push_str(&format!(". The user wants this fix: {}", one_line(fix)));
    }
    if !f.note.is_empty() {
        line.push_str(&format!(". The user's comment: {}", one_line(&f.note)));
    }
    if f.state == FindingState::Dismissed && !f.reason.is_empty() {
        line.push_str(&format!(". Dismissed because: {}", f.reason));
    }
    line
}

/// `what` as a file name has it: its words, "review-comments" of "review
/// comments", "findings" of "2 findings".
fn file_key(what: &str) -> String {
    let words: Vec<String> = what
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "fixes".into()
    } else {
        words.join("-")
    }
}

/// What AI review of `title` waits for in `review`, for its toast and
/// notification; `None` once nothing does.
fn waiting_text(title: &str, review: &AiReview) -> Option<String> {
    let open = review.count(FindingState::Open);
    let fix = review.count(FindingState::Fix);
    let what = if open > 0 {
        format!("{} to decide", plural(open, "finding"))
    } else if fix > 0 {
        format!("{} to fix", plural(fix, "finding"))
    } else {
        return None;
    };
    Some(format!("AI review of \u{201c}{title}\u{201d}: {what}"))
}

impl Gerdr {
    /// Read the code of feature `slug` and its `review.json` off the
    /// thread for run `n` of its AI review agent, whose file has `stamp`
    /// (see `review_read`).
    pub(crate) fn check_review_code(&mut self, project: &str, slug: &str, n: u32, stamp: Stamp) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let data = store::dir(None, project, slug);
        let Some(path) = artifacts::path(&data, Stage::AiReview) else {
            return;
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let there = self.machine_dirt(&project, &slug);
        self.feature_work(None, move |_| Done::ReviewRead {
            result: review_import::read_code(&feature, &path).map(|mut read| {
                crate::machine_work::with_dirt(&feature, &mut read.reading, &there);
                Box::new(read)
            }),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// The review of run `n` (file at `stamp`) can't be read in, as `why`
    /// says for the panel; `text` is told once until it changes (`error`:
    /// as an error, else a warning).
    fn refuse_review(
        &mut self,
        project: &str,
        slug: &str,
        stamp: Stamp,
        why: String,
        text: String,
        error: bool,
    ) {
        let key = (project.to_owned(), slug.to_owned(), Stage::AiReview);
        let Some(watch) = self.features.agents.get_mut(&key) else {
            return;
        };
        let again = watch.refused.as_ref() == Some(&(stamp, why.clone()));
        watch.refused = Some((stamp, why.clone()));
        if again {
            return;
        }
        tracing::warn!("feature {project}/{slug}: AI review not read: {why}");
        if let Some(feature) = self.feature_mut(project, slug) {
            feature.log(feature::now(), format!("AI review not read: {why}"));
            self.save_feature(project, slug);
        }
        if error {
            self.show_error(text);
        } else {
            self.show_warning(text);
        }
    }

    /// The code and `review.json` of feature `slug` were read for run `n`
    /// of its AI review agent, whose file had `stamp`. The agent must have
    /// left the code as Verify passed it; then its findings are read in,
    /// and AI review is marked done when none holds up, else its gate
    /// waits.
    pub(crate) fn review_read(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Box<CodeRead>, String>,
    ) {
        let key = (project.to_owned(), slug.to_owned(), Stage::AiReview);
        let Some(watch) = self.features.agents.get_mut(&key).filter(|w| w.n == n) else {
            return;
        };
        watch.checking = None;
        watch.refused_at = Some(Instant::now());
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let read = match result {
            Ok(read) => read,
            Err(why) => {
                let text = format!("Couldn't read the code of \u{201c}{title}\u{201d}: {why}");
                let why = format!("couldn't read the code: {why}");
                return self.refuse_review(project, slug, stamp, why, text, true);
            }
        };
        let running = feature.status == Status::Open
            && feature.current() == Some(Stage::AiReview)
            && feature.runs.get(&Stage::AiReview).is_some_and(|r| r.n == n);
        if !running {
            return;
        }
        let mut changed: Vec<String> = read.reading.dirty.clone();
        changed.extend(read.reading.stray.iter().cloned());
        changed.sort();
        changed.dedup();
        if !changed.is_empty() {
            let repos = changed.join(", ");
            let text = format!(
                "The AI review agent changed the code in {repos}; undo its changes for its findings to be read, or start a fresh agent"
            );
            let why = format!("the agent changed the code in {repos}");
            return self.refuse_review(project, slug, stamp, why, text, false);
        }
        let patch = read.reading.patch.clone();
        let verified = feature
            .marks
            .get(&Stage::Verify)
            .and_then(|m| m.patch.as_deref());
        if verified != Some(patch.as_str()) {
            // Reviewed on other code than Verify passed: that code goes
            // through Build and Verify, and is reviewed again.
            if let Some(f) = self.feature_mut(project, slug) {
                f.log(
                    feature::now(),
                    "AI review not read: the code changed since Verify",
                );
                self.save_feature(project, slug);
            }
            self.features.agents.remove(&key);
            self.feature_code_is(project, slug, &patch);
            let still = self
                .feature(project, slug)
                .is_some_and(|f| f.current() == Some(Stage::AiReview));
            if still {
                self.show_warning(format!(
                    "The code of \u{201c}{title}\u{201d} changed since Verify; mark Verify again"
                ));
            }
            return;
        }
        let raws = match read
            .text
            .clone()
            .and_then(|text| review_import::parse(&text).map_err(|lacks| lacks.join("; ")))
        {
            Ok(raws) => raws,
            Err(why) => {
                let text = format!("Couldn't read AI review's findings: {why}");
                let why = format!("couldn't read {}: {why}", review_import::REVIEW);
                return self.refuse_review(project, slug, stamp, why, text, true);
            }
        };
        if let Some(watch) = self.features.agents.get_mut(&key) {
            watch.refused = None;
        }
        let repos: Vec<(String, PathBuf)> = feature
            .touched()
            .map(|r| (r.key(), r.worktree.clone()))
            .collect();
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let review = f.review.get_or_insert_with(AiReview::default);
        let got = review_import::import(review, n, raws, &repos, &read.diff);
        review.reviewed = read.heads.clone();
        review.patch = Some(patch.clone());
        let waiting = waiting_text(&title, review);
        let pane = f.runs.get_mut(&Stage::AiReview).and_then(|r| {
            r.ready = Some(now);
            r.pane.clone()
        });
        f.log(
            now,
            format!(
                "AI review run {n} read: {} new, {} raised again, {} fixed",
                got.new, got.again, got.fixed
            ),
        );
        let Some(summary) = waiting else {
            f.tick(now, "gerdr", Some(&patch));
            self.save_feature(project, slug);
            self.features.agents.remove(&key);
            self.feature_cursor_to_current_of(project, slug);
            let summary = if got.fixed > 0 {
                format!("AI review of \u{201c}{title}\u{201d}: every finding fixed")
            } else {
                format!("AI review of \u{201c}{title}\u{201d} found nothing to fix")
            };
            self.notify_feature(project, slug, pane, &summary);
            return self.show_success(summary);
        };
        self.save_feature(project, slug);
        self.notify_feature(project, slug, pane, &summary);
        self.show_warning(summary);
    }

    /// Decide finding `id` of feature `slug`'s AI review: Fix, Dismiss
    /// with a reason, or back to undecided. Saves the record; tells the
    /// user why when it can't (the feature isn't at AI review, no such
    /// finding, an empty reason). Once no finding holds up, AI review is
    /// marked done on the code as it is.
    pub fn decide_finding(&mut self, project: &str, slug: &str, id: &str, decision: Decision) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let title = f.title.clone();
        if f.status != Status::Open || f.current() != Some(Stage::AiReview) {
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
        }
        let Some(review) = f.review.as_mut() else {
            return self.show_warning(format!("There's no finding {id}"));
        };
        let what = match review_import::decide(review, id, &decision) {
            Ok(what) => what,
            Err(why) => return self.show_warning(why),
        };
        f.log(feature::now(), format!("Finding {id}: {what}"));
        self.save_feature(project, slug);
        let word = match decision {
            Decision::Fix => "marked to fix",
            Decision::Dismiss(_) => "dismissed",
            Decision::Reopen => "reopened",
        };
        self.after_decision(project, slug, format!("Finding {id} {word}"), None);
    }

    /// What comes of a change to feature `slug`'s findings, said in `done`,
    /// with `undo` on its toast: nothing left, and AI review is done; nothing
    /// left to decide, and the fixes are offered. An offer has its own button,
    /// so only the toast that says `done` can undo.
    fn after_decision(
        &mut self,
        project: &str,
        slug: &str,
        done: String,
        undo: Option<MenuAction>,
    ) {
        let Some(review) = self.feature(project, slug).and_then(|f| f.review.as_ref()) else {
            return;
        };
        let open = review.open();
        let fix = review.count(FindingState::Fix);
        let undecided = review.count(FindingState::Open);
        let said = |this: &mut Self, done: String, undo: Option<MenuAction>| {
            let button = undo.map(|undo| ("Undo".to_owned(), undo));
            this.toast_with(done, Level::Success, button);
        };
        if open == 0 {
            said(self, done, undo);
            return self.read_and_tick(project, slug, Stage::AiReview);
        }
        if undecided == 0 && fix > 0 && undo.is_none() {
            return self.offer_fixes(
                project,
                slug,
                format!("All findings decided: {} to fix", plural(fix, "finding")),
            );
        }
        said(self, done, undo);
    }

    /// Say `text`, with a button for what comes next: nothing is left to
    /// decide of feature `slug`'s AI review. Nothing starts by itself.
    fn offer_fixes(&mut self, project: &str, slug: &str, text: String) {
        let (project, slug) = (project.to_owned(), slug.to_owned());
        let button = match self.findings_need_plan(&project, &slug) {
            true => (
                "Plan fixes".to_owned(),
                MenuAction::PlanFixes(project, slug),
            ),
            false => (
                "Make fixes".to_owned(),
                MenuAction::MakeFindingFixes(project, slug),
            ),
        };
        self.toast_with(text, Level::Success, Some(button));
    }

    /// Whether the findings to fix of feature `slug` need a fix plan, as far
    /// as is known without reading a file.
    fn findings_need_plan(&self, project: &str, slug: &str) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            return false;
        };
        let findings = feature.review.iter().flat_map(|r| &r.findings);
        review_import::needs_plan(findings, self.row_ctx(feature).comments)
    }

    /// Mark every undecided finding of feature `slug`'s AI review to fix.
    pub(crate) fn fix_all_findings(&mut self, project: &str, slug: &str) {
        let mut marked = 0;
        let done = self.change_review(project, slug, |review| {
            let open: Vec<String> = (review.findings.iter())
                .filter(|f| f.state == FindingState::Open)
                .map(|f| f.id.clone())
                .collect();
            if open.is_empty() {
                return Err("No finding left to decide".to_owned());
            }
            for id in &open {
                review_import::decide(review, id, &Decision::Fix)?;
            }
            marked = open.len();
            Ok(format!("Marked to fix: {}", open.join(", ")))
        });
        if done {
            let text = format!("{} marked to fix", plural(marked, "finding"));
            self.offer_fixes(project, slug, text);
        }
    }

    /// Change feature `slug`'s AI review by `what` and keep it: `change`
    /// says what it did, for the log, else why it can't, and so does the
    /// toast. Whether it did.
    fn change_review(
        &mut self,
        project: &str,
        slug: &str,
        change: impl FnOnce(&mut AiReview) -> Result<String, String>,
    ) -> bool {
        let Some(f) = self.feature_mut(project, slug) else {
            return false;
        };
        let title = f.title.clone();
        if f.status != Status::Open || f.current() != Some(Stage::AiReview) {
            self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
            return false;
        }
        let Some(review) = f.review.as_mut() else {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} has no findings read in yet"
            ));
            return false;
        };
        let what = match change(review) {
            Ok(what) => what,
            Err(why) => {
                self.show_warning(why);
                return false;
            }
        };
        f.log(feature::now(), what);
        self.save_feature(project, slug);
        self.feature_findings_changed(project, slug);
        true
    }

    /// Set the user's comment on finding `id`; empty deletes it. Whether it
    /// was set: why not is said.
    pub(crate) fn note_finding(&mut self, project: &str, slug: &str, id: &str, text: &str) -> bool {
        // Left as it was: nothing to save or log.
        let review = self.feature(project, slug).and_then(|f| f.review.as_ref());
        if review
            .and_then(|r| r.finding(id))
            .is_some_and(|f| f.note == text.trim())
        {
            return true;
        }
        self.change_review(project, slug, |review| {
            let what = review_import::set_note(review, id, text)?;
            Ok(format!("Finding {id}: {what}"))
        })
    }

    /// Delete the user's comment on finding `id`: the toast can bring it back
    /// (checklist C4).
    pub(crate) fn delete_finding_comment(&mut self, project: &str, slug: &str, id: &str) {
        let review = self.feature(project, slug).and_then(|f| f.review.as_ref());
        let note = review.and_then(|r| r.finding(id)).map(|f| f.note.clone());
        let Some(note) = note.filter(|note| !note.is_empty()) else {
            return;
        };
        if !self.note_finding(project, slug, id, "") {
            return;
        }
        let undo = MenuAction::RestoreFindingComment {
            project: project.to_owned(),
            slug: slug.to_owned(),
            id: id.to_owned(),
            note,
        };
        self.toast_with(
            format!("Comment on {id} deleted"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// Delete item `id` of the user's own from feature `slug`'s AI review: it
    /// goes for good, not dismissed. The toast can bring it back (checklist C4).
    pub(crate) fn delete_own_finding(&mut self, project: &str, slug: &str, id: &str) {
        let selected = self.selected_review_card();
        let mut gone = None;
        let done = self.change_review(project, slug, |review| {
            gone = Some(review_import::delete_own(review, id)?);
            Ok(format!("You deleted comment {id}"))
        });
        let Some((at, finding)) = gone.filter(|_| done) else {
            return;
        };
        self.feature_findings_changed(project, slug);
        self.reselect_review_card(selected);
        let undo = MenuAction::RestoreOwnFinding {
            project: project.to_owned(),
            slug: slug.to_owned(),
            at,
            finding: Box::new(finding),
        };
        self.after_decision(project, slug, format!("Comment {id} deleted"), Some(undo));
    }

    /// Undo `delete_own_finding`. Past AI review `change_review` refuses, and
    /// says why: its findings are settled.
    pub(crate) fn restore_own_finding(
        &mut self,
        project: &str,
        slug: &str,
        at: usize,
        finding: review_import::Finding,
    ) {
        let id = finding.id.clone();
        let selected = self.selected_review_card();
        let done = self.change_review(project, slug, |review| {
            review_import::restore_own(review, at, finding);
            Ok(format!("You brought back comment {id}"))
        });
        if done {
            self.feature_findings_changed(project, slug);
            self.reselect_review_card(selected);
        }
    }

    /// Change the comments the user left in feature `slug`'s diff with
    /// `change`: what it made of them, if it changed any. They go to the open
    /// diff if it is that feature's, else to their file; a failure to save says
    /// so. The page shows the change at once, with no wait for the next probe.
    fn change_diff_notes<R>(
        &mut self,
        project: &str,
        slug: &str,
        change: impl FnOnce(&mut Vec<review::Comment>) -> Option<R>,
    ) -> Option<R> {
        let selected = self.selected_review_card();
        let env = self.worktree_env();
        let open = self.wd_mut().filter(|wd| {
            (wd.feature.as_ref()).is_some_and(|fe| fe.project == project && fe.slug == slug)
        });
        let (done, own) = if let Some(wd) = open {
            let mut own = wd.diff.review.without_findings().comments;
            let done = change(&mut own)?;
            wd.diff.review.comments.clone_from(&own);
            wd.diff.review_dirty = true;
            wd.place_findings(env);
            self.feature_save_review();
            (done, own)
        } else {
            let path = store::dir(None, project, slug).join(COMMENTS);
            let mut review = review::load_file(&path);
            let done = change(&mut review.comments)?;
            if let Err(err) = review::save_file(&path, &review) {
                tracing::warn!("saving {}: {err}", path.display());
                self.show_error(format!("Couldn't save your comments: {err}"));
                return None;
            }
            (done, review.comments)
        };
        self.notes_are(project, slug, own);
        self.reselect_review_card(selected);
        self.probe_feature(project, slug);
        Some(done)
    }

    /// Delete comment `c` the user left in feature `slug`'s diff: the toast
    /// can bring it back (checklist C4).
    pub(crate) fn delete_diff_note(&mut self, project: &str, slug: &str, c: &review::Comment) {
        let at = self.change_diff_notes(project, slug, |own| {
            let at = own.iter().position(|x| x == c)?;
            own.remove(at);
            Some(at)
        });
        let Some(at) = at else {
            return self.show_warning("That comment is gone already".into());
        };
        let undo = MenuAction::RestoreDiffNote {
            project: project.to_owned(),
            slug: slug.to_owned(),
            at,
            comment: Box::new(c.clone()),
        };
        self.toast_with(
            "Comment deleted".to_owned(),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// Undo `delete_diff_note`.
    pub(crate) fn restore_diff_note(
        &mut self,
        project: &str,
        slug: &str,
        at: usize,
        comment: review::Comment,
    ) {
        self.change_diff_notes(project, slug, |own| {
            let at = at.min(own.len());
            own.insert(at, comment);
            Some(())
        });
    }

    /// Make comment `c` the user left in feature `slug`'s diff say `text`.
    pub(crate) fn edit_diff_note(
        &mut self,
        project: &str,
        slug: &str,
        c: &review::Comment,
        text: &str,
    ) {
        let text = text.trim().to_owned();
        let done = self.change_diff_notes(project, slug, |own| {
            own.iter_mut().find(|x| *x == c).map(|x| x.text = text)
        });
        match done {
            Some(()) => self.show_success("Comment saved, to go to Build with the fixes".into()),
            None => self.show_warning("That comment is gone already".into()),
        }
    }

    /// Show comment `c` the user left in feature `slug`'s diff.
    pub(crate) fn show_diff_note(&mut self, project: &str, slug: &str, c: review::Comment) {
        self.open_feature_diff(project, slug, None);
        let sent = self.sent_notes(project, slug);
        if let Some(fe) = (self.wd_mut().and_then(|wd| wd.feature.as_mut()))
            .filter(|fe| fe.project == project && fe.slug == slug)
        {
            fe.sent_notes = sent;
            // Its diff is being read: the cursor goes there once it is. One
            // that went to Build is found by id, as a finding is.
            match c.flag {
                Some(flag) => fe.focus = Some(flag.id),
                None => fe.focus_note = Some(c),
            }
        }
        // On the page it may have been read already.
        let env = self.worktree_env();
        if let Some(wd) = self.inline_wd_mut().filter(|wd| wd.read) {
            wd.place_findings(env);
        }
    }

    /// Write or edit the user's comment on finding `id`, on its card on the
    /// page: the keyboard goes to it there.
    pub(crate) fn write_finding_comment(&mut self, project: &str, slug: &str, id: &str) {
        let Some(f) = self.finding_to_ask(project, slug, id) else {
            return;
        };
        if let Err(why) = review_import::note_editable(&f.id, f.state) {
            return self.show_warning(why);
        }
        let mut field = TextField::default();
        field.set(&f.note);
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        panel.note = Some((CardNote::Finding(id.to_owned()), field));
        self.stop_key_repeat();
        self.select_panel_finding(id);
    }

    /// Reword finding `id`'s fix.
    pub(crate) fn edit_finding(&mut self, project: &str, slug: &str, id: &str, fix: &str) {
        let done = self.change_review(project, slug, |review| {
            let what = review_import::edit(review, id, fix)?;
            Ok(format!("Finding {id}: {what}"))
        });
        if !done {
            return;
        }
        let review = self.feature(project, slug).and_then(|f| f.review.as_ref());
        let left = review.map_or(1, |r| r.count(FindingState::Open));
        let text = format!("Finding {id} edited and marked to fix");
        match left {
            0 => self.offer_fixes(project, slug, text),
            _ => self.show_success(text),
        }
    }

    /// Rewrite item `id` of the user's own.
    pub(crate) fn edit_own_finding(&mut self, project: &str, slug: &str, id: &str, text: &str) {
        let done = self.change_review(project, slug, |review| {
            let what = review_import::edit_own(review, id, text)?;
            Ok(format!("Your comment {id}: {what}"))
        });
        if done {
            self.show_success(format!("Comment {id} edited"));
        }
    }

    /// Add an item of the user's own to feature `slug`'s AI review, to go
    /// to Build with the findings to fix.
    pub(crate) fn add_finding_comment(&mut self, project: &str, slug: &str, text: &str) {
        // Past AI review, its findings are settled: with the user's others.
        let stage = self.feature(project, slug).and_then(Feature::current);
        if stage == Some(Stage::CodeReview) {
            return self.add_general_comment(project, slug, text);
        }
        let mut added = String::new();
        let done = self.change_review(project, slug, |review| {
            added = review_import::add(review, text)?;
            Ok(format!("You added comment {added}"))
        });
        if done {
            if let Some(fe) = (self.wd_mut().and_then(|wd| wd.feature.as_mut()))
                .filter(|fe| fe.project == project && fe.slug == slug)
            {
                fe.focus = Some(added.clone());
            }
            self.feature_findings_changed(project, slug);
            // It lands at the top of the page's list.
            self.select_panel_finding(&added);
            self.show_success(format!(
                "Comment {added} added, to go to Build with the fixes"
            ));
        }
    }

    /// Send feature `slug`'s AI review findings the user wants fixed to
    /// its Build agent, through a fix plan, once none waits to be decided,
    /// with the comments they left in its diff; they are sent then, and the
    /// next review checks them.
    pub fn send_findings(&mut self, project: &str, slug: &str) {
        self.findings_go(project, slug, false);
    }

    /// The same with no fix plan: the Build agent makes the fixes at once.
    /// What the user wrote always goes through a plan, even if it came since
    /// the button was shown (`review_import::needs_plan`).
    pub fn make_finding_fixes(&mut self, project: &str, slug: &str) {
        self.findings_go(project, slug, true);
    }

    fn findings_go(&mut self, project: &str, slug: &str, unplanned: bool) {
        // A toast's button can come while the open diff has comments unsaved.
        self.feature_save_review();
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let title = f.title.clone();
        if f.status != Status::Open || f.current() != Some(Stage::AiReview) {
            return self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
        }
        let Some(review) = f.review.as_ref() else {
            return self.show_warning(format!("\u{201c}{title}\u{201d} has no findings to fix"));
        };
        let undecided = review.count(FindingState::Open);
        if undecided > 0 {
            return self.show_warning(decide_first(undecided));
        }
        let ids: Vec<String> = (review.findings.iter())
            .filter(|f| f.state == FindingState::Fix)
            .map(|f| f.id.clone())
            .collect();
        let (n, mut text) = review_import::fixes_markdown(review, &title, f.many_repos());
        if n == 0 {
            return self.show_warning(format!("\u{201c}{title}\u{201d} has no findings to fix"));
        }
        let path = store::dir(None, project, slug).join(COMMENTS);
        let own = review::load_file(&path);
        let comments = own.own();
        let mut what = plural(n, "finding");
        if comments > 0 {
            let names = self.projects.names(f);
            let roots: Vec<(PathBuf, String)> = f
                .touched()
                .map(|r| (r.worktree.clone(), names.label(r)))
                .collect();
            text.push_str(&own_comments_markdown(&own, &roots));
            what.push_str(&format!(" and {}", plural(comments, "comment")));
        }
        let plan = (unplanned && !review_import::needs_plan(&review.findings, comments))
            .then(|| review_import::fixes_plan(review, &title, f.many_repos()));
        let unplanned = plan.is_some();
        let started = match plan {
            Some(plan) => self
                .fix_input(project, slug, &what, &text)
                .is_some_and(|input| {
                    self.start_unplanned_fix_round(
                        project,
                        slug,
                        Stage::AiReview,
                        &what,
                        input,
                        &plan,
                    )
                }),
            None => self.send_to_build(project, slug, Stage::AiReview, &what, text),
        };
        if !started {
            return;
        }
        // Sent, so they don't wait for Code review as well.
        let kept = (comments > 0)
            .then(|| self.keep_comments(project, slug))
            .flatten();
        self.fix_round_gives_back(
            project,
            slug,
            GiveBack::Findings {
                ids,
                comments: kept,
            },
        );
        if comments > 0 {
            if let Err(err) = review::save_file(&path, &review::Review::default()) {
                tracing::warn!("deleting {}: {err}", path.display());
                self.show_error(format!("Couldn't delete the sent comments: {err}"));
            }
            // Their cards go with them, with no wait for the probe.
            self.notes_are(project, slug, Vec::new());
            self.probe_feature(project, slug);
        }
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if let Some(review) = f.review.as_mut() {
            for finding in &mut review.findings {
                if finding.state == FindingState::Fix {
                    finding.state = FindingState::Sent;
                    finding.made = false;
                }
            }
        }
        self.save_feature(project, slug);
        // From its diff: that closes, what it sent gone from it, as the
        // Build agent's tab shows.
        let open = (self.feature_of()).is_some_and(|fe| fe.project == project && fe.slug == slug);
        if open {
            if let Some(wd) = self.wd_mut() {
                wd.diff.review.comments.retain(|c| c.flag.is_some());
                wd.diff.review_dirty = false;
            }
            self.close_feature_diff(false);
        }
        // Not sent: its plan waits on the page, with what was said of why.
        if !unplanned || self.make_unplanned_fixes(project, slug) {
            self.show_build_agent(project, slug);
        }
    }

    /// Keep a copy of feature `slug`'s Code review comments, for its fix
    /// round to give back: where, if it could be made. A failure says so.
    pub(crate) fn keep_comments(&mut self, project: &str, slug: &str) -> Option<PathBuf> {
        let data = store::dir(None, project, slug);
        let n = self.feature(project, slug)?.fix_rounds;
        let copy = crate::feature::fix_round::comments_copy(&data, n);
        match std::fs::copy(data.join(COMMENTS), &copy) {
            Ok(_) => Some(copy),
            Err(err) => {
                tracing::warn!("keeping the comments of {project}/{slug}: {err}");
                self.show_warning(format!(
                    "Couldn't keep a copy of the comments: dropping the plan won't bring them back ({err})"
                ));
                None
            }
        }
    }

    /// Show feature `slug`'s Build agent, just sent work: its tab is where
    /// it runs, or waits for its prompt. A T3 Code draft shows itself
    /// (`t3_stage_drafted`).
    pub(crate) fn show_build_agent(&mut self, project: &str, slug: &str) {
        let Some(run) = (self.feature(project, slug)).and_then(|f| f.runs.get(&Stage::Build))
        else {
            return;
        };
        if run.t3_thread().is_some() {
            return self.show_t3_stage(project, slug, Stage::Build);
        }
        if let Some(pane) = (run.pane.clone()).filter(|p| self.pane_there(p)) {
            self.focus_agent(&crate::agents::AgentKey::Herdr(pane));
        }
    }

    /// Start a fix round on `text` (Markdown) of feature `slug`, for its
    /// Build agent: see `feature_fix`. `asked` is the stage sending it, and
    /// `what` names it for the log, the toasts and the prompts: "review
    /// comments", "2 findings". Whether it
    /// started: a refusal says why, and nothing is taken.
    ///
    /// It goes to `prompts/build-<n>-<what>-<k>.md`, and the agent is told
    /// where. Build stays done: the fixes' commits send the feature back to
    /// it, and gerdr reads the code once the agent rests.
    pub(crate) fn send_to_build(
        &mut self,
        project: &str,
        slug: &str,
        asked: Stage,
        what: &str,
        text: String,
    ) -> bool {
        self.fix_input(project, slug, what, &text)
            .is_some_and(|input| self.start_fix_round(project, slug, asked, what, input))
    }

    /// Where `text` went for a new fix round of feature `slug`: none if a
    /// round can't start now, as is said.
    pub(crate) fn fix_input(
        &mut self,
        project: &str,
        slug: &str,
        what: &str,
        text: &str,
    ) -> Option<PathBuf> {
        let feature = self.feature(project, slug)?.clone();
        let title = &feature.title;
        if feature.status != Status::Open || !feature.is_done(Stage::Build) {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} isn't built yet; nothing to send {what} to"
            ));
            return None;
        }
        if let Some(why) = self.fix_round_refusal(&feature) {
            self.show_warning(why);
            return None;
        }
        let data = store::dir(None, project, slug);
        let dir = data.join("prompts");
        let n = feature.runs.get(&Stage::Build).map_or(0, |r| r.n);
        let key = file_key(what);
        let path = (1..)
            .map(|k| dir.join(format!("build-{n}-{key}-{k}.md")))
            .find(|p| !p.exists())
            .unwrap_or_default();
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text));
        if let Err(err) = written {
            tracing::warn!("writing {}: {err}", path.display());
            self.show_error(format!("Couldn't write {}: {err}", path.display()));
            return None;
        }
        Some(path)
    }

    /// Start a fresh Build agent with `extra` on feature `slug`, whose Build
    /// stays done: what it fixes sends the feature back to Build by its
    /// commits, and its run is seen as back there. Whether it started; if
    /// not it said why.
    pub(crate) fn launch_fresh_build(
        &mut self,
        project: &str,
        slug: &str,
        extra: &[(&str, String)],
    ) -> bool {
        let build_at = (self.feature(project, slug))
            .and_then(|f| f.marks.get(&Stage::Build))
            .map(|m| m.at);
        if !self.launch_stage_agent(project, slug, Stage::Build, extra) {
            return false;
        }
        if let Some(r) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&Stage::Build))
        {
            r.ready = build_at;
        }
        self.save_feature(project, slug);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts;
    use crate::{
        feature::review_import::{Raw, Severity},
        features::tests::{feature, repo},
    };

    fn reviewed() -> Feature {
        let mut f = feature("a", vec![repo("api", true), repo("web", true)], 5);
        let mut review = AiReview::default();
        let raw = |title: &str| Raw {
            file: "src/a.rs".into(),
            repo: Some("api".into()),
            line: Some(3),
            title: title.into(),
            severity: Severity::High,
            ..Raw::default()
        };
        review_import::import(
            &mut review,
            1,
            vec![raw("sent"), raw("dismissed"), raw("open")],
            &[],
            &Default::default(),
        );
        review.findings[0].state = FindingState::Sent;
        review.findings[1].state = FindingState::Dismissed;
        review.findings[1].reason = "by design".into();
        review.reviewed.insert("api".into(), "f00d".into());
        f.review = Some(review);
        f
    }

    #[test]
    fn the_first_review_reads_the_whole_change() {
        let f = feature("a", vec![repo("api", true), repo("web", false)], 5);
        let v: prompts::render::Vars = review_vars(&f).into_iter().collect();
        assert_eq!(
            v["review.diffs"],
            "   - api, in /work/.features/bulk/api: `git diff abc123...HEAD`"
        );
        assert_eq!(v["review.sent"], "");
        assert_eq!(v["review.open"], "");
    }

    #[test]
    fn the_diffs_comments_follow_the_fixes() {
        let own = review::Review {
            comments: vec![review::Comment {
                root: "/w/api".into(),
                path: "src/a.rs".into(),
                text: "This name is wrong".into(),
                ..review::Comment::default()
            }],
            ..review::Review::default()
        };
        let md = own_comments_markdown(&own, &[("/w/api".into(), "api".to_owned())]);
        assert!(md.starts_with("\n---\n\n"), "{md}");
        for words in ["## api (/w/api)", "src/a.rs", "This name is wrong"] {
            assert!(md.contains(words), "{words}: {md}");
        }
        assert!(!md.contains("code changed since"), "{md}");
    }

    #[test]
    fn the_next_review_reads_the_users_words() {
        let f = Finding {
            id: "r1-2".into(),
            file: "src/a.rs".into(),
            title: "Off by one".into(),
            fix: "Reviewer's".into(),
            my_fix: Some("Mine\ntoo".into()),
            note: "Careful".into(),
            ..Finding::default()
        };
        let line = finding_line(&f, false);
        assert!(line.contains("The user wants this fix: Mine too"), "{line}");
        assert!(line.contains("The user's comment: Careful"), "{line}");
        assert!(
            !line.contains('\n') && !line.contains("Reviewer's"),
            "{line}"
        );
        let yours = Finding {
            id: "u1".into(),
            title: "Rename it".into(),
            body: "It reads badly".into(),
            yours: true,
            ..Finding::default()
        };
        assert_eq!(
            finding_line(&yours, false),
            "- u1 (added by the user): Rename it. It reads badly"
        );
    }

    #[test]
    fn a_later_review_reads_what_changed_and_the_earlier_findings() {
        let v: prompts::render::Vars = review_vars(&reviewed()).into_iter().collect();
        assert!(
            v["review.diffs"].contains("api, in /work/.features/bulk/api: `git diff f00d..HEAD`")
        );
        assert!(v["review.diffs"].contains("`git diff abc123...HEAD` shows the whole change"));
        assert!(
            v["review.diffs"]
                .contains("web, in /work/.features/bulk/web: `git diff abc123...HEAD`")
        );
        assert_eq!(v["review.sent"], "- r1-1 (high, api/src/a.rs:3): sent");
        assert_eq!(
            v["review.dismissed"],
            "- r1-2 (high, api/src/a.rs:3): dismissed. Dismissed because: by design"
        );
        assert_eq!(v["review.open"], "- r1-3 (high, api/src/a.rs:3): open");
        let mut all = crate::feature_agents::vars(
            &reviewed(),
            &crate::feature::PerProject::all("Acme".to_owned()),
            std::path::Path::new("/d"),
            Stage::AiReview,
        );
        all.extend(review_vars(&reviewed()));
        let text = prompts::render_builtin("ai-review", &all).unwrap();
        assert!(text.contains("/d/review.json") && text.contains("by design"));
    }

    #[test]
    fn what_is_sent_names_its_file() {
        assert_eq!(file_key("2 findings"), "findings");
        assert_eq!(file_key("review comments"), "review-comments");
        assert_eq!(file_key("42"), "fixes");
    }

    #[test]
    fn the_toast_says_what_waits() {
        let mut review = reviewed().review.unwrap();
        assert_eq!(
            waiting_text("T", &review).as_deref(),
            Some("AI review of \u{201c}T\u{201d}: 1 finding to decide")
        );
        review.findings[2].state = FindingState::Fix;
        assert_eq!(
            waiting_text("T", &review).as_deref(),
            Some("AI review of \u{201c}T\u{201d}: 1 finding to fix")
        );
        review.findings[2].state = FindingState::Dismissed;
        assert_eq!(waiting_text("T", &review), None);
    }
}
