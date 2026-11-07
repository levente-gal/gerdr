//! Manual test: the user tries the feature against its brief's acceptance
//! criteria, one by one, and marks each passed or failed.
//!
//! The criteria are the Given/When/Then items of the brief's `Acceptance
//! criteria` section, numbered `AC-1`, `AC-2`… in order. The results live
//! in `feature.json` (`Feature::manual`), each with the criterion's text
//! and the patch-id of the code it was given on. A result counts while its
//! criterion still reads the same, on whatever code: the user tests as the
//! fixes they asked for land, and what they found stays. An edited
//! criterion drops its result.

use serde::{Deserialize, Serialize};

use super::artifacts;

/// An acceptance criterion of the brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Criterion {
    /// `AC-1`, `AC-2`…
    pub id: String,
    /// What it checks, in a few words: the brief's own, before the Given,
    /// or made from the Then part.
    pub title: String,
    /// The Given/When/Then part. Results match on it, so a renamed title
    /// keeps them.
    pub text: String,
}

/// The criteria in `brief`: the Given/When/Then items of its `Acceptance
/// criteria` section (any heading level, any case), list markers and
/// checkboxes off, fenced code skipped. A title before the Given
/// (`**Title**: Given …`, `Title — Given …`) is split off.
pub fn parse_criteria(brief: &str) -> Vec<Criterion> {
    let Some(lines) = artifacts::section(brief, "acceptance criteria") else {
        return Vec::new();
    };
    lines
        .into_iter()
        .filter(|l| artifacts::given_when_then(l))
        .map(|l| {
            let text = artifacts::list_item(l).unwrap_or(l);
            let text = ["[ ]", "[x]", "[X]"]
                .into_iter()
                .find_map(|b| text.strip_prefix(b))
                .unwrap_or(text);
            text.trim()
        })
        .filter(|t| !t.is_empty())
        .enumerate()
        .map(|(i, line)| {
            let (title, text) = split_title(line);
            Criterion {
                id: format!("AC-{}", i + 1),
                title: title.unwrap_or_else(|| fallback_title(text)),
                text: text.to_string(),
            }
        })
        .collect()
}

/// Where `word` starts in `text` at or after byte `from`, as a whole word,
/// in any case.
fn word_at(text: &str, word: &str, from: usize) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let alnum = |at: usize| bytes.get(at).is_some_and(|b| b.is_ascii_alphanumeric());
    let mut at = from;
    while let Some(i) = lower.get(at..)?.find(word) {
        let start = at + i;
        let end = start + word.len();
        if (start == 0 || !alnum(start - 1)) && !alnum(end) {
            return Some(start);
        }
        at = end;
    }
    None
}

/// A criterion line's title, when it has one before its Given, and the
/// Given/When/Then rest. The title ends in `:`, a dash, or bold: "On the
/// page, given …" has none.
fn split_title(line: &str) -> (Option<String>, &str) {
    let Some(given) = word_at(line, "given", 0).filter(|g| *g > 0) else {
        return (None, line);
    };
    let head = line[..given].trim_end();
    let bold = head.ends_with("**") || head.ends_with("__");
    let cut = head.trim_end_matches([':', '\u{2014}', '\u{2013}', '-', '.', ' ']);
    if !bold && cut.len() == head.len() {
        return (None, line);
    }
    let title = cut
        .trim_matches(['*', '_', ' '])
        .trim_end_matches([':', '\u{2014}', '\u{2013}', '-', '.', ' ']);
    if title.is_empty() || title.split_whitespace().count() > 12 {
        return (None, line);
    }
    (Some(title.to_string()), &line[given..])
}

/// A Given/When/Then part without its spaces, commas and full stop.
fn part(s: &str) -> &str {
    s.trim()
        .trim_matches([',', ';'])
        .trim()
        .trim_end_matches('.')
        .trim_end()
}

/// The longest a title made from a criterion's Then part runs, in words.
const FALLBACK_WORDS: usize = 8;

