//! The Ship page: its checks, steps and log.

use super::*;

/// What Ship asks for, given the latest `probe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ShipAsk {
    /// No check yet.
    Checking,
    /// What stops it: the toast's text.
    Blocked(String),
    /// No step is ticked: it can finish, leaving these with uncommitted
    /// changes.
    Finish(Vec<String>),
    /// Every step ticked ran already.
    AllRan,
    Ready(Asked),
}

/// What `feature`'s ticks ask, as the latest `probe` found it, while Ship is
/// its stage.
pub(super) fn ship_planned(feature: &Feature, probe: Option<&Probe>) -> Option<Planned> {
    let check = probe?.ship.as_ref()?;
    Some(ship::plan_actions(feature, check))
}

/// What shipping `feature` asks for, given the latest `probe`.
pub(super) fn ship_ask(feature: &Feature, probe: Option<&Probe>) -> ShipAsk {
    let title = feature.title.as_str();
    let Some(probe) = probe else {
        return ShipAsk::Checking;
    };
    let Some(planned) = ship_planned(feature, Some(probe)) else {
        return match &probe.reading {
            Err(why) => ShipAsk::Blocked(blocked_text(&[read_blocker(why)])),
            Ok(_) => ShipAsk::Checking,
        };
    };
    if planned.nothing && feature.status == Status::Done {
        return ShipAsk::Blocked(format!("Nothing ticked to ship in \u{201c}{title}\u{201d}"));
    }
    if planned.nothing {
        let dirty = probe
            .reading
            .as_ref()
            .map_or(Vec::new(), |r| r.dirty.clone());
        return ShipAsk::Finish(dirty);
    }
    if !planned.blockers.is_empty() {
        return ShipAsk::Blocked(blocked_text(&planned.blockers));
    }
    // An agent works on its conflicts: what it ships is the agent's.
    if let Some(line) = resolving_line(feature) {
        return ShipAsk::Blocked(blocked_text(&[line.text]));
    }
    if planned.all_done {
        return ShipAsk::AllRan;
    }
    ShipAsk::Ready(Asked {
        actions: planned.actions,
        patch: probe.reading.as_ref().ok().map(|r| r.patch.clone()),
    })
}

/// What a feature's code that couldn't be read says in the Ship check.
fn read_blocker(why: &str) -> String {
    format!("Couldn't read the code: {why}")
}

/// The warning when a feature can't ship for `blockers`: the first, and how
/// many more. Its page names the feature.
fn blocked_text(blockers: &[String]) -> String {
    let first = blockers.first().map_or("", String::as_str);
    // What to do comes first: a toast cuts its end off.
    let first = match first.rsplit_once(": pull ") {
        Some((why, pull)) => format!("pull {pull} ({why})"),
        None => first.to_owned(),
    };
    match blockers.len() {
        0 | 1 => format!("Can't ship yet: {first}"),
        n => format!("Can't ship yet: {first}, and {} more", n - 1),
    }
}

/// The warning when the code of `title` changed, its marks from `stage`
/// on gone: it names the cause, as the page it shows over may say otherwise.
pub(super) fn changed_text(title: &str, stage: Stage) -> String {
    format!(
        "The code of \u{201c}{title}\u{201d} changed since it was checked: back to {}",
        stage.label()
    )
}

/// The success toast once the pull requests of `title` are open:
/// `opened` new ones, and `updated` ones it had open already, which the
/// push updated.
pub(super) fn opened_text(title: &str, opened: usize, updated: usize) -> String {
    let what = |n: usize| {
        if n == 1 {
            "pull request"
        } else {
            "pull requests"
        }
    };
    let title = format!("\u{201c}{title}\u{201d}");
    match (opened, updated) {
        (n, 0) => format!("Opened {n} {} for {title}", what(n)),
        (0, n) => format!("Updated {n} {} for {title}", what(n)),
        (o, u) => format!("Opened {o} and updated {u} {} for {title}", what(o + u)),
    }
}

/// `lines` as the panel has room for: past `MAX_CHECK_LINES`, the first
/// ones and how many more, in `tone`.
pub(super) fn capped(lines: Vec<CheckLine>, tone: CheckTone) -> Vec<CheckLine> {
    if lines.len() <= MAX_CHECK_LINES {
        return lines;
    }
    let more = lines.len() - (MAX_CHECK_LINES - 1);
    let mut lines = lines;
    lines.truncate(MAX_CHECK_LINES - 1);
    lines.push(CheckLine {
        text: format!("and {more} more"),
        tone,
    });
    lines
}

