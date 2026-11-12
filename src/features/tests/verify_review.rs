//! Tests of Verify's page, Code review and their fix rounds.

use super::*;

#[test]
fn verifys_checks_are_cards_with_their_own_keys() {
    let shift = |sym| {
        let mods = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        (mods, sym)
    };
    let f = verified(false);
    let mut panel = FeaturePanel::default();
    // The page's own keys, from the stages already.
    assert_eq!(
        press(&mut panel, &f, shift(Keysym::R)),
        Some(PanelOutcome::RunFailed)
    );
    assert_eq!(press(&mut panel, &f, key(Keysym::r)), None);
    // Tab goes to the cards, Up and Down through them.
    assert_eq!(press(&mut panel, &f, key(Keysym::Tab)), None);
    assert_eq!(panel.focus, TabArea::Findings);
    assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::ShowCheck(1))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::r)),
        Some(PanelOutcome::RerunCheck(1))
    );
    // While they run: stop, not run again.
    let running = verified(true);
    assert_eq!(
        press(&mut panel, &running, key(Keysym::s)),
        Some(PanelOutcome::StopCheck(1))
    );
    assert_eq!(
        press(&mut panel, &running, shift(Keysym::S)),
        Some(PanelOutcome::StopChecks)
    );
    assert_eq!(press(&mut panel, &running, shift(Keysym::R)), None);
    // A key of the findings does nothing here.
    assert_eq!(press(&mut panel, &f, key(Keysym::f)), None);
}

/// `f`, with a fix round asked by `stage`, in `phase`.
#[test]
fn a_code_review_at_hand_or_done_goes_on_its_page() {
    use crate::feature_diff::review_views;
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    assert_eq!(f.current(), Some(Stage::CodeReview));
    assert!(review_views(&f));
    // Its code isn't changed: nothing to review.
    let mut no_code = f.clone();
    no_code.repos[0].touched = false;
    assert!(!review_views(&no_code));
    // Another machine writes it: the page says so.
    let mut elsewhere = f.clone();
    elsewhere.writer = Some("m1".into());
    assert!(!review_views(&elsewhere));
    // A fix plan of its own keeps the review there, beside it.
    assert!(review_views(&in_round(
        f.clone(),
        Stage::CodeReview,
        FixPhase::Ready
    )));
    // Closed: the page says how it went.
    let mut closed = f.clone();
    closed.status = Status::Abandoned;
    assert!(!review_views(&closed));
    // Done with it: the review stays, only to read.
    let done = feature("f", vec![repo("api", true)], code_review + 1);
    assert!(review_views(&done));
}

#[test]
fn the_review_page_goes_by_the_page_showing_and_the_keys_list() {
    use crate::feature_diff::review_page;
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let mut panel = FeaturePanel::default();
    assert!(review_page(&panel, &f));
    panel.pick(&f, Stage::Build);
    assert!(!review_page(&panel, &f));
    panel.unpick();
    panel.keys = true;
    assert!(!review_page(&panel, &f));
    // The page's view says so, with no strip's or footer's keys.
    panel.keys = false;
    let view = tab_view(&panel, &f, &[], &PanelState::default(), 2_000);
    assert!(view.review_page && view.review_keys && view.hints.is_empty());
    panel.rail_keys = true;
    let view = tab_view(&panel, &f, &[], &PanelState::default(), 2_000);
    assert!(view.review_page && !view.review_keys);
}

/// `f` with Code review's round `n` open at `phase`, taking C1 and C2.
fn comment_round(mut f: Feature, phase: FixPhase) -> Feature {
    f = in_round(f, Stage::CodeReview, phase);
    f.fix_rounds = 1;
    let round = f.fix_round.as_mut().unwrap();
    round.what = "review comments".into();
    round.give_back = feature::fix_round::GiveBack::Comments {
        copy: "/d/c.json".into(),
        ids: vec!["C1".into(), "C2".into()],
    };
    f.comment_fixes = vec![CommentFix {
        round: 1,
        made: Vec::new(),
    }];
    f
}

fn fixes_probe(notes: Vec<Comment>, plan: bool) -> Probe {
    Probe {
        docs: feature::page::Docs {
            notes,
            comment_plan: plan.then(|| {
                std::sync::Arc::new(vec![crate::md_blocks::DocBlock::Note("The plan".into())])
            }),
            ..Default::default()
        },
        ..ship::probe_seen(&feature("x", vec![repo("api", true)], 6))
    }
}

#[test]
fn code_review_opens_on_its_code_then_on_its_fixes_for_good() {
    use crate::{chrome::ReviewView::*, feature_diff::review_page};
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let mut panel = FeaturePanel::default();
    // First time, the review.
    assert_eq!(panel.review_view_of(&f), Code);
    assert!(review_page(&panel, &f));
    // Plan fixes, and from there on its fixes.
    let planning = comment_round(f.clone(), FixPhase::Planning);
    assert_eq!(panel.review_view_of(&planning), Fixes);
    assert!(!review_page(&panel, &planning));
    let ready = comment_round(f.clone(), FixPhase::Ready);
    assert_eq!(panel.review_view_of(&ready), Fixes);
    let mut over = ready.clone();
    over.last_fix_round = over.fix_round.take();
    assert_eq!(panel.review_view_of(&over), Fixes);
    // The code changed and sent it back: the page is still where the
    // fixes are followed, and still goes to the code.
    let mut fixing = comment_round(f.clone(), FixPhase::Fixing);
    fixing.marks.remove(&Stage::Build);
    assert_eq!(fixing.current(), Some(Stage::Build));
    // While they are made, the commits sending it back don't move the page.
    assert_eq!(panel.page_stage(&fixing), Stage::CodeReview);
    assert_eq!(panel.review_view_of(&fixing), Fixes);
    panel.pick(&fixing, Stage::Build);
    assert_eq!(panel.page_stage(&fixing), Stage::Build);
    panel.unpick();
    let mut back = over.clone();
    back.marks.remove(&Stage::AiReview);
    assert_eq!(back.current(), Some(Stage::AiReview));
    // Made, the page goes on with the feature.
    assert_eq!(panel.page_stage(&back), Stage::AiReview);
    panel.pick(&back, Stage::CodeReview);
    assert_eq!(panel.review_view_of(&back), Fixes);
    assert!(crate::feature_diff::review_views(&back));
    panel.unpick();
    // Picked, it stays so whatever the default.
    panel.review_view = Some(Code);
    assert_eq!(panel.review_view_of(&over), Code);
    assert!(review_page(&panel, &over));
    panel.review_view = None;
    assert!(!review_page(&panel, &over));
    // Dropped: nothing to follow, so its code.
    let mut dropped = over;
    dropped.comment_fixes.clear();
    dropped.last_fix_round = None;
    assert_eq!(panel.review_view_of(&dropped), Code);
}

#[test]
fn alt_digits_go_between_code_review_and_its_fixes_only_there() {
    use crate::chrome::ReviewView::*;
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let mut panel = FeaturePanel::default();
    assert_eq!(
        press(&mut panel, &f, alt(Keysym::_2)),
        Some(PanelOutcome::ReviewView(Fixes))
    );
    assert_eq!(
        press(&mut panel, &f, alt(Keysym::_1)),
        Some(PanelOutcome::ReviewView(Code))
    );
    let build = feature("b", vec![repo("api", true)], stage_index(Stage::Build));
    assert_eq!(press(&mut panel, &build, alt(Keysym::_2)), None);
    // Off the cards, as on AI review's page, a comment of one's own.
    panel.review_view = Some(Fixes);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::c)),
        Some(PanelOutcome::AddComment)
    );
    panel.review_view = None;
    // Another machine writes it: no switch to go by.
    let mut elsewhere = f.clone();
    elsewhere.writer = Some("m1".into());
    assert_eq!(press(&mut panel, &elsewhere, alt(Keysym::_2)), None);
}

