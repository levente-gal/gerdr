//! What AI review's findings and Plan review's suggestions share: the items
//! a review stage shows as cards, decides with the same keys and words of
//! its own (`Words`), and keeps in order (`rank`).
//!
//! The records stay per stage (`review_import::Finding`,
//! `plan_review::Suggestion`); each maps where it stands onto `Standing`
//! (`FindingState`): a suggestion is open, accepted (`Fix`) or rejected
//! (`Dismissed`), and never sent or fixed.
//!
//! The decide keys speak `review_import::Decision` for both: Fix is accept,
//! Dismiss is reject, Reopen is clear.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::review_import::{Decision, FIXED, FindingState, POSTED, SENT, Severity};

/// Which stage's item it is. Ids like `r1-2` and `u1` are in both, so this
/// goes in every key of one: card, armed key, draft, Undo and signal.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ItemOf {
    /// AI review's.
    #[default]
    Finding,
    /// Plan review's.
    Suggestion,
}

/// Where an item stands, whichever stage's it is.
pub type Standing = FindingState;

/// Why a suggestion can't change where it stands: suggestions are never sent
/// or fixed, so only a broken record says this.
pub const SUGGESTION_DONE: &str = "That suggestion can't change now";

/// What a kind of item is called, and what its cards, keys, dialogs and
/// diff or doc flags say. Nothing the user reads of an item is written
/// anywhere else, so "fix" or "dismiss" never shows on a suggestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    /// One of them: "finding".
    pub item: &'static str,
    /// More than one: "findings".
    pub items: &'static str,
    /// Who raised the reviewer's, on its flag: "AI review".
    pub by: &'static str,
    /// One of the user's own, on its flag: "Your comment".
    pub own_head: &'static str,
    /// The button wanting it: "Fix".
    pub want: &'static str,
    /// The button taking that back, on one of the reviewer's: "Reopen", the
    /// word that puts any decided one back to undecided.
    pub unwant: &'static str,
    /// The button refusing it, asking why: "Dismiss…".
    pub refuse: &'static str,
    /// The refusing dialog's title and button: "Dismiss".
    pub refuse_verb: &'static str,
    /// The button putting a refused one back: "Reopen".
    pub reopen: &'static str,
    /// The button rewording the reviewer's change: "Edit fix…".
    pub edit: &'static str,
    /// The button rewording one of the user's own: "Edit…".
    pub edit_own: &'static str,
    /// The quiet button showing where it is: "Show in diff".
    pub show: &'static str,
    /// Where a wanted one stands, on its card: "to fix".
    pub wanted: &'static str,
    /// Where a refused one stands, before the reason: "dismissed".
    pub refused: &'static str,
    /// What the reviewer would change, before it: "Fix".
    pub change: &'static str,
    /// The user's wording of that, before it: "Your fix".
    pub my_change: &'static str,
    /// Its place names nowhere the user can see: "not in the diff".
    pub not_placed: &'static str,
    /// The refusing dialog's field: "Why this doesn't need fixing".
    pub why_hint: &'static str,
    /// The refusing dialog with nothing typed: "Say why you dismiss it."
    pub say_why: &'static str,
    /// The rewording dialog's title, before the id: "Edit fix of", as its button says it.
    pub edit_title: &'static str,
    /// The rewording dialog's field label: "Fix".
    pub edit_label: &'static str,
    /// The rewording dialog's button when saving wants it too: "Save and fix".
    pub save_want: &'static str,
    /// One of the user's own with nothing typed: "Say what to fix."
    pub say_what: &'static str,
    /// The button wanting every open one: "Fix all open".
    pub want_all: &'static str,
    /// The page's button adding one of the user's own: "Add comment…".
    pub add_own: &'static str,
    /// Their list on the page, over them: "Findings".
    pub heading: &'static str,
    /// What a review reads, changing since: "code".
    pub reads: &'static str,
    /// The field of one of the user's own, in its dialog: "Comment".
    pub own_label: &'static str,
    /// That field with nothing typed: "What should change".
    pub own_placeholder: &'static str,
    /// The dialog adding one of the user's own: "Add comment", as its button says it.
    pub add_title: &'static str,
    /// The user's comment on one, over it: "Your comment".
    pub comment: &'static str,
    /// The user's comment on a question, over it: "Your answer".
    pub answer: &'static str,
    /// Where an own one goes: "It goes to the Build agent with the fixes".
    pub own_goes: &'static str,
    /// The card or flag under the cursor isn't one: "Move the cursor to a
    /// finding first".
    pub none_here: &'static str,
}

