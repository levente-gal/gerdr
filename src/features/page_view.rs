//! A stage's page and the feature tab's whole view.

use super::*;

/// `stage`'s page of `feature`, its buttons `actions`.
pub(super) fn stage_page(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> StagePage {
    let mark = stage_mark(feature, stage);
    let mut sections = Vec::new();
    if let Some((text, wrong)) = &state.seen_note {
        let tone = if *wrong {
            CheckTone::Blocker
        } else {
            CheckTone::Note
        };
        sections.push(lines("Another machine", vec![line(text.clone(), tone)]));
    }
    sections.extend((feature.pulls().iter()).map(|pull| pull_card(feature, pull, actions, now)));
    // Going round again shows on the page, not only in a toast.
    sections.extend(loop_line(feature, stage).map(|l| lines("Round", vec![l])));
    let status = match mark {
        StageMark::Later => {
            let before = Stage::ALL[stage_index(stage).saturating_sub(1)];
            Some(line(
                format!("Comes after {}", before.label()),
                CheckTone::Note,
            ))
        }
        // Shipped through a pull request not merged yet: not done yet.
        StageMark::Done if stage == Stage::Ship && pr_wait(feature, state.probe).is_some() => {
            feature.marks.get(&stage).map(|m| {
                let wait = pr_wait(feature, state.probe).unwrap_or_default();
                let when = crate::clock::ago(now, m.at);
                line(format!("Shipped {when} by {}: {wait}", m.by), CheckTone::Note)
            })
        }
        StageMark::Done => feature.marks.get(&stage).map(|m| {
            let mut text = format!("Done {} by {}", crate::clock::ago(now, m.at), m.by);
            if let Some(by) = state.ctx.locked_by(feature, stage) {
                text.push_str(&format!(" \u{b7} locked: {}", lock_why(feature, by)));
            }
            line(text, CheckTone::Ready)
        }),
        // The bar over the page counts its criteria.
        StageMark::Current if stage == Stage::ManualTest && !state.criteria.is_empty() => None,
        // One colour per state, as the rail and the sidebar's card say it.
        StageMark::Current => stage_line(feature, stage, state).map(|text| {
            let failed = stage_failed(feature, stage, state.probe) || text.contains("failed");
            let tone = if failed {
                CheckTone::Failed
            } else if text.contains("not ready") || waits(feature, stage, &state.into()) {
                CheckTone::Blocker
            } else {
                CheckTone::Note
            };
            line(text, tone)
        }),
    };
    // The open fix round on top, the page's own work under its agent, then
    // the rounds over (Build lists its own after its work).
    let (mut round_top, round_under, history) = round_sections(feature, stage, state, actions, now);
    // A stage not reached yet may start by itself: its agent is picked ahead.
    if mark == StageMark::Later {
        sections.append(&mut round_top);
        sections.extend(agent_card(feature, stage, state, actions, now));
        // What it will run is there to see ahead too.
        if !check_entries(feature, stage, &state.plan).is_empty() {
            sections.extend(verify_sections(feature, state, actions, now));
        }
        if stage == Stage::Build {
            sections.extend(round_cards(feature, stage, state, actions, now));
        }
        // So does the review they came from, or the plan's last.
        if is_review_stage(stage) && review_kept(feature, stage) {
            sections.extend(review_page_sections(feature, stage, mark, state, actions));
        }
        // And the comments, whose fixes this came back for.
        if stage == Stage::CodeReview && crate::feature_diff::has_rounds(feature) {
            sections.extend(comment_sections(feature, state, actions));
        }
        // Its fixes took it back to Verify: what it found stays, and more
        // can be tried meanwhile.
        if shows_criteria(feature, stage) && !state.criteria.is_empty() {
            let said = failures_waiting(state, "for Manual test");
            sections.push(lines("Criteria", said.into_iter().collect()));
            sections.extend(criterion_sections(state, actions, false));
        }
        if offers_sandbox(feature, stage) {
            sections.extend(sandbox_sections(feature, state));
        }
    }
    // Build's go under its agent, where its work is listed.
    if mark != StageMark::Later && stage == Stage::Build {
    } else if mark != StageMark::Later && ROUND_CARD_STAGES.contains(&stage) {
        sections.append(&mut round_top);
    } else if mark != StageMark::Later {
        let open = fix_round_on(feature, stage);
        sections.extend(open.map(|r| fix_round_card(feature, r, actions, now)));
        // Over, it stays in the same place.
        sections.extend(last_round_on(feature, stage).map(|r| last_round_card(r, actions, now)));
    }
    // Work sent back to Build heads the page; else on AI review the agent
    // fixes go to follows the reviewer.
    let mut fixes = rework_card(feature, stage, state, actions);
    let fixes_under = stage == Stage::AiReview && rework_on(feature, stage, state.ctx).is_none();
    if mark != StageMark::Later {
        if !fixes_under {
            sections.extend(fixes.take());
        }
        if let Some(m) = feature.marks.get(&stage).filter(|m| !m.note.is_empty()) {
            sections.push(lines("Note", vec![line(m.note.clone(), CheckTone::Note)]));
        }
        if is_stale(feature, stage, state.probe) {
            let text = "The code changed since it was marked";
            sections.push(lines("Stale", vec![line(text, CheckTone::Blocker)]));
        }
        let changed = state.probe.is_some_and(|p| p.docs.changed.contains(&stage));
        if changed && mark == StageMark::Done {
            let name = crate::feature::artifacts::name(stage).unwrap_or("Its file");
            let text = format!("{name} changed after it was approved");
            sections.push(lines("Changed", vec![line(text, CheckTone::Blocker)]));
        }
        let card = || agent_card(feature, stage, state, actions, now);
        match stage {
            // Both reviews' pages are laid out alike: Plan review's final
            // plan, once written, goes over the suggestions it came from.
            Stage::PlanReview | Stage::AiReview => {
                sections.extend(card());
                sections.extend(fixes.take());
                sections.extend(review_page_sections(feature, stage, mark, state, actions));
            }
            Stage::Discover | Stage::Plan => {
                sections.extend(card());
                sections.push(file_card(feature, stage, state.ctx, actions));
                sections.extend(draft_cards(feature, stage, state, actions, now));
                sections.extend(doc_of(state.probe, stage));
            }
            Stage::Build => {
                let blocked = feature.runs.get(&stage).and_then(|r| r.blocked.as_ref());
                if let Some(blocked) = blocked {
                    let why = line(blocked.why.clone(), CheckTone::Failed);
                    sections.push(lines("Blocked", vec![why]));
                }
                sections.extend(card());
                if milestone_switch(feature, stage) {
                    sections.push(PageSection::MilestoneMode {
                        fresh: feature.fresh_milestones,
                    });
                }
                // The build log by name once read, else the counts of its
                // last stop.
                let stop = feature.runs.get(&stage).and_then(|r| r.stop);
                let counted = stop.map(|stop| PageSection::Milestones {
                    label: stage.label().to_owned(),
                    state: None,
                    done: stop.done as usize,
                    total: stop.total as usize,
                    items: Vec::new(),
                    running: Vec::new(),
                    buttons: Vec::new(),
                });
                let at_work = feature.current() == Some(stage)
                    && matches!(state.agent_status, Some(AgentStatus::Working));
                let building = at_work && feature.fix_round.is_none();
                sections.extend(round_cards(feature, stage, state, actions, now));
                let started = feature.runs.contains_key(&stage);
                sections.extend(
                    (milestones_of(state.probe, stage, building).or(counted))
                        .or_else(|| planned_milestones(state.probe, started)),
                );
                sections.extend(setup_sections(feature, state, actions));
            }
            Stage::Verify => {
                sections.extend(ci_sections(feature, state.ctx, actions));
                sections.extend(verify_sections(feature, state, actions, now));
                sections.extend(setup_sections(feature, state, actions));
            }
            Stage::CodeReview => {
                // The switcher above says it, where there is one.
                if mark == StageMark::Current && !crate::feature_diff::review_views(feature) {
                    let text = (state.code_review.clone())
                        .unwrap_or_else(|| "review every changed file".into());
                    sections.push(lines("Files", vec![line(text, CheckTone::Note)]));
                }
                sections.extend(comment_sections(feature, state, actions));
                // Its fixes made, what comes next is said under them.
                if mark == StageMark::Current && last_round_on(feature, stage).is_some() {
                    let next = stage.next().map_or("the next stage", Stage::label);
                    let text = format!(
                        "Next: Review code shows what the fixes changed, then Approve goes on to {next}"
                    );
                    sections.push(lines("Next", vec![line(text, CheckTone::Note)]));
                }
            }
            Stage::ManualTest if mark == StageMark::Current => {
                if state.criteria.is_empty() {
                    let text = match feature.pull() {
                        Some(_) => {
                            "no criteria in the pull request's description: Mark done by hand"
                        }
                        None => "no criteria in the brief: Mark done by hand",
                    };
                    sections.push(lines("Criteria", vec![line(text, CheckTone::Note)]));
                } else {
                    // Its status says how far they went; this, what holds it up.
                    let said = match state.probe.map(|p| &p.reading) {
                        None => Some("reading the code".to_owned()),
                        Some(Ok(reading)) if !reading.dirty.is_empty() => Some(format!(
                            "uncommitted changes in {}",
                            reading.dirty.join(", ")
                        )),
                        Some(_) => None,
                    };
                    let mut said: Vec<CheckLine> = said
                        .map(|text| line(text, CheckTone::Note))
                        .into_iter()
                        .collect();
                    // Found while a round is open: the next one takes them.
                    if feature.fix_round.is_some() {
                        said.extend(failures_waiting(state, "for the current round"));
                    }
                    if feature.posts_review() {
                        said.extend(failures_waiting(state, "for the review on GitHub"));
                    }
                    if manual_sends(feature, state.ctx) {
                        let why = manual_own_words(state.ctx);
                        said.extend(why.map(|why| line(why, CheckTone::Note)));
                    }
                    if fixes_unverified(feature, stage) {
                        let cause = (feature.fix_asked.iter().rev())
                            .find(|(_, s)| **s == Stage::ManualTest)
                            .map_or_else(
                                || "Fixes".to_owned(),
                                |(n, _)| feature::fix_round::label(*n),
                            );
                        let text = format!(
                            "{cause} changed the code: Mark passed first takes it through Verify, AI review and Code review again, then back here"
                        );
                        said.push(line(text, CheckTone::Ready));
                    }
                    sections.push(lines("Criteria", said));
                    sections.extend(criterion_sections(state, actions, true));
                }
                sections.extend(sandbox_sections(feature, state));
            }
            // What was found, kept to see, and more can be tried: only
            // reopening sends what fails.
            Stage::ManualTest if !state.criteria.is_empty() => {
                let by_hand = state.criteria.iter().all(|row| row.found.is_none());
                let said = by_hand.then(|| line("marked done by hand, not tried", CheckTone::Note));
                let waiting = failures_waiting(state, "since it was done: reopen it to send them");
                sections.push(lines("Criteria", said.into_iter().chain(waiting).collect()));
                sections.extend(criterion_sections(state, actions, false));
                if offers_sandbox(feature, stage) {
                    sections.extend(sandbox_sections(feature, state));
                }
            }
            Stage::Ship if feature.pull().is_some() => {
                sections.extend(card());
                sections.push(PageSection::Recap(recap_card(feature, state, now)));
                sections.extend(submit_sections(feature, state, actions, now));
                sections.extend(shipped_after_group(feature, actions, now));
            }
            Stage::Ship => {
                sections.extend(card());
                sections.extend(resolving_card(feature, state, actions, now));
                let check = check_lines(feature, state.probe, state.shipping, now);
                if !check.is_empty() {
                    sections.push(lines("Ship check", check));
                }
                let again: Vec<usize> = (actions.iter().enumerate())
                    .filter(|(_, (_, o))| *o == PanelOutcome::CheckShip)
                    .map(|(i, _)| i)
                    .collect();
                if !again.is_empty() {
                    sections.push(PageSection::Toolbar {
                        groups: vec![(String::new(), again)],
                    });
                }
                let mut recap = recap_card(feature, state, now);
                recap.buttons = (actions.iter().enumerate())
                    .filter(|(_, (_, o))| {
                        matches!(o, PanelOutcome::WriteSummary | PanelOutcome::EditSummary)
                    })
                    .map(|(i, _)| i)
                    .collect();
                sections.push(PageSection::Recap(recap));
                let cards = ship_sections(feature, state, actions, now);
                let carded = !cards.is_empty();
                sections.extend(cards);
                sections.extend(shipped_after_group(feature, actions, now));
                // Without cards, as once its worktrees are gone, its pull
                // requests are listed.
                let prs: Vec<CheckLine> = (feature.shipping.prs.iter())
                    .filter(|_| !carded)
                    .map(|pr| line(pr_tail(&pr.url), CheckTone::Note))
                    .collect();
                if !prs.is_empty() {
                    sections.push(lines("Pull requests", prs));
                }
                // The cards' rows say what ran: the log only without them.
                let log = ship_log_lines(feature, now);
                if !log.is_empty() && !carded {
                    sections.push(lines("Ship log", log));
                }
            }
            Stage::ManualTest => {
                if offers_sandbox(feature, stage) {
                    sections.extend(sandbox_sections(feature, state));
                }
            }
        }
        // Ended before Ship, as a review can: what follows shows where.
        if stage != Stage::Ship && stage == after_stage(feature) {
            sections.extend(shipped_after_group(feature, actions, now));
        }
    }
    // Fixes sent back the feature: the rounds that sent them stay.
    sections.extend(round_under);
    sections.extend(history);
    // A stage with nothing run yet says what happens on it: under its
    // agent's card, where the page is otherwise empty.
    // A stage no agent runs has it last, under what its page already holds.
    if let Some(next) = next_section(feature, stage, mark) {
        // The page's own agent's, else, on a stage with one, the agent's
        // its fixes go to.
        let agent = (sections.iter())
            .position(|s| matches!(s, PageSection::Agent { stage: s, .. } if *s == stage))
            .or_else(|| {
                needs(stage)?;
                (sections.iter()).position(|s| matches!(s, PageSection::Agent { .. }))
            });
        let at = match (agent, needs(stage)) {
            (Some(i), _) => i + 1,
            (None, Some(_)) => 0,
            (None, None) => sections.len(),
        };
        sections.insert(at, next);
    }
    // The feature's own agents go under the stage's, else first.
    let own = own_cards(feature, stage, state, actions, now);
    if !own.is_empty() {
        let at = (sections.iter())
            .rposition(|s| matches!(s, PageSection::Agent { .. }))
            .map_or(0, |i| i + 1);
        sections.splice(at..at, own);
    }
    // The agent's card says what it does: the status line needn't again.
    let carded = |l: &CheckLine| {
        (sections.iter()).any(|s| {
            matches!(s, PageSection::Agent { status, .. }
                if !status.is_empty() && l.text.ends_with(status.as_str()))
        })
    };
    // Ship's check says how it stands, and more.
    let checked = mark == StageMark::Current && stage == Stage::Ship;
    // Code review's switch says how its files stand.
    let switched = stage == Stage::CodeReview && crate::feature_diff::review_views(feature);
    let status = status.filter(|l| !carded(l) && !checked && !switched);
    // A restart runs the agent picked: the button says so when it's another.
    let lit = state.picks.iter().find(|c| c.lit).map(|c| c.name.as_str());
    let other = lit.filter(|lit| {
        (feature.runs.get(&stage)).is_some_and(|run| !run.agent.eq_ignore_ascii_case(lit))
    });
    let label = |(b, o): &(PageButton, PanelOutcome)| match (other, o) {
        (Some(lit), o) if *o == PanelOutcome::StartAgent(stage, true) => PageButton {
            label: format!("Restart with {lit}"),
            ..b.clone()
        },
        (_, PanelOutcome::FoldRound(n)) if state.rounds_open.contains(n) => PageButton {
            label: "Hide".to_owned(),
            ..b.clone()
        },
        (_, PanelOutcome::FoldFixRound(n))
            if fix_round_open(newest_round(feature, stage), *n, &state.fix_rounds_flipped) =>
        {
            PageButton {
                label: "Hide".to_owned(),
                ..b.clone()
            }
        }
        _ => b.clone(),
    };
    // A loop names its round on each stage it goes through.
    let title = match loop_line(feature, stage).and_then(|_| loop_cause(feature)) {
        Some(cause) => format!("{} \u{b7} after {cause}", stage.label()),
        None => stage.label().to_owned(),
    };
    let mut page = StagePage {
        primary: None,
        menu: (actions.iter().enumerate())
            .filter(|(_, (b, o))| in_menu(b, o))
            .map(|(i, _)| i)
            .collect(),
        title,
        about: stage.about().to_owned(),
        status,
        buttons: actions.iter().map(label).collect(),
        sections,
    };
    place_buttons(&mut page);
    page
}

/// "What happens next" on `stage`'s page of `feature`, while it is the
/// current one and nothing ran on it yet: what its agent or the user does
/// there, and what follows. One place for every stage's page to say it; a
/// stage with more to say adds its lines here.
fn next_section(feature: &Feature, stage: Stage, mark: StageMark) -> Option<PageSection> {
    if mark != StageMark::Current || feature.status != Status::Open {
        return None;
    }
    let fresh = !feature.runs.contains_key(&stage);
    let says: &[&str] = match stage {
        Stage::Discover if fresh => &[
            "Start agent opens a chat with an agent: it asks what the feature is for, then writes brief.md",
            "You then read the brief here, and approve it or send it back with comments",
        ],
        // It doesn't start by itself: the agent and its model are picked first.
        Stage::Plan if fresh => &[
            "Plan doesn't start by itself: pick its agent above, then Start agent",
            "Its agent writes plan.md from the approved brief; you review it here and approve it",
        ],
        Stage::PlanReview if fresh && feature.plan_review.is_none() => &[
            "Start agent has another agent check the plan",
            "Its suggestions come here: accept or reject each, then write the final plan",
        ],
        Stage::Build if fresh && feature.fix_round.is_none() => &[
            "Start agent has an agent build the plan's milestones, committing as it goes",
            "They tick off here as they land; Verify runs the checks after",
        ],
        Stage::Verify if feature.verify.is_none() => &[
            "Run checks runs the project's checks on the feature's worktrees",
            "Failures can go back to Build as fixes; once they pass, AI review is next",
        ],
        Stage::AiReview if fresh && feature.review.is_none() => &[
            "Start agent has another model review the feature's diff",
            "You decide on each finding here: fix it, or dismiss it with a reason",
        ],
        Stage::CodeReview if feature.comment_fixes.is_empty() => &[
            "You read every changed file and mark it viewed; comments go back to Build as fixes",
            "Approve once every file is viewed: Manual test is next",
        ],
        Stage::ManualTest if feature.manual.is_none() => &[
            "You try each acceptance criterion, in a sandbox if you start one, and mark it passed or failed",
            "Failures go back to Build as fixes; once all pass, Ship is next",
        ],
        Stage::Ship if feature.ship_log().is_empty() => &[
            "Pick how each repository ships, then Ship… runs the steps ticked",
            "What follows shipping is ticked under Afterwards: each can be undone",
        ],
        _ => return None,
    };
    let lines = says
        .iter()
        .map(|text| line(*text, CheckTone::Note))
        .collect();
    Some(PageSection::Lines {
        label: "What happens next".to_owned(),
        lines,
    })
}

/// Whether a page's button is a rare one, a row of the tab's `⋯` menu rather
/// than a button on the page: starting over, setting up again, the project's
/// commands (unless the page wants them), a stopped sandbox's removal. A
/// running sandbox's Reset data stays on the page, tried between runs.
fn in_menu(button: &PageButton, outcome: &PanelOutcome) -> bool {
    match outcome {
        PanelOutcome::StartOver(_) | PanelOutcome::SetUp | PanelOutcome::RemoveSandbox => true,
        PanelOutcome::EditCommands(_) => button.style == ButtonStyle::Quiet,
        _ => false,
    }
}

/// Where `page`'s buttons go: its one primary to the page's bar (a card of
/// its own holding it, an agent's, a finding's, a round's, a repository's on
/// Ship, keeps it too, drawn plain), the rare ones (`StagePage::menu`) off
/// the page; and each row of buttons in one style: a quiet one beside
/// outlined ones is outlined too.
pub(crate) fn place_buttons(page: &mut StagePage) {
    let menu = page.menu.clone();
    let local: Vec<usize> = (page.sections.iter())
        .flat_map(|s| match s {
            PageSection::Finding(card) => (card.buttons.iter().chain(&card.comment_buttons))
                .copied()
                .collect(),
            PageSection::Round(card) => card.buttons.clone(),
            PageSection::ShipGroup(group) => group.buttons(),
            _ => Vec::new(),
        })
        .collect();
    // A card's own comes before the stage's; and one only is filled.
    let filled = |i: &usize| {
        !menu.contains(i) && (page.buttons.get(*i)).is_some_and(|b| b.style == ButtonStyle::Primary)
    };
    page.primary = (local.iter().copied().find(|i| filled(i)))
        .or_else(|| (0..page.buttons.len()).find(|i| filled(i)));
    for (i, b) in page.buttons.iter_mut().enumerate() {
        if b.style == ButtonStyle::Primary && page.primary != Some(i) {
            b.style = ButtonStyle::Plain;
        }
    }
    let gone = |i: &usize| menu.contains(i) || page.primary == Some(*i);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for section in &mut page.sections {
        match section {
            // An agent's card keeps the way to it, the bar's primary or not.
            PageSection::Agent { buttons, .. } => {
                buttons.retain(|i| !menu.contains(i));
                rows.push(buttons.clone());
            }
            PageSection::File { buttons, .. }
            | PageSection::Milestones { buttons, .. } => {
                buttons.retain(|i| !gone(i));
                rows.push(buttons.clone());
            }
            PageSection::Review(summary) => {
                summary.buttons.retain(|i| !gone(i));
                rows.push(summary.buttons.clone());
            }
            PageSection::Recap(card) => {
                card.buttons.retain(|i| !gone(i));
                rows.push(card.buttons.clone());
            }
            PageSection::Toolbar { groups } => {
                for (_, held) in groups.iter_mut() {
                    held.retain(|i| !gone(i));
                }
                groups.retain(|(word, held)| !word.is_empty() || !held.is_empty());
                rows.push(groups.iter().flat_map(|(_, h)| h.clone()).collect());
            }
            PageSection::Finding(card) => {
                rows.push(card.buttons.clone());
                rows.push(card.comment_buttons.clone());
            }
            PageSection::Round(card) => rows.push(card.buttons.clone()),
            _ => {}
        }
    }
    page.sections
        .retain(|s| !matches!(s, PageSection::Toolbar { groups } if groups.is_empty()));
    let held: Vec<usize> = rows.iter().flatten().copied().collect();
    let held_ship: Vec<usize> = (page.sections.iter())
        .flat_map(|s| match s {
            PageSection::ShipGroup(group) => group.buttons(),
            _ => Vec::new(),
        })
        .collect();
    // What no card holds goes at the title's right, on the page's bar: a row of its own.
    rows.push(
        (0..page.buttons.len())
            .filter(|i| !gone(i) && !held.contains(i) && !held_ship.contains(i))
            .collect(),
    );
    for row in rows {
        let outlined = (row.iter())
            .any(|i| (page.buttons.get(*i)).is_some_and(|b| b.style != ButtonStyle::Quiet));
        for i in row.iter().filter(|_| outlined) {
            if let Some(b) = page.buttons.get_mut(*i)
                && b.style == ButtonStyle::Quiet
            {
                b.style = ButtonStyle::Plain;
            }
        }
    }
}

/// Code review's page of `feature` below its fix round cards: the comments as
/// AI review's findings are, those not sent yet first, and the plan to read.
pub(super) fn comment_sections(
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    if !lists_comments(feature) {
        return Vec::new();
    }
    let multi = feature.many_repos();
    let notes = shown_notes(feature, Stage::CodeReview, notes_of(state.probe));
    let made: &[String] = (feature.comment_fixes.last()).map_or(&[], |fix| &fix.made);
    let planning = feature::fix_round::planning(feature.fix_round.as_ref(), Stage::CodeReview);
    let sent = notes.iter().filter(|c| c.flag.is_some());
    let done = sent.clone().filter(|c| note_made(c, made)).count();
    let mut sections = Vec::new();
    match (notes.is_empty(), sent.count()) {
        (true, _) => {
            let text = "none yet: write them in Code review, then plan the fixes";
            sections.push(lines("Comments", vec![line(text, CheckTone::Note)]));
        }
        (false, 0) => sections.push(lines("Comments", Vec::new())),
        (false, total) => sections.push(lines(
            &format!("Comments \u{b7} {done} of {total} made"),
            Vec::new(),
        )),
    }
    sections.extend(notes.iter().map(|c| {
        PageSection::Finding(match c.flag.is_some() {
            true => sent_note_card(c, multi, (note_made(c, made), planning), actions),
            false => note_card(c, multi, actions),
        })
    }));
    sections.extend(comment_plan(feature, state.probe));
    sections
}

/// What Code review's line says to do next, and its buttons: those of the
/// review laid into the page (`panel.review`), while its code shows. Approve
/// is the one to press once nothing holds it up, Plan fixes while comments
/// wait for it.
fn review_toolbar(
    panel: &FeaturePanel,
    feature: &Feature,
    files: Option<chrome::ReviewFiles>,
    ctx: RowCtx,
) -> (Option<(String, bool)>, Vec<chrome::ReviewButton>) {
    use crate::chrome::ReposButton as B;
    let wd = (panel.review.as_deref())
        .filter(|_| crate::feature_diff::review_page(panel, feature))
        .filter(|wd| {
            (wd.feature.as_ref())
                .is_some_and(|fe| fe.project == feature.project && fe.slug == feature.slug)
        });
    let Some((wd, gate)) = wd.and_then(|wd| Some((wd, wd.approve_gate(feature.current())?))) else {
        return (None, Vec::new());
    };
    let (total, changed) = files.map_or((0, 0), |f| (f.total, f.changed));
    // Done with, it reads as it was worked; Reopen waits on the next stage.
    let done = (wd.feature.as_ref()).is_some_and(|fe| fe.locked().is_some());
    let locked = (ctx.locked_by(feature, Stage::CodeReview)).filter(|_| done);
    let next = match done {
        true => match locked {
            Some(by) => (
                format!("Approved \u{b7} locked: {}", lock_why(feature, by)),
                true,
            ),
            None => (
                "Approved \u{b7} only to read: Reopen to change it".to_owned(),
                true,
            ),
        },
        false => crate::feature_diff::next_step(&gate, total, changed),
    };
    let buttons = (wd.feature_hints().into_iter())
        .map(|mut hint| {
            if let (B::Reopen, Some(by)) = (hint.button, locked) {
                let why = crate::stage_lock::locked_warning(feature, Stage::CodeReview, by);
                hint.disabled = Some(why);
            }
            let style = match (&hint.disabled, hint.button) {
                (Some(_), _) => ButtonStyle::Off,
                (None, B::Approve | B::GoOn) => ButtonStyle::Primary,
                (None, B::PlanFixes) if gate.comments > 0 => ButtonStyle::Primary,
                _ => ButtonStyle::Plain,
            };
            chrome::ReviewButton {
                button: hint.button,
                label: hint.label,
                style,
                key: hint.key,
                disabled: hint.disabled,
            }
        })
        .collect::<Vec<_>>();
    // The one to press goes rightmost, as in a dialog: Plan fixes past a
    // greyed Approve while comments wait.
    let mut buttons = buttons;
    buttons.sort_by_key(|b| b.style == ButtonStyle::Primary);
    (Some(next), buttons)
}

/// The files of `feature`'s code review, as the switch counts them: by its last
/// read once there was one, else by what the probe found and the viewed marks.
fn review_files(feature: &Feature, state: &PanelState) -> Option<chrome::ReviewFiles> {
    if let Some(counts) = state.code_counts {
        return Some(chrome::ReviewFiles {
            viewed: counts.viewed,
            total: counts.files,
            changed: counts.changed,
        });
    }
    let total: usize = state.probe?.stats.values().map(|stat| stat.files).sum();
    (total > 0).then(|| chrome::ReviewFiles {
        viewed: feature.viewed.0.len().min(total),
        total,
        changed: 0,
    })
}

/// How the fixes of `feature`'s Code review stand, as the switch says it; `notes`
/// as `Docs::notes` holds them.
pub(super) fn fixes_word(feature: &Feature, notes: &[Comment]) -> String {
    if let Some(round) = (feature.fix_round.as_ref()).filter(|r| r.stage == Stage::CodeReview) {
        return round.line();
    }
    let made: &[String] = (feature.comment_fixes.last()).map_or(&[], |fix| &fix.made);
    let sent = notes.iter().filter(|c| c.flag.is_some());
    match sent.clone().count() {
        0 if feature.is_done(Stage::CodeReview) => "none".to_owned(),
        0 => "none yet".to_owned(),
        total => {
            // What it counts, as the Comments heading under it does.
            let done = sent.filter(|c| note_made(c, made)).count();
            format!("{done} of {} made", plural(total, "comment"))
        }
    }
}

/// Whether sent comment `c`'s id is among those of its round `made`.
fn note_made(c: &Comment, made: &[String]) -> bool {
    (c.flag.as_ref()).is_some_and(|flag| made.contains(&flag.id))
}

/// The fix plan of `feature`'s latest Code review round, as `probe` last read
/// it, once written: while the agent writes it again, what is there is old.
fn comment_plan(feature: &Feature, probe: Option<&Probe>) -> Option<PageSection> {
    let writing = (feature.fix_round.as_ref()).is_some_and(|round| {
        round.stage == Stage::CodeReview
            && matches!(round.phase, FixPhase::Compacting | FixPhase::Planning)
    });
    let blocks = probe?.docs.comment_plan.as_ref().filter(|_| !writing)?;
    Some(PageSection::Doc {
        name: feature::fix_round::FILE.to_owned(),
        blocks: blocks.clone(),
        here: false,
    })
}

/// How a line of `feature`'s history reads in the activity: a failure as
/// failed, a send-back as the user's turn, a tick or a pass as done, the
/// rest a note.
pub(super) fn event_tone(what: &str) -> CheckTone {
    let lower = what.to_lowercase();
    // A stop between milestones is a pause, not a failure.
    if lower.contains("stopped after milestone") {
        return CheckTone::Note;
    }
    let failed = ["couldn't", "refused", "failed", "stalled", "blocked"];
    if failed.iter().any(|f| lower.contains(f)) {
        return CheckTone::Failed;
    }
    let blocker = [
        "sent back",
        "code changed",
        "not done",
        "not read",
        "stopped",
        "stalled",
        "to do again",
        "abandoned",
    ];
    let ready = [
        "ticked",
        " done",
        "approved",
        "passed",
        "build done",
        "opened",
        "updated",
    ];
    if blocker.iter().any(|b| lower.contains(b)) {
        CheckTone::Blocker
    } else if ready.iter().any(|r| lower.contains(r)) || matches!(what, "Set up" | "Setup done") {
        CheckTone::Ready
    } else {
        CheckTone::Note
    }
}

/// Let go of what the problems list says of the setup and the sandbox of
/// feature `title`, once it closed: nothing of it is left to fix there.
/// What a removed feature's sandbox left on a machine stays.
pub(super) fn forget_problems_of(title: &str) {
    let of = |what: &str| format!("{what} \u{201c}{title}\u{201d} ");
    let (setup, sandbox) = (of("Setup of"), of("The sandbox of"));
    crate::problems::forget_matching("features", |t| t.starts_with(&setup));
    crate::problems::forget_matching(crate::feature_sandbox::PROBLEMS, |t| {
        t.starts_with(&sandbox) && !t.ends_with("until that can be reached")
    });
}

/// What a toast says of `stage` of `feature` just marked done, one shape
/// for every approval: the activity's line for it, then what comes next
/// ("Plan approved · Plan review is next"), or that the feature is done.
pub(crate) fn done_toast(feature: &Feature, stage: Stage) -> String {
    if feature.status == Status::Done {
        return format!("\u{201c}{}\u{201d} is done", feature.title);
    }
    let said = (feature.history.last())
        .filter(|e| e.what.starts_with(stage.label()))
        .map_or_else(|| format!("{} done", stage.label()), |e| e.what.clone());
    match feature.current() {
        Some(next) => format!("{said} \u{b7} {} is next", next.label()),
        None => said,
    }
}

/// `what` with its first letter upper case.
fn capitalised(what: &str) -> String {
    let mut chars = what.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `feature`'s history, newest first.
pub(super) fn activity(feature: &Feature, now: u64) -> Vec<ActivityRow> {
    let history = &feature.history;
    // "Plan ticked" right with "Plan approved" (or "Verify run 2 passed")
    // says the same twice: the one that says why stays.
    let ticked = |i: usize| history[i].what.ends_with(" ticked");
    let doubled = |i: usize| {
        let near = |j: usize| {
            j < history.len() && !ticked(j) && history[j].at.abs_diff(history[i].at) <= 2
        };
        ticked(i) && ((i > 0 && near(i - 1)) || near(i + 1))
    };
    (0..history.len())
        .rev()
        .filter(|&i| !doubled(i))
        .map(|i| &history[i])
        .map(|e| ActivityRow {
            // Capitalised as every line is, older records' "created" too.
            what: capitalised(&e.what),
            ago: crate::clock::ago(now, e.at),
            tone: event_tone(&e.what),
        })
        .collect()
}

/// What fold button `i` of `page` does, with its key; none for another.
pub(super) fn fold_tip(page: &chrome::StagePage, i: usize) -> Option<String> {
    let word = page.sections.iter().find_map(|s| match s {
        PageSection::Toolbar { groups } => (groups.iter())
            .find(|(_, held)| held.contains(&i))
            .map(|(word, _)| word.as_str()),
        _ => None,
    })?;
    let label = &page.buttons.get(i)?.label;
    // Other toolbars' buttons (the setup's, Ship's) fold nothing.
    let open = match (word, label.as_str()) {
        ("Fold", _) => false,
        ("", "Open all") => true,
        _ => return None,
    };
    let (what, key) = match label.as_str() {
        "All" | "Open all" => (
            "every criterion",
            if open { "Alt+Right" } else { "Alt+Left" },
        ),
        FOLD_PASSED => ("the criteria passed", "Alt+D"),
        _ => ("the criteria read: opened or marked", "Alt+S"),
    };
    let verb = if open { "Open" } else { "Fold" };
    Some(format!("{verb} {what} ({key})"))
}

/// The kind of item `stage`'s page has as cards: Plan review's
/// suggestions, else findings.
pub(super) fn stage_items(stage: Stage) -> ItemOf {
    match stage {
        Stage::PlanReview => ItemOf::Suggestion,
        _ => ItemOf::Finding,
    }
}

/// The key opening the feature tab's `⋯` menu (Shift+F10 and the Menu key
/// too, as a context menu's).
pub(crate) const MENU_KEY: &str = "F10";

/// Alt+E on `stage`'s page of `feature`, named by what it folds: the
/// history (earlier reviews and fix rounds over), the drafts (Discover,
/// Plan) or Build's fix rounds.
pub(super) fn fold_hint(feature: &Feature, stage: Stage) -> Option<(String, String)> {
    let what = if earlier_reviews(feature, stage).next().is_some() {
        "history"
    } else if !folds_round_cards(feature, stage) {
        return None;
    } else if matches!(stage, Stage::Discover | Stage::Plan) {
        "drafts"
    } else if stage == Stage::Build {
        "fix rounds"
    } else {
        "history"
    };
    Some(("Alt+E".to_owned(), what.to_owned()))
}

/// `label` with its first letter in lower case, as hints are written: a
/// button's "Go to Plan" is "go to Plan" in the footer.
pub(crate) fn lower_first(label: &str) -> String {
    let mut chars = label.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_lowercase().chain(chars).collect())
}

/// The heading over the home project's repositories the feature could
/// take.
const NOT_ADDED: &str = "Not added";

/// The group of rows `entry` belongs in, by its heading.
fn entry_group(feature: &Feature, entry: RepoEntry) -> &'static str {
    match entry {
        RepoEntry::Held(i) if feature.repos.get(i).is_some_and(|r| r.touched) => "In feature",
        RepoEntry::Held(_) | RepoEntry::Context(_) => "Context, read only",
        RepoEntry::Removed(_) => "Removed",
        RepoEntry::Offered { .. } => NOT_ADDED,
    }
}

