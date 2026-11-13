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
        plan_review::{Kind, Placed, Source, State, Suggestion},
        review_import::{self, Decision, Finding as ReviewFinding, FindingState, Severity},
        review_items::{self, ItemOf, Standing, Words, words, words_in},
    },
    features::{PanelOutcome, can_edit_note, shown_notes, stage_mark},
    key_table::{self, Binding},
    review::Comment,
};
use smithay::input::keyboard::{Keysym, ModifiersState};

/// One item a review stage's page shows as a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item<'a> {
    Finding(&'a ReviewFinding),
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
    /// Write the answer to a question not answered yet, on its card, and
    /// accept it once saved: what Accept does to one (`f`, its button).
    Answer,
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
            ItemAct::Comment | ItemAct::Answer => PanelOutcome::CommentFinding(id),
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
/// Plan review's are every suggestion, rejected and left out ones too
/// (`PlanReview::in_order`).
pub(crate) fn items(feature: &Feature, stage: Stage) -> Vec<Item<'_>> {
    match stage {
        Stage::AiReview => (feature.review.as_ref())
            .map(|review| review.in_order().into_iter().map(Item::Finding).collect())
            .unwrap_or_default(),
        Stage::PlanReview => (feature.plan_review.as_ref())
            .map(|review| {
                (review.in_order().into_iter())
                    .map(Item::Suggestion)
                    .collect()
            })
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
    if stage == Stage::PlanReview {
        let writing = (feature.plan_review.as_ref()).is_some_and(|r| r.writing);
        return Some(match stage_mark(feature, stage) {
            StageMark::Done => SUGGESTIONS_DONE,
            StageMark::Later => "The plan changed since this review: the next one reads it",
            StageMark::Current if writing => SUGGESTIONS_WRITING,
            StageMark::Current => "The reviewer hasn't finished: decide once it's read",
        });
    }
    Some(match stage_mark(feature, stage) {
        StageMark::Later => "The code changed since this review: the next one checks it",
        StageMark::Current => "A new review hasn't finished: decide once it's read",
        StageMark::Done => "AI review is done: reopen it to change its findings",
    })
}

/// Why Plan review's suggestions can't be decided once the stage is done.
pub(crate) const SUGGESTIONS_DONE: &str = "Plan review is done: only to read";

/// Why they can't while the final plan is written from them.
pub(crate) const SUGGESTIONS_WRITING: &str =
    "The final plan is being written from these: Change decisions\u{2026} to change them";

/// Why `f` does nothing on a suggestion of the user's own: it has no
/// undecided to go back to.
pub(crate) const OWN_ACCEPTED: &str = "Your own suggestion stays accepted: delete it instead";

/// Whether the items on `stage`'s page of `feature` can be decided, and a
/// comment of the user's added: AI review, current, its latest run read;
/// Plan review, current, a review read in and the final plan not being
/// written from it (`feature_plan_review::deciding`). A reviewer at work on
/// its next run doesn't stop it: decisions go across by id.
pub(crate) fn can_decide(feature: &Feature, stage: Stage) -> bool {
    let current = feature.status == Status::Open && feature.current() == Some(stage);
    match stage {
        Stage::AiReview => {
            current
                && (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_some())
                && feature.review.is_some()
        }
        Stage::PlanReview => current && crate::feature_plan_review::deciding(feature).is_some(),
        _ => false,
    }
}

/// Whether `stage`'s page of `feature` can want every undecided item at
/// once: mark each finding to fix, accept each suggestion (a question with
/// no answer yet stays).
pub(crate) fn offers_fix_all(feature: &Feature, stage: Stage) -> bool {
    if !can_decide(feature, stage) {
        return false;
    }
    match stage {
        Stage::PlanReview => (feature.plan_review.as_ref()).is_some_and(|r| {
            (r.suggestions.iter()).any(|s| s.state == State::Open && !s.unanswered())
        }),
        _ => {
            feature.fix_round.is_none()
                && (feature.review.as_ref()).is_some_and(|r| r.count(FindingState::Open) > 0)
        }
    }
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

/// The page an item's card is on, as its buttons and hints need it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CardPage {
    /// Its stage can decide items now.
    pub(crate) decides: bool,
    /// Its review posts to GitHub (`Feature::posts_review`): its words.
    pub(crate) posts: bool,
}

/// The buttons on `item`'s card: what can be decided of it while its
/// `page` decides, and showing it where it is when it has a place; in the
/// words of a review that posts to GitHub when it does.
pub(crate) fn item_buttons(item: Item, page: CardPage) -> Vec<(PageButton, PanelOutcome)> {
    let w = words_in(item.of(), page.posts);
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let plain = |label: &str| button(label, ButtonStyle::Plain);
    let act = |act| item_outcome(item.of(), item.id(), act);
    let (yours, standing, note) = (item.yours(), item.standing(), item.note());
    let mut out = Vec::new();
    let undecided = matches!(standing, FindingState::Open | FindingState::Fix);
    // A question is accepted by answering it.
    let unanswered = item.suggestion().is_some_and(Suggestion::unanswered);
    let question = item.suggestion().is_some_and(|s| s.kind == Kind::Question);
    if page.decides {
        match standing {
            FindingState::Open if unanswered => {
                out.push((plain(w.want), act(ItemAct::Answer)));
            }
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
            let label = match (note.is_empty(), question) {
                (true, true) => "Answer",
                (true, false) => "Comment",
                // Its place kept, so its neighbours stay put.
                (false, true) => "Edit answer",
                (false, false) => "Edit comment",
            };
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
    // Reading the code it cites changes nothing: there whatever the stage.
    if item
        .suggestion()
        .is_some_and(|s| !yours && !s.evidence.is_empty())
    {
        let open = button("Open code", ButtonStyle::Quiet);
        out.push((open, act(ItemAct::OpenCode)));
    }
    if item.shows() {
        out.push((button(w.show, ButtonStyle::Quiet), act(ItemAct::Show)));
    }
    out
}

/// An item's card with the keyboard, as its keys read it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CardCtx {
    of: ItemOf,
    /// Its page decides now, and its review posts to GitHub.
    page: CardPage,
    /// A first Del armed it.
    armed: bool,
    yours: bool,
    standing: Standing,
    /// A question with no answer yet.
    unanswered: bool,
    question: bool,
    /// The user commented on it, or answered it.
    noted: bool,
    /// It has a place to show it at.
    shows: bool,
    /// A suggestion citing code.
    cites: bool,
}

impl CardCtx {
    pub(crate) fn of(item: Item, page: CardPage, armed: bool) -> Self {
        let suggestion = item.suggestion();
        Self {
            of: item.of(),
            page,
            armed,
            yours: item.yours(),
            standing: item.standing(),
            unanswered: suggestion.is_some_and(Suggestion::unanswered),
            question: suggestion.is_some_and(|s| s.kind == Kind::Question),
            noted: !item.note().is_empty(),
            shows: item.shows(),
            cites: suggestion.is_some_and(|s| !s.evidence.is_empty()),
        }
    }

    /// It is open, or wanted: still to decide.
    fn undecided(&self) -> bool {
        matches!(self.standing, Standing::Open | Standing::Fix)
    }

    /// The footer offers `c` on it: a reviewer's, undecided or refused,
    /// while its page decides.
    fn comments(&self) -> bool {
        self.page.decides
            && !self.yours
            && (self.undecided() || self.standing == Standing::Dismissed)
    }

    /// What `key` does on it, in the words of its kind, as the footer says.
    fn says(&self, key: CardKey) -> String {
        let w = words_in(self.of, self.page.posts);
        let lower = |text: &str| text.to_lowercase();
        match key {
            CardKey::F if self.standing == Standing::Fix => lower(w.unwant),
            // It opens the answer first, `c`'s: accepted once answered.
            CardKey::F if self.unanswered => format!("{}\u{2026}", lower(w.want)),
            CardKey::F => lower(w.want),
            CardKey::CtrlE => lower(if self.yours { w.edit_own } else { w.edit }),
            CardKey::C => match (self.noted, self.question) {
                (false, true) => "answer",
                (false, false) => "comment",
                (true, true) => "edit answer",
                (true, false) => "edit comment",
            }
            .into(),
            CardKey::ShiftDel if self.question => "delete answer".into(),
            CardKey::ShiftDel => "delete comment".into(),
            CardKey::Del if self.yours && self.armed => "again to delete".into(),
            CardKey::Del if self.yours => "delete".into(),
            CardKey::Del if self.standing == Standing::Dismissed => lower(w.reopen),
            CardKey::Del => lower(w.refuse),
            CardKey::Enter if self.shows => lower(w.show),
            CardKey::Enter => "edit\u{2026}".into(),
            CardKey::CtrlO => "open code".into(),
        }
    }
}

/// The keys of an item's card, either kind; `card_key` does them. Each
/// key the footer hints only while its page decides (`CardCtx::comments`…)
/// still answers otherwise, from its row after, saying why it can't.
pub(crate) const CARD_KEYS: &[Binding<CardCtx, CardKey>] = &[
    Binding {
        chord: "f",
        does: |c| {
            let offered = match c.standing {
                // One of the user's own suggestions has nothing to take back.
                Standing::Fix => !(c.yours && c.of == ItemOf::Suggestion),
                Standing::Open | Standing::Dismissed => true,
                _ => false,
            };
            (c.page.decides && offered).then_some(CardKey::F)
        },
        footer: true,
        f1: "want it (fix, accept), or take that back",
    },
    Binding {
        chord: "f",
        does: |_| Some(CardKey::F),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+E",
        does: |c| (c.page.decides && c.undecided()).then_some(CardKey::CtrlE),
        footer: true,
        f1: "edit its fix or change, or one of your own",
    },
    Binding {
        chord: "Ctrl+E",
        does: |_| Some(CardKey::CtrlE),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "c",
        does: |c| c.comments().then_some(CardKey::C),
        footer: true,
        f1: "write or edit your comment on it, a question's answer",
    },
    // Off a reviewer's card `c` adds one of the user's own: the page's.
    Binding {
        chord: "c",
        does: |c| (!c.yours).then_some(CardKey::C),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Shift+Del",
        does: |c| (c.comments() && c.noted).then_some(CardKey::ShiftDel),
        footer: true,
        f1: "delete your comment on it",
    },
    Binding {
        chord: "Shift+Del",
        does: |_| Some(CardKey::ShiftDel),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Del",
        does: |c| {
            let standing = matches!(
                c.standing,
                Standing::Open | Standing::Fix | Standing::Dismissed
            );
            (c.page.decides && standing).then_some(CardKey::Del)
        },
        footer: true,
        f1: "refuse it, saying why, or reopen it; one of your own: press twice to delete",
    },
    Binding {
        chord: "Del",
        does: |_| Some(CardKey::Del),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Enter",
        does: |c| (c.shows || c.page.decides).then_some(CardKey::Enter),
        footer: true,
        f1: "show it in the diff or the plan; with nowhere to show it, edit it",
    },
    Binding {
        chord: "Enter",
        does: |_| Some(CardKey::Enter),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+O",
        does: |c| c.cites.then_some(CardKey::CtrlO),
        footer: true,
        f1: "open the code it cites",
    },
];

/// The key pressed on the card `ctx` has, as `card_key` takes it; none
/// for a key not the card's.
pub(crate) fn card_key_of(ctx: &CardCtx, mods: &ModifiersState, sym: Keysym) -> Option<CardKey> {
    key_table::press(CARD_KEYS, ctx, mods, sym)
}

/// The footer's hints for `item`'s card with the keyboard, key and what it
/// does, in the words of its kind (`card_key` says what each does);
/// deciding while its `page` decides, `armed` while a first Del armed
/// it. The page's own keys (Up / Down, `c` off the card, wanting all) are
/// the page's. A review that posts to GitHub has its words.
pub(crate) fn item_hints(item: Item, page: CardPage, armed: bool) -> Vec<(String, String)> {
    let ctx = CardCtx::of(item, page, armed);
    (key_table::hints(CARD_KEYS, &ctx, |k| ctx.says(*k)).into_iter())
        .map(|(k, what)| (k.to_owned(), what))
        .collect()
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

/// Whether a card's button deletes the user's comment on its item: it sits
/// on the comment's own card then. Edit comment keeps Comment's place on
/// the item's card, so the buttons there never shift.
fn on_comment(outcome: &PanelOutcome) -> bool {
    matches!(
        outcome,
        PanelOutcome::DeleteFindingComment(_)
            | PanelOutcome::OnSuggestion(_, ItemAct::DeleteComment)
    )
}

/// What `item`'s card says it is: the user's own, else its severity.
pub(crate) fn item_tag(item: Item) -> FindingTag {
    match (item.yours(), item.severity()) {
        (true, _) => FindingTag::Yours,
        (_, Severity::High) => FindingTag::High,
        (_, Severity::Medium) => FindingTag::Medium,
        (_, Severity::Low) => FindingTag::Low,
    }
}

/// What `card` says it is: a comment left in the diff is the user's own.
pub(crate) fn card_tag(card: ReviewCard) -> FindingTag {
    card.item().map_or(FindingTag::Yours, item_tag)
}

/// `item`'s card on its stage's page, its buttons those in `actions`.
pub(crate) fn item_card(
    item: Item,
    multi: bool,
    actions: &[(PageButton, PanelOutcome)],
    posts: bool,
) -> FindingCard {
    let tag = item_tag(item);
    let mut card = match item {
        Item::Finding(f) => finding_card(f, multi, words_in(ItemOf::Finding, posts)),
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

/// The card of finding `f`, its tag and buttons left to `item_card`, in
/// the words `w` of its review: of one that posts to GitHub, one wanted is
/// to comment.
fn finding_card(f: &ReviewFinding, multi: bool, w: &Words) -> FindingCard {
    // The plan's phase is the feature's: `page_card` says "in fix plan".
    let state = match f.state {
        FindingState::Open => "to decide".to_owned(),
        FindingState::Fix => w.wanted.to_owned(),
        FindingState::Sent => review_import::sent_word(f.made, false).to_owned(),
        FindingState::Fixed => "fixed".to_owned(),
        FindingState::Dismissed => format!("dismissed: {}", f.reason),
        FindingState::Posted => "on GitHub".to_owned(),
    };
    let mut place = vec![f.id.clone()];
    if !f.file.is_empty() {
        place.push(review_import::location(f, multi));
        if !f.placed {
            place.push("not in the diff".to_owned());
        }
    }
    let fix = changed_by_you("Your fix", f.my_fix.as_deref(), "Fix", &f.fix);
    // Its card shows this much while closed: the comment is behind it.
    let state = match f.state != FindingState::Open && !f.note.is_empty() {
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
        // Decided, it folds to its line: what is left to decide stands out.
        collapsed: f.state != FindingState::Open,
        folds: false,
        stripe: match f.state {
            FindingState::Fix => Some(CardStripe::ToFix),
            FindingState::Sent => Some(CardStripe::Sent),
            FindingState::Dismissed => Some(CardStripe::Dismissed),
            _ => None,
        },
        field: None,
        field_hint: "Your comment",
        selected: false,
        sheet: None,
        buttons: Vec::new(),
    }
}

/// The card of suggestion `s`, its tag and buttons left to `item_card`.
fn suggestion_card(s: &Suggestion) -> FindingCard {
    let w = words(ItemOf::Suggestion);
    let question = s.kind == Kind::Question;
    let state = match s.state {
        State::Open if s.unanswered() => "not answered yet".to_owned(),
        State::Open if question => "answered".to_owned(),
        State::Open => "to decide".to_owned(),
        // A question goes into the final plan as answered.
        State::Accepted if question && !s.note.is_empty() => "answered".to_owned(),
        State::Accepted => w.wanted.to_owned(),
        State::Rejected => format!("{}: {}", w.refused, s.reason),
    };
    // Its card shows this much while closed: the comment is behind it.
    let rejected = s.state == State::Rejected;
    let decided = s.state != State::Open;
    let state = match (
        decided && !s.note.is_empty() && state != "answered",
        question,
    ) {
        (true, true) => format!("{state} \u{b7} answered"),
        (true, false) => format!("{state} \u{b7} commented"),
        (false, _) => state,
    };
    let mut place = vec![s.id.clone(), s.kind.label().to_owned()];
    if !s.plan.is_empty() {
        place.push(s.plan.clone());
    }
    match s.placed {
        Some(Placed::Nowhere) => place.push(w.not_placed.to_owned()),
        Some(Placed::Heading(_)) if !s.plan_quote.is_empty() => {
            place.push("plan quote not found".to_owned());
        }
        _ => {}
    }
    if s.dropped {
        place.push("left out by a later run".to_owned());
    }
    if s.yours_earlier {
        place.push("what you wrote was on an earlier text".to_owned());
    }
    let mut body: Vec<String> = (!s.why.is_empty())
        .then(|| s.why.clone())
        .into_iter()
        .collect();
    body.extend(s.evidence.iter().map(cited));
    let change = changed_by_you(w.my_change, s.my_change.as_deref(), w.change, &s.change);
    FindingCard {
        tag: FindingTag::Medium,
        title: s.title.clone(),
        state,
        place: place.join(" \u{b7} "),
        body: body.join("\n"),
        preview: Vec::new(),
        fix: change,
        note: String::new(),
        comment: s.note.clone(),
        comment_buttons: Vec::new(),
        made: false,
        faded: rejected,
        // Decided, it folds to its line: what is left to decide stands out.
        collapsed: decided,
        folds: false,
        stripe: match s.state {
            State::Accepted => Some(CardStripe::ToFix),
            State::Rejected => Some(CardStripe::Dismissed),
            State::Open => None,
        },
        field: None,
        field_hint: if question { w.answer } else { w.comment },
        selected: false,
        sheet: None,
        buttons: Vec::new(),
    }
}

/// The change a card asks for, labelled: the user's wording `mine` over
/// what the reviewer said, which stays to read ("Your fix: …", then "The
/// reviewer's: …"), else the reviewer's alone.
fn changed_by_you(mine_label: &str, mine: Option<&str>, label: &str, theirs: &str) -> String {
    match mine {
        Some(mine) if theirs.trim().is_empty() || theirs.trim() == mine.trim() => {
            format!("{mine_label}: {mine}")
        }
        Some(mine) => format!("{mine_label}: {mine}\nThe reviewer's: {theirs}"),
        None if theirs.is_empty() => String::new(),
        None => format!("{label}: {theirs}"),
    }
}

/// `item`'s card on `stage`'s page of `feature`, as `item_card` makes it,
/// with what only the feature knows: a finding sent while its fix plan is
/// still made says "in fix plan", and an item the latest run raised (past
/// the first) says so, "new in run 2", "changed in run 2" or "raised again
/// in run 2", so a card that changed under a decision never does silently.
pub(crate) fn page_card(
    feature: &Feature,
    item: Item,
    actions: &[(PageButton, PanelOutcome)],
) -> FindingCard {
    let mut card = item_card(item, feature.many_repos(), actions, feature.posts_review());
    let planning = crate::feature::fix_round::planning(feature.fix_round.as_ref(), Stage::AiReview);
    if let Item::Finding(f) = item
        && f.state == FindingState::Sent
        && planning
        && !f.made
    {
        card.state = card.state.replacen(
            review_import::sent_word(false, false),
            review_import::sent_word(false, true),
            1,
        );
    }
    // After the id, which finds the card again (`CardNote::Item`).
    if let Some(tag) = run_tag(feature, item) {
        card.place = match card.place.split_once(" \u{b7} ") {
            Some((id, rest)) => format!("{id} \u{b7} {tag} \u{b7} {rest}"),
            None => format!("{} \u{b7} {tag}", card.place),
        };
    }
    card
}

/// What the latest run did to `item`, past the first: "new in run 2",
/// "changed in run 2", "raised again in run 2"; none for one it didn't
/// raise, or the user's own.
pub(crate) fn run_tag(feature: &Feature, item: Item) -> Option<String> {
    let (n, run, changed) = match item {
        Item::Finding(f) => (feature.review.as_ref()?.n, f.run, f.changed),
        Item::Suggestion(s) => (feature.plan_review.as_ref()?.n, s.run, s.changed),
    };
    if n < 2 || run != n || item.yours() {
        return None;
    }
    Some(match (item.id().starts_with(&format!("r{n}-")), changed) {
        (true, _) => format!("new in run {n}"),
        (false, true) => format!("changed in run {n}"),
        (false, false) => format!("raised again in run {n}"),
    })
}

/// A suggestion's citation as its card lists it: "Cited: api/src/a.rs:12",
/// and whether gerdr found it.
fn cited(e: &crate::feature::plan_review::Evidence) -> String {
    let at = match e.repo.is_empty() {
        true => format!("{}:{}", e.file, e.line),
        false => format!("{}/{}:{}", e.repo, e.file, e.line),
    };
    match e.found {
        Some(false) => format!("Cited: {at} \u{b7} not found in the code"),
        _ => format!("Cited: {at}"),
    }
}

/// How the items on `stage`'s page of `feature` stand, in a few words: "4 to
/// decide · 1 to fix".
pub(crate) fn items_count(feature: &Feature, stage: Stage) -> String {
    if stage == Stage::PlanReview {
        return suggestions_count(feature);
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
        said(state(FindingState::Fix), words_in(ItemOf::Finding, review.posts).wanted),
        said(
            state(FindingState::Sent) - made,
            review_import::sent_word(
                false,
                crate::feature::fix_round::planning(feature.fix_round.as_ref(), Stage::AiReview),
            ),
        ),
        (made > 0).then(|| review_import::fixes_made_words(made)),
        said(state(FindingState::Fixed), "fixed"),
        said(state(FindingState::Dismissed), "dismissed"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// How Plan review's suggestions stand: "2 to decide · 1 accepted · 1
/// citation not found in the code".
fn suggestions_count(feature: &Feature) -> String {
    let Some(review) = feature.plan_review.as_ref() else {
        return String::new();
    };
    let said = |n: usize, what: &str| (n > 0).then(|| format!("{n} {what}"));
    let not_found = review.not_found();
    [
        said(review.count(State::Open), "to decide"),
        said(review.count(State::Accepted), "accepted"),
        said(review.count(State::Rejected), "rejected"),
        (not_found > 0).then(|| {
            let cites = crate::features::plural(not_found, "citation");
            format!("{cites} not found in the code")
        }),
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
            if let Item::Suggestion(s) = item {
                // One of the user's own has no undecided to go back to.
                if s.source == Source::You {
                    return Some(PanelOutcome::FindingRefused(OWN_ACCEPTED));
                }
                // A question is accepted by answering it: the keyboard stays,
                // for the answer.
                if s.unanswered() && s.state != State::Accepted {
                    return Some(act(ItemAct::Answer));
                }
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
        // Off a reviewer's card `c` adds one of the user's own: the page's.
        CardKey::C if item.yours() => None,
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

    /// The page of a review that doesn't post to GitHub, deciding or not.
    fn page(decides: bool) -> CardPage {
        CardPage {
            decides,
            posts: false,
        }
    }

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
        (item_buttons(item, page(true)).into_iter())
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
        assert_eq!(labels(Item::Suggestion(&s))[0], "Reopen");
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
        let outcomes: Vec<PanelOutcome> = (item_buttons(Item::Suggestion(&s), page(true))
            .into_iter())
        .map(|(_, o)| o)
        .collect();
        assert_eq!(
            outcomes[0],
            PanelOutcome::OnSuggestion("r1-1".into(), ItemAct::Decide(Decision::Fix))
        );
        assert!((outcomes.iter()).all(|o| outcome_item(o) == Some((ItemOf::Suggestion, "r1-1"))));
        let f = finding("r1-1", FindingState::Open, false);
        let first = item_buttons(Item::Finding(&f), page(true)).remove(0).1;
        assert_eq!(outcome_item(&first), Some((ItemOf::Finding, "r1-1")));
    }

    #[test]
    fn a_rejected_suggestion_card_says_why_and_folds() {
        let s = suggestion("r1-1", State::Rejected, Source::Reviewer);
        let card = item_card(Item::Suggestion(&s), false, &[], false);
        assert_eq!(card.state, "rejected: Not needed");
        assert!(card.collapsed && card.faded);
        assert_eq!(card.place, "r1-1 \u{b7} gap");
        let mut q = suggestion("r1-2", State::Open, Source::Reviewer);
        q.kind = Kind::Question;
        let card = item_card(Item::Suggestion(&q), false, &[], false);
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

    fn at_plan_review(suggestions: Vec<Suggestion>) -> Feature {
        use crate::features::tests::{feature, repo};
        let mut f = feature("p", vec![repo("api", true)], 2);
        f.plan_review = Some(crate::feature::plan_review::PlanReview {
            n: 1,
            stamp: Some((1, 1)),
            suggestions,
            ..Default::default()
        });
        f
    }

    fn question(id: &str, note: &str) -> Suggestion {
        Suggestion {
            kind: Kind::Question,
            note: note.into(),
            ..suggestion(id, State::Open, Source::Reviewer)
        }
    }

    fn cited(found: Option<bool>) -> crate::feature::plan_review::Evidence {
        crate::feature::plan_review::Evidence {
            repo: "api".into(),
            file: "src/a.rs".into(),
            line: 12,
            quote: "fn a()".into(),
            found,
        }
    }

    fn outcomes(item: Item) -> Vec<(String, PanelOutcome)> {
        (item_buttons(item, page(true)).into_iter())
            .map(|(b, o)| (b.label, o))
            .collect()
    }

    #[test]
    fn plan_review_lists_every_suggestion_in_its_order() {
        let mut low = suggestion("r1-1", State::Rejected, Source::Reviewer);
        low.severity = Severity::Low;
        let mut high = suggestion("r1-2", State::Open, Source::Reviewer);
        high.severity = Severity::High;
        high.dropped = true;
        let own = suggestion("u1", State::Accepted, Source::You);
        let f = at_plan_review(vec![low, high, own]);
        let ids: Vec<&str> = (items(&f, Stage::PlanReview).into_iter())
            .map(Item::id)
            .collect();
        assert_eq!(ids, ["u1", "r1-2", "r1-1"]);
        assert!(items(&f, Stage::AiReview).is_empty());
    }

    #[test]
    fn suggestions_are_decided_only_while_plan_review_waits_on_them() {
        let mut f = at_plan_review(vec![suggestion("r1-1", State::Open, Source::Reviewer)]);
        assert!(can_decide(&f, Stage::PlanReview));
        assert_eq!(why_not_decide(&f, Stage::PlanReview), None);
        assert!(!can_decide(&f, Stage::AiReview));
        // A reviewer at work on its next run doesn't stop it.
        f.runs.insert(Stage::PlanReview, Default::default());
        assert!(can_decide(&f, Stage::PlanReview));
        // Not read in yet.
        f.plan_review.as_mut().unwrap().stamp = None;
        assert!(!can_decide(&f, Stage::PlanReview));
        assert!(why_not_decide(&f, Stage::PlanReview).is_some());
        // The final plan being written from them.
        let review = f.plan_review.as_mut().unwrap();
        (review.stamp, review.writing) = (Some((1, 1)), true);
        assert_eq!(
            why_not_decide(&f, Stage::PlanReview),
            Some(SUGGESTIONS_WRITING)
        );
        // Done.
        f.plan_review.as_mut().unwrap().writing = false;
        f.tick(1, "me", None);
        assert_eq!(
            why_not_decide(&f, Stage::PlanReview),
            Some(SUGGESTIONS_DONE)
        );
        for why in [SUGGESTIONS_DONE, SUGGESTIONS_WRITING] {
            assert!(!why.ends_with('.'), "{why}");
        }
    }

    #[test]
    fn plan_review_counts_its_suggestions_and_citations() {
        let mut cites = suggestion("r1-1", State::Open, Source::Reviewer);
        cites.evidence = vec![cited(Some(false)), cited(Some(true))];
        let f = at_plan_review(vec![
            cites,
            suggestion("r1-2", State::Open, Source::Reviewer),
            suggestion("r1-3", State::Accepted, Source::Reviewer),
            suggestion("r1-4", State::Rejected, Source::Reviewer),
        ]);
        assert_eq!(
            items_count(&f, Stage::PlanReview),
            "2 to decide \u{b7} 1 accepted \u{b7} 1 rejected \u{b7} 1 citation not found in the code"
        );
        let f = at_plan_review(vec![suggestion("r1-3", State::Accepted, Source::Reviewer)]);
        assert_eq!(items_count(&f, Stage::PlanReview), "1 accepted");
    }

    #[test]
    fn accept_all_is_offered_while_one_open_can_be_accepted() {
        let mut f = at_plan_review(vec![question("r1-1", "")]);
        assert!(
            !offers_fix_all(&f, Stage::PlanReview),
            "only a question to answer"
        );
        let review = f.plan_review.as_mut().unwrap();
        review
            .suggestions
            .push(suggestion("r1-2", State::Open, Source::Reviewer));
        assert!(offers_fix_all(&f, Stage::PlanReview));
        f.plan_review.as_mut().unwrap().writing = true;
        assert!(!offers_fix_all(&f, Stage::PlanReview));
    }

    #[test]
    fn a_suggestions_buttons_follow_where_it_stands() {
        let act = |id: &str, act| PanelOutcome::OnSuggestion(id.into(), act);
        let mut s = suggestion("r1-1", State::Open, Source::Reviewer);
        s.evidence = vec![cited(None)];
        assert_eq!(
            outcomes(Item::Suggestion(&s)),
            [
                ("Accept".into(), act("r1-1", ItemAct::Decide(Decision::Fix))),
                ("Comment".into(), act("r1-1", ItemAct::Comment)),
                ("Edit change\u{2026}".into(), act("r1-1", ItemAct::Edit)),
                ("Reject\u{2026}".into(), act("r1-1", ItemAct::AskRefuse)),
                ("Open code".into(), act("r1-1", ItemAct::OpenCode)),
                ("Show in plan".into(), act("r1-1", ItemAct::Show)),
            ]
        );
        s.state = State::Accepted;
        s.note = "Yes".into();
        assert_eq!(
            labels(Item::Suggestion(&s)),
            [
                "Reopen",
                "Edit comment",
                "Delete",
                "Edit change\u{2026}",
                "Reject\u{2026}",
                "Open code",
                "Show in plan"
            ]
        );
        s.state = State::Rejected;
        assert_eq!(
            labels(Item::Suggestion(&s)),
            [
                "Reopen",
                "Edit comment",
                "Delete",
                "Open code",
                "Show in plan"
            ]
        );
        // An unanswered question: Accept opens its answer.
        let q = question("r1-2", "");
        assert_eq!(
            outcomes(Item::Suggestion(&q))[..2],
            [
                ("Accept".into(), act("r1-2", ItemAct::Answer)),
                ("Answer".into(), act("r1-2", ItemAct::Comment)),
            ]
        );
        let q = question("r1-2", "Postgres");
        assert_eq!(
            outcomes(Item::Suggestion(&q))[0],
            ("Accept".into(), act("r1-2", ItemAct::Decide(Decision::Fix)))
        );
        // The user's own: no comment, no taking back, no code.
        let mut own = suggestion("u1", State::Accepted, Source::You);
        own.evidence = vec![cited(None)];
        assert_eq!(
            outcomes(Item::Suggestion(&own)),
            [
                ("Edit\u{2026}".into(), act("u1", ItemAct::Edit)),
                ("Delete".into(), act("u1", ItemAct::DeleteOwn)),
                ("Show in plan".into(), act("u1", ItemAct::Show)),
            ]
        );
        // Not decidable: only reading.
        let only: Vec<String> = (item_buttons(Item::Suggestion(&s), page(false)).into_iter())
            .map(|(b, _)| b.label)
            .collect();
        assert_eq!(only, ["Open code", "Show in plan"]);
    }

    #[test]
    fn a_suggestion_card_says_where_it_is_and_what_it_cites() {
        let mut s = suggestion("r1-1", State::Open, Source::Reviewer);
        s.severity = Severity::High;
        s.plan = "M2".into();
        s.why = "A restart loses it".into();
        s.change = "Keep it on disk".into();
        s.evidence = vec![cited(Some(false)), {
            let mut e = cited(None);
            e.repo.clear();
            e
        }];
        s.placed = Some(Placed::Nowhere);
        let card = item_card(Item::Suggestion(&s), false, &[], false);
        assert_eq!(card.tag, FindingTag::High);
        assert_eq!(card.state, "to decide");
        assert_eq!(
            card.place,
            "r1-1 \u{b7} gap \u{b7} M2 \u{b7} not placed in the plan"
        );
        assert_eq!(
            card.body,
            "A restart loses it\nCited: api/src/a.rs:12 \u{b7} not found in the code\nCited: src/a.rs:12"
        );
        assert_eq!(card.fix, "Change: Keep it on disk");
        assert_eq!(card.field_hint, "Your comment");
        assert!(card.stripe.is_none() && !card.collapsed && !card.faded);
        // A heading in place of its quote, left out by a later run.
        (s.placed, s.plan_quote, s.dropped) = (Some(Placed::Heading(3)), "x".into(), true);
        s.my_change = Some("Keep it in SQLite".into());
        s.state = State::Accepted;
        let card = item_card(Item::Suggestion(&s), false, &[], false);
        assert_eq!(
            card.place,
            "r1-1 \u{b7} gap \u{b7} M2 \u{b7} plan quote not found \u{b7} left out by a later run"
        );
        assert_eq!(
            card.fix,
            "Your change: Keep it in SQLite\nThe reviewer's: Keep it on disk"
        );
        assert_eq!(card.state, "accepted");
        assert!(card.collapsed);
        assert_eq!(card.stripe, Some(CardStripe::ToFix));
        // Placed on its quote: nothing to say.
        s.placed = Some(Placed::Quote(3));
        s.dropped = false;
        let card = item_card(Item::Suggestion(&s), false, &[], false);
        assert_eq!(card.place, "r1-1 \u{b7} gap \u{b7} M2");
        // Your own.
        let own = suggestion("u1", State::Accepted, Source::You);
        assert_eq!(
            item_card(Item::Suggestion(&own), false, &[], false).tag,
            FindingTag::Yours
        );
    }

    #[test]
    fn a_rejected_suggestion_folds_and_its_comment_holds_its_buttons() {
        let mut s = suggestion("r1-1", State::Rejected, Source::Reviewer);
        s.note = "Later".into();
        let actions = item_buttons(Item::Suggestion(&s), page(true));
        let card = item_card(Item::Suggestion(&s), false, &actions, false);
        assert_eq!(card.state, "rejected: Not needed \u{b7} commented");
        assert!(card.collapsed && card.faded);
        assert_eq!(card.comment, "Later");
        let on = |held: &[usize]| -> Vec<&str> {
            held.iter().map(|i| actions[*i].0.label.as_str()).collect()
        };
        // The first button is the folded card's.
        assert_eq!(
            on(&card.buttons),
            ["Reopen", "Edit comment", "Show in plan"]
        );
        assert_eq!(on(&card.comment_buttons), ["Delete"]);
        // A question: its answer.
        let q = question("r1-2", "");
        let card = item_card(Item::Suggestion(&q), false, &[], false);
        assert_eq!(card.state, "not answered yet");
        assert_eq!(card.field_hint, "Your answer");
        let mut q = question("r1-2", "Postgres");
        q.state = State::Rejected;
        let card = item_card(Item::Suggestion(&q), false, &[], false);
        assert_eq!(card.state, "rejected: Not needed \u{b7} answered");
    }

    #[test]
    fn a_suggestion_never_says_fix_dismiss_or_finding() {
        let mut all: Vec<Suggestion> = Vec::new();
        for state in [State::Open, State::Accepted, State::Rejected] {
            for source in [Source::Reviewer, Source::You] {
                for note in ["", "Yes"] {
                    let mut s = suggestion("r1-1", state, source);
                    s.note = note.into();
                    s.evidence = vec![cited(Some(false))];
                    s.change = "Keep it".into();
                    (s.placed, s.dropped) = (Some(Placed::Nowhere), true);
                    all.push(s.clone());
                    s.kind = Kind::Question;
                    all.push(s);
                }
            }
        }
        let bad = |text: &str| {
            let lower = text.to_lowercase();
            ["fix", "dismiss", "finding"]
                .iter()
                .any(|w| lower.contains(w))
        };
        for s in &all {
            let item = Item::Suggestion(s);
            for decides in [true, false] {
                for (b, _) in item_buttons(item, page(decides)) {
                    assert!(!bad(&b.label), "{}", b.label);
                }
                for (_, what) in item_hints(item, page(decides), false) {
                    assert!(!bad(&what), "{what}");
                }
            }
            let card = item_card(item, false, &[], false);
            for text in [&card.state, &card.place, &card.body, &card.fix, &card.title] {
                assert!(!bad(text), "{text}");
            }
            assert!(!bad(card.field_hint));
        }
        let f = at_plan_review(all.clone());
        assert!(!bad(&items_count(&f, Stage::PlanReview)));
        for why in [SUGGESTIONS_DONE, SUGGESTIONS_WRITING, OWN_ACCEPTED] {
            assert!(!bad(why), "{why}");
        }
    }

    #[test]
    fn each_key_on_a_suggestions_card() {
        let act = |id: &str, act| Some(PanelOutcome::OnSuggestion(id.into(), act));
        let mut open = suggestion("r1-1", State::Open, Source::Reviewer);
        open.evidence = vec![cited(None)];
        let accepted = suggestion("r1-2", State::Accepted, Source::Reviewer);
        let rejected = suggestion("r1-3", State::Rejected, Source::Reviewer);
        let unanswered = question("r1-4", "");
        let mut noted = suggestion("r1-5", State::Open, Source::Reviewer);
        noted.note = "Yes".into();
        let own = suggestion("u1", State::Accepted, Source::You);
        let cards = [
            ReviewCard::Item(Item::Suggestion(&own)),
            ReviewCard::Item(Item::Suggestion(&open)),
            ReviewCard::Item(Item::Suggestion(&accepted)),
            ReviewCard::Item(Item::Suggestion(&rejected)),
            ReviewCard::Item(Item::Suggestion(&unanswered)),
            ReviewCard::Item(Item::Suggestion(&noted)),
        ];
        let key = |k, at: usize| {
            let (mut at, mut arm) = (at, None);
            let got = card_key(k, &cards, &mut at, None, None, &mut arm);
            (got, at, arm)
        };
        // f: accept, and on to the next one open (the question, then round).
        assert_eq!(
            key(CardKey::F, 1),
            (act("r1-1", ItemAct::Decide(Decision::Fix)), 4, None)
        );
        assert_eq!(key(CardKey::F, 5).1, 1);
        assert_eq!(
            key(CardKey::F, 2),
            (act("r1-2", ItemAct::Decide(Decision::Reopen)), 2, None)
        );
        assert_eq!(
            key(CardKey::F, 3).0,
            act("r1-3", ItemAct::Decide(Decision::Fix))
        );
        // An unanswered question opens its answer, the keyboard staying.
        assert_eq!(key(CardKey::F, 4), (act("r1-4", ItemAct::Answer), 4, None));
        assert_eq!(
            key(CardKey::F, 0).0,
            Some(PanelOutcome::FindingRefused(OWN_ACCEPTED))
        );
        // Del: reject asking why, or reopen.
        assert_eq!(key(CardKey::Del, 1).0, act("r1-1", ItemAct::AskRefuse));
        assert_eq!(key(CardKey::Del, 2).0, act("r1-2", ItemAct::AskRefuse));
        assert_eq!(
            key(CardKey::Del, 3).0,
            act("r1-3", ItemAct::Decide(Decision::Reopen))
        );
        // Shift+Del: the comment, when there is one.
        assert_eq!(key(CardKey::ShiftDel, 1).0, None);
        assert_eq!(
            key(CardKey::ShiftDel, 5).0,
            act("r1-5", ItemAct::DeleteComment)
        );
        // c: a comment on a reviewer's; on the user's own the page adds one.
        assert_eq!(key(CardKey::C, 1).0, act("r1-1", ItemAct::Comment));
        assert_eq!(key(CardKey::C, 4).0, act("r1-4", ItemAct::Comment));
        assert_eq!(key(CardKey::C, 0).0, None);
        // Ctrl+E, Enter, Ctrl+O.
        assert_eq!(key(CardKey::CtrlE, 1).0, act("r1-1", ItemAct::Edit));
        assert_eq!(key(CardKey::CtrlE, 0).0, act("u1", ItemAct::Edit));
        assert_eq!(key(CardKey::Enter, 3).0, act("r1-3", ItemAct::Show));
        assert_eq!(key(CardKey::CtrlO, 1).0, act("r1-1", ItemAct::OpenCode));
        assert_eq!(key(CardKey::CtrlO, 2).0, None);
        // Refused: why, but reading still goes.
        let (mut at, mut arm) = (1, None);
        for k in [CardKey::F, CardKey::Del, CardKey::C, CardKey::CtrlE] {
            let got = card_key(k, &cards, &mut at, Some(SUGGESTIONS_DONE), None, &mut arm);
            assert_eq!(got, Some(PanelOutcome::FindingRefused(SUGGESTIONS_DONE)));
        }
        let got = card_key(
            CardKey::Enter,
            &cards,
            &mut at,
            Some(SUGGESTIONS_DONE),
            None,
            &mut arm,
        );
        assert_eq!(got, act("r1-1", ItemAct::Show));
        let got = card_key(
            CardKey::CtrlO,
            &cards,
            &mut at,
            Some(SUGGESTIONS_DONE),
            None,
            &mut arm,
        );
        assert_eq!(got, act("r1-1", ItemAct::OpenCode));
        // Del on the user's own: armed by kind, then deleted.
        let (got, _, arm) = key(CardKey::Del, 0);
        assert_eq!((got, arm.as_deref()), (None, Some("own-suggestion:u1")));
        let mut at = 0;
        let mut again = None;
        let got = card_key(
            CardKey::Del,
            &cards,
            &mut at,
            None,
            arm.as_deref(),
            &mut again,
        );
        assert_eq!(got, act("u1", ItemAct::DeleteOwn));
        // A finding's armed key doesn't delete it.
        let got = card_key(
            CardKey::Del,
            &cards,
            &mut at,
            None,
            Some("own:u1"),
            &mut again,
        );
        assert_eq!((got, again.as_deref()), (None, Some("own-suggestion:u1")));
    }

    #[test]
    fn the_hints_speak_the_kinds_words() {
        let hints = |item: Item, decides, armed| -> Vec<String> {
            (item_hints(item, page(decides), armed).into_iter())
                .map(|(k, what)| format!("{k} {what}"))
                .collect()
        };
        let f = finding("r1-1", FindingState::Open, false);
        assert_eq!(
            hints(Item::Finding(&f), true, false),
            [
                "f fix",
                "Ctrl+E edit fix\u{2026}",
                "c comment",
                "Del dismiss\u{2026}",
                "Enter show in diff"
            ]
        );
        let mut s = suggestion("r1-1", State::Accepted, Source::Reviewer);
        s.evidence = vec![cited(None)];
        s.note = "Yes".into();
        assert_eq!(
            hints(Item::Suggestion(&s), true, false),
            [
                "f reopen",
                "Ctrl+E edit change\u{2026}",
                // A written comment is edited in place, so `c` says so.
                "c edit comment",
                "Shift+Del delete comment",
                "Del reject\u{2026}",
                "Enter show in plan",
                "Ctrl+O open code"
            ]
        );
        let q = question("r1-2", "");
        assert_eq!(
            hints(Item::Suggestion(&q), true, false)[..3],
            [
                // As its button says: Accept, opening the answer first.
                "f accept\u{2026}",
                "Ctrl+E edit change\u{2026}",
                "c answer"
            ]
        );
        let own = suggestion("u1", State::Accepted, Source::You);
        assert_eq!(
            hints(Item::Suggestion(&own), true, true),
            [
                "Ctrl+E edit\u{2026}",
                "Del again to delete",
                "Enter show in plan"
            ]
        );
        assert_eq!(
            hints(Item::Suggestion(&own), false, false),
            ["Enter show in plan"]
        );
    }

    #[test]
    fn the_card_keys_are_one_table_the_footer_and_card_key_read() {
        let mut s = suggestion("r1-1", State::Accepted, Source::Reviewer);
        s.evidence = vec![cited(None)];
        s.note = "Yes".into();
        let own = suggestion("u1", State::Accepted, Source::You);
        let (q, open) = (
            question("r1-2", ""),
            suggestion("r1-3", State::Open, Source::Reviewer),
        );
        let fs = [
            finding("r1-1", FindingState::Open, false),
            finding("r1-2", FindingState::Fix, false),
            finding("r1-3", FindingState::Dismissed, false),
            finding("u1", FindingState::Fix, true),
            ReviewFinding {
                file: String::new(),
                ..finding("r1-4", FindingState::Open, false)
            },
        ];
        let items: Vec<Item> = [&s, &own, &q, &open]
            .map(Item::Suggestion)
            .into_iter()
            .chain(fs.iter().map(Item::Finding))
            .collect();
        let mut ctxs = Vec::new();
        for item in &items {
            for (decides, posts, armed) in [
                (true, false, false),
                (true, true, true),
                (false, false, false),
            ] {
                ctxs.push(CardCtx::of(*item, CardPage { decides, posts }, armed));
            }
        }
        key_table::check(CARD_KEYS, &ctxs);
        // Off a deciding page each key still reaches `card_key`, which says
        // why it can't; `c` on one of the user's own is the page's.
        let key = |item: Item, chord: &str| {
            let (mods, sym) = crate::keys::hint_key(chord).unwrap();
            card_key_of(&CardCtx::of(item, page(false), false), &mods, sym)
        };
        assert_eq!(key(Item::Suggestion(&s), "f"), Some(CardKey::F));
        assert_eq!(
            key(Item::Suggestion(&s), "Shift+Del"),
            Some(CardKey::ShiftDel)
        );
        assert_eq!(key(Item::Suggestion(&own), "c"), None);
        assert_eq!(key(Item::Suggestion(&own), "Ctrl+O"), None);
        assert_eq!(key(Item::Suggestion(&s), "Ctrl+O"), Some(CardKey::CtrlO));
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
