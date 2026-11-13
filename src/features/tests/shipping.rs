//! Tests of the Ship page, the toasts and the sandbox and setup sections.

use super::*;

#[test]
fn ship_is_asked_for_not_ticked_and_bound_stages_tick_as_before() {
    let shipping = feature("s", vec![repo("api", true)], 8);
    let ship = PanelRow::Stage(Stage::Ship);
    assert_eq!(
        row_action(&shipping, ship, RowCtx::default()),
        Some(PanelOutcome::Ship)
    );
    assert_eq!(row_hint(&shipping, ship, RowCtx::default()), "ship\u{2026}");
    assert_eq!(
        stage_button(&shipping, Stage::Ship, RowCtx::default()),
        Some("Ship\u{2026}")
    );
    assert_eq!(
        stage_button(&shipping, Stage::ManualTest, RowCtx::default()),
        Some("Reopen")
    );
    // Build has Verify done after it: locked, it doesn't reopen.
    assert_eq!(
        stage_button(&shipping, Stage::Build, RowCtx::default()),
        None
    );
    let verifying = feature("v", vec![repo("api", true)], 4);
    let verify = PanelRow::Stage(Stage::Verify);
    let none = RowCtx::default();
    assert_eq!(
        row_action(&verifying, verify, none),
        Some(PanelOutcome::Tick)
    );
    assert_eq!(row_hint(&verifying, verify, none), "mark done");
    assert_eq!(
        stage_button(&verifying, Stage::Verify, none),
        Some("Mark done")
    );
    assert_eq!(stage_button(&verifying, Stage::Ship, none), None);
    // Enter on the Ship row of the panel asks to ship.
    let mut panel = FeaturePanel::default();
    let slugs = vec!["s".to_owned()];
    let (mods, sym) = key(Keysym::Return);
    assert_eq!(
        panel.key(
            &mods,
            sym,
            &shipping,
            &slugs,
            RowCtx::default(),
            &[],
            &[],
            &[]
        ),
        Some(PanelOutcome::Ship)
    );
}

#[test]
fn a_repository_with_a_pull_request_opens_it_by_button_and_key() {
    let mut f = feature(
        "d",
        vec![repo("api", true), repo("web", true), repo("ui", false)],
        9,
    );
    f.status = Status::Done;
    let url = "https://github.com/acme/api/pull/12";
    f.shipping.prs.push(Pr {
        repo: "api".into(),
        url: url.into(),
    });
    assert_eq!(repo_button(&f, 0), Some(PanelOutcome::OpenPr(url.into())));
    assert_eq!(repo_button(&f, 1), None);
    assert_eq!(repo_button(&f, 2), Some(PanelOutcome::Touch("ui".into())));
    let slugs = vec!["d".to_owned()];
    let mut panel = FeaturePanel {
        focus: TabArea::Repos,
        ..FeaturePanel::default()
    };
    let shift_ctrl = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        panel.key(
            &shift_ctrl,
            Keysym::O,
            &f,
            &slugs,
            RowCtx::default(),
            &[],
            &[],
            &[],
        ),
        Some(PanelOutcome::OpenPr(url.into()))
    );
    // Ctrl+O still opens a tab; on a row without a PR, nothing.
    let (mods, sym) = ctrl(Keysym::o);
    assert_eq!(
        panel.key(&mods, sym, &f, &slugs, RowCtx::default(), &[], &[], &[]),
        Some(PanelOutcome::Open(f.dir.clone()))
    );
    panel.repo = 1;
    assert_eq!(
        panel.key(
            &shift_ctrl,
            Keysym::O,
            &f,
            &slugs,
            RowCtx::default(),
            &[],
            &[],
            &[],
        ),
        None
    );
    // The view: the button, the line, and the hint on that row only.
    let state = PanelState::default();
    panel.repo = 0;
    let view = tab_view(&panel, &f, &slugs, &state, 2_060);
    assert_eq!(view.repos[0].buttons.first(), Some(&"Open PR"));
    assert_eq!(view.repos[0].line, "feature/bulk \u{b7} PR api/pull/12");
    assert!(view.repos[1].buttons.is_empty(), "a shipped one's can't go");
    assert_eq!(view.repos[2].buttons.first(), Some(&FEATURE_BUTTON));
    assert!(
        view.hints
            .contains(&("Ctrl+Shift+O".into(), "open PR".into()))
    );
    let label = |s: &PageSection| match s {
        PageSection::Lines { label, .. } => label.clone(),
        _ => String::new(),
    };
    let labels: Vec<String> = view.page.sections.iter().map(label).collect();
    // Its worktree gone, it ships no more: its pull requests are listed.
    // The recap first, which has no label.
    assert_eq!(
        labels,
        ["", "Pull requests", "Ship log"],
        "no Ship check once shipped"
    );
    let buttons: Vec<&str> = view.page.buttons.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(buttons, ["Reopen", "Open PR"]);
    assert_eq!(view.page.buttons[0].style, ButtonStyle::Plain);
    panel.repo = 1;
    let view = tab_view(&panel, &f, &slugs, &state, 2_060);
    assert!(!view.hints.iter().any(|(k, _)| k == "Ctrl+Shift+O"));
}

#[test]
fn a_push_not_guarded_shows_on_its_repository() {
    let f = feature("g", vec![repo("api", true), repo("web", true)], 4);
    let mut p = probe(None);
    p.guards.insert("api".into(), Guard::On);
    p.guards
        .insert("web".into(), Guard::Off("hooks path is set".into()));
    assert_eq!(repo_line(&f, &f.repos[0], Some(&p)), "feature/bulk");
    assert_eq!(
        repo_line(&f, &f.repos[1], Some(&p)),
        "feature/bulk \u{b7} push not guarded: hooks path is set"
    );
}

#[test]
fn a_repository_built_on_another_branch_says_so() {
    let f = feature("b", vec![repo("api", true), repo("web", true)], 4);
    let mut p = probe(None);
    if let Ok(r) = p.reading.as_mut() {
        r.sits_on.insert("web".into(), ("port".into(), "c1".into()));
    }
    assert_eq!(repo_line(&f, &f.repos[0], Some(&p)), "feature/bulk");
    assert_eq!(
        repo_line(&f, &f.repos[1], Some(&p)),
        format!(
            "feature/bulk \u{b7} built on port, not on its base {}",
            f.repos[1].base
        )
    );
}

