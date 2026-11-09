//! The cards of a review stage's page: AI review's findings and Plan
//! review's suggestions (`Item`), with the comments the user left in the
//! diff among them. What each card shows, its buttons, and what its keys do
//! are here, the same for both kinds; every word comes from the kind's
//! `review_items::Words`. The records and what deciding does stay per stage
//! (`feature_review`, `feature_plan_review`).

use crate::{
    chrome::{ButtonStyle, CardStripe, FindingCard, FindingTag, PageButton, StageMark},
    feature::{
        Feature, Stage, Status,
        plan_review::{Source, Suggestion},
        review_import::{self, Decision, Finding as ReviewFinding, FindingState, Severity},
        review_items::{self, ItemOf, Standing, words},
    },
    features::{PanelOutcome, can_edit_note, shown_notes, stage_mark},
    review::Comment,
};

/// One item a review stage's page shows as a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item<'a> {
    Finding(&'a ReviewFinding),
    #[allow(dead_code)] // listed by `items` once Plan review has cards (wave 1 contract)
    Suggestion(&'a Suggestion),
}

impl<'a> Item<'a> {
    /// Which kind it is.
    pub(crate) fn of(self) -> ItemOf {
        match self {
            Self::Finding(_) => ItemOf::Finding,
            Self::Suggestion(_) => ItemOf::Suggestion,
        }
    }

    pub(crate) fn id(self) -> &'a str {
        match self {
            Self::Finding(f) => &f.id,
            Self::Suggestion(s) => &s.id,
        }
    }

    #[allow(dead_code)] // for Plan review's cards (wave 1 contract)
    pub(crate) fn title(self) -> &'a str {
        match self {
            Self::Finding(f) => &f.title,
            Self::Suggestion(s) => &s.title,
        }
    }

    /// The user added it.
    pub(crate) fn yours(self) -> bool {
        match self {
            Self::Finding(f) => f.yours,
            Self::Suggestion(s) => s.source == Source::You,
        }
    }

    pub(crate) fn severity(self) -> Severity {
        match self {
            Self::Finding(f) => f.severity,
            Self::Suggestion(s) => s.severity,
        }
    }

    /// Where it stands, as both kinds say it.
    pub(crate) fn standing(self) -> Standing {
        match self {
            Self::Finding(f) => f.state,
            Self::Suggestion(s) => s.standing(),
        }
    }

    /// The user's comment on it; a question's answer.
    pub(crate) fn note(self) -> &'a str {
        match self {
            Self::Finding(f) => &f.note,
            Self::Suggestion(s) => &s.note,
        }
    }

    /// It has a place to show it at: a finding with a file, in the diff;
    /// any suggestion, in the plan.
    pub(crate) fn shows(self) -> bool {
        match self {
            Self::Finding(f) => !f.file.is_empty(),
            Self::Suggestion(_) => true,
        }
    }

    #[allow(dead_code)] // WAVE1-STUB(B): features.rs goes by `item` now
    pub(crate) fn finding(self) -> Option<&'a ReviewFinding> {
        match self {
            Self::Finding(f) => Some(f),
            Self::Suggestion(_) => None,
        }
    }

    #[allow(dead_code)] // for Plan review's cards (wave 1 contract)
    pub(crate) fn suggestion(self) -> Option<&'a Suggestion> {
        match self {
            Self::Finding(_) => None,
            Self::Suggestion(s) => Some(s),
        }
    }
}

/// What a card's button or key does to its item, whichever kind it is.
/// Decisions speak AI review's words (`review_items`): Fix is accept,
/// Dismiss is reject, Reopen is clear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemAct {
    Decide(Decision),
    /// Ask why, to refuse it (Dismiss…, Reject…).
    AskRefuse,
    /// Reword its change, or one of the user's own.
    Edit,
    /// Write or edit the user's comment on it, on its card.
    Comment,
    DeleteComment,
    /// Delete it: one of the user's own.
    DeleteOwn,
    /// Show it where it is: in the diff, in the plan.
    Show,
    /// Open its first citation in Zed (a suggestion's).
    OpenCode,
}

