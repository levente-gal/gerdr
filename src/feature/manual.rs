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
//!
//! The user can add comments of their own beside them (`ManualTest::comments`),
//! on no criterion: "make the settings page look better". Each is tried as a
//! criterion is, numbered `C1`, `C2`… after the brief's (`comment_criteria`): it
//! starts failed, as it says what should change, goes to Build with the
//! failures, and blocks Mark passed until the user marks it done.

use std::collections::{BTreeMap, BTreeSet};

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
    /// keeps them. A comment's whole text.
    pub text: String,
    /// It is a comment of the user's own, not one of the brief's.
    pub comment: bool,
}

/// A comment of the user's own on a feature's manual test, on no criterion:
/// something to change that the brief doesn't say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// `C1`, `C2`…, never given twice in a feature.
    pub id: String,
    pub text: String,
    /// Unix seconds it was written.
    pub at: u64,
}

/// The most words of a comment its title shows.
const COMMENT_WORDS: usize = 10;

/// The criteria `test`'s comments are tried as, in the order written.
pub fn comment_criteria(test: Option<&ManualTest>) -> Vec<Criterion> {
    (test.iter().flat_map(|t| &t.comments))
        .map(|c| Criterion {
            id: c.id.clone(),
            title: comment_title(&c.text),
            text: c.text.clone(),
            comment: true,
        })
        .collect()
}

/// What a Manual test comment's id starts with: "T1", apart from Code
/// review's "C1" in the same feature. Those kept from before say "C".
const COMMENT_PREFIX: char = 'T';

/// A comment's title: its first line, cut to a few words.
fn comment_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut title = words[..words.len().min(COMMENT_WORDS)].join(" ");
    let more = words.len() > COMMENT_WORDS || text.trim().lines().nth(1).is_some();
    if more {
        title = title.trim_end_matches(['.', ',', ';', ':']).to_owned();
        title.push('\u{2026}');
    }
    title
}

/// Add a comment saying `text` to `test`, failed on `patch` at `now`: it is
/// what should change. Its id.
pub fn add_comment(test: &mut Option<ManualTest>, text: &str, patch: &str, now: u64) -> String {
    let t = test.get_or_insert_with(Default::default);
    t.comments_given += 1;
    let id = format!("{COMMENT_PREFIX}{}", t.comments_given);
    let comment = Comment {
        id: id.clone(),
        text: text.to_owned(),
        at: now,
    };
    t.comments.push(comment);
    let c = Criterion {
        id: id.clone(),
        title: comment_title(text),
        text: text.to_owned(),
        comment: true,
    };
    set(test, &c, patch, Verdict::Fail, "", now);
    id
}

/// Change comment `id` of `test` to say `text`, failed again on `patch` at
/// `now`: it asks for something new. The text it said before; `None` when
/// there is no such comment or it says that already.
pub fn edit_comment(
    test: &mut Option<ManualTest>,
    id: &str,
    text: &str,
    patch: &str,
    now: u64,
) -> Option<String> {
    let t = test.as_mut()?;
    let comment = t.comments.iter_mut().find(|c| c.id == id)?;
    if comment.text == text {
        return None;
    }
    let was = std::mem::replace(&mut comment.text, text.to_owned());
    let shots = (t.checks.iter())
        .find(|k| k.id == id && k.state == Verdict::Fail)
        .map(|k| k.shots.clone())
        .unwrap_or_default();
    let c = comment_criteria(test.as_ref())
        .into_iter()
        .find(|c| c.id == id)?;
    set(test, &c, patch, Verdict::Fail, "", now);
    if !shots.is_empty() {
        add_shots(test, &c, &shots);
    }
    Some(was)
}

/// A comment taken out of a manual test, with what was found of it, to put
/// back as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub at: usize,
    pub comment: Comment,
    pub check: Option<Check>,
    pub retest: Option<u32>,
}

/// Take comment `id` out of `test`, with its result.
pub fn delete_comment(test: &mut Option<ManualTest>, id: &str) -> Option<Removed> {
    let t = test.as_mut()?;
    let at = t.comments.iter().position(|c| c.id == id)?;
    let comment = t.comments.remove(at);
    let check = (t.checks.iter().position(|k| k.id == id)).map(|i| t.checks.remove(i));
    let retest = t.retest.remove(id);
    if t.is_empty() {
        *test = None;
    }
    Some(Removed {
        at,
        comment,
        check,
        retest,
    })
}

