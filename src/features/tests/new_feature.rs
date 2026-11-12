//! Tests of the new feature dialog, the panel's keys and its repositories' rows.

use super::*;

#[test]
fn a_review_picks_a_pull_request_per_repository() {
    let pull = |number: u64, head: &str| feature::pr::OpenPull {
        number,
        title: format!("Change {number}"),
        head_ref: head.into(),
        author: "bob".into(),
        url: format!("https://github.com/o/r/pull/{number}"),
        ..Default::default()
    };
    let repo = |name: &str| (None, name.to_owned());
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.review = Some(Reviewing {
        wanted: Some(("github.com".into(), "o/r".into(), 12)),
        ..Reviewing::default()
    });
    d.found(Ok(root(&["api", "docs", "web"])));
    assert_eq!(d.unlisted().len(), 3);
    assert!(d.unlisted().is_empty());
    d.pulls_listed(&repo("web"), Ok(vec![pull(30, "fix"), pull(31, "other")]));
    // The one it was opened for, and the same branch's elsewhere.
    d.pulls_listed(&repo("api"), Ok(vec![pull(12, "fix")]));
    d.pulls_listed(&repo("docs"), Ok(Vec::new()));
    let view = new_feature_view(&d, "Acme".into());
    assert_eq!(view.heading, "Review pull requests in \u{201c}Acme\u{201d}");
    assert_eq!(view.root, "#30 of web picked too: same branch fix");
    // web listed after api's pick: the same.
    let mut later = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    later.review = Some(Reviewing::default());
    later.found(Ok(root(&["api", "web"])));
    later.unlisted();
    later.pulls_listed(&repo("api"), Ok(vec![pull(12, "fix")]));
    later.pick_pull_of(&repo("api"), 12);
    later.pulls_listed(&repo("web"), Ok(vec![pull(30, "fix")]));
    let root_line = new_feature_view(&later, "Acme".into()).root;
    assert_eq!(root_line, "#30 of web picked too: same branch fix");
    let rows: Vec<(&str, bool, Option<&str>)> = (view.repos.iter())
        .map(|r| (r.place.as_str(), r.touched, r.base.as_deref()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("Change 12 \u{b7} @bob", true, Some("#12")),
            ("no open pull request", false, Some("Pick\u{2026}")),
            ("Change 30 \u{b7} @bob", true, Some("#30")),
        ]
    );
    // Space leaves web out; again, it lists web's to pick from.
    d.focus = Some(2);
    dkey(&mut d, key(Keysym::space));
    assert_eq!(
        new_feature_view(&d, "Acme".into()).repos[2].place,
        "left out"
    );
    dkey(&mut d, key(Keysym::space));
    assert_eq!(d.rows().len(), 2);
    dkey(&mut d, key(Keysym::Down));
    dkey(&mut d, key(Keysym::Return));
    assert!(d.choosing.is_none());
    let Some(DialogOutcome::Review { picks, title, .. }) = dkey(&mut d, key(Keysym::Return))
    else {
        panic!("a review")
    };
    assert!(title.is_empty());
    let picked: Vec<(&str, u64)> = picks.iter().map(|p| (p.repo.as_str(), p.number)).collect();
    assert_eq!(picked, vec![("api", 12), ("web", 31)]);
    assert_eq!(picks[0].main, PathBuf::from("/work/api"));
    // None picked: nothing to review.
    d.focus = Some(0);
    dkey(&mut d, key(Keysym::space));
    d.focus = Some(2);
    dkey(&mut d, key(Keysym::space));
    assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
    assert!(
        d.error
            .as_deref()
            .is_some_and(|e| e.starts_with("Pick a pull request"))
    );
}

#[test]
fn a_review_changes_its_pull_requests_in_the_dialog() {
    let mut f = feature("pr-12", vec![repo("api", true)], 4);
    f.title = "#12 Fix".into();
    f.kind = feature::Kind::PrReview(vec![feature::pr::PullRef {
        repo: "api".into(),
        number: 12,
        author: "bob".into(),
        head_ref: "fix".into(),
        url: "https://github.com/o/r/pull/12".into(),
        ..Default::default()
    }]);
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work".into());
    d.review = Some(Reviewing::default());
    d.edit(&f);
    d.found(Ok(root(&["api", "web"])));
    // Picked before its list is in, and listed all the same; web, which
    // it doesn't hold, isn't picked for its branch.
    assert_eq!(d.unlisted().len(), 2);
    let pull = |number: u64| feature::pr::OpenPull {
        number,
        author: "bob".into(),
        head_ref: "fix".into(),
        url: format!("https://github.com/o/r/pull/{number}"),
        ..Default::default()
    };
    d.pulls_listed(&(None, "api".into()), Ok(vec![pull(12)]));
    d.pulls_listed(&(None, "web".into()), Ok(vec![pull(30)]));
    assert_eq!(
        new_feature_view(&d, "Acme".into()).repos[1].place,
        "left out"
    );
    let view = new_feature_view(&d, "Acme".into());
    assert_eq!(view.heading, "Pull requests of \u{201c}#12 Fix\u{201d}");
    assert_eq!(view.create, chrome::SAVE_PULLS);
    assert_eq!(view.repos[0].place, "@bob");
    assert_eq!(view.repos[0].base.as_deref(), Some("#12"));
    let Some(DialogOutcome::Review { picks, into, .. }) = dkey(&mut d, key(Keysym::Return))
    else {
        panic!("a change")
    };
    assert_eq!(into.as_deref(), Some("pr-12"));
    assert_eq!(picks.iter().map(|p| p.number).collect::<Vec<_>>(), vec![12]);
}

#[test]
fn a_new_review_picks_its_reviewer_in_the_dialog() {
    let chip = |name: &str, lit| AgentChip {
        icon: AgentIcon::from_agent(name),
        name: name.into(),
        lit,
        off: false,
    };
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.review = Some(Reviewing {
        reviewers: vec![chip("Claude Code", true), chip("Cursor", false)],
        default_reviewer: Some("Claude Code".into()),
        ..Reviewing::default()
    });
    d.found(Ok(root(&["api"])));
    d.unlisted();
    let pull = feature::pr::OpenPull {
        number: 12,
        url: "https://github.com/o/r/pull/12".into(),
        ..Default::default()
    };
    d.pulls_listed(&(None, "api".into()), Ok(vec![pull]));
    d.pick_pull_of(&(None, "api".into()), 12);
    // Tab: the title, the repository, then the Reviewer row.
    dkey(&mut d, key(Keysym::Tab));
    dkey(&mut d, key(Keysym::Tab));
    let view = new_feature_view(&d, "Acme".into());
    assert!(view.reviewer_focus);
    // First, so it stays when the key row drops what doesn't fit.
    assert_eq!(view.keys.first(), Some(&("Left / Right", "reviewer")));
    let reviewer = |d: &mut NewFeatureDialog| match dkey(d, key(Keysym::Return)) {
        Some(DialogOutcome::Review { reviewer, .. }) => reviewer,
        other => panic!("a review, not {other:?}"),
    };
    // The default needs no pick; Right picks Cursor, past the end stays.
    assert_eq!(reviewer(&mut d), None);
    dkey(&mut d, key(Keysym::Right));
    dkey(&mut d, key(Keysym::Right));
    assert_eq!(reviewer(&mut d).as_deref(), Some("Cursor"));
    dkey(&mut d, key(Keysym::Left));
    assert_eq!(reviewer(&mut d), None);
    // Tab goes round to the title; Left there moves no pick.
    dkey(&mut d, key(Keysym::Tab));
    assert_eq!(d.focus, None);
    // Changing a review's pull requests offers no Reviewer row.
    let mut f = feature("pr-12", vec![repo("api", true)], 4);
    f.kind = feature::Kind::PrReview(Vec::new());
    d.edit(&f);
    assert!(new_feature_view(&d, "Acme".into()).reviewers.is_empty());
}

/// A feature at Plan with an agent at work, written on another machine.
fn written_elsewhere() -> Feature {
    let mut f = feature("f", vec![repo("api", true)], 1);
    f.runs.insert(
        Stage::Plan,
        feature::agents::StageRun {
            pane: Some("m1/w1:p1".into()),
            machine: Some("m1".into()),
            ..Default::default()
        },
    );
    f.writer = Some("m1".into());
    f
}

