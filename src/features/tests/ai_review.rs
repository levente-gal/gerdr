//! Tests of AI review's page and its cards.

use super::*;

#[test]
fn a_pull_request_merged_on_github_offers_no_undo() {
    let mut p = probe(None);
    let undo = |step| PanelOutcome::UndoShip("api".into(), step);
    assert!(!undoes_merged(&undo(ShipStep::OpenPr), Some(&p)));
    let merged = ship::PrState {
        state: "MERGED".into(),
        ..Default::default()
    };
    p.prs = Some(BTreeMap::from([("api".to_owned(), Ok(merged))]));
    assert!(undoes_merged(&undo(ShipStep::OpenPr), Some(&p)));
    assert!(undoes_merged(&undo(ShipStep::Push), Some(&p)));
    assert!(!undoes_merged(&undo(ShipStep::Merge), Some(&p)));
    let other = PanelOutcome::UndoShip("web".into(), ShipStep::OpenPr);
    assert!(!undoes_merged(&other, Some(&p)));
}

#[test]
fn the_ship_confirm_says_what_follows_shipping() {
    let mut f = feature("s", vec![repo("api", true)], 8);
    let said = |f: &Feature, removable| ship_after_line(f, removable, &[]);
    let closes = Some("Then its workspace closes".to_owned());
    assert_eq!(said(&f, true), closes);
    f.ship_plan.remove_worktrees = true;
    let go = Some("Then its worktree and branch go".to_owned());
    assert_eq!(said(&f, true), go);
    // What is ticked no longer merges everything: they stay.
    assert_eq!(said(&f, false), closes);
    f.ship_plan = Default::default();
    f.ship_plan.keep_workspace = true;
    let stops = Some("Then its other tabs close".to_owned());
    assert_eq!(said(&f, true), stops);
    let running = ["claude".to_owned(), "npm".to_owned()];
    assert_eq!(
        ship_after_line(&f, true, &running),
        Some("Then its other tabs close, and the 2 commands running in them stop".to_owned())
    );
    f.ship_plan.keep_workspace = false;
    f.status = Status::Done;
    assert_eq!(said(&f, true), None);
}

#[test]
fn a_base_to_pull_is_offered_and_ship_is_no_default_while_it_holds_up() {
    let f = feature("s", vec![repo("api", true)], 8);
    let why = "api: feature/s has 8 commits of origin/main that main lacks: pull main first";
    let mut c = check(&[], &["api"], &[]).unwrap();
    c.repos[0].route = Route::Merge;
    c.repos[0].no_merge = Some(why.to_owned());
    let p = probe(Some(c));
    assert_eq!(
        pull_first_of(&f, Some(&p)),
        Some(("api".into(), "main".into(), why.into()))
    );
    let pull = (PanelOutcome::PullBase("api".into()), ButtonStyle::Primary);
    let offered: Vec<_> = (ship_base_actions(&f, Some(&p)).into_iter())
        .map(|(b, o)| (o, b.style))
        .collect();
    assert!(offered.contains(&pull), "{offered:?}");
    let ctx = RowCtx {
        ship_blocked: true,
        ..RowCtx::default()
    };
    let ship = (page_actions(&f, Stage::Ship, ctx).into_iter())
        .find(|(_, o)| *o == PanelOutcome::Ship)
        .unwrap();
    assert_eq!(ship.0.style, ButtonStyle::Plain);
    // Something else holding it up: no pull asked.
    let mut c = check(&["api is dirty"], &["api"], &[]).unwrap();
    c.repos[0].route = Route::Merge;
    assert_eq!(pull_first_of(&f, Some(&probe(Some(c)))), None);
}