#[test]
fn the_fixes_view_shows_the_plan_and_comments_as_ai_review_does() {
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let mut f = comment_round(f, FixPhase::Ready);
    f.comment_fixes[0].made = vec!["C2".into()];
    let probe = fixes_probe(
        vec![
            note("src/new.rs", "Not sent yet"),
            sent("C1", "src/a.rs", "Rename it"),
            sent("C2", "src/b.rs", "Add a test"),
        ],
        true,
    );
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    let notes = notes_of(state.probe);
    let actions = stage_actions(&f, Stage::CodeReview, RowCtx::default(), &[], &[], notes);
    let page = stage_page(&f, Stage::CodeReview, &state, &actions, 2_000);
    // The round's card holds what acts on its plan, as ever: open, on
    // top.
    let Some(PageSection::Round(card)) = page.sections.first() else {
        panic!("{:?}", page.sections);
    };
    let held: Vec<&str> = (card.buttons.iter())
        .map(|i| page.buttons[*i].label.as_str())
        .collect();
    assert_eq!(held[..2], ["Review", "Make fixes"]);
    assert!(page.sections.iter().any(|s| matches!(s,
        PageSection::Lines { label, .. } if label == "Comments \u{b7} 1 of 2 made")));
    let cards: Vec<&FindingCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].state, "goes with the fixes");
    assert_eq!(cards[0].stripe, None);
    // Its plan waits for the user: not fixed yet.
    assert_eq!(cards[1].state, "in fix plan");
    assert_eq!(cards[1].stripe, Some(CardStripe::Sent));
    assert!(
        cards[1].place.starts_with("C1 \u{b7} "),
        "{}",
        cards[1].place
    );
    assert_eq!(cards[2].state, "fix made");
    assert!(cards[2].made);
    // Last, whatever comes before it, the plan to read.
    assert!(matches!(
        page.sections.last(),
        Some(PageSection::Doc { name, .. }) if name == feature::fix_round::FILE
    ));
    // What went to Build can be shown, not changed; what didn't, both.
    let label = |card: &FindingCard| -> Vec<String> {
        (card.buttons.iter())
            .map(|i| page.buttons[*i].label.clone())
            .collect()
    };
    assert_eq!(label(cards[1]), ["Show in diff"]);
    assert_eq!(label(cards[0]), ["Edit\u{2026}", "Delete", "Show in diff"]);
}

#[test]
fn a_plan_being_written_again_isnt_shown_as_if_it_were_the_one() {
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let probe = fixes_probe(vec![sent("C1", "src/a.rs", "Rename it")], true);
    let has_doc = |f: &Feature| {
        let state = PanelState {
            probe: Some(&probe),
            ..PanelState::default()
        };
        comment_sections(f, &state, &[])
            .iter()
            .any(|s| matches!(s, PageSection::Doc { .. }))
    };
    assert!(!has_doc(&comment_round(f.clone(), FixPhase::Planning)));
    assert!(!has_doc(&comment_round(f.clone(), FixPhase::Compacting)));
    assert!(has_doc(&comment_round(f.clone(), FixPhase::Ready)));
    assert!(has_doc(&comment_round(f, FixPhase::Fixing)));
}

#[test]
fn its_fixes_stay_on_the_page_once_the_feature_went_back_for_them() {
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let mut f = comment_round(f, FixPhase::Fixing);
    f.marks.remove(&Stage::AiReview);
    assert_eq!(stage_mark(&f, Stage::CodeReview), StageMark::Later);
    let probe = fixes_probe(vec![sent("C1", "src/a.rs", "Rename it")], true);
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    let actions = stage_actions(
        &f,
        Stage::CodeReview,
        RowCtx::default(),
        &[],
        &[],
        notes_of(state.probe),
    );
    let page = stage_page(&f, Stage::CodeReview, &state, &actions, 2_000);
    let has = |what: fn(&PageSection) -> bool| page.sections.iter().any(what);
    assert!(has(|s| matches!(s, PageSection::Round(_))));
    assert!(has(|s| matches!(s, PageSection::Finding(_))));
    assert!(has(|s| matches!(s, PageSection::Doc { .. })));
    let labels: Vec<&str> = page.buttons.iter().map(|b| b.label.as_str()).collect();
    assert!(labels.contains(&OPEN_PLAN), "{labels:?}");
    assert!(labels.contains(&"Show in diff"), "{labels:?}");
}

#[test]
fn a_comment_gone_to_build_only_shows_while_one_not_sent_can_change() {
    let code_review = stage_index(Stage::CodeReview);
    let f = comment_round(
        feature("f", vec![repo("api", true)], code_review),
        FixPhase::Ready,
    );
    let unsent = note("src/new.rs", "Not sent yet");
    let gone = sent("C1", "src/a.rs", "Rename it");
    let notes = [unsent.clone(), gone.clone()];
    let slugs: Vec<String> = Vec::new();
    let mut panel = FeaturePanel {
        review_view: Some(chrome::ReviewView::Fixes),
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    let hit = |panel: &mut FeaturePanel, at: usize, (mods, sym)| {
        panel.finding = at;
        panel.key(&mods, sym, &f, &slugs, RowCtx::default(), &[], &[], &notes)
    };
    assert_eq!(
        hit(&mut panel, 0, key(Keysym::c)),
        Some(PanelOutcome::EditDiffNote(unsent.clone()))
    );
    assert_eq!(
        hit(&mut panel, 1, key(Keysym::c)),
        Some(PanelOutcome::FindingRefused(SENT_NOTE))
    );
    assert_eq!(
        hit(&mut panel, 1, key(Keysym::Delete)),
        Some(PanelOutcome::FindingRefused(SENT_NOTE))
    );
    assert_eq!(
        hit(&mut panel, 1, key(Keysym::f)),
        Some(PanelOutcome::FindingRefused(SENT_NOTE))
    );
    assert_eq!(
        hit(&mut panel, 1, key(Keysym::Return)),
        Some(PanelOutcome::ShowDiffNote(gone))
    );
    // AI review lists none that went with Code review's round.
    let ai = feature("a", vec![repo("api", true)], stage_index(Stage::AiReview));
    assert_eq!(shown_notes(&ai, Stage::AiReview, &notes).len(), 0);
}

#[test]
fn code_reviews_rounds_show_as_cards_with_the_comments_they_took() {
    let code_review = stage_index(Stage::CodeReview);
    let mut f = comment_round(
        feature("f", vec![repo("api", true)], code_review),
        FixPhase::Fixing,
    );
    f.fix_round.as_mut().unwrap().n = 2;
    f.fix_rounds = 2;
    f.fix_asked = BTreeMap::from([(1, Stage::CodeReview), (2, Stage::CodeReview)]);
    // Round 1 is from before rounds were kept whole: only its comments.
    f.comment_fixes = vec![
        CommentFix {
            round: 1,
            made: vec!["C1".into()],
        },
        CommentFix {
            round: 2,
            made: Vec::new(),
        },
    ];
    let mut probe = fixes_probe(vec![sent("C1", "src/b.rs", "Latest")], false);
    probe.docs.old_notes = BTreeMap::from([(
        1,
        vec![
            sent("C1", "src/a.rs", "Old"),
            sent("C2", "src/a.rs", "Older"),
        ],
    )]);
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    let cards: Vec<chrome::RoundCard> =
        (round_cards(&f, Stage::CodeReview, &state, &[], 4_000).into_iter())
            .filter_map(|s| match s {
                PageSection::Round(card) => Some(card),
                _ => None,
            })
            .collect();
    let labels: Vec<&str> = cards.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Fix round 2", "Fix round 1"]);
    let asked = |card: &chrome::RoundCard| -> Vec<(String, String, Option<String>)> {
        (card.asked.iter())
            .map(|a| {
                (
                    a.id.clone(),
                    a.note.clone(),
                    a.now.as_ref().map(|l| l.text.clone()),
                )
            })
            .collect()
    };
    assert_eq!(
        asked(&cards[0]),
        [("C1".into(), "Latest".into(), Some("being fixed".into()))]
    );
    assert_eq!(
        asked(&cards[1]),
        [
            ("C1".into(), "Old".into(), Some("fix made".into())),
            ("C2".into(), "Older".into(), Some("not marked made".into())),
        ]
    );
    assert_eq!(cards[0].asked[0].title, "src/b.rs:3");
    // Kept only by its number, it has no times to say: only who asked.
    assert_eq!(cards[1].meta, "asked by Code review");
    assert_eq!(cards[1].dot, Dot::Done);
    // The cards take the place of the folded rows of earlier rounds.
    let sections = comment_sections(&f, &state, &[]);
    assert!(
        !sections
            .iter()
            .any(|s| matches!(s, PageSection::File { .. }))
    );
    // Alt+E folds them all, then opens them all, on this page.
    let mut panel = FeaturePanel {
        review_view: Some(chrome::ReviewView::Fixes),
        ..FeaturePanel::default()
    };
    assert_eq!(press(&mut panel, &f, alt(Keysym::e)), None);
    assert_eq!(panel.fix_rounds_flipped, BTreeSet::from([2]));
    assert_eq!(press(&mut panel, &f, alt(Keysym::e)), None);
    assert_eq!(panel.fix_rounds_flipped, BTreeSet::from([1]));
}

#[test]
fn ai_reviews_rounds_say_how_each_finding_they_took_stands() {
    use crate::feature::review_import::{AiReview, Finding};
    let mut f = feature("g", vec![repo("api", true)], stage_index(Stage::AiReview));
    let finding = |id: &str, state| Finding {
        id: id.into(),
        title: format!("Finding {id}"),
        file: "src/a.rs".into(),
        line: Some(4),
        state,
        ..Finding::default()
    };
    f.review = Some(AiReview {
        findings: vec![
            finding("r1-1", FindingState::Fixed),
            finding("r1-2", FindingState::Open),
        ],
        ..AiReview::default()
    });
    let mut round = manual_round(1, Some(2_000), Some("a1"));
    round.stage = Stage::AiReview;
    round.trail.asked = Vec::new();
    round.give_back = feature::fix_round::GiveBack::Findings {
        ids: vec!["r1-1".into(), "r1-2".into()],
        comments: None,
    };
    f.fix_asked.insert(1, Stage::AiReview);
    f.past_rounds = vec![round.clone()];
    f.fix_rounds = 1;
    let asked = asked_of(&f, &round, None);
    let titles: Vec<&str> = asked.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Finding r1-1 (src/a.rs:4)", "Finding r1-2 (src/a.rs:4)"]
    );
    let now = |id| asked_now(&f, &round, id, false).map(|l| l.text);
    assert_eq!(now("r1-1").as_deref(), Some("fixed"));
    assert_eq!(now("r1-2").as_deref(), Some("raised again"));
    // On AI review's page, whatever its mark.
    let state = PanelState::default();
    let mut actions = stage_actions(&f, Stage::AiReview, state.ctx, &[], &[], &[]);
    actions.extend(round_plan_actions(&f, Stage::AiReview, None));
    let page = stage_page(&f, Stage::AiReview, &state, &actions, 4_000);
    assert!(
        page.sections
            .iter()
            .any(|s| matches!(s, PageSection::Round(c) if c.label == "Fix round 1"))
    );
}

