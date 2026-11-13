//! The feature panel's rows and stage pages' actions: what each row and button does,
//! and the recap of a shipped feature.

use super::*;

/// The repositories the feature tab lists, in order: those on the feature's
/// branch, then those agents only read. Expanded, those it could take
/// follow, so what collapsed shows is the start of what expanded shows and
/// the selected row keeps its meaning across the two.
pub fn repo_entries(feature: &Feature, sources: &[RepoSource], expanded: bool) -> Vec<RepoEntry> {
    let held = |touched: bool| {
        (feature.repos.iter().enumerate())
            .filter(move |(_, repo)| repo.touched == touched)
            .map(|(i, _)| RepoEntry::Held(i))
    };
    let mut entries: Vec<RepoEntry> = (held(true).chain(held(false)))
        .chain((0..feature.context.len()).map(RepoEntry::Context))
        .collect();
    if expanded {
        entries.extend(repos_offered(feature, sources));
    }
    entries
}

/// The rows only the expanded repositories show: those taken out, those
/// of its project not added. Other projects stay out, unless the feature
/// still links the one a repository was taken out of. A shipped feature's
/// repositories can't change: none.
pub fn repos_offered(feature: &Feature, sources: &[RepoSource]) -> Vec<RepoEntry> {
    if !linkable(feature) {
        return Vec::new();
    }
    let mut entries: Vec<RepoEntry> = (feature.removed.iter().enumerate())
        .filter(|(_, gone)| (gone.project.as_deref()).is_none_or(|id| feature.link(id).is_some()))
        .map(|(i, _)| RepoEntry::Removed(i))
        .collect();
    let home = (sources.iter().enumerate()).filter(|(_, source)| source.project.is_none());
    for (at, source) in home {
        let Finding::Found(root) = &source.finding else {
            continue;
        };
        for (repo, found) in root.repos.iter().enumerate() {
            let key = feature::repo_key(source.project.as_deref(), &found.name);
            let known = feature.repo(&key).is_some()
                || feature.context.iter().any(|c| c.key() == key)
                || feature.removed.iter().any(|r| r.key() == key);
            if !known {
                entries.push(RepoEntry::Offered { source: at, repo });
            }
        }
    }
    entries
}

/// The `Repo::key` of the repository in `entry`.
pub fn entry_key(feature: &Feature, sources: &[RepoSource], entry: RepoEntry) -> Option<String> {
    match entry {
        RepoEntry::Held(i) => feature.repos.get(i).map(Repo::key),
        RepoEntry::Context(i) => feature.context.get(i).map(|c| c.key()),
        RepoEntry::Removed(i) => feature.removed.get(i).map(|r| r.key()),
        RepoEntry::Offered { source, repo } => {
            let source = sources.get(source)?;
            let Finding::Found(root) = &source.finding else {
                return None;
            };
            let found = root.repos.get(repo)?;
            Some(feature::repo_key(source.project.as_deref(), &found.name))
        }
    }
}

/// The buttons on the row of `entry`, with what they do. A repository can
/// only join what a source still finds it in: a removed one whose folder
/// went has none.
pub fn entry_buttons(
    feature: &Feature,
    sources: &[RepoSource],
    entry: RepoEntry,
) -> Vec<(&'static str, PanelOutcome)> {
    let Some(key) = entry_key(feature, sources, entry) else {
        return Vec::new();
    };
    let found = || {
        linkable(feature)
            && sources.iter().any(|source| {
                matches!(&source.finding, Finding::Found(root)
                if root.repos.iter().any(|r| {
                    feature::repo_key(source.project.as_deref(), &r.name) == key
                }))
            })
    };
    match entry {
        RepoEntry::Held(i) => {
            let Some(repo) = feature.repos.get(i) else {
                return Vec::new();
            };
            if !repo.touched {
                return vec![(FEATURE_BUTTON, PanelOutcome::Touch(key))];
            }
            let mut buttons = Vec::new();
            if let Some(pr) = pr_of(feature, &key) {
                buttons.push(("Open PR", PanelOutcome::OpenPr(pr.url.clone())));
            }
            if repo_removable(feature, i) {
                buttons.push((CONTEXT_BUTTON, PanelOutcome::MakeContext(key)));
            }
            buttons
        }
        RepoEntry::Context(_) if found() => {
            vec![(FEATURE_BUTTON, PanelOutcome::AddToFeature(key))]
        }
        RepoEntry::Removed(_) | RepoEntry::Offered { .. } if found() => vec![
            (FEATURE_BUTTON, PanelOutcome::AddToFeature(key.clone())),
            (CONTEXT_BUTTON, PanelOutcome::AddContext(key)),
        ],
        _ => Vec::new(),
    }
}

/// The button adding a repository to the feature, on its branch.
pub(super) const FEATURE_BUTTON: &str = "Feature";
/// The button keeping a repository for agents to read.
const CONTEXT_BUTTON: &str = "Context";

/// Where `stage` stands in `feature`.
pub fn stage_mark(feature: &Feature, stage: Stage) -> StageMark {
    if feature.is_done(stage) {
        StageMark::Done
    } else if feature.current() == Some(stage) {
        StageMark::Current
    } else {
        StageMark::Later
    }
}

/// What the panel's rows need besides the feature: whether its project
/// gives Verify checks to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowCtx {
    /// Verify has checks to run (see `feature::verify::checks`); without,
    /// Verify stays a mark set by hand.
    pub checks: bool,
    /// How many acceptance criteria the brief has: with some, Manual test
    /// is marked from its page, by them.
    pub criteria: usize,
    /// How far they went, on the code as last read.
    pub manual: feature::manual::Counts,
    /// Which stages' files are there, by `stage_index`, as the latest
    /// probe found them (see `page::Docs`): an earlier stage locks by
    /// them, and the brief's and the plan's pages offer Review.
    pub written: [bool; Stage::ALL.len()],
    /// The Build agent's dot while it runs again with Build done (see
    /// `Gerdr::build_agent_fixing`).
    pub fixing: Option<Dot>,
    /// Its sandbox is being started or stopped.
    pub sandbox_busy: bool,
    /// Verify has no checks to run because the feature skips them all.
    pub all_skipped: bool,
    /// At Ship, as last read, no step is ticked: it can only finish.
    pub ship_nothing: bool,
    /// On Ship's page, as last read, how many steps are left to run.
    pub ship_left: usize,
    /// Ship's check is read: its repositories' cards show, with the
    /// buttons that go on them.
    pub ship_read: bool,
    /// At Ship, as last read, something stops shipping: Ship… is no
    /// default then.
    pub ship_blocked: bool,
    /// Its checks are being started, or wait for a setup to end. Once their
    /// terminals are open `checks_running` says so too.
    pub checks_busy: bool,
    /// Its worktrees are being set up.
    pub setting_up: bool,
    /// Work of its runs off the thread (see `Gerdr::feature_busy`).
    pub feature_busy: bool,
    /// The comments the user left in its diff, as last read, or as its open
    /// diff has them: with any, AI review's fixes need a plan.
    pub comments: usize,
    /// Its sandbox is open to the network (see `sandbox_lan`).
    pub lan_open: bool,
    /// A pull request's review whose worktree is at the head reviewed,
    /// nothing changed, as its code was last read: Verify goes by GitHub's
    /// checks (see `pr_watch`).
    pub on_head: bool,
}

impl RowCtx {
    /// Whether `stage` has a file of its own, as the latest probe found.
    pub(crate) fn has_file(self, stage: Stage) -> bool {
        self.written[stage_index(stage)]
    }

    /// Whether `stage`'s file is there to read on its page: Discover's
    /// and Plan's only.
    pub(super) fn wrote(self, stage: Stage) -> bool {
        matches!(stage, Stage::Discover | Stage::Plan | Stage::PlanReview) && self.has_file(stage)
    }

    /// The stage that locks done `stage` of `feature`: see
    /// `Feature::locked_by`.
    pub(super) fn locked_by(self, feature: &Feature, stage: Stage) -> Option<Stage> {
        feature.locked_by(stage, |s| self.has_file(s))
    }
}

/// Why `by` locks the stage before it in `feature`, as a page or a tab
/// says it: "Plan has plan.md".
pub(crate) fn lock_why(feature: &Feature, by: Stage) -> String {
    let label = by.label();
    if feature.is_done(by) {
        return format!("{label} is done");
    }
    match by {
        Stage::Verify => format!("{label} has a run"),
        Stage::AiReview => format!("{label} has its review"),
        Stage::CodeReview => format!("{label} has viewed files or comments"),
        Stage::ManualTest => format!("{label} has results"),
        Stage::Ship => format!("{label} has pull requests"),
        _ => {
            let name = crate::feature::artifacts::name(by).unwrap_or("its file");
            format!("{label} has {name}")
        }
    }
}

/// What Enter (or the row's button) does on `row` of `feature`.
pub fn row_action(feature: &Feature, row: PanelRow, ctx: RowCtx) -> Option<PanelOutcome> {
    match row {
        PanelRow::Repo(i) => {
            let repo = feature.repos.get(i)?;
            Some(if repo.touched {
                PanelOutcome::Open(repo.worktree.clone())
            } else {
                PanelOutcome::Touch(repo.key())
            })
        }
        PanelRow::Other => None,
        PanelRow::Stage(stage) => Some(match stage_mark(feature, stage) {
            StageMark::Done if ctx.locked_by(feature, stage).is_some() => {
                feature.current()?;
                PanelOutcome::GoToCurrent
            }
            StageMark::Done => PanelOutcome::Untick(stage),
            // Finished at its current stage, as a pull request's review is:
            // nothing is left to do there.
            StageMark::Current if feature.status != Status::Open => return None,
            // A pull request's review goes by its pull request first.
            StageMark::Current if feature.pr_ended().is_some() => PanelOutcome::FinishReview,
            StageMark::Current if feature.pr_moved().is_some() => PanelOutcome::UpdatePrHead,
            // A review's Ship submits it to GitHub.
            StageMark::Current if stage == Stage::Ship && feature.pull().is_some() => {
                submit_action(feature, ctx)
            }
            StageMark::Current if stage == Stage::Ship && ctx.ship_nothing => PanelOutcome::Finish,
            StageMark::Current if stage == Stage::Ship => PanelOutcome::Ship,
            // At the head reviewed, GitHub's checks are its Verify.
            StageMark::Current if stage == Stage::Verify && ctx.on_head => ci_action(feature),
            StageMark::Current if stage == Stage::Verify && ctx.checks => PanelOutcome::RunChecks,
            StageMark::Current if stage == Stage::PlanReview => plan_review_action(feature),
            StageMark::Current if stage == Stage::AiReview => review_action(feature, ctx),
            StageMark::Current if stage == Stage::CodeReview => PanelOutcome::CodeReview,
            StageMark::Current if stage == Stage::ManualTest && ctx.criteria > 0 => {
                PanelOutcome::ManualTest
            }
            StageMark::Current if needs(stage).is_some() => agent_action(feature, stage),
            StageMark::Current => PanelOutcome::Tick,
            StageMark::Later => PanelOutcome::NotYet(feature.current()?),
        }),
    }
}

