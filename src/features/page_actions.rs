//! The buttons of each stage page, and the small helpers behind them.

use super::*;

/// Why `stage`'s page of `feature` can't decide the card at `at` of
/// `cards`, or edit the comment it is.
pub(super) fn card_refused(
    feature: &Feature,
    stage: Stage,
    cards: &[ReviewCard],
    at: usize,
) -> Option<&'static str> {
    match cards.get(at).copied().and_then(ReviewCard::diff_note) {
        Some(c) => why_not_edit_note(feature, stage, c),
        None => why_not_decide(feature, stage),
    }
}

/// A move of the feature tab's keys, through its stages or its
/// repositories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Back,
    On,
    First,
    Last,
}

impl Step {
    /// Where it goes from `at`, of `0..=last`: it stops at the ends.
    pub(super) fn from(self, at: usize, last: usize) -> usize {
        match self {
            Self::Back => at.saturating_sub(1),
            Self::On => (at + 1).min(last),
            Self::First => 0,
            Self::Last => last,
        }
    }
}

/// Where `stage` is in `Stage::ALL`.
pub(super) fn stage_index(stage: Stage) -> usize {
    Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0)
}

/// What Enter does on `stage`'s page of `feature`: its primary button,
/// else, on a later stage, says which comes first. On a done
/// stage it goes to the current one: Reopen has its own key.
pub(super) fn page_enter(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<PanelOutcome> {
    page_actions(feature, stage, ctx)
        .into_iter()
        .find(|(b, _)| b.style == ButtonStyle::Primary)
        .map(|(_, outcome)| outcome)
        // Its checks going, Enter says so: it never stops them, and never
        // marks the stage done under them.
        .or_else(|| verify_going(feature, stage, ctx).then_some(PanelOutcome::RunChecks))
        .or_else(|| row_action(feature, PanelRow::Stage(stage), ctx))
        .filter(|outcome| !matches!(outcome, PanelOutcome::Untick(_)))
}

/// Whether `stage` is the Plan review `feature` is at: what can be skipped.
pub(super) fn plan_review_open(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::PlanReview && feature.status == Status::Open && feature.current() == Some(stage)
}

/// The key of a done stage's Reopen.
pub(super) const REOPEN_KEY: &str = "Ctrl+Shift+Enter";
/// The key of the current stage's Start over.
pub(super) const START_OVER_KEY: &str = "Ctrl+Shift+Del";
/// The key picking the selected stage's agent's model, beside Ctrl+Shift+A
/// picking the agent: its own, as Ctrl+Shift+I shows the setup log on the
/// same pages (and Alt+A amends in the repositories panel).
pub(super) const MODEL_KEY: &str = "Alt+Shift+A";
/// The key abandoning the feature (or removing a done one's worktrees),
/// asking first. Del alone never does: it is the focused card's or row's
/// (and Alt+Del deletes a branch in the repositories panel).
pub const REMOVE_KEY: &str = "Alt+Shift+Del";
/// Why Ctrl+Enter makes no fixes while what the user wrote needs a plan.
pub(super) const PLAN_FIRST: &str = crate::worktree_diff::PLAN_FIRST;
/// Why `f` does nothing on a comment left in the diff.
pub(super) const DIFF_NOTE: &str = "This comment goes with the fixes: Del deletes it";
/// Why a comment that went to Build can't change.
pub(super) const SENT_NOTE: &str = crate::diff_view::SENT_COMMENT;

/// What opens a fix round's plan, on every round card.
pub(super) const OPEN_PLAN: &str = "Open plan";

/// The open fix round of `feature`, on the page of the stage that asked.
pub(super) fn fix_round_on(feature: &Feature, stage: Stage) -> Option<&FixRound> {
    (feature.status == Status::Open)
        .then_some(feature.fix_round.as_ref())
        .flatten()
        .filter(|r| {
            // Its first commit sends the feature back to an earlier stage,
            // and the round goes on there.
            r.stage == stage
                || (!r.before_fixing() && feature.current() == Some(stage) && stage < r.stage)
        })
}

/// The last fix round of `feature`, its fixes sent, on the page of the stage
/// that asked, or of an earlier one the feature came back to.
pub(super) fn last_round_on(feature: &Feature, stage: Stage) -> Option<&FixRound> {
    (feature.status == Status::Open && feature.fix_round.is_none())
        .then_some(feature.last_fix_round.as_ref())
        .flatten()
        .filter(|r| r.stage == stage || (feature.current() == Some(stage) && stage < r.stage))
}

/// Whether `stage`'s page can send the fixes of the last round again: only
/// where the feature is at, and only with nothing new of the stage's own to
/// send, as that is what goes on then, not the old round.
pub(super) fn sends_round_again(feature: &Feature, stage: Stage, ctx: RowCtx) -> bool {
    last_round_on(feature, stage).is_some()
        && feature.current() == Some(stage)
        && !has_new_fixes(feature, stage, ctx)
}

/// Whether `stage`'s page can send `feature`'s Verify failures to Build: at
/// Verify, its last run failed, nothing running and no round open.
pub(super) fn verify_sends(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::Verify
        && stage_mark(feature, stage) == StageMark::Current
        && feature.fix_round.is_none()
        && !checks_running(feature)
        && (feature.verify.as_ref()).is_some_and(|v| feature::verify::to_fix(v).is_some())
}

/// Whether Manual test of `feature` (`stage`) has fixes in its code that
/// Verify hasn't run on: marking it passed takes it back to Verify.
pub(super) fn fixes_unverified(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest
        && feature.status == Status::Open
        && feature.current() == Some(stage)
        && feature.unverified.is_some()
}

/// Whether `stage` of `feature` has fixes of its own waiting to be sent, or
/// to be decided: findings, failed checks or criteria, comments in the diff.
fn has_new_fixes(feature: &Feature, stage: Stage, ctx: RowCtx) -> bool {
    match stage {
        // Those to comment on GitHub go with the review, not to a fix round.
        Stage::AiReview => (feature.review.as_ref()).is_some_and(|r| {
            r.count(FindingState::Open) > 0 || (!r.posts && r.count(FindingState::Fix) > 0)
        }),
        Stage::Verify => {
            (feature.verify.as_ref()).is_some_and(|v| feature::verify::to_fix(v).is_some())
        }
        Stage::CodeReview => ctx.comments > 0,
        Stage::ManualTest => ctx.manual.failed > ctx.manual.sent,
        _ => false,
    }
}

/// Whether `stage`'s page opens a fix plan already sent: its Enter does
/// other things.
pub(super) fn opens_fix_plan(feature: &Feature, stage: Stage) -> bool {
    last_round_on(feature, stage).is_some()
        || fix_round_on(feature, stage).is_some_and(|r| !r.before_fixing())
}

/// The buttons of `round`, what Enter does first: what it waits for.
fn fix_round_buttons(
    feature: &Feature,
    round: &FixRound,
    ctx: RowCtx,
) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let mut out = Vec::new();
    // A fresh agent has its prompt to go first: nothing is planned before.
    let waits = (feature.runs.get(&Stage::Build)).is_some_and(|run| run.waits_to_send());
    if waits {
        out.push((
            button("Send prompt", ButtonStyle::Primary),
            PanelOutcome::SendPrompt(Stage::Build),
        ));
    }
    match round.phase {
        _ if waits => {}
        FixPhase::Ready => {
            out.push((
                button("Review", ButtonStyle::Primary),
                PanelOutcome::ReviewFixPlan,
            ));
            out.push((
                button("Make fixes", ButtonStyle::Plain),
                PanelOutcome::MakeFixes,
            ));
        }
        FixPhase::Planning if round.stalled.is_some() => out.push((
            button("Ask again", ButtonStyle::Primary),
            PanelOutcome::AskFixPlan,
        )),
        FixPhase::Fixing if round.stalled.is_some() => out.push((
            button("Make fixes again", ButtonStyle::Primary),
            PanelOutcome::MakeFixes,
        )),
        _ => {}
    }
    if let Some((label, outcome)) = agent_button(feature, Stage::Build, ctx) {
        let style = match out.is_empty() {
            true => ButtonStyle::Primary,
            false => ButtonStyle::Plain,
        };
        out.push((button(label, style), outcome));
    }
    if !round.before_fixing() && round.has_plan() {
        out.push((
            button(OPEN_PLAN, ButtonStyle::Plain),
            PanelOutcome::ReviewFixPlan,
        ));
    }
    if round.can_drop() {
        out.push((
            button(round.drop_label(), ButtonStyle::Plain),
            PanelOutcome::DropFixPlan,
        ));
    }
    out
}