#[test]
fn the_switch_says_how_far_the_review_and_its_fixes_have_come() {
    let code_review = stage_index(Stage::CodeReview);
    let f = feature("f", vec![repo("api", true)], code_review);
    let counts = crate::feature_diff::Counts {
        files: 8,
        viewed: 3,
        changed: 1,
        comments: 0,
    };
    let state = PanelState {
        code_counts: Some(counts),
        ..PanelState::default()
    };
    let view = tab_view(&FeaturePanel::default(), &f, &[], &state, 2_000);
    let switch = view.review_switch.expect("its switch");
    assert_eq!(switch.view, chrome::ReviewView::Code);
    assert_eq!(switch.fixes, "none yet");
    assert_eq!(
        switch.files,
        Some(chrome::ReviewFiles {
            viewed: 3,
            total: 8,
            changed: 1
        })
    );
    // The rail says the same as the switch.
    assert_eq!(
        rail_node(&f, Stage::CodeReview, &state, 2_000).line,
        "3 of 8 files"
    );
    let round = comment_round(f.clone(), FixPhase::Planning);
    assert_eq!(fixes_word(&round, &[]), "planning fixes");
    let mut over = comment_round(f.clone(), FixPhase::Ready);
    over.last_fix_round = over.fix_round.take();
    over.comment_fixes[0].made = vec!["C1".into()];
    let notes = [sent("C1", "a", "x"), sent("C2", "b", "y")];
    // It names what it counts, as the Comments heading under it does.
    assert_eq!(fixes_word(&over, &notes), "1 of 2 comments made");
    // Elsewhere, no switch.
    let build = feature("b", vec![repo("api", true)], stage_index(Stage::Build));
    let view = tab_view(&FeaturePanel::default(), &build, &[], &state, 2_000);
    assert_eq!(view.review_switch, None);
}

/// How `f`'s rail reads for `ctx`: its marks, in order.
fn rail_marks(f: &Feature, ctx: RowCtx) -> Vec<NodeMark> {
    let state = PanelState {
        ctx,
        ..PanelState::default()
    };
    (Stage::ALL.iter())
        .map(|s| rail_node(f, *s, &state, 2_000).mark)
        .collect()
}

#[test]
fn the_rail_stays_turned_back_from_a_rounds_start_to_builds_next_mark() {
    use NodeMark::{Current, Done, Fixing, Later, Paused, Ran, Waiting};
    let running = RowCtx {
        fixing: Some(Dot::Working),
        ..RowCtx::default()
    };
    let at_rest = RowCtx::default();
    // At AI review, nothing sent back yet.
    let mut f = feature("b", vec![repo("api", true)], 5);
    let mut run = held_run(Some("p1"));
    run.held = false;
    f.runs.insert(Stage::Build, run);
    assert_eq!(turned_back(&f, at_rest), None);
    assert_eq!(
        rail_marks(&f, at_rest),
        [Done, Done, Done, Done, Done, Current, Later, Later, Later]
    );
    // The agent plans: it alone stands out, the stage that asked waiting.
    let mut f = in_round(f, Stage::AiReview, FixPhase::Planning);
    assert_eq!(turned_back(&f, running), Some(Stage::AiReview));
    assert_eq!(
        rail_marks(&f, running),
        [Done, Done, Done, Fixing, Ran, Paused, Later, Later, Later]
    );
    // The plan is ready and the agent rests: the line stays, the next move
    // the user's.
    f = in_round(f, Stage::AiReview, FixPhase::Ready);
    assert_eq!(turned_back(&f, at_rest), Some(Stage::AiReview));
    assert_eq!(
        rail_marks(&f, at_rest),
        [Done, Done, Done, Ran, Ran, Waiting, Later, Later, Later]
    );
    // The fixes change the code: the marks from Build on go. What ran
    // doesn't turn into stages that never did, and when it ran still shows.
    f = in_round(f, Stage::AiReview, FixPhase::Fixing);
    assert_eq!(f.invalidate("changed", 2_500), Some(Stage::Build));
    assert_eq!(turned_back(&f, running), Some(Stage::AiReview));
    assert_eq!(
        rail_marks(&f, running),
        [Done, Done, Done, Current, Ran, Paused, Later, Later, Later]
    );
    let state = PanelState::default();
    let asked = rail_node(
        &f,
        Stage::AiReview,
        &state,
        f.ran_once[&Stage::AiReview] + 60,
    );
    assert_eq!(asked.line, "1m ago");
    // The round is over, and Build still to do again.
    f.last_fix_round = f.fix_round.take();
    assert_eq!(turned_back(&f, at_rest), Some(Stage::AiReview));
    // Build marked done: the flow goes on, and the line goes. The stages
    // ahead stay as they were until they are marked again.
    f.tick(2_600, "you", Some("changed"));
    assert_eq!(turned_back(&f, at_rest), None);
    assert_eq!(
        rail_marks(&f, at_rest),
        [Done, Done, Done, Done, Current, Paused, Later, Later, Later]
    );
}