#[test]
fn what_another_machine_writes_offers_what_this_one_would_and_taking_it_over() {
    let f = written_elsewhere();
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    let labels: Vec<&str> = actions.iter().map(|(b, _)| b.label.as_str()).collect();
    assert!(labels.contains(&"Show agent"), "{labels:?}");
    assert!(labels.contains(&"Take over"), "{labels:?}");
    let mut mine = f.clone();
    mine.writer = None;
    let own = page_actions(&mine, Stage::Plan, RowCtx::default());
    assert!(
        own.iter().all(|(_, o)| *o != PanelOutcome::TakeOver),
        "{own:?}"
    );
    // All else is the same: a press on it moves the feature here first.
    let without: Vec<_> = (actions.iter())
        .filter(|(_, o)| *o != PanelOutcome::TakeOver)
        .cloned()
        .collect();
    assert_eq!(without, own);
    assert!(stop_action(&f, Stage::Plan).is_some());
}

#[test]
fn what_another_machine_writes_has_the_buttons_of_its_writer() {
    let f = written_elsewhere();
    assert_eq!(remove_label(&f), "Abandon");
    let entry = RepoEntry::Held(0);
    let mut mine = f.clone();
    mine.writer = None;
    assert_eq!(
        entry_buttons(&f, &[], entry),
        entry_buttons(&mine, &[], entry)
    );
}

#[test]
fn what_is_over_on_another_machine_stays_only_to_look_at() {
    let mut f = written_elsewhere();
    f.status = Status::Done;
    assert_eq!(remove_label(&f), "");
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    assert!(
        actions.iter().all(|(_, o)| *o != PanelOutcome::TakeOver),
        "{actions:?}"
    );
}

#[test]
fn ctrl_shift_e_takes_over_only_what_another_machine_writes() {
    let f = written_elsewhere();
    let mut panel = FeaturePanel {
        slug: f.slug.clone(),
        ..FeaturePanel::default()
    };
    let mods = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    let press = |panel: &mut FeaturePanel, f: &Feature| {
        panel.key(&mods, Keysym::E, f, &[], RowCtx::default(), &[], &[], &[])
    };
    assert_eq!(press(&mut panel, &f), Some(PanelOutcome::TakeOver));
    let mut mine = f.clone();
    mine.writer = None;
    assert_eq!(press(&mut panel, &mine), None);
}

#[test]
fn a_feature_taken_over_here_offers_handing_it_back() {
    let mut f = feature("f", vec![repo("api", true)], 1);
    let offers = |f: &Feature| {
        (page_actions(f, Stage::Plan, RowCtx::default()).iter())
            .any(|(b, o)| *o == PanelOutcome::HandBack && b.label == "Hand back")
    };
    assert!(!offers(&f));
    f.came_from = Some("m1".into());
    assert!(offers(&f));
    let mut panel = FeaturePanel {
        slug: f.slug.clone(),
        ..FeaturePanel::default()
    };
    let mods = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    let pressed = panel.key(&mods, Keysym::E, &f, &[], RowCtx::default(), &[], &[], &[]);
    assert_eq!(pressed, Some(PanelOutcome::HandBack));
    // Over, it stays here; shown from another machine, it is taken over.
    let mut over = f.clone();
    over.status = Status::Done;
    assert!(!offers(&over));
    let mut seen = f.clone();
    seen.writer = Some("m1".into());
    assert!(!offers(&seen));
    let pressed = panel.key(
        &mods,
        Keysym::E,
        &seen,
        &[],
        RowCtx::default(),
        &[],
        &[],
        &[],
    );
    assert_eq!(pressed, Some(PanelOutcome::TakeOver));
}

/// A feature another machine writes, at Manual test, with its sandbox as
/// `alive` says.
fn sandbox_elsewhere(alive: bool) -> Feature {
    use feature::sandbox_run::{Elsewhere, SandboxRun};
    let mut f = feature("g", vec![repo("api", true)], Stage::ManualTest as usize);
    f.writer = Some("m1".into());
    f.sandbox = Some(SandboxRun {
        finished: Some(1),
        elsewhere: Some(Elsewhere {
            asked: true,
            alive,
            ports: vec![5173],
        }),
        ..SandboxRun::default()
    });
    f
}

/// The footer's hints of `panel` over `f` on `stage`, as `ctx` has it.
fn hints_at(
    panel: &FeaturePanel,
    f: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Vec<(String, String)> {
    let choices = AgentChoices::default();
    tab_hints(panel, f, stage, &[], ctx, choices, &[], &[], &[])
}

/// The hints of `chord` among `hints`.
fn hints_of<'h>(hints: &'h [(String, String)], chord: &str) -> Vec<&'h str> {
    (hints.iter())
        .filter(|(k, _)| k == chord)
        .map(|(_, w)| w.as_str())
        .collect()
}

#[test]
fn the_sandbox_keys_hint_what_their_handler_does() {
    // Being stopped, running on a worker: Ctrl+Shift+T shows its
    // terminals there, and says so.
    let mut f = sandbox_elsewhere(true);
    f.writer = None;
    f.sandbox.as_mut().unwrap().machine = Some("w1".into());
    let busy = RowCtx {
        sandbox_busy: true,
        ..RowCtx::default()
    };
    let mut panel = FeaturePanel::default();
    let hints = hints_at(&panel, &f, Stage::ManualTest, busy);
    assert_eq!(
        hints_of(&hints, "Ctrl+Shift+T"),
        ["show terminals"],
        "{hints:?}"
    );
    let (mods, sym) = crate::keys::hint_key("Ctrl+Shift+T").unwrap();
    let pressed = panel.key(&mods, sym, &f, &[], busy, &[], &[], &[]);
    assert_eq!(pressed, Some(PanelOutcome::ShowSandbox));
    // An agent there too: Ctrl+Shift+Q stops the agent, as pressed, and
    // is hinted once.
    f.runs.insert(
        Stage::ManualTest,
        feature::agents::StageRun {
            n: 1,
            agent: "Claude Code".into(),
            pane: Some("m/w1:p1".into()),
            machine: Some("w1".into()),
            ..Default::default()
        },
    );
    let ctx = RowCtx::default();
    let hints = hints_at(&panel, &f, Stage::ManualTest, ctx);
    assert_eq!(
        hints_of(&hints, "Ctrl+Shift+Q"),
        ["stop agent"],
        "{hints:?}"
    );
    let (mods, sym) = crate::keys::hint_key("Ctrl+Shift+Q").unwrap();
    let pressed = panel.key(&mods, sym, &f, &[], ctx, &[], &[], &[]);
    assert_eq!(
        pressed,
        Some(PanelOutcome::StopOnMachine(Stage::ManualTest))
    );
}

#[test]
fn alt_e_is_hinted_only_while_it_folds() {
    let f = history_feature();
    let mut panel = FeaturePanel::default();
    panel.pick(&f, Stage::AiReview);
    let ctx = RowCtx::default();
    assert_eq!(
        hints_of(&hints_at(&panel, &f, Stage::AiReview, ctx), "Alt+E"),
        ["history"]
    );
    // With all repositories shown it does nothing: no hint.
    panel.expanded = true;
    let hints = hints_at(&panel, &f, Stage::AiReview, ctx);
    assert!(hints_of(&hints, "Alt+E").is_empty(), "{hints:?}");
}

#[test]
fn the_review_views_hint_the_key_to_the_other_one() {
    let at = stage_index(Stage::CodeReview);
    let f = feature("c", vec![repo("api", true)], at);
    assert!(crate::feature_diff::review_views(&f));
    let mut panel = FeaturePanel {
        review_view: Some(chrome::ReviewView::Fixes),
        ..FeaturePanel::default()
    };
    let ctx = RowCtx::default();
    let hints = hints_at(&panel, &f, Stage::CodeReview, ctx);
    assert_eq!(hints_of(&hints, "Alt+1"), ["code review"], "{hints:?}");
    assert!(hints_of(&hints, "Alt+2").is_empty(), "{hints:?}");
    panel.review_view = Some(chrome::ReviewView::Code);
    let hints = hints_at(&panel, &f, Stage::CodeReview, ctx);
    assert_eq!(hints_of(&hints, "Alt+2"), ["fixes"], "{hints:?}");
    assert!(hints_of(&hints, "Alt+1").is_empty(), "{hints:?}");
}

#[test]
fn keys_hinted_from_before_the_panel_come_from_their_handlers_rows() {
    // F10 is `feature_panel_key`'s: hinted by its row, pressed by it.
    let f = feature("b", vec![repo("api", true)], 3);
    let hints = hints_at(&FeaturePanel::default(), &f, Stage::Plan, RowCtx::default());
    assert_eq!(hints_of(&hints, MENU_KEY), ["more actions"]);
    let pre = keys::PreCtx {
        feature: true,
        inline: false,
        keys: false,
    };
    for chord in [MENU_KEY, "Shift+F10"] {
        let (mods, sym) = crate::keys::hint_key(chord).unwrap();
        assert_eq!(keys::pre_key(&pre, &mods, sym), Some(keys::PreKey::Menu));
    }
    let (mods, sym) = crate::keys::hint_key("Page_Down").unwrap();
    assert_eq!(
        keys::pre_key(&pre, &mods, sym),
        Some(keys::PreKey::Page(false))
    );
    crate::key_table::check(keys::PRE_KEYS, &[pre]);
}