/// What the page does for `act` on item `id` of kind `of`: a finding's own
/// outcomes, a suggestion's through `PanelOutcome::OnSuggestion`.
pub(crate) fn item_outcome(of: ItemOf, id: &str, act: ItemAct) -> PanelOutcome {
    let id = id.to_owned();
    match of {
        ItemOf::Suggestion => PanelOutcome::OnSuggestion(id, act),
        ItemOf::Finding => match act {
            ItemAct::Decide(decision) => PanelOutcome::DecideFinding(id, decision),
            ItemAct::AskRefuse => PanelOutcome::AskDismiss(id),
            ItemAct::Edit => PanelOutcome::EditFinding(id),
            ItemAct::Comment => PanelOutcome::CommentFinding(id),
            ItemAct::DeleteComment => PanelOutcome::DeleteFindingComment(id),
            ItemAct::DeleteOwn => PanelOutcome::DeleteOwnFinding(id),
            ItemAct::Show | ItemAct::OpenCode => PanelOutcome::ShowFinding(id),
        },
    }
}

/// The latest review's items on `stage`'s page of `feature`, in their
/// order: a decision leaves a card where it is (`AiReview::in_order`). AI
/// review's findings stay once sent, while the code changes and while the
/// next review runs; `can_decide` says when they can be acted on.
///
/// Plan review's suggestions aren't cards yet: its page shows them through
/// the sheet.
pub(crate) fn items(feature: &Feature, stage: Stage) -> Vec<Item<'_>> {
    match stage {
        Stage::AiReview => (feature.review.as_ref())
            .map(|review| review.in_order().into_iter().map(Item::Finding).collect())
            .unwrap_or_default(),
        // WAVE1-STUB(B): Plan review's suggestions as cards, once read.
        Stage::PlanReview => (feature.plan_review.as_ref())
            .filter(|r| r.stamp.is_some())
            .map(|r| r.in_order().into_iter().map(Item::Suggestion).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A card of a review stage's page that takes the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewCard<'a> {
    Item(Item<'a>),
    /// A comment the user left in the diff.
    DiffNote(&'a Comment),
}

impl<'a> ReviewCard<'a> {
    pub(crate) fn item(self) -> Option<Item<'a>> {
        match self {
            Self::Item(item) => Some(item),
            Self::DiffNote(_) => None,
        }
    }

    #[allow(dead_code)] // WAVE1-STUB(B): features.rs goes by `item` now
    pub(crate) fn finding(self) -> Option<&'a ReviewFinding> {
        self.item().and_then(Item::finding)
    }

    pub(crate) fn diff_note(self) -> Option<&'a Comment> {
        match self {
            Self::Item(_) => None,
            Self::DiffNote(c) => Some(c),
        }
    }

    /// What finds this card again when the list around it changes.
    pub(crate) fn id(self) -> CardId {
        match self {
            Self::Item(item) => CardId::Item(item.of(), item.id().to_owned()),
            Self::DiffNote(c) => CardId::DiffNote(c.clone()),
        }
    }
}

/// Which card of a review stage's page, whatever its index. The kind goes
/// with the id: `r1-2` and `u1` are in both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CardId {
    Item(ItemOf, String),
    DiffNote(Comment),
}

/// The cards of `stage`'s page of `feature`, as `FeaturePanel::finding`
/// counts them: the user's own items, the comments they left in the diff,
/// then the reviewer's.
pub(crate) fn review_cards<'a>(
    feature: &'a Feature,
    stage: Stage,
    notes: &'a [Comment],
) -> Vec<ReviewCard<'a>> {
    let items = items(feature, stage);
    let own = items.iter().take_while(|i| i.yours()).count();
    let (mine, theirs) = items.split_at(own);
    (mine.iter().map(|i| ReviewCard::Item(*i)))
        .chain(
            shown_notes(feature, stage, notes)
                .iter()
                .map(ReviewCard::DiffNote),
        )
        .chain(theirs.iter().map(|i| ReviewCard::Item(*i)))
        .collect()
}

