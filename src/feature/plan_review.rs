//! Plan review: another agent checks the approved plan against the code and
//! writes suggestions, the user decides each one, and what was accepted
//! makes the final plan Build reads.
//!
//! `plan-review.json` (the reviewer writes it in the feature data dir):
//!
//! ```json
//! {"suggestions": [{"id": "r1-2", "severity": "high", "kind": "gap",
//!   "plan": "Milestone 3", "title": "…", "why": "…", "change": "…",
//!   "evidence": [{"repo": "api", "file": "src/a.rs", "line": 12,
//!                 "quote": "…"}]}]}
//! ```
//!
//! - `kind`: `gap`, `risk`, `edge-case`, `simplify` or `question`.
//! - `plan`: where in the plan it applies.
//! - `why`: what goes wrong, as a concrete case.
//! - `change`: what to change in the plan. A question may leave it out.
//! - `evidence`: where the code says so. Only a question may have none.
//! - `id`: only to raise a suggestion of an earlier run again; gerdr gives
//!   new ones their own, `r<run>-<k>`.
//!
//! The file is untrusted: it is capped, its text is cleaned as AI review's
//! is (`review_import::clean`), and a citation is only believed once gerdr
//! finds its quote at the line it names (`check_citations`). One it can't
//! find is kept and shown as not found: the user decides what it is worth.
//!
//! A decision follows its suggestion's id through later runs. A suggestion
//! a later run leaves out stays, marked `dropped`: what the user accepted
//! isn't lost to a rewrite. The user's own suggestions are never touched.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    agents::StageRun,
    review_import::{Severity, clean},
};

/// The file the reviewer writes, in the feature data dir.
pub const FILE: &str = "plan-review.json";
/// The largest `plan-review.json` read.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most suggestions kept of one run. The prompt asks for far fewer.
pub const MAX_SUGGESTIONS: usize = 40;
/// The most citations kept of one suggestion.
pub const MAX_EVIDENCE: usize = 6;
/// The most lacks listed of a bad file.
const MAX_LACKS: usize = 8;
/// How many lines either side of the one cited its quote may be on.
const NEAR: usize = 3;
/// The largest source file read for a citation.
const MAX_SOURCE: u64 = 4 * 1024 * 1024;

/// What sort of suggestion it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Gap,
    Risk,
    EdgeCase,
    Simplify,
    Question,
}

impl Kind {
    /// As the suggestions show it: `edge case`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Gap => "gap",
            Self::Risk => "risk",
            Self::EdgeCase => "edge case",
            Self::Simplify => "simplify",
            Self::Question => "question",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.to_lowercase().replace([' ', '_'], "-").as_str() {
            "gap" => Some(Self::Gap),
            "risk" => Some(Self::Risk),
            "edge-case" => Some(Self::EdgeCase),
            "simplify" => Some(Self::Simplify),
            "question" => Some(Self::Question),
            _ => None,
        }
    }
}

/// Where a suggestion stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Not decided yet.
    #[default]
    Open,
    Accepted,
    Rejected,
}

/// Who suggested it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    #[default]
    Reviewer,
    You,
}

/// Where the code says so.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The repository's `Repo::key`, or as the reviewer wrote it when it
    /// matches none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// From the repository's top folder.
    pub file: String,
    /// 1-based.
    pub line: usize,
    pub quote: String,
    /// Whether gerdr found the quote there; none until it looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<bool>,
}

/// One suggestion.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// `r<run>-<k>`, or `u<k>` for one of the user's: unique within the
    /// feature.
    pub id: String,
    pub severity: Severity,
    pub kind: Kind,
    /// Where in the plan it applies.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plan: String,
    pub title: String,
    /// What goes wrong, as a concrete case.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// What to change in the plan.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub state: State,
    /// What the user said of it; a question's answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub source: Source,
    /// The run that raised it last; 0 for one of the user's.
    #[serde(default)]
    pub run: u32,
    /// The last run left it out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dropped: bool,
}

