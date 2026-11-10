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
                f.pulls().iter().any(|p| {
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
    /// focused one's, else its first workspace's). A project of several
    /// repositories opens the review dialog with it picked instead.
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
            let found = layout::find_project_root(&from, &workspace_dirs, &others, &ignore, &bases);
            // Of several repositories, the others' pull requests may go
            // with it: the dialog picks them.
            if let Ok(root) = &found
                && root.multi
            {
                return Done::ReviewPick {
                    project: id,
                    slug: feature_slug,
                    from,
                    wanted: (host, slug, number),
                };
            }
            let made = found.and_then(|root| make(&id, &root, &host, &slug, number, &feature_slug));
            Done::PrReview {
                project: id,
                slug: feature_slug,
                numbers: format!("#{number}"),
                result: made.map(Box::new),
            }
        });
    }

    /// Review the pull requests `picks` of project `project` as one, off
    /// the thread: of the repositories of `root` and of the linked projects
    /// `links`, titled `title` (empty: after the first pull request).
    pub(crate) fn start_review_of(
        &mut self,
        project: String,
        root: layout::Root,
        links: Vec<layout::LinkSpec>,
        picks: Vec<Pick>,
        title: String,
    ) {
        let taken: Vec<String> = (self.features.all.get(&project).into_iter().flatten())
            .chain(self.viewed_of(&project))
            .map(|f| f.slug.clone())
            .collect();
        let heads: Vec<(u64, &str)> = picks
            .iter()
            .map(|p| (p.number, p.head_ref.as_str()))
            .collect();
        let feature_slug = free_slug(&pr::slug_for(&heads), &taken);
        let numbers = numbers_of(&picks);
        let busy = Busy {
            project: project.clone(),
            slug: feature_slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Starting the review of {numbers}"),
        };
        self.feature_work(Some(busy), move |_| {
            let made = make_review(&project, &root, &links, &picks, &title, &feature_slug);
            Done::PrReview {
                project,
                slug: feature_slug,
                numbers,
                result: made.map(Box::new),
            }
        });
    }

    /// The review of pull requests `numbers` (`#12, #14`) was made as
    /// `slug`, or not: keep it, show it and set it up.
    pub(crate) fn pr_review_made(
        &mut self,
        project: String,
        slug: String,
        numbers: String,
        result: Result<Box<(Feature, Vec<PathBuf>)>, String>,
    ) {
        let (feature, ignores) = match result {
            Ok(made) => *made,
            Err(why) => {
                tracing::warn!("starting the review of {numbers} in {project}: {why}");
                return self.show_error(format!("Couldn't start the review of {numbers}: {why}"));
            }
        };
        let text = crate::features::created_text(&feature.title, &feature.root, &ignores);
        self.features.docs_linked.remove(&feature.dir);
        (self.features.all.entry(project.clone()).or_default()).push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        // Someone else's code runs only when asked: a setup runs its install
        // scripts, as its checks run its tests.
        let theirs = self
            .feature(&project, &slug)
            .is_some_and(Feature::posts_review);
        match theirs {
            true => self.toast(
                format!("{text} \u{b7} not set up: that would run the code of {numbers}; set it up (Ctrl+Shift+U) once you trust it"),
                crate::toast::Level::Success,
            ),
            false => {
                self.show_success(text);
                self.start_setup(&project, &slug);
            }
        }
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

/// A pull request picked for a review: the repository it is of (the
/// linked project's id, `None` for the home one, and its name), that
/// repository's main worktree, and the pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub project: Option<String>,
    pub repo: String,
    pub main: PathBuf,
    pub host: String,
    pub slug: String,
    pub number: u64,
    /// Its head branch, as listed: for the review's slug.
    pub head_ref: String,
}

/// `#12`, `#12 and #14`, `#12, #14 and #15`.
fn numbers_of(picks: &[Pick]) -> String {
    let all: Vec<String> = picks.iter().map(|p| format!("#{}", p.number)).collect();
    match all.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => all.join(""),
    }
}