/// The Ship check the panel shows under the Ship row of `feature`, from
/// the latest `probe`: only while Ship is its current stage. `shipping`:
/// its pull requests are being opened now.
pub(super) fn check_lines(
    feature: &Feature,
    probe: Option<&Probe>,
    shipping: bool,
    now: u64,
) -> Vec<CheckLine> {
    if !feature.ships() {
        return Vec::new();
    }
    let line = |text: String, tone| CheckLine { text, tone };
    if shipping {
        return vec![line("shipping\u{2026}".into(), CheckTone::Note)];
    }
    if let Some(at) = feature
        .shipping
        .squash_at
        .filter(|_| feature.squash_going())
    {
        let when = crate::clock::ago(now, at);
        let text = format!("Squash started {when}: done once it is merged");
        return vec![line(text, CheckTone::Note)];
    }
    let check = probe.and_then(|p| p.ship.as_ref());
    let Some(check) = check else {
        return match probe.map(|p| &p.reading) {
            Some(Err(why)) => vec![line(read_blocker(why), CheckTone::Blocker)],
            _ => vec![line("checking\u{2026}".into(), CheckTone::Note)],
        };
    };
    let planned = ship::plan_actions(feature, check);
    // Failing first, each saying its way out; then the named checks passed.
    let passed = (feature.status == Status::Open)
        .then(|| passed_checks(feature, check, probe))
        .flatten();
    let resolving = resolving_line(feature);
    if !planned.blockers.is_empty() {
        let lines = (planned.blockers.iter())
            .map(|b| line(b.clone(), CheckTone::Failed))
            .chain(resolving)
            .collect();
        let mut lines = capped(lines, CheckTone::Failed);
        if lines.len() < MAX_CHECK_LINES {
            lines.extend(passed);
        }
        return lines;
    }
    let more = feature.status == Status::Done;
    let waits = pr_wait(feature, probe).filter(|_| more);
    let first = match (planned.nothing, planned.all_done, more) {
        _ if let Some(resolving) = &resolving => resolving.clone(),
        _ if let Some(waits) = &waits => {
            let mut text = waits.clone();
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            line(text, CheckTone::Note)
        }
        (true, _, false) => line(NOTHING_TICKED.into(), CheckTone::Note),
        // Its pull requests merged on GitHub: that is what shipped means.
        (_, _, true) if !feature.shipping.prs.is_empty() && planned.steps() == 0 => {
            line(MERGED_DONE.into(), CheckTone::Ready)
        }
        (true, _, true) => line(TICK_MORE.into(), CheckTone::Note),
        (false, true, false) => line(ALL_RAN.into(), CheckTone::Ready),
        (false, true, true) => line(ALL_RAN_DONE.into(), CheckTone::Note),
        (false, false, false) => line("Ready to ship".into(), CheckTone::Ready),
        (false, false, true) => line(steps_left(planned.steps()), CheckTone::Ready),
    };
    // Ready but for origin's base: its new commits conflict, so a pull
    // request would, and so would its base once pulled.
    let conflicts: Vec<CheckLine> = (check.repos.iter())
        .filter(|_| !planned.nothing && !planned.all_done)
        .filter(|repo| !repo.merged)
        .filter_map(|repo| {
            let state = probe?.bases.get(&repo.key)?;
            let new = count_of(state.behind, "new commit", "new commits");
            (!state.conflicts.is_empty()).then(|| {
                let text = format!(
                    "{}: {} has {new} that conflict: take them in first",
                    repo.key, state.onto
                );
                line(text, CheckTone::Blocker)
            })
        })
        .collect();
    let first: Vec<CheckLine> = match conflicts.is_empty() {
        true => vec![first],
        false => conflicts,
    };
    let notes = check.notes.iter().map(|n| line(n.clone(), CheckTone::Note));
    let passed = passed.filter(|_| resolving.is_none());
    // Shipped, what comes next: the end of its life, said plainly.
    let next = (more && (planned.nothing || planned.all_done)).then(|| {
        let what = worktrees_words(feature);
        let text = match waits {
            Some(_) => format!("Next: once it merges, Remove {what} in the header"),
            None => format!("Next: Remove {what} in the header; Restore brings them back"),
        };
        line(text, CheckTone::Note)
    });
    let lines = first.into_iter().chain(next).chain(passed).chain(notes).collect();
    capped(lines, CheckTone::Note)
}