/// The buttons on `stage`'s page of `feature`, each with what it does:
/// what Enter does first (the primary; on a done stage, going to the
/// current one, then Reopen unless it is locked), then its agent's tab, a
/// fresh agent, starting over, Verify's evidence, AI review's fixes to
/// send, and Ship's pull requests. A later stage has none.
/// The page's clicks and Enter both go by this list. Where each is drawn is
/// the page's to say: on the card of what it acts on, else at the title.
pub fn page_actions(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Vec<(PageButton, PanelOutcome)> {
    let mut out = writers_page_actions(feature, stage, ctx);
    if feature.writer.is_some() {
        // Its review is on the machine that writes it, until it is taken over.
        for (button, outcome) in &mut out {
            if matches!(outcome, PanelOutcome::OpenDoc(_)) {
                button.label = "Open".to_owned();
                button.style = ButtonStyle::Plain;
            }
        }
        if feature.status == Status::Open {
            let label = "Take over".to_owned();
            let style = ButtonStyle::Plain;
            out.push((PageButton { label, style }, PanelOutcome::TakeOver));
        }
    }
    if hands_back(feature) {
        let label = "Hand back".to_owned();
        let style = ButtonStyle::Plain;
        out.push((PageButton { label, style }, PanelOutcome::HandBack));
    }
    if stage == own_page(feature) {
        out.extend(own_actions(feature));
    }
    manual_order(feature, stage, ctx, &mut out);
    out
}

/// The page that shows `feature`'s own agents: that of the stage it is at.
pub(super) fn own_page(feature: &Feature) -> Stage {
    feature.current().unwrap_or(Stage::Ship)
}

/// The way to each of `feature`'s own agents whose tab is there, on its card.
fn own_actions(feature: &Feature) -> impl Iterator<Item = (PageButton, PanelOutcome)> + '_ {
    (feature.own_agents.iter())
        .filter(|a| a.pane.is_some())
        .map(|a| {
            let button = PageButton {
                label: "Show agent".to_owned(),
                style: ButtonStyle::Plain,
            };
            (button, PanelOutcome::ShowOwnAgent(a.n))
        })
}

