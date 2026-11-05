//! Manual test: the user tries the feature against its brief's acceptance
//! criteria, one by one, and marks each passed or failed.
//!
//! The criteria are the Given/When/Then items of the brief's `Acceptance
//! criteria` section, numbered `AC-1`, `AC-2`… in order. The results live
//! in `feature.json` (`Feature::manual`), each with the criterion's text
//! and, for the whole set, the patch-id of the code they were given on. A
//! result counts only while its criterion still reads the same and the
//! code is still that patch: an edited brief or changed code drops it.

use serde::{Deserialize, Serialize};

use super::artifacts;

/// An acceptance criterion of the brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Criterion {
    /// `AC-1`, `AC-2`…
    pub id: String,
    pub text: String,
}

/// The criteria in `brief`: the Given/When/Then items of its `Acceptance
/// criteria` section (any heading level, any case), list markers and
/// checkboxes off, fenced code skipped.
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
            text.trim().to_string()
        })
        .filter(|t| !t.is_empty())
        .enumerate()
        .map(|(i, text)| Criterion {
            id: format!("AC-{}", i + 1),
            text,
        })
        .collect()
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
}

fn is_false(b: &bool) -> bool {
    !b
}

/// The results of a manual test, on one patch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualTest {
    /// The patch-id of the code tried.
    pub patch: String,
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// The result of criterion `c` that still counts: tried on `patch` (when
/// known; `None` trusts the record) and on the criterion's current text.
pub fn check<'a>(
    test: Option<&'a ManualTest>,
    c: &Criterion,
    patch: Option<&str>,
) -> Option<&'a Check> {
    let test = test?;
    if patch.is_some_and(|p| p != test.patch) {
        return None;
    }
    test.checks
        .iter()
        .find(|k| k.id == c.id && k.text == c.text)
}

/// How many criteria passed, failed (and of those, went to Build) and
/// aren't tried yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub passed: usize,
    pub failed: usize,
    pub sent: usize,
    pub untested: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.untested
    }
}

/// The counts of `criteria` in `test` on `patch`.
pub fn counts(test: Option<&ManualTest>, criteria: &[Criterion], patch: Option<&str>) -> Counts {
    let mut n = Counts::default();
    for c in criteria {
        match check(test, c, patch) {
            Some(k) if k.state == Verdict::Pass => n.passed += 1,
            Some(k) => {
                n.failed += 1;
                n.sent += usize::from(k.sent);
            }
            None => n.untested += 1,
        }
    }
    n
}

/// Whether there are criteria and every one passed.
pub fn all_passed(test: Option<&ManualTest>, criteria: &[Criterion], patch: Option<&str>) -> bool {
    !criteria.is_empty() && counts(test, criteria, patch).passed == criteria.len()
}