/// The named checks `feature` passes before it ships, on one line ("checks
/// passed \u{b7} review done \u{b7} base current \u{b7} no conflicts"), as
/// `check` and `probe` read it; none when it passes none of them. Those it
/// fails say so on lines of their own, above it.
fn passed_checks(feature: &Feature, check: &ship::ShipCheck, probe: Option<&Probe>) -> Option<CheckLine> {
    let stale = check.blockers.iter().any(|b| b.starts_with("Code changed since"));
    let bases = probe.map(|p| &p.bases);
    let open = (feature.review.as_ref()).map_or(0, |r| r.open());
    let reviews = [Stage::AiReview, Stage::CodeReview, Stage::ManualTest];
    let current = !check.repos.is_empty()
        && check.repos.iter().all(|r| {
            r.merged
                || (!r.pulls_base()
                    && bases.and_then(|b| b.get(&r.key)).is_some_and(|b| b.behind == 0))
        });
    let clean = feature.shipping.resolving.is_none()
        && check.repos.iter().all(|r| {
            !(r.no_merge.as_deref()).is_some_and(|why| why.contains(" conflicts in "))
                && (bases.and_then(|b| b.get(&r.key))).is_none_or(|b| b.conflicts.is_empty())
        });
    let named = [
        (feature.is_done(Stage::Verify) && !stale, "checks passed"),
        (reviews.iter().all(|s| feature.is_done(*s)) && open == 0 && !stale, "review done"),
        (current, "base current"),
        (clean, "no conflicts"),
    ];
    let ok: Vec<&str> = named.iter().filter(|(ok, _)| *ok).map(|(_, w)| *w).collect();
    (!ok.is_empty()).then(|| CheckLine {
        text: format!("\u{2713} {}", ok.join(" \u{b7} ")),
        tone: CheckTone::Ready,
    })
}

/// What the Ship check says while an agent takes on `feature`'s conflicts
/// with its base: Ship waits for it.
fn resolving_line(feature: &Feature) -> Option<CheckLine> {
    let r = feature.shipping.resolving.as_ref()?;
    let doing = match r.planning {
        true => "plans how to resolve",
        false => "resolves",
    };
    Some(CheckLine {
        text: format!(
            "{}: {} {doing} the conflicts with {}: Ship waits for it",
            r.repo, r.agent, r.onto
        ),
        tone: CheckTone::Note,
    })
}

/// What a shipped `feature` waits for while its pull requests are open on
/// GitHub, not merged yet, as `probe` last read them: "pull request open,
/// not merged yet". None once they all merged, or with none.
pub(super) fn pr_wait(feature: &Feature, probe: Option<&Probe>) -> Option<String> {
    let prs = &feature.shipping.prs;
    if prs.is_empty() {
        return None;
    }
    let merged = prs.iter().filter(|pr| pr_merged(probe, &pr.repo)).count();
    match (prs.len(), merged) {
        (n, m) if m == n => None,
        (1, _) => Some("pull request open, not merged yet".to_owned()),
        (n, 0) => Some(format!("{n} pull requests open, not merged yet")),
        (n, m) => Some(format!("{m} of {n} pull requests merged; the rest open")),
    }
}

/// What Ship's mark and the history say of a feature finished unshipped.
pub(super) const FINISHED_UNSHIPPED: &str = "Finished without shipping";

/// What the Ship check says with no step ticked.
pub(super) const NOTHING_TICKED: &str = "No step ticked: Finish ends it without shipping";

/// What it says once a shipped feature's pull requests all merged.
const MERGED_DONE: &str = "Merged on GitHub: shipped";

/// What it says of a shipped feature with no step ticked.
const TICK_MORE: &str = "Shipped: tick a step to ship more";

/// What it says when every step ticked ran already, before Ship is done.
const ALL_RAN: &str = "Every step ticked ran: Ship marks it shipped";

/// And once it is.
const ALL_RAN_DONE: &str = "Shipped: every step ticked ran";

/// What a Ship card says when a step of its can't run: the step says why.
const HELD_UP: &str = "can't run yet";

/// "1 step", "3 steps".
pub(super) fn steps_word(n: usize) -> String {
    match n {
        1 => "1 step".to_owned(),
        n => format!("{n} steps"),
    }
}

/// What it says of a shipped feature with `n` steps ticked left to run.
fn steps_left(n: usize) -> String {
    match n {
        1 => "Shipped: 1 step ticked to run".to_owned(),
        n => format!("Shipped: {n} steps ticked to run"),
    }
}