/// The feature's plan review, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReview {
    /// The latest run read in: 1, 2…
    pub n: u32,
    /// Every suggestion, in the order raised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    /// The suggestions are decided and the final plan is being written.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writing: bool,
    /// When and how big the file read in was, to tell a rewrite: seconds,
    /// bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<(u64, u64)>,
    /// The code's patch-id when the review started: the code it mustn't
    /// change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    /// The reviewer's run while the final plan's writer has the stage's
    /// run: Change decisions brings it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<StageRun>,
}

impl PlanReview {
    /// The suggestion `id`.
    pub fn suggestion(&self, id: &str) -> Option<&Suggestion> {
        self.suggestions.iter().find(|s| s.id == id)
    }

    /// How many suggestions are in `state`.
    pub fn count(&self, state: State) -> usize {
        self.suggestions.iter().filter(|s| s.state == state).count()
    }

    /// How many suggestions wait for a decision.
    pub fn open(&self) -> usize {
        self.count(State::Open)
    }

    /// Whether anything was accepted: else there is nothing for a final
    /// plan to add, and the plan is used as it is.
    pub fn any_accepted(&self) -> bool {
        self.count(State::Accepted) > 0
    }

    /// How many citations gerdr couldn't find.
    pub fn not_found(&self) -> usize {
        (self.suggestions.iter())
            .flat_map(|s| &s.evidence)
            .filter(|e| e.found == Some(false))
            .count()
    }
}

/// One suggestion as the file has it, cleaned and capped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    /// An earlier suggestion's id, to raise it again.
    pub id: Option<String>,
    pub severity: Severity,
    pub kind: Kind,
    pub plan: String,
    pub title: String,
    pub why: String,
    pub change: String,
    pub evidence: Vec<Evidence>,
}

/// The suggestions of `plan-review.json` reading `text`; else what it lacks,
/// one line each, for the agent to be told. An empty list is good: nothing
/// is a valid thing to find.
pub fn parse(text: &str) -> Result<Vec<Raw>, Vec<String>> {
    if text.len() as u64 > MAX_BYTES {
        return Err(vec![format!(
            "{FILE} is larger than {} KiB",
            MAX_BYTES / 1024
        )]);
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| vec![format!("it isn't JSON: {e}")])?;
    let Some(list) = value.get("suggestions").and_then(Value::as_array) else {
        return Err(vec!["no \u{201c}suggestions\u{201d} list".into()]);
    };
    let mut lacks = Vec::new();
    let mut out = Vec::new();
    for (i, item) in list.iter().take(MAX_SUGGESTIONS).enumerate() {
        let k = i + 1;
        let Some(obj) = item.as_object() else {
            lacks.push(format!("suggestion {k} isn't an object"));
            continue;
        };
        let text_of = |key: &str| obj.get(key).and_then(Value::as_str).unwrap_or("");
        let kind = match Kind::parse(text_of("kind")) {
            Some(kind) => kind,
            None => {
                lacks.push(format!(
                    "suggestion {k}'s kind isn't gap, risk, edge-case, simplify or question"
                ));
                Kind::Gap
            }
        };
        let severity = match obj.get("severity") {
            None | Some(Value::Null) => Severity::Medium,
            Some(v) => match v.as_str().map(str::to_lowercase).as_deref() {
                Some("high") => Severity::High,
                Some("medium") => Severity::Medium,
                Some("low") => Severity::Low,
                _ => {
                    lacks.push(format!(
                        "suggestion {k}'s severity isn't high, medium or low"
                    ));
                    Severity::Medium
                }
            },
        };
        let raw = Raw {
            id: Some(clean(text_of("id"), false).chars().take(40).collect())
                .filter(|id: &String| !id.is_empty()),
            severity,
            kind,
            plan: clean(text_of("plan"), false),
            title: clean(text_of("title"), false),
            why: clean(text_of("why"), true),
            change: clean(text_of("change"), true),
            evidence: evidence_of(obj.get("evidence"), k, &mut lacks),
        };
        let mut lack = |what: &str| lacks.push(format!("suggestion {k} has no {what}"));
        for (text, what) in [
            (&raw.title, "title"),
            (&raw.plan, "plan reference"),
            (&raw.why, "why"),
        ] {
            if text.is_empty() {
                lack(what);
            }
        }
        if kind != Kind::Question {
            if raw.change.is_empty() {
                lack("change");
            }
            if raw.evidence.is_empty() {
                lack("evidence: cite the code, or make it a question");
            }
        }
        out.push(raw);
    }
    if lacks.len() > MAX_LACKS {
        let more = lacks.len() - MAX_LACKS;
        lacks.truncate(MAX_LACKS);
        lacks.push(format!("{more} more"));
    }
    if lacks.is_empty() {
        Ok(out)
    } else {
        Err(lacks)
    }
}