#[test]
fn the_ship_check_shows_what_stops_it_or_that_it_is_ready() {
    let f = feature("s", vec![repo("api", true)], 8);
    let none = check_lines(&f, None, false, 2_060);
    assert_eq!(texts(&none), [("checking\u{2026}", CheckTone::Note)]);
    let blocked = probe(check(&["api is dirty", "web is behind"], &["api"], &[]));
    // Failing first; then what passes, named on one line.
    let passed = "\u{2713} checks passed \u{b7} review done \u{b7} no conflicts";
    assert_eq!(
        texts(&check_lines(&f, Some(&blocked), false, 2_060)),
        [
            ("api is dirty", CheckTone::Failed),
            ("web is behind", CheckTone::Failed),
            (passed, CheckTone::Ready)
        ]
    );
    let many = probe(check(&["a", "b", "c", "d", "e", "f"], &["api"], &[]));
    let lines = check_lines(&f, Some(&many), false, 2_060);
    assert_eq!(lines.len(), MAX_CHECK_LINES);
    assert_eq!(lines[MAX_CHECK_LINES - 1].text, "and 3 more");
    let ready = probe(check(&[], &["api"], &["api pushes to origin"]));
    assert_eq!(
        texts(&check_lines(&f, Some(&ready), false, 2_060)),
        [
            ("Ready to ship", CheckTone::Ready),
            (passed, CheckTone::Ready),
            ("api pushes to origin", CheckTone::Note)
        ]
    );
    // Its base read as current: that passes too.
    let mut current = ready.clone();
    current.bases.insert("api".into(), ship::BaseState::default());
    assert_eq!(
        check_lines(&f, Some(&current), false, 2_060)[1].text,
        "\u{2713} checks passed \u{b7} review done \u{b7} base current \u{b7} no conflicts"
    );
    // Origin's base moved on in a way that conflicts: not ready.
    let mut behind = ready.clone();
    let state = ship::BaseState {
        onto: "origin/main".into(),
        behind: 8,
        conflicts: vec!["export.py".into()],
        ..Default::default()
    };
    behind.bases.insert("api".into(), state);
    assert_eq!(
        texts(&check_lines(&f, Some(&behind), false, 2_060)),
        [
            (
                "api: origin/main has 8 new commits that conflict: take them in first",
                CheckTone::Blocker
            ),
            ("\u{2713} checks passed \u{b7} review done", CheckTone::Ready),
            ("api pushes to origin", CheckTone::Note)
        ]
    );
    // Nothing to do: what would stop shipping stops nothing.
    let nothing = probe(check(
        &["api is dirty"],
        &[],
        &["web has no commits: left out"],
    ));
    assert_eq!(
        texts(&check_lines(&f, Some(&nothing), false, 2_060)),
        [
            (NOTHING_TICKED, CheckTone::Note),
            (passed, CheckTone::Ready),
            ("web has no commits: left out", CheckTone::Note)
        ]
    );
    let unread = Probe {
        reading: Err("no git".into()),
        ..probe(None)
    };
    assert_eq!(
        texts(&check_lines(&f, Some(&unread), false, 2_060)),
        [("Couldn't read the code: no git", CheckTone::Blocker)]
    );
    assert_eq!(
        texts(&check_lines(&f, Some(&ready), true, 2_060)),
        [("shipping\u{2026}", CheckTone::Note)]
    );
    let mut squashing = f.clone();
    squashing.shipping.squash_at = Some(2_000);
    let lines = check_lines(&squashing, Some(&ready), false, 2_060);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.starts_with("Squash started"));
    // Only while Ship is the current stage.
    let building = feature("b", vec![repo("api", true)], 3);
    assert!(check_lines(&building, Some(&ready), false, 2_060).is_empty());
    // While an agent takes on its conflicts, Ship waits for it.
    let mut resolving = f.clone();
    resolving.shipping.resolving = Some(feature::Resolving {
        at: 2_000,
        repos: vec![feature::Taking {
            repo: "api".into(),
            onto: "origin/main".into(),
            onto_sha: "c1".into(),
            before: "c0".into(),
            files: vec!["a.py".into()],
        }],
        agent: "claude".into(),
        pane: None,
        n: 1,
        planning: true,
        nudged: false,
    });
    let lines = check_lines(&resolving, Some(&current), false, 2_060);
    assert_eq!(
        texts(&lines[..1]),
        [(
            "api: claude plans how to resolve the conflicts with origin/main: Ship waits for it",
            CheckTone::Note
        )]
    );
    assert!(matches!(ship_ask(&resolving, Some(&current)), ShipAsk::Blocked(_)));
}

#[test]
fn a_shipped_pull_request_waits_to_merge_before_it_reads_as_done() {
    let mut f = feature("s", vec![repo("api", true)], 9);
    f.status = Status::Done;
    assert_eq!(pr_wait(&f, None), None, "no pull request");
    f.shipping.prs.push(Pr {
        repo: "api".into(),
        url: "https://x/pull/1".into(),
    });
    let open = Some("pull request open, not merged yet".to_owned());
    assert_eq!(pr_wait(&f, None), open);
    let mut p = probe(None);
    let merged = ship::PrState {
        state: "MERGED".into(),
        ..Default::default()
    };
    p.prs = Some(BTreeMap::from([("api".to_owned(), Ok(merged))]));
    assert_eq!(pr_wait(&f, Some(&p)), None);
}

#[test]
fn shipping_asks_with_the_last_check_or_says_why_not() {
    let mut f = feature("s", vec![repo("api", true)], 8);
    f.title = "T".into();
    assert_eq!(ship_ask(&f, None), ShipAsk::Checking);
    assert_eq!(ship_ask(&f, Some(&probe(None))), ShipAsk::Checking);
    let blocked = probe(check(&["a", "b", "c"], &["api"], &[]));
    assert_eq!(
        ship_ask(&f, Some(&blocked)),
        ShipAsk::Blocked("Can't ship yet: a, and 2 more".into())
    );
    let one = probe(check(&["a"], &["api"], &[]));
    assert_eq!(
        ship_ask(&f, Some(&one)),
        ShipAsk::Blocked("Can't ship yet: a".into())
    );
    let dirty = Probe {
        reading: Ok(Reading {
            patch: "p1".into(),
            dirty: vec!["api".into()],
            ..Reading::default()
        }),
        ..probe(check(&["api has uncommitted changes"], &[], &[]))
    };
    assert_eq!(
        ship_ask(&f, Some(&dirty)),
        ShipAsk::Finish(vec!["api".into()])
    );
    assert_eq!(
        ship_ask(&f, Some(&probe(check(&[], &["api"], &[])))),
        ShipAsk::Ready(Asked {
            actions: vec![
                Action::Push { repo: "api".into() },
                Action::OpenPr {
                    repo: "api".into(),
                    base: "main".into()
                }
            ],
            patch: Some("p1".into())
        })
    );
    // Code that can't be read says nothing of what shipping would do.
    let unread = Probe {
        reading: Err("no git".into()),
        ship: Some(ShipCheck {
            blockers: vec!["Couldn't read the code: no git".into()],
            unread: true,
            ..ShipCheck::default()
        }),
        ..probe(None)
    };
    assert!(matches!(ship_ask(&f, Some(&unread)), ShipAsk::Blocked(_)));
}

#[test]
fn the_ship_rows_follow_the_ticks_and_shut_once_a_squash_runs() {
    let mut f = feature("s", vec![repo("api", true)], 8);
    let found = probe(check(&[], &["api"], &[]));
    let keys = |f: &Feature| ship_rows(f, Some(&found));
    let api = || "api".to_owned();
    assert_eq!(
        keys(&f),
        [
            ShipRow::Route {
                repo: api(),
                route: Route::Pr
            },
            ShipRow::Step {
                repo: api(),
                step: ShipStep::Push
            },
            ShipRow::Step {
                repo: api(),
                step: ShipStep::OpenPr
            },
            ShipRow::After(ShipAfter::Close),
            ShipRow::After(ShipAfter::Remove),
        ]
    );
    // The choice goes round; Space on it says the next one.
    assert_eq!(
        keys(&f)[0].outcome(),
        PanelOutcome::ShipRoute(api(), Route::Merge)
    );
    assert_eq!(
        keys(&f)[1].outcome(),
        PanelOutcome::ShipTick(api(), ShipStep::Push)
    );
    f.ship_plan
        .repos
        .insert(api(), feature::RepoPlan::new(Route::Nothing));
    assert_eq!(keys(&f).len(), 3);
    f.shipping.squash_at = Some(5);
    assert!(keys(&f).is_empty());
}

#[test]
fn space_ticks_the_row_and_enter_presses_the_page_s_button() {
    let f = feature("s", vec![repo("api", true)], 8);
    assert_eq!(f.current(), Some(Stage::Ship));
    let found = probe(check(&[], &["api"], &[]));
    let ships = ship_rows(&f, Some(&found));
    let mods = ModifiersState::default();
    let mut panel = FeaturePanel::default();
    let none = RowCtx::default();
    let press = |panel: &mut FeaturePanel, sym, ctx| {
        panel.key(&mods, sym, &f, &[], ctx, &[], &ships, &[])
    };
    // From the rail, Tab goes to the rows.
    assert_eq!(press(&mut panel, Keysym::Tab, none), None);
    assert_eq!(panel.focus, TabArea::Findings);
    assert_eq!(
        press(&mut panel, Keysym::space, none),
        Some(PanelOutcome::ShipRoute("api".into(), Route::Merge))
    );
    assert_eq!(press(&mut panel, Keysym::Down, none), None);
    assert_eq!(
        press(&mut panel, Keysym::space, none),
        Some(PanelOutcome::ShipTick("api".into(), ShipStep::Push))
    );
    assert_eq!(
        press(&mut panel, Keysym::Return, none),
        Some(PanelOutcome::Ship)
    );
    // With nothing ticked, it finishes.
    let nothing = RowCtx {
        ship_nothing: true,
        ..none
    };
    assert_eq!(
        press(&mut panel, Keysym::Return, nothing),
        Some(PanelOutcome::Finish)
    );
    assert_eq!(
        stage_button(&f, Stage::Ship, nothing),
        Some("Finish\u{2026}")
    );
}