/// The button to `stage`'s last agent, done stage or not: its tab, or its
/// chat once its tab is closed. A chat of a feature no longer open has no
/// folder to go back to, or none to work in; that of a locked stage would
/// change what the next stage is built on.
pub(super) fn agent_button(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Option<(&'static str, PanelOutcome)> {
    if needs(stage).is_none() || stage_mark(feature, stage) == StageMark::Later {
        return None;
    }
    let run = feature.runs.get(&stage)?;
    if run.live() {
        return Some(("Show agent", PanelOutcome::ShowAgent(stage)));
    }
    // Its prompt is still to send, also from a done Build: fixes wait so.
    if run.held {
        return (feature.status == Status::Open)
            .then_some(("Start agent", PanelOutcome::StartAgent(stage, false)));
    }
    (feature.status == Status::Open && run.resumes() && ctx.locked_by(feature, stage).is_none())
        .then_some(("Resume agent", PanelOutcome::ResumeAgent(stage)))
}

/// Where a done Build's agent stands with work sent back to it: fixes,
/// review comments, failed tests. The stage the feature is at is a later
/// one, so the rail and that stage's page say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rework {
    /// In its tab, waiting for the user to send its prompt.
    Waits,
    /// Its prompt still to send, and its tab closed.
    Closed,
    /// Waiting in T3 Code as a draft.
    Draft,
    /// Running, with this dot.
    Runs(Dot),
}

impl Rework {
    /// Its mark and short line on the rail.
    pub(super) fn node(self) -> (NodeMark, String) {
        let (mark, line) = match self {
            Rework::Waits => (NodeMark::Waiting, "waiting to send"),
            Rework::Closed => (NodeMark::Waiting, "agent closed"),
            Rework::Draft => (NodeMark::Waiting, "draft in T3 Code"),
            Rework::Runs(_) => (NodeMark::Fixing, "working again"),
        };
        (mark, line.to_owned())
    }

    /// How it colours the feature's sidebar card.
    pub(super) fn tone(self) -> chrome::CardTone {
        match self {
            Rework::Runs(_) => chrome::CardTone::Working,
            Rework::Waits | Rework::Closed | Rework::Draft => chrome::CardTone::Waits,
        }
    }

    /// What its agent's card says.
    pub(super) fn status(self) -> &'static str {
        match self {
            Rework::Waits => "your turn \u{b7} send the prompt",
            Rework::Closed => "its tab closed \u{b7} Start Build agent opens it again",
            Rework::Draft => "draft in T3 Code \u{b7} pick a model and send it",
            Rework::Runs(Dot::Blocked) => "your turn \u{b7} it asks you something",
            Rework::Runs(_) => "working again",
        }
    }

    /// Whether its agent waits on the user: its card stands out for it.
    /// A closed tab waits on nobody's answer.
    pub(super) fn waits_on_you(self) -> bool {
        matches!(
            self,
            Rework::Waits | Rework::Draft | Rework::Runs(Dot::Blocked)
        )
    }
}

/// How `feature`'s Build agent stands with work sent back to it, if any.
pub(super) fn rework(feature: &Feature, ctx: RowCtx) -> Option<Rework> {
    if feature.status != Status::Open || !feature.is_done(Stage::Build) {
        return None;
    }
    let run = feature.runs.get(&Stage::Build)?;
    if run.t3 == Some(feature::agents::T3Run::Draft) {
        return Some(Rework::Draft);
    }
    if run.held {
        return Some(if run.live() {
            Rework::Waits
        } else {
            Rework::Closed
        });
    }
    ctx.fixing.map(Rework::Runs)
}

/// The stage that sent work back to `feature`'s Build agent, while the flow
/// is back there: from a fix round's start to its end, at rest as it waits
/// for the user too, then for as long as the agent is seen at it or the fixes
/// have Build to do again. The rail reads as turned back then.
pub(super) fn turned_back(feature: &Feature, ctx: RowCtx) -> Option<Stage> {
    if feature.status != Status::Open {
        return None;
    }
    let last = feature.last_fix_round.as_ref().map(|r| r.stage);
    let current = feature.current();
    let from = if let Some(round) = &feature.fix_round {
        Some(round.stage)
    } else if current == Some(Stage::Build) {
        last
    } else if rework(feature, ctx).is_some() {
        // The feature may have come back to before the stage that asked.
        match (last, current) {
            (Some(last), Some(current)) => Some(last.min(current)),
            (last, current) => last.or(current),
        }
    } else {
        None
    };
    from.filter(|stage| *stage > Stage::Build)
}

/// The line back on `feature`'s rail: into Build while the flow is turned
/// back there (`turned_back`), else into the stage the flow is at on its way
/// back from Verify, for fixes Manual test asked for or a base update
/// (`Feature::back_from`).
pub(super) fn rail_back(feature: &Feature, ctx: RowCtx) -> Option<RailBack> {
    let line = |from: Stage, to: Stage| RailBack {
        from: stage_index(from),
        to: stage_index(to),
    };
    if let Some(from) = turned_back(feature, ctx) {
        return Some(line(from, Stage::Build));
    }
    // From the stage that turned it to where the flow is on its way back.
    let current = feature.current()?;
    let from = feature.back_from.filter(|from| {
        feature.status == Status::Open && current >= Stage::Verify && *from > current
    })?;
    Some(line(from, current))
}

/// What took `feature` round again from Verify, in a few words: "fix round
/// 4" for the fixes Manual test asked for, or its base update.
pub(super) fn loop_cause(feature: &Feature) -> Option<String> {
    match feature.back_from? {
        Stage::Ship => Some("the base update".to_owned()),
        from => {
            let n = (feature.fix_asked.iter().rev())
                .find(|(_, s)| **s == from)
                .map(|(n, _)| *n);
            Some(
                n.map_or_else(|| "its fixes".to_owned(), feature::fix_round::label)
                    .to_lowercase(),
            )
        }
    }
}

/// While `feature` goes round again from Verify (`Feature::back_from`), what
/// `stage`'s page says of it, the same line on each stage of the loop: why,
/// and where it ends. Back where it started, that the marks there are kept
/// and the stage is to be marked again.
pub(super) fn loop_line(feature: &Feature, stage: Stage) -> Option<CheckLine> {
    let from = feature.back_from?;
    let current = feature.current()?;
    if feature.status != Status::Open || stage != current || current > from {
        return None;
    }
    let cause = loop_cause(feature)?;
    let between: Vec<&str> = (Stage::ALL.iter())
        .filter(|s| **s >= Stage::Verify && **s < from)
        .map(|s| s.label())
        .collect();
    let between = match between.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => (*last).to_owned(),
        None => String::new(),
    };
    let text = match current == from {
        true => format!(
            "Back after {cause}: {between} passed on the update. Your marks are kept: {} again to go on",
            match from {
                Stage::ManualTest => "mark it passed",
                _ => "mark it done",
            }
        ),
        false => format!(
            "{} changed the code: {between} run again on the update, then {}",
            capitalized(&cause),
            from.label()
        ),
    };
    Some(line(text, CheckTone::Ready))
}

