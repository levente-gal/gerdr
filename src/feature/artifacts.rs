//! The files agent stages write into the feature data dir, and the checks
//! that make one good enough to review: `brief.md` (Discover), `plan.md`
//! (Plan) and `build.md` (Build).
//!
//! A brief needs `## Outcome`, `## In scope`, `## Out of scope` and
//! `## Acceptance criteria` sections, none empty, and at least one
//! criterion in Given/When/Then form. A plan needs `## Milestones` with
//! at least one checkbox item, `## Repositories` and `## Merge order`
//! sections, none empty, and a `Migrations:` line; `named_repos` tells
//! whether it names the touched repositories. Headings match at any
//! level, case-insensitively; fenced code doesn't count.
//!
//! A build log (`build.md`, Build) needs `## Milestones` with at least one
//! checkbox item, and is good once every one is ticked (`- [x] …`);
//! `milestones` counts them, for the stops between milestones. A
//! checkbox item may be a numbered one too (`1. [ ] …`).

use std::path::{Path, PathBuf};

use super::Stage;

/// The file `stage` writes, if any: `brief.md`, `plan.md`.
pub fn name(stage: Stage) -> Option<&'static str> {
    super::agents::needs(stage)?.artifact
}

/// Where `stage`'s file is, under feature data dir `dir`.
pub fn path(dir: &Path, stage: Stage) -> Option<PathBuf> {
    Some(dir.join(name(stage)?))
}

/// Whether `text` is a good `stage` file; else what it lacks, one line
/// each ("no Outcome section").
pub fn check(stage: Stage, text: &str) -> Result<(), Vec<String>> {
    let doc = Sections::of(text);
    let mut lacks = Vec::new();
    match stage {
        Stage::Discover => {
            for heading in ["Outcome", "In scope", "Out of scope", "Acceptance criteria"] {
                doc.need(heading, &mut lacks);
            }
            if let Some(body) = doc.get("Acceptance criteria")
                && !body.is_empty()
                && !body.iter().any(|line| given_when_then(line))
            {
                lacks.push("no acceptance criterion as Given/When/Then".into());
            }
        }
        Stage::Plan => {
            if doc.need("Milestones", &mut lacks)
                && !doc
                    .get("Milestones")
                    .unwrap_or_default()
                    .iter()
                    .any(|l| checkbox(l).is_some())
            {
                lacks.push("no milestone as a checkbox item (“- [ ] …”)".into());
            }
            doc.need("Repositories", &mut lacks);
            doc.need("Merge order", &mut lacks);
            if !doc.lines.iter().any(|line| migrations_line(line)) {
                lacks.push("no “Migrations:” line".into());
            }
        }
        Stage::Build if doc.need("Milestones", &mut lacks) => match milestones(text) {
            (_, 0) => lacks.push("no milestone as a checkbox item (“- [ ] …”)".into()),
            (ticked, total) if ticked < total => lacks.push(format!(
                "{} of {total} milestones not ticked",
                total - ticked
            )),
            _ => {}
        },
        _ => {}
    }
    if lacks.is_empty() { Ok(()) } else { Err(lacks) }
}

/// The checkbox items of `text`'s Milestones section: how many are ticked
/// (`[x]` or `[X]`), and how many there are.
pub fn milestones(text: &str) -> (usize, usize) {
    let body = Sections::of(text).get("Milestones").unwrap_or_default();
    let boxes: Vec<&str> = body.iter().filter_map(|l| checkbox(l)).collect();
    let ticked = boxes.iter().filter(|b| **b != "[ ]").count();
    (ticked, boxes.len())
}

/// The repositories of `repos` the plan's Repositories section doesn't
/// name (a word match, case-insensitive).
pub fn unnamed_repos<'a>(text: &str, repos: &[&'a str]) -> Vec<&'a str> {
    let doc = Sections::of(text);
    let body = doc
        .get("Repositories")
        .unwrap_or_default()
        .join("\n")
        .to_lowercase();
    repos
        .iter()
        .copied()
        .filter(|repo| {
            let repo = repo.to_lowercase();
            !body
                .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
                .any(|word| word.trim_end_matches('.') == repo)
        })
        .collect()
}

