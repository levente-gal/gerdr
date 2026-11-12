//! Tests of Manual test's page and its criteria.

use super::*;

#[test]
fn manual_test_goes_by_its_criteria_when_the_brief_has_some() {
    let f = feature("m", vec![repo("api", true)], 7);
    assert_eq!(f.current(), Some(Stage::ManualTest));
    let row = PanelRow::Stage(Stage::ManualTest);
    // Without criteria it is marked done by hand, as before.
    let none = RowCtx::default();
    assert_eq!(row_action(&f, row, none), Some(PanelOutcome::Tick));
    assert_eq!(stage_button(&f, Stage::ManualTest, none), Some("Mark done"));
    let some = RowCtx {
        criteria: 3,
        ..RowCtx::default()
    };
    assert_eq!(row_action(&f, row, some), Some(PanelOutcome::ManualTest));
    assert_eq!(row_hint(&f, row, some), "try the criteria");
    // Their cards are the way in: no button leads there.
    assert_eq!(stage_button(&f, Stage::ManualTest, some), None);
    let state = PanelState {
        ctx: some,
        manual: Some("2 of 3 criteria passed \u{b7} 1 failed".into()),
        manual_gate: Some((2, 3)),
        ..PanelState::default()
    };
    assert_eq!(
        stage_detail(&f, Stage::ManualTest, &state, 2_060),
        "2 of 3 criteria passed \u{b7} 1 failed"
    );
    let node = rail_node(&f, Stage::ManualTest, &state, 2_060);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Waiting, "2 of 3 passed")
    );
    let actions = page_actions(&f, Stage::ManualTest, some);
    // Not next while one is left: greyed, last, where the gate goes.
    let last = actions.last().map(|(b, _)| (b.label.as_str(), b.style));
    assert_eq!(last, Some(("Mark passed", ButtonStyle::Off)));
}

#[test]
fn manual_tests_buttons_press_what_comes_next() {
    use crate::{feature::manual::Counts, manual_test::Outcome};
    let f = feature("m", vec![repo("api", true)], 7);
    let ctx = |passed, failed, sent, untested| RowCtx {
        criteria: 3,
        manual: Counts {
            passed,
            failed,
            sent,
            made: 0,
            untested,
            ..Default::default()
        },
        ..RowCtx::default()
    };
    let enter = |ctx| page_enter(&f, Stage::ManualTest, ctx);
    // Nothing tried and no sandbox running: Enter starts the sandbox
    // the criteria are tried in.
    assert_eq!(enter(ctx(0, 0, 0, 3)), Some(PanelOutcome::StartSandbox));
    // A failure not sent: its fixes come next, made at once or
    // through a plan.
    assert_eq!(
        enter(ctx(2, 1, 0, 0)),
        Some(PanelOutcome::Manual(Outcome::MakeFixes))
    );
    let labels = |ctx| -> Vec<String> {
        (page_actions(&f, Stage::ManualTest, ctx).into_iter())
            .map(|(b, _)| b.label)
            .collect()
    };
    let sends = labels(ctx(2, 1, 0, 0));
    assert_eq!(sends[..2], ["Make fixes", "Plan fixes"]);
    assert_eq!(sends.last().map(String::as_str), Some("Mark passed"));
    // All sent: nothing to send, back to the criteria.
    assert!(!labels(ctx(2, 1, 1, 0)).contains(&"Plan fixes".to_owned()));
    assert_eq!(enter(ctx(2, 1, 1, 0)), Some(PanelOutcome::ManualTest));
    // All passed: it can be marked.
    assert_eq!(
        enter(ctx(3, 0, 0, 0)),
        Some(PanelOutcome::Manual(Outcome::MarkPassed))
    );
}