/// `text` with its first letter capital.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// `rework`, on the page of `stage` when `stage` is where the feature is at.
pub(super) fn rework_on(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<Rework> {
    (stage > Stage::Build && stage_mark(feature, stage) == StageMark::Current)
        .then(|| rework(feature, ctx))
        .flatten()
}

/// What the key for an agent's tab does on `stage`'s page: the Build agent's
/// while work sent back to it is under way, else what `agent_button` says.
pub(super) fn agent_key(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
) -> Option<(&'static str, PanelOutcome)> {
    rework_on(feature, stage, ctx)
        .and_then(|_| agent_button(feature, Stage::Build, ctx))
        .or_else(|| agent_button(feature, stage, ctx))
        .filter(|(_, o)| !verify_going(feature, stage, ctx) || works_during_checks(o))
}

/// What an agent stage's row does: start its agent, show it while its
/// tab is there, review its file once ready. Build has no file to review:
/// stopped after a milestone, it is let go on.
fn agent_action(feature: &Feature, stage: Stage) -> PanelOutcome {
    match feature.runs.get(&stage) {
        Some(run) if run.waits_to_send() => PanelOutcome::SendPrompt(stage),
        Some(run) if run.stop.is_some() => PanelOutcome::Continue(stage),
        Some(run) if run.ready.is_some() && stage != Stage::Build => PanelOutcome::OpenDoc(stage),
        Some(run) if run.live() => PanelOutcome::ShowAgent(stage),
        Some(run) if run.resumes() => PanelOutcome::ResumeAgent(stage),
        _ => PanelOutcome::StartAgent(stage, false),
    }
}

/// Whether `stage`'s file can be approved from its page: it is there and
/// waits for review, so reading it in its tab is the user's choice.
pub(super) fn approves_doc(feature: &Feature, stage: Stage, ctx: RowCtx) -> bool {
    feature.status == Status::Open
        && ctx.wrote(stage)
        && row_action(feature, PanelRow::Stage(stage), ctx) == Some(PanelOutcome::OpenDoc(stage))
}

/// Whether Discover's or Plan's file, at `stage` of `feature`, is written
/// while its agent still runs on it: not read yet, so not to approve.
pub(super) fn doc_unread(feature: &Feature, stage: Stage, ctx: RowCtx) -> bool {
    feature.status == Status::Open
        && matches!(stage, Stage::Discover | Stage::Plan)
        && feature.current() == Some(stage)
        && ctx.wrote(stage)
        && (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_none() && r.live())
}

/// Why Ctrl+Enter doesn't approve a file `doc_unread` says isn't read yet.
pub(super) const DOC_UNREAD: &str = "Not read yet: the agent may still change it";

/// What Plan review's row does: as any agent stage's while its agent
/// reviews or writes the final plan; in between, the suggestions to decide,
/// then what comes of them.
fn plan_review_action(feature: &Feature) -> PanelOutcome {
    let stage = Stage::PlanReview;
    if feature.runs.get(&stage).is_some_and(|r| r.waits_to_send()) {
        return PanelOutcome::SendPrompt(stage);
    }
    match crate::feature_plan_review::deciding(feature) {
        Some(review) if review.open() > 0 => PanelOutcome::ReviewInPlan,
        Some(_) => PanelOutcome::WriteFinalPlan,
        None => agent_action(feature, stage),
    }
}

/// `notes`, the comments left in the diff, as far as `stage`'s page of
/// `feature` lists them: those going with AI review's fixes. Once it is done
/// they wait for Code review instead.
pub(crate) fn shown_notes<'a>(
    feature: &Feature,
    stage: Stage,
    notes: &'a [Comment],
) -> &'a [Comment] {
    // Those that went to Build come after, and only Code review lists them.
    let own = notes.iter().take_while(|c| c.flag.is_none()).count();
    match stage {
        Stage::AiReview
            if stage_mark(feature, stage) == StageMark::Current && feature.review.is_some() =>
        {
            &notes[..own]
        }
        Stage::CodeReview if lists_comments(feature) => notes,
        _ => &[],
    }
}

/// Whether Code review's page of `feature` lists comments: while it waits for
/// them, and for good once some went to Build.
pub(super) fn lists_comments(feature: &Feature) -> bool {
    feature.current() == Some(Stage::CodeReview) || crate::feature_diff::has_rounds(feature)
}

/// Whether `stage`'s page of `feature` takes a comment of the user's own.
pub(super) fn can_add_comment(feature: &Feature, stage: Stage) -> bool {
    can_decide(feature, stage)
        || marks_criteria(feature, stage)
        || (stage == Stage::CodeReview
            && feature.status == Status::Open
            && feature.current() == Some(stage)
            && feature.writer.is_none())
}

/// Whether comment `c` left in `feature`'s diff can change on `stage`'s page.
pub(crate) fn can_edit_note(feature: &Feature, stage: Stage, c: &Comment) -> bool {
    c.flag.is_none()
        && match stage {
            Stage::CodeReview => feature.status == Status::Open && feature.current() == Some(stage),
            _ => can_decide(feature, stage),
        }
}

/// Why comment `c` left in `feature`'s diff can't change on `stage`'s page,
/// when it can't.
pub(super) fn why_not_edit_note(
    feature: &Feature,
    stage: Stage,
    c: &Comment,
) -> Option<&'static str> {
    if can_edit_note(feature, stage, c) {
        return None;
    }
    if c.flag.is_some() {
        return Some(SENT_NOTE);
    }
    if stage != Stage::CodeReview {
        return why_not_decide(feature, stage);
    }
    Some(match stage_mark(feature, stage) {
        StageMark::Later => "Code review comes later: change it there",
        _ => "Code review is done: reopen it to change its comments",
    })
}

/// The comments the user left in `feature`'s diff, as `probe` last read them.
pub(super) fn notes_of(probe: Option<&Probe>) -> &[Comment] {
    probe.map_or(&[], |p| p.docs.notes.as_slice())
}

/// The reviews before the latest on `stage`'s page of `feature`, the newest
/// first.
pub(super) fn earlier_reviews(
    feature: &Feature,
    stage: Stage,
) -> impl Iterator<Item = &ReviewRound> {
    (feature.review.as_ref())
        .filter(|_| stage == Stage::AiReview)
        .into_iter()
        .flat_map(|review| review.rounds.iter().rev())
}

/// What sending AI review's findings on `stage`'s page is now, if it can
/// be: with a fix plan open, the keys are the plan's.
pub(super) fn sends_findings(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<PanelOutcome> {
    row_action(feature, PanelRow::Stage(stage), ctx)
        .filter(|_| fix_round_on(feature, stage).is_none())
        .filter(|o| matches!(o, PanelOutcome::SendFixes | PanelOutcome::MakeFindingFixes))
}

/// What `stage`'s page of `feature` offers of the reviews it shows: their
/// findings' buttons, and showing each earlier one.
pub(super) fn review_actions(feature: &Feature, stage: Stage) -> Vec<(PageButton, PanelOutcome)> {
    let decides = can_decide(feature, stage);
    let cards = items(feature, stage).into_iter();
    let shows = earlier_reviews(feature, stage).map(|round| {
        let label = "Show".to_owned();
        let style = ButtonStyle::Plain;
        (
            PageButton { label, style },
            PanelOutcome::FoldRound(round.n),
        )
    });
    let posts = feature.posts_review();
    (cards.flat_map(|item| item_buttons(item, CardPage { decides, posts })))
        .chain(shows)
        .collect()
}

/// The card a note is being written on (`FeaturePanel::note`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardNote {
    /// A criterion, as `FeaturePanel::finding` counts them.
    Criterion(usize),
    /// An item of a review stage, by its kind and id: the user's comment
    /// on it, a question's answer; `accept`: accept it once saved.
    Item {
        of: ItemOf,
        id: String,
        accept: bool,
    },
}

impl CardNote {
    /// Whether `other` is a note on the same card, accepting or not.
    pub(super) fn same_card(&self, other: &CardNote) -> bool {
        match (self, other) {
            (Self::Criterion(a), Self::Criterion(b)) => a == b,
            (Self::Item { of, id, .. }, Self::Item { of: o, id: i, .. }) => of == o && id == i,
            _ => false,
        }
    }
}

/// `stage`'s checks in `feature`, as its page lists them: Verify's only.
pub(crate) fn check_rows(feature: &Feature, stage: Stage) -> &[feature::verify::CheckRun] {
    match (stage, feature.verify.as_ref()) {
        (Stage::Verify, Some(run)) => run.checks.as_slice(),
        _ => &[],
    }
}

/// Whether some of `stage`'s checks in `feature` can run again alone: the run
/// ended, and not all failed, or running them all would do.
pub(super) fn has_failed_checks(feature: &Feature, stage: Stage) -> bool {
    let rows = check_rows(feature, stage);
    !checks_running(feature)
        && rows.iter().any(check_passed)
        && rows.iter().any(|row| !check_passed(row))
}

/// One card of Verify's page: a check the last run ran, or one `plan` gives
/// the feature that it didn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CheckEntry {
    repo: String,
    pub(super) command: String,
    /// Its index in the last run's rows.
    pub(super) row: Option<usize>,
    /// The feature skips it now.
    pub(super) skipped: bool,
    /// It comes from a project's checks, so the feature can skip it. One no
    /// longer given isn't: its card stays until the next run.
    pub(super) borrowed: bool,
}

/// `stage`'s checks in `feature` as cards: the last run's first, so a card's
/// index is its row's, then those of `plan` no run has got to.
pub(super) fn check_entries(
    feature: &Feature,
    stage: Stage,
    plan: &[PlannedCheck],
) -> Vec<CheckEntry> {
    if stage != Stage::Verify {
        return Vec::new();
    }
    let rows = check_rows(feature, stage);
    let planned =
        |repo: &str, command: &str| (plan.iter()).find(|p| p.repo == repo && p.command == command);
    let ran = rows.iter().enumerate().map(|(i, row)| {
        let p = planned(&row.repo, &row.command);
        CheckEntry {
            repo: row.repo.clone(),
            command: row.command.clone(),
            row: Some(i),
            skipped: p.is_some_and(|p| p.skipped),
            borrowed: p.is_some_and(|p| !p.own),
        }
    });
    let waits = (plan.iter())
        .filter(|p| !(rows.iter()).any(|r| r.repo == p.repo && r.command == p.command))
        .map(|p| CheckEntry {
            repo: p.repo.clone(),
            command: p.command.clone(),
            row: None,
            skipped: p.skipped,
            borrowed: !p.own,
        });
    ran.chain(waits).collect()
}

/// Whether `feature`'s checks are going: starting, waiting for a setup, or
/// running.
pub(super) fn checks_going(feature: &Feature, ctx: RowCtx) -> bool {
    ctx.checks_busy || checks_running(feature)
}

/// Whether `stage` is the Verify `feature` is at, its checks going: its page
/// then offers only what `works_during_checks` lets through.
pub(super) fn verify_going(feature: &Feature, stage: Stage, ctx: RowCtx) -> bool {
    stage == Stage::Verify && feature.current() == Some(stage) && checks_going(feature, ctx)
}

/// Whether Verify's page offers `outcome` while its checks go: what stops
/// them, what only reads, and what leaves the code they run on be. Anything
/// else either refuses now or changes the code under them, and a new button
/// is left out until someone decides otherwise.
pub(super) fn works_during_checks(outcome: &PanelOutcome) -> bool {
    matches!(
        outcome,
        PanelOutcome::StopChecks
            | PanelOutcome::StopCheck(_)
            | PanelOutcome::ShowCheck(_)
            | PanelOutcome::Evidence
            | PanelOutcome::SetupLog
            | PanelOutcome::EditCommands(_)
            | PanelOutcome::ReviewFixPlan
            | PanelOutcome::DropFixPlan
            | PanelOutcome::ShowAgent(_)
            | PanelOutcome::ShowOwnAgent(_)
            | PanelOutcome::ShowResolvingAgent
            | PanelOutcome::GoToCurrent
    )
}