/// Manual test's buttons in the order of every page: its gate, Mark passed,
/// last before the primary, which the layout puts rightmost. With nothing
/// else to press, the primary is starting the sandbox the criteria are tried
/// in, while none runs and some are left to try.
fn manual_order(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    out: &mut Vec<(PageButton, PanelOutcome)>,
) {
    use crate::manual_test::Outcome;
    if !tries_criteria(feature, stage) {
        return;
    }
    let mark = PanelOutcome::Manual(Outcome::MarkPassed);
    if let Some(at) = out.iter().position(|(_, o)| *o == mark) {
        let entry = out.remove(at);
        out.push(entry);
    }
    let primary = out.iter().any(|(b, _)| b.style == ButtonStyle::Primary);
    if !primary && ctx.manual.untested > 0 {
        let start = (out.iter_mut()).find(|(_, o)| *o == PanelOutcome::StartSandbox);
        if let Some((button, _)) = start {
            button.style = ButtonStyle::Primary;
        }
    }
}

/// Why Manual test's Mark passed (Mark done, on someone else's pull request)
/// is greyed: what must happen first.
pub(super) fn mark_tip(page: &chrome::StagePage, i: usize) -> Option<String> {
    let button = page
        .buttons
        .get(i)
        .filter(|b| b.style == ButtonStyle::Off)?;
    let why = match button.label.as_str() {
        "Mark passed" => "Every criterion passes first: pressed, it says which are left",
        "Mark done" => "Every criterion is tried first: pressed, it says which are left",
        _ => return None,
    };
    Some(why.to_owned())
}

/// Whether `feature`, written here, can go back to the machine it was taken
/// over from (`Feature::came_from`).
pub(super) fn hands_back(feature: &Feature) -> bool {
    feature.writer.is_none() && feature.came_from.is_some() && feature.status == Status::Open
}

