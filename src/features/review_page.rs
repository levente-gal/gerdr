//! The page of a review stage, AI review's and Plan review's, laid out
//! alike under the agent's card:
//!
//! - what the stage made, first (`lead`): Plan review's final plan;
//! - how its items stand, a line over them, with notes on why they can't
//!   be decided now;
//! - the items as cards (`review_cards`), the user's own first;
//! - what came before (`archive`): AI review's earlier reviews.
//!
//! Each kind's words are `review_items::Words`; the cards are
//! `review_cards`'. AI review's fix round cards stay where every stage with
//! rounds has them (`ROUND_CARD_STAGES`).

use super::*;

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
    sections.extend(archive(feature, stage, state, actions));
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
    // Only comments of the user's left: no item to count.
    let label = match items_count(feature, stage) {
        count if count.is_empty() => w.heading.to_owned(),
        count => format!("{} \u{b7} {count}", w.heading),
    };
    let mut sections = vec![lines(&label, review_notes(feature, stage, mark, state))];
    let multi = feature.many_repos();
    sections.extend(cards.into_iter().map(|card| {
        PageSection::Finding(match card {
            ReviewCard::Item(item) => item_card(item, multi, actions),
            ReviewCard::DiffNote(c) => note_card(c, multi, actions),
        })
    }));
    sections
}

/// What the items' line says while there are no cards: not read yet, none,
/// or how many of each stand where (AI review's kept in earlier reviews).
fn review_empty(feature: &Feature, stage: Stage) -> Vec<CheckLine> {
    let w = words(stage_items(stage));
    if !review_read(feature, stage) {
        return vec![line("not read yet", CheckTone::Note)];
    }
    let review = feature.review.as_ref().filter(|_| stage == Stage::AiReview);
    let Some(review) = review.filter(|r| !r.findings.is_empty()) else {
        return vec![line(format!("no {}", w.items), CheckTone::Ready)];
    };
    let states = [
        (FindingState::Open, "to decide", CheckTone::Blocker),
        (FindingState::Fix, "to fix", CheckTone::Blocker),
        (FindingState::Sent, "sent to Build", CheckTone::Note),
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
    let writing = (feature.plan_review.as_ref()).is_some_and(|r| r.writing);
    if stage == Stage::PlanReview && writing && plan_review_open(feature, stage) {
        notes.push(line(SUGGESTIONS_WRITING, CheckTone::Note));
    }
    // Make fixes is gone: not with no word of why.
    if sends_findings(feature, stage, state.ctx) == Some(PanelOutcome::SendFixes) {
        let findings = feature.review.iter().flat_map(|r| &r.findings);
        let why = review_import::plan_why(findings, state.ctx.comments);
        notes.extend(why.map(|why| line(why, CheckTone::Note)));
    }
    notes
}

/// What came before the latest review: each of AI review's earlier ones, a
/// file card folded unless `state` shows it.
fn archive(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let multi = feature.many_repos();
    let mut sections = Vec::new();
    for round in earlier_reviews(feature, stage) {
        sections.push(PageSection::File {
            name: format!("Review {}", round.n),
            state: line(round_count(round), CheckTone::Note),
            buttons: (actions.iter().enumerate())
                .filter(|(_, (_, o))| *o == PanelOutcome::FoldRound(round.n))
                .map(|(i, _)| i)
                .collect(),
        });
        if state.rounds_open.contains(&round.n) {
            let mut cards: Vec<&ReviewFinding> = round.findings.iter().collect();
            cards.sort_by_key(|f| review_import::list_rank(f));
            sections
                .extend((cards.into_iter()).map(|f| PageSection::Finding(round_card(f, multi))));
        }
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
    let w = words(stage_items(stage));
    let button = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Plain,
    };
    let mut out = Vec::new();
    if offers_fix_all(feature, stage) {
        out.push((button(w.want_all), want_all_outcome(stage)));
    }
    if can_add_comment(feature, stage) {
        out.push((button(w.add_own), PanelOutcome::AddComment));
    }
    if plan_review_open(feature, stage) {
        let deciding = crate::feature_plan_review::deciding(feature);
        if deciding.is_some_and(|r| !feature::plan_review::notes(r).is_empty()) {
            out.push((button("Ask reviewer"), PanelOutcome::AskReviewer));
        }
        if feature.plan_review.as_ref().is_some_and(|r| r.writing) {
            out.push((
                button("Change decisions\u{2026}"),
                PanelOutcome::ChangeDecisions,
            ));
        }
        out.push((button("Skip review"), PanelOutcome::SkipPlanReview));
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