#[test]
fn keys_taken_with_a_modifier_more_still_work_unhinted() {
    let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
    let mut panel = FeaturePanel {
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let with = |chord: &str| crate::keys::hint_key(chord).unwrap();
    // Shift with the arrows moves as without.
    assert_eq!(press(&mut panel, &f, with("Shift+Down")), None);
    assert_eq!(panel.stage, Some(Stage::ALL[4]));
    assert_eq!(press(&mut panel, &f, with("Shift+Up")), None);
    assert_eq!(panel.stage, Some(Stage::ALL[3]));
    // Enter with a modifier no gate takes is the page's Enter.
    let enter = press(&mut panel, &f, key(Keysym::Return));
    assert_eq!(press(&mut panel, &f, with("Shift+Enter")), enter);
    assert_eq!(press(&mut panel, &f, with("Alt+Enter")), enter);
    assert_eq!(
        press(&mut panel, &f, with("Ctrl+Shift+C")),
        Some(PanelOutcome::CopyPath)
    );
    // No sandbox to open to the network: Ctrl+Shift+N is a new feature.
    assert_eq!(
        press(&mut panel, &f, with("Ctrl+Shift+N")),
        Some(PanelOutcome::New)
    );
    let hints = hints_at(&panel, &f, Stage::Plan, RowCtx::default());
    for alias in ["Shift+Enter", "Alt+Enter", "Ctrl+Shift+C", "Shift+Down"] {
        assert!(hints_of(&hints, alias).is_empty(), "{alias}: {hints:?}");
    }
}

#[test]
fn each_table_of_the_tab_hints_what_it_presses() {
    let slugs = vec!["a".to_owned(), "b".to_owned()];
    let mut tabs: Vec<FeaturePanel> = Vec::new();
    for focus in [TabArea::Stages, TabArea::Repos, TabArea::Findings] {
        for expanded in [false, true] {
            tabs.push(FeaturePanel {
                slug: "b".into(),
                focus,
                expanded,
                ..FeaturePanel::default()
            });
        }
    }
    for at in [
        3,
        stage_index(Stage::Verify),
        stage_index(Stage::ManualTest),
    ] {
        let f = feature("b", vec![repo("api", true), repo("web", false)], at);
        for stage in Stage::ALL {
            let ctxs: Vec<keys::TabCtx> = (tabs.iter())
                .map(|p| {
                    keys::TabCtx::new(
                        p,
                        &f,
                        stage,
                        &slugs,
                        RowCtx::default(),
                        &[],
                        &[],
                        &[],
                        None,
                    )
                })
                .collect();
            for link in keys::chain() {
                if let keys::Link::Rows(_, rows) = link {
                    crate::key_table::check(rows, &ctxs);
                }
            }
        }
    }
}

#[test]
fn a_sandbox_running_elsewhere_can_be_opened_looked_at_and_stopped_from_here() {
    let f = sandbox_elsewhere(true);
    assert_eq!(f.current(), Some(Stage::ManualTest));
    let offered = page_outcomes(&f, Stage::ManualTest);
    for yes in [
        PanelOutcome::OpenSandbox,
        PanelOutcome::ShowSandbox,
        PanelOutcome::StopSandbox,
        PanelOutcome::RestartSandbox,
        PanelOutcome::ResetSandbox,
    ] {
        assert!(offered.contains(&yes), "{offered:?}");
    }
    assert!(
        !offered.contains(&PanelOutcome::StartSandbox),
        "{offered:?}"
    );
    assert!(
        !offered.contains(&PanelOutcome::RemoveSandbox),
        "{offered:?}"
    );
    // Not running there: nothing to show. A new one starts where it is pressed.
    let off = page_outcomes(&sandbox_elsewhere(false), Stage::ManualTest);
    assert!(!off.contains(&PanelOutcome::OpenSandbox), "{off:?}");
    assert!(off.contains(&PanelOutcome::StartSandbox), "{off:?}");
    assert!(off.contains(&PanelOutcome::RemoveSandbox), "{off:?}");
}

#[test]
fn a_sandbox_placed_on_a_worker_shows_its_terminals_from_the_writer_too() {
    let mut f = sandbox_elsewhere(true);
    f.writer = None;
    f.sandbox.as_mut().unwrap().machine = Some("w1".into());
    let offered = page_outcomes(&f, Stage::ManualTest);
    assert!(offered.contains(&PanelOutcome::ShowSandbox), "{offered:?}");
    assert!(offered.contains(&PanelOutcome::StopSandbox), "{offered:?}");
}

#[test]
fn what_is_done_to_a_sandbox_elsewhere_doesnt_move_its_feature() {
    for outcome in [
        PanelOutcome::StopSandbox,
        PanelOutcome::RestartSandbox,
        PanelOutcome::ResetSandbox,
        PanelOutcome::RemoveSandbox,
    ] {
        assert!(outcome.acts_on_sandbox(), "{outcome:?}");
    }
    assert!(!PanelOutcome::StartSandbox.acts_on_sandbox());
}

#[test]
fn a_sandbox_here_has_no_button_for_terminals_elsewhere() {
    let mut f = sandbox_elsewhere(true);
    f.writer = None;
    let offered = page_outcomes(&f, Stage::ManualTest);
    assert!(offered.contains(&PanelOutcome::OpenSandbox), "{offered:?}");
    assert!(!offered.contains(&PanelOutcome::ShowSandbox), "{offered:?}");
    assert!(offered.contains(&PanelOutcome::StopSandbox), "{offered:?}");
}

#[test]
fn the_page_says_where_a_sandbox_elsewhere_runs_and_what_is_open_here() {
    let texts = |f: &Feature, seen: crate::machine_sandbox::SandboxSeen| -> Vec<String> {
        let state = PanelState {
            sandbox_seen: Some(seen),
            ..PanelState::default()
        };
        let sections = sandbox_sections(f, &state);
        format!("{sections:?}")
            .split("text: ")
            .skip(1)
            .map(str::to_owned)
            .collect()
    };
    let seen = crate::machine_sandbox::SandboxSeen {
        name: "box".into(),
        here: vec![5173],
        taken: vec![8000],
    };
    let said = texts(&sandbox_elsewhere(true), seen.clone()).join("\n");
    assert!(said.contains("running on box"), "{said}");
    assert!(!said.contains("stop it there"), "{said}");
    assert!(said.contains("taken here, so not open: 8000"), "{said}");
    assert!(said.contains("open here on the same ports: 5173"), "{said}");
    let closed = crate::machine_sandbox::SandboxSeen {
        name: "box".into(),
        ..Default::default()
    };
    let said = texts(&sandbox_elsewhere(true), closed).join("\n");
    assert!(said.contains("listening on 5173"), "{said}");
    let said = texts(&sandbox_elsewhere(false), seen.clone()).join("\n");
    assert!(said.contains("not running on box"), "{said}");
    // Read from what's kept, with nobody asked yet: that says nothing.
    let mut unknown = sandbox_elsewhere(false);
    unknown.sandbox.as_mut().unwrap().elsewhere = Some(Default::default());
    let said = texts(&unknown, seen.clone()).join("\n");
    assert!(said.contains("not known yet"), "{said}");
    assert!(!said.contains("not running"), "{said}");
    let mut stopped = sandbox_elsewhere(false);
    stopped.sandbox.as_mut().unwrap().stopped = true;
    let said = texts(&stopped, seen.clone()).join("\n");
    assert!(said.contains("stopped on box"), "{said}");
    let mut none = sandbox_elsewhere(false);
    none.sandbox = None;
    let said = texts(&none, seen).join("\n");
    assert!(said.contains("none on box"), "{said}");
}

#[test]
fn only_a_look_counts_as_only_reading() {
    for outcome in [
        PanelOutcome::ShowAgent(Stage::Plan),
        PanelOutcome::ShowCheck(0),
        PanelOutcome::OpenSandbox,
        PanelOutcome::ShowSandbox,
        PanelOutcome::CodeReview,
        PanelOutcome::Evidence,
        PanelOutcome::Show("g".into()),
    ] {
        assert!(outcome.reads_only(), "{outcome:?}");
    }
    for outcome in [
        PanelOutcome::Tick,
        PanelOutcome::Remove,
        PanelOutcome::StartAgent(Stage::Plan, false),
        PanelOutcome::StopChecks,
        PanelOutcome::ApproveDoc(Stage::Plan),
        PanelOutcome::StopSandbox,
        PanelOutcome::RestartSandbox,
        PanelOutcome::New,
        PanelOutcome::Open("/x".into()),
    ] {
        assert!(!outcome.reads_only(), "{outcome:?}");
    }
}

#[test]
fn a_picked_base_branch_goes_with_the_feature() {
    let mut d = with_branches(&["api", "web"], &["main", "dev", "next"]);
    type_into(&mut d, "Bulk");
    dkey(&mut d, alt(Keysym::b));
    // The repository ticked first, opened from the title.
    assert_eq!(d.choosing.as_ref().unwrap().repo.1, "api");
    assert_eq!(d.focus, Some(0));
    assert_eq!(d.branch_rows(), ["main", "dev", "next"]);
    type_into(&mut d, "de");
    assert_eq!(d.branch_rows(), ["dev"]);
    assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
    assert!(d.choosing.is_none());
    // Back as it was: the keyboard, the title, the filter gone.
    assert_eq!(d.focus, None);
    assert_eq!(&*d.title, "Bulk");
    assert!(d.query.is_empty());
    assert_eq!(base_of(&d, "api"), "dev");
    assert_eq!(base_of(&d, "web"), "main");
    let Some(DialogOutcome::Create { root, .. }) = dkey(&mut d, key(Keysym::Return)) else {
        panic!("it should create");
    };
    assert_eq!(root.repos[0].base, "dev");
    assert_eq!(root.repos[1].base, "main");
}

#[test]
fn esc_clears_the_branch_filter_then_keeps_the_base_it_had() {
    let mut d = with_branches(&["api"], &["main", "dev"]);
    dkey(&mut d, alt(Keysym::b));
    type_into(&mut d, "zz");
    assert_eq!(d.repos(), 0);
    // The filter can't move the field: it keeps its room.
    assert_eq!(d.slots(), 2);
    assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
    assert!(d.error.is_some() && d.choosing.is_some());
    dkey(&mut d, key(Keysym::Escape));
    assert!(d.query.is_empty() && d.choosing.is_some());
    assert_eq!(d.focus, Some(0));
    dkey(&mut d, key(Keysym::Escape));
    assert!(d.choosing.is_none());
    assert_eq!(base_of(&d, "api"), "main");
    // Esc only stepped back: the dialog is there.
    assert_eq!(
        dkey(&mut d, key(Keysym::Escape)),
        Some(DialogOutcome::Cancel)
    );
}

#[test]
fn origins_default_moves_a_base_nobody_picked() {
    let mut d = with_branches(&["api", "web", "app"], &["main", "dev"]);
    let (api, web, app) = (
        (None, "api".to_owned()),
        (None, "web".to_owned()),
        (None, "app".to_owned()),
    );
    d.remote_base(&api, "dev".into());
    assert_eq!(base_of(&d, "api"), "dev");
    // The default leads the list, and says so in the view.
    d.focus = Some(0);
    d.open_bases(0);
    assert_eq!(d.branch_rows(), ["dev", "main"]);
    let view = new_feature_view(&d, "Acme".into());
    assert_eq!(view.repos[0].place, "default");
    assert!(view.repos[0].touched);
    d.close_bases();
    // Picked by hand: origin's say comes too late.
    d.focus = Some(1);
    d.open_bases(1);
    d.pick_base(0);
    d.remote_base(&web, "dev".into());
    assert_eq!(base_of(&d, "web"), "main");
    // Never fetched: a worktree couldn't start from it.
    d.remote_base(&app, "gone".into());
    assert_eq!(base_of(&d, "app"), "main");
}

#[test]
fn the_only_repository_can_choose_its_base_too() {
    let mut d = with_branches(&["api"], &["main", "dev"]);
    let view = new_feature_view(&d, "Acme".into());
    assert_eq!(view.repos[0].base.as_deref(), Some("from main"));
    assert_eq!(view.repos[0].place, "the only repository");
    assert!(view.keys.contains(&("Alt+B", "base branch")));
    dkey(&mut d, alt(Keysym::b));
    dkey(&mut d, key(Keysym::Down));
    dkey(&mut d, key(Keysym::Return));
    assert_eq!(base_of(&d, "api"), "dev");
}

#[test]
fn only_linked_projects_have_their_branches_looked_for() {
    let mut d = with_branches(&["api"], &[]);
    assert!(d.unlooked().is_empty(), "once each");
    d.others.push(Other {
        project: "billing".into(),
        name: "Billing".into(),
        root: Finding::Found(root(&["ledger"])),
        linked: false,
        touched: vec![true],
    });
    assert!(d.unlooked().is_empty());
    d.others[0].linked = true;
    let looked = d.unlooked();
    assert_eq!(
        looked[0].0,
        (Some("billing".to_owned()), "ledger".to_owned())
    );
    // A linked repository's base goes with its link.
    let key = looked[0].0.clone();
    d.base_branches(&key, Ok(vec!["main".into(), "dev".into()]));
    let at = d
        .rows()
        .iter()
        .position(|r| matches!(r, DialogRow::Linked(..)));
    d.open_bases(at.unwrap());
    d.pick_base(1);
    assert_eq!(d.links()[0].root.repos[0].base, "dev");
}

#[test]
fn a_project_row_opens_no_base_list() {
    let mut d = with_branches(&["api"], &["main"]);
    d.others.push(Other {
        project: "billing".into(),
        name: "Billing".into(),
        root: Finding::Failed("nope".into()),
        linked: false,
        touched: Vec::new(),
    });
    d.pick();
    d.open_bases(0);
    assert!(d.choosing.is_none() && d.picking);
}

#[test]
fn esc_goes_back_to_the_title_then_clears_it_then_closes() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.found(Ok(root(&["api", "web"])));
    type_into(&mut d, "Bulk");
    assert_eq!(&*d.title, "Bulk");
    dkey(&mut d, key(Keysym::Tab));
    assert_eq!(d.focus, Some(0));
    assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
    assert_eq!(d.focus, None);
    assert_eq!(dkey(&mut d, key(Keysym::Escape)), None);
    assert!(d.title.is_empty());
    assert_eq!(
        dkey(&mut d, key(Keysym::Escape)),
        Some(DialogOutcome::Cancel)
    );
}

