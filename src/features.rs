//! Features in gerdr's window: the new feature dialog, the feature tab,
//! and the work they start off the UI thread (see `feature` for the record
//! and where it lives).
//!
//! The features of every project are held here, loaded at start and when
//! the projects' file is read again, and saved after every change.
//!
//! The new feature dialog: a title, where the feature goes (found off the
//! thread), and over several repositories which ones it changes. Enter
//! creates it, Tab and Shift+Tab (Up/Down) move between the title and the
//! repositories, Space ticks one, Esc clears the title, then closes.
//!
//! The feature tab: the pinned first tab of the feature's workspace (see
//! `feature_workspace`), full size, painted from `feature_tab_view` (its
//! state is `FeaturePanel`). It can't be closed, moved or renamed; closing
//! the workspace takes it along. A header with the feature's buttons, the
//! nine stages as a rail, and under it the repositories', the picked
//! stage's page's and the activity's columns, each scrolled on its own
//! (on a narrow tab the repositories are a row of chips and the activity
//! goes at the page's end). Tab moves the keyboard between the rail and
//! the repositories; the arrows (Ctrl+J/K), Home and End pick a stage's
//! page or a repository, which scrolls into view. Enter does the page's
//! next thing (start or show the stage's agent, send it its prompt once its
//! model is picked, review what it wrote, let Build go on after a milestone,
//! run Verify's checks, mark it done, ship) or the
//! repository's (unfold the rows, press a row's button); Ctrl+PgUp/Ctrl+PgDn
//! go to the previous or next feature, Ctrl+O opens a tab in the feature's folder,
//! Ctrl+Shift+O opens the pull request of the repository picked, or on
//! Verify's page its evidence, or a fix plan already sent, Ctrl+R starts the stage's agent afresh,
//! Ctrl+Shift+R shows its agent, or goes back to its chat if its tab closed,
//! Ctrl+Shift+U sets its worktrees up (Build and Verify: copies what they lack),
//! Ctrl+Shift+I shows what the setup runs and printed,
//! on Manual test Ctrl+Shift+T asks which commands its sandbox runs and starts
//! it, or stops it (see `feature_sandbox`), Ctrl+Shift+Y opens its app, Alt+Y
//! its next page (see `feature::sandbox_pages`), Ctrl+Shift+G restarts it and
//! Ctrl+Shift+X resets its data or removes it; a feature another machine writes
//! has only its app, and Ctrl+Shift+T shows its terminals (see `machine_sandbox`),
//! Ctrl+C copies its path, Ctrl+N starts a new one, Ctrl+Shift+L unfolds the
//! repository strip into rows of every repository it can hold (Esc folds them
//! first), Del on a repository of several takes it out (asking when it is on
//! the feature's branch, else pressed twice), Del elsewhere abandons the
//! feature (or, once done, removes its worktrees) after asking, and F1 lists
//! every key in place of the page. A done stage's Enter does nothing: Reopen is
//! only clicked. Each feature's panel is kept as it was left.
//!
//! The marks from Build on are bound to the code (see `feature::patch`):
//! marking one reads the feature's patch-id off the thread first, and the
//! patch watch probes the feature shown every few seconds, and every open
//! feature with a bound mark or a squash under way every half minute (see
//! `feature::ship::probe`). Code changed since sends it back to Build, a
//! squash merged makes it done.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point},
};

mod access;
mod cards;
mod dialog;
mod gerdr_dialog;
mod gerdr_panel;
mod gerdr_ship;
mod gerdr_state;
mod gerdr_tab;
mod gerdr_watch;
mod keys;
mod page_actions;
mod page_view;
mod panel;
#[cfg(test)]
mod pr_page_tests;
mod review_page;
mod rows;
mod ship_page;
mod status_lines;
#[cfg(test)]
pub(crate) mod tests;
mod texts;

pub(crate) use self::{cards::*, page_actions::*, page_view::*, rows::*, texts::*};
use self::{ship_page::*, status_lines::*};

use keys::{AgentChoices, keys_page, ship_keys, submit_key, tab_hints};

use review_page::{
    is_review_stage, review_kept, review_page_actions, review_page_sections, want_all_outcome,
};

use crate::feature::manual::Verdict;
use crate::machine_work::takes_a_place;
use crate::manual_test::Row;
pub(crate) use crate::review_cards::{
    CardId, CardKey, CardPage, Item, ItemAct, ReviewCard, SUGGESTIONS_WRITING, can_decide,
    card_key, card_tag, item_buttons, item_card, item_hints, items, items_count, next_open_card,
    note_actions, offers_fix_all, outcome_note, review_cards, why_not_decide,
};
use crate::{
    Gerdr,
    agent_icons::AgentIcon,
    ai_work::{self, observe::Follow},
    chrome::{
        self, ActivityRow, AgentChip, ButtonStyle, CardStripe, CheckLine, CheckTone, CommandsTab,
        Dot, FeatureRepoRow, FeatureTabHit, FeatureTabView, FindingCard, FindingTag,
        MAX_CHECK_LINES, NewFeatureHit, NewFeatureRepo, NewFeatureView, NodeMark, PageButton,
        PageSection, RailBack, RailNode, RepoKind, RepoNote, StageMark, StagePage, TabArea,
        TabColumn,
    },
    doc_review::DocFile,
    feature::{
        self, Event, Feature, Pr, Repo, Route, ShipStep, Stage, Status,
        agents::needs,
        fix_round::{FixPhase, FixRound},
        graph,
        hook::Guard,
        layout::{self, AddSpec, Created, Found, LinkSpec, Root},
        patch::{self, Reading},
        review_import::{
            self, CodeRead, Decision, Finding as ReviewFinding, FindingState, ReviewRound,
        },
        review_items::{ItemOf, words, words_in},
        ship::{self, Action, Planned, Probe},
        store,
        verify::PlannedCheck,
    },
    feature_checks::{check_passed, checks_running},
    feature_graph::stop_graphs,
    herdr::AgentStatus,
    home_dir,
    menu::MenuAction,
    paths, projects,
    review::Comment,
    scroll::{Bounds, Motion, Scroll},
    tasks::Task,
    text_field::{Edit, TextField},
    toast::Level,
    tooltip::Tooltip,
};