#[test]
fn a_comment_is_done_or_not_and_its_keys_edit_and_delete_it() {
    use crate::{feature::manual::Counts, manual_test::Outcome};
    let f = feature("m", vec![repo("api", true)], 7);
    let comment = |found| Row {
        id: "C1".into(),
        title: "Make the settings page look better".into(),
        text: "Make the settings page look better\nCards, not a table".into(),
        found,
        note: String::new(),
        sent: false,
        made: false,
        earlier: false,
        retest: None,
        open: true,
        seen: false,
        shots: Vec::new(),
        comment: true,
        planning: false,
    };
    let buttons = |row: &Row| -> Vec<(String, ButtonStyle, PanelOutcome)> {
        (criterion_buttons(2, row).into_iter())
            .map(|(b, o)| (b.label, b.style, o))
            .collect()
    };
    let on = |o| PanelOutcome::Manual(o);
    let open = buttons(&comment(Some(Verdict::Fail)));
    assert_eq!(
        open,
        [
            (
                "Edit\u{2026}".into(),
                ButtonStyle::Quiet,
                on(Outcome::EditComment(2))
            ),
            (
                "Delete".into(),
                ButtonStyle::Quiet,
                on(Outcome::DeleteComment(2))
            ),
            ("Done".into(), ButtonStyle::Plain, on(Outcome::Toggle(2))),
            (
                "Not done".into(),
                ButtonStyle::Failed,
                on(Outcome::Fail(2, String::new()))
            ),
        ]
    );
    let done = buttons(&comment(Some(Verdict::Pass)));
    assert_eq!(done[2].1, ButtonStyle::Passed);
    // Its card: its lines as its steps, "open" until it goes to Build.
    let row = comment(Some(Verdict::Fail));
    let card = criterion_card(2, &row, &[], true);
    assert_eq!(card.state, "open");
    assert_eq!(
        card.sheet.as_ref().unwrap().steps,
        [
            ("", "Make the settings page look better".to_owned()),
            ("", "Cards, not a table".to_owned()),
        ]
    );
    let made = Row {
        sent: true,
        made: true,
        ..row
    };
    assert_eq!(
        criterion_card(2, &made, &[], true).state,
        "fix made: look again"
    );
    // Its keys: two criteria and the comment, open and not sent.
    let ctx = RowCtx {
        criteria: 3,
        manual: Counts {
            passed: 2,
            failed: 1,
            comments_open: 1,
            ..Counts::default()
        },
        ..RowCtx::default()
    };
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "m".into(),
        focus: TabArea::Findings,
        finding: 2,
        ..FeaturePanel::default()
    };
    let slugs = vec!["m".to_owned()];
    let go = |p: &mut FeaturePanel, mods: ModifiersState, sym| {
        p.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[])
    };
    let bare = ModifiersState::default();
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        go(&mut panel, bare, Keysym::f),
        Some(on(Outcome::Fail(2, String::new())))
    );
    assert_eq!(
        go(&mut panel, ctrl, Keysym::e),
        Some(on(Outcome::EditComment(2)))
    );
    // Del arms first.
    assert_eq!(go(&mut panel, bare, Keysym::Delete), None);
    assert_eq!(
        go(&mut panel, bare, Keysym::Delete),
        Some(on(Outcome::DeleteComment(2)))
    );
    assert_eq!(
        go(&mut panel, bare, Keysym::c),
        Some(PanelOutcome::AddComment)
    );
    // It is to send, and the user's own: Ctrl+Shift+Enter plans,
    // Ctrl+Enter says a plan comes first.
    assert_eq!(
        go(&mut panel, ctrl, Keysym::Return),
        Some(PanelOutcome::FindingRefused(PLAN_FIRST))
    );
    let shift = ModifiersState {
        shift: true,
        ..ctrl
    };
    assert_eq!(
        go(&mut panel, shift, Keysym::Return),
        Some(on(Outcome::SendFailures))
    );
    // On a criterion the keys are as ever.
    panel.finding = 0;
    assert_eq!(go(&mut panel, bare, Keysym::f), Some(on(Outcome::Write(0))));
    assert_eq!(
        go(&mut panel, bare, Keysym::Delete),
        Some(on(Outcome::Clear(0)))
    );
    assert_eq!(go(&mut panel, ctrl, Keysym::e), None);
}

/// Manual test's rows: one not tried, open; one passed, folded; one
/// failed, open; the feature at Manual test.
fn sheet_rows() -> (Feature, Vec<Row>, RowCtx) {
    let f = feature("m", vec![repo("api", true)], 7);
    let row = |id: &str, title: &str, text: &str, found, open| Row {
        id: id.into(),
        title: title.into(),
        text: text.into(),
        found,
        note: String::new(),
        sent: false,
        made: false,
        earlier: false,
        retest: None,
        open,
        seen: false,
        shots: Vec::new(),
        comment: false,
        planning: false,
    };
    let rows = vec![
        row(
            "AC-1",
            "Saves",
            "Given a draft, when I save, then it lists",
            None,
            true,
        ),
        row(
            "AC-2",
            "Exports",
            "Given rows, when I export, then a CSV holds them",
            Some(Verdict::Pass),
            false,
        ),
        row(
            "AC-3",
            "Empty name",
            "Given the dialog, when the name is empty, then Save is off",
            Some(Verdict::Fail),
            true,
        ),
    ];
    let ctx = RowCtx {
        criteria: 3,
        ..RowCtx::default()
    };
    (f, rows, ctx)
}

