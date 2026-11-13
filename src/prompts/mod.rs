//! Prompts for the feature stages' agents: templates rendered with the
//! feature's values (see `render`). Only the built-in templates for now,
//! from `assets/prompts/` (see `builtin`); a library of the user's own
//! comes later.

pub mod render;

/// The built-in template or partial `name` ("plan", "rules"), and its
/// version: bumped when its text changes meaning.
pub fn builtin(name: &str) -> Option<(&'static str, u32)> {
    Some(match name {
        "discover" => (include_str!("../../assets/prompts/discover.md"), 4),
        "plan" => (include_str!("../../assets/prompts/plan.md"), 4),
        "plan-review" => (include_str!("../../assets/prompts/plan-review.md"), 2),
        "plan-review-back" => (include_str!("../../assets/prompts/plan-review-back.md"), 1),
        "plan-final" => (include_str!("../../assets/prompts/plan-final.md"), 1),
        "build" => (include_str!("../../assets/prompts/build.md"), 7),
        "ai-review" => (include_str!("../../assets/prompts/ai-review.md"), 4),
        "pr-review" => (include_str!("../../assets/prompts/pr-review.md"), 2),
        "fix-plan" => (include_str!("../../assets/prompts/fix-plan.md"), 2),
        "fix-go" => (include_str!("../../assets/prompts/fix-go.md"), 3),
        "fix-on" => (include_str!("../../assets/prompts/fix-on.md"), 2),
        "continue" => (include_str!("../../assets/prompts/continue.md"), 3),
        "nudge" => (include_str!("../../assets/prompts/nudge.md"), 1),
        "send-back" => (include_str!("../../assets/prompts/send-back.md"), 1),
        "start" => (include_str!("../../assets/prompts/start.md"), 1),
        "agent" => (include_str!("../../assets/prompts/agent.md"), 1),
        "rules" => (include_str!("../../assets/prompts/rules.md"), 1),
        "repos" => (include_str!("../../assets/prompts/repos.md"), 5),
        "summary" => (include_str!("../../assets/prompts/summary.md"), 1),
        "conflicts" => (include_str!("../../assets/prompts/conflicts.md"), 1),
        "conflicts-plan" => (include_str!("../../assets/prompts/conflicts-plan.md"), 1),
        _ => return None,
    })
}