/// Put back what `delete_comment` took; false when a comment of its id is
/// there already.
pub fn restore_comment(test: &mut Option<ManualTest>, removed: Removed) -> bool {
    let t = test.get_or_insert_with(Default::default);
    let id = removed.comment.id.clone();
    if t.comments.iter().any(|c| c.id == id) {
        return false;
    }
    // Its number stays given.
    let n = (id.trim_start_matches([COMMENT_PREFIX, 'C']).parse::<u32>()).unwrap_or(0);
    t.comments_given = t.comments_given.max(n);
    t.comments
        .insert(removed.at.min(t.comments.len()), removed.comment);
    if let Some(check) = removed.check {
        t.checks.retain(|k| k.id != id);
        t.checks.push(check);
    }
    if let Some(round) = removed.retest {
        t.retest.insert(id, round);
    }
    true
}

/// Which criteria of a feature show their steps on Manual test's page, and
/// which the user read, each by its Given/When/Then as results match. A
/// feature's criteria start folded: none is open until the user opens it.
/// Kept in `state.json`, as it only says how the page shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folds {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub open: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub seen: BTreeSet<String>,
}

impl Folds {
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.seen.is_empty()
    }

    /// Open or fold criterion `text`; whether that changed anything. Opened,
    /// it counts as read.
    pub fn set(&mut self, text: &str, open: bool) -> bool {
        let changed = match open {
            true => self.open.insert(text.to_owned()),
            false => self.open.remove(text),
        };
        let read = open && self.see(text);
        changed || read
    }

    /// Note criterion `text` as read; whether it wasn't yet.
    pub fn see(&mut self, text: &str) -> bool {
        !self.seen.contains(text) && self.seen.insert(text.to_owned())
    }

    /// Only what `criteria` still say: a criterion edited in the brief
    /// starts folded and unread, as its result goes too.
    pub fn keep_only(&mut self, criteria: &[Criterion]) -> bool {
        let before = (self.open.len(), self.seen.len());
        let live = |t: &String| criteria.iter().any(|c| &c.text == t);
        self.open.retain(live);
        self.seen.retain(live);
        before != (self.open.len(), self.seen.len())
    }
}

/// Which criteria a fold button acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldScope {
    All,
    /// Those that passed.
    Done,
    /// Those the user read: opened, or marked.
    Seen,
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
                comment: false,
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
    /// The screenshots taken of a failure, by file name in the feature's
    /// data folder's `SHOTS_DIR`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<String>,
}

/// The folder of a feature's data folder holding the screenshots of its
/// manual test's failures.
pub const SHOTS_DIR: &str = "manual";

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
    /// The criteria whose pass a later fix round took back, as its agent
    /// said its fixes changed them: by id, with that round's number. They
    /// are to try again; trying one drops it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub retest: BTreeMap<String, u32>,
    /// The user's own comments, on no criterion (see `comment_criteria`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// How many comments were given ids: the next is one more.
    #[serde(default, skip_serializing_if = "super::is_zero")]
    pub comments_given: u32,
}

impl ManualTest {
    /// Whether it holds nothing: no result, comment or retest tag.
    fn is_empty(&self) -> bool {
        self.checks.is_empty() && self.retest.is_empty() && self.comments.is_empty()
    }
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
/// those, had their fix made) and aren't tried yet, the user's comments
/// counted with them: and of all, how many comments there are, done and
/// open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub passed: usize,
    pub failed: usize,
    pub sent: usize,
    pub made: usize,
    pub untested: usize,
    /// Of those passed, the comments marked done.
    pub comments_done: usize,
    /// Of those failed, the comments still open.
    pub comments_open: usize,
    /// Of those untested, the comments to look at again.
    pub comments_untested: usize,
    /// Those sent are in a fix plan still made or waiting for the user
    /// (`review_import::sent_word`).
    pub planning: bool,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.passed + self.failed + self.untested
    }

    /// How many comments there are.
    pub fn comments(&self) -> usize {
        self.comments_done + self.comments_open + self.comments_untested
    }
}

