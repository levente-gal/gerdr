//! The page of a review stage, AI review's and Plan review's, laid out
//! alike under the agent's card:
//!
//! - what the stage made, first (`lead`): Plan review's final plan;
//! - how its items stand, a line over them, with notes on why they can't
//!   be decided now;
//! - the items as cards (`review_cards`), the user's own first;
//!
//! AI review's earlier reviews (`archive`) go in the page's history, each
//! over the fix rounds it asked for (`round_sections`).
//!
//! Each kind's words are `review_items::Words`; the cards are
//! `review_cards`'.

use crate::feature::review_items::words_in;
use super::*;
use crate::{
    chrome::ReviewSummary,
    feature::review_items::Standing,
    review_cards::{card_tag, page_card},
};

/// Whether `stage` is a review stage whose page this lays out.
pub(super) fn is_review_stage(stage: Stage) -> bool {
    matches!(stage, Stage::AiReview | Stage::PlanReview)
}

/// Whether `stage`'s review of `feature` has been read: its items are
/// there to show.
pub(super) fn review_read(feature: &Feature, stage: Stage) -> bool {
    match stage {
        Stage::AiReview => {
            feature.review.is_some()
                && (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_some())
        }
        Stage::PlanReview => (feature.plan_review.as_ref()).is_some_and(|r| r.stamp.is_some()),
        _ => false,
    }
}

/// Whether `stage`'s page of `feature` has a review to show while the
/// stage isn't reached: one read before the feature went back.
pub(super) fn review_kept(feature: &Feature, stage: Stage) -> bool {
    match stage {
        Stage::AiReview => feature.review.is_some(),
        Stage::PlanReview => review_read(feature, stage),
        _ => false,
    }
}

/// `stage`'s page of `feature` under its agent's card, at `mark`.
pub(super) fn review_page_sections(
    feature: &Feature,
    stage: Stage,
    mark: StageMark,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let mut sections = lead(feature, stage, mark, state, actions);
    sections.extend(item_sections(feature, stage, mark, state, actions));
    sections
}

/// What the stage made, over its items: Plan review's final plan, its file
/// card and its text, once written.
fn lead(
    feature: &Feature,
    stage: Stage,
    mark: StageMark,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    if stage != Stage::PlanReview || mark == StageMark::Later || !state.ctx.wrote(stage) {
        return Vec::new();
    }
    let mut out = vec![file_card(feature, stage, state.ctx, actions)];
    // The final plan's text goes here, not last as other pages' files do.
    out.extend(doc_of(state.probe, stage).map(|mut doc| {
        if let PageSection::Doc { here, .. } = &mut doc {
            *here = true;
        }
        doc
    }));
    out
}

/// The items' line and their cards; the line alone while there are none.
fn item_sections(
    feature: &Feature,
    stage: Stage,
    mark: StageMark,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let w = words(stage_items(stage));
    let cards = review_cards(feature, stage, notes_of(state.probe));
    if cards.is_empty() {
        return vec![lines(w.heading, review_empty(feature, stage))];
    }
    let w_items = items(feature, stage);
    let decided = (w_items.iter())
        .filter(|i| i.standing() != Standing::Open)
        .count();
    let refused = (w_items.iter())
        .filter(|i| i.standing() == Standing::Dismissed)
        .count();
    let tags: Vec<FindingTag> = cards.iter().map(|c| card_tag(*c)).collect();
    let mut counts: Vec<(FindingTag, usize)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for (card, tag) in cards.iter().zip(&tags) {
        let undecided = card.item().is_some_and(|i| i.standing() == Standing::Open);
        match counts.last_mut() {
            Some((last, n)) if last == tag => *n += 1,
            _ => {
                counts.push((*tag, 1));
                open.push(0);
            }
        }
        if let Some(n) = open.last_mut() {
            *n += usize::from(undecided);
        }
    }
    let want_all = want_all_outcome(stage);
    // Past the first run, which one this is: cards say what it changed.
    let run = match stage {
        Stage::PlanReview => feature.plan_review.as_ref().map(|r| r.n),
        _ => feature.review.as_ref().map(|r| r.n),
    };
    let label = match run {
        Some(n) if n > 1 => format!("{} \u{b7} run {n}", w.heading),
        _ => w.heading.to_owned(),
    };
    let summary = ReviewSummary {
        label,
        counts,
        open,
        decided,
        refused,
        total: w_items.len(),
        words: items_count(feature, stage),
        notes: review_notes(feature, stage, mark, state),
        buttons: (actions.iter().position(|(_, o)| *o == want_all))
            .into_iter()
            .collect(),
    };
    let mut sections = vec![PageSection::Review(summary)];
    let multi = feature.many_repos();
    // No heading per severity: the summary's pills and each card's pill say
    // it, and a pill goes to its first card.
    for card in cards {
        let mut card = match card {
            ReviewCard::Item(item) => page_card(feature, item, actions),
            ReviewCard::DiffNote(c) => note_card(c, multi, actions),
        };
        card.folds = true;
        sections.push(PageSection::Finding(card));
    }
    sections
}

