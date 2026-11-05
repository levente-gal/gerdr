//! AI review's findings: what the reviewer agent wrote to `review.json`,
//! read in and kept in the feature record (`Feature::review`), and what
//! the user decided of each.
//!
//! `review.json` (the reviewer writes it in the feature data dir):
//!
//! ```json
//! {"findings": [{"id": "r1-2", "severity": "high", "repo": "api",
//!   "file": "src/a.rs", "line": 12, "title": "…", "body": "…", "fix": "…"}]}
//! ```
//!
//! - `severity`: `high`, `medium` or `low`.
//! - `repo`: the repository's key (`Repo::key`: its folder name, or
//!   `<project>:<name>` in a linked project); may be left out for a
//!   feature over one repository.
//! - `file`: from the repository's top folder. `line`: on the new side of
//!   the diff, 1-based; may be left out for the whole file.
//! - `id`: only to raise a finding of an earlier run again (the prompt
//!   lists them); gerdr gives new ones their own, `r<run>-<k>`.
//!
//! The file is untrusted: it is capped (`MAX_BYTES`, `MAX_FINDINGS`, each
//! text `MAX_TEXT` characters), control characters and HTML tags are taken
//! out, and a finding on a file or line not in the diff is kept but not
//! `placed`.
//!
//! What the user decides (`Decision`): Fix, or Dismiss with a reason. They
//! may also reword a finding's fix and comment on it (`edit`), and add an
//! item of their own (`add`). Once
//! none is `Open`, the ones to fix go to the Build agent together (state
//! `Sent`); its fix changes the code, the feature goes back to Build, and
//! after Verify the reviewer runs again on what changed since it last
//! read the code (`AiReview::reviewed`), told which findings were sent to
//! be fixed. A sent finding it doesn't raise again is `Fixed`. AI review
//! holds once no finding is `Open`, `Fix` or `Sent`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Feature,
    layout::{base_ref, git},
    patch::{self, Reading},
};

/// The file the reviewer writes, in the feature data dir.
pub const REVIEW: &str = "review.json";
/// The largest `review.json` read.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most findings kept of one run.
pub const MAX_FINDINGS: usize = 100;
/// The most characters kept of a finding's title, body or fix.
pub const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    High,
    #[default]
    Medium,
    Low,
}

impl Severity {
    /// As the diff and panel show it: `high`.
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// Where a finding stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingState {
    /// Not decided yet.
    #[default]
    Open,
    /// The user wants it fixed; not sent yet.
    Fix,
    /// Sent to the Build agent to fix.
    Sent,
    /// Sent, and the next review didn't raise it again.
    Fixed,
    /// The user dismissed it (`Finding::reason` says why).
    Dismissed,
}

impl FindingState {
    /// Whether AI review still waits on it.
    pub fn holds_up(self) -> bool {
        matches!(self, Self::Open | Self::Fix | Self::Sent)
    }
}

/// One finding of the reviewer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// `r<run>-<k>`: unique within the feature.
    pub id: String,
    pub severity: Severity,
    /// The repository's `Repo::key`.
    pub repo: String,
    /// From the repository's top folder.
    pub file: String,
    /// On the new side of the diff, 1-based; none for the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// What the reviewer suggests doing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fix: String,
    #[serde(default)]
    pub state: FindingState,
    /// Why it was dismissed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The user's wording of the fix, in place of `fix`, which stays the
    /// reviewer's: a run raising the finding again rewrites that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_fix: Option<String>,
    /// The user's comment on it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The user added it: no reviewer raised it, and it may have no file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yours: bool,
    /// The review run that raised it last.
    pub run: u32,
    /// Its file (and line) is in the diff it was read against.
    #[serde(default)]
    pub placed: bool,
}

impl Finding {
    /// The fix to make: the user's wording, else the reviewer's.
    pub fn fix_text(&self) -> &str {
        self.my_fix.as_deref().unwrap_or(&self.fix)
    }
}

/// What the user decides of a finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Fix,
    /// Dismissed, with the reason (not empty).
    Dismiss(String),
    /// Back to undecided (from Fix or Dismissed).
    Reopen,
}

/// Why a finding sent to be fixed isn't decided again.
pub const SENT: &str = "That finding went to the Build agent to fix";
/// Why a fixed finding isn't decided again.
pub const FIXED: &str = "That finding is fixed";
/// The reason an item of the user's own is dismissed for: they needn't
/// tell themselves why.
pub const DROPPED: &str = "dropped by you";

/// What `f` does to a finding in `state`: to fix (from undecided or
/// dismissed), or back to undecided (from to fix); else why it does
/// nothing.
pub fn fix_key(state: FindingState) -> Result<Decision, &'static str> {
    match state {
        FindingState::Open | FindingState::Dismissed => Ok(Decision::Fix),
        FindingState::Fix => Ok(Decision::Reopen),
        FindingState::Sent => Err(SENT),
        FindingState::Fixed => Err(FIXED),
    }
}

/// What Del does to a finding in `state`: `None` to ask why and dismiss it
/// (from undecided or to fix), `Some` back to undecided (from dismissed);
/// else why it does nothing.
pub fn dismiss_key(state: FindingState) -> Result<Option<Decision>, &'static str> {
    match state {
        FindingState::Open | FindingState::Fix => Ok(None),
        FindingState::Dismissed => Ok(Some(Decision::Reopen)),
        FindingState::Sent => Err(SENT),
        FindingState::Fixed => Err(FIXED),
    }
}

/// A review as the user last saw it, kept when the next one is read: the
/// findings of the record change as later runs raise them again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRound {
    /// The run it came from: 1, 2…
    pub n: u32,
    /// What its page listed, each with the words and the decision it had.
    /// One sent to Build says how that went: fixed, or still sent when the
    /// next run raised it again.
    pub findings: Vec<Finding>,
}

/// The feature's AI review, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReview {
    /// The latest run read in: 1, 2…
    pub n: u32,
    /// Every finding of every run, in the order raised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    /// Per touched repository, the commit its code was at when the last
    /// run read it: the next run reviews what changed since.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reviewed: BTreeMap<String, String>,
    /// The feature's patch-id the last run was read on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// The reviews before the latest, oldest first; none with nothing to
    /// show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rounds: Vec<ReviewRound>,
}