#[test]
fn criteria_show_their_steps_as_left_with_a_row_folding_them() {
    use crate::feature::manual::FoldScope;
    let (f, rows, ctx) = sheet_rows();
    let state = PanelState {
        ctx,
        criteria: rows.clone(),
        ..PanelState::default()
    };
    let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows, &[]);
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    let cards: Vec<&FindingCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect();
    // Each its steps, open or folded as the user left it.
    let open: Vec<bool> = cards.iter().map(|c| !c.collapsed).collect();
    assert_eq!(open, [true, false, true]);
    let sheet = cards[0].sheet.as_ref().unwrap();
    assert_eq!(
        sheet.steps,
        [
            ("Given", "a draft".to_owned()),
            ("When", "I save".to_owned()),
            ("Then", "it lists".to_owned()),
        ]
    );
    assert!(
        cards
            .iter()
            .all(|c| c.sheet.as_ref().is_some_and(|s| !s.hidden))
    );
    // The row over them: Fold all, passed and read; Open all.
    let groups = (page.sections.iter())
        .find_map(|s| match s {
            PageSection::Toolbar { groups } => Some(groups.clone()),
            _ => None,
        })
        .unwrap();
    let words: Vec<&str> = groups.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(words, ["Fold", ""]);
    let of = |g: usize| -> Vec<PanelOutcome> {
        groups[g].1.iter().map(|b| actions[*b].1.clone()).collect()
    };
    let fold = PanelOutcome::FoldCriteria;
    assert_eq!(
        of(0),
        [
            fold(FoldScope::All, false),
            fold(FoldScope::Done, false),
            fold(FoldScope::Seen, false)
        ]
    );
    assert_eq!(of(1), [fold(FoldScope::All, true)]);
    // Folding only shows them otherwise: it changes no feature.
    assert!(fold(FoldScope::All, true).reads_only());
    assert!(PanelOutcome::FoldCriterion(0, false).reads_only());
    // Also on the done page, where nothing else acts.
    let mut done = f.clone();
    done.tick(100, "you", None);
    assert_eq!(stage_mark(&done, Stage::ManualTest), StageMark::Done);
    let actions = stage_actions(&done, Stage::ManualTest, ctx, &[], &rows, &[]);
    let folds = actions
        .iter()
        .filter(|(_, o)| matches!(o, PanelOutcome::FoldCriteria(..)));
    assert_eq!(folds.count(), 4);
}

#[test]
fn left_and_right_fold_a_criterion_and_alt_folds_them_all() {
    use crate::feature::manual::FoldScope;
    let (f, _, ctx) = sheet_rows();
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        finding: 2,
        ..FeaturePanel::default()
    };
    let slugs = vec!["m".to_owned()];
    let mut hit =
        |mods: ModifiersState, sym| panel.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[]);
    let none = ModifiersState::default();
    assert_eq!(
        hit(none, Keysym::Left),
        Some(PanelOutcome::FoldCriterion(2, false))
    );
    assert_eq!(
        hit(none, Keysym::Right),
        Some(PanelOutcome::FoldCriterion(2, true))
    );
    let alt = ModifiersState { alt: true, ..none };
    let alt_shift = ModifiersState { shift: true, ..alt };
    let fold = PanelOutcome::FoldCriteria;
    assert_eq!(hit(alt, Keysym::Left), Some(fold(FoldScope::All, false)));
    assert_eq!(hit(alt, Keysym::Right), Some(fold(FoldScope::All, true)));
    // Shift+D comes as `d`, Shift held (K8).
    assert_eq!(hit(alt, Keysym::d), Some(fold(FoldScope::Done, false)));
    assert_eq!(hit(alt_shift, Keysym::d), Some(fold(FoldScope::Done, true)));
    assert_eq!(hit(alt, Keysym::s), Some(fold(FoldScope::Seen, false)));
    assert_eq!(hit(alt_shift, Keysym::s), Some(fold(FoldScope::Seen, true)));
    // Up and Down still go through them.
    assert_eq!(hit(none, Keysym::Up), None);
    assert_eq!(panel.finding, 1);
    // The footer's "/" presses the key that opens the search.
    assert_eq!(crate::keys::hint_key("/").map(|k| k.1), Some(Keysym::slash));
}