#[test]
fn the_stage_left_off_at_is_the_one_that_ran_last() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    assert_eq!(f.current(), Some(Stage::Build));
    assert_eq!(left_off(&f), None);
    // What ran last counts, not what stands furthest on.
    f.ran_once.insert(Stage::ManualTest, 100);
    f.ran_once.insert(Stage::AiReview, 200);
    assert_eq!(left_off(&f), Some(Stage::AiReview));
    // In the same second, the later stage.
    f.ran_once.insert(Stage::CodeReview, 200);
    assert_eq!(left_off(&f), Some(Stage::CodeReview));
    // Starting over forgets what came after it.
    assert!(f.start_over(Stage::Build, 300));
    assert_eq!(left_off(&f), None);
    // The stage it is at isn't one it left, though it ran once.
    let mut f = feature("b", vec![repo("api", true)], 5);
    f.ran_once.insert(Stage::AiReview, 200);
    assert_eq!(left_off(&f), None);
    let marks = rail_marks(&f, RowCtx::default());
    assert_eq!(marks[stage_index(Stage::AiReview)], NodeMark::Current);
}

#[test]
fn the_line_back_starts_at_the_stage_that_asked_and_only_past_build() {
    let at_rest = RowCtx::default();
    let done = |stage: Stage| stage_index(stage);
    for stage in [
        Stage::Verify,
        Stage::AiReview,
        Stage::CodeReview,
        Stage::ManualTest,
    ] {
        let f = feature("b", vec![repo("api", true)], done(stage));
        let f = in_round(f, stage, FixPhase::Planning);
        assert_eq!(turned_back(&f, at_rest), Some(stage));
    }
    let f = feature("b", vec![repo("api", true)], 5);
    let early = in_round(f.clone(), Stage::Build, FixPhase::Planning);
    assert_eq!(turned_back(&early, at_rest), None);
    let mut closed = in_round(f, Stage::AiReview, FixPhase::Planning);
    closed.status = Status::Abandoned;
    assert_eq!(turned_back(&closed, at_rest), None);
}

#[test]
fn manual_tests_fixes_draw_the_line_back_into_verify() {
    use NodeMark::{Current, Done, Later, Paused, Ran};
    let at_rest = RowCtx::default();
    let verify = stage_index(Stage::Verify);
    // At Manual test, its fixes made: no line yet.
    let mut f = feature("b", vec![repo("api", true)], stage_index(Stage::ManualTest));
    assert_eq!(rail_back(&f, at_rest), None);
    // Marked passed, it goes back to Verify as `back_to_verify` does.
    assert!(f.untick(Stage::Verify, 3_000));
    // Its results make it a stage that ran; the fixture has none.
    f.ran_once.insert(Stage::ManualTest, 3_000);
    f.back_from = Some(Stage::ManualTest);
    let back = rail_back(&f, at_rest);
    assert_eq!(
        back,
        Some(RailBack {
            from: stage_index(Stage::ManualTest),
            to: verify,
        })
    );
    assert_eq!(
        rail_marks(&f, at_rest),
        [Done, Done, Done, Done, Current, Ran, Ran, Paused, Later]
    );
    // Work sent back to Build from there draws its own line.
    let round = in_round(f.clone(), Stage::Verify, FixPhase::Planning);
    assert_eq!(
        rail_back(&round, at_rest).map(|b| b.to),
        Some(stage_index(Stage::Build))
    );
    // Verify passes: the line follows the flow on its way back.
    f.tick(3_100, "you", Some("p1"));
    assert_eq!(f.back_from, Some(Stage::ManualTest));
    assert_eq!(
        rail_back(&f, at_rest).map(|b| b.to),
        Some(stage_index(Stage::AiReview))
    );
    // Back at Manual test: no line, and it is marked done once more.
    f.tick(3_200, "you", Some("p1"));
    f.tick(3_300, "you", Some("p1"));
    assert_eq!(f.current(), Some(Stage::ManualTest));
    assert_eq!(rail_back(&f, at_rest), None);
    assert_eq!(f.back_from, Some(Stage::ManualTest));
    f.tick(3_400, "you", Some("p1"));
    assert_eq!(f.back_from, None);
}

#[test]
fn code_changed_waits_for_the_round_to_be_over() {
    let mut f = feature("b", vec![repo("api", true)], 6);
    for mark in f.marks.values_mut() {
        mark.patch = Some("p0".into());
    }
    // Done stages on other code than the probe read: stale, at rest.
    let p = probe(None);
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let node = rail_node(&f, Stage::Verify, &state, 2_000);
    assert_eq!(node.mark, NodeMark::Stale);
    // In a round it's what's expected, and only ran once.
    let f = in_round(f, Stage::CodeReview, FixPhase::Fixing);
    let node = rail_node(&f, Stage::Verify, &state, 2_000);
    assert_eq!(node.mark, NodeMark::Ran);
    assert_ne!(node.line, "code changed");
}

/// The buttons at the title of Verify's page, not those on its cards.
fn verify_title_buttons(f: &Feature, ctx: RowCtx) -> Vec<String> {
    (page_actions(f, Stage::Verify, ctx).into_iter())
        .filter(|(_, o)| outcome_check(o).is_none())
        .map(|(button, _)| button.label)
        .collect()
}

#[test]
fn verify_offers_what_works_for_how_its_checks_stand() {
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    // Before a run.
    let before = feature("v", vec![repo("api", true)], 4);
    assert_eq!(
        verify_title_buttons(&before, ctx),
        ["Run checks", "Set up", "Commands\u{2026}"]
    );
    // Starting, or waiting for a setup: nothing to stop yet.
    let starting = RowCtx {
        checks_busy: true,
        ..ctx
    };
    assert_eq!(
        verify_title_buttons(&before, starting),
        ["Commands\u{2026}"]
    );
    // Running: Stop all first, and what only reads.
    let running = verified(true);
    assert_eq!(
        verify_title_buttons(&running, ctx),
        ["Stop all", "Evidence", "Commands\u{2026}"]
    );
    let stop = page_actions(&running, Stage::Verify, ctx).remove(0);
    assert_eq!(stop.1, PanelOutcome::StopChecks);
    assert_eq!(stop.0.style, ButtonStyle::Plain);
    // Ended, failed.
    let mut failed = verified(false);
    failed.verify.as_mut().unwrap().passed = Some(false);
    assert_eq!(
        verify_title_buttons(&failed, ctx),
        [
            "Make fixes",
            "Plan fixes",
            "Run failed",
            "Run checks",
            "Start over\u{2026}",
            "Evidence",
            "Set up",
            "Commands\u{2026}"
        ]
    );
}

#[test]
fn verify_cards_offer_stop_only_on_a_check_still_running() {
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let on_cards = |f: &Feature, ctx| -> Vec<PanelOutcome> {
        (page_actions(f, Stage::Verify, ctx).into_iter())
            .filter(|(_, o)| outcome_check(o).is_some())
            .map(|(_, o)| o)
            .collect()
    };
    assert_eq!(
        on_cards(&verified(true), ctx),
        [
            PanelOutcome::ShowCheck(0),
            PanelOutcome::ShowCheck(1),
            PanelOutcome::StopCheck(1)
        ]
    );
    let starting = RowCtx {
        checks_busy: true,
        ..ctx
    };
    let mut ended = verified(false);
    ended.verify.as_mut().unwrap().passed = Some(false);
    assert_eq!(
        on_cards(&ended, starting),
        [PanelOutcome::ShowCheck(0), PanelOutcome::ShowCheck(1)]
    );
    assert!(on_cards(&ended, ctx).contains(&PanelOutcome::RerunCheck(0)));
}

