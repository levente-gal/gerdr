//! Watching the pull requests under review (see `feature::pr`): every
//! `INTERVAL`, off the event loop, gh says how each open review's pull
//! request stands on GitHub, for every project. What changed is kept in
//! its record (`PullRef::remote`) and told once: new commits or another
//! base raise the `NewHead` gate, and the review moves to them only when
//! the user says so (`update_pr_head`); a merged or closed one raises
//! `PrEnded`, and the review can be finished (`finish_pr_review`).

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

/// How often the pull requests under review are read.
const INTERVAL: Duration = Duration::from_secs(60);

/// Where a pull request watch can't read GitHub goes (see `problems`).
pub const PROBLEMS: &str = "pull requests";

/// A review whose pull request is to be read: its project and slug, a
/// folder of its repository to run gh in, and the pull request.
type Asked = (String, String, PathBuf, pr::PullRef);

impl Gerdr {
    /// Read the pull requests under review now, then every `INTERVAL`.
    pub fn watch_pr_reviews(&mut self) {
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), |_, _, state| {
                state.refresh_pr_reviews();
                TimeoutAction::ToDuration(INTERVAL)
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
            .filter_map(|(project, f)| {
                let pull = f.pull()?.clone();
                let dir = f.repos.first().map(|r| r.main.clone())?;
                Some((project.clone(), f.slug.clone(), dir, pull))
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
                            (project.clone(), slug.clone(), read)
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
        reads: Vec<(String, String, Result<pr::Remote, String>)>,
    ) {
        let mut failed = Vec::new();
        for (project, slug, read) in reads {
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
            let feature::Kind::PrReview(pull) = &mut f.kind else {
                continue;
            };
            if !pull.take_remote(read) {
                continue;
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

    /// Move the worktree of pull request review `slug` to the head and base
    /// GitHub has now, off the thread; then it is reviewed from Verify on.
    pub(crate) fn update_pr_head(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return self.show_warning("That review is gone".into());
        };
        let Some(pull) = f.pull() else {
            return;
        };
        let Some(to) = pull.moved().cloned() else {
            return self.show_warning(format!("#{} is on its head already", pull.number));
        };
        if f.status != Status::Open || f.writer.is_some() {
            return self.show_warning(format!(
                "“{}” is done or open on another machine: nothing to update here",
                f.title
            ));
        }
        if self.feature_busy(project, slug) {
            return self.show_warning(format!("“{}” has work running: wait for it", f.title));
        }
        let Some(repo) = f.repos.first().cloned() else {
            return;
        };
        let pull = pull.clone();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::PrHead,
            repo: None,
            what: format!("Updating #{} to its new head", pull.number),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::PrMoved {
            result: layout::update_pr(&repo, &pull, &to),
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
        result: Result<layout::MovedPr, String>,
    ) {
        self.unbusy(project, slug, Job::PrHead);
        let moved = match result {
            Ok(moved) => moved,
            Err(why) => {
                tracing::warn!("updating {project}/{slug} to its new head: {why}");
                return self.show_error(format!("Couldn't update to the new head: {why}"));
            }
        };
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if let Some(repo) = f.repos.first_mut() {
            repo.base = moved.base.clone();
            repo.base_commit = moved.base_commit.clone();
        }
        let feature::Kind::PrReview(pull) = &mut f.kind else {
            return;
        };
        pull.moved_to(&moved.head_sha, &moved.base_ref);
        let text = format!("#{} updated to {}", pull.number, pull.short_head());
        f.log(feature::now(), text.clone());
        self.save_feature(project, slug);
        self.show_success(text);
        // Its code is read: new code sends it back to Verify.
        self.probe_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
    }

    /// Ask before finishing review `slug`, whose pull request ended.
    pub(crate) fn ask_finish_pr_review(&mut self, project: &str, slug: &str) {
        let Some(f) = self.feature(project, slug) else {
            return;
        };
        let Some(end) = f.pull().and_then(pr::PullRef::ended) else {
            return self.show_warning(format!("“{}” is still open on GitHub", f.title));
        };
        let how = if end.merged() { "merged" } else { "closed" };
        let then = match f.ship_plan.keep_workspace {
            true => "Its workspace stays open",
            false => "Its workspace closes",
        };
        let body = format!("It was {how} on GitHub. Nothing is posted\n{then}; its worktree stays");
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
        let Some(end) = f.pull().and_then(pr::PullRef::ended).cloned() else {
            return;
        };
        if f.status != Status::Open {
            return;
        }
        let how = if end.merged() { "merged" } else { "closed" };
        f.status = Status::Done;
        f.log(feature::now(), format!("Finished: {how} on GitHub"));
        let mut text = format!("“{}” finished", f.title);
        self.save_feature(project, slug);
        if let Some(kept) = self.after_ship(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_shipped(project, slug, text);
        self.probe_feature(project, slug);
    }
}