/// The features of every project, and the dialog and panel on them.
#[derive(Default)]
pub struct Features {
    /// By project id, oldest first.
    pub all: BTreeMap<String, Vec<Feature>>,
    /// The features other machines write that are shown here, the same way:
    /// apart from `all`, so what walks those never acts on these (see
    /// `machine_views`).
    pub(crate) viewed: BTreeMap<String, Vec<Feature>>,
    /// The panes their records name, as their writers say (see `feature::view`).
    pub(crate) seen: BTreeMap<String, feature::view::SeenPane>,
    /// What was last said of this gerdr's own panes, and when.
    pub(crate) live_last: Option<(feature::view::Live, u64)>,
    /// How the code of those shown here stands, by project and slug.
    pub(crate) code_here: BTreeMap<(String, String), crate::machine_views::CodeHere>,
    /// The features of other machines already said not to show, by machine and
    /// slug: their names are taken here.
    pub(crate) clashes: BTreeSet<(String, String)>,
    /// Those being taken over from another machine, by project and slug.
    pub(crate) taking: BTreeMap<(String, String), crate::machine_lease::Taking>,
    /// What was pressed on one, while the user is asked whether to take it over.
    pub(crate) take_asked: BTreeMap<(String, String), Option<PanelOutcome>>,
    /// This gerdr's features another machine was told it can have, until it has.
    pub(crate) handing: BTreeMap<(String, String), crate::machine_lease::Handing>,
    /// Sandboxes running here for features another machine writes now, by
    /// project and slug: this gerdr's tabs run their commands (`lease::KEPT`).
    pub(crate) kept: BTreeMap<(String, String), feature::lease::Kept>,
    /// Features this gerdr takes back because the machine writing them handed
    /// them back (`lease::HAND_BACK`): taken, they have nobody to hand back to.
    pub(crate) taking_back: std::collections::BTreeSet<(String, String)>,
    /// When each machine was last told its record lost, by machine, project, slug.
    pub(crate) claims: HashMap<(String, String, String), Instant>,
    /// Cursor's models, for the picker of a stage's model.
    pub cursor_models: crate::stage_models::CursorModels,
    pub dialog: Option<NewFeatureDialog>,
    /// The project commands dialog, while it is open.
    pub commands: Option<crate::project_commands::CommandsDialog>,
    /// What was typed in it and not saved, for its next opening.
    pub commands_drafts:
        BTreeMap<crate::project_commands::CommandsScope, crate::project_commands::CommandsDraft>,
    /// The dialog asking for a finding's text (see `finding_dialog`).
    pub finding_dialog: Option<crate::finding_dialog::FindingDialog>,
    /// What was typed in it and left, for its next opening on the same.
    pub finding_drafts: crate::finding_dialog::Drafts,
    pub panel: Option<FeaturePanel>,
    /// The feature last shown in the panel, by project: the features key
    /// opens it again.
    shown: BTreeMap<String, String>,
    /// The panels of the feature tabs not shown, by project and slug: a
    /// feature's tab shows again as it was left.
    parked: BTreeMap<(String, String), FeaturePanel>,
    /// What runs off the thread now, for the tasks panel.
    busy: Vec<Busy>,
    /// When each of `busy`'s kinds of work on a feature started, for the
    /// feature tab's strip of what runs.
    since: HashMap<(String, String, Job), Instant>,
    /// The latest probe of each feature, by project and slug.
    probes: BTreeMap<(String, String), Probed>,
    /// The features whose summary was started on its own this run: once,
    /// whatever came of it.
    pub(crate) summary_tried: BTreeSet<(String, String)>,
    /// Features whose Ship was pressed before their code was read: it asks once
    /// the read is back.
    ship_waits: BTreeSet<(String, String)>,
    /// The feature whose workspace shipping closed just now, for the toast
    /// saying it shipped to offer it back (see `Gerdr::show_shipped`).
    pub(crate) just_closed: Option<(String, String)>,
    /// The feature folders whose docs link was seen to this run:
    /// `link_feature_docs` goes once for each.
    pub(crate) docs_linked: BTreeSet<PathBuf>,
    /// The project roots `layout::free_for_cursor` went over this run.
    pub(crate) cursor_freed: BTreeSet<PathBuf>,
    /// The code graph builds running, by project and slug, to stop.
    pub(crate) graphs: BTreeMap<(String, String), Arc<graph::Run>>,
    /// The features whose repositories changed under a code graph build:
    /// another follows it.
    pub(crate) graphs_again: BTreeSet<(String, String)>,
    /// Whether the user was told graphify is missing, this run.
    pub(crate) graphify_told: bool,
    /// What the Ship sheet open or last open asked to do, by project and
    /// slug: what shipping does once confirmed.
    asked: BTreeMap<(String, String), Asked>,
    /// Whether the patch watch's timer runs.
    watching: bool,
    /// The serial of the last dialog opened: a root found for an older one
    /// is dropped.
    pub(crate) serial: u64,
    /// A feature of another project to show in the panel once that
    /// project is entered, by project and slug (see `open_linked_feature`).
    pending: Option<(String, String)>,
    done: Option<channel::Sender<Done>>,
    /// The stage agents being watched (see `feature_agents`).
    pub(crate) agents: BTreeMap<crate::feature_agents::WatchKey, crate::feature_agents::Watch>,
    /// What Build and Verify keep while gerdr runs (see `feature_build`).
    #[allow(dead_code)] // read by `feature_build` (inc 4 contract)
    pub(crate) build: crate::feature_build::BuildState,
    /// The acceptance criteria of each open feature's brief, by project
    /// and slug, as last read: none kept for a brief without.
    pub(crate) criteria: BTreeMap<(String, String), Vec<feature::manual::Criterion>>,
    /// The last lines of the shown feature's current stage agent's pane,
    /// while its tab shows and the pane is there.
    tail: Option<AgentTail>,
    /// The last lines of the panes of the Verify checks running on the page
    /// shown, by pane.
    check_tails: HashMap<String, AgentTail>,
    /// Whether the tail's timer runs: only while a feature's tab shows.
    tailing: bool,
}

/// The last lines of a stage agent's pane (see `page::tail`).
#[derive(Debug, Default)]
struct AgentTail {
    pane: String,
    lines: Vec<String>,
    /// A read runs now: only one at a time.
    reading: bool,
    /// The read error last logged: a new one only goes to the log.
    logged: Option<String>,
}

impl AgentTail {
    /// A read of its pane `pane` is back. On an error what was read before
    /// stays; the card's status still says how it runs.
    fn read(&mut self, pane: &str, lines: Result<Vec<String>, String>) {
        self.reading = false;
        match lines {
            Ok(lines) => {
                self.lines = lines;
                self.logged = None;
            }
            Err(why) if self.logged.as_ref() != Some(&why) => {
                tracing::warn!("reading the feature's pane {pane}: {why}");
                self.logged = Some(why);
            }
            Err(_) => {}
        }
    }
}

/// How often the patch watch looks: the feature shown, and one whose
/// squash is under way, are probed this often, the others every
/// `WATCH_EVERY`.
const PANEL_EVERY: Duration = Duration::from_secs(5);
const WATCH_EVERY: Duration = Duration::from_secs(30);
/// How far a notch of the wheel moves a column of the feature tab, in
/// logical pixels: as far as the sidebar's.
const TAB_NOTCH: f64 = 48.0;
/// How much of the page column PgUp / PgDn keep in view, in logical
/// pixels: a line or two of what showed.
const PAGE_KEEP: i32 = 48;