impl AiReview {
    /// What the latest review lists: every finding but those fixed that an
    /// earlier review in `rounds` shows. Those still to decide, still with
    /// Build and dismissed carry on from earlier runs, as they can be acted
    /// on only here.
    pub fn shown(&self) -> Vec<&Finding> {
        let kept =
            |id: &str| (self.rounds.iter()).any(|round| round.findings.iter().any(|f| f.id == id));
        (self.findings.iter())
            .filter(|f| f.state != FindingState::Fixed || !kept(&f.id))
            .collect()
    }

    /// How many findings AI review still waits on.
    pub fn open(&self) -> usize {
        self.findings.iter().filter(|f| f.state.holds_up()).count()
    }

    /// The finding `id`.
    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }

    /// How many findings are in `state`.
    pub fn count(&self, state: FindingState) -> usize {
        self.findings.iter().filter(|f| f.state == state).count()
    }

    /// The first finding not decided yet, in the order raised.
    pub fn first_open(&self) -> Option<&Finding> {
        self.findings.iter().find(|f| f.state == FindingState::Open)
    }
}

/// Per touched repository, per file of its diff, the new side's line
/// ranges its hunks show (first and last, 1-based, context included).
pub type Diff = BTreeMap<String, BTreeMap<String, Vec<(usize, usize)>>>;

/// One finding as `review.json` has it, cleaned and capped; not placed
/// yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    /// An earlier finding's id, to raise it again.
    pub id: Option<String>,
    pub severity: Severity,
    pub repo: Option<String>,
    pub file: String,
    pub line: Option<usize>,
    pub title: String,
    pub body: String,
    pub fix: String,
}

/// The most lacks listed of a bad `review.json`.
const MAX_LACKS: usize = 8;

