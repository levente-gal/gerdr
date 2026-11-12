//! Tests of the stage agents' cards and Plan review's page.

use super::*;

/// A run of `base` whose tab is closed.
fn kept_run(base: &str) -> feature::agents::StageRun {
    feature::agents::StageRun {
        base: base.into(),
        ..agent_run(None, None)
    }
}

#[test]
fn the_recap_counts_every_run_names_agents_as_the_rail_and_leaves_comments_out() {
    let mut f = feature("r", vec![repo("api", true)], 8);
    let run = |n| feature::agents::StageRun {
        agent: "claude".into(),
        n,
        ..Default::default()
    };
    f.runs.insert(Stage::Discover, run(1));
    f.runs.insert(Stage::AiReview, run(4));
    // Started over twice: forgotten, still counted.
    f.run_counts.insert(Stage::Build, 2);
    let manual = feature::manual::Counts {
        passed: 5,
        comments_done: 1,
        ..Default::default()
    };
    let state = PanelState {
        ctx: RowCtx {
            criteria: 5,
            manual,
            ..RowCtx::default()
        },
        ..PanelState::default()
    };
    let recap = recap_card(&f, &state, 3_000);
    let runs = ("7".to_owned(), "agent runs".to_owned());
    assert!(recap.figures.contains(&runs));
    let said = |label: &str| {
        let (_, said) = recap.stages.iter().find(|(l, _)| l == label).unwrap();
        said.clone()
    };
    assert!(
        said("Discover").starts_with("claude \u{b7} "),
        "{}",
        said("Discover")
    );
    assert!(said("AI review").starts_with("claude \u{b7} 4 runs"));
    assert!(said("Manual test").contains("4 of 4 criteria passed \u{b7} took"));
}

#[test]
fn enter_sends_the_prompt_of_an_agent_waiting_for_it() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    f.runs.insert(Stage::Plan, held_run(Some("p1")));
    let send = PanelOutcome::SendPrompt(Stage::Plan);
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    assert_eq!(actions[0].1, send);
    assert_eq!(actions[0].0.style, ButtonStyle::Primary);
    assert_eq!(actions[0].0.label, "Send prompt");
    assert_eq!(
        page_outcomes(&f, Stage::Plan)
            .iter()
            .filter(|o| **o == send)
            .count(),
        1
    );
    assert!(page_outcomes(&f, Stage::Plan).contains(&PanelOutcome::ShowAgent(Stage::Plan)));
    assert_eq!(
        page_enter(&f, Stage::Plan, RowCtx::default()),
        Some(send.clone())
    );
    // Nothing is being written while it waits.
    assert_eq!(
        file_line(&f, Stage::Plan, RowCtx::default()),
        line("not written yet", CheckTone::Note)
    );
    assert_eq!(
        file_line(&f, Stage::Plan, wrote_all()),
        line("written", CheckTone::Note)
    );
    // It sits on the agent's card, with what else acts on the agent.
    assert!(agent_buttons(Stage::Plan, &actions).contains(&0));
}

#[test]
fn a_waiting_agents_closed_tab_opens_again_rather_than_resume() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    f.runs.insert(Stage::Plan, held_run(None));
    let out = page_outcomes(&f, Stage::Plan);
    assert!(out.contains(&PanelOutcome::StartAgent(Stage::Plan, false)));
    assert!(!out.contains(&PanelOutcome::SendPrompt(Stage::Plan)));
    assert!(!out.contains(&PanelOutcome::ResumeAgent(Stage::Plan)));
    assert_eq!(
        page_enter(&f, Stage::Plan, RowCtx::default()),
        Some(PanelOutcome::StartAgent(Stage::Plan, false))
    );
}

#[test]
fn a_verify_with_nothing_to_run_points_to_its_commands() {
    let f = feature("v", vec![repo("api", true)], 4);
    assert_eq!(f.current(), Some(Stage::Verify));
    let edit = PanelOutcome::EditCommands(CommandsTab::Feature);
    let style = |ctx: RowCtx| {
        let actions = page_actions(&f, Stage::Verify, ctx);
        let found = actions.into_iter().find(|(_, o)| *o == edit);
        found.map(|(button, _)| button.style)
    };
    assert_eq!(style(RowCtx::default()), Some(ButtonStyle::Plain));
    let checks = RowCtx {
        checks: true,
        ..RowCtx::default()
    };
    assert_eq!(style(checks), Some(ButtonStyle::Quiet));
    // Enter still does what it did: the button doesn't take over.
    assert_eq!(
        page_enter(&f, Stage::Verify, RowCtx::default()),
        Some(PanelOutcome::Tick)
    );
    // Not offered on stages that have no commands.
    let actions = page_actions(&f, Stage::AiReview, RowCtx::default());
    assert!(
        !actions
            .iter()
            .any(|(_, o)| matches!(o, PanelOutcome::EditCommands(_)))
    );
}

