//! Tests of the stage pages' cards, the rail, rounds and drafts.

use super::*;

/// Shipped with only its push, a feature can open its pull request later:
/// the push shows as done on its card, the pull request as a tick box,
/// and the page offers to ship what is left.
#[test]
fn a_shipped_feature_ships_more_from_its_page() {
    use crate::feature::{RepoPlan, ShipDone};
    let dir = std::env::temp_dir().join(format!("gerdr-ship-more-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut api = repo("api", true);
    api.worktree = dir.clone();
    let mut f = feature("s", vec![api], Stage::ALL.len());
    f.status = Status::Done;
    f.ship_ran(ShipDone {
        at: 1_900,
        repo: "api".into(),
        step: ShipStep::Push,
        sha: Some("3f9c2a1deadbeef".into()),
        url: None,
        undone: None,
    });
    f.ship_plan
        .repos
        .insert("api".into(), RepoPlan::new(Route::Pr));
    assert!(f.ships() && f.ships_more());
    let mut found = check(&[], &["api"], &[]).unwrap();
    found.repos[0].head = Some("3f9c2a1deadbeef".into());
    let p = probe(Some(found));
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    assert_eq!(
        ship_rows(&f, Some(&p)).len(),
        2,
        "its way, and the pull request"
    );
    let ctx = RowCtx {
        ship_left: 1,
        ship_read: true,
        ..RowCtx::default()
    };
    let actions = page_actions(&f, Stage::Ship, ctx);
    assert_eq!(actions[0].0.label, "Ship 1 step\u{2026}");
    assert_eq!(actions[0].1, PanelOutcome::Ship);
    let page = stage_page(&f, Stage::Ship, &state, &actions, 2_000);
    assert_eq!(
        page.sections[0],
        lines(
            "Ship check",
            vec![line("Shipped: 1 step ticked to run", CheckTone::Ready)]
        )
    );
    assert!(matches!(page.sections[1], PageSection::Recap(_)));
    let Some(PageSection::ShipGroup(group)) = page.sections.get(2) else {
        panic!("no card: {:?}", page.sections);
    };
    let steps: Vec<(&str, bool)> = (group.steps.iter())
        .map(|s| (s.label.as_str(), s.done))
        .collect();
    assert_eq!(
        steps,
        [
            ("Pushed feature/s to origin", true),
            ("Open a pull request into main", false)
        ]
    );
    assert_eq!(
        group.steps[0].detail.as_ref().map(|d| d.text.as_str()),
        Some("1m ago \u{b7} 3f9c2a1")
    );
    let card = group.card.as_ref().unwrap();
    assert_eq!(card.state.text, "1 step to run");
    // Its rows say what ran: no log repeats them.
    assert!(!page.sections.iter().any(|s| {
        matches!(s, PageSection::Lines { label, .. } if label == "Ship log")
    }));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_branch_moved_since_its_push_keeps_undo_and_logs_its_update() {
    use crate::feature::{BaseUpdate, RepoPlan, ShipDone, UpdateWay};
    let dir = std::env::temp_dir().join(format!("gerdr-ship-moved-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut api = repo("api", true);
    api.worktree = dir.clone();
    let mut f = feature("s", vec![api], Stage::ALL.len());
    f.status = Status::Done;
    f.ship_ran(ShipDone {
        at: 1_900,
        repo: "api".into(),
        step: ShipStep::Push,
        sha: Some("3f9c2a1deadbeef".into()),
        url: None,
        undone: None,
    });
    f.shipping.updates.push(BaseUpdate {
        at: 1_950,
        repo: "api".into(),
        way: UpdateWay::Merge,
        onto: "origin/main".into(),
        onto_sha: "6496d98aaaa".into(),
        before: "3f9c2a1deadbeef".into(),
        after: "73462c1bbbb".into(),
    });
    f.ship_plan
        .repos
        .insert("api".into(), RepoPlan::new(Route::Pr));
    // The branch took its base in: origin has the push from before.
    let mut found = check(&[], &["api"], &[]).unwrap();
    found.repos[0].head = Some("73462c1bbbb".into());
    let p = probe(Some(found));
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    // Read not yet, its card's buttons wait for it: none sits on the
    // title's row meanwhile.
    let unread = page_actions(&f, Stage::Ship, RowCtx::default());
    assert!(
        !unread
            .iter()
            .any(|(_, o)| matches!(o, PanelOutcome::UndoShip(..)))
    );
    let ctx = RowCtx {
        ship_read: true,
        ..RowCtx::default()
    };
    let actions = page_actions(&f, Stage::Ship, ctx);
    let page = stage_page(&f, Stage::Ship, &state, &actions, 2_000);
    let Some(PageSection::ShipGroup(group)) = page.sections.get(2) else {
        panic!("no card: {:?}", page.sections);
    };
    let again = &group.steps[0];
    assert_eq!(
        again.label,
        "Push feature/s to origin again: it moved since"
    );
    assert!(!again.done);
    // Its undo is on its row, not left over for the page's title.
    let undo = again.button.expect("an undo on the row");
    assert_eq!(
        actions[undo].1,
        PanelOutcome::UndoShip("api".into(), ShipStep::Push)
    );
    // The log, with its update from the base, once no card shows.
    assert_eq!(
        ship_log_lines(&f, 2_000),
        vec![
            line(
                "just now \u{b7} api: merged origin/main \u{b7} 6496d98",
                CheckTone::Note
            ),
            line(
                "1m ago \u{b7} api: pushed feature/s \u{b7} 3f9c2a1",
                CheckTone::Note
            ),
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_workspace_shipping_left_open_can_still_close() {
    let mut f = feature("s", vec![repo("api", true)], Stage::ALL.len());
    f.status = Status::Done;
    f.dir = std::env::temp_dir().join(format!("gerdr-close-left-{}", std::process::id()));
    std::fs::create_dir_all(&f.dir).unwrap();
    let close = |f: &Feature| {
        let actions = page_actions(f, Stage::Ship, RowCtx::default());
        let page = stage_page(f, Stage::Ship, &PanelState::default(), &actions, 2_000);
        let row = page.sections.iter().find_map(|s| match s {
            PageSection::ShipGroup(g) if g.head == "Afterwards" => g
                .steps
                .iter()
                .find(|r| r.label == "Close the workspace")
                .cloned(),
            _ => None,
        });
        row.map(|row| actions[row.button.expect("its button")].1.clone())
    };
    assert_eq!(close(&f), Some(PanelOutcome::CloseShipped));
    // Kept open by its tick, or closed once already: nothing to offer.
    f.ship_plan.keep_workspace = true;
    assert_eq!(close(&f), None);
    f.ship_plan.keep_workspace = false;
    f.after_ran(crate::feature::AfterDone {
        at: 1_950,
        step: crate::feature::AfterStep::Close,
        tabs: Vec::new(),
        heads: Vec::new(),
        undone: Some(1_960),
    });
    assert_eq!(close(&f), None);
    f.shipping.after.clear();
    std::fs::remove_dir_all(&f.dir).unwrap();
    // Its folder gone: no workspace to close.
    assert_eq!(close(&f), None);
}

#[test]
fn what_followed_shipping_shows_with_its_undo() {
    use crate::feature::{AfterDone, AfterStep, KeptHead, RepoPlan, ShipDone};
    let dir = std::env::temp_dir().join(format!("gerdr-ship-after-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut api = repo("api", true);
    api.worktree = dir.clone();
    let mut f = feature("s", vec![api], Stage::ALL.len());
    f.status = Status::Done;
    f.ship_ran(ShipDone {
        at: 1_900,
        repo: "api".into(),
        step: ShipStep::Push,
        sha: Some("3f9c2a1deadbeef".into()),
        url: None,
        undone: None,
    });
    f.ship_plan
        .repos
        .insert("api".into(), RepoPlan::new(Route::Pr));
    let after = |at, step, heads: Vec<KeptHead>| AfterDone {
        at,
        step,
        tabs: Vec::new(),
        heads,
        undone: None,
    };
    f.after_ran(after(1_950, AfterStep::Close, Vec::new()));
    let mut found = check(&[], &["api"], &[]).unwrap();
    found.repos[0].head = Some("3f9c2a1deadbeef".into());
    let p = probe(Some(found));
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let ctx = RowCtx {
        ship_read: true,
        ..RowCtx::default()
    };
    let afterwards = |f: &Feature, actions: &[(PageButton, PanelOutcome)]| {
        let page = stage_page(f, Stage::Ship, &state, actions, 2_000);
        let group = page.sections.iter().find_map(|s| match s {
            PageSection::ShipGroup(g) if g.head == "Afterwards" => Some(g.clone()),
            _ => None,
        });
        (group.expect("an Afterwards group"), page)
    };
    // Its tabs come back first: the latest that can be undone.
    let actions = page_actions(&f, Stage::Ship, ctx);
    let undos: Vec<&PanelOutcome> = (actions.iter())
        .map(|(_, o)| o)
        .filter(|o| matches!(o, PanelOutcome::UndoShip(..) | PanelOutcome::UndoAfter(_)))
        .collect();
    assert_eq!(
        undos,
        [
            &PanelOutcome::UndoAfter(AfterStep::Close),
            &PanelOutcome::UndoShip("api".into(), ShipStep::Push)
        ]
    );
    let (group, page) = afterwards(&f, &actions);
    assert_eq!(group.steps.len(), 1);
    assert_eq!(group.steps[0].label, "Closed the workspace");
    assert!(group.steps[0].done);
    let button = group.steps[0].button.expect("its undo");
    assert_eq!(actions[button].1, PanelOutcome::UndoAfter(AfterStep::Close));
    assert_eq!(actions[button].0.label, "Undo");
    // A step's undo names it: two never read the same off their rows.
    let push = PanelOutcome::UndoShip("api".into(), ShipStep::Push);
    let named = actions.iter().find(|(_, o)| *o == push).unwrap();
    assert_eq!(named.0.label, "Undo push\u{2026}");
    // Its cards and Afterwards say what ran: the log isn't shown again.
    assert!(!page.sections.iter().any(|s| {
        matches!(s, PageSection::Lines { label, .. } if label == "Ship log")
    }));
    let log = ship_log_lines(&f, 2_000);
    assert_eq!(log[0].text, "just now \u{b7} closed the workspace");
    // Alt+Z: off the rows, the latest; on api's, its push.
    let alt_z = |repo: Option<&str>| {
        let keys = ship_keys(&f, ctx, Some(&p), repo);
        let key = keys.into_iter().find(|k| k.chord == "Alt+Z").unwrap();
        (key.what, key.outcome)
    };
    // The hint says what its button says.
    assert_eq!(
        alt_z(None),
        ("undo".to_owned(), PanelOutcome::UndoAfter(AfterStep::Close))
    );
    assert_eq!(
        alt_z(Some("api")).1,
        PanelOutcome::UndoShip("api".into(), ShipStep::Push)
    );

    // Its worktrees gone since, they come back first, then its tabs.
    let head = KeptHead {
        repo: "api".into(),
        sha: "3f9c2a1deadbeef".into(),
        changes: None,
    };
    f.after_ran(after(1_990, AfterStep::Remove, vec![head]));
    let why = crate::ship_after::after_refusal(&f, AfterStep::Close).unwrap();
    assert_eq!(
        why,
        "The worktrees of \u{201c}Feature s\u{201d} are gone: restore them first"
    );
    let actions = page_actions(&f, Stage::Ship, ctx);
    assert!(
        actions
            .iter()
            .any(|(_, o)| *o == PanelOutcome::UndoAfter(AfterStep::Remove))
    );
    assert!(
        !actions
            .iter()
            .any(|(_, o)| *o == PanelOutcome::UndoAfter(AfterStep::Close))
    );
    f.after_undid(AfterStep::Remove, 1_995);
    assert!(f.after_done(AfterStep::Remove).is_none());
    let actions = page_actions(&f, Stage::Ship, ctx);
    let (group, _) = afterwards(&f, &actions);
    let labels: Vec<(&str, Option<&str>)> = (group.steps.iter())
        .map(|s| (s.label.as_str(), s.detail.as_ref().map(|d| d.text.as_str())))
        .collect();
    assert_eq!(
        labels,
        [
            ("Closed the workspace", Some("just now")),
            (
                "Removed the worktree and branch",
                Some("just now \u{b7} restored just now")
            ),
        ]
    );
    assert!(group.steps[1].button.is_none());
    assert!(group.steps[0].button.is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ships_buttons_have_keys_that_follow_the_selected_repository() {
    use crate::feature::{Pr, RepoPlan, ShipDone, UpdateWay};
    let root = std::env::temp_dir().join(format!("gerdr-ship-keys-{}", std::process::id()));
    let mut repos = Vec::new();
    for name in ["api", "web"] {
        let mut r = repo(name, true);
        r.worktree = root.join(name);
        std::fs::create_dir_all(&r.worktree).unwrap();
        repos.push(r);
    }
    let mut f = feature("s", repos, Stage::ALL.len());
    f.status = Status::Done;
    for name in ["api", "web"] {
        f.ship_ran(ShipDone {
            at: 1_900,
            repo: name.into(),
            step: ShipStep::Push,
            sha: Some(format!("{name}-head")),
            url: None,
            undone: None,
        });
        f.ship_plan
            .repos
            .insert(name.into(), RepoPlan::new(Route::Pr));
    }
    f.shipping.prs.push(Pr {
        repo: "web".into(),
        url: "https://github.com/o/web/pull/7".into(),
    });
    let mut found = check(&[], &["api", "web"], &[]).unwrap();
    found.repos[0].head = Some("api-head".into());
    found.repos[1].head = Some("web-head".into());
    let mut p = probe(Some(found));
    // api's base conflicts, web's merges cleanly.
    p.bases.insert(
        "api".into(),
        ship::BaseState {
            onto: "origin/main".into(),
            behind: 2,
            conflicts: vec!["a.rs".into()],
            ..ship::BaseState::default()
        },
    );
    p.bases.insert(
        "web".into(),
        ship::BaseState {
            onto: "origin/main".into(),
            behind: 1,
            ..ship::BaseState::default()
        },
    );
    let ctx = RowCtx {
        ship_read: true,
        ..RowCtx::default()
    };
    let of = |repo: Option<&str>| -> Vec<(&'static str, PanelOutcome)> {
        (ship_keys(&f, ctx, Some(&p), repo).into_iter())
            .map(|k| (k.chord, k.outcome))
            .collect()
    };
    // No row: the first card's, api's conflicts.
    assert_eq!(
        of(None),
        [
            (
                "Ctrl+M",
                PanelOutcome::ResolveConflicts("api".into(), false)
            ),
            (
                "Ctrl+Shift+M",
                PanelOutcome::UpdateBase("web".into(), UpdateWay::Rebase)
            ),
            ("Alt+M", PanelOutcome::ResolveConflicts("api".into(), true)),
            (
                "Ctrl+Shift+O",
                PanelOutcome::OpenPr("https://github.com/o/web/pull/7".into())
            ),
            // The latest step: web's push ran after api's.
            (
                "Alt+Z",
                PanelOutcome::UndoShip("web".into(), ShipStep::Push)
            ),
            ("Ctrl+R", PanelOutcome::WriteSummary),
            ("Ctrl+E", PanelOutcome::EditSummary),
        ]
    );
    let undo = ship_keys(&f, ctx, Some(&p), None);
    let undo = undo.iter().find(|k| k.chord == "Alt+Z").unwrap();
    // As its button says it, naming the step and its repository.
    assert_eq!(undo.what, "undo push in web\u{2026}");
    // On web's rows: web's merge, its pull request, its push.
    let web = of(Some("web"));
    assert!(web.contains(&(
        "Ctrl+M",
        PanelOutcome::UpdateBase("web".into(), UpdateWay::Merge)
    )));
    assert!(web.contains(&(
        "Alt+Z",
        PanelOutcome::UndoShip("web".into(), ShipStep::Push)
    )));
    let api = of(Some("api"));
    assert!(api.contains(&(
        "Alt+Z",
        PanelOutcome::UndoShip("api".into(), ShipStep::Push)
    )));
    assert!(
        !web.iter().any(|(chord, _)| *chord == "Alt+M"),
        "web has no conflicts"
    );
    // Keys come as their own symbol (checklist K8).
    let keys = ship_keys(&f, ctx, Some(&p), None);
    let pressed = |ctrl, shift, alt, sym| {
        let mods = ModifiersState {
            ctrl,
            shift,
            alt,
            ..ModifiersState::default()
        };
        (keys.iter())
            .find(|k| crate::key_table::is(k.chord, &mods, sym))
            .map(|k| k.chord)
    };
    assert_eq!(pressed(true, false, false, Keysym::m), Some("Ctrl+M"));
    assert_eq!(pressed(true, true, false, Keysym::m), Some("Ctrl+Shift+M"));
    assert_eq!(pressed(false, false, true, Keysym::m), Some("Alt+M"));
    assert_eq!(pressed(false, false, true, Keysym::z), Some("Alt+Z"));
    assert_eq!(pressed(false, false, false, Keysym::m), None);
    assert_eq!(pressed(true, false, false, Keysym::z), None);
    // While an agent resolves: stop it by the key that abandons, and the
    // footer says so in place of starting over.
    f.shipping.resolving = Some(crate::feature::Resolving {
        at: 1,
        repo: "api".into(),
        onto: "origin/main".into(),
        onto_sha: "b".into(),
        before: "a".into(),
        files: vec!["a.rs".into()],
        agent: "claude".into(),
        pane: None,
        n: 1,
        planning: false,
        nudged: false,
    });
    let keys = ship_keys(&f, ctx, Some(&p), None);
    assert!(
        keys.iter()
            .any(|k| k.chord == "Ctrl+Shift+Del" && k.outcome == PanelOutcome::StopResolving)
    );
    let panel = FeaturePanel::default();
    let hints = vec![
        ("Ctrl+Shift+Del".to_owned(), "start over\u{2026}".to_owned()),
        ("F1".to_owned(), "keys".to_owned()),
    ];
    let ships = ship_rows(&f, Some(&p));
    let hints = with_ship_hints(hints, &panel, &f, Stage::Ship, (ctx, Some(&p)), &ships);
    let stop = (hints.iter())
        .filter(|(k, _)| k == "Ctrl+Shift+Del")
        .collect::<Vec<_>>();
    assert_eq!(
        stop,
        [&("Ctrl+Shift+Del".to_owned(), "stop\u{2026}".to_owned())]
    );
    assert_eq!(hints.last().unwrap().0, "F1", "before F1");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_blocked_ship_says_what_stops_it() {
    let f = feature("s", vec![repo("api", true)], 8);
    let p = probe(check(&["api has uncommitted changes"], &["api"], &[]));
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Ship, RowCtx::default());
    assert_eq!(
        labels(&actions),
        [
            ("Ship\u{2026}", PanelOutcome::Ship),
            ("Write summary", PanelOutcome::WriteSummary),
            ("Edit", PanelOutcome::EditSummary)
        ]
    );
    let page = stage_page(&f, Stage::Ship, &state, &actions, 2_000);
    assert_eq!(
        page.sections[0],
        lines(
            "Ship check",
            vec![
                line("api has uncommitted changes", CheckTone::Failed),
                line(
                    "\u{2713} checks passed \u{b7} review done \u{b7} no conflicts",
                    CheckTone::Ready
                )
            ]
        )
    );
    // Below it, what shipping would do, and what comes after.
    let heads: Vec<&str> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::ShipGroup(group) => Some(group.head.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(heads, ["api", "Afterwards"]);
    let card = (page.sections.iter())
        .find_map(|s| match s {
            PageSection::ShipGroup(group) => group.card.clone(),
            _ => None,
        })
        .expect("a card");
    assert_eq!(card.meta, "1 commit \u{b7} into main");
    let node = rail_node(&f, Stage::Ship, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Failed, "blocked")
    );
    let ready = probe(check(&[], &["api"], &[]));
    let state = PanelState {
        probe: Some(&ready),
        ..PanelState::default()
    };
    assert_eq!(
        rail_node(&f, Stage::Ship, &state, 2_000).line,
        "ready to ship"
    );
}

#[test]
fn a_cards_line_says_the_stage_and_how_it_stands_else_how_long_it_sat() {
    let state = LineState::from(&PanelState::default());
    let now = 10 * 86_400;
    let mut f = feature("d", vec![repo("api", true)], 2);
    f.created = 0;
    f.history = vec![feature::Event {
        at: now - 3 * 86_400,
        what: "Plan ticked".into(),
    }];
    assert_eq!(
        card_line(&f, Some(Stage::Build), &state, now),
        format!("Build \u{b7} {}", crate::clock::ago(now, now - 3 * 86_400)),
    );
    let setting_up = LineState {
        setup: Some("npm ci"),
        ..LineState::from(&PanelState::default())
    };
    assert_eq!(
        card_line(&f, Some(Stage::Build), &setting_up, now),
        "Build \u{b7} setting up",
    );
    assert_eq!(card_line(&f, None, &state, now), "Every stage done");
}

#[test]
fn a_card_says_what_the_rail_says_of_work_sent_back_to_build() {
    use chrome::CardTone::{Waits, Working};
    let state = LineState::from(&PanelState::default());
    let mut f = feature("d", vec![repo("api", true)], 5);
    f.verify = Some(feature::verify::VerifyRun {
        passed: Some(false),
        ..Default::default()
    });
    let status = |back| card_status(&f, &state, CardHold::default(), back, 0);
    let runs = status(Some(Rework::Runs(Dot::Working)));
    assert_eq!(runs.0, Some(Stage::Build));
    assert_eq!(runs.1, "Build \u{b7} working again");
    assert_eq!(runs.2, Working);
    let waits = status(Some(Rework::Waits));
    assert_eq!(waits.1, "Build \u{b7} waiting to send");
    assert_eq!(waits.2, Waits);
    assert_eq!(status(None).0, f.current());
}

#[test]
fn a_fix_round_sent_back_to_build_by_its_commits_reads_the_same_everywhere() {
    let state = LineState::from(&PanelState::default());
    let verify = feature("d", vec![repo("api", true)], stage_index(Stage::Verify));
    let mut f = in_round(verify, Stage::Verify, FixPhase::Fixing);
    let status = |f: &Feature| card_status(f, &state, CardHold::default(), None, 0).1;
    assert_eq!(status(&f), "Verify \u{b7} making fixes");
    // Its first commit has Build to do again: the rail's node is Build's.
    f.untick(Stage::Build, 5);
    assert_eq!(f.current(), Some(Stage::Build));
    assert_eq!(status(&f), "Build \u{b7} making fixes");
    // Its agent resting is the round's to end, not a Build stopped short.
    let run = feature::agents::StageRun {
        pane: Some("p".into()),
        started: 1,
        ..Default::default()
    };
    let idle = Some(AgentStatus::Idle);
    let read = crate::feature_agents::as_read(&f, Stage::Build, &run);
    assert_eq!(
        crate::feature_agents::agent_standing(&read, Stage::Build, idle, 9),
        chrome::Standing::Done
    );
    let waits = crate::feature_agents::waits_on_you(&read, Stage::Build, idle);
    assert!(!waits);
}

#[test]
fn an_agent_gone_from_its_tab_reads_failed_on_the_rail_and_the_card() {
    let mut f = feature("d", vec![repo("api", true)], stage_index(Stage::Plan));
    let run = feature::agents::StageRun {
        pane: Some("p".into()),
        ..Default::default()
    };
    f.runs.insert(Stage::Plan, run);
    let state = LineState {
        agent: Some(chrome::Standing::Failed),
        ..LineState::from(&PanelState::default())
    };
    assert_eq!(current_line(&f, Stage::Plan, &state, 0), "failed");
    let tone = card_tone(&f, Some(Stage::Plan), &state, CardHold::default());
    assert_eq!(tone, chrome::CardTone::Failed);
    let ctx = RowCtx::default();
    assert_eq!(node_mark(&f, Stage::Plan, ctx, &state), NodeMark::Failed);
}

#[test]
fn an_approval_logs_one_line_and_its_toast_says_it_and_what_is_next() {
    let mut f = feature("d", vec![repo("api", true)], 1);
    assert_eq!(f.current(), Some(Stage::Plan));
    let before = f.history.len();
    f.tick(5, "you", None);
    assert_eq!(f.history.len(), before + 1);
    assert_eq!(
        done_toast(&f, Stage::Plan),
        "Plan approved \u{b7} Plan review is next"
    );
    f.tick_said(
        6,
        "gerdr",
        None,
        "done: every milestone ticked and committed",
    );
    assert_eq!(
        done_toast(&f, Stage::PlanReview),
        "Plan review done: every milestone ticked and committed \u{b7} Build is next"
    );
}

#[test]
fn a_cards_tone_puts_a_failure_over_a_gate_over_work() {
    use chrome::CardTone::{Failed, Idle, Waits, Working};
    let state = LineState::from(&PanelState::default());
    let f = feature("d", vec![repo("api", true)], 2);
    let tone = |gate, busy| card_tone(&f, Some(Stage::Build), &state, CardHold { gate, busy });
    assert_eq!(tone(false, false), Idle);
    assert_eq!(tone(false, true), Working);
    assert_eq!(tone(true, true), Waits);
    let mut verify = feature("d", vec![repo("api", true)], 3);
    verify.verify = Some(feature::verify::VerifyRun {
        passed: Some(false),
        ..Default::default()
    });
    let hold = CardHold {
        gate: true,
        busy: true,
    };
    assert_eq!(
        card_tone(&verify, Some(Stage::Verify), &state, hold),
        Failed
    );
}

#[test]
fn a_done_stage_says_when_and_reopens_and_a_later_one_waits() {
    let mut f = feature("d", vec![repo("api", true)], 2);
    f.marks.get_mut(&Stage::Plan).unwrap().note = "looks good".into();
    let mut built = feature("d", vec![repo("api", true)], 4);
    built.marks.get_mut(&Stage::Plan).unwrap().note = "looks good".into();
    let state = PanelState::default();
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    assert_eq!(
        labels(&actions),
        [
            ("Go to Plan review", PanelOutcome::GoToCurrent),
            ("Reopen", PanelOutcome::Untick(Stage::Plan)),
        ]
    );
    assert_eq!(actions[0].0.style, ButtonStyle::Primary);
    assert_eq!(actions[1].0.style, ButtonStyle::Plain);
    let page = stage_page(&f, Stage::Plan, &state, &actions, 2_060);
    assert_eq!(
        page.status,
        Some(line("Done 1m ago by you", CheckTone::Ready))
    );
    assert_eq!(
        page.sections[0],
        lines("Note", vec![line("looks good", CheckTone::Note)])
    );
    // Code changed since Build was marked: stale, Discover not.
    let f = built;
    let p = probe(None);
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let node = rail_node(&f, Stage::Build, &state, 2_060);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Stale, "code changed")
    );
    assert_eq!(
        rail_node(&f, Stage::Discover, &state, 2_060).mark,
        NodeMark::Done
    );
    let page = stage_page(&f, Stage::Build, &state, &[], 2_060);
    assert!(page.sections.contains(&lines(
        "Stale",
        vec![line(
            "The code changed since it was marked",
            CheckTone::Blocker
        )]
    )));
    // A later stage: what it is for, what comes first, no buttons.
    assert!(page_actions(&f, Stage::Ship, RowCtx::default()).is_empty());
    let page = stage_page(&f, Stage::Ship, &state, &[], 2_060);
    assert_eq!(page.about, Stage::Ship.about());
    assert_eq!(
        page.status,
        Some(line("Comes after Manual test", CheckTone::Note))
    );
    assert!(page.sections.is_empty() && page.buttons.is_empty());
    assert_eq!(
        page_enter(&f, Stage::Ship, RowCtx::default()),
        Some(PanelOutcome::NotYet(Stage::Verify))
    );
}

#[test]
fn a_tick_said_with_its_reason_shows_once() {
    let mut f = feature("a", vec![repo("api", true)], 0);
    for (at, what) in [
        (1_000, "Plan approved"),
        (1_000, "Plan ticked"),
        (1_200, "Code review ticked"),
        (1_300, "Verify run 2 passed"),
        (1_301, "Verify ticked"),
    ] {
        f.history.push(Event {
            at,
            what: what.into(),
        });
    }
    let rows: Vec<String> = activity(&f, 1_400).into_iter().map(|r| r.what).collect();
    assert_eq!(
        rows,
        ["Verify run 2 passed", "Code review ticked", "Plan approved"]
    );
}

#[test]
fn the_activity_is_newest_first_in_its_tones() {
    let mut f = feature("a", vec![repo("api", true)], 0);
    for (at, what) in [
        (1_000, "Brief ticked"),
        (1_100, "Plan sent back (1)"),
        (1_200, "Verify run 1 failed: api: npm test"),
        (1_300, "touched web"),
        (1_400, "Set up"),
        (1_500, "Code changed: Build to do again"),
        (1_600, "Verify run 2 passed"),
    ] {
        f.history.push(Event {
            at,
            what: what.into(),
        });
    }
    let rows = activity(&f, 1_660);
    let got: Vec<(&str, &str, CheckTone)> = (rows.iter())
        .map(|r| (r.what.as_str(), r.ago.as_str(), r.tone))
        .collect();
    assert_eq!(
        got,
        [
            ("Verify run 2 passed", "1m ago", CheckTone::Ready),
            (
                "Code changed: Build to do again",
                "2m ago",
                CheckTone::Blocker
            ),
            ("Set up", "4m ago", CheckTone::Ready),
            ("Touched web", "6m ago", CheckTone::Note),
            (
                "Verify run 1 failed: api: npm test",
                "7m ago",
                CheckTone::Failed
            ),
            ("Plan sent back (1)", "9m ago", CheckTone::Blocker),
            ("Brief ticked", "11m ago", CheckTone::Ready),
        ]
    );
    assert_eq!(event_tone("Setup started"), CheckTone::Note);
}

#[test]
fn every_gate_decision_reads_in_its_tone() {
    for (what, tone) in [
        ("Plan agent started: Claude Code (run 1)", CheckTone::Note),
        ("Plan ready for review", CheckTone::Note),
        ("Plan approved", CheckTone::Ready),
        ("Brief sent back (2)", CheckTone::Blocker),
        ("Verify run 3 passed", CheckTone::Ready),
        ("Verify run 3 failed: api: npm test", CheckTone::Failed),
        (
            "Verify's failures sent to the Build agent (fix 1 of 3)",
            CheckTone::Note,
        ),
        ("Sent 2 findings to the Build agent", CheckTone::Note),
        (
            "Opened 1 pull request for \u{201c}T\u{201d}",
            CheckTone::Ready,
        ),
        ("Build stopped after milestone 1 of 3", CheckTone::Note),
        ("Shipping stopped: no gh", CheckTone::Blocker),
        (
            "Plan refused: the agent changed the code in api",
            CheckTone::Failed,
        ),
        (
            "Build agent couldn't start: no coding agent CLI installed",
            CheckTone::Failed,
        ),
    ] {
        assert_eq!(event_tone(what), tone, "{what}");
    }
}

#[test]
fn a_finished_features_tab_shows_no_code_error_nor_stale_marks() {
    let mut f = feature("d", vec![repo("api", true)], 9);
    f.status = Status::Done;
    let p = Probe {
        stats: BTreeMap::from([("api".to_owned(), feature::page::Stat::default())]),
        ..ship::probe_finished(&f)
    };
    assert!(p.reading.is_err());
    assert_eq!(stale_from(&f, Some(&p)), None);
    let mut q = probe(None);
    q.reading = Ok(feature::patch::Reading {
        patch: "changed".into(),
        ..Default::default()
    });
    assert_eq!(stale_from(&f, Some(&q)), None, "its marks stay as shipped");
    f.status = Status::Open;
    assert!(stale_from(&f, Some(&q)).is_some(), "an open one's go stale");
}

#[test]
fn a_written_plan_offers_review() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    f.runs.insert(Stage::Plan, agent_run(None, None));
    // Not there yet: no button.
    let none = page_actions(&f, Stage::Plan, RowCtx::default());
    assert!(
        !none
            .iter()
            .any(|(_, o)| *o == PanelOutcome::OpenDoc(Stage::Plan))
    );
    let written = RowCtx {
        written: [true; Stage::ALL.len()],
        ..RowCtx::default()
    };
    let actions = page_actions(&f, Stage::Plan, written);
    let got = labels(&actions);
    assert!(got.contains(&("Review", PanelOutcome::OpenDoc(Stage::Plan))));
    assert!(!got.iter().any(|(label, _)| *label == "Preview"));
    // Ready for review: the primary reviews, no second Review.
    f.runs.get_mut(&Stage::Plan).unwrap().ready = Some(1_500);
    let actions = page_actions(&f, Stage::Plan, written);
    let reviews = (actions.iter())
        .filter(|(_, o)| *o == PanelOutcome::OpenDoc(Stage::Plan))
        .count();
    assert_eq!(reviews, 1);
    // The primary Review goes to the page's bar, Approve stays on the card.
    let held = actions
        .iter()
        .position(|(_, o)| *o == PanelOutcome::OpenDoc(Stage::Plan))
        .unwrap();
    assert_eq!(actions[held].0.style, ButtonStyle::Primary);
    let approve = actions
        .iter()
        .position(|(_, o)| *o == PanelOutcome::ApproveDoc(Stage::Plan))
        .unwrap();
    let page = stage_page(&f, Stage::Plan, &PanelState::default(), &actions, 2_000);
    let card = page
        .sections
        .iter()
        .find(|s| matches!(s, PageSection::File { .. }));
    assert_eq!(
        card,
        Some(&file(
            "plan.md",
            line("ready for review", CheckTone::Blocker),
            &[approve]
        ))
    );
    assert_eq!(page.primary, Some(held));
}

/// A file reads on its stage's page, under all else, once there is one
/// to read; a stage not reached shows none of it.
#[test]
fn a_stage_page_reads_its_file_last() {
    use crate::md_blocks::DocBlock;
    use std::sync::Arc;
    let blocks = Arc::new(vec![DocBlock::Para("why".to_owned())]);
    let mut p = probe(None);
    p.docs.previews.insert(Stage::Plan, blocks.clone());
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let doc = PageSection::Doc {
        name: "plan.md".to_owned(),
        blocks,
        here: false,
    };
    let has_doc =
        |page: &StagePage| (page.sections.iter()).any(|s| matches!(s, PageSection::Doc { .. }));
    // Not reached yet.
    let later = feature("b", vec![repo("api", true)], 0);
    assert!(!has_doc(&stage_page(
        &later,
        Stage::Plan,
        &state,
        &[],
        2_000
    )));
    // The stage it is at, and one it is past.
    for at in [1, 2] {
        let mut f = feature("b", vec![repo("api", true)], at);
        f.runs.insert(Stage::Plan, agent_run(None, None));
        let page = stage_page(&f, Stage::Plan, &state, &[], 2_000);
        assert_eq!(page.sections.last(), Some(&doc), "at stage {at}");
    }
    // Nothing read: nothing to show.
    let mut f = feature("b", vec![repo("api", true)], 1);
    f.runs.insert(Stage::Plan, agent_run(None, None));
    let none = stage_page(&f, Stage::Plan, &PanelState::default(), &[], 2_000);
    assert!(!has_doc(&none));
    // A stage with no file of its own reads none, whatever was read.
    let mut p = probe(None);
    let read = Arc::new(vec![DocBlock::Para("why".to_owned())]);
    p.docs.previews.insert(Stage::Build, read);
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let f = feature("b", vec![repo("api", true)], 3);
    assert!(!has_doc(&stage_page(&f, Stage::Build, &state, &[], 2_000)));
}

/// A brief ready for review: its page's one review is "Review",
/// and nothing offers a preview of what the page shows.
#[test]
fn a_ready_brief_has_one_review_and_no_preview() {
    let mut f = feature("b", vec![repo("api", true)], 0);
    f.runs.insert(Stage::Discover, agent_run(None, None));
    f.runs.get_mut(&Stage::Discover).unwrap().ready = Some(1_500);
    let written = RowCtx {
        written: [true; Stage::ALL.len()],
        ..RowCtx::default()
    };
    let actions = page_actions(&f, Stage::Discover, written);
    let got = labels(&actions);
    assert!(got.iter().any(|(label, _)| *label == "Review"));
    assert!(!got.iter().any(|(label, _)| label.contains("Preview")));
    let reviews = (actions.iter())
        .filter(|(_, o)| *o == PanelOutcome::OpenDoc(Stage::Discover))
        .count();
    assert_eq!(reviews, 1);
}

#[test]
fn the_build_page_shows_its_log_by_name_and_its_agent_tail() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    f.runs.insert(Stage::Build, agent_run(Some("p1"), None));
    f.runs.get_mut(&Stage::Build).unwrap().stop = Some(feature::agents::Stop {
        done: 1,
        total: 3,
        at: 1_900,
    });
    let mut p = probe(None);
    let items = vec![
        (true, "API".to_owned()),
        (true, "UI".to_owned()),
        (false, "Docs".to_owned()),
    ];
    p.docs.milestones.insert(Stage::Build, items.clone());
    let tail = vec!["Editing src/a.rs".to_owned(), "Running tests".to_owned()];
    let state = PanelState {
        probe: Some(&p),
        tail: &tail,
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Build, RowCtx::default());
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    // The log read beats the counts of the last stop.
    assert_eq!(
        page.sections.last(),
        Some(&PageSection::Milestones {
            label: "Build".to_owned(),
            state: None,
            done: 2,
            total: 3,
            items,
            running: Vec::new(),
            buttons: Vec::new(),
        })
    );
    let PageSection::Agent { tail: shown, .. } = &page.sections[0] else {
        panic!("the agent's card first");
    };
    // Its last lines, all of them while it waits on the user.
    assert!(!shown.is_empty() && tail.ends_with(shown), "{shown:?}");
    // Another stage's card has no tail: it is the current agent's.
    let plan = stage_page(&f, Stage::Plan, &state, &[], 2_000);
    assert!(!plan.sections.iter().any(|s| matches!(
        s,
        PageSection::Agent { tail, .. } if !tail.is_empty()
    )));
    // Each fix round's list comes above, newest first, under its own label.
    let round = |n, items: &[(bool, &str)]| {
        (
            n,
            items.iter().map(|(d, t)| (*d, (*t).to_owned())).collect(),
        )
    };
    let mut read = p.clone();
    read.docs.fix_rounds = vec![
        round(1, &[(true, "r1-1"), (true, "r1-2")]),
        round(3, &[(false, "r3-1")]),
    ];
    // Verify's failures started this one.
    f.verify_fixes = vec![verify_fix(3, 2)];
    let state = PanelState {
        probe: Some(&read),
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    let groups: Vec<(&str, usize, usize)> = (page.sections.iter())
        .filter_map(|section| match section {
            PageSection::Milestones {
                label, done, total, ..
            } => Some((label.as_str(), *done, *total)),
            PageSection::Round(card) => Some((
                card.label.as_str(),
                card.fixes.iter().filter(|(d, _)| *d).count(),
                card.fixes.len(),
            )),
            _ => None,
        })
        .collect();
    // A round known only by the build log has its card all the same.
    assert_eq!(
        groups,
        [
            ("Fix round 3", 0, 1),
            ("Fix round 1", 2, 2),
            ("Build", 2, 3)
        ]
    );
}

#[test]
fn what_blinks_on_builds_page_is_what_the_agent_is_at_work_on() {
    let items: Vec<(bool, String)> = [(true, "a"), (false, "b"), (false, "c"), (false, "d")]
        .map(|(d, t)| (d, t.to_owned()))
        .into();
    // Resting, it is on nothing, whatever the log says.
    assert!(items_running(&items, &[2], false).is_empty());
    // By its marks, several at once.
    assert_eq!(items_running(&items, &[1, 3], true), [1, 3]);
    // An agent that doesn't mark: the first it hasn't done.
    assert_eq!(items_running(&items, &[], true), [1]);
    assert!(items_running(&items[..1], &[], true).is_empty());
}

#[test]
fn a_done_stage_on_the_rail_says_what_ran_it() {
    let run = |agent: &str, model: Option<&str>| feature::agents::StageRun {
        agent: agent.to_owned(),
        model: model.map(str::to_owned),
        ..feature::agents::StageRun::default()
    };
    let mut f = feature("b", vec![repo("api", true)], 2);
    let state = PanelState::default();
    let agent_of = |f: &Feature, stage| rail_node(f, stage, &state, 2_000).agent;
    // Done with no agent, as by hand: nothing to say.
    assert_eq!(agent_of(&f, Stage::Discover), None);
    // Its model, without the word its logo says; else the agent's name.
    f.runs
        .insert(Stage::Discover, run("Claude Code", Some("claude-opus-5-5")));
    f.runs.insert(Stage::Plan, run("Codex", None));
    assert_eq!(
        agent_of(&f, Stage::Discover),
        Some((AgentIcon::Claude, "opus-5-5".to_owned()))
    );
    assert_eq!(
        agent_of(&f, Stage::Plan),
        Some((AgentIcon::Codex, "Codex".to_owned()))
    );
    // Not done yet: it is on the page, and its line says how it goes.
    f.runs.insert(Stage::Build, run("Cursor", Some("grok-4.7")));
    assert_eq!(agent_of(&f, Stage::Build), None);
}

#[test]
fn an_agent_card_of_a_stage_gone_by_names_its_model() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    let run = feature::agents::StageRun {
        agent: "Cursor".to_owned(),
        model: Some("grok-4.7".to_owned()),
        started: 1_000,
        ..feature::agents::StageRun::default()
    };
    f.runs.insert(Stage::Discover, run.clone());
    f.runs.insert(Stage::Plan, run);
    let state = PanelState {
        agent: Some("Cursor working".into()),
        ..PanelState::default()
    };
    let status_of = |stage| match agent_card(&f, stage, &state, &[], 2_000) {
        Some(PageSection::Agent { status, .. }) => status,
        other => panic!("no card: {other:?}"),
    };
    assert!(
        status_of(Stage::Discover).starts_with("grok-4.7 \u{b7} started "),
        "{}",
        status_of(Stage::Discover)
    );
    // The current stage's says what it does, as its status line does.
    assert_eq!(status_of(Stage::Plan), "working");
}

#[test]
fn a_repository_row_shows_its_line_stat_once_read() {
    let f = feature("b", vec![repo("api", true), repo("web", true)], 3);
    let panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let slugs = vec!["b".to_owned()];
    let mut p = probe(None);
    let stat = feature::page::Stat {
        files: 2,
        added: 12,
        removed: 3,
    };
    p.stats.insert(f.repos[0].key(), stat);
    let state = PanelState {
        probe: Some(&p),
        ..PanelState::default()
    };
    let view = tab_view(&panel, &f, &slugs, &state, 2_060);
    let stats: Vec<_> = view.repos.iter().map(|r| r.stat).collect();
    // The one not read shows none, not a zero.
    assert_eq!(stats, [Some(stat), None]);
}

#[test]
fn ctrl_shift_b_flips_the_milestone_choice_on_builds_page_only() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let mods = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    let ctx = RowCtx::default();
    let mut panel = FeaturePanel {
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let press =
        |p: &mut FeaturePanel, f: &Feature| p.key(&mods, Keysym::B, f, &[], ctx, &[], &[], &[]);
    assert_eq!(
        press(&mut panel, &f),
        Some(PanelOutcome::FreshMilestones(true))
    );
    f.fresh_milestones = true;
    assert_eq!(
        press(&mut panel, &f),
        Some(PanelOutcome::FreshMilestones(false))
    );
    panel.pick(&f, Stage::Plan);
    assert_eq!(press(&mut panel, &f), None);
}

#[test]
fn only_an_open_build_offers_the_milestone_choice() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    assert!(milestone_switch(&f, Stage::Build));
    assert!(!milestone_switch(&f, Stage::Plan));
    let state = PanelState::default();
    let mode = |f: &Feature| {
        let page = stage_page(f, Stage::Build, &state, &[], 2_000);
        page.sections.into_iter().find_map(|s| match s {
            PageSection::MilestoneMode { fresh } => Some(fresh),
            _ => None,
        })
    };
    assert_eq!(mode(&f), Some(false));
    f.fresh_milestones = true;
    assert_eq!(mode(&f), Some(true));
    f.status = Status::Abandoned;
    assert!(!milestone_switch(&f, Stage::Build));
    assert_eq!(mode(&f), None);
}

#[test]
fn a_blocked_build_says_why_on_its_page_and_rail() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let mut run = agent_run(Some("p1"), None);
    run.blocked = Some(feature::agents::Blocked {
        why: "the plan names no such table".into(),
        at: 1_000,
    });
    f.runs.insert(Stage::Build, run);
    let state = PanelState::default();
    let page = stage_page(&f, Stage::Build, &state, &[], 2_000);
    assert_eq!(
        page.sections.first(),
        Some(&lines(
            "Blocked",
            vec![line("the plan names no such table", CheckTone::Failed)]
        ))
    );
    let node = rail_node(&f, Stage::Build, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Failed, "blocked")
    );
}

#[test]
fn fix_pages_pick_the_agent_fixes_go_to_and_ship_picks_its_own() {
    let chip = |name: &str, lit| AgentChip {
        icon: AgentIcon::from_agent(name),
        name: name.into(),
        lit,
        off: false,
    };
    let chips = vec![chip("Claude Code", true), chip("Cursor", false)];
    let state = PanelState {
        picks: chips.clone(),
        fix_picks: chips.clone(),
        fix_model: Some("Model: default".into()),
        ..PanelState::default()
    };
    let cards = |f: &Feature, stage: Stage| {
        let actions = page_actions(f, stage, RowCtx::default());
        let page = stage_page(f, stage, &state, &actions, 2_000);
        (page.sections.iter())
            .filter_map(|s| match s {
                PageSection::Agent {
                    stage,
                    name,
                    status,
                    picks,
                    ..
                } => Some((*stage, name.clone(), status.clone(), picks.len())),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // AI review, Build done: the reviewer's card, then the fixes'.
    let mut f = feature("b", vec![repo("api", true)], 5);
    assert!(keys::fix_page(&f, Stage::AiReview));
    f.runs.insert(Stage::Build, agent_run(None, None));
    assert_eq!(
        cards(&f, Stage::AiReview),
        [
            (Stage::AiReview, String::new(), String::new(), 2),
            (
                Stage::Build,
                "Claude Code".into(),
                "Build agent \u{b7} makes the fixes".into(),
                2
            ),
        ]
    );
    // The keys pick the reviewer, the fixes' agent from the findings.
    assert_eq!(
        keys::keyed_stage(&f, Stage::AiReview, false),
        Some(Stage::AiReview)
    );
    assert_eq!(
        keys::keyed_stage(&f, Stage::AiReview, true),
        Some(Stage::Build)
    );
    // None built it: the card only says where fixes go.
    f.runs.remove(&Stage::Build);
    let fixes = (Stage::Build, String::new(), "Fixes".into(), 2);
    assert_eq!(cards(&f, Stage::AiReview)[1], fixes);
    // Verify has no agent of its own: its keys pick the fixes'.
    let f = feature("b", vec![repo("api", true)], 4);
    assert_eq!(cards(&f, Stage::Verify), [fixes]);
    assert_eq!(
        keys::keyed_stage(&f, Stage::Verify, false),
        Some(Stage::Build)
    );
    // Ship picks the agent writing its summary.
    let f = feature("b", vec![repo("api", true)], 8);
    assert_eq!(f.current(), Some(Stage::Ship));
    assert_eq!(
        cards(&f, Stage::Ship),
        [(Stage::Ship, String::new(), "Summary and conflicts agent".into(), 2)]
    );
    assert_eq!(keys::keyed_stage(&f, Stage::Ship, false), Some(Stage::Ship));
}

#[test]
fn a_stage_ahead_shows_its_chips_and_a_restart_names_the_other_agent() {
    let chips = vec![
        AgentChip {
            icon: AgentIcon::from_agent("Claude Code"),
            name: "Claude Code".into(),
            lit: false,
            off: false,
        },
        AgentChip {
            icon: AgentIcon::from_agent("Cursor"),
            name: "Cursor".into(),
            lit: true,
            off: false,
        },
    ];
    let state = PanelState {
        picks: chips.clone(),
        ..PanelState::default()
    };
    let mut f = feature("a", vec![repo("r", true)], 0);
    // Not reached yet: only the chips, no head.
    let page = stage_page(&f, Stage::Build, &state, &[], 2_000);
    let [PageSection::Agent { name, picks, .. }] = page.sections.as_slice() else {
        panic!("{:?}", page.sections);
    };
    assert!(name.is_empty());
    assert_eq!(picks, &chips);
    // Run by another: the restart says who comes next.
    let mut f2 = feature("b", vec![repo("r", true)], 3);
    f2.runs.insert(
        Stage::Build,
        feature::agents::StageRun {
            agent: "Claude Code".into(),
            ..Default::default()
        },
    );
    let actions = page_actions(&f2, Stage::Build, RowCtx::default());
    let page = stage_page(&f2, Stage::Build, &state, &actions, 2_000);
    let restart = |page: &StagePage| {
        (page.buttons.iter())
            .find(|b| b.label.starts_with("Restart"))
            .map(|b| b.label.clone())
    };
    assert_eq!(restart(&page).as_deref(), Some("Restart with Cursor"));
    // The agent that runs is the one picked: nothing to say.
    f2.runs.get_mut(&Stage::Build).unwrap().agent = "Cursor".into();
    let actions = page_actions(&f2, Stage::Build, RowCtx::default());
    let page = stage_page(&f2, Stage::Build, &state, &actions, 2_000);
    assert_eq!(restart(&page).as_deref(), Some("Restart"));
    // No chips, no card on a stage without a run.
    f.status = Status::Abandoned;
    let page = stage_page(&f, Stage::Build, &PanelState::default(), &[], 2_000);
    assert!(page.sections.is_empty(), "{:?}", page.sections);
}

#[test]
fn ctrl_shift_a_asks_for_the_next_agent_on_an_agent_stage_only() {
    let f = feature("a", vec![repo("r", true)], 0);
    let mut panel = FeaturePanel {
        slug: "a".into(),
        ..FeaturePanel::default()
    };
    let mods = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    let ctx = RowCtx::default();
    let press = |p: &mut FeaturePanel| p.key(&mods, Keysym::A, &f, &[], ctx, &[], &[], &[]);
    assert_eq!(
        press(&mut panel),
        Some(PanelOutcome::CycleAgent(Stage::Discover))
    );
    panel.pick(&f, Stage::Verify);
    assert_eq!(press(&mut panel), None);
}

#[test]
fn alt_shift_a_asks_for_the_model_on_an_agent_stage_only() {
    let f = feature("a", vec![repo("r", true)], 0);
    let mut panel = FeaturePanel {
        slug: "a".into(),
        ..FeaturePanel::default()
    };
    // Alt+Shift+A: its own key, off the setup log's Ctrl+Shift+I. Shift
    // comes with the unshifted sym (K8).
    let mods = ModifiersState {
        alt: true,
        shift: true,
        ..ModifiersState::default()
    };
    let ctx = RowCtx::default();
    let press = |p: &mut FeaturePanel| p.key(&mods, Keysym::a, &f, &[], ctx, &[], &[], &[]);
    assert_eq!(
        press(&mut panel),
        Some(PanelOutcome::PickModel(Stage::Discover))
    );
    panel.pick(&f, Stage::Verify);
    assert_eq!(press(&mut panel), None);
}

#[test]
fn the_model_button_and_its_key_show_where_gerdr_can_tell_a_model() {
    let f = feature("a", vec![repo("r", true)], 0);
    let chip = AgentChip {
        icon: AgentIcon::from_agent("Claude Code"),
        name: "Claude Code".into(),
        lit: true,
        off: false,
    };
    let state = |model: Option<&str>| PanelState {
        picks: vec![chip.clone()],
        model: model.map(str::to_owned),
        ..PanelState::default()
    };
    let label_on = |state: &PanelState| {
        let page = stage_page(&f, Stage::Build, state, &[], 2_000);
        let [PageSection::Agent { model, .. }] = page.sections.as_slice() else {
            panic!("{:?}", page.sections);
        };
        model.clone()
    };
    assert_eq!(label_on(&state(None)), None);
    assert_eq!(
        label_on(&state(Some("Model: opus"))).as_deref(),
        Some("Model: opus")
    );
    assert_eq!(model_label(None), "Model: default");
    assert_eq!(model_label(Some("opus")), "Model: opus");

    let panel = FeaturePanel {
        slug: "a".into(),
        ..FeaturePanel::default()
    };
    let hints = |model| {
        let choices = AgentChoices {
            picks: &[],
            model,
            place: false,
            ..AgentChoices::default()
        };
        tab_hints(
            &panel,
            &f,
            Stage::Discover,
            &[],
            RowCtx::default(),
            choices,
            &[],
            &[],
            &[],
        )
    };
    let shown = |hints: &[(String, String)]| hints.iter().any(|(k, _)| k == MODEL_KEY);
    assert!(shown(&hints(true)));
    assert!(!shown(&hints(false)));
}

/// `f` with a run of Plan's agent on another machine.
fn on_a_machine() -> Feature {
    let mut f = feature("a", vec![repo("r", true)], 0);
    f.tick(1, "you", None);
    f.runs.insert(
        Stage::Plan,
        feature::agents::StageRun {
            n: 1,
            agent: "Claude Code".into(),
            pane: Some("m/w1:p1".into()),
            machine: Some("m".into()),
            ..Default::default()
        },
    );
    f
}

#[test]
fn a_stage_placed_on_a_machine_says_so_and_offers_its_button() {
    let f = on_a_machine();
    let mut actions = page_actions(&f, Stage::Plan, RowCtx::default());
    let at = actions.len();
    actions.extend(place_action(Some("Runs on box"), Stage::Plan));
    assert_eq!(
        actions[at].1,
        PanelOutcome::CycleMachine(Stage::Plan),
        "the button acts on where the next agent runs"
    );
    assert_eq!(place_action(None, Stage::Plan), None);
    // What runs there can be stopped; what runs here closes with its tab.
    let stop = stop_action(&f, Stage::Plan).map(|(_, outcome)| outcome);
    assert_eq!(stop, Some(PanelOutcome::StopOnMachine(Stage::Plan)));
    assert_eq!(stop_action(&f, Stage::Build), None);
    let status_of = |machine_line| {
        let state = PanelState {
            machine_line,
            ..PanelState::default()
        };
        let page = stage_page(&f, Stage::Plan, &state, &actions, 2_000);
        (page.sections.iter()).find_map(|s| match s {
            PageSection::Agent {
                status, buttons, ..
            } => Some((status.clone(), buttons.contains(&at))),
            _ => None,
        })
    };
    let (status, held) = status_of(Some(("on box".to_owned(), false))).unwrap();
    assert!(status.ends_with("\u{b7} on box"), "{status}");
    assert!(held, "the button is on the agent's card");
    // Out of reach, nothing else is known of the agent.
    let (status, _) = status_of(Some(("box can't be reached".to_owned(), true))).unwrap();
    assert_eq!(status, "box can't be reached");
}

#[test]
fn a_card_says_where_its_agent_runs() {
    let f = on_a_machine();
    let line = |machine| {
        let state = LineState {
            probe: None,
            shipping: false,
            checks: false,
            setup: None,
            verify: None,
            manual_gate: None,
            code_counts: None,
            machine,
            agent: None,
        };
        card_line(&f, Some(Stage::Plan), &state, 2_000)
    };
    let on = line(Some(("on box".to_owned(), false)));
    assert!(
        on.starts_with("Plan \u{b7} ") && on.ends_with(" \u{b7} on box"),
        "{on}"
    );
    assert_eq!(
        line(Some(("box can't be reached".to_owned(), true))),
        "Plan \u{b7} box can't be reached"
    );
}

#[test]
fn the_footer_offers_a_machine_only_with_one_to_pick() {
    let f = feature("a", vec![repo("r", true)], 0);
    let panel = FeaturePanel {
        slug: "a".into(),
        ..FeaturePanel::default()
    };
    let shown = |place| {
        let choices = AgentChoices {
            picks: &[],
            model: false,
            place,
            ..AgentChoices::default()
        };
        let hints = tab_hints(
            &panel,
            &f,
            Stage::Discover,
            &[],
            RowCtx::default(),
            choices,
            &[],
            &[],
            &[],
        );
        hints.iter().any(|(k, _)| k == "Ctrl+Shift+W")
    };
    assert!(shown(true));
    assert!(!shown(false));
}

#[test]
fn a_record_from_before_machines_loads_and_a_placed_run_round_trips() {
    let mut f = feature("a", vec![repo("r", true)], 0);
    let json = serde_json::to_string(&f).unwrap();
    assert!(!json.contains("stage_machines"), "{json}");
    f.stage_machines.insert(Stage::Build, "m".into());
    f.runs = on_a_machine().runs;
    let json = serde_json::to_string(&f).unwrap();
    assert_eq!(serde_json::from_str::<Feature>(&json).unwrap(), f);
    let run = feature::agents::StageRun::default();
    let json = serde_json::to_string(&run).unwrap();
    assert!(!json.contains("machine"), "{json}");
}

#[test]
fn the_feature_keys_old_record_has_no_picks_and_a_pick_round_trips() {
    let mut f = feature("a", vec![repo("r", true)], 0);
    let json = serde_json::to_string(&f).unwrap();
    assert!(!json.contains("stage_agents"), "{json}");
    assert!(!json.contains("stage_models"), "{json}");
    f.stage_agents.insert(Stage::Plan, "Cursor".into());
    (f.stage_models.entry(Stage::Plan).or_default()).insert("Cursor".into(), "gpt-5.2".into());
    let json = serde_json::to_string(&f).unwrap();
    assert_eq!(serde_json::from_str::<Feature>(&json).unwrap(), f);
}

/// A Manual test round `n`, over at `ended` unless open, its fixes gone
/// with the code read at `from` when given.
/// An item asked of an open round is "in fix plan" until its fixes go,
/// whichever stage asked it (Verify's checks, Manual test's criteria).
#[test]
fn asked_items_are_in_the_fix_plan_until_the_fixes_go() {
    let mut round = manual_round(1, None, None);
    for (phase, word) in [
        (FixPhase::Compacting, "in fix plan"),
        (FixPhase::Planning, "in fix plan"),
        (FixPhase::Ready, "in fix plan"),
        (FixPhase::Fixing, "being fixed"),
    ] {
        round.phase = phase;
        assert_eq!(sent_now(&round), word, "{phase:?}");
    }
}

/// The round cards of `f`'s Manual test page, with their buttons' labels.
fn round_cards_of(f: &Feature, state: &PanelState) -> Vec<(chrome::RoundCard, Vec<String>)> {
    let ctx = state.ctx;
    let mut actions = stage_actions(f, Stage::ManualTest, ctx, &[], &state.criteria, &[]);
    actions.extend(round_plan_actions(f, Stage::ManualTest, state.probe));
    let page = stage_page(f, Stage::ManualTest, state, &actions, 4_000);
    (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card.clone()),
            _ => None,
        })
        .map(|card| {
            let labels = (card.buttons.iter())
                .map(|&i| page.buttons[i].label.clone())
                .collect();
            (card, labels)
        })
        .collect()
}

#[test]
fn manual_tests_page_lists_every_round_newest_first_with_its_diff() {
    let mut f = feature("g", vec![repo("api", true)], Stage::ManualTest as usize);
    assert_eq!(f.current(), Some(Stage::ManualTest));
    for n in 1..=3 {
        f.fix_asked.insert(n, Stage::ManualTest);
    }
    // Round 1 was read before its fixes, round 2 wasn't; 3 is open.
    f.past_rounds = vec![
        manual_round(1, Some(2_000), Some("a1")),
        manual_round(2, Some(3_000), None),
    ];
    f.last_fix_round = None;
    f.fix_round = Some(manual_round(3, None, Some("a3")));
    f.fix_rounds = 3;
    let state = PanelState::default();
    let cards = round_cards_of(&f, &state);
    let labels: Vec<&str> = cards.iter().map(|(c, _)| c.label.as_str()).collect();
    assert_eq!(labels, ["Fix round 3", "Fix round 2", "Fix round 1"]);
    // The open one shows all of it, at work; the rest fold.
    let opened: Vec<bool> = cards.iter().map(|(c, _)| c.open).collect();
    assert_eq!(opened, [true, false, false]);
    assert_eq!(cards[0].0.dot, Dot::Working);
    assert_eq!(cards[2].0.dot, Dot::Done);
    assert_eq!(cards[2].0.state.text, "1 of 1 made");
    // Only a round read before its fixes has a diff.
    assert!(cards[0].1.contains(&"Diff".to_owned()));
    assert!(!cards[1].1.contains(&"Diff".to_owned()));
    assert_eq!(cards[2].1, ["Diff", "Show"]);
    assert_eq!(
        cards[1].0.changed,
        ["not known: the code wasn't read before the fixes went"]
    );
    // Who asked, who made them and when, and what it was asked as said
    // then.
    let meta = &cards[2].0.meta;
    assert!(meta.starts_with("asked by Manual test "), "{meta}");
    assert!(
        meta.contains("made by the Build agent (Claude Code)"),
        "{meta}"
    );
    assert!(meta.ends_with("took 14 min"), "{meta}");
    // The open one, its plan pending, is the Build agent's too.
    if let Some(open) = f.fix_round.as_mut() {
        open.trail.agent.clear();
    }
    f.runs.insert(Stage::Build, agent_run(None, None));
    let pending = round_cards_of(&f, &state);
    let meta = &pending[0].0.meta;
    assert!(
        meta.contains("made by the Build agent (Claude Code)"),
        "{meta}"
    );
    let asked = &cards[0].0.asked[0];
    assert_eq!(asked.title, "Save keeps the title");
    assert_eq!(asked.note, "the title went back (1 screenshot)");
    // How it stands now goes on the newest round asking it only.
    assert!(asked.now.is_some());
    assert!(cards[2].0.asked[0].now.is_none());
    // Its fixes, from the plan until the build log lists them.
    assert_eq!(cards[0].0.fixes, [(false, "Keep the title".to_owned())]);
}

#[test]
fn a_fix_round_card_folds_and_opens_by_its_button_and_all_by_alt_e() {
    let mut f = feature("g", vec![repo("api", true)], Stage::ManualTest as usize);
    for n in 1..=2 {
        f.fix_asked.insert(n, Stage::ManualTest);
    }
    f.past_rounds = vec![manual_round(1, Some(2_000), Some("a1"))];
    f.last_fix_round = Some(manual_round(2, Some(3_000), Some("a2")));
    f.fix_rounds = 2;
    let mut panel = FeaturePanel::default();
    let open = |panel: &FeaturePanel| -> Vec<bool> {
        let state = PanelState {
            fix_rounds_flipped: panel.fix_rounds_flipped.clone(),
            ..PanelState::default()
        };
        round_cards_of(&f, &state)
            .iter()
            .map(|(c, _)| c.open)
            .collect()
    };
    // Both over: folded in the history.
    assert_eq!(open(&panel), [false, false]);
    panel.toggle_fix_round(1);
    assert_eq!(open(&panel), [false, true]);
    panel.toggle_rounds(&f, Stage::ManualTest);
    assert_eq!(open(&panel), [false, false]);
    panel.toggle_rounds(&f, Stage::ManualTest);
    assert_eq!(open(&panel), [true, true]);
    // The last round keeps its own buttons first: sent again by name.
    let state = PanelState {
        fix_rounds_flipped: panel.fix_rounds_flipped.clone(),
        ..PanelState::default()
    };
    let cards = round_cards_of(&f, &state);
    assert_eq!(
        cards[0].1,
        [OPEN_PLAN, "Send fix round 2 again", "Diff", "Hide"]
    );
    assert_eq!(cards[1].1, ["Diff", "Hide"]);
    // Nothing of theirs is left for the title row.
    let actions = stage_actions(&f, Stage::ManualTest, state.ctx, &[], &[], &[]);
    let page = stage_page(&f, Stage::ManualTest, &state, &actions, 4_000);
    let held: Vec<usize> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card.buttons.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    for (i, (_, outcome)) in actions.iter().enumerate() {
        if matches!(
            outcome,
            PanelOutcome::ReviewFixPlan
                | PanelOutcome::ResendFixes
                | PanelOutcome::RoundDiff(_)
        ) {
            assert!(held.contains(&i), "{outcome:?} isn't on a card");
        }
    }
}

#[test]
fn a_criterion_a_round_asked_says_how_it_stands_since() {
    use crate::feature::manual::{Check, ManualTest, Verdict};
    let mut f = feature("g", vec![repo("api", true)], Stage::ManualTest as usize);
    let round = manual_round(1, Some(2_000), Some("a1"));
    let check = |state, sent, made, at| Check {
        id: "AC-1".into(),
        text: "t".into(),
        state,
        note: String::new(),
        at,
        sent,
        patch: String::new(),
        made,
        shots: Vec::new(),
    };
    let now = |f: &Feature| asked_now(f, &round, "AC-1", false).map(|l| l.text);
    assert_eq!(now(&f).as_deref(), Some("not tried since"));
    let mut with = |k: Check| {
        f.manual = Some(ManualTest {
            patch: String::new(),
            checks: vec![k],
            ..ManualTest::default()
        });
        now(&f)
    };
    assert_eq!(
        with(check(Verdict::Pass, false, false, 2_100)).as_deref(),
        Some("passed since")
    );
    assert_eq!(
        with(check(Verdict::Fail, true, true, 900)).as_deref(),
        Some("fix made: try it again")
    );
    assert_eq!(
        with(check(Verdict::Fail, false, false, 2_100)).as_deref(),
        Some("failed again")
    );
}

#[test]
fn a_rounds_state_and_time_read_short() {
    let mut round = manual_round(1, Some(2_000), None);
    assert_eq!(
        round_state_words(&round, 2_000 + 720),
        "1 of 1 made 12m ago"
    );
    round.trail.dropped = Some(2_000);
    assert_eq!(round_state_words(&round, 2_060), "dropped 1m ago");
    assert_eq!(took(0, 30), "1 min");
    assert_eq!(took(0, 3_600), "1 h");
    assert_eq!(took(0, 3_900), "1 h 5 min");
}

#[test]
fn verifys_rounds_say_how_each_failed_check_did_since() {
    let mut f = verified(false);
    assert_eq!(f.current(), Some(Stage::Verify));
    let failed = |c: &str| format!("api: `{c}` exited with 1");
    f.verify_fixes = vec![VerifyFix {
        round: 1,
        run: 1,
        at: 1_500,
        agent: "Claude Code".into(),
        failures: vec![failed("test"), failed("lint")],
        why: vec!["boom".into()],
        dropped: false,
        next: Some((2, false)),
    }];
    f.fix_asked.insert(1, Stage::Verify);
    let mut round = manual_round(1, Some(2_000), Some("a1"));
    round.stage = Stage::Verify;
    round.trail.asked = Vec::new();
    round.trail.agent = String::new();
    f.past_rounds = vec![round];
    f.fix_rounds = 1;
    let state = PanelState::default();
    let mut actions = stage_actions(&f, Stage::Verify, state.ctx, &[], &[], &[]);
    actions.extend(round_plan_actions(&f, Stage::Verify, None));
    let page = stage_page(&f, Stage::Verify, &state, &actions, 4_000);
    let cards: Vec<&chrome::RoundCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].label, "Fix round 1");
    let asked: Vec<(&str, &str, Option<&str>)> = (cards[0].asked.iter())
        .map(|a| {
            (
                a.title.as_str(),
                a.note.as_str(),
                a.now.as_ref().map(|l| l.text.as_str()),
            )
        })
        .collect();
    assert_eq!(
        asked,
        [
            (
                failed("test").as_str(),
                "boom",
                Some("failed again in run 2")
            ),
            (failed("lint").as_str(), "", Some("passed in run 2")),
        ]
    );
    let meta = &cards[0].meta;
    assert!(meta.starts_with("asked by Verify"), "{meta}");
    assert!(
        meta.contains("made by the Build agent (Claude Code) \u{b7} run 1 failed "),
        "{meta}"
    );
    assert!(meta.ends_with("run 2 failed"), "{meta}");
    // One from before rounds were kept stands in by its number.
    f.past_rounds.clear();
    f.verify_fixes[0].dropped = true;
    let rounds = card_rounds(&f, Stage::Verify, None);
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].trail.dropped, Some(1_500));
}

#[test]
fn a_round_held_on_a_page_it_wasnt_asked_by_still_gets_a_card() {
    // Verify's failures sent from Manual test: the round is Verify's, its
    // buttons Manual test's.
    let f = feature("g", vec![repo("api", true)], Stage::ManualTest as usize);
    let mut f = in_round(f, Stage::ManualTest, FixPhase::Ready);
    f.fix_asked.insert(1, Stage::Verify);
    f.fix_rounds = 1;
    let state = PanelState::default();
    let cards = round_cards_of(&f, &state);
    assert_eq!(cards.len(), 1);
    assert!(
        cards[0].1.contains(&"Review".to_owned()),
        "{:?}",
        cards[0].1
    );
}

#[test]
fn builds_page_lists_every_round_as_a_card_with_its_ticks() {
    let mut f = feature("b", vec![repo("api", true)], 4);
    f.fix_asked = BTreeMap::from([
        (1, Stage::AiReview),
        (2, Stage::CodeReview),
        (3, Stage::Verify),
    ]);
    // 1 over and kept whole, 3 only in Verify's record, 2 open.
    let mut one = manual_round(1, Some(2_000), Some("a1"));
    one.stage = Stage::AiReview;
    one.trail.asked = Vec::new();
    f.past_rounds = vec![one];
    f.verify_fixes = vec![verify_fix(3, 2)];
    let mut two = manual_round(2, None, None);
    two.stage = Stage::CodeReview;
    two.phase = FixPhase::Planning;
    two.trail = Default::default();
    f.fix_round = Some(two);
    f.fix_rounds = 3;
    let mut read = probe(None);
    read.docs.fix_rounds = vec![(1, vec![(true, "r1-1".to_owned())])];
    read.docs.plans.insert(1);
    let state = PanelState {
        probe: Some(&read),
        ..PanelState::default()
    };
    let mut actions = stage_actions(&f, Stage::Build, state.ctx, &[], &[], &[]);
    actions.extend(round_plan_actions(&f, Stage::Build, state.probe));
    let page = stage_page(&f, Stage::Build, &state, &actions, 4_000);
    let cards: Vec<&chrome::RoundCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card),
            _ => None,
        })
        .collect();
    // Newest first, each by its one name; who asked is on its meta line.
    let labels: Vec<&str> = cards.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Fix round 3", "Fix round 2", "Fix round 1"]);
    assert!(
        cards[2].meta.starts_with("asked by AI review"),
        "{}",
        cards[2].meta
    );
    // The open one shows open whatever its number.
    let opened: Vec<bool> = cards.iter().map(|c| c.open).collect();
    assert_eq!(opened, [false, true, false]);
    assert_eq!(cards[1].dot, Dot::Working);
    // The build log's ticks, and the plan and diff of one over.
    assert_eq!(cards[2].fixes, [(true, "r1-1".to_owned())]);
    let held = |card: &chrome::RoundCard| -> Vec<String> {
        (card.buttons.iter())
            .map(|&i| page.buttons[i].label.clone())
            .collect()
    };
    assert_eq!(held(cards[2]), ["Diff", "Open plan", "Show"]);
    // Verify's from before rounds were kept: what failed, and why.
    assert_eq!(cards[0].asked.len(), 2);
    assert_eq!(cards[0].asked[0].note, "FAIL auth");
    // Nothing of the old lists is left.
    assert!(!page.sections.iter().any(|s| matches!(s,
        PageSection::Lines { label, .. } if label == "Verify failures")));
    // On AI review's page the same round reads by the same name.
    let page = stage_page(&f, Stage::AiReview, &state, &[], 4_000);
    assert!(page.sections.iter().any(|s| matches!(s,
        PageSection::Round(c) if c.label == "Fix round 1")));
}