const FINDING: Words = Words {
    item: "finding",
    items: "findings",
    by: "AI review",
    own_head: "Your comment",
    want: "Fix",
    unwant: "Reopen",
    refuse: "Dismiss\u{2026}",
    refuse_verb: "Dismiss",
    reopen: "Reopen",
    edit: "Edit fix\u{2026}",
    edit_own: "Edit\u{2026}",
    show: "Show in diff",
    wanted: "to fix",
    refused: "dismissed",
    change: "Fix",
    my_change: "Your fix",
    not_placed: "not in the diff",
    why_hint: "Why this doesn't need fixing",
    say_why: "Say why you dismiss it.",
    edit_title: "Edit fix of",
    edit_label: "Fix",
    save_want: "Save and fix",
    say_what: "Say what to fix.",
    want_all: "Fix all open",
    add_own: "Add comment\u{2026}",
    heading: "Findings",
    reads: "code",
    own_label: "Comment",
    own_placeholder: "What should change",
    add_title: "Add comment",
    comment: "Your comment",
    answer: "Your comment",
    own_goes: "It goes to the Build agent with the fixes",
    none_here: "Move the cursor to a finding first",
};

const SUGGESTION: Words = Words {
    item: "suggestion",
    items: "suggestions",
    by: "Plan review",
    own_head: "Your suggestion",
    want: "Accept",
    unwant: "Reopen",
    refuse: "Reject\u{2026}",
    refuse_verb: "Reject",
    reopen: "Reopen",
    edit: "Edit change\u{2026}",
    edit_own: "Edit\u{2026}",
    show: "Show in plan",
    wanted: "accepted",
    refused: "rejected",
    change: "Change",
    my_change: "Your change",
    not_placed: "not placed in the plan",
    why_hint: "Why the plan doesn't need this",
    say_why: "Say why you reject it.",
    edit_title: "Edit change of",
    edit_label: "Change",
    save_want: "Save and accept",
    say_what: "Say what the plan should change.",
    want_all: "Accept all open",
    add_own: "Add suggestion\u{2026}",
    heading: "Suggestions",
    reads: "plan",
    own_label: "Suggestion",
    own_placeholder: "Its title on the first line, then the change",
    add_title: "Add suggestion",
    comment: "Your comment",
    answer: "Your answer",
    own_goes: "It goes into the final plan",
    none_here: "Move the cursor to a suggestion first",
};

/// AI review's findings on someone else's pull request: what is wanted of
/// one goes to GitHub as a comment of the review, not to an agent to fix.
const PR_FINDING: Words = Words {
    want: "Comment",
    wanted: "to comment",
    save_want: "Save and comment",
    say_what: "Say what to comment.",
    want_all: "Comment on all open",
    why_hint: "Why this needs no comment",
    own_goes: "It goes to GitHub with the review",
    ..FINDING
};

/// `words(of)`, for a review that `posts` its findings to GitHub (someone
/// else's pull request, `Feature::posts_review`).
pub fn words_in(of: ItemOf, posts: bool) -> &'static Words {
    match (of, posts) {
        (ItemOf::Finding, true) => &PR_FINDING,
        _ => words(of),
    }
}

/// What items `of` are called and what their cards and keys say.
pub fn words(of: ItemOf) -> &'static Words {
    match of {
        ItemOf::Finding => &FINDING,
        ItemOf::Suggestion => &SUGGESTION,
    }
}