/// A feature's probes (see `ship::probe`).
#[derive(Debug, Default)]
struct Probed {
    /// The latest one that came back.
    last: Option<Probe>,
    /// When the latest one started.
    at: Option<Instant>,
    /// One runs now: only one at a time per feature.
    running: bool,
    /// Probe again once the one running is back: what it reads may be
    /// from before a change.
    again: bool,
    /// The reading error last logged: a new one only goes to the log.
    logged: Option<String>,
    /// When its pull requests were last asked of GitHub (`ship::read_prs`).
    prs_at: Option<Instant>,
    /// When its bases were last fetched from origin (`ship::read_bases`).
    fetch_at: Option<Instant>,
}

/// How often a feature's bases are fetched from origin while its page shows.
const FETCH_EVERY: Duration = Duration::from_secs(300);

/// How often a feature's pull requests are asked of GitHub while its page
/// shows.
const PRS_EVERY: Duration = Duration::from_secs(60);

/// What the Ship sheet asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    actions: Vec<Action>,
    /// The patch-id the check was on, for Ship's mark.
    patch: Option<String>,
}

/// What a feature's work off the thread is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Job {
    Create,
    /// Linking projects to an open feature.
    Link,
    Touch,
    Losses,
    Remove,
    /// Taking repositories out of an open feature: what they lose first,
    /// then their worktrees.
    RemoveRepos,
    /// Reading the patch-id to mark a stage done.
    Read,
    Ship,
    /// Setting up its worktrees (see `feature::setup`).
    Setup,
    /// Running Verify's checks (see `feature::verify`).
    Verify,
    /// Building its code graphs (see `feature::graph`): holds nothing up.
    Graph,
    /// Starting or stopping its sandbox (see `feature::sandbox_run`).
    Sandbox,
    /// Asking the machine it was taken over from to take it back
    /// (see `machine_handback`).
    HandBack,
    /// Moving a pull request's review to its new head (see `pr_watch`).
    PrHead,
}

/// Work on a feature running off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Busy {
    pub(crate) project: String,
    /// The feature's slug; for one being created, the slug it gets.
    pub(crate) slug: String,
    pub(crate) job: Job,
    /// The repository being touched, if that is the work.
    pub(crate) repo: Option<String>,
    /// What it does, for the tasks panel: "Creating “Bulk export”".
    pub(crate) what: String,
}