/// A title for a criterion `text` the brief gave none: its Then part, cut
/// to a few words.
fn fallback_title(text: &str) -> String {
    let then = match split_gwt(text) {
        Some([_, _, then]) => then,
        None => text,
    };
    let words: Vec<&str> = then.split_whitespace().collect();
    let mut title = words[..words.len().min(FALLBACK_WORDS)].join(" ");
    title = title.trim_end_matches(['.', ',', ';']).to_string();
    if words.len() > FALLBACK_WORDS {
        title.push('\u{2026}');
    }
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => title,
    }
}

/// The Given, When and Then parts of criterion `text`, their words and
/// commas off: "two rows", "I edit both", "both change".
pub fn split_gwt(text: &str) -> Option<[&str; 3]> {
    let g = word_at(text, "given", 0)?;
    let w = word_at(text, "when", g + 5)?;
    let t = word_at(text, "then", w + 4)?;
    let parts = [&text[g + 5..w], &text[w + 4..t], &text[t + 4..]].map(part);
    parts.iter().all(|p| !p.is_empty()).then_some(parts)
}

/// What the user found of a criterion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Pass,
    Fail,
}

/// The result of one criterion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    /// The criterion's text when it was tried.
    pub text: String,
    pub state: Verdict,
    /// What went wrong, for a failure.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Unix seconds.
    pub at: u64,
    /// Whether this failure went to the Build agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sent: bool,
    /// The patch-id of the code it was tried on; empty in older records,
    /// which kept one for all (`ManualTest::patch`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub patch: String,
    /// Whether the Build agent made the fix of this sent failure: it is to
    /// try again.
    #[serde(default, skip_serializing_if = "is_false")]
    pub made: bool,
}

impl Check {
    /// Whether this sent failure's fix is made, and it waits to be tried
    /// again.
    pub fn fix_made(&self) -> bool {
        self.state == Verdict::Fail && self.sent && self.made
    }
}

fn is_false(b: &bool) -> bool {
    !b
}

/// The results of a manual test.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualTest {
    /// The patch-id of the code last tried.
    pub patch: String,
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// The result of criterion `c` that still counts: one given on its current
/// text, on any code.
pub fn check<'a>(test: Option<&'a ManualTest>, c: &Criterion) -> Option<&'a Check> {
    (test?.checks.iter()).find(|k| k.id == c.id && k.text == c.text)
}

/// The patch-id of the code `k` of `test` was tried on.
pub fn tried_on<'a>(test: &'a ManualTest, k: &'a Check) -> &'a str {
    match k.patch.is_empty() {
        true => &test.patch,
        false => &k.patch,
    }
}

/// How many criteria passed, failed (and of those, went to Build, and of
/// those, had their fix made) and aren't tried yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub passed: usize,
    pub failed: usize,
    pub sent: usize,
    pub made: usize,
    pub untested: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.untested
    }
}

/// The counts of `criteria` in `test`.
pub fn counts(test: Option<&ManualTest>, criteria: &[Criterion]) -> Counts {
    let mut n = Counts::default();
    for c in criteria {
        match check(test, c) {
            Some(k) if k.state == Verdict::Pass => n.passed += 1,
            Some(k) => {
                n.failed += 1;
                n.sent += usize::from(k.sent);
                n.made += usize::from(k.fix_made());
            }
            None => n.untested += 1,
        }
    }
    n
}

/// Whether there are criteria and every one passed.
pub fn all_passed(test: Option<&ManualTest>, criteria: &[Criterion]) -> bool {
    !criteria.is_empty() && counts(test, criteria).passed == criteria.len()
}

