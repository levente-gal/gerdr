//! Features in gerdr's window: the new feature dialog, the feature panel,
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
//! The feature panel: a feature's repositories and its eight stages. It is
//! the pinned first tab of the feature's workspace (see `feature_workspace`):
//! showing that tab shows the panel, and it can't be closed, moved or
//! renamed; closing the workspace takes it along. Up and
//! Down (Ctrl+J/K) move, Enter does the row's own thing (start, show or
//! review the current stage's agent, let Build go on after a milestone,
//! run Verify's checks, mark a stage without either done, ship it once
//! Ship is the current one, reopen a done one, touch a repository or open
//! a tab in it), Left/Right go to the previous or next feature, Ctrl+O
//! opens a tab in the feature's folder, Ctrl+Shift+O opens the pull
//! request of the repository under the cursor, or on Verify's row its
//! evidence, Ctrl+R starts the current stage's agent afresh, Ctrl+C copies
//! its path, Ctrl+N starts a new one, Del abandons it (or, once done,
//! removes its worktrees) after asking. Build's row says what
//! setup does while it runs, and Verify's what its checks do, or what
//! failed.
//!
//! The marks from Build on are bound to the code (see `feature::patch`):
//! marking one reads the feature's patch-id off the thread first, and the
//! patch watch probes the feature shown every few seconds, and every open
//! feature with a bound mark or a squash under way every half minute (see
//! `feature::ship::probe`). Code changed since sends it back to Build, a
//! squash merged makes it done.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
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

use crate::{
    Gerdr,
    chrome::{
        self, CheckLine, CheckTone, Dot, FeatureHit, FeaturePanelView, FeatureRepoRow,
        MAX_CHECK_LINES, NewFeatureHit, NewFeatureRepo, NewFeatureView, StageMark, StageRow,
    },
    feature::{
        self, Event, Feature, Pr, Stage, Status,
        agents::needs,
        hook::Guard,
        layout::{self, Created, LinkSpec, Root},
        patch::{self, Reading},
        review_import::{CodeRead, FindingState},
        ship::{self, Action, Probe},
        store,
    },
    home_dir, paths, projects,
    tasks::Task,
    text_field::{Edit, TextField},
};

/// The features of every project, and the dialog and panel on them.
#[derive(Default)]
pub struct Features {
    /// By project id, oldest first.
    pub all: BTreeMap<String, Vec<Feature>>,
    pub dialog: Option<NewFeatureDialog>,
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
    /// What the Ship sheet open or last open asked to do, by project and
    /// slug: what shipping does once confirmed.
    asked: BTreeMap<(String, String), Asked>,
    /// Whether the patch watch's timer runs.
    watching: bool,
    /// The serial of the last dialog opened: a root found for an older one
    /// is dropped.
    serial: u64,
    /// A feature of another project to show in the panel once that
    /// project is entered, by project and slug (see `open_linked_feature`).
    pending: Option<(String, String)>,
    done: Option<channel::Sender<Done>>,
    /// The stage agents being watched (see `feature_agents`).
    pub(crate) agents: BTreeMap<crate::feature_agents::WatchKey, crate::feature_agents::Watch>,
    /// What Build and Verify keep while gerdr runs (see `feature_build`).
    #[allow(dead_code)] // read by `feature_build` (inc 4 contract)
    pub(crate) build: crate::feature_build::BuildState,
    /// The manual test sheet, while it is open (see `manual_test`).
    pub(crate) manual: Option<crate::manual_test::ManualSheet>,
    /// The acceptance criteria of each open feature's brief, by project
    /// and slug, as last read: none kept for a brief without.
    pub(crate) criteria: BTreeMap<(String, String), Vec<feature::manual::Criterion>>,
}