#[test]
fn ai_review_shows_its_agent_then_its_findings_never_mark_done() {
    use FindingState as S;
    let row = PanelRow::Stage(Stage::AiReview);
    let none = RowCtx::default();
    let fresh = feature("r", vec![repo("api", true)], 5);
    assert_eq!(
        row_action(&fresh, row, none),
        Some(PanelOutcome::StartAgent(Stage::AiReview, false))
    );
    assert_eq!(
        stage_button(&fresh, Stage::AiReview, none),
        Some("Start agent")
    );
    // Running: its tab, whatever the earlier findings.
    let running = reviewing(&[S::Open], false);
    assert_eq!(
        row_action(&running, row, none),
        Some(PanelOutcome::ShowAgent(Stage::AiReview))
    );
    assert_eq!(
        review_line(&running, Some("Codex reviewing \u{b7} started 2m ago")).as_deref(),
        Some("Codex reviewing \u{b7} started 2m ago")
    );
    let open = reviewing(&[S::Open, S::Open, S::Fix, S::Dismissed], true);
    assert_eq!(row_action(&open, row, none), Some(PanelOutcome::Findings));
    assert_eq!(row_hint(&open, row, none), "review in diff");
    assert_eq!(
        stage_button(&open, Stage::AiReview, none),
        Some("Review in diff")
    );
    assert_eq!(
        review_line(&open, None).as_deref(),
        Some("2 findings to decide")
    );
    // While any waits, Plan fixes can't be pressed, so isn't there.
    let labels = |f: &Feature| -> Vec<String> {
        (page_actions(f, Stage::AiReview, none).into_iter())
            .map(|(b, _)| b.label)
            .collect()
    };
    assert!(labels(&open).contains(&"Fix all open".to_owned()));
    assert!(!labels(&open).contains(&"Plan fixes".to_owned()));
    assert!(!labels(&open).contains(&"Make fixes".to_owned()));
    // Taken as they are, the fixes go with no plan; one can be asked for.
    let fix = reviewing(&[S::Fix, S::Dismissed], true);
    let primary = |f: &Feature, ctx| page_enter(f, Stage::AiReview, ctx);
    let plain = (page_actions(&fix, Stage::AiReview, none).into_iter())
        .find(|(b, _)| b.label == "Plan fixes");
    assert_eq!(
        plain.map(|(b, o)| (b.style, o)),
        Some((ButtonStyle::Plain, PanelOutcome::SendFixes))
    );
    // Nothing left to want: greyed, in its place.
    let all = (page_actions(&fix, Stage::AiReview, none).into_iter())
        .find(|(b, _)| b.label == "Fix all open");
    assert_eq!(all.map(|(b, _)| b.style), Some(ButtonStyle::Off));
    assert_eq!(
        row_action(&fix, row, none),
        Some(PanelOutcome::MakeFindingFixes)
    );
    assert_eq!(primary(&fix, none), Some(PanelOutcome::MakeFindingFixes));
    assert_eq!(row_hint(&fix, row, none), "make fixes");
    // Ctrl+Enter sends them from the cards too, Ctrl+Shift+Enter plans.
    let mut panel = FeaturePanel {
        slug: fix.slug.clone(),
        stage: Some(Stage::AiReview),
        ..FeaturePanel::default()
    };
    for focus in [TabArea::Stages, TabArea::Findings] {
        panel.focus = focus;
        let ctrl = |shift| ModifiersState {
            ctrl: true,
            shift,
            ..ModifiersState::default()
        };
        assert_eq!(
            press(&mut panel, &fix, (ctrl(false), Keysym::Return)),
            Some(PanelOutcome::MakeFindingFixes)
        );
        assert_eq!(
            press(&mut panel, &fix, (ctrl(true), Keysym::Return)),
            Some(PanelOutcome::SendFixes)
        );
    }
    // A word of the user's asks for the plan: Make fixes is gone.
    let commented = RowCtx {
        comments: 1,
        ..none
    };
    let mut noted = fix.clone();
    let finding = &mut noted.review.as_mut().unwrap().findings[0];
    finding.note = "Careful".into();
    for (f, ctx) in [(&fix, commented), (&noted, none)] {
        assert_eq!(row_action(f, row, ctx), Some(PanelOutcome::SendFixes));
        assert_eq!(primary(f, ctx), Some(PanelOutcome::SendFixes));
        assert_eq!(row_hint(f, row, ctx), "plan fixes");
        let shown: Vec<String> = (page_actions(f, Stage::AiReview, ctx).into_iter())
            .map(|(b, _)| b.label)
            .collect();
        assert!(!shown.contains(&"Make fixes".to_owned()), "{shown:?}");
        assert!(shown.contains(&"Plan fixes".to_owned()), "{shown:?}");
        // The page says why.
        let state = PanelState {
            ctx,
            ..PanelState::default()
        };
        let actions = page_actions(f, Stage::AiReview, ctx);
        let sections = review_page::review_page_sections(
            f,
            Stage::AiReview,
            StageMark::Current,
            &state,
            &actions,
        );
        let why = summary_line(&sections).and_then(|(_, lines)| {
            (lines.iter())
                .find(|l| l.text.starts_with(review_import::OWN_WORDS))
                .map(|l| l.text.clone())
        });
        assert!(why.is_some(), "{sections:?}");
    }
    // There Ctrl+Enter makes no fixes and says why: Ctrl+Shift+Enter
    // plans them, the one key for that on every page.
    panel.focus = TabArea::Stages;
    let gate = |shift| ModifiersState {
        ctrl: true,
        shift,
        ..ModifiersState::default()
    };
    assert_eq!(
        press(&mut panel, &noted, (gate(false), Keysym::Return)),
        Some(PanelOutcome::FindingRefused(PLAN_FIRST))
    );
    assert_eq!(
        press(&mut panel, &noted, (gate(true), Keysym::Return)),
        Some(PanelOutcome::SendFixes)
    );
    // With no word of the user's, nothing to say.
    let state = PanelState::default();
    let actions = page_actions(&fix, Stage::AiReview, none);
    let sections = review_page::review_page_sections(
        &fix,
        Stage::AiReview,
        StageMark::Current,
        &state,
        &actions,
    );
    let says = |s: &PageSection| {
        matches!(s, PageSection::Lines { lines, .. }
            if lines.iter().any(|l| l.text.starts_with(review_import::OWN_WORDS)))
    };
    assert!(!sections.iter().any(says));
    assert_eq!(review_line(&fix, None).as_deref(), Some("1 finding to fix"));
    let sent = reviewing(&[S::Sent, S::Sent], true);
    assert_eq!(
        review_line(&sent, None).as_deref(),
        Some("2 findings being fixed")
    );
    let decided = reviewing(&[S::Fixed, S::Dismissed], true);
    assert_eq!(
        review_line(&decided, None).as_deref(),
        Some("all findings fixed or dismissed")
    );
    assert_eq!(
        row_action(&decided, row, none),
        Some(PanelOutcome::ShowAgent(Stage::AiReview))
    );
    assert_eq!(
        review_line(&reviewing(&[], true), None).as_deref(),
        Some("no findings")
    );
    // Ctrl+R: a fresh reviewer.
    let mut panel = FeaturePanel {
        ..FeaturePanel::default()
    };
    assert_eq!(
        press(&mut panel, &open, ctrl(Keysym::r)),
        Some(PanelOutcome::StartAgent(Stage::AiReview, true))
    );
    let slugs = vec!["r".to_owned()];
    let state = PanelState::default();
    let view = tab_view(&panel, &open, &slugs, &state, 2_060);
    assert_eq!(
        stage_detail(&open, Stage::AiReview, &state, 2_060),
        "2 findings to decide"
    );
    assert_eq!(view.page.buttons[0].label, "Review in diff");
    assert_eq!(view.rail[5].mark, NodeMark::Waiting);
    assert_eq!(view.rail[5].line, "2 to decide");
    assert!(view.hints.contains(&("Ctrl+R".into(), "restart".into())));
    assert!(
        view.hints
            .contains(&("Enter".into(), "review in diff".into()))
    );
}