#[test]
fn enter_creates_with_the_ticked_repositories_or_says_what_is_missing() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api/src".into());
    assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
    assert_eq!(d.error.as_deref(), Some("Give the feature a title."));
    type_into(&mut d, "Bulk export");
    // Typing clears the error; still looking, Enter says so.
    dkey(&mut d, key(Keysym::Return));
    assert!(
        d.error
            .as_deref()
            .is_some_and(|e| e.starts_with("Still finding"))
    );
    d.found(Ok(root(&["api", "web"])));
    // The repository it was opened from is ticked.
    assert_eq!(d.touched, [true, false]);
    // Space ticks the focused one; in the title it types.
    dkey(&mut d, key(Keysym::Down));
    dkey(&mut d, key(Keysym::Down));
    assert_eq!(d.focus, Some(1));
    dkey(&mut d, key(Keysym::space));
    dkey(&mut d, ctrl(Keysym::k));
    dkey(&mut d, key(Keysym::space));
    assert_eq!(d.touched, [false, true]);
    let out = dkey(&mut d, key(Keysym::Return));
    assert_eq!(
        out,
        Some(DialogOutcome::Create {
            title: "Bulk export".into(),
            root: root(&["api", "web"]),
            touched: vec!["web".into()],
            links: vec![],
        })
    );
    dkey(&mut d, key(Keysym::Down));
    dkey(&mut d, key(Keysym::space));
    assert_eq!(d.touched, [false, false]);
    assert_eq!(dkey(&mut d, key(Keysym::Return)), None);
    assert!(
        d.error
            .as_deref()
            .is_some_and(|e| e.starts_with("Tick at least"))
    );
    // Shift+Tab from the title goes round to the last repository.
    d.focus = None;
    let shift = ModifiersState {
        shift: true,
        ..ModifiersState::default()
    };
    dkey(&mut d, (shift, Keysym::ISO_Left_Tab));
    assert_eq!(d.focus, Some(1));
}