/// What the items' line says while there are no cards: not written yet, none,
/// or how many of each stand where (AI review's kept in earlier reviews).
fn review_empty(feature: &Feature, stage: Stage) -> Vec<CheckLine> {
    let w = words(stage_items(stage));
    if !review_read(feature, stage) {
        return vec![line("not written yet", CheckTone::Note)];
    }
    let review = feature.review.as_ref().filter(|_| stage == Stage::AiReview);
    let Some(review) = review.filter(|r| !r.findings.is_empty()) else {
        return vec![line(format!("no {}", w.items), CheckTone::Ready)];
    };
    let planning = feature::fix_round::planning(feature.fix_round.as_ref(), Stage::AiReview);
    let sent_word = crate::feature::review_import::sent_word;
    let states = [
        (FindingState::Open, "to decide", CheckTone::Blocker),
        (FindingState::Fix, "to fix", CheckTone::Blocker),
        (
            FindingState::Sent,
            sent_word(false, planning),
            CheckTone::Note,
        ),
        (FindingState::Fixed, "fixed", CheckTone::Ready),
        (FindingState::Dismissed, "dismissed", CheckTone::Note),
    ];
    (states.into_iter())
        .filter_map(|(state, what, tone)| match review.count(state) {
            0 => None,
            n => Some(line(format!("{} {what}", plural(n, w.item)), tone)),
        })
        .collect()
}

/// The notes under the items' line: why they can't be decided now, and
/// why AI review's Make fixes is gone.
fn review_notes(
    feature: &Feature,
    stage: Stage,
    mark: StageMark,
    state: &PanelState,
) -> Vec<CheckLine> {
    let w = words(stage_items(stage));
    let mut notes = Vec::new();
    match mark {
        StageMark::Later => notes.push(line(
            format!("From the last review. The {} has changed since.", w.reads),
            CheckTone::Note,
        )),
        StageMark::Current if !review_read(feature, stage) => notes.push(line(
            "From the last review. A new one hasn't finished.",
            CheckTone::Note,
        )),
        _ => {}
    }
    let review = feature.plan_review.as_ref().filter(|_| stage == Stage::PlanReview);
    let writing = review.is_some_and(|r| r.writing);
    if writing && plan_review_open(feature, stage) {
        // Once written it is no longer being written: it waits for review.
        let note = match state.ctx.wrote(stage) {
            true => FINAL_WRITTEN,
            false => SUGGESTIONS_WRITING,
        };
        notes.push(line(note, CheckTone::Note));
    }
    if review.is_some_and(|r| r.asked) && plan_review_open(feature, stage) {
        let who = (feature.runs.get(&stage)).map_or("the reviewer", |r| r.agent.as_str());
        notes.push(line(
            format!("Asked {who} about your notes: waiting for its answer, the next run"),
            CheckTone::Note,
        ));
    }
    // Make fixes is gone: not with no word of why.
    if sends_findings(feature, stage, state.ctx) == Some(PanelOutcome::SendFixes) {
        let findings = feature.review.iter().flat_map(|r| &r.findings);
        let why = review_import::plan_why(findings, state.ctx.comments);
        notes.extend(why.map(|why| line(why, CheckTone::Note)));
    }
    notes
}