/// Where an item goes in its review's list, those of a run being stable
/// within this: the user's own first, then by severity.
pub fn rank(yours: bool, severity: Severity) -> (bool, Severity) {
    (!yours, severity)
}

/// What `f` (Fix, Accept) does to an item `of` standing at `standing`: want
/// it (from undecided or refused), or back to undecided (from wanted); else
/// why it does nothing.
pub fn want_key(of: ItemOf, standing: Standing) -> Result<Decision, &'static str> {
    match (of, standing) {
        (_, Standing::Open | Standing::Dismissed) => Ok(Decision::Fix),
        (_, Standing::Fix) => Ok(Decision::Reopen),
        (ItemOf::Finding, Standing::Sent) => Err(SENT),
        (ItemOf::Finding, Standing::Fixed) => Err(FIXED),
        (_, Standing::Posted) => Err(POSTED),
        (ItemOf::Suggestion, Standing::Sent | Standing::Fixed) => Err(SUGGESTION_DONE),
    }
}

/// What Del (Dismiss, Reject) does to an item `of` standing at `standing`:
/// `None` to ask why and refuse it (from undecided or wanted), `Some` back
/// to undecided (from refused); else why it does nothing.
pub fn refuse_key(of: ItemOf, standing: Standing) -> Result<Option<Decision>, &'static str> {
    match (of, standing) {
        (_, Standing::Open | Standing::Fix) => Ok(None),
        (_, Standing::Dismissed) => Ok(Some(Decision::Reopen)),
        (ItemOf::Finding, Standing::Sent) => Err(SENT),
        (ItemOf::Finding, Standing::Fixed) => Err(FIXED),
        (_, Standing::Posted) => Err(POSTED),
        (ItemOf::Suggestion, Standing::Sent | Standing::Fixed) => Err(SUGGESTION_DONE),
    }
}

/// Whether an item `of`, `id`, standing at `standing` can be reworded or
/// commented on: undecided or wanted; else why not.
pub fn editable(of: ItemOf, id: &str, standing: Standing) -> Result<(), String> {
    match (of, standing) {
        (_, Standing::Open | Standing::Fix) => Ok(()),
        (ItemOf::Finding, Standing::Dismissed) => {
            Err(format!("{id} is dismissed: reopen it first"))
        }
        (ItemOf::Finding, Standing::Sent) => {
            Err(format!("{id} is being fixed: the next review checks it"))
        }
        (ItemOf::Finding, Standing::Fixed) => Err(format!("{id} is fixed already")),
        (_, Standing::Posted) => Err(format!("{id} is on GitHub already")),
        (ItemOf::Suggestion, Standing::Dismissed) => {
            Err(format!("{id} is rejected: reopen it first"))
        }
        (ItemOf::Suggestion, Standing::Sent | Standing::Fixed) => Err(SUGGESTION_DONE.to_owned()),
    }
}

/// Whether a comment of the user's on an item `of`, `id`, standing at
/// `standing` can be written, changed or deleted: a refused one keeps its
/// own, as saying why is part of refusing. Else why not.
pub fn note_editable(of: ItemOf, id: &str, standing: Standing) -> Result<(), String> {
    match standing {
        Standing::Dismissed => Ok(()),
        standing => editable(of, id, standing),
    }
}

/// An item a reviewer run reads into (`merge_run`).
pub trait RunItem {
    fn id(&self) -> &str;
    /// Whether it is the user's: they wrote it, or it went out (posted to
    /// GitHub). The reviewer never rewords one.
    fn own(&self) -> bool;
}

/// What `merge_run` did.
#[derive(Debug, Default)]
pub struct Merged {
    /// Raised for the first time.
    pub new: usize,
    /// Earlier ones the reviewer raised again and reworded.
    pub again: usize,
    /// The ids it raised, new and reworded, in its order.
    pub ids: Vec<String>,
    /// Earlier ones raised again, reworded or the user's.
    pub raised: BTreeSet<String>,
}