#[test]
fn fixes_waiting_on_a_done_build_can_be_sent_from_its_page() {
    // Build done, a later stage current: a fresh agent for fixes waits.
    let mut f = feature("b", vec![repo("api", true)], 4);
    assert!(f.is_done(Stage::Build));
    f.runs.insert(Stage::Build, held_run(Some("p1")));
    let send = PanelOutcome::SendPrompt(Stage::Build);
    let actions = page_actions(&f, Stage::Build, RowCtx::default());
    assert_eq!(actions[0].1, send);
    assert_eq!(actions[0].0.style, ButtonStyle::Primary);
    let primaries = (actions.iter())
        .filter(|(b, _)| b.style == ButtonStyle::Primary)
        .count();
    assert_eq!(primaries, 1);
    assert_eq!(page_enter(&f, Stage::Build, RowCtx::default()), Some(send));
    let page = stage_page(&f, Stage::Build, &PanelState::default(), &actions, 2_000);
    assert!(page.sections.iter().any(|s| matches!(
        s,
        PageSection::Agent { status, waits: true, .. } if status.starts_with("your turn")
    )));
    // Its tab closed: it can still open again from there.
    f.runs.insert(Stage::Build, held_run(None));
    assert!(
        page_outcomes(&f, Stage::Build)
            .contains(&PanelOutcome::StartAgent(Stage::Build, false))
    );
}

#[test]
fn an_agent_waiting_on_you_has_its_card_stand_out() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    assert_eq!(f.current(), Some(Stage::Plan));
    let card = |f: &Feature, status| {
        let state = PanelState {
            agent_status: status,
            ..PanelState::default()
        };
        let page = stage_page(f, Stage::Plan, &state, &[], 2_000);
        page.sections.iter().find_map(|s| match s {
            PageSection::Agent { waits, dot, .. } => Some((*waits, *dot)),
            _ => None,
        })
    };
    // Held for its prompt, or told something in its tab and at work.
    f.runs.insert(Stage::Plan, held_run(Some("p1")));
    assert_eq!(card(&f, None), Some((true, Dot::Blocked)));
    let working = Some((false, Dot::Working));
    assert_eq!(card(&f, Some(AgentStatus::Working)), working);
    // Running: asked something, idle with plan.md not ready, at work.
    f.runs.insert(Stage::Plan, agent_run(Some("p1"), None));
    let waits = Some((true, Dot::Blocked));
    assert_eq!(card(&f, Some(AgentStatus::Blocked)), waits);
    assert_eq!(card(&f, Some(AgentStatus::Idle)), waits);
    assert_eq!(card(&f, Some(AgentStatus::Working)), working);
    // Idle while gerdr reads what it wrote: that waits on gerdr.
    let state = PanelState {
        agent_status: Some(AgentStatus::Idle),
        agent_read: true,
        ..PanelState::default()
    };
    let page = stage_page(&f, Stage::Plan, &state, &[], 2_000);
    assert!(page.sections.iter().any(|s| matches!(
        s,
        PageSection::Agent {
            waits: false,
            dot: Dot::Idle,
            ..
        }
    )));
    // Its file ready, it waits on nothing.
    f.runs.insert(Stage::Plan, agent_run(Some("p1"), Some(1)));
    assert_eq!(card(&f, Some(AgentStatus::Idle)).map(|c| c.0), Some(false));
    // Its tab closed: no answer to wait for.
    f.runs.insert(Stage::Plan, agent_run(None, None));
    assert_eq!(card(&f, None).map(|c| c.0), Some(false));
}

#[test]
fn work_sent_back_to_build_shows_on_the_page_of_the_stage_at_hand() {
    // At AI review, its findings sent, the Build agent back on them.
    let stage = Stage::AiReview;
    let mut f = feature("b", vec![repo("api", true)], 5);
    assert_eq!(f.current(), Some(stage));
    let mut live = held_run(Some("p1"));
    live.held = false;
    f.runs.insert(Stage::Build, live);
    let card = |f: &Feature, ctx: RowCtx| {
        let state = PanelState {
            ctx,
            ..PanelState::default()
        };
        let actions = page_actions(f, stage, ctx);
        let page = stage_page(f, stage, &state, &actions, 2_000);
        let shown = page.sections.first().and_then(|s| match s {
            PageSection::Agent {
                status,
                dot,
                buttons,
                ..
            } => {
                let labels = buttons.iter().map(|b| actions[*b].0.label.clone());
                Some((status.clone(), *dot, labels.collect::<Vec<_>>()))
            }
            _ => None,
        });
        let node = rail_node(f, Stage::Build, &state, 2_000);
        (shown, (node.mark, node.line))
    };
    // At rest, nothing says it is at work.
    let at_rest = RowCtx::default();
    assert_eq!(rework_on(&f, stage, at_rest), None);
    assert_eq!(card(&f, at_rest).1.0, NodeMark::Done);
    // Running: its node and a card with a way to its tab.
    let running = RowCtx {
        fixing: Some(Dot::Working),
        ..RowCtx::default()
    };
    let (shown, node) = card(&f, running);
    assert_eq!(
        shown,
        Some((
            "Build agent \u{b7} working again".to_owned(),
            Dot::Working,
            vec!["Show Build agent".to_owned()]
        ))
    );
    assert_eq!(node, (NodeMark::Fixing, "working again".to_owned()));
    let show = PanelOutcome::ShowAgent(Stage::Build);
    assert_eq!(agent_key(&f, stage, running).map(|k| k.1), Some(show));
    // Enter stays this page's: it only runs.
    assert_ne!(
        page_enter(&f, stage, running),
        Some(PanelOutcome::SendPrompt(Stage::Build))
    );
    // Waiting for its prompt: sending it is what Enter does.
    f.runs.insert(Stage::Build, held_run(Some("p1")));
    let (shown, node) = card(&f, at_rest);
    let (status, _, buttons) = shown.unwrap();
    assert!(status.starts_with("Build agent \u{b7} your turn"));
    // Send prompt, the primary, is on the page's bar.
    assert_eq!(buttons, ["Show Build agent"]);
    assert_eq!(node, (NodeMark::Waiting, "waiting to send".to_owned()));
    assert_eq!(
        page_enter(&f, stage, at_rest),
        Some(PanelOutcome::SendPrompt(Stage::Build))
    );
    let actions = page_actions(&f, stage, at_rest);
    let primaries = (actions.iter())
        .filter(|(b, _)| b.style == ButtonStyle::Primary)
        .count();
    assert_eq!(primaries, 1);
    // Its tab closed: it can open again, and Enter is this page's again.
    f.runs.insert(Stage::Build, held_run(None));
    let (shown, node) = card(&f, at_rest);
    assert_eq!(shown.unwrap().2, ["Start Build agent"]);
    assert_eq!(node, (NodeMark::Waiting, "agent closed".to_owned()));
    assert_ne!(
        page_enter(&f, stage, at_rest),
        Some(PanelOutcome::StartAgent(Stage::Build, false))
    );
    // Build's own page says it too, not "done".
    f.runs.insert(Stage::Build, held_run(Some("p1")));
    f.runs.get_mut(&Stage::Build).unwrap().held = false;
    let state = PanelState {
        ctx: running,
        ..PanelState::default()
    };
    let actions = page_actions(&f, Stage::Build, running);
    let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
    assert!(page.sections.iter().any(|s| matches!(
        s,
        PageSection::Agent { status, dot: Dot::Working, .. } if status == "working again"
    )));
}