/// What skipping check `entry` of `feature` reads as, and does; nothing for
/// one it can't skip, while the checks go, or on a stage not reached.
pub(super) fn skip_action(
    feature: &Feature,
    stage: Stage,
    entry: &CheckEntry,
    ctx: RowCtx,
) -> Option<(&'static str, PanelOutcome)> {
    let can = entry.borrowed
        && feature.status == Status::Open
        && stage_mark(feature, stage) != StageMark::Later
        && !checks_going(feature, ctx);
    can.then(|| {
        let label = if entry.skipped { "Don't skip" } else { "Skip" };
        let skip = PanelOutcome::SkipCheck {
            repo: entry.repo.clone(),
            command: entry.command.clone(),
            skip: !entry.skipped,
        };
        (label, skip)
    })
}

/// The buttons on `stage`'s page of `feature`, as `page_actions` has them,
/// and what its cards of `plan`'s checks and of `criteria` offer.
pub(super) fn stage_actions(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    plan: &[PlannedCheck],
    criteria: &[Row],
    notes: &[Comment],
) -> Vec<(PageButton, PanelOutcome)> {
    let mut out = page_actions(feature, stage, ctx);
    out.extend(note_actions(feature, stage, notes));
    let entries = check_entries(feature, stage, plan);
    // What it skips doesn't run, once or at all.
    out.retain(|(_, outcome)| {
        !matches!(outcome, PanelOutcome::RerunCheck(i)
            if entries.get(*i).is_some_and(|entry| entry.skipped))
    });
    out.extend(
        (entries.iter())
            .filter_map(|entry| skip_action(feature, stage, entry, ctx))
            .map(|(label, outcome)| {
                let style = match label {
                    "Skip" => ButtonStyle::Quiet,
                    _ => ButtonStyle::Plain,
                };
                let label = label.to_owned();
                (PageButton { label, style }, outcome)
            }),
    );
    if marks_criteria(feature, stage) {
        out.extend((criteria.iter().enumerate()).flat_map(|(i, row)| criterion_buttons(i, row)));
    }
    if shows_criteria(feature, stage) && !criteria.is_empty() {
        out.extend(fold_actions());
    }
    out
}

/// Makes button `i` of a page's `buttons` its one `primary`: the only filled
/// one, on the page's bar.
pub(crate) fn fill_only(buttons: &mut [PageButton], primary: &mut Option<usize>, i: usize) {
    for button in buttons.iter_mut() {
        if button.style == ButtonStyle::Primary {
            button.style = ButtonStyle::Plain;
        }
    }
    if let Some(button) = buttons.get_mut(i) {
        button.style = ButtonStyle::Primary;
        *primary = Some(i);
    }
}

/// The buttons of the note or comment being written on a card, last of the
/// page's: Save and Discard, as a comment on a line of the diff has. None
/// while none is written.
/// Where `editing_actions` put Save and Discard among the page's `buttons`.
pub(super) fn note_buttons(buttons: &[PageButton]) -> Vec<usize> {
    let n = buttons.len();
    if n >= 2 && buttons[n - 1].label == "Save" && buttons[n - 2].label == "Discard" {
        vec![n - 2, n - 1]
    } else {
        Vec::new()
    }
}

pub(super) fn editing_actions(writing: bool) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    match writing {
        true => vec![
            (
                button("Discard", ButtonStyle::Quiet),
                PanelOutcome::NoteDiscard,
            ),
            (button("Save", ButtonStyle::Primary), PanelOutcome::NoteSave),
        ],
        false => Vec::new(),
    }
}

/// The buttons folding Manual test's criteria to their lines, or opening
/// them to their steps: all, those passed, those read.
pub(crate) fn fold_actions() -> Vec<(PageButton, PanelOutcome)> {
    use crate::feature::manual::FoldScope;
    // Folding goes by scope; opening only all at once: opening those passed
    // or read is rare, and has its keys (Alt+Shift+D, Alt+Shift+S).
    let button = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Plain,
    };
    vec![
        (
            button("All"),
            PanelOutcome::FoldCriteria(FoldScope::All, false),
        ),
        (
            button(FOLD_PASSED),
            PanelOutcome::FoldCriteria(FoldScope::Done, false),
        ),
        (
            button(FOLD_READ),
            PanelOutcome::FoldCriteria(FoldScope::Seen, false),
        ),
        (
            button("Open all"),
            PanelOutcome::FoldCriteria(FoldScope::All, true),
        ),
    ]
}

/// The fold buttons' words for the criteria passed, and those read (opened
/// or marked).
pub(super) const FOLD_PASSED: &str = "Passed";
const FOLD_READ: &str = "Read";

/// The row over Manual test's criteria: "Fold" with its buttons of `actions`
/// (see `fold_actions`), and opening them all.
pub(crate) fn fold_toolbar(actions: &[(PageButton, PanelOutcome)]) -> PageSection {
    let of = |open: bool| -> Vec<usize> {
        (actions.iter().enumerate())
            .filter(|(_, (_, o))| matches!(o, PanelOutcome::FoldCriteria(_, x) if *x == open))
            .map(|(at, _)| at)
            .collect()
    };
    PageSection::Toolbar {
        groups: vec![("Fold".to_owned(), of(false)), (String::new(), of(true))],
    }
}

/// Whether `stage`'s page of `feature` is where its criteria are tried:
/// Manual test's, while the feature is at it.
pub(crate) fn tries_criteria(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest
        && feature.status == Status::Open
        && stage_mark(feature, stage) == StageMark::Current
}

/// Whether `stage`'s page of `feature` lets its criteria be marked: Manual
/// test's while the feature is open and got to it, also while Verify and the
/// reviews go over its fixes, and once it is done. Only at it do marks move
/// it on (`tries_criteria`): failures marked elsewhere wait for it.
pub(crate) fn marks_criteria(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest && feature.status == Status::Open && reached_manual(feature)
}

/// Whether `stage`'s page of `feature` shows its criteria: Manual test's, once
/// the feature got to it, also when its fixes took it back to Verify.
pub(crate) fn shows_criteria(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest && reached_manual(feature)
}

/// Whether `feature` got to Manual test: at it or past it, or before it
/// again with what it found kept.
fn reached_manual(feature: &Feature) -> bool {
    stage_mark(feature, Stage::ManualTest) != StageMark::Later
        || feature.ran_once.contains_key(&Stage::ManualTest)
        || feature.manual.is_some()
}

/// The buttons on criterion `i`'s card: Pass and Fail, always both and in
/// that order, the one it stands at filled. Pressing that one again takes
/// it back; Fail on one not failed asks what went wrong. A failed one can
/// have its note edited too.
pub(crate) fn criterion_buttons(i: usize, row: &Row) -> Vec<(PageButton, PanelOutcome)> {
    use crate::manual_test::Outcome;
    let button = |label: &str, style, outcome| {
        let button = PageButton {
            label: label.to_owned(),
            style,
        };
        (button, PanelOutcome::Manual(outcome))
    };
    if row.comment {
        return comment_buttons(i, row);
    }
    let (pass, fail) = match row.found {
        None => (ButtonStyle::Plain, ButtonStyle::Plain),
        Some(Verdict::Pass) => (ButtonStyle::Passed, ButtonStyle::Plain),
        Some(Verdict::Fail) => (ButtonStyle::Plain, ButtonStyle::Failed),
    };
    // Fail asks what went wrong, unless it is what was chosen.
    let (fail_label, fail_outcome) = match row.found {
        Some(Verdict::Fail) => ("Fail", Outcome::Clear(i)),
        _ => ("Fail\u{2026}", Outcome::Write(i)),
    };
    let mut out = Vec::new();
    // To the left of the pair, so the pair stays put.
    if row.found == Some(Verdict::Fail) {
        let edit = button("Edit note\u{2026}", ButtonStyle::Quiet, Outcome::Write(i));
        out.push(edit);
    }
    out.push(button("Pass", pass, Outcome::Toggle(i)));
    out.push(button(fail_label, fail, fail_outcome));
    out
}

/// The buttons on comment `i`'s card: editing and deleting it, then Done and
/// Not done, always both and in that order, the one it stands at filled.
/// Done again opens it again; Not done on one whose fix is made says the fix
/// didn't do it, to send again.
fn comment_buttons(i: usize, row: &Row) -> Vec<(PageButton, PanelOutcome)> {
    use crate::manual_test::Outcome;
    let button = |label: &str, style, outcome| {
        let button = PageButton {
            label: label.to_owned(),
            style,
        };
        (button, PanelOutcome::Manual(outcome))
    };
    let (done, open) = match row.found {
        None => (ButtonStyle::Plain, ButtonStyle::Plain),
        Some(Verdict::Pass) => (ButtonStyle::Passed, ButtonStyle::Plain),
        Some(Verdict::Fail) => (ButtonStyle::Plain, ButtonStyle::Failed),
    };
    vec![
        button("Edit\u{2026}", ButtonStyle::Quiet, Outcome::EditComment(i)),
        button("Delete", ButtonStyle::Quiet, Outcome::DeleteComment(i)),
        button("Done", done, Outcome::Toggle(i)),
        button("Not done", open, Outcome::Fail(i, String::new())),
    ]
}

/// What criterion `row` checks, step by step, for its card while open: a
/// line each for its Given, When and Then, or its text as the brief has it
/// when it doesn't split. A comment's, a line each of its text.
fn criterion_steps(row: &Row) -> Vec<(&'static str, String)> {
    if row.comment {
        return (row.text.lines())
            .filter(|line| !line.trim().is_empty())
            .map(|line| ("", line.to_owned()))
            .collect();
    }
    match crate::feature::manual::split_gwt(&row.text) {
        Some([given, when, then]) => vec![
            ("Given", given.to_owned()),
            ("When", when.to_owned()),
            ("Then", then.to_owned()),
        ],
        None => vec![("", row.text.clone())],
    }
}