/// What the work off the thread came to.
pub(crate) enum Done {
    /// How the pull requests under review stand on GitHub (see `pr_watch`).
    PrRead {
        reads: Vec<(String, String, String, Result<feature::pr::Remote, String>)>,
    },
    /// A pull request's review was submitted to GitHub, or not.
    ReviewPosted {
        project: String,
        slug: String,
        results: Vec<(u64, Result<Box<crate::pr_post::Submitted>, String>)>,
    },
    /// Whether the user can push to each pull request's branch was read:
    /// by number, its repository, or why not.
    PushAccess {
        project: String,
        slug: String,
        results: Vec<(String, u64, feature::pr::Access)>,
    },
    /// The user's own pull request's commits were pushed, or not: the head.
    PrPushed {
        project: String,
        slug: String,
        results: Vec<(String, u64, Result<String, String>)>,
    },
    /// A pull request's failed runs were asked to run again on GitHub:
    /// how many went, and why the others didn't.
    CiRerun {
        project: String,
        slug: String,
        number: String,
        ran: usize,
        failed: Vec<String>,
    },
    /// A pull request's review moved to its new head, or not.
    PrMoved {
        project: String,
        slug: String,
        results: Vec<(String, Result<feature::layout::MovedPr, String>)>,
    },
    /// Pull requests were added to a review, or not; `drops` go next.
    ReviewAdded {
        project: String,
        slug: String,
        drops: Vec<String>,
        result: Result<Box<crate::pr_review::Added>, String>,
    },
    /// The open pull requests of a repository of the review dialog of
    /// `serial` were listed, or not.
    OpenPulls {
        serial: u64,
        repo: RepoKey,
        result: Result<Vec<feature::pr::OpenPull>, String>,
    },
    /// A pull request to review is of a project of several repositories:
    /// the dialog picks the others with it.
    ReviewPick {
        project: String,
        slug: String,
        from: PathBuf,
        wanted: (String, String, u64),
    },
    /// A pull request's review was made, or not (see `pr_review`).
    PrReview {
        project: String,
        slug: String,
        numbers: String,
        result: Result<Box<(Feature, Vec<PathBuf>)>, String>,
    },
    /// Looking for the open project's repositories on machine `id` ended.
    SameRepos {
        id: String,
        project: String,
        result: Result<crate::machine_repos::Paired, String>,
    },
    /// Asking the machine a feature came from to take it back ended.
    HandBack {
        project: String,
        slug: String,
        result: Result<(), String>,
    },
    /// What was asked of a sandbox another machine's gerdr writes of ended.
    SandboxAsked {
        project: String,
        slug: String,
        result: Result<crate::machine_sandbox::StopAfar, String>,
    },
    /// What a sandbox needs here was looked for before asking for its
    /// commands: what is missing, in words.
    SandboxTools {
        project: String,
        slug: String,
        missing: Option<String>,
    },
    /// Starting a feature's sandbox ended; `stopped`: because it was asked to.
    Sandbox {
        project: String,
        slug: String,
        run: feature::sandbox_run::SandboxRun,
        /// It opened its terminals on the machine it runs on: those it couldn't.
        opened: Option<Vec<String>>,
        stopped: bool,
    },
    /// Stopping a feature's sandbox ended.
    SandboxStopped {
        project: String,
        slug: String,
        what: crate::feature_sandbox::StopFor,
        result: Result<(), String>,
    },
    /// What a removed feature left on a machine went, or still can't.
    SandboxLeftGone {
        file: PathBuf,
        title: String,
        name: String,
        result: Result<(), String>,
    },
    /// The compose projects of sandboxes whose feature is gone.
    SandboxLeftovers {
        /// Where they were looked for: none, here.
        machine: Option<String>,
        stacks: Vec<crate::feature_sandbox::LeftStack>,
        /// Containers a copy cut short left paused that wouldn't unpause.
        still_paused: Vec<String>,
    },
    /// Removing those ended.
    SandboxLeftoversRemoved {
        removed: Vec<crate::feature_sandbox::LeftStack>,
        problems: Vec<String>,
    },
    /// `t3 thread send` to `thread` ended.
    T3Told {
        thread: String,
        result: Result<(), String>,
    },
    /// The repositories of `project` for the commands dialog of `serial`,
    /// with what their files suggest.
    CommandsFound {
        serial: u64,
        project: String,
        result: Result<Vec<(String, feature::suggest::Suggested)>, String>,
    },
    Found {
        serial: u64,
        /// The main worktree of the repository it was opened from, if any.
        from: Option<PathBuf>,
        result: Result<Root, String>,
    },
    /// The root of `project`, another project the dialog can link.
    /// A root the feature panel looks in for repositories to offer (see
    /// `Gerdr::find_repo_sources`).
    FoundRepos {
        project: String,
        slug: String,
        /// The project it is of; `None`: the feature's home project.
        source: Option<String>,
        result: Result<Root, String>,
    },
    FoundOther {
        serial: u64,
        project: String,
        result: Result<Root, String>,
    },
    /// The branches a repository of the dialog of `serial` can start from.
    BaseBranches {
        serial: u64,
        repo: RepoKey,
        result: Result<Vec<String>, String>,
    },
    /// The branch origin says is that repository's default.
    RemoteBase {
        serial: u64,
        repo: RepoKey,
        result: Result<Option<String>, String>,
    },
    Created {
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<(Created, Vec<(feature::Link, Created)>), String>,
    },
    /// A feature's folder linked to its stage files, and the exclude file
    /// that changed for it, or why not.
    DocsLinked {
        title: String,
        result: Result<Option<PathBuf>, String>,
    },
    /// `.features` was taken out of `.cursorignore` in `root` if it was
    /// there, and whether it was, or why not.
    CursorFreed {
        root: PathBuf,
        result: Result<bool, String>,
    },
    /// A build of feature `slug`'s code graphs ended.
    Graphed {
        project: String,
        slug: String,
        title: String,
        built: graph::Built,
    },
    /// Projects linked to open feature `slug`, or not.
    Linked {
        project: String,
        slug: String,
        names: Vec<String>,
        result: Result<layout::MoreLinked, String>,
    },
    Touched {
        project: String,
        slug: String,
        repo: String,
        result: Result<feature::Repo, String>,
    },
    Losses {
        project: String,
        slug: String,
        losses: Vec<String>,
    },
    Removed {
        project: String,
        slug: String,
        abandon: bool,
        result: Result<(), String>,
        /// Its worktrees' commits before they went, and its workspace's
        /// tabs: what restoring them takes.
        heads: Vec<feature::KeptHead>,
        tabs: Vec<serde_json::Value>,
    },
    /// Restoring feature `slug`'s worktrees, removed after it shipped, ended.
    WorktreesRestored {
        project: String,
        slug: String,
        result: Result<(), String>,
    },
    /// What taking repositories `keys` out of feature `slug` would lose, or
    /// why they can't go.
    RemovalLosses {
        project: String,
        slug: String,
        keys: Vec<String>,
        result: Result<Vec<String>, String>,
        to_context: bool,
    },
    /// Repositories taken out of feature `slug`, or not: per `Repo::key`.
    ReposRemoved {
        project: String,
        slug: String,
        results: Vec<(String, Result<(), String>)>,
        to_context: bool,
    },
    Probed {
        project: String,
        slug: String,
        probe: Box<Probe>,
    },
    /// The last lines of herdr pane `pane`, the shown feature's current
    /// stage agent's (see `page::read_tail`).
    Tail {
        pane: String,
        lines: Result<Vec<String>, String>,
    },
    /// What the card of the Verify check in herdr pane `pane` previews (see
    /// `page::read_check_tail`).
    CheckTail {
        pane: String,
        lines: Result<Vec<String>, String>,
    },
    /// The patch-id read to mark `stage` done.
    Read {
        project: String,
        slug: String,
        stage: Stage,
        result: Result<Reading, String>,
    },
    /// The code read once run `n` of the Plan agent was done, its file
    /// at `stamp`: whether the agent kept to reading.
    PlanChecked {
        project: String,
        slug: String,
        n: u32,
        stamp: crate::feature_agents::Stamp,
        result: Result<Reading, String>,
    },
    /// The code and `review.json` read once run `n` of the AI review
    /// agent was done, its file at `stamp` (see `feature_review`).
    PlanReviewBaseline {
        project: String,
        slug: String,
        /// The code's patch-id as a plan review's agent starts.
        result: Result<String, String>,
    },
    /// The code and what the agent wrote, once run `n` of Plan review's
    /// agent rested, its file at `stamp` (see `feature_plan_review`).
    PlanReviewRead {
        project: String,
        slug: String,
        n: u32,
        stamp: crate::feature_agents::Stamp,
        result: Result<Box<crate::feature_plan_review::PlanRead>, String>,
    },
    ReviewRead {
        project: String,
        slug: String,
        n: u32,
        stamp: crate::feature_agents::Stamp,
        result: Result<Box<CodeRead>, String>,
    },
    Shipped {
        project: String,
        slug: String,
        /// The patch-id the Ship check was on.
        patch: Option<String>,
        /// The squash steps, to start in a tab once the code is found
        /// unchanged: then done once merged, not now.
        squash: Vec<Action>,
        /// The repositories whose branch the steps push.
        pushed: Vec<String>,
        /// Each step that ran, however it ended.
        ran: Vec<feature::ShipDone>,
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
    },
    /// Taking a base into a feature's branch ended.
    BaseUpdated {
        project: String,
        slug: String,
        result: Box<Result<(feature::BaseUpdate, Option<String>), String>>,
    },
    /// A repository's base moved on to origin's, by that many commits, or
    /// didn't (see `ship::pull_base`).
    BasePulled {
        project: String,
        slug: String,
        repo: String,
        result: Result<usize, String>,
    },
    /// The merge for an agent to resolve began, or the plan of it is to be
    /// written; or why not.
    ConflictsBegun {
        project: String,
        slug: String,
        repo: String,
        planning: bool,
        result: Result<ship::MergeBegun, String>,
    },
    /// The merge an agent resolved was stopped, or wasn't.
    ResolvingStopped {
        project: String,
        slug: String,
        result: Result<(), String>,
    },
    /// Undoing steps shipping ran ended: those undone, and why it stopped.
    ShipUndone {
        project: String,
        slug: String,
        undone: Vec<(String, ShipStep)>,
        failed: Option<String>,
    },
    /// The bases a merged feature ticked to push, each by its
    /// `Repo::key`, its name and how it went.
    BasesPushed {
        project: String,
        slug: String,
        results: Vec<(String, String, Result<String, String>)>,
        /// They are what finishes Ship: what follows it runs after.
        finishing: bool,
    },
    /// What removing a shipped feature's worktrees loses, as ticked.
    ShippedLosses {
        project: String,
        slug: String,
        losses: Vec<String>,
    },
    /// The code read once run `n` of the Build agent was done, its
    /// `build.md` at `stamp`: whether Build holds (see `feature_build`).
    #[allow(dead_code)] // sent from `feature_build` (inc 4 contract)
    BuildChecked {
        project: String,
        slug: String,
        n: u32,
        stamp: crate::feature_agents::Stamp,
        result: Result<Reading, String>,
    },
    /// Work under way says what it does now, for the tasks panel.
    #[allow(dead_code)] // sent from `feature_build` (inc 4 contract)
    Progress {
        project: String,
        slug: String,
        job: Job,
        what: String,
    },
    /// The setup of feature `slug`'s worktrees ended.
    #[allow(dead_code)] // sent from `feature_build` (inc 4 contract)
    SetUp {
        project: String,
        slug: String,
        report: feature::setup::Report,
    },
    /// A setup on another machine has this terminal there.
    SetupPane {
        project: String,
        slug: String,
        pane: String,
    },
    /// The terminals of run `n` of feature `slug`'s checks on another machine
    /// were told to run: each one's qualified pane, or why it has none.
    ChecksOpened {
        project: String,
        slug: String,
        n: u32,
        panes: Vec<(usize, Result<String, String>)>,
    },
    /// The code run `n` of feature `slug`'s checks is to run on was read.
    ChecksBegun {
        project: String,
        slug: String,
        n: u32,
        begun: feature::verify::Begun,
    },
    /// Check `index` of run `n` of feature `slug` ended.
    CheckEnded {
        project: String,
        slug: String,
        n: u32,
        index: usize,
        outcome: feature::verify::Outcome,
    },
    /// Check `index` of run `n` of feature `slug` has to wait, or started.
    CheckTurn {
        project: String,
        slug: String,
        n: u32,
        index: usize,
        turn: feature::verify::Turn,
    },
    /// Verify's checks of feature `slug` ran: run `n`.
    #[allow(dead_code)] // sent from `feature_build` (inc 4 contract)
    Verified {
        project: String,
        slug: String,
        n: u32,
        evidence: feature::verify::Evidence,
    },
    /// The AI work's index was read for the stats panel.
    AiStats {
        scope: crate::ai_work::stats::Scope,
        result: Result<crate::ai_work::stats::View, String>,
    },
    /// What the AI work's thread came to.
    AiWork(crate::ai_work::Report),
}

