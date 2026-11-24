//! Shipping that stopped short (`Shipping::problem`), fixed from Ship's
//! page, planned first or at once. A commit or push hook that refused the
//! feature's code is the Build agent's, as one fix round asked by Ship (the
//! way CI failures go, `ship_ci`): its commits take the feature back through
//! Verify and the reviews. Anything else (origin moved on, a squash left
//! staged, a folder on the wrong branch, a login) goes to an agent of the
//! feature's own, told what failed and where git's output is.

use crate::{
    Gerdr,
    feature::{self, Feature, Stage, Status, ship},
};

/// What a fix round from a Ship problem is about, in its toasts, log and
/// file names.
const WHAT: &str = "Ship problems";

/// Whether `why` says a git hook here refused it: the feature's code is
/// what such a hook checks, so the Build agent fixes it. A hook on origin
/// (`pre-receive`) is the server's rule, not the code's.
pub(crate) fn by_hook(why: &str) -> bool {
    let why = why.to_ascii_lowercase();
    why.contains("hook") && !why.contains("pre-receive") && !why.contains("remote")
}

/// Where each repository of `feature` has its base checked out, as `check`
/// last read it (`RepoCheck::tree`), else its main worktree, one line each:
/// "- api: dev in /w/api". No git runs here: it is called on the UI thread.
fn bases(feature: &Feature, check: Option<&ship::ShipCheck>) -> String {
    (feature.touched())
        .map(|r| {
            let read = (check.into_iter().flat_map(|c| &c.repos)).find(|c| c.key == r.key());
            let tree = read.map_or(&r.main, |c| &c.tree);
            format!("- {}: {} in {}", r.key(), r.base, tree.display())
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// What the Build agent is told of a hook refusing `title` to ship: `why`,
/// and how to find the rest.
fn hook_text(title: &str, why: &str, log: &str) -> String {
    format!(
        "# Ship problem\n\nShipping \u{201c}{title}\u{201d} stopped: a git hook refused it.\n\n```\n{why}\n```\n\ngit's full output is at the end of {log}. Run what the hook runs in the feature's worktrees, find what fails on the feature's code, fix it and commit. Don't push, and never skip the hook.\n"
    )
}

impl Gerdr {
    /// Say `text`, why shipping feature `slug` stopped, with a way to have
    /// an agent plan its fix.
    pub(crate) fn ship_failed(&mut self, project: &str, slug: &str, text: String) {
        let action = crate::menu::MenuAction::FixShipProblem(project.to_owned(), slug.to_owned());
        let fix = Some(("Plan fix".to_owned(), action));
        self.toast_with(text, crate::toast::Level::Error, fix);
    }

    /// Keep `why` as why shipping feature `slug` stopped, or clear it.
    pub(crate) fn set_ship_problem(&mut self, project: &str, slug: &str, why: Option<String>) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        if f.shipping.problem.as_ref().map(|p| &p.why) == why.as_ref() {
            return;
        }
        f.shipping.problem = why.map(|why| feature::ShipProblem {
            at: feature::now(),
            why,
        });
        self.save_feature(project, slug);
    }

    /// Have an agent fix why shipping feature `slug` stopped: plan first
    /// when `planning`, else fix at once. A hook's refusal goes to the Build
    /// agent; the rest to an agent of the feature's own.
    pub(crate) fn fix_ship_problem(&mut self, project: &str, slug: &str, planning: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let Some(problem) = feature.shipping.problem.clone() else {
            return self.show_warning(format!(
                "Nothing stopped shipping \u{201c}{title}\u{201d} lately"
            ));
        };
        if feature.posts_review() {
            return self.show_warning(
                "On someone else's pull request, nothing goes to an agent: fix it here first"
                    .into(),
            );
        }
        let log = crate::state_dir().join("git.log").display().to_string();
        if by_hook(&problem.why) {
            return self.ship_problem_to_build(project, slug, planning, &problem.why, &log);
        }
        let check = self.last_probe(project, slug).and_then(|p| p.ship.as_ref());
        let bases = bases(&feature, check);
        let (_, command) = match self.picked_cli(&feature, Stage::Build) {
            Ok(picked) => picked,
            Err(why) => return self.show_warning(why),
        };
        let plan = if planning { "yes" } else { "" };
        let vars = [
            ("ship.why", problem.why),
            ("ship.log", log),
            ("ship.bases", bases),
            ("ship.plan", plan.to_owned()),
        ];
        let task = crate::feature_agents::OwnTask {
            template: "ship-fix",
            vars: vars.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
            called: "Ship fix agent",
            picked: "fix agent",
        };
        let Some((w, _)) = self.ensure_feature_workspace(project, slug) else {
            return;
        };
        // Its tab opens in front, the feature's log saying so.
        self.start_own_agent(w, project, slug, &command, Some(task));
    }

    /// Send the hook's refusal `why` of feature `slug` to its Build agent as
    /// a fix round asked by Ship, planned first when `planning`. A shipped
    /// feature opens again at Ship for it.
    fn ship_problem_to_build(
        &mut self,
        project: &str,
        slug: &str,
        planning: bool,
        why: &str,
        log: &str,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
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
            f.log(now, "A hook refused shipping: Ship to do again once fixed");
            self.save_feature(project, slug);
        }
        let text = hook_text(&title, why, log);
        let started = match planning {
            true => self.send_to_build(project, slug, Stage::Ship, WHAT, text),
            false => {
                let mut plan = feature::fix_round::header(
                    &title,
                    "a git hook refusing to ship it: the user wrote nothing of their own, so no plan was asked for. Run what the hook runs before you change code.",
                );
                plan.push_str(&format!("\n### {why}\n"));
                (self.fix_input(project, slug, WHAT, &text)).is_some_and(|input| {
                    self.start_unplanned_fix_round(project, slug, Stage::Ship, WHAT, input, &plan)
                })
            }
        };
        if !started {
            return;
        }
        tracing::info!("feature {project}/{slug}: Ship problem sent to Build");
        // Not sent: its plan waits on the page.
        if planning || self.make_unplanned_fixes(project, slug) {
            self.show_build_agent(project, slug);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hooks_refusal_goes_to_build_and_the_rest_to_an_own_agent() {
        assert!(by_hook("husky - pre-push hook exited with code 1 (error)"));
        assert!(!by_hook(
            "Couldn't push dev of api: pre-receive hook declined"
        ));
        assert!(!by_hook("Couldn't push dev of api: Updates were rejected"));
        let text = hook_text("Login", "pre-commit hook failed", "/s/git.log");
        assert!(text.contains("pre-commit hook failed") && text.contains("/s/git.log"));
        let plan = feature::fix_round::header("Login", "a hook");
        assert_eq!(
            feature::fix_round::items(&format!("{plan}\n### pre-commit hook failed\n"), &[]).len(),
            1
        );
    }
}