#[test]
fn a_check_told_to_stop_says_so_and_offers_no_stop_again() {
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let offered = |f: &Feature, o: PanelOutcome| {
        (page_actions(f, Stage::Verify, ctx).iter()).any(|(_, x)| *x == o)
    };
    let mut f = verified(true);
    assert!(offered(&f, PanelOutcome::StopCheck(1)));
    assert!(offered(&f, PanelOutcome::StopChecks));
    f.verify.as_mut().unwrap().checks[1].stopping = true;
    // The only one running: nothing is left to stop.
    assert!(!offered(&f, PanelOutcome::StopCheck(1)));
    assert!(!offered(&f, PanelOutcome::StopChecks));
    assert!(offered(&f, PanelOutcome::ShowCheck(1)));

    let entries = check_entries(&f, Stage::Verify, &[]);
    let card = check_card(&entries[1], &f, &PanelState::default(), &[], 2_000);
    assert_eq!(card.tag, FindingTag::Running);
    assert_eq!(card.state, "stopping");

    // Another still going: Stop all stays for it.
    f.verify.as_mut().unwrap().checks[0].outcome = None;
    assert!(offered(&f, PanelOutcome::StopCheck(0)));
    assert!(offered(&f, PanelOutcome::StopChecks));

    // Neither its key nor the hints offer it again.
    let slugs = vec!["v".to_owned()];
    f.verify.as_mut().unwrap().checks[0].outcome = verified(false)
        .verify
        .unwrap()
        .checks
        .swap_remove(0)
        .outcome;
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        finding: 1,
        ..FeaturePanel::default()
    };
    let (mods, sym) = key(Keysym::s);
    assert_eq!(panel.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[]), None);
    let keys: Vec<String> = (tab_hints(
        &panel,
        &f,
        Stage::Verify,
        &slugs,
        ctx,
        AgentChoices::default(),
        &[],
        &[],
        &[],
    )
    .into_iter())
    .map(|(key, _)| key)
    .collect();
    assert!(!keys.iter().any(|k| k == "s" || k == "Shift+S"), "{keys:?}");
}

#[test]
fn stopping_is_not_saved() {
    let mut f = verified(true);
    f.verify.as_mut().unwrap().checks[1].stopping = true;
    let text = serde_json::to_string(&f).unwrap();
    let back: Feature = serde_json::from_str(&text).unwrap();
    assert!(!back.verify.unwrap().checks[1].stopping);
}

#[test]
fn enter_never_stops_the_checks_nor_marks_verify_done_under_them() {
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let running = verified(true);
    assert_eq!(
        page_enter(&running, Stage::Verify, ctx),
        Some(PanelOutcome::RunChecks)
    );
    // Its checks edited away during the run: still not Mark done.
    let none = RowCtx::default();
    assert_eq!(
        page_enter(&running, Stage::Verify, none),
        Some(PanelOutcome::RunChecks)
    );
}

#[test]
fn work_that_changes_the_code_waits_for_the_checks() {
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let offered = |f: &Feature, o: PanelOutcome| {
        (page_actions(f, Stage::Verify, ctx).iter()).any(|(_, x)| *x == o)
    };
    let mut running = in_round(verified(true), Stage::Verify, FixPhase::Ready);
    assert!(!offered(&running, PanelOutcome::MakeFixes));
    assert!(offered(&running, PanelOutcome::ReviewFixPlan));
    running.fix_round = None;
    running.last_fix_round =
        in_round(verified(true), Stage::Verify, FixPhase::Fixing).fix_round;
    assert!(!offered(&running, PanelOutcome::MakeFixes));
    assert!(offered(&running, PanelOutcome::ReviewFixPlan));
    // On Build's page too: a setup would change the worktrees under them.
    let build = |f: &Feature, ctx| {
        (page_actions(f, Stage::Build, ctx).iter()).any(|(_, x)| *x == PanelOutcome::SetUp)
    };
    assert!(!build(&running, ctx));
    let idle = feature("v", vec![repo("api", true)], 4);
    assert!(build(&idle, ctx));
    let setting_up = RowCtx {
        setting_up: true,
        ..ctx
    };
    assert!(!build(&idle, setting_up));
}

#[test]
fn start_over_goes_while_work_of_the_feature_runs() {
    let mut failed = verified(false);
    failed.verify.as_mut().unwrap().passed = Some(false);
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    assert_eq!(
        start_over_stage(&failed, Stage::Verify, ctx),
        Some(Stage::Verify)
    );
    let busy = RowCtx {
        feature_busy: true,
        ..ctx
    };
    assert_eq!(start_over_stage(&failed, Stage::Verify, busy), None);
}

#[test]
fn verify_hints_match_its_buttons_while_checks_run() {
    let slugs = vec!["v".to_owned()];
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let keys = |f: &Feature, ctx: RowCtx| -> Vec<String> {
        let panel = FeaturePanel::default();
        (tab_hints(
            &panel,
            f,
            Stage::Verify,
            &slugs,
            ctx,
            AgentChoices::default(),
            &[],
            &[],
            &[],
        )
        .into_iter())
        .map(|(key, _)| key)
        .collect()
    };
    let running = keys(&verified(true), ctx);
    assert!(!running.iter().any(|k| k == "Enter"), "{running:?}");
    assert!(!running.iter().any(|k| k == "Ctrl+Shift+U"), "{running:?}");
    assert!(running.iter().any(|k| k == "Shift+S"), "{running:?}");
    let before = keys(&feature("v", vec![repo("api", true)], 4), ctx);
    assert!(before.iter().any(|k| k == "Enter"), "{before:?}");
    assert!(before.iter().any(|k| k == "Ctrl+Shift+U"), "{before:?}");
    assert!(!before.iter().any(|k| k == "Shift+S"), "{before:?}");
}

#[test]
fn s_stops_a_check_still_running_only() {
    let f = verified(true);
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let slugs = vec!["v".to_owned()];
    let plan = [PlannedCheck {
        repo: "api".into(),
        command: "lint".into(),
        skipped: false,
        own: true,
    }];
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    let (mods, sym) = key(Keysym::s);
    // Its first check ended, its second runs.
    panel.finding = 0;
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, ctx, &plan, &[], &[]),
        None
    );
    panel.finding = 1;
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, ctx, &plan, &[], &[]),
        Some(PanelOutcome::StopCheck(1))
    );
}

#[test]
fn a_failed_verify_waits_for_its_fixes_on_enter() {
    let mut f = verified(false);
    // `verified` ran to its end, a check failing.
    f.verify.as_mut().unwrap().finished = Some(1_100);
    f.verify.as_mut().unwrap().passed = Some(false);
    let ctx = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    // Enter plans them: the plan shows what is to be fixed first.
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::PlanFixes)
    );
    // Ctrl+Enter makes them too, Ctrl+Shift+Enter plans them, from the
    // stages and the check cards alike.
    let slugs = vec![f.slug.clone()];
    let plan = [planned("lint", false, true)];
    for focus in [TabArea::Stages, TabArea::Findings] {
        let mut panel = FeaturePanel {
            slug: f.slug.clone(),
            stage: Some(Stage::Verify),
            focus,
            ..FeaturePanel::default()
        };
        let ctrl = |shift| ModifiersState {
            ctrl: true,
            shift,
            ..ModifiersState::default()
        };
        let press = |panel: &mut FeaturePanel, shift| {
            let mods = ctrl(shift);
            panel.key(&mods, Keysym::Return, &f, &slugs, ctx, &plan, &[], &[])
        };
        let made = press(&mut panel, false);
        assert_eq!(made, Some(PanelOutcome::MakeVerifyFixes));
        assert_eq!(press(&mut panel, true), Some(PanelOutcome::PlanFixes));
    }
    let plain = page_actions(&f, Stage::Verify, ctx);
    let run = plain.iter().find(|(_, o)| *o == PanelOutcome::RunChecks);
    assert_eq!(run.map(|(b, _)| b.style), Some(ButtonStyle::Plain));
    // A check cut short says nothing of the code: run them again first.
    let cut = (f.verify.as_mut().unwrap().checks.iter_mut())
        .find_map(|c| c.outcome.as_mut())
        .unwrap();
    cut.interrupted = true;
    assert_eq!(
        page_enter(&f, Stage::Verify, ctx),
        Some(PanelOutcome::RunChecks)
    );
}

