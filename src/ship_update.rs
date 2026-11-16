//! Taking a base into a shipping feature's branch, from Ship's page: a pull
//! request that waited while its base moved on is brought up to date, by a
//! merge or a rebase in the feature's worktree. The feature then goes
//! through Verify again, as the code it would merge changed.

use crate::{
    Gerdr,
    confirm::Then,
    feature::{self, UpdateWay, ship},
    features::{Busy, Done, Job},
};

impl Gerdr {
    /// Ask before taking the bases of repositories `repos` into feature
    /// `slug`'s branch, `way`, saying what follows.
    pub(crate) fn ask_update_base(
        &mut self,
        project: &str,
        slug: &str,
        repos: Vec<String>,
        way: UpdateWay,
    ) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        if repos.is_empty() {
            return self.show_warning(format!(
                "Nothing of \u{201c}{title}\u{201d} is behind its base now"
            ));
        }
        let branch = feature.branch();
        let bases: Vec<&str> = (repos.iter())
            .map(|key| feature.repo(key).map_or("its base", |r| r.base.as_str()))
            .collect();
        let base = match bases.as_slice() {
            [first, rest @ ..] if rest.iter().all(|b| b == first) => *first,
            _ => "their bases",
        };
        let names = repos.join(", ");
        let question = match way {
            UpdateWay::Merge => format!("Merge {base} into {branch} of {names}?"),
            UpdateWay::Rebase => format!("Rebase {branch} of {names} onto {base}?"),
        };
        let mut body = "Then Verify runs again, AI review and Code review look at what \
                        changed, and Manual test comes back with what was found kept."
            .to_owned();
        if way == UpdateWay::Rebase {
            body.push_str(
                "\n\nRebasing rewrites the branch's commits: the next push replaces \
                 what origin has.",
            );
        }
        let then = Then::UpdateBase {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repos,
            way,
        };
        let go = match way {
            UpdateWay::Merge => "Merge",
            UpdateWay::Rebase => "Rebase",
        };
        self.ask(question, body, go, then);
    }

    /// Ask before merging their bases into every repository of feature
    /// `slug` that is behind with no conflict, as its last check read them.
    pub(crate) fn ask_update_all_bases(&mut self, project: &str, slug: &str) {
        let repos: Vec<String> = (self.last_probe(project, slug).into_iter())
            .flat_map(|p| p.bases.iter())
            .filter(|(_, state)| state.clean())
            .map(|(key, _)| key.clone())
            .collect();
        self.ask_update_base(project, slug, repos, UpdateWay::Merge);
    }

    /// Take the bases of repositories `repos` into feature `slug`'s branch
    /// off the thread, one after another, once asked.
    pub(crate) fn update_base(
        &mut self,
        project: &str,
        slug: &str,
        repos: Vec<String>,
        way: UpdateWay,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: (repos.len() == 1).then(|| repos[0].clone()),
            what: format!("Updating \u{201c}{}\u{201d} from its base", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::BaseUpdated {
            results: (repos.iter())
                .map(|repo| ship::update_from_base(&feature, repo, way, feature::now()))
                .collect(),
            project,
            slug,
        });
    }

    /// Move repository `repo`'s base on to origin's off the thread, only
    /// fast-forward: its squash into the base waits for that. Nothing is
    /// lost, so it doesn't ask.
    pub(crate) fn pull_base(&mut self, project: &str, slug: &str, repo: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let Some(found) = feature.repo(repo).cloned() else {
            return self.show_warning(format!("\u{201c}{title}\u{201d} has no {repo}"));
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: Some(repo.to_owned()),
            what: format!("Pulling {} of {repo}", found.base),
        };
        let (project, slug, repo) = (project.to_owned(), slug.to_owned(), repo.to_owned());
        self.feature_work(Some(busy), move |_| Done::BasePulled {
            result: ship::pull_base(&found),
            project,
            slug,
            repo,
        });
    }

    /// Repository `repo`'s base of feature `slug` moved on to origin's, or
    /// didn't: the Ship check reads it again.
    pub(crate) fn base_pulled(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        result: Result<usize, String>,
    ) {
        let base = (self.feature(project, slug))
            .and_then(|f| f.repo(repo))
            .map_or_else(|| "its base".to_owned(), |r| r.base.clone());
        match result {
            Ok(n) => {
                let commits = match n {
                    1 => "1 commit".to_owned(),
                    n => format!("{n} commits"),
                };
                self.show_success(format!("Pulled {base} of {repo}: {commits}"));
            }
            Err(why) => {
                tracing::warn!("pulling {base} of {repo} for feature {project}/{slug}: {why}");
                self.show_error(format!("Couldn't pull {base} of {repo}: {why}"));
            }
        }
        self.probe_feature(project, slug);
    }

    /// Feature `slug`'s branch took the bases of its repositories in, or
    /// some didn't: back to Verify with those that did, and why the rest
    /// didn't, in one toast.
    pub(crate) fn base_updated(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<Result<(feature::BaseUpdate, Option<String>), String>>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let (mut said, mut failed) = (Vec::new(), Vec::new());
        for result in results {
            match result {
                Ok((update, patch)) => {
                    said.push(format!(
                        "{} {} {}",
                        update.repo,
                        update.way.done_word(),
                        update.onto
                    ));
                    feature.updated_from_base(update, patch.as_deref(), feature::now());
                }
                Err(why) => {
                    tracing::warn!("updating feature {project}/{slug} from its base: {why}");
                    feature.log(
                        feature::now(),
                        format!("Update from the base stopped: {why}"),
                    );
                    failed.push(why);
                }
            }
        }
        self.save_feature(project, slug);
        let (said, failed) = (said.join(", "), failed.join("; "));
        if said.is_empty() {
            return self.show_error(format!("Couldn't update \u{201c}{title}\u{201d}: {failed}"));
        }
        self.feature_cursor_to_current_of(project, slug);
        self.show_stage_page(project, slug);
        match failed.is_empty() {
            true => self.show_success(format!("\u{201c}{title}\u{201d}: {said}; back to Verify")),
            false => self.show_warning(format!(
                "\u{201c}{title}\u{201d}: {said}; back to Verify; {failed}"
            )),
        }
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.build.verifying.contains(&key) {
            self.recheck(project, slug);
        }
        self.probe_feature(project, slug);
    }
}