/// A Markdown file cut at its headings; code fences left out.
struct Sections<'a> {
    /// Every line outside fences.
    lines: Vec<&'a str>,
    /// Each heading, normalised, with its body's lines.
    sections: Vec<(String, Vec<&'a str>)>,
}

impl<'a> Sections<'a> {
    fn of(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut sections: Vec<(String, Vec<&'a str>)> = Vec::new();
        let mut fenced = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            lines.push(line);
            if let Some(heading) = heading(trimmed) {
                sections.push((heading, Vec::new()));
            } else if let Some((_, body)) = sections.last_mut()
                && !trimmed.is_empty()
            {
                body.push(trimmed);
            }
        }
        Sections { lines, sections }
    }

    /// The body of the first section headed `heading`.
    fn get(&self, heading: &str) -> Option<Vec<&'a str>> {
        let want = heading.to_lowercase();
        self.sections
            .iter()
            .find(|(h, _)| *h == want)
            .map(|(_, body)| body.clone())
    }

    /// Note in `lacks` when section `heading` is missing or empty; true
    /// when it is there with a body.
    fn need(&self, heading: &str, lacks: &mut Vec<String>) -> bool {
        match self.get(heading) {
            None => lacks.push(format!("no {heading} section")),
            Some(body) if body.is_empty() => lacks.push(format!("empty {heading} section")),
            Some(_) => return true,
        }
        false
    }
}

/// The heading `line` (trimmed) is, lowercased, its marks and a trailing
/// colon off.
fn heading(line: &str) -> Option<String> {
    let rest = line.trim_start_matches('#');
    if rest.len() == line.len() || !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let text = rest
        .trim()
        .trim_end_matches('#')
        .trim()
        .trim_end_matches(':')
        .trim();
    Some(text.to_lowercase())
}

/// Whether `line` reads Given …, when …, then …, in that order.
fn given_when_then(line: &str) -> bool {
    let words: Vec<String> = line
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let at = |word: &str, from: usize| {
        words[from..]
            .iter()
            .position(|w| w == word)
            .map(|i| i + from)
    };
    at("given", 0)
        .and_then(|g| at("when", g + 1))
        .and_then(|w| at("then", w + 1))
        .is_some()
}

/// The box of checkbox list item `line` (trimmed): `[ ]`, `[x]` or
/// `[X]`. A numbered item (`1. [ ] …`, `1) [ ] …`) counts too.
fn checkbox(line: &str) -> Option<&'static str> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = if digits > 0 {
        line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
    } else {
        line.strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("+ "))
    };
    let rest = rest?.trim_start();
    ["[ ]", "[x]", "[X]"]
        .into_iter()
        .find(|b| rest.starts_with(b))
}