#[test]
fn plan_reviews_final_plan_shows_first_holding_its_diff_and_no_run_cards() {
    use crate::feature::plan_review::{self, PlanReview, RunRecord};
    let stage = Stage::PlanReview;
    let mut f = feature("p", vec![repo("api", true)], stage_index(stage));
    assert_eq!(f.current(), Some(stage));
    let mut review = PlanReview::default();
    let raws = |text: &str| plan_review::parse(text).unwrap();
    let got = plan_review::import(
        &mut review,
        1,
        raws(
            r#"{"suggestions": [{"kind": "question", "plan": "M1", "title": "A?", "why": "w"}]}"#,
        ),
    );
    plan_review::keep_run(
        &mut review,
        RunRecord {
            n: 1,
            at: 1_000,
            agent: "Codex".into(),
            raised: got.ids.clone(),
            new: got.new,
            ..RunRecord::default()
        },
    );
    review.stamp = Some((1, 1));
    f.plan_review = Some(review);
    let mut read = probe(None);
    read.docs.written.insert(stage);
    read.docs
        .previews
        .insert(stage, std::sync::Arc::new(Vec::new()));
    let state = PanelState {
        probe: Some(&read),
        ctx: wrote(&[stage]),
        ..PanelState::default()
    };
    let actions = round_plan_actions(&f, stage, state.probe);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].1, PanelOutcome::PlanChanges);
    let page = stage_page(&f, stage, &state, &actions, 1_120);
    // The suggestion cards say it all: no card per review run.
    assert!(
        !page
            .sections
            .iter()
            .any(|s| matches!(s, PageSection::Round(_)))
    );
    let Some(PageSection::File { buttons, .. }) =
        (page.sections.iter()).find(|s| matches!(s, PageSection::File { .. }))
    else {
        panic!("{:?}", page.sections);
    };
    let held: Vec<&str> = (buttons.iter())
        .map(|&i| page.buttons[i].label.as_str())
        .collect();
    assert!(held.contains(&"Diff"), "{held:?}");
    // The final plan comes first, its text too, the suggestions under
    // it.
    let at = |is: fn(&PageSection) -> bool| page.sections.iter().position(is);
    let file = at(|s| matches!(s, PageSection::File { .. })).unwrap();
    let text = at(|s| matches!(s, PageSection::Doc { here: true, .. })).unwrap();
    let card = at(|s| matches!(s, PageSection::Finding(_))).unwrap();
    assert!(file < text && text < card, "{:?}", page.sections);
    // No final plan yet: no Diff, and nothing for Alt+E to fold.
    assert!(round_plan_actions(&f, stage, None).is_empty());
    assert!(card_numbers(&f, stage).is_empty());
}