#[test]
fn a_single_repository_is_ticked_and_fixed() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/elsewhere".into());
    d.found(Ok(root(&["api"])));
    assert_eq!(d.touched, [true]);
    dkey(&mut d, key(Keysym::Tab));
    assert_eq!(d.focus, None, "nothing to move to");
    d.toggle(0);
    assert_eq!(d.touched, [true]);
    let view = new_feature_view(&d, "Acme".into());
    assert!(view.repos[0].fixed);
    assert_eq!(
        view.keys,
        [
            ("Alt+B", "base branch"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "close")
        ]
    );
}

#[test]
fn a_linked_projects_repositories_join_the_new_feature() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.found(Ok(root(&["api"])));
    type_into(&mut d, "Bulk");
    dkey(&mut d, ctrl_shift(Keysym::L));
    assert!(d.picking);
    d.others = vec![other("admin", "Admin"), other("docs", "Docs")];
    d.found_other("admin", Ok(root(&["panel", "jobs"])));
    d.found_other("docs", Err("no git repository".into()));
    let view = new_feature_view(&d, "Acme".into());
    assert_eq!(view.link, chrome::LINK_BUTTON[1]);
    let places: Vec<&str> = view.repos.iter().map(|r| r.place.as_str()).collect();
    let found = format!(
        "In {} \u{b7} 2 repositories",
        short_folder(Path::new("/work"))
    );
    assert_eq!(places, [found.as_str(), "can't link: no git repository"]);
    assert!(view.repos[1].fixed, "a project not found can't be picked");
    d.toggle(1);
    assert!(!d.others[1].linked);
    dkey(&mut d, key(Keysym::space));
    assert!(d.others[0].linked);
    // Out of the picker: the linked project's repositories follow the
    // home one's, labelled by their project.
    dkey(&mut d, key(Keysym::Escape));
    assert!(!d.picking);
    let view = new_feature_view(&d, "Acme".into());
    let names: Vec<&str> = view.repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["api", "Admin: panel", "Admin: jobs"]);
    assert_eq!(view.projects, "Projects: Acme (home) \u{b7} Admin");
    d.toggle(2);
    let Some(DialogOutcome::Create { touched, links, .. }) = dkey(&mut d, key(Keysym::Return))
    else {
        panic!("it creates");
    };
    assert_eq!(touched, ["api"]);
    assert_eq!(links.len(), 1);
    assert_eq!(
        (links[0].project.as_str(), &links[0].touched[..]),
        ("admin", &["jobs".to_owned()][..])
    );
}

#[test]
fn a_linked_project_lists_the_open_features_spanning_it() {
    let link = |project: &str| feature::Link {
        project: project.into(),
        root: PathBuf::from(format!("/{project}")),
        dir: PathBuf::from(format!("/{project}/.features/b")),
        multi: false,
    };
    let mut spanning = feature("b", vec![repo("api", true)], 1);
    spanning.links = vec![link("admin")];
    let mut done = feature("c", vec![repo("api", true)], 1);
    done.links = vec![link("admin")];
    done.status = Status::Done;
    let alone = feature("d", vec![repo("api", true)], 1);
    let all = BTreeMap::from([
        ("acme".to_owned(), vec![spanning, done, alone]),
        (
            "admin".to_owned(),
            vec![feature("e", vec![repo("panel", true)], 1)],
        ),
    ]);
    let listed: Vec<(&str, &str)> = linked_in(&all, "admin")
        .into_iter()
        .map(|(home, f)| (home, f.slug.as_str()))
        .collect();
    assert_eq!(listed, [("acme", "b")]);
    assert!(linked_in(&all, "acme").is_empty(), "nothing spans its home");
}

#[test]
fn a_feature_needs_one_changed_repository_in_any_project() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/elsewhere".into());
    d.found(Ok(root(&["api", "web"])));
    type_into(&mut d, "Bulk");
    assert_eq!(d.touched, [false, false]);
    d.others = vec![other("admin", "Admin")];
    d.found_other("admin", Ok(root(&["panel", "tools"])));
    d.others[0].linked = true;
    assert!(d.create().is_err());
    d.toggle(2);
    let Ok(DialogOutcome::Create { touched, links, .. }) = d.create() else {
        panic!("a linked project's repository is enough");
    };
    assert!(touched.is_empty());
    assert_eq!(links[0].touched, ["panel"]);
}

#[test]
fn a_project_of_one_repository_starts_ticked() {
    let mut d = NewFeatureDialog::new("acme".into(), 1, "/work/api".into());
    d.title.set("Bulk export");
    d.others = vec![other("ops", "Ops")];
    d.found(Ok(root(&["api"])));
    d.found_other("ops", Ok(root(&["infra"])));
    assert_eq!(d.others[0].touched, [true]);
    d.others[0].linked = true;
    let Ok(DialogOutcome::Create { links, .. }) = d.create() else {
        panic!("it creates");
    };
    assert_eq!(links[0].touched, ["infra"]);
}

#[test]
fn dropping_a_repository_drops_its_link_with_the_last_one() {
    let linked = |name: &str| feature::Repo {
        project: Some("admin".into()),
        ..repo(name, true)
    };
    let mut f = feature(
        "b",
        vec![repo("api", true), linked("panel"), linked("x")],
        1,
    );
    f.links = vec![feature::Link {
        project: "admin".into(),
        root: "/admin".into(),
        dir: "/admin/.features/b".into(),
        multi: true,
    }];
    drop_repo(&mut f, "admin:panel", 7);
    assert_eq!(f.links.len(), 1);
    drop_repo(&mut f, "admin:x", 8);
    assert!(f.links.is_empty());
    let keys: Vec<String> = f.repos.iter().map(|r| r.key()).collect();
    assert_eq!(keys, ["api"]);
    assert_eq!(
        remove_repos_body(1, &["admin:panel: 2 changed files".into()], false),
        "Its worktree and branch go, and with them admin:panel: 2 changed files.\n\
             You can add it back, on a new branch."
    );
    assert_eq!(
        remove_repos_body(1, &[], true),
        "Its worktree and branch go; nothing in them is lost.\n\
             Agents started from now on read its main checkout."
    );
}

#[test]
fn del_on_a_repository_of_several_asks_or_arms_by_what_it_holds() {
    let f = feature("b", vec![repo("api", true), repo("web", true)], 3);
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        focus: TabArea::Repos,
        repo: 1,
        ..FeaturePanel::default()
    };
    // One on the feature's branch: the question guards it, not arming.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::RemoveRepo("web".into()))
    );
    assert_eq!(panel.armed, None);
    // One the feature doesn't change arms first.
    let old = feature("c", vec![repo("api", true), repo("ui", false)], 3);
    assert_eq!(press(&mut panel, &old, key(Keysym::Delete)), None);
    assert_eq!(panel.armed.as_deref(), Some("ui"));
    assert_eq!(
        press(&mut panel, &old, key(Keysym::Delete)),
        Some(PanelOutcome::RemoveRepo("ui".into()))
    );
    // Any other key lets it go.
    press(&mut panel, &old, key(Keysym::Delete));
    press(&mut panel, &old, key(Keysym::End));
    assert_eq!(panel.armed, None);
    // On the stages Del does nothing: Alt+Shift+Del abandons, asking.
    panel.focus = TabArea::Stages;
    assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
    assert_eq!(
        press(&mut panel, &f, alt_shift(Keysym::Delete)),
        Some(PanelOutcome::Remove)
    );
    // Nor on a feature's only repository.
    let mut one = feature("c", vec![repo("api", true)], 3);
    one.multi = false;
    panel.focus = TabArea::Repos;
    panel.repo = 0;
    assert_eq!(press(&mut panel, &one, key(Keysym::Delete)), None);
}

#[test]
fn a_repository_that_can_leave_shows_its_button() {
    let mut f = feature("b", vec![repo("api", true), repo("web", true)], 3);
    assert!(repo_removable(&f, 0) && repo_removable(&f, 1));
    f.repos[1].touched = false;
    assert!(!repo_removable(&f, 0), "the last one to change stays");
    assert!(repo_removable(&f, 1), "a read-only one can go");
    f.repos[1].touched = true;
    f.shipping.prs.push(feature::Pr {
        repo: "web".into(),
        url: "https://x/pull/1".into(),
    });
    assert!(!repo_removable(&f, 1), "a pull request holds it");
    f.status = Status::Done;
    assert!(!repo_removable(&f, 0), "shipped");
}

