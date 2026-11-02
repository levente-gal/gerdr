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
        "build" => (include_str!("../../assets/prompts/build.md"), 1),
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
        [
            ("feature.title", "Bulk edit"),
            ("feature.slug", "bulk-edit"),
            ("feature.dir", "/w/bulk-edit"),
            ("feature.data", "/d/bulk-edit"),
            ("project.name", "Shop"),
            ("artifact.path", "/d/bulk-edit/plan.md"),
            ("brief.path", "/d/bulk-edit/brief.md"),
            ("multi_repo", if multi { "yes" } else { "" }),
            ("repos", "- api\n- web"),
            ("comments", comments),
            ("comments.path", "/d/c.md"),
            ("missing", "no Outcome section"),
            ("absent", ""),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    #[test]
    fn every_builtin_renders() {
        for name in ["discover", "plan", "nudge", "send-back", "rules", "repos"] {
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
}