#[test]
fn a_search_shows_only_the_criteria_saying_it_open_and_tinted() {
    let (f, rows, ctx) = sheet_rows();
    let state = PanelState {
        ctx,
        criteria: rows,
        ..PanelState::default()
    };
    let mut search = crate::manual_test::Search::default();
    search.query.set("CSV");
    let panel = FeaturePanel {
        search: Some(search),
        ..FeaturePanel::default()
    };
    let view = tab_view(&panel, &f, &["m".to_owned()], &state, 2_060);
    let cards = cards_of(&view);
    let hidden: Vec<bool> = (cards.iter())
        .map(|c| c.sheet.as_ref().unwrap().hidden)
        .collect();
    assert_eq!(hidden, [true, false, true]);
    // Folded, it opens while it is found.
    assert!(!cards[1].collapsed);
    assert_eq!(cards[1].sheet.as_ref().unwrap().needle, "CSV");
    // What it found, as laid out, is all in the criterion shown; the
    // count of those is `Gerdr::feature_tab_view`'s.
    let frame = smithay::utils::Rectangle::from_size((1180, 900).into());
    let layout = chrome::feature_tab_layout(&view, frame);
    assert!(!layout.matches.is_empty());
    assert!(layout.matches.iter().all(|m| m.card == Some(1)));
    // A match gone to is the one tinted more.
    let mut view = view;
    view.marks.found_at = Some(0);
    let layout = chrome::feature_tab_layout(&view, frame);
    assert_eq!(layout.matches.len(), 1 + (layout.matches.len() - 1));
}

