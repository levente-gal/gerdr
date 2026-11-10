//! Submitting a pull request's review to GitHub: what its stages found,
//! as one review of the reviews API, from Ship's page of the review (see
//! `pr_post`).
//!
//! What goes: AI review's findings marked to comment, the user's comments
//! of Code review (`review::Comment`), and Manual test's failures and
//! comments not sent yet. Each on a line of the diff goes on that line, if
//! GitHub's diff of the pull request shows it (`Hunks`, of
//! `<base>...<head>`): an added or kept line on its new side (`RIGHT`), a
//! removed one on its old side (`LEFT`), a quote of several lines from its
//! first. The rest go into the review's text, after the user's own words
//! (`BODY_FILE`), each saying where it is.
//!
//! Pure: reading the comments, the diff, and posting are `pr_post`'s.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    Feature,
    manual::{Check, Criterion},
    review_import::FindingState,
};
use crate::{diff::LineKind, review};

/// The user's own words over the review, in the review's data dir.
pub const BODY_FILE: &str = "review.md";

/// What a submitted review asks of the pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    Comment,
    Approve,
    RequestChanges,
}

impl Event {
    /// As the reviews API takes it.
    pub fn api(self) -> &'static str {
        match self {
            Self::Comment => "COMMENT",
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
        }
    }

    /// Its button: "Request changes…".
    pub fn label(self) -> &'static str {
        match self {
            Self::Comment => "Comment\u{2026}",
            Self::Approve => "Approve\u{2026}",
            Self::RequestChanges => "Request changes\u{2026}",
        }
    }

    /// In a sentence: "requested changes".
    pub fn done(self) -> &'static str {
        match self {
            Self::Comment => "commented",
            Self::Approve => "approved",
            Self::RequestChanges => "requested changes",
        }
    }
}

/// The side of a diff a comment goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The base's: a removed line.
    Left,
    /// The head's: an added or kept line.
    Right,
}

impl Side {
    fn api(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

/// Where an item is in the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    /// From the repository's top folder.
    pub path: String,
    /// Its line on `side`, and the first of several it quotes, if so.
    pub line: Option<(Side, usize, Option<usize>)>,
}

/// What an item came from, to mark once it is posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum From {
    /// AI review's finding, by id.
    Finding(String),
    /// The user's comment of Code review, as kept.
    Comment(review::Comment),
    /// Manual test's failure or comment, by criterion id.
    Manual(String),
}

/// One thing to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub from: From,
    /// The repository it is in, by `Repo::key`: its pull request's review
    /// takes it. None goes to the first pull request's.
    pub repo: Option<String>,
    /// Where it is, if anywhere.
    pub spot: Option<Spot>,
    /// What it says, as Markdown.
    pub body: String,
    /// One line naming it, for the page.
    pub title: String,
}

/// What goes into the review: `items` of `feature`, its comments of Code
/// review `own` (its file's), and Manual test's `manual` (`manual::unsent`).
pub fn items(
    feature: &Feature,
    own: &review::Review,
    manual: &[(&Criterion, &Check)],
) -> Vec<Item> {
    let mut out = Vec::new();
    let findings = feature.review.iter().flat_map(|r| &r.findings);
    for f in findings.filter(|f| f.state == FindingState::Fix) {
        let mut body = format!("**{}**", f.title.trim());
        if !f.body.trim().is_empty() {
            body.push_str(&format!("\n\n{}", f.body.trim()));
        }
        let fix = f.fix_text().trim();
        if !fix.is_empty() {
            body.push_str(&format!("\n\nSuggested fix: {fix}"));
        }
        if !f.note.trim().is_empty() {
            body.push_str(&format!("\n\n{}", f.note.trim()));
        }
        let spot = (!f.file.is_empty()).then(|| Spot {
            path: f.file.clone(),
            line: f.line.map(|n| (Side::Right, n, None)),
        });
        out.push(Item {
            repo: (!f.repo.is_empty()).then(|| f.repo.clone()),
            from: From::Finding(f.id.clone()),
            spot,
            body,
            title: f.title.trim().to_owned(),
        });
    }
    for c in own
        .comments
        .iter()
        .filter(|c| c.flag.is_none() && !c.text.trim().is_empty())
    {
        let spot = (!c.path.is_empty()).then(|| Spot {
            path: c.path.clone(),
            line: comment_line(&c.lines),
        });
        let first = c.text.trim().lines().next().unwrap_or_default().to_owned();
        out.push(Item {
            repo: (feature.repos.iter())
                .find(|r| r.worktree == c.root)
                .map(super::Repo::key),
            from: From::Comment(c.clone()),
            spot,
            body: c.text.trim().to_owned(),
            title: first,
        });
    }
    for (criterion, check) in manual {
        let body = match criterion.comment {
            true => {
                let mut text = criterion.text.trim().to_owned();
                if !check.note.trim().is_empty() {
                    text.push_str(&format!("\n\n{}", check.note.trim()));
                }
                text
            }
            false => format!(
                "**Tried by hand: {}** \u{2014} it failed.\n\n{}\n\nWhat went wrong: {}",
                criterion.title.trim(),
                criterion.text.trim(),
                check.note.trim()
            ),
        };
        out.push(Item {
            repo: None,
            from: From::Manual(criterion.id.clone()),
            spot: None,
            title: match criterion.comment {
                true => criterion
                    .text
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                false => format!("{} failed", criterion.title.trim()),
            },
            body,
        });
    }
    out
}