#[test]
fn discover_and_plan_list_their_drafts_as_cards() {
    use crate::feature::{drafts::Draft, fix_round::Asked};
    let stage = Stage::Plan;
    let mut f = feature("p", vec![repo("api", true)], stage_index(stage));
    assert_eq!(f.current(), Some(stage));
    let draft = |k: u32, sent_back: Option<u64>| Draft {
        stage,
        k,
        run: 1,
        agent: "Claude Code".into(),
        model: "opus".into(),
        written: 1_000 * u64::from(k),
        sent_back,
        comments: Vec::new(),
        approved: None,
    };
    let mut one = draft(1, Some(1_500));
    one.comments = vec![Asked {
        id: "C1".into(),
        title: "line 3: Use a queue".into(),
        note: "Why not cron?".into(),
        shots: 0,
    }];
    f.drafts = vec![one, draft(2, None)];
    f.runs.insert(
        stage,
        feature::agents::StageRun {
            n: 1,
            ready: Some(2_000),
            ..Default::default()
        },
    );
    let state = PanelState::default();
    let mut actions = stage_actions(&f, stage, state.ctx, &[], &[], &[]);
    actions.extend(round_plan_actions(&f, stage, None));
    let page = stage_page(&f, stage, &state, &actions, 2_120);
    let cards: Vec<&chrome::RoundCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card),
            _ => None,
        })
        .collect();
    let labels: Vec<&str> = cards.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Draft 2", "Draft 1"]);
    assert_eq!(cards[0].state.text, "ready for review");
    // The user's turn: the yellow dot.
    assert_eq!(cards[0].dot, Dot::Blocked);
    assert_eq!(cards[1].state.text, "sent back with 1 comment");
    // Sent back is over, not done: no green dot.
    assert_eq!(cards[1].dot, Dot::Unknown);
    assert_eq!(cards[1].asked_caption, "Comments sent back");
    assert_eq!(cards[1].asked[0].note, "Why not cron?");
    assert_eq!(cards[0].meta, "Claude Code (opus) \u{b7} written 2m ago");
    let held = |card: &chrome::RoundCard| -> Vec<&str> {
        (card.buttons.iter())
            .map(|&i| page.buttons[i].label.as_str())
            .collect()
    };
    // The first has nothing before it to show changes against.
    assert_eq!(held(cards[0]), ["Diff", "Hide"]);
    assert_eq!(held(cards[1]), ["Show"]);
    assert!(
        actions
            .iter()
            .any(|(_, o)| *o == PanelOutcome::DraftChanges(stage, 2))
    );
    // Sent back again and being rewritten: the newest says so.
    let mut back = f.clone();
    back.drafts[1].sent_back = Some(2_100);
    back.runs.get_mut(&stage).unwrap().ready = None;
    let page = stage_page(&back, stage, &state, &actions, 2_120);
    let newest = page.sections.iter().find_map(|s| match s {
        PageSection::Round(card) => Some(card),
        _ => None,
    });
    assert_eq!(
        newest.unwrap().state.text,
        "sent back \u{b7} being written again"
    );
    assert_eq!(newest.unwrap().dot, Dot::Working);
    // Alt+E folds them all, then opens them all.
    let mut panel = FeaturePanel::default();
    panel.toggle_rounds(&f, stage);
    assert_eq!(panel.fix_rounds_flipped, BTreeSet::from([2]));
}