/// What the row of criterion `row` says under its steps: whether it was
/// tried on other code than the feature's now.
fn criterion_earlier(row: &Row) -> String {
    match row.earlier && row.found.is_some() && !row.made {
        true => "Tried before the latest fixes".to_owned(),
        false => String::new(),
    }
}

/// What criterion `row` checks, step by step, as the criteria strip shows
/// it: a line each for its Given, When and Then, or its text as the brief
/// has it when it doesn't split.
pub(crate) fn criterion_details(row: &Row) -> String {
    let mut text = match crate::feature::manual::split_gwt(&row.text) {
        Some([given, when, then]) => format!("Given {given}\nWhen {when}\nThen {then}"),
        None => row.text.clone(),
    };
    if row.earlier && row.found.is_some() && !row.made {
        text.push_str("\n\nTried before the latest fixes");
    }
    if let Some(n) = row.retest {
        text.push_str(&format!("\n\nChanged by fix round {n}: try it again"));
    }
    text
}

/// Criterion `i`'s card on Manual test's page, its buttons those in
/// `actions`: its dot tells how it went, the note what went wrong. While
/// `trying`, one that passed fades: the rest are what is left to do.
pub(super) fn criterion_card(
    i: usize,
    row: &Row,
    actions: &[(PageButton, PanelOutcome)],
    trying: bool,
) -> FindingCard {
    let tag = match row.found {
        None => FindingTag::NotRun,
        Some(Verdict::Pass) => FindingTag::Passed,
        Some(Verdict::Fail) => FindingTag::Failed,
    };
    let again = match row.comment {
        true => "look again",
        false => "try again",
    };
    let state = match (row.made, row.sent, row.retest) {
        (true, _, _) => format!("fix made: {again}"),
        (false, true, _) => {
            crate::feature::review_import::sent_word(false, row.planning).to_owned()
        }
        (false, false, Some(n)) => format!("changed by fix round {n}: {again}"),
        (false, false, None) if row.comment && row.found == Some(Verdict::Fail) => {
            "open".to_owned()
        }
        (false, false, None) => String::new(),
    };
    // Folded, its line still says what went wrong: the note isn't only
    // behind its Edit button.
    let first = row
        .note
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim);
    let first = first.filter(|_| row.found == Some(Verdict::Fail));
    let state = match (row.open || row.comment, first) {
        (false, Some(note)) if state.is_empty() => format!("\u{201c}{note}\u{201d}"),
        (false, Some(note)) => format!("{state} \u{b7} \u{201c}{note}\u{201d}"),
        _ => state,
    };
    FindingCard {
        tag,
        title: row.title.clone(),
        state,
        place: row.id.clone(),
        body: criterion_earlier(row),
        sheet: Some(Box::new(chrome::SheetRow {
            open: row.open,
            steps: criterion_steps(row),
            hidden: false,
            needle: String::new(),
            whole: false,
        })),
        preview: Vec::new(),
        fix: String::new(),
        note: {
            let label = match row.comment {
                true => "Note",
                false => "What went wrong",
            };
            match (row.note.is_empty(), row.shots.len()) {
                (true, 0) => String::new(),
                (true, 1) => "1 screenshot".to_owned(),
                (true, n) => format!("{n} screenshots"),
                (false, 0) => format!("{label}: {}", row.note),
                (false, 1) => format!("{label}: {} (1 screenshot)", row.note),
                (false, n) => format!("{label}: {} ({n} screenshots)", row.note),
            }
        },
        comment: String::new(),
        comment_buttons: Vec::new(),
        made: row.made,
        faded: trying && tag == FindingTag::Passed,
        collapsed: !row.open,
        folds: false,
        stripe: None,
        field: None,
        field_hint: "What went wrong",
        selected: false,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                matches!(o, PanelOutcome::Manual(outcome) if outcome.criterion() == Some(i))
            })
            .map(|(at, _)| at)
            .collect(),
    }
}

/// The cards of `state`'s criteria, in their order, after the row folding
/// them.
pub(super) fn criterion_sections<'a>(
    state: &'a PanelState,
    actions: &'a [(PageButton, PanelOutcome)],
    trying: bool,
) -> impl Iterator<Item = PageSection> + 'a {
    let cards = (state.criteria.iter().enumerate())
        .map(move |(i, row)| PageSection::Finding(criterion_card(i, row, actions, trying)));
    std::iter::once(fold_toolbar(actions)).chain(cards)
}

/// The buttons on check `i`'s card: its terminal, then stopping it while it
/// runs, or running it again while Verify is `current`.
pub(super) fn check_buttons(
    i: usize,
    row: &feature::verify::CheckRun,
    feature: &Feature,
    current: bool,
) -> Vec<(PageButton, PanelOutcome)> {
    let plain = |label: &str| PageButton {
        label: label.to_owned(),
        style: ButtonStyle::Plain,
    };
    let mut out = Vec::new();
    if row.pane.is_some() {
        out.push((plain("Show"), PanelOutcome::ShowCheck(i)));
    }
    let running = checks_running(feature);
    if running && row.outcome.is_none() {
        // Told once: it ends by itself now.
        if !row.stopping {
            out.push((plain("Stop"), PanelOutcome::StopCheck(i)));
        }
    } else if current && !running {
        out.push((plain("Run again"), PanelOutcome::RerunCheck(i)));
    }
    out
}

/// Whether Manual test's page of `feature` has failures or comments to send
/// to Build now: none waits for a round to end.
/// Why Manual test's fixes go through a plan, if they do: its comments are
/// the user's own instructions (a failure's note only says what went wrong).
pub(super) fn manual_own_words(ctx: RowCtx) -> Option<String> {
    (ctx.manual.comments_open > 0)
        .then(|| review_import::own_words(&plural(ctx.manual.comments_open, "comment")))
}

pub(super) fn manual_sends(feature: &Feature, ctx: RowCtx) -> bool {
    // Someone else's pull request's go to GitHub with the review.
    ctx.manual.failed > ctx.manual.sent && feature.fix_round.is_none() && !feature.posts_review()
}

/// Whether some check of `feature` runs that hasn't been told to stop.
pub(super) fn can_stop_checks(feature: &Feature) -> bool {
    checks_running(feature)
        && (check_rows(feature, Stage::Verify).iter())
            .any(|row| row.outcome.is_none() && !row.stopping)
}

/// The check an action on a card is about.
pub(super) fn outcome_check(outcome: &PanelOutcome) -> Option<usize> {
    match outcome {
        PanelOutcome::RerunCheck(i) | PanelOutcome::StopCheck(i) | PanelOutcome::ShowCheck(i) => {
            Some(*i)
        }
        _ => None,
    }
}

/// A check's card on Verify's page, its buttons those in `actions`: its dot
/// tells how it went, the rest what that doesn't, and its output the last
/// lines of what it prints, or why it failed. A check not run again says which
/// run its result is from, and one on code changed since says so.
pub(super) fn check_card(
    entry: &CheckEntry,
    feature: &Feature,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> FindingCard {
    let verify = feature.verify.as_ref();
    let row = (entry.row).and_then(|i| check_rows(feature, Stage::Verify).get(i));
    let outcome = row.and_then(|row| row.outcome.as_ref());
    let running = row.is_some() && outcome.is_none() && checks_running(feature);
    let (tag, how) = match outcome {
        // It may still be going, skipped since: that ends by itself.
        _ if entry.skipped && !running => (
            FindingTag::Skipped,
            Some("skipped for this feature".to_owned()),
        ),
        Some(o) if o.passed() => {
            let (said, counted) = crate::test_runs::verdict(&o.tail, &entry.command);
            let counted = counted.filter(|_| said == crate::ci::CiState::Passed);
            (FindingTag::Passed, counted)
        }
        Some(o) if o.interrupted => (FindingTag::Stopped, Some(o.how())),
        Some(o) => (FindingTag::Failed, Some(o.how())),
        None if running && row.is_some_and(|row| row.queued) => (FindingTag::Queued, None),
        None if running => (FindingTag::Running, None),
        None => (FindingTag::NotRun, None),
    };
    let took = |secs: u64| crate::clock::took(std::time::Duration::from_secs(secs));
    let mut said = Vec::new();
    match (tag, outcome) {
        (FindingTag::Running | FindingTag::Queued, _) if row.is_some_and(|row| row.stopping) => {
            said.push("stopping".to_owned());
        }
        (FindingTag::Queued, _) => said.push("waiting for a check in this folder".to_owned()),
        (FindingTag::Running, _) => {
            let began = row.and_then(|row| row.began);
            said.extend(
                began
                    .or(verify.map(|v| v.started))
                    .map(|at| took(now.saturating_sub(at))),
            );
            said.extend(
                row.and_then(|row| row.last_secs)
                    .map(|secs| format!("took {} last run", took(secs))),
            );
        }
        (FindingTag::Skipped, _) | (_, None) => {}
        (_, Some(o)) => {
            said.push(took(o.secs));
            if let Some(row) = row.filter(|row| verify.is_some_and(|v| v.n != row.run)) {
                said.push(format!("from run {}", row.run));
            }
            if on_older_code(feature, state.probe) {
                said.push("on older code".to_owned());
            }
        }
    }
    let mut at = vec![feature::verify::place_of(&entry.repo).to_owned()];
    at.extend(how);
    let preview = match (tag, outcome) {
        (FindingTag::Running | FindingTag::Queued, _) => {
            let live = (row.and_then(|row| row.pane.as_ref()))
                .and_then(|pane| state.check_tails?.get(pane))
                .map_or(Vec::new(), |tail| tail.lines.clone());
            pad_preview(live)
        }
        (FindingTag::Skipped, _) | (_, None) => Vec::new(),
        (_, Some(o)) => o.preview(),
    };
    FindingCard {
        tag,
        title: entry.command.lines().next().unwrap_or_default().to_owned(),
        state: said.join(" \u{b7} "),
        place: at.join(" \u{b7} "),
        body: String::new(),
        preview,
        fix: String::new(),
        note: String::new(),
        comment: String::new(),
        comment_buttons: Vec::new(),
        made: false,
        faded: tag == FindingTag::Skipped,
        collapsed: false,
        folds: false,
        stripe: None,
        field: None,
        field_hint: "",
        selected: false,
        sheet: None,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                entry.row.is_some() && outcome_check(o) == entry.row
                    || matches!(o, PanelOutcome::SkipCheck { repo, command, .. }
                        if *repo == entry.repo && *command == entry.command)
            })
            .map(|(i, _)| i)
            .collect(),
    }
}