/// The next card after `from` still to decide, coming round past the last:
/// `from` when no other is.
pub(crate) fn next_open_card(cards: &[ReviewCard], from: usize) -> usize {
    let n = cards.len();
    let open = |i: &usize| (cards[*i].item()).is_some_and(|i| i.standing() == Standing::Open);
    (from + 1..n)
        .chain(0..from.min(n))
        .find(open)
        .unwrap_or(from)
}

/// Why `stage`'s items in `feature` can't be decided now, when they can't.
pub(crate) fn why_not_decide(feature: &Feature, stage: Stage) -> Option<&'static str> {
    if can_decide(feature, stage) {
        return None;
    }
    // WAVE1-STUB(B): Plan review's words of why not.
    if stage == Stage::PlanReview {
        let writing = (feature.plan_review.as_ref()).is_some_and(|r| r.writing);
        return Some(match stage_mark(feature, stage) {
            StageMark::Done => "Plan review is done: reopen it to change its suggestions",
            _ if writing => "The final plan is being written: change the decisions first",
            _ => "The review hasn't finished: decide once it's read",
        });
    }
    Some(match stage_mark(feature, stage) {
        StageMark::Later => "The code changed since this review: the next one checks it",
        StageMark::Current => "A new review hasn't finished: decide once it's read",
        StageMark::Done => "AI review is done: reopen it to change its findings",
    })
}

/// Whether the items on `stage`'s page of `feature` can be decided, and a
/// comment of the user's added: AI review, current, its latest run read.
pub(crate) fn can_decide(feature: &Feature, stage: Stage) -> bool {
    // WAVE1-STUB(B): Plan review's suggestions, while read and not written.
    if stage == Stage::PlanReview {
        return feature.status == Status::Open
            && feature.current() == Some(stage)
            && crate::feature_plan_review::deciding(feature).is_some();
    }
    stage == Stage::AiReview
        && feature.status == Status::Open
        && feature.current() == Some(stage)
        && (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_some())
        && feature.review.is_some()
}

/// Whether `stage`'s page of `feature` can mark every undecided finding to
/// fix at once.
pub(crate) fn offers_fix_all(feature: &Feature, stage: Stage) -> bool {
    // WAVE1-STUB(B): Accept all open, while one open can be accepted.
    if stage == Stage::PlanReview {
        return can_decide(feature, stage)
            && (feature.plan_review.as_ref()).is_some_and(|r| {
                (r.suggestions.iter())
                    .any(|s| s.state == crate::feature::plan_review::State::Open && !s.unanswered())
            });
    }
    can_decide(feature, stage)
        && feature.fix_round.is_none()
        && (feature.review.as_ref()).is_some_and(|r| r.count(FindingState::Open) > 0)
}

/// What Del does on an item of the user's own: it goes, with nothing to tell
/// themselves, unless it went to Build.
pub(crate) fn delete_own_outcome(item: Item) -> PanelOutcome {
    match item.standing() {
        FindingState::Sent if item.of() == ItemOf::Finding => {
            PanelOutcome::FindingRefused(review_import::SENT)
        }
        FindingState::Fixed if item.of() == ItemOf::Finding => {
            PanelOutcome::FindingRefused(review_import::FIXED)
        }
        _ => item_outcome(item.of(), item.id(), ItemAct::DeleteOwn),
    }
}