/// The row of `entry` on the feature tab, as `state` has it.
fn repo_row(
    panel: &FeaturePanel,
    feature: &Feature,
    entry: RepoEntry,
    state: &PanelState,
    now: u64,
) -> FeatureRepoRow {
    let key = entry_key(feature, &panel.sources, entry).unwrap_or_default();
    let mut row = FeatureRepoRow {
        name: key.clone(),
        line: String::new(),
        kind: RepoKind::Other,
        group: None,
        busy: state.busy.contains(&key.as_str()),
        buttons: (entry_buttons(feature, &panel.sources, entry).into_iter())
            .map(|(label, _)| label)
            .collect(),
        remove: false,
        stat: None,
    };
    match entry {
        RepoEntry::Held(i) => {
            if let Some(repo) = feature.repos.get(i) {
                row.name = state.names.label(repo);
                row.line = repo_line(feature, repo, state.probe);
                row.kind = match repo.touched {
                    true => RepoKind::Active,
                    false => RepoKind::Context,
                };
                row.remove = repo_removable(feature, i);
                row.stat = state.probe.and_then(|p| p.stats.get(&key).copied());
            }
        }
        RepoEntry::Context(i) => {
            if let Some(repo) = feature.context.get(i) {
                row.name = state.names.label_of(repo.project.as_deref(), &repo.name);
                row.line = short_folder(&repo.main);
                row.kind = RepoKind::Context;
                row.remove = linkable(feature);
            }
        }
        RepoEntry::Removed(i) => {
            if let Some(gone) = feature.removed.get(i) {
                row.name = state.names.label_of(gone.project.as_deref(), &gone.name);
                row.line = format!("removed {}", crate::clock::ago(now, gone.at));
            }
        }
        RepoEntry::Offered { source, repo } => {
            let source = panel.sources.get(source);
            if let Some(source) = source
                && let Finding::Found(root) = &source.finding
                && let Some(found) = root.repos.get(repo)
            {
                row.name = found.name.clone();
                row.line = found.base.clone();
            }
        }
    }
    if panel.armed.as_ref() == Some(&key) {
        row.line = "Press Del again".to_owned();
    }
    row
}