#[test]
fn the_reviewer_waits_for_its_prompt_too() {
    let mut f = feature("b", vec![repo("api", true)], 5);
    assert_eq!(f.current(), Some(Stage::AiReview));
    f.runs.insert(Stage::AiReview, held_run(Some("p1")));
    let none = RowCtx::default();
    assert_eq!(
        review_action(&f, none),
        PanelOutcome::SendPrompt(Stage::AiReview)
    );
    f.runs.insert(Stage::AiReview, held_run(None));
    assert_eq!(
        review_action(&f, none),
        PanelOutcome::StartAgent(Stage::AiReview, false)
    );
}

#[test]
fn plan_review_goes_from_its_agent_to_the_decisions_to_the_final_plan() {
    use crate::feature::plan_review::{self, Decision, PlanReview};
    let mut f = feature("p", vec![repo("api", true)], 2);
    let stage = Stage::PlanReview;
    let ctx = RowCtx::default();
    assert_eq!(f.current(), Some(stage));
    // The page's own buttons, not those on the suggestions' cards.
    let names = |f: &Feature| -> Vec<String> {
        let actions = page_actions(f, stage, RowCtx::default());
        (actions.iter())
            .filter(|(_, o)| !matches!(o, PanelOutcome::OnSuggestion(..)))
            .map(|(b, _)| b.label.clone())
            .collect()
    };
    // Nothing runs until asked for, and it can be skipped at once.
    assert_eq!(names(&f), ["Start agent", "Skip review"]);
    assert_eq!(
        page_enter(&f, stage, ctx),
        Some(PanelOutcome::StartAgent(stage, false))
    );
    let state = PanelState::default();
    // Its suggestions' line says none is written yet, as a file's card
    // does, under what happens next, as nothing ran yet.
    let page = stage_page(&f, stage, &state, &[], 2_000);
    assert!(matches!(
        &page.sections[..],
        [PageSection::Lines { label: next, .. }, PageSection::Lines { label, lines }]
            if next == "What happens next"
                && label == "Suggestions" && lines[0].text == "not written yet"
    ));
    // Read in with two to decide: they are cards, and shown in the plan.
    let text = r#"{"suggestions": [{"kind": "question", "plan": "M1", "title": "Which?",
            "why": "w"}, {"kind": "question", "plan": "M1", "title": "How?", "why": "w"}]}"#;
    let mut review = PlanReview {
        stamp: Some((1, 1)),
        ..PlanReview::default()
    };
    plan_review::import(&mut review, 1, plan_review::parse(text).unwrap());
    f.plan_review = Some(review);
    assert_eq!(page_enter(&f, stage, ctx), Some(PanelOutcome::ReviewInPlan));
    // Questions with no answer can't all be accepted at once: the
    // buttons stay, greyed, so none shifts once they can be pressed.
    assert_eq!(
        names(&f),
        [
            "Review in plan",
            "Accept all open",
            "Add suggestion\u{2026}",
            "Ask reviewer",
            "Skip review",
            "Start over\u{2026}"
        ]
    );
    assert_eq!(rail_node(&f, stage, &state, 2_000).line, "2 to decide");
    let page = stage_page(&f, stage, &state, &[], 2_000);
    assert!(matches!(page.sections[0], PageSection::Review(_)));
    assert_eq!(
        summary_line(&page.sections),
        Some(("Suggestions \u{b7} 2 to decide".to_owned(), Vec::new()))
    );
    let titles: Vec<&str> = (page.sections.iter())
        .filter_map(|s| match s {
            PageSection::Finding(card) => Some(card.title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Which?", "How?"]);
    // Its run shows as no card of its own: the suggestions say it.
    assert!(
        !page
            .sections
            .iter()
            .any(|s| matches!(s, PageSection::Round(_)))
    );
    assert_eq!(
        crate::feature_agents::gate_of(&f, |_| None).map(|g| g.2),
        Some(crate::feature_agents::GateKind::Suggestions { open: 2 })
    );
    // All rejected: nothing for a final plan to add.
    let review = f.plan_review.as_mut().unwrap();
    plan_review::decide(review, "r1-1", Decision::Reject("No".into())).unwrap();
    plan_review::decide(review, "r1-2", Decision::Reject("No".into())).unwrap();
    assert_eq!(
        page_enter(&f, stage, ctx),
        Some(PanelOutcome::WriteFinalPlan)
    );
    // All decided: skipping the review is no longer offered.
    assert_eq!(
        names(&f),
        [
            "Use the plan as it is",
            "Review in plan",
            "Accept all open",
            "Add suggestion\u{2026}",
            "Ask reviewer",
            "Start over\u{2026}"
        ]
    );
    assert_eq!(rail_node(&f, stage, &state, 2_000).line, "all decided");
    // One accepted: a final plan.
    let review = f.plan_review.as_mut().unwrap();
    plan_review::set_note(review, "r1-1", "This one").unwrap();
    plan_review::decide(review, "r1-1", Decision::Accept).unwrap();
    assert_eq!(names(&f)[0], "Write final plan");
    // With a note, the reviewer can be asked about it.
    assert!(names(&f).contains(&"Ask reviewer".to_owned()));
    // Being written: its agent's page, and a way back.
    f.plan_review.as_mut().unwrap().writing = true;
    f.runs.insert(
        stage,
        crate::feature::agents::StageRun {
            n: 2,
            ready: Some(50),
            ..Default::default()
        },
    );
    assert_eq!(
        page_enter(&f, stage, ctx),
        Some(PanelOutcome::OpenDoc(stage))
    );
    assert!(names(&f).contains(&"Change decisions\u{2026}".to_owned()));
    assert_eq!(
        crate::feature_agents::gate_of(&f, |_| None).map(|g| g.2),
        Some(crate::feature_agents::GateKind::Review)
    );
    // Its keys, only there.
    let mut panel = FeaturePanel::default();
    let shift = ModifiersState {
        ctrl: true,
        shift: true,
        ..ModifiersState::default()
    };
    assert_eq!(
        press(&mut panel, &f, (shift, Keysym::K)),
        Some(PanelOutcome::SkipPlanReview)
    );
    assert_eq!(
        press(&mut panel, &f, (shift, Keysym::D)),
        Some(PanelOutcome::ChangeDecisions)
    );
    let later = feature("q", vec![repo("api", true)], 3);
    assert_eq!(press(&mut panel, &later, (shift, Keysym::K)), None);
    assert_eq!(press(&mut panel, &later, (shift, Keysym::D)), None);
}

/// A feature at Plan review with its review read: `r1-1`, a high gap
/// citing code; `r1-2`, a question; `u1`, one of the user's own,
/// accepted. The page lists them `u1`, `r1-1`, `r1-2`.
fn suggesting() -> Feature {
    use crate::feature::plan_review::{Evidence, Kind, PlanReview, Source, State};
    use crate::feature::review_import::Severity;
    let mut f = feature("p", vec![repo("api", true)], 2);
    let s = |id: &str, kind, severity, source| crate::feature::plan_review::Suggestion {
        id: id.into(),
        title: format!("Title {id}"),
        kind,
        severity,
        source,
        run: 1,
        ..Default::default()
    };
    let mut gap = s("r1-1", Kind::Gap, Severity::High, Source::Reviewer);
    gap.change = "Add a rollback".into();
    gap.evidence = vec![Evidence {
        file: "src/a.rs".into(),
        line: 3,
        quote: "x".into(),
        ..Evidence::default()
    }];
    let question = s("r1-2", Kind::Question, Severity::Low, Source::Reviewer);
    let mut own = s("u1", Kind::Gap, Severity::Medium, Source::You);
    own.state = State::Accepted;
    f.plan_review = Some(PlanReview {
        n: 1,
        stamp: Some((1, 1)),
        suggestions: vec![gap, question, own],
        ..PlanReview::default()
    });
    assert_eq!(f.current(), Some(Stage::PlanReview));
    f
}

fn on_plan_review(f: &Feature) -> FeaturePanel {
    FeaturePanel {
        stage: Some(Stage::PlanReview),
        picked_at: f.current(),
        ..FeaturePanel::default()
    }
}

fn shifted(sym: Keysym) -> (ModifiersState, Keysym) {
    let mods = ModifiersState {
        shift: true,
        ..ModifiersState::default()
    };
    (mods, sym)
}

fn on_suggestion(id: &str, act: ItemAct) -> Option<PanelOutcome> {
    Some(PanelOutcome::OnSuggestion(id.to_owned(), act))
}

#[test]
fn review_cards_go_under_the_summary_that_counts_them_open() {
    let f = suggesting();
    let panel = on_plan_review(&f);
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    let at = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::Review(_)))
        .expect("the summary");
    let PageSection::Review(summary) = &view.page.sections[at] else {
        unreachable!()
    };
    assert_eq!(
        summary.counts,
        [
            (FindingTag::Yours, 1),
            (FindingTag::High, 1),
            (FindingTag::Low, 1)
        ]
    );
    // Each pill says how many are left to decide: the user's own is.
    assert_eq!(summary.open, [0, 1, 1]);
    assert_eq!((summary.decided, summary.refused, summary.total), (1, 0, 3));
    // Under it: its cards, with no heading per tag (the pills say it).
    let shape: Vec<String> = (view.page.sections[at + 1..].iter())
        .map(|s| match s {
            PageSection::Heading(label) => label.clone(),
            PageSection::Finding(c) => format!("card {:?}", c.tag),
            _ => "other".into(),
        })
        .collect();
    assert_eq!(shape, ["card Yours", "card High", "card Low"]);
    // Accept all open goes with the summary, not the title.
    let want = (view.page.buttons.iter())
        .position(|b| b.label == "Accept all open")
        .expect("accept all");
    assert_eq!(summary.buttons, [want]);
}

