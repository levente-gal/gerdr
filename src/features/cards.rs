//! The cards of the stage pages: agents, files, fix rounds, Ship's bases, drafts.

use super::*;

/// The card of `stage`'s agent in `feature`, once one ran or one can be
/// picked, with the page's buttons in `actions` that act on it.
pub(super) fn agent_card(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Option<PageSection> {
    let picks = state.picks.clone();
    let buttons = agent_buttons(stage, actions);
    let Some(run) = feature.runs.get(&stage) else {
        return (!picks.is_empty()).then(|| PageSection::Agent {
            stage,
            icon: AgentIcon::Generic,
            name: String::new(),
            dot: Dot::Unknown,
            // The card's label: on Ship, what its agent is for.
            status: match stage {
                Stage::Ship => "Summary and conflicts agent".to_owned(),
                _ => String::new(),
            },
            tail: Vec::new(),
            buttons,
            picks,
            model: state.model.clone(),
            waits: false,
        });
    };
    let mark = stage_mark(feature, stage);
    let current = mark == StageMark::Current;
    let back = rework(feature, state.ctx).filter(|_| stage == Stage::Build);
    let dot = match (mark, back) {
        (_, Some(Rework::Runs(dot))) => dot,
        (StageMark::Done, _) => Dot::Done,
        // Whose turn it is, in its colour, as the words beside it say.
        (StageMark::Current, _) if run.live() => match state.agent_standing {
            Some(chrome::Standing::Done) => Dot::Done,
            Some(chrome::Standing::Working) => Dot::Working,
            Some(chrome::Standing::Failed) => Dot::Failed,
            Some(chrome::Standing::Yours) => Dot::Blocked,
            _ => state.agent_status.map_or(Dot::Unknown, Dot::of_status),
        },
        _ => Dot::Unknown,
    };
    // The card's head names the agent already.
    let status = match (current, &state.agent) {
        // On a done Build too, where fixes wait: `started` would hide that.
        _ if run.waits_to_send() => "your turn \u{b7} send the prompt".to_owned(),
        (true, Some(line)) => (line.strip_prefix(run.agent.as_str()))
            .map_or(line.as_str(), str::trim_start)
            .to_owned(),
        _ => {
            let started = format!("started {}", crate::clock::ago(now, run.started));
            match &run.model {
                Some(model) => format!("{model} \u{b7} {started}"),
                None => started,
            }
        }
    };
    let status = back.map_or(status, |back| back.status().to_owned());
    // Where it runs goes with that, or says all there is to say of it.
    let status = match &state.machine_line {
        Some((line, true)) => line.clone(),
        Some((line, false)) if status.is_empty() => line.clone(),
        Some((line, false)) => format!("{status} \u{b7} {line}"),
        None => status,
    };
    // Once its work is read, what it said is old news.
    let done = state.agent_standing == Some(chrome::Standing::Done);
    // Asked something, held for its prompt, or idle with its work undone.
    let waits = match back {
        Some(back) => back.waits_on_you(),
        None if current && state.agent_read => false,
        None if current => {
            let run = crate::feature_agents::as_read(feature, stage, run);
            crate::feature_agents::waits_on_you(&run, stage, state.agent_status)
        }
        None => run.waits_to_send(),
    };
    // The tail read is the current stage's agent's, while its pane is: what
    // it said, not how gerdr launched it.
    let tail = match current && run.pane.is_some() && !done {
        true => plain_tail(state.tail, waits),
        false => Vec::new(),
    };
    Some(PageSection::Agent {
        stage,
        icon: AgentIcon::from_agent(&run.agent),
        name: run.agent.clone(),
        dot: if waits { Dot::Blocked } else { dot },
        status,
        tail,
        buttons,
        picks,
        model: state.model.clone(),
        waits,
    })
}

/// The cards of `feature`'s own agents whose tab is there, on the page of
/// the stage it is at (`own_page`), each with its Show agent in `actions`.
pub(super) fn own_cards(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    if stage != own_page(feature) {
        return Vec::new();
    }
    let live = feature.own_agents.iter().filter(|a| a.pane.is_some());
    live.map(|own| {
        let status = (state.own_status.iter())
            .find(|(n, _)| *n == own.n)
            .and_then(|(_, s)| *s);
        let started = format!("started {}", crate::clock::ago(now, own.started));
        let words = match status {
            Some(AgentStatus::Working) => "working",
            Some(AgentStatus::Blocked) => "asks you",
            Some(AgentStatus::Idle | AgentStatus::Done) => "your turn",
            _ => "",
        };
        let status_line = match words {
            "" => format!("agent {} \u{b7} {started}", own.n),
            words => format!("agent {} \u{b7} {words} \u{b7} {started}", own.n),
        };
        let buttons = (actions.iter().enumerate())
            .filter(|(_, (_, o))| *o == PanelOutcome::ShowOwnAgent(own.n))
            .map(|(i, _)| i)
            .collect();
        let waits = status == Some(AgentStatus::Blocked);
        PageSection::Agent {
            stage,
            icon: AgentIcon::from_agent(&own.agent),
            name: own.agent.clone(),
            dot: status.map_or(Dot::Unknown, Dot::of_status),
            status: status_line,
            tail: Vec::new(),
            buttons,
            picks: Vec::new(),
            model: None,
            waits,
        }
    })
    .collect()
}

/// What an agent card shows of its pane's last lines: none of the shell's
/// prompt or the command gerdr launched it with (a path and flags, cut off),
/// and while it doesn't wait on the user only its last line, a plain status;
/// waiting, the lines that hold what it asks.
pub(super) fn plain_tail(lines: &[String], waits: bool) -> Vec<String> {
    let launch = |l: &str| {
        let l = l.trim();
        [
            "--add-dir",
            "--session-id",
            "$(tee ",
            "/prompts/",
            "random/uuid",
            "% cd ",
            "$ cd ",
        ]
        .iter()
        .any(|bit| l.contains(bit))
            || l.ends_with('%')
            || l.ends_with('$')
    };
    let kept: Vec<String> = (lines.iter())
        .filter(|l| !l.trim().is_empty() && !launch(l))
        .cloned()
        .collect();
    let keep = if waits { kept.len() } else { 1 };
    kept[kept.len().saturating_sub(keep)..].to_vec()
}

/// The Build agent's card on the page of the stage `feature` is at, while
/// work sent back to that agent is under way, or while the page picks the
/// agent its fixes go to: this page's own agent is another one, and this
/// is the page the user comes back to.
pub(super) fn rework_card(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Option<PageSection> {
    let back = rework_on(feature, stage, state.ctx);
    if back.is_none() && (state.fix_picks.is_empty() || !keys::fix_page(feature, stage)) {
        return None;
    }
    let lit = state.fix_picks.iter().find(|c| c.lit);
    let buttons = agent_buttons(Stage::Build, actions);
    let Some(run) = feature.runs.get(&Stage::Build) else {
        // None built it (a pull request's own): only where its fixes go.
        return back.is_none().then(|| PageSection::Agent {
            stage: Stage::Build,
            icon: lit.map_or(AgentIcon::Generic, |c| c.icon),
            name: String::new(),
            dot: Dot::Unknown,
            status: "Fixes".to_owned(),
            tail: Vec::new(),
            buttons,
            picks: state.fix_picks.clone(),
            model: state.fix_model.clone(),
            waits: false,
        });
    };
    let status = match (back, lit) {
        (Some(back), _) => back.status().to_owned(),
        (None, Some(lit)) if state.fix_moved => format!("next fixes: a fresh {}", lit.name),
        (None, _) => "makes the fixes".to_owned(),
    };
    Some(PageSection::Agent {
        stage: Stage::Build,
        icon: AgentIcon::from_agent(&run.agent),
        name: run.agent.clone(),
        dot: match back {
            Some(back) if back.waits_on_you() => Dot::Blocked,
            Some(Rework::Runs(dot)) => dot,
            Some(_) => Dot::Unknown,
            None => Dot::Done,
        },
        status: format!("Build agent \u{b7} {status}"),
        tail: Vec::new(),
        buttons,
        picks: state.fix_picks.clone(),
        model: state.fix_model.clone(),
        waits: back.is_some_and(Rework::waits_on_you),
    })
}

/// The card of a stage's file in `feature`: how it stands, with the page's
/// buttons on it in `actions`: to its tab, plain or the one to press next,
/// Approve while it waits for review, and the final plan's Diff.
pub(super) fn file_card(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    actions: &[(PageButton, PanelOutcome)],
) -> PageSection {
    let own = [
        PanelOutcome::OpenDoc(stage),
        PanelOutcome::ApproveDoc(stage),
        PanelOutcome::PlanChanges,
    ];
    PageSection::File {
        name: crate::feature::artifacts::name(stage)
            .unwrap_or("file")
            .to_owned(),
        state: file_line(feature, stage, ctx),
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| own.contains(o))
            .map(|(i, _)| i)
            .collect(),
    }
}

/// What round `round` of `feature` is called, as Build's page has it.
pub(super) fn round_label(round: &FixRound) -> String {
    feature::fix_round::label(round.n)
}

/// The open fix round's card: which it is, what it fixes and how it stands,
/// at `now`, with the page's buttons in `actions` that act on the plan.
pub(super) fn fix_round_card(
    feature: &Feature,
    round: &FixRound,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> PageSection {
    let mut state = fix_round_state(feature, round);
    if let Some(at) = round.ready.filter(|_| round.phase == FixPhase::Ready) {
        let written = format!(" \u{b7} written {}", crate::clock::ago(now, at));
        state.text.push_str(&written);
    }
    PageSection::File {
        name: format!("Fix plan: {}", round_label(round)),
        state,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                matches!(
                    o,
                    PanelOutcome::ReviewFixPlan
                        | PanelOutcome::MakeFixes
                        | PanelOutcome::AskFixPlan
                        | PanelOutcome::DropFixPlan
                )
            })
            .map(|(i, _)| i)
            .collect(),
    }
}