/// The findings of `review.json` reading `text`, cleaned (see `clean`)
/// and capped at `MAX_FINDINGS`; else what it lacks, one line each ("no
/// “findings” list", "finding 2 has no title"). An empty list is good.
pub fn parse(text: &str) -> Result<Vec<Raw>, Vec<String>> {
    if text.len() as u64 > MAX_BYTES {
        return Err(vec![format!(
            "{REVIEW} is larger than {} KiB",
            MAX_BYTES / 1024
        )]);
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| vec![format!("it isn't JSON: {e}")])?;
    let Some(list) = value.get("findings").and_then(Value::as_array) else {
        return Err(vec!["no \u{201c}findings\u{201d} list".into()]);
    };
    let mut lacks = Vec::new();
    let mut out = Vec::new();
    for (i, item) in list.iter().take(MAX_FINDINGS).enumerate() {
        let k = i + 1;
        let Some(obj) = item.as_object() else {
            lacks.push(format!("finding {k} isn't an object"));
            continue;
        };
        let text_of = |key: &str| obj.get(key).and_then(Value::as_str).unwrap_or("");
        let title = clean(text_of("title"), false);
        if title.is_empty() {
            lacks.push(format!("finding {k} has no title"));
        }
        let file = clean(text_of("file"), false);
        if file.is_empty() {
            lacks.push(format!("finding {k} has no file"));
        }
        let severity = match obj.get("severity") {
            None | Some(Value::Null) => Severity::Medium,
            Some(v) => match v.as_str().map(str::to_lowercase).as_deref() {
                Some("high") => Severity::High,
                Some("medium") => Severity::Medium,
                Some("low") => Severity::Low,
                _ => {
                    lacks.push(format!("finding {k}'s severity isn't high, medium or low"));
                    Severity::Medium
                }
            },
        };
        let line = match obj.get("line") {
            None | Some(Value::Null) => None,
            Some(v) => match v.as_u64().filter(|n| *n >= 1) {
                Some(n) => Some(usize::try_from(n).unwrap_or(usize::MAX)),
                None => {
                    lacks.push(format!("finding {k}'s line isn't a number from 1"));
                    None
                }
            },
        };
        let some = |s: String| Some(s).filter(|s| !s.is_empty());
        out.push(Raw {
            id: some(clean(text_of("id"), false).chars().take(40).collect()),
            severity,
            repo: some(clean(text_of("repo"), false)),
            file,
            line,
            title,
            body: clean(text_of("body"), true),
            fix: clean(text_of("fix"), true),
        });
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

/// HTML tags taken out of untrusted text, as plain lowercase tag names
/// have them (`<script>`, `</b>`, `<!-- -->`): code such as `Vec<String>`
/// or `a < b` stays.
fn strip_tags(text: &str) -> String {
    const TAGS: &[&str] = &[
        "a", "b", "body", "br", "button", "code", "details", "div", "em", "embed", "form", "h1",
        "h2", "h3", "h4", "h5", "h6", "head", "hr", "html", "i", "iframe", "img", "input", "li",
        "link", "meta", "object", "ol", "p", "pre", "script", "span", "strong", "style", "summary",
        "svg", "table", "td", "th", "title", "tr", "u", "ul",
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after
            .find('>')
            .filter(|end| !after[..*end].contains(['<', '\n']));
        let tag = end.is_some_and(|end| {
            let inner = &after[..end];
            if inner.starts_with('!') {
                return true;
            }
            let name: String = inner
                .trim_start_matches('/')
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            TAGS.contains(&name.as_str())
        });
        match end.filter(|_| tag) {
            Some(end) => rest = &after[end + 1..],
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Untrusted text made safe to show: HTML tags and control characters
/// out (`lines`: line breaks stay, else they read as spaces), bidi
/// overrides out, trimmed and cut at `MAX_TEXT` characters.
pub fn clean(text: &str, lines: bool) -> String {
    scrub(&strip_tags(text), lines)
}

/// What the user wrote made safe to keep: as `clean`, but it is their text
/// and not an agent's, so what looks like a tag stays (a comment about a
/// `<div>`).
pub fn own_text(text: &str) -> String {
    scrub(text, true)
}

/// `text` without control characters (`lines`: line breaks stay, else they
/// read as spaces) and bidi overrides, trimmed and cut at `MAX_TEXT`
/// characters.
fn scrub(text: &str, lines: bool) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\n' if lines => out.push('\n'),
            '\n' | '\t' => out.push(' '),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim();
    if trimmed.chars().count() <= MAX_TEXT {
        return trimmed.to_owned();
    }
    let mut cut: String = trimmed.chars().take(MAX_TEXT - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// What reading a run's findings in came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Findings raised for the first time.
    pub new: usize,
    /// Earlier findings raised again (by their id).
    pub again: usize,
    /// Findings sent to be fixed that weren't raised again.
    pub fixed: usize,
}

/// The repository and file of a finding: an absolute path under a
/// touched repository's worktree (of `repos`, names and worktrees) is
/// taken as that repository's; without a repository, a feature over one
/// takes it.
fn place(repo: Option<String>, file: &str, repos: &[(String, PathBuf)]) -> (String, String) {
    for (name, worktree) in repos {
        if let Ok(rest) = Path::new(file).strip_prefix(worktree) {
            return (name.clone(), rest.display().to_string());
        }
    }
    let file = file.trim_start_matches("./").to_owned();
    match repo {
        // A linked project's repository by its plain name, when only one
        // has it.
        Some(repo) if !repos.iter().any(|(key, _)| *key == repo) => {
            let mut by_name = repos
                .iter()
                .filter(|(key, _)| key.rsplit(':').next() == Some(repo.as_str()));
            match (by_name.next(), by_name.next()) {
                (Some((key, _)), None) => (key.clone(), file),
                _ => (repo, file),
            }
        }
        Some(repo) => (repo, file),
        None if repos.len() == 1 => (repos[0].0.clone(), file),
        None => (String::new(), file),
    }
}

/// Whether `line` (none: the whole file) of `file` in `repo` is in `diff`.
fn placed(diff: &Diff, repo: &str, file: &str, line: Option<usize>) -> bool {
    let Some(ranges) = diff.get(repo).and_then(|files| files.get(file)) else {
        return false;
    };
    line.is_none_or(|l| ranges.iter().any(|(a, b)| (*a..=*b).contains(&l)))
}

/// Read run `run`'s findings (`raws`) into `review`, over the touched
/// `repos` (names and worktrees) whose code reads as `diff`:
///
/// - a finding naming an earlier one's id raises it again: its text and
///   run follow, and one sent or fixed is open again (one dismissed stays
///   dismissed, one undecided or to fix stays so);
/// - any other is new, its id `r<run>-<k>`;
/// - an earlier one sent to be fixed and not raised again is fixed.
pub fn import(
    review: &mut AiReview,
    run: u32,
    raws: Vec<Raw>,
    repos: &[(String, PathBuf)],
    diff: &Diff,
) -> Imported {
    let mut out = Imported::default();
    // Before this run rewrites them: what the user saw.
    let mut kept: Vec<Finding> = match run > review.n {
        true => review.shown().into_iter().cloned().collect(),
        false => Vec::new(),
    };
    let before = review.findings.len();
    let mut raised: BTreeSet<String> = BTreeSet::new();
    let mut k = 0;
    for raw in raws {
        let (repo, file) = place(raw.repo, &raw.file, repos);
        let at = placed(diff, &repo, &file, raw.line);
        let earlier = raw
            .id
            .as_ref()
            .filter(|id| !raised.contains(*id))
            .and_then(|id| review.findings[..before].iter().position(|f| f.id == *id));
        if let Some(i) = earlier {
            let f = &mut review.findings[i];
            raised.insert(f.id.clone());
            f.state = match f.state {
                FindingState::Sent | FindingState::Fixed => FindingState::Open,
                state => state,
            };
            f.severity = raw.severity;
            (f.repo, f.file, f.line) = (repo, file, raw.line);
            (f.title, f.body, f.fix) = (raw.title, raw.body, raw.fix);
            f.run = run;
            f.placed = at;
            out.again += 1;
            continue;
        }
        let id = loop {
            k += 1;
            let id = format!("r{run}-{k}");
            if review.finding(&id).is_none() {
                break id;
            }
        };
        review.findings.push(Finding {
            id,
            severity: raw.severity,
            repo,
            file,
            line: raw.line,
            title: raw.title,
            body: raw.body,
            fix: raw.fix,
            state: FindingState::Open,
            run,
            placed: at,
            ..Finding::default()
        });
        out.new += 1;
    }
    for f in &mut review.findings[..before] {
        if f.state == FindingState::Sent && !raised.contains(&f.id) {
            f.state = FindingState::Fixed;
            out.fixed += 1;
        }
    }
    for f in &mut kept {
        if f.state == FindingState::Sent && !raised.contains(&f.id) {
            f.state = FindingState::Fixed;
        }
    }
    if !kept.is_empty() {
        review.rounds.push(ReviewRound {
            n: review.n,
            findings: kept,
        });
    }
    review.n = run;
    out
}

/// Decide finding `id` of `review`: what it is now, in words for the
/// log ("dismissed: not reachable"); else why not.
pub fn decide(review: &mut AiReview, id: &str, decision: &Decision) -> Result<String, String> {
    use FindingState as S;
    let Some(f) = review.findings.iter_mut().find(|f| f.id == id) else {
        return Err(format!("There's no finding {id}"));
    };
    match (decision, f.state) {
        (Decision::Dismiss(why), _) if why.trim().is_empty() => {
            Err(format!("Say why you dismiss {id}"))
        }
        (_, S::Fixed) => Err(format!("{id} is fixed already")),
        (Decision::Fix, S::Open | S::Dismissed) => {
            f.state = S::Fix;
            f.reason.clear();
            Ok("to fix".into())
        }
        (Decision::Fix, S::Fix) => Err(format!("{id} is to be fixed already")),
        (Decision::Fix | Decision::Reopen, S::Sent) => {
            Err(format!("{id} was sent to Build: the next review checks it"))
        }
        (Decision::Dismiss(why), S::Open | S::Fix | S::Sent) => {
            f.state = S::Dismissed;
            f.reason = clean(why, true);
            Ok(format!("dismissed: {}", f.reason))
        }
        (Decision::Dismiss(_), S::Dismissed) => Err(format!("{id} is dismissed already")),
        (Decision::Reopen, S::Fix | S::Dismissed) => {
            f.state = S::Open;
            f.reason.clear();
            Ok("reopened".into())
        }
        (Decision::Reopen, S::Open) => Err(format!("{id} isn't decided yet")),
    }
}

/// Whether `f` can still be edited, undecided or to fix; else why not.
pub fn editable(f: &Finding) -> Result<(), String> {
    let id = &f.id;
    match f.state {
        FindingState::Open | FindingState::Fix => Ok(()),
        FindingState::Dismissed => Err(format!("{id} is dismissed: reopen it first")),
        FindingState::Sent => Err(format!("{id} was sent to Build: the next review checks it")),
        FindingState::Fixed => Err(format!("{id} is fixed already")),
    }
}

/// Reword finding `id`'s fix to `fix` and set the user's comment on it to
/// `note`: a fix left as the reviewer's, or empty, is theirs again. Saved
/// on an undecided finding it marks it to fix too, as rewording a fix means
/// wanting it. What it did, for the log; else why it can't.
pub fn edit(review: &mut AiReview, id: &str, fix: &str, note: &str) -> Result<String, String> {
    let Some(f) = review.findings.iter_mut().find(|f| f.id == id) else {
        return Err(format!("There's no finding {id}"));
    };
    editable(f)?;
    let fix = own_text(fix);
    f.my_fix = (!fix.is_empty() && fix != f.fix).then_some(fix);
    f.note = own_text(note);
    f.state = FindingState::Fix;
    Ok("edited, to fix".into())
}

/// `text` as an item's title (its first line) and body (the rest).
fn title_and_body(text: &str) -> (String, String) {
    let text = own_text(text);
    match text.split_once('\n') {
        Some((title, body)) => (title.trim().to_owned(), body.trim().to_owned()),
        None => (text, String::new()),
    }
}

/// An item of the user's own, saying `text`, added to the review to be
/// fixed like a finding marked so. Its id, `u<k>`; else why it can't.
pub fn add(review: &mut AiReview, text: &str) -> Result<String, String> {
    let (title, body) = title_and_body(text);
    if title.is_empty() {
        return Err("Say what to fix".into());
    }
    let id = (1..)
        .map(|k| format!("u{k}"))
        .find(|id| review.finding(id).is_none())
        .unwrap_or_default();
    review.findings.push(Finding {
        id: id.clone(),
        title,
        body,
        state: FindingState::Fix,
        run: review.n,
        yours: true,
        ..Finding::default()
    });
    Ok(id)
}

/// Rewrite item `id` of the user's own to say `text`. What it did, for the
/// log; else why it can't.
pub fn edit_own(review: &mut AiReview, id: &str, text: &str) -> Result<String, String> {
    let Some(f) = review.findings.iter_mut().find(|f| f.id == id && f.yours) else {
        return Err(format!("There's no item {id} of yours"));
    };
    editable(f)?;
    let (title, body) = title_and_body(text);
    if title.is_empty() {
        return Err("Say what to fix".into());
    }
    (f.title, f.body) = (title, body);
    f.state = FindingState::Fix;
    Ok("edited, to fix".into())
}

/// Where `f` is, as text: `api/src/a.rs:12`; the repository left out for
/// a feature over one (`multi` false).
pub fn location(f: &Finding, multi: bool) -> String {
    let mut at = if multi && !f.repo.is_empty() {
        format!("{}/{}", f.repo, f.file)
    } else {
        f.file.clone()
    };
    if let Some(line) = f.line {
        at.push_str(&format!(":{line}"));
    }
    at
}

/// The findings of `review` to fix, as Markdown for the Build agent of
/// feature `title` (`multi`: over several repositories), and how many.
pub fn fixes_markdown(review: &AiReview, title: &str, multi: bool) -> (usize, String) {
    let fix: Vec<&Finding> = review
        .findings
        .iter()
        .filter(|f| f.state == FindingState::Fix)
        .collect();
    let mut md = format!(
        "# Findings to fix\n\nAI review of \u{201c}{title}\u{201d} found these, and the user wants each one fixed.\n"
    );
    for f in &fix {
        md.push_str(&format!("\n## {}: {}\n\n", f.id, f.title));
        if f.yours {
            md.push_str("- Added by the user, not by the review\n");
        } else {
            md.push_str(&format!("- Severity: {}\n", f.severity.label()));
            md.push_str(&format!("- Where: {}\n", location(f, multi)));
        }
        if !f.body.is_empty() {
            md.push_str(&format!("\n{}\n", f.body));
        }
        match f.my_fix.as_deref() {
            Some(fix) => md.push_str(&format!("\nThe fix, as the user wants it: {fix}\n")),
            None if !f.fix.is_empty() => {
                md.push_str(&format!("\nSuggested fix: {}\n", f.fix));
            }
            None => {}
        }
        if !f.note.is_empty() {
            md.push_str(&format!("\nThe user's comment: {}\n", f.note));
        }
    }
    (fix.len(), md)
}

/// What the reviewer's work is read against, off the UI thread (see
/// `read_code`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRead {
    /// The feature's patch-id and what isn't committed.
    pub reading: Reading,
    /// `review.json`'s text, or why it couldn't be read.
    pub text: Result<String, String>,
    /// The diff the findings are placed on.
    pub diff: Diff,
    /// Per touched repository, the commit it is at.
    pub heads: BTreeMap<String, String>,
}

/// Read `feature`'s code and its `review.json` at `path`: its patch-id,
/// per touched repository its diff against its base and its commit.
/// Runs git: call it off the UI thread (checklist D3).
pub fn read_code(feature: &Feature, path: &Path) -> Result<CodeRead, String> {
    let reading = patch::read(feature)?;
    let text = match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_BYTES => {
            Err(format!("{REVIEW} is larger than {} KiB", MAX_BYTES / 1024))
        }
        Ok(_) => std::fs::read_to_string(path).map_err(|e| format!("reading {REVIEW}: {e}")),
        Err(e) => Err(format!("reading {REVIEW}: {e}")),
    };
    let mut out = CodeRead {
        reading,
        text,
        diff: Diff::new(),
        heads: BTreeMap::new(),
    };
    for repo in feature.touched() {
        let name = &repo.key();
        let range = format!("{}...HEAD", base_ref(repo));
        let diff = git(
            &repo.worktree,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "-U3",
                &range,
            ],
        )
        .map_err(|err| format!("{name}: couldn't read its change: {err}"))?;
        out.diff.insert(name.clone(), hunks(&diff));
        let head = git(&repo.worktree, &["rev-parse", "HEAD"])
            .map_err(|err| format!("{name}: couldn't read its commit: {err}"))?;
        out.heads.insert(name.clone(), head.trim().to_owned());
    }
    Ok(out)
}

/// The files of a `git diff` and, per file, the new side's line ranges
/// of its hunks.
fn hunks(diff: &str) -> BTreeMap<String, Vec<(usize, usize)>> {
    let mut out: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut file: Option<String> = None;
    let mut header = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            header = true;
            file = rest.rsplit_once(" b/").map(|(_, b)| b.to_owned());
            if let Some(f) = &file {
                out.entry(f.clone()).or_default();
            }
            continue;
        }
        if header && let Some(name) = line.strip_prefix("+++ b/") {
            file = Some(name.to_owned());
            out.entry(name.to_owned()).or_default();
            continue;
        }
        if !line.starts_with("@@ ") {
            continue;
        }
        header = false;
        let Some(f) = &file else {
            continue;
        };
        let new = line
            .split_whitespace()
            .find_map(|t| t.strip_prefix('+'))
            .unwrap_or("");
        let mut parts = new.split(',');
        let start: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let count: usize = parts
            .next()
            .map_or(Some(1), |n| n.parse().ok())
            .unwrap_or(0);
        if start > 0 && count > 0 {
            out.entry(f.clone())
                .or_default()
                .push((start, start + count - 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: Option<&str>, file: &str, line: Option<usize>, title: &str) -> Raw {
        Raw {
            id: id.map(str::to_owned),
            file: file.into(),
            line,
            title: title.into(),
            ..Raw::default()
        }
    }

    fn diff() -> Diff {
        let files = BTreeMap::from([("src/a.rs".to_owned(), vec![(10, 20)])]);
        BTreeMap::from([("api".to_owned(), files)])
    }

    fn repos() -> Vec<(String, PathBuf)> {
        vec![("api".into(), PathBuf::from("/w/f/api"))]
    }

    #[test]
    fn a_review_parses_and_an_empty_one_is_good() {
        assert_eq!(parse(r#"{"findings": []}"#), Ok(Vec::new()));
        let text = r#"{"findings": [{"severity": "HIGH", "repo": "api", "file": "src/a.rs",
            "line": 12, "title": "Off by one", "body": "x", "fix": "y", "id": "r1-2"}]}"#;
        let got = parse(text).unwrap();
        assert_eq!(
            got,
            [Raw {
                id: Some("r1-2".into()),
                severity: Severity::High,
                repo: Some("api".into()),
                file: "src/a.rs".into(),
                line: Some(12),
                title: "Off by one".into(),
                body: "x".into(),
                fix: "y".into(),
            }]
        );
        // Without a severity, medium.
        let one = parse(r#"{"findings": [{"file": "a", "title": "t"}]}"#).unwrap();
        assert_eq!(one[0].severity, Severity::Medium);
    }

    #[test]
    fn a_bad_review_says_what_it_lacks() {
        assert!(parse("nope").unwrap_err()[0].starts_with("it isn't JSON"));
        assert_eq!(
            parse(r#"{"found": []}"#).unwrap_err(),
            ["no \u{201c}findings\u{201d} list"]
        );
        let bad = r#"{"findings": [3, {"file": "a"}, {"title": "t", "file": "a",
            "severity": "dire", "line": 0}]}"#;
        assert_eq!(
            parse(bad).unwrap_err(),
            [
                "finding 1 isn't an object",
                "finding 2 has no title",
                "finding 3's severity isn't high, medium or low",
                "finding 3's line isn't a number from 1",
            ]
        );
        let big = format!(
            r#"{{"findings": [], "x": "{}"}}"#,
            "a".repeat(MAX_BYTES as usize)
        );
        assert!(parse(&big).unwrap_err()[0].contains("larger than 256 KiB"));
    }

    #[test]
    fn findings_are_capped_and_cleaned() {
        let one = r#"{"file": "a", "title": "t"}"#;
        let many = vec![one; MAX_FINDINGS + 5].join(",");
        let got = parse(&format!(r#"{{"findings": [{many}]}}"#)).unwrap();
        assert_eq!(got.len(), MAX_FINDINGS);

        assert_eq!(
            clean("<script>alert(1)</script> <b>bold</b>\u{7}", false),
            "alert(1) bold"
        );
        assert_eq!(
            clean("Vec<String> and a < b > c", false),
            "Vec<String> and a < b > c"
        );
        assert_eq!(clean("two\nlines\u{202e}", false), "two lines");
        assert_eq!(clean("two\nlines", true), "two\nlines");
        assert_eq!(clean("<!-- hidden -->shown", true), "shown");
        let long = clean(&"x".repeat(MAX_TEXT + 10), true);
        assert_eq!(long.chars().count(), MAX_TEXT);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn new_findings_get_ids_and_are_placed_on_the_diff() {
        let mut review = AiReview::default();
        let got = import(
            &mut review,
            1,
            vec![
                raw(None, "src/a.rs", Some(12), "in"),
                raw(None, "/w/f/api/src/a.rs", None, "absolute"),
                raw(None, "src/a.rs", Some(40), "out of the hunks"),
                raw(Some("r9-9"), "src/b.rs", None, "unknown id"),
            ],
            &repos(),
            &diff(),
        );
        assert_eq!(
            got,
            Imported {
                new: 4,
                ..Imported::default()
            }
        );
        let ids: Vec<&str> = review.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["r1-1", "r1-2", "r1-3", "r1-4"]);
        let placed: Vec<bool> = review.findings.iter().map(|f| f.placed).collect();
        assert_eq!(placed, [true, true, false, false]);
        assert!(review.findings.iter().all(|f| f.repo == "api"));
        assert_eq!(review.findings[1].file, "src/a.rs");
        assert_eq!(review.n, 1);
        assert_eq!(review.open(), 4);
    }

    #[test]
    fn the_next_run_keeps_the_review_as_the_user_saw_it() {
        let mut review = AiReview::default();
        let three = vec![
            raw(None, "src/a.rs", None, "one"),
            raw(None, "src/a.rs", None, "two"),
            raw(None, "src/a.rs", None, "three"),
        ];
        import(&mut review, 1, three, &repos(), &diff());
        assert!(review.rounds.is_empty());
        for id in ["r1-1", "r1-2"] {
            let f = review.findings.iter_mut().find(|f| f.id == id).unwrap();
            f.state = FindingState::Sent;
            f.my_fix = Some(format!("my fix of {id}"));
            f.note = format!("about {id}");
        }
        decide(&mut review, "r1-3", &Decision::Dismiss("by design".into())).unwrap();

        let again = vec![raw(Some("r1-2"), "src/a.rs", Some(11), "two, still")];
        import(&mut review, 2, again, &repos(), &diff());
        let [round] = review.rounds.as_slice() else {
            panic!("one round: {:?}", review.rounds);
        };
        assert_eq!(round.n, 1);
        let kept = |id: &str| round.findings.iter().find(|f| f.id == id).unwrap();
        // How it went, with what the user said of it.
        assert_eq!(kept("r1-1").state, FindingState::Fixed);
        assert_eq!(kept("r1-1").my_fix.as_deref(), Some("my fix of r1-1"));
        assert_eq!(kept("r1-1").note, "about r1-1");
        assert_eq!(kept("r1-3").reason, "by design");
        // Raised again: the words it had then, and still sent.
        assert_eq!(kept("r1-2").state, FindingState::Sent);
        assert_eq!(kept("r1-2").title, "two");
        assert_eq!(kept("r1-2").run, 1);
        assert_eq!(review.finding("r1-2").unwrap().title, "two, still");

        // The same run read again isn't another round; nor is a review
        // with nothing to show.
        import(&mut review, 2, Vec::new(), &repos(), &diff());
        assert_eq!(review.rounds.len(), 1);
        let mut empty = AiReview::default();
        import(&mut empty, 1, Vec::new(), &repos(), &diff());
        import(&mut empty, 2, Vec::new(), &repos(), &diff());
        assert!(empty.rounds.is_empty());
    }

    #[test]
    fn only_a_fixed_finding_an_earlier_review_shows_leaves_the_list() {
        let state = |id: &str, state| Finding {
            id: id.into(),
            state,
            ..Finding::default()
        };
        let mut review = AiReview {
            n: 2,
            findings: vec![
                state("r1-1", FindingState::Fixed),
                state("r1-2", FindingState::Fixed),
                state("r1-3", FindingState::Open),
                state("r1-4", FindingState::Dismissed),
                state("r2-1", FindingState::Sent),
            ],
            ..AiReview::default()
        };
        let ids = |review: &AiReview| -> Vec<String> {
            review.shown().iter().map(|f| f.id.clone()).collect()
        };
        // From before rounds were kept: nowhere else to see it.
        assert_eq!(ids(&review).len(), 5);
        review.rounds.push(ReviewRound {
            n: 1,
            findings: vec![
                state("r1-1", FindingState::Fixed),
                state("r1-3", FindingState::Open),
                state("r1-4", FindingState::Dismissed),
            ],
        });
        assert_eq!(ids(&review), ["r1-2", "r1-3", "r1-4", "r2-1"]);
    }

    #[test]
    fn a_record_with_no_rounds_loads_and_none_are_written() {
        let review: AiReview = serde_json::from_str(r#"{"n": 1}"#).unwrap();
        assert!(review.rounds.is_empty());
        assert!(!serde_json::to_string(&review).unwrap().contains("rounds"));
    }

    #[test]
    fn a_sent_finding_not_raised_again_is_fixed_and_one_raised_is_open() {
        let mut review = AiReview::default();
        let three = vec![
            raw(None, "src/a.rs", None, "one"),
            raw(None, "src/a.rs", None, "two"),
            raw(None, "src/a.rs", None, "three"),
        ];
        import(&mut review, 1, three, &repos(), &diff());
        for id in ["r1-1", "r1-2"] {
            decide(&mut review, id, &Decision::Fix).unwrap();
            review
                .findings
                .iter_mut()
                .find(|f| f.id == id)
                .unwrap()
                .state = FindingState::Sent;
        }
        decide(&mut review, "r1-3", &Decision::Dismiss("by design".into())).unwrap();

        let again = vec![
            raw(Some("r1-2"), "src/a.rs", Some(11), "two, still"),
            raw(Some("r1-3"), "src/a.rs", None, "three, again"),
            raw(None, "src/a.rs", None, "four"),
        ];
        let got = import(&mut review, 2, again, &repos(), &diff());
        assert_eq!(
            got,
            Imported {
                new: 1,
                again: 2,
                fixed: 1
            }
        );
        let state = |id: &str| review.finding(id).unwrap().state;
        assert_eq!(state("r1-1"), FindingState::Fixed);
        assert_eq!(state("r1-2"), FindingState::Open);
        assert_eq!(review.finding("r1-2").unwrap().run, 2);
        assert_eq!(review.finding("r1-2").unwrap().title, "two, still");
        // Dismissed stays dismissed, with its reason.
        assert_eq!(state("r1-3"), FindingState::Dismissed);
        assert_eq!(review.finding("r1-3").unwrap().reason, "by design");
        assert_eq!(state("r2-1"), FindingState::Open);
        assert_eq!(review.open(), 2);
    }

    #[test]
    fn decisions_follow_the_state() {
        let mut review = AiReview::default();
        import(
            &mut review,
            1,
            vec![raw(None, "a", None, "t")],
            &repos(),
            &diff(),
        );
        let id = "r1-1";
        assert_eq!(
            decide(&mut review, "r7-1", &Decision::Fix),
            Err("There's no finding r7-1".into())
        );
        assert_eq!(
            decide(&mut review, id, &Decision::Dismiss("  ".into())),
            Err("Say why you dismiss r1-1".into())
        );
        assert_eq!(
            decide(&mut review, id, &Decision::Reopen),
            Err("r1-1 isn't decided yet".into())
        );
        assert_eq!(decide(&mut review, id, &Decision::Fix), Ok("to fix".into()));
        assert_eq!(review.count(FindingState::Fix), 1);
        assert_eq!(
            decide(
                &mut review,
                id,
                &Decision::Dismiss(" <b>dead</b> code ".into())
            ),
            Ok("dismissed: dead code".into())
        );
        assert_eq!(review.open(), 0);
        assert_eq!(
            decide(&mut review, id, &Decision::Reopen),
            Ok("reopened".into())
        );
        assert_eq!(review.finding(id).unwrap().reason, "");
        assert_eq!(review.first_open().unwrap().id, id);
        review.findings[0].state = FindingState::Sent;
        assert!(decide(&mut review, id, &Decision::Fix).is_err());
        review.findings[0].state = FindingState::Fixed;
        assert_eq!(
            decide(&mut review, id, &Decision::Reopen),
            Err("r1-1 is fixed already".into())
        );
    }

    fn one_finding(state: FindingState) -> AiReview {
        AiReview {
            n: 1,
            findings: vec![Finding {
                id: "r1-1".into(),
                title: "Off by one".into(),
                file: "src/a.rs".into(),
                fix: "Use ..=".into(),
                state,
                ..Finding::default()
            }],
            ..AiReview::default()
        }
    }

    #[test]
    fn an_edit_rewords_the_fix_keeps_the_reviewers_and_marks_it_to_fix() {
        let mut review = one_finding(FindingState::Open);
        edit(&mut review, "r1-1", "Use a range", "Mind the tests").unwrap();
        let f = &review.findings[0];
        assert_eq!(f.fix, "Use ..=");
        assert_eq!(f.fix_text(), "Use a range");
        assert_eq!(f.note, "Mind the tests");
        assert_eq!(f.state, FindingState::Fix);
        // Left as the reviewer's, or empty, it is the reviewer's again.
        edit(&mut review, "r1-1", "Use ..=", "").unwrap();
        assert_eq!(review.findings[0].my_fix, None);
        edit(&mut review, "r1-1", "Mine", "").unwrap();
        edit(&mut review, "r1-1", "  ", "").unwrap();
        assert_eq!(review.findings[0].my_fix, None);
        assert_eq!(review.findings[0].fix_text(), "Use ..=");
        assert_eq!(review.findings[0].note, "");
    }

    #[test]
    fn only_an_undecided_finding_or_one_to_fix_is_edited() {
        for state in [
            FindingState::Dismissed,
            FindingState::Sent,
            FindingState::Fixed,
        ] {
            let mut review = one_finding(state);
            assert!(edit(&mut review, "r1-1", "x", "y").is_err(), "{state:?}");
            assert_eq!(review.findings[0].my_fix, None);
            assert_eq!(review.findings[0].state, state);
        }
        let mut review = one_finding(FindingState::Fix);
        assert!(edit(&mut review, "nope", "x", "y").is_err());
        assert!(edit(&mut review, "r1-1", "x", "y").is_ok());
    }

    #[test]
    fn what_the_user_wrote_stays_as_written_and_is_made_safe() {
        let mut review = one_finding(FindingState::Open);
        edit(
            &mut review,
            "r1-1",
            " keep <div> \u{202e}\u{7}here\n2nd ",
            "",
        )
        .unwrap();
        assert_eq!(
            review.findings[0].my_fix.as_deref(),
            Some("keep <div> here\n2nd")
        );
        // An agent's isn't.
        assert_eq!(clean("keep <div>here", false), "keep here");
    }

    #[test]
    fn the_users_words_survive_a_finding_raised_again() {
        let mut review = AiReview::default();
        let one = raw(None, "src/a.rs", Some(12), "Off by one");
        import(&mut review, 1, vec![one], &repos(), &diff());
        edit(&mut review, "r1-1", "Mine", "Careful").unwrap();
        review.findings[0].state = FindingState::Sent;
        let mut again = raw(Some("r1-1"), "src/a.rs", Some(12), "Still off by one");
        again.fix = "Reviewer's new".into();
        import(&mut review, 2, vec![again], &repos(), &diff());
        let f = &review.findings[0];
        assert_eq!(f.state, FindingState::Open);
        assert_eq!(f.title, "Still off by one");
        assert_eq!(f.fix, "Reviewer's new");
        assert_eq!(f.fix_text(), "Mine");
        assert_eq!(f.note, "Careful");
    }

    #[test]
    fn an_item_of_yours_is_to_fix_at_once_and_checked_by_the_next_review() {
        let mut review = AiReview {
            n: 1,
            ..AiReview::default()
        };
        assert_eq!(add(&mut review, " \n "), Err("Say what to fix".to_owned()));
        assert_eq!(
            add(&mut review, "Rename it\nIt reads badly\n").unwrap(),
            "u1"
        );
        assert_eq!(add(&mut review, "Another").unwrap(), "u2");
        let f = review.finding("u1").unwrap();
        assert_eq!(
            (f.title.as_str(), f.body.as_str()),
            ("Rename it", "It reads badly")
        );
        assert!(f.yours && f.file.is_empty());
        assert_eq!(f.state, FindingState::Fix);
        assert_eq!(review.open(), 2);
        edit_own(&mut review, "u2", "Better\nwords").unwrap();
        assert_eq!(review.finding("u2").unwrap().title, "Better");
        assert!(edit_own(&mut review, "u2", " ").is_err());
        assert!(
            edit_own(&mut review, "r1-1", "x").is_err(),
            "not the user's"
        );
        // Sent, and not raised again: fixed, like any other.
        for f in &mut review.findings {
            f.state = FindingState::Sent;
        }
        let got = import(&mut review, 2, Vec::new(), &repos(), &diff());
        assert_eq!(got.fixed, 2);
        assert_eq!(review.finding("u1").unwrap().state, FindingState::Fixed);
    }

    #[test]
    fn a_record_from_before_edits_reads_as_having_none() {
        let f: Finding = serde_json::from_str(
            r#"{"id": "r1-1", "severity": "high", "repo": "api", "file": "a", "title": "t", "run": 1}"#,
        )
        .unwrap();
        assert_eq!((f.my_fix, f.note.as_str(), f.yours), (None, "", false));
        // And nothing new is written for one with none.
        let text = serde_json::to_string(&one_finding(FindingState::Open).findings[0]).unwrap();
        for key in ["my_fix", "note", "yours"] {
            assert!(!text.contains(key), "{key}: {text}");
        }
    }

    #[test]
    fn the_fixes_carry_the_users_words_and_items() {
        let mut review = one_finding(FindingState::Open);
        edit(&mut review, "r1-1", "Use a range", "Mind the tests").unwrap();
        add(&mut review, "Rename it\nIt reads badly").unwrap();
        let (n, md) = fixes_markdown(&review, "Bulk edit", false);
        assert_eq!(n, 2);
        for words in [
            "The fix, as the user wants it: Use a range",
            "The user's comment: Mind the tests",
            "## u1: Rename it",
            "Added by the user",
            "It reads badly",
        ] {
            assert!(md.contains(words), "{words}: {md}");
        }
        assert!(!md.contains("Suggested fix: Use ..="), "{md}");
        assert!(!md.contains("Severity: medium\n- Where: \n"), "{md}");
    }

    #[test]
    fn f_and_del_on_a_finding_go_by_where_it_stands() {
        use FindingState as S;
        assert_eq!(fix_key(S::Open), Ok(Decision::Fix));
        assert_eq!(fix_key(S::Dismissed), Ok(Decision::Fix));
        assert_eq!(fix_key(S::Fix), Ok(Decision::Reopen));
        assert_eq!(fix_key(S::Sent), Err(SENT));
        assert_eq!(fix_key(S::Fixed), Err(FIXED));
        assert_eq!(dismiss_key(S::Open), Ok(None));
        assert_eq!(dismiss_key(S::Fix), Ok(None));
        assert_eq!(dismiss_key(S::Dismissed), Ok(Some(Decision::Reopen)));
        assert_eq!(dismiss_key(S::Sent), Err(SENT));
        assert_eq!(dismiss_key(S::Fixed), Err(FIXED));
    }

    #[test]
    fn the_fixes_read_as_markdown() {
        let mut review = AiReview::default();
        let mut one = raw(None, "src/a.rs", Some(12), "Off by one");
        one.severity = Severity::High;
        one.body = "The loop skips the last row.".into();
        one.fix = "Use ..=".into();
        import(
            &mut review,
            1,
            vec![one, raw(None, "b", None, "left open")],
            &repos(),
            &diff(),
        );
        decide(&mut review, "r1-1", &Decision::Fix).unwrap();
        let (n, md) = fixes_markdown(&review, "Bulk edit", true);
        assert_eq!(n, 1);
        for words in [
            "AI review of \u{201c}Bulk edit\u{201d}",
            "## r1-1: Off by one",
            "- Severity: high",
            "- Where: api/src/a.rs:12",
            "The loop skips the last row.",
            "Suggested fix: Use ..=",
        ] {
            assert!(md.contains(words), "{words}: {md}");
        }
        assert!(!md.contains("left open"));
        assert_eq!(location(&review.findings[0], false), "src/a.rs:12");
    }

    #[test]
    fn hunks_give_the_new_sides_ranges() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n x\n++++ b/fake\n@@ -10 +11 @@ fn\n-a\n+b\ndiff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        let got = hunks(diff);
        assert_eq!(got["src/a.rs"], [(1, 4), (11, 11)]);
        assert_eq!(got["gone"], []);
        assert!(!got.contains_key("fake"));
    }

    #[test]
    fn the_code_is_read_with_its_diff_and_commits() {
        use crate::feature::tests::{Temp, commit, feature_over};
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &["web"]);
        let wt = f.repos[0].worktree.clone();
        commit(&wt, "login", "log in");
        let path = t.0.join("review.json");
        std::fs::write(&path, r#"{"findings": []}"#).unwrap();
        let read = read_code(&f, &path).unwrap();
        assert_eq!(read.text.as_deref(), Ok(r#"{"findings": []}"#));
        assert_eq!(read.diff["api"]["login"], [(1, 1)]);
        assert_eq!(read.heads["api"].len(), 40);
        assert!(!read.diff.contains_key("web"));
    }

    #[test]
    fn a_linked_projects_repository_is_placed_by_its_key() {
        let repos = vec![
            ("api".to_owned(), PathBuf::from("/w/f/api")),
            ("admin:ui".to_owned(), PathBuf::from("/a/.features/f/ui")),
            ("admin:api".to_owned(), PathBuf::from("/a/.features/f/api")),
        ];
        let at = |repo: Option<&str>, file: &str| place(repo.map(str::to_owned), file, &repos);
        assert_eq!(
            at(None, "/a/.features/f/ui/src/b.rs"),
            ("admin:ui".into(), "src/b.rs".into())
        );
        assert_eq!(
            at(Some("admin:ui"), "src/b.rs"),
            ("admin:ui".into(), "src/b.rs".into())
        );
        // A plain name only one repository has.
        assert_eq!(
            at(Some("ui"), "src/b.rs"),
            ("admin:ui".into(), "src/b.rs".into())
        );
        // `api` is the home one's key: it stays.
        assert_eq!(
            at(Some("api"), "src/b.rs"),
            ("api".into(), "src/b.rs".into())
        );
    }
}