/// Where a comment quoting `lines` goes: its last line, on its side, from
/// its first on the same side.
fn comment_line(lines: &[review::Quoted]) -> Option<(Side, usize, Option<usize>)> {
    let side = |q: &review::Quoted| match q.kind {
        LineKind::Removed => Side::Left,
        LineKind::Added | LineKind::Context => Side::Right,
    };
    let last = lines.last()?;
    let first = (lines.iter())
        .find(|q| side(q) == side(last))
        .filter(|q| q.number < last.number)
        .map(|q| q.number);
    Some((side(last), last.number, first))
}

/// The lines of one side of a hunk; none for a side with no lines (a new
/// file's old one).
type Span = Option<std::ops::RangeInclusive<usize>>;

/// The lines a diff shows, per file: each hunk's on both sides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunks(BTreeMap<String, Vec<(Span, Span)>>);

impl Hunks {
    /// Those of `git diff`'s `text`, by each file's new path.
    pub fn of(text: &str) -> Self {
        let mut out: BTreeMap<String, Vec<_>> = BTreeMap::new();
        let mut file: Option<String> = None;
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("+++ ") {
                file = path.strip_prefix("b/").map(str::to_owned);
            } else if let Some(rest) = line.strip_prefix("@@ -")
                && let Some(file) = &file
                && let Some((ranges, _)) = rest.split_once(" @@")
                && let Some((old, new)) = ranges.split_once(" +")
            {
                let range = |r: &str| {
                    let (start, len) = r.split_once(',').unwrap_or((r, "1"));
                    let (start, len): (usize, usize) = (start.parse().ok()?, len.parse().ok()?);
                    Some((len > 0).then(|| start..=start + len - 1))
                };
                if let (Some(old), Some(new)) = (range(old), range(new)) {
                    out.entry(file.clone()).or_default().push((old, new));
                }
            }
        }
        Self(out)
    }

    /// Whether line `n` of `path` on `side` is one it shows.
    pub fn shows(&self, path: &str, side: Side, n: usize) -> bool {
        (self.0.get(path).into_iter().flatten()).any(|(old, new)| match side {
            Side::Left => old.as_ref().is_some_and(|r| r.contains(&n)),
            Side::Right => new.as_ref().is_some_and(|r| r.contains(&n)),
        })
    }
}

/// `items` laid out against `hunks`: those on lines it shows, the others.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub inline: Vec<Item>,
    pub in_body: Vec<Item>,
}

/// Lay `items` out (see `Placed`); all in the text when `inline` is off.
pub fn place(items: Vec<Item>, hunks: &Hunks, inline: bool) -> Placed {
    let mut placed = Placed::default();
    for item in items {
        let on = (item.spot.as_ref()).and_then(|s| {
            let (side, n, first) = s.line?;
            let shown = |n| hunks.shows(&s.path, side, n);
            (inline && shown(n) && first.is_none_or(shown)).then_some(())
        });
        match on {
            Some(()) => placed.inline.push(item),
            None => placed.in_body.push(item),
        }
    }
    placed
}

