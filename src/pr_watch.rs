//! Watching the pull requests under review (see `feature::pr`): every
//! `INTERVAL`, off the event loop, gh says how each open review's pull
//! request stands on GitHub, for every project. What changed is kept in
//! its record (`PullRef::remote`) and told once: new commits or another
//! base raise the `NewHead` gate, and the review moves to them only when
//! the user says so (`update_pr_head`); a merged or closed one raises
//! `PrEnded`, and the review can be finished (`finish_pr_review`).
//!
//! Its checks on the head reviewed are Verify's while the worktree is at
//! that head, nothing changed (`RowCtx::on_head`): once they all passed,
//! Verify is marked done by "GitHub CI" (`verify_from_ci`); failed ones run
//! again on GitHub from its page (`rerun_ci`). Local checks run only on
//! code changed here, or when asked to ("Run checks here").

use std::{path::PathBuf, time::Duration};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::{
    Gerdr,
    confirm::Then,
    feature::{self, Status, layout, pr},
    features::{Busy, Done, Job},
    menu::MenuAction,
    toast::Level,
};

/// How often the pull requests under review are read; while checks of one
/// at Verify run, `BUSY`.
const INTERVAL: Duration = Duration::from_secs(60);
const BUSY: Duration = Duration::from_secs(20);

/// Who marks Verify done once GitHub's checks passed.
pub const BY_CI: &str = "GitHub CI";

/// Where a pull request watch can't read GitHub goes (see `problems`).
pub const PROBLEMS: &str = "pull requests";

/// A review whose pull request is to be read: its project and slug, a
/// folder of its repository to run gh in, and the pull request.
type Asked = (String, String, PathBuf, pr::PullRef);

impl crate::features::Features {
    /// How long until the pull requests under review are read again: soon
    /// while one at Verify waits for its checks.
    pub(crate) fn pr_interval(&self) -> Duration {
        let waits = (self.all.values().flatten()).any(|f| {
            f.status == Status::Open
                && f.current() == Some(feature::Stage::Verify)
                && (f.pulls().iter())
                    .filter_map(pr::PullRef::ci)
                    .any(|ci| ci.verdict() == pr::Verdict::Running)
        });
        if waits { BUSY } else { INTERVAL }
    }
}