#[test]
fn plan_reviews_suggestions_show_as_cards_as_ai_reviews_findings_do() {
    use crate::feature::plan_review::{self, Decision};
    let mut f = suggesting();
    let panel = on_plan_review(&f);
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    let heading = summary_line(&view.page.sections).map(|(label, _)| label);
    assert_eq!(
        heading.as_deref(),
        Some("Suggestions \u{b7} 2 to decide \u{b7} 1 accepted")
    );
    let cards = cards_of(&view);
    let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Title u1", "Title r1-1", "Title r1-2"]);
    let labels = labels_of(&view, cards[1]);
    assert!(labels.contains(&"Accept".to_owned()), "{labels:?}");
    assert!(labels.contains(&"Show in plan".to_owned()), "{labels:?}");
    // Review in plan is the way on, with the page's own beside it.
    assert_eq!(
        page_enter(&f, Stage::PlanReview, RowCtx::default()),
        Some(PanelOutcome::ReviewInPlan)
    );
    let page: Vec<(String, PanelOutcome)> =
        (page_actions(&f, Stage::PlanReview, RowCtx::default()).into_iter())
            .filter(|(_, o)| !matches!(o, PanelOutcome::OnSuggestion(..)))
            .map(|(b, o)| (b.label, o))
            .collect();
    assert_eq!(
        page[0],
        ("Review in plan".to_owned(), PanelOutcome::ReviewInPlan)
    );
    let has = |label: &str, o: PanelOutcome| page.contains(&(label.to_owned(), o));
    assert!(has("Accept all open", PanelOutcome::AcceptAllSuggestions));
    assert!(has("Add suggestion\u{2026}", PanelOutcome::AddComment));
    // Nothing to ask yet: greyed.
    let ask = (page_actions(&f, Stage::PlanReview, RowCtx::default()).into_iter())
        .find(|(b, _)| b.label == "Ask reviewer");
    assert_eq!(ask.map(|(b, _)| b.style), Some(ButtonStyle::Off));
    // No word of AI review's.
    for (label, _) in &page {
        let label = label.to_lowercase();
        assert!(
            !label.contains("fix") && !label.contains("dismiss"),
            "{label}"
        );
    }
    // A rejected one says why on its card.
    let review = f.plan_review.as_mut().unwrap();
    plan_review::decide(review, "r1-1", Decision::Reject("Not needed".into())).unwrap();
    plan_review::set_note(review, "r1-2", "Both").unwrap();
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    assert_eq!(cards_of(&view)[1].state, "rejected: Not needed");
    // A note to ask about: the reviewer can be.
    let asks = page_actions(&f, Stage::PlanReview, RowCtx::default());
    assert!(
        asks.iter()
            .any(|(b, o)| b.label == "Ask reviewer" && *o == PanelOutcome::AskReviewer)
    );
}