/// Where a feature would go, as the dialog knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    Looking,
    Found(Root),
    Failed(String),
}

/// A project the feature panel looks for repositories in, as far as it is
/// known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSource {
    /// The project's id; `None`: the feature's home project.
    pub project: Option<String>,
    /// Its name.
    pub name: String,
    pub finding: Finding,
}

/// Another project of the store the new feature dialog can link: the
/// feature then spans its repositories too.
#[derive(Debug)]
pub struct Other {
    pub project: String,
    pub name: String,
    /// Its root, found like the home one's.
    pub root: Finding,
    pub linked: bool,
    /// Per repository of its root: whether the feature holds it.
    pub touched: Vec<bool>,
}

/// A repository of the new feature dialog: the linked project it is of
/// (`None`: the home one), and its name.
pub type RepoKey = (Option<String>, String);

/// What the new feature dialog knows of a repository's branches. The branch
/// a feature would start from is the `Found::base` of the root it holds.
#[derive(Debug, Default)]
pub struct RepoBranches {
    /// The branch the repository's features start from by itself.
    default: String,
    /// Those it can start from, once read.
    branches: Option<Result<Vec<String>, String>>,
    /// The user picked one: what origin says no longer moves it.
    picked: bool,
}

/// The repository whose base branch the new feature dialog is choosing.
#[derive(Debug)]
struct Choosing {
    repo: RepoKey,
    /// The focus and first row to go back to.
    back: (Option<usize>, usize),
}

/// The new feature dialog's state, while it is open.
#[derive(Debug)]
pub struct NewFeatureDialog {
    pub project: String,
    pub serial: u64,
    pub title: TextField,
    pub root: Finding,
    /// Whether the feature holds each repository of the root.
    pub touched: Vec<bool>,
    /// The row with the keyboard; `None`: the title.
    pub focus: Option<usize>,
    /// The first row shown, when not all fit.
    pub top: usize,
    pub error: Option<String>,
    pub hover: Option<NewFeatureHit>,
    /// The folder it was opened from: the repository ticked first.
    pub from: PathBuf,
    /// The store's other projects, once the project picker opened.
    pub others: Vec<Other>,
    /// The rows are the project picker's, not the repositories.
    pub picking: bool,
    /// The repositories' branches, read as each is found or linked.
    bases: BTreeMap<RepoKey, RepoBranches>,
    /// The rows are this repository's branches, not the repositories.
    choosing: Option<Choosing>,
    /// What filters those branches.
    query: TextField,
    /// It picks pull requests to review, not repositories to change.
    pub review: Option<Reviewing>,
    /// Projects to link as soon as the store's others are listed: those
    /// the review it changes spans.
    link_first: Vec<String>,
}

/// The new feature dialog picking pull requests to review together: one per
/// repository, the rest left out.
#[derive(Debug, Default)]
pub struct Reviewing {
    pulls: BTreeMap<RepoKey, RepoPulls>,
    /// The pull request it was opened for (host, slug, number): picked once
    /// its repository's list is in.
    wanted: Option<(String, String, u64)>,
    /// What picking did besides, said where the root is: the same branch
    /// picked in other repositories.
    note: Option<String>,
    /// The open review it changes the pull requests of: its slug and title.
    into: Option<(String, String)>,
}

/// What the review dialog knows of a repository's open pull requests.
#[derive(Debug, Default)]
struct RepoPulls {
    /// Once listed.
    list: Option<Result<Vec<feature::pr::OpenPull>, String>>,
    picked: Option<u64>,
    /// The user picked or cleared it: same-branch suggestions leave it be.
    chosen: bool,
    /// Its list was asked for.
    asked: bool,
    /// The one the review holds, as it has it, until listed.
    known: Option<feature::pr::OpenPull>,
}

impl RepoPulls {
    fn listed(&self) -> &[feature::pr::OpenPull] {
        match &self.list {
            Some(Ok(all)) => all,
            _ => &[],
        }
    }

    fn picked_pull(&self) -> Option<&feature::pr::OpenPull> {
        let n = self.picked?;
        (self.listed().iter().find(|p| p.number == n))
            .or(self.known.as_ref().filter(|p| p.number == n))
    }
}

/// The one pull request of `listed` on branch `head`, if only one is.
fn same_branch<'a>(
    listed: &'a [feature::pr::OpenPull],
    head: &str,
) -> Option<&'a feature::pr::OpenPull> {
    match listed
        .iter()
        .filter(|p| p.head_ref == head)
        .collect::<Vec<_>>()[..]
    {
        [one] => Some(one),
        _ => None,
    }
}

/// What a row of the new feature dialog is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogRow {
    /// The home root's repository `i`.
    Home(usize),
    /// Repository `i` of linked project `k` of `others`.
    Linked(usize, usize),
    /// Project `k` of `others`, in the picker.
    Project(usize),
    /// Branch `i` of `branch_rows`, in the base branch list.
    Branch(usize),
}