/// The review's text: the user's own words `own`, then what couldn't go on
/// a line, each saying where it is. Not empty when `event` needs one.
pub fn body(own: &str, placed: &Placed, event: Event) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !own.trim().is_empty() {
        parts.push(own.trim().to_owned());
    }
    for item in &placed.in_body {
        let at = match &item.spot {
            Some(Spot {
                path,
                line: Some((_, n, _)),
            }) => format!("`{path}:{n}`: "),
            Some(Spot { path, line: None }) => format!("`{path}`: "),
            None => String::new(),
        };
        parts.push(format!("{at}{}", item.body));
    }
    if parts.is_empty() && event != Event::Approve {
        parts.push("See the comments on the code.".to_owned());
    }
    parts.join("\n\n---\n\n")
}

/// What `gh api -X POST repos/<slug>/pulls/<n>/reviews` gets: `placed`
/// on commit `commit`, the user's words `own`.
pub fn payload(commit: &str, event: Event, own: &str, placed: &Placed) -> serde_json::Value {
    let comments: Vec<serde_json::Value> = (placed.inline.iter())
        .filter_map(|item| {
            let spot = item.spot.as_ref()?;
            let (side, line, first) = spot.line?;
            let mut c = serde_json::json!({
                "path": spot.path,
                "body": item.body,
                "line": line,
                "side": side.api(),
            });
            if let Some(first) = first {
                c["start_line"] = first.into();
                c["start_side"] = side.api().into();
            }
            Some(c)
        })
        .collect();
    serde_json::json!({
        "commit_id": commit,
        "event": event.api(),
        "body": body(own, placed, event),
        "comments": comments,
    })
}

/// `items` of the review of pull request `n` of `feature`'s (`pull.repo`,
/// its first or not): those of its repository, and for its first, those of
/// none or of a repository without one.
pub fn of_pull(feature: &Feature, items: &[Item], repo: &str, first: bool) -> Vec<Item> {
    let has = |key: &str| feature.pulls().iter().any(|p| p.repo == key);
    (items.iter())
        .filter(|item| match &item.repo {
            Some(key) if key == repo => true,
            Some(key) => first && !has(key),
            None => first,
        })
        .cloned()
        .collect()
}