#[test]
fn the_toasts_name_the_feature_and_what_happened() {
    assert_eq!(
        changed_text("Bulk", Stage::Build),
        "The code of \u{201c}Bulk\u{201d} changed since it was checked: back to Build"
    );
    assert_eq!(
        opened_text("Bulk", 1, 0),
        "Opened 1 pull request for \u{201c}Bulk\u{201d}"
    );
    assert_eq!(
        opened_text("Bulk", 2, 0),
        "Opened 2 pull requests for \u{201c}Bulk\u{201d}"
    );
    assert_eq!(
        opened_text("Bulk", 0, 2),
        "Updated 2 pull requests for \u{201c}Bulk\u{201d}"
    );
    assert_eq!(
        opened_text("Bulk", 1, 1),
        "Opened 1 and updated 1 pull requests for \u{201c}Bulk\u{201d}"
    );
    assert_eq!(
        pr_tail("https://github.com/acme/api/pull/12/"),
        "api/pull/12"
    );
    assert_eq!(pr_tail("12"), "12");
}

#[test]
fn the_watch_looks_at_open_features_bound_to_their_code() {
    assert!(!watched(&feature("a", vec![repo("api", true)], 3)));
    assert!(watched(&feature("b", vec![repo("api", true)], 4)));
    let mut squashing = feature("c", vec![repo("api", true)], 1);
    squashing.shipping.squash_at = Some(1);
    assert!(watched(&squashing));
    assert_eq!(watch_every(&squashing), PANEL_EVERY);
    assert_eq!(
        watch_every(&feature("b", vec![repo("api", true)], 4)),
        WATCH_EVERY
    );
    let mut done = feature("d", vec![repo("api", true)], 9);
    done.status = Status::Done;
    assert!(!watched(&done));
}

#[test]
fn sidebar_and_panel_list_the_right_features() {
    let mut open = feature("a", vec![repo("api", true)], 0);
    open.title = "Open".into();
    let mut abandoned = feature("b", vec![repo("api", true)], 1);
    abandoned.status = Status::Abandoned;
    let mut done = feature("c", vec![repo("api", true)], 9);
    done.status = Status::Done;
    let all = vec![open, abandoned, done];
    let shown: Vec<&str> = sidebar_features(&all)
        .iter()
        .map(|f| f.slug.as_str())
        .collect();
    assert_eq!(shown, ["a"]);
    // A done feature's folder is gone here: nothing to remove.
    let listed: Vec<&str> = listed(&all).iter().map(|f| f.slug.as_str()).collect();
    assert_eq!(listed, ["a"]);
    assert_eq!(remove_label(&all[2]), "Remove worktree and branch");
}

#[test]
fn slugs_are_unique_in_the_project() {
    let all = vec![
        feature("bulk-export", vec![], 0),
        feature("bulk-export-2", vec![], 0),
    ];
    assert_eq!(new_slug("Bulk export!", &all), "bulk-export-3");
    assert_eq!(new_slug("Other thing", &all), "other-thing");
}

#[test]
fn the_features_key_shows_the_open_then_the_last_shown_then_the_newest() {
    let slugs = vec!["a".to_owned(), "b".to_owned()];
    assert_eq!(pick_feature(&slugs, Some("a"), Some("b")), Some("a".into()));
    assert_eq!(pick_feature(&slugs, None, Some("a")), Some("a".into()));
    // One gone (abandoned) is passed over.
    assert_eq!(pick_feature(&slugs, None, Some("gone")), Some("b".into()));
    assert_eq!(pick_feature(&[], None, Some("a")), None);
}

#[test]
fn the_created_toast_names_the_ignore_files_from_the_root() {
    let root = Path::new("/w/multi");
    let ignores = vec![
        root.join(".cursorignore"),
        root.join("pyrightconfig.json"),
        PathBuf::from("/elsewhere/.git/info/exclude"),
    ];
    assert_eq!(
        created_text("Bulk export", root, &ignores),
        "Created \u{201c}Bulk export\u{201d} \u{b7} added .features to .cursorignore, \
             pyrightconfig.json, /elsewhere/.git/info/exclude"
    );
    assert_eq!(created_text("X", root, &[]), "Created \u{201c}X\u{201d}");
}

#[test]
fn the_cursor_toast_says_what_moved_where() {
    assert_eq!(
        cursor_freed_text(Path::new("/w/multi")),
        "Cursor agents can edit feature folders \u{b7} moved .features from \
             .cursorignore to .cursorindexingignore in /w/multi"
    );
}

#[test]
fn removing_names_what_is_lost() {
    assert_eq!(
        removal_body("worktree and branch", &[]),
        "Its worktree and branch go; nothing in them is lost"
    );
    assert_eq!(
        removal_body("worktree and branch", &["2 changed files in api".into()]),
        "Its worktree and branch go, and with them 2 changed files in api"
    );
}

#[test]
fn switching_goes_round_and_needs_two() {
    let slugs: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(switch(&slugs, "b", false).as_deref(), Some("a"));
    assert_eq!(switch(&slugs, "a", true).as_deref(), Some("b"));
    assert_eq!(switch(&slugs[..1], "a", true), None);
}

#[test]
fn setup_is_offered_where_tests_run_once_they_are_reached() {
    let at_build = feature("b", vec![repo("api", true)], 3);
    assert!(offers_setup(&at_build, Stage::Build));
    assert!(!offers_setup(&at_build, Stage::Verify));
    assert!(!offers_setup(&at_build, Stage::Plan));
    let mut f = feature("b", vec![repo("api", true)], 4);
    assert!(offers_setup(&f, Stage::Build));
    assert!(offers_setup(&f, Stage::Verify));
    assert!(!offers_setup(&f, Stage::AiReview));
    let setup = |f: &Feature| {
        let actions = page_actions(f, Stage::Verify, RowCtx::default());
        let on = actions.iter().find(|(_, o)| *o == PanelOutcome::SetUp);
        on.map(|(b, _)| (b.label.clone(), b.style))
    };
    assert_eq!(setup(&f), Some(("Set up".into(), ButtonStyle::Quiet)));
    // Its log, once one started.
    assert!(!offers_setup_log(&f, Stage::Verify));
    // Cut short by gerdr quitting: not run, as far as the button goes.
    f.setup = Some(feature::setup::SetupRun::default());
    assert_eq!(setup(&f).unwrap().0, "Set up");
    assert!(offers_setup_log(&f, Stage::Verify));
    assert!(!offers_setup_log(&f, Stage::AiReview));
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        ..Default::default()
    });
    assert_eq!(setup(&f).unwrap().0, "Set up again");
    f.status = Status::Abandoned;
    assert_eq!(setup(&f), None);
}

/// `f` with every stage before Manual test marked.
fn at_manual_test() -> Feature {
    let at = Stage::ALL
        .iter()
        .position(|stage| *stage == Stage::ManualTest)
        .unwrap();
    feature("x", vec![repo("api", true)], at)
}

fn sandbox_labels(f: &Feature, stage: Stage, ctx: RowCtx) -> Vec<String> {
    page_actions(f, stage, ctx)
        .into_iter()
        .filter(|(_, o)| {
            matches!(
                o,
                PanelOutcome::StartSandbox
                    | PanelOutcome::OpenSandbox
                    | PanelOutcome::OpenSandboxPage(_)
                    | PanelOutcome::RestartSandbox
                    | PanelOutcome::ResetSandbox
                    | PanelOutcome::StopSandbox
                    | PanelOutcome::RemoveSandbox
            )
        })
        .map(|(b, _)| b.label)
        .collect()
}

fn running_run() -> feature::sandbox_run::SandboxRun {
    let me = std::process::id() as i32;
    let start = std::fs::read_to_string(format!("/proc/{me}/stat")).unwrap();
    let start: u64 = start
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let proc = feature::sandbox_run::Proc { pid: me, start };
    feature::sandbox_run::SandboxRun {
        holder: Some(proc.clone()),
        pasta: Some(proc),
        ..Default::default()
    }
}