/// What open round `round` of `feature` is about and how it stands.
fn fix_round_state(feature: &Feature, round: &FixRound) -> CheckLine {
    let waits = (feature.runs.get(&Stage::Build)).is_some_and(|run| run.waits_to_send());
    // One colour per state, as on the rail and the sidebar's card.
    let tone = match (round.phase, round.stalled.is_some()) {
        _ if waits => CheckTone::Blocker,
        (_, true) => CheckTone::Failed,
        (FixPhase::Ready, _) => CheckTone::Blocker,
        _ => CheckTone::Note,
    };
    let said = match (waits, &round.stalled) {
        (true, _) => "your turn \u{b7} send the prompt".to_owned(),
        // One word for it everywhere, then why.
        (false, Some(why)) => format!("{} \u{b7} {why}", round.line()),
        (false, None) => round.line(),
    };
    line(format!("{} \u{b7} {said}", round.what), tone)
}

/// The card of the last fix round: which it was, what it made and when, at
/// `now`, its plan at hand.
pub(super) fn last_round_card(
    round: &FixRound,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> PageSection {
    let mut said = vec![round.what.clone(), round.made()];
    let ago = |at| crate::clock::ago(now, at);
    said.extend(round.ready.map(|at| format!("written {}", ago(at))));
    said.extend(round.ended.map(|at| format!("finished {}", ago(at))));
    PageSection::File {
        name: format!("Last fix plan: {}", round_label(round)),
        state: line(said.join(" \u{b7} "), CheckTone::Note),
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                matches!(o, PanelOutcome::ReviewFixPlan | PanelOutcome::ResendFixes)
            })
            .map(|(i, _)| i)
            .collect(),
    }
}

/// How Discover's or Plan's file stands in `feature`, as `ctx` found it.
/// A restarted agent leaves the last run's file be until it writes its
/// own, so the card doesn't say it is gone.
pub(super) fn file_line(feature: &Feature, stage: Stage, ctx: RowCtx) -> CheckLine {
    if feature.is_done(stage) {
        let text = match ctx.locked_by(feature, stage) {
            Some(_) => "approved \u{b7} locked",
            None => "approved",
        };
        return line(text, CheckTone::Ready);
    }
    let written = ctx.wrote(stage);
    match feature.runs.get(&stage) {
        // The user's turn: in its colour, as on the rail and the sidebar.
        Some(run) if run.ready.is_some() => line("ready for review", CheckTone::Blocker),
        // Its agent hasn't its prompt yet: nothing is being written.
        Some(run) if run.held => {
            let what = if written {
                "written"
            } else {
                "not written yet"
            };
            line(what, CheckTone::Note)
        }
        Some(run) if run.sent_back > 0 => {
            let n = run.sent_back;
            line(
                format!("sent back \u{d7}{n}, being written again"),
                CheckTone::Note,
            )
        }
        Some(run) if written && run.live() => line(
            "written \u{b7} the agent may still change it",
            CheckTone::Note,
        ),
        _ if written => line("written", CheckTone::Note),
        Some(_) => line("being written", CheckTone::Note),
        None => line("not written yet", CheckTone::Note),
    }
}

/// Verify's checks in `feature`, as `state` has them.
pub(super) fn checks_lines(feature: &Feature, state: &PanelState) -> Vec<CheckLine> {
    if let Some(what) = state.verify {
        return vec![line(inline(what), CheckTone::Note)];
    }
    last_checks(feature, state)
}

/// Verify's page below its status: a card for each check, under a heading
/// that counts them and says what the cards don't.
pub(super) fn verify_sections(
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    let cards: Vec<FindingCard> = (check_entries(feature, Stage::Verify, &state.plan).iter())
        .map(|entry| check_card(entry, feature, state, actions, now))
        .collect();
    let mut said = checks_lines(feature, state);
    if cards.is_empty() {
        return vec![lines("Checks", said)];
    }
    // Its status says the first.
    if !said.is_empty() {
        said.remove(0);
    }
    if let Some((place, _)) = &state.machine_line {
        said.insert(0, line(place.clone(), CheckTone::Note));
    }
    let mut out = vec![lines(&checks_heading(&cards), said)];
    out.extend(cards.into_iter().map(PageSection::Finding));
    out
}

/// How Verify's checks last went in `feature`.
fn last_checks(feature: &Feature, state: &PanelState) -> Vec<CheckLine> {
    match &feature.verify {
        Some(run) if run.passed == Some(true) => {
            vec![line(format!("run {} passed", run.n), CheckTone::Ready)]
        }
        Some(run) if run.passed == Some(false) => {
            let mut out = vec![line(format!("run {} failed", run.n), CheckTone::Failed)];
            // A check's card says how it failed; this says the rest.
            let on_card = |f: &String| {
                (run.checks.iter())
                    .any(|c| c.outcome.is_some() && f.starts_with(&format!("{}: `", c.repo)))
            };
            let failures = (run.failures.iter())
                .filter(|f| !on_card(f))
                .map(|f| line(f.clone(), CheckTone::Failed));
            out.extend(capped(failures.collect(), CheckTone::Failed));
            out
        }
        Some(run) => vec![line(format!("run {} running", run.n), CheckTone::Note)],
        None if !state.ctx.checks => {
            vec![line(
                "no checks for its repositories: set some, or Mark done by hand",
                CheckTone::Note,
            )]
        }
        None => vec![line("not run yet", CheckTone::Note)],
    }
}