/// What the new feature dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Cancel,
    /// Create a feature with this title in the root found, holding these
    /// repositories, spanning these linked projects too.
    Create {
        title: String,
        root: Root,
        touched: Vec<String>,
        links: Vec<LinkSpec>,
    },
    /// Review these pull requests together, of the root found and these
    /// linked projects; titled so, or after the first if empty.
    Review {
        title: String,
        root: Root,
        links: Vec<LinkSpec>,
        picks: Vec<crate::pr_review::Pick>,
        /// The open review whose pull requests become these, by slug.
        into: Option<String>,
    },
}

/// Whether a press at `pos` on a bar button named `label` at `now` is the
/// second of a double click whose first, `last`, was on another button.
fn other_button_again(
    last: &(Instant, Point<f64, Logical>, String),
    now: Instant,
    pos: Point<f64, Logical>,
    label: &str,
) -> bool {
    let (when, from, was) = last;
    let slop = crate::input::MULTI_CLICK_SLOP;
    now.duration_since(*when) < crate::input::MULTI_CLICK
        && (from.x - pos.x).abs() < slop
        && (from.y - pos.y).abs() < slop
        && was != label
}

/// The feature panel's state, while it is open.
#[derive(Debug, Default)]
pub struct FeaturePanel {
    pub project: String,
    pub slug: String,
    /// The feature tab's area with the keyboard.
    pub focus: TabArea,
    /// The stage whose page shows, if the user picked one; none shows
    /// the current stage.
    pub stage: Option<Stage>,
    /// The current stage when `stage` was picked: once the feature moves
    /// on (a tick, a send-back, code changed), the pick is dropped.
    pub picked_at: Option<Stage>,
    /// The repository with the keyboard while `focus` is `Repos`.
    pub repo: usize,
    /// The card with the keyboard while `focus` is `Findings`: a finding, a
    /// check or a criterion, in the page's order.
    pub finding: usize,
    /// The repository (`Repo::key`) a first Del was on: a second takes it
    /// out, any other key lets it go.
    pub armed: Option<String>,
    /// The card a note is being written on, and the note. It stays with the
    /// panel when another tab shows.
    pub note: Option<(CardNote, TextField)>,
    /// Notes Esc put away unsaved, by the card they were on: opening a note
    /// there again goes on with one (Esc never saves, nor throws a note away).
    pub drafts: Vec<(CardNote, TextField)>,
    /// The feature's code review on its Code review page, once that showed
    /// (see `feature_diff::inline_review`): where it stands stays with the
    /// panel.
    pub review: Option<Box<crate::worktree_diff::WorktreeDiff>>,
    /// That page's review is up to date with what shows, read as it came to
    /// show: another diff or panel over it, or another page, and it isn't.
    pub review_shown: bool,
    /// Where that review goes, and the tab area it was worked out in: the
    /// rail changes height only with what takes the page from it.
    pub review_rect: Option<chrome::Rect>,
    pub review_area: Option<chrome::Rect>,
    /// On that page, the rail has the keys, not the review (Esc there).
    pub rail_keys: bool,
    /// The last press on the page's bar: when, where, and the button's
    /// label (see `bar_press_again`).
    pub bar_press: Option<(Instant, Point<f64, Logical>, String)>,
    /// Each column's scroll, indexed by `TabColumn` (logical pixels).
    pub scroll: [Scroll; 3],
    /// The page the Page column's scroll is for: the stage, and whether
    /// the keys list showed. Another page starts at its top.
    pub page_of: Option<(Stage, bool)>,
    /// Every key of the tab is listed in place of the page (F1).
    pub keys: bool,
    pub tab_hover: Option<FeatureTabHit>,
    /// Where the expanded repositories look (home first): the feature's
    /// project, which they offer the rest of, and those it already links,
    /// for what it holds or held of them. Read off the thread when it opens
    /// and on each expand.
    pub sources: Vec<RepoSource>,
    /// The repositories show as rows, with all they could be, over the page.
    pub expanded: bool,
    /// Where the keyboard goes back to when they collapse.
    pub collapse_to: TabArea,
    /// The earlier AI reviews showing their findings, by run.
    pub rounds_open: BTreeSet<u32>,
    /// The review cards the user opened (true) or folded: they stay so,
    /// selected or not. The rest show as their decision has them.
    pub(crate) cards_open: Vec<(CardId, bool)>,
    /// The fix round cards the user opened or folded, by round: a card shows
    /// open when it is the newest, unless it is here (see `fix_round_open`).
    pub fix_rounds_flipped: BTreeSet<u32>,
    /// What the user picked Code review's page to show; none, what goes by
    /// where the feature stands (`review_view_of`).
    pub review_view: Option<chrome::ReviewView>,
    /// The button Left and Right picked on the selected row (see
    /// `FeaturePanel::picked`).
    pub repo_pick: Option<usize>,
    /// The search through Manual test's criteria, while open.
    pub search: Option<crate::manual_test::Search>,
    /// The text selected on the page with the pointer.
    pub pick: Option<chrome::TextPick>,
    /// The pointer holds a press selecting text on the page.
    pub picking: bool,
    /// What the selection on the page goes by (chars, or the words or
    /// lines a double or triple click took) and the span its press took,
    /// which it keeps whichever way it is dragged or Shift+clicked on.
    pub(crate) pick_by: Option<(
        crate::scrollback::Grain,
        (chrome::TextSpot, chrome::TextSpot),
    )>,
    /// A press selecting the page's text held past its top or bottom:
    /// when the page last went on by it (see `scrolling::edge_steps`).
    pub pick_edge: Option<(std::time::Instant, f64)>,
}

/// What follows shipping, as the Ship page ticks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipAfter {
    /// The workspace closes.
    Close,
    /// The worktrees and branches go.
    Remove,
}

/// A row of the Ship page the keyboard and pointer can reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShipRow {
    /// Repository `repo`'s choice of way, now `route`.
    Route { repo: String, route: Route },
    /// A tick box of repository `repo`.
    Step { repo: String, step: ShipStep },
    /// A tick box of what follows.
    After(ShipAfter),
}

impl ShipRow {
    /// What Space does on it, and a click on a tick box.
    fn outcome(&self) -> PanelOutcome {
        match self {
            Self::Route { repo, route } => PanelOutcome::ShipRoute(repo.clone(), route.next()),
            Self::Step { repo, step } => PanelOutcome::ShipTick(repo.clone(), *step),
            Self::After(after) => PanelOutcome::ShipAfter(*after),
        }
    }
}