/// How often the patch watch looks: the feature shown, and one whose
/// squash is under way, are probed this often, the others every
/// `WATCH_EVERY`.
const PANEL_EVERY: Duration = Duration::from_secs(5);
const WATCH_EVERY: Duration = Duration::from_secs(30);

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
    /// Unlinking projects from an open feature: what they lose first,
    /// then their worktrees.
    Unlink,
    /// Reading the patch-id to mark a stage done.
    Read,
    Ship,
    /// Setting up its worktrees (see `feature::setup`).
    Setup,
    /// Running Verify's checks (see `feature::verify`).
    Verify,
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
    Found {
        serial: u64,
        result: Result<Root, String>,
    },
    /// The root of `project`, another project the dialog can link.
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
    /// Projects linked to open feature `slug`, or not.
    Linked {
        project: String,
        slug: String,
        names: Vec<String>,
        result: Result<Vec<(feature::Link, Created)>, String>,
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
    /// What unlinking `projects` from feature `slug` would lose.
    UnlinkLosses {
        project: String,
        slug: String,
        projects: Vec<String>,
        losses: Vec<String>,
    },
    /// Linked projects unlinked from feature `slug`, or not: per project id.
    Unlinked {
        project: String,
        slug: String,
        results: Vec<(String, Result<(), String>)>,
    },
    Probed {
        project: String,
        slug: String,
        probe: Probe,
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
        result: Result<(), String>,
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

/// Another project of the store the new feature dialog can link: the
/// feature then spans its repositories too.
#[derive(Debug)]
pub struct Other {
    pub project: String,
    pub name: String,
    /// Its root, found like the home one's.
    pub root: Finding,
    pub linked: bool,
    /// Linked already (linking to an open feature): unticked, it is
    /// unlinked.
    pub kept: bool,
    /// Of a project linked already, the repositories the feature changes:
    /// while it does, the project stays linked.
    pub holds: Vec<String>,
    /// Per repository of its root: whether the feature changes it.
    pub touched: Vec<bool>,
}

/// The new feature dialog's state, while it is open. With `existing`,
/// it links projects to that open feature instead.
#[derive(Debug)]
pub struct NewFeatureDialog {
    pub project: String,
    pub serial: u64,
    pub title: TextField,
    pub root: Finding,
    /// Whether the feature changes each repository of the root.
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
    /// The slug of the open feature it links projects to.
    pub existing: Option<String>,
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
    /// Create a feature with this title in the root found, changing these
    /// repositories, spanning these linked projects too.
    Create {
        title: String,
        root: Root,
        touched: Vec<String>,
        links: Vec<LinkSpec>,
    },
    /// Link these projects to open feature `slug`, their repositories
    /// untouched, and unlink these (by project id).
    Link {
        slug: String,
        links: Vec<LinkSpec>,
        unlinks: Vec<String>,
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
            existing: None,
        }
    }

    /// The root was found, or not: over several repositories the one it
    /// was opened from is ticked.
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

    /// Project `project`'s root was found, or not.
    fn found_other(&mut self, project: &str, result: Result<Root, String>) {
        let Some(other) = self.others.iter_mut().find(|o| o.project == project) else {
            return;
        };
        other.root = match result {
            Ok(root) if root.repos.is_empty() => Finding::Failed("no repositories in it".into()),
            Ok(root) => {
                other.touched = vec![false; root.repos.len()];
                Finding::Found(root)
            }
            Err(why) => Finding::Failed(why),
        };
    }

    /// Its rows: the projects while picking; else the repositories, the
    /// linked projects' after the home root's (none when linking to an
    /// open feature: those start untouched).
    fn rows(&self) -> Vec<DialogRow> {
        if self.picking || self.existing.is_some() {
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
            || self.existing.is_some()
            || matches!(&self.root, Finding::Found(root) if root.multi)
            || self.others.iter().any(|o| o.linked)
    }

    fn repos(&self) -> usize {
        self.rows().len()
    }

    /// Whether row `row` can't be ticked: the only home repository, a
    /// project kept or not found.
    fn fixed(&self, row: DialogRow) -> bool {
        match row {
            DialogRow::Home(_) => !matches!(&self.root, Finding::Found(root) if root.multi),
            DialogRow::Linked(..) => false,
            DialogRow::Project(k) => {
                let o = &self.others[k];
                if o.kept {
                    !o.holds.is_empty()
                } else {
                    !matches!(o.root, Finding::Found(_))
                }
            }
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

    /// The projects ticked and found, not linked already.
    fn links(&self) -> Vec<LinkSpec> {
        self.others
            .iter()
            .filter(|o| o.linked && !o.kept)
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

    /// The projects linked already and unticked: to unlink, by id.
    fn unlinks(&self) -> Vec<String> {
        (self.others.iter())
            .filter(|o| o.kept && !o.linked && o.holds.is_empty())
            .map(|o| o.project.clone())
            .collect()
    }

    /// What creating it would do, or why it can't, as a sentence.
    fn create(&self) -> Result<DialogOutcome, String> {
        if let Some(slug) = &self.existing {
            let (links, unlinks) = (self.links(), self.unlinks());
            if links.is_empty() && unlinks.is_empty() {
                return Err("Tick a project to link, or untick one to unlink.".into());
            }
            return Ok(DialogOutcome::Link {
                slug: slug.clone(),
                links,
                unlinks,
            });
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
        if self.existing.is_some() {
            return;
        }
        self.picking = !self.picking;
        // The picker's rows have the keyboard, found yet or not.
        self.focus = self.picking.then_some(0);
        self.top = 0;
        self.error = None;
    }

    /// Move the keyboard between the title and the rows; linking to an
    /// open feature, between the rows only.
    fn step(&mut self, back: bool) {
        let n = if self.multi() { self.repos() } else { 0 };
        if self.existing.is_some() {
            if n > 0 {
                let at = self.focus.unwrap_or(0);
                self.focus = Some(if back { (at + n - 1) % n } else { (at + 1) % n });
            }
            return;
        }
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
                if self.existing.is_some() {
                    return Some(DialogOutcome::Cancel);
                } else if self.picking {
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
            _ if self.focus.is_none()
                && self.existing.is_none()
                && self.title.key(mods, sym, typed) == Edit::Changed =>
            {
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
    /// The row with the keyboard: the repositories', then the stages'.
    pub cursor: usize,
    /// The first repository shown, when not all fit.
    pub repo_top: usize,
    pub hover: Option<FeatureHit>,
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
    /// Show this agent stage's agent.
    ShowAgent(Stage),
    /// Review this agent stage's file.
    ReviewDoc(Stage),
    /// Let this agent stage's agent go on after a milestone stop.
    Continue(Stage),
    /// Run Verify's checks (again).
    RunChecks,
    /// Show the diff at AI review's first finding to decide.
    Findings,
    /// Send AI review's findings to fix to the Build agent.
    SendFixes,
    /// Review the feature's diff (Code review).
    CodeReview,
    /// Open the manual test sheet (Manual test, with criteria).
    ManualTest,
    /// Show Verify's `evidence.md`.
    Evidence,
    /// Open this pull request's page.
    OpenPr(String),
    /// Clear this stage's mark and the later ones'.
    Untick(Stage),
    /// A later stage can't be marked yet: this one comes first.
    NotYet(Stage),
    /// Make this repository one the feature changes.
    Touch(String),
    /// Open a tab in this folder: the feature's, or a repository's
    /// worktree.
    Open(PathBuf),
    CopyPath,
    /// Abandon it, or once done remove its worktrees, after asking.
    Remove,
    New,
    /// Link another project to it, before Ship: it spans that one's
    /// repositories too.
    LinkProject,
    /// Show the feature with this slug.
    Show(String),
}

/// A row of the feature panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelRow {
    Repo(usize),
    Stage(Stage),
}

/// The rows of the feature panel for `feature`: its repositories, then
/// its stages.
pub fn panel_rows(feature: &Feature) -> Vec<PanelRow> {
    (0..feature.repos.len())
        .map(PanelRow::Repo)
        .chain(Stage::ALL.into_iter().map(PanelRow::Stage))
        .collect()
}

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
    /// is marked from its sheet.
    pub criteria: usize,
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
        PanelRow::Stage(stage) => Some(match stage_mark(feature, stage) {
            StageMark::Done => PanelOutcome::Untick(stage),
            StageMark::Current if stage == Stage::Ship => PanelOutcome::Ship,
            StageMark::Current if stage == Stage::Verify && ctx.checks => PanelOutcome::RunChecks,
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

/// What an agent stage's row does: start its agent, show it while its
/// tab is there, review its file once ready. Build has no file to review:
/// stopped after a milestone, it is let go on.
fn agent_action(feature: &Feature, stage: Stage) -> PanelOutcome {
    match feature.runs.get(&stage) {
        Some(run) if run.stop.is_some() => PanelOutcome::Continue(stage),
        Some(run) if run.ready.is_some() && stage != Stage::Build => PanelOutcome::ReviewDoc(stage),
        Some(run) if run.pane.is_some() => PanelOutcome::ShowAgent(stage),
        _ => PanelOutcome::StartAgent(stage, false),
    }
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
        _ if run.pane.is_some() => PanelOutcome::ShowAgent(stage),
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
        format!(
            "{} to decide \u{b7} Enter to review",
            plural(open, "finding")
        )
    } else if fix > 0 {
        format!(
            "{} to fix \u{b7} Enter to send to Build",
            plural(fix, "finding")
        )
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
        PanelRow::Stage(Stage::Verify) => has_evidence(feature).then_some(PanelOutcome::Evidence),
        PanelRow::Stage(_) => None,
    }
}

/// What Enter does on `row`, in words for its hint.
fn row_hint(feature: &Feature, row: PanelRow, ctx: RowCtx) -> &'static str {
    match row_action(feature, row, ctx) {
        Some(PanelOutcome::StartAgent(..)) => "start agent",
        Some(PanelOutcome::ShowAgent(_)) => "show agent",
        Some(PanelOutcome::ReviewDoc(Stage::Discover)) => "review brief",
        Some(PanelOutcome::ReviewDoc(_)) => "review plan",
        Some(PanelOutcome::Continue(_)) => "continue",
        Some(PanelOutcome::RunChecks) => "run checks",
        Some(PanelOutcome::Findings) => "review findings",
        Some(PanelOutcome::SendFixes) => "send fixes",
        Some(PanelOutcome::CodeReview) => "review code",
        Some(PanelOutcome::ManualTest) => "test",
        Some(PanelOutcome::Tick) => "mark done",
        Some(PanelOutcome::Ship) => "ship\u{2026}",
        Some(PanelOutcome::Untick(_)) => "reopen",
        Some(PanelOutcome::Touch(_)) => "touch",
        Some(PanelOutcome::Open(_)) => "open tab",
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
        PanelOutcome::ShowAgent(_) => Some("Show agent"),
        PanelOutcome::ReviewDoc(Stage::Discover) => Some("Review brief"),
        PanelOutcome::ReviewDoc(_) => Some("Review plan"),
        PanelOutcome::Continue(_) => Some("Continue"),
        PanelOutcome::RunChecks => Some("Run checks"),
        PanelOutcome::Findings => Some("Review findings"),
        PanelOutcome::SendFixes => Some("Send fixes"),
        PanelOutcome::CodeReview => Some("Review code"),
        PanelOutcome::ManualTest => Some("Test\u{2026}"),
        _ => None,
    }
}

/// The pull request `feature` opened for repository `repo`.
fn pr_of<'a>(feature: &'a Feature, repo: &str) -> Option<&'a Pr> {
    feature.shipping.prs.iter().find(|pr| pr.repo == repo)
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
    /// Handle a key over `feature`, one of `slugs` listed.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        feature: &Feature,
        slugs: &[String],
        ctx: RowCtx,
    ) -> Option<PanelOutcome> {
        let rows = panel_rows(feature);
        let last = rows.len().saturating_sub(1);
        self.cursor = self.cursor.min(last);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        match sym {
            Keysym::Up | Keysym::KP_Up => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Down | Keysym::KP_Down => self.cursor = (self.cursor + 1).min(last),
            Keysym::k | Keysym::K if ctrl => self.cursor = self.cursor.saturating_sub(1),
            Keysym::j | Keysym::J if ctrl => self.cursor = (self.cursor + 1).min(last),
            Keysym::Home | Keysym::KP_Home => self.cursor = 0,
            Keysym::End | Keysym::KP_End => self.cursor = last,
            Keysym::Return | Keysym::KP_Enter => {
                return row_action(feature, rows[self.cursor], ctx);
            }
            Keysym::Left | Keysym::KP_Left | Keysym::Right | Keysym::KP_Right if plain => {
                let back = matches!(sym, Keysym::Left | Keysym::KP_Left);
                return switch(slugs, &self.slug, back).map(PanelOutcome::Show);
            }
            Keysym::o | Keysym::O if ctrl && mods.shift => {
                return row_page(feature, rows[self.cursor]);
            }
            Keysym::o | Keysym::O if ctrl => return Some(PanelOutcome::Open(feature.dir.clone())),
            Keysym::c | Keysym::C if ctrl => return Some(PanelOutcome::CopyPath),
            Keysym::r | Keysym::R if ctrl => {
                return fresh_stage(feature, rows[self.cursor])
                    .map(|stage| PanelOutcome::StartAgent(stage, true));
            }
            Keysym::n | Keysym::N if ctrl => return Some(PanelOutcome::New),
            Keysym::l | Keysym::L if ctrl && mods.shift && linkable(feature) => {
                return Some(PanelOutcome::LinkProject);
            }
            Keysym::Delete | Keysym::KP_Delete if plain => return Some(PanelOutcome::Remove),
            _ => {}
        }
        None
    }

    /// Scroll so the cursor's repository is among the `fit` shown, of
    /// `repos`.
    fn reveal(&mut self, repos: usize, fit: usize) {
        let fit = fit.max(1);
        if self.cursor < repos {
            if self.cursor < self.repo_top {
                self.repo_top = self.cursor;
            } else if self.cursor >= self.repo_top + fit {
                self.repo_top = self.cursor + 1 - fit;
            }
        }
        self.repo_top = self.repo_top.min(repos.saturating_sub(fit));
    }
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

/// The cursor a feature's panel starts at: its current stage, else its
/// last.
fn start_cursor(feature: &Feature) -> usize {
    let stage = feature.current().unwrap_or(Stage::Ship);
    let at = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
    feature.repos.len() + at
}

/// A folder as the chrome shows it, from the home folder as `~`.
fn tilde(path: &Path) -> String {
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
        self.manual_sheet_follows();
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
            Done::Found { serial, result } => {
                if let Err(why) = &result {
                    tracing::warn!("finding where a feature goes: {why}");
                }
                // Dropped when the dialog it was for closed: a new one
                // looks again.
                if let Some(dialog) = self.features.dialog.as_mut()
                    && dialog.serial == serial
                {
                    dialog.found(result);
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
            Done::UnlinkLosses {
                project,
                slug,
                projects,
                losses,
            } => {
                self.unbusy(&project, &slug, Job::Unlink);
                self.ask_unlink(project, slug, projects, losses);
            }
            Done::Unlinked {
                project,
                slug,
                results,
            } => {
                self.unbusy(&project, &slug, Job::Unlink);
                self.feature_unlinked(&project, &slug, results);
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
                result,
            } => {
                self.unbusy(&project, &slug, Job::Setup);
                self.setup_done(&project, &slug, result);
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
        }
    }

    /// `job` on feature `slug` of `project` is over.
    pub(crate) fn unbusy(&mut self, project: &str, slug: &str, job: Job) {
        self.features
            .busy
            .retain(|b| !(b.project == project && b.slug == slug && b.job == job));
    }

    /// Whether feature `slug` of `project` has work running off the thread.
    pub(crate) fn feature_busy(&self, project: &str, slug: &str) -> bool {
        self.features
            .busy
            .iter()
            .any(|b| b.project == project && b.slug == slug)
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

    /// What the panel's rows need of `feature` besides it.
    fn row_ctx(&self, feature: &Feature) -> RowCtx {
        let config = self.projects.workflows(feature, |w| w.verify.clone());
        let checks = !feature::verify::checks(feature, &config).is_empty();
        let criteria = self.criteria(&feature.project, &feature.slug).len();
        RowCtx { checks, criteria }
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

    /// Probe what is due.
    fn feature_watch_tick(&mut self) {
        self.manual_sheet_follows();
        let mut due = Vec::new();
        if let Some(panel) = &self.features.panel {
            due.push((panel.project.clone(), panel.slug.clone(), PANEL_EVERY));
        }
        // The sheet's results count on the code as it is now.
        if let Some(sheet) = &self.features.manual {
            due.push((sheet.project.clone(), sheet.slug.clone(), PANEL_EVERY));
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
        if feature.status != Status::Open {
            return;
        }
        // The brief may have changed: its criteria are read again.
        self.load_criteria(project, slug);
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
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::Probed {
            probe: ship::probe(&feature, &modes),
            project,
            slug,
        });
    }

    /// A probe of feature `slug` of `project` is back: its code changed
    /// sends it back, its squash merged makes it done.
    fn feature_probed(&mut self, project: &str, slug: &str, probe: Probe) {
        let key = (project.to_owned(), slug.to_owned());
        let probed = self.features.probes.entry(key).or_default();
        probed.running = false;
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

    /// The criteria of feature `slug` are as last read from its brief, on
    /// its code (`patch`'s): Manual test, passed on its sheet, is to do
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

    /// Open the new feature dialog for the active project, from the
    /// folder of workspace `at`, else the active one's.
    pub fn open_new_feature(&mut self, at: Option<usize>) {
        let Some(id) = self.project.clone() else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let Some(project) = self.projects.project(&id) else {
            return self.show_warning("Open a project to start a feature in".into());
        };
        let workspace_dirs: Vec<PathBuf> =
            project.workspaces.iter().map(|w| w.dir.clone()).collect();
        let ignore = project.workflow.ignore.clone();
        let bases: Vec<(String, String)> = project
            .workflow
            .base
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
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
        self.features.dialog = Some(NewFeatureDialog::new(id, serial, from.clone()));
        self.feature_work(None, move |_| Done::Found {
            serial,
            result: layout::find_root(&from, &workspace_dirs, &ignore, &bases),
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Open the dialog linking other projects to open feature `slug` of
    /// `project`, in place of its panel; its repositories there start
    /// untouched. Only before it ships.
    pub fn open_link_projects(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        if !linkable(feature) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} has shipped; it can't span another project now"
            ));
        }
        if self.feature_busy(project, slug) {
            return self.show_warning(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ));
        }
        let kept: Vec<String> = feature.links.iter().map(|l| l.project.clone()).collect();
        let holds: Vec<(String, Vec<String>)> = (kept.iter())
            .map(|id| {
                let touched = (feature.repos_of(Some(id)))
                    .filter(|r| r.touched)
                    .map(|r| r.name.clone());
                (id.clone(), touched.collect())
            })
            .collect();
        self.features.serial += 1;
        let mut dialog =
            NewFeatureDialog::new(project.to_owned(), self.features.serial, PathBuf::new());
        dialog.existing = Some(slug.to_owned());
        dialog.title.set(&title);
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.dialog = Some(dialog);
        self.find_others();
        if let Some(dialog) = self.features.dialog.as_mut() {
            for other in &mut dialog.others {
                other.kept = kept.contains(&other.project);
                other.linked = other.kept;
                if let Some((_, held)) = holds.iter().find(|(id, _)| *id == other.project) {
                    other.holds = held.clone();
                }
            }
            dialog.focus = (!dialog.others.is_empty()).then_some(0);
        }
        self.refresh_pointer_focus();
    }

    /// List the store's other projects in the dialog's picker, once, and
    /// find each one's root off the thread (checklist D3), as the home
    /// one's is: from its first workspace's folder.
    fn find_others(&mut self) {
        let Some(dialog) = self.features.dialog.as_ref() else {
            return;
        };
        if !(dialog.picking || dialog.existing.is_some()) || !dialog.others.is_empty() {
            return;
        }
        let (home, serial) = (dialog.project.clone(), dialog.serial);
        let mut others = Vec::new();
        let mut finds = Vec::new();
        for p in self.projects.projects.iter().filter(|p| p.id != home) {
            let dirs: Vec<PathBuf> = p.workspaces.iter().map(|w| w.dir.clone()).collect();
            let root = match dirs.first() {
                Some(from) => {
                    let bases: Vec<(String, String)> = (p.workflow.base.iter())
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    finds.push((
                        p.id.clone(),
                        from.clone(),
                        dirs.clone(),
                        p.workflow.ignore.clone(),
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
                kept: false,
                holds: Vec::new(),
                touched: Vec::new(),
            });
        }
        if let Some(dialog) = self.features.dialog.as_mut() {
            dialog.others = others;
        }
        for (project, from, dirs, ignore, bases) in finds {
            self.feature_work(None, move |_| Done::FoundOther {
                serial,
                project,
                result: layout::find_root(&from, &dirs, &ignore, &bases),
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
            DialogOutcome::Link {
                slug,
                links,
                unlinks,
            } => {
                // Refused before anything starts, so neither half runs alone.
                if !unlinks.is_empty() && !self.may_unlink(&dialog.project, &slug, &unlinks) {
                    return;
                }
                if !links.is_empty() {
                    self.link_projects(dialog.project.clone(), slug.clone(), links);
                }
                if !unlinks.is_empty() {
                    self.check_unlink(dialog.project, slug, unlinks);
                }
                return;
            }
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
            history: vec![Event {
                at: now,
                what: "created".into(),
            }],
            shipping: Default::default(),
            runs: Default::default(),
            setup: None,
            verify: None,
            review: None,
            viewed: Default::default(),
            manual: None,
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
    }

    /// Link `links` to open feature `slug` off the thread, shown in the
    /// tasks panel meanwhile.
    fn link_projects(&mut self, project: String, slug: String, links: Vec<LinkSpec>) {
        let Some(feature) = self.feature(&project, &slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let names: Vec<String> = links.iter().map(|l| l.name.clone()).collect();
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
            let result = layout::create_links(&slug, &links);
            Done::Linked {
                project,
                slug,
                names,
                result,
            }
        });
    }

    /// Projects `names` were linked to feature `slug`, or not: kept, and
    /// its panel shown again.
    fn feature_linked(
        &mut self,
        project: &str,
        slug: &str,
        names: &[String],
        result: Result<Vec<(feature::Link, Created)>, String>,
    ) {
        let made = match result {
            Ok(made) => made,
            Err(why) => {
                tracing::warn!("linking projects to feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't link {}: {why}", names.join(", ")));
            }
        };
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        for (link, created) in made {
            feature.log(now, format!("linked {}", link.project));
            feature.links.push(link);
            feature.repos.extend(created.repos);
        }
        let title = feature.title.clone();
        self.save_feature(project, slug);
        if self.project.as_deref() == Some(project) && self.features.dialog.is_none() {
            self.open_feature(project, slug);
        }
        self.show_success(format!(
            "\u{201c}{title}\u{201d} spans {} too",
            names.join(", ")
        ));
        self.probe_feature(project, slug);
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
                if dialog.existing.is_none() {
                    dialog.focus = None;
                }
                None
            }
            Some(NewFeatureHit::Link) => {
                if dialog.existing.is_none() {
                    dialog.pick();
                    picked = true;
                }
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
            dialog.hover = (layout.hit(dialog.top, pos))
                .filter(|h| *h != NewFeatureHit::Link || dialog.existing.is_none());
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
    /// tab of the feature's workspace, which opens if none is, the cursor
    /// on the feature's current stage. The other panels close: they would
    /// cover it.
    pub fn open_feature(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let cursor = start_cursor(feature);
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
        // It starts afresh, not as it was left.
        let key = (project.to_owned(), slug.to_owned());
        self.features.parked.remove(&key);
        let same = |p: &FeaturePanel| p.project == project && p.slug == slug;
        let shown = self.features.panel.as_ref().is_some_and(same);
        if !self.show_feature_tab(project, slug) {
            return;
        }
        if let Some(panel) = self.features.panel.as_mut().filter(|p| same(p)) {
            panel.cursor = cursor;
        }
        self.reveal_feature_row();
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
        let on_tab = self.tab().is_some_and(|t| t.pinned);
        let shown = (self.project.clone())
            .zip(on_tab.then(|| self.workspace_feature(w)).flatten())
            .map(|(project, f)| (project, f.slug.clone(), start_cursor(f)));
        let open = (self.features.panel.as_ref()).map(|p| (p.project.clone(), p.slug.clone()));
        if open == shown.as_ref().map(|(p, s, _)| (p.clone(), s.clone())) {
            return;
        }
        if let Some(panel) = self.features.panel.take() {
            let key = (panel.project.clone(), panel.slug.clone());
            self.features.parked.insert(key, panel);
        }
        if let Some((project, slug, cursor)) = shown {
            let key = (project.clone(), slug.clone());
            let panel = self.features.parked.remove(&key).unwrap_or(FeaturePanel {
                project: project.clone(),
                slug: slug.clone(),
                cursor,
                ..FeaturePanel::default()
            });
            self.features.panel = Some(panel);
            self.features.shown.insert(project.clone(), slug.clone());
            self.reveal_feature_row();
            self.probe_feature(&project, &slug);
        }
        self.refresh_pointer_focus();
    }

    /// Open the feature panel on feature `slug` of `project` with the
    /// cursor on `stage`'s row. AI review with findings to decide shows
    /// the diff at the first instead, and Code review the diff, as their
    /// rows' Enter does; Manual test with criteria opens its sheet over it.
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
        // Manual test with criteria opens its sheet over the panel.
        let testing = stage == Stage::ManualTest
            && self
                .feature(project, slug)
                .is_some_and(|f| f.status == Status::Open && f.current() == Some(stage))
            && !self.criteria(project, slug).is_empty();
        if testing {
            self.open_manual_test(project, slug);
        }
        let row = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
        let at = self.panel_feature().map(|f| f.repos.len() + row);
        if let Some((panel, at)) = self.features.panel.as_mut().zip(at) {
            panel.cursor = at;
            self.reveal_feature_row();
        }
    }

    /// The feature the panel is on.
    fn panel_feature(&self) -> Option<&Feature> {
        let panel = self.features.panel.as_ref()?;
        self.feature(&panel.project, &panel.slug)
    }

    /// Scroll the panel's repositories to the cursor.
    fn reveal_feature_row(&mut self) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(&view, self.content_rect().size);
        if let Some(panel) = self.features.panel.as_mut() {
            panel.reveal(view.repos.len(), fit);
        }
    }

    /// A key while the feature panel has the keyboard.
    pub fn feature_panel_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
    ) -> Option<PanelOutcome> {
        let panel = self.features.panel.as_ref()?;
        let slugs = self.listed_slugs(&panel.project);
        let feature = self.feature(&panel.project, &panel.slug).cloned()?;
        let ctx = self.row_ctx(&feature);
        let out = self
            .features
            .panel
            .as_mut()?
            .key(mods, sym, &feature, &slugs, ctx);
        self.reveal_feature_row();
        out
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
        let now = feature::now();
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
            PanelOutcome::StartAgent(stage, fresh) => {
                self.start_stage_agent(&project, &slug, stage, fresh)
            }
            PanelOutcome::ShowAgent(stage) => self.show_stage_agent(&project, &slug, stage),
            PanelOutcome::ReviewDoc(stage) => self.open_doc_review(&project, &slug, stage),
            PanelOutcome::Continue(stage) => self.continue_stage_agent(&project, &slug, stage),
            PanelOutcome::RunChecks => match self.feature(&project, &slug) {
                Some(f) if self.busy_with(&project, &slug, Job::Verify).is_some() => {
                    let text = format!("The checks of \u{201c}{}\u{201d} run already", f.title);
                    self.show_warning(text);
                }
                Some(_) => self.run_verify(&project, &slug),
                None => {}
            },
            PanelOutcome::Evidence => self.open_evidence(&project, &slug),
            PanelOutcome::Findings => {
                let first = self
                    .feature(&project, &slug)
                    .and_then(|f| Some(f.review.as_ref()?.first_open()?.id.clone()));
                self.open_feature_diff(&project, &slug, first.as_deref());
            }
            PanelOutcome::SendFixes => self.send_findings(&project, &slug),
            PanelOutcome::CodeReview => self.open_feature_diff(&project, &slug, None),
            PanelOutcome::ManualTest => self.open_manual_test(&project, &slug),
            PanelOutcome::OpenPr(url) => self.open_feature_page(&project, &slug, url),
            PanelOutcome::Untick(stage) => {
                let Some(feature) = self.feature_mut(&project, &slug) else {
                    return;
                };
                if feature.untick(stage, now) {
                    self.save_feature(&project, &slug);
                    self.show_success(format!("{} reopened", stage.label()));
                    self.probe_feature(&project, &slug);
                } else {
                    self.show_warning(format!("{} isn't done", stage.label()));
                }
            }
            PanelOutcome::NotYet(first) => {
                self.show_warning(format!("Mark {} done first", first.label()));
            }
            PanelOutcome::Touch(repo) => self.touch_repo(&project, &slug, repo),
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
            PanelOutcome::LinkProject => self.open_link_projects(&project, &slug),
            // In its own tab, its workspace opened if need be.
            PanelOutcome::Show(to) => self.open_feature(&project, &to),
        }
    }

    /// Show the tab of `stage`'s agent; one gone is forgotten, and Enter
    /// starts a new one.
    fn show_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let pane = self
            .feature(project, slug)
            .and_then(|f| f.runs.get(&stage)?.pane.clone());
        if let Some(pane) = pane.filter(|p| self.herdr_pane_place(p).is_some()) {
            return self.focus_agent(&crate::agents::AgentKey::Herdr(pane));
        }
        if let Some(run) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&stage))
        {
            run.pane = None;
            self.save_feature(project, slug);
        }
        self.show_warning(format!(
            "{}'s agent tab is gone; Enter starts a new one",
            stage.label()
        ));
    }

    /// Put the panel's cursor on the current stage of feature `slug`,
    /// when the panel shows it.
    pub(crate) fn feature_cursor_to_current_of(&mut self, project: &str, slug: &str) {
        let on_it = self
            .features
            .panel
            .as_ref()
            .is_some_and(|p| p.project == project && p.slug == slug);
        let Some(cursor) = self.panel_feature().map(start_cursor) else {
            return;
        };
        if let Some(panel) = self.features.panel.as_mut().filter(|_| on_it) {
            panel.cursor = cursor;
        }
    }

    /// What repository `key` of `feature` is called on screen.
    fn repo_label(&self, feature: &Feature, key: &str) -> String {
        match feature.repo(key) {
            Some(repo) => self.projects.names(feature).label(repo),
            None => key.to_owned(),
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

    /// Whether linked projects `projects` (ids) can be unlinked from
    /// feature `slug` now; else says why not.
    fn may_unlink(&mut self, project: &str, slug: &str, projects: &[String]) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            self.show_warning("That feature is gone".into());
            return false;
        };
        let title = feature.title.clone();
        let why = if self.feature_busy(project, slug) {
            Some(format!(
                "\u{201c}{title}\u{201d} is busy; try again once it is done"
            ))
        } else {
            let names = self.projects.names(feature);
            projects.iter().find_map(|id| {
                let name = names.links.get(id).cloned().unwrap_or_else(|| id.clone());
                if let Some(why) = unlink_refusal(feature, id) {
                    return Some(format!("Can't unlink {name}: {why}"));
                }
                let user = self.folder_in_use(&feature.link(id)?.dir)?;
                Some(format!(
                    "{name}'s folder of \u{201c}{title}\u{201d} has {user} in it; close it first"
                ))
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

    /// Look at what unlinking `projects` from feature `slug` loses, off
    /// the thread, then ask (see `ask_unlink`).
    fn check_unlink(&mut self, project: String, slug: String, projects: Vec<String>) {
        let Some(feature) = self.feature(&project, &slug).cloned() else {
            return;
        };
        let busy = Busy {
            project: project.clone(),
            slug: slug.clone(),
            job: Job::Unlink,
            repo: None,
            what: format!(
                "Checking \u{201c}{}\u{201d} for unsaved work",
                feature.title
            ),
        };
        self.feature_work(Some(busy), move |_| Done::UnlinkLosses {
            losses: (projects.iter())
                .flat_map(|id| layout::losses_of(&feature, id))
                .collect(),
            project,
            slug,
            projects,
        });
    }

    /// Unlink `projects` from feature `slug` at once when nothing is lost;
    /// else ask first, naming what is (checklist C2).
    fn ask_unlink(
        &mut self,
        project: String,
        slug: String,
        projects: Vec<String>,
        losses: Vec<String>,
    ) {
        if losses.is_empty() {
            return self.unlink_projects(&project, &slug, &projects, false);
        }
        let Some(feature) = self.feature(&project, &slug) else {
            return;
        };
        let names = self.projects.names(feature);
        let named: Vec<String> = (projects.iter())
            .map(|id| names.links.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect();
        let question = format!(
            "Unlink {} from \u{201c}{}\u{201d}?",
            named.join(", "),
            feature.title
        );
        let then = crate::confirm::Then::UnlinkProjects {
            project,
            slug,
            projects,
            force: true,
        };
        self.ask_remove_worktrees(question, unlink_body(&losses), "Unlink", then);
    }

    /// Unlink `projects` from feature `slug` off the thread: their
    /// worktrees and folders go (`force`: with what they lose).
    pub fn unlink_projects(&mut self, project: &str, slug: &str, projects: &[String], force: bool) {
        if !self.may_unlink(project, slug, projects) {
            return;
        }
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let names = self.projects.names(&feature);
        let named: Vec<String> = (projects.iter())
            .map(|id| names.links.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Unlink,
            repo: None,
            what: format!(
                "Unlinking {} from \u{201c}{}\u{201d}",
                named.join(", "),
                feature.title
            ),
        };
        let (project, slug, projects) = (project.to_owned(), slug.to_owned(), projects.to_vec());
        self.feature_work(Some(busy), move |_| Done::Unlinked {
            results: (projects.into_iter())
                .map(|id| {
                    let result = layout::unlink(&feature, &id, force);
                    (id, result)
                })
                .collect(),
            project,
            slug,
        });
    }

    /// Linked projects were unlinked from feature `slug`, or not: those
    /// unlinked leave it, kept.
    fn feature_unlinked(
        &mut self,
        project: &str,
        slug: &str,
        results: Vec<(String, Result<(), String>)>,
    ) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let names = self.projects.names(feature);
        let name = |id: &str| {
            names
                .links
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_owned())
        };
        let mut gone = Vec::new();
        let mut failed = Vec::new();
        for (id, result) in results {
            match result {
                Ok(()) => gone.push(id),
                Err(why) => {
                    tracing::warn!("unlinking {id} from feature {project}/{slug}: {why}");
                    failed.push(format!("{}: {why}", name(&id)));
                }
            }
        }
        let named: Vec<String> = gone.iter().map(|id| name(id)).collect();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let now = feature::now();
        for id in &gone {
            drop_link(feature, id);
            feature.log(now, format!("unlinked {id}"));
        }
        let title = feature.title.clone();
        if !gone.is_empty() {
            self.save_feature(project, slug);
            self.show_success(format!(
                "\u{201c}{title}\u{201d} no longer spans {}",
                named.join(", ")
            ));
        }
        if !failed.is_empty() {
            self.show_error(format!("Couldn't unlink {}", failed.join("; ")));
        }
        if self.project.as_deref() == Some(project)
            && self.features.dialog.is_none()
            && self.features.panel.is_none()
        {
            self.open_feature(project, slug);
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
        let (project, slug) = (project.to_owned(), slug.to_owned());
        // Its workspace closes first, its terminals gone before its
        // worktrees go.
        let (p, s) = (project.clone(), slug.clone());
        self.close_feature_workspace_then(&p, &s, move |gerdr| {
            gerdr.feature_work(Some(busy), move |_| Done::Removed {
                result: layout::remove(&feature, force),
                project,
                slug,
                abandon,
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

    /// A left click in the feature panel's tab: off the panel it does
    /// nothing.
    pub fn feature_panel_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let Some(feature) = self.panel_feature().cloned() else {
            return;
        };
        let layout = chrome::feature_panel_layout(&view, self.content_rect());
        let rows = panel_rows(&feature);
        let repos = feature.repos.len();
        let stage = |i: usize| PanelRow::Stage(Stage::ALL[i.min(Stage::ALL.len() - 1)]);
        let outcome = match layout.hit(view.repo_top, pos) {
            None | Some(FeatureHit::Panel) => None,
            Some(FeatureHit::Hint(i)) => {
                if let Some((keys, _)) = view.hints.get(i).cloned() {
                    self.press_hint(crate::input::Overlay::Features, &keys, pos);
                }
                None
            }
            Some(FeatureHit::Prev | FeatureHit::Next) => {
                let slugs = self.listed_slugs(&feature.project);
                let back = layout.hit(view.repo_top, pos) == Some(FeatureHit::Prev);
                switch(&slugs, &feature.slug, back).map(PanelOutcome::Show)
            }
            Some(FeatureHit::Repo(i) | FeatureHit::Stage(i)) => {
                let at = match layout.hit(view.repo_top, pos) {
                    Some(FeatureHit::Stage(_)) => repos + i,
                    _ => i,
                };
                if let Some(panel) = self.features.panel.as_mut() {
                    panel.cursor = at.min(rows.len().saturating_sub(1));
                }
                None
            }
            Some(FeatureHit::RepoButton(i)) => repo_button(&feature, i),
            Some(FeatureHit::StageButton(i)) => {
                row_action(&feature, stage(i), self.row_ctx(&feature))
            }
            Some(FeatureHit::Button(0)) => Some(PanelOutcome::Open(feature.dir.clone())),
            Some(FeatureHit::Button(1)) => Some(PanelOutcome::CopyPath),
            Some(FeatureHit::Button(2)) => Some(PanelOutcome::New),
            Some(FeatureHit::Button(_)) => Some(PanelOutcome::Remove),
        };
        if let Some(outcome) = outcome {
            self.apply_feature_panel(outcome);
        }
    }

    /// The pointer moved while the feature panel is open.
    pub fn feature_panel_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = self.feature_panel_view().and_then(|view| {
            chrome::feature_panel_layout(&view, self.content_rect()).hit(view.repo_top, pos)
        });
        if let Some(panel) = self.features.panel.as_mut() {
            panel.hover = hit;
        }
    }

    /// The wheel over the feature panel: its repositories `by` rows on.
    pub fn feature_panel_scroll(&mut self, by: isize) {
        let Some(view) = self.feature_panel_view() else {
            return;
        };
        let fit = chrome::feature_repos_shown(&view, self.content_rect().size);
        if let Some(panel) = self.features.panel.as_mut() {
            let most = view.repos.len().saturating_sub(fit);
            panel.repo_top = panel.repo_top.saturating_add_signed(by).min(most);
        }
    }

    /// The panel's repository rows shown, for the wheel's pitch.
    pub fn feature_repo_rows(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.content_rect())
                .repos
                .into_iter()
                .map(|(row, _)| row)
                .collect()
        })
    }

    /// What the pointer can press in the feature panel.
    pub fn feature_panel_controls(&self) -> Vec<chrome::Rect> {
        self.feature_panel_view().map_or_else(Vec::new, |view| {
            chrome::feature_panel_layout(&view, self.content_rect()).controls()
        })
    }

    pub fn feature_panel_view(&self) -> Option<FeaturePanelView> {
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
        let agent = feature.current().and_then(|stage| {
            needs(stage)?;
            let run = feature.runs.get(&stage)?;
            let status = run.pane.as_ref().and_then(|p| {
                self.agents
                    .iter()
                    .find(|a| a.pane_id == *p)
                    .map(|a| a.agent_status)
            });
            let watch_key = (panel.project.clone(), panel.slug.clone(), stage);
            let watch = self.features.agents.get(&watch_key);
            Some(crate::feature_agents::agent_line(
                run,
                stage,
                status,
                watch,
                feature::now(),
            ))
        });
        let state = PanelState {
            busy: &busy,
            names: self.projects.names(feature),
            probe,
            shipping,
            agent,
            ctx: self.row_ctx(feature),
            setup: doing(Job::Setup),
            verify: doing(Job::Verify),
            code_review: self.code_review_line(&panel.project, &panel.slug),
            manual: self
                .manual_counts(feature)
                .map(crate::manual_test::counts_text),
        };
        Some(panel_view(panel, feature, &slugs, &state, feature::now()))
    }

    /// The sidebar's rows for the shown project's open features.
    pub fn feature_rows(&self) -> Vec<chrome::FeatureRowView> {
        self.sidebar_listed()
            .into_iter()
            .map(|f| chrome::FeatureRowView {
                title: self.row_title(f),
                marks: Stage::ALL.iter().map(|s| stage_mark(f, *s)).collect(),
            })
            .collect()
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

    /// How many feature rows the sidebar shows.
    pub fn sidebar_feature_count(&self) -> usize {
        self.sidebar_listed().len()
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
        _ => "feature",
    };
    let detail = match title {
        Some(title) if !busy.what.contains(title) => {
            format!("\u{201c}{title}\u{201d} \u{b7} {project}")
        }
        _ => project.to_owned(),
    };
    Task {
        title: busy.what.clone(),
        chip: chip.into(),
        detail,
        dot: Dot::Working,
        actions: Vec::new(),
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

/// Why linked project `id` can't be unlinked from `feature`, if it
/// can't: the feature changes a repository of it.
fn unlink_refusal(feature: &Feature, id: &str) -> Option<String> {
    feature.link(id)?;
    let touched: Vec<&str> = (feature.repos_of(Some(id)))
        .filter(|r| r.touched)
        .map(|r| r.name.as_str())
        .collect();
    (!touched.is_empty()).then(|| format!("the feature changes {}", touched.join(", ")))
}

/// Drop linked project `id` and its repositories from `feature`.
fn drop_link(feature: &mut Feature, id: &str) {
    feature.links.retain(|l| l.project != id);
    feature.repos.retain(|r| r.project.as_deref() != Some(id));
}

/// What a confirm dialog about unlinking projects says of `losses`, what
/// goes with their worktrees.
fn unlink_body(losses: &[String]) -> String {
    format!(
        "Their worktrees and feature folders go, and with them {}",
        losses.join(", ")
    )
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

/// Whether another project can still be linked to `feature`: open, and
/// not shipped.
fn linkable(feature: &Feature) -> bool {
    feature.status == Status::Open && !feature.marks.contains_key(&Stage::Ship)
}

/// What the new feature dialog shows of `dialog`, in project `project`.
fn new_feature_view(dialog: &NewFeatureDialog, project: String) -> NewFeatureView {
    let linking = dialog.existing.is_some();
    let place = |base: &str, touched: bool, fixed: bool| {
        if fixed {
            format!("from {base} \u{b7} the only repository")
        } else if touched {
            format!("from {base} \u{b7} changed")
        } else {
            format!("from {base} \u{b7} context")
        }
    };
    let found = |root: &Root| {
        let n = root.repos.len();
        if root.multi {
            format!("In {} \u{b7} {n} repositories", tilde(&root.path))
        } else {
            format!("In {}", tilde(&root.path))
        }
    };
    let (root, root_failed) = if (dialog.picking || linking) && dialog.others.is_empty() {
        ("The store has no other project to link".to_owned(), true)
    } else if linking {
        (
            "Their repositories join it as context; tick them in the panel".to_owned(),
            false,
        )
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
                        _ if other.kept && !other.holds.is_empty() => {
                            format!("linked \u{b7} changes {}", other.holds.join(", "))
                        }
                        _ if other.kept && !other.linked => "unlinked once applied".to_owned(),
                        _ if other.kept => "linked already".to_owned(),
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
    if !linking {
        keys.push(if dialog.picking {
            ("Ctrl+Shift+L", "repositories")
        } else {
            ("Ctrl+Shift+L", "link project")
        });
    }
    // Esc steps back as `NewFeatureDialog::key` has it.
    keys.push(if linking {
        ("Esc", "close")
    } else if dialog.picking || dialog.focus.is_some() {
        ("Esc", "back")
    } else if !dialog.title.is_empty() {
        ("Esc", "clear")
    } else {
        ("Esc", "close")
    });
    let heading = if linking {
        format!("Link projects to \u{201c}{}\u{201d}", dialog.title.trim())
    } else {
        format!("New feature in \u{201c}{project}\u{201d}")
    };
    NewFeatureView {
        heading,
        title: dialog.title.line(),
        focus: dialog.focus,
        root,
        root_failed,
        projects,
        link: (!linking).then_some(chrome::LINK_BUTTON[usize::from(dialog.picking)]),
        repos,
        go: match (
            linking,
            dialog.links().is_empty(),
            dialog.unlinks().is_empty(),
        ) {
            (false, ..) => "Create",
            (true, _, true) => "Link",
            (true, true, false) => "Unlink",
            (true, false, false) => "Apply",
        },
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
    /// What its rows need besides it.
    ctx: RowCtx,
    /// What its setup does now, while it runs.
    setup: Option<&'a str>,
    /// What Verify's checks do now, while they run.
    verify: Option<&'a str>,
    /// What Code review's row says once the diff was read (see
    /// `code_review_line`).
    code_review: Option<String>,
    /// How far Manual test's criteria went, in words, when the brief has
    /// some (see `manual_test::counts_text`).
    manual: Option<String>,
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
        return Some("no checks set: Mark done by hand".into());
    }
    let run = feature
        .verify
        .as_ref()
        .filter(|v| v.passed == Some(false))?;
    let mut line = match run.failures.first() {
        Some(first) => format!("failed: {first}"),
        None => "failed".to_owned(),
    };
    let most = feature::verify::MAX_FIXES;
    if run.fixes >= most {
        line.push_str(" \u{b7} fix it, then Run checks");
    } else if run.fixes > 0 {
        line.push_str(&format!(
            " \u{b7} fix sent to the Build agent ({} of {most})",
            run.fixes
        ));
    }
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

/// What the feature panel shows of `feature`, one of `slugs`, as `state`
/// has it.
fn panel_view(
    panel: &FeaturePanel,
    feature: &Feature,
    slugs: &[String],
    state: &PanelState,
    now: u64,
) -> FeaturePanelView {
    let repos = feature
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| FeatureRepoRow {
            name: state.names.label(r),
            line: repo_line(feature, r, state.probe),
            touched: r.touched,
            busy: state.busy.contains(&r.key().as_str()),
            button: match repo_button(feature, i) {
                Some(PanelOutcome::Touch(_)) => Some("Touch"),
                Some(PanelOutcome::OpenPr(_)) => Some("Open PR"),
                _ => None,
            },
        })
        .collect();
    let stages = Stage::ALL
        .iter()
        .map(|s| {
            let mark = stage_mark(feature, *s);
            let agent = state
                .agent
                .as_deref()
                .filter(|_| mark == StageMark::Current);
            let line = match s {
                Stage::Build => build_line(feature, state.setup, agent),
                Stage::Verify => verify_line(feature, state.ctx, state.verify),
                Stage::AiReview if mark == StageMark::Current => review_line(feature, agent),
                Stage::CodeReview if mark == StageMark::Current => Some(
                    state
                        .code_review
                        .clone()
                        .unwrap_or_else(|| "review every changed file".into()),
                ),
                Stage::ManualTest if mark == StageMark::Current => state.manual.clone(),
                _ => agent.map(str::to_owned),
            };
            StageRow {
                label: s.label().to_owned(),
                detail: match (feature.marks.get(s), line) {
                    (Some(m), _) => mark_detail(m, now),
                    (None, Some(line)) => line,
                    (None, None) => s.about().to_owned(),
                },
                mark,
                button: stage_button(feature, *s, state.ctx),
            }
        })
        .collect();
    let rows = panel_rows(feature);
    let row = rows[panel.cursor.min(rows.len() - 1)];
    let hint = |k: &str, what: &str| (k.to_owned(), what.to_owned());
    let mut hints = vec![hint("Up / Down", "move")];
    let enter = row_hint(feature, row, state.ctx);
    if !enter.is_empty() {
        hints.push(hint("Enter", enter));
    }
    match row_page(feature, row) {
        Some(PanelOutcome::OpenPr(_)) => hints.push(hint("Ctrl+Shift+O", "open PR")),
        Some(PanelOutcome::Evidence) => hints.push(hint("Ctrl+Shift+O", "evidence")),
        _ => {}
    }
    if fresh_stage(feature, row).is_some() {
        hints.push(hint("Ctrl+R", "fresh agent"));
    }
    if slugs.len() > 1 {
        hints.push(hint("Left / Right", "other feature"));
    }
    hints.extend([
        hint("Ctrl+O", "open tab"),
        hint("Ctrl+C", "copy path"),
        hint("Ctrl+N", "new"),
    ]);
    if linkable(feature) {
        hints.push(hint("Ctrl+Shift+L", "link project"));
    }
    hints.push(hint("Del", &remove_label(feature).to_lowercase()));
    let which = slugs
        .iter()
        .position(|s| *s == feature.slug)
        .map(|i| (i, slugs.len()));
    let mut sub = format!("{} \u{b7} {}", feature.slug, tilde(&feature.dir));
    if feature.status == Status::Done {
        sub = format!("done \u{b7} {sub}");
    }
    FeaturePanelView {
        title: feature.title.clone(),
        sub,
        which,
        repos,
        stages,
        cursor: panel.cursor,
        repo_top: panel.repo_top,
        remove: remove_label(feature),
        hover: panel.hover,
        hints,
        check: check_lines(feature, state.probe, state.shipping, now),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::feature::{Mark, Repo, ship::ShipCheck};
    use layout::Found;

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
            setup: None,
            verify: None,
            review: None,
            viewed: Default::default(),
            manual: None,
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
        panel.key(&mods, sym, f, &slugs, RowCtx::default())
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
            kept: false,
            holds: Vec::new(),
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
        assert_eq!(view.link, Some(chrome::LINK_BUTTON[1]));
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
        d.found_other("admin", Ok(root(&["panel"])));
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
    fn linking_to_an_open_feature_picks_new_projects_only() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, PathBuf::new());
        d.existing = Some("bulk".into());
        d.title.set("Bulk export");
        d.others = vec![other("admin", "Admin"), other("docs", "Docs")];
        d.found_other("admin", Ok(root(&["panel"])));
        d.found_other("docs", Ok(root(&["site"])));
        d.others[0].kept = true;
        d.others[0].linked = true;
        d.others[0].holds = vec!["panel".into()];
        d.focus = Some(0);
        let view = new_feature_view(&d, "Acme".into());
        assert_eq!(view.heading, "Link projects to \u{201c}Bulk export\u{201d}");
        assert_eq!((view.link, view.go), (None, "Link"));
        assert_eq!(view.repos[0].place, "linked \u{b7} changes panel");
        assert!(view.repos[0].fixed, "it changes a repository there");
        dkey(&mut d, key(Keysym::space));
        assert!(d.others[0].linked);
        assert_eq!(
            dkey(&mut d, key(Keysym::Return)),
            None,
            "nothing new is ticked"
        );
        assert_eq!(
            d.error.as_deref(),
            Some("Tick a project to link, or untick one to unlink.")
        );
        dkey(&mut d, key(Keysym::Tab));
        assert_eq!(d.focus, Some(1));
        type_into(&mut d, "x");
        assert_eq!(&*d.title, "Bulk export", "its title isn't edited");
        dkey(&mut d, key(Keysym::space));
        let Some(DialogOutcome::Link {
            slug,
            links,
            unlinks,
        }) = dkey(&mut d, key(Keysym::Return))
        else {
            panic!("it links");
        };
        assert_eq!(slug, "bulk");
        assert!(unlinks.is_empty());
        let projects: Vec<&str> = links.iter().map(|l| l.project.as_str()).collect();
        assert_eq!(projects, ["docs"], "the kept one isn't linked again");
        assert!(links[0].touched.is_empty());
        assert_eq!(
            dkey(&mut d, key(Keysym::Escape)),
            Some(DialogOutcome::Cancel)
        );
    }

    #[test]
    fn a_linked_project_unticked_is_unlinked_unless_the_feature_changes_it() {
        let mut d = NewFeatureDialog::new("acme".into(), 1, PathBuf::new());
        d.existing = Some("bulk".into());
        d.title.set("Bulk export");
        d.others = vec![other("admin", "Admin"), other("docs", "Docs")];
        d.found_other("admin", Ok(root(&["panel"])));
        d.found_other("docs", Ok(root(&["site"])));
        for o in &mut d.others {
            (o.kept, o.linked) = (true, true);
        }
        d.others[0].holds = vec!["panel".into()];
        d.focus = Some(1);
        dkey(&mut d, key(Keysym::space));
        assert!(!d.others[1].linked);
        let view = new_feature_view(&d, "Acme".into());
        assert_eq!(view.repos[1].place, "unlinked once applied");
        assert_eq!(view.go, "Unlink");
        d.others.push(other("ops", "Ops"));
        d.found_other("ops", Ok(root(&["infra"])));
        d.toggle(2);
        assert_eq!(new_feature_view(&d, "Acme".into()).go, "Apply");
        let Ok(DialogOutcome::Link { links, unlinks, .. }) = d.create() else {
            panic!("it links and unlinks");
        };
        assert_eq!(links[0].project, "ops");
        assert_eq!(unlinks, ["docs"]);
    }

    #[test]
    fn unlinking_drops_the_project_and_its_repositories_only() {
        let linked = |name: &str, touched| feature::Repo {
            project: Some("admin".into()),
            ..repo(name, touched)
        };
        let mut f = feature("b", vec![repo("api", true), linked("panel", false)], 1);
        f.links = vec![feature::Link {
            project: "admin".into(),
            root: "/admin".into(),
            dir: "/admin/.features/b".into(),
            multi: false,
        }];
        assert_eq!(unlink_refusal(&f, "admin"), None);
        assert_eq!(
            unlink_refusal(&f, "docs"),
            None,
            "not linked: nothing to refuse"
        );
        f.repos[1].touched = true;
        assert_eq!(
            unlink_refusal(&f, "admin").as_deref(),
            Some("the feature changes panel")
        );
        drop_link(&mut f, "admin");
        assert!(f.links.is_empty());
        let keys: Vec<String> = f.repos.iter().map(|r| r.key()).collect();
        assert_eq!(keys, ["api"]);
        assert_eq!(
            unlink_body(&["admin:panel: 2 changed files".into()]),
            "Their worktrees and feature folders go, and with them admin:panel: 2 changed files"
        );
    }

    #[test]
    fn panel_keys_ask_for_the_rows_and_the_features_actions() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 2);
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        // It starts on the current stage: Plan done, Build next, its
        // agent to start.
        assert_eq!(panel.cursor, 2 + 2);
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::StartAgent(Stage::Build, false))
        );
        press(&mut panel, &f, key(Keysym::Up));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Untick(Stage::Plan))
        );
        press(&mut panel, &f, ctrl(Keysym::j));
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::NotYet(Stage::Build))
        );
        press(&mut panel, &f, key(Keysym::Home));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Open("/work/.features/bulk/api".into()))
        );
        press(&mut panel, &f, key(Keysym::Down));
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Return)),
            Some(PanelOutcome::Touch("web".into()))
        );
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
        assert_eq!(
            press(&mut panel, &f, ctrl_shift(Keysym::L)),
            Some(PanelOutcome::LinkProject)
        );
        let mut shipped = f.clone();
        shipped
            .marks
            .insert(Stage::Ship, f.marks.values().next().unwrap().clone());
        assert_eq!(
            press(&mut panel, &shipped, ctrl_shift(Keysym::L)),
            None,
            "a shipped feature spans no more projects"
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Delete)),
            Some(PanelOutcome::Remove)
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Left)),
            Some(PanelOutcome::Show("a".into()))
        );
        assert_eq!(
            press(&mut panel, &f, key(Keysym::Right)),
            Some(PanelOutcome::Show("c".into()))
        );
        // Esc leaves it be: it is a tab.
        assert_eq!(press(&mut panel, &f, key(Keysym::Escape)), None);
        // Ctrl+N and Ctrl+P never move.
        let at = panel.cursor;
        press(&mut panel, &f, ctrl(Keysym::p));
        assert_eq!(panel.cursor, at);
    }

    #[test]
    fn the_panel_shows_repositories_then_stages_and_the_right_hints() {
        let f = feature("b", vec![repo("api", true), repo("web", false)], 3);
        let panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["b".to_owned()];
        let state = PanelState {
            busy: &["web"],
            ..PanelState::default()
        };
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(view.repos[0].line, "feature/bulk");
        assert_eq!(view.repos[1].line, "context, at main");
        assert!(view.repos[1].busy);
        let marks: Vec<StageMark> = view.stages.iter().map(|s| s.mark).collect();
        assert_eq!(
            &marks[..4],
            [
                StageMark::Done,
                StageMark::Done,
                StageMark::Done,
                StageMark::Current
            ]
        );
        assert_eq!(view.stages[0].detail, "done 1m ago by you");
        assert_eq!(view.stages[3].detail, "no checks set: Mark done by hand");
        assert_eq!(view.which, Some((0, 1)));
        let keys: Vec<&str> = view.hints.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "Up / Down",
                "Enter",
                "Ctrl+O",
                "Ctrl+C",
                "Ctrl+N",
                "Ctrl+Shift+L",
                "Del",
            ]
        );
        assert_eq!(view.hints[1].1, "mark done");
        assert_eq!(view.remove, "Abandon");
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

    #[test]
    fn agent_stage_rows_start_show_and_review_their_agent() {
        let mut f = feature("b", vec![repo("api", true)], 1);
        let mut panel = FeaturePanel {
            project: "acme".into(),
            slug: "b".into(),
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["b".to_owned()];
        let plan = PanelRow::Stage(Stage::Plan);
        let hints = |panel: &FeaturePanel, f: &Feature, state: &PanelState| {
            panel_view(panel, f, &slugs, state, 2_000)
                .hints
                .into_iter()
                .take(3)
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
                pair("Up / Down", "move"),
                pair("Enter", "start agent"),
                pair("Ctrl+O", "open tab")
            ]
        );
        assert_eq!(
            panel_view(&panel, &f, &slugs, &state, 2_000).stages[1].detail,
            Stage::Plan.about()
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
                pair("Up / Down", "move"),
                pair("Enter", "show agent"),
                pair("Ctrl+R", "fresh agent")
            ]
        );
        let view = panel_view(&panel, &f, &slugs, &state, 2_000);
        assert_eq!(
            view.stages[1].detail,
            "Claude Code working \u{b7} started 3m ago"
        );
        // Only the current stage's row says it.
        assert_eq!(view.stages[2].detail, Stage::Build.about());
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
            Some(PanelOutcome::ReviewDoc(Stage::Plan))
        );
        assert_eq!(
            stage_button(&f, Stage::Plan, RowCtx::default()),
            Some("Review plan")
        );
        assert_eq!(hints(&panel, &f, &state)[1], pair("Enter", "review plan"));
        // Ctrl+R only on the agent stage's own row.
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
        let mut f = feature("b", vec![repo("api", true)], 2);
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
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        assert_eq!(
            press(&mut panel, &f, ctrl(Keysym::r)),
            Some(PanelOutcome::StartAgent(Stage::Build, true))
        );
    }

    #[test]
    fn verify_runs_its_checks_when_it_has_some_else_is_marked() {
        let f = feature("v", vec![repo("api", true)], 3);
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
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["v".to_owned()];
        let (mods, sym) = key(Keysym::Return);
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, checks),
            Some(PanelOutcome::RunChecks)
        );
        let none = RowCtx::default();
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, none),
            Some(PanelOutcome::Tick)
        );
        // Later stages aren't run early.
        let early = feature("e", vec![repo("api", true)], 2);
        assert_eq!(
            row_action(&early, verify, checks),
            Some(PanelOutcome::NotYet(Stage::Build))
        );
    }

    #[test]
    fn evidence_opens_from_the_verify_row_once_a_run_wrote_it() {
        let mut f = feature("v", vec![repo("api", true)], 3);
        let mut panel = FeaturePanel {
            cursor: start_cursor(&f),
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
            panel_view(panel, f, &slugs, &PanelState::default(), 2_000)
                .hints
                .contains(&("Ctrl+Shift+O".into(), "evidence".into()))
        };
        // No run yet: nothing to open, no hint.
        assert_eq!(panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none), None);
        assert!(!hinted(&panel, &f));
        // The first run, still running: nothing written yet.
        f.verify = Some(feature::verify::VerifyRun {
            n: 1,
            started: 1_000,
            ..Default::default()
        });
        assert_eq!(panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none), None);
        // Ended: there.
        f.verify.as_mut().unwrap().finished = Some(1_100);
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none),
            Some(PanelOutcome::Evidence)
        );
        assert!(hinted(&panel, &f));
        // Another row: not offered.
        panel.cursor -= 1;
        assert_eq!(panel.key(&shift_ctrl, Keysym::O, &f, &slugs, none), None);
        assert!(!hinted(&panel, &f));
    }

    #[test]
    fn builds_row_says_setup_first_then_its_agent() {
        let mut f = feature("b", vec![repo("api", true)], 2);
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
        // In the view, on Build's row only; a done Build says when.
        let panel = FeaturePanel {
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["b".to_owned()];
        let view = panel_view(&panel, &f, &slugs, &PanelState::default(), 2_000);
        assert_eq!(
            view.stages[2].detail,
            "setup failed: api: `npm ci` exited with 1"
        );
        let done = feature("d", vec![repo("api", true)], 3);
        let view = panel_view(&panel, &done, &slugs, &PanelState::default(), 2_060);
        assert_eq!(view.stages[2].detail, "done 1m ago by you");
    }

    #[test]
    fn verifys_row_says_what_runs_failed_or_that_it_has_no_checks() {
        let mut f = feature("v", vec![repo("api", true)], 3);
        let checks = RowCtx {
            checks: true,
            ..RowCtx::default()
        };
        let none = RowCtx::default();
        assert_eq!(
            verify_line(&f, none, None).as_deref(),
            Some("no checks set: Mark done by hand")
        );
        assert_eq!(verify_line(&f, checks, None), None);
        assert_eq!(
            verify_line(&f, checks, Some("api: npm test (2 of 5)")).as_deref(),
            Some("api: npm test (2 of 5)")
        );
        let failed = |fixes: u32| feature::verify::VerifyRun {
            n: 2,
            started: 1_000,
            finished: Some(1_100),
            passed: Some(false),
            failures: vec!["api: `npm test` exited with 1".into(), "web: lint".into()],
            fixes,
            ..Default::default()
        };
        f.verify = Some(failed(0));
        assert_eq!(
            verify_line(&f, checks, None).as_deref(),
            Some("failed: api: `npm test` exited with 1")
        );
        f.verify = Some(failed(1));
        assert_eq!(
            verify_line(&f, checks, None).as_deref(),
            Some(
                "failed: api: `npm test` exited with 1 \u{b7} fix sent to the Build agent (1 of 3)"
            )
        );
        f.verify = Some(failed(feature::verify::MAX_FIXES));
        assert_eq!(
            verify_line(&f, checks, None).as_deref(),
            Some("failed: api: `npm test` exited with 1 \u{b7} fix it, then Run checks")
        );
        // Passed: the stage's own words.
        f.verify = Some(feature::verify::VerifyRun {
            passed: Some(true),
            ..failed(0)
        });
        assert_eq!(verify_line(&f, checks, None), None);
        let panel = FeaturePanel {
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["v".to_owned()];
        let state = PanelState {
            ctx: checks,
            ..PanelState::default()
        };
        let view = panel_view(&panel, &f, &slugs, &state, 2_000);
        assert_eq!(view.stages[3].detail, Stage::Verify.about());
        assert_eq!(view.stages[3].button, Some("Run checks"));
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
            ship,
            merged: false,
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
        let mut f = feature("r", vec![repo("api", true)], 4);
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
        let fresh = feature("r", vec![repo("api", true)], 4);
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
        assert_eq!(row_hint(&open, row, none), "review findings");
        assert_eq!(
            stage_button(&open, Stage::AiReview, none),
            Some("Review findings")
        );
        assert_eq!(
            review_line(&open, None).as_deref(),
            Some("2 findings to decide \u{b7} Enter to review")
        );
        let fix = reviewing(&[S::Fix, S::Dismissed], true);
        assert_eq!(row_action(&fix, row, none), Some(PanelOutcome::SendFixes));
        assert_eq!(row_hint(&fix, row, none), "send fixes");
        assert_eq!(
            review_line(&fix, None).as_deref(),
            Some("1 finding to fix \u{b7} Enter to send to Build")
        );
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
            cursor: start_cursor(&open),
            ..FeaturePanel::default()
        };
        assert_eq!(
            press(&mut panel, &open, ctrl(Keysym::r)),
            Some(PanelOutcome::StartAgent(Stage::AiReview, true))
        );
        let slugs = vec!["r".to_owned()];
        let view = panel_view(&panel, &open, &slugs, &PanelState::default(), 2_060);
        let row = &view.stages[4];
        assert_eq!(row.detail, "2 findings to decide \u{b7} Enter to review");
        assert_eq!(row.button, Some("Review findings"));
        assert!(
            view.hints
                .contains(&("Ctrl+R".into(), "fresh agent".into()))
        );
        assert!(
            view.hints
                .contains(&("Enter".into(), "review findings".into()))
        );
    }

    #[test]
    fn code_review_shows_the_diff() {
        let f = feature("c", vec![repo("api", true)], 5);
        let row = PanelRow::Stage(Stage::CodeReview);
        let none = RowCtx::default();
        assert_eq!(row_action(&f, row, none), Some(PanelOutcome::CodeReview));
        assert_eq!(row_hint(&f, row, none), "review code");
        assert_eq!(
            stage_button(&f, Stage::CodeReview, none),
            Some("Review code")
        );
        let panel = FeaturePanel {
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["c".to_owned()];
        let view = panel_view(&panel, &f, &slugs, &PanelState::default(), 2_060);
        assert_eq!(view.stages[5].detail, "review every changed file");
        let state = PanelState {
            code_review: Some("3 of 7 files viewed".into()),
            ..PanelState::default()
        };
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(view.stages[5].detail, "3 of 7 files viewed");
        assert_eq!(view.stages[5].button, Some("Review code"));
    }

    fn texts(lines: &[CheckLine]) -> Vec<(&str, CheckTone)> {
        lines.iter().map(|l| (l.text.as_str(), l.tone)).collect()
    }

    #[test]
    fn manual_test_opens_its_sheet_when_the_brief_has_criteria() {
        let f = feature("m", vec![repo("api", true)], 6);
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
        assert_eq!(row_hint(&f, row, some), "test");
        assert_eq!(
            stage_button(&f, Stage::ManualTest, some),
            Some("Test\u{2026}")
        );
        let panel = FeaturePanel {
            cursor: start_cursor(&f),
            ..FeaturePanel::default()
        };
        let slugs = vec!["m".to_owned()];
        let state = PanelState {
            ctx: some,
            manual: Some("2 of 3 criteria passed \u{b7} 1 failed".into()),
            ..PanelState::default()
        };
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(
            view.stages[6].detail,
            "2 of 3 criteria passed \u{b7} 1 failed"
        );
        assert_eq!(view.stages[6].button, Some("Test\u{2026}"));
    }

    #[test]
    fn ship_is_asked_for_not_ticked_and_bound_stages_tick_as_before() {
        let shipping = feature("s", vec![repo("api", true)], 7);
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
            stage_button(&shipping, Stage::Build, RowCtx::default()),
            Some("Reopen")
        );
        let verifying = feature("v", vec![repo("api", true)], 3);
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
        let mut panel = FeaturePanel {
            cursor: 1 + Stage::ALL.len() - 1,
            ..FeaturePanel::default()
        };
        let slugs = vec!["s".to_owned()];
        let (mods, sym) = key(Keysym::Return);
        assert_eq!(
            panel.key(&mods, sym, &shipping, &slugs, RowCtx::default()),
            Some(PanelOutcome::Ship)
        );
    }

    #[test]
    fn a_repository_with_a_pull_request_opens_it_by_button_and_key() {
        let mut f = feature(
            "d",
            vec![repo("api", true), repo("web", true), repo("ui", false)],
            8,
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
        let mut panel = FeaturePanel::default();
        let shift_ctrl = ModifiersState {
            ctrl: true,
            shift: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, RowCtx::default()),
            Some(PanelOutcome::OpenPr(url.into()))
        );
        // Ctrl+O still opens a tab; on a row without a PR, nothing.
        let (mods, sym) = ctrl(Keysym::o);
        assert_eq!(
            panel.key(&mods, sym, &f, &slugs, RowCtx::default()),
            Some(PanelOutcome::Open(f.dir.clone()))
        );
        panel.cursor = 1;
        assert_eq!(
            panel.key(&shift_ctrl, Keysym::O, &f, &slugs, RowCtx::default()),
            None
        );
        // The view: the button, the line, and the hint on that row only.
        let state = PanelState::default();
        panel.cursor = 0;
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert_eq!(view.repos[0].button, Some("Open PR"));
        assert_eq!(view.repos[0].line, "feature/bulk \u{b7} PR api/pull/12");
        assert_eq!(view.repos[1].button, None);
        assert_eq!(view.repos[2].button, Some("Touch"));
        assert!(
            view.hints
                .contains(&("Ctrl+Shift+O".into(), "open PR".into()))
        );
        assert!(view.check.is_empty());
        panel.cursor = 1;
        let view = panel_view(&panel, &f, &slugs, &state, 2_060);
        assert!(!view.hints.iter().any(|(k, _)| k == "Ctrl+Shift+O"));
    }

    #[test]
    fn a_push_not_guarded_shows_on_its_repository() {
        let f = feature("g", vec![repo("api", true), repo("web", true)], 3);
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
        let f = feature("s", vec![repo("api", true)], 7);
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
        let building = feature("b", vec![repo("api", true)], 2);
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
        assert!(!watched(&feature("a", vec![repo("api", true)], 2)));
        assert!(watched(&feature("b", vec![repo("api", true)], 3)));
        let mut squashing = feature("c", vec![repo("api", true)], 1);
        squashing.shipping.squash_at = Some(1);
        assert!(watched(&squashing));
        assert_eq!(watch_every(&squashing), PANEL_EVERY);
        assert_eq!(
            watch_every(&feature("b", vec![repo("api", true)], 3)),
            WATCH_EVERY
        );
        let mut done = feature("d", vec![repo("api", true)], 8);
        done.status = Status::Done;
        assert!(!watched(&done));
    }

    #[test]
    fn sidebar_and_panel_list_the_right_features() {
        let mut open = feature("a", vec![repo("api", true)], 0);
        open.title = "Open".into();
        let mut abandoned = feature("b", vec![repo("api", true)], 1);
        abandoned.status = Status::Abandoned;
        let mut done = feature("c", vec![repo("api", true)], 8);
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
}
