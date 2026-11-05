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
//! it, or stops it (see `feature_sandbox`), Ctrl+Shift+Y opens its app, Ctrl+Shift+G restarts it and
//! Ctrl+Shift+X resets its data or removes it,
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

use crate::feature::manual::Verdict;
use crate::manual_test::Row;
use crate::{
    Gerdr,
    agent_icons::AgentIcon,
    chrome::{
        self, ActivityRow, AgentChip, ButtonStyle, CheckLine, CheckTone, CommandsTab, Dot,
        FeatureRepoRow, FeatureTabHit, FeatureTabView, FindingCard, FindingTag, MAX_CHECK_LINES,
        NewFeatureHit, NewFeatureRepo, NewFeatureView, NodeMark, PageButton, PageSection, RailNode,
        RepoKind, RepoNote, StageMark, StagePage, TabArea, TabColumn,
    },
    doc_review::DocFile,
    feature::{
        self, Event, Feature, Pr, Repo, Stage, Status,
        agents::needs,
        fix_round::{FixPhase, FixRound},
        graph,
        hook::Guard,
        layout::{self, AddSpec, Created, Found, LinkSpec, Root},
        patch::{self, Reading},
        review_import::{
            self, CodeRead, Decision, Finding as ReviewFinding, FindingState, ReviewRound, Severity,
        },
        ship::{self, Action, Probe},
        store,
        verify::PlannedCheck,
    },
    feature_checks::{check_passed, checks_running},
    feature_graph::stop_graphs,
    herdr::AgentStatus,
    home_dir,
    menu::MenuAction,
    paths, projects,
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
    /// The latest probe of each feature, by project and slug.
    probes: BTreeMap<(String, String), Probed>,
    /// The feature folders whose docs link was seen to this run:
    /// `link_feature_docs` goes once for each.
    pub(crate) docs_linked: BTreeSet<PathBuf>,
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
    /// The plan review's sheet, while it is open (see `plan_sheet`).
    pub(crate) plan_sheet: Option<crate::plan_sheet::PlanSheet>,
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
}

/// What the Ship sheet asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    actions: Vec<Action>,
    /// The patch-id the check was on, for Ship's mark.
    patch: Option<String>,
}

/// What a feature's work off the thread is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Starting a feature's sandbox ended; `stopped`: because it was asked to.
    Sandbox {
        project: String,
        slug: String,
        run: feature::sandbox_run::SandboxRun,
        stopped: bool,
    },
    /// Stopping a feature's sandbox ended.
    SandboxStopped {
        project: String,
        slug: String,
        what: crate::feature_sandbox::StopFor,
        result: Result<(), String>,
    },
    /// The compose projects of sandboxes whose feature is gone.
    SandboxLeftovers { stacks: Vec<String> },
    /// Removing those ended.
    SandboxLeftoversRemoved { problems: Vec<String> },
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
        probe: Probe,
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
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
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
}

/// An open feature of another project spanning the one shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedFeature {
    /// Its home project's id and name.
    pub home: String,
    pub home_name: String,
    pub slug: String,
    pub title: String,
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
}

impl NewFeatureDialog {
    fn new(project: String, serial: u64, from: PathBuf) -> Self {
        Self {
            project,
            serial,
            title: TextField::default(),
            root: Finding::Looking,
            touched: Vec::new(),
            focus: None,
            top: 0,
            error: None,
            hover: None,
            from,
            others: Vec::new(),
            picking: false,
        }
    }

    /// The root was found, or not: every repository of the project is
    /// offered. Of several, the one it was opened from is ticked.
    fn found(&mut self, result: Result<Root, String>) {
        match result {
            Ok(root) => {
                self.touched = root
                    .repos
                    .iter()
                    .map(|r| !root.multi || self.from.starts_with(&r.main))
                    .collect();
                self.root = Finding::Found(root);
            }
            Err(why) => self.root = Finding::Failed(why),
        }
        self.error = None;
    }

    /// Project `project`'s root was found, or not. Nothing of it is ticked,
    /// unless it has only one repository to link it for.
    fn found_other(&mut self, project: &str, result: Result<Root, String>) {
        let Some(k) = self.others.iter().position(|o| o.project == project) else {
            return;
        };
        let other = &mut self.others[k];
        other.root = match result {
            Ok(root) if root.repos.is_empty() => Finding::Failed("no repositories in it".into()),
            Ok(root) => {
                other.touched = vec![root.repos.len() == 1; root.repos.len()];
                Finding::Found(root)
            }
            Err(why) => Finding::Failed(why),
        };
    }

    /// Its rows: the projects while picking; else the repositories, the
    /// linked projects' after the home root's.
    fn rows(&self) -> Vec<DialogRow> {
        if self.picking {
            return (0..self.others.len()).map(DialogRow::Project).collect();
        }
        let mut rows: Vec<DialogRow> = match &self.root {
            Finding::Found(root) => (0..root.repos.len()).map(DialogRow::Home).collect(),
            _ => Vec::new(),
        };
        for (k, other) in self.others.iter().enumerate() {
            if let (true, Finding::Found(root)) = (other.linked, &other.root) {
                rows.extend((0..root.repos.len()).map(|i| DialogRow::Linked(k, i)));
            }
        }
        rows
    }

    /// Whether its rows can be ticked: projects, linked repositories, and
    /// the home root's when it holds several.
    fn multi(&self) -> bool {
        self.picking
            || matches!(&self.root, Finding::Found(root) if root.multi)
            || self.others.iter().any(|o| o.linked)
    }

    fn repos(&self) -> usize {
        self.rows().len()
    }

    /// Whether row `row` can't be ticked: the only home repository, a
    /// project not found.
    fn fixed(&self, row: DialogRow) -> bool {
        match row {
            DialogRow::Home(_) => !matches!(&self.root, Finding::Found(root) if root.multi),
            DialogRow::Linked(..) => false,
            DialogRow::Project(k) => !matches!(self.others[k].root, Finding::Found(_)),
        }
    }

    fn toggle(&mut self, i: usize) {
        let Some(row) = self.rows().get(i).copied() else {
            return;
        };
        if self.fixed(row) {
            return;
        }
        let tick = match row {
            DialogRow::Home(i) => self.touched.get_mut(i),
            DialogRow::Linked(k, i) => self.others[k].touched.get_mut(i),
            DialogRow::Project(k) => Some(&mut self.others[k].linked),
        };
        if let Some(t) = tick {
            *t = !*t;
        }
        self.error = None;
    }

    /// The projects ticked and found.
    fn links(&self) -> Vec<LinkSpec> {
        self.others
            .iter()
            .filter(|o| o.linked)
            .filter_map(|o| {
                let Finding::Found(root) = &o.root else {
                    return None;
                };
                Some(LinkSpec {
                    project: o.project.clone(),
                    name: o.name.clone(),
                    root: root.clone(),
                    touched: (root.repos.iter().zip(&o.touched))
                        .filter(|(_, t)| **t)
                        .map(|(r, _)| r.name.clone())
                        .collect(),
                })
            })
            .collect()
    }

    /// A project ticked with none of its repositories, as a form error.
    fn unpicked(&self) -> Option<String> {
        let o = (self.others.iter()).find(|o| {
            o.linked && matches!(o.root, Finding::Found(_)) && !o.touched.iter().any(|t| *t)
        })?;
        Some(format!("Tick a repository of {}, or unlink it.", o.name))
    }

    /// What creating it would do, or why it can't, as a sentence.
    fn create(&self) -> Result<DialogOutcome, String> {
        if let Some(why) = self.unpicked() {
            return Err(why);
        }
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Give the feature a title.".into());
        }
        let root = match &self.root {
            Finding::Looking => {
                return Err("Still finding the repositories; try again in a moment.".into());
            }
            Finding::Failed(why) => return Err(format!("No repositories to start from: {why}.")),
            Finding::Found(root) => root,
        };
        let touched: Vec<String> = root
            .repos
            .iter()
            .zip(&self.touched)
            .filter(|(_, t)| **t)
            .map(|(r, _)| r.name.clone())
            .collect();
        let links = self.links();
        if touched.is_empty() && links.iter().all(|l| l.touched.is_empty()) {
            return Err("Tick at least one repository the feature changes.".into());
        }
        Ok(DialogOutcome::Create {
            title: title.to_owned(),
            root: root.clone(),
            touched,
            links,
        })
    }

    /// Switch the rows between the repositories and the project picker.
    fn pick(&mut self) {
        self.picking = !self.picking;
        // The picker's rows have the keyboard, found yet or not.
        self.focus = self.picking.then_some(0);
        self.top = 0;
        self.error = None;
    }

    /// Move the keyboard between the title and the rows.
    fn step(&mut self, back: bool) {
        let n = if self.multi() { self.repos() } else { 0 };
        let at = self.focus.map_or(0, |i| i + 1);
        let to = if back {
            (at + n) % (n + 1)
        } else {
            (at + 1) % (n + 1)
        };
        self.focus = to.checked_sub(1);
    }

    /// Scroll so the focused row is among the `fit` shown.
    fn reveal(&mut self, fit: usize) {
        let fit = fit.max(1);
        if let Some(i) = self.focus {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + fit {
                self.top = i + 1 - fit;
            }
        }
        self.top = self.top.min(self.repos().saturating_sub(fit));
    }

    /// Handle a key, `fit` rows shown at once: `sym` is the key's own
    /// symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        fit: usize,
    ) -> Option<DialogOutcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        match sym {
            Keysym::Escape => {
                // Out of the picker, back to the title, then the title
                // cleared, then closed.
                if self.picking {
                    self.pick();
                } else if self.focus.is_some() {
                    self.focus = None;
                } else if !self.title.is_empty() {
                    self.title.clear();
                    self.error = None;
                } else {
                    return Some(DialogOutcome::Cancel);
                }
            }
            Keysym::Return | Keysym::KP_Enter => match self.create() {
                Ok(outcome) => return Some(outcome),
                Err(why) => self.error = Some(why),
            },
            Keysym::l | Keysym::L if ctrl && mods.shift => self.pick(),
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::Down | Keysym::KP_Down => self.step(false),
            Keysym::Up | Keysym::KP_Up => self.step(true),
            Keysym::j | Keysym::J if ctrl => self.step(false),
            Keysym::k | Keysym::K if ctrl => self.step(true),
            Keysym::space if plain && self.focus.is_some() => {
                if let Some(i) = self.focus {
                    self.toggle(i);
                }
            }
            _ if self.focus.is_none() && self.title.key(mods, sym, typed) == Edit::Changed => {
                self.error = None;
            }
            _ => {}
        }
        self.reveal(fit);
        None
    }
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
    /// The criterion (as `FeaturePanel::finding` counts them) whose note is
    /// being written on its card, and the note. It stays with the panel when
    /// another tab shows.
    pub note: Option<(usize, TextField)>,
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
    /// The button Left and Right picked on the selected row (see
    /// `FeaturePanel::picked`).
    pub repo_pick: Option<usize>,
}

/// What the feature panel asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelOutcome {
    /// Mark the current stage done.
    Tick,
    /// Ship it, after the Ship sheet: the current stage is Ship.
    Ship,
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
    /// Open the sandbox's app.
    OpenSandbox,
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
    /// Send AI review's findings to fix to the Build agent.
    SendFixes,
    /// Mark every undecided finding of its AI review to fix.
    FixAllFindings,
    /// Start a fix round on Verify's failures.
    PlanFixes,
    /// Open the fix plan under review.
    ReviewFixPlan,
    /// The user approved the fix plan: the agent makes the fixes.
    MakeFixes,
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
    /// Ask for a comment of the user's own, to go with the fixes.
    AddComment,
    /// Show this finding in the diff.
    ShowFinding(String),
    /// Say why a key did nothing to a finding.
    FindingRefused(&'static str),
    /// Show this earlier review's findings on its page, or fold them.
    FoldRound(u32),
    /// Review the feature's diff (Code review).
    CodeReview,
    /// Give the keyboard to Manual test's criteria.
    ManualTest,
    /// What a Manual test page's criteria ask.
    Manual(crate::manual_test::Outcome),
    /// Open Plan review's suggestions, to decide them.
    PlanSuggestions,
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
    /// Abandon it, or once done remove its worktrees, after asking.
    Remove,
    New,
    /// Show the feature with this slug.
    Show(String),
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
const FEATURE_BUTTON: &str = "Feature";
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
}

impl RowCtx {
    /// Whether `stage` has a file of its own, as the latest probe found.
    pub(crate) fn has_file(self, stage: Stage) -> bool {
        self.written[stage_index(stage)]
    }

    /// Whether `stage`'s file is there to read on its page: Discover's
    /// and Plan's only.
    fn wrote(self, stage: Stage) -> bool {
        matches!(stage, Stage::Discover | Stage::Plan | Stage::PlanReview) && self.has_file(stage)
    }

    /// The stage that locks done `stage` of `feature`: see
    /// `Feature::locked_by`.
    fn locked_by(self, feature: &Feature, stage: Stage) -> Option<Stage> {
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
            StageMark::Current if stage == Stage::Ship => PanelOutcome::Ship,
            StageMark::Current if stage == Stage::Verify && ctx.checks => PanelOutcome::RunChecks,
            StageMark::Current if stage == Stage::PlanReview => plan_review_action(feature),
            StageMark::Current if stage == Stage::AiReview => review_action(feature),
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
fn agent_button(
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
enum Rework {
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
    fn node(self) -> (NodeMark, String) {
        let (mark, line) = match self {
            Rework::Waits => (NodeMark::Waiting, "waiting to send"),
            Rework::Closed => (NodeMark::Waiting, "agent closed"),
            Rework::Draft => (NodeMark::Waiting, "draft in T3 Code"),
            Rework::Runs(_) => (NodeMark::Fixing, "running again"),
        };
        (mark, line.to_owned())
    }

    /// How it colours the feature's sidebar card.
    fn tone(self) -> chrome::CardTone {
        match self {
            Rework::Runs(_) => chrome::CardTone::Working,
            Rework::Waits | Rework::Closed | Rework::Draft => chrome::CardTone::Waits,
        }
    }

    /// What its agent's card says.
    fn status(self) -> &'static str {
        match self {
            Rework::Waits => "waiting for you \u{b7} pick a model and send the prompt",
            Rework::Closed => "its tab closed \u{b7} Start agent opens it again",
            Rework::Draft => "draft in T3 Code \u{b7} pick a model and send it",
            Rework::Runs(_) => "running again",
        }
    }
}

/// How `feature`'s Build agent stands with work sent back to it, if any.
fn rework(feature: &Feature, ctx: RowCtx) -> Option<Rework> {
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

/// `rework`, on the page of `stage` when `stage` is where the feature is at.
fn rework_on(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<Rework> {
    (stage > Stage::Build && stage_mark(feature, stage) == StageMark::Current)
        .then(|| rework(feature, ctx))
        .flatten()
}

/// What the key for an agent's tab does on `stage`'s page: the Build agent's
/// while work sent back to it is under way, else what `agent_button` says.
fn agent_key(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<(&'static str, PanelOutcome)> {
    rework_on(feature, stage, ctx)
        .and_then(|_| agent_button(feature, Stage::Build, ctx))
        .or_else(|| agent_button(feature, stage, ctx))
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

/// What Plan review's row does: as any agent stage's while its agent
/// reviews or writes the final plan; in between, the suggestions to decide,
/// then what comes of them.
fn plan_review_action(feature: &Feature) -> PanelOutcome {
    let stage = Stage::PlanReview;
    if feature.runs.get(&stage).is_some_and(|r| r.waits_to_send()) {
        return PanelOutcome::SendPrompt(stage);
    }
    match crate::feature_plan_review::deciding(feature) {
        Some(review) if review.open() > 0 => PanelOutcome::PlanSuggestions,
        Some(_) => PanelOutcome::WriteFinalPlan,
        None => agent_action(feature, stage),
    }
}

/// Where a finding in `state` goes among the cards: those to decide first,
/// those gone to the end.
fn finding_rank(state: FindingState) -> u8 {
    match state {
        FindingState::Open => 0,
        FindingState::Fix => 1,
        FindingState::Sent => 2,
        FindingState::Fixed => 3,
        FindingState::Dismissed => 4,
    }
}

/// The latest AI review's findings on `stage`'s page of `feature`: those to
/// decide first, those gone to the end, each group in the order raised.
/// They stay once sent, while the code changes and while the next review
/// runs; `can_decide` says when they can be acted on.
pub(crate) fn finding_cards(feature: &Feature, stage: Stage) -> Vec<&ReviewFinding> {
    if stage != Stage::AiReview {
        return Vec::new();
    }
    let Some(review) = feature.review.as_ref() else {
        return Vec::new();
    };
    let mut cards = review.shown();
    cards.sort_by_key(|f| finding_rank(f.state));
    cards
}

/// The reviews before the latest on `stage`'s page of `feature`, the newest
/// first.
fn earlier_reviews(feature: &Feature, stage: Stage) -> impl Iterator<Item = &ReviewRound> {
    (feature.review.as_ref())
        .filter(|_| stage == Stage::AiReview)
        .into_iter()
        .flat_map(|review| review.rounds.iter().rev())
}

/// Why `stage`'s findings in `feature` can't be decided now, when they can't.
fn why_not_decide(feature: &Feature, stage: Stage) -> Option<&'static str> {
    if can_decide(feature, stage) {
        return None;
    }
    Some(match stage_mark(feature, stage) {
        StageMark::Later => "The code changed since this review: the next one checks it",
        StageMark::Current => "A new review hasn't finished: decide once it's read",
        StageMark::Done => "AI review is done: reopen it to change its findings",
    })
}

/// Whether the findings on `stage`'s page of `feature` can be decided, and a
/// comment of the user's added: AI review, current, its latest run read.
fn can_decide(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::AiReview
        && feature.status == Status::Open
        && feature.current() == Some(stage)
        && (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_some())
        && feature.review.is_some()
}

/// Whether `stage`'s page of `feature` can mark every undecided finding to
/// fix at once.
fn offers_fix_all(feature: &Feature, stage: Stage) -> bool {
    can_decide(feature, stage)
        && feature.fix_round.is_none()
        && (feature.review.as_ref()).is_some_and(|r| r.count(FindingState::Open) > 0)
}

/// What dismissing `f` does: an item of the user's own goes at once, with
/// nothing to tell themselves; a finding asks why.
fn dismiss_outcome(f: &ReviewFinding) -> PanelOutcome {
    match f.yours {
        true => PanelOutcome::DecideFinding(
            f.id.clone(),
            Decision::Dismiss(review_import::DROPPED.to_owned()),
        ),
        false => PanelOutcome::AskDismiss(f.id.clone()),
    }
}

/// The buttons on `f`'s card: what can be decided of it while AI review is
/// `current`, and showing it in the diff when it has a file.
fn finding_buttons(f: &ReviewFinding, current: bool) -> Vec<(PageButton, PanelOutcome)> {
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
    let plain = |label: &str| button(label, ButtonStyle::Plain);
    let id = || f.id.clone();
    let mut out = Vec::new();
    let undecided = matches!(f.state, FindingState::Open | FindingState::Fix);
    if current {
        match f.state {
            FindingState::Open => {
                out.push((
                    plain("Fix"),
                    PanelOutcome::DecideFinding(id(), Decision::Fix),
                ));
            }
            FindingState::Fix if !f.yours => {
                let back = PanelOutcome::DecideFinding(id(), Decision::Reopen);
                out.push((plain("Don't fix"), back));
            }
            FindingState::Dismissed => {
                let back = PanelOutcome::DecideFinding(id(), Decision::Reopen);
                out.push((plain("Reopen"), back));
            }
            _ => {}
        }
        if undecided {
            out.push((plain("Edit\u{2026}"), PanelOutcome::EditFinding(id())));
            let label = if f.yours { "Drop" } else { "Dismiss\u{2026}" };
            out.push((plain(label), dismiss_outcome(f)));
        }
    }
    if !f.file.is_empty() {
        out.push((
            button("Show in diff", ButtonStyle::Quiet),
            PanelOutcome::ShowFinding(id()),
        ));
    }
    out
}

/// What `stage`'s page of `feature` offers of the reviews it shows: their
/// findings' buttons, and showing each earlier one.
fn review_actions(feature: &Feature, stage: Stage) -> Vec<(PageButton, PanelOutcome)> {
    let decides = can_decide(feature, stage);
    let cards = finding_cards(feature, stage).into_iter();
    let shows = earlier_reviews(feature, stage).map(|round| {
        let label = "Show".to_owned();
        let style = ButtonStyle::Plain;
        (
            PageButton { label, style },
            PanelOutcome::FoldRound(round.n),
        )
    });
    (cards.flat_map(|f| finding_buttons(f, decides)))
        .chain(shows)
        .collect()
}

/// The finding a card's button acts on.
fn outcome_finding(outcome: &PanelOutcome) -> Option<&str> {
    match outcome {
        PanelOutcome::DecideFinding(id, _)
        | PanelOutcome::AskDismiss(id)
        | PanelOutcome::EditFinding(id)
        | PanelOutcome::ShowFinding(id) => Some(id),
        _ => None,
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
fn has_failed_checks(feature: &Feature, stage: Stage) -> bool {
    let rows = check_rows(feature, stage);
    !checks_running(feature)
        && rows.iter().any(check_passed)
        && rows.iter().any(|row| !check_passed(row))
}

/// One card of Verify's page: a check the last run ran, or one `plan` gives
/// the feature that it didn't.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CheckEntry {
    repo: String,
    command: String,
    /// Its index in the last run's rows.
    row: Option<usize>,
    /// The feature skips it now.
    skipped: bool,
    /// It comes from a project's checks, so the feature can skip it. One no
    /// longer given isn't: its card stays until the next run.
    borrowed: bool,
}

/// `stage`'s checks in `feature` as cards: the last run's first, so a card's
/// index is its row's, then those of `plan` no run has got to.
fn check_entries(feature: &Feature, stage: Stage, plan: &[PlannedCheck]) -> Vec<CheckEntry> {
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

/// What skipping check `entry` of `feature` reads as, and does; nothing for
/// one it can't skip, while the checks run, or on a stage not reached.
fn skip_action(
    feature: &Feature,
    stage: Stage,
    entry: &CheckEntry,
) -> Option<(&'static str, PanelOutcome)> {
    let can = entry.borrowed
        && feature.status == Status::Open
        && stage_mark(feature, stage) != StageMark::Later
        && !checks_running(feature);
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
fn stage_actions(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    plan: &[PlannedCheck],
    criteria: &[Row],
) -> Vec<(PageButton, PanelOutcome)> {
    let mut out = page_actions(feature, stage, ctx);
    let entries = check_entries(feature, stage, plan);
    // What it skips doesn't run, once or at all.
    out.retain(|(_, outcome)| {
        !matches!(outcome, PanelOutcome::RerunCheck(i)
            if entries.get(*i).is_some_and(|entry| entry.skipped))
    });
    out.extend(
        (entries.iter())
            .filter_map(|entry| skip_action(feature, stage, entry))
            .map(|(label, outcome)| {
                let style = match label {
                    "Skip" => ButtonStyle::Quiet,
                    _ => ButtonStyle::Plain,
                };
                let label = label.to_owned();
                (PageButton { label, style }, outcome)
            }),
    );
    if tries_criteria(feature, stage) {
        out.extend((criteria.iter().enumerate()).flat_map(|(i, row)| criterion_buttons(i, row)));
    }
    out
}

/// Whether `stage`'s page of `feature` is where its criteria are tried:
/// Manual test's, while the feature is at it.
fn tries_criteria(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest
        && feature.status == Status::Open
        && stage_mark(feature, stage) == StageMark::Current
}

/// Whether `stage`'s page of `feature` shows its criteria: Manual test's, once
/// the feature got to it. Once done they are what was found, not to change.
fn shows_criteria(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest && stage_mark(feature, stage) != StageMark::Later
}

/// The buttons on criterion `i`'s card, as it stands.
fn criterion_buttons(i: usize, row: &Row) -> Vec<(PageButton, PanelOutcome)> {
    use crate::manual_test::Outcome;
    let plain = |label: &str, outcome| {
        let button = PageButton {
            label: label.to_owned(),
            style: ButtonStyle::Plain,
        };
        (button, PanelOutcome::Manual(outcome))
    };
    let (pass, fail, clear) = (
        plain("Pass", Outcome::Toggle(i)),
        plain("Fail\u{2026}", Outcome::Write(i)),
        plain("Clear", Outcome::Clear(i)),
    );
    match row.found {
        None => vec![pass, fail],
        Some(Verdict::Pass) => vec![clear, fail],
        Some(Verdict::Fail) => vec![clear, plain("Edit note\u{2026}", Outcome::Write(i)), pass],
    }
}

/// Criterion `i`'s card on Manual test's page, its buttons those in
/// `actions`: its dot tells how it went, the note what went wrong. While
/// `trying`, one that passed fades: the rest are what is left to do.
fn criterion_card(
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
    let state = match row.sent {
        true => "sent to Build",
        false => "",
    };
    FindingCard {
        tag,
        title: row.text.clone(),
        state: state.to_owned(),
        place: row.id.clone(),
        body: String::new(),
        preview: Vec::new(),
        fix: String::new(),
        note: match row.note.is_empty() {
            true => String::new(),
            false => format!("What went wrong: {}", row.note),
        },
        faded: trying && tag == FindingTag::Passed,
        field: None,
        selected: false,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                matches!(o, PanelOutcome::Manual(outcome) if outcome.criterion() == Some(i))
            })
            .map(|(at, _)| at)
            .collect(),
    }
}

/// The cards of `state`'s criteria, in their order.
fn criterion_sections<'a>(
    state: &'a PanelState,
    actions: &'a [(PageButton, PanelOutcome)],
    trying: bool,
) -> impl Iterator<Item = PageSection> + 'a {
    (state.criteria.iter().enumerate())
        .map(move |(i, row)| PageSection::Finding(criterion_card(i, row, actions, trying)))
}

/// The buttons on check `i`'s card: its terminal, then stopping it while it
/// runs, or running it again while Verify is `current`.
fn check_buttons(
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
        out.push((plain("Stop"), PanelOutcome::StopCheck(i)));
    } else if current && !running {
        out.push((plain("Run again"), PanelOutcome::RerunCheck(i)));
    }
    out
}

/// The check an action on a card is about.
fn outcome_check(outcome: &PanelOutcome) -> Option<usize> {
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
fn check_card(
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
    let took = |secs: u64| crate::test_runs::took(std::time::Duration::from_secs(secs));
    let mut said = Vec::new();
    match (tag, outcome) {
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
        faded: tag == FindingTag::Skipped,
        field: None,
        selected: false,
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
fn on_older_code(feature: &Feature, probe: Option<&Probe>) -> bool {
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
fn checks_heading(cards: &[FindingCard]) -> String {
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

/// `f`'s card on AI review's page, its buttons those in `actions`.
fn finding_card(
    f: &ReviewFinding,
    multi: bool,
    actions: &[(PageButton, PanelOutcome)],
) -> FindingCard {
    let tag = match (f.yours, f.severity) {
        (true, _) => FindingTag::Yours,
        (_, Severity::High) => FindingTag::High,
        (_, Severity::Medium) => FindingTag::Medium,
        (_, Severity::Low) => FindingTag::Low,
    };
    let state = match f.state {
        FindingState::Open => String::new(),
        FindingState::Fix => "to fix".to_owned(),
        FindingState::Sent => "sent to Build".to_owned(),
        FindingState::Fixed => "fixed".to_owned(),
        FindingState::Dismissed => format!("dismissed: {}", f.reason),
    };
    let mut place = vec![f.id.clone()];
    if !f.file.is_empty() {
        place.push(review_import::location(f, multi));
        if !f.placed {
            place.push("not in the diff".to_owned());
        }
    }
    let fix = match &f.my_fix {
        Some(fix) => format!("Your fix: {fix}"),
        None if f.fix.is_empty() => String::new(),
        None => format!("Fix: {}", f.fix),
    };
    let note = match f.note.is_empty() {
        true => String::new(),
        false => format!("Your comment: {}", f.note),
    };
    FindingCard {
        tag,
        title: f.title.clone(),
        state,
        place: place.join(" \u{b7} "),
        body: f.body.clone(),
        preview: Vec::new(),
        fix,
        note,
        faded: matches!(f.state, FindingState::Fixed | FindingState::Dismissed),
        field: None,
        selected: false,
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| outcome_finding(o) == Some(f.id.as_str()))
            .map(|(i, _)| i)
            .collect(),
    }
}

/// How `round`, an earlier review, went, in a few words: "3 fixed · 1 raised
/// again".
fn round_count(round: &ReviewRound) -> String {
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
fn round_card(f: &ReviewFinding, multi: bool) -> FindingCard {
    let mut card = finding_card(f, multi, &[]);
    card.faded = true;
    if f.state == FindingState::Sent {
        "raised again".clone_into(&mut card.state);
    }
    card
}

/// What AI review's row does: show its agent while it runs (start one
/// when there is none), then once its findings are read, show the first
/// to decide, or send those to fix to the Build agent. Never marked done
/// by hand: gerdr marks it once no finding holds up.
fn review_action(feature: &Feature) -> PanelOutcome {
    let stage = Stage::AiReview;
    let Some(run) = feature.runs.get(&stage) else {
        return PanelOutcome::StartAgent(stage, false);
    };
    let review = feature.review.as_ref().filter(|_| run.ready.is_some());
    match review {
        Some(r) if r.count(FindingState::Open) > 0 => PanelOutcome::Findings,
        Some(r) if r.count(FindingState::Fix) > 0 => PanelOutcome::SendFixes,
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

/// The detail of AI review's row while it is current, beside `agent`,
/// its agent's line: that line while the agent reviews; once its findings
/// are read, what waits for the user.
fn review_line(feature: &Feature, agent: Option<&str>) -> Option<String> {
    let run = feature.runs.get(&Stage::AiReview)?;
    let review = feature.review.as_ref().filter(|_| run.ready.is_some());
    let Some(review) = review else {
        return agent.map(str::to_owned);
    };
    let open = review.count(FindingState::Open);
    let fix = review.count(FindingState::Fix);
    let sent = review.count(FindingState::Sent);
    Some(if open > 0 {
        format!("{} to decide", plural(open, "finding"))
    } else if fix > 0 {
        format!("{} to fix", plural(fix, "finding"))
    } else if sent > 0 {
        format!("{} sent to Build", plural(sent, "finding"))
    } else if review.findings.is_empty() {
        "no findings".into()
    } else {
        "all findings fixed or dismissed".into()
    })
}

/// The agent stage under `row` when it is current and has had an agent:
/// Ctrl+R starts a fresh one.
fn fresh_stage(feature: &Feature, row: PanelRow) -> Option<Stage> {
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
/// on the page goes first: that key drops it.
fn start_over_stage(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<Stage> {
    let live = (feature.runs.get(&stage)).is_some_and(|run| run.live());
    (feature.status == Status::Open
        && feature.current() == Some(stage)
        && stage != Stage::Discover
        && fix_round_on(feature, stage).is_none()
        && (live || feature.has_output(stage, |s| ctx.has_file(s))))
    .then_some(stage)
}

/// Whether Verify of `feature` wrote `evidence.md`: a run of it ended, or
/// one before the one running.
fn has_evidence(feature: &Feature) -> bool {
    feature
        .verify
        .as_ref()
        .is_some_and(|v| v.finished.is_some() || v.n > 1)
}

/// What Ctrl+Shift+O does on `row`: open the pull request of a repository
/// that has one, or Verify's evidence once it has some.
fn row_page(feature: &Feature, row: PanelRow) -> Option<PanelOutcome> {
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
fn row_hint(feature: &Feature, row: PanelRow, ctx: RowCtx) -> &'static str {
    row_action(feature, row, ctx).map_or("", |action| action_hint(&action))
}

/// What a key doing `action` is called in the hints.
fn action_hint(action: &PanelOutcome) -> &'static str {
    match action {
        PanelOutcome::StartAgent(..) => "start agent",
        PanelOutcome::SendPrompt(_) => "send prompt",
        PanelOutcome::ShowAgent(_) => "show agent",
        PanelOutcome::ResumeAgent(_) => "resume agent",
        PanelOutcome::OpenDoc(Stage::Discover) => "review brief",
        PanelOutcome::OpenDoc(_) => "review plan",
        PanelOutcome::Continue(_) => "continue",
        PanelOutcome::RunChecks => "run checks",
        PanelOutcome::SetUp => "set up",
        PanelOutcome::SetupLog => "show setup log",
        PanelOutcome::StartSandbox => "start sandbox\u{2026}",
        PanelOutcome::OpenSandbox => "open app",
        PanelOutcome::RestartSandbox => "restart sandbox",
        PanelOutcome::ResetSandbox => "reset data\u{2026}",
        PanelOutcome::StopSandbox => "stop sandbox\u{2026}",
        PanelOutcome::RemoveSandbox => "remove sandbox\u{2026}",
        PanelOutcome::Findings => "review in diff",
        PanelOutcome::SendFixes | PanelOutcome::PlanFixes => "plan fixes",
        PanelOutcome::FixAllFindings => "fix all open",
        PanelOutcome::ReviewFixPlan => "review fix plan",
        PanelOutcome::MakeFixes => "make fixes",
        PanelOutcome::AskFixPlan => "ask again",
        PanelOutcome::EditCommands(_) => "edit commands",
        PanelOutcome::CodeReview => "review code",
        PanelOutcome::ManualTest => "try the criteria",
        PanelOutcome::Manual(crate::manual_test::Outcome::MarkPassed) => "mark passed",
        PanelOutcome::Manual(crate::manual_test::Outcome::SendFailures) => "plan fixes",
        PanelOutcome::PlanSuggestions => "decide",
        PanelOutcome::WriteFinalPlan => "go on",
        PanelOutcome::Tick => "mark done",
        PanelOutcome::Ship => "ship\u{2026}",
        PanelOutcome::Untick(_) => "reopen",
        PanelOutcome::GoToCurrent => "go to the current stage",
        PanelOutcome::Touch(_) => "add to feature",
        PanelOutcome::Open(_) => "open tab",
        _ => "",
    }
}

/// The label of `stage`'s button in `feature`: none for a later one.
fn stage_button(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<&'static str> {
    match row_action(feature, PanelRow::Stage(stage), ctx)? {
        PanelOutcome::Tick => Some("Mark done"),
        PanelOutcome::Ship => Some("Ship\u{2026}"),
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
        PanelOutcome::CodeReview => Some("Review code"),
        PanelOutcome::PlanSuggestions => Some("Decide\u{2026}"),
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
fn pr_of<'a>(feature: &'a Feature, repo: &str) -> Option<&'a Pr> {
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
fn pr_tail(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').take(3).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// What Ship asks for, given the latest `probe`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ShipAsk {
    /// No check yet.
    Checking,
    /// What stops it: the toast's text.
    Blocked(String),
    /// It can ship, but nothing would.
    Nothing,
    Ready(Asked),
}

/// What shipping `title` asks for, given the latest `probe`.
fn ship_ask(title: &str, probe: Option<&Probe>) -> ShipAsk {
    let Some(probe) = probe else {
        return ShipAsk::Checking;
    };
    let Some(check) = &probe.ship else {
        return match &probe.reading {
            Err(why) => ShipAsk::Blocked(blocked_text(title, &[read_blocker(why)])),
            Ok(_) => ShipAsk::Checking,
        };
    };
    if !check.blockers.is_empty() {
        return ShipAsk::Blocked(blocked_text(title, &check.blockers));
    }
    if check.actions.is_empty() {
        return ShipAsk::Nothing;
    }
    ShipAsk::Ready(Asked {
        actions: check.actions.clone(),
        patch: probe.reading.as_ref().ok().map(|r| r.patch.clone()),
    })
}

/// What a feature's code that couldn't be read says in the Ship check.
fn read_blocker(why: &str) -> String {
    format!("Couldn't read the code: {why}")
}

/// The warning when `title` can't ship for `blockers`: the first, and how
/// many more.
fn blocked_text(title: &str, blockers: &[String]) -> String {
    let first = blockers.first().map_or("", String::as_str);
    match blockers.len() {
        0 | 1 => format!("Can't ship \u{201c}{title}\u{201d} yet: {first}"),
        n => format!(
            "Can't ship \u{201c}{title}\u{201d} yet: {first}, and {} more",
            n - 1
        ),
    }
}

/// The warning when the code of `title` changed, its marks from `stage`
/// on gone.
fn changed_text(title: &str, stage: Stage) -> String {
    format!("\u{201c}{title}\u{201d} changed: back to {}", stage.label())
}

/// The success toast once the pull requests of `title` are open:
/// `opened` new ones, and `updated` ones it had open already, which the
/// push updated.
fn opened_text(title: &str, opened: usize, updated: usize) -> String {
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
fn capped(lines: Vec<CheckLine>, tone: CheckTone) -> Vec<CheckLine> {
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
fn check_lines(
    feature: &Feature,
    probe: Option<&Probe>,
    shipping: bool,
    now: u64,
) -> Vec<CheckLine> {
    if feature.status != Status::Open || feature.current() != Some(Stage::Ship) {
        return Vec::new();
    }
    let line = |text: String, tone| CheckLine { text, tone };
    if shipping {
        return vec![line("shipping\u{2026}".into(), CheckTone::Note)];
    }
    if let Some(at) = feature.shipping.squash_at {
        let when = projects::ago(now, at);
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
    if !check.blockers.is_empty() {
        let lines = check
            .blockers
            .iter()
            .map(|b| line(b.clone(), CheckTone::Blocker))
            .collect();
        return capped(lines, CheckTone::Blocker);
    }
    let ready = line("Ready to ship".into(), CheckTone::Ready);
    let notes = check.notes.iter().map(|n| line(n.clone(), CheckTone::Note));
    capped(
        std::iter::once(ready).chain(notes).collect(),
        CheckTone::Note,
    )
}

/// Whether the patch watch keeps an eye on `feature`: open, with a mark
/// bound to its code or a squash under way.
fn watched(feature: &Feature) -> bool {
    feature.status == Status::Open
        && (feature.marks.keys().any(|s| s.bound()) || feature.shipping.squash_at.is_some())
}

/// How often the patch watch probes watched `feature` while the panel
/// shows another: as often as the one shown while its squash is under
/// way, as the user waits on it in its tab (and the panel closed to show
/// that tab), else every `WATCH_EVERY`.
fn watch_every(feature: &Feature) -> Duration {
    if feature.shipping.squash_at.is_some() {
        PANEL_EVERY
    } else {
        WATCH_EVERY
    }
}

/// The label of what removes `feature`'s worktrees: abandoning it while
/// open, removing them once done.
pub fn remove_label(feature: &Feature) -> &'static str {
    if feature.status == Status::Done {
        "Remove worktrees"
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
fn linked_in<'a>(
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

impl FeaturePanel {
    /// The stage whose page shows on `feature`: the one picked, while the
    /// feature is still at the stage it was at then, else its current
    /// one (Ship once every stage is done).
    pub fn page_stage(&self, feature: &Feature) -> Stage {
        let current = feature.current();
        (self.stage.filter(|_| self.picked_at == current))
            .or(current)
            .unwrap_or(Stage::Ship)
    }

    /// Show `stage`'s page of `feature`, until the feature moves on.
    pub fn pick(&mut self, feature: &Feature, stage: Stage) {
        self.stage = Some(stage);
        self.picked_at = feature.current();
    }

    /// Show the current stage's page again.
    pub fn unpick(&mut self) {
        self.stage = None;
        self.picked_at = None;
    }

    /// The page the Page column shows on `feature`: its stage, and
    /// whether the keys list shows in its place.
    fn page_key(&self, feature: &Feature) -> (Stage, bool) {
        (self.page_stage(feature), self.keys)
    }

    /// The Page column's scroll as it shows on `feature`: none while it
    /// was scrolled for another page.
    fn page_scroll(&self, feature: &Feature) -> Option<&Scroll> {
        let col = TabColumn::Page as usize;
        (self.page_of == Some(self.page_key(feature))).then(|| &self.scroll[col])
    }

    /// Hold the Page column's scroll for page `key`, from its top if it
    /// was for another.
    fn hold_page(&mut self, key: (Stage, bool)) {
        if self.page_of != Some(key) {
            self.scroll[TabColumn::Page as usize] = Scroll::default();
            self.page_of = Some(key);
        }
    }

    /// The repository row at `at` takes the keyboard.
    fn select_row(&mut self, at: usize) {
        self.focus = TabArea::Repos;
        self.repo_pick = (self.repo == at).then_some(self.repo_pick).flatten();
        self.repo = at;
    }

    /// The row of repository `key` takes the keyboard, if it has one.
    pub fn select_repo(&mut self, feature: &Feature, key: &str) {
        // The rows just changed under a pointer that stayed: what it was
        // over is somewhere else now, until it moves.
        self.tab_hover = None;
        let at = repo_entries(feature, &self.sources, self.expanded)
            .into_iter()
            .position(|e| entry_key(feature, &self.sources, e).as_deref() == Some(key));
        if let Some(at) = at {
            self.select_row(at);
            self.repo_pick = None;
        }
    }

    /// `select_repo`, leaving the keyboard where it is and the button
    /// `pick` picked: what the rows did before them changed, not the user.
    fn select_repo_keeping(&mut self, feature: &Feature, key: &str, pick: Option<usize>) {
        let focus = self.focus;
        self.select_repo(feature, key);
        self.focus = focus;
        self.repo_pick = pick;
    }

    /// Show the earlier review `n`'s findings, or fold them.
    pub fn toggle_round(&mut self, n: u32) {
        if !self.rounds_open.remove(&n) {
            self.rounds_open.insert(n);
        }
    }

    /// Fold the earlier reviews of `stage`'s page of `feature` if any shows,
    /// else show them all.
    fn toggle_rounds(&mut self, feature: &Feature, stage: Stage) {
        let all: BTreeSet<u32> = earlier_reviews(feature, stage).map(|r| r.n).collect();
        self.rounds_open = match all.iter().any(|n| self.rounds_open.contains(n)) {
            true => BTreeSet::new(),
            false => all,
        };
    }

    /// Show the repositories as rows, or back as the strip's chips. The
    /// keyboard goes to the rows, and back to where it was. A row past
    /// what collapsed lists is left to clamp to the last chip.
    pub fn toggle_repos(&mut self) {
        self.expanded = !self.expanded;
        self.repo_pick = None;
        self.tab_hover = None;
        if self.expanded {
            self.collapse_to = self.focus;
            self.focus = TabArea::Repos;
            self.scroll[TabColumn::Repos as usize] = Scroll::default();
        } else {
            self.focus = self.collapse_to;
        }
    }

    /// The button on the row of `entry` that Enter presses: the one Left
    /// and Right picked, else the first of a row that has nothing else to
    /// do. A row the feature holds opens its tab until one is picked.
    pub fn picked(&self, feature: &Feature, entry: RepoEntry) -> Option<usize> {
        let buttons = entry_buttons(feature, &self.sources, entry).len();
        let first = (!matches!(entry, RepoEntry::Held(_))).then_some(0);
        self.repo_pick
            .filter(|at| *at < buttons)
            .or(first)
            .filter(|at| *at < buttons)
    }

    /// Left (`back`) or Right on the row of `entry`.
    fn pick_button(&mut self, feature: &Feature, entry: RepoEntry, back: bool) {
        let buttons = entry_buttons(feature, &self.sources, entry).len();
        let picked = self.picked(feature, entry);
        self.repo_pick = match (picked, back) {
            _ if buttons == 0 => None,
            (None, false) => Some(0),
            (Some(at), false) => Some((at + 1).min(buttons - 1)),
            (Some(at), true) if at > 0 => Some(at - 1),
            (Some(_), true) if matches!(entry, RepoEntry::Held(_)) => None,
            (at, true) => at,
        };
    }

    /// Del with the keyboard on the row of `entry`, if it is one the
    /// feature can do without. A repository on the feature's branch asks
    /// (see `ask_remove_repos`), so its Del acts at once; one that goes by
    /// itself arms first (checklist C4). Rows the feature doesn't hold have
    /// nothing to take out, and never fall through to abandoning it.
    fn delete_row(
        &mut self,
        feature: &Feature,
        entry: RepoEntry,
        armed: Option<String>,
    ) -> Option<PanelOutcome> {
        let arm = |panel: &mut Self, key: String, then: PanelOutcome| {
            if armed.as_ref() == Some(&key) {
                return Some(then);
            }
            panel.armed = Some(key);
            None
        };
        match entry {
            RepoEntry::Held(i) if feature.many_repos() => {
                let repo = feature.repos.get(i)?;
                let key = repo.key();
                match repo.touched {
                    true => Some(PanelOutcome::RemoveRepo(key)),
                    false => arm(self, key.clone(), PanelOutcome::RemoveRepo(key)),
                }
            }
            RepoEntry::Held(_) => Some(PanelOutcome::Remove),
            RepoEntry::Context(i) if linkable(feature) => {
                let key = feature.context.get(i)?.key();
                arm(self, key.clone(), PanelOutcome::DropContext(key))
            }
            _ => None,
        }
    }

    /// Handle a key over `feature`, one of `slugs` listed.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        feature: &Feature,
        slugs: &[String],
        ctx: RowCtx,
        plan: &[PlannedCheck],
    ) -> Option<PanelOutcome> {
        let entries = repo_entries(feature, &self.sources, self.expanded);
        let repos = entries.len();
        self.repo = self.repo.min(repos.saturating_sub(1));
        if repos == 0 && self.focus == TabArea::Repos {
            self.focus = TabArea::Stages;
        }
        let entry = entries.get(self.repo).copied();
        let armed = self.armed.take();
        use crate::manual_test::Outcome;
        // The pick no longer holds once the feature moved on.
        if self.stage.is_some() && self.picked_at != feature.current() {
            self.unpick();
        }
        let stage = self.page_stage(feature);
        let cards = finding_cards(feature, stage);
        let checks = check_entries(feature, stage, plan);
        let criteria = match tries_criteria(feature, stage) {
            true => ctx.criteria,
            false => 0,
        };
        let shown = match shows_criteria(feature, stage) {
            true => ctx.criteria,
            false => 0,
        };
        let carded = cards.len() + checks.len() + shown;
        self.finding = self.finding.min(carded.saturating_sub(1));
        if self.focus == TabArea::Findings && (carded == 0 || self.expanded) {
            self.focus = TabArea::Stages;
        }
        let card = cards.get(self.finding).copied();
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let bare = plain && !mods.shift;
        let refused = why_not_decide(feature, stage);
        // The keys list: F1 or Esc puts it away, as does any other key
        // (it does nothing else then).
        if self.keys {
            self.keys = false;
            return None;
        }
        if sym == Keysym::F1 && bare {
            self.keys = true;
            return None;
        }
        // Esc folds the rows first, and steps back as ever after that.
        if sym == Keysym::Escape && bare && self.expanded {
            self.toggle_repos();
            return None;
        }
        // On the rows Left and Right go through the row's buttons.
        if plain && self.expanded && self.focus == TabArea::Repos {
            let back = match sym {
                Keysym::Left | Keysym::KP_Left => Some(true),
                Keysym::Right | Keysym::KP_Right => Some(false),
                _ => None,
            };
            if let (Some(back), Some(entry)) = (back, entry) {
                self.pick_button(feature, entry, back);
                return None;
            }
        }
        let step = match sym {
            Keysym::Up | Keysym::KP_Up | Keysym::Left | Keysym::KP_Left if plain => {
                Some(Step::Back)
            }
            Keysym::Down | Keysym::KP_Down | Keysym::Right | Keysym::KP_Right if plain => {
                Some(Step::On)
            }
            Keysym::k | Keysym::K if ctrl && !mods.shift => Some(Step::Back),
            Keysym::j | Keysym::J if ctrl && !mods.shift => Some(Step::On),
            Keysym::Home | Keysym::KP_Home if plain => Some(Step::First),
            Keysym::End | Keysym::KP_End if plain => Some(Step::Last),
            _ => None,
        };
        if let Some(step) = step {
            match self.focus {
                TabArea::Stages => {
                    let at = stage_index(stage);
                    let to = step.from(at, Stage::ALL.len() - 1);
                    self.pick(feature, Stage::ALL[to]);
                }
                TabArea::Repos => {
                    let to = step.from(self.repo, repos.saturating_sub(1));
                    if to != self.repo {
                        self.repo = to;
                        self.repo_pick = None;
                    }
                }
                TabArea::Findings => {
                    self.finding = step.from(self.finding, carded.saturating_sub(1));
                }
            }
            return None;
        }
        let row = match (self.focus, entry) {
            (TabArea::Stages | TabArea::Findings, _) => PanelRow::Stage(stage),
            (TabArea::Repos, Some(RepoEntry::Held(i))) => PanelRow::Repo(i),
            (TabArea::Repos, _) => PanelRow::Other,
        };
        match sym {
            Keysym::Tab | Keysym::ISO_Left_Tab if plain => {
                let mut stops = vec![TabArea::Stages];
                if carded > 0 && !self.expanded {
                    stops.push(TabArea::Findings);
                }
                if repos > 0 {
                    stops.push(TabArea::Repos);
                }
                let at = stops.iter().position(|s| *s == self.focus).unwrap_or(0);
                let back = mods.shift || sym == Keysym::ISO_Left_Tab;
                let by = if back { stops.len() - 1 } else { 1 };
                self.focus = stops[(at + by) % stops.len()];
            }
            // A criterion's card with the keyboard.
            Keysym::space | Keysym::Return | Keysym::KP_Enter
                if bare && self.focus == TabArea::Findings && criteria > 0 =>
            {
                return Some(PanelOutcome::Manual(Outcome::Toggle(self.finding)));
            }
            Keysym::f if bare && self.focus == TabArea::Findings && criteria > 0 => {
                return Some(PanelOutcome::Manual(Outcome::Write(self.finding)));
            }
            Keysym::Delete | Keysym::KP_Delete
                if bare && self.focus == TabArea::Findings && criteria > 0 =>
            {
                return Some(PanelOutcome::Manual(Outcome::Clear(self.finding)));
            }
            // From anywhere on the page. Reopen and a fix plan's keys are
            // for when it has one: then these are not its keys.
            Keysym::Return | Keysym::KP_Enter
                if ctrl
                    && criteria > 0
                    && self.focus != TabArea::Repos
                    && fix_round_on(feature, stage).is_none() =>
            {
                return Some(PanelOutcome::Manual(match mods.shift {
                    true => Outcome::SendFailures,
                    false => Outcome::MarkPassed,
                }));
            }
            // A check's card with the keyboard.
            Keysym::Return | Keysym::KP_Enter
                if plain && self.focus == TabArea::Findings && !checks.is_empty() =>
            {
                let row = checks.get(self.finding)?.row?;
                return Some(PanelOutcome::ShowCheck(row));
            }
            Keysym::r if bare && self.focus == TabArea::Findings && !checks.is_empty() => {
                let check = checks.get(self.finding).filter(|c| !c.skipped)?;
                return Some(PanelOutcome::RerunCheck(check.row?));
            }
            Keysym::s if bare && self.focus == TabArea::Findings && !checks.is_empty() => {
                let row = checks.get(self.finding)?.row?;
                return Some(PanelOutcome::StopCheck(row));
            }
            Keysym::space if bare && self.focus == TabArea::Findings && !checks.is_empty() => {
                let check = checks.get(self.finding)?;
                return skip_action(feature, stage, check).map(|(_, outcome)| outcome);
            }
            // All of them, from anywhere on the page.
            Keysym::r | Keysym::R
                if plain
                    && mods.shift
                    && self.focus != TabArea::Repos
                    && has_failed_checks(feature, stage) =>
            {
                return Some(PanelOutcome::RunFailed);
            }
            Keysym::s | Keysym::S
                if plain
                    && mods.shift
                    && self.focus != TabArea::Repos
                    && stage == Stage::Verify
                    && checks_running(feature) =>
            {
                return Some(PanelOutcome::StopChecks);
            }
            Keysym::f | Keysym::F if plain && mods.shift && offers_fix_all(feature, stage) => {
                return Some(PanelOutcome::FixAllFindings);
            }
            // The card with the keyboard: the diff's keys for a finding.
            Keysym::e | Keysym::E
                if mods.alt
                    && !(mods.ctrl || mods.shift || mods.logo)
                    && !self.expanded
                    && earlier_reviews(feature, stage).next().is_some() =>
            {
                self.toggle_rounds(feature, stage);
                return None;
            }
            Keysym::f | Keysym::F if bare && self.focus == TabArea::Findings => {
                let f = card?;
                if let Some(why) = refused {
                    return Some(PanelOutcome::FindingRefused(why));
                }
                return Some(match review_import::fix_key(f.state) {
                    Ok(decision) => PanelOutcome::DecideFinding(f.id.clone(), decision),
                    Err(why) => PanelOutcome::FindingRefused(why),
                });
            }
            Keysym::e | Keysym::E if ctrl && !mods.shift && self.focus == TabArea::Findings => {
                let f = card?;
                return Some(match refused {
                    Some(why) => PanelOutcome::FindingRefused(why),
                    None => PanelOutcome::EditFinding(f.id.clone()),
                });
            }
            Keysym::Delete | Keysym::KP_Delete if bare && self.focus == TabArea::Findings => {
                let f = card?;
                if let Some(why) = refused {
                    return Some(PanelOutcome::FindingRefused(why));
                }
                return Some(match review_import::dismiss_key(f.state) {
                    Ok(None) => dismiss_outcome(f),
                    Ok(Some(decision)) => PanelOutcome::DecideFinding(f.id.clone(), decision),
                    Err(why) => PanelOutcome::FindingRefused(why),
                });
            }
            Keysym::Return | Keysym::KP_Enter if plain && self.focus == TabArea::Findings => {
                let f = card?;
                return Some(match (f.file.is_empty(), refused) {
                    (true, Some(why)) => PanelOutcome::FindingRefused(why),
                    (true, None) => PanelOutcome::EditFinding(f.id.clone()),
                    (false, _) => PanelOutcome::ShowFinding(f.id.clone()),
                });
            }
            Keysym::c | Keysym::C
                if bare && self.focus != TabArea::Repos && can_decide(feature, stage) =>
            {
                return Some(PanelOutcome::AddComment);
            }
            // A locked stage's is refused when done, saying why; any other
            // stage has nothing to reopen.
            Keysym::Return | Keysym::KP_Enter
                if ctrl
                    && mods.shift
                    && (self.focus == TabArea::Stages
                        || (self.focus == TabArea::Findings && shown > 0 && criteria == 0)) =>
            {
                return (stage_mark(feature, stage) == StageMark::Done)
                    .then_some(PanelOutcome::Untick(stage));
            }
            // The file's review has it too.
            Keysym::Return | Keysym::KP_Enter
                if ctrl
                    && (self.focus == TabArea::Stages
                        || self.focus == TabArea::Findings && criteria > 0)
                    && (fix_round_on(feature, stage)
                        .is_some_and(|r| r.phase == FixPhase::Ready)
                        || makes_fixes_again(feature, stage)) =>
            {
                return Some(PanelOutcome::MakeFixes);
            }
            // With a fix plan under way a criterion's card has no use for it.
            Keysym::Return | Keysym::KP_Enter
                if ctrl && self.focus == TabArea::Findings && criteria > 0 =>
            {
                return None;
            }
            Keysym::Return | Keysym::KP_Enter => {
                if self.focus == TabArea::Stages {
                    return page_enter(feature, stage, ctx);
                }
                if !self.expanded {
                    self.toggle_repos();
                    return None;
                }
                let picked = entry.and_then(|e| {
                    let mut buttons = entry_buttons(feature, &self.sources, e);
                    let at = self.picked(feature, e)?;
                    (at < buttons.len()).then(|| buttons.swap_remove(at).1)
                });
                return picked.or_else(|| row_action(feature, row, ctx));
            }
            Keysym::Prior | Keysym::KP_Prior | Keysym::Next | Keysym::KP_Next
                if ctrl && !mods.shift =>
            {
                let back = matches!(sym, Keysym::Prior | Keysym::KP_Prior);
                return switch(slugs, &self.slug, back).map(PanelOutcome::Show);
            }
            Keysym::o | Keysym::O if ctrl && mods.shift => {
                return row_page(feature, row).or_else(|| {
                    opens_fix_plan(feature, stage).then_some(PanelOutcome::ReviewFixPlan)
                });
            }
            Keysym::o | Keysym::O if ctrl => return Some(PanelOutcome::Open(feature.dir.clone())),
            Keysym::c | Keysym::C if ctrl => return Some(PanelOutcome::CopyPath),
            Keysym::r | Keysym::R if ctrl && mods.shift => {
                return agent_key(feature, stage, ctx).map(|(_, outcome)| outcome);
            }
            Keysym::r | Keysym::R if ctrl => {
                return fresh_stage(feature, row)
                    .map(|stage| PanelOutcome::StartAgent(stage, true));
            }
            Keysym::n | Keysym::N if ctrl => return Some(PanelOutcome::New),
            Keysym::a | Keysym::A if ctrl && mods.shift && needs(stage).is_some() => {
                return Some(PanelOutcome::CycleAgent(stage));
            }
            Keysym::i | Keysym::I if ctrl && mods.shift && needs(stage).is_some() => {
                return Some(PanelOutcome::PickModel(stage));
            }
            Keysym::b | Keysym::B if ctrl && mods.shift && milestone_switch(feature, stage) => {
                return Some(PanelOutcome::FreshMilestones(!feature.fresh_milestones));
            }
            Keysym::l | Keysym::L if ctrl && mods.shift => self.toggle_repos(),
            Keysym::u | Keysym::U if ctrl && mods.shift && offers_setup(feature, stage) => {
                return Some(PanelOutcome::SetUp);
            }
            Keysym::i | Keysym::I if ctrl && mods.shift && offers_setup_log(feature, stage) => {
                return Some(PanelOutcome::SetupLog);
            }
            Keysym::e | Keysym::E
                if ctrl
                    && !mods.shift
                    && (offers_setup(feature, stage) || offers_sandbox(feature, stage)) =>
            {
                return Some(PanelOutcome::EditCommands(commands_tab(stage)));
            }
            Keysym::t | Keysym::T if ctrl && mods.shift && offers_sandbox(feature, stage) => {
                let running = ctx.sandbox_busy || sandbox_alive(feature);
                return Some(match running {
                    true => PanelOutcome::StopSandbox,
                    false => PanelOutcome::StartSandbox,
                });
            }
            Keysym::y | Keysym::Y
                if ctrl
                    && mods.shift
                    && offers_sandbox(feature, stage)
                    && sandbox_alive(feature) =>
            {
                return Some(PanelOutcome::OpenSandbox);
            }
            Keysym::g | Keysym::G
                if ctrl
                    && mods.shift
                    && offers_sandbox(feature, stage)
                    && sandbox_alive(feature) =>
            {
                return Some(PanelOutcome::RestartSandbox);
            }
            Keysym::x | Keysym::X if ctrl && mods.shift && offers_sandbox(feature, stage) => {
                match (sandbox_alive(feature), feature.sandbox.is_some()) {
                    (true, _) => return Some(PanelOutcome::ResetSandbox),
                    (false, true) if !ctx.sandbox_busy => {
                        return Some(PanelOutcome::RemoveSandbox);
                    }
                    _ => {}
                }
            }
            Keysym::k | Keysym::K if ctrl && mods.shift && plan_review_open(feature, stage) => {
                return Some(PanelOutcome::SkipPlanReview);
            }
            Keysym::d | Keysym::D
                if ctrl
                    && mods.shift
                    && plan_review_open(feature, stage)
                    && feature.plan_review.as_ref().is_some_and(|r| r.writing) =>
            {
                return Some(PanelOutcome::ChangeDecisions);
            }
            Keysym::Delete | Keysym::KP_Delete if ctrl && mods.shift => {
                if fix_round_on(feature, stage).is_some_and(FixRound::before_fixing) {
                    return Some(PanelOutcome::DropFixPlan);
                }
                return start_over_stage(feature, stage, ctx).map(PanelOutcome::StartOver);
            }
            Keysym::Delete | Keysym::KP_Delete if plain && self.focus == TabArea::Repos => {
                return self.delete_row(feature, entry?, armed);
            }
            Keysym::Delete | Keysym::KP_Delete if plain => return Some(PanelOutcome::Remove),
            _ => {}
        }
        None
    }
}

/// A move of the feature tab's keys, through its stages or its
/// repositories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Back,
    On,
    First,
    Last,
}

impl Step {
    /// Where it goes from `at`, of `0..=last`: it stops at the ends.
    fn from(self, at: usize, last: usize) -> usize {
        match self {
            Self::Back => at.saturating_sub(1),
            Self::On => (at + 1).min(last),
            Self::First => 0,
            Self::Last => last,
        }
    }
}

/// Where `stage` is in `Stage::ALL`.
fn stage_index(stage: Stage) -> usize {
    Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0)
}

/// What Enter does on `stage`'s page of `feature`: its primary button,
/// else, on a later stage, says which comes first. On a done
/// stage it goes to the current one: Reopen has its own key.
fn page_enter(feature: &Feature, stage: Stage, ctx: RowCtx) -> Option<PanelOutcome> {
    page_actions(feature, stage, ctx)
        .into_iter()
        .find(|(b, _)| b.style == ButtonStyle::Primary)
        .map(|(_, outcome)| outcome)
        .or_else(|| row_action(feature, PanelRow::Stage(stage), ctx))
        .filter(|outcome| !matches!(outcome, PanelOutcome::Untick(_)))
}

/// Whether `stage` is the Plan review `feature` is at: what can be skipped.
fn plan_review_open(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::PlanReview && feature.status == Status::Open && feature.current() == Some(stage)
}

/// The key of a done stage's Reopen.
const REOPEN_KEY: &str = "Ctrl+Shift+Enter";
/// The key of the current stage's Start over.
const START_OVER_KEY: &str = "Ctrl+Shift+Del";

/// The open fix round of `feature`, on the page of the stage that asked.
fn fix_round_on(feature: &Feature, stage: Stage) -> Option<&FixRound> {
    (feature.status == Status::Open)
        .then_some(feature.fix_round.as_ref())
        .flatten()
        .filter(|r| r.stage == stage)
}

/// The last fix round of `feature`, its fixes sent, on the page of the stage
/// that asked, or of an earlier one the feature came back to.
fn last_round_on(feature: &Feature, stage: Stage) -> Option<&FixRound> {
    (feature.status == Status::Open && feature.fix_round.is_none())
        .then_some(feature.last_fix_round.as_ref())
        .flatten()
        .filter(|r| r.stage == stage || (feature.current() == Some(stage) && stage < r.stage))
}

/// Whether `stage`'s page can send the fixes of the last round again: only
/// where the feature is at.
fn makes_fixes_again(feature: &Feature, stage: Stage) -> bool {
    last_round_on(feature, stage).is_some() && feature.current() == Some(stage)
}

/// Whether `stage`'s page opens a fix plan already sent: its Enter does
/// other things.
fn opens_fix_plan(feature: &Feature, stage: Stage) -> bool {
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
        _ => {}
    }
    if let Some((label, outcome)) = agent_button(feature, Stage::Build, ctx) {
        let style = match out.is_empty() {
            true => ButtonStyle::Primary,
            false => ButtonStyle::Plain,
        };
        out.push((button(label, style), outcome));
    }
    if !round.before_fixing() {
        out.push((
            button("Open", ButtonStyle::Plain),
            PanelOutcome::ReviewFixPlan,
        ));
    }
    if round.before_fixing() {
        out.push((
            button("Drop plan\u{2026}", ButtonStyle::Quiet),
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
    let mark = stage_mark(feature, stage);
    if mark == StageMark::Later {
        return review_actions(feature, stage);
    }
    let button = |label: &str, style| PageButton {
        label: label.to_owned(),
        style,
    };
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
    if last_round_on(feature, stage).is_some() {
        out.push((
            button("Open", ButtonStyle::Plain),
            PanelOutcome::ReviewFixPlan,
        ));
        if makes_fixes_again(feature, stage) {
            out.push((
                button("Make fixes again", ButtonStyle::Plain),
                PanelOutcome::MakeFixes,
            ));
        }
    }
    let plan_fixes = stage == Stage::Verify
        && mark == StageMark::Current
        && feature.fix_round.is_none()
        && !checks_running(feature)
        && (feature.verify.as_ref()).is_some_and(|v| feature::verify::to_fix(v).is_some());
    if plan_fixes {
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
            _ if back == Some(Rework::Waits) || round.is_some() || plan_fixes => ButtonStyle::Plain,
            _ => ButtonStyle::Primary,
        };
        out.push((button(label, style), main));
    }
    if tries_criteria(feature, stage) && ctx.criteria > 0 {
        use crate::manual_test::Outcome;
        // What the fix plan or Build waits for comes first.
        let free = back != Some(Rework::Waits) && round.is_none();
        let style = |next: bool| match free && next {
            true => ButtonStyle::Primary,
            false => ButtonStyle::Plain,
        };
        if ctx.manual.failed > ctx.manual.sent {
            out.push((
                button("Plan fixes", style(true)),
                PanelOutcome::Manual(Outcome::SendFailures),
            ));
        }
        out.push((
            button("Mark passed", style(ctx.manual.passed == ctx.criteria)),
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
    if back.is_some()
        && let Some((label, outcome)) =
            agent_button(feature, Stage::Build, ctx).filter(|(_, o)| !has(&out, o))
    {
        out.push((button(label, ButtonStyle::Plain), outcome));
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
    if plan_review_open(feature, stage) {
        let suggestions = PanelOutcome::PlanSuggestions;
        let decided = crate::feature_plan_review::deciding(feature).is_some();
        if decided && !has(&out, &suggestions) {
            out.push((button("Decide\u{2026}", ButtonStyle::Plain), suggestions));
        }
        if feature.plan_review.as_ref().is_some_and(|r| r.writing) {
            let label = "Change decisions\u{2026}";
            out.push((
                button(label, ButtonStyle::Plain),
                PanelOutcome::ChangeDecisions,
            ));
        }
        let skip = PanelOutcome::SkipPlanReview;
        out.push((button("Skip review", ButtonStyle::Plain), skip));
    }
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
        out.push((button(label, ButtonStyle::Quiet), PanelOutcome::SetUp));
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
    if offers_sandbox(feature, stage) {
        if ctx.sandbox_busy {
            out.push((
                button("Stop", ButtonStyle::Plain),
                PanelOutcome::StopSandbox,
            ));
        } else if sandbox_alive(feature) {
            out.extend([
                (
                    button("Open app", ButtonStyle::Plain),
                    PanelOutcome::OpenSandbox,
                ),
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
    }
    if offers_fix_all(feature, stage) {
        out.push((
            button("Fix all open", ButtonStyle::Plain),
            PanelOutcome::FixAllFindings,
        ));
    }
    if can_decide(feature, stage) {
        out.push((
            button("Add comment\u{2026}", ButtonStyle::Plain),
            PanelOutcome::AddComment,
        ));
    }
    let current = mark == StageMark::Current;
    out.extend(review_actions(feature, stage));
    if current && has_failed_checks(feature, stage) {
        out.push((
            button("Run failed", ButtonStyle::Plain),
            PanelOutcome::RunFailed,
        ));
    }
    if stage == Stage::Verify && checks_running(feature) {
        out.push((
            button("Stop all", ButtonStyle::Quiet),
            PanelOutcome::StopChecks,
        ));
    }
    for (i, row) in check_rows(feature, stage).iter().enumerate() {
        out.extend(check_buttons(i, row, feature, current));
    }
    if stage == Stage::Ship {
        let one = feature.shipping.prs.len() == 1;
        for pr in &feature.shipping.prs {
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
fn commands_tab(stage: Stage) -> CommandsTab {
    match stage {
        Stage::Build => CommandsTab::Setup,
        Stage::ManualTest => CommandsTab::Run,
        Stage::Verify => CommandsTab::Feature,
        _ => CommandsTab::Checks,
    }
}

/// Whether `stage`'s page of `feature` offers its sandbox: where it is tried
/// by hand, once there.
fn offers_sandbox(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::ManualTest
        && feature.status == Status::Open
        && !feature.repos.is_empty()
        && stage_mark(feature, stage) != StageMark::Later
}

/// Whether `feature`'s sandbox runs.
fn sandbox_alive(feature: &Feature) -> bool {
    feature.sandbox.as_ref().is_some_and(|run| run.alive())
}

/// Whether `stage`'s page of `feature` offers to set its worktrees up, and
/// so to edit the project's commands.
fn offers_setup(feature: &Feature, stage: Stage) -> bool {
    matches!(stage, Stage::Build | Stage::Verify)
        && feature.status == Status::Open
        && !feature.repos.is_empty()
        && stage_mark(feature, stage) != StageMark::Later
}

/// Whether `stage`'s page of `feature` offers what its setup runs and printed:
/// once one started.
fn offers_setup_log(feature: &Feature, stage: Stage) -> bool {
    offers_setup(feature, stage) && feature.setup.is_some()
}

/// The name of the feature tab's header button `i`: `FEATURE_BUTTONS`,
/// then `remove`.
fn header_label(remove: &'static str, i: usize) -> Option<&'static str> {
    chrome::FEATURE_BUTTONS
        .get(i)
        .copied()
        .or((i == chrome::FEATURE_BUTTONS.len()).then_some(remove))
}

/// The repository `rows` on from `at`, of `len`: it stops at the ends.
fn strip_step(at: usize, rows: isize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    at.min(last).saturating_add_signed(rows).min(last)
}

/// The slug before (`back`) or after `slug` in `slugs`, round the ends;
/// `None` with only one.
fn switch(slugs: &[String], slug: &str, back: bool) -> Option<String> {
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
fn pick_feature(slugs: &[String], open: Option<&str>, last: Option<&str>) -> Option<String> {
    [open, last]
        .into_iter()
        .flatten()
        .find(|s| slugs.iter().any(|l| l == s))
        .map(str::to_owned)
        .or_else(|| slugs.last().cloned())
}

/// A folder as the chrome shows it, from the home folder as `~`.
pub(crate) fn tilde(path: &Path) -> String {
    paths::display(path, home_dir().as_deref(), 60)
}

/// What a done stage's row says of its mark.
fn mark_detail(mark: &feature::Mark, now: u64) -> String {
    let when = projects::ago(now, mark.at);
    if mark.note.is_empty() {
        format!("done {when} by {}", mark.by)
    } else {
        format!("done {when} by {} \u{b7} {}", mark.by, mark.note)
    }
}

impl Gerdr {
    // ------------------------------------------------------------ state

    /// Read the features of every project from their store, as at start
    /// and when the projects' file is read again. A project whose features
    /// can't be read keeps those it had, and says so.
    pub fn load_features(&mut self) {
        let ids: Vec<(String, String)> = self
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let mut all = BTreeMap::new();
        for (id, name) in ids {
            match store::load_all(None, &id) {
                Ok(features) => {
                    all.insert(id, features);
                }
                Err(why) => {
                    tracing::warn!("reading the features of {id}: {why}");
                    self.show_error(format!("Couldn't read the features of {name}: {why}"));
                    if let Some(kept) = self.features.all.remove(&id) {
                        all.insert(id, kept);
                    }
                }
            }
        }
        self.features.all = all;
        let all = &self.features.all;
        let kept = |(project, slug): &(String, String)| {
            all.get(project)
                .is_some_and(|fs| fs.iter().any(|f| f.slug == *slug))
        };
        self.features.probes.retain(|key, _| kept(key));
        self.features.asked.retain(|key, _| kept(key));
        self.watch_features();
        // The workspaces on features get their tab, those on features gone
        // lose it, and the panel follows.
        self.prune();
        self.sync_feature_panel();
    }

    /// Feature `slug` of project `project`.
    pub fn feature(&self, project: &str, slug: &str) -> Option<&Feature> {
        self.features
            .all
            .get(project)?
            .iter()
            .find(|f| f.slug == slug)
    }

    /// Feature `slug` of project `project`, to change.
    pub(crate) fn feature_mut(&mut self, project: &str, slug: &str) -> Option<&mut Feature> {
        self.features
            .all
            .get_mut(project)?
            .iter_mut()
            .find(|f| f.slug == slug)
    }

    /// Save feature `slug` of `project` as it is now; a failure says so.
    pub(crate) fn save_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        if let Err(err) = store::save(None, feature) {
            tracing::warn!("saving feature {project}/{slug}: {err}");
            let title = feature.title.clone();
            self.show_error(format!("Couldn't save \u{201c}{title}\u{201d}: {err}"));
        }
        self.plan_sheet_follows();
        self.feature_diff_stands(project, slug);
    }

    /// The slugs of the features the panel lists for `project`.
    fn listed_slugs(&self, project: &str) -> Vec<String> {
        self.features
            .all
            .get(project)
            .map(|all| listed(all).into_iter().map(|f| f.slug.clone()).collect())
            .unwrap_or_default()
    }

    /// Where the threads send what they came to; set up the first time.
    fn feature_sender(&mut self) -> Option<channel::Sender<Done>> {
        if self.features.done.is_none() {
            let (tx, rx) = channel::channel::<Done>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg(done) => state.feature_done(done),
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.features.done = Some(tx),
                Err(err) => {
                    tracing::warn!("working on features: {err}");
                    self.show_error(format!("Couldn't start the work on features: {err}"));
                    return None;
                }
            }
        }
        self.features.done.clone()
    }

    /// Run `work` off the thread, its result sent back as a `Done`; `busy`
    /// shows in the tasks panel meanwhile.
    pub(crate) fn feature_work(
        &mut self,
        busy: Option<Busy>,
        work: impl FnOnce(channel::Sender<Done>) -> Done + Send + 'static,
    ) {
        let Some(tx) = self.feature_sender() else {
            return;
        };
        self.features.busy.extend(busy);
        std::thread::spawn(move || {
            // It fails only once the loop is gone: gerdr is quitting, and
            // nothing is left to show the result in.
            let done = work(tx.clone());
            let _ = tx.send(done);
        });
    }

    /// The end of work on a feature, the panel open or not: what it came
    /// to is kept and said.
    fn feature_done(&mut self, done: Done) {
        match done {
            Done::Sandbox {
                project,
                slug,
                run,
                stopped,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_started(&project, &slug, run, stopped);
            }
            Done::SandboxStopped {
                project,
                slug,
                what,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Sandbox);
                self.sandbox_stopped(&project, &slug, what, result);
            }
            Done::SandboxLeftovers { stacks } => self.sandbox_leftovers(stacks),
            Done::SandboxLeftoversRemoved { problems } => {
                self.sandbox_leftovers_removed(problems);
            }
            Done::CommandsFound {
                serial,
                project,
                result,
            } => self.commands_found(serial, &project, result),
            Done::Found {
                serial,
                from,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("finding where a feature goes: {why}");
                }
                // Dropped when the dialog it was for closed: a new one
                // looks again.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    if let Some(from) = from {
                        dialog.from = from;
                    }
                    dialog.found(result);
                }
            }
            Done::FoundRepos {
                project,
                slug,
                source,
                result,
            } => {
                if let Err(why) = &result {
                    let of = source.as_deref().unwrap_or(&project);
                    tracing::warn!("finding the repositories of project {of}: {why}");
                }
                let found = match result {
                    Ok(root) => Finding::Found(root),
                    Err(why) => Finding::Failed(why),
                };
                let key = (project.clone(), slug.clone());
                let open = (self.features.panel.as_mut())
                    .filter(|p| p.project == project && p.slug == slug);
                let feature = (self.features.all.get(&project))
                    .and_then(|list| list.iter().find(|f| f.slug == slug));
                if let Some(panel) = open.or_else(|| self.features.parked.get_mut(&key))
                    && let Some(feature) = feature
                    && let Some(at) = panel.sources.iter().position(|s| s.project == source)
                {
                    // New rows may come before the selected one: it stays.
                    let entries = repo_entries(feature, &panel.sources, panel.expanded);
                    let held = (entries.get(panel.repo))
                        .and_then(|e| entry_key(feature, &panel.sources, *e));
                    let pick = panel.repo_pick;
                    panel.sources[at].finding = found;
                    if let Some(key) = held {
                        panel.select_repo_keeping(feature, &key, pick);
                    }
                }
            }
            Done::FoundOther {
                serial,
                project,
                result,
            } => {
                if let Err(why) = &result {
                    tracing::warn!("finding the repositories of project {project}: {why}");
                }
                // Dropped when the dialog it was for closed, as above.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.found_other(&project, result);
                }
            }
            Done::DocsLinked { title, result } => match result {
                Ok(None) => {}
                Ok(Some(exclude)) => self.show_success(format!(
                    "Added /{} to {}",
                    layout::DOCS_LINK,
                    tilde(&exclude)
                )),
                Err(why) => {
                    tracing::warn!("linking the stage files of {title}: {why}");
                    self.show_warning(format!(
                        "Agents in \u{201c}{title}\u{201d} can't name its stage files: {why}"
                    ));
                }
            },
            Done::Graphed {
                project,
                slug,
                title,
                built,
            } => self.feature_graphs_built(&project, &slug, &title, built),
            Done::Linked {
                project,
                slug,
                names,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Link);
                self.feature_linked(&project, &slug, &names, result);
            }
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Create);
                self.feature_created(project, title, slug, root, result);
            }
            Done::Touched {
                project,
                slug,
                repo,
                result,
            } => {
                self.features.busy.retain(|b| {
                    !(b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo))
                });
                self.feature_touched(&project, &slug, &repo, result);
            }
            Done::Losses {
                project,
                slug,
                losses,
            } => {
                self.unbusy(&project, &slug, Job::Losses);
                self.ask_remove_feature(project, slug, losses);
            }
            Done::RemovalLosses {
                project,
                slug,
                keys,
                to_context,
                result,
            } => {
                self.unbusy(&project, &slug, Job::RemoveRepos);
                self.ask_remove_repos(project, slug, keys, to_context, result);
            }
            Done::ReposRemoved {
                project,
                slug,
                to_context,
                results,
            } => {
                self.unbusy(&project, &slug, Job::RemoveRepos);
                self.feature_repos_removed(&project, &slug, to_context, results);
            }
            Done::Removed {
                project,
                slug,
                abandon,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Remove);
                self.feature_removed(&project, &slug, abandon, result);
            }
            Done::Probed {
                project,
                slug,
                probe,
            } => self.feature_probed(&project, &slug, probe),
            Done::Tail { pane, lines } => self.feature_tail_read(&pane, lines),
            Done::CheckTail { pane, lines } => {
                if let Some(tail) = self.features.check_tails.get_mut(&pane) {
                    tail.read(&pane, lines);
                }
            }
            Done::T3Told { thread, result } => self.t3_told(&thread, result),
            Done::Read {
                project,
                slug,
                stage,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Read);
                self.feature_read(&project, &slug, stage, result);
            }
            Done::PlanChecked {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.plan_code_checked(&project, &slug, n, stamp, result),
            Done::ReviewRead {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.review_read(&project, &slug, n, stamp, result),
            Done::PlanReviewBaseline {
                project,
                slug,
                result,
            } => self.plan_review_baseline(&project, &slug, result),
            Done::PlanReviewRead {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.plan_review_read(&project, &slug, n, stamp, result),
            Done::Shipped {
                project,
                slug,
                patch,
                squash,
                result,
            } => {
                self.unbusy(&project, &slug, Job::Ship);
                self.feature_shipped(&project, &slug, patch.as_deref(), &squash, result);
            }
            Done::BuildChecked {
                project,
                slug,
                n,
                stamp,
                result,
            } => self.build_code_checked(&project, &slug, n, stamp, result),
            Done::Progress {
                project,
                slug,
                job,
                what,
            } => {
                if let Some(busy) = self
                    .features
                    .busy
                    .iter_mut()
                    .find(|b| b.project == project && b.slug == slug && b.job == job)
                {
                    busy.what = what;
                }
            }
            Done::SetUp {
                project,
                slug,
                report,
            } => {
                self.unbusy(&project, &slug, Job::Setup);
                self.setup_done(&project, &slug, report);
            }
            Done::Verified {
                project,
                slug,
                n,
                evidence,
            } => {
                self.unbusy(&project, &slug, Job::Verify);
                self.verify_done(&project, &slug, n, evidence);
            }
            Done::ChecksBegun {
                project,
                slug,
                n,
                begun,
            } => self.checks_begun(&project, &slug, n, begun),
            Done::CheckEnded {
                project,
                slug,
                n,
                index,
                outcome,
            } => self.check_ended(&project, &slug, n, index, outcome),
            Done::CheckTurn {
                project,
                slug,
                n,
                index,
                turn,
            } => self.check_turn(&project, &slug, n, index, turn),
        }
    }

    /// `job` on feature `slug` of `project` is over.
    pub(crate) fn unbusy(&mut self, project: &str, slug: &str, job: Job) {
        self.features
            .busy
            .retain(|b| !(b.project == project && b.slug == slug && b.job == job));
    }

    /// Whether feature `slug` of `project` has work running off the thread
    /// that other work on it must wait for.
    pub(crate) fn feature_busy(&self, project: &str, slug: &str) -> bool {
        self.features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.job != Job::Graph)
    }

    /// Feature `slug`'s work of kind `job` running off the thread.
    pub(crate) fn busy_with(&self, project: &str, slug: &str, job: Job) -> Option<&Busy> {
        self.features
            .busy
            .iter()
            .find(|b| b.project == project && b.slug == slug && b.job == job)
    }

    /// What the latest probe of feature `slug` of `project` read of its
    /// code, if it could.
    pub(crate) fn feature_reading(&self, project: &str, slug: &str) -> Option<&Reading> {
        let key = (project.to_owned(), slug.to_owned());
        let probe = self.features.probes.get(&key)?.last.as_ref()?;
        probe.reading.as_ref().ok()
    }

    /// What the latest probe of feature `slug` of `project` found of its
    /// files, if one is back.
    pub(crate) fn feature_docs(&self, project: &str, slug: &str) -> Option<&feature::page::Docs> {
        let key = (project.to_owned(), slug.to_owned());
        Some(&self.features.probes.get(&key)?.last.as_ref()?.docs)
    }

    /// The checks `feature`'s Verify page lists (see `verify::plan_cards`).
    fn planned_checks(&self, feature: &Feature) -> Vec<PlannedCheck> {
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        feature::verify::plan_cards(feature, &config)
    }

    /// What the panel's rows need of `feature` besides it.
    fn row_ctx(&self, feature: &Feature) -> RowCtx {
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        let checks = !feature::verify::checks(feature, &config).is_empty();
        let all_skipped = !checks && !feature::verify::skipped(feature, &config).is_empty();
        let criteria = self.criteria(&feature.project, &feature.slug).len();
        let manual = self.manual_counts(feature).unwrap_or_default();
        let key = (feature.project.clone(), feature.slug.clone());
        let docs = (self.features.probes.get(&key))
            .and_then(|p| p.last.as_ref())
            .map(|p| &p.docs);
        let written = Stage::ALL.map(|stage| docs.is_some_and(|d| d.written.contains(&stage)));
        RowCtx {
            checks,
            criteria,
            manual,
            written,
            fixing: self.build_agent_fixing(feature),
            sandbox_busy: (self.features.build.sandbox_stops)
                .contains_key(&(feature.project.clone(), feature.slug.clone())),
            all_skipped,
        }
    }

    // ------------------------------------------------------ patch watch

    /// Start the patch watch, once: every `PANEL_EVERY` it probes the
    /// feature shown, and the watched ones not probed for as long as
    /// `watch_every` says.
    fn watch_features(&mut self) {
        if self.features.watching {
            return;
        }
        let timer = Timer::from_duration(PANEL_EVERY);
        let inserted = self.loop_handle.insert_source(timer, |_, _, state| {
            state.feature_watch_tick();
            TimeoutAction::ToDuration(PANEL_EVERY)
        });
        match inserted {
            Ok(_) => self.features.watching = true,
            Err(err) => tracing::warn!("watching the features' code: {err}"),
        }
    }

    /// Read the shown feature's agent tail now, then every
    /// `page::TAIL_EVERY` while a feature's tab shows; the timer stops
    /// once none does.
    fn watch_feature_tail(&mut self) {
        self.feature_tail_tick();
        if self.features.tailing {
            return;
        }
        let every = feature::page::TAIL_EVERY;
        let inserted =
            self.loop_handle
                .insert_source(Timer::from_duration(every), move |_, _, state| {
                    if state.feature_tail_tick() {
                        TimeoutAction::ToDuration(every)
                    } else {
                        state.features.tailing = false;
                        TimeoutAction::Drop
                    }
                });
        match inserted {
            Ok(_) => self.features.tailing = true,
            Err(err) => tracing::warn!("reading the feature agent's pane: {err}"),
        }
    }

    /// The pane of the shown feature's current stage agent, while it is
    /// there.
    fn tail_pane(&self) -> Option<String> {
        let feature = self.panel_feature()?;
        let stage = feature.current().filter(|s| needs(*s).is_some())?;
        let pane = feature.runs.get(&stage)?.pane.clone()?;
        self.herdr_pane_place(&pane).is_some().then_some(pane)
    }

    /// Read the shown feature's agent tail off the thread, unless a read
    /// runs; with no agent pane, the tail goes. Whether a feature's tab
    /// still shows.
    fn feature_tail_tick(&mut self) -> bool {
        if self.features.panel.is_none() {
            self.features.tail = None;
            self.features.check_tails.clear();
            return false;
        }
        self.check_tails_tick();
        let Some(pane) = self.tail_pane() else {
            self.features.tail = None;
            return true;
        };
        let tail = self.features.tail.get_or_insert_with(AgentTail::default);
        if tail.pane != pane {
            *tail = AgentTail {
                pane: pane.clone(),
                ..AgentTail::default()
            };
        }
        if tail.reading {
            return true;
        }
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return true;
        };
        tail.reading = true;
        self.feature_work(None, move |_| Done::Tail {
            lines: feature::page::read_tail(socket, &pane),
            pane,
        });
        true
    }

    /// Read off the thread the panes of the checks running on the page
    /// shown, each unless a read of it runs. A check that ended takes its
    /// tail along: its card has what it printed by then.
    fn check_tails_tick(&mut self) {
        let panes: Vec<String> = (self.panel_feature().zip(self.features.panel.as_ref()))
            .filter(|(feature, panel)| panel.page_stage(feature) == Stage::Verify && !panel.keys)
            .map(|(feature, _)| {
                (check_rows(feature, Stage::Verify).iter())
                    .filter(|row| row.outcome.is_none() && checks_running(feature))
                    .filter_map(|row| row.pane.clone())
                    .filter(|pane| self.herdr_pane_place(pane).is_some())
                    .collect()
            })
            .unwrap_or_default();
        (self.features.check_tails).retain(|pane, _| panes.contains(pane));
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return;
        };
        for pane in panes {
            let tail = (self.features.check_tails.entry(pane.clone())).or_default();
            if tail.reading {
                continue;
            }
            tail.pane = pane.clone();
            tail.reading = true;
            let socket = socket.clone();
            self.feature_work(None, move |_| Done::CheckTail {
                lines: feature::page::read_check_tail(socket, &pane),
                pane,
            });
        }
    }

    /// A read of pane `pane`'s tail is back. One for a pane no longer the
    /// tail's (the agent or the tab changed meanwhile) is dropped: the
    /// tail of the new one is read on its own.
    fn feature_tail_read(&mut self, pane: &str, lines: Result<Vec<String>, String>) {
        let Some(tail) = self.features.tail.as_mut().filter(|t| t.pane == pane) else {
            return;
        };
        tail.read(pane, lines);
    }

    /// Probe what is due.
    fn feature_watch_tick(&mut self) {
        self.plan_sheet_follows();
        let mut due = Vec::new();
        if let Some(panel) = &self.features.panel {
            due.push((panel.project.clone(), panel.slug.clone(), PANEL_EVERY));
        }
        // A file's tab says whether its stage is locked by now.
        if let Some(doc) = (self.tab().and_then(|t| t.review())).and_then(|r| r.wd.doc.as_ref()) {
            due.push((doc.project.clone(), doc.slug.clone(), PANEL_EVERY));
        }
        for (project, all) in &self.features.all {
            for feature in all.iter().filter(|f| watched(f)) {
                due.push((project.clone(), feature.slug.clone(), watch_every(feature)));
            }
        }
        let now = Instant::now();
        // The timer's own jitter doesn't skip a round.
        let slack = Duration::from_secs(1);
        for (project, slug, every) in due {
            let probed = self.features.probes.get(&(project.clone(), slug.clone()));
            let fresh = probed
                .and_then(|p| p.at)
                .is_some_and(|at| now.duration_since(at) + slack < every);
            if !fresh && !probed.is_some_and(|p| p.running) {
                self.probe_feature(&project, &slug);
            }
        }
    }

    /// Probe open feature `slug` of `project` off the thread (see
    /// `ship::probe`); one running already goes again once it is back.
    pub(crate) fn probe_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        // The brief may have changed: its criteria are read again. A finished
        // feature's page shows them too.
        self.load_criteria(project, slug);
        if feature.status != Status::Open {
            self.probe_finished(project, slug, &feature);
            return;
        }
        if self.feature_sender().is_none() {
            return;
        }
        let modes = self.projects.workflows(&feature, |w| w.ship);
        let probed = self
            .features
            .probes
            .entry((project.to_owned(), slug.to_owned()))
            .or_default();
        if probed.running {
            probed.again = true;
            return;
        }
        probed.running = true;
        probed.at = Some(Instant::now());
        let data = store::dir(None, project, slug);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::Probed {
            probe: Probe {
                docs: feature::page::read_docs(&data),
                models: feature::agents::read_models(&feature),
                ..ship::probe(&feature, &modes)
            },
            project,
            slug,
        });
    }

    /// Read finished `feature` (`slug` of `project`) for its tab: its
    /// stage files and line counts only (`ship::probe_finished`). Its code
    /// isn't watched any more, nor its hooks made sure of.
    fn probe_finished(&mut self, project: &str, slug: &str, feature: &Feature) {
        if self.feature_sender().is_none() {
            return;
        }
        let probed = self
            .features
            .probes
            .entry((project.to_owned(), slug.to_owned()))
            .or_default();
        if probed.running {
            return;
        }
        probed.running = true;
        probed.at = Some(Instant::now());
        let data = store::dir(None, project, slug);
        let (project, slug, feature) = (project.to_owned(), slug.to_owned(), feature.clone());
        self.feature_work(None, move |_| Done::Probed {
            probe: Probe {
                docs: feature::page::read_docs(&data),
                models: feature::agents::read_models(&feature),
                ..ship::probe_finished(&feature)
            },
            project,
            slug,
        });
    }

    /// A probe of feature `slug` of `project` is back: its code changed
    /// sends it back, its squash merged makes it done.
    fn feature_probed(&mut self, project: &str, slug: &str, mut probe: Probe) {
        self.feature_models_are(project, slug, std::mem::take(&mut probe.models));
        let key = (project.to_owned(), slug.to_owned());
        let open = (self.feature(project, slug)).is_some_and(|f| f.status == Status::Open);
        let probed = self.features.probes.entry(key).or_default();
        probed.running = false;
        let docs = std::mem::take(&mut probe.docs);
        probe.docs = docs.keeping(probed.last.as_ref().map(|last| &last.docs));
        // A finished feature's read is for its tab only.
        if !open {
            probed.again = false;
            probed.last = Some(probe);
            return;
        }
        let again = std::mem::take(&mut probed.again);
        match &probe.reading {
            Err(why) if probed.logged.as_ref() != Some(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                probed.logged = Some(why.clone());
            }
            Err(_) => {}
            Ok(_) => probed.logged = None,
        }
        let patch = probe.reading.as_ref().ok().map(|r| r.patch.clone());
        let merged = probe.merged;
        probed.last = Some(probe);
        if let Some(patch) = &patch {
            self.feature_code_is(project, slug, patch);
            self.feature_criteria_are(project, slug, patch);
        }
        if merged {
            self.feature_merged(project, slug, patch.as_deref());
        }
        if again {
            self.probe_feature(project, slug);
        }
    }

    /// The models feature `slug`'s agents use, as last read: kept with their
    /// runs. A run started since the read isn't the one it was of.
    fn feature_models_are(
        &mut self,
        project: &str,
        slug: &str,
        models: BTreeMap<Stage, (u32, String)>,
    ) {
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let mut changed = false;
        for (stage, (n, model)) in models {
            if let Some(run) = f.runs.get_mut(&stage).filter(|r| r.n == n)
                && run.model.as_ref() != Some(&model)
            {
                run.model = Some(model);
                changed = true;
            }
        }
        if changed {
            self.save_feature(project, slug);
        }
    }

    /// The criteria of feature `slug` are as last read from its brief, on
    /// its code (`patch`'s): Manual test, passed on its page, is to do
    /// again when one of them was added or reworded since, and the toast
    /// says so. A Manual test ticked plainly (no results kept) stays.
    fn feature_criteria_are(&mut self, project: &str, slug: &str, patch: &str) {
        let criteria = self.criteria(project, slug).to_vec();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let reopen = feature.status == Status::Open
            && feature.marks.contains_key(&Stage::ManualTest)
            && feature.manual.is_some()
            && !criteria.is_empty()
            && !feature::manual::all_passed(feature.manual.as_ref(), &criteria, Some(patch));
        if !reopen || !feature.untick(Stage::ManualTest, feature::now()) {
            return;
        }
        let title = feature.title.clone();
        tracing::info!("criteria of {project}/{slug} changed: Manual test to do again");
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_warning(format!(
            "The acceptance criteria of \u{201c}{title}\u{201d} changed: back to Manual test"
        ));
    }

    /// The code of feature `slug` is `patch`'s now: the marks that no
    /// longer hold go, and the toast says so.
    pub(crate) fn feature_code_is(&mut self, project: &str, slug: &str, patch: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.invalidate(patch, feature::now()) else {
            return;
        };
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        self.show_warning(changed_text(&title, stage));
    }

    /// The squash of feature `slug` is merged: Ship is done.
    fn feature_merged(&mut self, project: &str, slug: &str, patch: Option<&str>) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let squashing = feature.status == Status::Open
            && feature.current() == Some(Stage::Ship)
            && feature.shipping.squash_at.is_some();
        if !squashing {
            return;
        }
        feature.tick(feature::now(), "gerdr", patch);
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        let mut text = format!("\u{201c}{title}\u{201d} is merged: done");
        if let Some(kept) = self.close_shipped_workspace(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_success(text);
    }

    /// Read the code of feature `slug` off the thread for run `n` of its
    /// Plan agent, whose file has `stamp` (see `plan_code_checked`).
    pub(crate) fn check_plan_code(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: crate::feature_agents::Stamp,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::PlanChecked {
            result: patch::read(&feature),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// Mark bound stage `stage` of feature `slug` done once its code is
    /// read off the thread (see `feature_read`).
    pub(crate) fn read_and_tick(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Read,
            repo: None,
            what: format!("Checking \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::Read {
            result: patch::read(&feature),
            project,
            slug,
            stage,
        });
    }

    /// The code of feature `slug` is read to mark `stage` done: done on
    /// that code, unless it has changes not committed, or `stage` is no
    /// longer the current one. AI review is marked by gerdr, once no
    /// finding holds up.
    fn feature_read(
        &mut self,
        project: &str,
        slug: &str,
        stage: Stage,
        result: Result<Reading, String>,
    ) {
        let reading = match result {
            Ok(reading) => reading,
            Err(why) => {
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't mark {} done: {why}", stage.label()));
            }
        };
        if !reading.dirty.is_empty() {
            return self.show_warning(format!(
                "Commit the changes in {} first",
                reading.dirty.join(", ")
            ));
        }
        self.feature_code_is(project, slug, &reading.patch);
        let still = self
            .feature(project, slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage));
        let review = self.feature(project, slug).and_then(|f| f.review.as_ref());
        let read_in = review.is_some();
        let open = review.map_or(0, |r| r.open());
        match stage {
            _ if !still => {}
            Stage::AiReview if !read_in => {
                self.show_warning("AI review has no findings read in yet".into());
            }
            Stage::AiReview if open > 0 => {
                let title = self.feature(project, slug).map_or("", |f| f.title.as_str());
                let text = format!(
                    "AI review of \u{201c}{title}\u{201d} still has {}",
                    plural(open, "finding")
                );
                self.show_warning(text);
            }
            Stage::AiReview => self.tick_feature(project, slug, Some(&reading.patch), "gerdr"),
            Stage::ManualTest if !self.criteria(project, slug).is_empty() => {
                let criteria = self.criteria(project, slug).to_vec();
                let passed = self.feature(project, slug).is_some_and(|f| {
                    feature::manual::all_passed(f.manual.as_ref(), &criteria, Some(&reading.patch))
                });
                if !passed {
                    self.show_warning(
                        "The code changed since the manual test; try the criteria again".into(),
                    );
                } else {
                    self.tick_feature(project, slug, Some(&reading.patch), "you");
                    let n = criteria.len();
                    if let Some(f) = self.feature_mut(project, slug) {
                        let all = if n == 1 {
                            "its 1 criterion".to_owned()
                        } else {
                            format!("all {n} criteria")
                        };
                        f.log(feature::now(), format!("Manual test passed: {all}"));
                    }
                    self.save_feature(project, slug);
                    tracing::info!("manual test of {project}/{slug} passed: {n} criteria");
                }
            }
            Stage::Verify => {
                // Marked by hand, as when no checks are set: AI review
                // starts as on a pass, its last run's findings checked again.
                self.tick_feature(project, slug, Some(&reading.patch), "you");
                let reviewing = self.feature(project, slug).is_some_and(|f| {
                    f.status == Status::Open && f.current() == Some(Stage::AiReview)
                });
                if reviewing {
                    self.launch_stage_agent(project, slug, Stage::AiReview, &[]);
                }
            }
            _ => self.tick_feature(project, slug, Some(&reading.patch), "you"),
        }
        self.probe_feature(project, slug);
    }

    /// Mark the current stage of feature `slug` done by `by` ("you",
    /// "gerdr"), on the code whose patch-id is `patch`, and say so.
    fn tick_feature(&mut self, project: &str, slug: &str, patch: Option<&str>, by: &str) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(stage) = feature.tick(feature::now(), by, patch) else {
            return self.show_warning("Nothing left to mark done".into());
        };
        let shipped = feature.status == Status::Done;
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.show_success(if shipped {
            format!("\u{201c}{title}\u{201d} is done")
        } else if stage == Stage::AiReview {
            "AI review done: Code review is next".to_owned()
        } else {
            format!("{} marked done", stage.label())
        });
        self.feature_cursor_to_current_of(project, slug);
        self.feature_diff_changed(project, slug);
    }

    /// The tasks running on features, for the tasks panel.
    pub fn feature_tasks(&self) -> Vec<Task> {
        self.features
            .busy
            .iter()
            .map(|b| {
                let project = self
                    .projects
                    .project(&b.project)
                    .map_or(b.project.as_str(), |p| p.name.as_str());
                let title = self.feature(&b.project, &b.slug).map(|f| f.title.as_str());
                task_of(b, title, project)
            })
            .collect()
    }

    // ----------------------------------------------------------- dialog

    /// Open the new feature dialog for the active project, offering all
    /// its repositories: the one workspace `at` is in, else the active
    /// one's, starts ticked.
    pub fn open_new_feature(&mut self, at: Option<usize>) {
        let Some(id) = self.project.clone() else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let Some(project) = self.projects.project(&id) else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let workspace_dirs: Vec<PathBuf> =
            project.workspaces.iter().map(|w| w.dir.clone()).collect();
        let from = at
            .and_then(|i| self.workspaces.get(i))
            .or_else(|| self.workspace())
            .map(|ws| ws.dir.clone())
            .or_else(|| workspace_dirs.first().cloned());
        let Some(from) = from else {
            return self.show_warning("Add a workspace with a folder to start a feature".into());
        };
        self.features.serial += 1;
        let serial = self.features.serial;
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.dialog = Some(NewFeatureDialog::new(id.clone(), serial, from.clone()));
        self.find_home_root(&id, from, serial);
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Find project `id`'s root from folder `from` off the thread, for the
    /// dialog of `serial`.
    fn find_home_root(&mut self, id: &str, from: PathBuf, serial: u64) {
        let Some(project) = self.projects.project(id) else {
            return;
        };
        let (workspace_dirs, ignore, bases) = root_search(project);
        let others = self.other_projects_dirs(id);
        self.feature_work(None, move |_| Done::Found {
            serial,
            from: layout::main_top(&from),
            result: layout::find_project_root(&from, &workspace_dirs, &others, &ignore, &bases),
        });
    }

    /// Read off the thread (checklist D3) which repositories the feature
    /// panel can offer: those of its project, and of the projects it links
    /// for what it holds or held of them. What was found before stays until
    /// the new read is in. A linked project with no workspace folder has
    /// none to find. A shipped feature's
    /// repositories can't change: it looks nowhere.
    pub(crate) fn find_repo_sources(&mut self) {
        let Some(panel) = self.features.panel.as_ref() else {
            return;
        };
        let (project, slug) = (panel.project.clone(), panel.slug.clone());
        let Some(feature) = self.feature(&project, &slug).filter(|f| linkable(f)) else {
            return;
        };
        let home_dir = feature.dir.clone();
        let known = |source: &Option<String>| {
            (panel.sources.iter())
                .find(|s| s.project == *source)
                .map_or(Finding::Looking, |s| s.finding.clone())
        };
        let mut sources = Vec::new();
        let mut finds = Vec::new();
        // The home project first, whatever its place in the store.
        let (home, others): (Vec<_>, Vec<_>) = (self.projects.projects.iter())
            .filter(|p| p.id == project || feature.link(&p.id).is_some())
            .partition(|p| p.id == project);
        for p in home.into_iter().chain(others) {
            let is_home = p.id == project;
            let search = root_search(p);
            let from = match is_home {
                true => Some(home_dir.clone()),
                false => search.0.first().cloned(),
            };
            let Some(from) = from else { continue };
            let source = (!is_home).then(|| p.id.clone());
            sources.push(RepoSource {
                finding: known(&source),
                project: source.clone(),
                name: p.name.clone(),
            });
            finds.push((source, from, search, self.other_projects_dirs(&p.id)));
        }
        if let Some(panel) = self.features.panel.as_mut() {
            panel.sources = sources;
        }
        for (source, from, (dirs, ignore, bases), others) in finds {
            let (project, slug) = (project.clone(), slug.clone());
            self.feature_work(None, move |_| Done::FoundRepos {
                project,
                slug,
                source,
                result: layout::find_project_root(&from, &dirs, &others, &ignore, &bases),
            });
        }
    }

    /// The workspace folders of every project but `id`, whose repositories
    /// are theirs.
    pub(crate) fn other_projects_dirs(&self, id: &str) -> Vec<PathBuf> {
        (self.projects.projects.iter())
            .filter(|p| p.id != id)
            .flat_map(|p| p.workspaces.iter().map(|w| w.dir.clone()))
            .collect()
    }

    /// The repository `key` as the feature panel's sources found it: its
    /// source, the root holding it, and where it is.
    fn offered_repo(&self, key: &str) -> Option<(RepoSource, Found)> {
        let panel = self.features.panel.as_ref()?;
        panel.sources.iter().find_map(|source| {
            let Finding::Found(root) = &source.finding else {
                return None;
            };
            let found = (root.repos.iter())
                .find(|r| feature::repo_key(source.project.as_deref(), &r.name) == key)?;
            Some((source.clone(), found.clone()))
        })
    }

    /// Why feature `slug` can't change its repositories now, said; `false`
    /// when it can.
    fn repos_locked(&mut self, project: &str, slug: &str) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            self.show_warning("That feature is gone".into());
            return true;
        };
        let title = feature.title.clone();
        if !linkable(feature) {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} has shipped; its repositories can't change now"
            ));
            return true;
        }
        if self.feature_busy(project, slug) {
            self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
            return true;
        }
        false
    }

    /// Make feature `slug` hold repository `key` on its branch: a new
    /// worktree from its base. Taken
    /// out before, it gets a new branch.
    fn add_repo_to_feature(&mut self, project: &str, slug: &str, key: &str) {
        if self.repos_locked(project, slug) {
            return;
        }
        let Some((source, found)) = self.offered_repo(key) else {
            return self.show_warning(format!(
                "{key} wasn't found among the projects' repositories"
            ));
        };
        let Finding::Found(root) = source.finding else {
            return;
        };
        let add = AddSpec {
            project: source.project,
            name: source.name,
            root,
            names: vec![found.name],
        };
        self.link_repos(project.to_owned(), slug.to_owned(), vec![add]);
    }

    /// Let agents of feature `slug` read repository `key`'s main checkout:
    /// no git, no worktree. An agent running keeps what it started with,
    /// which the toast says.
    fn add_context_repo(&mut self, project: &str, slug: &str, key: &str) {
        if self.repos_locked(project, slug) {
            return;
        }
        let Some((source, found)) = self.offered_repo(key) else {
            return self.show_warning(format!(
                "{key} wasn't found among the projects' repositories"
            ));
        };
        let repo = feature::ContextRepo {
            name: found.name,
            main: found.main,
            project: source.project,
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        feature.log(feature::now(), format!("added {key} as context"));
        feature.add_context(repo);
        self.save_feature(project, slug);
        self.select_repo(project, slug, key);
        self.show_success(context_text(&self.repo_name(project, slug, key)));
    }

    /// Stop offering repository `key` to agents of feature `slug`: nothing
    /// is lost, and the toast can undo it (checklist C4).
    fn drop_context_repo(&mut self, project: &str, slug: &str, key: &str) {
        if self.repos_locked(project, slug) {
            return;
        }
        let named = self.repo_name(project, slug, key);
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(repo) = feature.context.iter().find(|c| c.key() == key).cloned() else {
            return;
        };
        let now = feature::now();
        feature.context.retain(|c| c.key() != key);
        feature.note_removed(repo.project.clone(), repo.name.clone(), now);
        feature.log(now, format!("removed {key} from context"));
        let title = feature.title.clone();
        self.save_feature(project, slug);
        self.select_repo(project, slug, key);
        let undo = MenuAction::RestoreContextRepo {
            project: project.to_owned(),
            slug: slug.to_owned(),
            repo: Box::new(repo),
        };
        self.toast_with(
            format!("\u{201c}{title}\u{201d} no longer has {named} as context"),
            Level::Success,
            Some(("Undo".to_owned(), undo)),
        );
    }

    /// Undo `drop_context_repo`.
    pub(crate) fn restore_context_repo(
        &mut self,
        project: &str,
        slug: &str,
        repo: feature::ContextRepo,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let key = repo.key();
        // Added to the feature since: it holds it, which is more.
        if feature.repo(&key).is_some() {
            return;
        }
        feature.log(feature::now(), format!("added {key} as context"));
        feature.add_context(repo);
        self.save_feature(project, slug);
        self.select_repo(project, slug, &key);
    }

    /// The row of repository `key` takes the keyboard on the panel of
    /// feature `slug`, wherever it went: it moved group.
    fn select_repo(&mut self, project: &str, slug: &str, key: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let open =
            (self.features.panel.as_mut()).filter(|p| p.project == project && p.slug == slug);
        if let Some(panel) = open {
            panel.select_repo(&feature, key);
        }
    }

    /// List the store's other projects in the dialog's picker, once, and
    /// find each one's root off the thread (checklist D3), as the home
    /// one's is: from its first workspace's folder.
    fn find_others(&mut self) {
        let Some(dialog) = self.features.dialog.as_ref() else {
            return;
        };
        if !dialog.picking || !dialog.others.is_empty() {
            return;
        }
        let (home, serial) = (dialog.project.clone(), dialog.serial);
        let mut others = Vec::new();
        let mut finds = Vec::new();
        for p in self.projects.projects.iter().filter(|p| p.id != home) {
            let (dirs, ignore, bases) = root_search(p);
            let root = match dirs.first() {
                Some(from) => {
                    let theirs = self.other_projects_dirs(&p.id);
                    finds.push((
                        p.id.clone(),
                        from.clone(),
                        dirs.clone(),
                        theirs,
                        ignore,
                        bases,
                    ));
                    Finding::Looking
                }
                None => Finding::Failed("it has no workspace with a folder".into()),
            };
            others.push(Other {
                project: p.id.clone(),
                name: p.name.clone(),
                root,
                linked: false,
                touched: Vec::new(),
            });
        }
        if let Some(dialog) = self.features.dialog.as_mut() {
            dialog.others = others;
        }
        for (project, from, dirs, theirs, ignore, bases) in finds {
            self.feature_work(None, move |_| Done::FoundOther {
                serial,
                project,
                result: layout::find_project_root(&from, &dirs, &theirs, &ignore, &bases),
            });
        }
    }

    /// How many repositories the dialog shows at once.
    fn new_feature_fit(&self) -> usize {
        let Some(dialog) = &self.features.dialog else {
            return 1;
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .rows
            .len()
    }

    /// A key while the new feature dialog has the keyboard.
    pub fn new_feature_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<DialogOutcome> {
        let fit = self.new_feature_fit();
        let outcome = self.features.dialog.as_mut()?.key(mods, sym, typed, fit);
        self.find_others();
        outcome
    }

    /// Do what the new feature dialog asked: create the feature off the
    /// thread, shown in the tasks panel meanwhile.
    pub fn apply_new_feature(&mut self, outcome: DialogOutcome) {
        let Some(dialog) = self.features.dialog.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        let (title, root, touched, links) = match outcome {
            DialogOutcome::Cancel => return,
            DialogOutcome::Create {
                title,
                root,
                touched,
                links,
            } => (title, root, touched, links),
        };
        let project = dialog.project;
        let all = self
            .features
            .all
            .get(&project)
            .map_or(&[][..], Vec::as_slice);
        let slug = new_slug(&title, all);
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Create,
            repo: None,
            what: format!("Creating \u{201c}{title}\u{201d}"),
        };
        self.feature_work(Some(busy), move |_| {
            let result = layout::create_spanning(&root, &slug, &touched, &links);
            Done::Created {
                project,
                title,
                slug,
                root,
                result,
            }
        });
    }

    /// A feature's worktrees were made, or not: keep it, show it, and say
    /// what was changed outside its folder.
    fn feature_created(
        &mut self,
        project: String,
        title: String,
        slug: String,
        root: Root,
        result: Result<(Created, Vec<(feature::Link, Created)>), String>,
    ) {
        let (created, linked) = match result {
            Ok(made) => made,
            Err(why) => {
                tracing::warn!("creating feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't create \u{201c}{title}\u{201d}: {why}"));
            }
        };
        // A slug used before: this folder has no link yet.
        self.features.docs_linked.remove(&created.dir);
        let mut ignores = created.ignores.clone();
        ignores.extend(linked.iter().flat_map(|(_, c)| c.ignores.iter().cloned()));
        let text = created_text(&title, &root.path, &ignores);
        let now = feature::now();
        let mut repos = created.repos;
        let mut links = Vec::new();
        for (link, made) in linked {
            links.push(link);
            repos.extend(made.repos);
        }
        let feature = Feature {
            version: feature::VERSION,
            slug: slug.clone(),
            title: title.clone(),
            project: project.clone(),
            created: now,
            root: root.path,
            dir: created.dir,
            multi: root.multi,
            repos,
            links,
            status: Status::Open,
            marks: BTreeMap::new(),
            run_counts: BTreeMap::new(),
            context: Vec::new(),
            removed: Vec::new(),
            history: vec![Event {
                at: now,
                what: "created".into(),
            }],
            shipping: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            fix_rounds: 0,
        };
        self.features
            .all
            .entry(project.clone())
            .or_default()
            .push(feature);
        self.save_feature(&project, &slug);
        if self.project.as_deref() == Some(project.as_str()) {
            self.open_feature(&project, &slug);
        }
        self.show_success(text);
        self.start_setup(&project, &slug);
        self.refresh_feature_graphs(&project, &slug, true);
    }

    /// Bring `adds` into the folders of open feature `slug` off the thread, shown in the tasks panel meanwhile. A setup or check
    /// under way has a copy of the feature that wouldn't see them: refused
    /// meanwhile.
    fn link_repos(&mut self, project: String, slug: String, adds: Vec<AddSpec>) {
        let Some(feature) = self.feature(&project, &slug).cloned() else {
            return self.show_warning("That feature is gone".into());
        };
        if self.feature_busy(&project, &slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        let names: Vec<String> = adds.iter().flat_map(|a| a.names.iter().cloned()).collect();
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Link,
            repo: None,
            what: format!(
                "Linking {} to \u{201c}{}\u{201d}",
                names.join(", "),
                feature.title
            ),
        };
        self.feature_work(Some(busy), move |_| {
            let result = layout::link_more(&feature, &[], &adds);
            Done::Linked {
                project,
                slug,
                names,
                result,
            }
        });
    }

    /// `names` were linked to feature `slug`, or not: kept, and what joined
    /// set up.
    fn feature_linked(
        &mut self,
        project: &str,
        slug: &str,
        names: &[String],
        result: Result<layout::MoreLinked, String>,
    ) {
        let (made, repos) = match result {
            Ok(made) => made,
            Err(why) => {
                tracing::warn!("linking repositories to feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't link {}: {why}", names.join(", ")));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        let root = feature.root.clone();
        let mut ignores = Vec::new();
        let mut keys = Vec::new();
        let mut joined = repos;
        for (link, created) in made {
            feature.log(now, format!("linked project {}", link.project));
            feature.links.push(link);
            ignores.extend(created.ignores);
            joined.extend(created.repos);
        }
        for repo in &joined {
            keys.push(repo.key());
            feature.log(now, format!("linked {}", repo.key()));
        }
        feature.insert_repos(joined);
        let title = feature.title.clone();
        self.save_feature(project, slug);
        if let Some(key) = keys.first() {
            self.select_repo(project, slug, key);
        }
        self.show_success(linked_text(&title, names, &root, &ignores));
        self.probe_feature(project, slug);
        self.rebuild_feature_graphs(project, slug);
        self.start_setup_of(project, slug, Some(keys));
    }

    /// A left click while the new feature dialog is open.
    pub fn new_feature_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.features.dialog.as_mut() else {
            return;
        };
        let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
        let mut picked = false;
        let outcome = match layout.hit(dialog.top, pos) {
            Some(NewFeatureHit::Button(chrome::CREATE)) => match dialog.create() {
                Ok(outcome) => Some(outcome),
                Err(why) => {
                    dialog.error = Some(why);
                    None
                }
            },
            Some(NewFeatureHit::Button(_)) => Some(DialogOutcome::Cancel),
            Some(NewFeatureHit::Repo(i)) => {
                if dialog.multi() {
                    dialog.focus = Some(i);
                    dialog.toggle(i);
                }
                None
            }
            Some(NewFeatureHit::Field) => {
                dialog.focus = None;
                None
            }
            Some(NewFeatureHit::Link) => {
                dialog.pick();
                picked = true;
                None
            }
            None if layout.panel.to_f64().contains(pos) => None,
            // Off it, it stays: a click away never throws the title away.
            None => None,
        };
        if picked {
            self.find_others();
        }
        if let Some(outcome) = outcome {
            self.apply_new_feature(outcome);
        }
    }

    /// The pointer moved while the new feature dialog is open.
    pub fn new_feature_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let layout = chrome::new_feature_layout(size, dialog.repos(), dialog.error.is_some());
            dialog.hover = layout.hit(dialog.top, pos);
        }
    }

    /// Scroll the dialog's repositories `by` rows.
    pub fn new_feature_scroll(&mut self, by: isize) {
        let fit = self.new_feature_fit();
        if let Some(dialog) = self.features.dialog.as_mut() {
            let most = dialog.repos().saturating_sub(fit.max(1));
            dialog.top = dialog.top.saturating_add_signed(by).min(most);
        }
    }

    /// The dialog's repository rows shown, for the wheel's pitch.
    pub fn new_feature_rows(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some()).rows
    }

    /// What the pointer can press in the new feature dialog.
    pub fn new_feature_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.features.dialog else {
            return Vec::new();
        };
        chrome::new_feature_layout(self.output_size(), dialog.repos(), dialog.error.is_some())
            .controls()
    }

    pub fn new_feature_view(&self) -> Option<NewFeatureView> {
        let dialog = self.features.dialog.as_ref()?;
        let project = self
            .projects
            .project(&dialog.project)
            .map_or_else(|| dialog.project.clone(), |p| p.name.clone());
        Some(new_feature_view(dialog, project))
    }

    // ------------------------------------------------------------ panel

    /// The features key: the panel on the active project's features, or
    /// the new feature dialog when it has none.
    pub fn open_features(&mut self) {
        let Some(project) = self.project.clone() else {
            return self.show_warning("Open a project to see its features".into());
        };
        let slugs = self.listed_slugs(&project);
        let open = self.features.panel.as_ref().map(|p| p.slug.as_str());
        let last = self.features.shown.get(&project).map(String::as_str);
        match pick_feature(&slugs, open, last) {
            Some(slug) => self.open_feature(&project, &slug),
            None => self.open_new_feature(None),
        }
    }

    /// The open features of other projects that span project `project`
    /// too: what its sidebar lists as "in <home name>" (see
    /// `open_linked_feature`).
    pub fn linked_features(&self, project: &str) -> Vec<LinkedFeature> {
        linked_in(&self.features.all, project)
            .into_iter()
            .map(|(home, f)| LinkedFeature {
                home: home.to_owned(),
                home_name: (self.projects.project(home))
                    .map_or_else(|| home.to_owned(), |p| p.name.clone()),
                slug: f.slug.clone(),
                title: f.title.clone(),
            })
            .collect()
    }

    /// Show feature `slug` of project `home` in the panel, entering that
    /// project first when another is shown: a linked project's sidebar row
    /// opens it so.
    pub fn open_linked_feature(&mut self, home: &str, slug: &str) {
        if self.feature(home, slug).is_none() {
            return self.show_warning("That feature is gone".into());
        }
        if self.project.as_deref() == Some(home) {
            return self.open_feature(home, slug);
        }
        self.features.pending = Some((home.to_owned(), slug.to_owned()));
        self.switch_project(home);
    }

    /// Once project `id` was entered: the feature a linked project's row
    /// asked for, if it is that project's.
    pub fn open_pending_feature(&mut self, id: &str) {
        if let Some((home, slug)) = self.features.pending.take()
            && home == id
        {
            self.open_feature(&home, &slug);
        }
    }

    /// Show the feature panel on feature `slug` of `project`: the first
    /// tab of the feature's workspace, which opens if none is, on the
    /// feature's current stage's page. The other panels close: they would
    /// cover it.
    pub fn open_feature(&mut self, project: &str, slug: &str) {
        self.show_feature(project, slug, true);
    }

    /// Back on the feature panel of feature `slug` of `project` from what
    /// covered it, on the page it was left on.
    pub(crate) fn back_to_feature(&mut self, project: &str, slug: &str) {
        self.show_feature(project, slug, false);
    }

    /// See `open_feature`; `to_current`: on its current stage's page even
    /// when it shows already.
    fn show_feature(&mut self, project: &str, slug: &str, to_current: bool) {
        if self.feature(project, slug).is_none() {
            return self.show_warning("That feature is gone".into());
        }
        self.switcher = None;
        self.repos.close();
        self.worktrees.close();
        self.project_menu = None;
        self.scrollback = None;
        self.tasks_panel = None;
        if self.inbox.is_open() {
            self.close_inbox();
        }
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        // It shows as it was left (its parked panel), on the picked
        // stage while the feature didn't move on.
        let same = |p: &FeaturePanel| p.project == project && p.slug == slug;
        let shown = self.features.panel.as_ref().is_some_and(same);
        if !self.show_feature_tab(project, slug) {
            return;
        }
        // Showing it again where it shows goes back to its current stage.
        if shown
            && to_current
            && let Some(panel) = self.features.panel.as_mut().filter(|p| same(p))
        {
            panel.unpick();
            panel.keys = false;
        }
        self.sync_feature_page();
        // Showing its tab read the feature again, unless it showed already.
        if shown {
            self.probe_feature(project, slug);
        }
    }

    /// Make the feature panel what the shown tab is: the panel of the
    /// feature whose workspace it is when it is that workspace's pinned
    /// tab, else none. A panel left is kept to show as it was when its
    /// tab shows again.
    pub(crate) fn sync_feature_panel(&mut self) {
        let w = self.active_workspace;
        let on_tab = self.tab().is_some_and(crate::Tab::is_feature);
        let shown = (self.project.clone())
            .zip(on_tab.then(|| self.workspace_feature(w)).flatten())
            .map(|(project, f)| (project, f.slug.clone()));
        let open = (self.features.panel.as_ref()).map(|p| (p.project.clone(), p.slug.clone()));
        if open == shown {
            return;
        }
        if let Some(panel) = self.features.panel.take() {
            let key = (panel.project.clone(), panel.slug.clone());
            self.features.parked.insert(key, panel);
        }
        if let Some((project, slug)) = shown {
            let key = (project.clone(), slug.clone());
            let panel = self.features.parked.remove(&key).unwrap_or(FeaturePanel {
                project: project.clone(),
                slug: slug.clone(),
                ..FeaturePanel::default()
            });
            let fresh = panel.sources.is_empty();
            self.features.panel = Some(panel);
            self.features.shown.insert(project.clone(), slug.clone());
            self.sync_feature_page();
            self.probe_feature(&project, &slug);
            self.watch_feature_tail();
            if fresh {
                self.find_repo_sources();
            }
        }
        self.refresh_pointer_focus();
    }

    /// Open the feature panel on feature `slug` of `project` on `stage`'s
    /// page. AI review with findings to decide shows
    /// the diff at the first instead, and Code review the diff, as their
    /// rows' Enter does; Manual test with criteria has the keyboard on them.
    pub fn open_feature_at(&mut self, project: &str, slug: &str, stage: Stage) {
        let current = self
            .feature(project, slug)
            .filter(|f| f.status == Status::Open && f.current() == Some(stage));
        if let Some(f) = current
            && matches!(stage, Stage::AiReview | Stage::CodeReview)
        {
            let ready = f.runs.get(&stage).is_some_and(|r| r.ready.is_some());
            let first = f
                .review
                .as_ref()
                .and_then(|r| r.first_open())
                .map(|x| x.id.clone())
                .filter(|_| ready);
            if stage == Stage::CodeReview || first.is_some() {
                return self.open_feature_diff(project, slug, first.as_deref());
            }
        }
        self.open_feature(project, slug);
        // Suggestions to decide open over the panel.
        let deciding = stage == Stage::PlanReview
            && (self.feature(project, slug))
                .filter(|f| f.status == Status::Open && f.current() == Some(stage))
                .and_then(crate::feature_plan_review::deciding)
                .is_some_and(|r| r.open() > 0);
        if deciding {
            self.open_plan_sheet(project, slug);
        }
        let testing = stage == Stage::ManualTest
            && self
                .feature(project, slug)
                .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage))
            && !self.criteria(project, slug).is_empty();
        let Some(feature) = self.panel_feature().cloned() else {
            return;
        };
        let on_it = |p: &&mut FeaturePanel| p.project == project && p.slug == slug;
        if let Some(panel) = self.features.panel.as_mut().filter(on_it) {
            panel.focus = TabArea::Stages;
            panel.keys = false;
            panel.pick(&feature, stage);
        }
        self.sync_feature_page();
        if testing {
            self.focus_criteria(project, slug);
        }
    }

    /// Hold the feature tab's Page scroll for the page it shows now (see
    /// `FeaturePanel::sync_page`).
    pub(crate) fn sync_feature_page(&mut self) {
        let Some(panel) = self.features.panel.as_ref() else {
            return;
        };
        let Some(key) = (self.feature(&panel.project, &panel.slug)).map(|f| panel.page_key(f))
        else {
            return;
        };
        if let Some(panel) = self.features.panel.as_mut() {
            panel.hold_page(key);
        }
    }

    /// The feature the panel is on.
    fn panel_feature(&self) -> Option<&Feature> {
        let panel = self.features.panel.as_ref()?;
        self.feature(&panel.project, &panel.slug)
    }

    /// A key while the feature panel has the keyboard.
    pub fn feature_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<PanelOutcome> {
        if self.criterion_note_key(mods, sym, typed) {
            return None;
        }
        let plain = !(mods.ctrl || mods.shift || mods.alt || mods.logo);
        let page = match sym {
            Keysym::Page_Up | Keysym::KP_Page_Up if plain => Some(-1.0),
            Keysym::Page_Down | Keysym::KP_Page_Down if plain => Some(1.0),
            _ => None,
        };
        if let Some(sign) = page {
            self.page_feature_tab(sign);
            return None;
        }
        // Up and Down scroll the keys list a notch.
        let keys = (self.features.panel.as_ref()).is_some_and(|p| p.keys);
        let notch = match sym {
            Keysym::Up | Keysym::KP_Up if plain => Some(-1.0),
            Keysym::Down | Keysym::KP_Down if plain => Some(1.0),
            _ => None,
        };
        if let Some(sign) = notch.filter(|_| keys) {
            self.feature_panel_scroll_page(Motion::Notches(sign));
            return None;
        }
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let feature = self.feature(&panel.project, &panel.slug).cloned()?;
        let ctx = self.row_ctx(&feature);
        let plan = self.planned_checks(&feature);
        let panel = self.features.panel.as_mut()?;
        let before = (panel.focus, panel.repo, panel.expanded);
        let card_before = panel.finding;
        let outcome = panel.key(mods, sym, &feature, &slugs, ctx, &plan);
        let now = (self.features.panel.as_ref()).map(|p| (p.focus, p.repo, p.expanded));
        if now.is_some_and(|(focus, repo, expanded)| {
            focus == TabArea::Repos && (focus, repo, expanded) != before
        }) {
            self.reveal_feature_repo();
        }
        let card_now = (self.features.panel.as_ref()).map(|p| (p.focus, p.finding));
        if card_now.is_some_and(|(focus, card)| {
            focus == TabArea::Findings && (focus != before.0 || card != card_before)
        }) {
            self.sync_feature_page();
            self.reveal_feature_finding();
        }
        if now.is_some_and(|(_, _, expanded)| expanded && !before.2) {
            self.find_repo_sources();
        }
        self.sync_feature_page();
        outcome
    }

    /// Move the feature tab's Page column by `motion`, wherever the
    /// pointer is.
    fn feature_panel_scroll_page(&mut self, motion: Motion) {
        self.sync_feature_page();
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let col = TabColumn::Page;
        if let Some(panel) = self.features.panel.as_mut() {
            panel.scroll[col as usize].apply(motion, Instant::now(), tab_bounds(&layout, col));
        }
    }

    /// Glide the feature tab's page column a page down (`sign` 1) or up
    /// (-1), keeping a little of what showed in view, as PgUp / PgDn do.
    fn page_feature_tab(&mut self, sign: f64) {
        self.sync_feature_page();
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let col = TabColumn::Page;
        let Some(place) = layout.columns[col as usize] else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let by = (place.size.h - PAGE_KEEP).max(place.size.h / 2) as f64;
        let scroll = &mut panel.scroll[col as usize];
        let to = scroll.pos() + sign * by;
        scroll.glide_to(to, Instant::now(), tab_bounds(&layout, col));
    }

    /// Scroll the feature tab's repositories' column to show the one with
    /// the keyboard, as laid out now.
    fn reveal_feature_repo(&mut self) {
        let Some((view, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Repos as usize];
        if let Some(to) = layout.reveal_repo(view.repo, scroll.pos())
            && to != scroll.pos()
        {
            scroll.jump(to);
        }
    }

    /// Scroll the feature tab's page to show the finding with the keyboard,
    /// as laid out now.
    pub(crate) fn reveal_feature_finding(&mut self) {
        let Some((_, layout)) = self.feature_tab_parts() else {
            return;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        let scroll = &mut panel.scroll[TabColumn::Page as usize];
        if let Some(to) = layout.reveal_finding(panel.finding, scroll.pos())
            && to != scroll.pos()
        {
            scroll.jump(to);
        }
    }

    /// Unfold the feature tab's repositories or fold them, reading again
    /// what they can offer when they unfold.
    fn toggle_feature_repos(&mut self) {
        let Some(panel) = self.features.panel.as_mut() else {
            return;
        };
        panel.toggle_repos();
        if panel.expanded {
            self.find_repo_sources();
        }
    }

    /// Do what the feature panel asked.
    pub fn apply_feature_panel(&mut self, outcome: PanelOutcome) {
        let Some((project, slug)) = self
            .features
            .panel
            .as_ref()
            .map(|p| (p.project.clone(), p.slug.clone()))
        else {
            return;
        };
        match outcome {
            PanelOutcome::Tick => match self.feature(&project, &slug).and_then(Feature::current) {
                Some(stage) if needs(stage).is_some() => {
                    self.approve_stage(&project, &slug, stage);
                }
                Some(stage) if stage.bound() => self.read_and_tick(&project, &slug, stage),
                Some(_) => {
                    self.tick_feature(&project, &slug, None, "you");
                    self.probe_feature(&project, &slug);
                }
                None => self.show_warning("Nothing left to mark done".into()),
            },
            PanelOutcome::Ship => self.ask_ship_feature(&project, &slug),
            PanelOutcome::SetUp => self.set_up_again(&project, &slug),
            PanelOutcome::SetupLog => self.open_setup_log(&project, &slug),
            PanelOutcome::StartSandbox => self.ask_start_sandbox(&project, &slug),
            PanelOutcome::OpenSandbox => self.open_sandbox_app(&project, &slug),
            PanelOutcome::RestartSandbox => {
                let what = crate::feature_sandbox::StopFor::Restart;
                self.finish_stop(&project, &slug, what);
            }
            PanelOutcome::ResetSandbox => self.ask_reset_sandbox(&project, &slug),
            PanelOutcome::StopSandbox => self.ask_stop_sandbox(&project, &slug),
            PanelOutcome::RemoveSandbox => self.ask_remove_sandbox(&project, &slug),
            PanelOutcome::StartAgent(stage, fresh) => {
                self.start_stage_agent(&project, &slug, stage, fresh)
            }
            PanelOutcome::SendPrompt(stage) => self.send_stage_prompt(&project, &slug, stage),
            PanelOutcome::ShowAgent(stage) => self.show_stage_agent(&project, &slug, stage),
            PanelOutcome::ResumeAgent(stage) => self.resume_stage_agent(&project, &slug, stage),
            PanelOutcome::OpenDoc(stage) => {
                self.open_doc_tab(&project, &slug, DocFile::Stage(stage));
            }
            PanelOutcome::Continue(stage) => self.continue_stage_agent(&project, &slug, stage),
            PanelOutcome::RunChecks => match self.feature(&project, &slug) {
                Some(f) if self.busy_with(&project, &slug, Job::Verify).is_some() => {
                    let text = format!("The checks of \u{201c}{}\u{201d} run already", f.title);
                    self.show_warning(text);
                }
                Some(_) => self.run_verify(&project, &slug),
                None => {}
            },
            PanelOutcome::RunFailed => self.rerun_failed_checks(&project, &slug),
            PanelOutcome::RerunCheck(index) => self.rerun_check(&project, &slug, index),
            PanelOutcome::StopCheck(index) => self.stop_checks(&project, &slug, Some(index)),
            PanelOutcome::StopChecks => self.stop_checks(&project, &slug, None),
            PanelOutcome::ShowCheck(index) => self.show_check(&project, &slug, index),
            PanelOutcome::SkipCheck {
                repo,
                command,
                skip,
            } => self.set_check_skipped(&project, &slug, &repo, &command, skip),
            PanelOutcome::Evidence => self.open_evidence(&project, &slug),
            PanelOutcome::Findings => {
                let first = self
                    .feature(&project, &slug)
                    .and_then(|f| Some(f.review.as_ref()?.first_open()?.id.clone()));
                self.open_feature_diff(&project, &slug, first.as_deref());
            }
            PanelOutcome::SendFixes => self.send_findings(&project, &slug),
            PanelOutcome::FixAllFindings => self.fix_all_findings(&project, &slug),
            PanelOutcome::PlanFixes => self.plan_verify_fixes(&project, &slug),
            PanelOutcome::ReviewFixPlan => self.open_doc_tab(&project, &slug, DocFile::FixPlan),
            PanelOutcome::MakeFixes => self.make_fixes_now(&project, &slug),
            PanelOutcome::AskFixPlan => self.ask_fix_plan(&project, &slug),
            PanelOutcome::DropFixPlan => self.ask_drop_fix_plan(project.clone(), slug.clone()),
            PanelOutcome::EditCommands(tab) => {
                // With nothing to run yet, what a repository always checks
                // comes first.
                let any = (self.feature(&project, &slug)).is_some_and(|f| {
                    self.row_ctx(f).checks || !f.checks.is_empty() || !f.skip_checks.is_empty()
                });
                let tab = match tab {
                    CommandsTab::Feature if !any => CommandsTab::Checks,
                    tab => tab,
                };
                self.open_project_commands(&project, Some(&slug), tab);
            }
            PanelOutcome::DecideFinding(id, decision) => {
                self.decide_finding(&project, &slug, &id, decision);
            }
            PanelOutcome::AskDismiss(id) => self.ask_dismiss_finding(&project, &slug, &id),
            PanelOutcome::EditFinding(id) => self.ask_edit_finding(&project, &slug, &id),
            PanelOutcome::AddComment => self.ask_add_comment(&project, &slug),
            PanelOutcome::ShowFinding(id) => {
                self.open_feature_diff(&project, &slug, Some(&id));
            }
            PanelOutcome::FindingRefused(why) => self.show_warning(why.to_owned()),
            PanelOutcome::FoldRound(n) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.toggle_round(n);
                }
            }
            PanelOutcome::CodeReview => self.open_feature_diff(&project, &slug, None),
            PanelOutcome::ManualTest => self.focus_criteria(&project, &slug),
            PanelOutcome::Manual(outcome) => self.apply_manual_test(outcome),
            PanelOutcome::PlanSuggestions => self.open_plan_sheet(&project, &slug),
            PanelOutcome::WriteFinalPlan => self.write_final_plan(&project, &slug),
            PanelOutcome::SkipPlanReview => self.ask_skip_plan_review(&project, &slug),
            PanelOutcome::ChangeDecisions => self.ask_change_decisions(&project, &slug),
            PanelOutcome::OpenPr(url) => self.open_feature_page(&project, &slug, url),
            PanelOutcome::PickAgent(stage, name) => {
                self.pick_stage_agent(&project, &slug, stage, &name);
            }
            PanelOutcome::CycleAgent(stage) => self.cycle_stage_agent(&project, &slug, stage),
            PanelOutcome::PickModel(stage) => self.pick_stage_model(&project, &slug, stage),
            PanelOutcome::FreshMilestones(fresh) => {
                self.set_fresh_milestones(&project, &slug, fresh);
            }
            PanelOutcome::Untick(stage) => self.reopen_stage(&project, &slug, stage),
            PanelOutcome::GoToCurrent => self.feature_cursor_to_current_of(&project, &slug),
            PanelOutcome::StartOver(stage) => self.ask_start_over(&project, &slug, stage),
            PanelOutcome::NotYet(first) => {
                self.show_warning(format!("Mark {} done first", first.label()));
            }
            PanelOutcome::Touch(repo) => self.touch_repo(&project, &slug, repo),
            PanelOutcome::RemoveRepo(key) => {
                let keys = vec![key];
                if self.may_remove_repos(&project, &slug, &keys, false) {
                    self.check_remove_repos(project.clone(), slug.clone(), keys, false);
                }
            }
            PanelOutcome::MakeContext(key) => {
                let keys = vec![key];
                if self.may_remove_repos(&project, &slug, &keys, true) {
                    self.check_remove_repos(project.clone(), slug.clone(), keys, true);
                }
            }
            PanelOutcome::AddToFeature(key) => self.add_repo_to_feature(&project, &slug, &key),
            PanelOutcome::AddContext(key) => self.add_context_repo(&project, &slug, &key),
            PanelOutcome::DropContext(key) => self.drop_context_repo(&project, &slug, &key),
            PanelOutcome::Open(dir) => {
                if !dir.exists() {
                    return self.show_error(format!("{} is gone", tilde(&dir)));
                }
                self.open_feature_project_tab(&project, &slug, &dir);
            }
            PanelOutcome::CopyPath => {
                if let Some(dir) = self.feature(&project, &slug).map(|f| f.dir.clone()) {
                    self.copy_text(dir.display().to_string());
                    self.show_success(format!("Copied {}", tilde(&dir)));
                }
            }
            PanelOutcome::Remove => self.check_remove_feature(&project, &slug),
            PanelOutcome::New => self.open_new_feature(None),
            // In its own tab, its workspace opened if need be.
            PanelOutcome::Show(to) => self.open_feature(&project, &to),
        }
    }

    /// Show the tab of `stage`'s agent; one gone is forgotten, and Enter
    /// starts a new one.
    fn show_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let in_t3 = (self.feature(project, slug))
            .and_then(|f| f.runs.get(&stage))
            .is_some_and(|run| run.t3.is_some());
        if in_t3 {
            return self.show_t3_stage(project, slug, stage);
        }
        let pane = self
            .feature(project, slug)
            .and_then(|f| f.runs.get(&stage)?.pane.clone());
        if let Some(pane) = pane.filter(|p| self.herdr_pane_place(p).is_some()) {
            return self.focus_agent(&crate::agents::AgentKey::Herdr(pane));
        }
        let mut resumes = false;
        if let Some(run) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&stage))
        {
            run.pane = None;
            resumes = run.resumes();
            self.save_feature(project, slug);
        }
        // Build is locked by the stage that asked for the fix plan, and what
        // comes of its chat is the round's to say.
        let round = stage == Stage::Build
            && (self.feature(project, slug)).is_some_and(|f| f.fix_round.is_some());
        if round {
            return self.show_warning(
                "The Build agent's tab is gone: a fresh agent takes the fix plan on".to_owned(),
            );
        }
        if resumes {
            return self.resume_stage_agent(project, slug, stage);
        }
        self.show_warning(format!(
            "{}'s agent tab is gone; Enter starts a new one",
            stage.label()
        ));
    }

    /// Show the current stage's page of feature `slug`, when the panel
    /// shows it: it moved on.
    pub(crate) fn feature_cursor_to_current_of(&mut self, project: &str, slug: &str) {
        let on_it = |p: &&mut FeaturePanel| p.project == project && p.slug == slug;
        if let Some(panel) = self.features.panel.as_mut().filter(on_it) {
            panel.unpick();
        }
    }

    /// What repository `key` of `feature` is called on screen.
    fn repo_label(&self, feature: &Feature, key: &str) -> String {
        match feature.repo(key) {
            Some(repo) => self.projects.names(feature).label(repo),
            None => key.to_owned(),
        }
    }

    /// What repository `key` of feature `slug` is called on screen, held
    /// by it or not.
    fn repo_name(&self, project: &str, slug: &str, key: &str) -> String {
        let Some(feature) = self.feature(project, slug) else {
            return key.to_owned();
        };
        let names = self.projects.names(feature);
        if let Some(repo) = feature.repo(key) {
            return names.label(repo);
        }
        let context = (feature.context.iter()).find(|c| c.key() == key);
        let gone = (feature.removed.iter()).find(|r| r.key() == key);
        match (context, gone) {
            (Some(c), _) => names.label_of(c.project.as_deref(), &c.name),
            (_, Some(r)) => names.label_of(r.project.as_deref(), &r.name),
            _ => key.to_owned(),
        }
    }

    /// Make `repo` (a `Repo::key`) one feature `slug` changes, off the
    /// thread.
    fn touch_repo(&mut self, project: &str, slug: &str, repo: String) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let doing = self
            .features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug && b.repo.as_ref() == Some(&repo));
        let label = self.repo_label(&feature, &repo);
        if doing {
            return self.show_warning(format!("{label} is being touched already"));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Touch,
            repo: Some(repo.clone()),
            what: format!("Touching {label} for \u{201c}{}\u{201d}", feature.title),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| {
            let result = layout::touch(&feature, &repo);
            Done::Touched {
                project,
                slug,
                repo,
                result,
            }
        });
    }

    fn feature_touched(
        &mut self,
        project: &str,
        slug: &str,
        repo: &str,
        result: Result<feature::Repo, String>,
    ) {
        let label = match self.feature(project, slug) {
            Some(feature) => self.repo_label(feature, repo),
            None => repo.to_owned(),
        };
        let touched = match result {
            Ok(touched) => touched,
            Err(why) => {
                tracing::warn!("touching {repo} for feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't touch {label}: {why}"));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let branch = touched.branch.clone().unwrap_or_else(|| feature.branch());
        match feature.repos.iter_mut().find(|r| r.key() == repo) {
            Some(r) => *r = touched,
            None => feature.repos.push(touched),
        }
        feature.log(feature::now(), format!("touched {repo}"));
        self.save_feature(project, slug);
        self.show_success(format!("{label} is on {branch} now"));
        self.probe_feature(project, slug);
    }

    /// Whether repositories `keys` can leave feature `slug` now, whatever
    /// they hold; else says why not.
    fn may_remove_repos(
        &mut self,
        project: &str,
        slug: &str,
        keys: &[String],
        to_context: bool,
    ) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            self.show_warning("That feature is gone".into());
            return false;
        };
        let title = &feature.title;
        let named = keys
            .iter()
            .map(|k| self.repo_label(feature, k))
            .collect::<Vec<_>>()
            .join(", ");
        let why = if self.feature_busy(project, slug) {
            Some(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ))
        } else if !linkable(feature) {
            Some(format!(
                "\u{201c}{title}\u{201d} has shipped; its repositories can't change now"
            ))
        } else if let Some(why) = layout::removal_refusal(feature, keys) {
            Some(match to_context {
                true => format!("Can't make {named} context only: {why}"),
                false => format!("Can't remove {named}: {why}"),
            })
        } else {
            let goes = |r: &feature::Repo| keys.contains(&r.key());
            // A link left with none takes its folder along.
            let folders = (feature.links.iter())
                .filter(|l| feature.repos_of(Some(&l.project)).all(goes))
                .map(|l| l.dir.as_path());
            let worktrees = (feature.repos.iter())
                .filter(|r| goes(r))
                .map(|r| r.worktree.as_path());
            folders.chain(worktrees).find_map(|dir| {
                let user = self.folder_in_use(dir)?;
                Some(format!("{} has {user} in it; close it first", tilde(dir)))
            })
        };
        match why {
            Some(why) => {
                self.show_warning(why);
                false
            }
            None => true,
        }
    }

    /// Look at what taking repositories `keys` out of feature `slug` loses,
    /// off the thread, then ask (see `ask_remove_repos`).
    pub(crate) fn check_remove_repos(
        &mut self,
        project: String,
        slug: String,
        keys: Vec<String>,
        to_context: bool,
    ) {
        let Some(feature) = self.feature(&project, &slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::RemoveRepos,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        self.feature_work(Some(busy), move |_| Done::RemovalLosses {
            result: layout::removal_losses(&feature, &keys),
            project,
            slug,
            keys,
            to_context,
        });
    }

    /// Take repositories `keys` out of feature `slug`. One on the feature's
    /// branch always asks first: its branch goes, whatever it holds, and
    /// the question names what is lost with it. Others go at once when
    /// nothing is lost. Commits stop them, with a word on why. `to_context`:
    /// they stay on as context (see `remove_repos`).
    fn ask_remove_repos(
        &mut self,
        project: String,
        slug: String,
        keys: Vec<String>,
        to_context: bool,
        result: Result<Vec<String>, String>,
    ) {
        let losses = match result {
            Ok(losses) => losses,
            Err(why) => return self.show_warning(format!("Can't remove it: {why}")),
        };
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        if !removal_asks(feature, &keys, &losses) {
            return self.remove_repos(&project, &slug, &keys, false, to_context);
        }
        let named: Vec<String> = (keys.iter()).map(|k| self.repo_label(feature, k)).collect();
        let named = named.join(", ");
        let (question, go) = match to_context {
            true => (format!("Make {named} context only?"), "Make context"),
            false => (
                format!("Remove {named} from \u{201c}{}\u{201d}?", feature.title),
                "Remove",
            ),
        };
        let repos = keys.len();
        let then = crate::confirm::Then::RemoveRepos {
            project,
            slug,
            keys,
            force: !losses.is_empty(),
            to_context,
        };
        self.ask_remove_worktrees(
            question,
            remove_repos_body(repos, &losses, to_context),
            go,
            then,
        );
    }

    /// Take repositories `keys` out of feature `slug` off the thread: their
    /// worktrees and branches go (`force`: with what they lose), and with
    /// `to_context` agents go on reading their main checkouts.
    pub fn remove_repos(
        &mut self,
        project: &str,
        slug: &str,
        keys: &[String],
        force: bool,
        to_context: bool,
    ) {
        if !self.may_remove_repos(project, slug, keys, to_context) {
            return;
        }
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let named: Vec<String> = (keys.iter())
            .map(|k| self.repo_label(&feature, k))
            .collect();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::RemoveRepos,
            repo: None,
            what: format!(
                "Removing {} from \u{201c}{}\u{201d}",
                named.join(", "),
                feature.title
            ),
        };
        let graphs = self.running_feature_graphs(project, slug);
        let (project, slug, keys) = (project.to_owned(), slug.to_owned(), keys.to_vec());
        self.feature_work(Some(busy), move |_| {
            stop_graphs(graphs);
            Done::ReposRemoved {
                results: layout::remove_repos(&feature, &keys, force),
                project,
                slug,
                to_context,
            }
        });
    }

    /// Repositories were taken out of feature `slug`, or not: those gone
    /// leave it, with a link left with none, kept.
    fn feature_repos_removed(
        &mut self,
        project: &str,
        slug: &str,
        to_context: bool,
        results: Vec<(String, Result<(), String>)>,
    ) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let mut gone = Vec::new();
        let mut named = Vec::new();
        let mut failed = Vec::new();
        for (key, result) in results {
            let label = self.repo_label(feature, &key);
            match result {
                Ok(()) => {
                    gone.push(key);
                    named.push(label);
                }
                Err(why) => {
                    tracing::warn!("removing {key} from feature {project}/{slug}: {why}");
                    failed.push(format!("{label}: {why}"));
                }
            }
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        let title = feature.title.clone();
        for key in &gone {
            let held = feature.repo(key).cloned().filter(|_| to_context);
            drop_repo(feature, key, now);
            match held {
                Some(repo) => {
                    feature.add_context(feature::ContextRepo {
                        name: repo.name,
                        main: repo.main,
                        project: repo.project,
                    });
                    feature.log(now, format!("made {key} context only"));
                }
                None => feature.log(now, format!("removed {key}")),
            }
        }
        if !gone.is_empty() {
            self.save_feature(project, slug);
            self.show_success(match to_context {
                true => context_text(&named.join(", ")),
                false => format!(
                    "\u{201c}{title}\u{201d} no longer holds {}",
                    named.join(", ")
                ),
            });
            self.rebuild_feature_graphs(project, slug);
        }
        if let Some(key) = gone.first() {
            self.select_repo(project, slug, key);
        }
        if !failed.is_empty() {
            self.show_error(format!("Couldn't remove {}", failed.join("; ")));
        }
        self.probe_feature(project, slug);
    }

    /// Look at what removing feature `slug`'s worktrees loses, off the
    /// thread, then ask (see `ask_remove_feature`).
    fn check_remove_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} is busy; try again once it is done",
                feature.title
            ));
        }
        // Its own workspace closes with it; anything else in it refuses.
        if let Some(user) = self.feature_folder_in_use(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{}\u{201d} has {user} in it; close it first",
                feature.title
            ));
        }
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Losses,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |_| Done::Losses {
            losses: layout::losses(&feature),
            project,
            slug,
        });
    }

    /// Ship feature `slug` of `project` as its last Ship check said, once
    /// the Ship sheet is confirmed: open its pull requests off the thread,
    /// or start its squash in a tab.
    pub fn ship_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = feature.title.clone();
        let key = (project.to_owned(), slug.to_owned());
        let asked = self.features.asked.remove(&key).or_else(|| {
            let probe = self.features.probes.get(&key)?.last.as_ref();
            match ship_ask(&title, probe) {
                ShipAsk::Ready(asked) => Some(asked),
                _ => None,
            }
        });
        let Some(Asked { actions, patch }) = asked else {
            return self.show_warning(format!("Nothing to ship in \u{201c}{title}\u{201d}"));
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let (squash, prs): (Vec<Action>, Vec<Action>) = actions
            .into_iter()
            .partition(|a| matches!(a, Action::Squash { .. }));
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Ship,
            repo: None,
            what: format!("Shipping \u{201c}{title}\u{201d}"),
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // The code is read again first: a commit since the Ship check
        // would ship what no gate saw.
        self.feature_work(Some(busy), move |_| {
            let result = still_checked(&feature, patch.as_deref())
                .map_err(|why| (why, Vec::new()))
                .and_then(|()| {
                    if prs.is_empty() {
                        Ok(Vec::new())
                    } else {
                        ship::open_prs(&feature, &prs, "gh")
                    }
                });
            Done::Shipped {
                result,
                project,
                slug,
                patch,
                squash,
            }
        });
    }

    /// Start the squash of feature `slug` (`squash`, its Squash steps) in a
    /// tab: done once its branches read as merged.
    fn start_squash(&mut self, project: &str, slug: &str, squash: &[Action]) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let (dir, command) = ship::squash_command(feature, squash);
        if command.trim().is_empty() {
            tracing::warn!("shipping feature {project}/{slug}: no squash command");
            return self.show_error(format!(
                "Couldn't ship \u{201c}{title}\u{201d}: no squash command for it"
            ));
        }
        if !dir.exists() {
            return self.show_error(format!(
                "Couldn't ship \u{201c}{title}\u{201d}: {} is gone",
                tilde(&dir)
            ));
        }
        // In its own workspace; another project's feature can't open one.
        if (self.in_feature_workspace(project, slug, |gerdr, w| {
            gerdr.new_tab_in(w, &dir, Some(command))
        }))
        .is_none()
        {
            return;
        }
        if let Some(feature) = self.feature_mut(project, slug) {
            let now = feature::now();
            feature.shipping.squash_at = Some(now);
            feature.log(now, "Squash started");
        }
        self.save_feature(project, slug);
    }

    /// Ship was asked for on feature `slug`: say what stops it, or ask
    /// with what shipping does, as its latest Ship check has it.
    fn ask_ship_feature(&mut self, project: &str, slug: &str) {
        let Some((title, branch)) = self
            .feature(project, slug)
            .map(|f| (f.title.clone(), f.branch()))
        else {
            return;
        };
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let key = (project.to_owned(), slug.to_owned());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        match ship_ask(&title, probe) {
            ShipAsk::Checking => {
                self.show_warning(format!(
                    "Still checking \u{201c}{title}\u{201d}; try again in a moment"
                ));
                self.probe_feature(project, slug);
            }
            ShipAsk::Blocked(why) => self.show_warning(why),
            ShipAsk::Nothing => {
                self.show_warning(format!("Nothing to ship in \u{201c}{title}\u{201d}"));
            }
            ShipAsk::Ready(asked) => {
                let steps: Vec<String> = asked.actions.iter().map(Action::label).collect();
                self.features.asked.insert(key, asked);
                self.ask_ship(project.to_owned(), slug.to_owned(), &title, &branch, &steps);
            }
        }
    }

    /// The pull requests of feature `slug` are open, or some are and the
    /// rest failed: they are kept, and once all are open, Ship is done
    /// (on the code whose patch-id is `patch`), unless a squash goes on.
    fn feature_shipped(
        &mut self,
        project: &str,
        slug: &str,
        patch: Option<&str>,
        squash: &[Action],
        result: Result<Vec<Pr>, (String, Vec<Pr>)>,
    ) {
        if result.is_ok() && !squash.is_empty() {
            self.start_squash(project, slug, squash);
        }
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let now = feature::now();
        let (prs, failed) = match result {
            Ok(prs) => (prs, None),
            Err((why, prs)) => (prs, Some(why)),
        };
        let n = prs.len();
        // Those it had open already, the push updated.
        let updated = prs
            .iter()
            .filter(|pr| feature.shipping.prs.contains(pr))
            .count();
        for pr in prs {
            match feature.shipping.prs.iter_mut().find(|p| p.repo == pr.repo) {
                Some(old) => *old = pr,
                None => feature.shipping.prs.push(pr),
            }
        }
        if let Some(why) = failed {
            feature.log(now, format!("Shipping stopped: {why}"));
            self.save_feature(project, slug);
            tracing::warn!("shipping feature {project}/{slug}: {why}");
            self.show_error(format!("Couldn't ship \u{201c}{title}\u{201d}: {why}"));
            return self.probe_feature(project, slug);
        }
        if n == 0 {
            return self.probe_feature(project, slug);
        }
        let mut text = opened_text(&title, n - updated, updated);
        feature.log(now, text.clone());
        let done = squash.is_empty()
            && feature.status == Status::Open
            && feature.current() == Some(Stage::Ship);
        if done {
            feature.tick(now, "gerdr", patch);
        }
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        // Shipped: its sandbox has done its work, and goes, or its tabs would
        // keep the workspace open.
        if done
            && self
                .feature(project, slug)
                .is_some_and(|f| f.sandbox.is_some())
        {
            self.finish_stop(project, slug, crate::feature_sandbox::StopFor::Remove);
        }
        // Shipped: its workspace closes, unless something runs in it.
        if done && let Some(kept) = self.close_shipped_workspace(project, slug) {
            text = format!("{text}; {kept}");
        }
        self.show_success(text);
        self.probe_feature(project, slug);
    }

    /// Ask before removing feature `slug`'s worktrees, naming what goes
    /// with them.
    fn ask_remove_feature(&mut self, project: String, slug: String, losses: Vec<String>) {
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let title = feature.title.clone();
        let abandon = feature.status != Status::Done;
        let (question, go) = if abandon {
            (format!("Abandon \u{201c}{title}\u{201d}?"), "Abandon")
        } else {
            (
                format!("Remove the worktrees of \u{201c}{title}\u{201d}?"),
                "Remove worktrees",
            )
        };
        let mut body = removal_body(&losses);
        // On a line of its own: the dialog cuts a line to its width, and
        // both together don't fit.
        if let Some(w) = self.feature_workspace(&project, &slug) {
            let running = self.workspace_running(w);
            body = format!(
                "{body}.\n{}.",
                crate::feature_workspace::closing_text(&running)
            );
        }
        let force = !losses.is_empty();
        let then = crate::confirm::Then::RemoveFeature {
            project,
            slug,
            abandon,
            force,
        };
        self.ask_remove_worktrees(question, body, go, then);
    }

    /// Remove feature `slug`'s worktrees off the thread (`force`: with
    /// what they lose), then mark it abandoned if `abandon`.
    pub fn remove_feature(&mut self, project: &str, slug: &str, abandon: bool, force: bool) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Remove,
            repo: None,
            what: format!(
                "Removing the worktrees of \u{201c}{}\u{201d}",
                feature.title
            ),
        };
        // Its sandbox goes with it: nothing else would ever stop it.
        let sandbox = self
            .feature_mut(project, slug)
            .and_then(|f| f.sandbox.take());
        if sandbox.is_some() {
            self.save_feature(project, slug);
        }
        let sandbox_dir = crate::feature_sandbox::sandbox_dir(project, slug);
        self.t3_forget_project(&feature.dir);
        for stage in feature.runs.keys() {
            if let Some(left) = self.t3_left_running(project, slug, *stage) {
                self.show_warning(left);
            }
        }
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // Its workspace closes first, its terminals gone before its
        // worktrees go.
        let (p, s) = (project.clone(), slug.clone());
        self.close_feature_workspace_then(&p, &s, move |gerdr| {
            let graphs = gerdr.running_feature_graphs(&project, &slug);
            gerdr.feature_work(Some(busy), move |_| {
                stop_graphs(graphs);
                if let Some(run) = &sandbox
                    && let Err(why) = feature::sandbox_run::stop(run, &sandbox_dir, true)
                {
                    tracing::error!("removing the sandbox of {slug}: {why}");
                    let text = format!("A removed feature's sandbox stayed behind: {why}");
                    crate::problems::report(crate::feature_sandbox::PROBLEMS, text);
                }
                Done::Removed {
                    result: layout::remove(&feature, force),
                    project,
                    slug,
                    abandon,
                }
            });
        });
    }

    fn feature_removed(
        &mut self,
        project: &str,
        slug: &str,
        abandon: bool,
        result: Result<(), String>,
    ) {
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if let Err(why) = result {
            tracing::warn!("removing the worktrees of feature {project}/{slug}: {why}");
            return self.show_error(format!(
                "Couldn't remove the worktrees of \u{201c}{title}\u{201d}: {why}"
            ));
        }
        let now = feature::now();
        if abandon {
            feature.status = Status::Abandoned;
            feature.log(now, "abandoned");
        } else {
            feature.log(now, "worktrees removed");
        }
        self.save_feature(project, slug);
        self.show_success(if abandon {
            format!("Abandoned \u{201c}{title}\u{201d}")
        } else {
            format!("Removed the worktrees of \u{201c}{title}\u{201d}")
        });
    }

    /// The feature tab's view and its layout in the tab's area.
    fn feature_tab_parts(&self) -> Option<(chrome::FeatureTabView, chrome::FeatureTabLayout)> {
        let view = self.feature_tab_view()?;
        let layout = chrome::feature_tab_layout(&view, self.content_rect());
        Some((view, layout))
    }

    /// What is at `pos` on the feature tab.
    fn feature_tab_hit(&self, pos: Point<f64, Logical>) -> Option<FeatureTabHit> {
        let (view, layout) = self.feature_tab_parts()?;
        layout.hit(&view, pos.to_i32_round())
    }

    /// A left click in the feature tab: a stage's node shows its page, a
    /// repository takes the keyboard, a button does what it says.
    pub fn feature_panel_click(&mut self, pos: Point<f64, Logical>) {
        use crate::manual_test::Outcome;
        let Some(feature) = self.panel_feature().cloned() else {
            return;
        };
        let ctx = self.row_ctx(&feature);
        let Some(hit) = self.feature_tab_hit(pos) else {
            return;
        };
        let outcome = match hit {
            FeatureTabHit::Tab | FeatureTabHit::Column(_) | FeatureTabHit::AgentPickOff(_) => None,
            FeatureTabHit::MilestoneMode(i) => Some(PanelOutcome::FreshMilestones(i == 1)),
            FeatureTabHit::Finding(i) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    // An earlier review's card is a record: no keys to give it.
                    let stage = panel.page_stage(&feature);
                    if stage != Stage::AiReview || i < finding_cards(&feature, stage).len() {
                        panel.focus = TabArea::Findings;
                        panel.finding = i;
                    }
                }
                None
            }
            FeatureTabHit::AgentPick(i) => {
                let stage = (self.features.panel.as_ref())
                    .filter(|p| !p.keys)
                    .map(|p| p.page_stage(&feature));
                stage.and_then(|stage| {
                    let mut chips = self.chips_of(&feature.project, &feature.slug, stage);
                    (i < chips.len())
                        .then(|| PanelOutcome::PickAgent(stage, chips.swap_remove(i).name))
                })
            }
            FeatureTabHit::AgentModel => (self.features.panel.as_ref())
                .filter(|p| !p.keys)
                .map(|p| PanelOutcome::PickModel(p.page_stage(&feature))),
            FeatureTabHit::Hint(i) => {
                let keys = self
                    .feature_tab_view()
                    .and_then(|v| v.hints.get(i).cloned());
                if let Some((keys, _)) = keys {
                    self.press_hint(crate::input::Overlay::Features, &keys, pos);
                }
                None
            }
            FeatureTabHit::Prev | FeatureTabHit::Next => {
                let slugs = self.listed_slugs(&feature.project);
                let back = hit == FeatureTabHit::Prev;
                switch(&slugs, &feature.slug, back).map(PanelOutcome::Show)
            }
            FeatureTabHit::Node(i) => {
                if let (Some(panel), Some(stage)) =
                    (self.features.panel.as_mut(), Stage::ALL.get(i))
                {
                    panel.focus = TabArea::Stages;
                    panel.keys = false;
                    panel.pick(&feature, *stage);
                }
                None
            }
            FeatureTabHit::ToggleRepos => {
                self.toggle_feature_repos();
                None
            }
            // A chip unfolds the rows, on its own.
            FeatureTabHit::Repo(i) => {
                if (self.features.panel.as_ref()).is_some_and(|p| !p.expanded) {
                    self.toggle_feature_repos();
                }
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.select_row(i);
                }
                None
            }
            FeatureTabHit::MoreRepos(i) => {
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.select_row(i);
                }
                None
            }
            FeatureTabHit::RepoButton(i, b) => {
                // Its row takes the keyboard too.
                let panel = self.features.panel.as_mut();
                panel.and_then(|panel| {
                    panel.select_row(i);
                    panel.repo_pick = Some(b);
                    let entries = repo_entries(&feature, &panel.sources, panel.expanded);
                    let mut buttons = entry_buttons(&feature, &panel.sources, *entries.get(i)?);
                    (b < buttons.len()).then(|| buttons.swap_remove(b).1)
                })
            }
            FeatureTabHit::RepoRemove(i) => {
                let panel = self.features.panel.as_mut();
                panel.and_then(|panel| {
                    panel.select_row(i);
                    let entries = repo_entries(&feature, &panel.sources, panel.expanded);
                    match entries.get(i)? {
                        RepoEntry::Held(i) => {
                            Some(PanelOutcome::RemoveRepo(feature.repos.get(*i)?.key()))
                        }
                        RepoEntry::Context(i) => {
                            Some(PanelOutcome::DropContext(feature.context.get(*i)?.key()))
                        }
                        _ => None,
                    }
                })
            }
            FeatureTabHit::PageButton(i) => {
                let stage = (self.features.panel.as_ref())
                    .filter(|p| !p.keys)
                    .map(|p| p.page_stage(&feature));
                stage.and_then(|stage| {
                    let plan = self.planned_checks(&feature);
                    let rows = self.criterion_rows(&feature);
                    let mut actions = stage_actions(&feature, stage, ctx, &plan, &rows);
                    (i < actions.len()).then(|| actions.swap_remove(i).1)
                })
            }
            FeatureTabHit::Button(0) => Some(PanelOutcome::Open(feature.dir.clone())),
            FeatureTabHit::Button(1) => Some(PanelOutcome::CopyPath),
            FeatureTabHit::Button(2) => Some(PanelOutcome::New),
            FeatureTabHit::Button(_) => Some(PanelOutcome::Remove),
        };
        self.sync_feature_page();
        // A note being written: a click in it goes on writing, as does its own
        // card's button; one anywhere else keeps it first, and if it can't be
        // kept yet it waits.
        if self.writing_criterion_note() {
            let spot = self.feature_tab_view().and_then(|view| {
                chrome::feature_tab_field(&view, self.content_rect(), self.scale())
            });
            let in_it = spot.is_some_and(|field| field.area.to_f64().contains(pos));
            let at = (self.features.panel.as_ref()).and_then(|p| p.note.as_ref().map(|n| n.0));
            let own = matches!((&outcome, at),
                (Some(PanelOutcome::Manual(Outcome::Write(i))), Some(at)) if *i == at);
            let picked = (self.features.panel.as_ref()).map(|p| p.finding);
            if in_it || own || !self.keep_criterion_note() {
                return;
            }
            // Keeping it selects its card: the click was for another.
            if let Some(panel) = self.features.panel.as_mut()
                && matches!(hit, FeatureTabHit::Finding(_))
            {
                panel.finding = picked.unwrap_or(panel.finding);
            }
        }
        if let Some(outcome) = outcome {
            self.apply_feature_panel(outcome);
        }
    }

    /// The pointer moved while the feature tab shows: what is under it
    /// lights up, and a stage's node tells what is done in it.
    pub fn feature_panel_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let (view, layout) = self.feature_tab_parts()?;
        let hit = layout.hit(&view, pos.to_i32_round());
        if let Some(panel) = self.features.panel.as_mut() {
            panel.tab_hover = hit;
        }
        match hit? {
            FeatureTabHit::Node(i) => {
                let stage = Stage::ALL.get(i)?;
                let feature = self.panel_feature()?;
                let text = match feature.marks.get(stage) {
                    Some(mark) => mark_detail(mark, feature::now()),
                    None => stage.about().to_owned(),
                };
                Some(Tooltip::new(&text, *layout.nodes.get(i)?))
            }
            // Icons alone on a narrow tab: their names.
            FeatureTabHit::Button(i) if layout.compact => {
                let label = header_label(view.remove, i)?;
                Some(Tooltip::new(label, *layout.buttons.get(i)?))
            }
            FeatureTabHit::ToggleRepos => {
                let text = match view.expanded {
                    true => "Hide the repositories (Ctrl+Shift+L)",
                    false => "Show all repositories (Ctrl+Shift+L)",
                };
                Some(Tooltip::new(text, layout.repos_toggle))
            }
            // What joining or leaving makes of the repository.
            FeatureTabHit::RepoButton(i, b) => {
                let feature = self.panel_feature()?;
                let panel = self.features.panel.as_ref()?;
                let entries = repo_entries(feature, &panel.sources, panel.expanded);
                let buttons = entry_buttons(feature, &panel.sources, *entries.get(i)?);
                let text = repo_button_tip(feature, &buttons.get(b)?.1)?;
                let mut spot = *layout.repo_buttons.get(i)?.get(b)?;
                spot.loc.y -= view.scroll[TabColumn::Repos as usize].round() as i32;
                Some(Tooltip::new(&text, spot))
            }
            // Logos alone: their names.
            FeatureTabHit::AgentPick(i) | FeatureTabHit::AgentPickOff(i) => {
                let (_, chips) = layout.agent_picks.as_ref()?;
                let mut spot = *chips.get(i)?;
                spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                let chip = view.agent_chip(i)?;
                let text = match chip.off {
                    true => format!("{} built the code, so it can't review it", chip.name),
                    false => chip.name.clone(),
                };
                Some(Tooltip::new(&text, spot))
            }
            FeatureTabHit::AgentModel => {
                let mut spot = layout.agent_model?;
                spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                Some(Tooltip::new(
                    "The model its next start uses; an agent already open keeps its own",
                    spot,
                ))
            }
            FeatureTabHit::MilestoneMode(i) => {
                let (_, segments) = layout.milestone_mode.as_ref()?;
                let mut spot = *segments.get(i)?;
                spot.loc.y -= view.scroll[TabColumn::Page as usize].round() as i32;
                let text = match i {
                    0 => "One agent keeps its chat through all milestones",
                    _ => "A fresh agent builds each milestone, reading the build log and plan",
                };
                Some(Tooltip::new(text, spot))
            }
            FeatureTabHit::Prev | FeatureTabHit::Next => {
                let (_, prev, next) = layout.switch?;
                Some(match hit? {
                    FeatureTabHit::Prev => Tooltip::new("Previous feature", prev),
                    _ => Tooltip::new("Next feature", next),
                })
            }
            _ => None,
        }
    }

    /// Whether something that acts is at `pos` on the feature tab: the
    /// pointer is a hand over it.
    pub fn feature_tab_control_at(&self, pos: Point<f64, Logical>) -> bool {
        !matches!(
            self.feature_tab_hit(pos),
            None | Some(
                FeatureTabHit::Tab
                    | FeatureTabHit::Column(_)
                    | FeatureTabHit::AgentPickOff(_)
                    | FeatureTabHit::Finding(_)
            )
        )
    }

    /// The column of the feature tab at `pos`, as laid out now, and what
    /// it scrolls within.
    fn feature_tab_column(&self, pos: Point<f64, Logical>) -> Option<(TabColumn, Bounds)> {
        let (_, layout) = self.feature_tab_parts()?;
        let col = layout.column_at(pos.to_i32_round())?;
        Some((col, tab_bounds(&layout, col)))
    }

    /// The wheel or the touchpad over the feature tab: the column under
    /// the pointer moves.
    /// Over the narrow tab's row of repository chips, the wheel steps
    /// through the repositories instead. What is under the pointer is
    /// found again once the column stops (see `tick_scrolls`).
    pub fn feature_panel_scroll(&mut self, motion: Motion) {
        self.sync_feature_page();
        let pos = self.pointer.to_i32_round();
        let on_strip = (self.feature_tab_parts()).is_some_and(|(_, l)| {
            l.columns[TabColumn::Repos as usize].is_none() && l.strip.contains(pos)
        });
        if on_strip {
            return self.feature_strip_scroll(motion);
        }
        if let Some((col, bounds)) = self.feature_tab_column(self.pointer)
            && let Some(panel) = self.features.panel.as_mut()
        {
            panel.scroll[col as usize].apply(motion, Instant::now(), bounds);
        }
        // What is under the pointer is found again now, and once the
        // column stops (see `tick_scrolls`): a tooltip left goes.
        self.scrolling = true;
        let time = self.start_time.elapsed().as_millis() as u32;
        self.pointer_moved(self.pointer, time);
    }

    /// The wheel over the narrow tab's repository chips: the next one down
    /// (or up) takes the keyboard.
    fn feature_strip_scroll(&mut self, motion: Motion) {
        let sources = (self.features.panel.as_ref()).map_or(&[][..], |p| p.sources.as_slice());
        let Some(repos) = self
            .panel_feature()
            .map(|f| repo_entries(f, sources, false).len())
        else {
            return;
        };
        let rows = self.motion_rows(motion, TAB_NOTCH);
        if rows == 0 || repos == 0 {
            return;
        }
        if let Some(panel) = self.features.panel.as_mut() {
            panel.select_row(strip_step(panel.repo, rows, repos));
        }
        let time = self.start_time.elapsed().as_millis() as u32;
        self.pointer_moved(self.pointer, time);
    }

    /// Move the feature tab's columns on to `now` if they move on their
    /// own; whether one still does.
    pub fn feature_tab_scroll_tick(&mut self, now: Instant) -> bool {
        // Every frame: a page shown afresh (picked, or the feature moved
        // on) starts at its top.
        self.sync_feature_page();
        let moving = (self.features.panel.as_ref()).is_some_and(|p| {
            p.scroll
                .iter()
                .any(|s| s.moving(now) || s.overscroll() != 0.0)
        });
        if !moving {
            return false;
        }
        let Some((_, layout)) = self.feature_tab_parts() else {
            return false;
        };
        let Some(panel) = self.features.panel.as_mut() else {
            return false;
        };
        let mut still = false;
        for col in TabColumn::ALL {
            still |= panel.scroll[col as usize].tick(now, tab_bounds(&layout, col));
        }
        still
    }

    /// Stop the feature tab's columns flung; whether one was.
    pub fn feature_tab_stop_fling(&mut self) -> bool {
        (self.features.panel.as_mut()).is_some_and(|p| {
            p.scroll
                .iter_mut()
                .fold(false, |any, s| s.stop_fling() | any)
        })
    }

    /// The feature tab's view, while a feature's tab shows.
    pub fn feature_tab_view(&self) -> Option<chrome::FeatureTabView> {
        let panel = self.features.panel.as_ref()?;
        let feature = self.feature(&panel.project, &panel.slug)?;
        let slugs = self.listed_slugs(&panel.project);
        let mine = || {
            self.features
                .busy
                .iter()
                .filter(|b| b.project == panel.project && b.slug == panel.slug)
        };
        let busy: Vec<&str> = mine().filter_map(|b| b.repo.as_deref()).collect();
        let shipping = mine().any(|b| b.job == Job::Ship);
        let doing = |job: Job| mine().find(|b| b.job == job).map(|b| b.what.as_str());
        let key = (panel.project.clone(), panel.slug.clone());
        let probe = self.features.probes.get(&key).and_then(|p| p.last.as_ref());
        let now = feature::now();
        let current = feature.current().filter(|s| needs(*s).is_some());
        let run = current.and_then(|stage| Some((stage, feature.runs.get(&stage)?)));
        let agent_status = run.and_then(|(_, run)| self.stage_run_state(run).0);
        let agent = run.map(|(stage, run)| {
            let watch_key = (panel.project.clone(), panel.slug.clone(), stage);
            let watch = self.features.agents.get(&watch_key);
            crate::feature_agents::agent_line(run, stage, agent_status, watch, now)
        });
        let manual_counts = self.manual_counts(feature);
        let pane = run.and_then(|(_, run)| run.pane.as_deref());
        let tail = (self.features.tail.as_ref())
            .filter(|t| Some(t.pane.as_str()) == pane)
            .map_or(&[][..], |t| t.lines.as_slice());
        let state = PanelState {
            busy: &busy,
            names: self.projects.names(feature),
            probe,
            shipping,
            agent,
            agent_status,
            tail,
            ctx: self.row_ctx(feature),
            setup: doing(Job::Setup),
            sandbox: doing(Job::Sandbox),
            verify: doing(Job::Verify),
            plan: self.planned_checks(feature),
            check_tails: Some(&self.features.check_tails),
            code_review: self.code_review_line(&panel.project, &panel.slug),
            picks: self.chips_of(&panel.project, &panel.slug, panel.page_stage(feature)),
            model: self
                .model_slot_of(&panel.project, &panel.slug, panel.page_stage(feature))
                .map(|slot| model_label(slot.picked.as_deref())),
            manual: manual_counts.map(crate::manual_test::counts_text),
            criteria: self.criterion_rows(feature),
            manual_gate: manual_counts.map(manual_gate),
            rounds_open: panel.rounds_open.clone(),
        };
        Some(tab_view(panel, feature, &slugs, &state, now))
    }

    /// The sidebar's rows for the shown project's open features.
    pub fn feature_rows(&self) -> Vec<chrome::FeatureRowView> {
        self.sidebar_listed()
            .into_iter()
            .map(|f| chrome::FeatureRowView {
                title: self.row_title(f),
                card: self.feature_card(f),
            })
            .collect()
    }

    /// What `f`'s card says, on its row or its workspace's header.
    pub(crate) fn feature_card(&self, f: &Feature) -> chrome::FeatureCard {
        let mine = || {
            let busy = self.features.busy.iter();
            busy.filter(|b| b.project == f.project && b.slug == f.slug)
        };
        let doing = |job: Job| mine().find(|b| b.job == job).map(|b| b.what.as_str());
        let key = (f.project.clone(), f.slug.clone());
        let manual = self.manual_counts(f).map(manual_gate);
        let ctx = self.row_ctx(f);
        let line = LineState {
            probe: (self.features.probes.get(&key)).and_then(|p| p.last.as_ref()),
            shipping: mine().any(|b| b.job == Job::Ship),
            checks: ctx.checks,
            setup: doing(Job::Setup),
            verify: doing(Job::Verify),
            manual_gate: manual,
        };
        let gate = crate::feature_agents::gate_of(f, |_| manual).is_some();
        let busy = mine().next().is_some();
        let back = rework(f, ctx);
        let (stage, text, tone) = card_status(f, &line, gate, busy, back, feature::now());
        chrome::FeatureCard {
            marks: Stage::ALL.iter().map(|s| stage_mark(f, *s)).collect(),
            stage,
            line: text,
            tone,
            gate,
            repos: f.repos.len(),
        }
    }

    /// A sidebar row's title for `f`: another project's feature (one
    /// spanning the shown project) says whose it is.
    pub(crate) fn row_title(&self, f: &Feature) -> String {
        if self.project.as_deref() == Some(f.project.as_str()) {
            return f.title.clone();
        }
        format!("{} \u{b7} in {}", f.title, self.home_name(f))
    }

    /// The name of `f`'s home project.
    pub(crate) fn home_name<'a>(&'a self, f: &'a Feature) -> &'a str {
        (self.projects.project(&f.project)).map_or(f.project.as_str(), |p| &p.name)
    }

    /// Feature `i` of the sidebar's rows.
    pub fn sidebar_feature(&self, i: usize) -> Option<&Feature> {
        self.sidebar_listed().get(i).copied()
    }

    /// The features the sidebar's Features section lists, in order: the
    /// shown project's open ones with no workspace open (their own
    /// workspace's header shows them instead), then other projects' open
    /// features that span the shown project too.
    fn sidebar_listed(&self) -> Vec<&Feature> {
        let Some(project) = self.project.as_deref() else {
            return Vec::new();
        };
        let dirs: Vec<&Path> = self.workspaces.iter().map(|ws| ws.dir.as_path()).collect();
        let own = self
            .features
            .all
            .get(project)
            .map(|all| sidebar_features(all));
        let mut listed =
            crate::feature_workspace::without_workspace(own.unwrap_or_default(), &dirs);
        listed.extend(
            linked_in(&self.features.all, project)
                .into_iter()
                .map(|(_, f)| f),
        );
        listed
    }
}

/// The tasks panel's row for `busy`, work on the feature called `title`
/// (when it has one yet) of the project called `project`: setup and
/// Verify's checks name the feature when their progress doesn't.
fn task_of(busy: &Busy, title: Option<&str>, project: &str) -> Task {
    let chip = match busy.job {
        Job::Setup => "setup",
        Job::Verify => "checks",
        Job::Graph => "graph",
        Job::Sandbox => "sandbox",
        _ => "feature",
    };
    let detail = match title {
        Some(title) if !busy.what.contains(title) => {
            format!("\u{201c}{title}\u{201d} \u{b7} {project}")
        }
        _ => project.to_owned(),
    };
    let actions = match busy.job {
        Job::Setup => vec![(
            "Log",
            MenuAction::SetupLog(busy.project.clone(), busy.slug.clone()),
        )],
        _ => Vec::new(),
    };
    Task {
        title: busy.what.clone(),
        chip: chip.into(),
        detail,
        dot: Dot::Working,
        actions,
    }
}

/// What the toast says once feature `title` is made under `root`, naming
/// the files `.features` was added to (`ignores`) from the root: whole
/// paths are long enough to cut the last ones off.
fn created_text(title: &str, root: &Path, ignores: &[PathBuf]) -> String {
    if ignores.is_empty() {
        return format!("Created \u{201c}{title}\u{201d}");
    }
    let names: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| tilde(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "Created \u{201c}{title}\u{201d} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        names.join(", ")
    )
}

/// What the toast says once `names` were linked to feature `title`, and
/// the ignore files that took it (checklist D1).
fn linked_text(title: &str, names: &[String], root: &Path, ignores: &[PathBuf]) -> String {
    let text = format!("\u{201c}{title}\u{201d} holds {} now", names.join(", "));
    if ignores.is_empty() {
        return text;
    }
    let files: Vec<String> = ignores
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map_or_else(|_| tilde(p), |r| r.display().to_string())
        })
        .collect();
    format!(
        "{text} \u{b7} added {} to {}",
        layout::FEATURES_DIR,
        files.join(", ")
    )
}

/// Whether `feature`'s code is still the code whose patch-id is `patch`,
/// the one its Ship check was on, with nothing uncommitted; else why not.
/// Runs git: off the UI thread.
fn still_checked(feature: &Feature, patch: Option<&str>) -> Result<(), String> {
    let reading = patch::read(feature)?;
    if !reading.dirty.is_empty() {
        return Err(format!(
            "{} has uncommitted changes",
            reading.dirty.join(", ")
        ));
    }
    if patch != Some(reading.patch.as_str()) {
        return Err("its code changed since the check".into());
    }
    Ok(())
}

/// Drop repository `key` from `feature`, and the link it was the last of.
fn drop_repo(feature: &mut Feature, key: &str, now: u64) {
    if let Some(repo) = feature.repo(key).cloned() {
        feature.note_removed(repo.project, repo.name, now);
    }
    feature.repos.retain(|r| r.key() != key);
    let repos = &feature.repos;
    feature
        .links
        .retain(|l| repos.iter().any(|r| r.project.as_ref() == Some(&l.project)));
}

/// Whether taking `keys` out of `feature` asks first: when `losses` go
/// with them, and always for a repository on the feature's branch.
fn removal_asks(feature: &Feature, keys: &[String], losses: &[String]) -> bool {
    !losses.is_empty() || (keys.iter()).any(|k| feature.repo(k).is_some_and(|r| r.touched))
}

/// What a confirm dialog about taking repositories out says of `losses`,
/// what goes with their worktrees, and of what happens after: as context
/// agents go on reading them; else they can come back, on a new branch
/// and without what was lost. A line each: together they don't fit the
/// dialog's width.
fn remove_repos_body(repos: usize, losses: &[String], to_context: bool) -> String {
    let what = if repos == 1 {
        "Its worktree and branch go"
    } else {
        "Their worktrees and branches go"
    };
    let lost = match losses {
        [] => format!("{what}; nothing in them is lost"),
        losses => format!("{what}, and with them {}", losses.join(", ")),
    };
    let after = match (to_context, repos) {
        (true, 1) => "Agents started from now on read its main checkout",
        (true, _) => "Agents started from now on read their main checkouts",
        (false, 1) => "You can add it back, on a new branch",
        (false, _) => "You can add them back, on new branches",
    };
    format!("{lost}.\n{after}.")
}

/// What a confirm dialog about removing a feature's worktrees says of
/// `losses`, what goes with them.
fn removal_body(losses: &[String]) -> String {
    match losses {
        [] => "Its worktrees go; nothing in them is lost".into(),
        [one] => format!("Its worktrees go, and with them {one}"),
        more => format!("Its worktrees go, and with them {}", more.join(", ")),
    }
}

/// What `layout::find_root` goes by in project `project`: its workspaces'
/// folders, the repositories it ignores, and their base branches.
pub(crate) fn root_search(
    project: &crate::projects::Project,
) -> (Vec<PathBuf>, Vec<String>, Vec<(String, String)>) {
    let dirs = project.workspaces.iter().map(|w| w.dir.clone()).collect();
    let bases = (project.workflow.base.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (dirs, project.workflow.ignore.clone(), bases)
}

/// What a repository row's button does beyond its one word, as its
/// tooltip says it.
fn repo_button_tip(feature: &Feature, outcome: &PanelOutcome) -> Option<String> {
    match outcome {
        PanelOutcome::Touch(_) | PanelOutcome::AddToFeature(_) => {
            Some(format!("Add a worktree on {}", feature.branch()))
        }
        PanelOutcome::AddContext(_) => {
            Some("Agents can read its main checkout; no worktree".to_owned())
        }
        PanelOutcome::MakeContext(_) => {
            Some("Removes its worktree; agents can read its main checkout".to_owned())
        }
        _ => None,
    }
}

/// What the toast says when repository `named` becomes context: an agent
/// already running keeps what it started with.
fn context_text(named: &str) -> String {
    format!("Agents started from now on can read {named}")
}

/// Whether the repositories of `feature` can still change: open, and not
/// shipped.
fn linkable(feature: &Feature) -> bool {
    feature.status == Status::Open && !feature.marks.contains_key(&Stage::Ship)
}

/// What the new feature dialog shows of `dialog`, in project `project`.
fn new_feature_view(dialog: &NewFeatureDialog, project: String) -> NewFeatureView {
    let place = |base: &str, ticked: bool, fixed: bool| {
        let word = match (fixed, ticked) {
            (true, _) => "the only repository",
            (_, true) => "linked",
            (_, false) => "left out",
        };
        format!("from {base} \u{b7} {word}")
    };
    let found = |root: &Root| {
        let n = root.repos.len();
        if root.multi {
            format!("In {} \u{b7} {n} repositories", tilde(&root.path))
        } else {
            format!("In {}", tilde(&root.path))
        }
    };
    let (root, root_failed) = if dialog.picking && dialog.others.is_empty() {
        ("The store has no other project to link".to_owned(), true)
    } else {
        match &dialog.root {
            Finding::Looking => ("finding repositories\u{2026}".to_owned(), false),
            Finding::Failed(why) => (format!("No repositories found: {why}"), true),
            Finding::Found(root) => (found(root), false),
        }
    };
    let repos = dialog
        .rows()
        .into_iter()
        .filter_map(|row| {
            let fixed = dialog.fixed(row);
            Some(match row {
                DialogRow::Home(i) => {
                    let Finding::Found(root) = &dialog.root else {
                        return None;
                    };
                    let (r, touched) = (root.repos.get(i)?, *dialog.touched.get(i)?);
                    NewFeatureRepo {
                        name: r.name.clone(),
                        place: place(&r.base, touched, fixed),
                        touched,
                        fixed,
                    }
                }
                DialogRow::Linked(k, i) => {
                    let other = &dialog.others[k];
                    let Finding::Found(root) = &other.root else {
                        return None;
                    };
                    let (r, touched) = (root.repos.get(i)?, *other.touched.get(i)?);
                    NewFeatureRepo {
                        name: format!("{}: {}", other.name, r.name),
                        place: place(&r.base, touched, false),
                        touched,
                        fixed: false,
                    }
                }
                DialogRow::Project(k) => {
                    let other = &dialog.others[k];
                    let place = match &other.root {
                        Finding::Looking => "finding repositories\u{2026}".to_owned(),
                        Finding::Failed(why) => format!("can't link: {why}"),
                        Finding::Found(root) => found(root),
                    };
                    NewFeatureRepo {
                        name: other.name.clone(),
                        place,
                        touched: other.linked,
                        fixed,
                    }
                }
            })
        })
        .collect();
    let mut projects = format!("Projects: {project} (home)");
    for other in dialog.others.iter().filter(|o| o.linked) {
        projects.push_str(&format!(" \u{b7} {}", other.name));
    }
    let mut keys = if dialog.multi() {
        vec![("Tab", "next"), ("Space", "tick")]
    } else {
        Vec::new()
    };
    keys.push(if dialog.picking {
        ("Ctrl+Shift+L", "repositories")
    } else {
        ("Ctrl+Shift+L", "link project")
    });
    // Esc steps back as `NewFeatureDialog::key` has it.
    keys.push(if dialog.picking || dialog.focus.is_some() {
        ("Esc", "back")
    } else if !dialog.title.is_empty() {
        ("Esc", "clear")
    } else {
        ("Esc", "close")
    });
    let heading = format!("New feature in \u{201c}{project}\u{201d}");
    NewFeatureView {
        heading,
        title: dialog.title.line(),
        focus: dialog.focus,
        root,
        root_failed,
        projects,
        link: chrome::LINK_BUTTON[usize::from(dialog.picking)],
        repos,
        top: dialog.top,
        error: dialog.error.clone(),
        hover: dialog.hover,
        keys,
    }
}

/// What the feature panel shows of a feature besides the feature itself.
#[derive(Default)]
struct PanelState<'a> {
    /// The repositories being touched, by `Repo::key`.
    busy: &'a [&'a str],
    /// The feature's projects' names, for its repositories' labels.
    names: feature::PerProject<String>,
    /// Its latest probe.
    probe: Option<&'a Probe>,
    /// Its pull requests are being opened.
    shipping: bool,
    /// What its current stage's agent does, for that stage's row (see
    /// `feature_agents::agent_line`).
    agent: Option<String>,
    /// Its current stage's agent's status, while its tab is there.
    agent_status: Option<AgentStatus>,
    /// The last lines of its current stage's agent's pane, as last read.
    tail: &'a [String],
    /// What its rows need besides it.
    ctx: RowCtx,
    /// What its setup does now, while it runs.
    setup: Option<&'a str>,
    /// What is being done to its sandbox now.
    sandbox: Option<&'a str>,
    /// What Verify's checks do now, while they run.
    verify: Option<&'a str>,
    /// The checks its project gives Verify, and what the feature skips.
    plan: Vec<PlannedCheck>,
    /// The last lines of the panes of Verify's running checks, as last read.
    check_tails: Option<&'a HashMap<String, AgentTail>>,
    /// What Code review's row says once the diff was read (see
    /// `code_review_line`).
    code_review: Option<String>,
    /// How far Manual test's criteria went, in words, when the brief has
    /// some (see `manual_test::counts_text`).
    manual: Option<String>,
    /// How many of Manual test's criteria passed, of how many (see
    /// `feature_agents::gate_of`).
    manual_gate: Option<(u32, u32)>,
    /// Manual test's criteria, as their cards show them.
    criteria: Vec<Row>,
    /// The earlier AI reviews showing their findings (see
    /// `FeaturePanel::rounds_open`).
    rounds_open: BTreeSet<u32>,
    /// The chips to pick the agent of the stage on the page with (see
    /// `feature_agents::stage_chips`).
    picks: Vec<AgentChip>,
    /// The button picking its model, where gerdr can start the agent on
    /// one (see `feature_agents::stage_model_slot`).
    model: Option<String>,
}

/// What the button picking a stage's model says.
fn model_label(picked: Option<&str>) -> String {
    format!("Model: {}", picked.unwrap_or("default"))
}

/// `what` a task does as an inline status: "Setting up “T”" reads
/// "setting up “T”"; "services-api: npm ci (2 of 3)" stays.
fn inline(what: &str) -> String {
    let mut chars = what.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => first
            .to_lowercase()
            .chain(what[first.len_utf8()..].chars())
            .collect(),
        _ => what.to_owned(),
    }
}

/// The detail of Build's row, not marked done, beside `agent`, its
/// agent's line when Build is current: setup first, while it runs (`setup`
/// says what it does), or why it failed before an agent started. With an
/// agent, its line comes first: what it waits for matters more.
fn build_line(feature: &Feature, setup: Option<&str>, agent: Option<&str>) -> Option<String> {
    if let Some(what) = setup {
        // The task's text names the feature: its own panel needn't.
        let own = format!("Setting up \u{201c}{}\u{201d}", feature.title);
        let rest = what
            .strip_prefix(&own)
            .map(|r| r.trim_start_matches(':').trim());
        return Some(match rest {
            Some("") => "setting up".to_owned(),
            Some(rest) => format!("setting up: {rest}"),
            None => format!("setting up: {}", inline(what)),
        });
    }
    let failed = feature.setup.as_ref().and_then(|s| s.failed.as_deref());
    match (agent, failed) {
        (Some(agent), Some(_)) => Some(format!("{agent} \u{b7} setup failed")),
        (None, Some(why)) => Some(format!("setup failed: {why}")),
        (agent, None) => agent.map(str::to_owned),
    }
}

/// The detail of Verify's row, not marked done: what its checks do while
/// they run (`running`), that there are none to run, or what failed and
/// who fixes it.
fn verify_line(feature: &Feature, ctx: RowCtx, running: Option<&str>) -> Option<String> {
    if let Some(what) = running {
        return Some(inline(what));
    }
    let current = feature.status == Status::Open && feature.current() == Some(Stage::Verify);
    if current && !ctx.checks {
        return Some(match ctx.all_skipped {
            true => "every check is skipped: stop skipping one, or Mark done by hand".into(),
            false => "no checks for its repositories: set some, or Mark done by hand".into(),
        });
    }
    let run = feature
        .verify
        .as_ref()
        .filter(|v| v.passed == Some(false))?;
    let line = match run.failures.first() {
        Some(first) => format!("failed: {first}"),
        None => "failed".to_owned(),
    };
    Some(line)
}

/// The line under repository `repo` of `feature`: its branch, its pull
/// request, and a push not guarded as `probe` found; for one it doesn't
/// change, its base.
fn repo_line(feature: &Feature, repo: &feature::Repo, probe: Option<&Probe>) -> String {
    if !repo.touched {
        return format!("context, at {}", repo.base);
    }
    let mut line = repo.branch.clone().unwrap_or_else(|| feature.branch());
    if let Some(pr) = pr_of(feature, &repo.key()) {
        line.push_str(&format!(" \u{b7} PR {}", pr_tail(&pr.url)));
    }
    if let Some(Guard::Off(why)) = probe.and_then(|p| p.guards.get(&repo.key())) {
        line.push_str(&format!(" \u{b7} push not guarded: {why}"));
    }
    line
}

/// What a stage of `feature` not marked done says of where it stands,
/// as `state` has it: what setup or the checks do, the agent's line on
/// the current stage, what waits on AI review, Code review and Manual
/// test.
fn stage_line(feature: &Feature, stage: Stage, state: &PanelState) -> Option<String> {
    let current = stage_mark(feature, stage) == StageMark::Current;
    let agent = state.agent.as_deref().filter(|_| current);
    match stage {
        Stage::Build => build_line(feature, state.setup, agent),
        Stage::Verify => verify_line(feature, state.ctx, state.verify),
        Stage::AiReview if current => review_line(feature, agent),
        Stage::CodeReview if current => {
            Some((state.code_review.clone()).unwrap_or_else(|| "review every changed file".into()))
        }
        Stage::ManualTest if current => state.manual.clone(),
        _ => agent.map(str::to_owned),
    }
}

#[cfg(test)]
/// What `stage` of `feature` says in one line: when it was done, else
/// where it stands, else what it is for.
fn stage_detail(feature: &Feature, stage: Stage, state: &PanelState, now: u64) -> String {
    match (feature.marks.get(&stage), stage_line(feature, stage, state)) {
        (Some(m), _) => mark_detail(m, now),
        (None, Some(line)) => line,
        (None, None) => stage.about().to_owned(),
    }
}

/// The first stage of `feature` whose mark the code changed since, as the
/// latest probe read it.
fn stale_from(feature: &Feature, probe: Option<&Probe>) -> Option<Stage> {
    if feature.status != Status::Open {
        return None;
    }
    match probe.map(|p| &p.reading) {
        Some(Ok(reading)) => feature.stale(&reading.patch),
        _ => None,
    }
}

/// Whether done `stage` of `feature` no longer holds: bound to the code,
/// and the code changed since it or a stage before it was marked.
fn is_stale(feature: &Feature, stage: Stage, probe: Option<&Probe>) -> bool {
    feature.is_done(stage)
        && stage.bound()
        && stale_from(feature, probe).is_some_and(|from| stage_index(stage) >= stage_index(from))
}

/// How many of Manual test's criteria passed, of how many (see
/// `feature_agents::gate_of`).
fn manual_gate(n: crate::feature::manual::Counts) -> (u32, u32) {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (count(n.passed), count(n.total()))
}

/// What a current stage's short line reads besides its feature (see
/// `current_line`), on the rail and on the sidebar's cards.
struct LineState<'a> {
    probe: Option<&'a Probe>,
    shipping: bool,
    checks: bool,
    setup: Option<&'a str>,
    verify: Option<&'a str>,
    manual_gate: Option<(u32, u32)>,
}

impl<'a> From<&PanelState<'a>> for LineState<'a> {
    fn from(state: &PanelState<'a>) -> Self {
        Self {
            probe: state.probe,
            shipping: state.shipping,
            checks: state.ctx.checks,
            setup: state.setup,
            verify: state.verify,
            manual_gate: state.manual_gate,
        }
    }
}

/// The stage a card shows, its line and its tone. Work sent back to Build
/// speaks for the card, as it does on the rail (see `Rework::node`).
fn card_status(
    feature: &Feature,
    state: &LineState,
    gate: bool,
    busy: bool,
    back: Option<Rework>,
    now: u64,
) -> (Option<Stage>, String, chrome::CardTone) {
    // A round's agent at work says what it does; one waiting for its prompt
    // has the user to act.
    let round = feature.fix_round.as_ref();
    if let Some(back) = back.filter(|b| round.is_none() || !matches!(b, Rework::Runs(_))) {
        let line = format!("{} \u{b7} {}", Stage::Build.label(), back.node().1);
        return (Some(Stage::Build), line, back.tone());
    }
    if let Some(round) = round {
        let tone = match (round.phase, round.stalled.is_some()) {
            (FixPhase::Ready, _) => chrome::CardTone::Waits,
            (_, true) => chrome::CardTone::Failed,
            _ => chrome::CardTone::Working,
        };
        let line = format!("{} \u{b7} {}", round.stage.label(), round.line());
        return (Some(round.stage), line, tone);
    }
    let stage = feature.current();
    let line = card_line(feature, stage, state, now);
    (stage, line, card_tone(feature, stage, state, gate, busy))
}

/// A card's line for `feature` at `stage`: the stage and how it stands,
/// else how long nothing happened to it.
fn card_line(feature: &Feature, stage: Option<Stage>, state: &LineState, now: u64) -> String {
    let Some(stage) = stage else {
        return "Every stage done".to_owned();
    };
    let mut what = current_line(feature, stage, state, now);
    if what.is_empty() {
        let last = feature.history.iter().map(|e| e.at).max();
        what = projects::ago(now, last.unwrap_or(feature.created));
    }
    format!("{} \u{b7} {what}", stage.label())
}

/// How `feature` at `stage` stands, for its card; `gate`: a gate holds it,
/// `busy`: a job of it runs.
fn card_tone(
    feature: &Feature,
    stage: Option<Stage>,
    state: &LineState,
    gate: bool,
    busy: bool,
) -> chrome::CardTone {
    let run = stage.and_then(|stage| feature.runs.get(&stage));
    let failed = match stage {
        Some(Stage::Build) => {
            run.is_none() && (feature.setup.as_ref()).is_some_and(|s| s.failed.is_some())
        }
        Some(Stage::Verify) => (feature.verify.as_ref()).is_some_and(|v| v.passed == Some(false)),
        Some(Stage::Ship) => {
            let check = state.probe.and_then(|p| p.ship.as_ref());
            check.is_some_and(|c| !c.blockers.is_empty())
        }
        _ => false,
    };
    if failed {
        chrome::CardTone::Failed
    } else if gate {
        chrome::CardTone::Waits
    } else if busy || run.is_some_and(feature::agents::StageRun::live) {
        chrome::CardTone::Working
    } else {
        chrome::CardTone::Idle
    }
}

/// Whether the current `stage` of `feature` waits for the user: a file to
/// review, a stop to go on from, findings to decide (see
/// `feature_agents::gate_of`).
fn waits(feature: &Feature, stage: Stage, state: &PanelState) -> bool {
    crate::feature_agents::gate_of(feature, |_| state.manual_gate).is_some_and(|g| g.0 == stage)
}

/// "running 14m": how long an agent started at `started` has run.
fn running_for(now: u64, started: u64) -> String {
    match projects::ago(now, started).strip_suffix(" ago") {
        Some(long) => format!("running {long}"),
        None => "running".to_owned(),
    }
}

/// The short line under current `stage`'s node on the rail.
fn current_line(feature: &Feature, stage: Stage, state: &LineState, now: u64) -> String {
    if let Some(round) = fix_round_on(feature, stage) {
        let waits = (feature.runs.get(&Stage::Build)).is_some_and(|run| run.waits_to_send());
        return match waits {
            true => "waiting to send".to_owned(),
            false => round.line().to_owned(),
        };
    }
    let run = feature.runs.get(&stage);
    let agent = || match run {
        Some(run) if run.t3 == Some(feature::agents::T3Run::Draft) => "draft in T3 Code".to_owned(),
        Some(run) if run.waits_to_send() => "waiting to send".to_owned(),
        Some(run) if run.live() => running_for(now, run.started),
        Some(_) => "agent closed".to_owned(),
        None => String::new(),
    };
    let failed_setup = (feature.setup.as_ref()).is_some_and(|s| s.failed.is_some());
    match stage {
        Stage::Discover | Stage::Plan if run.is_some_and(|r| r.ready.is_some()) => {
            "ready for review".into()
        }
        Stage::PlanReview => match crate::feature_plan_review::deciding(feature) {
            Some(review) if review.open() > 0 => format!("{} to decide", review.open()),
            Some(_) => "all decided".into(),
            None if run.is_some_and(|r| r.ready.is_some()) => "ready for review".into(),
            None => agent(),
        },
        Stage::Build if state.setup.is_some() => "setting up".into(),
        Stage::Build if failed_setup && run.is_none() => "setup failed".into(),
        Stage::Build if run.is_some_and(|r| r.blocked.is_some()) => "blocked".into(),
        Stage::Build => match run.and_then(|r| r.stop) {
            Some(stop) => format!("milestone {}/{}", stop.done, stop.total),
            None => agent(),
        },
        Stage::Verify if state.verify.is_some() => "running".into(),
        Stage::Verify => match &feature.verify {
            Some(v) if v.passed == Some(false) => "failed".into(),
            _ if !state.checks => "mark by hand".into(),
            _ => String::new(),
        },
        Stage::AiReview => {
            let read = (feature.review.as_ref()).filter(|_| run.is_some_and(|r| r.ready.is_some()));
            match read {
                Some(r) if r.count(FindingState::Open) > 0 => {
                    format!("{} to decide", r.count(FindingState::Open))
                }
                Some(r) if r.count(FindingState::Fix) > 0 => {
                    format!("{} to fix", r.count(FindingState::Fix))
                }
                Some(_) => "findings read".into(),
                None => agent(),
            }
        }
        Stage::CodeReview => "to review".into(),
        Stage::ManualTest => match state.manual_gate {
            Some((passed, total)) if total > 0 => format!("{passed} of {total} passed"),
            _ => String::new(),
        },
        Stage::Ship => {
            let check = state.probe.and_then(|p| p.ship.as_ref());
            if state.shipping {
                "shipping".into()
            } else if feature.shipping.squash_at.is_some() {
                "merging".into()
            } else if check.is_some_and(|c| !c.blockers.is_empty()) {
                "blocked".into()
            } else if check.is_some() {
                "ready to ship".into()
            } else {
                String::new()
            }
        }
        Stage::Discover | Stage::Plan => agent(),
    }
}

/// `stage`'s node on the rail of `feature`.
fn rail_node(feature: &Feature, stage: Stage, state: &PanelState, now: u64) -> RailNode {
    let done = stage_mark(feature, stage) == StageMark::Done;
    let agent = (feature.runs.get(&stage).filter(|_| done)).map(|run| {
        let what = run.model.as_deref().map_or(run.agent.as_str(), short_model);
        (AgentIcon::from_agent(&run.agent), what.to_owned())
    });
    let back = rework(feature, state.ctx).filter(|_| stage == Stage::Build);
    let (mark, line) = match (stage_mark(feature, stage), back) {
        (_, Some(back)) => back.node(),
        (StageMark::Done, _) if is_stale(feature, stage, state.probe) => {
            (NodeMark::Stale, "code changed".to_owned())
        }
        (StageMark::Done, _) => {
            let at = feature.marks.get(&stage).map_or(0, |m| m.at);
            (NodeMark::Done, projects::ago(now, at))
        }
        (StageMark::Current, _) => {
            let mark = match waits(feature, stage, state) {
                true => NodeMark::Waiting,
                false => NodeMark::Current,
            };
            (mark, current_line(feature, stage, &state.into(), now))
        }
        (StageMark::Later, _) => (NodeMark::Later, String::new()),
    };
    RailNode {
        label: stage.label().to_owned(),
        line,
        mark,
        selected: false,
        agent,
    }
}

/// `model` where its agent's logo stands beside it: Claude's says the first
/// word of its names.
fn short_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

/// A line in `tone`.
fn line(text: impl Into<String>, tone: CheckTone) -> CheckLine {
    CheckLine {
        text: text.into(),
        tone,
    }
}

/// A labelled section of `lines`.
fn lines(label: &str, lines: Vec<CheckLine>) -> PageSection {
    PageSection::Lines {
        label: label.to_owned(),
        lines,
    }
}

/// The indices in `actions` of what acts on `stage`'s agent: the buttons of
/// its card.
fn agent_buttons(stage: Stage, actions: &[(PageButton, PanelOutcome)]) -> Vec<usize> {
    (actions.iter().enumerate())
        .filter(|(_, (_, outcome))| match outcome {
            PanelOutcome::SendPrompt(s)
            | PanelOutcome::ShowAgent(s)
            | PanelOutcome::ResumeAgent(s)
            | PanelOutcome::Continue(s)
            | PanelOutcome::StartAgent(s, _) => *s == stage,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

/// The card of `stage`'s agent in `feature`, once one ran or one can be
/// picked, with the page's buttons in `actions` that act on it.
fn agent_card(
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
            icon: AgentIcon::Generic,
            name: String::new(),
            dot: Dot::Unknown,
            status: String::new(),
            tail: Vec::new(),
            buttons,
            picks,
            model: state.model.clone(),
        });
    };
    let mark = stage_mark(feature, stage);
    let current = mark == StageMark::Current;
    let back = rework(feature, state.ctx).filter(|_| stage == Stage::Build);
    let dot = match (mark, back) {
        (_, Some(Rework::Runs(dot))) => dot,
        (StageMark::Done, _) => Dot::Done,
        (StageMark::Current, _) if run.live() => {
            state.agent_status.map_or(Dot::Unknown, Dot::of_status)
        }
        _ => Dot::Unknown,
    };
    // The card's head names the agent already.
    let status = match (current, &state.agent) {
        // On a done Build too, where fixes wait: `started` would hide that.
        _ if run.waits_to_send() => {
            "waiting for you \u{b7} pick a model and send the prompt".to_owned()
        }
        (true, Some(line)) => (line.strip_prefix(run.agent.as_str()))
            .map_or(line.as_str(), str::trim_start)
            .to_owned(),
        _ => {
            let started = format!("started {}", projects::ago(now, run.started));
            match &run.model {
                Some(model) => format!("{model} \u{b7} {started}"),
                None => started,
            }
        }
    };
    let status = back.map_or(status, |back| back.status().to_owned());
    // The tail read is the current stage's agent's, while its pane is.
    let tail = match current && run.pane.is_some() {
        true => state.tail.to_vec(),
        false => Vec::new(),
    };
    Some(PageSection::Agent {
        icon: AgentIcon::from_agent(&run.agent),
        name: run.agent.clone(),
        dot,
        status,
        tail,
        buttons,
        picks,
        model: state.model.clone(),
    })
}

/// The Build agent's card on the page of the stage `feature` is at, while
/// work sent back to that agent is under way: this page's own agent is
/// another one, and this is the page the user comes back to.
fn rework_card(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Option<PageSection> {
    let back = rework_on(feature, stage, state.ctx)?;
    let run = feature.runs.get(&Stage::Build)?;
    Some(PageSection::Agent {
        icon: AgentIcon::from_agent(&run.agent),
        name: run.agent.clone(),
        dot: match back {
            Rework::Runs(dot) => dot,
            _ => Dot::Unknown,
        },
        status: format!("Build agent \u{b7} {}", back.status()),
        tail: Vec::new(),
        buttons: agent_buttons(Stage::Build, actions),
        picks: Vec::new(),
        model: None,
    })
}

/// The card of Discover's or Plan's file in `feature`: how it stands, with
/// the page's button to its tab in `actions`, plain or the one to press next.
fn file_card(
    feature: &Feature,
    stage: Stage,
    ctx: RowCtx,
    actions: &[(PageButton, PanelOutcome)],
) -> PageSection {
    let own = PanelOutcome::OpenDoc(stage);
    PageSection::File {
        name: crate::feature::artifacts::name(stage)
            .unwrap_or("file")
            .to_owned(),
        state: file_line(feature, stage, ctx),
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| *o == own)
            .map(|(i, _)| i)
            .collect(),
    }
}

/// The open fix round's card: what it fixes and how it stands, with the
/// page's buttons in `actions` that act on the plan.
fn fix_round_card(
    feature: &Feature,
    round: &FixRound,
    actions: &[(PageButton, PanelOutcome)],
) -> PageSection {
    let waits = (feature.runs.get(&Stage::Build)).is_some_and(|run| run.waits_to_send());
    let tone = match (round.phase, round.stalled.is_some()) {
        _ if waits => CheckTone::Note,
        (FixPhase::Ready, _) => CheckTone::Ready,
        (_, true) => CheckTone::Blocker,
        _ => CheckTone::Note,
    };
    let said = match waits {
        true => "waiting for you \u{b7} pick a model and send the prompt",
        false => round.stalled.as_deref().unwrap_or(round.line()),
    };
    PageSection::File {
        name: "Fix plan".to_owned(),
        state: line(format!("{} \u{b7} {said}", round.what), tone),
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

/// The card of the last fix round: its plan and what it fixed stay at hand.
fn last_round_card(round: &FixRound, actions: &[(PageButton, PanelOutcome)]) -> PageSection {
    PageSection::File {
        name: "Fix plan".to_owned(),
        state: line(
            format!("{} \u{b7} {}", round.what, feature::fix_round::SENT),
            CheckTone::Note,
        ),
        buttons: (actions.iter().enumerate())
            .filter(|(_, (_, o))| {
                matches!(o, PanelOutcome::ReviewFixPlan | PanelOutcome::MakeFixes)
            })
            .map(|(i, _)| i)
            .collect(),
    }
}

/// How Discover's or Plan's file stands in `feature`, as `ctx` found it.
/// A restarted agent leaves the last run's file be until it writes its
/// own, so the card doesn't say it is gone.
fn file_line(feature: &Feature, stage: Stage, ctx: RowCtx) -> CheckLine {
    if feature.is_done(stage) {
        let text = match ctx.locked_by(feature, stage) {
            Some(_) => "approved \u{b7} locked",
            None => "approved",
        };
        return line(text, CheckTone::Ready);
    }
    let written = ctx.wrote(stage);
    match feature.runs.get(&stage) {
        Some(run) if run.ready.is_some() => line("ready for review", CheckTone::Ready),
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
fn checks_lines(feature: &Feature, state: &PanelState) -> Vec<CheckLine> {
    if let Some(what) = state.verify {
        return vec![line(inline(what), CheckTone::Note)];
    }
    last_checks(feature, state)
}

/// Verify's page below its status: a card for each check, under a heading
/// that counts them and says what the cards don't.
fn verify_sections(
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
            let mut out = vec![line(format!("run {} failed", run.n), CheckTone::Blocker)];
            // A check's card says how it failed; this says the rest.
            let on_card = |f: &String| {
                (run.checks.iter())
                    .any(|c| c.outcome.is_some() && f.starts_with(&format!("{}: `", c.repo)))
            };
            let failures = (run.failures.iter())
                .filter(|f| !on_card(f))
                .map(|f| line(f.clone(), CheckTone::Blocker));
            out.extend(capped(failures.collect(), CheckTone::Blocker));
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
fn sandbox_sections(feature: &Feature, state: &PanelState) -> Vec<PageSection> {
    if let Some(what) = state.sandbox {
        return vec![lines("Sandbox", vec![line(inline(what), CheckTone::Note)])];
    }
    let mut out = Vec::new();
    let Some(run) = feature.sandbox.as_ref() else {
        let text = "the project running here, on a copy of its containers and data";
        return vec![lines("Sandbox", vec![line(text, CheckTone::Note)])];
    };
    if let Some(why) = &run.failed {
        return vec![lines(
            "Sandbox",
            vec![line(format!("failed: {why}"), CheckTone::Blocker)],
        )];
    }
    if run.alive() {
        out.push(line("running", CheckTone::Ready));
        let ports = run.listening();
        if !ports.is_empty() {
            let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
            out.push(line(
                format!("listening on {}", ports.join(", ")),
                CheckTone::Note,
            ));
        }
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
    out.push(line(
        "files, unix sockets and the internet are shared with main",
        CheckTone::Note,
    ));
    let mut sections = vec![lines("Sandbox", capped(out, CheckTone::Note))];
    let mut out = Vec::new();
    for forward in &run.forwards {
        let text = format!(
            "{}: port {} here, 127.0.0.1:{} from outside",
            forward.service, forward.main_port, forward.host_port
        );
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

fn setup_section(feature: &Feature, state: &PanelState) -> Option<PageSection> {
    if let Some(what) = state.setup {
        return Some(lines("Setup", vec![line(inline(what), CheckTone::Note)]));
    }
    let run = feature
        .setup
        .as_ref()
        .filter(|run| run.finished.is_some())?;
    let failed = (run.failed.iter()).map(|why| line(format!("failed: {why}"), CheckTone::Blocker));
    let missed = (run.not_copied.iter())
        .map(|what| line(format!("couldn't copy {what}"), CheckTone::Blocker));
    let copied = (run.copied.iter()).map(|what| line(format!("copied {what}"), CheckTone::Note));
    let ran = (run.ran.iter()).map(|what| line(format!("ran {what}"), CheckTone::Note));
    let mut out: Vec<_> = failed.chain(missed).chain(copied).chain(ran).collect();
    if out.is_empty() {
        out.push(line("nothing was missing", CheckTone::Note));
    }
    Some(lines("Setup", capped(out, CheckTone::Note)))
}

/// AI review's findings in `feature`, by state, once read.
fn suggestions_section(feature: &Feature) -> Option<PageSection> {
    use crate::feature::plan_review::State;
    let review = feature.plan_review.as_ref().filter(|r| r.stamp.is_some())?;
    if review.suggestions.is_empty() {
        return Some(lines(
            "Suggestions",
            vec![line("nothing to change", CheckTone::Ready)],
        ));
    }
    let states = [
        (State::Open, "to decide", CheckTone::Blocker),
        (State::Accepted, "accepted", CheckTone::Ready),
        (State::Rejected, "rejected", CheckTone::Note),
    ];
    let mut rows: Vec<CheckLine> = (states.into_iter())
        .filter_map(|(state, what, tone)| match review.count(state) {
            0 => None,
            n => Some(line(format!("{n} {what}"), tone)),
        })
        .collect();
    let not_found = review.not_found();
    if not_found > 0 {
        let text = format!("{} not found in the code", plural(not_found, "citation"));
        rows.push(line(text, CheckTone::Blocker));
    }
    Some(lines("Suggestions", rows))
}

fn findings_lines(feature: &Feature) -> Vec<CheckLine> {
    let ready = (feature.runs.get(&Stage::AiReview)).is_some_and(|r| r.ready.is_some());
    let Some(review) = feature.review.as_ref().filter(|_| ready) else {
        return vec![line("not read yet", CheckTone::Note)];
    };
    if review.findings.is_empty() {
        return vec![line("no findings", CheckTone::Ready)];
    }
    let states = [
        (FindingState::Open, "to decide", CheckTone::Blocker),
        (FindingState::Fix, "to fix", CheckTone::Blocker),
        (FindingState::Sent, "sent to Build", CheckTone::Note),
        (FindingState::Fixed, "fixed", CheckTone::Ready),
        (FindingState::Dismissed, "dismissed", CheckTone::Note),
    ];
    states
        .into_iter()
        .filter_map(|(state, what, tone)| match review.count(state) {
            0 => None,
            n => Some(line(format!("{} {what}", plural(n, "finding")), tone)),
        })
        .collect()
}

/// How AI review's findings in `feature` stand, in a few words: "4 to
/// decide · 1 to fix".
fn findings_count(feature: &Feature) -> String {
    let Some(review) = feature.review.as_ref() else {
        return String::new();
    };
    let shown = review.shown();
    [
        (FindingState::Open, "to decide"),
        (FindingState::Fix, "to fix"),
        (FindingState::Sent, "sent to Build"),
        (FindingState::Fixed, "fixed"),
        (FindingState::Dismissed, "dismissed"),
    ]
    .into_iter()
    .filter_map(
        |(state, what)| match shown.iter().filter(|f| f.state == state).count() {
            0 => None,
            n => Some(format!("{n} {what}")),
        },
    )
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// The milestones of `stage`'s file (Build's: nothing ticks a plan's) by
/// name, as `probe` last read them; none while it has none.
fn milestones_of(probe: Option<&Probe>, stage: Stage) -> Option<PageSection> {
    let items = probe?.docs.milestones.get(&stage)?;
    (!items.is_empty()).then(|| PageSection::Milestones {
        done: items.iter().filter(|(done, _)| *done).count(),
        total: items.len(),
        items: items.clone(),
    })
}

/// `stage`'s file to read on its page, as `probe` last read it; none while
/// it has none.
fn doc_of(probe: Option<&Probe>, stage: Stage) -> Option<PageSection> {
    let blocks = probe?.docs.previews.get(&stage)?;
    Some(PageSection::Doc {
        name: crate::feature::artifacts::name(stage)?.to_owned(),
        blocks: blocks.clone(),
    })
}

/// Whether `stage`'s page of `feature` offers the choice of one agent or a
/// fresh one per milestone: Build's, until it is done.
fn milestone_switch(feature: &Feature, stage: Stage) -> bool {
    stage == Stage::Build && feature.status == Status::Open && !feature.is_done(stage)
}

/// `stage`'s page of `feature`, its buttons `actions`.
fn stage_page(
    feature: &Feature,
    stage: Stage,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
    now: u64,
) -> StagePage {
    let mark = stage_mark(feature, stage);
    let mut sections = Vec::new();
    let status = match mark {
        StageMark::Later => {
            let before = Stage::ALL[stage_index(stage).saturating_sub(1)];
            Some(line(
                format!("Comes after {}", before.label()),
                CheckTone::Note,
            ))
        }
        StageMark::Done => feature.marks.get(&stage).map(|m| {
            let mut text = format!("Done {} by {}", projects::ago(now, m.at), m.by);
            if let Some(by) = state.ctx.locked_by(feature, stage) {
                text.push_str(&format!(" \u{b7} locked: {}", lock_why(feature, by)));
            }
            line(text, CheckTone::Ready)
        }),
        StageMark::Current => stage_line(feature, stage, state).map(|text| {
            let failed = text.contains("failed") || text.contains("not ready");
            let tone = if failed {
                CheckTone::Blocker
            } else if waits(feature, stage, state) {
                CheckTone::Ready
            } else {
                CheckTone::Note
            };
            line(text, tone)
        }),
    };
    // A stage not reached yet may start by itself: its agent is picked ahead.
    if mark == StageMark::Later {
        sections.extend(agent_card(feature, stage, state, actions, now));
        // What it will run is there to see ahead too.
        if !check_entries(feature, stage, &state.plan).is_empty() {
            sections.extend(verify_sections(feature, state, actions, now));
        }
        // Fixes sent back the feature: the review they came from stays.
        if stage == Stage::AiReview && feature.review.is_some() {
            sections.extend(review_sections(feature, stage, mark, state, actions));
        }
    }
    if mark != StageMark::Later {
        sections.extend(fix_round_on(feature, stage).map(|r| fix_round_card(feature, r, actions)));
        sections.extend(last_round_on(feature, stage).map(|r| last_round_card(r, actions)));
        sections.extend(rework_card(feature, stage, state, actions));
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
            Stage::PlanReview => {
                sections.extend(card());
                if let Some(section) = suggestions_section(feature) {
                    sections.push(section);
                }
                if state.ctx.wrote(stage) {
                    sections.push(file_card(feature, stage, state.ctx, actions));
                    sections.extend(doc_of(state.probe, stage));
                }
            }
            Stage::Discover | Stage::Plan => {
                sections.extend(card());
                sections.push(file_card(feature, stage, state.ctx, actions));
                sections.extend(doc_of(state.probe, stage));
            }
            Stage::Build => {
                let blocked = feature.runs.get(&stage).and_then(|r| r.blocked.as_ref());
                if let Some(blocked) = blocked {
                    let why = line(blocked.why.clone(), CheckTone::Blocker);
                    sections.push(lines("Blocked", vec![why]));
                }
                sections.extend(setup_section(feature, state));
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
                    done: stop.done as usize,
                    total: stop.total as usize,
                    items: Vec::new(),
                });
                sections.extend(milestones_of(state.probe, stage).or(counted));
            }
            Stage::Verify => {
                sections.extend(verify_sections(feature, state, actions, now));
                sections.extend(setup_section(feature, state));
            }
            Stage::AiReview => {
                sections.extend(card());
                sections.extend(review_sections(feature, stage, mark, state, actions));
            }
            Stage::CodeReview if mark == StageMark::Current => {
                let text = (state.code_review.clone())
                    .unwrap_or_else(|| "review every changed file".into());
                sections.push(lines("Files", vec![line(text, CheckTone::Note)]));
            }
            Stage::ManualTest if mark == StageMark::Current => {
                if state.criteria.is_empty() {
                    let text = "no criteria in the brief: Mark done by hand";
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
                    let said = said.map(|text| line(text, CheckTone::Note));
                    sections.push(lines("Criteria", said.into_iter().collect()));
                    sections.extend(criterion_sections(state, actions, true));
                }
                sections.extend(sandbox_sections(feature, state));
            }
            // What was found, kept to see: reopening is what changes it.
            Stage::ManualTest if !state.criteria.is_empty() => {
                let by_hand = state.criteria.iter().all(|row| row.found.is_none());
                let said = by_hand.then(|| line("marked done by hand, not tried", CheckTone::Note));
                sections.push(lines("Criteria", said.into_iter().collect()));
                sections.extend(criterion_sections(state, actions, false));
            }
            Stage::Ship => {
                let check = check_lines(feature, state.probe, state.shipping, now);
                if !check.is_empty() {
                    sections.push(lines("Ship check", check));
                }
                let prs: Vec<CheckLine> = (feature.shipping.prs.iter())
                    .map(|pr| line(pr_tail(&pr.url), CheckTone::Note))
                    .collect();
                if !prs.is_empty() {
                    sections.push(lines("Pull requests", prs));
                }
            }
            Stage::CodeReview | Stage::ManualTest => {}
        }
    }
    // The agent's card says what it does: the status line needn't again.
    let carded = |l: &CheckLine| {
        (sections.iter()).any(|s| {
            matches!(s, PageSection::Agent { status, .. }
                if !status.is_empty() && l.text.ends_with(status.as_str()))
        })
    };
    let status = status.filter(|l| !carded(l));
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
        _ => b.clone(),
    };
    StagePage {
        title: stage.label().to_owned(),
        about: stage.about().to_owned(),
        status,
        buttons: actions.iter().map(label).collect(),
        sections,
    }
}

/// AI review's page of `feature` below its agent's card: the latest review's
/// findings, then each earlier review, folded unless `state` shows it.
fn review_sections(
    feature: &Feature,
    stage: Stage,
    mark: StageMark,
    state: &PanelState,
    actions: &[(PageButton, PanelOutcome)],
) -> Vec<PageSection> {
    let mut sections = Vec::new();
    let multi = feature.many_repos();
    let cards = finding_cards(feature, stage);
    if cards.is_empty() {
        sections.push(lines("Findings", findings_lines(feature)));
    } else {
        let label = format!("Findings \u{b7} {}", findings_count(feature));
        let comments = state.probe.map_or(0, |p| p.docs.comments);
        let mut notes = Vec::new();
        if comments > 0 && mark == StageMark::Current {
            let goes = if comments == 1 { "goes" } else { "go" };
            let text = format!(
                "{} from the diff {goes} with the fixes",
                plural(comments, "comment")
            );
            notes.push(line(text, CheckTone::Note));
        }
        let ready = (feature.runs.get(&stage)).is_some_and(|r| r.ready.is_some());
        match mark {
            StageMark::Later => notes.push(line(
                "From the last review. The code has changed since.",
                CheckTone::Note,
            )),
            StageMark::Current if !ready => notes.push(line(
                "From the last review. A new one hasn't finished.",
                CheckTone::Note,
            )),
            _ => {}
        }
        sections.push(lines(&label, notes));
        sections.extend(
            cards
                .into_iter()
                .map(|f| PageSection::Finding(finding_card(f, multi, actions))),
        );
    }
    for round in earlier_reviews(feature, stage) {
        sections.push(PageSection::File {
            name: format!("Review {}", round.n),
            state: line(round_count(round), CheckTone::Note),
            buttons: (actions.iter().enumerate())
                .filter(|(_, (_, o))| *o == PanelOutcome::FoldRound(round.n))
                .map(|(i, _)| i)
                .collect(),
        });
        if state.rounds_open.contains(&round.n) {
            let mut cards: Vec<&ReviewFinding> = round.findings.iter().collect();
            cards.sort_by_key(|f| finding_rank(f.state));
            sections.extend(
                cards
                    .into_iter()
                    .map(|f| PageSection::Finding(round_card(f, multi))),
            );
        }
    }
    sections
}

/// How a line of `feature`'s history reads in the activity: a send-back
/// or a failure as a blocker, a tick or a pass as ready, the rest a note.
fn event_tone(what: &str) -> CheckTone {
    let lower = what.to_lowercase();
    // A stop between milestones is a pause, not a failure.
    if lower.contains("stopped after milestone") {
        return CheckTone::Note;
    }
    let blocker = [
        "couldn't",
        "refused",
        "sent back",
        "failed",
        "code changed",
        "not done",
        "not read",
        "stopped",
        "to do again",
        "abandoned",
    ];
    let ready = [
        "ticked",
        "approved",
        "passed",
        "build done",
        "opened",
        "updated",
    ];
    if blocker.iter().any(|b| lower.contains(b)) {
        CheckTone::Blocker
    } else if ready.iter().any(|r| lower.contains(r)) || what == "Set up" {
        CheckTone::Ready
    } else {
        CheckTone::Note
    }
}

/// `feature`'s history, newest first.
fn activity(feature: &Feature, now: u64) -> Vec<ActivityRow> {
    (feature.history.iter().rev())
        .map(|e| ActivityRow {
            what: e.what.clone(),
            ago: projects::ago(now, e.at),
            tone: event_tone(&e.what),
        })
        .collect()
}

/// Every key the feature tab takes, by where it works, as F1 lists them:
/// (heading, [(keys, what they do)]).
const TAB_KEYS: [(&str, &[(&str, &str)]); 5] = [
    (
        "Stages",
        &[
            ("Left / Right", "previous or next stage"),
            ("Up / Down", "previous or next stage"),
            ("Ctrl+K / Ctrl+J", "previous or next stage"),
            ("Home / End", "first or last stage"),
            (
                "Enter",
                "the page's main button; on a done stage, go to the current one",
            ),
            (
                "Ctrl+Shift+Enter",
                "reopen a done stage, unless it is locked",
            ),
            (
                "Ctrl+Shift+Del",
                "start the current stage over, or drop its fix plan, after asking",
            ),
            (
                "Ctrl+Enter",
                "make the fixes of a fix plan ready for review, or send them again",
            ),
            (
                "Ctrl+Shift+O",
                "Verify's evidence, or a fix plan already sent",
            ),
            ("Tab", "Verify: to the checks, then Up / Down through them"),
            ("Enter", "a check: show its terminal"),
            ("r / s", "a check: run it again, stop it"),
            ("Space", "a check: skip it, or stop skipping it"),
            ("Shift+R / Shift+S", "Verify: run the failed ones, stop all"),
            (
                "Ctrl+Shift+U",
                "copy what the worktrees lack: Build and Verify",
            ),
            (
                "Ctrl+Shift+I",
                "what the setup runs and printed: Build and Verify",
            ),
            (
                "Ctrl+E",
                "edit the project's commands: Build, Verify, Manual test",
            ),
            ("Ctrl+Shift+T", "Manual test: start the sandbox, or stop it"),
            ("Ctrl+Shift+Y", "Manual test: open the sandbox's app"),
            ("Ctrl+Shift+G", "Manual test: restart the sandbox"),
            (
                "Ctrl+Shift+X",
                "Manual test: reset the sandbox's data, or remove it once stopped",
            ),
            ("Ctrl+R", "start the stage's agent afresh"),
            ("Ctrl+Shift+R", "show the stage's agent, or resume its chat"),
            ("Ctrl+Shift+A", "pick the stage's next agent"),
            ("Ctrl+Shift+I", "pick the model of the stage's agent"),
            (
                "Ctrl+Shift+B",
                "Build: one agent, or a fresh agent per milestone",
            ),
            ("Ctrl+Shift+K", "skip Plan review"),
            ("Ctrl+Shift+D", "change the decisions on a final plan"),
        ],
    ),
    (
        "Manual test's criteria",
        &[
            (
                "Tab",
                "to the criteria; Enter on the page goes to the first not tried",
            ),
            ("Up / Down", "previous or next criterion"),
            ("Home / End", "first or last criterion"),
            ("Space / Enter", "mark it passed, or not"),
            ("f", "mark it failed, saying what went wrong"),
            ("Del", "clear its result"),
            ("Ctrl+Enter", "mark Manual test passed"),
            ("Ctrl+Shift+Enter", "plan the fixes of the failures"),
        ],
    ),
    (
        "AI review's findings",
        &[
            ("Up / Down", "previous or next finding"),
            ("Home / End", "first or last finding"),
            ("f", "fix the finding, or take that back"),
            ("Ctrl+E", "edit its fix, or comment on it"),
            ("Del", "dismiss it, saying why, or reopen it"),
            ("Enter", "show it in the diff"),
            ("c", "add a comment of your own, to go with the fixes"),
            ("Alt+E", "show or hide the earlier reviews"),
        ],
    ),
    (
        "Repositories",
        &[
            ("Up / Down", "previous or next repository"),
            (
                "Left / Right",
                "previous or next repository; with all shown, the row's button",
            ),
            ("Ctrl+K / Ctrl+J", "previous or next repository"),
            ("Home / End", "first or last repository"),
            (
                "Enter",
                "show all repositories; with all shown, press the row's button",
            ),
            ("Esc", "hide them again"),
            ("Ctrl+Shift+O", "open its pull request"),
        ],
    ),
    (
        "Anywhere on the tab",
        &[
            (
                "Tab / Shift+Tab",
                "stages, findings, checks, criteria or repositories",
            ),
            ("PgUp / PgDn", "scroll the page"),
            ("Ctrl+PgUp / Ctrl+PgDn", "previous or next feature"),
            ("Ctrl+O", "open a tab in the feature's folder"),
            ("Ctrl+C", "copy the feature's path"),
            ("Ctrl+N", "new feature"),
            ("Ctrl+Shift+L", "show or hide all repositories"),
            (
                "Del",
                "on a repository of several, remove it (one on the branch asks, others press \
                 twice); else abandon the feature, or remove its worktrees once done",
            ),
            ("F1", "list these keys; any key puts them away"),
        ],
    ),
];

/// The page listing every key of the tab (F1), in place of a stage's.
fn keys_page() -> StagePage {
    let sections = TAB_KEYS
        .iter()
        .map(|(label, keys)| {
            let rows = (keys.iter())
                .map(|(k, what)| line(format!("{k}: {what}"), CheckTone::Note))
                .collect();
            lines(label, rows)
        })
        .collect();
    StagePage {
        title: "Keys".to_owned(),
        about: "Every key of the feature tab. Press any key to go back.".to_owned(),
        status: None,
        buttons: Vec::new(),
        sections,
    }
}

/// The feature tab's key hints over `feature`, one of `slugs`, as `panel`
/// has the keyboard on `stage`'s page.
/// What can be picked of a stage's next agent, for the feature tab's footer.
#[derive(Clone, Copy, Default)]
struct AgentChoices<'a> {
    picks: &'a [AgentChip],
    /// Its model too.
    model: bool,
}

fn tab_hints(
    panel: &FeaturePanel,
    feature: &Feature,
    stage: Stage,
    slugs: &[String],
    ctx: RowCtx,
    choices: AgentChoices,
    plan: &[PlannedCheck],
) -> Vec<(String, String)> {
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    if panel.keys {
        return vec![hint("Esc", "back to the page"), hint("F1", "hide the keys")];
    }
    if let Some((_, field)) = &panel.note {
        let esc = match field.trim().is_empty() {
            true => "cancel",
            false => "save",
        };
        return vec![hint("Enter", "save"), hint("Esc", esc)];
    }
    let mut hints = Vec::new();
    let entries = repo_entries(feature, &panel.sources, panel.expanded);
    let repos = !entries.is_empty();
    let entry = (entries
        .get(panel.repo.min(entries.len().saturating_sub(1)))
        .copied())
    .filter(|_| panel.focus == TabArea::Repos);
    match panel.focus {
        TabArea::Repos if repos => {
            let row = match entry {
                Some(RepoEntry::Held(i)) => PanelRow::Repo(i),
                _ => PanelRow::Other,
            };
            let buttons = entry.map_or(Vec::new(), |e| entry_buttons(feature, &panel.sources, e));
            if panel.expanded {
                hints.push(hint("Up / Down", "repository"));
                if !buttons.is_empty() {
                    hints.push(hint("Left / Right", "button"));
                }
                let picked = entry.and_then(|e| panel.picked(feature, e));
                let enter = match picked.and_then(|at| buttons.get(at)) {
                    Some((label, _)) => format!("{} button", label.to_lowercase()),
                    None => row_hint(feature, row, ctx).to_owned(),
                };
                if !enter.is_empty() {
                    hints.push(hint("Enter", &enter));
                }
                hints.push(hint("Esc", "hide them"));
            } else {
                hints.push(hint("Left / Right", "repository"));
                hints.push(hint("Enter", "show all"));
            }
            hints.push(hint("Tab", "stages"));
            if row_page(feature, row).is_some() {
                hints.push(hint("Ctrl+Shift+O", "open PR"));
            }
            let removable = match entry {
                Some(RepoEntry::Held(i)) => feature.many_repos() && repo_removable(feature, i),
                Some(RepoEntry::Context(_)) => linkable(feature),
                _ => false,
            };
            if removable {
                hints.push(if panel.armed.is_some() {
                    hint("Del", "again to remove")
                } else {
                    hint("Del", "remove repository")
                });
            }
        }
        TabArea::Findings if tries_criteria(feature, stage) && ctx.criteria > 0 => {
            hints.push(hint("Up / Down", "criterion"));
            hints.push(hint("Space", "pass"));
            hints.push(hint("f", "fail\u{2026}"));
            hints.push(hint("Del", "clear"));
            if fix_round_on(feature, stage).is_none() {
                hints.push(hint("Ctrl+Enter", "mark passed"));
                hints.push(hint("Ctrl+Shift+Enter", "plan fixes"));
            }
            hints.push(hint("Tab", if repos { "repositories" } else { "stages" }));
        }
        TabArea::Findings if shows_criteria(feature, stage) && ctx.criteria > 0 => {
            hints.push(hint("Up / Down", "criterion"));
            let row = PanelRow::Stage(stage);
            if matches!(row_action(feature, row, ctx), Some(PanelOutcome::Untick(_))) {
                hints.push(hint(REOPEN_KEY, "reopen"));
            }
            hints.push(hint("Tab", if repos { "repositories" } else { "stages" }));
        }
        TabArea::Findings if !check_entries(feature, stage, plan).is_empty() => {
            let entries = check_entries(feature, stage, plan);
            let entry = &entries[panel.finding.min(entries.len() - 1)];
            let row = entry.row.and_then(|i| check_rows(feature, stage).get(i));
            hints.push(hint("Up / Down", "check"));
            if row.is_some_and(|row| row.pane.is_some()) {
                hints.push(hint("Enter", "show terminal"));
            }
            let running = checks_running(feature);
            if running && row.is_some_and(|row| row.outcome.is_none()) {
                hints.push(hint("s", "stop"));
            }
            if !running && feature.current() == Some(stage) && row.is_some() && !entry.skipped {
                hints.push(hint("r", "run again"));
            }
            if let Some((label, _)) = skip_action(feature, stage, entry) {
                hints.push(hint("Space", &label.to_lowercase()));
            }
            if has_failed_checks(feature, stage) {
                hints.push(hint("Shift+R", "run failed"));
            }
            if running {
                hints.push(hint("Shift+S", "stop all"));
            }
            hints.push(hint("Tab", if repos { "repositories" } else { "stages" }));
        }
        TabArea::Findings => {
            let card = (finding_cards(feature, stage).get(panel.finding)).copied();
            hints.push(hint("Up / Down", "finding"));
            let decides = can_decide(feature, stage);
            if let Some(f) = card.filter(|_| decides) {
                match f.state {
                    FindingState::Open | FindingState::Dismissed => hints.push(hint("f", "fix")),
                    FindingState::Fix => hints.push(hint("f", "don't fix")),
                    _ => {}
                }
                if matches!(f.state, FindingState::Open | FindingState::Fix) {
                    hints.push(hint("Ctrl+E", "edit\u{2026}"));
                    hints.push(match f.yours {
                        true => hint("Del", "drop"),
                        false => hint("Del", "dismiss\u{2026}"),
                    });
                } else if f.state == FindingState::Dismissed {
                    hints.push(hint("Del", "reopen"));
                }
            }
            if let Some(f) = card {
                if !f.file.is_empty() {
                    hints.push(hint("Enter", "show in diff"));
                } else if decides {
                    hints.push(hint("Enter", "edit\u{2026}"));
                }
            }
            if can_decide(feature, stage) {
                hints.push(hint("c", "add comment\u{2026}"));
            }
            if offers_fix_all(feature, stage) {
                hints.push(hint("Shift+F", "fix all open"));
            }
            if earlier_reviews(feature, stage).next().is_some() {
                hints.push(hint("Alt+E", "earlier reviews"));
            }
            hints.push(hint("Tab", if repos { "repositories" } else { "stages" }));
        }
        _ => {
            let row = PanelRow::Stage(stage);
            hints.push(hint("Left / Right", "stage"));
            let enter = match page_enter(feature, stage, ctx) {
                Some(PanelOutcome::NotYet(_)) | None => "",
                Some(action) => action_hint(&action),
            };
            if !enter.is_empty() {
                hints.push(hint("Enter", enter));
            }
            if offers_fix_all(feature, stage) {
                hints.push(hint("Shift+F", "fix all open"));
            }
            if earlier_reviews(feature, stage).next().is_some() {
                hints.push(hint("Alt+E", "earlier reviews"));
            }
            if matches!(row_action(feature, row, ctx), Some(PanelOutcome::Untick(_))) {
                hints.push(hint(REOPEN_KEY, "reopen"));
            }
            if let Some(round) = fix_round_on(feature, stage) {
                if round.phase == FixPhase::Ready {
                    hints.push(hint("Ctrl+Enter", "make fixes"));
                }
                if round.before_fixing() {
                    hints.push(hint(START_OVER_KEY, "drop plan\u{2026}"));
                }
            }
            // Manual test's has its own use for it.
            if makes_fixes_again(feature, stage) && !tries_criteria(feature, stage) {
                hints.push(hint("Ctrl+Enter", "make fixes again"));
            }
            if can_decide(feature, stage) {
                hints.push(hint("c", "add comment\u{2026}"));
            }
            if has_failed_checks(feature, stage) && feature.current() == Some(stage) {
                hints.push(hint("Shift+R", "run failed"));
            }
            if stage == Stage::Verify && checks_running(feature) {
                hints.push(hint("Shift+S", "stop all"));
            }
            if !finding_cards(feature, stage).is_empty() && !panel.expanded {
                hints.push(hint("Tab", "findings"));
            } else if !check_entries(feature, stage, plan).is_empty() && !panel.expanded {
                hints.push(hint("Tab", "checks"));
            } else if shows_criteria(feature, stage) && ctx.criteria > 0 && !panel.expanded {
                hints.push(hint("Tab", "criteria"));
            } else if repos {
                hints.push(hint("Tab", "repositories"));
            }
            if row_page(feature, row).is_some() {
                hints.push(hint("Ctrl+Shift+O", "evidence"));
            } else if opens_fix_plan(feature, stage) {
                hints.push(hint("Ctrl+Shift+O", "fix plan"));
            }
            if offers_setup(feature, stage) {
                hints.push(hint("Ctrl+Shift+U", "set up"));
                if offers_setup_log(feature, stage) {
                    hints.push(hint("Ctrl+Shift+I", "setup log"));
                }
                hints.push(hint("Ctrl+E", "edit commands"));
            }
            if offers_sandbox(feature, stage) {
                if ctx.sandbox_busy {
                    hints.push(hint("Ctrl+Shift+T", "stop"));
                } else if sandbox_alive(feature) {
                    hints.push(hint("Ctrl+Shift+Y", "open app"));
                    hints.push(hint("Ctrl+Shift+G", "restart"));
                    hints.push(hint("Ctrl+Shift+X", "reset data\u{2026}"));
                    hints.push(hint("Ctrl+Shift+T", "stop\u{2026}"));
                } else {
                    hints.push(hint("Ctrl+Shift+T", "start sandbox\u{2026}"));
                    if feature.sandbox.is_some() {
                        hints.push(hint("Ctrl+Shift+X", "remove sandbox\u{2026}"));
                    }
                }
                hints.push(hint("Ctrl+E", "edit commands"));
            }
            if fresh_stage(feature, row).is_some() {
                hints.push(hint("Ctrl+R", "fresh agent"));
            }
            if start_over_stage(feature, stage, ctx).is_some() {
                hints.push(hint(START_OVER_KEY, "start over\u{2026}"));
            }
            if let Some((label, outcome)) = agent_key(feature, stage, ctx)
                && page_enter(feature, stage, ctx).as_ref() != Some(&outcome)
            {
                hints.push(hint("Ctrl+Shift+R", &label.to_lowercase()));
            }
            if choices.picks.iter().filter(|c| !c.off).count() > 1 {
                hints.push(hint("Ctrl+Shift+A", "pick agent"));
            }
            if choices.model {
                hints.push(hint("Ctrl+Shift+I", "pick model"));
            }
            if milestone_switch(feature, stage) {
                let what = match feature.fresh_milestones {
                    true => "one agent",
                    false => "agent per milestone",
                };
                hints.push(hint("Ctrl+Shift+B", what));
            }
        }
    }
    if slugs.len() > 1 {
        hints.push(hint("Ctrl+PgUp / Ctrl+PgDn", "other feature"));
    }
    hints.extend([
        hint("Ctrl+O", "open tab"),
        hint("Ctrl+C", "copy path"),
        hint("Ctrl+N", "new"),
    ]);
    hints.push(hint(
        "Ctrl+Shift+L",
        match panel.expanded {
            true => "hide repositories",
            false => "all repositories",
        },
    ));
    let del_abandons = match entry {
        _ if panel.focus == TabArea::Findings => false,
        Some(RepoEntry::Held(_)) => !feature.many_repos(),
        Some(_) => false,
        None => true,
    };
    if del_abandons {
        hints.push(hint("Del", &remove_label(feature).to_lowercase()));
    }
    hints.push(hint("F1", "all keys"));
    hints
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
                row.line = tilde(&repo.main);
                row.kind = RepoKind::Context;
                row.remove = linkable(feature);
            }
        }
        RepoEntry::Removed(i) => {
            if let Some(gone) = feature.removed.get(i) {
                row.name = state.names.label_of(gone.project.as_deref(), &gone.name);
                row.line = format!("removed {}", projects::ago(now, gone.at));
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
fn tab_view(
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
        .map(|s| RailNode {
            selected: *s == stage,
            ..rail_node(feature, *s, state, now)
        })
        .collect();
    let mut page = match panel.keys {
        true => keys_page(),
        false => stage_page(
            feature,
            stage,
            state,
            &stage_actions(feature, stage, state.ctx, &state.plan, &state.criteria),
            now,
        ),
    };
    if panel.focus == TabArea::Findings && !panel.expanded {
        let cards = (page.sections.iter_mut()).filter_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        });
        if let Some(card) = cards.into_iter().nth(panel.finding) {
            card.selected = true;
        }
    }
    // The note being written, on its card in place of the one saved.
    if let Some((i, field)) = panel
        .note
        .as_ref()
        .filter(|_| !panel.keys && !panel.expanded)
    {
        let card = (page.sections.iter_mut())
            .filter_map(|s| match s {
                PageSection::Finding(card) => Some(card),
                _ => None,
            })
            .nth(*i);
        if let Some(card) = card.filter(|_| tries_criteria(feature, stage)) {
            card.field = Some(field.line());
            card.note = String::new();
        }
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
        tilde(&feature.dir),
        projects::ago(now, feature.created)
    );
    if feature.status == Status::Done {
        sub = format!("done \u{b7} {sub}");
    }
    let carded = !panel.expanded
        && !(finding_cards(feature, stage).is_empty() && check_rows(feature, stage).is_empty());
    let focus = match panel.focus {
        TabArea::Repos if repos.is_empty() => TabArea::Stages,
        TabArea::Findings if !carded => TabArea::Stages,
        focus => focus,
    };
    let at = panel.repo.min(repos.len().saturating_sub(1));
    let picked = (entries.get(at))
        .filter(|_| panel.expanded && focus == TabArea::Repos)
        .and_then(|e| panel.picked(feature, *e));
    FeatureTabView {
        title: feature.title.clone(),
        sub,
        which,
        remove: remove_label(feature),
        rail,
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
        scroll: std::array::from_fn(|col| {
            let s = match col == TabColumn::Page as usize {
                true => panel.page_scroll(feature),
                false => Some(&panel.scroll[col]),
            };
            s.map_or(0.0, |s| s.pos() - s.overscroll())
        }),
        hover: panel.tab_hover,
        hints: tab_hints(
            panel,
            feature,
            stage,
            slugs,
            state.ctx,
            AgentChoices {
                picks: &state.picks,
                model: state.model.is_some(),
            },
            &state.plan,
        ),
    }
}

/// How far `col` of the tab laid out as `layout` scrolls, for its
/// `Scroll`: in pixels, a notch as the sidebar's.
fn tab_bounds(layout: &chrome::FeatureTabLayout, col: TabColumn) -> Bounds {
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::feature::{Mark, ship::ShipCheck};

    pub(crate) fn repo(name: &str, touched: bool) -> Repo {
        Repo {
            name: name.into(),
            main: PathBuf::from(format!("/work/{name}")),
            base: "main".into(),
            base_commit: "abc123".into(),
            worktree: PathBuf::from(format!("/work/.features/bulk/{name}")),
            touched,
            branch: touched.then(|| "feature/bulk".into()),
            project: None,
        }
    }

    /// A feature over `repos`, its first `done` stages marked.
    pub(crate) fn feature(slug: &str, repos: Vec<Repo>, done: usize) -> Feature {
        Feature {
            version: feature::VERSION,
            slug: slug.into(),
            title: format!("Feature {slug}"),
            project: "acme".into(),
            created: 1_000,
            root: "/work".into(),
            dir: format!("/work/.features/{slug}").into(),
            multi: repos.len() > 1,
            repos,
            links: Vec::new(),
            status: Status::Open,
            run_counts: BTreeMap::new(),
            context: Vec::new(),
            removed: Vec::new(),
            marks: Stage::ALL
                .iter()
                .take(done)
                .map(|s| {
                    let mark = Mark {
                        at: 2_000,
                        by: "you".into(),
                        note: String::new(),
                        patch: None,
                    };
                    (*s, mark)
                })
                .collect(),
            history: Vec::new(),
            shipping: Default::default(),
            runs: Default::default(),
            stage_agents: Default::default(),
            stage_models: Default::default(),
            fresh_milestones: false,
            setup: None,
            checks: Default::default(),
            skip_checks: Default::default(),
            verify: None,
            sandbox: None,
            sandbox_picks: Default::default(),
            review: None,
            plan_review: None,
            viewed: Default::default(),
            manual: None,
            fix_round: None,
            last_fix_round: None,
            fix_rounds: 0,
        }
    }

    fn key(sym: Keysym) -> (ModifiersState, Keysym) {
        (ModifiersState::default(), sym)
    }

    fn ctrl(sym: Keysym) -> (ModifiersState, Keysym) {
        let mods = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        (mods, sym)
    }

    fn press(
        panel: &mut FeaturePanel,
        f: &Feature,
        (mods, sym): (ModifiersState, Keysym),
    ) -> Option<PanelOutcome> {
        let slugs = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        panel.key(&mods, sym, f, &slugs, RowCtx::default(), &[])
    }

    fn root(names: &[&str]) -> Root {
        Root {
            path: "/work".into(),
            multi: names.len() > 1,
            repos: names
                .iter()
                .map(|n| Found {
                    name: (*n).into(),
                    main: PathBuf::from(format!("/work/{n}")),
                    base: "main".into(),
                })
                .collect(),
        }
    }

    fn type_into(d: &mut NewFeatureDialog, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            d.key(&ModifiersState::default(), sym, sym, 8);
        }
    }

    fn dkey(
        d: &mut NewFeatureDialog,
        (mods, sym): (ModifiersState, Keysym),
    ) -> Option<DialogOutcome> {
        d.key(&mods, sym, sym, 8)
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
            [("Ctrl+Shift+L", "link project"), ("Esc", "close")]
        );
    }

    /// Another project of the store, as `find_others` lists it.
    fn other(project: &str, name: &str) -> Other {
        Other {
            project: project.into(),
            name: name.into(),
            root: Finding::Looking,
            linked: false,
            touched: Vec::new(),
        }
    }

    fn ctrl_shift(sym: Keysym) -> (ModifiersState, Keysym) {
        let mods = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        (mods, sym)
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
        let found = format!("In {} \u{b7} 2 repositories", tilde(Path::new("/work")));
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
        // On the stages it is the feature's, as it was.
        panel.focus = TabArea::Stages;
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
        // So on a feature's only repository.
        let mut one = feature("c", vec![repo("api", true)], 3);
        one.multi = false;
        panel.focus = TabArea::Repos;
        panel.repo = 0;
        assert_eq!(
            press(&mut panel, &one, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
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
            press(&mut panel, &f, key(Keysym::Delete)),
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
            .filter_map(|l| l.split_once(": ").map(|(k, _)| k))
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
        // The only one a feature has: Del abandons it, and says so.
        let one = feature("b", vec![repo("api", true)], 3);
        assert_eq!(del(&one, 0).as_deref(), Some("abandon"));
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
    fn header_buttons_and_the_strip_wheel() {
        assert_eq!(header_label("Abandon", 0), Some("Open tab"));
        assert_eq!(header_label("Abandon", 2), Some("New feature\u{2026}"));
        assert_eq!(header_label("Abandon", 3), Some("Abandon"));
        assert_eq!(header_label("Abandon", 4), None);
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
                "Ctrl+O",
                "Ctrl+C",
                "Ctrl+N",
                "Ctrl+Shift+L",
                "Del",
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

    fn agent_run(pane: Option<&str>, ready: Option<u64>) -> feature::agents::StageRun {
        feature::agents::StageRun {
            n: 1,
            agent: "Claude Code".into(),
            pane: pane.map(str::to_owned),
            ready,
            ..feature::agents::StageRun::default()
        }
    }

    /// A run of `base` whose tab is closed.
    fn kept_run(base: &str) -> feature::agents::StageRun {
        feature::agents::StageRun {
            base: base.into(),
            ..agent_run(None, None)
        }
    }

    /// A run of `base` whose agent waits for its prompt, its tab `pane`.
    fn held_run(pane: Option<&str>) -> feature::agents::StageRun {
        feature::agents::StageRun {
            base: "claude".into(),
            held: true,
            ..agent_run(pane, None)
        }
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
            PageSection::Agent { status, .. } if status.starts_with("waiting for you")
        )));
        // Its tab closed: it can still open again from there.
        f.runs.insert(Stage::Build, held_run(None));
        assert!(
            page_outcomes(&f, Stage::Build)
                .contains(&PanelOutcome::StartAgent(Stage::Build, false))
        );
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
                "Build agent \u{b7} running again".to_owned(),
                Dot::Working,
                vec!["Show agent".to_owned()]
            ))
        );
        assert_eq!(node, (NodeMark::Fixing, "running again".to_owned()));
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
        assert!(status.starts_with("Build agent \u{b7} waiting for you"));
        assert_eq!(buttons, ["Send prompt", "Show agent"]);
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
        assert_eq!(shown.unwrap().2, ["Start agent"]);
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
            PageSection::Agent { status, dot: Dot::Working, .. } if status == "running again"
        )));
    }

    #[test]
    fn the_reviewer_waits_for_its_prompt_too() {
        let mut f = feature("b", vec![repo("api", true)], 5);
        assert_eq!(f.current(), Some(Stage::AiReview));
        f.runs.insert(Stage::AiReview, held_run(Some("p1")));
        assert_eq!(review_action(&f), PanelOutcome::SendPrompt(Stage::AiReview));
        f.runs.insert(Stage::AiReview, held_run(None));
        assert_eq!(
            review_action(&f),
            PanelOutcome::StartAgent(Stage::AiReview, false)
        );
    }

    fn page_outcomes(f: &Feature, stage: Stage) -> Vec<PanelOutcome> {
        let actions = page_actions(f, stage, RowCtx::default());
        actions.into_iter().map(|(_, o)| o).collect()
    }

    #[test]
    fn plan_review_goes_from_its_agent_to_the_decisions_to_the_final_plan() {
        use crate::feature::plan_review::{self, Decision, PlanReview};
        let mut f = feature("p", vec![repo("api", true)], 2);
        let stage = Stage::PlanReview;
        let ctx = RowCtx::default();
        assert_eq!(f.current(), Some(stage));
        let names = |f: &Feature| -> Vec<String> {
            let actions = page_actions(f, stage, RowCtx::default());
            actions.iter().map(|(b, _)| b.label.clone()).collect()
        };
        // Nothing runs until asked for, and it can be skipped at once.
        assert_eq!(names(&f), ["Start agent", "Skip review"]);
        assert_eq!(
            page_enter(&f, stage, ctx),
            Some(PanelOutcome::StartAgent(stage, false))
        );
        let state = PanelState::default();
        assert_eq!(stage_page(&f, stage, &state, &[], 2_000).sections.len(), 0);
        // Read in with one to decide: the sheet is the way on.
        let text = r#"{"suggestions": [{"kind": "question", "plan": "M1", "title": "Which?",
            "why": "w"}, {"kind": "question", "plan": "M1", "title": "How?", "why": "w"}]}"#;
        let mut review = PlanReview {
            stamp: Some((1, 1)),
            ..PlanReview::default()
        };
        plan_review::import(&mut review, 1, plan_review::parse(text).unwrap());
        f.plan_review = Some(review);
        assert_eq!(
            page_enter(&f, stage, ctx),
            Some(PanelOutcome::PlanSuggestions)
        );
        assert_eq!(
            names(&f),
            ["Decide\u{2026}", "Skip review", "Start over\u{2026}"]
        );
        assert_eq!(rail_node(&f, stage, &state, 2_000).line, "2 to decide");
        let page = stage_page(&f, stage, &state, &[], 2_000);
        assert_eq!(
            page.sections,
            [lines(
                "Suggestions",
                vec![line("2 to decide", CheckTone::Blocker)]
            )]
        );
        assert_eq!(
            crate::feature_agents::gate_of(&f, |_| None).map(|g| g.2),
            Some(crate::feature_agents::GateKind::Suggestions { open: 2 })
        );
        // All rejected: nothing for a final plan to add.
        let review = f.plan_review.as_mut().unwrap();
        plan_review::decide(review, "r1-1", Decision::Reject).unwrap();
        plan_review::decide(review, "r1-2", Decision::Reject).unwrap();
        assert_eq!(
            page_enter(&f, stage, ctx),
            Some(PanelOutcome::WriteFinalPlan)
        );
        assert_eq!(
            names(&f),
            [
                "Use the plan as it is",
                "Decide\u{2026}",
                "Skip review",
                "Start over\u{2026}"
            ]
        );
        assert_eq!(rail_node(&f, stage, &state, 2_000).line, "all decided");
        // One accepted: a final plan.
        let review = f.plan_review.as_mut().unwrap();
        plan_review::set_note(review, "r1-1", "This one");
        plan_review::decide(review, "r1-1", Decision::Accept).unwrap();
        assert_eq!(names(&f)[0], "Write final plan");
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
                pair("Ctrl+O", "open tab")
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
                pair("Ctrl+R", "fresh agent")
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
        assert_eq!(hints(&panel, &f, &state)[1], pair("Enter", "review plan"));
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

    /// A feature on Verify with a passed and a failed check, its run ended.
    fn verified(running: bool) -> Feature {
        use feature::verify::{CheckRun, Outcome, VerifyRun};
        let mut f = feature("v", vec![repo("api", true)], 4);
        let row = |command: &str, exit: i32| CheckRun {
            repo: "api".into(),
            command: command.into(),
            pane: Some(format!("p-{command}")),
            run: 2,
            outcome: Some(Outcome {
                exit: Some(exit),
                tail: "first\nboom\n".into(),
                ..Outcome::default()
            }),
            ..CheckRun::default()
        };
        let mut checks = vec![row("lint", 0), row("test", 1)];
        if running {
            checks[1].outcome = None;
        }
        f.verify = Some(VerifyRun {
            n: 2,
            opened: true,
            finished: (!running).then_some(1),
            checks,
            ..VerifyRun::default()
        });
        f
    }

    #[test]
    fn verifys_checks_are_cards_with_their_own_keys() {
        let shift = |sym| {
            let mods = ModifiersState {
                shift: true,
                ..ModifiersState::default()
            };
            (mods, sym)
        };
        let f = verified(false);
        let mut panel = FeaturePanel::default();
        // The page's own keys, from the stages already.
        assert_eq!(
            press(&mut panel, &f, shift(Keysym::R)),
            Some(PanelOutcome::RunFailed)
        );
        assert_eq!(press(&mut panel, &f, key(Keysym::r)), None);
        // Tab goes to the cards, Up and Down through them.
        assert_eq!(press(&mut panel, &f, key(Keysym::Tab)), None);
        assert_eq!(panel.focus, TabArea::Findings);
        assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::ShowCheck(1))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::r)),
            Some(PanelOutcome::RerunCheck(1))
        );
        // While they run: stop, not run again.
        let running = verified(true);
        assert_eq!(
            press(&mut panel, &running, key(Keysym::s)),
            Some(PanelOutcome::StopCheck(1))
        );
        assert_eq!(
            press(&mut panel, &running, shift(Keysym::S)),
            Some(PanelOutcome::StopChecks)
        );
        assert_eq!(press(&mut panel, &running, shift(Keysym::R)), None);
        // A key of the findings does nothing here.
        assert_eq!(press(&mut panel, &f, key(Keysym::f)), None);
    }

    /// `f`, with a fix round asked by `stage`, in `phase`.
    fn in_round(mut f: Feature, stage: Stage, phase: FixPhase) -> Feature {
        f.fix_round = Some(FixRound {
            n: 1,
            what: "Verify failures".into(),
            input: "/d/a.md".into(),
            stage,
            phase,
            sent_back: 0,
            asked: 1_000,
            ready: (phase == FixPhase::Ready).then_some(1_100),
            stalled: None,
            give_back: Default::default(),
        });
        f
    }

    #[test]
    fn a_failed_verify_waits_for_plan_fixes_on_enter() {
        let mut f = verified(false);
        // `verified` ran to its end, a check failing.
        f.verify.as_mut().unwrap().finished = Some(1_100);
        f.verify.as_mut().unwrap().passed = Some(false);
        let ctx = RowCtx {
            checks: true,
            ..RowCtx::default()
        };
        assert_eq!(
            page_enter(&f, Stage::Verify, ctx),
            Some(PanelOutcome::PlanFixes)
        );
        let plain = page_actions(&f, Stage::Verify, ctx);
        let run = plain.iter().find(|(_, o)| *o == PanelOutcome::RunChecks);
        assert_eq!(run.map(|(b, _)| b.style), Some(ButtonStyle::Plain));
        // A check cut short says nothing of the code: run them again first.
        let cut = (f.verify.as_mut().unwrap().checks.iter_mut())
            .find_map(|c| c.outcome.as_mut())
            .unwrap();
        cut.interrupted = true;
        assert_eq!(
            page_enter(&f, Stage::Verify, ctx),
            Some(PanelOutcome::RunChecks)
        );
    }

    #[test]
    fn a_fix_round_shows_on_the_page_that_asked_and_takes_enter() {
        let mut f = verified(false);
        f.verify.as_mut().unwrap().finished = Some(1_100);
        f.verify.as_mut().unwrap().passed = Some(false);
        let labels_of = |f: &Feature| -> Vec<String> {
            (page_actions(f, Stage::Verify, RowCtx::default()).into_iter())
                .map(|(b, _)| b.label)
                .collect()
        };
        let ready = in_round(f.clone(), Stage::Verify, FixPhase::Ready);
        assert_eq!(
            page_enter(&ready, Stage::Verify, RowCtx::default()),
            Some(PanelOutcome::ReviewFixPlan)
        );
        let shown = labels_of(&ready);
        for label in ["Review", "Make fixes", "Drop plan\u{2026}"] {
            assert!(shown.contains(&label.to_owned()), "{label}: {shown:?}");
        }
        // Finish or drop it before starting the stage over.
        assert!(
            !shown.contains(&"Start over\u{2026}".to_owned()),
            "{shown:?}"
        );
        assert_eq!(
            start_over_stage(&ready, Stage::Verify, RowCtx::default()),
            None
        );
        assert!(!shown.contains(&"Plan fixes".to_owned()), "{shown:?}");
        let actions = page_actions(&ready, Stage::Verify, RowCtx::default());
        let state = PanelState::default();
        let page = stage_page(&ready, Stage::Verify, &state, &actions, 2_000);
        let Some(PageSection::File {
            name,
            state,
            buttons,
        }) = page.sections.first()
        else {
            panic!("the fix plan's card first: {:?}", page.sections);
        };
        assert_eq!(name, "Fix plan");
        assert_eq!(state.text, "Verify failures \u{b7} fix plan ready");
        assert_eq!(state.tone, CheckTone::Ready);
        let held: Vec<&str> = buttons
            .iter()
            .map(|i| actions[*i].0.label.as_str())
            .collect();
        assert_eq!(held, ["Review", "Make fixes", "Drop plan\u{2026}"]);

        let planning = in_round(f.clone(), Stage::Verify, FixPhase::Planning);
        let shown = labels_of(&planning);
        assert!(!shown.contains(&"Make fixes".to_owned()), "{shown:?}");
        assert!(shown.contains(&"Drop plan\u{2026}".to_owned()), "{shown:?}");
        // A fresh agent held: its prompt goes first, and only one is primary.
        let mut held = in_round(f.clone(), Stage::Verify, FixPhase::Planning);
        held.runs.insert(
            Stage::Build,
            feature::agents::StageRun {
                pane: Some("p1".into()),
                held: true,
                ..Default::default()
            },
        );
        let actions = page_actions(&held, Stage::Verify, RowCtx::default());
        let primary: Vec<&str> = (actions.iter())
            .filter(|(b, _)| b.style == ButtonStyle::Primary)
            .map(|(b, _)| b.label.as_str())
            .collect();
        assert_eq!(primary, ["Send prompt"]);
        assert_eq!(
            page_enter(&held, Stage::Verify, RowCtx::default()),
            Some(PanelOutcome::SendPrompt(Stage::Build))
        );
        let sends = (actions.iter())
            .filter(|(_, o)| *o == PanelOutcome::SendPrompt(Stage::Build))
            .count();
        assert_eq!(sends, 1);
        // Stopped without a plan: ask again, or drop it.
        let mut stalled = planning.clone();
        stalled.fix_round.as_mut().unwrap().stalled = Some("it stopped".into());
        assert_eq!(
            page_enter(&stalled, Stage::Verify, RowCtx::default()),
            Some(PanelOutcome::AskFixPlan)
        );
        // Making the fixes: nothing left to drop.
        let fixing = in_round(f.clone(), Stage::Verify, FixPhase::Fixing);
        let shown = labels_of(&fixing);
        assert!(
            !shown.contains(&"Drop plan\u{2026}".to_owned()),
            "{shown:?}"
        );
        assert!(!shown.contains(&"Make fixes".to_owned()), "{shown:?}");
        // No other stage's page has it.
        let other = page_actions(&ready, Stage::Build, RowCtx::default());
        assert!(!other.iter().any(|(_, o)| *o == PanelOutcome::MakeFixes));
    }

    #[test]
    fn a_fix_plan_stays_on_the_page_once_its_fixes_went() {
        let mut f = verified(false);
        f.verify.as_mut().unwrap().finished = Some(1_100);
        f.verify.as_mut().unwrap().passed = Some(false);
        let held = |f: &Feature, stage| {
            let actions = page_actions(f, stage, RowCtx::default());
            let page = stage_page(f, stage, &PanelState::default(), &actions, 2_000);
            let card = page.sections.into_iter().find_map(|s| match s {
                PageSection::File {
                    name,
                    state,
                    buttons,
                } if name == "Fix plan" => Some((state.text, buttons)),
                _ => None,
            });
            card.map(|(said, buttons)| {
                let labels: Vec<String> = buttons
                    .iter()
                    .map(|i| actions[*i].0.label.clone())
                    .collect();
                (said, labels)
            })
        };
        // About to make them: the plan can be read, not dropped any more.
        let compacting = in_round(f.clone(), Stage::Verify, FixPhase::CompactingToFix);
        assert_eq!(
            held(&compacting, Stage::Verify),
            Some((
                "Verify failures \u{b7} compacting the agent's chat".to_owned(),
                vec!["Open".to_owned()]
            ))
        );
        // Making them: the plan can be read.
        let fixing = in_round(f.clone(), Stage::Verify, FixPhase::Fixing);
        assert_eq!(
            held(&fixing, Stage::Verify),
            Some((
                "Verify failures \u{b7} making fixes".to_owned(),
                vec!["Open".to_owned()]
            ))
        );
        // Its agent rested: read, or sent again as a rest doesn't tell the
        // fixes were made.
        let before = page_enter(&f, Stage::Verify, RowCtx::default());
        let mut over = f.clone();
        over.last_fix_round = fixing.fix_round.clone();
        assert_eq!(
            held(&over, Stage::Verify),
            Some((
                "Verify failures \u{b7} fixes sent".to_owned(),
                vec!["Open".to_owned(), "Make fixes again".to_owned()]
            ))
        );
        // Enter still does what the stage waits for.
        assert_eq!(page_enter(&over, Stage::Verify, RowCtx::default()), before);
        // Each has its key.
        let slugs = vec![over.slug.clone()];
        let mut panel = FeaturePanel {
            slug: over.slug.clone(),
            ..FeaturePanel::default()
        };
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let none = RowCtx::default();
        assert_eq!(
            panel.key(&ctrl, Keysym::Return, &over, &slugs, none, &[]),
            Some(PanelOutcome::MakeFixes)
        );
        let hints = tab_view(&panel, &over, &slugs, &PanelState::default(), 2_000).hints;
        assert!(hints.contains(&("Ctrl+Enter".into(), "make fixes again".into())));
        // Past its stage it is only read.
        let at = over.current().unwrap();
        let mut past = over.clone();
        past.last_fix_round.as_mut().unwrap().stage = Stage::Build;
        assert!(at > Stage::Build);
        assert_eq!(
            held(&past, Stage::Build),
            Some((
                "Verify failures \u{b7} fixes sent".to_owned(),
                vec!["Open".to_owned()]
            ))
        );
        assert_eq!(held(&past, at), None);
        // Back on a stage before the one that asked, it comes along.
        let mut back = over.clone();
        back.last_fix_round.as_mut().unwrap().stage = Stage::Ship;
        assert!(makes_fixes_again(&back, at));
        // A round open has the card to itself.
        let mut both = fixing.clone();
        both.last_fix_round = fixing.fix_round.clone();
        assert!(last_round_on(&both, Stage::Verify).is_none());
    }

    #[test]
    fn verifys_page_holds_a_card_per_check() {
        let labels = |f: &Feature| -> Vec<String> {
            let actions = page_actions(f, Stage::Verify, RowCtx::default());
            actions.into_iter().map(|(b, _)| b.label).collect()
        };
        let f = verified(false);
        let shown = labels(&f);
        assert!(shown.contains(&"Run failed".to_owned()), "{shown:?}");
        assert!(!shown.contains(&"Stop all".to_owned()), "{shown:?}");
        let state = PanelState::default();
        let actions = page_actions(&f, Stage::Verify, RowCtx::default());
        let entries = check_entries(&f, Stage::Verify, &[]);
        let (passed, failed) = (
            check_card(&entries[0], &f, &state, &actions, 2_000),
            check_card(&entries[1], &f, &state, &actions, 2_000),
        );
        assert_eq!(passed.tag, FindingTag::Passed);
        assert_eq!(passed.preview, ["first", "boom"]);
        assert_eq!(failed.tag, FindingTag::Failed);
        assert_eq!(failed.place, "api \u{b7} exited with 1");
        assert_eq!(failed.preview, ["first", "boom"]);
        let held: Vec<&str> = (failed.buttons.iter())
            .map(|i| actions[*i].0.label.as_str())
            .collect();
        assert_eq!(held, ["Show", "Run again"]);

        let running = verified(true);
        let shown = labels(&running);
        assert!(shown.contains(&"Stop all".to_owned()), "{shown:?}");
        assert!(!shown.contains(&"Run failed".to_owned()), "{shown:?}");
        let actions = page_actions(&running, Stage::Verify, RowCtx::default());
        let entries = check_entries(&running, Stage::Verify, &[]);
        let card = check_card(&entries[1], &running, &state, &actions, 2_000);
        assert_eq!(card.tag, FindingTag::Running);
        let held: Vec<&str> = (card.buttons.iter())
            .map(|i| actions[*i].0.label.as_str())
            .collect();
        assert_eq!(held, ["Show", "Stop"]);
        // A later stage's page has none.
        assert!(check_rows(&f, Stage::Build).is_empty());
    }

    #[test]
    fn verify_runs_its_checks_when_it_has_some_else_is_marked() {
        let f = feature("v", vec![repo("api", true)], 4);
        let verify = PanelRow::Stage(Stage::Verify);
        let checks = RowCtx {
            checks: true,
            ..RowCtx::default()
        };
        assert_eq!(
            row_action(&f, verify, checks),
            Some(PanelOutcome::RunChecks)
        );
        assert_eq!(row_hint(&f, verify, checks), "run checks");
        assert_eq!(stage_button(&f, Stage::Verify, checks), Some("Run checks"));
        let mut panel = FeaturePanel {
            ..FeaturePanel::default()
        };
        let slugs = vec!["v".to_owned()];
        let (mods, sym) = key(Keysym::Return);
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, checks, &[]),
            Some(PanelOutcome::RunChecks)
        );
        let none = RowCtx::default();
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, none, &[]),
            Some(PanelOutcome::Tick)
        );
        // Later stages aren't run early.
        let early = feature("e", vec![repo("api", true)], 3);
        assert_eq!(
            row_action(&early, verify, checks),
            Some(PanelOutcome::NotYet(Stage::Build))
        );
    }

    #[test]
    fn evidence_opens_from_the_verify_row_once_a_run_wrote_it() {
        let mut f = feature("v", vec![repo("api", true)], 4);
        let mut panel = FeaturePanel {
            ..FeaturePanel::default()
        };
        let slugs = vec!["v".to_owned()];
        let shift_ctrl = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        let none = RowCtx::default();
        let hinted = |panel: &FeaturePanel, f: &Feature| {
            tab_view(panel, f, &slugs, &PanelState::default(), 2_000)
                .hints
                .contains(&("Ctrl+Shift+O".into(), "evidence".into()))
        };
        // No run yet: nothing to open, no hint.
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[]),
            None
        );
        assert!(!hinted(&panel, &f));
        // The first run, still running: nothing written yet.
        f.verify = Some(feature::verify::VerifyRun {
            n: 1,
            started: 1_000,
            ..Default::default()
        });
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[]),
            None
        );
        // Ended: there.
        f.verify.as_mut().unwrap().finished = Some(1_100);
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[]),
            Some(PanelOutcome::Evidence)
        );
        assert!(hinted(&panel, &f));
        // Another page: not offered.
        panel.pick(&f, Stage::Build);
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none, &[]),
            None
        );
        assert!(!hinted(&panel, &f));
    }

    #[test]
    fn builds_row_says_setup_first_then_its_agent() {
        let mut f = feature("b", vec![repo("api", true)], 3);
        let agent = Some("Claude Code working");
        assert_eq!(build_line(&f, None, agent).as_deref(), agent);
        assert_eq!(build_line(&f, None, None), None);
        assert_eq!(
            build_line(&f, Some("services-api: npm ci (2 of 3)"), agent).as_deref(),
            Some("setting up: services-api: npm ci (2 of 3)")
        );
        let own = format!("Setting up \u{201c}{}\u{201d}", f.title);
        assert_eq!(
            build_line(&f, Some(&own), agent).as_deref(),
            Some("setting up")
        );
        let step = format!("{own}: api: npm ci (1 of 2)");
        assert_eq!(
            build_line(&f, Some(&step), agent).as_deref(),
            Some("setting up: api: npm ci (1 of 2)")
        );
        f.setup = Some(feature::setup::SetupRun {
            started: 1_000,
            finished: Some(1_100),
            failed: Some("api: `npm ci` exited with 1".into()),
            ..Default::default()
        });
        assert_eq!(
            build_line(&f, None, None).as_deref(),
            Some("setup failed: api: `npm ci` exited with 1")
        );
        // An agent started anyway: what it waits for comes first.
        let stopped = Some("milestone 1 of 3 done \u{b7} Enter to continue");
        assert_eq!(
            build_line(&f, None, stopped).as_deref(),
            Some("milestone 1 of 3 done \u{b7} Enter to continue \u{b7} setup failed")
        );
        // On Build only; a done Build says when.
        let state = PanelState::default();
        assert_eq!(
            stage_detail(&f, Stage::Build, &state, 2_000),
            "setup failed: api: `npm ci` exited with 1"
        );
        let done = feature("d", vec![repo("api", true)], 4);
        assert_eq!(
            stage_detail(&done, Stage::Build, &state, 2_060),
            "done 1m ago by you"
        );
    }

    #[test]
    fn verifys_row_says_what_runs_failed_or_that_it_has_no_checks() {
        let mut f = feature("v", vec![repo("api", true)], 4);
        let checks = RowCtx {
            checks: true,
            ..RowCtx::default()
        };
        let none = RowCtx::default();
        assert_eq!(
            verify_line(&f, none, None).as_deref(),
            Some("no checks for its repositories: set some, or Mark done by hand")
        );
        assert_eq!(verify_line(&f, checks, None), None);
        assert_eq!(
            verify_line(&f, checks, Some("api: npm test (2 of 5)")).as_deref(),
            Some("api: npm test (2 of 5)")
        );
        let failed = || feature::verify::VerifyRun {
            n: 2,
            started: 1_000,
            finished: Some(1_100),
            passed: Some(false),
            failures: vec!["api: `npm test` exited with 1".into(), "web: lint".into()],
            ..Default::default()
        };
        f.verify = Some(failed());
        assert_eq!(
            verify_line(&f, checks, None).as_deref(),
            Some("failed: api: `npm test` exited with 1")
        );
        // Passed: the stage's own words.
        f.verify = Some(feature::verify::VerifyRun {
            passed: Some(true),
            ..failed()
        });
        assert_eq!(verify_line(&f, checks, None), None);
        let state = PanelState {
            ctx: checks,
            ..PanelState::default()
        };
        assert_eq!(
            stage_detail(&f, Stage::Verify, &state, 2_000),
            Stage::Verify.about()
        );
        assert_eq!(stage_button(&f, Stage::Verify, checks), Some("Run checks"));
    }

    #[test]
    fn setup_and_checks_tasks_say_so_and_name_their_feature() {
        let busy = |job: Job, what: &str| Busy {
            project: "acme".into(),
            slug: "b".into(),
            job,
            repo: None,
            what: what.into(),
        };
        let task = task_of(
            &busy(Job::Setup, "services-api: npm ci (2 of 3)"),
            Some("Bulk"),
            "Acme",
        );
        assert_eq!(task.chip, "setup");
        assert_eq!(task.title, "services-api: npm ci (2 of 3)");
        assert_eq!(task.detail, "\u{201c}Bulk\u{201d} \u{b7} Acme");
        let task = task_of(
            &busy(Job::Verify, "Running the checks of \u{201c}Bulk\u{201d}"),
            Some("Bulk"),
            "Acme",
        );
        assert_eq!(task.chip, "checks");
        assert_eq!(task.detail, "Acme");
        let task = task_of(
            &busy(
                Job::Graph,
                "Building the code graph of \u{201c}Bulk\u{201d}",
            ),
            Some("Bulk"),
            "Acme",
        );
        assert_eq!(task.chip, "graph");
        assert_eq!(task.detail, "Acme");
        let task = task_of(
            &busy(Job::Create, "Creating \u{201c}Bulk\u{201d}"),
            None,
            "Acme",
        );
        assert_eq!(task.chip, "feature");
        assert_eq!(task.detail, "Acme");
    }

    #[test]
    fn inline_lowers_only_a_capitalised_word() {
        assert_eq!(inline("Setting up"), "setting up");
        assert_eq!(inline("API: npm ci"), "API: npm ci");
        assert_eq!(inline("api: npm ci"), "api: npm ci");
        assert_eq!(inline(""), "");
    }

    fn probe(ship: Option<ShipCheck>) -> Probe {
        Probe {
            reading: Ok(Reading {
                patch: "p1".into(),
                ..Reading::default()
            }),
            guards: BTreeMap::new(),
            models: BTreeMap::new(),
            ship,
            merged: false,
            stats: BTreeMap::new(),
            docs: feature::page::Docs::default(),
        }
    }

    fn check(blockers: &[&str], actions: Vec<Action>, notes: &[&str]) -> Option<ShipCheck> {
        Some(ShipCheck {
            blockers: blockers.iter().map(|b| (*b).to_owned()).collect(),
            actions,
            notes: notes.iter().map(|n| (*n).to_owned()).collect(),
        })
    }

    fn reviewing(states: &[FindingState], ready: bool) -> Feature {
        use crate::feature::{
            agents::StageRun,
            review_import::{AiReview, Finding},
        };
        let mut f = feature("r", vec![repo("api", true)], 5);
        f.runs.insert(
            Stage::AiReview,
            StageRun {
                n: 1,
                agent: "Codex".into(),
                pane: Some("p1".into()),
                started: 2_000,
                ready: ready.then_some(2_050),
                ..StageRun::default()
            },
        );
        let findings = states
            .iter()
            .enumerate()
            .map(|(i, state)| Finding {
                id: format!("r1-{}", i + 1),
                state: *state,
                ..Finding::default()
            })
            .collect();
        f.review = Some(AiReview {
            n: 1,
            findings,
            ..AiReview::default()
        });
        f
    }

    #[test]
    fn ai_review_shows_its_agent_then_its_findings_never_mark_done() {
        use FindingState as S;
        let row = PanelRow::Stage(Stage::AiReview);
        let none = RowCtx::default();
        let fresh = feature("r", vec![repo("api", true)], 5);
        assert_eq!(
            row_action(&fresh, row, none),
            Some(PanelOutcome::StartAgent(Stage::AiReview, false))
        );
        assert_eq!(
            stage_button(&fresh, Stage::AiReview, none),
            Some("Start agent")
        );
        // Running: its tab, whatever the earlier findings.
        let running = reviewing(&[S::Open], false);
        assert_eq!(
            row_action(&running, row, none),
            Some(PanelOutcome::ShowAgent(Stage::AiReview))
        );
        assert_eq!(
            review_line(&running, Some("Codex reviewing \u{b7} started 2m ago")).as_deref(),
            Some("Codex reviewing \u{b7} started 2m ago")
        );
        let open = reviewing(&[S::Open, S::Open, S::Fix, S::Dismissed], true);
        assert_eq!(row_action(&open, row, none), Some(PanelOutcome::Findings));
        assert_eq!(row_hint(&open, row, none), "review in diff");
        assert_eq!(
            stage_button(&open, Stage::AiReview, none),
            Some("Review in diff")
        );
        assert_eq!(
            review_line(&open, None).as_deref(),
            Some("2 findings to decide")
        );
        // While any waits, Plan fixes can't be pressed, so isn't there.
        let labels = |f: &Feature| -> Vec<String> {
            (page_actions(f, Stage::AiReview, none).into_iter())
                .map(|(b, _)| b.label)
                .collect()
        };
        assert!(labels(&open).contains(&"Fix all open".to_owned()));
        assert!(!labels(&open).contains(&"Plan fixes".to_owned()));
        let fix = reviewing(&[S::Fix, S::Dismissed], true);
        assert!(labels(&fix).contains(&"Plan fixes".to_owned()));
        assert!(!labels(&fix).contains(&"Fix all open".to_owned()));
        assert_eq!(row_action(&fix, row, none), Some(PanelOutcome::SendFixes));
        assert_eq!(row_hint(&fix, row, none), "plan fixes");
        assert_eq!(review_line(&fix, None).as_deref(), Some("1 finding to fix"));
        let sent = reviewing(&[S::Sent, S::Sent], true);
        assert_eq!(
            review_line(&sent, None).as_deref(),
            Some("2 findings sent to Build")
        );
        let decided = reviewing(&[S::Fixed, S::Dismissed], true);
        assert_eq!(
            review_line(&decided, None).as_deref(),
            Some("all findings fixed or dismissed")
        );
        assert_eq!(
            row_action(&decided, row, none),
            Some(PanelOutcome::ShowAgent(Stage::AiReview))
        );
        assert_eq!(
            review_line(&reviewing(&[], true), None).as_deref(),
            Some("no findings")
        );
        // Ctrl+R: a fresh reviewer.
        let mut panel = FeaturePanel {
            ..FeaturePanel::default()
        };
        assert_eq!(
            press(&mut panel, &open, ctrl(Keysym::r)),
            Some(PanelOutcome::StartAgent(Stage::AiReview, true))
        );
        let slugs = vec!["r".to_owned()];
        let state = PanelState::default();
        let view = tab_view(&panel, &open, &slugs, &state, 2_060);
        assert_eq!(
            stage_detail(&open, Stage::AiReview, &state, 2_060),
            "2 findings to decide"
        );
        assert_eq!(view.page.buttons[0].label, "Review in diff");
        assert_eq!(view.rail[5].mark, NodeMark::Waiting);
        assert_eq!(view.rail[5].line, "2 to decide");
        assert!(
            view.hints
                .contains(&("Ctrl+R".into(), "fresh agent".into()))
        );
        assert!(
            view.hints
                .contains(&("Enter".into(), "review in diff".into()))
        );
    }

    fn cards_of(view: &FeatureTabView) -> Vec<&FindingCard> {
        (view.page.sections.iter())
            .filter_map(|s| match s {
                PageSection::Finding(card) => Some(card),
                _ => None,
            })
            .collect()
    }

    /// The labels of `card`'s buttons on `page`.
    fn card_labels<'a>(page: &'a StagePage, card: &FindingCard) -> Vec<&'a str> {
        (card.buttons.iter())
            .map(|b| page.buttons[*b].label.as_str())
            .collect()
    }

    /// `f` with its AI review on its page and its findings placed in a file.
    fn carded(states: &[FindingState]) -> Feature {
        let mut f = reviewing(states, true);
        for (i, finding) in (f.review.as_mut().unwrap().findings.iter_mut()).enumerate() {
            finding.file = format!("src/{i}.rs");
            finding.title = format!("Title {i}");
            finding.placed = true;
        }
        f
    }

    #[test]
    fn findings_show_as_cards_to_decide_first_each_with_what_it_can_do() {
        use FindingState as S;
        let mut f = carded(&[S::Dismissed, S::Fix, S::Open, S::Sent, S::Fixed]);
        (f.review.as_mut().unwrap().findings[0]).reason = "by design".into();
        let panel = FeaturePanel::default();
        let slugs = vec!["r".to_owned()];
        let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
        let cards = cards_of(&view);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Title 2", "Title 1", "Title 3", "Title 4", "Title 0"]
        );
        let labels: Vec<Vec<&str>> = cards.iter().map(|c| card_labels(&view.page, c)).collect();
        assert_eq!(
            labels[0],
            ["Fix", "Edit\u{2026}", "Dismiss\u{2026}", "Show in diff"]
        );
        assert_eq!(
            labels[1],
            [
                "Don't fix",
                "Edit\u{2026}",
                "Dismiss\u{2026}",
                "Show in diff"
            ]
        );
        assert_eq!(labels[2], ["Show in diff"]);
        assert_eq!(labels[3], ["Show in diff"]);
        assert_eq!(labels[4], ["Reopen", "Show in diff"]);
        assert_eq!(cards[1].state, "to fix");
        assert_eq!(cards[4].state, "dismissed: by design");
        assert!(cards[4].faded && !cards[0].faded);
        assert!(cards[0].place.contains("src/2.rs"));
        // No button of a card is the page's own, and the count says it all.
        let on_cards: Vec<usize> = cards.iter().flat_map(|c| c.buttons.clone()).collect();
        let own: Vec<&str> = (0..view.page.buttons.len())
            .filter(|b| !on_cards.contains(b))
            .map(|b| view.page.buttons[b].label.as_str())
            .collect();
        assert!(
            own.contains(&"Review in diff") && own.contains(&"Add comment\u{2026}"),
            "{own:?}"
        );
        assert!(view.page.sections.iter().any(|s| matches!(s,
            PageSection::Lines { label, .. }
                if label == "Findings \u{b7} 1 to decide \u{b7} 1 to fix \u{b7} 1 sent to Build \u{b7} 1 fixed \u{b7} 1 dismissed")));
    }

    #[test]
    fn a_finding_says_whose_fix_it_is_and_an_item_of_yours_has_no_place() {
        use FindingState as S;
        let mut f = carded(&[S::Open, S::Fix]);
        let findings = &mut f.review.as_mut().unwrap().findings;
        findings[0].fix = "Reviewer's".into();
        findings[1].fix = "Reviewer's".into();
        findings[1].my_fix = Some("Mine".into());
        findings[1].note = "Careful".into();
        review_import::add(f.review.as_mut().unwrap(), "Also rename it\nIt reads badly").unwrap();
        let panel = FeaturePanel::default();
        let slugs = vec!["r".to_owned()];
        let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
        let cards = cards_of(&view);
        assert_eq!(cards[0].fix, "Fix: Reviewer's");
        assert_eq!(cards[1].fix, "Your fix: Mine");
        assert_eq!(cards[1].note, "Your comment: Careful");
        let yours = cards[2];
        assert_eq!(yours.tag, FindingTag::Yours);
        assert_eq!(yours.title, "Also rename it");
        assert_eq!(yours.body, "It reads badly");
        assert_eq!(yours.place, "u1");
        assert_eq!(card_labels(&view.page, yours), ["Edit\u{2026}", "Drop"]);
    }

    #[test]
    fn a_done_ai_review_only_shows_its_findings() {
        use FindingState as S;
        let mut f = carded(&[S::Fixed, S::Dismissed]);
        f.tick(2_070, "gerdr", None);
        assert_ne!(f.current(), Some(Stage::AiReview));
        let mut panel = FeaturePanel::default();
        panel.pick(&f, Stage::AiReview);
        let slugs = vec!["r".to_owned()];
        let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_080);
        let cards = cards_of(&view);
        assert_eq!(cards.len(), 2);
        for card in cards {
            assert_eq!(card_labels(&view.page, card), ["Show in diff"]);
        }
        assert!(
            !view
                .page
                .buttons
                .iter()
                .any(|b| b.label.starts_with("Add comment"))
        );
        assert_eq!(press(&mut panel, &f, key(Keysym::c)), None);
    }

    #[test]
    fn tab_goes_through_the_findings_and_shift_tab_comes_back() {
        use FindingState as S;
        let f = carded(&[S::Open]);
        let mut panel = FeaturePanel::default();
        assert_eq!(panel.focus, TabArea::Stages);
        press(&mut panel, &f, key(Keysym::Tab));
        assert_eq!(panel.focus, TabArea::Findings);
        press(&mut panel, &f, key(Keysym::Tab));
        assert_eq!(panel.focus, TabArea::Repos);
        press(&mut panel, &f, key(Keysym::Tab));
        assert_eq!(panel.focus, TabArea::Stages);
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
        assert_eq!(panel.focus, TabArea::Repos);
        press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
        assert_eq!(panel.focus, TabArea::Findings);
        press(&mut panel, &f, (shift, Keysym::ISO_Left_Tab));
        assert_eq!(panel.focus, TabArea::Stages);
        // With no cards the findings are no stop, and lose the keyboard.
        let none = reviewing(&[], false);
        press(&mut panel, &none, key(Keysym::Tab));
        assert_eq!(panel.focus, TabArea::Repos);
        panel.focus = TabArea::Findings;
        press(&mut panel, &none, key(Keysym::F2));
        assert_eq!(panel.focus, TabArea::Stages);
    }

    /// A review read and a run of it, with the code changed since: AI review
    /// is a stage to come.
    fn sent_back(f: &mut Feature) {
        f.marks.retain(|stage, _| *stage < Stage::Build);
        assert_eq!(stage_mark(f, Stage::AiReview), StageMark::Later);
    }

    /// `f`, its last review kept as earlier run 1, with one fixed finding.
    fn with_round(mut f: Feature) -> Feature {
        let review = f.review.as_mut().unwrap();
        review.rounds.push(ReviewRound {
            n: 1,
            findings: vec![
                ReviewFinding {
                    id: "r1-9".into(),
                    title: "Old fixed".into(),
                    file: "src/old.rs".into(),
                    state: FindingState::Fixed,
                    ..ReviewFinding::default()
                },
                ReviewFinding {
                    id: "r1-8".into(),
                    title: "Old raised".into(),
                    state: FindingState::Sent,
                    ..ReviewFinding::default()
                },
            ],
        });
        review.n = 2;
        f
    }

    fn view_of(f: &Feature, state: &PanelState) -> FeatureTabView {
        let panel = FeaturePanel {
            stage: Some(Stage::AiReview),
            picked_at: f.current(),
            ..FeaturePanel::default()
        };
        tab_view(&panel, f, &["r".to_owned()], state, 2_060)
    }

    fn labels_of(view: &FeatureTabView, card: &FindingCard) -> Vec<String> {
        (card.buttons.iter())
            .map(|i| view.page.buttons[*i].label.clone())
            .collect()
    }

    #[test]
    fn the_last_review_stays_once_its_fixes_change_the_code() {
        use FindingState as S;
        let mut f = carded(&[S::Sent, S::Dismissed]);
        sent_back(&mut f);
        let view = view_of(&f, &PanelState::default());
        let cards = cards_of(&view);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].state, "sent to Build");
        // Only to look at, and it says why.
        assert_eq!(labels_of(&view, cards[0]), ["Show in diff"]);
        assert_eq!(labels_of(&view, cards[1]), ["Show in diff"]);
        let said = (view.page.sections.iter()).any(|s| match s {
            PageSection::Lines { label, lines } => {
                label.starts_with("Findings")
                    && lines.iter().any(|l| l.text.contains("code has changed"))
            }
            _ => false,
        });
        assert!(said, "{:?}", view.page.sections);
    }

    #[test]
    fn the_last_review_stays_while_the_next_runs() {
        use FindingState as S;
        let f = {
            let mut f = carded(&[S::Fixed, S::Open]);
            f.runs.get_mut(&Stage::AiReview).unwrap().ready = None;
            f
        };
        let view = view_of(&f, &PanelState::default());
        let cards = cards_of(&view);
        assert_eq!(cards.len(), 2);
        assert_eq!(labels_of(&view, cards[0]), ["Show in diff"]);
        let said = (view.page.sections.iter()).any(|s| match s {
            PageSection::Lines { lines, .. } => {
                lines.iter().any(|l| l.text.contains("hasn't finished"))
            }
            _ => false,
        });
        assert!(said, "{:?}", view.page.sections);
    }

    #[test]
    fn a_review_that_is_only_to_look_at_refuses_the_keys_that_decide() {
        use FindingState as S;
        let mut sent = carded(&[S::Open]);
        sent_back(&mut sent);
        let mut running = carded(&[S::Open]);
        running.runs.get_mut(&Stage::AiReview).unwrap().ready = None;
        for f in [&sent, &running] {
            let mut panel = FeaturePanel {
                focus: TabArea::Findings,
                stage: Some(Stage::AiReview),
                picked_at: f.current(),
                ..FeaturePanel::default()
            };
            let refused = |out: Option<PanelOutcome>| {
                assert!(
                    matches!(out, Some(PanelOutcome::FindingRefused(_))),
                    "{out:?}"
                );
            };
            refused(press(&mut panel, f, key(Keysym::f)));
            refused(press(&mut panel, f, ctrl(Keysym::e)));
            refused(press(&mut panel, f, key(Keysym::Delete)));
            assert_eq!(
                press(&mut panel, f, key(Keysym::Return)),
                Some(PanelOutcome::ShowFinding("r1-1".to_owned()))
            );
        }
    }

    #[test]
    fn an_earlier_review_folds_and_shows_as_a_record() {
        use FindingState as S;
        let f = with_round(carded(&[S::Open]));
        // Folded: its name and how it went, with one button.
        let view = view_of(&f, &PanelState::default());
        assert_eq!(cards_of(&view).len(), 1);
        let card = view.page.sections.iter().find_map(|s| match s {
            PageSection::File {
                name,
                state,
                buttons,
            } => Some((name, state, buttons)),
            _ => None,
        });
        let (name, said, buttons) = card.expect("the round's card");
        assert_eq!(name, "Review 1");
        assert_eq!(said.text, "1 fixed \u{b7} 1 raised again");
        assert_eq!(view.page.buttons[buttons[0]].label, "Show");

        // Shown: its cards below the latest's, with no buttons at all, even
        // on a finding with the id of one that has buttons.
        let state = PanelState {
            rounds_open: BTreeSet::from([1]),
            ..PanelState::default()
        };
        let view = view_of(&f, &state);
        let cards = cards_of(&view);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Title 0", "Old raised", "Old fixed"]);
        assert_eq!(cards[1].state, "raised again");
        assert!(cards[1].faded && cards[1].buttons.is_empty());
        assert!(cards[2].faded && cards[2].buttons.is_empty());
        let label = (view.page.buttons.iter()).find(|b| b.label == "Hide");
        assert!(label.is_some(), "{:?}", view.page.buttons);
    }

    #[test]
    fn an_earlier_review_shows_on_a_page_sent_back_too() {
        use FindingState as S;
        let mut f = with_round(carded(&[S::Sent]));
        sent_back(&mut f);
        let view = view_of(&f, &PanelState::default());
        let card = view.page.sections.iter().find_map(|s| match s {
            PageSection::File { buttons, .. } => Some(buttons),
            _ => None,
        });
        let buttons = card.expect("the round's card");
        assert_eq!(view.page.buttons[buttons[0]].label, "Show");
    }

    #[test]
    fn alt_e_shows_the_earlier_reviews_or_folds_them() {
        use FindingState as S;
        let alt = ModifiersState {
            alt: true,
            ..ModifiersState::default()
        };
        let f = with_round(carded(&[S::Open]));
        let mut panel = FeaturePanel {
            stage: Some(Stage::AiReview),
            picked_at: f.current(),
            ..FeaturePanel::default()
        };
        assert_eq!(press(&mut panel, &f, (alt, Keysym::e)), None);
        assert_eq!(panel.rounds_open, BTreeSet::from([1]));
        press(&mut panel, &f, (alt, Keysym::e));
        assert!(panel.rounds_open.is_empty());
        // The button shows one; the key then folds what shows.
        panel.toggle_round(1);
        assert_eq!(panel.rounds_open, BTreeSet::from([1]));
        press(&mut panel, &f, (alt, Keysym::e));
        assert!(panel.rounds_open.is_empty());
        // With none to show, it does nothing.
        let none = carded(&[S::Open]);
        press(&mut panel, &none, (alt, Keysym::e));
        assert!(panel.rounds_open.is_empty());
    }

    #[test]
    fn the_footer_says_only_what_works_on_the_cards() {
        use FindingState as S;
        let keys_of = |f: &Feature| -> Vec<String> {
            let panel = FeaturePanel {
                focus: TabArea::Findings,
                stage: Some(Stage::AiReview),
                picked_at: f.current(),
                ..FeaturePanel::default()
            };
            let view = tab_view(&panel, f, &["r".to_owned()], &PanelState::default(), 2_060);
            view.hints.iter().map(|(key, _)| key.to_string()).collect()
        };
        let open = with_round(carded(&[S::Open]));
        let keys = keys_of(&open);
        for key in ["f", "Ctrl+E", "Del", "Enter", "Alt+E"] {
            assert!(keys.iter().any(|k| k == key), "{key} in {keys:?}");
        }
        let mut sent = with_round(carded(&[S::Open]));
        sent_back(&mut sent);
        let keys = keys_of(&sent);
        for key in ["f", "Ctrl+E", "Del"] {
            assert!(!keys.iter().any(|k| k == key), "{key} in {keys:?}");
        }
        assert!(keys.iter().any(|k| k == "Enter"), "{keys:?}");
        assert!(keys.iter().any(|k| k == "Alt+E"), "{keys:?}");
    }

    #[test]
    fn the_keys_pick_a_card_and_decide_it_as_in_the_diff() {
        use FindingState as S;
        let mut f = carded(&[S::Open, S::Fix, S::Dismissed, S::Sent]);
        let mut panel = FeaturePanel {
            focus: TabArea::Findings,
            ..FeaturePanel::default()
        };
        let id = |n: &str| n.to_owned();
        // r1-1, undecided.
        assert_eq!(
            press(&mut panel, &f, key(Keysym::f)),
            Some(PanelOutcome::DecideFinding(id("r1-1"), Decision::Fix))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::AskDismiss(id("r1-1")))
        );
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::e)),
            Some(PanelOutcome::EditFinding(id("r1-1")))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::ShowFinding(id("r1-1")))
        );
        // r1-2, to fix: Down is the next card, as ordered.
        assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
        assert_eq!(panel.finding, 1);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::f)),
            Some(PanelOutcome::DecideFinding(id("r1-2"), Decision::Reopen))
        );
        // r1-4, sent: nothing to decide, and it says why.
        assert_eq!(press(&mut panel, &f, key(Keysym::Down)), None);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::f)),
            Some(PanelOutcome::FindingRefused(review_import::SENT))
        );
        // r1-3, dismissed, last: both of its keys bring it back or fix it.
        assert_eq!(press(&mut panel, &f, key(Keysym::End)), None);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::f)),
            Some(PanelOutcome::DecideFinding(id("r1-3"), Decision::Fix))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::DecideFinding(id("r1-3"), Decision::Reopen))
        );
        // All open ones to fix at once, from anywhere on the page.
        let shift_f = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            panel.key(&shift_f, Keysym::F, &f, &[], RowCtx::default(), &[]),
            Some(PanelOutcome::FixAllFindings)
        );
        // A comment from anywhere on the page.
        assert_eq!(
            press(&mut panel, &f, key(Keysym::c)),
            Some(PanelOutcome::AddComment)
        );
        panel.focus = TabArea::Stages;
        assert_eq!(
            press(&mut panel, &f, key(Keysym::c)),
            Some(PanelOutcome::AddComment)
        );
        // Over the repositories it isn't offered.
        panel.focus = TabArea::Repos;
        assert_eq!(press(&mut panel, &f, key(Keysym::c)), None);
        panel.focus = TabArea::Stages;
        // Off the cards Del is the tab's, and f nothing.
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
        assert_eq!(press(&mut panel, &f, key(Keysym::f)), None);
        // An item of the user's: dropped at once, Enter edits it.
        let added = review_import::add(f.review.as_mut().unwrap(), "Rename it").unwrap();
        (f.review.as_mut().unwrap().findings).retain(|finding| finding.id == added);
        panel.focus = TabArea::Findings;
        panel.finding = 0;
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::DecideFinding(
                added.clone(),
                Decision::Dismiss(review_import::DROPPED.to_owned())
            ))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::EditFinding(added))
        );
    }

    #[test]
    fn the_card_with_the_keyboard_shows_it_and_the_hints_say_what_it_takes() {
        use FindingState as S;
        let f = carded(&[S::Open, S::Fix]);
        let panel = FeaturePanel {
            focus: TabArea::Findings,
            finding: 1,
            ..FeaturePanel::default()
        };
        let slugs = vec!["r".to_owned()];
        let view = tab_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
        assert_eq!(view.focus, TabArea::Findings);
        let selected: Vec<bool> = cards_of(&view).iter().map(|c| c.selected).collect();
        assert_eq!(selected, [false, true]);
        for hint in [
            ("Up / Down", "finding"),
            ("f", "don't fix"),
            ("Ctrl+E", "edit\u{2026}"),
            ("Del", "dismiss\u{2026}"),
            ("Enter", "show in diff"),
            ("c", "add comment\u{2026}"),
            ("Tab", "repositories"),
        ] {
            let hint = (hint.0.to_owned(), hint.1.to_owned());
            assert!(view.hints.contains(&hint), "{hint:?}: {:?}", view.hints);
        }
        // Del is the card's here: it says so once.
        assert_eq!(view.hints.iter().filter(|(k, _)| k == "Del").count(), 1);
        let stages = FeaturePanel::default();
        let view = tab_view(&stages, &f, &slugs, &PanelState::default(), 2_060);
        assert!(view.hints.contains(&("Tab".into(), "findings".into())));
        assert!(cards_of(&view).iter().all(|c| !c.selected));
    }

    #[test]
    fn code_review_shows_the_diff() {
        let f = feature("c", vec![repo("api", true)], 6);
        let row = PanelRow::Stage(Stage::CodeReview);
        let none = RowCtx::default();
        assert_eq!(row_action(&f, row, none), Some(PanelOutcome::CodeReview));
        assert_eq!(row_hint(&f, row, none), "review code");
        assert_eq!(
            stage_button(&f, Stage::CodeReview, none),
            Some("Review code")
        );
        let state = PanelState::default();
        assert_eq!(
            stage_detail(&f, Stage::CodeReview, &state, 2_060),
            "review every changed file"
        );
        let state = PanelState {
            code_review: Some("3 of 7 files viewed".into()),
            ..PanelState::default()
        };
        assert_eq!(
            stage_detail(&f, Stage::CodeReview, &state, 2_060),
            "3 of 7 files viewed"
        );
        let actions = page_actions(&f, Stage::CodeReview, none);
        let page = stage_page(&f, Stage::CodeReview, &state, &actions, 2_060);
        assert_eq!(page.buttons[0].label, "Review code");
        assert_eq!(
            page.sections,
            [lines(
                "Files",
                vec![line("3 of 7 files viewed", CheckTone::Note)]
            )]
        );
    }

    fn texts(lines: &[CheckLine]) -> Vec<(&str, CheckTone)> {
        lines.iter().map(|l| (l.text.as_str(), l.tone)).collect()
    }

    #[test]
    fn manual_test_goes_by_its_criteria_when_the_brief_has_some() {
        let f = feature("m", vec![repo("api", true)], 7);
        assert_eq!(f.current(), Some(Stage::ManualTest));
        let row = PanelRow::Stage(Stage::ManualTest);
        // Without criteria it is marked done by hand, as before.
        let none = RowCtx::default();
        assert_eq!(row_action(&f, row, none), Some(PanelOutcome::Tick));
        assert_eq!(stage_button(&f, Stage::ManualTest, none), Some("Mark done"));
        let some = RowCtx {
            criteria: 3,
            ..RowCtx::default()
        };
        assert_eq!(row_action(&f, row, some), Some(PanelOutcome::ManualTest));
        assert_eq!(row_hint(&f, row, some), "try the criteria");
        // Their cards are the way in: no button leads there.
        assert_eq!(stage_button(&f, Stage::ManualTest, some), None);
        let state = PanelState {
            ctx: some,
            manual: Some("2 of 3 criteria passed \u{b7} 1 failed".into()),
            manual_gate: Some((2, 3)),
            ..PanelState::default()
        };
        assert_eq!(
            stage_detail(&f, Stage::ManualTest, &state, 2_060),
            "2 of 3 criteria passed \u{b7} 1 failed"
        );
        let node = rail_node(&f, Stage::ManualTest, &state, 2_060);
        assert_eq!(
            (node.mark, node.line.as_str()),
            (NodeMark::Waiting, "2 of 3 passed")
        );
        let actions = page_actions(&f, Stage::ManualTest, some);
        assert_eq!(actions[0].0.label, "Mark passed");
        assert_eq!(actions[0].0.style, ButtonStyle::Plain);
    }

    #[test]
    fn manual_tests_buttons_press_what_comes_next() {
        use crate::{feature::manual::Counts, manual_test::Outcome};
        let f = feature("m", vec![repo("api", true)], 7);
        let ctx = |passed, failed, sent, untested| RowCtx {
            criteria: 3,
            manual: Counts {
                passed,
                failed,
                sent,
                untested,
            },
            ..RowCtx::default()
        };
        let enter = |ctx| page_enter(&f, Stage::ManualTest, ctx);
        // Nothing tried: Enter goes to the criteria.
        assert_eq!(enter(ctx(0, 0, 0, 3)), Some(PanelOutcome::ManualTest));
        // A failure not sent: its fixes come next.
        assert_eq!(
            enter(ctx(2, 1, 0, 0)),
            Some(PanelOutcome::Manual(Outcome::SendFailures))
        );
        // All sent: nothing to send, back to the criteria.
        let labels = |ctx| -> Vec<String> {
            (page_actions(&f, Stage::ManualTest, ctx).into_iter())
                .map(|(b, _)| b.label)
                .collect()
        };
        assert!(!labels(ctx(2, 1, 1, 0)).contains(&"Plan fixes".to_owned()));
        assert_eq!(enter(ctx(2, 1, 1, 0)), Some(PanelOutcome::ManualTest));
        // All passed: it can be marked.
        assert_eq!(
            enter(ctx(3, 0, 0, 0)),
            Some(PanelOutcome::Manual(Outcome::MarkPassed))
        );
    }

    #[test]
    fn a_criterions_card_holds_its_keys_and_buttons() {
        use crate::{chrome::FindingTag, manual_test::Outcome};
        let f = feature("m", vec![repo("api", true)], 7);
        let row = |found, note: &str| Row {
            id: "AC-1".into(),
            text: "Given a, when b, then c".into(),
            found,
            note: note.into(),
            sent: false,
        };
        let rows = vec![
            row(None, ""),
            row(Some(Verdict::Pass), ""),
            row(Some(Verdict::Fail), "it broke"),
        ];
        let ctx = RowCtx {
            criteria: 3,
            ..RowCtx::default()
        };
        let state = PanelState {
            ctx,
            criteria: rows.clone(),
            ..PanelState::default()
        };
        let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows);
        let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
        // Under the label, in their order.
        assert!(matches!(
            &page.sections[0],
            PageSection::Lines { label, .. } if label == "Criteria"
        ));
        let cards: Vec<&FindingCard> = (page.sections.iter())
            .filter_map(|s| match s {
                PageSection::Finding(card) => Some(card),
                _ => None,
            })
            .collect();
        let tags: Vec<FindingTag> = cards.iter().map(|c| c.tag).collect();
        assert_eq!(
            tags,
            [FindingTag::NotRun, FindingTag::Passed, FindingTag::Failed]
        );
        assert_eq!(cards[2].note, "What went wrong: it broke");
        let labels = |card: &FindingCard| -> Vec<String> {
            let labels = card.buttons.iter();
            labels.map(|b| page.buttons[*b].label.clone()).collect()
        };
        assert_eq!(labels(cards[0]), ["Pass", "Fail\u{2026}"]);
        assert_eq!(labels(cards[1]), ["Clear", "Fail\u{2026}"]);
        assert_eq!(labels(cards[2]), ["Clear", "Edit note\u{2026}", "Pass"]);
        // Not on another stage's page, nor once done.
        assert!(
            stage_actions(&f, Stage::Build, ctx, &[], &rows)
                .iter()
                .all(|(_, o)| !matches!(o, PanelOutcome::Manual(Outcome::Toggle(_))))
        );
        // The keys, with Tab to get there.
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "m".into(),
            ..FeaturePanel::default()
        };
        let slugs = vec!["m".to_owned()];
        let go = |panel: &mut FeaturePanel, (mods, sym): (ModifiersState, Keysym)| {
            panel.key(&mods, sym, &f, &slugs, ctx, &[])
        };
        assert_eq!(go(&mut panel, key(Keysym::space)), None);
        assert_eq!(go(&mut panel, key(Keysym::Tab)), None);
        assert_eq!(panel.focus, TabArea::Findings);
        assert_eq!(go(&mut panel, key(Keysym::Down)), None);
        assert_eq!(panel.finding, 1);
        let on = |o| Some(PanelOutcome::Manual(o));
        assert_eq!(go(&mut panel, key(Keysym::space)), on(Outcome::Toggle(1)));
        assert_eq!(go(&mut panel, key(Keysym::Return)), on(Outcome::Toggle(1)));
        assert_eq!(go(&mut panel, key(Keysym::f)), on(Outcome::Write(1)));
        assert_eq!(go(&mut panel, key(Keysym::Delete)), on(Outcome::Clear(1)));
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let both = ModifiersState {
            shift: true,
            ..ctrl
        };
        assert_eq!(
            go(&mut panel, (ctrl, Keysym::Return)),
            on(Outcome::MarkPassed)
        );
        assert_eq!(
            go(&mut panel, (both, Keysym::Return)),
            on(Outcome::SendFailures)
        );
        // Writing a note, the footer has only its keys.
        panel.note = Some((1, TextField::default()));
        let hints = tab_hints(
            &panel,
            &f,
            Stage::ManualTest,
            &slugs,
            ctx,
            AgentChoices::default(),
            &[],
        );
        assert_eq!(
            hints,
            [
                ("Enter".to_owned(), "save".to_owned()),
                ("Esc".to_owned(), "cancel".to_owned())
            ]
        );
    }

    #[test]
    fn an_approved_manual_test_shows_its_criteria_with_nothing_to_press() {
        use crate::chrome::FindingTag;
        let f = feature("m", vec![repo("api", true)], 8);
        assert_eq!(stage_mark(&f, Stage::ManualTest), StageMark::Done);
        let row = |found| Row {
            id: "AC-1".into(),
            text: "Given a, when b, then c".into(),
            found,
            note: String::new(),
            sent: false,
        };
        let rows = vec![row(Some(Verdict::Pass)), row(Some(Verdict::Pass))];
        let ctx = RowCtx {
            criteria: 2,
            ..RowCtx::default()
        };
        let state = PanelState {
            ctx,
            criteria: rows.clone(),
            ..PanelState::default()
        };
        let actions = stage_actions(&f, Stage::ManualTest, ctx, &[], &rows);
        let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
        let cards: Vec<&FindingCard> = (page.sections.iter())
            .filter_map(|s| match s {
                PageSection::Finding(card) => Some(card),
                _ => None,
            })
            .collect();
        assert_eq!(cards.len(), 2);
        // Readable, not faded as on the way, and with no buttons.
        assert!(cards.iter().all(|c| c.tag == FindingTag::Passed));
        assert!(cards.iter().all(|c| !c.faded && c.buttons.is_empty()));
        assert!(matches!(
            (page.sections.iter()).find(|s| matches!(s, PageSection::Lines { .. })),
            Some(PageSection::Lines { label, lines, .. }) if label == "Criteria" && lines.is_empty()
        ));
        // Tab and the arrows go through them; nothing changes a result.
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "m".into(),
            ..FeaturePanel::default()
        };
        panel.pick(&f, Stage::ManualTest);
        let slugs = vec!["m".to_owned()];
        let mut go = |sym| {
            let (mods, sym) = key(sym);
            (
                panel.key(&mods, sym, &f, &slugs, ctx, &[]),
                panel.focus,
                panel.finding,
            )
        };
        assert_eq!(go(Keysym::Tab), (None, TabArea::Findings, 0));
        assert_eq!(go(Keysym::Down), (None, TabArea::Findings, 1));
        for sym in [Keysym::space, Keysym::f, Keysym::Delete] {
            assert_eq!(go(sym).0, None);
        }
        let hints = tab_hints(
            &panel,
            &f,
            Stage::ManualTest,
            &slugs,
            ctx,
            AgentChoices::default(),
            &[],
        );
        assert!(hints.iter().any(|(_, what)| what == "criterion"));
        assert!(hints.iter().any(|(key, _)| key == REOPEN_KEY));
        assert!(
            !hints
                .iter()
                .any(|(_, what)| what == "pass" || what == "clear")
        );
        // What the footer says it reopens, from the cards too.
        let both = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            panel.key(&both, Keysym::Return, &f, &slugs, ctx, &[]),
            Some(PanelOutcome::Untick(Stage::ManualTest))
        );
        // Ticked by hand, they say so rather than read as not tried yet.
        let rows = vec![row(None), row(None)];
        let state = PanelState {
            ctx,
            criteria: rows,
            ..PanelState::default()
        };
        let page = stage_page(&f, Stage::ManualTest, &state, &actions, 2_060);
        assert!(matches!(
            (page.sections.iter()).find(|s| matches!(s, PageSection::Lines { .. })),
            Some(PageSection::Lines { lines, .. })
                if lines.first().is_some_and(|l| l.text == "marked done by hand, not tried")
        ));
        // A finished feature stuck on it shows them, and nothing tries them.
        let mut left = feature("l", vec![repo("api", true)], 7);
        assert!(tries_criteria(&left, Stage::ManualTest));
        left.status = Status::Abandoned;
        assert!(!tries_criteria(&left, Stage::ManualTest));
        assert!(shows_criteria(&left, Stage::ManualTest));
    }

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
            panel.key(&mods, sym, &shipping, &slugs, RowCtx::default(), &[]),
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
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, RowCtx::default(), &[]),
            Some(PanelOutcome::OpenPr(url.into()))
        );
        // Ctrl+O still opens a tab; on a row without a PR, nothing.
        let (mods, sym) = ctrl(Keysym::o);
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, RowCtx::default(), &[]),
            Some(PanelOutcome::Open(f.dir.clone()))
        );
        panel.repo = 1;
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, RowCtx::default(), &[]),
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
        assert_eq!(labels, ["Pull requests"], "no Ship check once shipped");
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
    fn the_ship_check_shows_what_stops_it_or_that_it_is_ready() {
        let f = feature("s", vec![repo("api", true)], 8);
        let none = check_lines(&f, None, false, 2_060);
        assert_eq!(texts(&none), [("checking\u{2026}", CheckTone::Note)]);
        let blocked = probe(check(&["api is dirty", "web is behind"], Vec::new(), &[]));
        assert_eq!(
            texts(&check_lines(&f, Some(&blocked), false, 2_060)),
            [
                ("api is dirty", CheckTone::Blocker),
                ("web is behind", CheckTone::Blocker)
            ]
        );
        let many = probe(check(&["a", "b", "c", "d", "e", "f"], Vec::new(), &[]));
        let lines = check_lines(&f, Some(&many), false, 2_060);
        assert_eq!(lines.len(), MAX_CHECK_LINES);
        assert_eq!(lines[MAX_CHECK_LINES - 1].text, "and 3 more");
        let push = Action::Push { repo: "api".into() };
        let ready = probe(check(&[], vec![push.clone()], &["api pushes to origin"]));
        assert_eq!(
            texts(&check_lines(&f, Some(&ready), false, 2_060)),
            [
                ("Ready to ship", CheckTone::Ready),
                ("api pushes to origin", CheckTone::Note)
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
    }

    #[test]
    fn shipping_asks_with_the_last_check_or_says_why_not() {
        assert_eq!(ship_ask("T", None), ShipAsk::Checking);
        assert_eq!(ship_ask("T", Some(&probe(None))), ShipAsk::Checking);
        let blocked = probe(check(&["a", "b", "c"], Vec::new(), &[]));
        assert_eq!(
            ship_ask("T", Some(&blocked)),
            ShipAsk::Blocked("Can't ship \u{201c}T\u{201d} yet: a, and 2 more".into())
        );
        let one = probe(check(&["a"], Vec::new(), &[]));
        assert_eq!(
            ship_ask("T", Some(&one)),
            ShipAsk::Blocked("Can't ship \u{201c}T\u{201d} yet: a".into())
        );
        assert_eq!(
            ship_ask("T", Some(&probe(check(&[], Vec::new(), &[])))),
            ShipAsk::Nothing
        );
        let actions = vec![Action::Push { repo: "api".into() }, Action::LinkPrs];
        assert_eq!(
            ship_ask("T", Some(&probe(check(&[], actions.clone(), &[])))),
            ShipAsk::Ready(Asked {
                actions,
                patch: Some("p1".into())
            })
        );
    }

    #[test]
    fn the_toasts_name_the_feature_and_what_happened() {
        assert_eq!(
            changed_text("Bulk", Stage::Build),
            "\u{201c}Bulk\u{201d} changed: back to Build"
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
        assert_eq!(remove_label(&all[2]), "Remove worktrees");
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
    fn removing_names_what_is_lost() {
        assert_eq!(
            removal_body(&[]),
            "Its worktrees go; nothing in them is lost"
        );
        assert_eq!(
            removal_body(&["2 changed files in api".into()]),
            "Its worktrees go, and with them 2 changed files in api"
        );
    }

    #[test]
    fn switching_goes_round_and_needs_two() {
        let slugs: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(switch(&slugs, "b", false).as_deref(), Some("a"));
        assert_eq!(switch(&slugs, "a", true).as_deref(), Some("b"));
        assert_eq!(switch(&slugs[..1], "a", true), None);
    }

    /// What `actions` say on their buttons, with what they do.
    fn labels(actions: &[(PageButton, PanelOutcome)]) -> Vec<(&str, PanelOutcome)> {
        (actions.iter())
            .map(|(b, o)| (b.label.as_str(), o.clone()))
            .collect()
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
                    line("failed: api: `npm ci` exited with 1", CheckTone::Blocker),
                    line(
                        "couldn't copy api: .venv (exited with 1)",
                        CheckTone::Blocker
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
                    icon: AgentIcon::Claude,
                    name: "Claude Code".into(),
                    dot: Dot::Working,
                    status: "working \u{b7} started 16m ago".into(),
                    tail: Vec::new(),
                    buttons: vec![0, 1],
                    picks: Vec::new(),
                    model: None,
                },
                PageSection::MilestoneMode { fresh: false }
            ]
        );
        let node = rail_node(&f, Stage::Build, &state, 2_000);
        assert_eq!(
            (node.mark, node.line.as_str()),
            (NodeMark::Current, "running 16m")
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
                done: 1,
                total: 3,
                items: Vec::new()
            })
        );
        let PageSection::Agent { buttons, .. } = &page.sections[0] else {
            panic!("the agent's card first");
        };
        assert_eq!(buttons[..2], [0, 1], "Continue, then Show agent");
        let node = rail_node(&f, Stage::Build, &state, 2_000);
        assert_eq!(
            (node.mark, node.line.as_str()),
            (NodeMark::Waiting, "milestone 1/3")
        );
        // Its setup failing shows above.
        f.setup = Some(feature::setup::SetupRun {
            started: 1_000,
            finished: Some(1_100),
            failed: Some("api: `npm ci` exited with 1".into()),
            ..Default::default()
        });
        let page = stage_page(&f, Stage::Build, &state, &actions, 2_000);
        assert_eq!(
            page.sections[0],
            lines(
                "Setup",
                vec![line(
                    "failed: api: `npm ci` exited with 1",
                    CheckTone::Blocker
                )]
            )
        );
        assert_eq!(page.status.map(|s| s.tone), Some(CheckTone::Blocker));
    }

    #[test]
    fn a_plan_ready_asks_for_its_review() {
        let mut f = feature("p", vec![repo("api", true)], 1);
        let node = rail_node(&f, Stage::Plan, &PanelState::default(), 2_000);
        assert_eq!((node.mark, node.line.as_str()), (NodeMark::Current, ""));
        let page = stage_page(&f, Stage::Plan, &PanelState::default(), &[], 2_000);
        assert_eq!(
            page.sections,
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
        // Show agent and Restart are its card's, Start over the stage's.
        let PageSection::Agent { buttons, .. } = &page.sections[0] else {
            panic!("the agent's card first");
        };
        assert_eq!(*buttons, [1, 2]);
        assert_eq!(
            page.sections[1],
            file("plan.md", line("ready for review", CheckTone::Ready), &[0])
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

    /// `stages`' files are there.
    fn wrote(stages: &[Stage]) -> RowCtx {
        RowCtx {
            written: Stage::ALL.map(|s| stages.contains(&s)),
            ..RowCtx::default()
        }
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
            )
        };
        let enter = ("Enter".to_owned(), "go to the current stage".to_owned());
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
    fn wrote_all() -> RowCtx {
        RowCtx {
            written: [true; Stage::ALL.len()],
            ..RowCtx::default()
        }
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
            Some(CheckTone::Blocker)
        );
        let PageSection::Lines { label, lines } = &page.sections[0] else {
            panic!("the checks' lines");
        };
        assert_eq!(label, "Checks");
        assert_eq!(
            texts(lines),
            [
                ("run 2 failed", CheckTone::Blocker),
                ("api: `npm test` exited with 1", CheckTone::Blocker),
                ("web: lint", CheckTone::Blocker),
            ]
        );
        let node = rail_node(&f, Stage::Verify, &state, 2_000);
        assert_eq!(
            (node.mark, node.line.as_str()),
            (NodeMark::Waiting, "failed")
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

    fn planned(command: &str, skipped: bool, own: bool) -> PlannedCheck {
        PlannedCheck {
            repo: "api".into(),
            command: command.into(),
            skipped,
            own,
        }
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
        let actions = stage_actions(f, Stage::Verify, state.ctx, &state.plan, &[]);
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
        let actions = stage_actions(&early, Stage::Verify, state.ctx, &state.plan, &[]);
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
        assert!(skip_action(&f, Stage::Verify, &entries[0]).is_none());
        assert!(check_entries(&f, Stage::Build, &plan).is_empty());
    }

    #[test]
    fn a_skipped_check_offers_only_to_stop_skipping_it() {
        let f = verified(false);
        let plan = vec![planned("lint", true, false), planned("test", false, true)];
        let (heading, cards) = verify_page(&f, plan.clone());
        assert_eq!(heading, "Checks \u{b7} 1 failed \u{b7} 1 skipped");
        let actions = stage_actions(&f, Stage::Verify, RowCtx::default(), &plan, &[]);
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
            panel.key(&mods, sym, f, &slugs, RowCtx::default(), &plan)
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
    fn a_blocked_ship_says_what_stops_it() {
        let f = feature("s", vec![repo("api", true)], 8);
        let p = probe(check(&["api has uncommitted changes"], Vec::new(), &[]));
        let state = PanelState {
            probe: Some(&p),
            ..PanelState::default()
        };
        let actions = page_actions(&f, Stage::Ship, RowCtx::default());
        assert_eq!(labels(&actions), [("Ship\u{2026}", PanelOutcome::Ship)]);
        let page = stage_page(&f, Stage::Ship, &state, &actions, 2_000);
        assert_eq!(
            page.sections,
            [lines(
                "Ship check",
                vec![line("api has uncommitted changes", CheckTone::Blocker)]
            )]
        );
        let node = rail_node(&f, Stage::Ship, &state, 2_000);
        assert_eq!(
            (node.mark, node.line.as_str()),
            (NodeMark::Current, "blocked")
        );
        let ready = probe(check(&[], Vec::new(), &[]));
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
            format!("Build \u{b7} {}", projects::ago(now, now - 3 * 86_400)),
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
        let status = |back| card_status(&f, &state, false, false, back, 0);
        let runs = status(Some(Rework::Runs(Dot::Working)));
        assert_eq!(runs.0, Some(Stage::Build));
        assert_eq!(runs.1, "Build \u{b7} running again");
        assert_eq!(runs.2, Working);
        let waits = status(Some(Rework::Waits));
        assert_eq!(waits.1, "Build \u{b7} waiting to send");
        assert_eq!(waits.2, Waits);
        assert_eq!(status(None).0, f.current());
    }

    #[test]
    fn a_cards_tone_puts_a_failure_over_a_gate_over_work() {
        use chrome::CardTone::{Failed, Idle, Waits, Working};
        let state = LineState::from(&PanelState::default());
        let f = feature("d", vec![repo("api", true)], 2);
        let tone = |gate, busy| card_tone(&f, Some(Stage::Build), &state, gate, busy);
        assert_eq!(tone(false, false), Idle);
        assert_eq!(tone(false, true), Working);
        assert_eq!(tone(true, true), Waits);
        let mut verify = feature("d", vec![repo("api", true)], 3);
        verify.verify = Some(feature::verify::VerifyRun {
            passed: Some(false),
            ..Default::default()
        });
        assert_eq!(
            card_tone(&verify, Some(Stage::Verify), &state, true, true),
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
                ("touched web", "6m ago", CheckTone::Note),
                (
                    "Verify run 1 failed: api: npm test",
                    "7m ago",
                    CheckTone::Blocker
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
            ("Verify run 3 failed: api: npm test", CheckTone::Blocker),
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
                CheckTone::Blocker,
            ),
            (
                "Build agent couldn't start: no coding agent CLI installed",
                CheckTone::Blocker,
            ),
        ] {
            assert_eq!(event_tone(what), tone, "{what}");
        }
    }

    /// A file card as `stage_page` makes it.
    fn file(name: &str, state: CheckLine, buttons: &[usize]) -> PageSection {
        PageSection::File {
            name: name.to_owned(),
            state,
            buttons: buttons.to_vec(),
        }
    }

    #[test]
    fn a_finished_features_tab_shows_no_code_error_nor_stale_marks() {
        let mut f = feature("d", vec![repo("api", true)], 9);
        f.status = Status::Done;
        let p = Probe {
            stats: BTreeMap::from([("api".to_owned(), (4, 1))]),
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
        // The primary Review stays on the file's card, as the plain one does.
        let held = actions
            .iter()
            .position(|(_, o)| *o == PanelOutcome::OpenDoc(Stage::Plan))
            .unwrap();
        assert_eq!(actions[held].0.style, ButtonStyle::Primary);
        let page = stage_page(&f, Stage::Plan, &PanelState::default(), &actions, 2_000);
        let card = page
            .sections
            .iter()
            .find(|s| matches!(s, PageSection::File { .. }));
        assert_eq!(
            card,
            Some(&file(
                "plan.md",
                line("ready for review", CheckTone::Ready),
                &[held]
            ))
        );
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
                done: 2,
                total: 3,
                items
            })
        );
        let PageSection::Agent { tail: shown, .. } = &page.sections[0] else {
            panic!("the agent's card first");
        };
        assert_eq!(shown, &tail);
        // Another stage's card has no tail: it is the current agent's.
        let plan = stage_page(&f, Stage::Plan, &state, &[], 2_000);
        assert!(!plan.sections.iter().any(|s| matches!(
            s,
            PageSection::Agent { tail, .. } if !tail.is_empty()
        )));
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
        p.stats.insert(f.repos[0].key(), (12, 3));
        let state = PanelState {
            probe: Some(&p),
            ..PanelState::default()
        };
        let view = tab_view(&panel, &f, &slugs, &state, 2_060);
        let stats: Vec<_> = view.repos.iter().map(|r| r.stat).collect();
        // The one not read shows none, not a zero.
        assert_eq!(stats, [Some((12, 3)), None]);
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
        let press = |p: &mut FeaturePanel, f: &Feature| p.key(&mods, Keysym::B, f, &[], ctx, &[]);
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
                vec![line("the plan names no such table", CheckTone::Blocker)]
            ))
        );
        let node = rail_node(&f, Stage::Build, &state, 2_000);
        assert_eq!(node.line, "blocked");
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
        let press = |p: &mut FeaturePanel| p.key(&mods, Keysym::A, &f, &[], ctx, &[]);
        assert_eq!(
            press(&mut panel),
            Some(PanelOutcome::CycleAgent(Stage::Discover))
        );
        panel.pick(&f, Stage::Verify);
        assert_eq!(press(&mut panel), None);
    }

    #[test]
    fn ctrl_shift_i_asks_for_the_model_on_an_agent_stage_only() {
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
        let press = |p: &mut FeaturePanel| p.key(&mods, Keysym::I, &f, &[], ctx, &[]);
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
            let choices = AgentChoices { picks: &[], model };
            tab_hints(
                &panel,
                &f,
                Stage::Discover,
                &[],
                RowCtx::default(),
                choices,
                &[],
            )
        };
        let shown = |hints: &[(String, String)]| hints.iter().any(|(k, _)| k == "Ctrl+Shift+I");
        assert!(shown(&hints(true)));
        assert!(!shown(&hints(false)));
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
}