#[test]
fn the_sandbox_is_offered_on_manual_test_only_and_by_its_state() {
    let mut f = at_manual_test();
    let none = RowCtx::default();
    assert_eq!(f.current(), Some(Stage::ManualTest));
    assert_eq!(
        sandbox_labels(&f, Stage::ManualTest, none),
        ["Start sandbox\u{2026}"]
    );
    assert!(sandbox_labels(&f, Stage::Verify, none).is_empty());
    assert!(sandbox_labels(&f, Stage::Ship, none).is_empty());
    let busy = RowCtx {
        sandbox_busy: true,
        ..none
    };
    assert_eq!(sandbox_labels(&f, Stage::ManualTest, busy), ["Stop"]);
    f.sandbox = Some(running_run());
    assert_eq!(
        sandbox_labels(&f, Stage::ManualTest, none),
        ["Open app", "Restart", "Reset data\u{2026}", "Stop\u{2026}"]
    );
    // Reset data is tried between runs: a button on the page, not a menu row.
    let actions = page_actions(&f, Stage::ManualTest, none);
    let page = stage_page(&f, Stage::ManualTest, &PanelState::default(), &actions, 0);
    let reset = (actions.iter())
        .position(|(_, o)| *o == PanelOutcome::ResetSandbox)
        .unwrap();
    assert!(!page.menu.contains(&reset), "{:?}", page.menu);
    f.sandbox = Some(feature::sandbox_run::SandboxRun {
        stopped: true,
        ..running_run()
    });
    assert_eq!(
        sandbox_labels(&f, Stage::ManualTest, none),
        ["Start sandbox\u{2026}", "Remove sandbox\u{2026}"]
    );
    f.status = Status::Abandoned;
    assert!(sandbox_labels(&f, Stage::ManualTest, none).is_empty());
}

#[test]
fn each_page_of_the_sandbox_has_its_button_and_alt_y_goes_on_through_them() {
    use feature::sandbox_pages::Page;
    let mut f = at_manual_test();
    let page = |name: &str, url: &str| Page {
        name: name.into(),
        url: url.into(),
        guess: false,
    };
    f.sandbox = Some(feature::sandbox_run::SandboxRun {
        pages: vec![page("Web app", "http://localhost:9001/")],
        ..running_run()
    });
    let mut panel = FeaturePanel {
        slug: f.slug.clone(),
        ..FeaturePanel::default()
    };
    let mods = ModifiersState {
        alt: true,
        ..ModifiersState::default()
    };
    let mut alt_y =
        |f: &Feature| panel.key(&mods, Keysym::y, f, &[], RowCtx::default(), &[], &[], &[]);
    // One page: Open app is all, and Alt+Y has nothing more to open.
    let none = RowCtx::default();
    assert_eq!(
        sandbox_labels(&f, Stage::ManualTest, none),
        ["Open app", "Restart", "Reset data\u{2026}", "Stop\u{2026}"]
    );
    assert_eq!(alt_y(&f), None);
    if let Some(run) = f.sandbox.as_mut() {
        run.pages
            .push(page("Django admin", "http://localhost:9001/admin/"));
        run.pages.push(page("Expo web", "http://localhost:8081/"));
    }
    assert_eq!(
        sandbox_labels(&f, Stage::ManualTest, none),
        [
            "Open app",
            "Django admin",
            "Expo web",
            "Restart",
            "Reset data\u{2026}",
            "Stop\u{2026}"
        ]
    );
    let offered = page_outcomes(&f, Stage::ManualTest);
    assert!(
        offered.contains(&PanelOutcome::OpenSandboxPage(2)),
        "{offered:?}"
    );
    assert_eq!(alt_y(&f), Some(PanelOutcome::NextSandboxPage));
    assert!(PanelOutcome::NextSandboxPage.reads_only());
    assert!(PanelOutcome::OpenSandboxPage(1).reads_only());
}

#[test]
fn the_sandbox_section_says_what_it_copied_and_what_it_shares() {
    let mut f = at_manual_test();
    let mut state = PanelState::default();
    let text = |f: &Feature, state: &PanelState| -> Vec<String> {
        let mut said = Vec::new();
        for section in sandbox_sections(f, state) {
            let PageSection::Lines { lines, .. } = section else {
                panic!("no lines");
            };
            said.extend(lines.into_iter().map(|l| l.text));
        }
        said
    };
    assert_eq!(
        text(&f, &state),
        ["the project running here, on a copy of its containers and data"]
    );
    f.sandbox = Some(feature::sandbox_run::SandboxRun {
        failed: Some("Docker isn't running, and a sandbox needs it".into()),
        ..Default::default()
    });
    assert_eq!(
        text(&f, &state),
        ["failed: Docker isn't running, and a sandbox needs it"]
    );
    f.sandbox = Some(feature::sandbox_run::SandboxRun {
        copied: vec!["docker_pgdata".into()],
        fresh: vec!["redis".into()],
        outward: vec!["EMAIL_HOST".into()],
        out_of_reach: vec!["DATABASE_URL (docker-db-1, port 5433)".into()],
        forwards: vec![feature::sandbox::Forward {
            service: "db".into(),
            main_port: 5433,
            host_port: 41001,
            udp: false,
        }],
        stopped: true,
        ..Default::default()
    });
    let said = text(&f, &state).join("\n");
    for part in [
        "stopped, its data kept",
        "db: port 5433 here, 127.0.0.1:41001 from outside",
        "copied docker_pgdata",
        "redis started empty",
        "can reach real services: EMAIL_HOST",
        "can't reach what runs outside its project: DATABASE_URL (docker-db-1, port 5433)",
        "shared with main",
    ] {
        assert!(said.contains(part), "{part} not in {said}");
    }
    state.sandbox = Some("Copying docker_pgdata");
    assert_eq!(text(&f, &state).len(), 1);
}

#[test]
fn the_setup_holds_its_own_buttons() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let state = PanelState::default();
    let button = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Quiet,
    };
    let actions = vec![
        (button("Run checks"), PanelOutcome::RunChecks),
        (button("Set up again"), PanelOutcome::SetUp),
        (button("Setup log"), PanelOutcome::SetupLog),
    ];
    // No setup ran: they stay at the title.
    assert!(setup_sections(&f, &state, &actions).is_empty());
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        ..Default::default()
    });
    let sections = setup_sections(&f, &state, &actions);
    assert!(matches!(&sections[0], PageSection::Lines { label, .. } if label == "Setup"));
    assert!(
        matches!(&sections[1], PageSection::Toolbar { groups } if groups[0].1 == vec![1, 2])
    );
}

#[test]
fn the_setup_section_says_what_went_wrong_then_what_it_copied() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let state = PanelState::default();
    assert_eq!(setup_section(&f, &state), None);
    // Under way, and cut short by gerdr quitting.
    f.setup = Some(feature::setup::SetupRun::default());
    assert_eq!(setup_section(&f, &state), None);
    let step = "Setting up \u{201c}B\u{201d}: api: copying .venv".to_owned();
    let running = PanelState {
        setup: Some(&step),
        ..PanelState::default()
    };
    let PageSection::Lines { lines: said, .. } = setup_section(&f, &running).unwrap() else {
        panic!("not lines");
    };
    assert_eq!(said[0].tone, CheckTone::Note);
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        ..Default::default()
    });
    assert_eq!(
        setup_section(&f, &state),
        Some(lines(
            "Setup",
            vec![line("nothing was missing", CheckTone::Note)]
        ))
    );
    // A command that ran isn't "nothing", whatever it changed.
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        ran: vec!["api: pip install -r requirements.txt".into()],
        ..Default::default()
    });
    assert_eq!(
        setup_section(&f, &state),
        Some(lines(
            "Setup",
            vec![line(
                "ran api: pip install -r requirements.txt",
                CheckTone::Note
            )]
        ))
    );
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        failed: Some("api: `npm ci` exited with 1".into()),
        copied: vec!["api: .env".into()],
        not_copied: vec!["api: .venv (exited with 1)".into()],
        ..Default::default()
    });
    assert_eq!(
        setup_section(&f, &state),
        Some(lines(
            "Setup",
            vec![
                line("failed: api: `npm ci` exited with 1", CheckTone::Failed),
                line(
                    "couldn't copy api: .venv (exited with 1)",
                    CheckTone::Failed
                ),
                line("copied api: .env", CheckTone::Note),
            ]
        ))
    );
}