/// The citations of suggestion `k`, what is wrong with them added to `lacks`.
fn evidence_of(value: Option<&Value>, k: usize, lacks: &mut Vec<String>) -> Vec<Evidence> {
    let Some(list) = value.filter(|v| !v.is_null()) else {
        return Vec::new();
    };
    let Some(list) = list.as_array() else {
        lacks.push(format!("suggestion {k}'s evidence isn't a list"));
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in list.iter().take(MAX_EVIDENCE) {
        let text_of = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
        let file = clean(text_of("file"), false);
        let quote = clean(text_of("quote"), false);
        let line = item.get("line").and_then(Value::as_u64).filter(|n| *n >= 1);
        let (false, false, Some(line)) = (file.is_empty(), quote.is_empty(), line) else {
            lacks.push(format!(
                "suggestion {k}'s evidence needs a file, a line from 1 and a quote"
            ));
            continue;
        };
        out.push(Evidence {
            repo: clean(text_of("repo"), false),
            file,
            line: usize::try_from(line).unwrap_or(usize::MAX),
            quote,
            found: None,
        });
    }
    out
}

/// What reading a run's suggestions in came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Raised for the first time.
    pub new: usize,
    /// Earlier ones raised again (by their id).
    pub again: usize,
    /// Earlier ones this run left out.
    pub dropped: usize,
}

/// Read run `run`'s suggestions (`raws`) into `review`:
///
/// - one naming an earlier one's id raises it again: its text and run
///   follow, its decision and note stay;
/// - any other is new, its id `r<run>-<k>`;
/// - an earlier one of the reviewer's not raised again stays, `dropped`;
/// - the user's own are left alone.
pub fn import(review: &mut PlanReview, run: u32, raws: Vec<Raw>) -> Imported {
    let mut out = Imported::default();
    let before = review.suggestions.len();
    let mut raised: BTreeSet<String> = BTreeSet::new();
    let mut k = 0;
    for raw in raws {
        let earlier = (raw.id.as_ref())
            .filter(|id| !raised.contains(*id))
            .and_then(|id| {
                review.suggestions[..before]
                    .iter()
                    .position(|s| s.id == *id && s.source == Source::Reviewer)
            });
        if let Some(i) = earlier {
            let s = &mut review.suggestions[i];
            raised.insert(s.id.clone());
            (s.severity, s.kind) = (raw.severity, raw.kind);
            (s.plan, s.title) = (raw.plan, raw.title);
            (s.why, s.change) = (raw.why, raw.change);
            s.evidence = raw.evidence;
            s.run = run;
            s.dropped = false;
            out.again += 1;
            continue;
        }
        let id = loop {
            k += 1;
            let id = format!("r{run}-{k}");
            if review.suggestion(&id).is_none() {
                break id;
            }
        };
        review.suggestions.push(Suggestion {
            id,
            severity: raw.severity,
            kind: raw.kind,
            plan: raw.plan,
            title: raw.title,
            why: raw.why,
            change: raw.change,
            evidence: raw.evidence,
            run,
            ..Suggestion::default()
        });
        out.new += 1;
    }
    for s in &mut review.suggestions[..before] {
        if s.source == Source::Reviewer && !raised.contains(&s.id) {
            if !s.dropped {
                out.dropped += 1;
            }
            s.dropped = true;
        }
    }
    review.n = run;
    out
}