/// The Ship page's rows the keyboard and pointer reach, in the order they
/// show: none unless its ticks can still change, that is, with Ship the
/// current stage and no squash under way.
pub(super) fn ship_rows(feature: &Feature, probe: Option<&Probe>) -> Vec<ShipRow> {
    let check = probe.and_then(|p| p.ship.as_ref());
    let Some(check) = check.filter(|c| !c.unread && feature.ships() && !feature.squash_going())
    else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    // A merged repository has nothing left to choose: but its base to push.
    // Nor has one shipped through its pull request: it went that way.
    for repo in &check.repos {
        let route = ship::repo_plan(feature, repo).route;
        let went = feature.status == Status::Done
            && feature.ship_done(&repo.key, ShipStep::OpenPr).is_some();
        if !repo.merged && !went {
            rows.push(ShipRow::Route {
                repo: repo.key.clone(),
                route,
            });
        }
        rows.extend(
            (route.steps().iter())
                .filter(|step| ship_step_open(feature, repo, **step))
                .map(|step| ShipRow::Step {
                    repo: repo.key.clone(),
                    step: *step,
                }),
        );
    }
    // Once shipped, what followed it is past.
    if feature.status == Status::Open {
        rows.push(ShipRow::After(ShipAfter::Close));
        rows.push(ShipRow::After(ShipAfter::Remove));
    }
    rows
}

/// Whether `step` of `repo` can still be ticked: it didn't run, and its
/// repository isn't merged, but for pushing its base after its squash.
fn ship_step_open(feature: &Feature, repo: &ship::RepoCheck, step: ShipStep) -> bool {
    if ship::step_done(feature, repo, step).is_some() {
        return false;
    }
    let squashed = feature.ship_done(&repo.key, ShipStep::Merge).is_some();
    !repo.merged || (step == ShipStep::PushBase && squashed)
}

/// What repository `repo`'s card says under its name: "12 commits \u{b7}
/// +420 \u{2212}80 \u{b7} into main".
fn ship_repo_meta(repo: &ship::RepoCheck, stat: Option<feature::page::Stat>) -> String {
    let commits = match repo.commits {
        1 => "1 commit".to_owned(),
        n => format!("{n} commits"),
    };
    let stat = stat.map_or(String::new(), |stat| {
        format!(" \u{b7} +{} \u{2212}{}", stat.added, stat.removed)
    });
    format!("{commits}{stat} \u{b7} into {}", repo.base)
}

/// When a step ran and what it sent, for its row: "2h ago \u{b7} a1b2c3d".
fn ran_when(done: &feature::ShipDone, now: u64) -> String {
    let when = (done.at > 0).then(|| crate::clock::ago(now, done.at));
    let sha = (done.sha.as_deref()).map(|sha| sha.chars().take(7).collect::<String>());
    let url = done.url.as_deref().map(pr_tail);
    let parts: Vec<String> = when.into_iter().chain(sha).chain(url).collect();
    parts.join(" \u{b7} ")
}