fn comment_labels<'a>(page: &'a StagePage, card: &FindingCard) -> Vec<&'a str> {
    (card.comment_buttons.iter())
        .map(|&i| page.buttons[i].label.as_str())
        .collect()
}

/// The labels of `card`'s buttons on `page`.
fn card_labels<'a>(page: &'a StagePage, card: &FindingCard) -> Vec<&'a str> {
    (card.buttons.iter())
        .map(|b| page.buttons[*b].label.as_str())
        .collect()
}

#[test]
fn findings_show_as_cards_in_their_order_each_with_what_it_can_do() {
    use FindingState as S;
    let mut f = carded(&[S::Dismissed, S::Fix, S::Open, S::Sent, S::Fixed]);
    (f.review.as_mut().unwrap().findings[0]).reason = "by design".into();
    let panel = FeaturePanel::default();
    let slugs = vec!["r".to_owned()];
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
    let cards = cards_of(&view);
    let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
    // Deciding moves nothing: all of a severity stay in the order raised.
    assert_eq!(
        titles,
        ["Title 0", "Title 1", "Title 2", "Title 3", "Title 4"]
    );
    let labels: Vec<Vec<&str>> = cards.iter().map(|c| card_labels(&view.page, c)).collect();
    assert_eq!(
        labels[2],
        [
            "Fix",
            "Comment",
            "Edit fix\u{2026}",
            "Dismiss\u{2026}",
            "Show in diff"
        ]
    );
    assert_eq!(
        labels[1],
        [
            "Reopen",
            "Comment",
            "Edit fix\u{2026}",
            "Dismiss\u{2026}",
            "Show in diff"
        ]
    );
    assert_eq!(labels[3], ["Show in diff"]);
    assert_eq!(labels[4], ["Show in diff"]);
    assert_eq!(labels[0], ["Reopen", "Comment", "Show in diff"]);
    assert_eq!(cards[1].state, "to fix");
    assert_eq!(cards[0].state, "dismissed: by design");
    assert!(cards[0].faded && !cards[2].faded);
    assert!(cards[2].place.contains("src/2.rs"));
    // Every decided card closes to a line, so what is left to decide
    // stands out; to fix, sent and dismissed say so down their edge.
    let closed: Vec<bool> = cards.iter().map(|c| c.collapsed).collect();
    assert_eq!(closed, [true, true, false, true, true]);
    let stripes: Vec<Option<CardStripe>> = cards.iter().map(|c| c.stripe).collect();
    assert_eq!(
        stripes,
        [
            Some(CardStripe::Dismissed),
            Some(CardStripe::ToFix),
            None,
            Some(CardStripe::Sent),
            None
        ]
    );
    // No button of a card is the page's own, and the count says it all.
    let on_cards: Vec<usize> = cards.iter().flat_map(|c| c.buttons.clone()).collect();
    let own: Vec<&str> = (0..view.page.buttons.len())
        .filter(|b| !on_cards.contains(b))
        .map(|b| view.page.buttons[b].label.as_str())
        .collect();
    assert!(
        own.contains(&"Review in diff") && own.contains(&"Add comment\u{2026}"),
        "{own:?}"
    );
    assert_eq!(
        summary_line(&view.page.sections)
            .map(|(label, _)| label)
            .as_deref(),
        Some(
            "Findings \u{b7} 1 to decide \u{b7} 1 to fix \u{b7} 1 being fixed \u{b7} 1 fixed \u{b7} 1 dismissed"
        )
    );
}

#[test]
fn a_finding_says_whose_fix_it_is_and_an_item_of_yours_has_no_place() {
    use FindingState as S;
    let mut f = carded(&[S::Open, S::Fix]);
    let findings = &mut f.review.as_mut().unwrap().findings;
    findings[0].fix = "Reviewer's".into();
    findings[1].fix = "Reviewer's".into();
    findings[1].my_fix = Some("Mine".into());
    findings[1].note = "Careful".into();
    review_import::add(f.review.as_mut().unwrap(), "Also rename it\nIt reads badly").unwrap();
    let panel = FeaturePanel::default();
    let slugs = vec!["r".to_owned()];
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
    let cards = cards_of(&view);
    assert_eq!(cards[1].fix, "Fix: Reviewer's");
    // The reviewer's stays under the user's wording.
    assert_eq!(cards[2].fix, "Your fix: Mine\nThe reviewer's: Reviewer's");
    assert_eq!(cards[2].comment, "Careful");
    assert_eq!(cards[2].note, "");
    assert_eq!(comment_labels(&view.page, cards[2]), ["Delete"]);
    // The user's own comes first.
    let yours = cards[0];
    assert_eq!(yours.tag, FindingTag::Yours);
    assert_eq!(yours.title, "Also rename it");
    assert_eq!(yours.body, "It reads badly");
    assert_eq!(yours.place, "u1");
    assert_eq!(card_labels(&view.page, yours), ["Edit\u{2026}", "Delete"]);
}