/// The setup's section of `feature`'s page, as `state` has it: what runs,
/// or once it ended what went wrong and what it copied.
/// What the Manual test page says of `feature`'s sandbox, once it has one, and
/// what running in it doesn't keep apart from main.
pub(super) fn sandbox_sections(feature: &Feature, state: &PanelState) -> Vec<PageSection> {
    if let Some(what) = state.sandbox {
        return vec![lines("Sandbox", vec![line(inline(what), CheckTone::Note)])];
    }
    let mut out = Vec::new();
    let seen = state.sandbox_seen.as_ref();
    let Some(run) = feature.sandbox.as_ref() else {
        let text = match seen {
            Some(seen) => format!(
                "none on {}: starting one brings the feature here",
                seen.name
            ),
            // It goes where the page's button says, and copies what runs there.
            None if state
                .place
                .as_deref()
                .is_some_and(|place| place != "Runs here") =>
            {
                "the project, on a copy of the containers and data of the machine it runs on"
                    .to_owned()
            }
            None => "the project running here, on a copy of its containers and data".to_owned(),
        };
        return vec![lines("Sandbox", vec![line(text, CheckTone::Note)])];
    };
    if let Some(why) = &run.failed {
        return vec![lines(
            "Sandbox",
            vec![line(format!("failed: {why}"), CheckTone::Failed)],
        )];
    }
    let ports_text = |ports: &[u16]| -> String {
        let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
        ports.join(", ")
    };
    if run.alive() {
        let text = match seen {
            Some(seen) => format!("running on {}", seen.name),
            None => "running".to_owned(),
        };
        out.push(line(text, CheckTone::Ready));
        // What can hurt first, again: and what's open here says the rest.
        if let Some(seen) = seen.filter(|seen| !seen.taken.is_empty()) {
            let text = format!("taken here, so not open: {}", ports_text(&seen.taken));
            out.push(line(text, CheckTone::Blocker));
        }
        // Anything on the network reaches it: said before the rest.
        if let Some(lan) = &state.sandbox_lan {
            let text = match lan.open.is_empty() {
                true => format!("open to your network at {}, nothing listening yet", lan.ip),
                false => format!(
                    "open to your network at {}: {}",
                    lan.ip,
                    ports_text(&lan.open)
                ),
            };
            out.push(line(text, CheckTone::Blocker));
            if !lan.taken.is_empty() {
                let text = format!(
                    "taken here, so not on the network: {}",
                    ports_text(&lan.taken)
                );
                out.push(line(text, CheckTone::Blocker));
            }
        }
        let ports = run.listening();
        let here = seen.map_or(&[][..], |seen| seen.here.as_slice());
        if !here.is_empty() {
            let text = format!("open here on the same ports: {}", ports_text(here));
            out.push(line(text, CheckTone::Note));
        } else if !ports.is_empty() {
            out.push(line(
                format!("listening on {}", ports_text(&ports)),
                CheckTone::Note,
            ));
        }
    } else if let Some(seen) = seen {
        let asked = run.elsewhere.as_ref().is_some_and(|there| there.asked);
        let text = match (run.stopped, run.finished) {
            (true, _) if feature.writer.is_some() && feature.writer == run.machine => format!(
                "stopped on {}, its data kept: starting one here brings the feature here, with a new copy",
                seen.name
            ),
            (true, _) => format!("stopped on {}, its data kept", seen.name),
            (false, None) => format!("starting on {}", seen.name),
            (false, Some(_)) if asked => {
                format!("not running on {}: its network ended", seen.name)
            }
            (false, Some(_)) => format!("not known yet how it stands on {}", seen.name),
        };
        out.push(line(text, CheckTone::Note));
    } else if run.stopped {
        out.push(line("stopped, its data kept", CheckTone::Note));
    } else {
        out.push(line("not running: its network ended", CheckTone::Note));
    }
    // What can hurt first: a long list cuts its end.
    if !run.outward.is_empty() {
        let text = format!("can reach real services: {}", run.outward.join(", "));
        out.push(line(text, CheckTone::Blocker));
    }
    if !run.out_of_reach.is_empty() {
        let text = format!(
            "can't reach what runs outside its project: {}",
            run.out_of_reach.join(", ")
        );
        out.push(line(text, CheckTone::Blocker));
    }
    for name in &run.not_copied {
        let text = format!("not copied: {name}, which compose didn't start");
        out.push(line(text, CheckTone::Blocker));
    }
    for name in &run.not_there {
        let on = seen.map_or("that machine", |seen| seen.name.as_str());
        let text = format!("not copied: {name}, which doesn't run on {on}");
        out.push(line(text, CheckTone::Blocker));
    }
    out.push(line(
        "files, unix sockets and the internet are shared with main",
        CheckTone::Note,
    ));
    let mut sections = vec![lines("Sandbox", capped(out, CheckTone::Note))];
    let mut out = Vec::new();
    for forward in &run.forwards {
        let text = match seen {
            Some(seen) => format!(
                "{}: port {} inside it, 127.0.0.1:{} outside it on {}",
                forward.service, forward.main_port, forward.host_port, seen.name
            ),
            None => format!(
                "{}: port {} here, 127.0.0.1:{} from outside",
                forward.service, forward.main_port, forward.host_port
            ),
        };
        out.push(line(text, CheckTone::Note));
    }
    for name in &run.copied {
        out.push(line(format!("copied {name}"), CheckTone::Note));
    }
    for service in &run.fresh {
        out.push(line(format!("{service} started empty"), CheckTone::Note));
    }
    // All of them: the ports are what another tool connects to.
    if !out.is_empty() {
        sections.push(lines("Containers", out));
    }
    sections
}

/// The setup's section with its own buttons (Set up again, Setup log) beside
/// its caption, off the page's title, which holds what acts on the stage.
pub(super) fn setup_sections(
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let Some(section) = setup_section(feature, state) else {
        return Vec::new();
    };
    let held: Vec<usize> = (actions.iter().enumerate())
        .filter(|(_, (_, o))| matches!(o, PanelOutcome::SetUp | PanelOutcome::SetupLog))
        .map(|(i, _)| i)
        .collect();
    let mut out = vec![section];
    if !held.is_empty() {
        out.push(PageSection::Toolbar {
            groups: vec![(String::new(), held)],
        });
    }
    out
}

pub(super) fn setup_section(feature: &Feature, state: &PanelState) -> Option<PageSection> {
    if let Some(what) = state.setup {
        return Some(lines("Setup", vec![line(inline(what), CheckTone::Note)]));
    }
    let run = feature
        .setup
        .as_ref()
        .filter(|run| run.finished.is_some())?;
    let place = (state.setup_on.iter()).map(|name| line(format!("on {name}"), CheckTone::Note));
    let failed = (run.failed.iter()).map(|why| line(format!("failed: {why}"), CheckTone::Failed));
    let missed = (run.not_copied.iter())
        .map(|what| line(format!("couldn't copy {what}"), CheckTone::Failed));
    let copied = (run.copied.iter()).map(|what| line(format!("copied {what}"), CheckTone::Note));
    let ran = (run.ran.iter()).map(|what| line(format!("ran {what}"), CheckTone::Note));
    let mut out: Vec<_> = failed.chain(missed).chain(copied).chain(ran).collect();
    if out.is_empty() {
        out.push(line("nothing was missing", CheckTone::Note));
    }
    out.splice(0..0, place);
    Some(lines("Setup", capped(out, CheckTone::Note)))
}

/// The milestones of `stage`'s file (Build's: nothing ticks a plan's) by
/// name, as `probe` last read them; none while it has none.
pub(super) fn milestones_of(
    probe: Option<&Probe>,
    stage: Stage,
    at_work: bool,
) -> Option<PageSection> {
    let probe = probe?;
    let items = probe.docs.milestones.get(&stage)?;
    let running = items_running(items, &probe.docs.milestones_running, at_work);
    (!items.is_empty())
        .then(|| milestones_section(stage.label().to_owned(), items, None).running(running))
}

/// The plan's milestones on Build's page while its build log lists none,
/// none ticked, with what happens to them: before its agent `started`, and
/// after, as the log is still empty.
pub(super) fn planned_milestones(probe: Option<&Probe>, started: bool) -> Option<PageSection> {
    let docs = &probe?.docs;
    let items = [Stage::PlanReview, Stage::Plan]
        .iter()
        .find_map(|s| docs.milestones.get(s).filter(|m| !m.is_empty()))?;
    let items: Vec<(bool, String)> = items.iter().map(|(_, t)| (false, t.clone())).collect();
    let said = match started {
        false => {
            "from the plan: the Build agent builds them in order and ticks each in its build log, then Verify runs the checks"
        }
        true => {
            "from the plan: the build log lists none yet, so nothing is ticked; the Build agent's tab says what it is at"
        }
    };
    Some(milestones_section(
        Stage::Build.label().to_owned(),
        &items,
        Some(line(said, CheckTone::Note)),
    ))
}

/// Which of `items` blink: with an agent `at_work`, those it says it is on
/// (`marked`), else the first it hasn't done.
pub(super) fn items_running(
    items: &[(bool, String)],
    marked: &[usize],
    at_work: bool,
) -> Vec<usize> {
    match (at_work, marked.is_empty()) {
        (false, _) => Vec::new(),
        (true, false) => marked.to_vec(),
        (true, true) => items
            .iter()
            .position(|(done, _)| !done)
            .into_iter()
            .collect(),
    }
}

impl PageSection {
    /// This group of Build's list with `items` in `running` blinking.
    fn running(mut self, items: Vec<usize>) -> Self {
        if let Self::Milestones { running, .. } = &mut self {
            *running = items;
        }
        self
    }
}

/// `items` as a group of Build's list, called `label`, saying `state` of how
/// it stands.
fn milestones_section(
    label: String,
    items: &[(bool, String)],
    state: Option<CheckLine>,
) -> PageSection {
    PageSection::Milestones {
        label,
        state,
        done: items.iter().filter(|(done, _)| *done).count(),
        total: items.len(),
        items: items.to_vec(),
        running: Vec::new(),
        buttons: Vec::new(),
    }
}