/// The counts of `criteria` in `test`.
pub fn counts(test: Option<&ManualTest>, criteria: &[Criterion]) -> Counts {
    let mut n = Counts::default();
    for c in criteria {
        let comment = usize::from(c.comment);
        match check(test, c) {
            Some(k) if k.state == Verdict::Pass => {
                n.passed += 1;
                n.comments_done += comment;
            }
            Some(k) => {
                n.failed += 1;
                n.comments_open += comment;
                n.sent += usize::from(k.sent);
                n.made += usize::from(k.fix_made());
            }
            None => {
                n.untested += 1;
                n.comments_untested += comment;
            }
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
    // A failure's note edited keeps the screenshots of what went wrong.
    let shots = (t.checks.iter())
        .find(|k| k.id == c.id && k.text == c.text && k.state == Verdict::Fail)
        .filter(|_| state == Verdict::Fail)
        .map(|k| k.shots.clone())
        .unwrap_or_default();
    t.checks.retain(|k| k.id != c.id);
    t.retest.remove(&c.id);
    t.checks.push(Check {
        id: c.id.clone(),
        text: c.text.clone(),
        state,
        note: note.to_string(),
        at: now,
        sent,
        patch: patch.to_string(),
        made: false,
        shots,
    });
}

/// Add screenshots `names` to the failure of criterion `c`; false when it
/// has none.
pub fn add_shots(test: &mut Option<ManualTest>, c: &Criterion, names: &[String]) -> bool {
    let Some(k) = (test.iter_mut().flat_map(|t| &mut t.checks))
        .find(|k| k.id == c.id && k.text == c.text && k.state == Verdict::Fail)
    else {
        return false;
    };
    for name in names {
        if !k.shots.contains(name) {
            k.shots.push(name.clone());
        }
    }
    true
}

/// The name of the next screenshot of criterion `id`, in a folder holding
/// `taken`: `AC-3-1.png`, then `AC-3-2.png`.
pub fn shot_name(id: &str, taken: &[String]) -> String {
    (1..)
        .map(|n| format!("{id}-{n}.png"))
        .find(|name| !taken.contains(name))
        .unwrap_or_default()
}

/// Forget the result of criterion `id`; true when there was one.
pub fn clear(test: &mut Option<ManualTest>, id: &str) -> bool {
    let Some(t) = test else { return false };
    let before = t.checks.len();
    t.checks.retain(|k| k.id != id);
    let gone = t.checks.len() != before;
    let tagged = t.retest.remove(id).is_some();
    if t.is_empty() {
        *test = None;
    }
    gone || tagged
}

/// Take back the passes of criteria `ids` given before `ended`, when fix
/// round `round` that changed them ended: they are untested again, tagged
/// with the round. Failures keep their own way (`Check::made`). Returns the
/// ids taken back.
pub fn retest(
    test: &mut Option<ManualTest>,
    ids: &[String],
    round: u32,
    ended: u64,
) -> Vec<String> {
    let Some(t) = test else {
        return Vec::new();
    };
    let mut back = Vec::new();
    t.checks.retain(|k| {
        let old = k.state == Verdict::Pass && k.at < ended && ids.contains(&k.id);
        if old {
            back.push(k.id.clone());
        }
        !old
    });
    for id in &back {
        t.retest.insert(id.clone(), round);
    }
    back
}

/// The acceptance criteria as a fix round's file lists them, the user's
/// comments with them, for its agent to name those its fixes change: none
/// when there are none.
pub fn criteria_block(criteria: &[Criterion]) -> String {
    if criteria.is_empty() {
        return String::new();
    }
    let what = match criteria.iter().any(|c| c.comment) {
        true => "acceptance criteria, and the user's comments (`C1`…),",
        false => "acceptance criteria,",
    };
    let mut md = format!(
        "\n# Acceptance criteria\n\nThe feature's {what} which the user tries by hand. Once the fixes are committed, add one line under this round's `## Fix round …` section of the build log naming those whose behaviour your fixes change, like `Criteria: AC-1, AC-3`, or `Criteria: none`.\n\n",
    );
    for c in criteria {
        md.push_str(&format!("- {}: {}\n", c.id, c.title));
    }
    md
}

/// The failures of `criteria` not yet sent to Build.
pub fn unsent<'a>(
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
#[cfg(test)]
pub fn unsent_count(test: Option<&ManualTest>, criteria: &[Criterion]) -> usize {
    unsent(test, criteria).len()
}

/// The failures and comments not sent yet, as Markdown for the Build agent of
/// feature `title`, and how many.
pub fn failures_markdown(
    test: Option<&ManualTest>,
    criteria: &[Criterion],
    title: &str,
) -> (usize, String) {
    let all = unsent(test, criteria);
    let (comments, fails): (Vec<_>, Vec<_>) = all.iter().partition(|(c, _)| c.comment);
    let mut md = match (fails.is_empty(), comments.is_empty()) {
        (false, true) => format!(
            "# Manual test failures\n\nThe user tried \u{201c}{title}\u{201d} by hand against its acceptance criteria, and these failed. Fix each one.\n"
        ),
        (true, false) => format!(
            "# Manual test comments\n\nThe user tried \u{201c}{title}\u{201d} by hand, and wants these changes. Make each one.\n"
        ),
        _ => format!(
            "# Manual test failures and comments\n\nThe user tried \u{201c}{title}\u{201d} by hand against its acceptance criteria. Fix each failure, and make each change their comments ask for.\n"
        ),
    };
    for (c, k) in &fails {
        md.push_str(&format!("\n## {}: {}\n\n{}\n\n", c.id, c.title, c.text));
        md.push_str(&format!("What went wrong: {}\n", k.note));
        md.push_str(&shots_markdown(k));
    }
    for (c, k) in &comments {
        md.push_str(&format!("\n## {}: Comment\n\n{}\n", c.id, c.text));
        if !k.note.is_empty() {
            md.push_str(&format!("\n{}\n", k.note));
        }
        md.push_str(&shots_markdown(k));
    }
    (all.len(), md)
}

/// Where the screenshots of `k` are, for its agent.
fn shots_markdown(k: &Check) -> String {
    (k.shots.iter())
        .map(|shot| {
            format!(
                "\nScreenshot: {}/{SHOTS_DIR}/{shot}\n",
                super::layout::DOCS_LINK
            )
        })
        .collect()
}

/// The failures and comments not sent yet as the plan gerdr writes of them,
/// when the user wants them made with none of the agent's (see
/// `review_import::fixes_plan`).
pub fn fixes_plan(test: Option<&ManualTest>, criteria: &[Criterion], title: &str) -> String {
    let mut md = format!(
        "# Fix plan of \u{201c}{title}\u{201d}\n\ngerdr wrote this from the manual test: the user wants these made as they say, so no plan was asked for. Find each one's cause in the code before you change it.\n\n## {}\n",
        super::fix_round::FIXES
    );
    let flat = super::review_import::no_headings;
    for (c, k) in unsent(test, criteria) {
        md.push_str(&format!(
            "\n### {}: {}\n\n{}\n",
            c.id,
            c.title,
            flat(&c.text)
        ));
        match c.comment {
            false => md.push_str(&format!("\nWhat went wrong: {}\n", flat(&k.note))),
            true if !k.note.is_empty() => md.push_str(&format!("\n{}\n", flat(&k.note))),
            true => {}
        }
        md.push_str(&shots_markdown(k));
    }
    md
}

/// The failures not sent yet, as a fix round keeps what it was asked.
pub fn asked(test: Option<&ManualTest>, criteria: &[Criterion]) -> Vec<super::fix_round::Asked> {
    (unsent(test, criteria).into_iter())
        .map(|(c, k)| super::fix_round::Asked {
            id: c.id.clone(),
            title: c.title.clone(),
            note: k.note.clone(),
            shots: k.shots.len(),
        })
        .collect()
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
    #[test]
    fn folds_say_what_is_open_and_read_and_forget_edited_criteria() {
        let c = |id: &str, text: &str| Criterion {
            id: id.into(),
            title: String::new(),
            text: text.into(),
            comment: false,
        };
        let mut folds = Folds::default();
        // Opened, it is read too; opened again, nothing changes.
        assert!(folds.set("a", true));
        assert!(!folds.set("a", true));
        assert!(folds.seen.contains("a"));
        // Folded, it stays read.
        assert!(folds.set("a", false));
        assert!(folds.open.is_empty() && folds.seen.contains("a"));
        assert!(folds.see("b") && !folds.see("b"));
        folds.set("b", true);
        // What the brief no longer says goes.
        assert!(folds.keep_only(&[c("AC-1", "b")]));
        assert_eq!(folds.seen.iter().collect::<Vec<_>>(), ["b"]);
        assert!(!folds.keep_only(&[c("AC-1", "b")]));
        assert!(!folds.is_empty());
    }

    use super::*;

    #[test]
    fn a_comment_is_tried_as_a_criterion_after_the_briefs_and_sent_with_the_failures() {
        let all = parse_criteria(BRIEF);
        let mut t = None;
        set(&mut t, &all[0], "p1", Verdict::Fail, "stays", 10);
        let id = add_comment(
            &mut t,
            "Make the settings page look better\nCards",
            "p1",
            20,
        );
        assert_eq!(id, "T1");
        let comments = comment_criteria(t.as_ref());
        assert_eq!(comments.len(), 1);
        let c = &comments[0];
        assert!(c.comment);
        assert_eq!(c.title, "Make the settings page look better\u{2026}");
        // It starts open: what it says should change.
        assert_eq!(check(t.as_ref(), c).map(|k| k.state), Some(Verdict::Fail));
        let mut criteria = all.clone();
        criteria.extend(comments.clone());
        let n = counts(t.as_ref(), &criteria);
        assert_eq!((n.failed, n.comments_open, n.comments()), (2, 1, 1));
        // Sent with the failure, after it.
        let (sent, md) = failures_markdown(t.as_ref(), &criteria, "Bulk");
        assert_eq!(sent, 2);
        assert!(
            md.starts_with("# Manual test failures and comments"),
            "{md}"
        );
        let at = |s: &str| md.find(s).unwrap();
        assert!(
            at("## AC-1:") < at("## T1: Comment\n\nMake the settings page"),
            "{md}"
        );
        let plan = fixes_plan(t.as_ref(), &criteria, "Bulk");
        let items = super::super::fix_round::items(&plan, &[]);
        assert_eq!(items.len(), 2, "{plan}");
        assert!(items[1].starts_with("T1: "), "{items:?}");
        assert!(criteria_block(&criteria).contains("- T1: Make the settings"));
        // Only comments: said so.
        let (_, md) = failures_markdown(t.as_ref(), &comments, "Bulk");
        assert!(md.starts_with("# Manual test comments"), "{md}");
        // Done, it passes; the next gets the next number.
        set(&mut t, c, "p1", Verdict::Pass, "", 30);
        assert_eq!(counts(t.as_ref(), &criteria).comments_done, 1);
        assert_eq!(add_comment(&mut t, "Bigger buttons", "p1", 40), "T2");
    }

    #[test]
    fn an_edited_comment_is_open_again_and_a_deleted_one_comes_back() {
        let mut t = None;
        add_comment(&mut t, "Bigger buttons", "p1", 10);
        let c = comment_criteria(t.as_ref()).remove(0);
        set(&mut t, &c, "p1", Verdict::Pass, "", 20);
        assert_eq!(edit_comment(&mut t, "T1", "Bigger buttons", "p1", 30), None);
        let was = edit_comment(&mut t, "T1", "Bigger, round buttons", "p2", 30);
        assert_eq!(was.as_deref(), Some("Bigger buttons"));
        let c = comment_criteria(t.as_ref()).remove(0);
        assert_eq!(c.text, "Bigger, round buttons");
        let k = check(t.as_ref(), &c).unwrap();
        assert_eq!(
            (k.state, k.sent, k.patch.as_str()),
            (Verdict::Fail, false, "p2")
        );
        assert_eq!(t.as_ref().unwrap().checks.len(), 1);
        // Deleted, nothing is left; put back, all of it is.
        let before = t.clone();
        let removed = delete_comment(&mut t, "T1").unwrap();
        assert_eq!(t, None);
        assert!(restore_comment(&mut t, removed.clone()));
        let kept =
            |t: &Option<ManualTest>| t.as_ref().map(|t| (t.checks.clone(), t.comments.clone()));
        assert_eq!(kept(&t), kept(&before));
        assert!(!restore_comment(&mut t, removed));
        // Its number isn't given again.
        assert_eq!(add_comment(&mut t, "More", "p2", 40), "T2");
        // Clearing a comment's result keeps the comment.
        assert!(clear(&mut t, "T1"));
        assert_eq!(comment_criteria(t.as_ref()).len(), 2);
    }

    const BRIEF: &str = "# Bulk edit\n\n## Outcome\nGiven nothing, when no, then not this.\n\n### ACCEPTANCE Criteria:\n- [ ] Given two rows, when I edit both, then both change.\n1. Given one row, when I clear it, then it is empty.\n- not a criterion\n```\n- Given a fence, when read, then skipped.\n```\n* [x] Given a done box, when read, then kept.\n\n## Notes\n- Given later, when read, then not in.\n";

    fn crit(n: usize, text: &str) -> Criterion {
        Criterion {
            id: format!("AC-{n}"),
            title: fallback_title(text),
            text: text.into(),
            comment: false,
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
    fn a_failures_screenshots_stay_with_it_and_go_to_build() {
        let a = crit(1, "Given a, when b, then c.");
        let all = vec![a.clone()];
        let mut t = None;
        // Nothing failed: nothing to put them on.
        assert!(!add_shots(&mut t, &a, &["AC-1-1.png".into()]));
        set(&mut t, &a, "p", Verdict::Fail, "nothing happens", 1);
        assert!(add_shots(&mut t, &a, &["AC-1-1.png".into(), "AC-1-1.png".into()]));
        assert_eq!(check(t.as_ref(), &a).unwrap().shots, ["AC-1-1.png"]);
        // Its note edited, they stay; passed, they go with the failure.
        set(&mut t, &a, "p", Verdict::Fail, "it crashes", 2);
        assert_eq!(check(t.as_ref(), &a).unwrap().shots, ["AC-1-1.png"]);
        let (_, md) = failures_markdown(t.as_ref(), &all, "Bulk");
        assert!(md.contains("Screenshot: feature-docs/manual/AC-1-1.png"), "{md}");
        set(&mut t, &a, "p", Verdict::Pass, "", 3);
        assert!(check(t.as_ref(), &a).unwrap().shots.is_empty());
        assert_eq!(shot_name("AC-1", &[]), "AC-1-1.png");
        assert_eq!(shot_name("AC-1", &["AC-1-1.png".into()]), "AC-1-2.png");
    }

    #[test]
    fn a_fix_that_changed_a_passed_criterion_takes_its_pass_back() {
        let [a, b, c] = [1, 2, 3].map(|n| crit(n, &format!("Given {n}, when b, then c.")));
        let all = vec![a.clone(), b.clone(), c.clone()];
        let mut t = None;
        set(&mut t, &a, "p", Verdict::Pass, "", 10);
        set(&mut t, &b, "p", Verdict::Fail, "broke", 10);
        set(&mut t, &c, "p", Verdict::Pass, "", 30);
        let named = ["AC-1", "AC-2", "AC-3", "AC-9"].map(str::to_owned);
        // Only passes from before the round ended go; a failure keeps its own way.
        assert_eq!(retest(&mut t, &named, 4, 20), ["AC-1"]);
        let test = t.as_ref().unwrap();
        assert_eq!(test.retest.get("AC-1"), Some(&4));
        assert_eq!(check(t.as_ref(), &b).unwrap().state, Verdict::Fail);
        assert!(check(t.as_ref(), &c).is_some());
        assert_eq!(counts(t.as_ref(), &all).untested, 1);
        // Tried again, it drops the tag.
        set(&mut t, &a, "q", Verdict::Pass, "", 40);
        assert!(t.as_ref().unwrap().retest.is_empty());
        // The only thing left, a tag keeps the record; clearing drops it.
        let mut only = None;
        set(&mut only, &a, "p", Verdict::Pass, "", 1);
        assert_eq!(retest(&mut only, &named, 5, 2), ["AC-1"]);
        assert!(only.as_ref().is_some_and(|t| t.checks.is_empty()));
        assert!(clear(&mut only, "AC-1"));
        assert!(only.is_none());
        assert!(retest(&mut None, &named, 5, 2).is_empty());
        // The criteria list for a fix round's agent.
        assert_eq!(criteria_block(&[]), "");
        let block = criteria_block(&all);
        assert!(block.contains("- AC-1: C\n") && block.contains("`Criteria: none`"));
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