#[test]
fn a_done_ai_review_only_shows_its_findings() {
    use FindingState as S;
    let mut f = carded(&[S::Fixed, S::Dismissed]);
    f.tick(2_070, "gerdr", None);
    assert_ne!(f.current(), Some(Stage::AiReview));
    let mut panel = FeaturePanel::default();
    panel.pick(&f, Stage::AiReview);
    let slugs = vec!["r".to_owned()];
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_080);
    let cards = cards_of(&view);
    assert_eq!(cards.len(), 2);
    for card in cards {
        assert_eq!(card_labels(&view.page, card), ["Show in diff"]);
    }
    assert!(
        !view
            .page
            .buttons
            .iter()
            .any(|b| b.label.starts_with("Add comment"))
    );
    assert_eq!(press(&mut panel, &f, key(Keysym::c)), None);
}

#[test]
fn tab_goes_through_the_findings_and_shift_tab_comes_back() {
    use FindingState as S;
    let f = carded(&[S::Open]);
    let mut panel = FeaturePanel::default();
    assert_eq!(panel.focus, TabArea::Stages);
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Findings);
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Repos);
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Stages);
    let shift = ModifiersState {
        shift: true,
        ..ModifiersState::default()
    };
    press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
    assert_eq!(panel.focus, TabArea::Repos);
    press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
    assert_eq!(panel.focus, TabArea::Findings);
    press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
    assert_eq!(panel.focus, TabArea::Stages);
    // With no cards the findings are no stop, and lose the keyboard.
    let none = reviewing(&[], false);
    press(&mut panel, &none, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Repos);
    panel.focus = TabArea::Findings;
    press(&mut panel, &none, key(Keysym::F2));
    assert_eq!(panel.focus, TabArea::Stages);
}

/// `f`, its last review kept as earlier run 1, with one fixed finding.
fn with_round(mut f: Feature) -> Feature {
    let review = f.review.as_mut().unwrap();
    review.rounds.push(ReviewRound {
        n: 1,
        findings: vec![
            ReviewFinding {
                id: "r1-9".into(),
                title: "Old fixed".into(),
                file: "src/old.rs".into(),
                state: FindingState::Fixed,
                ..ReviewFinding::default()
            },
            ReviewFinding {
                id: "r1-8".into(),
                title: "Old raised".into(),
                state: FindingState::Sent,
                ..ReviewFinding::default()
            },
        ],
    });
    review.n = 2;
    f
}

#[test]
fn the_last_review_stays_once_its_fixes_change_the_code() {
    use FindingState as S;
    let mut f = carded(&[S::Sent, S::Dismissed]);
    sent_back(&mut f);
    let view = view_of(&f, &PanelState::default());
    let cards = cards_of(&view);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].state, "being fixed");
    // Only to look at, and it says why.
    assert_eq!(labels_of(&view, cards[0]), ["Show in diff"]);
    assert_eq!(labels_of(&view, cards[1]), ["Show in diff"]);
    let said = summary_line(&view.page.sections).is_some_and(|(label, lines)| {
        label.starts_with("Findings")
            && lines.iter().any(|l| l.text.contains("code has changed"))
    });
    assert!(said, "{:?}", view.page.sections);
}

#[test]
fn the_last_review_stays_while_the_next_runs() {
    use FindingState as S;
    let f = {
        let mut f = carded(&[S::Fixed, S::Open]);
        f.runs.get_mut(&Stage::AiReview).unwrap().ready = None;
        f
    };
    let view = view_of(&f, &PanelState::default());
    let cards = cards_of(&view);
    assert_eq!(cards.len(), 2);
    assert_eq!(labels_of(&view, cards[0]), ["Show in diff"]);
    let said = summary_line(&view.page.sections)
        .is_some_and(|(_, lines)| lines.iter().any(|l| l.text.contains("hasn't finished")));
    assert!(said, "{:?}", view.page.sections);
}

#[test]
fn a_review_that_is_only_to_look_at_refuses_the_keys_that_decide() {
    use FindingState as S;
    let mut sent = carded(&[S::Open]);
    sent_back(&mut sent);
    let mut running = carded(&[S::Open]);
    running.runs.get_mut(&Stage::AiReview).unwrap().ready = None;
    for f in [&sent, &running] {
        let mut panel = FeaturePanel {
            focus: TabArea::Findings,
            stage: Some(Stage::AiReview),
            picked_at: f.current(),
            ..FeaturePanel::default()
        };
        let refused = |out: Option<PanelOutcome>| {
            assert!(
                matches!(out, Some(PanelOutcome::FindingRefused(_))),
                "{out:?}"
            );
        };
        refused(press(&mut panel, f, key(Keysym::f)));
        refused(press(&mut panel, f, ctrl(Keysym::e)));
        refused(press(&mut panel, f, key(Keysym::Delete)));
        assert_eq!(
            press(&mut panel, f, key(Keysym::Return)),
            Some(PanelOutcome::ShowFinding("r1-1".to_owned()))
        );
    }
}

#[test]
fn an_earlier_review_folds_and_shows_as_a_record() {
    use FindingState as S;
    let f = with_round(carded(&[S::Open]));
    // Folded: its name and how it went, with one button.
    let view = view_of(&f, &PanelState::default());
    assert_eq!(cards_of(&view).len(), 1);
    let card = view.page.sections.iter().find_map(|s| match s {
        PageSection::File {
            name,
            state,
            buttons,
        } => Some((name, state, buttons)),
        _ => None,
    });
    let (name, said, buttons) = card.expect("the round's card");
    assert_eq!(name, "AI review run 1");
    assert_eq!(said.text, "1 fixed \u{b7} 1 raised again");
    assert_eq!(view.page.buttons[buttons[0]].label, "Show");

    // Shown: its cards below the latest's, with no buttons at all, even
    // on a finding with the id of one that has buttons.
    let state = PanelState {
        rounds_open: BTreeSet::from([1]),
        ..PanelState::default()
    };
    let view = view_of(&f, &state);
    let cards = cards_of(&view);
    let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Title 0", "Old fixed", "Old raised"]);
    assert_eq!(cards[2].state, "raised again");
    assert!(cards[1].faded && cards[1].buttons.is_empty());
    assert!(cards[2].faded && cards[2].buttons.is_empty());
    // Nothing can select a record, so nothing closes it.
    assert!(!cards[1].collapsed && !cards[2].collapsed);
    let label = (view.page.buttons.iter()).find(|b| b.label == "Hide");
    assert!(label.is_some(), "{:?}", view.page.buttons);
}