/// A review submitted, kept in the record (`Shipping::reviews`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    /// The repository of the pull request it was on, by `Repo::key`; empty
    /// in a review of one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// Unix seconds.
    pub at: u64,
    pub event: Event,
    /// Its page on GitHub.
    pub url: String,
    /// The head it was on.
    pub commit: String,
    /// How many items went on lines, and into its text.
    pub inline: usize,
    pub in_body: usize,
    /// Its comments all went into its text, as GitHub refused their lines.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flattened: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{
        manual::Verdict,
        review_import::{AiReview, Finding},
    };

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,4 +10,6 @@ fn x() {\n ctx\n+new\n+new\n ctx\n-old\n ctx\n@@ -40 +42 @@\n-a\n+b\ndiff --git a/new.rs b/new.rs\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,3 @@\n+1\n+2\n+3\n";

    #[test]
    fn hunks_say_which_lines_github_shows() {
        let h = Hunks::of(DIFF);
        assert!(h.shows("src/a.rs", Side::Right, 10) && h.shows("src/a.rs", Side::Right, 15));
        assert!(!h.shows("src/a.rs", Side::Right, 16));
        assert!(h.shows("src/a.rs", Side::Left, 13) && !h.shows("src/a.rs", Side::Left, 14));
        assert!(h.shows("src/a.rs", Side::Right, 42) && h.shows("src/a.rs", Side::Left, 40));
        assert!(h.shows("new.rs", Side::Right, 3) && !h.shows("new.rs", Side::Left, 0));
        assert!(!h.shows("other.rs", Side::Right, 1));
    }

    fn quoted(kind: LineKind, number: usize) -> review::Quoted {
        review::Quoted {
            kind,
            number,
            text: String::new(),
        }
    }

    #[test]
    fn everything_found_goes_on_its_line_or_into_the_text() {
        let finding = |id: &str, line, state| Finding {
            id: id.into(),
            file: "src/a.rs".into(),
            line,
            title: format!("Bug {id}"),
            body: "It breaks.".into(),
            fix: "Check it.".into(),
            state,
            ..Finding::default()
        };
        let feature = Feature {
            review: Some(AiReview {
                findings: vec![
                    finding("r1-1", Some(11), FindingState::Fix),
                    finding("r1-2", Some(99), FindingState::Fix),
                    finding("r1-3", Some(11), FindingState::Dismissed),
                ],
                ..AiReview::default()
            }),
            ..Feature::default()
        };
        let comment = |path: &str, lines| review::Comment {
            root: "/w".into(),
            path: path.into(),
            lines,
            text: "Rename this".into(),
            flag: None,
        };
        let own = review::Review {
            comments: vec![
                comment(
                    "src/a.rs",
                    vec![quoted(LineKind::Context, 10), quoted(LineKind::Added, 12)],
                ),
                comment("src/a.rs", vec![quoted(LineKind::Removed, 13)]),
                comment("", vec![]),
            ],
            ..review::Review::default()
        };
        let ac = Criterion {
            id: "AC-1".into(),
            title: "Log in".into(),
            text: "Given a user, when they log in, then it works".into(),
            comment: false,
        };
        let check = Check {
            id: "AC-1".into(),
            text: ac.text.clone(),
            state: Verdict::Fail,
            note: "500 error".into(),
            at: 1,
            sent: false,
            patch: String::new(),
            made: false,
            shots: Vec::new(),
        };
        let all = items(&feature, &own, &[(&ac, &check)]);
        assert_eq!(all.len(), 6);
        assert_eq!(
            all[0].body,
            "**Bug r1-1**\n\nIt breaks.\n\nSuggested fix: Check it."
        );
        assert_eq!(
            all[2].spot.as_ref().unwrap().line,
            Some((Side::Right, 12, Some(10)))
        );
        assert_eq!(
            all[3].spot.as_ref().unwrap().line,
            Some((Side::Left, 13, None))
        );
        let placed = place(all.clone(), &Hunks::of(DIFF), true);
        let ids = |items: &[Item]| items.iter().map(|i| i.title.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&placed.inline),
            vec!["Bug r1-1", "Rename this", "Rename this"]
        );
        assert_eq!(
            ids(&placed.in_body),
            vec!["Bug r1-2", "Rename this", "Log in failed"]
        );
        let json = payload("abc", Event::RequestChanges, "Thanks!", &placed);
        assert_eq!(json["event"], "REQUEST_CHANGES");
        assert_eq!(json["commit_id"], "abc");
        assert_eq!(json["comments"][1]["start_line"], 10);
        assert_eq!(json["comments"][2]["side"], "LEFT");
        let text = json["body"].as_str().unwrap();
        assert!(
            text.starts_with("Thanks!\n\n---\n\n`src/a.rs:99`: **Bug r1-2**"),
            "{text}"
        );
        assert!(text.contains("**Tried by hand: Log in**"));
        // Refused lines: all of it in the text.
        let flat = place(all, &Hunks::of(DIFF), false);
        assert!(flat.inline.is_empty() && flat.in_body.len() == 6);
    }

    #[test]
    fn each_pull_request_gets_what_is_on_its_repository() {
        let item = |repo: Option<&str>, title: &str| Item {
            from: From::Manual(title.into()),
            repo: repo.map(str::to_owned),
            spot: None,
            body: String::new(),
            title: title.into(),
        };
        let pull = |repo: &str| crate::feature::pr::PullRef {
            repo: repo.into(),
            ..Default::default()
        };
        let feature = Feature {
            kind: crate::feature::Kind::PrReview(vec![pull("api"), pull("web")]),
            ..Feature::default()
        };
        let all = vec![
            item(Some("api"), "in api"),
            item(Some("web"), "in web"),
            item(Some("docs"), "in a repo with no pull request"),
            item(None, "general"),
        ];
        let titles = |items: Vec<Item>| items.into_iter().map(|i| i.title).collect::<Vec<_>>();
        assert_eq!(
            titles(of_pull(&feature, &all, "api", true)),
            vec!["in api", "in a repo with no pull request", "general"]
        );
        assert_eq!(
            titles(of_pull(&feature, &all, "web", false)),
            vec!["in web"]
        );
    }

    #[test]
    fn a_review_always_says_something_but_an_approval() {
        let none = Placed::default();
        assert_eq!(body("", &none, Event::Approve), "");
        assert_eq!(
            body("", &none, Event::Comment),
            "See the comments on the code."
        );
        assert_eq!(body(" LGTM \n", &none, Event::Approve), "LGTM");
    }
}
