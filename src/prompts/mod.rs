//! Prompts for the feature stages' agents: templates rendered with the
//! feature's values (see `render`). Only the built-in templates for now,
//! from `assets/prompts/` (see `builtin`); a library of the user's own
//! comes later.

#![allow(dead_code)]

pub mod render;

/// The built-in template or partial `name` ("plan", "rules"), and its
/// version: bumped when its text changes meaning.
pub fn builtin(name: &str) -> Option<(&'static str, u32)> {
    Some(match name {
        "discover" => (include_str!("../../assets/prompts/discover.md"), 1),
        "plan" => (include_str!("../../assets/prompts/plan.md"), 1),
        "build" => (include_str!("../../assets/prompts/build.md"), 2),
        "ai-review" => (include_str!("../../assets/prompts/ai-review.md"), 1),
        "send-fixes" => (include_str!("../../assets/prompts/send-fixes.md"), 1),
        "fix" => (include_str!("../../assets/prompts/fix.md"), 1),
        "continue" => (include_str!("../../assets/prompts/continue.md"), 1),
        "nudge" => (include_str!("../../assets/prompts/nudge.md"), 1),
        "send-back" => (include_str!("../../assets/prompts/send-back.md"), 1),
        "rules" => (include_str!("../../assets/prompts/rules.md"), 1),
        "repos" => (include_str!("../../assets/prompts/repos.md"), 1),
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
            ("feature.slug", "bulk-edit"),
            ("feature.dir", "/w/bulk-edit"),
            ("feature.data", "/d/bulk-edit"),
            ("project.name", "Shop"),
            ("artifact.path", "/d/bulk-edit/plan.md"),
            ("brief.path", "/d/bulk-edit/brief.md"),
            ("plan.path", "/d/bulk-edit/plan.md"),
            ("evidence.path", "/d/bulk-edit/evidence.md"),
            ("evidence", evidence),
            ("branch", "feature/bulk-edit"),
            ("multi_repo", if multi { "yes" } else { "" }),
            ("repos", "- api\n- web"),
            ("comments", comments),
            ("comments.path", "/d/c.md"),
            ("missing", "no Outcome section"),
            ("absent", ""),
            ("fixes", ""),
            ("fixes.path", "/d/bulk-edit/prompts/build-1-findings-1.md"),
            ("fixes.what", "2 findings"),
            ("review.diffs", "- api: `git diff abc123...HEAD`"),
            ("review.sent", ""),
            ("review.dismissed", ""),
            ("review.open", ""),
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
            "build",
            "fix",
            "continue",
            "nudge",
            "send-back",
            "rules",
            "repos",
            "ai-review",
            "send-fixes",
        ] {
            for multi in [false, true] {
                for comments in ["", "Fix the scope."] {
                    let text = render_builtin(name, &vars(multi, comments))
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert!(!text.contains("{{"), "{name}: {text}");
                }
            }
        }
        assert!(render_builtin("nope", &vars(false, "")).is_err());
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
            "only that one",
            "feature/bulk-edit, which is already checked out",
            "never push",
            "Leave nothing uncommitted and no untracked files",
            "Then STOP.",
            "gerdr waits for",
            "do not start the next milestone on\n   your own",
        ] {
            assert!(one.contains(words), "{words}: {one}");
        }
        assert!(!one.contains("context only"));
        assert!(!one.contains("Verify failed"));
        assert!(!one.contains("comments on the build"));

        let many = build(
            true,
            "Name the endpoint bulk.",
            "api: `npm test` exited with 1",
        );
        assert!(many.contains("“context only” are there to read: never edit them"));
        assert!(many.contains("Verify failed on the code"));
        assert!(many.contains("api: `npm test` exited with 1"));
        assert!(many.contains("/d/bulk-edit/evidence.md"));
        assert!(many.contains("fix the\n   failures, commit the fixes, and stop"));
        // A fix round builds no milestone.
        assert!(!many.contains("Build the first unticked milestone"));
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
        assert!(!text.contains("Build the first unticked milestone"));
        let line = render_builtin("send-fixes", &v).unwrap();
        assert!(!line.trim_end().contains('\n'), "{line}");
        assert!(line.contains("2 findings") && line.contains("build-1-findings-1.md"));
        // Typed into a pane: a shell there must not take it for a command.
        assert!(line.starts_with("Please "), "{line}");
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
    fn fix_and_continue_are_one_line() {
        // Typed into a live terminal, where a newline is Enter.
        let fix = render_builtin("fix", &build_vars(true, "", "x")).unwrap();
        assert!(!fix.trim_end().contains('\n'), "{fix}");
        assert!(fix.contains("Verify failed") && fix.contains("/d/bulk-edit/evidence.md"));
        assert!(fix.contains("commit") && fix.contains("stop"));
        let mut v = build_vars(false, "", "");
        v.insert("artifact.path".into(), "/d/bulk-edit/build.md".into());
        let go = render_builtin("continue", &v).unwrap();
        assert!(!go.trim_end().contains('\n'), "{go}");
        assert!(go.contains("next unticked milestone in /d/bulk-edit/build.md"));
        assert!(go.contains("stop after it"));
    }
}