#[test]
fn plan_reviews_cards_take_ai_reviews_keys() {
    let f = suggesting();
    let mut panel = on_plan_review(&f);
    // Tab goes to the cards.
    press(&mut panel, &f, key(Keysym::Tab));
    assert_eq!(panel.focus, TabArea::Findings);
    panel.finding = 1;
    // Accepted, the keyboard goes on to the next still to decide.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        on_suggestion("r1-1", ItemAct::Decide(Decision::Fix))
    );
    assert_eq!(panel.finding, 2);
    panel.finding = 1;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        on_suggestion("r1-1", ItemAct::AskRefuse)
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::c)),
        on_suggestion("r1-1", ItemAct::Comment)
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::e)),
        on_suggestion("r1-1", ItemAct::Edit)
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        on_suggestion("r1-1", ItemAct::Show)
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::o)),
        on_suggestion("r1-1", ItemAct::OpenCode)
    );
    // No comment to delete.
    assert_eq!(press(&mut panel, &f, shifted(Keysym::Delete)), None);
    // Citing nothing, Ctrl+O opens the folder as anywhere; a question
    // with no answer is accepted by answering it.
    panel.finding = 2;
    assert_eq!(
        press(&mut panel, &f, key(Keysym::f)),
        on_suggestion("r1-2", ItemAct::Answer)
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::o)),
        Some(PanelOutcome::Open(f.dir.clone()))
    );
    // One of the user's own: nothing to take back, Del arms, then
    // deletes; `c` adds another.
    panel.finding = 0;
    assert!(matches!(
        press(&mut panel, &f, key(Keysym::f)),
        Some(PanelOutcome::FindingRefused(_))
    ));
    assert_eq!(press(&mut panel, &f, key(Keysym::Delete)), None);
    assert_eq!(panel.armed.as_deref(), Some("own-suggestion:u1"));
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    assert_eq!(cards_of(&view)[0].state, "Press Del again");
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Delete)),
        on_suggestion("u1", ItemAct::DeleteOwn)
    );
    assert_eq!(
        press(&mut panel, &f, key(Keysym::c)),
        Some(PanelOutcome::AddComment)
    );
    // From anywhere on the page: all open, going on, asking.
    for focus in [TabArea::Findings, TabArea::Stages] {
        panel.focus = focus;
        assert_eq!(
            press(&mut panel, &f, shifted(Keysym::f)),
            Some(PanelOutcome::AcceptAllSuggestions)
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::Return)),
            Some(PanelOutcome::WriteFinalPlan)
        );
        // Not Reopen's: the stage is current.
        assert_eq!(
            press(&mut panel, &f, ctrl_shift(Keysym::Return)),
            Some(PanelOutcome::AskReviewer)
        );
    }
    // Still Plan review's own.
    assert_eq!(
        press(&mut panel, &f, ctrl_shift(Keysym::k)),
        Some(PanelOutcome::SkipPlanReview)
    );
}