#[test]
fn the_review_page_puts_its_open_round_on_top_and_pairs_the_history() {
    let f = history_feature();
    let view = view_of(&f, &PanelState::default());
    let order: Vec<String> = (view.page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(c) if c.nested => Some(format!("  {}", c.label)),
            PageSection::Round(c) => Some(c.label.clone()),
            PageSection::File { name, .. } => Some(name.clone()),
            PageSection::Agent { .. } => Some("agent".into()),
            PageSection::Review(_) => Some("summary".into()),
            PageSection::Finding(c) => Some(c.title.clone()),
            PageSection::Lines { label, .. } if label == "History" => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            "Fix round 9",
            "agent",
            "summary",
            "Title 0",
            "Fix round 8",
            "History",
            "AI review run 5",
            "  Fix round 7",
            "AI review run 4",
            "AI review run 3",
            "  Fix round 4",
            "AI review run 2",
            "AI review run 1",
            "  Fix round 2",
        ]
    );
    // The open round shows open, those over folded.
    let open: Vec<bool> = (view.page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(c) => Some(c.open),
            _ => None,
        })
        .collect();
    assert_eq!(open, [true, false, false, false, false]);
    assert!(view.page.sections.iter().any(|s| matches!(s,
        PageSection::Round(c) if c.label == "Fix round 2" && c.meta.contains("from run 1"))));
    assert_eq!(
        fold_hint(&f, Stage::AiReview),
        Some(("Alt+E".to_owned(), "history".to_owned()))
    );
}

#[test]
fn a_finding_sent_to_build_says_when_its_fix_is_made() {
    use FindingState as S;
    let mut f = with_round(carded(&[S::Sent, S::Sent]));
    let review = f.review.as_mut().unwrap();
    review.findings[1].made = true;
    // An earlier review's is a record: raised again, whatever it was.
    review.rounds[0].findings[1].made = true;
    sent_back(&mut f);
    let state = PanelState {
        rounds_open: BTreeSet::from([1]),
        ..PanelState::default()
    };
    let view = view_of(&f, &state);
    let cards = cards_of(&view);
    let said: Vec<(&str, bool)> = (cards.iter()).map(|c| (c.state.as_str(), c.made)).collect();
    assert_eq!(
        said,
        [
            ("being fixed", false),
            ("fix made", true),
            ("fixed", false),
            ("raised again", false),
        ]
    );
    assert_eq!(
        items_count(&f, Stage::AiReview),
        "1 being fixed \u{b7} 1 fix made"
    );
    f.review.as_mut().unwrap().findings[0].made = true;
    assert_eq!(items_count(&f, Stage::AiReview), "2 fixes made");
    // Only the next review can call it fixed: it still holds AI review up.
    assert_eq!(f.review.as_ref().unwrap().open(), 2);
}

#[test]
fn an_earlier_review_shows_on_a_page_sent_back_too() {
    use FindingState as S;
    let mut f = with_round(carded(&[S::Sent]));
    sent_back(&mut f);
    let view = view_of(&f, &PanelState::default());
    let card = view.page.sections.iter().find_map(|s| match s {
        PageSection::File { buttons, .. } => Some(buttons),
        _ => None,
    });
    let buttons = card.expect("the round's card");
    assert_eq!(view.page.buttons[buttons[0]].label, "Show");
}

#[test]
fn plan_review_shows_its_suggestions_before_it_is_reached() {
    use crate::feature::plan_review::{self, PlanReview};
    let stage = Stage::PlanReview;
    let mut f = feature("p", vec![repo("api", true)], stage_index(stage));
    let mut review = PlanReview::default();
    let raws = plan_review::parse(
        r#"{"suggestions": [{"kind": "question", "plan": "M1", "title": "A?", "why": "w"}]}"#,
    )
    .unwrap();
    plan_review::import(&mut review, 1, raws);
    review.stamp = Some((1, 1));
    f.plan_review = Some(review);
    // The feature went back to Plan: the review waits there, as AI
    // review's does when it went back to Build.
    f.marks.retain(|s, _| *s < Stage::Plan);
    assert_eq!(stage_mark(&f, stage), StageMark::Later);
    let page = stage_page(&f, stage, &PanelState::default(), &[], 1_120);
    let (label, lines) = summary_line(&page.sections).expect("the suggestions' line");
    assert_eq!(label, "Suggestions \u{b7} 1 to decide");
    assert_eq!(
        lines[0].text,
        "From the last review. The plan has changed since."
    );
    assert!(
        page.sections
            .iter()
            .any(|s| matches!(s, PageSection::Finding(_)))
    );
}