#[test]
fn a_criterions_card_holds_its_keys_and_buttons() {
    use crate::{chrome::FindingTag, manual_test::Outcome};
    let f = feature("m", vec![repo("api", true)], 7);
    let row = |found, note: &str| Row {
        open: false,
        seen: false,
        id: "AC-1".into(),
        title: "C".into(),
        text: "Given a, when b, then c".into(),
        found,
        note: note.into(),
        sent: false,
        made: false,
        earlier: false,
        retest: None,
        shots: Vec::new(),
        comment: false,
        planning: false,
    };
    let rows = vec![
        row(None, ""),
        row(Some(Verdict::Pass), ""),
        row(Some(Verdict::Fail), "it broke"),
    ];
    let ctx = RowCtx {
        criteria: 3,
        ..RowCtx::default()
    };
    let state = PanelState {
        ctx,
        criteria: rows.clone(),
        ..PanelState::default()
    };
    let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows, &[]);
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    // Under the label, in their order.
    assert!(matches!(
        &page.sections[0],
        PageSection::Lines { label, .. } if label == "Criteria"
    ));
    let cards: Vec<&FindingCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect();
    let tags: Vec<FindingTag> = cards.iter().map(|c| c.tag).collect();
    assert_eq!(
        tags,
        [FindingTag::NotRun, FindingTag::Passed, FindingTag::Failed]
    );
    assert_eq!(cards[2].note, "What went wrong: it broke");
    let labels = |card: &FindingCard| -> Vec<String> {
        let labels = card.buttons.iter();
        labels.map(|b| page.buttons[*b].label.clone()).collect()
    };
    // Pass and Fail always, in place; the one chosen filled, and
    // pressed again it takes the choice back.
    assert_eq!(labels(cards[0]), ["Pass", "Fail\u{2026}"]);
    assert_eq!(labels(cards[1]), ["Pass", "Fail\u{2026}"]);
    assert_eq!(labels(cards[2]), ["Edit note\u{2026}", "Pass", "Fail"]);
    let styles = |card: &FindingCard| -> Vec<ButtonStyle> {
        card.buttons
            .iter()
            .map(|b| page.buttons[*b].style)
            .collect()
    };
    assert_eq!(styles(cards[0]), [ButtonStyle::Plain, ButtonStyle::Plain]);
    assert_eq!(styles(cards[1]), [ButtonStyle::Passed, ButtonStyle::Plain]);
    assert_eq!(
        styles(cards[2]),
        // One style to a row: the quiet Edit note… outlined like Pass.
        [ButtonStyle::Plain, ButtonStyle::Plain, ButtonStyle::Failed]
    );
    let outcomes = |card: &FindingCard| -> Vec<PanelOutcome> {
        card.buttons.iter().map(|b| actions[*b].1.clone()).collect()
    };
    let on = |o| PanelOutcome::Manual(o);
    assert_eq!(
        outcomes(cards[0]),
        [on(Outcome::Toggle(0)), on(Outcome::Write(0))]
    );
    assert_eq!(
        outcomes(cards[2]),
        [
            on(Outcome::Write(2)),
            on(Outcome::Toggle(2)),
            on(Outcome::Clear(2))
        ]
    );
    // The title alone, until selected.
    assert_eq!(cards[0].title, "C");
    assert!(cards.iter().all(|c| c.body.is_empty()));
    // Not on another stage's page, nor once done.
    assert!(
        stage_actions(&f, Stage::Build, ctx, &[], &rows, &[])
            .iter()
            .all(|(_, o)| !matches!(o, PanelOutcome::Manual(Outcome::Toggle(_))))
    );
    // The keys, with Tab to get there.
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "m".into(),
        ..FeaturePanel::default()
    };
    let slugs = vec!["m".to_owned()];
    let go = |panel: &mut FeaturePanel, (mods, sym): (ModifiersState, Keysym)| {
        panel.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[])
    };
    assert_eq!(go(&mut panel, key(Keysym::space)), None);
    assert_eq!(go(&mut panel, key(Keysym::Tab)), None);
    assert_eq!(panel.focus, TabArea::Findings);
    assert_eq!(go(&mut panel, key(Keysym::Down)), None);
    assert_eq!(panel.finding, 1);
    let on = |o| Some(PanelOutcome::Manual(o));
    assert_eq!(go(&mut panel, key(Keysym::space)), on(Outcome::Toggle(1)));
    assert_eq!(go(&mut panel, key(Keysym::Return)), on(Outcome::Toggle(1)));
    assert_eq!(go(&mut panel, key(Keysym::f)), on(Outcome::Write(1)));
    assert_eq!(go(&mut panel, key(Keysym::Delete)), on(Outcome::Clear(1)));
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    let both = ModifiersState {
        shift: true,
        ..ctrl
    };
    assert_eq!(
        go(&mut panel, (ctrl, Keysym::Return)),
        on(Outcome::MarkPassed)
    );
    assert_eq!(
        go(&mut panel, (both, Keysym::Return)),
        on(Outcome::SendFailures)
    );
    // Writing a note, the footer has only its keys.
    panel.note = Some((CardNote::Criterion(1), TextField::default()));
    let hints = tab_hints(
        &panel,
        &f,
        Stage::ManualTest,
        &slugs,
        ctx,
        AgentChoices::default(),
        &[],
        &[],
        &[],
    );
    assert_eq!(
        hints,
        [
            ("Enter".to_owned(), "save".to_owned()),
            ("Shift+Enter".to_owned(), "new line".to_owned()),
            ("Esc".to_owned(), "cancel".to_owned())
        ]
    );
}