/// The buttons on `item`'s card: what can be decided of it while its stage
/// is `current`, and showing it where it is when it has a place.
pub(crate) fn item_buttons(item: Item, current: bool) -> Vec<(PageButton, PanelOutcome)> {
    let w = words(item.of());
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let plain = |label: &str| button(label, ButtonStyle::Plain);
    let act = |act| item_outcome(item.of(), item.id(), act);
    let (yours, standing, note) = (item.yours(), item.standing(), item.note());
    let mut out = Vec::new();
    let undecided = matches!(standing, FindingState::Open | FindingState::Fix);
    if current {
        match standing {
            FindingState::Open => {
                out.push((plain(w.want), act(ItemAct::Decide(Decision::Fix))));
            }
            FindingState::Fix if !yours => {
                let back = act(ItemAct::Decide(Decision::Reopen));
                out.push((plain(w.unwant), back));
            }
            FindingState::Dismissed => {
                let back = act(ItemAct::Decide(Decision::Reopen));
                out.push((plain(w.reopen), back));
            }
            _ => {}
        }
        // What the user says of it goes in a comment, on the card; a refused
        // one keeps that open too.
        if !yours && (undecided || standing == FindingState::Dismissed) {
            let label = if note.is_empty() { "Comment" } else { "Edit" };
            out.push((plain(label), act(ItemAct::Comment)));
            if !note.is_empty() {
                out.push((plain("Delete"), act(ItemAct::DeleteComment)));
            }
        }
        if undecided {
            let edit = if yours { w.edit_own } else { w.edit };
            out.push((plain(edit), act(ItemAct::Edit)));
            if !yours {
                out.push((plain(w.refuse), act(ItemAct::AskRefuse)));
            }
        }
        if yours && (undecided || standing == FindingState::Dismissed) {
            out.push((plain("Delete"), delete_own_outcome(item)));
        }
    }
    if item.shows() {
        out.push((button(w.show, ButtonStyle::Quiet), act(ItemAct::Show)));
    }
    out
}

/// The item a card's button acts on.
pub(crate) fn outcome_item(outcome: &PanelOutcome) -> Option<(ItemOf, &str)> {
    match outcome {
        PanelOutcome::DecideFinding(id, _)
        | PanelOutcome::AskDismiss(id)
        | PanelOutcome::EditFinding(id)
        | PanelOutcome::CommentFinding(id)
        | PanelOutcome::DeleteFindingComment(id)
        | PanelOutcome::DeleteOwnFinding(id)
        | PanelOutcome::ShowFinding(id) => Some((ItemOf::Finding, id)),
        PanelOutcome::OnSuggestion(id, _) => Some((ItemOf::Suggestion, id)),
        _ => None,
    }
}

/// Whether a card's button acts on the user's comment on its item: it sits
/// on the comment's own card then.
fn on_comment(outcome: &PanelOutcome) -> bool {
    matches!(
        outcome,
        PanelOutcome::CommentFinding(_)
            | PanelOutcome::DeleteFindingComment(_)
            | PanelOutcome::OnSuggestion(_, ItemAct::Comment | ItemAct::DeleteComment)
    )
}

/// `item`'s card on its stage's page, its buttons those in `actions`.
pub(crate) fn item_card(
    item: Item,
    multi: bool,
    actions: &[(PageButton, PanelOutcome)],
) -> FindingCard {
    let tag = match (item.yours(), item.severity()) {
        (true, _) => FindingTag::Yours,
        (_, Severity::High) => FindingTag::High,
        (_, Severity::Medium) => FindingTag::Medium,
        (_, Severity::Low) => FindingTag::Low,
    };
    let mut card = match item {
        Item::Finding(f) => finding_card(f, multi),
        Item::Suggestion(s) => suggestion_card(s),
    };
    card.tag = tag;
    // With a comment, the buttons on it sit on its card.
    let has_note = !item.note().is_empty();
    let held = |comment: bool| -> Vec<usize> {
        (actions.iter().enumerate())
            .filter(|(_, (_, o))| outcome_item(o) == Some((item.of(), item.id())))
            .filter(|(_, (_, o))| (has_note && on_comment(o)) == comment)
            .map(|(i, _)| i)
            .collect()
    };
    card.comment_buttons = held(true);
    card.buttons = held(false);
    card
}