/// `lines` of a check that runs, as tall as it will be: the cards under it
/// stay where they are as it prints.
fn pad_preview(mut lines: Vec<String>) -> Vec<String> {
    lines.resize(feature::verify::PREVIEW_LINES, String::new());
    lines
}

/// Whether the code changed since `feature`'s last run read it, as the latest
/// probe read it. A finished feature's code isn't read.
pub(super) fn on_older_code(feature: &Feature, probe: Option<&Probe>) -> bool {
    let ran = (feature.verify.as_ref())
        .and_then(|v| v.patch.as_deref())
        .filter(|patch| !patch.is_empty());
    let now = match probe.map(|p| &p.reading) {
        Some(Ok(reading)) if feature.status == Status::Open => Some(reading.patch.as_str()),
        _ => None,
    };
    ran.zip(now.filter(|patch| !patch.is_empty()))
        .is_some_and(|(ran, now)| ran != now)
}

/// The heading over Verify's cards, with how many of them stand how.
pub(super) fn checks_heading(cards: &[FindingCard]) -> String {
    let mut out = "Checks".to_owned();
    for (tag, what) in [
        (FindingTag::Passed, "passed"),
        (FindingTag::Failed, "failed"),
        (FindingTag::Running, "running"),
        (FindingTag::Queued, "waiting"),
        (FindingTag::Stopped, "stopped"),
        (FindingTag::NotRun, "not run"),
        (FindingTag::Skipped, "skipped"),
    ] {
        let n = cards.iter().filter(|c| c.tag == tag).count();
        if n > 0 {
            out.push_str(&format!(" \u{b7} {n} {what}"));
        }
    }
    out
}

/// Where comment `c` of the diff is, in a few words: `api/src/a.rs:12`; the
/// repository left out for a feature over one (`multi` false).
pub(super) fn note_place(c: &Comment, multi: bool) -> String {
    if crate::review::is_general(c) {
        return "the whole change".to_owned();
    }
    let repo = (c.root.file_name().and_then(|n| n.to_str())).filter(|_| multi);
    let mut at = match repo {
        Some(repo) => format!("{repo}/{}", c.path),
        None => c.path.clone(),
    };
    if let Some(line) = c.lines.last() {
        at.push_str(&format!(":{}", line.number));
    }
    at
}

/// What the finding dialog calls comment `c`, and keeps its draft by.
pub(crate) fn note_id(c: &Comment) -> String {
    note_place(c, false)
}

/// The card of `c`, a comment the user left in the diff, its buttons those
/// in `actions`.
pub(super) fn note_card(
    c: &Comment,
    multi: bool,
    actions: &[(PageButton, PanelOutcome)],
) -> FindingCard {
    let (title, body) = match c.text.split_once('\n') {
        Some((title, body)) => (title.trim().to_owned(), body.trim().to_owned()),
        None => (c.text.trim().to_owned(), String::new()),
    };
    FindingCard {
        tag: FindingTag::Yours,
        title,
        state: "goes with the fixes".to_owned(),
        place: note_place(c, multi),
        body,
        preview: Vec::new(),
        fix: String::new(),
        note: String::new(),
        comment: String::new(),
        comment_buttons: Vec::new(),
        made: false,
        faded: false,
        collapsed: false,
        folds: false,
        stripe: None,
        field: None,
        field_hint: "",
        selected: false,
        sheet: None,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| outcome_note(o) == Some(c))
            .map(|(i, _)| i)
            .collect(),
    }
}

/// The card of `c`, a comment that went to Build, whose fix is `made` or not;
/// it holds its buttons in `actions`, none for one of an earlier round.
pub(super) fn sent_note_card(
    c: &Comment,
    multi: bool,
    (made, planning): (bool, bool),
    actions: &[(PageButton, PanelOutcome)],
) -> FindingCard {
    let mut card = note_card(c, multi, actions);
    if let Some(flag) = &c.flag {
        card.place = format!("{} \u{b7} {}", flag.id, card.place);
    }
    card.state = review_import::sent_word(made, planning).to_owned();
    card.made = made;
    card.stripe = Some(CardStripe::Sent);
    card
}