#[test]
fn manual_test_goes_on_while_its_fixes_are_made() {
    use crate::{chrome::FindingTag, manual_test::Outcome};
    let f = feature("m", vec![repo("api", true)], 7);
    assert_eq!(f.current(), Some(Stage::ManualTest));
    let row = |found, sent, made| Row {
        open: false,
        seen: false,
        id: "AC-1".into(),
        title: "C".into(),
        text: "Given a, when b, then c".into(),
        found,
        note: "x".into(),
        sent,
        made,
        earlier: true,
        retest: None,
        shots: Vec::new(),
        comment: false,
        planning: false,
    };
    let rows = vec![
        row(Some(Verdict::Fail), true, true),
        row(Some(Verdict::Fail), false, false),
        row(Some(Verdict::Pass), false, false),
        row(None, false, false),
    ];
    let ctx = RowCtx {
        criteria: 4,
        manual: crate::feature::manual::Counts {
            passed: 1,
            failed: 2,
            sent: 1,
            made: 1,
            untested: 1,
            ..Default::default()
        },
        ..RowCtx::default()
    };
    let state = PanelState {
        ctx,
        criteria: rows.clone(),
        ..PanelState::default()
    };
    let labels = |f: &Feature| -> Vec<String> {
        let actions = stage_actions(f, Stage::ManualTest, ctx, &[], &rows, &[]);
        let page = stage_page(f, Stage::ManualTest, &state, &actions, 2_060);
        page.buttons.iter().map(|b| b.label.clone()).collect()
    };
    let said = |f: &Feature| -> Vec<(String, Vec<String>)> {
        let actions = stage_actions(f, Stage::ManualTest, ctx, &[], &rows, &[]);
        let page = stage_page(f, Stage::ManualTest, &state, &actions, 2_060);
        (page.sections.iter())
            .filter_map(|s| match s {
                PageSection::Lines { label, lines } => Some((
                    label.clone(),
                    lines.iter().map(|l| l.text.clone()).collect(),
                )),
                _ => None,
            })
            .collect()
    };
    // No round open: the unsent failure can go.
    assert!(labels(&f).contains(&"Plan fixes".to_owned()));
    // A round open: it waits for the next, and the page says so.
    let fixing = in_round(f.clone(), Stage::ManualTest, FixPhase::Fixing);
    assert!(!labels(&fixing).contains(&"Plan fixes".to_owned()));
    let criteria = said(&fixing).into_iter().find(|(l, _)| l == "Criteria");
    assert_eq!(
        criteria.unwrap().1,
        ["reading the code", "1 failure waits for the current round"]
    );
    // The fix made: its card says to try it again, with a dot.
    let actions = stage_actions(&fixing, Stage::ManualTest, ctx, &[], &rows, &[]);
    let page = stage_page(&fixing, Stage::ManualTest, &state, &actions, 2_060);
    let cards: Vec<&FindingCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards[0].tag, FindingTag::Failed);
    // Folded, each says what went wrong too.
    assert_eq!(
        cards[0].state,
        "fix made: try again \u{b7} \u{201c}x\u{201d}"
    );
    assert!(cards[0].made);
    assert_eq!(cards[1].state, "\u{201c}x\u{201d}");
    // The counts go in the bar over the page, not its status.
    assert!(page.status.is_none());
    let strip = test_strip(&rows).unwrap();
    assert_eq!((strip.passed, strip.failed, strip.untested), (1, 2, 1));
    assert_eq!(
        strip.text,
        "1 passed \u{b7} 2 failed \u{b7} 1 untested \u{b7} 1 fix made"
    );
    assert_eq!(test_strip(&[]), None);
    // A pass tried before the fixes says so once selected; a made fix
    // says to try again instead.
    assert!(criterion_details(&rows[2]).ends_with("Tried before the latest fixes"));
    assert!(!criterion_details(&rows[0]).contains("Tried before"));
    // A pass a later fix round changed: untested, saying so.
    let changed = Row {
        retest: Some(3),
        ..row(None, false, false)
    };
    assert!(criterion_details(&changed).ends_with("Changed by fix round 3: try it again"));
    assert_eq!(
        criterion_card(0, &changed, &[], true).state,
        "changed by fix round 3: try again"
    );
    // Fixes landed that Verify hasn't run on: the page says marking it
    // passed goes back there, and Verify's failures aren't its to send.
    let mut checked = f.clone();
    checked.unverified = Some("p2".into());
    checked.verify = Some(feature::verify::VerifyRun {
        n: 2,
        started: 1_000,
        finished: Some(1_100),
        patch: Some("p2".into()),
        passed: Some(false),
        failures: vec!["api: `make` exited 2".into()],
        ..Default::default()
    });
    let criteria = said(&checked).into_iter().find(|(l, _)| l == "Criteria");
    // The loop back is named, stage by stage.
    let note = "Fixes changed the code: Mark passed first takes it through Verify, \
                    AI review and Code review again, then back here";
    let criteria = criteria.unwrap().1;
    assert!(criteria.contains(&note.to_owned()), "{criteria:?}");
    assert!((said(&checked).iter()).all(|(l, _)| l != "Verify of the fixes"));
    let actions = page_actions(&checked, Stage::ManualTest, ctx);
    assert!(!(actions.iter()).any(|(_, o)| *o == PanelOutcome::MakeVerifyFixes));
    assert!(!labels(&checked).contains(&"Plan Verify fixes".to_owned()));
    assert!((actions.iter()).any(|(_, o)| *o == PanelOutcome::Manual(Outcome::SendFailures)));
}