/// The rounds of `feature` whose plans can be shown, as `probe` found them:
/// the latest's is `fix_round::FILE`, those before it put aside.
fn round_plans(feature: &Feature, probe: Option<&Probe>) -> Vec<u32> {
    let Some(docs) = probe.map(|p| &p.docs) else {
        return Vec::new();
    };
    let latest = (docs.plan && feature.fix_rounds > 0).then_some(feature.fix_rounds);
    let mut out: Vec<u32> = docs.plans.iter().copied().chain(latest).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The stages whose pages list the fix rounds they asked for as cards.
pub(super) const ROUND_CARD_STAGES: [Stage; 5] = [
    Stage::Build,
    Stage::Verify,
    Stage::AiReview,
    Stage::CodeReview,
    Stage::ManualTest,
];

/// The fix rounds `stage`'s page of `feature` shows as cards, newest first:
/// none but on `ROUND_CARD_STAGES`. Build's page lists every round, those
/// older than their record by the number `probe` found in the build log. Code review's and Verify's rounds from
/// before rounds were kept whole stand in by their number, from the comments
/// or the failures they took.
pub(super) fn card_rounds(feature: &Feature, stage: Stage, probe: Option<&Probe>) -> Vec<FixRound> {
    if !ROUND_CARD_STAGES.contains(&stage) {
        return Vec::new();
    }
    // Build's page lists every round: they all go to its agent.
    let mut out: Vec<FixRound> = match stage {
        Stage::Build => (feature.fix_round.iter())
            .chain(&feature.last_fix_round)
            .chain(feature.past_rounds.iter().rev())
            .cloned()
            .collect(),
        _ => feature.rounds_of(stage).into_iter().cloned().collect(),
    };
    let mut seen = BTreeSet::new();
    out.retain(|r| seen.insert(r.n));
    // A round another stage asked for that this page holds the buttons of:
    // Verify's fixes sent from Manual test, a round the feature came back for.
    let held = fix_round_on(feature, stage).or_else(|| last_round_on(feature, stage));
    if let Some(round) = held.filter(|r| !out.iter().any(|o| o.n == r.n)) {
        out.push(round.clone());
    }
    let stub = |n: u32, what: &str, asked: Stage, dropped: Option<u64>| FixRound {
        n,
        what: what.into(),
        input: PathBuf::new(),
        stage: feature.fix_asked.get(&n).copied().unwrap_or(asked),
        phase: FixPhase::Fixing,
        sent_back: 0,
        asked: 0,
        ready: None,
        stalled: None,
        give_back: Default::default(),
        items: Vec::new(),
        done: 0,
        ended: None,
        trail: feature::fix_round::Trail {
            dropped,
            ..Default::default()
        },
    };
    let comments = (feature.comment_fixes.iter())
        .map(|fix| (fix.round, "Code review's comments", Stage::CodeReview, None));
    let failures = (feature.verify_fixes.iter()).map(|fix| {
        let dropped = fix.dropped.then_some(fix.at);
        (fix.round, "Verify's failures", Stage::Verify, dropped)
    });
    let kept: Vec<(u32, &str, Stage, Option<u64>)> =
        match stage {
            Stage::CodeReview => comments.collect(),
            Stage::Verify => failures.collect(),
            // Every round it heard of, those from before they were kept whole
            // by who asked.
            Stage::Build => (comments.chain(failures))
                .chain(feature.fix_asked.keys().map(|n| (*n, "fixes", stage, None)))
                .chain((probe.iter()).flat_map(|p| {
                    (p.docs.fix_rounds.iter()).map(|(n, _)| (*n, "fixes", stage, None))
                }))
                .collect(),
            _ => Vec::new(),
        };
    for (n, what, asked, dropped) in kept {
        if !out.iter().any(|r| r.n == n) {
            out.push(stub(n, what, asked, dropped));
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.n));
    out
}

/// Whether `stage`'s page of `feature` has fix round cards that fold: more
/// than one, or one that isn't open. Alt+E folds them.
pub(super) fn folds_round_cards(feature: &Feature, stage: Stage) -> bool {
    let cards = card_numbers(feature, stage);
    cards.len() > 1
        || cards
            .first()
            .is_some_and(|n| newest_round(feature, stage) != Some(*n))
}

/// The numbers of the cards of rounds `stage`'s page of `feature` lists,
/// newest first: its fix rounds, or Discover's and Plan's drafts.
pub(super) fn card_numbers(feature: &Feature, stage: Stage) -> Vec<u32> {
    match stage {
        Stage::Discover | Stage::Plan => (feature::drafts::of(&feature.drafts, stage).iter())
            .map(|d| d.k)
            .collect(),
        _ => card_rounds(feature, stage, None)
            .iter()
            .map(|r| r.n)
            .collect(),
    }
}

/// The buttons of the fix round cards on `stage`'s page of `feature`, by
/// `probe`, newest round first: what each changed, the plan of one over (the
/// open and the last have theirs on their own buttons), and folding.
fn round_card_actions(
    feature: &Feature,
    stage: Stage,
    probe: Option<&Probe>,
) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Plain,
    };
    let rounds = card_rounds(feature, stage, probe);
    let plans = round_plans(feature, probe);
    let newest = newest_round(feature, stage);
    let mut out = Vec::new();
    for round in &rounds {
        if has_diff(round) {
            out.push((button("Diff"), PanelOutcome::RoundDiff(round.n)));
        }
        if plans.contains(&round.n) && !has_own_plan_button(feature, round) {
            out.push((button(OPEN_PLAN), PanelOutcome::OpenRoundPlan(round.n)));
        }
        if rounds.len() > 1 || newest != Some(round.n) {
            out.push((button("Show"), PanelOutcome::FoldFixRound(round.n)));
        }
    }
    out
}

/// Whether `round`'s code was read before its fixes went: its diff can show
/// what they changed.
fn has_diff(round: &FixRound) -> bool {
    round.trail.went.is_some() && !round.trail.from.is_empty()
}

/// Whether `round` of `feature` is the open one or the last, whose card has
/// the plan's own buttons (Review, Open).
fn has_own_plan_button(feature: &Feature, round: &FixRound) -> bool {
    let n = Some(round.n);
    feature.status == Status::Open
        && (feature.fix_round.as_ref().map(|r| r.n) == n
            || (feature.fix_round.is_none() && feature.last_fix_round.as_ref().map(|r| r.n) == n))
}

/// The fix round whose card `stage`'s page of `feature` shows open first:
/// the open round if the page lists it, else on Build's the newest. The
/// other pages keep rounds over in their history, folded.
pub(super) fn newest_round(feature: &Feature, stage: Stage) -> Option<u32> {
    if matches!(stage, Stage::PlanReview | Stage::Discover | Stage::Plan) {
        return card_numbers(feature, stage).first().copied();
    }
    let rounds = card_rounds(feature, stage, None);
    let open = feature.fix_round.as_ref().map(|r| r.n);
    let open = open.filter(|n| rounds.iter().any(|r| r.n == *n));
    match stage {
        Stage::Build => open.or(rounds.first().map(|r| r.n)),
        _ => open,
    }
}

/// Whether round `n`'s card shows open: the newest's does, the others are
/// folded, but those the user `flipped`.
pub(super) fn fix_round_open(newest: Option<u32>, n: u32, flipped: &BTreeSet<u32>) -> bool {
    (newest == Some(n)) != flipped.contains(&n)
}

/// How fix round `round` stands, in a few words, at `now`: "fixes made 12m
/// ago", "making fixes · 1 of 3 made".
pub(crate) fn round_state_words(round: &FixRound, now: u64) -> String {
    match (round.trail.dropped, round.ended) {
        (Some(at), _) => format!("dropped {}", crate::clock::ago(now, at)),
        (None, Some(at)) => format!("{} {}", round.made(), crate::clock::ago(now, at)),
        (None, None) => round.stalled.clone().unwrap_or_else(|| round.line()),
    }
}