/// The card of finding `f`, its tag and buttons left to `item_card`.
fn finding_card(f: &ReviewFinding, multi: bool) -> FindingCard {
    let state = match f.state {
        FindingState::Open => String::new(),
        FindingState::Fix => "to fix".to_owned(),
        FindingState::Sent if f.made => "fix made".to_owned(),
        FindingState::Sent => "sent to Build".to_owned(),
        FindingState::Fixed => "fixed".to_owned(),
        FindingState::Dismissed => format!("dismissed: {}", f.reason),
    };
    let mut place = vec![f.id.clone()];
    if !f.file.is_empty() {
        place.push(review_import::location(f, multi));
        if !f.placed {
            place.push("not in the diff".to_owned());
        }
    }
    let fix = match &f.my_fix {
        Some(fix) => format!("Your fix: {fix}"),
        None if f.fix.is_empty() => String::new(),
        None => format!("Fix: {}", f.fix),
    };
    // Its card shows this much while closed: the comment is behind it.
    let state = match f.state == FindingState::Dismissed && !f.note.is_empty() {
        true => format!("{state} \u{b7} commented"),
        false => state,
    };
    FindingCard {
        tag: FindingTag::Medium,
        title: f.title.clone(),
        state,
        place: place.join(" \u{b7} "),
        body: f.body.clone(),
        preview: Vec::new(),
        fix,
        note: String::new(),
        comment: f.note.clone(),
        comment_buttons: Vec::new(),
        made: f.fix_made(),
        faded: matches!(f.state, FindingState::Fixed | FindingState::Dismissed),
        collapsed: matches!(f.state, FindingState::Fixed | FindingState::Dismissed),
        stripe: match f.state {
            FindingState::Fix => Some(CardStripe::ToFix),
            FindingState::Sent => Some(CardStripe::Sent),
            _ => None,
        },
        field: None,
        field_hint: "Your comment",
        selected: false,
        sheet: None,
        buttons: Vec::new(),
    }
}

/// The card of suggestion `s`, its tag and buttons left to `item_card`. A
/// first cut: Plan review's page doesn't show these yet.
fn suggestion_card(s: &Suggestion) -> FindingCard {
    let w = words(ItemOf::Suggestion);
    let state = match s.state {
        crate::feature::plan_review::State::Open => String::new(),
        crate::feature::plan_review::State::Accepted => w.wanted.to_owned(),
        crate::feature::plan_review::State::Rejected => format!("{}: {}", w.refused, s.reason),
    };
    let mut place = vec![s.id.clone(), s.kind.label().to_owned()];
    if !s.plan.is_empty() {
        place.push(s.plan.clone());
    }
    let change = match &s.my_change {
        Some(change) => format!("{}: {change}", w.my_change),
        None if s.change.is_empty() => String::new(),
        None => format!("{}: {}", w.change, s.change),
    };
    let rejected = s.state == crate::feature::plan_review::State::Rejected;
    let question = s.kind == crate::feature::plan_review::Kind::Question;
    FindingCard {
        tag: FindingTag::Medium,
        title: s.title.clone(),
        state,
        place: place.join(" \u{b7} "),
        body: s.why.clone(),
        preview: Vec::new(),
        fix: change,
        note: String::new(),
        comment: s.note.clone(),
        comment_buttons: Vec::new(),
        made: false,
        faded: rejected,
        collapsed: rejected,
        stripe: (s.state == crate::feature::plan_review::State::Accepted)
            .then_some(CardStripe::ToFix),
        field: None,
        field_hint: if question { w.answer } else { w.comment },
        selected: false,
        sheet: None,
        buttons: Vec::new(),
    }
}