#[test]
fn panel_keys_move_through_stages_and_repositories_and_ask_for_actions() {
    let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    // It starts on the current stage's page: Plan review done, Build next,
    // its agent to start.
    assert_eq!(panel.focus, TabArea::Stages);
    assert_eq!(panel.page_stage(&f), Stage::Build);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::StartAgent(Stage::Build, false))
    );
    // Left and Right (Up, Down, Ctrl+J/K) pick another stage's page.
    assert_eq!(press(&mut panel, &f, key(Keysym::Left)), None);
    assert_eq!(panel.page_stage(&f), Stage::PlanReview);
    // A done stage's Enter goes on to the current one; only its own
    // key reopens it.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::GoToCurrent)
    );
    let actions = page_actions(&f, Stage::PlanReview, RowCtx::default());
    assert_eq!(actions[1].1, PanelOutcome::Untick(Stage::PlanReview));
    assert_eq!(actions[1].0.style, ButtonStyle::Plain);
    press(&mut panel, &f, ctrl(Keysym::j));
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(panel.page_stage(&f), Stage::Verify);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::NotYet(Stage::Build))
    );
    press(&mut panel, &f, ctrl(Keysym::k));
    press(&mut panel, &f, key(Keysym::Up));
    assert_eq!(panel.page_stage(&f), Stage::PlanReview);
    press(&mut panel, &f, key(Keysym::Home));
    assert_eq!(panel.page_stage(&f), Stage::Discover);
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(
        panel.page_stage(&f),
        Stage::Discover,
        "it stops at the ends"
    );
    press(&mut panel, &f, key(Keysym::End));
    assert_eq!(panel.page_stage(&f), Stage::Ship);
    // Tab moves to the repositories, where the same keys move.
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Repos);
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(panel.repo, 1);
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(panel.repo, 0);
    assert_eq!(panel.page_stage(&f), Stage::Ship, "the page stays");
    // Enter on a chip unfolds the rows, where it does the row's thing.
    assert_eq!(press(&mut panel, &f, key(Keysym::Return)), None);
    assert!(panel.expanded);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::Open("/work/.features/bulk/api".into()))
    );
    press(&mut panel, &f, key(Keysym::Down));
    assert_eq!(panel.repo, 1);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::Touch("web".into()))
    );
    press(&mut panel, &f, key(Keysym::Down));
    assert_eq!(panel.repo, 1, "it stops at the last");
    press(&mut panel, &f, key(Keysym::Home));
    assert_eq!(panel.repo, 0);
    // Shift+Tab goes back too.
    press(&mut panel, &f, key(Keysym::ISO_Left_Tab));
    assert_eq!(panel.focus, TabArea::Stages);
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::o)),
        Some(PanelOutcome::Open("/work/.features/b".into()))
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::c)),
        Some(PanelOutcome::CopyPath)
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::n)),
        Some(PanelOutcome::New)
    );
    assert_eq!(press(&mut panel, &f, ctrl_shift(Keysym::L)), None);
    assert!(!panel.expanded, "the chord folds what Enter unfolded");
    let mut shipped = f.clone();
    shipped
        .marks
        .insert(Stage::Ship, f.marks.values().next().unwrap().clone());
    assert!(
        repos_offered(&shipped, &[]).is_empty(),
        "a shipped feature takes no more repositories"
    );
    let focus = std::mem::replace(&mut panel.focus, TabArea::Stages);
    assert_eq!(
        press(&mut panel, &f, alt_shift(Keysym::Delete)),
        Some(PanelOutcome::Remove)
    );
    panel.focus = focus;
    // Ctrl+PgUp and Ctrl+PgDn go to the other features, from either.
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::Prior)),
        Some(PanelOutcome::Show("a".into()))
    );
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::Next)),
        Some(PanelOutcome::Show("c".into()))
    );
    // Plain PgUp, Esc and Ctrl+P leave it be: it is a tab.
    let at = (panel.focus, panel.page_stage(&f), panel.repo);
    assert_eq!(press(&mut panel, &f, key(Keysym::Prior)), None);
    assert_eq!(press(&mut panel, &f, key(Keysym::Escape)), None);
    assert_eq!(press(&mut panel, &f, ctrl(Keysym::p)), None);
    assert_eq!((panel.focus, panel.page_stage(&f), panel.repo), at);
}

#[test]
fn without_repositories_tab_stays_on_the_stages() {
    let f = feature("b", Vec::new(), 3);
    let mut panel = FeaturePanel::default();
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Stages);
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(panel.page_stage(&f), Stage::Verify);
}

#[test]
fn another_page_starts_at_its_top() {
    let f = feature("b", vec![repo("api", true)], 3);
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let page = TabColumn::Page as usize;
    let slugs = vec!["b".to_owned()];
    let state = PanelState::default();
    // Build's page, scrolled down (PgDn to its end).
    panel.hold_page(panel.page_key(&f));
    panel.scroll[page].jump(300.0);
    assert_eq!(
        tab_view(&panel, &f, &slugs, &state, 2_060).scroll[page],
        300.0
    );
    // Right: Verify's page shows from its top, at once and held.
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(
        tab_view(&panel, &f, &slugs, &state, 2_060).scroll[page],
        0.0
    );
    panel.hold_page(panel.page_key(&f));
    assert_eq!(panel.scroll[page].pos(), 0.0);
    // Back on Build, scrolled again: the feature moving on (the reset
    // rule) shows the current stage's page from its top too.
    press(&mut panel, &f, key(Keysym::Left));
    panel.hold_page(panel.page_key(&f));
    panel.scroll[page].jump(200.0);
    press(&mut panel, &f, key(Keysym::Home));
    panel.hold_page(panel.page_key(&f));
    assert_eq!(panel.page_stage(&f), Stage::Discover);
    panel.scroll[page].jump(120.0);
    let moved_on = feature("b", vec![repo("api", true)], 4);
    assert_eq!(panel.page_stage(&moved_on), Stage::Verify);
    assert_eq!(
        tab_view(&panel, &moved_on, &slugs, &state, 2_060).scroll[page],
        0.0
    );
    // The other columns keep theirs.
    panel.scroll[TabColumn::Repos as usize].jump(40.0);
    panel.hold_page(panel.page_key(&moved_on));
    assert_eq!(panel.scroll[TabColumn::Repos as usize].pos(), 40.0);
    assert_eq!(panel.scroll[page].pos(), 0.0);
}

#[test]
fn f1_lists_every_key_in_place_of_the_page() {
    let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let three = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let state = PanelState::default();
    // Every hint the footer offers, either focus, is in the list (K3).
    let mut offered: Vec<String> = Vec::new();
    for focus in [TabArea::Stages, TabArea::Repos] {
        panel.focus = focus;
        for stage in Stage::ALL {
            panel.pick(&f, stage);
            let view = tab_view(&panel, &f, &three, &state, 2_060);
            offered.extend(view.hints.into_iter().map(|(k, _)| k));
        }
    }
    panel.focus = TabArea::Stages;
    panel.unpick();
    assert_eq!(press(&mut panel, &f, key(Keysym::F1)), None);
    assert!(panel.keys);
    let view = tab_view(&panel, &f, &three, &state, 2_060);
    assert_eq!(view.page.title, "Keys");
    assert!(view.page.buttons.is_empty());
    let listed: Vec<String> = (view.page.sections.iter())
        .flat_map(|s| match s {
            PageSection::Lines { lines, .. } => lines.iter().map(|l| l.text.clone()).collect(),
            _ => Vec::new(),
        })
        .collect();
    // Each key offered, alone or among others ("Tab" in "Tab /
    // Shift+Tab").
    let listed_keys: Vec<&str> = (listed.iter())
        .filter_map(|l| l.split_once('\t').map(|(k, _)| k))
        .flat_map(|k| k.split(" / "))
        .collect();
    for keys in &offered {
        for k in keys.split(" / ") {
            assert!(listed_keys.contains(&k), "{k} listed");
        }
    }
    let hints: Vec<&str> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(hints, ["Esc", "F1"]);
    // Any key puts the list away and does nothing else.
    assert_eq!(press(&mut panel, &f, key(Keysym::Return)), None);
    assert!(!panel.keys);
    assert_eq!(panel.page_stage(&f), Stage::Build);
    press(&mut panel, &f, key(Keysym::F1));
    press(&mut panel, &f, key(Keysym::F1));
    assert!(!panel.keys);
    press(&mut panel, &f, key(Keysym::F1));
    press(&mut panel, &f, key(Keysym::Escape));
    assert!(!panel.keys);
}

fn sources_with(found: &[&str]) -> Vec<RepoSource> {
    vec![RepoSource {
        project: None,
        name: "acme".into(),
        finding: Finding::Found(root(found)),
    }]
}

fn context_of(name: &str) -> feature::ContextRepo {
    feature::ContextRepo {
        name: name.into(),
        main: PathBuf::from(format!("/work/{name}")),
        project: None,
    }
}