/// What the feature tab shows of `feature`, one of `slugs`, as `state`
/// has it.
pub(super) fn tab_view(
    panel: &FeaturePanel,
    feature: &Feature,
    slugs: &[String],
    state: &PanelState,
    now: u64,
) -> FeatureTabView {
    let entries = repo_entries(feature, &panel.sources, panel.expanded);
    let mut group = None;
    let repos: Vec<FeatureRepoRow> = (entries.iter())
        .map(|entry| {
            let mut row = repo_row(panel, feature, *entry, state, now);
            let caption = entry_group(feature, *entry);
            if group != Some(caption) {
                group = Some(caption);
                row.group = Some(caption.to_owned());
            }
            row
        })
        .collect();
    let repo_notes = (panel.sources.iter())
        .filter(|_| panel.expanded && linkable(feature))
        .filter_map(|source| {
            let (text, error) = match &source.finding {
                Finding::Looking => ("looking\u{2026}".to_owned(), false),
                Finding::Failed(why) => (format!("couldn't read: {why}"), true),
                Finding::Found(_) => return None,
            };
            let caption = match source.project {
                None => NOT_ADDED.to_owned(),
                Some(_) => source.name.clone(),
            };
            Some(RepoNote {
                caption,
                text,
                error,
            })
        })
        .collect();
    let stage = panel.page_stage(feature);
    let rail = Stage::ALL
        .iter()
        .map(|s| {
            let mut node = rail_node(feature, *s, state, now);
            node.selected = *s == stage;
            // Another stage's page shows: the stage the feature is at says
            // so, apart from the one ringed as viewed.
            if feature.current() == Some(*s) && *s != stage {
                node.line = match node.line.is_empty() {
                    true => "current".to_owned(),
                    false => format!("current \u{b7} {}", node.line),
                };
            }
            node
        })
        .collect();
    let back = rail_back(feature, state.ctx);
    let mut page = match panel.keys {
        true => keys_page(),
        false => {
            let mut actions = stage_actions(
                feature,
                stage,
                state.ctx,
                &state.plan,
                &state.criteria,
                notes_of(state.probe),
            );
            actions.retain(|(_, o)| !undoes_merged(o, state.probe));
            actions.extend(round_plan_actions(feature, stage, state.probe));
            actions.extend(place_action(state.place.as_deref(), stage));
            actions.extend(stop_action(feature, stage));
            with_pull_actions(&mut actions, feature, stage, state.ctx);
            actions.extend(editing_actions(panel.note.is_some()));
            stage_page(feature, stage, state, &actions, now)
        }
    };
    // The search hides the criteria not saying what it looks for, and opens
    // those that do, the places tinted.
    let search = (panel.search.as_ref()).filter(|_| !panel.keys && shows_criteria(feature, stage));
    let whole = search.is_some_and(|s| s.whole);
    let needle = search
        .map(crate::manual_test::Search::needle)
        .filter(|n| !n.is_empty());
    let mut said = None;
    if let Some(needle) = &needle {
        let shown = crate::manual_test::shown(&state.criteria, needle, whole);
        // Under the field, how many of the list show: the count by the field
        // is of the places found, more than one in a criterion as may be.
        let k = shown.iter().filter(|s| **s).count();
        said = Some(format!(
            "showing {k} of {} saying \u{201c}{needle}\u{201d}",
            plural(shown.len(), "criterion").replace("criterions", "criteria")
        ));
        let sheets = (page.sections.iter_mut()).filter_map(|s| match s {
            PageSection::Finding(card) if card.sheet.is_some() => Some(card),
            _ => None,
        });
        for (card, shown) in sheets.zip(shown) {
            card.collapsed = !shown;
            if let Some(sheet) = card.sheet.as_mut() {
                sheet.hidden = !shown;
                sheet.needle = needle.clone();
                sheet.whole = whole;
            }
        }
    }
    if panel.focus == TabArea::Findings && !panel.expanded {
        let cards = (page.sections.iter_mut()).filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        });
        if let Some(card) = cards.into_iter().nth(panel.finding) {
            card.selected = true;
            // A criterion stays folded as the user left it: selecting it
            // only shows which it is.
            if card.sheet.is_none() {
                card.collapsed = false;
            }
            // A review card's decision is what the selected one asks for:
            // its first button, Fix or Accept, is the page's one primary
            // while the keyboard is on the cards.
            let posts = feature.posts_review();
            let wants = |label: &str| {
                [ItemOf::Finding, ItemOf::Suggestion]
                    .iter()
                    .any(|of| label == crate::feature::review_items::words_in(*of, posts).want)
            };
            let first = card.buttons.first().copied();
            if card.folds
                && let Some(b) = first
                && page
                    .buttons
                    .get(b)
                    .is_some_and(|button| wants(&button.label))
            {
                // The page's bar shows it, and what it had goes back among
                // the bar's plain ones.
                fill_only(&mut page.buttons, &mut page.primary, b);
            }
            if panel.armed.is_some() {
                "Press Del again".clone_into(&mut card.state);
            }
        }
        for section in &mut page.sections {
            if let PageSection::ShipGroup(group) = section {
                group.selected = Some(panel.finding);
            }
        }
    }
    // A review card the user opened or folded stays so, selected or not.
    if !panel.cards_open.is_empty() {
        let ids = review_cards(feature, stage, notes_of(state.probe));
        let cards = (page.sections.iter_mut()).filter_map(|s| match s {
            PageSection::Finding(card) if card.folds => Some(card),
            _ => None,
        });
        for (card, id) in cards.zip(ids.iter().map(|c| c.id())) {
            if let Some((_, open)) = panel.cards_open.iter().find(|(c, _)| *c == id) {
                card.collapsed = !open;
            }
        }
    }
    // The note being written, on its card in place of the one saved.
    if let Some((i, field)) = panel
        .note
        .as_ref()
        .filter(|_| !panel.keys && !panel.expanded)
    {
        let mut cards = (page.sections.iter_mut()).filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        });
        match i {
            CardNote::Criterion(i) => {
                let card = cards.nth(*i);
                if let Some(card) = card.filter(|_| marks_criteria(feature, stage)) {
                    // Opened to write in, and shown, whatever the search says.
                    card.collapsed = false;
                    if let Some(sheet) = card.sheet.as_mut() {
                        sheet.hidden = false;
                    }
                    card.field = Some(field.line());
                    card.note = String::new();
                    card.buttons.extend(note_buttons(&page.buttons));
                    // Its Fail shows chosen while what went wrong is written.
                    for b in &card.buttons {
                        if let Some(button) = page.buttons.get_mut(*b)
                            && button.label.starts_with("Fail")
                        {
                            button.style = ButtonStyle::Failed;
                        }
                    }
                }
            }
            // The live card, before those of earlier reviews with its id; a
            // suggestion's only on Plan review's page, a finding's on AI
            // review's.
            CardNote::Item { of, id, .. } => {
                let card = cards.find(|c| c.place.split(" \u{b7} ").next() == Some(id.as_str()));
                let here = (*of == ItemOf::Suggestion) == (stage == Stage::PlanReview);
                if let Some(card) = card.filter(|_| here && can_decide(feature, stage)) {
                    card.collapsed = false;
                    card.field = Some(field.line());
                    // Its buttons stay its own, unplaced, while it is written.
                    card.comment = String::new();
                    card.buttons.extend(note_buttons(&page.buttons));
                }
            }
        }
    }
    // Save is what to press next while a note is written.
    if let Some(save) = note_buttons(&page.buttons).last().copied() {
        fill_only(&mut page.buttons, &mut page.primary, save);
    }
    // A finished feature's code isn't read (`ship::probe_finished`).
    let read = state.probe.filter(|_| feature.status == Status::Open);
    let repos_error = match read.map(|p| &p.reading) {
        Some(Err(why)) => Some(format!("Couldn't read the code: {why}")),
        _ => None,
    };
    let which = (slugs.iter())
        .position(|s| *s == feature.slug)
        .map(|i| (i, slugs.len()));
    let mut sub = format!(
        "{} \u{b7} {} \u{b7} started {}",
        feature.slug,
        short_folder(&feature.dir),
        crate::clock::ago(now, feature.created)
    );
    if feature.status == Status::Done {
        sub = format!("done \u{b7} {sub}");
    }
    let ships = match stage {
        Stage::Ship => ship_rows(feature, state.probe),
        _ => Vec::new(),
    };
    let carded = !panel.expanded
        && !(review_cards(feature, stage, notes_of(state.probe)).is_empty()
            && check_rows(feature, stage).is_empty()
            && ships.is_empty());
    let focus = match panel.focus {
        TabArea::Repos if repos.is_empty() => TabArea::Stages,
        TabArea::Findings if !carded => TabArea::Stages,
        focus => focus,
    };
    let at = panel.repo.min(repos.len().saturating_sub(1));
    let picked = (entries.get(at))
        .filter(|_| panel.expanded && focus == TabArea::Repos)
        .and_then(|e| panel.picked(feature, *e));
    let review_switch =
        (stage == Stage::CodeReview && !panel.keys && crate::feature_diff::review_views(feature))
            .then(|| {
                let files = review_files(feature, state);
                let (next, buttons) = review_toolbar(panel, feature, files, state.ctx);
                // The code's view hides the fixes' card: its chips go on
                // the bar, by its buttons.
                let code = crate::feature_diff::review_page(panel, feature);
                chrome::ReviewSwitch {
                    view: panel.review_view_of(feature),
                    files,
                    fixes: fixes_word(feature, notes_of(state.probe)),
                    next,
                    buttons,
                    picks: match code {
                        true => state.fix_picks.clone(),
                        false => Vec::new(),
                    },
                    model: state.fix_model.clone().filter(|_| code),
                }
            });
    let test_strip = (!panel.keys && shows_criteria(feature, stage))
        .then(|| test_strip(&state.criteria))
        .flatten()
        .map(|mut strip| {
            strip.shown = said;
            // Its count is of the places found, as the page lays them out:
            // `Gerdr::feature_tab_view` gives it.
            strip.search = panel.search.as_ref().map(|search| chrome::SearchBar {
                query: search.query.line(),
                placeholder: chrome::CRITERIA_SEARCH_HINT.into(),
                count: String::new(),
                found: needle.is_none(),
                focused: search.typing,
                whole: search.whole,
            });
            strip
        });
    let mut view = FeatureTabView {
        review_page: crate::feature_diff::review_page(panel, feature),
        review_switch,
        test_strip,
        review_keys: false,
        title: feature.title.clone(),
        sub,
        which,
        remove: remove_label(feature),
        rail,
        back,
        repo: at,
        repos,
        repos_error,
        expanded: panel.expanded,
        more: repos_offered(feature, &panel.sources).len(),
        picked,
        repo_notes,
        focus,
        page,
        activity: activity(feature, now),
        // `Gerdr::feature_tab_view` says what runs.
        running: Vec::new(),
        running_more: 0,
        scroll: std::array::from_fn(|col| {
            let s = match col == TabColumn::Page as usize {
                true => panel.page_scroll(feature),
                false => Some(&panel.scroll[col]),
            };
            s.map_or(0.0, |s| s.pos() - s.overscroll())
        }),
        hover: panel.tab_hover,
        hints: with_ship_hints(
            tab_hints(
                panel,
                feature,
                stage,
                slugs,
                state.ctx,
                AgentChoices {
                    picks: &state.picks,
                    model: state.model.is_some(),
                    place: state.place.is_some(),
                    fix_picks: &state.fix_picks,
                    fix_model: state.fix_model.is_some(),
                },
                &state.plan,
                &ships,
                notes_of(state.probe),
            ),
            panel,
            feature,
            stage,
            (state.ctx, state.probe),
            &ships,
        ),
        marks: chrome::PageMarks {
            found_at: search.and_then(|s| s.at).filter(|_| needle.is_some()),
            pick: panel.pick.clone(),
        },
    };
    // The review under the rail has its own keys and footer.
    if view.review_page {
        view.hints.clear();
        view.review_keys = !panel.rail_keys;
    }
    view
}

/// How far `col` of the tab laid out as `layout` scrolls, for its
/// `Scroll`: in pixels, a notch as the sidebar's.
pub(super) fn tab_bounds(layout: &chrome::FeatureTabLayout, col: TabColumn) -> Bounds {
    let view = layout.columns[col as usize].map_or(0, |r| r.size.h);
    Bounds {
        max: f64::from(layout.scroll_room(col)),
        notch: TAB_NOTCH,
        unit: 1.0,
        next: 1.0,
        prev: 1.0,
        view: f64::from(view),
    }
}