#[test]
fn a_double_click_on_the_bar_presses_no_other_button() {
    let t = Instant::now();
    let at: Point<f64, Logical> = (900.0, 200.0).into();
    let last = (t, at, "Fix".to_owned());
    let soon = t + Duration::from_millis(150);
    // The button changed under the pointer: the second press is not it.
    assert!(other_button_again(&last, soon, at, "Review in diff"));
    // The same button again, a late press, a press elsewhere: all taken.
    assert!(!other_button_again(&last, soon, at, "Fix"));
    let late = t + crate::input::MULTI_CLICK;
    assert!(!other_button_again(&last, late, at, "Review in diff"));
    let off: Point<f64, Logical> = (800.0, 200.0).into();
    assert!(!other_button_again(&last, soon, off, "Add comment\u{2026}"));
}

#[test]
fn a_fix_round_shows_on_the_page_that_asked_and_takes_enter() {
    let mut f = verified(false);
    f.verify.as_mut().unwrap().finished = Some(1_100);
    f.verify.as_mut().unwrap().passed = Some(false);
    let labels_of = |f: &Feature| -> Vec<String> {
        (page_actions(f, Stage::Verify, RowCtx::default()).into_iter())
            .map(|(b, _)| b.label)
            .collect()
    };
    let ready = in_round(f.clone(), Stage::Verify, FixPhase::Ready);
    assert_eq!(
        page_enter(&ready, Stage::Verify, RowCtx::default()),
        Some(PanelOutcome::ReviewFixPlan)
    );
    let shown = labels_of(&ready);
    for label in ["Review", "Make fixes", "Drop plan\u{2026}"] {
        assert!(shown.contains(&label.to_owned()), "{label}: {shown:?}");
    }
    // Finish or drop it before starting the stage over.
    assert!(
        !shown.contains(&"Start over\u{2026}".to_owned()),
        "{shown:?}"
    );
    assert_eq!(
        start_over_stage(&ready, Stage::Verify, RowCtx::default()),
        None
    );
    assert!(!shown.contains(&"Plan fixes".to_owned()), "{shown:?}");
    let actions = page_actions(&ready, Stage::Verify, RowCtx::default());
    let state = PanelState::default();
    let page = stage_page(&ready, Stage::Verify, &state, &actions, 2_000);
    let Some(PageSection::Round(card)) = page.sections.first() else {
        panic!("the round's card first: {:?}", page.sections);
    };
    assert_eq!(card.label, "Fix round 1");
    assert_eq!(card.state.text, "Verify failures \u{b7} fix plan ready");
    assert!(card.meta.ends_with("plan written 15m ago"), "{}", card.meta);
    assert_eq!(card.state.tone, CheckTone::Blocker);
    let held: Vec<&str> = (card.buttons)
        .iter()
        .map(|i| actions[*i].0.label.as_str())
        .collect();
    assert_eq!(held, ["Review", "Make fixes", "Drop plan\u{2026}"]);
    // Review, the round's primary, is the page's one: on its bar, filled,
    // and still on its card.
    let primary = page.primary.expect("the round's");
    assert_eq!(actions[primary].0.label, "Review");
    assert!(card.buttons.contains(&primary));
    let filled = |b: &&PageButton| b.style == ButtonStyle::Primary;
    assert_eq!(page.buttons.iter().filter(filled).count(), 1);

    let planning = in_round(f.clone(), Stage::Verify, FixPhase::Planning);
    let shown = labels_of(&planning);
    assert!(!shown.contains(&"Make fixes".to_owned()), "{shown:?}");
    // No plan yet: what drops is the round.
    assert!(
        shown.contains(&"Drop fix round\u{2026}".to_owned()),
        "{shown:?}"
    );
    // A fresh agent held: its prompt goes first, and only one is primary.
    let mut held = in_round(f.clone(), Stage::Verify, FixPhase::Planning);
    held.runs.insert(
        Stage::Build,
        feature::agents::StageRun {
            pane: Some("p1".into()),
            held: true,
            ..Default::default()
        },
    );
    let actions = page_actions(&held, Stage::Verify, RowCtx::default());
    let primary: Vec<&str> = (actions.iter())
        .filter(|(b, _)| b.style == ButtonStyle::Primary)
        .map(|(b, _)| b.label.as_str())
        .collect();
    assert_eq!(primary, ["Send prompt"]);
    assert_eq!(
        page_enter(&held, Stage::Verify, RowCtx::default()),
        Some(PanelOutcome::SendPrompt(Stage::Build))
    );
    let sends = (actions.iter())
        .filter(|(_, o)| *o == PanelOutcome::SendPrompt(Stage::Build))
        .count();
    assert_eq!(sends, 1);
    // Stopped without a plan: ask again, or drop it.
    let mut stalled = planning.clone();
    stalled.fix_round.as_mut().unwrap().stalled = Some("it stopped".into());
    assert_eq!(
        page_enter(&stalled, Stage::Verify, RowCtx::default()),
        Some(PanelOutcome::AskFixPlan)
    );
    // Making the fixes: nothing left to drop.
    let fixing = in_round(f.clone(), Stage::Verify, FixPhase::Fixing);
    let shown = labels_of(&fixing);
    assert!(
        !shown.contains(&"Drop plan\u{2026}".to_owned()),
        "{shown:?}"
    );
    assert!(!shown.contains(&"Make fixes".to_owned()), "{shown:?}");
    // Its agent stopped with fixes left: the user can send it again, or
    // drop the plan, also on the stage its commits sent the feature to.
    let mut stopped = fixing.clone();
    let round = stopped.fix_round.as_mut().unwrap();
    round.stalled = Some("it stopped".into());
    let shown = labels_of(&stopped);
    for label in ["Make fixes again", "Drop plan\u{2026}"] {
        assert!(shown.contains(&label.to_owned()), "{label}: {shown:?}");
    }
    let mut back = stopped.clone();
    back.fix_round.as_mut().unwrap().stage = Stage::Ship;
    let at = back.current().unwrap();
    assert!(fix_round_on(&back, at).is_some());
    // No other stage's page has it.
    let other = page_actions(&ready, Stage::Build, RowCtx::default());
    assert!(!other.iter().any(|(_, o)| *o == PanelOutcome::MakeFixes));
}

#[test]
fn the_last_round_says_which_it_was_what_it_made_and_when() {
    let fixing = in_round(verified(false), Stage::CodeReview, FixPhase::Fixing);
    let mut f = fixing.clone();
    let mut round = fixing.fix_round.clone().unwrap();
    f.fix_round = None;
    f.fix_asked.insert(round.n, Stage::CodeReview);
    round.what = "review comments".into();
    round.items = vec!["One".into(), "Two".into()];
    round.done = 2;
    round.ready = Some(1_000);
    round.ended = Some(1_900);
    let PageSection::File { name, state, .. } = last_round_card(&round, &[], 2_000) else {
        panic!("a file card");
    };
    assert_eq!(name, format!("Last fix plan: Fix round {}", round.n));
    assert_eq!(
        state.text,
        "review comments \u{b7} 2 of 2 made \u{b7} written 16m ago \u{b7} finished 1m ago"
    );
    // One kept from before it was timed says what it knows.
    round.ended = None;
    round.ready = None;
    round.items.clear();
    let PageSection::File { state, .. } = last_round_card(&round, &[], 2_000) else {
        panic!("a file card");
    };
    assert_eq!(state.text, "review comments \u{b7} fixes made");
}

