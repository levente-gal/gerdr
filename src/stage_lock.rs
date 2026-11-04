//! A done stage is closed: it can be read, and reopened only while the
//! stage after it has made nothing from it. Once that one has its file, its
//! run or its record, the earlier one is locked until the later one starts
//! over, which moves what it wrote to the archive folder and forgets what it
//! recorded (see `Feature::locked_by`, `artifacts::archive_stage`).
//!
//! The panel and the tabs go by the latest probe (`page::Docs`), a few
//! seconds old; what changes something here reads the files first.

use crate::{
    Gerdr,
    confirm::Then,
    feature::{self, Feature, Stage, Status, artifacts, store},
    features::lock_why,
    review::Review,
    toast::Level,
    worktree_diff::DocOf,
};

/// What refusing to change locked `stage` of `feature` says: why, and what
/// frees it. The later stage `by` reopens first when it is done, else it
/// starts over.
fn locked_warning(feature: &Feature, stage: Stage, by: Stage) -> String {
    let what = match feature.is_done(by) {
        true => "reopen",
        false => "start",
    };
    let over = if feature.is_done(by) { "" } else { " over" };
    format!(
        "{} is locked: {}; {what} {}{over} first",
        stage.label(),
        lock_why(feature, by),
        by.label(),
    )
}

/// What the dialog before starting `stage` over says goes, `live`: its
/// agent's tab is there.
fn start_over_body(stage: Stage, live: bool) -> String {
    let file = artifacts::name(stage).unwrap_or("Its file");
    let mut body = match stage {
        Stage::Discover | Stage::Plan => format!("{file} moves to the archive folder."),
        Stage::PlanReview => {
            "Its suggestions and decisions are forgotten and its files move to the archive folder."
                .to_owned()
        }
        Stage::Build => format!("{file} moves to the archive folder. Its commits stay."),
        Stage::Verify => format!(
            "Its run is forgotten and {} moves to the archive folder.",
            feature::verify::EVIDENCE
        ),
        Stage::AiReview => {
            format!(
                "Its findings and decisions are forgotten and {file} moves to the archive folder."
            )
        }
        Stage::CodeReview => {
            "Its viewed files are forgotten and its comments move to the archive folder.".to_owned()
        }
        Stage::ManualTest => "Its test results are forgotten.".to_owned(),
        Stage::Ship => "gerdr forgets its pull requests; they stay open on GitHub.".to_owned(),
    };
    if live {
        body.push_str(" Its agent stops.");
    }
    body
}

impl Gerdr {
    /// The later stage that locks done `stage` of feature `slug`, from
    /// what is on disk now.
    fn stage_locked_by(&self, project: &str, slug: &str, stage: Stage) -> Option<Stage> {
        let feature = self.feature(project, slug)?;
        let outputs = artifacts::outputs(&store::dir(None, project, slug));
        feature.locked_by(stage, |s| outputs.contains(&s))
    }

    /// What refusing to change `stage` of feature `slug` says, when it is
    /// locked.
    pub(crate) fn stage_lock_warning(
        &self,
        project: &str,
        slug: &str,
        stage: Stage,
    ) -> Option<String> {
        let by = self.stage_locked_by(project, slug, stage)?;
        Some(locked_warning(self.feature(project, slug)?, stage, by))
    }

    /// `stage`'s agent pane of feature `slug`, if it is still there.
    fn live_stage_pane(&self, project: &str, slug: &str, stage: Stage) -> Option<String> {
        let run = self.feature(project, slug)?.runs.get(&stage)?;
        let pane = run.pane.clone()?;
        self.herdr_pane_place(&pane).map(|_| pane)
    }

