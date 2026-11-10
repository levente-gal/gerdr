//! Starting the review of a GitHub pull request (see `feature::pr`): from a
//! pull request's chip on the GitHub card, its inbox item, or its page.
//! Its record is a feature's, so it shows, opens and goes through its
//! stages as one does.

use std::path::PathBuf;

use crate::{
    Gerdr, ci,
    feature::{self, Feature, Status, layout, pr, store},
    features::{Busy, Done, Job, root_search},
};

impl Gerdr {
    /// The open review of pull request `slug#number` of `host` in project
    /// `project`, by its slug.
    pub(crate) fn pr_review_of(
        &self,
        project: &str,
        host: &str,
        slug: &str,
        number: u64,
    ) -> Option<String> {
        let all = self.features.all.get(project)?;
        (all.iter())
            .filter(|f| f.status == Status::Open)
            .find(|f| {
                f.pull().is_some_and(|p| {
                    p.number == number
                        && p.host.eq_ignore_ascii_case(host)
                        && p.slug.eq_ignore_ascii_case(slug)
                })
            })
            .map(|f| f.slug.clone())
    }

    /// Review the pull request whose page is `url` in the open project:
    /// open its review if it has one, else make it off the thread, the
    /// project's repositories found from workspace `at`'s folder (else the
    /// focused one's, else its first workspace's).
    pub(crate) fn start_pr_review(&mut self, at: Option<usize>, url: &str) {
        let Some((host, slug, number)) = ci::pull_request_of(url) else {
            return self.show_warning(format!("{url} isn't a pull request"));
        };
        let Some(id) = self.project.clone() else {
            return self.show_warning("Open a project to review a pull request in".into());
        };
        if let Some(open) = self.pr_review_of(&id, &host, &slug, number) {
            return self.open_feature(&id, &open);
        }
        let Some(project) = self.projects.project(&id) else {
            return self.show_warning("Open a project to review a pull request in".into());
        };
        let from = at
            .and_then(|i| self.workspaces.get(i))
            .or_else(|| self.workspace())
            .map(|ws| ws.dir.clone())
            .or_else(|| project.workspaces.first().map(|w| w.dir.clone()));
        let Some(from) = from else {
            return self
                .show_warning("Add a workspace with a folder to review a pull request".into());
        };
        let (workspace_dirs, ignore, bases) = root_search(project);
        let others = self.other_projects_dirs(&id);
        let taken: Vec<String> = (self.features.all.get(&id).into_iter().flatten())
            .chain(self.viewed_of(&id))
            .map(|f| f.slug.clone())
            .collect();
        let feature_slug = free_slug(&pr::slug_of(number), &taken);
        let busy = Busy {
            project: id.clone(),
            slug: feature_slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Starting the review of #{number}"),
        };
        self.feature_work(Some(busy), move |_| {
            let made = (|| {
                let root =
                    layout::find_project_root(&from, &workspace_dirs, &others, &ignore, &bases)?;
                make(&id, &root, &host, &slug, number, &feature_slug)
            })();
            Done::PrReview {
                project: id,
                slug: feature_slug,
                number,
                result: made.map(Box::new),
            }
        });
    }

    /// The review of pull request `number` was made as `slug`, or not: keep
    /// it, show it and set it up.
    pub(crate) fn pr_review_made(
        &mut self,
        project: String,
        slug: String,
        number: u64,
        result: Result<Box<(Feature, Vec<PathBuf>)>, String>,
    ) {
        let (feature, ignores) = match result {
            Ok(made) => *made,
            Err(why) => {
                tracing::warn!("starting the review of #{number} in {project}: {why}");
                return self.show_error(format!("Couldn't start the review of #{number}: {why}"));
            }
        };
        let text = crate::features::created_text(&feature.title, &feature.root, &ignores);
        self.features.docs_linked.remove(&feature.dir);
        (self.features.all.entry(project.clone()).or_default()).push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        self.show_success(text);
        self.start_setup(&project, &slug);
        self.refresh_feature_graphs(&project, &slug, true);
    }
}

/// `base`, or the first of `base-2`, `base-3`… that no feature in `taken`
/// has: two repositories of a project can each have a pull request 12.
fn free_slug(base: &str, taken: &[String]) -> String {
    (1..)
        .map(|k| match k {
            1 => base.to_owned(),
            k => format!("{base}-{k}"),
        })
        .find(|s| !taken.contains(s))
        .unwrap_or_else(|| base.to_owned())
}

/// Make the review of pull request `number` of repository `slug` of `host`
/// in project `project`, as `feature_slug`, in `root`: its worktree, its
/// brief, and its record with the code's patch-id read. Off the thread.
fn make(
    project: &str,
    root: &layout::Root,
    host: &str,
    slug: &str,
    number: u64,
    feature_slug: &str,
) -> Result<(Feature, Vec<PathBuf>), String> {
    let Some(repo) = pr::repo_of(root, host, slug) else {
        return Err(format!(
            "no repository of this project has {slug} as its origin"
        ));
    };
    let main = (root.repos.iter())
        .find(|r| r.name == repo)
        .map(|r| r.main.clone())
        .unwrap_or_else(|| root.path.clone());
    let details = pr::read(&main, host, slug, number)?;
    if !details.is_open() {
        return Err(format!("it is {}", details.state.to_ascii_lowercase()));
    }
    // Its brief first: a folder made for nothing is easier to take back.
    let data = store::dir(None, project, feature_slug);
    let made_data = !data.exists();
    let brief = feature::artifacts::path(&data, feature::Stage::Discover)
        .unwrap_or_else(|| data.join("brief.md"));
    std::fs::create_dir_all(&data)
        .and_then(|()| std::fs::write(&brief, pr::brief(&details)))
        .map_err(|err| format!("couldn't write its brief {}: {err}", brief.display()))?;
    let created = match layout::create_pr(root, &repo, &details.pull, feature_slug) {
        Ok(created) => created,
        Err(err) => {
            if made_data && let Err(e) = std::fs::remove_dir_all(&data) {
                tracing::warn!("removing {}: {e}", data.display());
            }
            return Err(err);
        }
    };
    let ignores = created.ignores.clone();
    let now = feature::now();
    let mut feature = pr::record(project, root, created, &details, None, now);
    feature.slug = feature_slug.to_owned();
    // Unread, Build's mark waits for the first reading to bind it.
    match feature::patch::read(&feature) {
        Ok(reading) => {
            if let Some(mark) = feature.marks.get_mut(&feature::Stage::Build) {
                mark.patch = Some(reading.patch);
            }
        }
        Err(err) => tracing::warn!("reading the code of #{number}: {err}"),
    }
    Ok((feature, ignores))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_never_clash() {
        assert_eq!(free_slug("pr-12", &[]), "pr-12");
        let taken = vec!["pr-12".to_owned(), "pr-12-2".to_owned()];
        assert_eq!(free_slug("pr-12", &taken), "pr-12-3");
    }
}