/// Make the review of pull request `number` of repository `slug` of `host`
/// in project `project`, as `feature_slug`, in `root` (see `make_review`).
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
    let pick = Pick {
        project: None,
        repo,
        main,
        host: host.to_owned(),
        slug: slug.to_owned(),
        number,
        head_ref: String::new(),
    };
    make_review(project, root, &[], &[pick], "", feature_slug)
}

/// Make the review of pull requests `picks` in project `project`, as
/// `feature_slug`, in `root` and the linked projects `links`: their
/// worktrees, its brief, and its record with the code's patch-id read. Off
/// the thread.
fn make_review(
    project: &str,
    root: &layout::Root,
    links: &[layout::LinkSpec],
    picks: &[Pick],
    title: &str,
    feature_slug: &str,
) -> Result<(Feature, Vec<PathBuf>), String> {
    let mut all = Vec::new();
    for pick in picks {
        let mut details = pr::read(&pick.main, &pick.host, &pick.slug, pick.number)?;
        if !details.is_open() {
            let state = details.state.to_ascii_lowercase();
            return Err(format!("#{} is {state}", pick.number));
        }
        details.pull.repo = feature::repo_key(pick.project.as_deref(), &pick.repo);
        all.push(details);
    }
    let of = |project: Option<&str>| -> Vec<(String, pr::PullRef)> {
        (picks.iter().zip(&all))
            .filter(|(p, _)| p.project.as_deref() == project)
            .map(|(p, d)| (p.repo.clone(), d.pull.clone()))
            .collect()
    };
    let linked: Vec<layout::LinkedPicks> = (links.iter())
        .map(|spec| layout::LinkedPicks {
            spec: spec.clone(),
            picks: of(Some(&spec.project)),
        })
        .filter(|l| !l.picks.is_empty())
        .collect();
    // Its brief first: a folder made for nothing is easier to take back.
    let data = store::dir(None, project, feature_slug);
    let made_data = !data.exists();
    let brief = feature::artifacts::path(&data, feature::Stage::Discover)
        .unwrap_or_else(|| data.join("brief.md"));
    std::fs::create_dir_all(&data)
        .and_then(|()| std::fs::write(&brief, pr::briefs(&all)))
        .map_err(|err| format!("couldn't write its brief {}: {err}", brief.display()))?;
    let (created, made) = match layout::create_review(root, &of(None), &linked, feature_slug) {
        Ok(created) => created,
        Err(err) => {
            if made_data && let Err(e) = std::fs::remove_dir_all(&data) {
                tracing::warn!("removing {}: {e}", data.display());
            }
            return Err(err);
        }
    };
    let mut ignores = created.ignores.clone();
    let now = feature::now();
    let mut feature = pr::record(project, root, created, &all, None, now);
    feature.slug = feature_slug.to_owned();
    if !title.trim().is_empty() {
        feature.title = title.trim().to_owned();
    }
    for (link, created) in made {
        ignores.extend(created.ignores);
        feature.links.push(link);
        feature.repos.extend(created.repos);
    }
    // Unread, Build's mark waits for the first reading to bind it.
    match feature::patch::read(&feature) {
        Ok(reading) => {
            if let Some(mark) = feature.marks.get_mut(&feature::Stage::Build) {
                mark.patch = Some(reading.patch);
            }
        }
        Err(err) => tracing::warn!("reading the code of {feature_slug}: {err}"),
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

    #[test]
    fn several_are_named_together() {
        let pick = |number| Pick {
            project: None,
            repo: "api".into(),
            main: PathBuf::new(),
            host: "github.com".into(),
            slug: "o/api".into(),
            number,
            head_ref: "fix".into(),
        };
        assert_eq!(numbers_of(&[pick(12)]), "#12");
        assert_eq!(numbers_of(&[pick(12), pick(14)]), "#12 and #14");
        assert_eq!(numbers_of(&[pick(1), pick(2), pick(3)]), "#1, #2 and #3");
        assert_eq!(pr::slug_for(&[(12, "fix")]), "pr-12");
        assert_eq!(
            pr::slug_for(&[(12, "fix/Login"), (14, "fix/Login")]),
            "pr-fix-login"
        );
        assert_eq!(pr::slug_for(&[(12, "a"), (14, "b")]), "pr-12-14");
    }
}