/// Record `state` (with `note`) for criterion `c` on `patch` at `now`. The
/// others stay, whatever code they were tried on. A failure keeps its sent
/// mark while its note stays the same, until its fix is made: tried again
/// after that, it is a new failure to send.
pub fn set(
    test: &mut Option<ManualTest>,
    c: &Criterion,
    patch: &str,
    state: Verdict,
    note: &str,
    now: u64,
) {
    let t = test.get_or_insert_with(Default::default);
    // Older records keep one patch for all: each says its own from now on.
    for k in &mut t.checks {
        if k.patch.is_empty() {
            k.patch = t.patch.clone();
        }
    }
    t.patch = patch.to_string();
    let sent = t
        .checks
        .iter()
        .find(|k| k.id == c.id && k.text == c.text)
        .is_some_and(|k| k.sent && !k.made && k.state == state && k.note == note);
    t.checks.retain(|k| k.id != c.id);
    t.checks.push(Check {
        id: c.id.clone(),
        text: c.text.clone(),
        state,
        note: note.to_string(),
        at: now,
        sent,
        patch: patch.to_string(),
        made: false,
    });
}

/// Forget the result of criterion `id`; true when there was one.
pub fn clear(test: &mut Option<ManualTest>, id: &str) -> bool {
    let Some(t) = test else { return false };
    let before = t.checks.len();
    t.checks.retain(|k| k.id != id);
    let gone = t.checks.len() != before;
    if t.checks.is_empty() {
        *test = None;
    }
    gone
}

/// The failures of `criteria` not yet sent to Build.
fn unsent<'a>(
    test: Option<&'a ManualTest>,
    criteria: &'a [Criterion],
) -> Vec<(&'a Criterion, &'a Check)> {
    criteria
        .iter()
        .filter_map(|c| Some((c, check(test, c)?)))
        .filter(|(_, k)| k.state == Verdict::Fail && !k.sent)
        .collect()
}

/// How many failures haven't gone to Build yet.
pub fn unsent_count(test: Option<&ManualTest>, criteria: &[Criterion]) -> usize {
    unsent(test, criteria).len()
}

/// The failures not sent yet, as Markdown for the Build agent of feature
/// `title`, and how many.
pub fn failures_markdown(
    test: Option<&ManualTest>,
    criteria: &[Criterion],
    title: &str,
) -> (usize, String) {
    let fails = unsent(test, criteria);
    let mut md = format!(
        "# Manual test failures\n\nThe user tried \u{201c}{title}\u{201d} by hand against its acceptance criteria, and these failed. Fix each one.\n"
    );
    for (c, k) in &fails {
        md.push_str(&format!("\n## {}: {}\n\n{}\n\n", c.id, c.title, c.text));
        md.push_str(&format!("What went wrong: {}\n", k.note));
    }
    (fails.len(), md)
}

/// Mark the failures not sent yet as sent: which, by id and text.
pub fn mark_sent(test: &mut Option<ManualTest>, criteria: &[Criterion]) -> Vec<(String, String)> {
    let ids: Vec<(String, String)> = unsent(test.as_ref(), criteria)
        .into_iter()
        .map(|(c, _)| (c.id.clone(), c.text.clone()))
        .collect();
    if let Some(t) = test {
        for k in &mut t.checks {
            if ids.iter().any(|(id, text)| *id == k.id && *text == k.text) {
                k.sent = true;
                k.made = false;
            }
        }
    }
    ids
}