#[test]
fn plan_reviews_cards_refuse_deciding_while_the_final_plan_is_written() {
    let mut f = suggesting();
    f.plan_review.as_mut().unwrap().writing = true;
    let mut panel = on_plan_review(&f);
    panel.focus = TabArea::Findings;
    panel.finding = 1;
    let refused = |out: Option<PanelOutcome>| {
        assert!(
            matches!(out, Some(PanelOutcome::FindingRefused(_))),
            "{out:?}"
        );
    };
    refused(press(&mut panel, &f, key(Keysym::f)));
    refused(press(&mut panel, &f, key(Keysym::Delete)));
    refused(press(&mut panel, &f, key(Keysym::c)));
    refused(press(&mut panel, &f, ctrl(Keysym::e)));
    // What only shows still does.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        on_suggestion("r1-1", ItemAct::Show)
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::o)),
        on_suggestion("r1-1", ItemAct::OpenCode)
    );
    assert_ne!(
        press(&mut panel, &f, shifted(Keysym::f)),
        Some(PanelOutcome::AcceptAllSuggestions)
    );
    assert_ne!(
        press(&mut panel, &f, ctrl_shift(Keysym::Return)),
        Some(PanelOutcome::AskReviewer)
    );
    // The page says so, and how to go back.
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    let said = summary_line(&view.page.sections).is_some_and(|(label, lines)| {
        label.starts_with("Suggestions")
            && lines.iter().any(|l| l.text.contains("Change decisions"))
    });
    assert!(said, "{:?}", view.page.sections);
    // A comment field left open doesn't show on a card it can't change.
    let note = CardNote::Item {
        of: ItemOf::Suggestion,
        id: "r1-1".into(),
        accept: false,
    };
    panel.note = Some((note, TextField::default()));
    let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
    assert!(cards_of(&view).iter().all(|c| c.field.is_none()));
}

#[test]
fn a_suggestions_comment_is_written_on_its_card_and_not_a_findings() {
    let f = suggesting();
    let mut panel = on_plan_review(&f);
    for (of, shows) in [(ItemOf::Suggestion, true), (ItemOf::Finding, false)] {
        let note = CardNote::Item {
            of,
            id: "r1-1".into(),
            accept: false,
        };
        panel.note = Some((note, TextField::default()));
        let view = tab_view(&panel, &f, &["p".to_owned()], &PanelState::default(), 2_000);
        assert_eq!(cards_of(&view)[1].field.is_some(), shows, "{of:?}");
    }
    // A question's answer accepting it once saved says so.
    let note = CardNote::Item {
        of: ItemOf::Suggestion,
        id: "r1-2".into(),
        accept: true,
    };
    panel.note = Some((note, TextField::multiline("Both")));
    let hints = tab_hints(
        &panel,
        &f,
        Stage::PlanReview,
        &["p".to_owned()],
        RowCtx::default(),
        AgentChoices::default(),
        &[],
        &[],
        &[],
    );
    assert_eq!(hints[0], ("Enter".to_owned(), "save and accept".to_owned()));
}