/// Read run `run`'s `raws` into `items`, which a stage keeps in its own
/// record:
///
/// - `earlier` finds the earlier item a raw raises again, of those there
///   before the run and given the ones raised so far (it must skip those);
/// - one of the user's own (`own`) keeps its words and the raw goes in as a
///   new item; any other is reworded (`reword`);
/// - every other raw is new (`new`), its id `r<run>-<k>`;
/// - `settle` then takes each earlier item, with whether the run raised it
///   again.
pub fn merge_run<T: RunItem, R>(
    items: &mut Vec<T>,
    run: u32,
    raws: impl IntoIterator<Item = R>,
    earlier: impl Fn(&[T], &R, &BTreeSet<String>) -> Option<usize>,
    mut reword: impl FnMut(&mut T, R),
    mut new: impl FnMut(String, R) -> T,
    mut settle: impl FnMut(&mut T, bool),
) -> Merged {
    let mut out = Merged::default();
    let before = items.len();
    let mut k = 0;
    for raw in raws {
        let i = earlier(&items[..before], &raw, &out.raised);
        if let Some(i) = i {
            out.raised.insert(items[i].id().to_owned());
        }
        if let Some(t) = i.map(|i| &mut items[i]).filter(|t| !t.own()) {
            out.ids.push(t.id().to_owned());
            reword(t, raw);
            out.again += 1;
            continue;
        }
        let id = loop {
            k += 1;
            let id = format!("r{run}-{k}");
            if !items.iter().any(|t| t.id() == id) {
                break id;
            }
        };
        out.ids.push(id.clone());
        items.push(new(id, raw));
        out.new += 1;
    }
    for t in &mut items[..before] {
        settle(t, out.raised.contains(t.id()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [ItemOf; 2] = [ItemOf::Finding, ItemOf::Suggestion];

    /// An item of id, words and whether it is the user's.
    struct Item(String, String, bool);

    impl RunItem for Item {
        fn id(&self) -> &str {
            &self.0
        }

        fn own(&self) -> bool {
            self.2
        }
    }

    #[test]
    fn a_run_never_rewords_the_users_own_and_settles_the_rest() {
        let mut items = vec![
            Item("u1".into(), "mine".into(), true),
            Item("r1-1".into(), "old".into(), false),
            Item("r1-2".into(), "gone".into(), false),
        ];
        let raws = [("u1", "theirs"), ("r1-1", "reworded"), ("r1-1", "twice")];
        let mut settled = Vec::new();
        let got = merge_run(
            &mut items,
            2,
            raws,
            |earlier, (id, _), raised| {
                (earlier.iter()).position(|t| t.0 == *id && !raised.contains(*id))
            },
            |t, (_, words)| t.1 = words.into(),
            |id, (_, words)| Item(id, words.into(), false),
            |t, raised| settled.push((t.0.clone(), raised)),
        );
        let words: Vec<_> = items.iter().map(|t| (&*t.0, &*t.1)).collect();
        assert_eq!(
            words,
            [
                ("u1", "mine"),
                ("r1-1", "reworded"),
                ("r1-2", "gone"),
                ("r2-1", "theirs"),
                ("r2-2", "twice")
            ]
        );
        assert_eq!((got.new, got.again), (2, 1));
        assert_eq!(got.ids, ["r2-1", "r1-1", "r2-2"]);
        let raised = |id: &str| (id.to_owned(), id != "r1-2");
        assert_eq!(settled, [raised("u1"), raised("r1-1"), raised("r1-2")]);
    }

    #[test]
    fn each_kind_has_its_own_words() {
        let f = words(ItemOf::Finding);
        assert_eq!(
            (f.want, f.unwant, f.refuse),
            ("Fix", "Reopen", "Dismiss\u{2026}")
        );
        assert_eq!(
            (f.show, f.wanted, f.refused),
            ("Show in diff", "to fix", "dismissed")
        );
        assert_eq!(f.edit, "Edit fix\u{2026}");
        let s = words(ItemOf::Suggestion);
        assert_eq!(
            (s.want, s.unwant, s.refuse),
            ("Accept", "Reopen", "Reject\u{2026}")
        );
        assert_eq!(
            (s.show, s.wanted, s.refused),
            ("Show in plan", "accepted", "rejected")
        );
        assert_eq!((s.edit, s.answer), ("Edit change\u{2026}", "Your answer"));
    }

    #[test]
    fn a_suggestion_never_says_fix_or_dismiss() {
        let s = words(ItemOf::Suggestion);
        let all = [
            s.item,
            s.items,
            s.by,
            s.own_head,
            s.want,
            s.unwant,
            s.refuse,
            s.refuse_verb,
            s.reopen,
            s.edit,
            s.edit_own,
            s.show,
            s.wanted,
            s.refused,
            s.change,
            s.my_change,
            s.not_placed,
            s.why_hint,
            s.say_why,
            s.edit_title,
            s.edit_label,
            s.save_want,
            s.say_what,
            s.want_all,
            s.own_label,
            s.own_placeholder,
            s.add_title,
            s.comment,
            s.answer,
            s.own_goes,
            s.none_here,
            SUGGESTION_DONE,
        ];
        for text in all {
            let lower = text.to_lowercase();
            assert!(
                !lower.contains("fix") && !lower.contains("dismiss") && !lower.contains("finding"),
                "{text}"
            );
        }
    }

    #[test]
    fn the_keys_want_refuse_and_reopen_alike() {
        for of in BOTH {
            assert_eq!(want_key(of, Standing::Open), Ok(Decision::Fix));
            assert_eq!(want_key(of, Standing::Dismissed), Ok(Decision::Fix));
            assert_eq!(want_key(of, Standing::Fix), Ok(Decision::Reopen));
            assert_eq!(refuse_key(of, Standing::Open), Ok(None));
            assert_eq!(refuse_key(of, Standing::Fix), Ok(None));
            assert_eq!(
                refuse_key(of, Standing::Dismissed),
                Ok(Some(Decision::Reopen))
            );
        }
        assert_eq!(want_key(ItemOf::Finding, Standing::Sent), Err(SENT));
        assert_eq!(refuse_key(ItemOf::Finding, Standing::Fixed), Err(FIXED));
        assert_eq!(
            want_key(ItemOf::Suggestion, Standing::Sent),
            Err(SUGGESTION_DONE)
        );
    }

    #[test]
    fn a_refused_item_keeps_its_comment_open_but_not_its_wording() {
        for of in BOTH {
            assert!(editable(of, "r1-1", Standing::Open).is_ok());
            assert!(editable(of, "r1-1", Standing::Fix).is_ok());
            assert!(editable(of, "r1-1", Standing::Dismissed).is_err());
            assert!(note_editable(of, "r1-1", Standing::Dismissed).is_ok());
        }
        assert_eq!(
            editable(ItemOf::Finding, "r1-1", Standing::Dismissed).unwrap_err(),
            "r1-1 is dismissed: reopen it first"
        );
        assert_eq!(
            editable(ItemOf::Suggestion, "r1-1", Standing::Dismissed).unwrap_err(),
            "r1-1 is rejected: reopen it first"
        );
        assert!(note_editable(ItemOf::Finding, "r1-1", Standing::Sent).is_err());
    }

    #[test]
    fn the_users_own_come_first_then_by_severity() {
        let mut list = vec![
            (false, Severity::Low),
            (true, Severity::Low),
            (false, Severity::High),
            (true, Severity::High),
        ];
        list.sort_by_key(|(yours, severity)| rank(*yours, *severity));
        assert_eq!(
            list,
            [
                (true, Severity::High),
                (true, Severity::Low),
                (false, Severity::High),
                (false, Severity::Low),
            ]
        );
    }

    #[test]
    fn an_old_record_is_a_finding() {
        assert_eq!(ItemOf::default(), ItemOf::Finding);
        let of: ItemOf = serde_json::from_str("\"suggestion\"").unwrap();
        assert_eq!(of, ItemOf::Suggestion);
    }
}