/// What the feature panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelOutcome {
    /// Mark the current stage done.
    Tick,
    /// Ship it, after the Ship sheet: the current stage is Ship.
    Ship,
    /// Finish it with nothing shipped, after asking: the current stage is
    /// Ship, and no step is ticked.
    Finish,
    /// Send repository `.0` of the Ship page the way `.1` says.
    ShipRoute(String, Route),
    /// Flip a tick of repository `.0` on the Ship page.
    ShipTick(String, ShipStep),
    /// Flip a tick of what follows shipping.
    ShipAfter(ShipAfter),
    /// Start this agent stage's agent (fresh: even if one runs).
    StartAgent(Stage, bool),
    /// Tell the agent waiting on this stage to read its prompt.
    SendPrompt(Stage),
    /// Show this agent stage's agent.
    ShowAgent(Stage),
    /// Go back to the chat of this agent stage's agent, its tab closed.
    ResumeAgent(Stage),
    /// Show the stage's file in its tab: one already open as it stands, a
    /// new one as `DocMode::opens_on` it.
    OpenDoc(Stage),
    /// Approve `stage`'s file from its page, without opening its tab.
    ApproveDoc(Stage),
    /// Let this agent stage's agent go on after a milestone stop.
    Continue(Stage),
    /// Run Verify's checks (again).
    RunChecks,
    /// Run again those of Verify's checks that didn't pass.
    RunFailed,
    /// Run this one of Verify's checks again.
    RerunCheck(usize),
    /// Stop this one of Verify's checks.
    StopCheck(usize),
    /// Stop all of Verify's checks.
    StopChecks,
    /// Start the sandbox.
    StartSandbox,
    /// Open the sandbox's app: the first of its pages.
    OpenSandbox,
    /// Open this page of the sandbox's app, by its index in
    /// `SandboxRun::pages`.
    OpenSandboxPage(usize),
    /// Open the page of the sandbox's app after the one last opened.
    NextSandboxPage,
    /// Open the sandbox to the network, for a phone, asking; or close it.
    SandboxLan,
    /// Show the terminals of a sandbox on another machine.
    ShowSandbox,
    /// Start the sandbox again on the same data.
    RestartSandbox,
    /// Start the sandbox again on a new copy of main's data, asking.
    ResetSandbox,
    /// Stop the sandbox, asking; at once while it starts.
    StopSandbox,
    /// Remove the stopped sandbox with its data, asking.
    RemoveSandbox,
    /// Show the terminal of this one of Verify's checks.
    ShowCheck(usize),
    /// Skip this check the feature borrows, or stop skipping it.
    SkipCheck {
        repo: String,
        command: String,
        skip: bool,
    },
    /// Set the feature's worktrees up (again): copy what they lack.
    SetUp,
    /// Show what the setup runs and printed.
    SetupLog,
    /// Show the diff at AI review's first finding to decide.
    Findings,
    /// Send AI review's findings to fix to the Build agent, through a plan.
    SendFixes,
    /// The same with no plan: nothing of the user's asks for one.
    MakeFindingFixes,
    /// Mark every undecided finding of its AI review to fix.
    FixAllFindings,
    /// Start a fix round on Verify's failures.
    PlanFixes,
    /// Open the fix plan under review.
    ReviewFixPlan,
    /// The user approved the fix plan: the agent makes the fixes.
    MakeFixes,
    /// The fixes of the last round, over, go once more, unticked
    /// (`Gerdr::resend_fixes`).
    ResendFixes,
    /// Verify's failures go to the Build agent with no plan.
    MakeVerifyFixes,
    /// Show round `n`'s fix plan, from Build's page: the latest in its tab,
    /// an earlier one only to read.
    OpenRoundPlan(u32),
    /// Show what the final plan changed in the plan, only to read.
    PlanChanges,
    /// Show what draft `k` of the stage's file changed from the one before.
    DraftChanges(Stage, u32),
    /// Show what round `n`'s fixes changed, in a diff only to read.
    RoundDiff(u32),
    /// Open fix round `n`'s card on its page, or fold it.
    FoldFixRound(u32),
    /// Tell the agent again to write the fix plan: it stopped without one.
    AskFixPlan,
    /// Drop the open fix plan, after asking.
    DropFixPlan,
    /// Open the project commands dialog on this tab.
    EditCommands(CommandsTab),
    /// Decide this finding of AI review.
    DecideFinding(String, Decision),
    /// Ask why, then dismiss this finding.
    AskDismiss(String),
    /// Ask for this finding's fix and comment.
    EditFinding(String),
    /// Write or edit the user's comment on a finding, on its card.
    CommentFinding(String),
    /// Delete the user's comment on a finding.
    DeleteFindingComment(String),
    /// Delete this item of the user's own.
    DeleteOwnFinding(String),
    /// Ask for what this comment of the user's in the diff should say.
    EditDiffNote(Comment),
    /// Delete this comment of the user's in the diff.
    DeleteDiffNote(Comment),
    /// Show this comment of the user's in the diff.
    ShowDiffNote(Comment),
    /// Ask for a comment of the user's own, to go with the fixes.
    AddComment,
    /// Show this finding in the diff.
    ShowFinding(String),
    /// Say why a key did nothing to a finding.
    FindingRefused(&'static str),
    /// Show this earlier review's findings on its page, or fold them.
    FoldRound(u32),
    /// Show Code review's code, or the fixes it asked for.
    ReviewView(chrome::ReviewView),
    /// Review the feature's diff (Code review).
    CodeReview,
    /// Give the keyboard to Manual test's criteria.
    ManualTest,
    /// What a Manual test page's criteria ask.
    Manual(crate::manual_test::Outcome),
    /// Open Manual test's criterion `.0` to its steps (true), or fold it.
    FoldCriterion(usize, bool),
    /// Open review card `.0` (true), or fold it to its line.
    FoldCard(usize, bool),
    /// Open the criteria `.0` names (true), or fold them.
    FoldCriteria(crate::feature::manual::FoldScope, bool),
    /// Save the note or comment being written on its card.
    NoteSave,
    /// Drop the note or comment being written: the card stays as it was.
    NoteDiscard,
    /// Show Plan review's suggestions in the plan, to decide them there.
    ReviewInPlan,
    /// Do `.1` to Plan review's suggestion `.0`, from its card.
    OnSuggestion(String, ItemAct),
    /// Accept every open suggestion of Plan review's that can be.
    AcceptAllSuggestions,
    /// Send the notes on Plan review's suggestions to its reviewer.
    AskReviewer,
    /// The suggestions are decided: start the final plan, or with nothing
    /// accepted, go on with the plan as it is.
    WriteFinalPlan,
    /// Skip Plan review, asking first when something would be lost.
    SkipPlanReview,
    /// Back from the final plan to the decisions on the suggestions.
    ChangeDecisions,
    /// Show Verify's `evidence.md`.
    Evidence,
    /// Open this pull request's page.
    OpenPr(String),
    /// A pull request's review: move its worktree to the head GitHub has
    /// now (see `pr_watch`).
    UpdatePrHead,
    /// A pull request's review whose pull request ended: finish it, after
    /// asking.
    FinishReview,
    /// A pull request's review: run its failed GitHub Actions runs again on
    /// GitHub (see `pr_watch::rerun_ci`).
    RerunCi,
    /// A pull request's review: submit it to GitHub as this, after asking
    /// (see `pr_post`).
    SubmitReview(feature::pr_submit::Event),
    /// The user's own pull request's review: push its commits, after asking.
    PushPr,
    /// Someone else's pull request's review: fix it here, after asking, or
    /// comment on it again (see `pr_post::toggle_fix_here`).
    FixHere,
    /// Add pull requests to the review, or leave some out (see
    /// `Gerdr::open_review_edit`).
    EditPulls,
    /// Ask an agent to write the feature's summary again.
    WriteSummary,
    /// Undo step `.1` that shipping ran for repository `.0`, after asking.
    UndoShip(String, ShipStep),
    /// Undo what followed shipping: bring back its workspace's tabs, or its
    /// worktrees.
    UndoAfter(feature::AfterStep),
    /// Take repository `.0`'s base into its branch, `.1`, after asking.
    UpdateBase(String, feature::UpdateWay),
    /// Move repository `.0`'s base on to origin's, which its branch took
    /// in already: what its squash waits for.
    PullBase(String),
    /// Close the workspace that shipping left open (see `close_left`).
    CloseShipped,
    /// Have an agent take on repository `.0`'s conflicts with its base:
    /// plan their resolution (`.1`), or resolve them.
    ResolveConflicts(String, bool),
    /// Open the plan of the conflicts' resolution.
    ReviewConflictPlan,
    /// Stop the conflicts an agent takes on, after asking.
    StopResolving,
    /// Open the feature's summary to edit.
    EditSummary,
    /// Reopen this done stage, once it isn't locked (see
    /// `Gerdr::reopen_stage`).
    Untick(Stage),
    /// Show the stage the feature is at, from a done one's page.
    GoToCurrent,
    /// Start this current stage over, after asking (see
    /// `Gerdr::ask_start_over`).
    StartOver(Stage),
    /// A later stage can't be marked yet: this one comes first.
    NotYet(Stage),
    /// Run this agent stage with this agent, from its next start on.
    PickAgent(Stage, String),
    /// Open the picker of the stage's model.
    PickModel(Stage),
    /// Run this agent stage's next agent on the next machine that is up.
    CycleMachine(Stage),
    /// Stop this stage's agent, on another machine.
    StopOnMachine(Stage),
    /// Make a feature another machine writes this machine's, with nothing else to
    /// do. Any other that changes it does that first.
    TakeOver,
    /// Hand a feature written here back to the machine it was taken over from:
    /// that machine's gerdr takes it (see `machine_handback`).
    HandBack,
    /// Pick the next agent this agent stage's chips offer.
    CycleAgent(Stage),
    /// Build with a fresh agent per milestone, or one through them all.
    FreshMilestones(bool),
    /// Make this repository one the feature changes.
    Touch(String),
    /// Take this repository (`Repo::key`) out of the feature.
    RemoveRepo(String),
    /// Make the feature hold this repository, on its branch: one it
    /// doesn't hold now, offered by `FeaturePanel::sources`.
    AddToFeature(String),
    /// Let agents read this repository's main checkout, as context.
    AddContext(String),
    /// Take this repository out of the feature, keeping it as context.
    MakeContext(String),
    /// Stop offering this repository to agents as context.
    DropContext(String),
    /// Open a tab in this folder: the feature's, or a repository's
    /// worktree.
    Open(PathBuf),
    CopyPath,
    /// Show what its AI work came to, in the stats panel.
    Stats,
    /// Abandon it, or once done remove its worktrees, after asking.
    Remove,
    New,
    /// Show the feature with this slug.
    Show(String),
}