/// How repository `repo`'s card stands: its dot and words.
fn ship_card_state(
    feature: &Feature,
    repo: &ship::RepoCheck,
    shipping: bool,
    pr_state: Option<&Result<ship::PrState, String>>,
) -> (Dot, CheckLine) {
    let plan = ship::repo_plan(feature, repo);
    let squashing = feature.squash_going() && feature.shipping.squash_repos.contains(&repo.key);
    if repo.merged {
        let base = &repo.base;
        let left = ship_step_open(feature, repo, ShipStep::PushBase);
        return match left && plan.ticked(ShipStep::PushBase) {
            true => (
                Dot::Idle,
                line(format!("merged into {base}: push it"), CheckTone::Note),
            ),
            false => (
                Dot::Done,
                line(format!("merged into {base}"), CheckTone::Ready),
            ),
        };
    }
    if squashing {
        let text = "squash under way in a tab: done once merged";
        return (Dot::Working, line(text, CheckTone::Note));
    }
    let open: Vec<ShipStep> = (plan.steps().into_iter())
        .filter(|step| ship_step_open(feature, repo, *step))
        .collect();
    if shipping && !open.is_empty() {
        return (Dot::Working, line("shipping\u{2026}", CheckTone::Note));
    }
    // Its step says why.
    if open.iter().any(|step| repo.why_not(*step).is_some()) {
        return (Dot::Blocked, line(HELD_UP, CheckTone::Blocker));
    }
    let pr = feature.ship_done(&repo.key, ShipStep::OpenPr).is_some();
    let pushed = feature.ship_done(&repo.key, ShipStep::Push).is_some();
    // Its pull request as GitHub has it, once read.
    if let (0, Some(Ok(state))) = (open.len(), pr_state) {
        let text = format!("pull request {}", state.words());
        return match state.state.as_str() {
            "MERGED" => (Dot::Done, line(text, CheckTone::Ready)),
            "CLOSED" => (Dot::Unknown, line(text, CheckTone::Note)),
            _ if state.failed > 0 || state.mergeable == "CONFLICTING" => {
                (Dot::Failed, line(text, CheckTone::Failed))
            }
            _ if state.running > 0 => (Dot::Working, line(text, CheckTone::Note)),
            _ if state.review == "CHANGES_REQUESTED" => {
                (Dot::Blocked, line(text, CheckTone::Blocker))
            }
            _ => (Dot::Done, line(text, CheckTone::Ready)),
        };
    }
    if let (0, Some(Err(why))) = (open.len(), pr_state) {
        let text = format!("pull request open; couldn't read it on GitHub: {why}");
        return (Dot::Unknown, line(text, CheckTone::Note));
    }
    match (open.len(), plan.route) {
        (0, _) if pr => (Dot::Done, line("pull request open", CheckTone::Ready)),
        (0, _) if pushed => (Dot::Done, line("pushed", CheckTone::Ready)),
        (0, Route::Nothing) => (Dot::Idle, line("stays as it is", CheckTone::Note)),
        (0, _) => (Dot::Idle, line("no step ticked", CheckTone::Note)),
        (1, _) => (Dot::Idle, line("1 step to run", CheckTone::Note)),
        (n, _) => (
            Dot::Idle,
            line(format!("{n} steps to run"), CheckTone::Note),
        ),
    }
}

/// The row of `step` of `repo` on the Ship page.
fn ship_step_row(
    feature: &Feature,
    repo: &ship::RepoCheck,
    step: ShipStep,
    row: usize,
    (actions, now): (&[(PageButton, PanelOutcome)], u64),
) -> chrome::ShipStepRow {
    let plan = ship::repo_plan(feature, repo);
    let base = &repo.base;
    let branch = feature.branch();
    // What ran says so, and when; it is no tick box any more.
    let ran = ship::step_done(feature, repo, step).or_else(|| {
        (step == ShipStep::Merge && repo.merged).then(|| feature::ShipDone {
            at: 0,
            repo: repo.key.clone(),
            step,
            sha: None,
            url: None,
            undone: None,
        })
    });
    let ran = ran.filter(|_| !ship_step_open(feature, repo, step));
    // Undoing it, while it can be: also once it is to run again, as a
    // branch that moved since its push still has that push on origin.
    let undo = (actions.iter()).position(|(_, o)| {
        matches!(o, PanelOutcome::UndoShip(r, s) if *r == repo.key && feature::same_step(*s, step))
    });
    if let Some(done) = ran {
        let label = match step {
            ShipStep::Push | ShipStep::PushBranch => format!("Pushed {branch} to origin"),
            ShipStep::OpenPr => format!("Opened a pull request into {base}"),
            ShipStep::Merge => format!("Merged into {base}"),
            ShipStep::PushBase => format!("Pushed {base} to origin"),
        };
        let when = ran_when(&done, now);
        // What can't be undone says so.
        let when = match (undo, step) {
            (None, ShipStep::PushBase) if !when.is_empty() => {
                format!("{when} \u{b7} on origin: no undo")
            }
            _ => when,
        };
        return chrome::ShipStepRow {
            row,
            label,
            detail: (!when.is_empty()).then(|| line(when, CheckTone::Note)),
            ticked: true,
            off: false,
            done: true,
            button: undo,
        };
    }
    // Merged some other way: what else it would do is past.
    if repo.merged {
        let label = match step {
            ShipStep::PushBase => format!("Push {base} to origin"),
            _ => String::new(),
        };
        return chrome::ShipStepRow {
            row,
            label,
            detail: None,
            ticked: plan.ticked(step),
            off: false,
            done: false,
            button: None,
        };
    }
    let pushed = feature.ship_done(&repo.key, ShipStep::Push).is_some();
    let label = match step {
        ShipStep::Push | ShipStep::PushBranch if pushed => {
            format!("Push {branch} to origin again: it moved since")
        }
        ShipStep::Push | ShipStep::PushBranch => format!("Push {} to origin", feature.branch()),
        ShipStep::OpenPr => match feature.shipping.prs.iter().find(|p| p.repo == repo.key) {
            Some(pr) => format!("Keep its pull request open: {}", pr_tail(&pr.url)),
            None => format!("Open a pull request into {base}"),
        },
        ShipStep::Merge => format!("Merge into {base} in {}", short_folder(&repo.tree)),
        ShipStep::PushBase => format!("Push {base} to origin"),
    };
    let why = repo.why_not(step);
    let detail = match (why, step) {
        // The Ship check says why in full: once is enough.
        (Some(_), ShipStep::Merge) if repo.pulls_base() => Some(CheckLine {
            text: format!("Pull {base} first"),
            tone: CheckTone::Blocker,
        }),
        (Some(why), _) => Some(CheckLine {
            text: why.to_owned(),
            tone: CheckTone::Blocker,
        }),
        (None, ShipStep::Merge) => Some(CheckLine {
            text: "Squash; you edit the message in a tab".to_owned(),
            tone: CheckTone::Note,
        }),
        _ => None,
    };
    chrome::ShipStepRow {
        row,
        label,
        detail,
        ticked: plan.ticked(step),
        off: why.is_some(),
        done: false,
        button: undo,
    }
}