#[test]
fn a_running_build_shows_its_agent_then_its_milestones() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let mut run = agent_run(Some("p1"), None);
    run.started = 1_000;
    f.runs.insert(Stage::Build, run);
    let state = PanelState {
        agent: Some("Claude Code working \u{b7} started 16m ago".into()),
        agent_status: Some(AgentStatus::Working),
        ..PanelState::default()
    };
    let none = RowCtx::default();
    let actions = page_actions(&f, Stage::Build, none);
    assert_eq!(
        labels(&actions),
        [
            ("Show agent", PanelOutcome::ShowAgent(Stage::Build)),
            ("Restart", PanelOutcome::StartAgent(Stage::Build, true)),
            ("Start over\u{2026}", PanelOutcome::StartOver(Stage::Build)),
            ("Set up", PanelOutcome::SetUp),
            (
                "Commands\u{2026}",
                PanelOutcome::EditCommands(CommandsTab::Setup)
            ),
        ]
    );
    assert_eq!(actions[0].0.style, ButtonStyle::Primary);
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    assert_eq!(page.title, "Build");
    // The agent's card below says what it does.
    assert_eq!(page.status, None);
    assert_eq!(
        page.sections,
        [
            PageSection::Agent {
                stage: Stage::Build,
                icon: AgentIcon::Claude,
                name: "Claude Code".into(),
                dot: Dot::Working,
                status: "working \u{b7} started 16m ago".into(),
                tail: Vec::new(),
                buttons: vec![0, 1],
                picks: Vec::new(),
                model: None,
                waits: false,
            },
            PageSection::MilestoneMode { fresh: false }
        ]
    );
    // Show agent, the primary, is on the page's bar and its card; the rare
    // ones in `⋯`.
    assert_eq!(
        (page.primary, page.menu.as_slice()),
        (Some(0), &[2, 3, 4][..])
    );
    let node = rail_node(&f, Stage::Build, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Current, "working 16m")
    );
    // Stopped after a milestone: Continue first, and how far it got.
    f.runs.get_mut(&Stage::Build).unwrap().stop = Some(feature::agents::Stop {
        done: 1,
        total: 3,
        at: 1_900,
    });
    let actions = page_actions(&f, Stage::Build, none);
    assert_eq!(actions[0].1, PanelOutcome::Continue(Stage::Build));
    assert_eq!(actions[1].1, PanelOutcome::ShowAgent(Stage::Build));
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    assert_eq!(
        page.sections.last(),
        Some(&PageSection::Milestones {
            label: "Build".to_owned(),
            state: None,
            done: 1,
            total: 3,
            items: Vec::new(),
            running: Vec::new(),
            buttons: Vec::new(),
        })
    );
    let PageSection::Agent { buttons, .. } = &page.sections[0] else {
        panic!("the agent's card first");
    };
    // Continue, the primary, is on the page's bar and the card, with Show agent.
    assert_eq!(page.primary, Some(0));
    assert_eq!(buttons[..2], [0, 1], "Continue, Show agent");
    let node = rail_node(&f, Stage::Build, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Waiting, "milestone 1/3")
    );
    // Its setup goes last, its failure said by the status at the top;
    // Set up again is a rare one, a row of the `⋯` menu.
    f.setup = Some(feature::setup::SetupRun {
        started: 1_000,
        finished: Some(1_100),
        failed: Some("api: `npm ci` exited with 1".into()),
        ..Default::default()
    });
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    assert!(
        (page.menu.iter()).any(|&b| actions[b].1 == PanelOutcome::SetUp),
        "{:?}",
        page.menu
    );
    // Each of the menu's rows from the page shows its key.
    let panel = FeaturePanel::default();
    let tab = keys::TabCtx::new(
        &panel,
        &f,
        Stage::Build,
        &[],
        state.ctx,
        &[],
        &[],
        &[],
        None,
    );
    let tab = tab.on_rail();
    assert!((page.menu.iter()).all(|&b| tab.chord_of(&actions[b].1).is_some()));
    assert_eq!(
        page.sections.last(),
        Some(&lines(
            "Setup",
            vec![line(
                "failed: api: `npm ci` exited with 1",
                CheckTone::Failed
            )]
        ))
    );
    assert!(matches!(page.sections[0], PageSection::Agent { .. }));
    assert_eq!(page.status.map(|s| s.tone), Some(CheckTone::Failed));
}

/// `f`'s Build page sections, past its agent's card and what's picked.
fn build_sections(f: &Feature) -> Vec<PageSection> {
    stage_page(f, Stage::Build, &PanelState::default(), &[], 10_000).sections
}

#[test]
fn builds_page_says_what_verify_sent_back_above_its_list_and_setup_last() {
    let mut f = feature("p", vec![repo("api", true)], 4);
    assert!(
        !(build_sections(&f).iter()).any(|s| matches!(s, PageSection::Round(_))),
        "none with no such round"
    );
    f.setup = Some(feature::setup::SetupRun {
        finished: Some(2),
        ..Default::default()
    });
    f.verify_fixes = vec![
        VerifyFix {
            next: Some((3, false)),
            ..verify_fix(1, 2)
        },
        VerifyFix {
            dropped: true,
            ..verify_fix(2, 3)
        },
    ];
    let mut read = probe(None);
    read.docs.milestones = BTreeMap::from([(Stage::Build, vec![(true, "API".to_owned())])]);
    let state = PanelState {
        probe: Some(&read),
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::Build, &state, &[], 10_000);
    let names: Vec<&str> = (page.sections.iter())
        .map(|s| match s {
            PageSection::Lines { label, .. } | PageSection::Milestones { label, .. } => {
                label.as_str()
            }
            PageSection::Round(card) => card.label.as_str(),
            _ => "",
        })
        .filter(|name| !name.is_empty() && *name != "Stale")
        .collect();
    assert_eq!(
        names,
        [
            "Fix rounds \u{b7} 2",
            "Fix round 2",
            "Fix round 1",
            "Build",
            "Setup"
        ]
    );
    let cards: Vec<&chrome::RoundCard> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Round(card) => Some(card),
            _ => None,
        })
        .collect();
    // What each sent, the first with why; how each went.
    assert_eq!(cards[0].state.text, "dropped");
    assert_eq!(cards[1].state.text, "fixes made");
    assert_eq!(
        cards[1].meta,
        "asked by Verify \u{b7} made by the Build agent (Claude Code) \u{b7} run 2 failed 16m ago \u{b7} run 3 failed"
    );
    let asked: Vec<(&str, &str)> = (cards[1].asked.iter())
        .map(|a| (a.title.as_str(), a.note.as_str()))
        .collect();
    assert_eq!(
        asked,
        [
            ("api: `test` exited with 1", "FAIL auth"),
            ("web: `lint` exited with 2", ""),
        ]
    );
}

#[test]
fn a_record_without_verify_fixes_reads_and_starting_over_forgets_them() {
    let mut f = feature("p", vec![repo("api", true)], 3);
    f.verify_fixes = vec![verify_fix(1, 2)];
    let mut raw = serde_json::to_value(&f).unwrap();
    raw.as_object_mut().unwrap().remove("verify-fixes");
    raw.as_object_mut().unwrap().remove("verify_fixes");
    let old: Feature = serde_json::from_value(raw).unwrap();
    assert!(old.verify_fixes.is_empty());
    assert!(card_rounds(&old, Stage::Build, None).is_empty());
    let again: Feature = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
    assert_eq!(again.verify_fixes, f.verify_fixes);
    let at = f.current().unwrap();
    assert!(f.start_over(at, 10_000));
    assert!(f.verify_fixes.is_empty());
}

#[test]
fn a_plan_ready_asks_for_its_review() {
    let mut f = feature("p", vec![repo("api", true)], 1);
    let node = rail_node(&f, Stage::Plan, &PanelState::default(), 2_000);
    assert_eq!((node.mark, node.line.as_str()), (NodeMark::Current, ""));
    let page = stage_page(&f, Stage::Plan, &PanelState::default(), &[], 2_000);
    // Nothing ran: it says what happens, and that it doesn't start itself.
    let PageSection::Lines { label, lines: next } = &page.sections[0] else {
        panic!("what happens next first");
    };
    assert_eq!(label, "What happens next");
    assert!(next[0].text.starts_with("Plan doesn't start by itself"));
    assert_eq!(
        page.sections[1..],
        [file(
            "plan.md",
            line("not written yet", CheckTone::Note),
            &[]
        )]
    );
    f.runs
        .insert(Stage::Plan, agent_run(Some("p1"), Some(1_500)));
    let state = PanelState {
        agent: Some("Claude Code done \u{b7} plan.md ready".into()),
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    assert_eq!(
        labels(&actions),
        [
            ("Review", PanelOutcome::OpenDoc(Stage::Plan)),
            ("Show agent", PanelOutcome::ShowAgent(Stage::Plan)),
            ("Restart", PanelOutcome::StartAgent(Stage::Plan, true)),
            ("Start over\u{2026}", PanelOutcome::StartOver(Stage::Plan)),
        ]
    );
    let page = stage_page(&f, Stage::Plan, &state, &actions, 2_000);
    // The agent's card says it: no status line says it again.
    assert_eq!(page.status, None);
    assert!(matches!(
        &page.sections[0],
        PageSection::Agent { status, .. } if status == "done \u{b7} plan.md ready"
    ));
    assert!(matches!(
        page.sections[0],
        PageSection::Agent {
            dot: Dot::Unknown,
            ..
        }
    ));
    // Show agent and Restart are its card's, Start over the `⋯` menu's,
    // Review, the primary, the bar's.
    let PageSection::Agent { buttons, .. } = &page.sections[0] else {
        panic!("the agent's card first");
    };
    assert_eq!(*buttons, [1, 2]);
    assert_eq!((page.primary, page.menu.as_slice()), (Some(0), &[3][..]));
    assert_eq!(
        page.sections[1],
        file("plan.md", line("ready for review", CheckTone::Blocker), &[])
    );
    let node = rail_node(&f, Stage::Plan, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Waiting, "ready for review")
    );
    // Sent back: written again.
    let run = f.runs.get_mut(&Stage::Plan).unwrap();
    (run.ready, run.sent_back) = (None, 2);
    assert_eq!(
        file_line(&f, Stage::Plan, wrote_all()),
        line("sent back \u{d7}2, being written again", CheckTone::Note)
    );
}