/// What the suggestions' line says once the final plan was written from
/// them: it waits for the user's review above.
const FINAL_WRITTEN: &str =
    "The final plan above was written from these: Change decisions\u{2026} to change them";

/// An earlier AI review, `round`: a file card, its findings under it
/// unless `state` folds them.
pub(super) fn archive(
    feature: &Feature,
    round: &ReviewRound,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let mut sections = vec![PageSection::File {
        name: format!("AI review run {}", round.n),
        state: line(round_count(round), CheckTone::Note),
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| *o == PanelOutcome::FoldRound(round.n))
            .map(|(i, _)| i)
            .collect(),
    }];
    if state.rounds_open.contains(&round.n) {
        let mut cards: Vec<&ReviewFinding> = round.findings.iter().collect();
        cards.sort_by_key(|f| review_import::list_rank(f));
        let multi = feature.many_repos();
        sections.extend((cards.into_iter()).map(|f| PageSection::Finding(round_card(f, multi))));
    }
    sections
}

/// The page's own buttons of `stage`'s review, in one order on both:
/// wanting every open item, adding one of the user's own, then what only
/// Plan review has.
pub(super) fn review_page_actions(
    feature: &Feature,
    stage: Stage,
) -> Vec<(PageButton, PanelOutcome)> {
    if !is_review_stage(stage) {
        return Vec::new();
    }
    let w = words_in(stage_items(stage), feature.posts_review());
    let button = |label: &str, on: bool| PageButton {
        label: label.to_owned(),
        style: match on {
            true => ButtonStyle::Plain,
            false => ButtonStyle::Off,
        },
    };
    let mut out = Vec::new();
    // The buttons stay while their items can be decided, greyed once there
    // is nothing for them to do: pressed, they say why, and none shifts.
    if can_decide(feature, stage) {
        out.push((
            button(w.want_all, offers_fix_all(feature, stage)),
            want_all_outcome(stage),
        ));
    }
    // Once the findings are decided the page's next step takes its place:
    // the diff is still there to read them in.
    let in_diff = stage == Stage::AiReview
        && review_read(feature, stage)
        && (feature.review.as_ref()).is_some_and(|r| {
            r.count(FindingState::Open) == 0 && r.findings.iter().any(|f| !f.file.is_empty())
        });
    if in_diff {
        out.push((button("Review in diff", true), PanelOutcome::Findings));
    }
    if can_add_comment(feature, stage) {
        out.push((button(w.add_own, true), PanelOutcome::AddComment));
    }
    if plan_review_open(feature, stage) {
        let deciding = crate::feature_plan_review::deciding(feature);
        let review = feature.plan_review.as_ref();
        if let Some(r) = deciding {
            // Asked already, it waits for the answer.
            let asks = !feature::plan_review::notes(r).is_empty() && !r.asked;
            out.push((button("Ask reviewer", asks), PanelOutcome::AskReviewer));
        }
        let writing = review.is_some_and(|r| r.writing);
        if writing {
            out.push((
                button("Change decisions\u{2026}", true),
                PanelOutcome::ChangeDecisions,
            ));
        }
        // Skipping is for a review not worth deciding: not once all is
        // decided, nor while the final plan is written or waits.
        let all_decided =
            deciding.is_some_and(|r| r.open() == 0 && !r.suggestions.is_empty());
        if !writing && !all_decided {
            out.push((button("Skip review", true), PanelOutcome::SkipPlanReview));
        }
    }
    out
}

/// What wanting every open item of `stage`'s page does.
pub(super) fn want_all_outcome(stage: Stage) -> PanelOutcome {
    match stage {
        Stage::PlanReview => PanelOutcome::AcceptAllSuggestions,
        _ => PanelOutcome::FixAllFindings,
    }
}