/// A multi-repository feature with `api` and `web` on its branch,
/// `docs` as context and `auth` taken out.
fn every_kind_of_row() -> Feature {
    let mut f = feature("b", vec![repo("api", true), repo("web", true)], 3);
    f.add_context(context_of("docs"));
    f.note_removed(None, "auth".into(), 5);
    f
}

#[test]
fn leaving_an_active_repository_always_asks() {
    let mut f = feature("b", vec![repo("api", true), repo("ui", false)], 3);
    let keys = |k: &str| vec![k.to_owned()];
    assert!(
        removal_asks(&f, &keys("api"), &[]),
        "nothing lost, still asks"
    );
    assert!(!removal_asks(&f, &keys("ui"), &[]), "not on the branch");
    assert!(removal_asks(
        &f,
        &keys("ui"),
        &["ui: 1 changed file".into()]
    ));
    f.repos[0].touched = false;
    assert!(!removal_asks(&f, &keys("api"), &[]));
}

#[test]
fn rows_run_held_context_removed_then_not_added() {
    let f = every_kind_of_row();
    let sources = sources_with(&["api", "auth", "docs", "infra", "web"]);
    assert_eq!(
        repo_entries(&f, &sources, true),
        [
            RepoEntry::Held(0),
            RepoEntry::Held(1),
            RepoEntry::Context(0),
            RepoEntry::Removed(0),
            RepoEntry::Offered { source: 0, repo: 3 },
        ]
    );
    // Collapsed lists only what is involved.
    assert_eq!(
        repo_entries(&f, &sources, false),
        [
            RepoEntry::Held(0),
            RepoEntry::Held(1),
            RepoEntry::Context(0)
        ]
    );
    assert_eq!(repos_offered(&f, &sources).len(), 2);
    // A shipped feature's repositories can't change.
    let mut done = f.clone();
    done.status = Status::Done;
    assert!(repos_offered(&done, &sources).is_empty());
}

#[test]
fn each_kind_of_row_has_its_buttons() {
    let f = every_kind_of_row();
    let sources = sources_with(&["api", "auth", "docs", "infra", "web"]);
    let of = |entry| -> Vec<PanelOutcome> {
        entry_buttons(&f, &sources, entry)
            .into_iter()
            .map(|(_, then)| then)
            .collect()
    };
    assert_eq!(
        of(RepoEntry::Held(0)),
        [PanelOutcome::MakeContext("api".into())]
    );
    assert_eq!(
        of(RepoEntry::Context(0)),
        [PanelOutcome::AddToFeature("docs".into())]
    );
    assert_eq!(
        of(RepoEntry::Removed(0)),
        [
            PanelOutcome::AddToFeature("auth".into()),
            PanelOutcome::AddContext("auth".into())
        ]
    );
    assert_eq!(
        of(RepoEntry::Offered { source: 0, repo: 3 }),
        [
            PanelOutcome::AddToFeature("infra".into()),
            PanelOutcome::AddContext("infra".into())
        ]
    );
    // A removed one whose folder went can't come back.
    let gone = sources_with(&["api", "docs", "web"]);
    assert!(entry_buttons(&f, &gone, RepoEntry::Removed(0)).is_empty());
}

#[test]
fn del_goes_by_the_kind_of_row() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel {
        focus: TabArea::Repos,
        expanded: true,
        sources: sources_with(&["api", "auth", "docs", "infra", "web"]),
        ..FeaturePanel::default()
    };
    // Context arms, then goes without a question.
    panel.repo = 2;
    assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
    assert_eq!(panel.armed.as_deref(), Some("docs"));
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        Some(PanelOutcome::DropContext("docs".into()))
    );
    // Rows it doesn't hold have nothing to take out, least of all it.
    for row in [3, 4] {
        panel.repo = row;
        assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
        assert_eq!(panel.armed, None);
    }
}

#[test]
fn esc_folds_the_rows_and_gives_the_keyboard_back() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel::default();
    assert_eq!(panel.focus, TabArea::Stages);
    assert_eq!(press(&mut panel, &f, ctrl_shift(Keysym::L)), None);
    assert!(panel.expanded);
    assert_eq!(panel.focus, TabArea::Repos, "the rows have the keyboard");
    assert_eq!(press(&mut panel, &f, key(Keysym::Escape)), None);
    assert!(!panel.expanded);
    assert_eq!(panel.focus, TabArea::Stages, "where it was");
    // The chord folds them too.
    press(&mut panel, &f, ctrl_shift(Keysym::L));
    press(&mut panel, &f, ctrl_shift(Keysym::L));
    assert!(!panel.expanded);
    assert_eq!(panel.focus, TabArea::Stages);
}

#[test]
fn left_and_right_pick_a_rows_button_for_enter() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel {
        focus: TabArea::Repos,
        expanded: true,
        sources: sources_with(&["api", "auth", "docs", "infra", "web"]),
        ..FeaturePanel::default()
    };
    // A row not held has nothing but its buttons: the first is picked.
    panel.repo = 3;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::AddToFeature("auth".into()))
    );
    press(&mut panel, &f, key(Keysym::Right));
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(panel.repo, 3, "the sides don't leave the row");
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::AddContext("auth".into()))
    );
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::AddToFeature("auth".into()))
    );
    // Another row starts over.
    press(&mut panel, &f, key(Keysym::Right));
    press(&mut panel, &f, key(Keysym::Up));
    assert_eq!(panel.repo_pick, None);
    // One the feature holds opens until a button is picked, and back.
    panel.repo = 0;
    let opens = Some(PanelOutcome::Open(f.repos[0].worktree.clone()));
    assert_eq!(press(&mut panel, &f, key(Keysym::Return)), opens);
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::MakeContext("api".into()))
    );
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(press(&mut panel, &f, key(Keysym::Return)), opens);
}

#[test]
fn rows_arriving_keep_the_selected_one() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel {
        focus: TabArea::Stages,
        expanded: true,
        repo: 3,
        ..FeaturePanel::default()
    };
    panel.sources = sources_with(&["auth"]);
    assert_eq!(
        entry_key(
            &f,
            &panel.sources,
            repo_entries(&f, &panel.sources, true)[3]
        )
        .as_deref(),
        Some("auth")
    );
    panel.select_repo_keeping(&f, "auth", Some(1));
    assert_eq!(panel.repo, 3);
    assert_eq!(panel.focus, TabArea::Stages, "the keyboard stays");
    assert_eq!(panel.repo_pick, Some(1));
}

#[test]
fn rows_changing_forget_what_the_pointer_was_over() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel {
        expanded: true,
        tab_hover: Some(FeatureTabHit::RepoRemove(1)),
        ..FeaturePanel::default()
    };
    panel.select_repo(&f, "api");
    assert_eq!(panel.tab_hover, None);
    panel.tab_hover = Some(FeatureTabHit::Repo(1));
    panel.toggle_repos();
    assert_eq!(panel.tab_hover, None);
}

#[test]
fn del_says_remove_only_where_it_removes() {
    let del = |f: &Feature, repo: usize| {
        let panel = FeaturePanel {
            focus: TabArea::Repos,
            repo,
            ..FeaturePanel::default()
        };
        let view = tab_view(&panel, f, &["b".to_owned()], &PanelState::default(), 2_060);
        (view.hints.iter())
            .find(|(key, _)| key == "Del")
            .map(|(_, what)| what.clone())
    };
    let two = feature("b", vec![repo("api", true), repo("web", true)], 3);
    assert_eq!(del(&two, 0).as_deref(), Some("remove repository"));
    // The last one of several left: Del would only be refused.
    let mut last = two.clone();
    last.repos.pop();
    assert_eq!(del(&last, 0), None);
    // The only one a feature has: Del never abandons it (Alt+Shift+Del
    // does, asking), so it offers nothing there.
    let one = feature("b", vec![repo("api", true)], 3);
    assert_eq!(del(&one, 0), None);
    let mut panel = FeaturePanel {
        focus: TabArea::Repos,
        ..FeaturePanel::default()
    };
    let ctx = RowCtx::default();
    let bare = ModifiersState::default();
    let delete = |p: &mut FeaturePanel, mods: &ModifiersState| {
        p.key(mods, Keysym::Delete, &one, &[], ctx, &[], &[], &[])
    };
    assert_eq!(delete(&mut panel, &bare), None);
    panel.focus = TabArea::Stages;
    assert_eq!(delete(&mut panel, &bare), None);
    let alt_shift = ModifiersState {
        alt: true,
        shift: true,
        ..ModifiersState::default()
    };
    assert_eq!(delete(&mut panel, &alt_shift), Some(PanelOutcome::Remove));
}