#[test]
fn an_approved_brief_reopens_until_the_plan_is_written() {
    let f = feature("a", vec![repo("api", true)], 1);
    // The plan isn't written: the brief reopens.
    let approved = wrote(&[Stage::Discover]);
    assert_eq!(
        labels(&page_actions(&f, Stage::Discover, approved)),
        [
            ("Go to Plan", PanelOutcome::GoToCurrent),
            ("Reopen", PanelOutcome::Untick(Stage::Discover)),
            ("Open", PanelOutcome::OpenDoc(Stage::Discover)),
        ]
    );
    assert_eq!(
        file_line(&f, Stage::Discover, approved),
        line("approved", CheckTone::Ready)
    );
    // The plan is written: nothing changes the brief any more.
    let locked = wrote(&[Stage::Discover, Stage::Plan]);
    assert_eq!(
        labels(&page_actions(&f, Stage::Discover, locked)),
        [
            ("Go to Plan", PanelOutcome::GoToCurrent),
            ("Open", PanelOutcome::OpenDoc(Stage::Discover)),
        ]
    );
    assert_eq!(
        file_line(&f, Stage::Discover, locked),
        line("approved \u{b7} locked", CheckTone::Ready)
    );
    let state = PanelState {
        ctx: locked,
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Discover, locked);
    let page = stage_page(&f, Stage::Discover, &state, &actions, 2_000);
    let status = page.status.unwrap();
    assert!(
        status.text.ends_with("locked: Plan has plan.md"),
        "{status:?}"
    );
    // Enter is called what it does, on both.
    let panel = FeaturePanel::default();
    let hints = |ctx| {
        tab_hints(
            &panel,
            &f,
            Stage::Discover,
            &[],
            ctx,
            AgentChoices::default(),
            &[],
            &[],
            &[],
        )
    };
    let enter = ("Enter".to_owned(), "go to Plan".to_owned());
    let reopen = (REOPEN_KEY.to_owned(), "reopen".to_owned());
    assert!(hints(approved).contains(&enter));
    assert!(hints(approved).contains(&reopen));
    assert!(hints(locked).contains(&enter));
    assert!(!hints(locked).contains(&reopen));
    // The plan's page offers what frees it.
    let plan = page_actions(&f, Stage::Plan, locked);
    assert!(
        labels(&plan).contains(&("Start over\u{2026}", PanelOutcome::StartOver(Stage::Plan)))
    );
    // Plan's Restart doesn't reopen the brief: it stays locked.
    assert!(
        labels(&plan)
            .iter()
            .all(|(_, o)| *o != PanelOutcome::Untick(Stage::Discover))
    );
}

#[test]
fn a_changed_approved_file_is_told_on_its_page() {
    let f = feature("c", vec![repo("api", true)], 1);
    let ctx = wrote(&[Stage::Discover]);
    let mut p = probe(None);
    p.docs.changed.insert(Stage::Discover);
    let state = PanelState {
        ctx,
        probe: Some(&p),
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::Discover, &state, &[], 2_000);
    assert!(page.sections.contains(&lines(
        "Changed",
        vec![line(
            "brief.md changed after it was approved",
            CheckTone::Blocker
        )]
    )));
}

#[test]
fn panel_keys_reopen_and_start_over_a_stage() {
    let mut panel = FeaturePanel::default();
    let f = feature("k", vec![repo("api", true)], 3);
    let ctrl_shift = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    // Their hints are clicked by the key they name.
    assert_eq!(crate::keys::hint_key(REOPEN_KEY).unwrap().1, Keysym::Return);
    assert_eq!(
        crate::keys::hint_key(START_OVER_KEY).unwrap().1,
        Keysym::Delete
    );
    let reopen = (ctrl_shift, Keysym::Return);
    let start_over = (ctrl_shift, Keysym::Delete);
    // On Build, current with nothing made: nothing to start over or reopen.
    assert_eq!(press(&mut panel, &f, reopen), None);
    assert_eq!(press(&mut panel, &f, start_over), None);
    // On Plan review, done: it reopens.
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(panel.page_stage(&f), Stage::PlanReview);
    assert_eq!(
        press(&mut panel, &f, reopen),
        Some(PanelOutcome::Untick(Stage::PlanReview))
    );
    assert_eq!(press(&mut panel, &f, start_over), None);
}

/// Every stage's file is there.
#[test]
fn a_file_ready_for_review_is_approved_from_its_page() {
    let stage = Stage::Discover;
    let approve = PanelOutcome::ApproveDoc(stage);
    let ctx = wrote(&[stage]);
    let mut f = feature("a", vec![repo("api", true)], 0);
    let offered = |f: &Feature, ctx| {
        page_actions(f, stage, ctx)
            .iter()
            .any(|(_, o)| *o == approve)
    };
    // Being written: nothing to approve yet.
    f.runs.insert(stage, agent_run(Some("p1"), None));
    assert!(!offered(&f, ctx));
    f.runs.insert(stage, agent_run(Some("p1"), Some(1_500)));
    assert!(!offered(&f, RowCtx::default()));
    let actions = page_actions(&f, stage, ctx);
    // On the file's card, next to Review, which Enter still presses.
    let state = PanelState {
        ctx,
        ..PanelState::default()
    };
    let page = stage_page(&f, stage, &state, &actions, 2_000);
    let held = page.sections.iter().find_map(|s| match s {
        PageSection::File { buttons, .. } => Some(buttons.clone()),
        _ => None,
    });
    let on_card: Vec<&str> = (held.unwrap().iter())
        .map(|i| actions[*i].0.label.as_str())
        .collect();
    // Review, the primary, is on the page's bar.
    assert_eq!(on_card, ["Approve"]);
    assert_eq!(
        page.primary.map(|i| actions[i].0.label.as_str()),
        Some("Review")
    );
    assert_eq!(
        page_enter(&f, stage, ctx),
        Some(PanelOutcome::OpenDoc(stage))
    );
    // Ctrl+Enter approves, and the footer says so.
    let mut panel = FeaturePanel::default();
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    let slugs = ["a".to_owned()];
    let got = panel.key(&ctrl, Keysym::Return, &f, &slugs, ctx, &[], &[], &[]);
    assert_eq!(got, Some(approve.clone()));
    // From the cards or the repositories too, not unfolding them.
    for focus in [TabArea::Findings, TabArea::Repos] {
        panel.focus = focus;
        let got = panel.key(&ctrl, Keysym::Return, &f, &slugs, ctx, &[], &[], &[]);
        assert_eq!(got, Some(approve.clone()));
        assert!(!panel.expanded);
    }
    panel.focus = TabArea::Stages;
    let hints = tab_hints(
        &panel,
        &f,
        stage,
        &[],
        ctx,
        AgentChoices::default(),
        &[],
        &[],
        &[],
    );
    assert!(hints.contains(&("Ctrl+Enter".into(), "approve".into())));
    // Off the cards too, as the key does.
    panel.focus = TabArea::Findings;
    let hints = tab_hints(
        &panel,
        &f,
        stage,
        &[],
        ctx,
        AgentChoices::default(),
        &[],
        &[],
        &[],
    );
    assert!(hints.contains(&("Ctrl+Enter".into(), "approve".into())));
    // A closed feature approves nothing.
    f.status = Status::Abandoned;
    assert!(!offered(&f, ctx));
}