/// What the user decides of a suggestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Reject,
    /// Back to undecided; one of the user's own goes.
    Clear,
}

/// Decide suggestion `id` of `review`; why not when it can't be. A question
/// is accepted by answering it, so it needs its note first.
pub fn decide(review: &mut PlanReview, id: &str, decision: Decision) -> Result<(), String> {
    let Some(i) = review.suggestions.iter().position(|s| s.id == id) else {
        return Err("That suggestion is gone".into());
    };
    let s = &mut review.suggestions[i];
    match decision {
        Decision::Accept if s.kind == Kind::Question && s.note.trim().is_empty() => {
            return Err("Answer the question first: write it as a note".into());
        }
        Decision::Accept => s.state = State::Accepted,
        Decision::Reject => s.state = State::Rejected,
        Decision::Clear if s.source == Source::You => {
            review.suggestions.remove(i);
        }
        Decision::Clear => s.state = State::Open,
    }
    Ok(())
}

/// Set suggestion `id`'s note; whether it is there.
pub fn set_note(review: &mut PlanReview, id: &str, note: &str) -> bool {
    let Some(s) = review.suggestions.iter_mut().find(|s| s.id == id) else {
        return false;
    };
    s.note = note.trim().to_owned();
    true
}

/// Add the user's own suggestion, accepted: writing it is deciding it.
/// Its id.
pub fn add_own(review: &mut PlanReview, text: &str) -> String {
    let id = (1..)
        .map(|k| format!("u{k}"))
        .find(|id| review.suggestion(id).is_none())
        .unwrap_or_default();
    review.suggestions.push(Suggestion {
        id: id.clone(),
        title: clean(text, false),
        state: State::Accepted,
        source: Source::You,
        ..Suggestion::default()
    });
    id
}

/// A line of text as a citation compares it: cleaned as the file's quote
/// was, spaces collapsed.
fn normal(text: &str) -> String {
    clean(text, false)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Which of `repos` (keys and worktrees) `evidence` names, and its file
/// inside it: by key, by plain name when only one has it, or the only one
/// there is. An absolute path names the repository it is under.
fn locate<'a>(evidence: &Evidence, repos: &'a [(String, PathBuf)]) -> Option<&'a Path> {
    let file = Path::new(&evidence.file);
    if file.is_absolute() {
        return (repos.iter())
            .find(|(_, worktree)| file.starts_with(worktree))
            .map(|(_, worktree)| worktree.as_path());
    }
    let repo = evidence.repo.as_str();
    if let Some((_, worktree)) = repos.iter().find(|(key, _)| key == repo) {
        return Some(worktree);
    }
    if repo.is_empty() {
        return match repos {
            [(_, worktree)] => Some(worktree),
            _ => None,
        };
    }
    let mut by_name = (repos.iter()).filter(|(key, _)| key.rsplit(':').next() == Some(repo));
    match (by_name.next(), by_name.next()) {
        (Some((_, worktree)), None) => Some(worktree),
        _ => None,
    }
}

/// Whether `evidence`'s quote is on or near the line it names, in the
/// repository of `repos` it names. A file outside that repository is not
/// read: the path comes from the agent. Reads files: not for the UI thread.
fn citation_holds(evidence: &Evidence, repos: &[(String, PathBuf)]) -> bool {
    let Some(worktree) = locate(evidence, repos) else {
        return false;
    };
    let (Ok(root), Ok(path)) = (
        worktree.canonicalize(),
        worktree.join(&evidence.file).canonicalize(),
    ) else {
        return false;
    };
    if !path.starts_with(&root) {
        return false;
    }
    let small = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_SOURCE);
    let Some(bytes) = small.then(|| std::fs::read(&path).ok()).flatten() else {
        return false;
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let first = evidence.line.saturating_sub(NEAR + 1).min(lines.len());
    let last = evidence.line.saturating_add(NEAR).min(lines.len());
    let window = normal(&lines[first..last].join(" "));
    let quote = normal(&evidence.quote);
    !quote.is_empty() && window.contains(&quote)
}

