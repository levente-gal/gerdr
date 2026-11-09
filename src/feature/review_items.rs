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

use serde::{Deserialize, Serialize};

use super::review_import::{Decision, FIXED, FindingState, SENT, Severity};

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
    /// The button wanting it: "Fix".
    pub want: &'static str,
    /// The button taking that back, on one of the reviewer's: "Don't fix".
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
    /// The rewording dialog's title, before the id: "Edit the fix of".
    pub edit_title: &'static str,
    /// The rewording dialog's field label: "Fix".
    pub edit_label: &'static str,
    /// The rewording dialog's button when saving wants it too: "Save and fix".
    pub save_want: &'static str,
    /// One of the user's own with nothing typed: "Say what to fix."
    pub say_what: &'static str,
    /// The button wanting every open one: "Fix all open".
    pub want_all: &'static str,
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
    want: "Fix",
    unwant: "Don't fix",
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
    edit_title: "Edit the fix of",
    edit_label: "Fix",
    save_want: "Save and fix",
    say_what: "Say what to fix.",
    want_all: "Fix all open",
    comment: "Your comment",
    answer: "Your comment",
    own_goes: "It goes to the Build agent with the fixes",
    none_here: "Move the cursor to a finding first",
};

const SUGGESTION: Words = Words {
    item: "suggestion",
    items: "suggestions",
    want: "Accept",
    unwant: "Don't accept",
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
    why_hint: "Why the plan shouldn't take it",
    say_why: "Say why you reject it.",
    edit_title: "Edit the change of",
    edit_label: "Change",
    save_want: "Save and accept",
    say_what: "Say what the plan should change.",
    want_all: "Accept all open",
    comment: "Your comment",
    answer: "Your answer",
    own_goes: "It goes into the final plan",
    none_here: "Move the cursor to a suggestion first",
};

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
            Err(format!("{id} was sent to Build: the next review checks it"))
        }
        (ItemOf::Finding, Standing::Fixed) => Err(format!("{id} is fixed already")),
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

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [ItemOf; 2] = [ItemOf::Finding, ItemOf::Suggestion];

    #[test]
    fn each_kind_has_its_own_words() {
        let f = words(ItemOf::Finding);
        assert_eq!(
            (f.want, f.unwant, f.refuse),
            ("Fix", "Don't fix", "Dismiss\u{2026}")
        );
        assert_eq!(
            (f.show, f.wanted, f.refused),
            ("Show in diff", "to fix", "dismissed")
        );
        assert_eq!(f.edit, "Edit fix\u{2026}");
        let s = words(ItemOf::Suggestion);
        assert_eq!(
            (s.want, s.unwant, s.refuse),
            ("Accept", "Don't accept", "Reject\u{2026}")
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