#[test]
fn a_restarted_agent_leaves_the_file_shown_as_written() {
    let mut f = feature("r", vec![repo("api", true)], 0);
    // The file card and its Review button read the same probe.
    let written = RowCtx {
        written: [true; Stage::ALL.len()],
        ..RowCtx::default()
    };
    // Restarted: the new run isn't ready, the last run's brief is there.
    f.runs.insert(Stage::Discover, agent_run(Some("p2"), None));
    let actions = page_actions(&f, Stage::Discover, written);
    let review = PanelOutcome::OpenDoc(Stage::Discover);
    assert!(labels(&actions).contains(&("Review", review)));
    let state = PanelState {
        ctx: written,
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::Discover, &state, &actions, 2_000);
    let card = (page.sections.iter()).find_map(|s| match s {
        PageSection::File { state, .. } => Some(state.clone()),
        _ => None,
    });
    let kept = line(
        "written \u{b7} the agent may still change it",
        CheckTone::Note,
    );
    assert_eq!(card, Some(kept));
    // Ctrl+Enter can't approve it yet, and says why.
    let ctrl = ModifiersState {
        ctrl: true,
        ..ModifiersState::default()
    };
    let slugs = [f.slug.clone()];
    let got =
        FeaturePanel::default().key(&ctrl, Keysym::Return, &f, &slugs, written, &[], &[], &[]);
    assert_eq!(got, Some(PanelOutcome::FindingRefused(DOC_UNREAD)));
    // Nothing there yet: it is being written, as before.
    assert_eq!(
        file_line(&f, Stage::Discover, RowCtx::default()),
        line("being written", CheckTone::Note)
    );
    // That agent's tab closed: nothing is changing the file.
    f.runs.get_mut(&Stage::Discover).unwrap().pane = None;
    assert_eq!(
        file_line(&f, Stage::Discover, wrote_all()),
        line("written", CheckTone::Note)
    );
    // Never had an agent, the file put there by hand.
    f.runs.remove(&Stage::Discover);
    assert_eq!(
        file_line(&f, Stage::Discover, wrote_all()),
        line("written", CheckTone::Note)
    );
    assert_eq!(
        file_line(&f, Stage::Discover, RowCtx::default()),
        line("not written yet", CheckTone::Note)
    );
}

#[test]
fn a_failed_verify_lists_its_failures_and_who_fixes_them() {
    let mut f = feature("v", vec![repo("api", true)], 4);
    let fails = vec![
        "api: `npm test` exited with 1".to_owned(),
        "web: lint".into(),
    ];
    f.verify = Some(feature::verify::VerifyRun {
        n: 2,
        started: 1_000,
        finished: Some(1_100),
        passed: Some(false),
        failures: fails,
        ..Default::default()
    });
    let checks = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    let state = PanelState {
        ctx: checks,
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Verify, checks);
    assert_eq!(
        labels(&actions),
        [
            ("Make fixes", PanelOutcome::MakeVerifyFixes),
            ("Plan fixes", PanelOutcome::PlanFixes),
            ("Run checks", PanelOutcome::RunChecks),
            ("Start over\u{2026}", PanelOutcome::StartOver(Stage::Verify)),
            ("Evidence", PanelOutcome::Evidence),
            ("Set up", PanelOutcome::SetUp),
            (
                "Commands\u{2026}",
                PanelOutcome::EditCommands(CommandsTab::Feature)
            ),
        ]
    );
    let page = stage_page(&f, Stage::Verify, &state, &actions, 2_000);
    assert_eq!(
        page.status.as_ref().map(|s| s.tone),
        Some(CheckTone::Failed)
    );
    let PageSection::Lines { label, lines } = &page.sections[0] else {
        panic!("the checks' lines");
    };
    assert_eq!(label, "Checks");
    assert_eq!(
        texts(lines),
        [
            ("run 2 failed", CheckTone::Failed),
            ("api: `npm test` exited with 1", CheckTone::Failed),
            ("web: lint", CheckTone::Failed),
        ]
    );
    let node = rail_node(&f, Stage::Verify, &state, 2_000);
    assert_eq!(
        (node.mark, node.line.as_str()),
        (NodeMark::Failed, "failed")
    );
    // While they run, what they do.
    let state = PanelState {
        ctx: checks,
        verify: Some("api: npm test (2 of 5)"),
        ..PanelState::default()
    };
    assert_eq!(
        checks_lines(&f, &state),
        [line("api: npm test (2 of 5)", CheckTone::Note)]
    );
    assert_eq!(rail_node(&f, Stage::Verify, &state, 2_000).line, "running");
}

/// The cards `f`'s Verify page holds with `plan`, and its heading.
fn verify_page(f: &Feature, plan: Vec<PlannedCheck>) -> (String, Vec<FindingCard>) {
    let state = PanelState {
        ctx: RowCtx {
            checks: true,
            ..RowCtx::default()
        },
        plan,
        ..PanelState::default()
    };
    let actions = stage_actions(f, Stage::Verify, state.ctx, &state.plan, &[], &[]);
    let sections = verify_sections(f, &state, &actions, 2_000);
    let mut heading = String::new();
    let mut cards = Vec::new();
    for section in sections {
        match section {
            PageSection::Finding(card) => cards.push(card),
            PageSection::Lines { label, .. } => heading = label,
            _ => {}
        }
    }
    (heading, cards)
}

#[test]
fn verifys_page_has_a_card_per_check_from_the_start() {
    let f = feature("f", vec![repo("api", true)], 4);
    let plan = vec![
        planned("make", false, false),
        planned("mine", false, true),
        planned("slow", true, false),
    ];
    let (heading, cards) = verify_page(&f, plan.clone());
    assert_eq!(heading, "Checks \u{b7} 2 not run \u{b7} 1 skipped");
    let tags: Vec<_> = cards.iter().map(|c| (c.title.as_str(), c.tag)).collect();
    assert_eq!(
        tags,
        [
            ("make", FindingTag::NotRun),
            ("mine", FindingTag::NotRun),
            ("slow", FindingTag::Skipped),
        ]
    );
    assert!(cards[2].faded && cards[2].place.contains("skipped for this feature"));
    assert!(cards.iter().all(|c| c.preview.is_empty()));
    // Nothing to run says so, as ever; without cards nothing counts.
    let state = PanelState {
        ctx: RowCtx {
            checks: true,
            ..RowCtx::default()
        },
        verify: Some("api: make (1 of 2)"),
        ..PanelState::default()
    };
    assert_eq!(
        checks_lines(&f, &state),
        [line("api: make (1 of 2)", CheckTone::Note)]
    );
    // A stage not reached yet shows them, with nothing to press.
    let mut early = f.clone();
    early.marks.clear();
    let state = PanelState {
        plan,
        ..PanelState::default()
    };
    let actions = stage_actions(&early, Stage::Verify, state.ctx, &state.plan, &[], &[]);
    assert!(actions.is_empty());
    let page = stage_page(&early, Stage::Verify, &state, &actions, 2_000);
    let shown = (page.sections.iter())
        .filter(|s| matches!(s, PageSection::Finding(_)))
        .count();
    assert_eq!(shown, 3);
}

#[test]
fn verifys_cards_put_the_last_runs_first_and_keep_a_removed_check() {
    let f = verified(false);
    let plan = vec![
        planned("test", false, false),
        planned("new", false, false),
        planned("lint", true, false),
        planned("slow", true, false),
    ];
    let entries = check_entries(&f, Stage::Verify, &plan);
    let got: Vec<_> = (entries.iter())
        .map(|e| (e.command.as_str(), e.row, e.skipped))
        .collect();
    assert_eq!(
        got,
        [
            ("lint", Some(0), true),
            ("test", Some(1), false),
            ("new", None, false),
            ("slow", None, true),
        ]
    );
    // No longer given: it stays, and can't be skipped.
    let entries = check_entries(&f, Stage::Verify, &[planned("test", false, false)]);
    assert_eq!(entries[0].command, "lint");
    assert!(!entries[0].borrowed);
    assert!(skip_action(&f, Stage::Verify, &entries[0], RowCtx::default()).is_none());
    assert!(check_entries(&f, Stage::Build, &plan).is_empty());
}