#[test]
fn only_the_project_of_the_feature_is_offered() {
    let mut f = every_kind_of_row();
    f.note_removed(Some("admin".into()), "panel".into(), 6);
    f.note_removed(Some("tools".into()), "cli".into(), 7);
    f.links.push(feature::Link {
        project: "admin".into(),
        root: "/admin".into(),
        dir: "/admin/.features/b".into(),
        multi: true,
    });
    let mut sources = sources_with(&["api", "auth", "docs", "infra", "web"]);
    sources.push(RepoSource {
        project: Some("admin".into()),
        name: "Admin".into(),
        finding: sources_with(&["panel", "users"]).remove(0).finding,
    });
    let keys: Vec<String> = (repos_offered(&f, &sources).into_iter())
        .filter_map(|e| entry_key(&f, &sources, e))
        .collect();
    // Not a project it doesn't link, nor the rest of one it does; taken
    // out of one it links, it can come back.
    assert_eq!(keys, ["admin:panel", "auth", "infra"]);
    let back = RepoEntry::Removed(1);
    assert_eq!(
        entry_key(&f, &sources, back).as_deref(),
        Some("admin:panel")
    );
    assert_eq!(entry_buttons(&f, &sources, back).len(), 2);
}

#[test]
fn the_unfolded_rows_say_their_groups_and_what_failed() {
    let f = every_kind_of_row();
    let mut panel = FeaturePanel {
        expanded: true,
        sources: sources_with(&["api", "auth", "docs", "infra", "web"]),
        ..FeaturePanel::default()
    };
    panel.sources.push(RepoSource {
        project: Some("tools".into()),
        name: "tools".into(),
        finding: Finding::Failed("no such folder".into()),
    });
    let slugs = vec!["b".to_owned()];
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
    let groups: Vec<&str> = (view.repos.iter())
        .filter_map(|r| r.group.as_deref())
        .collect();
    assert_eq!(
        groups,
        ["In feature", "Context, read only", "Removed", "Not added"]
    );
    assert_eq!(view.more, 2);
    assert_eq!(view.repo_notes.len(), 1);
    assert!(
        view.repo_notes[0].error,
        "a failed read isn't an empty group"
    );
    assert_eq!(view.repo_notes[0].caption, "tools");
    // Collapsed lists only those involved, and still knows how many more.
    panel.expanded = false;
    let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
    assert_eq!(view.repos.len(), 3);
    assert_eq!(view.more, 2);
}

#[test]
fn an_agent_card_shows_what_it_said_not_how_it_was_launched() {
    let lines: Vec<String> = [
        "[ana@box]/work/shop% cd /work/shop && claude --add-dir /run/x --session-id \"$(tee /run/x/prompts/discover-1.session",
        "fake claude ready",
        "",
        "Which totals should it keep?",
        "[ana@box]/work/shop%",
    ]
    .map(str::to_owned)
    .to_vec();
    assert_eq!(plain_tail(&lines, false), ["Which totals should it keep?"]);
    assert_eq!(
        plain_tail(&lines, true),
        ["fake claude ready", "Which totals should it keep?"]
    );
    assert!(plain_tail(&[], false).is_empty());
}

#[test]
fn header_buttons_and_the_strip_wheel() {
    // The wheel over the chips steps the repositories, stopping at
    // the ends.
    assert_eq!(strip_step(0, 1, 3), 1);
    assert_eq!(strip_step(2, 1, 3), 2);
    assert_eq!(strip_step(1, -3, 3), 0);
    assert_eq!(strip_step(5, 0, 3), 2);
}

#[test]
fn a_picked_page_holds_until_the_feature_moves_on() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let mut panel = FeaturePanel::default();
    press(&mut panel, &f, key(Keysym::Left));
    assert_eq!(panel.page_stage(&f), Stage::PlanReview);
    // Build ticked: the pick is dropped, the new current shows.
    let mark = f.marks[&Stage::Plan].clone();
    f.marks.insert(Stage::Build, mark);
    assert_eq!(panel.page_stage(&f), Stage::Verify);
    // The keys drop it too, and move on from the current one.
    press(&mut panel, &f, key(Keysym::Right));
    assert_eq!(panel.page_stage(&f), Stage::AiReview);
    assert_eq!(panel.picked_at, Some(Stage::Verify));
    // Sent back to Build: the current stage shows again.
    f.marks.remove(&Stage::Build);
    assert_eq!(panel.page_stage(&f), Stage::Build);
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::StartAgent(Stage::Build, false))
    );
    assert_eq!(panel.stage, None);
    // Picking the page the feature is at holds as any other.
    panel.pick(&f, Stage::Discover);
    assert_eq!(panel.page_stage(&f), Stage::Discover);
    panel.unpick();
    assert_eq!(panel.page_stage(&f), Stage::Build);
    // A feature with every stage done shows Ship.
    let done = feature("d", vec![repo("api", true)], 9);
    assert_eq!(FeaturePanel::default().page_stage(&done), Stage::Ship);
}

#[test]
fn the_tab_shows_repositories_the_rail_and_the_right_hints() {
    let f = feature("b", vec![repo("api", true), repo("web", false)], 4);
    let panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let slugs = vec!["b".to_owned()];
    let state = PanelState {
        busy: &["web"],
        ..PanelState::default()
    };
    let view = tab_view(&panel, &f, &slugs, &state, 2_060);
    assert_eq!(view.repos[0].line, "feature/bulk");
    assert_eq!(view.repos[1].line, "context, at main");
    assert!(view.repos[1].busy);
    assert_eq!(view.repos_error, None);
    let marks: Vec<NodeMark> = view.rail.iter().map(|s| s.mark).collect();
    assert_eq!(
        &marks[..6],
        [
            NodeMark::Done,
            NodeMark::Done,
            NodeMark::Done,
            NodeMark::Done,
            NodeMark::Current,
            NodeMark::Later
        ]
    );
    assert_eq!(view.rail.len(), 9);
    assert_eq!(view.rail[0].line, "1m ago");
    assert_eq!(view.rail[4].line, "mark by hand");
    assert_eq!(view.rail[5].line, "");
    let selected: Vec<bool> = view.rail.iter().map(|n| n.selected).collect();
    assert_eq!(selected.iter().filter(|s| **s).count(), 1);
    assert!(selected[4]);
    assert_eq!(view.page.title, "Verify");
    assert_eq!(
        stage_detail(&f, Stage::Discover, &state, 2_060),
        "done 1m ago by you"
    );
    assert_eq!(
        stage_detail(&f, Stage::Verify, &state, 2_060),
        "no checks for its repositories: set some, or Mark done by hand"
    );
    assert_eq!(view.which, Some((0, 1)));
    assert_eq!(
        view.sub,
        "b \u{b7} /work/.features/b \u{b7} started 17m ago"
    );
    let keys: Vec<&str> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "Left / Right",
            "Enter",
            "Tab",
            "Ctrl+Shift+U",
            "Ctrl+E",
            "F10",
            "F1",
        ]
    );
    assert_eq!(view.hints[1].1, "mark done");
    assert_eq!(view.hints[2].1, "repositories");
    assert_eq!(view.remove, "Abandon");
    // On the repositories: their keys, and the others' features.
    let on_repos = FeaturePanel {
        focus: TabArea::Repos,
        repo: 1,
        ..FeaturePanel::default()
    };
    let three = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    let view = tab_view(&on_repos, &f, &three, &state, 2_060);
    assert_eq!((view.focus, view.repo), (TabArea::Repos, 1));
    assert_eq!(view.which, Some((1, 3)));
    let hints: Vec<(&str, &str)> = (view.hints.iter())
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        &hints[..4],
        [
            ("Left / Right", "repository"),
            ("Enter", "show all"),
            ("Tab", "stages"),
            ("Del", "remove repository"),
        ]
    );
    // Unfolded they go through the rows and the row's buttons.
    let unfolded = FeaturePanel {
        expanded: true,
        ..on_repos
    };
    let view = tab_view(&unfolded, &f, &three, &state, 2_060);
    let hints: Vec<(&str, &str)> = (view.hints.iter())
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(hints[0], ("Up / Down", "repository"));
    assert!(hints.contains(&("Left / Right", "button")), "{hints:?}");
    assert!(hints.contains(&("Esc", "hide them")), "{hints:?}");
    // A probe that couldn't read the code says so.
    let failed = Probe {
        reading: Err("git exited with 128".into()),
        ..probe(None)
    };
    let state = PanelState {
        probe: Some(&failed),
        ..PanelState::default()
    };
    let view = tab_view(&panel, &f, &slugs, &state, 2_060);
    assert_eq!(
        view.repos_error.as_deref(),
        Some("Couldn't read the code: git exited with 128")
    );
}