#[test]
fn a_fix_plan_stays_on_the_page_once_its_fixes_went() {
    let mut f = verified(false);
    f.verify.as_mut().unwrap().finished = Some(1_100);
    f.verify.as_mut().unwrap().passed = Some(false);
    let held = |f: &Feature, stage| {
        let actions = page_actions(f, stage, RowCtx::default());
        let page = stage_page(f, stage, &PanelState::default(), &actions, 2_000);
        let card = page.sections.into_iter().find_map(|s| match s {
            PageSection::File {
                name,
                state,
                buttons,
            } if name.starts_with("Fix plan: ") || name.starts_with("Last fix plan: ") => {
                Some((state.text, buttons))
            }
            PageSection::Round(card) => Some((card.state.text, card.buttons)),
            _ => None,
        });
        card.map(|(said, buttons)| {
            let labels: Vec<String> = buttons
                .iter()
                .map(|i| actions[*i].0.label.clone())
                .collect();
            (said, labels)
        })
    };
    // About to make them: the plan can be read, not dropped any more.
    let compacting = in_round(f.clone(), Stage::Verify, FixPhase::CompactingToFix);
    assert_eq!(
        held(&compacting, Stage::Verify),
        Some((
            "Verify failures \u{b7} making fixes".to_owned(),
            vec![OPEN_PLAN.to_owned()]
        ))
    );
    // Making them: the plan can be read.
    let fixing = in_round(f.clone(), Stage::Verify, FixPhase::Fixing);
    assert_eq!(
        held(&fixing, Stage::Verify),
        Some((
            "Verify failures \u{b7} making fixes".to_owned(),
            vec![OPEN_PLAN.to_owned()]
        ))
    );
    // Every fix ticked: read. With failures still to send, those go, not
    // the old round.
    let before = page_enter(&f, Stage::Verify, RowCtx::default());
    let mut over = f.clone();
    over.last_fix_round = fixing.fix_round.clone();
    assert_eq!(
        held(&over, Stage::Verify),
        Some(("fixes made".to_owned(), vec![OPEN_PLAN.to_owned()]))
    );
    // With nothing new it can go again, by its name, if one turned out
    // not made.
    let mut quiet = over.clone();
    quiet.verify.as_mut().unwrap().passed = Some(true);
    let (_, labels) = held(&quiet, Stage::Verify).unwrap();
    assert_eq!(labels, [OPEN_PLAN, "Send fix round 1 again"]);
    // Enter still does what the stage waits for.
    assert_eq!(page_enter(&over, Stage::Verify, RowCtx::default()), before);
    assert_ne!(
        page_enter(&quiet, Stage::Verify, RowCtx::default()),
        Some(PanelOutcome::ResendFixes)
    );
    // No key sends it again.
    let slugs = vec![quiet.slug.clone()];
    let mut panel = FeaturePanel {
        slug: over.slug.clone(),
        ..FeaturePanel::default()
    };
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    let none = RowCtx::default();
    for f in [&over, &quiet] {
        let pressed = panel.key(&ctrl, Keysym::Return, f, &slugs, none, &[], &[], &[]);
        assert!(
            !matches!(
                pressed,
                Some(PanelOutcome::MakeFixes | PanelOutcome::ResendFixes)
            ),
            "{pressed:?}"
        );
        let hints = tab_view(&panel, f, &slugs, &PanelState::default(), 2_000).hints;
        let again = hints.iter().any(|(_, said)| said.contains("again"));
        assert!(!again, "{hints:?}");
    }
    // Past its stage it is only read.
    let at = over.current().unwrap();
    let mut past = over.clone();
    past.last_fix_round.as_mut().unwrap().stage = Stage::Build;
    assert!(at > Stage::Build);
    assert_eq!(
        held(&past, Stage::Build),
        Some(("fixes made".to_owned(), vec![OPEN_PLAN.to_owned()]))
    );
    assert_eq!(held(&past, at), None);
    // Back on a stage before the one that asked, it comes along.
    let mut back = quiet.clone();
    back.last_fix_round.as_mut().unwrap().stage = Stage::Ship;
    assert!(sends_round_again(&back, at, none));
    assert!(!sends_round_again(&over, at, none), "failures to send");
    // A round open has the card to itself.
    let mut both = fixing.clone();
    both.last_fix_round = fixing.fix_round.clone();
    assert!(last_round_on(&both, Stage::Verify).is_none());
}

#[test]
fn verifys_page_holds_a_card_per_check() {
    let labels = |f: &Feature| -> Vec<String> {
        let actions = page_actions(f, Stage::Verify, RowCtx::default());
        actions.into_iter().map(|(b, _)| b.label).collect()
    };
    let f = verified(false);
    let shown = labels(&f);
    assert!(shown.contains(&"Run failed".to_owned()), "{shown:?}");
    assert!(!shown.contains(&"Stop all".to_owned()), "{shown:?}");
    let state = PanelState::default();
    let actions = page_actions(&f, Stage::Verify, RowCtx::default());
    let entries = check_entries(&f, Stage::Verify, &[]);
    let (passed, failed) = (
        check_card(&entries[0], &f, &state, &actions, 2_000),
        check_card(&entries[1], &f, &state, &actions, 2_000),
    );
    assert_eq!(passed.tag, FindingTag::Passed);
    assert_eq!(passed.preview, ["first", "boom"]);
    assert_eq!(failed.tag, FindingTag::Failed);
    assert_eq!(failed.place, "api \u{b7} exited with 1");
    assert_eq!(failed.preview, ["first", "boom"]);
    let held: Vec<&str> = (failed.buttons.iter())
        .map(|i| actions[*i].0.label.as_str())
        .collect();
    assert_eq!(held, ["Show", "Run again"]);

    let running = verified(true);
    let shown = labels(&running);
    assert!(shown.contains(&"Stop all".to_owned()), "{shown:?}");
    assert!(!shown.contains(&"Run failed".to_owned()), "{shown:?}");
    let actions = page_actions(&running, Stage::Verify, RowCtx::default());
    let entries = check_entries(&running, Stage::Verify, &[]);
    let card = check_card(&entries[1], &running, &state, &actions, 2_000);
    assert_eq!(card.tag, FindingTag::Running);
    let held: Vec<&str> = (card.buttons.iter())
        .map(|i| actions[*i].0.label.as_str())
        .collect();
    assert_eq!(held, ["Show", "Stop"]);
    // A later stage's page has none.
    assert!(check_rows(&f, Stage::Build).is_empty());
}

#[test]
fn verify_runs_its_checks_when_it_has_some_else_is_marked() {
    let f = feature("v", vec![repo("api", true)], 4);
    let verify = PanelRow::Stage(Stage::Verify);
    let checks = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    assert_eq!(
        row_action(&f, verify, checks),
        Some(PanelOutcome::RunChecks)
    );
    assert_eq!(row_hint(&f, verify, checks), "run checks");
    assert_eq!(stage_button(&f, Stage::Verify, checks), Some("Run checks"));
    let mut panel = FeaturePanel {
        ..FeaturePanel::default()
    };
    let slugs = vec!["v".to_owned()];
    let (mods, sym) = key(Keysym::Return);
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, checks, &[], &[], &[]),
        Some(PanelOutcome::RunChecks)
    );
    let none = RowCtx::default();
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, none, &[], &[], &[]),
        Some(PanelOutcome::Tick)
    );
    // Later stages aren't run early.
    let early = feature("e", vec![repo("api", true)], 3);
    assert_eq!(
        row_action(&early, verify, checks),
        Some(PanelOutcome::NotYet(Stage::Build))
    );
}

#[test]
fn evidence_opens_from_the_verify_row_once_a_run_wrote_it() {
    let mut f = feature("v", vec![repo("api", true)], 4);
    let mut panel = FeaturePanel {
        ..FeaturePanel::default()
    };
    let slugs = vec!["v".to_owned()];
    let shift_ctrl = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    let none = RowCtx::default();
    let hinted = |panel: &FeaturePanel, f: &Feature| {
        tab_view(panel, f, &slugs, &PanelState::default(), 2_000)
            .hints
            .contains(&("Ctrl+Shift+O".into(), "evidence".into()))
    };
    // No run yet: nothing to open, no hint.
    assert_eq!(
        panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[], &[], &[]),
        None
    );
    assert!(!hinted(&panel, &f));
    // The first run, still running: nothing written yet.
    f.verify = Some(feature::verify::VerifyRun {
        n: 1,
        started: 1_000,
        ..Default::default()
    });
    assert_eq!(
        panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[], &[], &[]),
        None
    );
    // Ended: there.
    f.verify.as_mut().unwrap().finished = Some(1_100);
    assert_eq!(
        panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[], &[], &[]),
        Some(PanelOutcome::Evidence)
    );
    assert!(hinted(&panel, &f));
    // Another page: not offered.
    panel.pick(&f, Stage::Build);
    assert_eq!(
        panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[], &[], &[]),
        None
    );
    assert!(!hinted(&panel, &f));
}