    /// What to say when `stage`'s agent of feature `slug` is at work in a
    /// T3 Code thread that gerdr lets go of: T3 Code's command line has no
    /// way to stop one.
    pub(crate) fn t3_left_running(
        &self,
        project: &str,
        slug: &str,
        stage: Stage,
    ) -> Option<String> {
        let run = self.feature(project, slug)?.runs.get(&stage)?;
        let (status, _) = self.t3_thread_state(run.t3_thread()?);
        matches!(
            status,
            Some(crate::herdr::AgentStatus::Working | crate::herdr::AgentStatus::Blocked)
        )
        .then(|| {
            format!(
                "{}'s T3 Code thread keeps running: stop it in T3 Code",
                stage.label()
            )
        })
    }

    /// Reopen done `stage` of feature `slug`, unless a later stage locks
    /// it. The next stage's agent has written nothing then, or it would
    /// lock it, but it stops: that is asked first.
    pub(crate) fn reopen_stage(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if !feature.is_done(stage) {
            return self.show_warning(format!("{} isn't done", stage.label()));
        }
        if let Some(why) = self.stage_lock_warning(project, slug, stage) {
            return self.show_warning(why);
        }
        let running = stage
            .next()
            .filter(|next| self.live_stage_pane(project, slug, *next).is_some());
        let Some(next) = running else {
            return self.reopen_stage_now(project, slug, stage);
        };
        let body = format!(
            "{}'s agent is running and stops; it has written nothing yet.",
            next.label()
        );
        let then = Then::ReopenStage {
            project: project.to_owned(),
            slug: slug.to_owned(),
            stage,
        };
        self.ask(format!("Reopen {}?", stage.label()), body, "Reopen", then);
    }

    /// Reopen done `stage` of feature `slug`, the next stage's agent
    /// stopped: it starts afresh once it is reached again.
    pub(crate) fn reopen_stage_now(&mut self, project: &str, slug: &str, stage: Stage) {
        if let Some(why) = self.stage_lock_warning(project, slug, stage) {
            return self.show_warning(why);
        }
        let next = stage.next();
        if let Some(pane) = next.and_then(|next| self.live_stage_pane(project, slug, next)) {
            self.close_herdr_pane(&pane);
        }
        let left = next.and_then(|next| self.t3_left_running(project, slug, next));
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if !feature.untick(stage, now) {
            return self.show_warning(format!("{} isn't done", stage.label()));
        }
        if let Some(next) = next {
            feature.forget_run(next);
            let key = (project.to_owned(), slug.to_owned(), next);
            self.features.agents.remove(&key);
        }
        artifacts::forget_approved(&store::dir(None, project, slug), stage);
        self.save_feature(project, slug);
        self.show_success(format!("{} reopened", stage.label()));
        if let Some(left) = left {
            self.show_warning(left);
        }
        self.probe_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
    }

    /// Why current `stage` of feature `slug` can't start over now.
    fn start_over_refused(&self, project: &str, slug: &str, stage: Stage) -> Option<String> {
        let Some(feature) = self.feature(project, slug) else {
            return Some("That feature is gone".into());
        };
        if feature.status != Status::Open {
            return Some("The feature is closed".into());
        }
        if feature.current() != Some(stage) {
            return Some(format!("{} isn't the current stage", stage.label()));
        }
        // What runs would write its result into the stage started over.
        self.feature_busy(project, slug)
            .then(|| format!("{} has work running: wait for it", feature.title))
    }

    /// Ask before starting current `stage` of feature `slug` over.
    pub(crate) fn ask_start_over(&mut self, project: &str, slug: &str, stage: Stage) {
        if let Some(why) = self.start_over_refused(project, slug, stage) {
            return self.show_warning(why);
        }
        let live = self.live_stage_pane(project, slug, stage).is_some();
        let then = Then::StartOverStage {
            project: project.to_owned(),
            slug: slug.to_owned(),
            stage,
        };
        self.ask(
            format!("Start {} over?", stage.label()),
            start_over_body(stage, live),
            "Start over",
            then,
        );
    }