/// How the items on `stage`'s page of `feature` stand, in a few words: "4 to
/// decide · 1 to fix".
pub(crate) fn items_count(feature: &Feature, stage: Stage) -> String {
    // WAVE1-STUB(B): how Plan review's suggestions stand.
    if stage == Stage::PlanReview {
        use crate::feature::plan_review::State;
        let Some(review) = feature.plan_review.as_ref() else {
            return String::new();
        };
        let said = |state, what: &str| match review.count(state) {
            0 => None,
            n => Some(format!("{n} {what}")),
        };
        return [
            said(State::Open, "to decide"),
            said(State::Accepted, "accepted"),
            said(State::Rejected, "rejected"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    }
    if stage != Stage::AiReview {
        return String::new();
    }
    let Some(review) = feature.review.as_ref() else {
        return String::new();
    };
    let shown = review.shown();
    let count = |of: &dyn Fn(&ReviewFinding) -> bool| shown.iter().filter(|f| of(f)).count();
    let state = |state| count(&|f| f.state == state);
    let made = count(&|f| f.fix_made());
    let said = |n: usize, what: &str| (n > 0).then(|| format!("{n} {what}"));
    [
        said(state(FindingState::Open), "to decide"),
        said(state(FindingState::Fix), "to fix"),
        said(state(FindingState::Sent) - made, "sent to Build"),
        (made > 0).then(|| review_import::fixes_made_words(made)),
        said(state(FindingState::Fixed), "fixed"),
        said(state(FindingState::Dismissed), "dismissed"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// The comment in the diff a card's button acts on.
pub(crate) fn outcome_note(outcome: &PanelOutcome) -> Option<&Comment> {
    match outcome {
        PanelOutcome::EditDiffNote(c)
        | PanelOutcome::DeleteDiffNote(c)
        | PanelOutcome::ShowDiffNote(c) => Some(c),
        _ => None,
    }
}

/// The buttons on the cards of `notes`, comments left in the diff: changing
/// one as AI review's findings can be decided, showing one that has a place.
pub(crate) fn note_actions(
    feature: &Feature,
    stage: Stage,
    notes: &[Comment],
) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let mut out = Vec::new();
    for c in shown_notes(feature, stage, notes) {
        if can_edit_note(feature, stage, c) {
            out.push((
                button("Edit\u{2026}", ButtonStyle::Plain),
                PanelOutcome::EditDiffNote(c.clone()),
            ));
            out.push((
                button("Delete", ButtonStyle::Plain),
                PanelOutcome::DeleteDiffNote(c.clone()),
            ));
        }
        if !crate::review::is_general(c) {
            out.push((
                button("Show in diff", ButtonStyle::Quiet),
                PanelOutcome::ShowDiffNote(c.clone()),
            ));
        }
    }
    out
}

/// A key on an item's card, as `FeaturePanel::key` decodes it; which keys
/// reach here, and when, is the panel's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardKey {
    /// `f`: want it, or take that back.
    F,
    /// Del: refuse it asking why, reopen it, or delete one of the user's
    /// own (armed first).
    Del,
    /// Shift+Del: delete the user's comment on it.
    ShiftDel,
    /// `c`: comment on it.
    C,
    /// Ctrl+E: reword it.
    CtrlE,
    /// Enter: show it, or reword one with nowhere to show.
    Enter,
    /// Ctrl+O: open its citation.
    #[allow(dead_code)] // decoded once Plan review has cards (wave 1 contract)
    CtrlO,
}

/// What `key` does on the card at `at` of `cards`, an item's; `refused`
/// says why its stage can't decide now. `at` moves on to the next card left
/// to decide once one is; `armed` is the key the last press armed, and a
/// press that arms one sets `arm`. None when there is nothing to do (also
/// when the card isn't an item's).
pub(crate) fn card_key(
    key: CardKey,
    cards: &[ReviewCard],
    at: &mut usize,
    refused: Option<&'static str>,
    armed: Option<&str>,
    arm: &mut Option<String>,
) -> Option<PanelOutcome> {
    let item = cards.get(*at).copied().and_then(ReviewCard::item)?;
    let (of, id) = (item.of(), item.id());
    let act = |act| item_outcome(of, id, act);
    match key {
        CardKey::F => {
            if let Some(why) = refused {
                return Some(PanelOutcome::FindingRefused(why));
            }
            // Decided, the keyboard goes on to what is left to decide.
            if item.standing() == Standing::Open {
                *at = next_open_card(cards, *at);
            }
            Some(match review_items::want_key(of, item.standing()) {
                Ok(decision) => act(ItemAct::Decide(decision)),
                Err(why) => PanelOutcome::FindingRefused(why),
            })
        }
        CardKey::CtrlE => Some(match refused {
            Some(why) => PanelOutcome::FindingRefused(why),
            None => act(ItemAct::Edit),
        }),
        CardKey::C => Some(match refused {
            Some(why) => PanelOutcome::FindingRefused(why),
            None => act(ItemAct::Comment),
        }),
        CardKey::ShiftDel => {
            if item.note().is_empty() {
                return None;
            }
            Some(match refused {
                Some(why) => PanelOutcome::FindingRefused(why),
                None => act(ItemAct::DeleteComment),
            })
        }
        CardKey::Del => {
            if let Some(why) = refused {
                return Some(PanelOutcome::FindingRefused(why));
            }
            if item.yours() {
                let outcome = delete_own_outcome(item);
                let key = match of {
                    ItemOf::Finding => format!("own:{id}"),
                    ItemOf::Suggestion => format!("own-suggestion:{id}"),
                };
                // As a comment left in the diff: the first press arms it.
                let deletes = matches!(
                    outcome,
                    PanelOutcome::DeleteOwnFinding(_)
                        | PanelOutcome::OnSuggestion(_, ItemAct::DeleteOwn)
                );
                if deletes && armed != Some(key.as_str()) {
                    *arm = Some(key);
                    return None;
                }
                return Some(outcome);
            }
            Some(match review_items::refuse_key(of, item.standing()) {
                Ok(None) => act(ItemAct::AskRefuse),
                Ok(Some(decision)) => act(ItemAct::Decide(decision)),
                Err(why) => PanelOutcome::FindingRefused(why),
            })
        }
        CardKey::Enter => Some(match (item.shows(), refused) {
            (false, Some(why)) => PanelOutcome::FindingRefused(why),
            (false, None) => act(ItemAct::Edit),
            (true, _) => act(ItemAct::Show),
        }),
        CardKey::CtrlO => match item {
            Item::Suggestion(s) if !s.evidence.is_empty() => Some(act(ItemAct::OpenCode)),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::plan_review::{Kind, State};

    fn finding(id: &str, state: FindingState, yours: bool) -> ReviewFinding {
        ReviewFinding {
            id: id.into(),
            title: "Off by one".into(),
            file: "src/a.rs".into(),
            state,
            yours,
            ..ReviewFinding::default()
        }
    }

    fn suggestion(id: &str, state: State, source: Source) -> Suggestion {
        Suggestion {
            id: id.into(),
            title: "No rollback".into(),
            kind: Kind::Gap,
            state,
            source,
            reason: "Not needed".into(),
            ..Suggestion::default()
        }
    }

    fn labels(item: Item) -> Vec<String> {
        (item_buttons(item, true).into_iter())
            .map(|(b, _)| b.label)
            .collect()
    }

    #[test]
    fn each_kind_has_its_own_buttons() {
        let f = finding("r1-1", FindingState::Open, false);
        assert_eq!(
            labels(Item::Finding(&f)),
            [
                "Fix",
                "Comment",
                "Edit fix\u{2026}",
                "Dismiss\u{2026}",
                "Show in diff"
            ]
        );
        let s = suggestion("r1-1", State::Open, Source::Reviewer);
        assert_eq!(
            labels(Item::Suggestion(&s)),
            [
                "Accept",
                "Comment",
                "Edit change\u{2026}",
                "Reject\u{2026}",
                "Show in plan"
            ]
        );
        let s = suggestion("r1-1", State::Accepted, Source::Reviewer);
        assert_eq!(labels(Item::Suggestion(&s))[0], "Don't accept");
        let s = suggestion("r1-1", State::Rejected, Source::Reviewer);
        assert_eq!(labels(Item::Suggestion(&s))[0], "Reopen");
        let own = suggestion("u1", State::Accepted, Source::You);
        assert_eq!(
            labels(Item::Suggestion(&own)),
            ["Edit\u{2026}", "Delete", "Show in plan"]
        );
    }

    #[test]
    fn a_suggestions_buttons_say_what_it_is() {
        let s = suggestion("r1-1", State::Open, Source::Reviewer);
        let outcomes: Vec<PanelOutcome> = (item_buttons(Item::Suggestion(&s), true).into_iter())
            .map(|(_, o)| o)
            .collect();
        assert_eq!(
            outcomes[0],
            PanelOutcome::OnSuggestion("r1-1".into(), ItemAct::Decide(Decision::Fix))
        );
        assert!((outcomes.iter()).all(|o| outcome_item(o) == Some((ItemOf::Suggestion, "r1-1"))));
        let f = finding("r1-1", FindingState::Open, false);
        let first = item_buttons(Item::Finding(&f), true).remove(0).1;
        assert_eq!(outcome_item(&first), Some((ItemOf::Finding, "r1-1")));
    }

    #[test]
    fn a_rejected_suggestion_card_says_why_and_folds() {
        let s = suggestion("r1-1", State::Rejected, Source::Reviewer);
        let card = item_card(Item::Suggestion(&s), false, &[]);
        assert_eq!(card.state, "rejected: Not needed");
        assert!(card.collapsed && card.faded);
        assert_eq!(card.place, "r1-1 \u{b7} gap");
        let mut q = suggestion("r1-2", State::Open, Source::Reviewer);
        q.kind = Kind::Question;
        let card = item_card(Item::Suggestion(&q), false, &[]);
        assert_eq!(card.field_hint, "Your answer");
    }

    #[test]
    fn the_card_keys_act_on_either_kind_and_del_arms_your_own() {
        let theirs = suggestion("r1-1", State::Open, Source::Reviewer);
        let own = suggestion("u1", State::Accepted, Source::You);
        let f = finding("u1", FindingState::Fix, true);
        let cards = [
            ReviewCard::Item(Item::Suggestion(&own)),
            ReviewCard::Item(Item::Finding(&f)),
            ReviewCard::Item(Item::Suggestion(&theirs)),
        ];
        let mut arm = None;
        let mut at = 2;
        // `f` decides, and the keyboard goes on to the next one open.
        let got = card_key(CardKey::F, &cards, &mut at, None, None, &mut arm);
        assert_eq!(
            got,
            Some(PanelOutcome::OnSuggestion(
                "r1-1".into(),
                ItemAct::Decide(Decision::Fix)
            ))
        );
        let got = card_key(CardKey::Del, &cards, &mut at, None, None, &mut arm);
        assert_eq!(
            got,
            Some(PanelOutcome::OnSuggestion(
                "r1-1".into(),
                ItemAct::AskRefuse
            ))
        );
        // Refused: why, whatever the key.
        let got = card_key(CardKey::F, &cards, &mut at, Some("No"), None, &mut arm);
        assert_eq!(got, Some(PanelOutcome::FindingRefused("No")));
        // Del on one of the user's own arms first, by kind and id.
        let mut at = 0;
        assert_eq!(
            card_key(CardKey::Del, &cards, &mut at, None, None, &mut arm),
            None
        );
        assert_eq!(arm.as_deref(), Some("own-suggestion:u1"));
        let mut at = 1;
        let armed = arm.take();
        // The finding `u1` isn't armed by the suggestion `u1`.
        assert_eq!(
            card_key(
                CardKey::Del,
                &cards,
                &mut at,
                None,
                armed.as_deref(),
                &mut arm
            ),
            None
        );
        assert_eq!(arm.as_deref(), Some("own:u1"));
        let mut at = 0;
        let got = card_key(
            CardKey::Del,
            &cards,
            &mut at,
            None,
            armed.as_deref(),
            &mut arm,
        );
        assert_eq!(
            got,
            Some(PanelOutcome::OnSuggestion("u1".into(), ItemAct::DeleteOwn))
        );
        // Enter shows a suggestion in the plan.
        let got = card_key(CardKey::Enter, &cards, &mut at, None, None, &mut arm);
        assert_eq!(
            got,
            Some(PanelOutcome::OnSuggestion("u1".into(), ItemAct::Show))
        );
        // Ctrl+O has nothing to open with no citation.
        assert_eq!(
            card_key(CardKey::CtrlO, &cards, &mut at, None, None, &mut arm),
            None
        );
    }

    #[test]
    fn card_ids_keep_the_kinds_apart() {
        let s = suggestion("u1", State::Accepted, Source::You);
        let f = finding("u1", FindingState::Fix, true);
        let (a, b) = (
            ReviewCard::Item(Item::Suggestion(&s)).id(),
            ReviewCard::Item(Item::Finding(&f)).id(),
        );
        assert_ne!(a, b);
        assert_eq!(a, CardId::Item(ItemOf::Suggestion, "u1".into()));
    }
}