#[test]
fn plan_reviews_footer_says_its_cards_keys_in_its_own_words() {
    let f = suggesting();
    let mut panel = on_plan_review(&f);
    let hints_on = |panel: &FeaturePanel| {
        tab_hints(
            panel,
            &f,
            Stage::PlanReview,
            &["p".to_owned()],
            RowCtx::default(),
            AgentChoices::default(),
            &[],
            &[],
            &[],
        )
    };
    let has = |hints: &[(String, String)], k: &str, what: &str| {
        hints.contains(&(k.to_owned(), what.to_owned()))
    };
    panel.focus = TabArea::Findings;
    panel.finding = 1;
    let hints = hints_on(&panel);
    for (k, what) in [
        ("Up / Down", "suggestion"),
        ("f", "accept"),
        ("Ctrl+E", "edit change\u{2026}"),
        ("c", "comment"),
        ("Del", "reject\u{2026}"),
        ("Enter", "show in plan"),
        ("Ctrl+O", "open code"),
        ("Shift+F", "accept all open"),
    ] {
        assert!(has(&hints, k, what), "{k} {what}: {hints:?}");
    }
    for (_, what) in &hints {
        assert!(!what.contains("fix") && !what.contains("dismiss"), "{what}");
    }
    // Still to decide, Ctrl+Enter isn't offered; nothing noted, nor asking.
    assert!(
        !hints
            .iter()
            .any(|(k, _)| k == "Ctrl+Enter" || k == "Ctrl+Shift+Enter")
    );
    // A question with no answer takes one: `f` accepts it once it has,
    // `c` writes it, one word for each key.
    panel.finding = 2;
    let hints = hints_on(&panel);
    assert!(
        has(&hints, "f", "accept\u{2026}") && has(&hints, "c", "answer"),
        "{hints:?}"
    );
    // One of the user's own, accepted.
    panel.finding = 0;
    let hints = hints_on(&panel);
    assert!(!hints.iter().any(|(k, _)| k == "f") && has(&hints, "Ctrl+E", "edit\u{2026}"));
    assert!(has(&hints, "Del", "delete") && has(&hints, "c", "add suggestion\u{2026}"));
    // On the stages, Tab goes to them, and Enter shows them in the plan.
    panel.focus = TabArea::Stages;
    let hints = hints_on(&panel);
    assert!(has(&hints, "Tab", "suggestions") && has(&hints, "Enter", "review in plan"));
}

#[test]
fn a_done_stage_can_go_back_to_its_agents_chat() {
    let mut f = feature("b", vec![repo("api", true)], 2);
    f.runs.insert(Stage::Plan, kept_run("claude"));
    let resume = PanelOutcome::ResumeAgent(Stage::Plan);
    assert!(page_outcomes(&f, Stage::Plan).contains(&resume));
    // Enter on a done stage doesn't: it's a button away.
    assert_ne!(
        page_enter(&f, Stage::Plan, RowCtx::default()),
        Some(resume.clone())
    );
    // The agent card's button is that one.
    let actions = page_actions(&f, Stage::Plan, RowCtx::default());
    let page = stage_page(&f, Stage::Plan, &PanelState::default(), &actions, 2_000);
    let button = page.sections.iter().find_map(|s| match s {
        PageSection::Agent { buttons, .. } => buttons.first().copied(),
        _ => None,
    });
    assert_eq!(
        button.map(|i| page.buttons[i].label.as_str()),
        Some("Resume agent")
    );
}

#[test]
fn only_a_kept_chat_of_an_open_feature_resumes() {
    let resume = PanelOutcome::ResumeAgent(Stage::Plan);
    let mut f = feature("b", vec![repo("api", true)], 3);
    f.status = Status::Abandoned;
    f.runs.insert(Stage::Plan, kept_run("claude"));
    assert!(!page_outcomes(&f, Stage::Plan).contains(&resume));
    f.status = Status::Open;
    for base in ["", "codex", "aider"] {
        f.runs.insert(Stage::Plan, kept_run(base));
        assert!(!page_outcomes(&f, Stage::Plan).contains(&resume), "{base}");
    }
}

#[test]
fn a_tab_still_there_is_shown_not_resumed() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    f.runs.insert(
        Stage::Plan,
        feature::agents::StageRun {
            base: "claude".into(),
            ..agent_run(Some("p1"), None)
        },
    );
    let out = page_outcomes(&f, Stage::Plan);
    assert!(out.contains(&PanelOutcome::ShowAgent(Stage::Plan)));
    assert!(!out.contains(&PanelOutcome::ResumeAgent(Stage::Plan)));
}