#[test]
fn alt_e_shows_the_earlier_reviews_or_folds_them() {
    use FindingState as S;
    let alt = ModifiersState {
        alt: true,
        ..ModifiersState::default()
    };
    let f = with_round(carded(&[S::Open]));
    let mut panel = FeaturePanel {
        stage: Some(Stage::AiReview),
        picked_at: f.current(),
        ..FeaturePanel::default()
    };
    assert_eq!(press(&mut panel, &f, (alt, Keysym::e)), None);
    assert_eq!(panel.rounds_open, BTreeSet::from([1]));
    press(&mut panel, &f, (alt, Keysym::e));
    assert!(panel.rounds_open.is_empty());
    // The button shows one; the key then folds what shows.
    panel.toggle_round(1);
    assert_eq!(panel.rounds_open, BTreeSet::from([1]));
    press(&mut panel, &f, (alt, Keysym::e));
    assert!(panel.rounds_open.is_empty());
    // With none to show, it does nothing.
    let none = carded(&[S::Open]);
    press(&mut panel, &none, (alt, Keysym::e));
    assert!(panel.rounds_open.is_empty());
}

#[test]
fn the_footer_says_only_what_works_on_the_cards() {
    use FindingState as S;
    let keys_of = |f: &Feature| -> Vec<String> {
        let panel = FeaturePanel {
            focus: TabArea::Findings,
            stage: Some(Stage::AiReview),
            picked_at: f.current(),
            ..FeaturePanel::default()
        };
        let view = tab_view(&panel, f, &["r".to_owned()], &PanelState::default(), 2_060);
        view.hints.iter().map(|(key, _)| key.to_string()).collect()
    };
    let open = with_round(carded(&[S::Open]));
    let keys = keys_of(&open);
    for key in ["f", "Ctrl+E", "Del", "Enter", "Alt+E"] {
        assert!(keys.iter().any(|k| k == key), "{key} in {keys:?}");
    }
    let mut sent = with_round(carded(&[S::Open]));
    sent_back(&mut sent);
    let keys = keys_of(&sent);
    for key in ["f", "Ctrl+E", "Del"] {
        assert!(!keys.iter().any(|k| k == key), "{key} in {keys:?}");
    }
    assert!(keys.iter().any(|k| k == "Enter"), "{keys:?}");
    assert!(keys.iter().any(|k| k == "Alt+E"), "{keys:?}");
}

/// `notes` as a probe has them.
fn probe_with(notes: Vec<Comment>) -> Probe {
    Probe {
        reading: Err(String::new()),
        guards: BTreeMap::new(),
        ship: None,
        merged: false,
        stats: BTreeMap::new(),
        rounds: BTreeMap::new(),
        docs: feature::page::Docs {
            comments: notes.len(),
            notes,
            ..feature::page::Docs::default()
        },
        models: BTreeMap::new(),
        prs: None,
        bases: BTreeMap::new(),
        resolution: None,
    }
}

#[test]
fn findings_go_by_severity_with_the_users_own_and_diff_comments_first() {
    use FindingState as S;
    let mut f = carded(&[S::Open, S::Open, S::Open]);
    let review = f.review.as_mut().unwrap();
    review.findings[0].severity = review_import::Severity::Low;
    review.findings[1].severity = review_import::Severity::High;
    review_import::add(review, "Mine").unwrap();
    let notes = [note("src/a.rs", "From the diff")];
    let probe = probe_with(notes.to_vec());
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    let view = view_of(&f, &state);
    let titles: Vec<&str> = cards_of(&view).iter().map(|c| c.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Mine", "From the diff", "Title 1", "Title 2", "Title 0"]
    );
    // A decision moves nothing.
    decide_all(&mut f);
    let view = view_of(&f, &state);
    let after: Vec<&str> = cards_of(&view).iter().map(|c| c.title.as_str()).collect();
    assert_eq!(after, titles);
}

/// Every finding of `f` dismissed, but the first to fix.
fn decide_all(f: &mut Feature) {
    let review = f.review.as_mut().unwrap();
    for (i, finding) in review.findings.iter_mut().enumerate() {
        finding.state = match i {
            0 => FindingState::Fix,
            _ => FindingState::Dismissed,
        };
    }
}

#[test]
fn a_closed_card_opens_while_selected() {
    use FindingState as S;
    let f = carded(&[S::Dismissed, S::Open]);
    let mut panel = FeaturePanel {
        stage: Some(Stage::AiReview),
        picked_at: f.current(),
        focus: TabArea::Findings,
        finding: 1,
        ..FeaturePanel::default()
    };
    let view = |panel: &FeaturePanel| {
        tab_view(panel, &f, &["r".to_owned()], &PanelState::default(), 2_060)
    };
    assert!(cards_of(&view(&panel))[0].collapsed);
    panel.finding = 0;
    let open = view(&panel);
    assert!(!cards_of(&open)[0].collapsed && cards_of(&open)[0].selected);
}

/// Left folds any review card and Right opens it, as a criterion; what
/// the user chose holds over selection and the decision.
#[test]
fn a_review_card_folds_and_opens_by_key() {
    use FindingState as S;
    let f = carded(&[S::Dismissed, S::Open]);
    let mut panel = FeaturePanel {
        stage: Some(Stage::AiReview),
        picked_at: f.current(),
        focus: TabArea::Findings,
        finding: 1,
        ..FeaturePanel::default()
    };
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Left)),
        Some(PanelOutcome::FoldCard(1, false))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Right)),
        Some(PanelOutcome::FoldCard(1, true))
    );
    let view = |panel: &FeaturePanel| {
        tab_view(panel, &f, &["r".to_owned()], &PanelState::default(), 2_060)
    };
    assert!(cards_of(&view(&panel)).iter().all(|c| c.folds));
    let ids: Vec<CardId> = review_cards(&f, Stage::AiReview, &[])
        .iter()
        .map(|c| c.id())
        .collect();
    // The open one folds while selected; the dismissed one opens.
    panel.cards_open = vec![(ids[1].clone(), false), (ids[0].clone(), true)];
    let shown = view(&panel);
    let cards = cards_of(&shown);
    assert!(cards[1].collapsed && cards[1].selected);
    assert!(!cards[0].collapsed);
    panel.finding = 0;
    panel.cards_open = vec![(ids[0].clone(), false)];
    assert!(cards_of(&view(&panel))[0].collapsed);
}