    /// Start current `stage` of feature `slug` over: what it wrote moved
    /// to the archive folder, first as it can fail; its agent stopped; what
    /// it recorded forgotten; what shows its file gone. The stage before it
    /// can be reopened again.
    pub(crate) fn start_over_now(&mut self, project: &str, slug: &str, stage: Stage) {
        // What runs of the stage goes with it, its terminals staying.
        if stage == Stage::Verify {
            self.halt_checks(project, slug, None);
        }
        if let Some(why) = self.start_over_refused(project, slug, stage) {
            return self.show_warning(why);
        }
        let dir = store::dir(None, project, slug);
        let moved = match artifacts::archive_stage(&dir, stage) {
            Ok(moved) => moved,
            Err(why) => {
                tracing::warn!("archiving {project}/{slug}'s {stage:?}: {why}");
                return self.toast(
                    format!("Couldn't start {} over: {why}", stage.label()),
                    Level::Error,
                );
            }
        };
        if let Some(pane) = self.live_stage_pane(project, slug, stage) {
            self.close_herdr_pane(&pane);
        }
        if stage == Stage::PlanReview {
            self.close_parked_reviewer(project, slug);
        }
        if let Some(left) = self.t3_left_running(project, slug, stage) {
            self.show_warning(left);
        }
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.start_over(stage, now);
        let (project_id, slug_id) = (project.to_owned(), slug.to_owned());
        self.features
            .agents
            .remove(&(project_id.clone(), slug_id.clone(), stage));
        if stage == Stage::CodeReview {
            self.code_reviews.remove(&(project_id, slug_id));
            let open = self
                .feature_of()
                .is_some_and(|fe| fe.project == project && fe.slug == slug);
            if open {
                self.close_feature_diff(false);
            }
        }
        self.forget_doc_tabs(project, slug, stage);
        self.save_feature(project, slug);
        self.show_success(match moved.first() {
            Some((name, to)) => format!("{} started over: {name} moved to {to}", stage.label()),
            None => format!("{} started over", stage.label()),
        });
        self.probe_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
    }

    /// `stage`'s file of feature `slug` went to the archive folder: its
    /// tabs here close, and those of other workspaces, with no comments
    /// left to save, say it is gone when they show.
    pub(crate) fn forget_doc_tabs(&mut self, project: &str, slug: &str, stage: Stage) {
        let is_it = |d: &DocOf| d.project == project && d.slug == slug && d.file.stage() == stage;
        let tabs = self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs);
        for wd in tabs.filter_map(|tab| tab.review_mut()).map(|r| &mut r.wd) {
            if wd.doc.as_ref().is_some_and(is_it) {
                wd.diff.review = Review::default();
                wd.diff.review_dirty = false;
            }
        }
        let Some(ws) = self.workspace() else {
            return;
        };
        let here: Vec<usize> = (ws.tabs.iter().enumerate())
            .filter(|(_, tab)| {
                let doc = tab.review().and_then(|r| r.wd.doc.as_ref());
                doc.is_some_and(is_it)
            })
            .map(|(idx, _)| idx)
            .collect();
        for idx in here.into_iter().rev() {
            self.close_tab_now(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_says_why_and_what_frees_it() {
        // Discover done; Plan has its file but isn't done.
        let f = crate::features::tests::feature("login", Vec::new(), 1);
        assert_eq!(
            locked_warning(&f, Stage::Discover, Stage::Plan),
            "Discover is locked: Plan has plan.md; start Plan over first"
        );
        // Plan done as well: it reopens first.
        let f = crate::features::tests::feature("login", Vec::new(), 3);
        assert_eq!(
            locked_warning(&f, Stage::Discover, Stage::Plan),
            "Discover is locked: Plan is done; reopen Plan first"
        );
    }

    #[test]
    fn starting_over_says_what_goes() {
        assert_eq!(
            start_over_body(Stage::Plan, false),
            "plan.md moves to the archive folder."
        );
        assert_eq!(
            start_over_body(Stage::Plan, true),
            "plan.md moves to the archive folder. Its agent stops."
        );
        assert!(start_over_body(Stage::Build, false).contains("Its commits stay"));
        assert!(start_over_body(Stage::Ship, false).contains("stay open on GitHub"));
    }
}