#[test]
fn agent_stage_rows_start_show_and_review_their_agent() {
    let mut f = feature("b", vec![repo("api", true)], 1);
    let mut panel = FeaturePanel {
        project: "acme".into(),
        slug: "b".into(),
        ..FeaturePanel::default()
    };
    let slugs = vec!["b".to_owned()];
    let plan = PanelRow::Stage(Stage::Plan);
    let hints = |panel: &FeaturePanel, f: &Feature, state: &PanelState| {
        tab_view(panel, f, &slugs, state, 2_000)
            .hints
            .into_iter()
            .take(4)
            .collect::<Vec<_>>()
    };
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    // No run yet: Enter starts one; no fresh one to ask for.
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::StartAgent(Stage::Plan, false))
    );
    assert_eq!(press(&mut panel, &f, ctrl(Keysym::r)), None);
    assert_eq!(
        stage_button(&f, Stage::Plan, RowCtx::default()),
        Some("Start agent")
    );
    let state = PanelState::default();
    assert_eq!(
        hints(&panel, &f, &state),
        [
            pair("Left / Right", "stage"),
            pair("Enter", "start agent"),
            pair("Tab", "repositories"),
            pair("F10", "more actions")
        ]
    );
    assert_eq!(
        stage_detail(&f, Stage::Plan, &state, 2_000),
        Stage::Plan.about()
    );
    // Its tab closed, from before gerdr kept chats: Enter starts anew.
    f.runs.insert(Stage::Plan, agent_run(None, None));
    assert_eq!(
        row_action(&f, plan, RowCtx::default()),
        Some(PanelOutcome::StartAgent(Stage::Plan, false))
    );
    // Its tab closed, its chat kept: Enter goes back to it, Ctrl+R
    // still starts a fresh one.
    f.runs.insert(Stage::Plan, kept_run("claude"));
    assert_eq!(
        row_action(&f, plan, RowCtx::default()),
        Some(PanelOutcome::ResumeAgent(Stage::Plan))
    );
    assert_eq!(
        stage_button(&f, Stage::Plan, RowCtx::default()),
        Some("Resume agent")
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::r)),
        Some(PanelOutcome::StartAgent(Stage::Plan, true))
    );
    let (mut mods, sym) = ctrl(Keysym::R);
    mods.shift = true;
    assert_eq!(
        press(&mut panel, &f, (mods, sym)),
        Some(PanelOutcome::ResumeAgent(Stage::Plan))
    );
    // A CLI that can't go back to a chat starts anew.
    f.runs.insert(Stage::Plan, kept_run("codex"));
    assert_eq!(press(&mut panel, &f, (mods, sym)), None);
    assert_eq!(
        row_action(&f, plan, RowCtx::default()),
        Some(PanelOutcome::StartAgent(Stage::Plan, false))
    );
    // Running: Enter shows it, Ctrl+R starts a fresh one.
    f.runs.insert(Stage::Plan, agent_run(Some("p1"), None));
    assert_eq!(
        row_action(&f, plan, RowCtx::default()),
        Some(PanelOutcome::ShowAgent(Stage::Plan))
    );
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::r)),
        Some(PanelOutcome::StartAgent(Stage::Plan, true))
    );
    assert_eq!(
        stage_button(&f, Stage::Plan, RowCtx::default()),
        Some("Show agent")
    );
    let state = PanelState {
        agent: Some("Claude Code working \u{b7} started 3m ago".into()),
        ..PanelState::default()
    };
    assert_eq!(
        hints(&panel, &f, &state),
        [
            pair("Left / Right", "stage"),
            pair("Enter", "show agent"),
            pair("Tab", "repositories"),
            pair("Ctrl+R", "restart")
        ]
    );
    assert_eq!(
        stage_detail(&f, Stage::Plan, &state, 2_000),
        "Claude Code working \u{b7} started 3m ago"
    );
    // Only the current stage says it.
    assert_eq!(
        stage_detail(&f, Stage::Build, &state, 2_000),
        Stage::Build.about()
    );
    // Its tab gone: Enter starts again.
    f.runs.insert(Stage::Plan, agent_run(None, None));
    assert_eq!(
        row_action(&f, plan, RowCtx::default()),
        Some(PanelOutcome::StartAgent(Stage::Plan, false))
    );
    // Ready: Enter reviews the plan.
    f.runs
        .insert(Stage::Plan, agent_run(Some("p1"), Some(1_500)));
    assert_eq!(
        press(&mut panel, &f, key(Keysym::Return)),
        Some(PanelOutcome::OpenDoc(Stage::Plan))
    );
    assert_eq!(
        stage_button(&f, Stage::Plan, RowCtx::default()),
        Some("Review")
    );
    // As the file card's button says it.
    assert_eq!(hints(&panel, &f, &state)[1], pair("Enter", "review"));
    // Ctrl+R only on the agent stage's own page.
    press(&mut panel, &f, key(Keysym::Up));
    assert_eq!(press(&mut panel, &f, ctrl(Keysym::r)), None);
    let discover = feature("a", vec![repo("api", true)], 0);
    assert_eq!(
        stage_button(&discover, Stage::Discover, RowCtx::default()),
        Some("Start agent")
    );
}

#[test]
fn build_continues_after_a_milestone_and_offers_no_mark() {
    let mut f = feature("b", vec![repo("api", true)], 3);
    let build = PanelRow::Stage(Stage::Build);
    let none = RowCtx::default();
    // No agent yet: Enter starts one; never a mark.
    assert_eq!(
        row_action(&f, build, none),
        Some(PanelOutcome::StartAgent(Stage::Build, false))
    );
    assert_eq!(stage_button(&f, Stage::Build, none), Some("Start agent"));
    // Its tab there: shown, even once its file is written.
    f.runs
        .insert(Stage::Build, agent_run(Some("p1"), Some(1_500)));
    assert_eq!(
        row_action(&f, build, none),
        Some(PanelOutcome::ShowAgent(Stage::Build))
    );
    assert_eq!(fresh_stage(&f, build), Some(Stage::Build));
    // Stopped after a milestone: Enter lets it go on.
    let mut run = agent_run(Some("p1"), None);
    run.stop = Some(feature::agents::Stop {
        done: 2,
        total: 4,
        ..Default::default()
    });
    f.runs.insert(Stage::Build, run);
    assert_eq!(
        row_action(&f, build, none),
        Some(PanelOutcome::Continue(Stage::Build))
    );
    assert_eq!(row_hint(&f, build, none), "continue");
    assert_eq!(stage_button(&f, Stage::Build, none), Some("Continue"));
    // Ctrl+R still starts a fresh one.
    let mut panel = FeaturePanel {
        ..FeaturePanel::default()
    };
    assert_eq!(
        press(&mut panel, &f, ctrl(Keysym::r)),
        Some(PanelOutcome::StartAgent(Stage::Build, true))
    );
}