/// Record `state` (with `note`) for criterion `c` on `patch` at `now`.
/// Results on another patch go: the code they were given on is gone. A
/// failure keeps its sent mark while its note stays the same.
pub fn set(
    test: &mut Option<ManualTest>,
    c: &Criterion,
    patch: &str,
    state: Verdict,
    note: &str,
    now: u64,
) {
    let t = test.get_or_insert_with(Default::default);
    if t.patch != patch {
        *t = ManualTest {
            patch: patch.to_string(),
            checks: Vec::new(),
        };
    }
    let sent = t
        .checks
        .iter()
        .find(|k| k.id == c.id && k.text == c.text)
        .is_some_and(|k| k.sent && k.state == state && k.note == note);
    t.checks.retain(|k| k.id != c.id);
    t.checks.push(Check {
        id: c.id.clone(),
        text: c.text.clone(),
        state,
        note: note.to_string(),
        at: now,
        sent,
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

/// The failures of `criteria` on `patch` not yet sent to Build.
fn unsent<'a>(
    test: Option<&'a ManualTest>,
    criteria: &'a [Criterion],
    patch: Option<&str>,
) -> Vec<(&'a Criterion, &'a Check)> {
    criteria
        .iter()
        .filter_map(|c| Some((c, check(test, c, patch)?)))
        .filter(|(_, k)| k.state == Verdict::Fail && !k.sent)
        .collect()
}

/// How many failures haven't gone to Build yet.
pub fn unsent_count(
    test: Option<&ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
) -> usize {
    unsent(test, criteria, patch).len()
}

/// The failures not sent yet, as Markdown for the Build agent of feature
/// `title`, and how many.
pub fn failures_markdown(
    test: Option<&ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
    title: &str,
) -> (usize, String) {
    let fails = unsent(test, criteria, patch);
    let mut md = format!(
        "# Manual test failures\n\nThe user tried \u{201c}{title}\u{201d} by hand against its acceptance criteria, and these failed. Fix each one.\n"
    );
    for (c, k) in &fails {
        md.push_str(&format!("\n## {}: {}\n\n", c.id, c.text));
        md.push_str(&format!("What went wrong: {}\n", k.note));
    }
    (fails.len(), md)
}

/// Mark the failures not sent yet as sent: which, by id and text.
pub fn mark_sent(
    test: &mut Option<ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
) -> Vec<(String, String)> {
    let ids: Vec<(String, String)> = unsent(test.as_ref(), criteria, patch)
        .into_iter()
        .map(|(c, _)| (c.id.clone(), c.text.clone()))
        .collect();
    if let Some(t) = test {
        for k in &mut t.checks {
            if ids.iter().any(|(id, text)| *id == k.id && *text == k.text) {
                k.sent = true;
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
            text: text.into(),
        }
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
        let n = counts(t.as_ref(), &all, Some("p1"));
        assert_eq!((n.passed, n.failed, n.untested), (1, 1, 0));
        assert!(!all_passed(t.as_ref(), &all, Some("p1")));
        // Another patch: nothing counts.
        assert_eq!(counts(t.as_ref(), &all, Some("p2")).untested, 2);
        // Unknown patch trusts the record.
        assert_eq!(counts(t.as_ref(), &all, None).passed, 1);
        // An edited criterion drops its result.
        let edited = vec![a.clone(), crit(2, "Given d, when e, then g.")];
        assert_eq!(counts(t.as_ref(), &edited, Some("p1")).untested, 1);
        // Passing b makes all pass.
        set(&mut t, &b, "p1", Verdict::Pass, "", 12);
        assert!(all_passed(t.as_ref(), &all, Some("p1")));
        assert!(!all_passed(t.as_ref(), &[], Some("p1")));
        // A result on a new patch drops the old ones.
        set(&mut t, &a, "p2", Verdict::Pass, "", 13);
        assert_eq!(t.as_ref().unwrap().checks.len(), 1);
        assert!(clear(&mut t, "AC-1"));
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
        let (n, md) = failures_markdown(t.as_ref(), &all, Some("p"), "Bulk");
        assert_eq!(n, 1);
        assert!(md.contains("## AC-1: Given a, when b, then c."));
        assert!(md.contains("What went wrong: nothing happens"));
        assert!(!md.contains("AC-2"));
        let sent = mark_sent(&mut t, &all, Some("p"));
        assert_eq!(sent.len(), 1);
        assert_eq!(unsent_count(t.as_ref(), &all, Some("p")), 0);
        assert_eq!(counts(t.as_ref(), &all, Some("p")).sent, 1);
        // A dropped fix plan gives them back, and they can go again.
        unmark_sent(t.as_mut().unwrap(), &sent);
        assert_eq!(unsent_count(t.as_ref(), &all, Some("p")), 1);
        assert_eq!(mark_sent(&mut t, &all, Some("p")), sent);
        // The same note keeps it sent; a new note makes it unsent.
        set(&mut t, &a, "p", Verdict::Fail, "nothing happens", 2);
        assert_eq!(unsent_count(t.as_ref(), &all, Some("p")), 0);
        set(&mut t, &a, "p", Verdict::Fail, "it crashes", 3);
        assert_eq!(unsent_count(t.as_ref(), &all, Some("p")), 1);
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