#[test]
fn an_approved_manual_test_keeps_its_criteria_to_try_more() {
    use crate::chrome::FindingTag;
    use crate::manual_test::Outcome;
    let f = feature("m", vec![repo("api", true)], 8);
    assert_eq!(stage_mark(&f, Stage::ManualTest), StageMark::Done);
    let row = |found| Row {
        open: false,
        seen: false,
        id: "AC-1".into(),
        title: "C".into(),
        text: "Given a, when b, then c".into(),
        found,
        note: String::new(),
        sent: false,
        made: false,
        earlier: false,
        retest: None,
        shots: Vec::new(),
        comment: false,
        planning: false,
    };
    let rows = vec![row(Some(Verdict::Pass)), row(Some(Verdict::Pass))];
    let ctx = RowCtx {
        criteria: 2,
        ..RowCtx::default()
    };
    let state = PanelState {
        ctx,
        criteria: rows.clone(),
        ..PanelState::default()
    };
    let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows, &[]);
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    let cards: Vec<&FindingCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 2);
    // Readable, not faded as on the way, and still to try: Pass and Fail.
    assert!(cards.iter().all(|c| c.tag == FindingTag::Passed));
    assert!(cards.iter().all(|c| !c.faded && c.buttons.len() == 2));
    assert!(matches!(
        (page.sections.iter()).find(|s| matches!(s, PageSection::Lines { .. })),
        Some(PageSection::Lines { label, lines, .. }) if label == "Criteria" && lines.is_empty()
    ));
    // Its sandbox stays too.
    assert!((actions.iter()).any(|(_, o)| *o == PanelOutcome::StartSandbox));
    assert!(
        (page.sections.iter())
            .any(|s| matches!(s, PageSection::Lines { label, .. } if label == "Sandbox"))
    );
    // Nothing moves it on: only reopening sends what fails.
    assert!(!(actions.iter()).any(|(_, o)| matches!(
        o,
        PanelOutcome::Manual(Outcome::MarkPassed | Outcome::SendFailures)
    )));
    // Tab and the arrows go through them; the keys mark them.
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "m".into(),
        ..FeaturePanel::default()
    };
    panel.pick(&f, Stage::ManualTest);
    let slugs = vec!["m".to_owned()];
    let mut go = |sym| {
        let (mods, sym) = key(sym);
        (
            panel.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[]),
            panel.focus,
            panel.finding,
        )
    };
    assert_eq!(go(Keysym::Tab), (None, TabArea::Findings, 0));
    assert_eq!(go(Keysym::Down), (None, TabArea::Findings, 1));
    assert_eq!(
        go(Keysym::space).0,
        Some(PanelOutcome::Manual(Outcome::Toggle(1)))
    );
    assert_eq!(
        go(Keysym::f).0,
        Some(PanelOutcome::Manual(Outcome::Write(1)))
    );
    assert_eq!(
        go(Keysym::Delete).0,
        Some(PanelOutcome::Manual(Outcome::Clear(1)))
    );
    let hints = tab_hints(
        &panel,
        &f,
        Stage::ManualTest,
        &slugs,
        ctx,
        AgentChoices::default(),
        &[],
        &[],
        &[],
    );
    assert!(hints.iter().any(|(_, what)| what == "criterion"));
    assert!(hints.iter().any(|(key, _)| key == REOPEN_KEY));
    assert!(hints.iter().any(|(_, what)| what == "pass"));
    assert!(
        !hints
            .iter()
            .any(|(_, what)| what == "mark passed" || what == "plan fixes")
    );
    // Failed since, it says how to send them.
    let failed = PanelState {
        ctx: RowCtx {
            manual: crate::feature::manual::Counts {
                failed: 1,
                ..Default::default()
            },
            ..ctx
        },
        criteria: vec![row(Some(Verdict::Pass)), row(Some(Verdict::Fail))],
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::ManualTest, &failed, &actions, 2_060);
    assert!(matches!(
        (page.sections.iter()).find(|s| matches!(s, PageSection::Lines { .. })),
        Some(PageSection::Lines { lines, .. }) if lines.first().is_some_and(|l| l.text
            == "1 failure waits since it was done: reopen it to send them")
    ));
    // What the footer says it reopens, from the cards too.
    let both = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        panel.key(&both, Keysym::Return, &f, &slugs, ctx, &[], &[], &[]),
        Some(PanelOutcome::Untick(Stage::ManualTest))
    );
    // Ticked by hand, they say so rather than read as not tried yet.
    let rows = vec![row(None), row(None)];
    let state = PanelState {
        ctx,
        criteria: rows,
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    assert!(matches!(
        (page.sections.iter()).find(|s| matches!(s, PageSection::Lines { .. })),
        Some(PageSection::Lines { lines, .. })
            if lines.first().is_some_and(|l| l.text == "marked done by hand, not tried")
    ));
    // A finished feature stuck on it shows them, and nothing tries them.
    let mut left = feature("l", vec![repo("api", true)], 7);
    assert!(tries_criteria(&left, Stage::ManualTest));
    left.status = Status::Abandoned;
    assert!(!tries_criteria(&left, Stage::ManualTest));
    assert!(!marks_criteria(&left, Stage::ManualTest));
    assert!(shows_criteria(&left, Stage::ManualTest));
}