#[test]
fn builds_row_says_setup_first_then_its_agent() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let agent = Some("Claude Code working");
    assert_eq!(build_line(&f, None, agent).as_deref(), agent);
    assert_eq!(build_line(&f, None, None), None);
    assert_eq!(
        build_line(&f, Some("services-api: npm ci (2 of 3)"), agent).as_deref(),
        Some("setting up: services-api: npm ci (2 of 3)")
    );
    let own = format!("Setting up \u{201c}{}\u{201d}", f.title);
    assert_eq!(
        build_line(&f, Some(&own), agent).as_deref(),
        Some("setting up")
    );
    let step = format!("{own}: api: npm ci (1 of 2)");
    assert_eq!(
        build_line(&f, Some(&step), agent).as_deref(),
        Some("setting up: api: npm ci (1 of 2)")
    );
    f.setup = Some(feature::setup::SetupRun {
        started: 1_000,
        finished: Some(1_100),
        failed: Some("api: `npm ci` exited with 1".into()),
        ..Default::default()
    });
    assert_eq!(
        build_line(&f, None, None).as_deref(),
        Some("setup failed: api: `npm ci` exited with 1")
    );
    // An agent started anyway: what it waits for comes first.
    let stopped = Some("stopped after milestone 1 of 3 \u{b7} Enter to continue");
    assert_eq!(
        build_line(&f, None, stopped).as_deref(),
        Some("stopped after milestone 1 of 3 \u{b7} Enter to continue \u{b7} setup failed")
    );
    // On Build only; a done Build says when.
    let state = PanelState::default();
    assert_eq!(
        stage_detail(&f, Stage::Build, &state, 2_000),
        "setup failed: api: `npm ci` exited with 1"
    );
    let done = feature("d", vec![repo("api", true)], 4);
    assert_eq!(
        stage_detail(&done, Stage::Build, &state, 2_060),
        "done 1m ago by you"
    );
}

#[test]
fn verifys_row_says_what_runs_failed_or_that_it_has_no_checks() {
    let mut f = feature("v", vec![repo("api", true)], 4);
    let checks = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let none = RowCtx::default();
    assert_eq!(
        verify_line(&f, none, None).as_deref(),
        Some("no checks for its repositories: set some, or Mark done by hand")
    );
    assert_eq!(verify_line(&f, checks, None), None);
    assert_eq!(
        verify_line(&f, checks, Some("api: npm test (2 of 5)")).as_deref(),
        Some("api: npm test (2 of 5)")
    );
    let failed = || feature::verify::VerifyRun {
        n: 2,
        started: 1_000,
        finished: Some(1_100),
        passed: Some(false),
        failures: vec!["api: `npm test` exited with 1".into(), "web: lint".into()],
        ..Default::default()
    };
    f.verify = Some(failed());
    assert_eq!(
        verify_line(&f, checks, None).as_deref(),
        Some("failed: api: `npm test` exited with 1")
    );
    // Passed: the stage's own words.
    f.verify = Some(feature::verify::VerifyRun {
        passed: Some(true),
        ..failed()
    });
    assert_eq!(verify_line(&f, checks, None), None);
    let state = PanelState {
        ctx: checks,
        ..PanelState::default()
    };
    assert_eq!(
        stage_detail(&f, Stage::Verify, &state, 2_000),
        Stage::Verify.about()
    );
    assert_eq!(stage_button(&f, Stage::Verify, checks), Some("Run checks"));
}

#[test]
fn the_strip_says_what_runs_its_steps_and_for_how_long() {
    let busy = |job: Job, what: &str| Busy {
        project: "acme".into(),
        slug: "b".into(),
        job,
        repo: None,
        what: what.into(),
    };
    let now = Instant::now();
    let ago = |ms: u64| now - Duration::from_millis(ms);
    let setup = busy(
        Job::Setup,
        "Setting up \u{201c}Bulk\u{201d}: api: npm ci (2 of 3)",
    );
    let verify = busy(Job::Verify, "Verifying \u{201c}Bulk\u{201d}: 1 of 4 done");
    let graph = busy(
        Job::Graph,
        "Building the code graph of \u{201c}Bulk\u{201d}",
    );
    let read = busy(Job::Read, "Reading \u{201c}Bulk\u{201d}");
    let sandbox = busy(
        Job::Sandbox,
        "Sandbox of \u{201c}Bulk\u{201d}: Making its network",
    );
    let ship = busy(Job::Ship, "Shipping \u{201c}Bulk\u{201d}");
    let all = [
        (&setup, ago(61_000)),
        (&verify, ago(5_000)),
        (&graph, ago(400)),
        // Too new to show: a quick read doesn't flash.
        (&read, ago(100)),
        (&sandbox, ago(2_000)),
        (&ship, ago(1_000)),
    ];
    let (rows, more) = running_rows(&all, "Bulk", now);
    assert_eq!(more, 2, "sandbox and ship are counted, not shown");
    let row = |i: usize| &rows[i].0;
    assert_eq!(
        rows.iter().map(|(_, j)| *j).collect::<Vec<_>>(),
        [Job::Setup, Job::Verify, Job::Graph]
    );
    assert_eq!(
        (row(0).word, row(0).what.as_str(), row(0).steps),
        ("setting up", "api: npm ci", Some((2, 3)))
    );
    assert_eq!(row(0).took, "1:01");
    assert_eq!(row(0).buttons, ["Log"]);
    assert_eq!(row(0).node, Some(3), "setup shows on Build's page");
    assert_eq!((row(1).what.as_str(), row(1).steps), ("", Some((1, 4))));
    assert_eq!(
        (row(1).buttons.as_slice(), row(1).node),
        (&["Stop"][..], Some(4))
    );
    assert_eq!(
        (row(2).word, row(2).what.as_str(), row(2).steps, row(2).node),
        ("code graph", "", None, None)
    );
    let (rows, _) = running_rows(&[(&sandbox, ago(2_000))], "Bulk", now);
    assert_eq!(rows[0].0.what, "Making its network");
    assert_eq!(rows[0].0.node, Some(7));
    // Counts it can't read stay text.
    assert_eq!(steps_of("2 of the files"), ("2 of the files".into(), None));
    assert_eq!(
        steps_of("web: pnpm i (3 of 3)"),
        ("web: pnpm i".into(), Some((3, 3)))
    );
}

#[test]
fn setup_and_checks_tasks_say_so_and_name_their_feature() {
    let busy = |job: Job, what: &str| Busy {
        project: "acme".into(),
        slug: "b".into(),
        job,
        repo: None,
        what: what.into(),
    };
    let task = task_of(
        &busy(Job::Setup, "services-api: npm ci (2 of 3)"),
        Some("Bulk"),
        "Acme",
    );
    assert_eq!(task.chip, "setup");
    assert_eq!(task.title, "services-api: npm ci (2 of 3)");
    assert_eq!(task.detail, "\u{201c}Bulk\u{201d} \u{b7} Acme");
    let task = task_of(
        &busy(Job::Verify, "Running the checks of \u{201c}Bulk\u{201d}"),
        Some("Bulk"),
        "Acme",
    );
    assert_eq!(task.chip, "checks");
    assert_eq!(task.detail, "Acme");
    let task = task_of(
        &busy(
            Job::Graph,
            "Building the code graph of \u{201c}Bulk\u{201d}",
        ),
        Some("Bulk"),
        "Acme",
    );
    assert_eq!(task.chip, "graph");
    assert_eq!(task.detail, "Acme");
    let task = task_of(
        &busy(Job::Create, "Creating \u{201c}Bulk\u{201d}"),
        None,
        "Acme",
    );
    assert_eq!(task.chip, "feature");
    assert_eq!(task.detail, "Acme");
}

#[test]
fn inline_lowers_only_a_capitalised_word() {
    assert_eq!(inline("Setting up"), "setting up");
    assert_eq!(inline("API: npm ci"), "API: npm ci");
    assert_eq!(inline("api: npm ci"), "api: npm ci");
    assert_eq!(inline(""), "");
}