/// Mark `ids`, by id and text, as not sent: what `mark_sent` did, undone.
pub fn unmark_sent(test: &mut ManualTest, ids: &[(String, String)]) {
    for k in &mut test.checks {
        if ids.iter().any(|(id, text)| *id == k.id && *text == k.text) {
            k.sent = false;
            k.made = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRIEF: &str = "# Bulk edit\n\n## Outcome\nGiven nothing, when no, then not this.\n\n### ACCEPTANCE Criteria:\n- [ ] Given two rows, when I edit both, then both change.\n1. Given one row, when I clear it, then it is empty.\n- not a criterion\n```\n- Given a fence, when read, then skipped.\n```\n* [x] Given a done box, when read, then kept.\n\n## Notes\n- Given later, when read, then not in.\n";

    fn crit(n: usize, text: &str) -> Criterion {
        Criterion {
            id: format!("AC-{n}"),
            title: fallback_title(text),
            text: text.into(),
        }
    }

    #[test]
    fn a_title_before_the_given_is_split_off() {
        let brief = "## Acceptance criteria\n\
            - **Both rows change**: Given two rows, when I edit both, then both change.\n\
            - **Row empties:** Given one row, when I clear it, then it is empty.\n\
            - Port fetch \u{2014} Given a port, when picked, then prices load.\n\
            - On the page, given a row, when read, then shown.\n\
            - Given the custom port switch is on, when the editor opens, then the ballast/laden selector shows the value inferred.\n";
        let got = parse_criteria(brief);
        let titles: Vec<&str> = got.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Both rows change",
                "Row empties",
                "Port fetch",
                "Shown",
                "The ballast/laden selector shows the value inferred",
            ]
        );
        assert_eq!(
            got[0].text,
            "Given two rows, when I edit both, then both change."
        );
        assert_eq!(got[2].text, "Given a port, when picked, then prices load.");
        assert_eq!(
            got[3].text,
            "On the page, given a row, when read, then shown."
        );
        // A long Then part is cut, with an ellipsis.
        assert_eq!(
            fallback_title(
                "Given a, when b, then the decision is created and recosted exactly as today."
            ),
            "The decision is created and recosted exactly as\u{2026}"
        );
    }

    #[test]
    fn criteria_split_into_given_when_then() {
        assert_eq!(
            split_gwt("Given two rows, when I edit both, then both change."),
            Some(["two rows", "I edit both", "both change"])
        );
        // Words inside others don't count: "whenever", "thence".
        assert_eq!(
            split_gwt("Given whenever, When thence it runs, Then done"),
            Some(["whenever", "thence it runs", "done"])
        );
        assert_eq!(split_gwt("Given a, when b"), None);
    }

    #[test]
    fn a_renamed_title_keeps_the_result() {
        let old = parse_criteria("## Acceptance criteria\n- Given a, when b, then c.\n");
        let new =
            parse_criteria("## Acceptance criteria\n- **C holds**: Given a, when b, then c.\n");
        let mut t = None;
        set(&mut t, &old[0], "p", Verdict::Pass, "", 1);
        assert!(check(t.as_ref(), &new[0]).is_some());
    }

    #[test]
    fn criteria_are_the_given_when_then_items_in_order() {
        let got = parse_criteria(BRIEF);
        assert_eq!(
            got,
            vec![
                crit(1, "Given two rows, when I edit both, then both change."),
                crit(2, "Given one row, when I clear it, then it is empty."),
                crit(3, "Given a done box, when read, then kept."),
            ]
        );
        assert!(parse_criteria("# T\n## Outcome\nx\n").is_empty());
        assert!(parse_criteria("## Acceptance criteria\n- just words\n").is_empty());
    }

    #[test]
    fn results_count_on_their_patch_and_text_only() {
        let a = crit(1, "Given a, when b, then c.");
        let b = crit(2, "Given d, when e, then f.");
        let all = vec![a.clone(), b.clone()];
        let mut t = None;
        set(&mut t, &a, "p1", Verdict::Pass, "", 10);
        set(&mut t, &b, "p1", Verdict::Fail, "broke", 11);
        let n = counts(t.as_ref(), &all);
        assert_eq!((n.passed, n.failed, n.untested), (1, 1, 0));
        assert!(!all_passed(t.as_ref(), &all));
        // An edited criterion drops its result.
        let edited = vec![a.clone(), crit(2, "Given d, when e, then g.")];
        assert_eq!(counts(t.as_ref(), &edited).untested, 1);
        // A result on new code keeps the others, each on its own code.
        set(&mut t, &b, "p2", Verdict::Pass, "", 12);
        assert!(all_passed(t.as_ref(), &all));
        assert!(!all_passed(t.as_ref(), &[]));
        let test = t.as_ref().unwrap();
        assert_eq!(test.checks.len(), 2);
        assert_eq!(tried_on(test, check(t.as_ref(), &a).unwrap()), "p1");
        assert_eq!(tried_on(test, check(t.as_ref(), &b).unwrap()), "p2");
        assert!(clear(&mut t, "AC-1"));
        assert!(clear(&mut t, "AC-2"));
        assert!(t.is_none());
        assert!(!clear(&mut t, "AC-1"));
    }

    #[test]
    fn failures_go_to_build_once() {
        let a = crit(1, "Given a, when b, then c.");
        let b = crit(2, "Given d, when e, then f.");
        let all = vec![a.clone(), b.clone()];
        let mut t = None;
        set(&mut t, &a, "p", Verdict::Fail, "nothing happens", 1);
        set(&mut t, &b, "p", Verdict::Pass, "", 1);
        let (n, md) = failures_markdown(t.as_ref(), &all, "Bulk");
        assert_eq!(n, 1);
        assert!(md.contains("## AC-1: C\n\nGiven a, when b, then c."));
        assert!(md.contains("What went wrong: nothing happens"));
        assert!(!md.contains("AC-2"));
        let sent = mark_sent(&mut t, &all);
        assert_eq!(sent.len(), 1);
        assert_eq!(unsent_count(t.as_ref(), &all), 0);
        assert_eq!(counts(t.as_ref(), &all).sent, 1);
        // A dropped fix plan gives them back, and they can go again.
        unmark_sent(t.as_mut().unwrap(), &sent);
        assert_eq!(unsent_count(t.as_ref(), &all), 1);
        assert_eq!(mark_sent(&mut t, &all), sent);
        // The same note keeps it sent; a new note makes it unsent.
        set(&mut t, &a, "p", Verdict::Fail, "nothing happens", 2);
        assert_eq!(unsent_count(t.as_ref(), &all), 0);
        set(&mut t, &a, "p", Verdict::Fail, "it crashes", 3);
        assert_eq!(unsent_count(t.as_ref(), &all), 1);
        // Its fix made, it waits to be tried again; failed again, even with
        // the same note, it is a new failure to send.
        mark_sent(&mut t, &all);
        t.as_mut().unwrap().checks.iter_mut().for_each(|k| k.made = k.sent);
        let n = counts(t.as_ref(), &all);
        assert_eq!((n.failed, n.sent, n.made), (1, 1, 1));
        assert!(check(t.as_ref(), &a).unwrap().fix_made());
        set(&mut t, &a, "p2", Verdict::Fail, "it crashes", 4);
        assert_eq!(unsent_count(t.as_ref(), &all), 1);
        assert!(!check(t.as_ref(), &a).unwrap().fix_made());
    }

    #[test]
    fn an_older_record_loads_with_its_one_patch() {
        let json = r#"{"patch":"p","checks":[{"id":"AC-1","text":"Given a, when b, then c.","state":"pass","at":1}]}"#;
        let test: ManualTest = serde_json::from_str(json).unwrap();
        let a = crit(1, "Given a, when b, then c.");
        let k = check(Some(&test), &a).unwrap();
        assert_eq!((tried_on(&test, k), k.made), ("p", false));
        // A new result elsewhere keeps the patch it had.
        let mut t = Some(test);
        set(&mut t, &crit(2, "Given d, when e, then f."), "q", Verdict::Pass, "", 2);
        let test = t.as_ref().unwrap();
        assert_eq!(tried_on(test, check(t.as_ref(), &a).unwrap()), "p");
    }

    #[test]
    fn the_record_round_trips() {
        let mut t = None;
        set(
            &mut t,
            &crit(1, "Given a, when b, then c."),
            "p",
            Verdict::Fail,
            "x",
            5,
        );
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"fail\""));
        let back: Option<ManualTest> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }
}