/// Whether `line` is the `Migrations:` line: as a list item or in bold
/// too, or a `Migrations` heading.
fn migrations_line(line: &str) -> bool {
    let line = line.trim().trim_start_matches(['-', '*', '+', '#', ' ']);
    let line = line.trim_start_matches('*').trim_start_matches('_');
    let lower = line.to_lowercase();
    lower.starts_with("migrations:")
        || lower.starts_with("migrations**:")
        || lower.starts_with("migrations__:")
        || lower == "migrations"
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRIEF: &str = "# Bulk edit\n\n## Outcome\nEdit many rows at once.\n\n## In scope\n- price\n\n## Out of Scope:\n- stock\n\n## Acceptance criteria\n- Given two rows, when I edit both, then both change.\n";

    const PLAN: &str = "## Milestones\n- [ ] API\n- [x] UI\n\n## Repositories\n- api: endpoint\n- web: form\n\n## Merge order\napi then web\n\n**Migrations:** none\n";

    #[test]
    fn names_follow_the_stages() {
        assert_eq!(name(Stage::Discover), Some("brief.md"));
        assert_eq!(name(Stage::Plan), Some("plan.md"));
        assert_eq!(name(Stage::Build), Some("build.md"));
        assert_eq!(name(Stage::Verify), None);
        assert_eq!(
            path(Path::new("/d"), Stage::Plan),
            Some(PathBuf::from("/d/plan.md"))
        );
    }

    #[test]
    fn a_good_brief_passes() {
        assert_eq!(check(Stage::Discover, BRIEF), Ok(()));
    }

    #[test]
    fn a_brief_lists_what_it_lacks() {
        assert_eq!(
            check(Stage::Discover, "## Outcome\n\n## In scope\n- a\n"),
            Err(vec![
                "empty Outcome section".to_owned(),
                "no Out of scope section".to_owned(),
                "no Acceptance criteria section".to_owned(),
            ])
        );
        let vague = BRIEF.replace("Given two rows, when I edit both, then", "It works and");
        assert_eq!(
            check(Stage::Discover, &vague),
            Err(vec![
                "no acceptance criterion as Given/When/Then".to_owned()
            ])
        );
        // Headings inside code don't count.
        let fenced = format!("```\n{BRIEF}```\n");
        assert_eq!(check(Stage::Discover, &fenced).unwrap_err().len(), 4);
        // Nor does a hashtag.
        assert!(check(Stage::Discover, "#Outcome\nx").is_err());
    }

    #[test]
    fn a_plan_needs_milestones_repositories_merge_order_and_migrations() {
        assert_eq!(check(Stage::Plan, PLAN), Ok(()));
        assert_eq!(
            check(
                Stage::Plan,
                &PLAN.replace("**Migrations:**", "- Migrations:")
            ),
            Ok(())
        );
        assert_eq!(
            check(Stage::Plan, "## Milestones\n1. API\n## Repositories\n"),
            Err(vec![
                "no milestone as a checkbox item (“- [ ] …”)".to_owned(),
                "empty Repositories section".to_owned(),
                "no Merge order section".to_owned(),
                "no “Migrations:” line".to_owned(),
            ])
        );
        // A stage without a file has nothing to check.
        assert_eq!(check(Stage::Verify, ""), Ok(()));
    }

    #[test]
    fn a_build_log_is_good_once_every_milestone_is_ticked() {
        let log = "# Build\n\n## Milestones\n- [x] API\n  Endpoint added.\n* [X] UI\n1. [ ] Docs\n2) [ ] Tests\n\n## Notes\n- [ ] not a milestone\n";
        assert_eq!(milestones(log), (2, 4));
        assert_eq!(
            check(Stage::Build, log),
            Err(vec!["2 of 4 milestones not ticked".to_owned()])
        );
        let done = log
            .replace("[ ] Docs", "[x] Docs")
            .replace("[ ] Tests", "[x] Tests");
        assert_eq!(milestones(&done), (4, 4));
        assert_eq!(check(Stage::Build, &done), Ok(()));
        // Boxes in code don't count.
        let fenced = "## Milestones\n```\n- [ ] a\n```\n- [x] b\n";
        assert_eq!(milestones(fenced), (1, 1));
    }

    #[test]
    fn a_build_log_needs_milestones_as_checkboxes() {
        assert_eq!(milestones(""), (0, 0));
        assert_eq!(
            check(Stage::Build, ""),
            Err(vec!["no Milestones section".to_owned()])
        );
        assert_eq!(
            check(Stage::Build, "## Milestones\n- API\n1. UI\n"),
            Err(vec![
                "no milestone as a checkbox item (“- [ ] …”)".to_owned()
            ])
        );
        assert_eq!(milestones("## Milestones\n- API\n- [x]API\n"), (1, 1));
    }

    #[test]
    fn the_plan_names_the_touched_repositories() {
        assert!(unnamed_repos(PLAN, &["api", "web"]).is_empty());
        assert_eq!(
            unnamed_repos(PLAN, &["api", "worker", "Web"]),
            vec!["worker"]
        );
        assert_eq!(unnamed_repos("x", &["api"]), vec!["api"]);
    }
}