/// How `round`, an earlier review, went, in a few words: "3 fixed · 1 raised
/// again".
pub(super) fn round_count(round: &ReviewRound) -> String {
    let count = |states: &[FindingState]| {
        (round.findings.iter())
            .filter(|f| states.contains(&f.state))
            .count()
    };
    [
        (count(&[FindingState::Fixed]), "fixed"),
        (count(&[FindingState::Sent]), "raised again"),
        (count(&[FindingState::Dismissed]), "dismissed"),
        (
            count(&[FindingState::Open, FindingState::Fix]),
            "carried over",
        ),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// A finding of an earlier review: a record, with nothing to do on it.
pub(super) fn round_card(f: &ReviewFinding, multi: bool) -> FindingCard {
    let mut card = item_card(Item::Finding(f), multi, &[], false);
    card.faded = true;
    card.made = false;
    // Nothing can select it, so nothing could open it again.
    card.collapsed = false;
    card.stripe = None;
    if f.state == FindingState::Sent {
        "raised again".clone_into(&mut card.state);
    }
    card
}

/// What AI review's row does: show its agent while it runs (start one
/// when there is none), then once its findings are read, show the first
/// to decide, or send those to fix to the Build agent, through a plan if
/// the user wrote anything of their own. Never marked done by hand: gerdr
/// marks it once no finding holds up.
pub(super) fn review_action(feature: &Feature, ctx: RowCtx) -> PanelOutcome {
    let stage = Stage::AiReview;
    let Some(run) = feature.runs.get(&stage) else {
        return PanelOutcome::StartAgent(stage, false);
    };
    let review = feature.review.as_ref().filter(|_| run.ready.is_some());
    match review {
        Some(r) if r.count(FindingState::Open) > 0 => PanelOutcome::Findings,
        Some(r) if r.count(FindingState::Fix) > 0 => {
            match review_import::needs_plan(&r.findings, ctx.comments) {
                true => PanelOutcome::SendFixes,
                false => PanelOutcome::MakeFindingFixes,
            }
        }
        _ if run.waits_to_send() => PanelOutcome::SendPrompt(stage),
        _ if run.live() => PanelOutcome::ShowAgent(stage),
        _ if run.resumes() => PanelOutcome::ResumeAgent(stage),
        _ => PanelOutcome::StartAgent(stage, false),
    }
}

/// `n` `thing`s, in words: "1 finding", "3 findings".
pub(crate) fn plural(n: usize, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

/// The bar over Manual test's criteria `rows`: how many passed, failed and
/// are left, and the fixes made to try again; none with no criteria.
pub(super) fn test_strip(rows: &[Row]) -> Option<chrome::TestStrip> {
    if rows.is_empty() {
        return None;
    }
    let mut n = crate::feature::manual::Counts::default();
    // The user's comments are counted apart: a new one isn't a failed
    // criterion (`strip_text` says them as comments).
    for row in rows {
        let comment = usize::from(row.comment);
        match row.found {
            Some(Verdict::Pass) => {
                n.passed += 1;
                n.comments_done += comment;
            }
            Some(Verdict::Fail) => {
                n.failed += 1;
                n.comments_open += comment;
                n.sent += usize::from(row.sent);
                n.made += usize::from(row.made);
            }
            None => {
                n.untested += 1;
                n.comments_untested += comment;
            }
        }
    }
    Some(chrome::TestStrip {
        passed: n.passed - n.comments_done,
        failed: n.failed - n.comments_open,
        untested: n.untested - n.comments_untested,
        text: crate::manual_test::strip_text(n),
        search: None,
        shown: None,
    })
}

/// The detail of AI review's row while it is current, beside `agent`,
/// its agent's line: that line while the agent reviews; once its findings
/// are read, what waits for the user.
pub(super) fn review_line(feature: &Feature, agent: Option<&str>) -> Option<String> {
    let run = feature.runs.get(&Stage::AiReview)?;
    let review = feature.review.as_ref().filter(|_| run.ready.is_some());
    let Some(review) = review else {
        return agent.map(str::to_owned);
    };
    let open = review.count(FindingState::Open);
    let fix = review.count(FindingState::Fix);
    let sent = review.count(FindingState::Sent);
    let wanted = words_in(ItemOf::Finding, review.posts).wanted;
    Some(if open > 0 {
        format!("{} to decide", plural(open, "finding"))
    } else if fix > 0 {
        format!("{} {wanted}", plural(fix, "finding"))
    } else if sent > 0 {
        let planning = feature::fix_round::planning(feature.fix_round.as_ref(), Stage::AiReview);
        let word = review_import::sent_word(false, planning);
        let sent = format!("{} {word}", plural(sent, "finding"));
        match review.fixes_made() {
            0 => sent,
            made => format!("{sent} \u{b7} {made} made"),
        }
    } else if review.findings.is_empty() {
        "no findings".into()
    } else {
        "all findings fixed or dismissed".into()
    })
}

/// The agent stage under `row` when it is current and has had an agent:
/// Ctrl+R starts a fresh one.
pub(super) fn fresh_stage(feature: &Feature, row: PanelRow) -> Option<Stage> {
    let PanelRow::Stage(stage) = row else {
        return None;
    };
    (feature.status == Status::Open
        && feature.current() == Some(stage)
        && needs(stage).is_some()
        && feature.runs.contains_key(&stage))
    .then_some(stage)
}

/// The stage Ctrl+Shift+Del starts over on `stage`'s page of `feature`:
/// the current one, once there is something to lose. Discover has no stage
/// before it to free, and Ctrl+R starts its agent afresh. A fix plan open
/// on the page goes first: that key drops it. Not while work of the feature
/// runs: it would write its result into the stage started over.
pub(super) fn start_over_stage(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<Stage> {
    let live = (feature.runs.get(&stage)).is_some_and(|run| run.live());
    (feature.status == Status::Open
        && feature.current() == Some(stage)
        && stage != Stage::Discover
        && fix_round_on(feature, stage).is_none()
        && !ctx.feature_busy
        && (live || feature.has_output(stage, |s| ctx.has_file(s))))
    .then_some(stage)
}

/// Whether Verify of `feature` wrote `evidence.md`: a run of it ended, or
/// one before the one running.
pub(super) fn has_evidence(feature: &Feature) -> bool {
    feature
        .verify
        .as_ref()
        .is_some_and(|v| v.finished.is_some() || v.n > 1)
}

/// What Ctrl+Shift+O does on `row`: open the pull request of a repository
/// that has one, or Verify's evidence once it has some.
pub(super) fn row_page(feature: &Feature, row: PanelRow) -> Option<PanelOutcome> {
    match row {
        PanelRow::Repo(i) => {
            repo_button(feature, i).filter(|out| matches!(out, PanelOutcome::OpenPr(_)))
        }
        PanelRow::Other => None,
        PanelRow::Stage(Stage::Verify) => has_evidence(feature).then_some(PanelOutcome::Evidence),
        PanelRow::Stage(_) => None,
    }
}

/// What Enter does on `row`, in words for its hint.
pub(super) fn row_hint(feature: &Feature, row: PanelRow, ctx: RowCtx) -> &'static str {
    row_action(feature, row, ctx).map_or("", |action| action_hint(&action))
}

/// What a key doing `action` is called in the hints.
pub(super) fn action_hint(action: &PanelOutcome) -> &'static str {
    match action {
        PanelOutcome::StartAgent(..) => "start agent",
        PanelOutcome::SendPrompt(_) => "send prompt",
        PanelOutcome::ShowAgent(_)
        | PanelOutcome::ShowOwnAgent(_)
        | PanelOutcome::ShowResolvingAgent => "show agent",
        PanelOutcome::ResumeAgent(_) => "resume agent",
        PanelOutcome::OpenDoc(Stage::Discover) => "review brief",
        PanelOutcome::OpenDoc(_) => "review plan",
        PanelOutcome::ApproveDoc(_) => "approve",
        PanelOutcome::Continue(_) => "continue",
        PanelOutcome::RunChecks => "run checks",
        PanelOutcome::SetUp => "set up",
        PanelOutcome::SetupLog => "show setup log",
        PanelOutcome::StartSandbox => "start sandbox\u{2026}",
        PanelOutcome::OpenSandbox => "open app",
        PanelOutcome::OpenSandboxPage(_) | PanelOutcome::NextSandboxPage => "open page",
        PanelOutcome::SandboxLan => "network",
        PanelOutcome::ShowSandbox => "show terminals",
        PanelOutcome::RestartSandbox => "restart sandbox",
        PanelOutcome::ResetSandbox => "reset data\u{2026}",
        PanelOutcome::StopSandbox => "stop sandbox\u{2026}",
        PanelOutcome::RemoveSandbox => "remove sandbox\u{2026}",
        PanelOutcome::Findings => "review in diff",
        PanelOutcome::SendFixes | PanelOutcome::PlanFixes => "plan fixes",
        PanelOutcome::MakeFindingFixes => "make fixes",
        PanelOutcome::FixAllFindings => "fix all open",
        PanelOutcome::ReviewFixPlan => "review fix plan",
        PanelOutcome::MakeFixes => "make fixes",
        PanelOutcome::ResendFixes => "send the fixes again",
        PanelOutcome::OpenRoundPlan(_) => "open the round's plan",
        PanelOutcome::PlanChanges => "show what the final plan changed",
        PanelOutcome::DraftChanges(..) => "show what the draft changed",
        PanelOutcome::RoundDiff(_) => "show what the fixes changed",
        PanelOutcome::MakeVerifyFixes => "make fixes",
        PanelOutcome::AskFixPlan => "ask again",
        PanelOutcome::EditCommands(_) => "edit commands",
        PanelOutcome::CodeReview => "review code",
        PanelOutcome::ManualTest => "try the criteria",
        PanelOutcome::Manual(crate::manual_test::Outcome::MarkPassed) => "mark passed",
        PanelOutcome::Manual(crate::manual_test::Outcome::SendFailures) => "plan fixes",
        PanelOutcome::Manual(crate::manual_test::Outcome::MakeFixes) => "make fixes",
        PanelOutcome::ReviewInPlan => "review in plan",
        PanelOutcome::AcceptAllSuggestions => "accept all open",
        PanelOutcome::AskReviewer => "ask reviewer",
        PanelOutcome::WriteFinalPlan => "go on",
        PanelOutcome::Tick => "mark done",
        PanelOutcome::Ship => "ship\u{2026}",
        PanelOutcome::WriteSummary => "write the summary again",
        PanelOutcome::CheckShip => "check again",
        PanelOutcome::UndoShip(..) => "undo\u{2026}",
        PanelOutcome::UndoAfter(step) => after_undo_words(*step),
        PanelOutcome::UpdateBase(..) => "update from the base\u{2026}",
        PanelOutcome::PullBase(_) => "pull the base",
        PanelOutcome::CloseShipped => "close the workspace",
        PanelOutcome::ResolveConflicts(_, true) => "plan the resolution",
        PanelOutcome::ResolveConflicts(_, false) => "resolve with an agent",
        PanelOutcome::ReviewConflictPlan(_) => "review the plan",
        PanelOutcome::StopResolving(_) => "stop\u{2026}",
        PanelOutcome::EditSummary => "edit the summary",
        PanelOutcome::Finish => "finish\u{2026}",
        PanelOutcome::UpdatePrHead => "update to new head",
        PanelOutcome::RerunCi => "rerun on GitHub",
        PanelOutcome::SubmitReview(feature::pr_submit::Event::Comment) => "comment\u{2026}",
        PanelOutcome::SubmitReview(feature::pr_submit::Event::Approve) => "approve\u{2026}",
        PanelOutcome::SubmitReview(feature::pr_submit::Event::RequestChanges) => {
            "request changes\u{2026}"
        }
        PanelOutcome::PushPr => "push\u{2026}",
        PanelOutcome::FixHere => "fix here or comment",
        PanelOutcome::EditPulls => "change pull requests",
        PanelOutcome::OpenPr(_) => "open on GitHub",
        PanelOutcome::FinishReview => "finish\u{2026}",
        PanelOutcome::Untick(_) => "reopen",
        PanelOutcome::GoToCurrent => "go to the current stage",
        PanelOutcome::SkipPlanReview => "skip review",
        PanelOutcome::ChangeDecisions => "change decisions",
        PanelOutcome::DropFixPlan => "drop plan\u{2026}",
        PanelOutcome::StartOver(_) => "start over\u{2026}",
        PanelOutcome::Touch(_) => "add to feature",
        PanelOutcome::Open(_) => "open tab",
        _ => "",
    }
}

/// The label of `stage`'s button in `feature`: none for a later one.
pub(super) fn stage_button(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<&'static str> {
    match row_action(feature, PanelRow::Stage(stage), ctx)? {
        PanelOutcome::Tick => Some("Mark done"),
        PanelOutcome::Ship => Some("Ship\u{2026}"),
        PanelOutcome::Finish => Some("Finish\u{2026}"),
        PanelOutcome::UpdatePrHead => Some(UPDATE_HEAD),
        PanelOutcome::RerunCi => Some("Rerun failed on GitHub"),
        PanelOutcome::SubmitReview(event) => Some(event.label()),
        PanelOutcome::PushPr => Some("Push\u{2026}"),
        PanelOutcome::OpenPr(_) => Some("Open checks"),
        PanelOutcome::FinishReview => Some("Finish\u{2026}"),
        PanelOutcome::Untick(_) => Some("Reopen"),
        PanelOutcome::StartAgent(..) => Some("Start agent"),
        PanelOutcome::SendPrompt(_) => Some("Send prompt"),
        PanelOutcome::ShowAgent(_) => Some("Show agent"),
        PanelOutcome::ResumeAgent(_) => Some("Resume agent"),
        PanelOutcome::OpenDoc(_) => Some("Review"),
        PanelOutcome::Continue(_) => Some("Continue"),
        PanelOutcome::RunChecks => Some("Run checks"),
        PanelOutcome::Findings => Some("Review in diff"),
        PanelOutcome::SendFixes => Some("Plan fixes"),
        PanelOutcome::MakeFindingFixes => Some("Make fixes"),
        PanelOutcome::CodeReview => Some("Review code"),
        PanelOutcome::ReviewInPlan => Some(crate::worktree_diff::REVIEW_IN_PLAN),
        PanelOutcome::WriteFinalPlan => {
            let accepted = (feature.plan_review.as_ref()).is_some_and(|r| r.any_accepted());
            Some(if accepted {
                "Write final plan"
            } else {
                "Use the plan as it is"
            })
        }
        _ => None,
    }
}

/// The pull request `feature` opened for repository `repo`.
pub(super) fn pr_of<'a>(feature: &'a Feature, repo: &str) -> Option<&'a Pr> {
    feature.shipping.prs.iter().find(|pr| pr.repo == repo)
}

/// Whether repository `i` of `feature` shows a button taking it out: not
/// when it can't go whatever it holds, or a pull request has it. One with
/// commits shows it too, and says why not when pressed: reading them takes
/// git.
pub fn repo_removable(feature: &Feature, i: usize) -> bool {
    feature.repos.get(i).is_some_and(|repo| {
        linkable(feature)
            && pr_of(feature, &repo.key()).is_none()
            && layout::removal_refusal(feature, &[repo.key()]).is_none()
    })
}

/// What repository `i`'s button does: touch one the feature doesn't
/// change, open the pull request of one that has one.
pub fn repo_button(feature: &Feature, i: usize) -> Option<PanelOutcome> {
    let repo = feature.repos.get(i)?;
    if !repo.touched {
        return Some(PanelOutcome::Touch(repo.key()));
    }
    pr_of(feature, &repo.key()).map(|pr| PanelOutcome::OpenPr(pr.url.clone()))
}

