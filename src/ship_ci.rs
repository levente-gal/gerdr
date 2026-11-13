//! Checks failing on GitHub on the pull requests a feature shipped, fixed
//! from Ship's page the way Verify's failures are: they go to the Build
//! agent as one fix round, planned first or not. Its commits take the
//! feature back through Verify and the reviews to Ship, whose Push then
//! updates the pull requests. A failed GitHub Actions run can also run
//! again as it is, for a check that failed by chance.

use std::collections::BTreeMap;

use crate::{
    Gerdr,
    feature::{
        self, Feature, Stage, Status,
        pr::{self, CiCheck},
        ship::PrState,
    },
};

/// What a fix round from Ship is about, in its toasts, log and file names.
const WHAT: &str = "CI failures";

/// A repository of a feature whose pull request fails checks: its key, its
/// worktree, its pull request's address and the checks that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failing {
    pub repo: String,
    pub dir: String,
    pub url: String,
    pub checks: Vec<CiCheck>,
}

/// Each repository of `feature` whose pull request fails checks, as `prs`
/// last read them, in their order.
pub(crate) fn failing(
    feature: &Feature,
    prs: &BTreeMap<String, Result<PrState, String>>,
) -> Vec<Failing> {
    (feature.shipping.prs.iter())
        .filter_map(|pr| {
            let state = prs.get(&pr.repo)?.as_ref().ok()?;
            let checks: Vec<CiCheck> = state.failed_checks().into_iter().cloned().collect();
            let repo = feature.repo(&pr.repo)?;
            (!checks.is_empty()).then(|| Failing {
                repo: pr.repo.clone(),
                dir: repo.worktree.display().to_string(),
                url: pr.url.clone(),
                checks,
            })
        })
        .collect()
}

/// What the Build agent is told of `failing`, the pull requests of `title`
/// failing checks: each check, its page, and how to read why it failed.
pub(crate) fn failures_text(title: &str, failing: &[Failing]) -> String {
    let mut md = format!(
        "# CI failures\n\nThe pull requests of \u{201c}{title}\u{201d} failed checks on GitHub. Find each one's cause before you change code, fix it, and commit.\n"
    );
    for f in failing {
        md.push_str(&format!("\n## {} ({})\n\nIn {}:\n\n", f.repo, f.url, f.dir));
        for check in &f.checks {
            let line = match (
                check.link.as_deref(),
                check.link.as_deref().and_then(pr::run_id),
            ) {
                (Some(link), Some(id)) => format!(
                    "- {}: {link}; `gh run view {id} --log-failed` prints its failed jobs' output (tail it, it can be long)\n",
                    check.name
                ),
                (Some(link), None) => format!(
                    "- {}: {link}; not a GitHub Actions run: read its page\n",
                    check.name
                ),
                (None, _) => format!("- {}: GitHub gave no page for it\n", check.name),
            };
            md.push_str(&line);
        }
    }
    md
}

/// The plan gerdr writes for fixes of `failing` made with none asked for:
/// each failed check is one thing to tick (`fix_round::items`).
pub(crate) fn fixes_plan(title: &str, failing: &[Failing]) -> String {
    let mut md = feature::fix_round::header(
        title,
        "the checks failing on its pull requests: the user wrote nothing of their own, so no plan was asked for. Read each check's output, as the prompt says, before you change code.",
    );
    for f in failing {
        for check in &f.checks {
            md.push_str(&format!("\n### {}: {}\n", f.repo, check.name));
        }
    }
    md
}

impl Gerdr {
    /// The pull requests of feature `slug` failing checks, as its probe last
    /// read them.
    fn ci_failing(&self, project: &str, slug: &str) -> Vec<Failing> {
        let Some(feature) = self.feature(project, slug) else {
            return Vec::new();
        };
        (self.features.last_probe(project, slug))
            .and_then(|p| p.prs.as_ref())
            .map_or_else(Vec::new, |prs| failing(feature, prs))
    }