impl Gerdr {
    /// Read the pull requests under review now, then every `INTERVAL`.
    pub fn watch_pr_reviews(&mut self) {
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), |_, _, state| {
                state.refresh_pr_reviews();
                TimeoutAction::ToDuration(state.features.pr_interval())
            });
        if let Err(err) = timer {
            tracing::warn!("watching pull requests: {err}");
        }
    }

    /// Read how the pull request of each open review written here stands on
    /// GitHub, off the thread, all side by side.
    pub fn refresh_pr_reviews(&mut self) {
        let asked: Vec<Asked> = (self.features.all.iter())
            .flat_map(|(project, all)| all.iter().map(move |f| (project, f)))
            .filter(|(_, f)| f.status == Status::Open && f.writer.is_none())
            .flat_map(|(project, f)| {
                f.pulls().iter().filter_map(move |pull| {
                    let repo = f.repo(&pull.repo).or(f.repos.first())?;
                    Some((
                        project.clone(),
                        f.slug.clone(),
                        repo.main.clone(),
                        pull.clone(),
                    ))
                })
            })
            .collect();
        if asked.is_empty() {
            return;
        }
        self.feature_work(None, move |_| {
            let now = feature::now();
            let reads = std::thread::scope(|scope| {
                let reads: Vec<_> = (asked.iter())
                    .map(|(project, slug, dir, pull)| {
                        scope.spawn(move || {
                            let read = pr::read_remote(dir, pull, now);
                            (project.clone(), slug.clone(), pull.repo.clone(), read)
                        })
                    })
                    .collect();
                (reads.into_iter()).filter_map(|r| r.join().ok()).collect()
            });
            Done::PrRead { reads }
        });
    }

    /// Keep what reading the pull requests under review found. What changed
    /// is saved and told once; what couldn't be read is a problem until it
    /// can, as it is read again in a minute anyway.
    pub(crate) fn pr_reviews_read(
        &mut self,
        reads: Vec<(String, String, String, Result<pr::Remote, String>)>,
    ) {
        let mut failed = Vec::new();
        for (project, slug, key, read) in reads {
            let read = match read {
                Ok(read) => read,
                Err(why) => {
                    tracing::warn!("reading the pull request of {project}/{slug}: {why}");
                    failed.push(why);
                    continue;
                }
            };
            // Gone meanwhile, or moved to another machine: nothing to keep.
            let Some(f) = self.feature_mut(&project, &slug) else {
                continue;
            };
            if f.status != Status::Open {
                continue;
            }
            // By repository: two can each have a #12.
            let Some(pull) = f.pulls_mut().iter_mut().find(|p| p.repo == key) else {
                continue;
            };
            let first = pull.remote.is_none();
            match pull.take_remote(read) {
                pr::Change::Nothing => {
                    self.verify_from_ci(&project, &slug);
                    continue;
                }
                pr::Change::Checks => {
                    self.save_feature(&project, &slug);
                    self.verify_from_ci(&project, &slug);
                    continue;
                }
                // Read the first time as it was reviewed: nothing to tell.
                pr::Change::Pull if first && pull.ended().is_none() && pull.moved().is_none() => {
                    self.save_feature(&project, &slug);
                    self.verify_from_ci(&project, &slug);
                    continue;
                }
                pr::Change::Pull => {}
            }
            let number = pull.number;
            let (what, tell) = match (pull.ended(), pull.moved()) {
                (Some(end), _) => {
                    let how = if end.merged() { "merged" } else { "closed" };
                    let text = format!("#{number} was {how} on GitHub: its review can finish");
                    (format!("#{number} {how} on GitHub"), Some(text))
                }
                (None, Some(_)) => (
                    format!("#{number} has new commits on GitHub"),
                    Some(format!("#{number} has new commits on GitHub")),
                ),
                // Back where the worktree is: an update elsewhere, or undone.
                (None, None) => (format!("#{number} is as reviewed again"), None),
            };
            let moved = pull.moved().is_some();
            f.log(feature::now(), what);
            self.save_feature(&project, &slug);
            self.verify_from_ci(&project, &slug);
            let Some(text) = tell else { continue };
            let button = match moved {
                true => ("Update".to_owned(), MenuAction::UpdatePrHead(project, slug)),
                false => ("Open".to_owned(), MenuAction::OpenFeature(project, slug)),
            };
            self.toast_with(text, Level::Warning, Some(button));
        }
        match failed.first() {
            Some(why) => crate::problems::report(
                PROBLEMS,
                format!("Couldn't read the pull requests under review: {why}"),
            ),
            None => crate::problems::clear(PROBLEMS),
        }
    }

    /// Mark Verify of review `slug` done once GitHub's checks of the head
    /// reviewed all passed, while its worktree is at that head, nothing
    /// changed, as its code was last read; then AI review starts, as after
    /// checks passing here.
    pub(crate) fn verify_from_ci(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        if f.status != Status::Open
            || f.writer.is_some()
            || f.current() != Some(feature::Stage::Verify)
            || self.feature_busy(project, slug)
            || !self.row_ctx(f).on_head
        {
            return;
        }
        let Some(ci) = f.pr_ci().filter(|ci| ci.verdict() == pr::Verdict::Passed) else {
            return;
        };
        let patch = (f.marks.get(&feature::Stage::Build)).and_then(|m| m.patch.clone());
        let Some(patch) = patch else {
            return;
        };
        let heads: Vec<String> = (f.pulls().iter())
            .map(|p| format!("#{} at {}", p.number, p.short_head()))
            .collect();
        let note = format!("{} checks passed on GitHub: {}", ci.total, heads.join(", "));
        let title = f.title.clone();
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if f.tick_said(now, BY_CI, Some(&patch), &format!("done: {note}"))
            .is_none()
        {
            return;
        }
        if let Some(mark) = f.marks.get_mut(&feature::Stage::Verify) {
            mark.note = note.clone();
        }
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.start_reviewer(project, slug, &title);
    }

    /// Run the GitHub Actions runs whose checks failed on the head of review
    /// `slug` again, on GitHub, off the thread. Other checks can't be.
    pub(crate) fn rerun_ci(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        // Each pull request's runs, in its own repository.
        let runs: Vec<(u64, String, std::path::PathBuf, Vec<u64>)> = (f.pulls().iter())
            .filter_map(|p| {
                let ids = p.checks().map(pr::rerun_ids).unwrap_or_default();
                let dir = f.repo(&p.repo).or(f.repos.first())?.main.clone();
                (!ids.is_empty()).then(|| (p.number, p.repo_arg(), dir, ids))
            })
            .collect();
        let Some(pull) = f.pull() else {
            return;
        };
        if runs.is_empty() {
            return self.show_warning(format!(
                "The failed checks of #{} aren't GitHub Actions runs: run them again where they ran",
                pull.number
            ));
        }
        let number = (runs.iter())
            .map(|(n, ..)| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Running the failed checks of {number} again on GitHub"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let mut ran = 0;
            let mut failed = Vec::new();
            for (n, arg, dir, ids) in runs {
                for id in ids {
                    let id = id.to_string();
                    let args = ["run", "rerun", &id, "--failed", "--repo", &arg];
                    match feature::ship::run_gh("gh", &dir, &args) {
                        Ok(_) => ran += 1,
                        Err(why) => failed.push(format!("#{n} run {id}: {why}")),
                    }
                }
            }
            Done::CiRerun {
                project,
                slug,
                number,
                ran,
                failed,
            }
        });
    }

    /// The failed runs of review `slug` were asked to run again: say so, and
    /// read how its checks stand soon.
    pub(crate) fn ci_rerun(
        &mut self,
        project: &str,
        slug: &str,
        number: String,
        ran: usize,
        failed: Vec<String>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        if let Some(why) = failed.first() {
            tracing::warn!(
                "running the checks of {number} again: {}",
                failed.join("; ")
            );
            self.show_error(format!("Couldn't run the checks of {number} again: {why}"));
        }
        if ran > 0 {
            let runs = if ran == 1 { "run" } else { "runs" };
            self.show_success(format!(
                "{number}: {ran} failed {runs} going again on GitHub"
            ));
            self.refresh_pr_reviews();
        }
    }

    /// Move the worktree of pull request review `slug` to the head and base
    /// GitHub has now, off the thread; then it is reviewed from Verify on.
    pub(crate) fn update_pr_head(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return self.show_warning("That review is gone".into());
        };
        let Some((first, _)) = f.pr_moved() else {
            let n = f.pull().map_or(0, |p| p.number);
            return self.show_warning(format!("#{n} is on its head already"));
        };
        let number = first.number;
        if f.status != Status::Open || f.writer.is_some() {
            return self.show_warning(format!(
                "“{}” is done or open on another machine: nothing to update here",
                f.title
            ));
        }
        if self.feature_busy(project, slug) {
            return self.show_warning(format!("“{}” has work running: wait for it", f.title));
        }
        // Every pull request GitHub moved, each in its repository.
        let moves: Vec<(feature::Repo, pr::PullRef, pr::Remote)> = (f.pulls().iter())
            .filter_map(|p| {
                let to = p.moved()?.clone();
                let repo = f.repo(&p.repo).or(f.repos.first())?.clone();
                Some((repo, p.clone(), to))
            })
            .collect();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Updating #{number} to its new head"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PrMoved {
            results: (moves.iter())
                .map(|(repo, pull, to)| (repo.key(), layout::update_pr(repo, pull, to)))
                .collect(),
            project,
            slug,
        });
    }

    /// The worktree of review `slug` moved, or not: keep where it is now and
    /// read its code, which sends it back to Verify.
    pub(crate) fn pr_head_moved(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(String, Result<layout::MovedPr, String>)>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let mut done = Vec::new();
        for (key, result) in results {
            let moved = match result {
                Ok(moved) => moved,
                Err(why) => {
                    tracing::warn!("updating {key} of {project}/{slug} to its new head: {why}");
                    self.show_error(format!("Couldn't update {key} to its new head: {why}"));
                    continue;
                }
            };
            let Some(f) = self.feature_mut(project, slug) else {
                return;
            };
            if let Some(repo) = f.repos.iter_mut().find(|r| r.key() == key) {
                repo.base = moved.base.clone();
                repo.base_commit = moved.base_commit.clone();
            }
            let first = f.repos.first().is_some_and(|r| r.key() == key);
            let Some(pull) =
                (f.pulls_mut().iter_mut()).find(|p| p.repo == key || (p.repo.is_empty() && first))
            else {
                continue;
            };
            pull.moved_to(&moved.head_sha, &moved.base_ref);
            let text = format!("#{} updated to {}", pull.number, pull.short_head());
            f.log(feature::now(), text.clone());
            done.push(text);
        }
        if done.is_empty() {
            return;
        }
        self.save_feature(project, slug);
        self.show_success(done.join(", "));
        // Its code is read: new code sends it back to Verify.
        self.probe_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
    }

    /// Ask before finishing review `slug`, whose pull request ended.
    pub(crate) fn ask_finish_pr_review(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let submitted = (f.shipping.reviews.last()).map(|r| r.event.done());
        let ended = f.pr_ended();
        let own_at_ship = f.pulls().iter().any(pr::PullRef::works_here)
            && f.current() == Some(feature::Stage::Ship);
        let how = match (ended, submitted) {
            (Some(end), _) if end.merged() => "It was merged on GitHub".to_owned(),
            (Some(_), _) => "It was closed on GitHub".to_owned(),
            (None, Some(done)) => format!("You {done} on GitHub; new commits won't come up here"),
            (None, None) if own_at_ship => "The pull request stays open on GitHub".to_owned(),
            (None, None) => {
                return self.show_warning(format!(
                    "“{}” is still open on GitHub: submit its review first",
                    f.title
                ));
            }
        };
        let then = match f.ship_plan.keep_workspace {
            true => "Its workspace stays open",
            false => "Its workspace closes",
        };
        let body = format!("{how}\n{then}; its worktree stays");
        let then = Then::FinishPrReview {
            project: project.to_owned(),
            slug: slug.to_owned(),
        };
        self.ask(format!("Finish “{}”?", f.title), body, "Finish", then);
    }

    /// Finish review `slug`, its pull request ended: done, its workspace
    /// closed unless its Ship page keeps it.
    pub(crate) fn finish_pr_review(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if f.pull().is_none() || f.status != Status::Open {
            return;
        }
        let how = match (f.pr_ended(), f.shipping.reviews.last()) {
            (Some(end), _) if end.merged() => "merged on GitHub".to_owned(),
            (Some(_), _) => "closed on GitHub".to_owned(),
            (None, Some(r)) => format!("{} on GitHub", r.event.done()),
            (None, None) => "nothing posted".to_owned(),
        };
        f.status = Status::Done;
        f.log(feature::now(), format!("Finished: {how}"));
        let mut text = format!("“{}” finished", f.title);
        self.save_feature(project, slug);
        if let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
        self.probe_feature(project, slug);
    }
}