#[test]
fn deciding_by_key_goes_on_to_the_next_left_to_decide() {
    use FindingState as S;
    let f = carded(&[S::Open, S::Fix, S::Open, S::Dismissed]);
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    // Past the one already to fix.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding("r1-1".into(), Decision::Fix))
    );
    assert_eq!(panel.finding, 2);
    // Round past the last.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding("r1-3".into(), Decision::Fix))
    );
    assert_eq!(panel.finding, 0);
    // Taking a decision back stays: nothing was decided.
    panel.finding = 1;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding("r1-2".into(), Decision::Reopen))
    );
    assert_eq!(panel.finding, 1);
    // None left but this one: it stays.
    let f = carded(&[S::Open, S::Fix]);
    panel.finding = 0;
    press(&mut panel, &f, key(Keysym::f));
    assert_eq!(panel.finding, 0);
}

#[test]
fn del_deletes_an_item_of_the_users_own_until_it_goes_to_build() {
    use FindingState as S;
    let mut f = carded(&[]);
    let id = review_import::add(f.review.as_mut().unwrap(), "Mine").unwrap();
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    for state in [S::Open, S::Fix, S::Dismissed] {
        f.review.as_mut().unwrap().findings[0].state = state;
        // The first press arms it; any other key lets it go.
        assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
        assert!(panel.armed.is_some());
        assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
        assert_eq!(panel.armed, None);
        assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::DeleteOwnFinding(id.clone())),
            "{state:?}"
        );
    }
    f.review.as_mut().unwrap().findings[0].state = S::Sent;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::FindingRefused(review_import::SENT))
    );
    // One an older gerdr dropped can come back too, or go.
    f.review.as_mut().unwrap().findings[0].state = S::Dismissed;
    let view = view_of(&f, &PanelState::default());
    assert_eq!(labels_of(&view, cards_of(&view)[0]), ["Reopen", "Delete"]);
}

#[test]
fn a_comment_left_in_the_diff_has_a_card_with_its_own_keys() {
    use FindingState as S;
    let f = carded(&[S::Open]);
    let c = note("src/a.rs", "First line\nand more");
    let notes = [c.clone()];
    let probe = probe_with(notes.to_vec());
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    let view = view_of(&f, &state);
    let card = cards_of(&view)[0];
    assert_eq!(card.tag, FindingTag::Yours);
    assert_eq!(card.title, "First line");
    assert_eq!(card.body, "and more");
    assert_eq!(card.place, "src/a.rs:12");
    assert_eq!(card.state, "goes with the fixes");
    assert_eq!(
        labels_of(&view, card),
        ["Edit\u{2026}", "Delete", "Show in diff"]
    );
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    let mods = ModifiersState::default();
    let mut hit = |sym| panel.key(&mods, sym, &f, &[], RowCtx::default(), &[], &[], &notes);
    assert_eq!(hit(Keysym::c), Some(PanelOutcome::EditDiffNote(c.clone())));
    assert_eq!(hit(Keysym::Delete), None);
    assert_eq!(
        hit(Keysym::Delete),
        Some(PanelOutcome::DeleteDiffNote(c.clone()))
    );
    assert_eq!(
        hit(Keysym::Return),
        Some(PanelOutcome::ShowDiffNote(c.clone()))
    );
    assert_eq!(
        hit(Keysym::f),
        Some(PanelOutcome::FindingRefused(DIFF_NOTE))
    );
}

#[test]
fn a_comment_left_in_the_diff_changes_only_while_findings_can_be_decided() {
    use FindingState as S;
    let mut f = carded(&[S::Open]);
    let notes = [note("src/a.rs", "Mine")];
    let probe = probe_with(notes.to_vec());
    let state = PanelState {
        probe: Some(&probe),
        ..PanelState::default()
    };
    // A new review running: only to look at.
    f.runs.get_mut(&Stage::AiReview).unwrap().ready = None;
    let view = view_of(&f, &state);
    assert_eq!(labels_of(&view, cards_of(&view)[0]), ["Show in diff"]);
    // Past AI review they wait for Code review: not on this page.
    let mut done = carded(&[S::Fixed]);
    sent_back(&mut done);
    let cards = review_cards(&done, Stage::AiReview, &notes);
    assert_eq!(cards.len(), 1);
    assert!(cards[0].item().is_some_and(|i| i.of() == ItemOf::Finding));
    // One on the whole change has nowhere to show.
    let mut general = note("", "All of it");
    general.lines.clear();
    let actions = note_actions(&carded(&[S::Open]), Stage::AiReview, &[general.clone()]);
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    assert_eq!(labels, ["Edit\u{2026}", "Delete"]);
    assert_eq!(note_place(&general, false), "the whole change");
}

