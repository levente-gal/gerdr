//! Undoing what shipping a feature did, from Ship's page: a pushed branch
//! goes off origin, a pull request is closed, a squash not yet on origin
//! comes off the base. What went to origin's base stays: that would rewrite
//! what others may have.

use crate::{
    Gerdr,
    confirm::Then,
    feature::{self, ShipStep, ship},
    features::{Busy, Done, Job},
};

/// The question asking before undoing `step` of `repo`.
fn question(repo: &str, step: ShipStep) -> String {
    match step {
        ShipStep::Push | ShipStep::PushBranch => format!("Undo the push of {repo}?"),
        ShipStep::OpenPr => format!("Close the pull request of {repo}?"),
        ShipStep::Merge => format!("Undo the squash of {repo}?"),
        ShipStep::PushBase => format!("Undo the push of {repo}'s base?"),
    }
}

impl Gerdr {
    /// Ask before undoing `step` of repository `repo` of feature `slug`,
    /// saying what it does; or say why it can't be undone.
    pub(crate) fn ask_undo_ship(&mut self, project: &str, slug: &str, repo: &str, step: ShipStep) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if self.feature_busy(project, slug) {
            let title = feature.title.clone();
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let probe = self.last_probe(project, slug);
        if matches!(
            step,
            ShipStep::Push | ShipStep::PushBranch | ShipStep::OpenPr
        ) && crate::features::pr_merged(probe, repo)
        {
            return self.show_warning(format!(
                "The pull request of {repo} was merged on GitHub: that can't be undone here"
            ));
        }
        let undos = match ship::undo_of(feature, repo, step) {
            Ok(undos) => undos,
            Err(why) => return self.show_warning(why),
        };
        let branch = feature.branch();
        let body = (undos.iter())
            .map(|u| u.label(&branch))
            .collect::<Vec<_>>()
            .join("\n");
        let then = Then::UndoShip {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: repo.to_owned(),
            step,
        };
        self.ask(question(repo, step), body, "Undo", then);
    }

    /// Undo `step` of repository `repo` of feature `slug` off the thread,
    /// once asked.
    pub(crate) fn undo_ship(&mut self, project: &str, slug: &str, repo: &str, step: ShipStep) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let undos = match ship::undo_of(&feature, repo, step) {
            Ok(undos) => undos,
            Err(why) => return self.show_warning(why),
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: Some(repo.to_owned()),
            what: format!("Undoing what shipped \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let (undone, failed) = ship::run_undo(&feature, &undos, "gh");
            Done::ShipUndone {
                project,
                slug,
                undone,
                failed,
            }
        });
    }

    /// What undoing feature `slug`'s steps came to: those undone count as
    /// never run, so its page offers them again.
    pub(crate) fn ship_undone(
        &mut self,
        project: &str,
        slug: &str,
        undone: Vec<(String, ShipStep)>,
        failed: Option<String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let branch = feature.branch();
        let now = feature::now();
        let mut said = Vec::new();
        for (repo, step) in &undone {
            let base = feature.repo(repo).map_or("its base", |r| r.base.as_str());
            let what = (feature.ship_done(repo, *step)).map(|d| d.said(&branch, base));
            if let Some(what) = what {
                said.push(format!("{repo}: {what}"));
            }
            feature.ship_undid(repo, *step, now);
        }
        if !said.is_empty() {
            feature.log(now, format!("Undone: {}", said.join(", ")));
        }
        if let Some(why) = &failed {
            feature.log(now, format!("Undo stopped: {why}"));
        }
        // Nothing it shipped stands any more: it isn't shipped, so Ship is
        // to do again (not once its worktrees went: there is nothing to ship).
        if feature.status == feature::Status::Done && !feature.shipped_any() && feature.dir.exists()
        {
            feature.untick(feature::Stage::Ship, now);
        }
        self.save_feature(project, slug);
        self.probe_feature(project, slug);
        match failed {
            Some(why) => {
                tracing::warn!("undoing what shipped {project}/{slug}: {why}");
                self.show_error(format!(
                    "Couldn't undo all of \u{201c}{title}\u{201d}: {why}"
                ));
            }
            None => self.show_success(format!(
                "Undone for \u{201c}{title}\u{201d}: {}",
                said.join(", ")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_undo_dialog_says_what_follows_not_its_question_again() {
        let undos = [
            ship::Undo::ClosePr {
                repo: "shop".into(),
                url: "https://x/pull/7".into(),
            },
            ship::Undo::DeleteBranch {
                repo: "shop".into(),
                sha: None,
            },
        ];
        assert_eq!(
            undos
                .iter()
                .map(|u| u.label("feature/x"))
                .collect::<Vec<_>>()
                .join("\n"),
            "Its pull request closes on GitHub: https://x/pull/7\nfeature/x goes off origin"
        );
    }
}