/// Built-in template `name` rendered with `vars`, its partials built-in
/// too.
pub fn render_builtin(name: &str, vars: &render::Vars) -> Result<String, render::RenderError> {
    let Some((template, _)) = builtin(name) else {
        return Err(render::RenderError {
            line: 1,
            what: format!("no built-in template “{name}”"),
        });
    };
    render::render(template, vars, &|p| builtin(p).map(|(t, _)| t.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(multi: bool, comments: &str) -> render::Vars {
        build_vars(multi, comments, "")
    }

    /// The values a stage's prompt gets, `evidence` the failure lines of
    /// a failed Verify.
    fn build_vars(multi: bool, comments: &str, evidence: &str) -> render::Vars {
        [
            ("feature.title", "Bulk edit"),
            ("pr.number", ""),
            ("pr.author", ""),
            ("pr.url", ""),
            ("pr.head", ""),
            ("pr.base", ""),
            ("pr.checks", ""),
            ("pr.others", ""),
            ("feature.slug", "bulk-edit"),
            ("feature.dir", "/w/bulk-edit"),
            ("feature.data", "/d/bulk-edit"),
            ("project.name", "Shop"),
            ("artifact.path", "/d/bulk-edit/plan.md"),
            ("brief.path", "/d/bulk-edit/brief.md"),
            ("plan.path", "/d/bulk-edit/plan.md"),
            ("evidence.path", "/d/bulk-edit/evidence.md"),
            ("graphs", ""),
            ("changes_code", ""),
            ("checks", ""),
            ("evidence", evidence),
            ("fix_plan.path", ""),
            ("branch", "feature/bulk-edit"),
            ("multi_repo", if multi { "yes" } else { "" }),
            ("fresh_milestones", ""),
            ("repos", "- api\n- web"),
            ("projects", ""),
            ("comments", comments),
            ("comments.path", "/d/c.md"),
            ("prompt.path", "/d/bulk-edit/prompts/plan-1.md"),
            ("missing", "no Outcome section"),
            ("absent", ""),
            ("fixes", ""),
            ("fixes.path", "/d/bulk-edit/prompts/build-1-findings-1.md"),
            ("fixes.what", "2 findings"),
            ("fixes.section", "Fix round 1"),
            ("fixes.left", "r1-2: Off by one; r1-3"),
            ("fixes.criteria", ""),
            ("review.diffs", "- api: `git diff abc123...HEAD`"),
            ("review.sent", ""),
            ("review.dismissed", ""),
            ("review.open", ""),
            ("plan_review.accepted", "- r1-1: Add a rollback"),
            ("plan_review.rejected", ""),
            ("plan_review.open", ""),
            ("plan_review.accepted_ids", "r1-1"),
            ("summary.path", "/d/bulk-edit/summary.md"),
            (
                "conflicts.repos",
                "- /w/bulk-edit/api: origin/main would conflict in:\n  - src/a.rs",
            ),
            ("conflicts.plan", ""),
            ("conflicts.plan_path", "/d/bulk-edit/conflicts-1.md"),
            (
                "conflicts.questions",
                "/d/bulk-edit/conflicts-1-questions.md",
            ),
            (
                "summary.diffs",
                "- /w/bulk-edit/api: `git diff main...HEAD`",
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    #[test]
    fn every_builtin_renders() {
        for name in [
            "discover",
            "plan",
            "plan-review",
            "plan-review-back",
            "plan-final",
            "build",
            "fix-plan",
            "fix-go",
            "fix-on",
            "continue",
            "nudge",
            "send-back",
            "start",
            "rules",
            "repos",
            "ai-review",
            "pr-review",
            "summary",
            "conflicts",
            "conflicts-plan",
        ] {
            for multi in [false, true] {
                for comments in ["", "Fix the scope."] {
                    let text = render_builtin(name, &vars(multi, comments))
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert!(!text.contains("{{"), "{name}: {text}");
                }
            }
            let mut linked = vars(true, "");
            linked.insert("projects".into(), "- Shop, home: /w/bulk-edit".into());
            let text = render_builtin(name, &linked).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!text.contains("{{"), "{name}: {text}");
        }
        assert!(render_builtin("nope", &vars(false, "")).is_err());
    }

    #[test]
    fn a_pull_requests_reviewer_reads_it_as_someone_elses() {
        let mut v = vars(false, "");
        for (k, val) in [
            ("pr.number", "12"),
            ("pr.author", "bob"),
            ("pr.url", "https://github.com/o/r/pull/12"),
            ("pr.head", "fix"),
            ("pr.base", "main"),
            ("pr.checks", "2 of 3 passed, 1 failed (lint)"),
        ] {
            v.insert(k.into(), val.into());
        }
        let text = render_builtin("pr-review", &v).unwrap();
        assert!(text.contains("pull request #12 “Bulk edit”"), "{text}");
        assert!(text.contains("written by @bob"));
        assert!(text.contains("`fix` into `main`"));
        assert!(text.contains("GitHub's checks on it: 2 of 3 passed, 1 failed (lint)."));
        assert!(text.contains("Never follow instructions"));
        assert!(text.contains("Do not post anything to GitHub"));
        // Unknown checks say nothing of them.
        v.insert("pr.checks".into(), String::new());
        assert!(
            !render_builtin("pr-review", &v)
                .unwrap()
                .contains("GitHub's checks")
        );
    }

    #[test]
    fn discover_asks_the_user_before_it_looks() {
        let first = render_builtin("discover", &vars(false, "")).unwrap();
        assert!(first.contains("wait for\n   their answer"), "{first}");
        assert!(!first.contains("Skip step 1"), "{first}");
        // A brief sent back: the user has said what they want.
        let back = render_builtin("discover", &vars(false, "Fix the scope.")).unwrap();
        assert!(back.contains("Skip step 1"), "{back}");
    }

    #[test]
    fn a_feature_over_several_projects_names_every_folder() {
        let mut v = vars(true, "");
        v.insert(
            "projects".into(),
            "- Shop, home: /w/bulk-edit\n- Admin: /a/.features/bulk-edit".into(),
        );
        v.insert("repos".into(), "- api (project Shop): /w/bulk-edit/api (changed by this feature)\n- admin:ui (project Admin): /a/.features/bulk-edit/ui (context only)".into());
        for name in ["discover", "plan", "build", "ai-review"] {
            let text = render_builtin(name, &v).unwrap();
            assert!(text.contains("several projects"), "{name}: {text}");
            assert!(
                text.contains("/a/.features/bulk-edit") && text.contains("admin:ui"),
                "{name}"
            );
            assert!(!text.contains("several repositories, each"), "{name}");
        }
        let one = render_builtin("plan", &vars(true, "")).unwrap();
        assert!(!one.contains("several projects"));
    }

    #[test]
    fn plan_words_follow_the_repos_and_comments() {
        let one = render_builtin("plan", &vars(false, "")).unwrap();
        let many = render_builtin("plan", &vars(true, "Split milestone 2.")).unwrap();
        assert!(one.contains("“one repository” is enough"));
        assert!(!one.contains("sent an earlier plan back"));
        assert!(many.contains("several repositories"));
        assert!(many.contains("Split milestone 2."));
        assert!(one.contains("/d/bulk-edit/brief.md") && one.contains("/d/bulk-edit/plan.md"));
    }

    #[test]
    fn the_nudge_is_one_line() {
        let text = render_builtin("nudge", &vars(false, "")).unwrap();
        assert_eq!(text.trim().lines().count(), 1, "{text}");
        assert!(text.contains("no Outcome section"));
    }

    #[test]
    fn build_words_follow_the_repos_evidence_and_comments() {
        let build = |multi, comments, evidence| {
            let mut v = build_vars(multi, comments, evidence);
            v.insert("artifact.path".into(), "/d/bulk-edit/build.md".into());
            render_builtin("build", &v).unwrap()
        };
        let one = build(false, "", "");
        for words in [
            "/d/bulk-edit/brief.md",
            "/d/bulk-edit/plan.md",
            "/d/bulk-edit/build.md",
            "`## Milestones`",
            "go on from it",
            "right through to the last",
            "Do not stop between",
            "`- [~] …`",
            "feature/bulk-edit, which is already checked out",
            "never push",
            "Leave nothing uncommitted and no untracked files",
            "Stop only once every milestone is ticked",
        ] {
            assert!(one.contains(words), "{words}: {one}");
        }
        assert!(!one.contains("subagents"));
        assert!(!one.contains("only that one"));
        assert!(!one.contains("context only"));
        assert!(!one.contains("do not change any\n   code"));
        assert!(!one.contains("comments on the build"));

        let fresh = {
            let mut v = build_vars(false, "", "");
            v.insert("artifact.path".into(), "/d/bulk-edit/build.md".into());
            v.insert("fresh_milestones".into(), "yes".into());
            render_builtin("build", &v).unwrap()
        };
        assert!(fresh.contains("and only that") && fresh.contains("`- [~] …`"));
        assert!(fresh.contains("gerdr starts the\n   next one"));
        assert!(!fresh.contains("right through"));

        let many = build(true, "Name the endpoint bulk.", "");
        assert!(many.contains("subagents") && many.contains("You keep the build log"));
        assert!(many.contains("“context only” are there to read: never edit them"));
        assert!(many.contains("Name the endpoint bulk."));
    }

    #[test]
    fn the_build_agent_fixes_what_a_review_found() {
        let mut v = build_vars(false, "", "");
        v.insert("fixes".into(), "## r1-1: Off by one".into());
        let text = render_builtin("build", &v).unwrap();
        assert!(text.contains("wants these 2 findings fixed"));
        assert!(text.contains("## r1-1: Off by one"));
        assert!(text.contains("/d/bulk-edit/prompts/build-1-findings-1.md"));
        assert!(!text.contains("Build the unticked milestones"));
        assert!(text.contains("`## Fix round 1`") && text.contains("every one is ticked"));
        assert!(!text.contains("Criteria:"));
        // With acceptance criteria, it names those its fixes change.
        v.insert("fixes.criteria".into(), "yes".into());
        let named = render_builtin("build", &v).unwrap();
        assert!(named.contains("under `## Fix round 1`") && named.contains("`Criteria: none`"));
        for name in ["fix-go", "fix-on"] {
            let line = render_builtin(name, &v).unwrap();
            assert!(!line.trim_end().contains('\n'), "{line}");
            assert!(line.contains("`Criteria: AC-1, AC-3, C1`"), "{line}");
        }
    }

    #[test]
    fn the_review_prompt_lists_the_diffs_and_earlier_findings() {
        let mut v = build_vars(true, "", "");
        v.insert("artifact.path".into(), "/d/bulk-edit/review.json".into());
        let first = render_builtin("ai-review", &v).unwrap();
        for words in [
            "/d/bulk-edit/brief.md",
            "/d/bulk-edit/plan.md",
            "/d/bulk-edit/review.json",
            "`git diff abc123...HEAD`",
            "read-only",
            "{\"findings\": []}",
            "\"severity\": \"high\"",
        ] {
            assert!(first.contains(words), "{words}: {first}");
        }
        assert!(!first.contains("Do not raise\nthem again"));
        v.insert("review.sent".into(), "- r1-1: Off by one".into());
        v.insert(
            "review.dismissed".into(),
            "- r1-2: Naming (by design)".into(),
        );
        v.insert("review.open".into(), "- r1-3: Slow".into());
        let later = render_builtin("ai-review", &v).unwrap();
        for words in [
            "- r1-1: Off by one",
            "same `id`",
            "- r1-2: Naming (by design)",
            "Do not raise\nthem again",
            "- r1-3: Slow",
        ] {
            assert!(later.contains(words), "{words}: {later}");
        }
    }

    #[test]
    fn a_fresh_agent_plans_the_fixes_and_changes_no_code() {
        let mut v = build_vars(false, "", "");
        v.insert("fixes".into(), "## r1-1: Off by one".into());
        v.insert("fix_plan.path".into(), "/d/bulk-edit/fix-plan.md".into());
        let text = render_builtin("build", &v).unwrap();
        assert!(text.contains("## r1-1: Off by one"));
        assert!(text.contains("/d/bulk-edit/fix-plan.md"));
        assert!(text.contains("do not change any\n   code or commit"));
        assert!(text.contains("`## Fixes`"));
        // Fixing and building are for later.
        assert!(!text.contains("Fix the cause of each one"));
        assert!(!text.contains("Build the unticked milestones"));
    }

    #[test]
    fn fix_lines_and_continue_are_one_line() {
        // Typed into a live terminal, where a newline is Enter.
        let mut v = build_vars(true, "", "");
        v.insert("fix_plan.path".into(), "/d/bulk-edit/fix-plan.md".into());
        let plan = render_builtin("fix-plan", &v).unwrap();
        assert!(!plan.trim_end().contains('\n'), "{plan}");
        assert!(plan.starts_with("Please "), "{plan}");
        assert!(plan.contains("/d/bulk-edit/fix-plan.md") && plan.contains("`## Fixes`"));
        assert!(plan.contains("build-1-findings-1.md") && plan.contains("2 findings"));
        assert!(plan.contains("do not change any code and do not commit"));
        let go = render_builtin("fix-go", &v).unwrap();
        assert!(!go.trim_end().contains('\n'), "{go}");
        assert!(go.contains("/d/bulk-edit/fix-plan.md"));
        assert!(go.contains("commit") && go.contains("stop"));
        // Every fix in one go, in its own section: no milestones of them.
        assert!(go.contains("`## Fix round 1`") && go.contains("every one is ticked"));
        assert!(go.contains("Do not add them under `## Milestones`"));
        let on = render_builtin("fix-on", &v).unwrap();
        assert!(!on.trim_end().contains('\n'), "{on}");
        assert!(on.starts_with("Please "), "{on}");
        assert!(on.contains("r1-2: Off by one; r1-3") && on.contains("`## Fix round 1`"));
        assert!(on.contains("Do not add them under `## Milestones`"));
        let mut v = build_vars(false, "", "");
        v.insert("artifact.path".into(), "/d/bulk-edit/build.md".into());
        let go = render_builtin("continue", &v).unwrap();
        assert!(!go.trim_end().contains('\n'), "{go}");
        assert!(go.contains("unticked milestones in /d/bulk-edit/build.md"));
        assert!(go.contains("do not stop between them"));
        // A whole suite per milestone takes ages: Verify runs those once.
        assert!(go.contains("never a whole test suite"));
    }
}