    /// Send the checks failing on feature `slug`'s pull requests to its
    /// Build agent as a fix round asked by Ship: to plan first when
    /// `planning`, else to fix at once. A shipped feature opens again at
    /// Ship for it.
    pub(crate) fn fix_ship_ci(&mut self, project: &str, slug: &str, planning: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let failing = self.ci_failing(project, slug);
        if failing.is_empty() {
            return self.show_warning(format!(
                "No check of \u{201c}{title}\u{201d}'s pull requests failed, as GitHub was last read"
            ));
        }
        let now = feature::now();
        if feature.status == Status::Done {
            // Refused before it opens, so a refusal changes nothing.
            let mut open = feature.clone();
            open.untick(Stage::Ship, now);
            if let Some(why) = self.fix_round_refusal(&open) {
                return self.show_warning(why);
            }
            let Some(f) = self.feature_mut(project, slug) else {
                return;
            };
            f.untick(Stage::Ship, now);
            f.log(now, "CI failed on GitHub: Ship to do again once fixed");
            self.save_feature(project, slug);
        }
        let text = failures_text(&title, &failing);
        let started = match planning {
            true => self.send_to_build(project, slug, Stage::Ship, WHAT, text),
            false => {
                let plan = fixes_plan(&title, &failing);
                (self.fix_input(project, slug, WHAT, &text)).is_some_and(|input| {
                    self.start_unplanned_fix_round(project, slug, Stage::Ship, WHAT, input, &plan)
                })
            }
        };
        if !started {
            return;
        }
        tracing::info!("feature {project}/{slug}: CI failures sent to Build");
        // Not sent: its plan waits on the page.
        if planning || self.make_unplanned_fixes(project, slug) {
            self.show_build_agent(project, slug);
        }
    }

    /// Run the failed GitHub Actions runs of repository `repo`'s pull
    /// request of feature `slug` again on GitHub.
    pub(crate) fn rerun_ship_ci(&mut self, project: &str, slug: &str, repo: &str) {
        let Some(f) = (self.ci_failing(project, slug).into_iter()).find(|f| f.repo == repo) else {
            return self.show_warning(format!(
                "{repo}'s pull request has no failed check to run again"
            ));
        };
        let ids = pr::rerun_ids(&f.checks);
        if ids.is_empty() {
            return self.show_warning(format!(
                "The failed checks of {repo}'s pull request aren't GitHub Actions runs: run them again where they ran"
            ));
        }
        // `--repo` as gh takes it: `github.com/acme/api` of its address.
        let (base, number) = f.url.rsplit_once("/pull/").unwrap_or((&f.url, ""));
        let arg = base.trim_start_matches("https://").to_owned();
        let number = number.trim_end_matches('/').parse().unwrap_or_default();
        let runs = vec![(number, arg, f.dir.into(), ids)];
        self.rerun_failed_runs(project, slug, runs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ci::CiState, feature::Pr};

    fn check(name: &str, state: CiState, link: Option<&str>) -> CiCheck {
        CiCheck {
            name: name.into(),
            state,
            link: link.map(str::to_owned),
        }
    }

    #[test]
    fn only_open_pull_requests_failed_checks_are_sent() {
        let mut f = Feature::default();
        f.repos.push(crate::features::tests::repo("api", true));
        f.shipping.prs.push(Pr {
            repo: "api".into(),
            url: "https://github.com/o/api/pull/3".into(),
        });
        let run = "https://github.com/o/api/actions/runs/77/job/9";
        let state = PrState {
            state: "OPEN".into(),
            checks: vec![
                check("lint", CiState::Passed, None),
                check("test", CiState::Failed, Some(run)),
                check("deploy", CiState::Failed, Some("https://ci.example/5")),
            ],
            ..Default::default()
        };
        let prs = BTreeMap::from([("api".to_owned(), Ok(state.clone()))]);
        let got = failing(&f, &prs);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].dir, "/work/.features/bulk/api");
        let names: Vec<&str> = got[0].checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["test", "deploy"]);
        let text = failures_text("Login", &got);
        assert!(text.contains("`gh run view 77 --log-failed`"), "{text}");
        assert!(text.contains("deploy: https://ci.example/5; not a GitHub Actions run"));
        assert!(!text.contains("lint"));
        let plan = fixes_plan("Login", &got);
        assert!(plan.contains("\n### api: test\n") && plan.contains("\n### api: deploy\n"));
        assert_eq!(feature::fix_round::items(&plan, &[]).len(), 2);
        // Merged, nothing is left to fix.
        let merged = PrState {
            state: "MERGED".into(),
            ..state
        };
        let prs = BTreeMap::from([("api".to_owned(), Ok(merged))]);
        assert!(failing(&f, &prs).is_empty());
    }
}