/// The buttons of `feature`'s sandbox on `stage`'s page, where it offers
/// one: on Manual test's whenever its criteria can be marked.
fn sandbox_actions(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Vec<(PageButton, PanelOutcome)> {
    let mut out = Vec::new();
    if !offers_sandbox(feature, stage) {
        return out;
    }
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    if ctx.sandbox_busy {
        out.push((
            button("Stop", ButtonStyle::Plain),
            PanelOutcome::StopSandbox,
        ));
    } else if sandbox_alive(feature) {
        out.push((
            button("Open app", ButtonStyle::Plain),
            PanelOutcome::OpenSandbox,
        ));
        for (i, page) in sandbox_pages(feature).iter().enumerate().skip(1) {
            out.push((
                button(&page.name, ButtonStyle::Quiet),
                PanelOutcome::OpenSandboxPage(i),
            ));
        }
        out.push((
            button("Edit pages\u{2026}", ButtonStyle::Quiet),
            PanelOutcome::EditCommands(CommandsTab::Pages),
        ));
        // A phone reaches only what runs on this machine.
        if !sandbox_elsewhere(feature) {
            let label = match ctx.lan_open {
                true => "Close to network",
                false => "Open to network\u{2026}",
            };
            out.push((button(label, ButtonStyle::Quiet), PanelOutcome::SandboxLan));
        }
        // Here they are in a tab of their own already.
        if sandbox_elsewhere(feature) {
            out.push((
                button("Show terminals", ButtonStyle::Quiet),
                PanelOutcome::ShowSandbox,
            ));
        }
        out.extend([
            (
                button("Restart", ButtonStyle::Quiet),
                PanelOutcome::RestartSandbox,
            ),
            (
                button("Reset data\u{2026}", ButtonStyle::Quiet),
                PanelOutcome::ResetSandbox,
            ),
            (
                button("Stop\u{2026}", ButtonStyle::Quiet),
                PanelOutcome::StopSandbox,
            ),
        ]);
    } else {
        out.push((
            button("Start sandbox\u{2026}", ButtonStyle::Plain),
            PanelOutcome::StartSandbox,
        ));
        if feature.sandbox.is_some() {
            out.push((
                button("Remove sandbox\u{2026}", ButtonStyle::Quiet),
                PanelOutcome::RemoveSandbox,
            ));
        }
    }
    out.push((
        button("Commands\u{2026}", ButtonStyle::Quiet),
        PanelOutcome::EditCommands(commands_tab(stage)),
    ));
    out
}

/// `page_actions` on the machine writing `feature`.
fn writers_page_actions(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Vec<(PageButton, PanelOutcome)> {
    let mark = stage_mark(feature, stage);
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    if mark == StageMark::Later {
        let mut out = review_actions(feature, stage);
        // The fixes its comments asked for go on from an earlier stage, and
        // its page stays where they are followed.
        if stage == Stage::CodeReview {
            if let Some(round) = fix_round_on(feature, stage) {
                out.extend(fix_round_buttons(feature, round, ctx));
            }
            if last_round_on(feature, stage).is_some_and(FixRound::has_plan) {
                out.push((
                    button(OPEN_PLAN, ButtonStyle::Plain),
                    PanelOutcome::ReviewFixPlan,
                ));
            }
        }
        // Manual test's sandbox stays to try more while its fixes are gone
        // over: what fails waits for it.
        out.extend(sandbox_actions(feature, stage, ctx));
        return out;
    }
    let row = PanelRow::Stage(stage);
    let mut out = Vec::new();
    // First whatever the stage's mark: a fresh Build agent for fixes waits
    // on a done Build.
    let send = PanelOutcome::SendPrompt(stage);
    // The open fix plan, on the page of the stage that asked, comes before
    // all else: its stage waits for it.
    let round = fix_round_on(feature, stage);
    if let Some(round) = round {
        out.extend(fix_round_buttons(feature, round, ctx));
    }
    // Over, it asks nothing: Enter goes on with the stage.
    if let Some(last) = last_round_on(feature, stage) {
        if last.has_plan() {
            out.push((
                button(OPEN_PLAN, ButtonStyle::Plain),
                PanelOutcome::ReviewFixPlan,
            ));
        }
        if let Some(round) =
            last_round_on(feature, stage).filter(|_| sends_round_again(feature, stage, ctx))
        {
            let label = round_label(round).to_lowercase();
            out.push((
                button(&format!("Send {label} again"), ButtonStyle::Quiet),
                PanelOutcome::ResendFixes,
            ));
        }
    }
    // Verify's failures hold no word of the user's: they need no plan, but
    // planning them is what comes next by default, so the user sees the
    // fixes before they are made (Make fixes skips that).
    let plan_fixes = verify_sends(feature, stage);
    if plan_fixes {
        out.push((
            button("Make fixes", ButtonStyle::Plain),
            PanelOutcome::MakeVerifyFixes,
        ));
        out.push((
            button("Plan fixes", ButtonStyle::Primary),
            PanelOutcome::PlanFixes,
        ));
    }
    let waits = feature.status == Status::Open
        && (feature.runs.get(&stage)).is_some_and(|run| run.waits_to_send());
    if waits {
        out.push((button("Send prompt", ButtonStyle::Primary), send.clone()));
    }
    // Work sent back to the Build agent, seen from the stage the feature is
    // at: what it waits for comes first.
    let back = rework_on(feature, stage, ctx);
    if back == Some(Rework::Waits) && round.is_none() {
        out.push((
            button("Send prompt", ButtonStyle::Primary),
            PanelOutcome::SendPrompt(Stage::Build),
        ));
    }
    // Shipped, it can ship more: what is ticked and left to run.
    if mark == StageMark::Done && stage == Stage::Ship && feature.ships_more() && ctx.ship_left > 0
    {
        let label = format!("Ship {}\u{2026}", steps_word(ctx.ship_left));
        out.push((button(&label, ButtonStyle::Primary), PanelOutcome::Ship));
    }
    if let (StageMark::Done, Some(current)) = (mark, feature.current()) {
        let label = format!("Go to {}", current.label());
        let style = match waits {
            true => ButtonStyle::Plain,
            false => ButtonStyle::Primary,
        };
        out.push((button(&label, style), PanelOutcome::GoToCurrent));
    }
    if let (Some(label), Some(main)) = (
        stage_button(feature, stage, ctx),
        row_action(feature, row, ctx).filter(|main| *main != send),
    ) {
        // Reopen is no default: Enter leaves a done stage be.
        let style = match main {
            PanelOutcome::Untick(_) => ButtonStyle::Plain,
            // What stops it is to be done first: pressed, it only says so.
            PanelOutcome::Ship if ctx.ship_blocked => ButtonStyle::Plain,
            _ if back == Some(Rework::Waits) || round.is_some() || plan_fixes => ButtonStyle::Plain,
            _ => ButtonStyle::Primary,
        };
        let direct = main == PanelOutcome::MakeFindingFixes;
        out.push((button(label, style), main));
        // Nothing asks for a plan, but one can still be wanted.
        if direct {
            out.push((
                button("Plan fixes", ButtonStyle::Plain),
                PanelOutcome::SendFixes,
            ));
        }
    }
    if tries_criteria(feature, stage) && ctx.criteria > 0 {
        use crate::manual_test::Outcome;
        // What the fix plan, Build or Verify's failures wait for comes first.
        let free = back != Some(Rework::Waits) && round.is_none() && !plan_fixes;
        let style = |next: bool| match free && next {
            true => ButtonStyle::Primary,
            false => ButtonStyle::Plain,
        };
        // Found while a round is open, they wait for the next one. As on
        // every page: what the user wrote goes through a plan.
        if manual_sends(feature, ctx) {
            let own = manual_own_words(ctx).is_some();
            if !own {
                out.push((
                    button("Make fixes", style(true)),
                    PanelOutcome::Manual(Outcome::MakeFixes),
                ));
            }
            out.push((
                button("Plan fixes", style(own)),
                PanelOutcome::Manual(Outcome::SendFailures),
            ));
        }
        // On someone else's pull request what failed goes with the review:
        // it is done once every criterion is tried.
        let (label, ready) = match feature.posts_review() {
            true if ctx.manual.failed > 0 => ("Mark done", ctx.manual.untested == 0),
            true => ("Mark passed", ctx.manual.untested == 0),
            false => ("Mark passed", ctx.manual.passed == ctx.criteria),
        };
        // Greyed while criteria are left, saying why (`mark_tip`); it
        // keeps its place, last before the primary (`page_actions`).
        let mark = match ready {
            true => style(true),
            false => ButtonStyle::Off,
        };
        out.push((
            button(label, mark),
            PanelOutcome::Manual(Outcome::MarkPassed),
        ));
    }
    let has =
        |out: &[(PageButton, PanelOutcome)], o: &PanelOutcome| out.iter().any(|(_, x)| x == o);
    // Also on a done stage: its agent's chat is still worth reading.
    if let Some((label, outcome)) = agent_button(feature, stage, ctx).filter(|(_, o)| !has(&out, o))
    {
        out.push((button(label, ButtonStyle::Plain), outcome));
    }
    // Named apart from this stage's own agent's button on the same page.
    if back.is_some()
        && let Some((label, outcome)) =
            agent_button(feature, Stage::Build, ctx).filter(|(_, o)| !has(&out, o))
    {
        let label = label.replace("agent", "Build agent");
        out.push((button(&label, ButtonStyle::Plain), outcome));
    }
    // The brief's or the plan's file, once there: the page shows it, its tab
    // works on it. What opens that tab already is that button.
    let open = PanelOutcome::OpenDoc(stage);
    if ctx.wrote(stage) && !has(&out, &open) {
        // Nothing to review on a done stage: it is only read.
        let label = match mark {
            StageMark::Done => "Open",
            _ => "Review",
        };
        out.push((button(label, ButtonStyle::Plain), open));
    }
    if approves_doc(feature, stage, ctx) {
        let approve = PanelOutcome::ApproveDoc(stage);
        out.push((button("Approve", ButtonStyle::Plain), approve));
    }
    if plan_review_open(feature, stage) {
        let suggestions = PanelOutcome::ReviewInPlan;
        let deciding = crate::feature_plan_review::deciding(feature);
        if deciding.is_some() && !has(&out, &suggestions) {
            let label = crate::worktree_diff::REVIEW_IN_PLAN;
            out.push((button(label, ButtonStyle::Plain), suggestions));
        }
    }
    // The review's own, in one order on both review pages.
    out.extend(review_page_actions(feature, stage));
    if let Some(stage) = fresh_stage(feature, row) {
        let fresh = PanelOutcome::StartAgent(stage, true);
        out.push((button("Restart", ButtonStyle::Quiet), fresh));
    }
    if let Some(stage) = start_over_stage(feature, stage, ctx) {
        out.push((
            button("Start over\u{2026}", ButtonStyle::Quiet),
            PanelOutcome::StartOver(stage),
        ));
    }
    if let Some(page) = row_page(feature, row) {
        out.push((button("Evidence", ButtonStyle::Plain), page));
    }
    if offers_setup(feature, stage) {
        let ran = (feature.setup.as_ref()).is_some_and(|run| run.finished.is_some());
        let label = if ran { "Set up again" } else { "Set up" };
        if can_set_up(feature, ctx) {
            out.push((button(label, ButtonStyle::Quiet), PanelOutcome::SetUp));
        }
        if offers_setup_log(feature, stage) {
            out.push((
                button("Setup log", ButtonStyle::Quiet),
                PanelOutcome::SetupLog,
            ));
        }
        // It matters once Verify has nothing to run.
        let wanted = stage == Stage::Verify && mark == StageMark::Current && !ctx.checks;
        let style = match wanted {
            true => ButtonStyle::Plain,
            false => ButtonStyle::Quiet,
        };
        let edit = PanelOutcome::EditCommands(commands_tab(stage));
        out.push((button("Commands\u{2026}", style), edit));
    }
    out.extend(sandbox_actions(feature, stage, ctx));
    // The review pages' are with their own, above.
    if can_add_comment(feature, stage) && !is_review_stage(stage) {
        out.push((
            button("Add comment\u{2026}", ButtonStyle::Plain),
            PanelOutcome::AddComment,
        ));
    }
    let current = mark == StageMark::Current;
    out.extend(review_actions(feature, stage));
    if current && has_failed_checks(feature, stage) {
        // Before Run checks: after a failure it is the one wanted.
        let at = (out.iter().position(|(_, o)| *o == PanelOutcome::RunChecks)).unwrap_or(out.len());
        out.insert(
            at,
            (
                button("Run failed", ButtonStyle::Plain),
                PanelOutcome::RunFailed,
            ),
        );
    }
    if stage == Stage::Verify && can_stop_checks(feature) {
        // First, as Run checks is when none runs. Not the primary: Enter
        // pressed twice must not stop what it started.
        out.insert(
            0,
            (
                button("Stop all", ButtonStyle::Plain),
                PanelOutcome::StopChecks,
            ),
        );
    }
    for (i, row) in check_rows(feature, stage).iter().enumerate() {
        out.extend(check_buttons(i, row, feature, current));
    }
    if verify_going(feature, stage, ctx) {
        out.retain(|(_, outcome)| works_during_checks(outcome));
    }
    // The recap's: its summary, written again or by hand, while there is a
    // worktree to write it from.
    if stage == Stage::Ship && feature.ships() {
        let written = feature.shipping.summary.is_some();
        let label = if written {
            "Write again"
        } else {
            "Write summary"
        };
        out.push((
            button(label, ButtonStyle::Quiet),
            PanelOutcome::WriteSummary,
        ));
        out.push((
            button("Edit", ButtonStyle::Quiet),
            PanelOutcome::EditSummary,
        ));
    }
    // The Ship check, read again now: it reads on its own every few
    // seconds, but fetches origin only every few minutes.
    if stage == Stage::Ship && feature.ships() {
        out.push((
            button("Check again", ButtonStyle::Quiet),
            PanelOutcome::CheckShip,
        ));
    }
    // Each step that ran and can be undone, on its row (once the rows
    // show), the latest first: those of the repositories, then what
    // followed shipping.
    let mut undos: Vec<(u64, PageButton, PanelOutcome)> = Vec::new();
    if stage == Stage::Ship && feature.ships() && ctx.ship_read {
        let mut seen: Vec<(String, ShipStep)> = Vec::new();
        for done in feature.ship_log().iter().rev() {
            let step = done.step;
            let known = (seen.iter()).any(|(r, s)| *r == done.repo && feature::same_step(*s, step));
            if known {
                continue;
            }
            seen.push((done.repo.clone(), step));
            let undoable =
                done.undone.is_none() && ship::undo_of(feature, &done.repo, step).is_ok();
            if undoable {
                // Named, as two can sit side by side off their rows.
                let what = match step {
                    ShipStep::Push | ShipStep::PushBranch => "push",
                    ShipStep::OpenPr => "pull request",
                    ShipStep::Merge => "merge",
                    ShipStep::PushBase => "base push",
                };
                let label = match feature.many_repos() {
                    true => format!("Undo {what} in {}\u{2026}", done.repo),
                    false => format!("Undo {what}\u{2026}"),
                };
                undos.push((
                    done.at,
                    button(&label, ButtonStyle::Quiet),
                    PanelOutcome::UndoShip(done.repo.clone(), step),
                ));
            }
        }
    }
    if stage == after_stage(feature) && feature.status != Status::Open {
        for step in [feature::AfterStep::Close, feature::AfterStep::Remove] {
            let Some(done) = feature.after_done(step) else {
                continue;
            };
            if crate::ship_after::after_refusal(feature, step).is_none() {
                undos.push((
                    done.at,
                    button("Undo", ButtonStyle::Quiet),
                    PanelOutcome::UndoAfter(step),
                ));
            }
        }
    }
    undos.sort_by_key(|(at, ..)| std::cmp::Reverse(*at));
    out.extend(undos.into_iter().map(|(_, b, o)| (b, o)));
    if stage == after_stage(feature) && close_left(feature) {
        out.push((
            button("Close", ButtonStyle::Plain),
            PanelOutcome::CloseShipped,
        ));
    }
    if stage == Stage::Ship {
        // On its repository's card while it can ship, once the cards show;
        // else listed by name.
        let one = feature.shipping.prs.len() == 1 || feature.ships();
        let held = feature.ships() && !ctx.ship_read;
        for pr in feature.shipping.prs.iter().filter(|_| !held) {
            let label = match one {
                true => "Open PR".to_owned(),
                false => format!("Open {}", pr_tail(&pr.url)),
            };
            let open = PanelOutcome::OpenPr(pr.url.clone());
            out.push((button(&label, ButtonStyle::Plain), open));
        }
    }
    out
}

/// Whether `stage`'s page offers to set `feature`'s worktrees up: those
/// where tests and builds run, once reached.
pub(super) fn commands_tab(stage: Stage) -> CommandsTab {
    match stage {
        Stage::Build => CommandsTab::Setup,
        Stage::ManualTest => CommandsTab::Run,
        Stage::Verify => CommandsTab::Feature,
        _ => CommandsTab::Checks,
    }
}

/// Whether `stage`'s page of `feature` offers its sandbox: where it is tried
/// by hand, once there.
pub(super) fn offers_sandbox(feature: &Feature, stage: Stage) -> bool {
    !feature.repos.is_empty() && marks_criteria(feature, stage)
}

/// Whether `feature`'s sandbox is on another machine, as far as its terminals go.
pub(super) fn sandbox_elsewhere(feature: &Feature) -> bool {
    feature.writer.is_some()
        || feature
            .sandbox
            .as_ref()
            .is_some_and(|run| run.machine.is_some())
}

/// The pages of `feature`'s sandbox's app, the first the one Open app opens.
pub(super) fn sandbox_pages(feature: &Feature) -> &[feature::sandbox_pages::Page] {
    feature.sandbox.as_ref().map_or(&[], |run| &run.pages)
}

/// Whether `feature`'s sandbox runs.
pub(super) fn sandbox_alive(feature: &Feature) -> bool {
    feature.sandbox.as_ref().is_some_and(|run| run.alive())
}

/// Whether `stage`'s page of `feature` offers to set its worktrees up, and
/// so to edit the project's commands.
pub(super) fn offers_setup(feature: &Feature, stage: Stage) -> bool {
    matches!(stage, Stage::Build | Stage::Verify)
        && feature.status == Status::Open
        && !feature.repos.is_empty()
        && stage_mark(feature, stage) != StageMark::Later
}

/// Whether a setup of `feature` can start now: not while one runs, nor under
/// its checks, whose worktrees it would change.
pub(super) fn can_set_up(feature: &Feature, ctx: RowCtx) -> bool {
    !ctx.setting_up && !checks_going(feature, ctx)
}

/// Whether `stage`'s page of `feature` offers what its setup runs and printed:
/// once one started.
pub(super) fn offers_setup_log(feature: &Feature, stage: Stage) -> bool {
    offers_setup(feature, stage) && feature.setup.is_some()
}

/// The repository `rows` on from `at`, of `len`: it stops at the ends.
pub(super) fn strip_step(at: usize, rows: isize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    at.min(last).saturating_add_signed(rows).min(last)
}

/// The slug before (`back`) or after `slug` in `slugs`, round the ends;
/// `None` with only one.
pub(super) fn switch(slugs: &[String], slug: &str, back: bool) -> Option<String> {
    if slugs.len() < 2 {
        return None;
    }
    let at = slugs.iter().position(|s| s == slug).unwrap_or(0);
    let n = slugs.len();
    let to = if back { (at + n - 1) % n } else { (at + 1) % n };
    Some(slugs[to].clone())
}

/// The feature the features key shows, of `slugs` listed: the one open,
/// else the one last shown, else the newest.
pub(super) fn pick_feature(
    slugs: &[String],
    open: Option<&str>,
    last: Option<&str>,
) -> Option<String> {
    [open, last]
        .into_iter()
        .flatten()
        .find(|s| slugs.iter().any(|l| l == s))
        .map(str::to_owned)
        .or_else(|| slugs.last().cloned())
}

/// A folder as the chrome shows it, from the home folder as `~` and cut to
/// 60 characters in the middle (not `paths::tilde`, which never cuts).
pub(crate) fn short_folder(path: &Path) -> String {
    paths::display(path, home_dir().as_deref(), 60)
}


/// What a done stage's row says of its mark.
pub(super) fn mark_detail(mark: &feature::Mark, now: u64) -> String {
    let when = crate::clock::ago(now, mark.at);
    if mark.note.is_empty() {
        format!("done {when} by {}", mark.by)
    } else {
        format!("done {when} by {} \u{b7} {}", mark.by, mark.note)
    }
}

/// Keep the prompt run `n` of `stage` of `feature` started on, in AI work
/// folder `dir`; none when it can't be read.
pub(super) fn keep_prompt(
    dir: &Path,
    feature: &Feature,
    stage: Stage,
    n: u32,
) -> Option<ai_work::BlobRef> {
    let (_, run) = ai_work::observe::runs_of(feature).find(|(s, r)| *s == stage && r.n == n)?;
    let kept = std::fs::read(&run.prompt).and_then(|text| ai_work::blobs::put(dir, &text));
    kept.inspect_err(|err| {
        tracing::warn!("keeping the prompt {}: {err}", run.prompt.display());
    })
    .ok()
}

/// Keep the whole record of `feature`, as it is now, in AI work folder `dir`.
pub(super) fn snapshot(dir: &Path, feature: &Feature) -> std::io::Result<ai_work::BlobRef> {
    let record = serde_json::to_vec_pretty(feature).map_err(std::io::Error::other)?;
    ai_work::blobs::put(dir, &record)
}