#[test]
fn manual_test_back_at_verify_keeps_its_criteria_and_sandbox_to_try_more() {
    use crate::manual_test::Outcome;
    let row = |found| Row {
        open: false,
        seen: false,
        id: "AC-1".into(),
        title: "C".into(),
        text: "Given a, when b, then c".into(),
        found,
        note: String::new(),
        sent: false,
        made: false,
        earlier: false,
        retest: None,
        shots: Vec::new(),
        comment: false,
        planning: false,
    };
    let rows = vec![row(Some(Verdict::Pass)), row(Some(Verdict::Fail))];
    let ctx = RowCtx {
        criteria: 2,
        manual: crate::feature::manual::Counts {
            passed: 1,
            failed: 1,
            ..Default::default()
        },
        ..RowCtx::default()
    };
    let state = PanelState {
        ctx,
        criteria: rows.clone(),
        ..PanelState::default()
    };
    // Never got there: nothing of it shows yet.
    let mut f = feature("m", vec![repo("api", true)], stage_index(Stage::Verify));
    assert_eq!(stage_mark(&f, Stage::ManualTest), StageMark::Later);
    assert!(!shows_criteria(&f, Stage::ManualTest));
    assert!(!marks_criteria(&f, Stage::ManualTest));
    let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows, &[]);
    assert!(actions.is_empty(), "{actions:?}");
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    assert!(!(page.sections.iter()).any(|s| matches!(s, PageSection::Finding(_))));
    // Its fixes took it back to Verify: the sheet, its keys and the
    // sandbox stay, and what fails waits for it.
    f.ran_once.insert(Stage::ManualTest, 100);
    assert!(shows_criteria(&f, Stage::ManualTest));
    assert!(marks_criteria(&f, Stage::ManualTest));
    assert!(!tries_criteria(&f, Stage::ManualTest));
    let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows, &[]);
    let outcomes: Vec<&PanelOutcome> = actions.iter().map(|(_, o)| o).collect();
    assert!(outcomes.contains(&&PanelOutcome::Manual(Outcome::Toggle(0))));
    assert!(outcomes.contains(&&PanelOutcome::StartSandbox));
    assert!(!(outcomes.iter()).any(|o| matches!(
        o,
        PanelOutcome::Manual(Outcome::MarkPassed | Outcome::SendFailures)
    )));
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
    let cards = (page.sections.iter()).filter(|s| matches!(s, PageSection::Finding(_)));
    assert_eq!(cards.count(), 2);
    assert!((page.sections.iter()).any(|s| matches!(s, PageSection::Toolbar { .. })));
    assert!((page.sections.iter()).any(|s| matches!(
        s,
        PageSection::Lines { label, lines, .. } if label == "Criteria"
            && lines.first().is_some_and(|l| l.text == "1 failure waits for Manual test")
    )));
    assert!(
        (page.sections.iter())
            .any(|s| matches!(s, PageSection::Lines { label, .. } if label == "Sandbox"))
    );
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "m".into(),
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    panel.pick(&f, Stage::ManualTest);
    panel.focus = TabArea::Findings;
    let slugs = vec!["m".to_owned()];
    let (mods, sym) = key(Keysym::space);
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, ctx, &[], &[], &[]),
        Some(PanelOutcome::Manual(Outcome::Toggle(0)))
    );
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        panel.key(&ctrl, Keysym::Return, &f, &slugs, ctx, &[], &[], &[]),
        None
    );
}