/// Mark whether each citation of `raws` holds, before they are read in.
/// Reads files: not for the UI thread.
pub fn check_citations(raws: &mut [Raw], repos: &[(String, PathBuf)]) {
    for evidence in raws.iter_mut().flat_map(|raw| &mut raw.evidence) {
        evidence.found = Some(citation_holds(evidence, repos));
    }
}

/// The user's notes on the suggestions, as a list for the reviewer.
pub fn notes(review: &PlanReview) -> String {
    (review.suggestions.iter())
        .filter(|s| !s.note.is_empty() && s.source == Source::Reviewer)
        .map(|s| format!("- {}: {}: {}", s.id, s.title, s.note.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The suggestions with state `state` as a list for an agent: id, title,
/// what to change, and the user's note.
fn listed(review: &PlanReview, state: State, change: bool) -> String {
    let mut out = String::new();
    for s in (review.suggestions.iter()).filter(|s| s.state == state) {
        out.push_str(&format!("- {}: {}", s.id, s.title));
        if !s.plan.is_empty() {
            out.push_str(&format!(" ({})", s.plan));
        }
        out.push('\n');
        if change && !s.change.is_empty() {
            out.push_str(&format!("  Change: {}\n", s.change.replace('\n', " ")));
        }
        if change {
            for e in &s.evidence {
                let repo = if e.repo.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", e.repo)
                };
                out.push_str(&format!("  Cited: {repo}{}:{}\n", e.file, e.line));
            }
        }
        if !s.note.is_empty() {
            out.push_str(&format!("  The user said: {}\n", s.note.replace('\n', " ")));
        }
    }
    out.trim_end().to_owned()
}

/// What a reviewer's or a writer's prompt is told of the suggestions so far.
pub fn prompt_vars(review: Option<&PlanReview>) -> [(&'static str, String); 4] {
    let empty = PlanReview::default();
    let review = review.unwrap_or(&empty);
    let open = (review.suggestions.iter())
        .filter(|s| s.source == Source::Reviewer && s.state == State::Open)
        .map(|s| format!("- {}: {}", s.id, s.title))
        .collect::<Vec<_>>()
        .join("\n");
    [
        ("plan_review.accepted", listed(review, State::Accepted, true)),
        ("plan_review.rejected", listed(review, State::Rejected, false)),
        ("plan_review.open", open),
        (
            "plan_review.accepted_ids",
            (review.suggestions.iter())
                .filter(|s| s.state == State::Accepted)
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    ]
}

/// The accepted suggestions `text`'s `## Review changes` section doesn't
/// name: a final plan has to say where each one went.
pub fn unmapped<'a>(review: &'a PlanReview, text: &str) -> Vec<&'a str> {
    let section = super::artifacts::section(text, "review changes").unwrap_or_default();
    let body = section.join("\n");
    let names = |id: &str| {
        body.match_indices(id).any(|(at, _)| {
            let word = |c: char| c.is_alphanumeric() || c == '-';
            !body[..at].ends_with(word) && !body[at + id.len()..].starts_with(word)
        })
    };
    (review.suggestions.iter())
        .filter(|s| s.state == State::Accepted && !names(&s.id))
        .map(|s| s.id.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"suggestions": [
        {"severity": "high", "kind": "gap", "plan": "Milestone 3",
         "title": "No rollback", "why": "A failed run leaves rows", "change": "Add a rollback",
         "evidence": [{"repo": "api", "file": "src/a.rs", "line": 2, "quote": "fn a()"}]},
        {"kind": "question", "plan": "Milestone 1", "title": "Which queue?", "why": "Two exist"}
    ]}"#;

    fn raws(text: &str) -> Vec<Raw> {
        parse(text).unwrap()
    }

    #[test]
    fn a_good_file_reads_and_nothing_found_is_good_too() {
        let all = raws(GOOD);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].severity, Severity::High);
        assert_eq!(all[0].evidence[0].line, 2);
        assert_eq!(all[1].kind, Kind::Question);
        assert_eq!(all[1].severity, Severity::Medium);
        assert!(raws(r#"{"suggestions": []}"#).is_empty());
    }

    #[test]
    fn a_bad_file_says_what_it_lacks() {
        assert_eq!(
            parse("{}").unwrap_err(),
            ["no \u{201c}suggestions\u{201d} list"]
        );
        assert!(parse("nope").unwrap_err()[0].starts_with("it isn't JSON"));
        let big = format!("{{\"suggestions\":[], \"x\":\"{}\"}}", "x".repeat(300_000));
        assert!(parse(&big).unwrap_err()[0].contains("larger than"));
        let lacks = parse(r#"{"suggestions": [{"kind": "gap", "severity": "huge"}, 3]}"#)
            .unwrap_err();
        for wanted in [
            "suggestion 1's severity isn't high, medium or low",
            "suggestion 1 has no title",
            "suggestion 1 has no plan reference",
            "suggestion 1 has no why",
            "suggestion 1 has no change",
            "suggestion 1 has no evidence: cite the code, or make it a question",
            "suggestion 2 isn't an object",
        ] {
            assert!(lacks.iter().any(|l| l == wanted), "{wanted}: {lacks:?}");
        }
        let lacks = parse(
            r#"{"suggestions": [{"kind": "nope", "plan": "p", "title": "t", "why": "w",
                "change": "c", "evidence": [{"file": "a", "line": 0, "quote": "q"}]}]}"#,
        )
        .unwrap_err();
        assert!(lacks[0].contains("kind isn't"));
        assert!(lacks[1].contains("needs a file, a line from 1 and a quote"));
    }

    #[test]
    fn a_list_too_long_is_cut_and_its_text_is_cleaned() {
        let one = r#"{"kind":"question","plan":"p","title":"<script>t</script>\u202e","why":"w"}"#;
        let many = vec![one; MAX_SUGGESTIONS + 5].join(",");
        let all = raws(&format!("{{\"suggestions\":[{many}]}}"));
        assert_eq!(all.len(), MAX_SUGGESTIONS);
        assert_eq!(all[0].title, "t");
    }

    #[test]
    fn decisions_follow_the_id_through_a_rewrite() {
        let mut review = PlanReview::default();
        let first = import(&mut review, 1, raws(GOOD));
        assert_eq!(first.new, 2);
        assert_eq!(review.n, 1);
        assert_eq!(review.suggestions[0].id, "r1-1");
        decide(&mut review, "r1-1", Decision::Accept).unwrap();
        set_note(&mut review, "r1-1", " keep it short ");
        let own = add_own(&mut review, "Add a metric");
        assert_eq!(own, "u1");
        // A rewrite keeps r1-1, reworded, and leaves the question out.
        let rewrite = r#"{"suggestions": [
            {"id": "r1-1", "kind": "risk", "plan": "Milestone 3", "title": "Reworded",
             "why": "w", "change": "c",
             "evidence": [{"file": "a", "line": 1, "quote": "q"}]},
            {"id": "u1", "kind": "question", "plan": "p", "title": "Takes the id of yours", "why": "w"}
        ]}"#;
        let again = import(&mut review, 1, raws(rewrite));
        assert_eq!(
            again,
            Imported {
                new: 1,
                again: 1,
                dropped: 1
            }
        );
        let kept = review.suggestion("r1-1").unwrap();
        assert_eq!(kept.title, "Reworded");
        assert_eq!(kept.state, State::Accepted);
        assert_eq!(kept.note, "keep it short");
        assert!(!kept.dropped);
        assert!(review.suggestion("r1-2").unwrap().dropped);
        // An agent can't overwrite the user's own by its id.
        let yours = review.suggestion("u1").unwrap();
        assert_eq!(yours.title, "Add a metric");
        assert_eq!(yours.state, State::Accepted);
        assert_eq!(review.suggestions.len(), 4);
        // Raised again later, it isn't dropped any more.
        let back = r#"{"suggestions": [{"id": "r1-2", "kind": "question", "plan": "p",
            "title": "Back", "why": "w"}]}"#;
        import(&mut review, 2, raws(back));
        assert!(!review.suggestion("r1-2").unwrap().dropped);
        assert!(review.suggestion("r1-1").unwrap().dropped);
        assert_eq!(review.suggestion("r1-1").unwrap().state, State::Accepted);
    }

    #[test]
    fn a_question_is_accepted_by_answering_it() {
        let mut review = PlanReview::default();
        import(&mut review, 1, raws(GOOD));
        assert!(
            decide(&mut review, "r1-2", Decision::Accept)
                .unwrap_err()
                .contains("Answer the question")
        );
        assert_eq!(review.open(), 2);
        set_note(&mut review, "r1-2", "The new one");
        decide(&mut review, "r1-2", Decision::Accept).unwrap();
        decide(&mut review, "r1-1", Decision::Reject).unwrap();
        assert_eq!(review.open(), 0);
        decide(&mut review, "r1-1", Decision::Clear).unwrap();
        assert_eq!(review.open(), 1);
        assert!(decide(&mut review, "gone", Decision::Accept).is_err());
    }

    #[test]
    fn clearing_your_own_takes_it_away() {
        let mut review = PlanReview::default();
        let own = add_own(&mut review, "One <b>more</b>");
        assert_eq!(review.suggestion(&own).unwrap().title, "One more");
        assert!(review.any_accepted());
        decide(&mut review, &own, Decision::Clear).unwrap();
        assert!(review.suggestions.is_empty());
        assert!(!review.any_accepted());
    }

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-plan-review-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(dir.join("api/src")).unwrap();
            std::fs::create_dir_all(dir.join("web")).unwrap();
            std::fs::write(
                dir.join("api/src/a.rs"),
                "one\n\tfn   a<T>() {\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            )
            .unwrap();
            std::fs::write(dir.join("secret.txt"), "the secret\n").unwrap();
            std::fs::write(dir.join("web/w.ts"), "web line\n").unwrap();
            Self(dir)
        }

        fn repos(&self) -> Vec<(String, PathBuf)> {
            vec![
                ("api".into(), self.0.join("api")),
                ("other:web".into(), self.0.join("web")),
            ]
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cited(repo: &str, file: &str, line: usize, quote: &str) -> Evidence {
        Evidence {
            repo: repo.into(),
            file: file.into(),
            line,
            quote: quote.into(),
            found: None,
        }
    }

    #[test]
    fn a_citation_holds_only_where_it_says() {
        let t = Temp::new();
        let repos = t.repos();
        let holds = |e: Evidence| citation_holds(&e, &repos);
        // Spaces, tabs and tags don't count: the file's quote lost them too.
        assert!(holds(cited("api", "src/a.rs", 2, "fn a<T>() {")));
        // A few lines off still reads as the same place.
        assert!(holds(cited("api", "src/a.rs", 5, "fn a<T>()")));
        // Too far off doesn't.
        assert!(!holds(cited("api", "src/a.rs", 6, "fn a<T>()")));
        assert!(!holds(cited("api", "src/a.rs", 2, "not in the file")));
        assert!(!holds(cited("api", "src/none.rs", 2, "one")));
        // Past the end of the file.
        assert!(!holds(cited("api", "src/a.rs", 500, "one")));
        // A linked repository, by key or plain name.
        assert!(holds(cited("other:web", "w.ts", 1, "web line")));
        assert!(holds(cited("web", "w.ts", 1, "web line")));
        assert!(!holds(cited("nope", "w.ts", 1, "web line")));
        // No repository, several to pick from.
        assert!(!holds(cited("", "src/a.rs", 2, "fn")));
    }

    #[test]
    fn a_citation_never_reads_outside_its_repository() {
        let t = Temp::new();
        let repos = t.repos();
        let secret = t.0.join("secret.txt").display().to_string();
        let holds = |e: Evidence| citation_holds(&e, &repos);
        assert!(!holds(cited("api", "../secret.txt", 1, "the secret")));
        assert!(!holds(cited("api", &secret, 1, "the secret")));
        let inside = t.0.join("api/src/a.rs").display().to_string();
        assert!(holds(cited("api", &inside, 1, "one")));
        // A link out is outside too.
        std::os::unix::fs::symlink(t.0.join("secret.txt"), t.0.join("api/link")).unwrap();
        assert!(!holds(cited("api", "link", 1, "the secret")));
        // Too big to read is not found, not read.
        let big = t.0.join("api/big");
        std::fs::write(&big, "x".repeat((MAX_SOURCE + 1) as usize)).unwrap();
        assert!(!holds(cited("api", "big", 1, "x")));
    }

    #[test]
    fn what_the_check_found_is_marked_and_counted() {
        let t = Temp::new();
        let mut all = raws(GOOD);
        // Only the first has citations, and "fn a()" isn't what's there.
        check_citations(&mut all, &t.repos());
        let mut review = PlanReview::default();
        import(&mut review, 1, all);
        assert_eq!(review.not_found(), 1);
        let evidence = &review.suggestion("r1-1").unwrap().evidence[0];
        assert_eq!(evidence.found, Some(false));
        set_note(&mut review, "r1-1", "Which rows?");
        add_own(&mut review, "Mine, with no note");
        set_note(&mut review, "u1", "Not for the reviewer");
        assert_eq!(notes(&review), "- r1-1: No rollback: Which rows?");
    }

    #[test]
    fn prompts_are_told_what_was_decided() {
        let mut review = PlanReview::default();
        import(&mut review, 1, raws(GOOD));
        set_note(&mut review, "r1-2", "The new one");
        decide(&mut review, "r1-1", Decision::Reject).unwrap();
        let [(_, accepted), (_, rejected), (_, open), (_, ids)] = prompt_vars(Some(&review));
        assert_eq!(accepted, "");
        assert_eq!(rejected, "- r1-1: No rollback (Milestone 3)");
        assert_eq!(open, "- r1-2: Which queue?");
        assert_eq!(ids, "");
        decide(&mut review, "r1-1", Decision::Accept).unwrap();
        decide(&mut review, "r1-2", Decision::Accept).unwrap();
        add_own(&mut review, "Add a metric");
        let [(_, accepted), (_, rejected), (_, open), (_, ids)] = prompt_vars(Some(&review));
        assert_eq!(
            accepted,
            "- r1-1: No rollback (Milestone 3)\n  Change: Add a rollback\n  Cited: api: src/a.rs:2\n\
             - r1-2: Which queue? (Milestone 1)\n  The user said: The new one\n\
             - u1: Add a metric"
        );
        assert_eq!((rejected.as_str(), open.as_str()), ("", ""));
        assert_eq!(ids, "r1-1, r1-2, u1");
        assert_eq!(prompt_vars(None)[0].1, "");
    }

    #[test]
    fn a_final_plan_has_to_say_where_each_accepted_one_went() {
        let mut review = PlanReview::default();
        import(&mut review, 1, raws(GOOD));
        set_note(&mut review, "r1-2", "Yes");
        decide(&mut review, "r1-1", Decision::Accept).unwrap();
        decide(&mut review, "r1-2", Decision::Accept).unwrap();
        assert_eq!(unmapped(&review, "# no section"), ["r1-1", "r1-2"]);
        // Named elsewhere doesn't count; a longer id doesn't either.
        let text = "## Milestones\n- [ ] r1-2\n\n## Review changes\n- r1-1: Milestone 2\n- r1-22: x\n";
        assert_eq!(unmapped(&review, text), ["r1-2"]);
        let both = "## Review changes\n- r1-1: Milestone 2\n- r1-2: Milestone 1\n";
        assert!(unmapped(&review, both).is_empty());
    }
}