/// How long from `from` to `to`, in a few words: "12 min", "1 h 5 min".
/// Not `clock::took` ("2m 5s") on purpose: a feature's times read in minutes.
pub(super) fn took(from: u64, to: u64) -> String {
    let mins = to.saturating_sub(from).div_ceil(60);
    match (mins / 60, mins % 60) {
        (0, m) => format!("{} min", m.max(1)),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The cards of `feature`'s fix rounds `stage` asked for, newest first, under
/// a caption counting them; none if it asked for none.
pub(super) fn round_cards(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    let cards = round_card_list(feature, stage, state, actions, now);
    let caption = match cards.len() {
        0 => return Vec::new(),
        1 => "Fix round".to_owned(),
        n => format!("Fix rounds \u{b7} {n}"),
    };
    let cards = cards.into_iter().map(|(_, card)| PageSection::Round(card));
    std::iter::once(lines(&caption, Vec::new()))
        .chain(cards)
        .collect()
}

/// Each of `feature`'s fix rounds `stage` asked for, newest first, with its
/// card.
fn round_card_list(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<(FixRound, chrome::RoundCard)> {
    let rounds = card_rounds(feature, stage, state.probe);
    let newest = newest_round(feature, stage);
    // A criterion or a finding says how it stands now on the newest round
    // that asked it. Code review's comments are each round's own.
    let mut told: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for round in rounds {
        let open = fix_round_open(newest, round.n, &state.fix_rounds_flipped);
        let asked = asked_of(feature, &round, state.probe);
        let mut card = fix_card(feature, &round, state, actions, now, &asked, &told);
        card.open = open;
        // Build's page lists rounds of every stage: each its own.
        if stage != Stage::CodeReview && stage != Stage::Build {
            let key = |a: feature::fix_round::Asked| if a.id.is_empty() { a.title } else { a.id };
            told.extend(asked.into_iter().map(key));
        }
        out.push((round, card));
    }
    out
}

/// The run of `stage` that asked for fix round `round` of `feature`, if it
/// asked it and that is known.
fn round_run(feature: &Feature, stage: Stage, round: &FixRound) -> Option<u32> {
    let asker = feature::fix_round::asker(round.n, &feature.verify_fixes, &feature.fix_asked)
        .unwrap_or(round.stage);
    (asker == stage)
        .then(|| feature::fix_round::run_of(round, &feature.verify_fixes))
        .flatten()
}

/// The fix round cards of `stage`'s page of `feature`, but Build's, in
/// three places: the open round's, on top; those over that the run on the
/// page asked for, under its work; and the history, newest first, each of
/// AI review's earlier runs followed by the rounds it asked for, drawn in
/// under it.
pub(super) fn round_sections(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> (Vec<PageSection>, Vec<PageSection>, Vec<PageSection>) {
    let (mut top, mut under, mut over) = (Vec::new(), Vec::new(), Vec::new());
    if stage == Stage::Build || !ROUND_CARD_STAGES.contains(&stage) {
        return (top, under, Vec::new());
    }
    let open = feature.fix_round.as_ref().map(|r| r.n);
    let current = match stage {
        Stage::AiReview => feature.review.as_ref().map(|r| r.n),
        Stage::Verify => feature.verify.as_ref().map(|r| r.n),
        _ => None,
    };
    for (round, card) in round_card_list(feature, stage, state, actions, now) {
        let run = round_run(feature, stage, &round);
        match () {
            _ if open == Some(round.n) => top.push(PageSection::Round(card)),
            _ if run.is_some() && run == current => under.push(PageSection::Round(card)),
            _ => over.push((run, card)),
        }
    }
    // Newest run first, a run's rounds newest first; those of no known run
    // after every run.
    over.sort_by_key(|(run, _)| std::cmp::Reverse(run.unwrap_or(0)));
    let mut runs = earlier_reviews(feature, stage).peekable();
    let mut history = Vec::new();
    let mut last = None;
    for (run, mut card) in over {
        while let Some(review) = runs.next_if(|r| r.n >= run.unwrap_or(0)) {
            history.extend(review_page::archive(feature, review, state, actions));
            last = Some(review.n);
        }
        card.nested = run.is_some() && run == last;
        history.push(PageSection::Round(card));
    }
    for review in runs {
        history.extend(review_page::archive(feature, review, state, actions));
    }
    if !history.is_empty() {
        history.insert(0, lines("History", Vec::new()));
    }
    (top, under, history)
}

/// The card of fix round `round` of `feature`, at `now`: how it stands, who
/// made its fixes and when, what it was asked, its fixes, what they changed,
/// and its buttons in `actions`. What it was asked says how it stands now,
/// unless a newer round asked it too (`told`).
#[allow(clippy::too_many_arguments)]
fn fix_card(
    feature: &Feature,
    round: &FixRound,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
    asked: &[feature::fix_round::Asked],
    told: &BTreeSet<String>,
) -> chrome::RoundCard {
    let is_open = feature.status == Status::Open
        && feature.fix_round.as_ref().is_some_and(|r| r.n == round.n);
    let is_last = !is_open && has_own_plan_button(feature, round);
    let (dot, state_line) = match (is_open, round.trail.dropped, round.ended) {
        (true, ..) => {
            let line = fix_round_state(feature, round);
            let dot = match (round.phase, round.stalled.is_some()) {
                (_, true) => Dot::Blocked,
                (FixPhase::Ready, false) => Dot::Idle,
                _ => Dot::Working,
            };
            (dot, line)
        }
        (false, Some(_), _) => (Dot::Unknown, line("dropped", CheckTone::Note)),
        (false, None, _) => (Dot::Done, line(round.made(), CheckTone::Ready)),
    };
    let mut meta = Vec::new();
    // Who asked it and who makes it: the round is followed on the asker's
    // page, its fixes are the Build agent's work.
    let asker = feature::fix_round::asker(round.n, &feature.verify_fixes, &feature.fix_asked)
        .unwrap_or(round.stage);
    let asked_by = Some(format!("asked by {}", asker.label()));
    // Verify's says its run below.
    let run = (asker == Stage::AiReview)
        .then(|| feature::fix_round::run_of(round, &feature.verify_fixes))
        .flatten();
    meta.extend(run.map(|k| format!("from run {k}")));
    let verify = (feature.verify_fixes.iter()).find(|fix| fix.round == round.n);
    if !round.trail.agent.is_empty() {
        meta.push(match round.trail.model.is_empty() {
            true => format!("made by the Build agent ({})", round.trail.agent),
            false => format!(
                "made by the Build agent ({}, {})",
                round.trail.agent, round.trail.model
            ),
        });
    } else if is_open && verify.is_none() {
        // Its plan still pending: the Build agent makes them all the same.
        let build = (feature.runs.get(&Stage::Build)).filter(|r| !r.agent.is_empty());
        meta.extend(build.map(|r| format!("made by the Build agent ({})", r.agent)));
    }
    if asked.is_empty() {
        meta.push(round.what.clone());
    }
    if let Some(fix) = verify {
        if round.trail.agent.is_empty() && !fix.agent.is_empty() {
            meta.push(format!("made by the Build agent ({})", fix.agent));
        }
        meta.push(format!(
            "run {} failed {}",
            fix.run,
            crate::clock::ago(now, fix.at)
        ));
    }
    // A round kept only by its number has no times.
    match (asked_by, round.asked > 0) {
        (Some(by), true) => meta.insert(0, format!("{by} {}", crate::clock::ago(now, round.asked))),
        (Some(by), false) => meta.insert(0, by),
        (None, true) => meta.push(format!("asked {}", crate::clock::ago(now, round.asked))),
        (None, false) => {}
    }
    if let Some(at) = round.ready.filter(|_| round.trail.went.is_none()) {
        meta.push(format!("plan written {}", crate::clock::ago(now, at)));
    }
    if let Some(went) = round.trail.went {
        meta.push(format!("fixes went {}", crate::clock::ago(now, went)));
    }
    if let Some(over) = round.over_at() {
        if round.trail.dropped.is_none() {
            meta.push(format!("finished {}", crate::clock::ago(now, over)));
        }
        if let Some(went) = round.trail.went {
            meta.push(format!("took {}", took(went, over)));
        }
    }
    // Verify's next run says whether the fixes did it.
    if let Some((n, passed)) = verify.and_then(|fix| fix.next) {
        let how = if passed { "passed" } else { "failed" };
        meta.push(format!("run {n} {how}"));
    }
    let asked = (asked.iter())
        .map(|a| chrome::RoundAsked {
            id: a.id.clone(),
            title: a.title.clone(),
            note: match a.shots {
                0 => a.note.clone(),
                n => format!("{} ({})", a.note, plural(n, "screenshot")),
            },
            now: {
                // A check's failure has no id: its words say which.
                let key = if a.id.is_empty() { &a.title } else { &a.id };
                (!told.contains(key))
                    .then(|| asked_now(feature, round, key, is_open))
                    .flatten()
            },
        })
        .collect();
    let listed = (state.probe.map(|p| &p.docs.fix_rounds))
        .and_then(|rounds| rounds.iter().find(|(n, _)| *n == round.n))
        .map(|(_, items)| items.clone());
    let fixes = listed.unwrap_or_else(|| {
        (round.planned().iter())
            .map(|item| (false, item.clone()))
            .collect()
    });
    let at_work = is_open
        && round.phase == FixPhase::Fixing
        && round.stalled.is_none()
        && matches!(state.agent_status, Some(AgentStatus::Working));
    let running = items_running(&fixes, &[], at_work);
    let names = |key: &str| (feature.repo(key)).map_or_else(|| key.to_owned(), |r| r.name.clone());
    let changed = match state.probe.and_then(|p| p.rounds.get(&round.n)) {
        _ if round.trail.went.is_none() => Vec::new(),
        _ if round.trail.from.is_empty() => {
            vec!["not known: the code wasn't read before the fixes went".to_owned()]
        }
        Some(Err(why)) => vec![format!("couldn't be read: {why}")],
        Some(Ok(stats)) if stats.is_empty() => vec!["no code yet".to_owned()],
        Some(Ok(stats)) => (stats.iter())
            .map(|s| {
                let mut parts = vec![plural(s.commits, "commit"), plural(s.stat.files, "file")];
                parts.push(format!("+{} \u{2212}{}", s.stat.added, s.stat.removed));
                match feature.many_repos() {
                    true => format!("{} \u{b7} {}", names(&s.repo), parts.join(" \u{b7} ")),
                    false => parts.join(" \u{b7} "),
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let held = |o: &PanelOutcome| match o {
        PanelOutcome::RoundDiff(n)
        | PanelOutcome::OpenRoundPlan(n)
        | PanelOutcome::FoldFixRound(n) => *n == round.n,
        PanelOutcome::MakeFixes | PanelOutcome::AskFixPlan | PanelOutcome::DropFixPlan => is_open,
        // Open, or over and the last: its plan.
        PanelOutcome::ReviewFixPlan => is_open || is_last,
        PanelOutcome::ResendFixes => is_last,
        _ => false,
    };
    // The round's own first, what it shows after, folding last.
    let rank = |o: &PanelOutcome| match o {
        PanelOutcome::FoldFixRound(_) => 2,
        PanelOutcome::RoundDiff(_) | PanelOutcome::OpenRoundPlan(_) => 1,
        _ => 0,
    };
    let mut buttons: Vec<usize> = (actions.iter().enumerate())
        .filter(|(_, (_, o))| held(o))
        .map(|(i, _)| i)
        .collect();
    buttons.sort_by_key(|&i| (rank(&actions[i].1), i));
    let label = round_label(round);
    chrome::RoundCard {
        label,
        dot,
        state: state_line,
        meta: meta.join(" \u{b7} "),
        asked,
        asked_caption: "Asked to fix",
        fixes,
        running,
        changed,
        open: true,
        nested: false,
        buttons,
    }
}

/// What fix round `round` of `feature` was asked to fix: as it kept it when
/// sent, else, for a round from before that, Code review's comments it took
/// as `probe` read them, or AI review's findings it took by their ids.
pub(super) fn asked_of(
    feature: &Feature,
    round: &FixRound,
    probe: Option<&Probe>,
) -> Vec<feature::fix_round::Asked> {
    use feature::fix_round::{Asked, GiveBack};
    if !round.trail.asked.is_empty() {
        return round.trail.asked.clone();
    }
    match feature
        .fix_asked
        .get(&round.n)
        .copied()
        .unwrap_or(round.stage)
    {
        Stage::CodeReview => (round_comments(feature, round.n, probe).iter())
            .map(|c| Asked {
                id: c.flag.as_ref().map(|f| f.id.clone()).unwrap_or_default(),
                title: note_card(c, feature.many_repos(), &[]).place,
                note: c.text.clone(),
                shots: 0,
            })
            .collect(),
        Stage::AiReview => match &round.give_back {
            GiveBack::Findings { ids, .. } => review_import::asked(feature.review.as_ref(), ids),
            _ => Vec::new(),
        },
        // What failed, the first with why: one has no id but its check.
        Stage::Verify => (feature.verify_fixes.iter().find(|fix| fix.round == round.n))
            .map(|fix| {
                (fix.failures.iter().enumerate())
                    .map(|(i, failure)| Asked {
                        id: String::new(),
                        title: failure.clone(),
                        note: match i {
                            0 => fix.why.join(" "),
                            _ => String::new(),
                        },
                        shots: 0,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The comments Code review's fix round `n` of `feature` took, as `probe` last
/// read them: the latest round's are with the diff's, flagged sent.
fn round_comments<'a>(feature: &Feature, n: u32, probe: Option<&'a Probe>) -> &'a [Comment] {
    let Some(docs) = probe.map(|p| &p.docs) else {
        return &[];
    };
    match docs.old_notes.get(&n) {
        Some(old) => old,
        None if latest_comment_round(feature) == Some(n) => {
            let own = docs.notes.iter().take_while(|c| c.flag.is_none()).count();
            &docs.notes[own..]
        }
        None => &[],
    }
}

/// Code review's latest fix round in `feature`, as `Docs::with_comment_fixes`
/// takes it.
fn latest_comment_round(feature: &Feature) -> Option<u32> {
    (feature.comment_fixes.last().map(|fix| fix.round)).or_else(|| {
        (feature.fix_round.as_ref())
            .filter(|round| round.stage == Stage::CodeReview)
            .map(|round| round.n)
    })
}

/// How `id`, which fix round `round` of `feature` was asked to fix, stands
/// now, by the stage that asked: a criterion tried again, a finding the next
/// review saw, a comment whose fix was made.
pub(super) fn asked_now(
    feature: &Feature,
    round: &FixRound,
    id: &str,
    open: bool,
) -> Option<CheckLine> {
    match feature
        .fix_asked
        .get(&round.n)
        .copied()
        .unwrap_or(round.stage)
    {
        Stage::ManualTest => criterion_now(feature, round, id, open),
        Stage::AiReview => {
            let review = feature.review.as_ref()?;
            let found = review.find(id)?;
            Some(match found.state {
                FindingState::Fixed => line("fixed", CheckTone::Ready),
                FindingState::Dismissed => line("dismissed", CheckTone::Note),
                FindingState::Sent if found.fix_made() => line("fix made", CheckTone::Ready),
                FindingState::Sent if open => line(sent_now(round), CheckTone::Note),
                FindingState::Sent => line("not marked made", CheckTone::Note),
                FindingState::Open | FindingState::Fix => line("raised again", CheckTone::Blocker),
                FindingState::Posted => line("on GitHub", CheckTone::Note),
            })
        }
        Stage::CodeReview => {
            let fix = feature
                .comment_fixes
                .iter()
                .find(|fix| fix.round == round.n);
            Some(
                match fix.is_some_and(|fix| fix.made.iter().any(|m| m == id)) {
                    true => line("fix made", CheckTone::Ready),
                    false if open => line(sent_now(round), CheckTone::Note),
                    false => line("not marked made", CheckTone::Note),
                },
            )
        }
        Stage::Verify => check_now(feature, round, id, open),
        _ => None,
    }
}

/// How the check whose failure `failure` (`VerifyRun::failures`' words)
/// fix round `round` of `feature` was asked to fix stands now: as a run of
/// Verify since that round failed says it.
fn check_now(feature: &Feature, round: &FixRound, failure: &str, open: bool) -> Option<CheckLine> {
    let fix = feature
        .verify_fixes
        .iter()
        .find(|fix| fix.round == round.n)?;
    let run = feature.verify.as_ref()?;
    let check = (run.checks.iter())
        .find(|c| failure.starts_with(&format!("{}: `{}`", c.repo, c.command)))?;
    if run.n <= fix.run || check.run <= fix.run {
        return open.then(|| line(sent_now(round), CheckTone::Note));
    }
    Some(match &check.outcome {
        None => line(format!("running in run {}", check.run), CheckTone::Note),
        Some(o) if o.passed() => line(format!("passed in run {}", check.run), CheckTone::Ready),
        Some(_) => line(
            format!("failed again in run {}", check.run),
            CheckTone::Failed,
        ),
    })
}

/// How criterion `id`, which fix round `round` of `feature` was asked to fix,
/// stands now: tried again since, its fix made, or not tried since.
fn criterion_now(feature: &Feature, round: &FixRound, id: &str, open: bool) -> Option<CheckLine> {
    let check =
        (feature.manual.as_ref()).and_then(|test| test.checks.iter().rev().find(|k| k.id == id));
    let since = |k: &feature::manual::Check| round.over_at().is_some_and(|over| k.at >= over);
    Some(match check {
        Some(k) if k.state == feature::manual::Verdict::Pass => {
            line("passed since", CheckTone::Ready)
        }
        Some(k) if k.fix_made() => line("fix made: try it again", CheckTone::Note),
        Some(k) if since(k) && !k.sent => line("failed again", CheckTone::Failed),
        Some(_) if open => line(sent_now(round), CheckTone::Note),
        _ => line("not tried since", CheckTone::Note),
    })
}

/// What an item open round `round` was asked to fix is while it goes: "in
/// fix plan" while its plan is written or waits, then "being fixed".
pub(super) fn sent_now(round: &FixRound) -> &'static str {
    review_import::sent_word(
        false,
        feature::fix_round::planning(Some(round), round.stage),
    )
}

/// The buttons of the fix round cards on `stage`'s page, by `probe`: on the
/// pages that list them only.
pub(super) fn round_plan_actions(
    feature: &Feature,
    stage: Stage,
    probe: Option<&Probe>,
) -> Vec<(PageButton, PanelOutcome)> {
    match ROUND_CARD_STAGES.contains(&stage) {
        true => round_card_actions(feature, stage, probe),
        false if stage == Stage::PlanReview => plan_changes_action(probe),
        false if stage == Stage::Ship => ship_base_actions(feature, probe),
        false if matches!(stage, Stage::Discover | Stage::Plan) => {
            draft_card_actions(feature, stage)
        }
        false => Vec::new(),
    }
}

/// Whether repository `repo`'s pull request was merged on GitHub, as `probe`
/// last read it.
pub(crate) fn pr_merged(probe: Option<&Probe>, repo: &str) -> bool {
    (probe.and_then(|p| p.prs.as_ref()))
        .and_then(|prs| prs.get(repo))
        .is_some_and(|state| state.as_ref().is_ok_and(|s| s.state == "MERGED"))
}

/// Whether `outcome` undoes a push or pull request whose pull request was
/// merged on GitHub: closing it then undoes nothing, so it isn't offered.
pub(super) fn undoes_merged(outcome: &PanelOutcome, probe: Option<&Probe>) -> bool {
    matches!(outcome, PanelOutcome::UndoShip(repo, step)
        if *step != ShipStep::Merge && *step != ShipStep::PushBase && pr_merged(probe, repo))
}

/// The buttons of Ship's cards that go by how each branch stands against its
/// base, as `probe` read it: taking a base in that went on, merged or
/// rebased.
pub(super) fn ship_base_actions(
    feature: &Feature,
    probe: Option<&Probe>,
) -> Vec<(PageButton, PanelOutcome)> {
    let mut out = Vec::new();
    if !feature.ships() || feature.squash_going() {
        return out;
    }
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    // An agent at work on conflicts: its plan, and stopping it.
    if let Some(r) = feature.shipping.resolving.as_ref() {
        let found = probe.and_then(|p| p.resolution.as_ref());
        if r.planning && found.is_some_and(|f| f.plan) {
            out.push((
                button("Review plan", ButtonStyle::Plain),
                PanelOutcome::ReviewConflictPlan,
            ));
            let first = r.repos.first().map_or_else(String::new, |t| t.repo.clone());
            out.push((
                button("Resolve as planned", ButtonStyle::Primary),
                PanelOutcome::ResolveConflicts(first, false),
            ));
        }
        out.push((
            button("Stop\u{2026}", ButtonStyle::Quiet),
            PanelOutcome::StopResolving,
        ));
        return out;
    }
    // Its squash waits for its base to be pulled: that, first.
    let repos = (probe.and_then(|p| p.ship.as_ref())).map_or(&[][..], |c| c.repos.as_slice());
    for repo in repos {
        if repo.pulls_base() && ship::repo_plan(feature, repo).ticked(ShipStep::Merge) {
            out.push((
                button(&format!("Pull {}", repo.base), ButtonStyle::Primary),
                PanelOutcome::PullBase(repo.key.clone()),
            ));
        }
    }
    let bases = probe.map(|p| &p.bases);
    // One agent takes every conflicting repository on: each card says so.
    let conflicting = (bases.into_iter().flatten())
        .filter(|(_, state)| !state.conflicts.is_empty())
        .count();
    let (resolve, plan) = match conflicting {
        1 => ("Resolve with agent", "Plan resolution"),
        _ => ("Resolve all with agent", "Plan resolution of all"),
    };
    for (key, state) in bases.into_iter().flatten() {
        if !state.conflicts.is_empty() {
            // Ship is no default while they conflict: this is.
            out.push((
                button(resolve, ButtonStyle::Primary),
                PanelOutcome::ResolveConflicts(key.clone(), false),
            ));
            out.push((
                button(plan, ButtonStyle::Quiet),
                PanelOutcome::ResolveConflicts(key.clone(), true),
            ));
            continue;
        }
        if !state.clean() {
            continue;
        }
        let base = feature.repo(key).map_or("the base", |r| r.base.as_str());
        let merge = PageButton {
            label: format!("Merge {base} in\u{2026}"),
            style: ButtonStyle::Plain,
        };
        let rebase = PageButton {
            label: "Rebase\u{2026}".to_owned(),
            style: ButtonStyle::Quiet,
        };
        out.push((
            merge,
            PanelOutcome::UpdateBase(key.clone(), feature::UpdateWay::Merge),
        ));
        // Over merges it would stop on what they settled: merge again.
        if !state.merges {
            out.push((
                rebase,
                PanelOutcome::UpdateBase(key.clone(), feature::UpdateWay::Rebase),
            ));
        }
    }
    out
}

/// A key of a button on Ship's page (checklist K4): the chord, as the
/// footer shows it, what it does in the footer's words, and what the button
/// it presses asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ShipKey {
    pub(super) chord: &'static str,
    pub(super) what: String,
    pub(super) outcome: PanelOutcome,
}

/// The repository of Ship's row the keyboard is on, if it is one's.
pub(super) fn ship_row_repo(panel: &FeaturePanel, ships: &[ShipRow]) -> Option<String> {
    if panel.focus != TabArea::Findings {
        return None;
    }
    match ships.get(panel.finding)? {
        ShipRow::Route { repo, .. } | ShipRow::Step { repo, .. } => Some(repo.clone()),
        ShipRow::After(_) => None,
    }
}

/// The repository button `outcome` of Ship's page acts on; none for the
/// page's own.
pub(super) fn ship_outcome_repo(feature: &Feature, outcome: &PanelOutcome) -> Option<String> {
    match outcome {
        PanelOutcome::UpdateBase(repo, _) | PanelOutcome::UndoShip(repo, _) => Some(repo.clone()),
        PanelOutcome::ResolveConflicts(repo, _) if feature.shipping.resolving.is_none() => {
            Some(repo.clone())
        }
        PanelOutcome::OpenPr(url) => (feature.shipping.prs.iter())
            .find(|pr| pr.url == *url)
            .map(|pr| pr.repo.clone()),
        // A take on conflicts is the page's: it spans their repositories.
        _ => None,
    }
}

/// `hints` with the keys of Ship's buttons in, before F1, in place of any
/// the page had on the same chord.
pub(super) fn with_ship_hints(
    mut hints: Vec<(String, String)>,
    panel: &FeaturePanel,
    feature: &Feature,
    stage: Stage,
    (ctx, probe): (RowCtx, Option<&Probe>),
    ships: &[ShipRow],
) -> Vec<(String, String)> {
    let quiet = panel.keys || panel.note.is_some() || panel.focus == TabArea::Repos;
    if stage != Stage::Ship || quiet {
        return hints;
    }
    // A pull request's review submits instead: its own keys.
    if feature.pull().is_some() {
        let keys = crate::key_table::hints(keys::SUBMIT_KEYS, feature, keys::submit_said);
        hints.retain(|(chord, _)| !keys.iter().any(|(k, _)| k == chord));
        let at = (hints.iter())
            .position(|(chord, _)| chord == "F1")
            .unwrap_or(hints.len());
        hints.splice(at..at, keys.into_iter().map(|(k, w)| (k.to_owned(), w)));
        return hints;
    }
    let repo = ship_row_repo(panel, ships);
    let keys = ship_keys(feature, ctx, probe, repo.as_deref());
    hints.retain(|(chord, _)| !keys.iter().any(|k| k.chord == chord));
    let at = (hints.iter())
        .position(|(chord, _)| chord == "F1")
        .unwrap_or(hints.len());
    hints.splice(
        at..at,
        keys.into_iter().map(|k| (k.chord.to_owned(), k.what)),
    );
    hints
}

/// How repository `key`'s branch stands against its base, as `probe` read
/// it, for its card.
pub(super) fn ship_base_line(
    feature: &Feature,
    probe: Option<&Probe>,
    key: &str,
) -> Option<CheckLine> {
    // Its conflicts taken on: how that goes.
    let resolving = feature.shipping.resolving.as_ref();
    if let Some((r, t)) = resolving.and_then(|r| Some((r, r.taking(key)?))) {
        let who = r.agent.clone();
        let found = probe.and_then(|p| p.resolution.as_ref());
        let mine = found.and_then(|f| f.of(key));
        let files = t.files.join(", ");
        return Some(match (r.planning, found) {
            (_, Some(f)) if f.questions => line(
                format!(
                    "{who} has questions on the conflicts: see conflicts-{}-questions.md",
                    r.n
                ),
                CheckTone::Blocker,
            ),
            (true, Some(f)) if f.plan => line(
                "The resolution's plan is written: review it",
                CheckTone::Ready,
            ),
            // Told once and still not through: the user's to look at.
            (true, _) if r.nudged => line(
                format!("{who} was nudged to write its plan: see its tab if none comes"),
                CheckTone::Blocker,
            ),
            (false, _) if r.nudged => line(
                format!("{who} was nudged to commit the merge: see its tab if it doesn't"),
                CheckTone::Blocker,
            ),
            (true, _) => line(
                format!("{who} plans how to resolve {files}\u{2026}"),
                CheckTone::Note,
            ),
            (false, Some(_)) if let Some(f) = mine.filter(|f| !f.left.is_empty()) => line(
                format!("{who} resolves {}\u{2026}", f.left.join(", ")),
                CheckTone::Note,
            ),
            // Committed: the others' merges are what it waits for.
            (false, Some(_)) if mine.is_some_and(|f| f.took_in) => line(
                format!("{} merged in \u{b7} waits for the rest", t.onto),
                CheckTone::Note,
            ),
            (false, _) => line(
                format!("{who} finishes the merge of {}\u{2026}", t.onto),
                CheckTone::Note,
            ),
        });
    }
    let state = probe?.bases.get(key)?;
    let onto = &state.onto;
    let new = count_of(state.behind, "new commit", "new commits");
    Some(match (state.behind, &state.why) {
        (0, Some(why)) => line(why.clone(), CheckTone::Note),
        (0, None) => line(format!("up to date with {onto}"), CheckTone::Note),
        (_, _) if !state.conflicts.is_empty() => line(
            format!(
                "{onto} has {new} \u{b7} conflicts in {}",
                state.conflicts.join(", ")
            ),
            CheckTone::Blocker,
        ),
        (_, Some(why)) => line(format!("{onto} has {new} \u{b7} {why}"), CheckTone::Note),
        (_, None) => line(
            format!("{onto} has {new} \u{b7} merges cleanly"),
            CheckTone::Note,
        ),
    })
}

/// The button on Plan review's final plan card, by `probe`: what the final
/// plan changed, once there is one.
fn plan_changes_action(probe: Option<&Probe>) -> Vec<(PageButton, PanelOutcome)> {
    let written = probe.is_some_and(|p| p.docs.written.contains(&Stage::PlanReview));
    let button = PageButton {
        label: "Diff".to_owned(),
        style: ButtonStyle::Plain,
    };
    match written {
        true => vec![(button, PanelOutcome::PlanChanges)],
        false => Vec::new(),
    }
}

/// The buttons of the draft cards on `stage`'s page of `feature`: what each
/// but the first changed, and folding.
fn draft_card_actions(feature: &Feature, stage: Stage) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Plain,
    };
    let ks = card_numbers(feature, stage);
    let mut out = Vec::new();
    for &k in &ks {
        if k > 1 {
            out.push((button("Diff"), PanelOutcome::DraftChanges(stage, k)));
        }
        if ks.len() > 1 {
            out.push((button("Show"), PanelOutcome::FoldFixRound(k)));
        }
    }
    out
}

/// The drafts of `stage`'s file in `feature` as cards, newest first, under a
/// caption counting them: who wrote each and when, what it was sent back
/// with or when it was approved, the newest saying where the review stands.
pub(super) fn draft_cards(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    let drafts = feature::drafts::of(&feature.drafts, stage);
    let Some(newest) = drafts.first().map(|d| d.k) else {
        return Vec::new();
    };
    let caption = match drafts.len() {
        1 => "Draft".to_owned(),
        n => format!("Drafts \u{b7} {n}"),
    };
    let mut out = vec![lines(&caption, Vec::new())];
    let run = feature.runs.get(&stage);
    let current = feature.status == Status::Open && feature.current() == Some(stage);
    for draft in drafts {
        let latest = draft.k == newest;
        let sent = match draft.comments.len() {
            0 => "sent back".to_owned(),
            n => format!("sent back with {}", plural(n, "comment")),
        };
        let (dot, state_line) = match (draft.approved, draft.sent_back) {
            // Approved, then its stage reopened: it is up for review again,
            // as its file's card says.
            (Some(_), _) if latest && current && !feature.is_done(stage) => (
                Dot::Blocked,
                line("ready for review \u{b7} reopened", CheckTone::Blocker),
            ),
            (Some(_), _) => (Dot::Done, line("approved", CheckTone::Ready)),
            // Sent back and not written again yet: its agent is at it.
            (None, Some(_)) if latest && current && run.is_some_and(|r| r.ready.is_none()) => (
                Dot::Working,
                line(
                    format!("{sent} \u{b7} being written again"),
                    CheckTone::Note,
                ),
            ),
            // Sent back: not done, only over, as a dropped round reads.
            (None, Some(_)) => (Dot::Unknown, line(sent, CheckTone::Note)),
            (None, None) if latest && current => {
                (Dot::Blocked, line("ready for review", CheckTone::Blocker))
            }
            (None, None) => (Dot::Done, line("written", CheckTone::Note)),
        };
        let mut meta = Vec::new();
        if !draft.agent.is_empty() {
            meta.push(match draft.model.is_empty() {
                true => draft.agent.clone(),
                false => format!("{} ({})", draft.agent, draft.model),
            });
        }
        meta.push(format!("written {}", crate::clock::ago(now, draft.written)));
        if let Some(at) = draft.sent_back {
            meta.push(format!("sent back {}", crate::clock::ago(now, at)));
        }
        if let Some(at) = draft.approved {
            meta.push(format!("approved {}", crate::clock::ago(now, at)));
        }
        let held = |o: &PanelOutcome| match o {
            PanelOutcome::DraftChanges(s, k) => *s == stage && *k == draft.k,
            PanelOutcome::FoldFixRound(k) => *k == draft.k,
            _ => false,
        };
        let buttons = (actions.iter().enumerate())
            .filter(|(_, (_, o))| held(o))
            .map(|(i, _)| i)
            .collect();
        out.push(PageSection::Round(chrome::RoundCard {
            label: format!("Draft {}", draft.k),
            dot,
            state: state_line,
            meta: meta.join(" \u{b7} "),
            asked: (draft.comments.iter())
                .map(|c| chrome::RoundAsked {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    note: c.note.clone(),
                    now: None,
                })
                .collect(),
            asked_caption: "Comments sent back",
            fixes: Vec::new(),
            running: Vec::new(),
            changed: Vec::new(),
            open: fix_round_open(Some(newest), draft.k, &state.fix_rounds_flipped),
            nested: false,
            buttons,
        }));
    }
    out
}

/// `stage`'s file to read on its page, as `probe` last read it; none while
/// it has none.
pub(super) fn doc_of(probe: Option<&Probe>, stage: Stage) -> Option<PageSection> {
    let blocks = probe?.docs.previews.get(&stage)?;
    Some(PageSection::Doc {
        name: crate::feature::artifacts::name(stage)?.to_owned(),
        blocks: blocks.clone(),
        here: false,
    })
}

/// Whether `stage`'s page of `feature` offers the choice of one agent or a
/// fresh one per milestone: Build's, until it is done.
pub(super) fn milestone_switch(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::Build && feature.status == Status::Open && !feature.is_done(stage)
}

/// The line saying how many failures of Manual test's criteria in `state`
/// are not sent yet, then `when`: none while all are.
pub(super) fn failures_waiting(state: &PanelState, when: &str) -> Option<CheckLine> {
    let n = state
        .ctx
        .manual
        .failed
        .saturating_sub(state.ctx.manual.sent);
    if n == 0 {
        return None;
    }
    // The user's comments open and not sent wait the same way.
    let comments = (state.criteria.iter())
        .filter(|r| r.comment && r.found == Some(Verdict::Fail) && !r.sent)
        .count()
        .min(n);
    let what = match (n - comments, comments) {
        (_, 0) => plural(n, "failure"),
        (0, _) => plural(n, "comment"),
        (f, c) => format!("{} and {}", plural(f, "failure"), plural(c, "comment")),
    };
    let wait = if n == 1 { "waits" } else { "wait" };
    Some(line(format!("{what} {wait} {when}"), CheckTone::Note))
}