impl PanelOutcome {
    /// Whether it acts on a feature's sandbox as it is: that goes on on the machine
    /// that started it, whichever writes the feature.
    pub fn acts_on_sandbox(&self) -> bool {
        matches!(
            self,
            Self::StopSandbox | Self::RestartSandbox | Self::ResetSandbox | Self::RemoveSandbox
        )
    }

    /// Whether it only shows something: all a feature another machine writes can
    /// do here without moving it here. Anything new counts as changing it until
    /// said otherwise.
    pub fn reads_only(&self) -> bool {
        matches!(
            self,
            Self::ShowAgent(_)
                | Self::ShowCheck(_)
                | Self::OpenSandbox
                | Self::OpenSandboxPage(_)
                | Self::NextSandboxPage
                | Self::SandboxLan
                | Self::ShowSandbox
                | Self::OpenDoc(_)
                | Self::ReviewFixPlan
                | Self::Evidence
                | Self::SetupLog
                | Self::Findings
                | Self::ShowFinding(_)
                | Self::ShowDiffNote(_)
                | Self::ReviewInPlan
                | Self::OnSuggestion(_, ItemAct::Show | ItemAct::OpenCode)
                | Self::FoldRound(_)
                | Self::FoldFixRound(_)
                | Self::RoundDiff(_)
                | Self::PlanChanges
                | Self::DraftChanges(..)
                | Self::OpenRoundPlan(_)
                | Self::FoldCriterion(..)
                | Self::FoldCard(..)
                | Self::FoldCriteria(..)
                | Self::NoteDiscard
                | Self::ReviewView(_)
                | Self::CodeReview
                | Self::OpenPr(_)
                | Self::GoToCurrent
                | Self::CopyPath
                | Self::Stats
                | Self::Show(_)
        )
    }
}

/// A row of the feature panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelRow {
    Repo(usize),
    /// A repository row the feature doesn't hold: only its buttons act.
    Other,
    Stage(Stage),
}

/// A row of the feature tab's repositories, which `FeaturePanel::repo`
/// counts in. See `repo_entries`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoEntry {
    /// `Feature::repos` at this index.
    Held(usize),
    /// `Feature::context` at this index.
    Context(usize),
    /// `Feature::removed` at this index.
    Removed(usize),
    /// A repository of the feature's project it has nothing to do with
    /// yet: `repo` in the root of `FeaturePanel::sources[source]`.
    Offered { source: usize, repo: usize },
}

/// How `Gerdr::remove_feature` takes a feature's worktrees away.
#[derive(Clone, Copy, Debug)]
pub struct FeatureRemoval {
    /// Mark the feature abandoned after.
    pub abandon: bool,
    /// Remove them with what they lose.
    pub force: bool,
}

/// How `Gerdr::remove_repos` takes repositories out of a feature.
#[derive(Clone, Copy, Debug)]
pub struct RepoRemoval {
    /// Remove their worktrees and branches with what they lose.
    pub force: bool,
    /// Agents go on reading their main checkouts.
    pub to_context: bool,
}