/// The end of a pull request's address, as the panel shows it:
/// `api/pull/12` of `https://github.com/acme/api/pull/12`.
pub(super) fn pr_tail(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').take(3).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// What happened while `feature` was made, for Ship's page: the big
/// numbers, then a line per stage that ran, as its records keep it.
pub(super) fn recap_card(feature: &Feature, state: &PanelState, now: u64) -> chrome::RecapCard {
    let probe = state.probe;
    let end = (feature.marks.get(&Stage::Ship)).map_or(now, |m| m.at);
    let mut figures = vec![(
        span_words(end.saturating_sub(feature.created)),
        match feature.status {
            Status::Open => "so far".to_owned(),
            _ => "start to ship".to_owned(),
        },
    )];
    if let Some(Ok(reading)) = probe.map(|p| &p.reading) {
        let commits: usize = reading.commits.values().sum();
        if commits > 0 {
            let word = if commits == 1 { "commit" } else { "commits" };
            figures.push((commits.to_string(), word.to_owned()));
        }
    }
    let stats: Vec<&feature::page::Stat> = probe.map_or(Vec::new(), |p| p.stats.values().collect());
    if !stats.is_empty() {
        let (added, removed, files) = stats.iter().fold((0, 0, 0), |(a, r, f), s| {
            (a + s.added, r + s.removed, f + s.files)
        });
        figures.push((format!("+{added} \u{2212}{removed}"), "lines".to_owned()));
        figures.push((
            files.to_string(),
            if files == 1 { "file" } else { "files" }.to_owned(),
        ));
    }
    let touched = feature.touched().count();
    if touched > 1 {
        figures.push((touched.to_string(), "repositories".to_owned()));
    }
    // Every stage's runs, those a start over forgot too.
    let runs: u32 = (Stage::ALL.into_iter())
        .map(|stage| feature.next_run(stage) - 1)
        .sum();
    if runs > 0 {
        let word = if runs == 1 { "agent run" } else { "agent runs" };
        figures.push((runs.to_string(), word.to_owned()));
    }
    figures.push((
        feature.fix_rounds.to_string(),
        if feature.fix_rounds == 1 {
            "fix round"
        } else {
            "fix rounds"
        }
        .to_owned(),
    ));
    let mut stages = Vec::new();
    let mut since = feature.created;
    // Those it went through: a review's before Verify were marked, not run.
    for stage in (feature.stages().iter().copied()).filter(|s| *s != Stage::Ship) {
        let Some(mark) = feature.marks.get(&stage) else {
            continue;
        };
        let mut said = Vec::new();
        if let Some(run) = feature.runs.get(&stage) {
            // As the rail and its tab name it.
            let mut who = run.agent.clone();
            if let Some(model) = run.model.as_deref() {
                who = format!("{who} {}", short_model(model));
            }
            said.push(who);
            if run.n > 1 {
                said.push(format!("{} runs", run.n));
            }
        } else if mark.by != "gerdr" {
            said.push(format!("by {}", mark.by));
        }
        said.extend(recap_stage_words(feature, stage, state));
        said.push(format!(
            "took {}",
            span_words(mark.at.saturating_sub(since))
        ));
        since = since.max(mark.at);
        stages.push((stage.label().to_owned(), said.join(" \u{b7} ")));
    }
    // A review's text is the user's, under Submit: no summary of an agent's.
    let (summary, summary_note) = match feature.pull() {
        Some(_) => (Vec::new(), None),
        None => recap_summary(feature, probe),
    };
    chrome::RecapCard {
        summary,
        summary_note,
        figures,
        stages,
        lines: Vec::new(),
        buttons: Vec::new(),
    }
}

/// The summary's paragraphs and what to say under them: who wrote it, or
/// that it is being written, or that there is none.
pub(super) fn recap_summary(
    feature: &Feature,
    probe: Option<&Probe>,
) -> (Vec<String>, Option<String>) {
    let run = feature.shipping.summary.as_ref();
    let who = run.map_or("an agent".to_owned(), |r| r.agent.clone());
    let read = probe.and_then(|p| p.docs.summary.as_ref());
    match read {
        Some((Ok(text), at)) if !text.trim().is_empty() => {
            let paragraphs = (text.split("\n\n"))
                .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|p| !p.is_empty())
                .collect();
            // Asked again since: the new one is on its way.
            let note = match run.filter(|r| r.at > *at) {
                Some(_) => format!("{who} is writing it again\u{2026}"),
                None => "Goes in the pull requests and the squash's message".to_owned(),
            };
            (paragraphs, Some(note))
        }
        Some((Err(why), _)) => (
            Vec::new(),
            Some(format!("Couldn't read the summary: {why}")),
        ),
        _ => match run {
            Some(_) => (
                Vec::new(),
                Some(format!("{who} is writing a summary\u{2026}")),
            ),
            None => (Vec::new(), Some("No summary yet".to_owned())),
        },
    }
}

/// How `stage` of `feature` went, in a few words each, for its recap line.
fn recap_stage_words(feature: &Feature, stage: Stage, state: &PanelState) -> Vec<String> {
    use feature::{plan_review::State as Taken, review_import::FindingState};
    let drafts = |stage| feature.drafts.iter().filter(|d| d.stage == stage).count();
    match stage {
        Stage::Discover | Stage::Plan if drafts(stage) > 1 => {
            vec![format!("{} drafts", drafts(stage))]
        }
        Stage::PlanReview => (feature.plan_review.as_ref())
            .map(|review| {
                let all = review.suggestions.len();
                let taken = (review.suggestions.iter())
                    .filter(|s| s.state == Taken::Accepted)
                    .count();
                vec![
                    count_of(all, "suggestion", "suggestions"),
                    format!("{taken} taken"),
                ]
            })
            .unwrap_or_default(),
        Stage::Verify => (feature.verify.as_ref())
            .map(|run| {
                let passed = (run.checks.iter())
                    .filter(|c| c.outcome.as_ref().is_some_and(|o| o.passed()))
                    .count();
                let mut out = vec![format!("{passed} of {} checks passed", run.checks.len())];
                if run.n > 1 {
                    out.push(format!("{} runs", run.n));
                }
                out
            })
            .unwrap_or_default(),
        Stage::AiReview => (feature.review.as_ref())
            .map(|review| {
                let of = |s: FindingState| review.findings.iter().filter(|f| f.state == s).count();
                let mut out = vec![count_of(review.findings.len(), "finding", "findings")];
                let fixed = of(FindingState::Fixed) + of(FindingState::Sent);
                if fixed > 0 {
                    out.push(format!("{fixed} fixed"));
                }
                if of(FindingState::Dismissed) > 0 {
                    out.push(format!("{} dismissed", of(FindingState::Dismissed)));
                }
                out
            })
            .unwrap_or_default(),
        Stage::CodeReview => {
            let rounds = feature.comment_fixes.len();
            match rounds {
                0 => vec!["no comments".to_owned()],
                n => vec![count_of(n, "round of comments", "rounds of comments")],
            }
        }
        // Its general comments are no criteria: as its page counts them.
        Stage::ManualTest if state.ctx.criteria > 0 => {
            vec![crate::manual_test::counts_text(state.ctx.manual)]
        }
        _ => Vec::new(),
    }
}

/// Whether the patch watch keeps an eye on `feature`: open, with a mark
/// bound to its code or a squash under way.
pub(super) fn watched(feature: &Feature) -> bool {
    let open = feature.status == Status::Open
        && (feature.marks.keys().any(|s| s.bound()) || feature.shipping.squash_at.is_some());
    // A squash of a shipped one's is waited on as an open one's.
    open || (feature.ships_more() && feature.squash_going())
}

/// How often the patch watch probes watched `feature` while the panel
/// shows another: as often as the one shown while its squash is under
/// way, as the user waits on it in its tab (and the panel closed to show
/// that tab), else every `WATCH_EVERY`.
pub(super) fn watch_every(feature: &Feature) -> Duration {
    // An agent merging its base in moves it on: the check follows.
    if feature.squash_going() || feature.shipping.resolving.is_some() {
        PANEL_EVERY
    } else {
        WATCH_EVERY
    }
}

/// The label of what removes `feature`'s worktrees: abandoning it while
/// open, removing them once done.
pub fn remove_label(feature: &Feature) -> &'static str {
    if feature.writer.is_some() && feature.status != Status::Open {
        ""
    } else if feature.status == Status::Done {
        match feature.touched().count() > 1 {
            true => "Remove worktrees and branches",
            false => "Remove worktree and branch",
        }
    } else {
        "Abandon"
    }
}

/// The features the panel lists, of a project's `all`: the open ones, and
/// the done ones whose worktrees are still there to remove.
pub fn listed(all: &[Feature]) -> Vec<&Feature> {
    all.iter()
        .filter(|f| match f.status {
            Status::Open => true,
            Status::Done => f.dir.exists(),
            Status::Abandoned => false,
        })
        .collect()
}

/// The features the sidebar shows, of a project's `all`: the open ones.
pub fn sidebar_features(all: &[Feature]) -> Vec<&Feature> {
    all.iter().filter(|f| f.status == Status::Open).collect()
}

/// The open features of other projects in `all` (by home project id)
/// that span project `project` too, with their home's id.
pub(super) fn linked_in<'a>(
    all: &'a BTreeMap<String, Vec<Feature>>,
    project: &str,
) -> Vec<(&'a str, &'a Feature)> {
    (all.iter())
        .filter(|(home, _)| home.as_str() != project)
        .flat_map(|(home, all)| {
            sidebar_features(all)
                .into_iter()
                .filter(|f| f.link(project).is_some())
                .map(move |f| (home.as_str(), f))
        })
        .collect()
}

/// A slug for a feature called `title`, unique among `all`.
pub fn new_slug(title: &str, all: &[Feature]) -> String {
    let taken: Vec<&str> = all.iter().map(|f| f.slug.as_str()).collect();
    projects::slugify(title, &taken)
}