/// The Ship page's choices and tick boxes, as `ship_rows` numbers them.
pub(super) fn ship_sections(
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Vec<PageSection> {
    let probe = state.probe;
    let rows = ship_rows(feature, probe);
    let going = feature.squash_going();
    let Some(check) = probe
        .and_then(|p| p.ship.as_ref())
        .filter(|c| !c.unread && (!rows.is_empty() || going))
    else {
        return Vec::new();
    };
    let at = |wanted: &ShipRow| rows.iter().position(|r| r == wanted);
    let mut out = Vec::new();
    for repo in &check.repos {
        let plan = ship::repo_plan(feature, repo);
        let stat = probe.and_then(|p| p.stats.get(&repo.key)).copied();
        // Every step of its way: those that ran, then those to tick. A
        // merged repository shows only what ran, and its base to push.
        let steps = (plan.route.steps().iter())
            .filter_map(|step| {
                let wanted = ShipRow::Step {
                    repo: repo.key.clone(),
                    step: *step,
                };
                let row = at(&wanted).unwrap_or(0);
                let row = ship_step_row(feature, repo, *step, row, (actions, now));
                let shown = row.done || at(&wanted).is_some();
                shown.then_some(row)
            })
            .collect();
        let pr = feature.ship_done(&repo.key, ShipStep::OpenPr);
        let note = match (plan.route, pr.is_some(), repo.merged) {
            (_, _, true) => None,
            (Route::Nothing, false, _) => Some("Nothing gets pushed or merged"),
            (Route::Nothing, true, _) => {
                Some("Nothing gets pushed or merged; its pull request stays open on GitHub")
            }
            (Route::Merge, true, _) => Some("Its pull request stays open on GitHub"),
            _ => None,
        };
        let pr_state = (probe.and_then(|p| p.prs.as_ref())).and_then(|prs| prs.get(&repo.key));
        let (dot, state_line) = ship_card_state(feature, repo, state.shipping, pr_state);
        let open = (pr.and_then(|pr| pr.url)).and_then(|url| {
            (actions.iter()).position(|(_, o)| *o == PanelOutcome::OpenPr(url.clone()))
        });
        // Taking its base in, then its pull request.
        let buttons = (actions.iter().enumerate())
            .filter(|(_, (_, o))| match o {
                PanelOutcome::UpdateBase(r, _)
                | PanelOutcome::ResolveConflicts(r, _)
                | PanelOutcome::PullBase(r) => *r == repo.key,
                PanelOutcome::ReviewConflictPlan | PanelOutcome::StopResolving => {
                    (feature.shipping.resolving.as_ref()).is_some_and(|r| r.repo == repo.key)
                }
                _ => false,
            })
            .map(|(i, _)| i)
            .chain(open)
            .collect();
        let route_row = at(&ShipRow::Route {
            repo: repo.key.clone(),
            route: plan.route,
        });
        out.push(PageSection::ShipGroup(chrome::ShipGroup {
            head: repo.key.clone(),
            caption: false,
            card: Some(chrome::ShipCard {
                dot,
                state: state_line,
                meta: ship_repo_meta(repo, stat),
                // Its squash waits for the base to be pulled: its step says
                // so, and "up to date" would read as the opposite.
                base: (!repo.merged && !repo.pulls_base())
                    .then(|| ship_base_line(feature, probe, &repo.key))
                    .flatten(),
                buttons,
            }),
            route: route_row.map(|row| chrome::ShipRouteRow {
                row,
                on: plan.route.index(),
            }),
            note: note.map(str::to_owned),
            steps,
            selected: None,
        }));
    }
    // Once shipped, what followed is past: the cards are all there is.
    if feature.status != Status::Open || rows.is_empty() {
        return out;
    }
    let at = |wanted: &ShipRow| at(wanted).unwrap_or(0);
    let removable = ship::removable(feature, check);
    let remove = removable && feature.ship_plan.remove_worktrees;
    let after = |which, label: &str, ticked, why: Option<&str>| chrome::ShipStepRow {
        row: at(&ShipRow::After(which)),
        label: label.to_owned(),
        detail: why.map(|why| CheckLine {
            text: why.to_owned(),
            tone: CheckTone::Note,
        }),
        ticked,
        off: why.is_some(),
        done: false,
        button: None,
    };
    let remove_label = match feature.touched().count() > 1 {
        true => "Remove the worktrees and branches",
        false => "Remove the worktree and branch",
    };
    out.push(PageSection::ShipGroup(chrome::ShipGroup {
        head: "Afterwards".to_owned(),
        caption: true,
        card: None,
        route: None,
        note: None,
        steps: vec![
            after(
                ShipAfter::Close,
                "Close the workspace",
                remove || !feature.ship_plan.keep_workspace,
                remove.then_some(REMOVING_CLOSES),
            ),
            after(
                ShipAfter::Remove,
                remove_label,
                remove,
                (!removable).then_some(ONLY_ONCE_MERGING),
            ),
        ],
        selected: None,
    }));
    out
}

/// Whether shipped `feature`'s Afterwards tick to close its workspace never
/// ran, as something ran in it when it shipped: its Ship page offers it then.
/// What shows the page has its workspace open.
pub(super) fn close_left(feature: &Feature) -> bool {
    feature.status == Status::Done
        && feature.writer.is_none()
        && feature.dir.exists()
        && !feature.ship_plan.keep_workspace
        && feature.shipping.after.is_empty()
}

/// The page holding what follows `feature`'s end (Close, Undo): Ship, or
/// the stage it was at when it ended there, as a review can.
pub(super) fn after_stage(feature: &Feature) -> Stage {
    (feature.current())
        .filter(|_| feature.status != Status::Open)
        .unwrap_or(Stage::Ship)
}

/// What followed shipping `feature`, once it is done: when its workspace
/// closed and its worktrees went, and when that was undone; each with its
/// Undo while it can be.
pub(super) fn shipped_after_group(
    feature: &Feature,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> Option<PageSection> {
    use feature::AfterStep;
    if feature.status == Status::Open {
        return None;
    }
    // Ticked, it didn't close for what ran in it: it still can.
    let left = close_left(feature).then(|| chrome::ShipStepRow {
        row: 0,
        label: "Close the workspace".to_owned(),
        detail: Some(line("Shipping left it open".to_owned(), CheckTone::Note)),
        ticked: true,
        off: false,
        done: false,
        button: (actions.iter()).position(|(_, o)| *o == PanelOutcome::CloseShipped),
    });
    let steps: Vec<chrome::ShipStepRow> = [AfterStep::Close, AfterStep::Remove]
        .into_iter()
        .filter_map(|step| {
            let done = (feature.shipping.after.iter().rev()).find(|d| d.step == step)?;
            let label = match step {
                AfterStep::Close => "Closed the workspace",
                AfterStep::Remove if feature.touched().count() > 1 => {
                    "Removed the worktrees and branches"
                }
                AfterStep::Remove => "Removed the worktree and branch",
            };
            let mut when = crate::clock::ago(now, done.at);
            if let Some(at) = done.undone {
                let word = match step {
                    AfterStep::Close => "reopened",
                    AfterStep::Remove => "restored",
                };
                when = format!("{when} \u{b7} {word} {}", crate::clock::ago(now, at));
            }
            let button = (actions.iter()).position(|(_, o)| *o == PanelOutcome::UndoAfter(step));
            Some(chrome::ShipStepRow {
                row: 0,
                label: label.to_owned(),
                detail: Some(line(when, CheckTone::Note)),
                ticked: true,
                off: false,
                done: true,
                button,
            })
        })
        .chain(left)
        .collect();
    (!steps.is_empty()).then(|| {
        PageSection::ShipGroup(chrome::ShipGroup {
            head: "Afterwards".to_owned(),
            caption: true,
            card: None,
            route: None,
            note: None,
            steps,
            selected: None,
        })
    })
}

/// What undoing `step` does, for the footer and the button's tooltip.
pub(super) fn after_undo_words(step: feature::AfterStep) -> &'static str {
    match step {
        feature::AfterStep::Close => "bring back its tabs",
        feature::AfterStep::Remove => "restore the worktrees",
    }
}

/// What the Ship confirm says follows shipping `feature`, as its Afterwards
/// ticks have it (`removable`: what is ticked merges everything); none when
/// it is shipped already and ships more.
pub(super) fn ship_after_line(feature: &Feature, removable: bool) -> Option<String> {
    if feature.status != Status::Open {
        return None;
    }
    let plan = &feature.ship_plan;
    if plan.remove_worktrees && removable {
        return Some(match feature.touched().count() > 1 {
            true => "Then its worktrees and branches go".to_owned(),
            false => "Then its worktree and branch go".to_owned(),
        });
    }
    Some(match plan.keep_workspace {
        true => "Then its agents stop and its other tabs close".to_owned(),
        false => "Then its workspace closes".to_owned(),
    })
}

/// The one repository whose base is to be pulled when that alone stops
/// shipping `feature`, as `probe` read it: its key, its base, and what stops
/// it in words.
pub(super) fn pull_first_of(
    feature: &Feature,
    probe: Option<&Probe>,
) -> Option<(String, String, String)> {
    let check = probe?.ship.as_ref()?;
    let planned = ship::plan_actions(feature, check);
    let [said] = planned.blockers.as_slice() else {
        return None;
    };
    let repo =
        (check.repos.iter()).find(|r| r.pulls_base() && r.no_merge.as_ref() == Some(said))?;
    Some((repo.key.clone(), repo.base.clone(), said.clone()))
}

/// Why the workspace's tick can't go with the worktrees' on.
pub(super) const REMOVING_CLOSES: &str = "Removing the worktrees closes it";

/// Why the worktrees can't go with what is ticked.
pub(super) const ONLY_ONCE_MERGING: &str = "Only when every repository merges into its base";

/// The steps `feature`'s shipping ran, newest first: "2h ago \u{b7} api:
/// pushed feature/x \u{b7} a1b2c3d".
pub(super) fn ship_log_lines(feature: &Feature, now: u64) -> Vec<CheckLine> {
    let branch = feature.branch();
    let short = |sha: &str| sha.chars().take(7).collect::<String>();
    let steps = (feature.ship_log().into_iter().rev()).map(|done| {
        let base = feature
            .repo(&done.repo)
            .map_or("its base", |r| r.base.as_str());
        let said = format!("{}: {}", done.repo, done.said(&branch, base));
        let when = (done.at > 0).then(|| crate::clock::ago(now, done.at));
        let sha = done.sha.as_deref().map(short);
        let url = done.url.as_deref().map(pr_tail);
        let undone = (done.undone).map(|at| format!("undone {}", crate::clock::ago(now, at)));
        let parts: Vec<String> = (when.into_iter())
            .chain(std::iter::once(said))
            .chain(sha)
            .chain(url)
            .chain(undone)
            .collect();
        (done.at, parts.join(" \u{b7} "))
    });
    // Its updates from the base go in among them, by when.
    let updates = feature.shipping.updates.iter().map(|update| {
        let text = format!(
            "{} \u{b7} {}: {} {} \u{b7} {}",
            crate::clock::ago(now, update.at),
            update.repo,
            update.way.done_word(),
            update.onto,
            short(&update.onto_sha)
        );
        (update.at, text)
    });
    // And what followed it.
    let after = feature.shipping.after.iter().map(|done| {
        let said = match done.step {
            feature::AfterStep::Close => "closed the workspace",
            feature::AfterStep::Remove => "removed the worktrees",
        };
        let undone = (done.undone).map(|at| format!(" \u{b7} undone {}", crate::clock::ago(now, at)));
        let text = format!(
            "{} \u{b7} {said}{}",
            crate::clock::ago(now, done.at),
            undone.unwrap_or_default()
        );
        (done.at, text)
    });
    // What followed shipping comes after what it followed, in the same
    // second too.
    let mut all: Vec<(u64, String)> = after.chain(steps).chain(updates).collect();
    all.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    (all.into_iter())
        .map(|(_, text)| line(text, CheckTone::Note))
        .collect()
}

/// A time span in words, its two largest units: "45s", "12m", "3h 10m",
/// "2d 4h".
pub(super) fn span_words(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// "1 commit", "3 commits": `n` of `one`, `many` when not one.
pub(super) fn count_of(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}