#[test]
fn a_skipped_check_offers_only_to_stop_skipping_it() {
    let f = verified(false);
    let plan = vec![planned("lint", true, false), planned("test", false, true)];
    let (heading, cards) = verify_page(&f, plan.clone());
    assert_eq!(heading, "Checks \u{b7} 1 failed \u{b7} 1 skipped");
    let actions = stage_actions(&f, Stage::Verify, RowCtx::default(), &plan, &[], &[]);
    let held = |card: &FindingCard| -> Vec<String> {
        (card.buttons.iter())
            .map(|i| actions[*i].0.label.clone())
            .collect()
    };
    assert_eq!(held(&cards[0]), ["Show", "Don't skip"]);
    // Its own check can't be skipped.
    assert_eq!(held(&cards[1]), ["Show", "Run again"]);
    // Still going when it was skipped: it runs to its end on its card.
    let running = verified(true);
    let (_, cards) = verify_page(&running, vec![planned("test", true, false)]);
    assert_eq!(cards[1].tag, FindingTag::Running);
}

#[test]
fn verifys_cards_keys_act_on_what_a_card_has() {
    let f = verified(false);
    let plan = vec![
        planned("lint", false, false),
        planned("test", true, false),
        planned("new", false, false),
    ];
    let mut panel = FeaturePanel {
        focus: TabArea::Findings,
        ..FeaturePanel::default()
    };
    let hit = |panel: &mut FeaturePanel, f: &Feature, sym| {
        let (mods, sym) = key(sym);
        let slugs = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        panel.key(&mods, sym, f, &slugs, RowCtx::default(), &plan, &[], &[])
    };
    // A card with a row: Space skips it.
    assert_eq!(
        hit(&mut panel, &f, Keysym::space),
        Some(PanelOutcome::SkipCheck {
            repo: "api".into(),
            command: "lint".into(),
            skip: true,
        })
    );
    // A skipped one: Space stops that, `r` doesn't run it.
    hit(&mut panel, &f, Keysym::Down);
    assert_eq!(panel.finding, 1);
    assert_eq!(hit(&mut panel, &f, Keysym::r), None);
    assert_eq!(
        hit(&mut panel, &f, Keysym::space),
        Some(PanelOutcome::SkipCheck {
            repo: "api".into(),
            command: "test".into(),
            skip: false,
        })
    );
    // A card no run got to has no terminal: nothing to show or stop,
    // and Enter doesn't start the page's primary.
    hit(&mut panel, &f, Keysym::Down);
    assert_eq!(panel.finding, 2);
    assert_eq!(hit(&mut panel, &f, Keysym::Return), None);
    assert_eq!(hit(&mut panel, &f, Keysym::r), None);
    assert_eq!(hit(&mut panel, &f, Keysym::s), None);
    // While they run, nothing is skipped.
    let running = verified(true);
    panel.finding = 0;
    assert_eq!(hit(&mut panel, &running, Keysym::space), None);
    // Nor on a stage not reached yet.
    let mut early = verified(false);
    early.marks.clear();
    assert_eq!(hit(&mut panel, &early, Keysym::space), None);
}

#[test]
fn a_running_card_says_how_long_and_keeps_its_height() {
    let mut f = verified(true);
    let run = f.verify.as_mut().unwrap();
    run.started = 1_900;
    run.checks[1].last_secs = Some(100);
    let plan = vec![planned("lint", false, false), planned("test", false, false)];
    let tails = HashMap::from([(
        "p-test".to_owned(),
        AgentTail {
            lines: vec!["compiling".to_owned()],
            ..AgentTail::default()
        },
    )]);
    let state = PanelState {
        plan,
        check_tails: Some(&tails),
        ..PanelState::default()
    };
    let entries = check_entries(&f, Stage::Verify, &state.plan);
    let card = check_card(&entries[1], &f, &state, &[], 2_000);
    assert_eq!(card.tag, FindingTag::Running);
    assert_eq!(card.state, "1m 40s \u{b7} took 1m 40s last run");
    assert_eq!(card.preview, ["compiling", "", "", ""]);
    // The one that ended keeps what it printed.
    let ended = check_card(&entries[0], &f, &state, &[], 2_000);
    assert_eq!(ended.preview, ["first", "boom"]);
}

#[test]
fn a_check_in_the_queue_says_so_and_its_clock_starts_with_it() {
    let mut f = verified(true);
    let run = f.verify.as_mut().unwrap();
    run.started = 1_000;
    run.checks[1].queued = true;
    run.checks[1].last_secs = Some(100);
    let state = PanelState {
        plan: vec![planned("lint", false, false), planned("test", false, false)],
        ..PanelState::default()
    };
    let entries = check_entries(&f, Stage::Verify, &state.plan);
    let card = check_card(&entries[1], &f, &state, &[], 2_000);
    assert_eq!(card.tag, FindingTag::Queued);
    assert_eq!(card.state, "waiting for a check in this folder");
    assert!(checks_heading(std::slice::from_ref(&card)).ends_with("1 waiting"));
    // Its turn came: the time before it isn't the check's.
    let run = f.verify.as_mut().unwrap();
    run.checks[1].queued = false;
    run.checks[1].began = Some(1_990);
    let card = check_card(&entries[1], &f, &state, &[], 2_000);
    assert_eq!(card.tag, FindingTag::Running);
    assert!(card.state.starts_with("10s"), "{}", card.state);
}

#[test]
fn the_older_code_mark_needs_both_codes() {
    let mut f = verified(false);
    let reading = |patch: &str| Probe {
        reading: Ok(Reading {
            patch: patch.to_owned(),
            ..Reading::default()
        }),
        ..probe(None)
    };
    // The run read none.
    assert!(!on_older_code(&f, Some(&reading("b"))));
    f.verify.as_mut().unwrap().patch = Some("a".to_owned());
    assert!(!on_older_code(&f, None));
    assert!(!on_older_code(&f, Some(&reading(""))));
    assert!(!on_older_code(&f, Some(&reading("a"))));
    assert!(on_older_code(&f, Some(&reading("b"))));
    let (a, b) = (reading("a"), reading("b"));
    let state = |probe| PanelState {
        probe,
        plan: vec![planned("lint", false, false)],
        ..PanelState::default()
    };
    let card = |probe| {
        let state = state(probe);
        let entries = check_entries(&f, Stage::Verify, &state.plan);
        check_card(&entries[0], &f, &state, &[], 2_000).state
    };
    assert!(!card(Some(&a)).contains("on older code"));
    assert!(card(Some(&b)).contains("on older code"));
}

#[test]
fn every_check_skipped_doesnt_ask_to_set_some() {
    let mut f = feature("f", vec![repo("api", true)], 4);
    f.marks.remove(&Stage::Verify);
    let line = verify_line(
        &f,
        RowCtx {
            all_skipped: true,
            ..RowCtx::default()
        },
        None,
    );
    assert!(line.is_some_and(|l| l.starts_with("every check is skipped")));
}

#[test]
fn spans_read_in_their_two_largest_units() {
    assert_eq!(span_words(42), "42s");
    assert_eq!(span_words(12 * 60 + 5), "12m");
    assert_eq!(span_words(3 * 3_600), "3h");
    assert_eq!(span_words(3 * 3_600 + 10 * 60), "3h 10m");
    assert_eq!(span_words(2 * 86_400 + 4 * 3_600 + 59), "2d 4h");
    assert_eq!(span_words(86_400 + 30), "1d");
}

/// The summary shows as its paragraphs, each on one line, with what is
/// known of who writes it.
#[test]
fn the_recap_shows_the_summary_and_who_writes_it() {
    use crate::feature::SummaryRun;
    let mut f = feature("s", vec![repo("api", true)], 8);
    let mut p = probe(None);
    assert_eq!(
        recap_summary(&f, Some(&p)),
        (Vec::new(), Some("No summary yet".to_owned()))
    );
    f.shipping.summary = Some(SummaryRun {
        at: 100,
        agent: "claude".into(),
        pane: None,
        n: 1,
    });
    assert_eq!(
        recap_summary(&f, Some(&p)).1.as_deref(),
        Some("claude is writing a summary\u{2026}")
    );
    p.docs.summary = Some((Ok("One\nline.\n\nTwo.\n".into()), 200));
    let (paragraphs, note) = recap_summary(&f, Some(&p));
    assert_eq!(paragraphs, ["One line.", "Two."]);
    assert_eq!(
        note.as_deref(),
        Some("Goes in the pull requests and the squash's message")
    );
    // Asked again since it was written: the new one is on its way.
    f.shipping.summary.as_mut().unwrap().at = 300;
    assert_eq!(
        recap_summary(&f, Some(&p)).1.as_deref(),
        Some("claude is writing it again\u{2026}")
    );
}