#[test]
fn the_keys_pick_a_card_and_decide_it_as_in_the_diff() {
    use FindingState as S;
    let mut f = carded(&[S::Open, S::Fix, S::Dismissed, S::Sent]);
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    let id = |n: &str| n.to_owned();
    // r1-1, undecided.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding(id("r1-1"), Decision::Fix))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::AskDismiss(id("r1-1")))
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::e)),
        Some(PanelOutcome::EditFinding(id("r1-1")))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::ShowFinding(id("r1-1")))
    );
    // r1-2, to fix: Down is the next card, as ordered.
    assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
    assert_eq!(panel.finding, 1);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding(id("r1-2"), Decision::Reopen))
    );
    // r1-3, dismissed: both of its keys bring it back or fix it.
    assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::DecideFinding(id("r1-3"), Decision::Fix))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::DecideFinding(id("r1-3"), Decision::Reopen))
    );
    // r1-4, sent, last: nothing to decide, and it says why.
    assert_eq!(press(&mut panel, &f, key(Keysym::End)), None);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::FindingRefused(review_import::SENT))
    );
    // All open ones to fix at once, from anywhere on the page.
    let shift_f = ModifiersState {
        shift: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        panel.key(
            &shift_f,
            Keysym::F,
            &f,
            &[],
            RowCtx::default(),
            &[],
            &[],
            &[]
        ),
        Some(PanelOutcome::FixAllFindings)
    );
    // On a reviewer's card, a comment on it; on any other, one of the
    // user's own.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::c)),
        Some(PanelOutcome::CommentFinding(id("r1-4")))
    );
    let shift = ModifiersState {
        shift: true,
        ..ModifiersState::default()
    };
    // Nothing to delete: Shift+Del isn't Del, and does nothing here.
    assert_eq!(
        panel.key(
            &shift,
            Keysym::Delete,
            &f,
            &[],
            RowCtx::default(),
            &[],
            &[],
            &[]
        ),
        None
    );
    panel.focus = TabArea::Stages;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::c)),
        Some(PanelOutcome::AddComment)
    );
    // Over the repositories it isn't offered.
    panel.focus = TabArea::Repos;
    assert_eq!(press(&mut panel, &f, key(Keysym::c)), None);
    panel.focus = TabArea::Stages;
    // Off the cards Del does nothing, nor f: Alt+Shift+Del abandons.
    assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
    assert_eq!(
        press(&mut panel, &f, alt_shift(Keysym::Delete)),
        Some(PanelOutcome::Remove)
    );
    assert_eq!(press(&mut panel, &f, key(Keysym::f)), None);
    // An item of the user's: Del twice deletes it, Enter edits it.
    let added = review_import::add(f.review.as_mut().unwrap(), "Rename it").unwrap();
    (f.review.as_mut().unwrap().findings).retain(|finding| finding.id == added);
    panel.focus = TabArea::Findings;
    panel.finding = 0;
    assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::DeleteOwnFinding(added.clone()))
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::EditFinding(added))
    );
}

#[test]
fn the_card_with_the_keyboard_shows_it_and_the_hints_say_what_it_takes() {
    use FindingState as S;
    let f = carded(&[S::Open, S::Fix]);
    let panel = FeaturePanel {
        focus: TabArea::Findings,
        finding: 1,
        ..FeaturePanel::default()
    };
    let slugs = vec!["r".to_owned()];
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
    assert_eq!(view.focus, TabArea::Findings);
    let selected: Vec<bool> = cards_of(&view).iter().map(|c| c.selected).collect();
    assert_eq!(selected, [false, true]);
    for hint in [
        ("Up / Down", "finding"),
        ("f", "reopen"),
        ("Ctrl+E", "edit fix\u{2026}"),
        ("c", "comment"),
        ("Del", "dismiss\u{2026}"),
        ("Enter", "show in diff"),
        ("Tab", "repositories"),
    ] {
        let hint = (hint.0.to_owned(), hint.1.to_owned());
        assert!(view.hints.contains(&hint), "{hint:?}: {:?}", view.hints);
    }
    // Del is the card's here: it says so once.
    assert_eq!(view.hints.iter().filter(|(k, _)| k == "Del").count(), 1);
    let stages = FeaturePanel::default();
    let view = tab_view(&stages, &f, &slugs, &PanelState::default(), 2_060);
    assert!(view.hints.contains(&("Tab".into(), "findings".into())));
    assert!(cards_of(&view).iter().all(|c| !c.selected));
}

#[test]
fn code_review_shows_the_diff() {
    let f = feature("c", vec![repo("api", true)], 6);
    let row = PanelRow::Stage(Stage::CodeReview);
    let none = RowCtx::default();
    assert_eq!(row_action(&f, row, none), Some(PanelOutcome::CodeReview));
    assert_eq!(row_hint(&f, row, none), "review code");
    assert_eq!(
        stage_button(&f, Stage::CodeReview, none),
        Some("Review code")
    );
    let state = PanelState::default();
    assert_eq!(
        stage_detail(&f, Stage::CodeReview, &state, 2_060),
        "review every changed file"
    );
    let state = PanelState {
        code_review: Some("3 of 7 files viewed".into()),
        code_counts: None,
        ..PanelState::default()
    };
    assert_eq!(
        stage_detail(&f, Stage::CodeReview, &state, 2_060),
        "3 of 7 files viewed"
    );
    let actions = page_actions(&f, Stage::CodeReview, none);
    let page = stage_page(&f, Stage::CodeReview, &state, &actions, 2_060);
    assert_eq!(page.buttons[0].label, "Review code");
    let next = |text: &str| line(text, CheckTone::Note);
    assert_eq!(
        page.sections,
        [
            lines(
                "Comments",
                vec![line(
                    "none yet: write them in Code review, then plan the fixes",
                    CheckTone::Note
                )]
            ),
            lines(
                "What happens next",
                vec![
                    next(
                        "You read every changed file and mark it viewed; \
                             comments go back to Build as fixes"
                    ),
                    next("Approve once every file is viewed: Manual test is next"),
                ]
            )
        ]
    );
    // Written elsewhere there is no switch to count the files: the page does.
    let mut elsewhere = f.clone();
    elsewhere.writer = Some("m1".into());
    let page = stage_page(&elsewhere, Stage::CodeReview, &state, &actions, 2_060);
    assert_eq!(
        page.sections[0],
        lines("Files", vec![line("3 of 7 files viewed", CheckTone::Note)])
    );
}
