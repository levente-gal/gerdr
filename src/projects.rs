//! Projects: a named set of workspaces (usually one per repository) that
//! lives in a herdr session of its own, so it can keep running while gerdr is
//! closed and come back after a reboot. herdr restores the layout and the
//! folders by itself; what ran in the panes, the venvs and the apps gerdr
//! launched outside herdr are remembered here, a file per project in
//! `~/.local/share/gerdr/projects/<id>.json`; the rest (the project shown
//! last, the quit choice, which of Cursor's idle chats stay shown, how the
//! repositories panel lists files, the tabs to go back and forward
//! through) in
//! `~/.local/share/gerdr/state.json`.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cursor_agents::History,
    desktop::shell_quote,
    herdr, home_dir,
    layout::SavedNode,
    nav_history::{NavHistory, Visit},
    repos::Grouping,
};

/// The store's format.
const VERSION: u32 = 1;
/// The session gerdr used before it had projects, and the project made of it.
const LEGACY: &str = "gerdr";
/// What a pane's idle shell may be called: with one of these leading the
/// foreground process group, nothing worth running again runs.
const SHELLS: [&str; 7] = ["sh", "bash", "zsh", "fish", "dash", "nu", "herdr"];

/// Everything gerdr remembers about the projects. Also the format of the
/// old single file, `projects.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// The project gerdr showed last, by id.
    #[serde(default)]
    pub last_active: Option<String>,
    /// What closing gerdr does without asking, once "Don't ask again" was
    /// checked.
    #[serde(default)]
    pub quit_choice: Option<QuitChoice>,
    /// What the quit or switch dialog was last told to stop, checked
    /// again the next time it asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_picks: Option<Stops>,
    #[serde(default)]
    pub projects: Vec<Project>,
    /// Which of Cursor's idle chats stay shown and which were deleted.
    #[serde(default, skip_serializing_if = "History::is_empty")]
    pub cursor_history: History,
    /// Which of T3 Code's threads were hidden from gerdr.
    #[serde(default, skip_serializing_if = "crate::t3_agents::Hidden::is_empty")]
    pub t3_hidden: crate::t3_agents::Hidden,
    /// The T3 Code threads that finished out of the user's sight, shown
    /// done until the user looks at them in gerdr (see `t3_agents`).
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::t3_agents::de_held"
    )]
    pub t3_finished: crate::t3_agents::Held,
    /// The pull requests whose CI is watched, by repository.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ci_watched: crate::ci::Watched,
    /// How the repositories panel lists changed files, as last picked.
    #[serde(default, skip_serializing_if = "Grouping::is_list")]
    pub repo_grouping: Grouping,
    /// How wide the sidebar was left, in logical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<i32>,
    /// How big gerdr's window was left, in logical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_size: Option<(i32, i32)>,
    /// Where reading each service's output for errors left off, by
    /// `<herdr session>|<pane id>` (see `service_watch`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub service_anchors: BTreeMap<String, crate::service_errors::Anchor>,
    /// The services' errors found, the newest first (see `services`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_errors: Vec<crate::services::ErrorGroup>,
    /// The services' errors the user ignores: one per project and
    /// fingerprint, still counted as they come (see `services`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_errors: Vec<crate::services::ErrorGroup>,
    /// The tabs shown, to go back and forward through, each parked (see
    /// `nav_history`).
    #[serde(default, skip_serializing_if = "NavHistory::is_empty")]
    pub nav: NavHistory<Visit>,
    #[serde(skip)]
    books: Books,
}

/// What `state.json` holds: the store but its projects.
#[derive(Debug, Serialize, Deserialize)]
struct State {
    version: u32,
    #[serde(default)]
    last_active: Option<String>,
    #[serde(default)]
    quit_choice: Option<QuitChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_picks: Option<Stops>,
    #[serde(default, skip_serializing_if = "History::is_empty")]
    cursor_history: History,
    #[serde(default, skip_serializing_if = "crate::t3_agents::Hidden::is_empty")]
    t3_hidden: crate::t3_agents::Hidden,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::t3_agents::de_held"
    )]
    t3_finished: crate::t3_agents::Held,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    ci_watched: crate::ci::Watched,
    #[serde(default, skip_serializing_if = "Grouping::is_list")]
    repo_grouping: Grouping,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_size: Option<(i32, i32)>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    service_anchors: BTreeMap<String, crate::service_errors::Anchor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    service_errors: Vec<crate::services::ErrorGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ignored_errors: Vec<crate::services::ErrorGroup>,
    #[serde(default, skip_serializing_if = "NavHistory::is_empty")]
    nav: NavHistory<Visit>,
}

/// Where the store lives and what gerdr last read or wrote of its files.
#[derive(Clone, Debug, Default)]
struct Books {
    /// The folder of the files; `None` for `~/.local/share/gerdr`.
    root: Option<PathBuf>,
    /// Per project id, its file.
    files: BTreeMap<String, Disk>,
    /// The ids whose file couldn't be read nor set aside: saving would lose
    /// it, so it is never written nor removed.
    frozen: BTreeSet<String>,
    /// The old `projects.json` couldn't be read nor set aside: nothing is
    /// saved, lest the files made in its place hide it.
    legacy_frozen: bool,
    /// A digest of `state.json`'s bytes, as gerdr last read or wrote them.
    state: Option<u64>,
}

/// Bookkeeping, not part of what a store holds.
impl PartialEq for Books {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Books {
    fn root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(data_dir)
    }
}

/// What gerdr last read or wrote of a project's file, to tell an edit made
/// outside gerdr (in Zed, say) from its own, and not save over it.
#[derive(Clone, Debug, Default)]
struct Disk {
    /// A digest of the file's bytes, as gerdr last read or wrote them.
    written: Option<u64>,
    /// The file changed outside gerdr: its saves wait until it is reloaded,
    /// or put back as it was.
    conflict: bool,
    /// The user was told about the conflict.
    told: bool,
}

fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// What `Store::load` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// The saved store. A project file that can't be read is set aside as
    /// `<id>.json.bad-<unix time>`, or, if even that failed, left where it
    /// is and never saved over.
    File,
    /// No store yet: the old single file, `projects.json`, split into a file
    /// per project (and kept as `projects.json.migrated`); else the session
    /// gerdr used before it had projects, made a project, on disk only once
    /// saved.
    Migrated,
    /// No store yet, and nothing to make one of.
    Empty,
    /// A store that can't be read: the old `projects.json` set aside as
    /// `projects.json.bad-<unix time>` for the user to rescue, or, if even
    /// that failed, left where it is and nothing saved; or the projects
    /// folder can't be listed.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitChoice {
    /// Keep the project's session running in the background.
    Keep,
    /// Stop the project's session and its apps kept running.
    Stop,
    /// Stop what is checked.
    Stops(Stops),
}

impl QuitChoice {
    /// What quitting stops.
    pub fn stops(self) -> Stops {
        match self {
            Self::Keep => Stops::default(),
            Self::Stop => Stops::ALL,
            Self::Stops(stops) => stops,
        }
    }
}

/// What of a project to stop when gerdr quits or shows another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stops {
    /// Its herdr session: its terminals and agents.
    #[serde(default)]
    pub session: bool,
    /// Its apps kept running (see `keep`), Cursor by default.
    #[serde(default)]
    pub apps: bool,
    /// T3 Code's server, which every project shares (see `t3_server`):
    /// asked only when quitting.
    #[serde(default)]
    pub t3: bool,
}

impl Stops {
    /// All of the project's own: not T3 Code's server, which others use.
    pub const ALL: Self = Self {
        session: true,
        apps: true,
        t3: false,
    };

    /// Whether it stops anything.
    pub fn any(self) -> bool {
        self.session || self.apps
    }
}

/// Whether the project was left running. A crash or a reboot leaves it
/// `Kept`: only stopping it on quit makes it `Stopped`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    #[default]
    Kept,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Unique, from `slugify`.
    pub id: String,
    pub name: String,
    /// The herdr session holding it: `session_name(id)`, or `gerdr` for the
    /// session gerdr used before it had projects.
    pub session: String,
    /// Unix seconds.
    pub created: u64,
    pub last_used: u64,
    #[serde(default)]
    pub state: RunState,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceRecord>,
    /// A shell line run before its session starts, when it isn't running:
    /// `docker compose up -d`, a script. Empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub before_start: String,
    /// A shell line run after its session stops. Empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub after_stop: String,
    /// Repositories that hold only tickets: their issues assigned to the
    /// user show in the inbox's Tickets, in the order of their board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tickets: Vec<crate::issues::TicketRepo>,
    /// How features are made in it (see `feature`).
    #[serde(default, skip_serializing_if = "Workflow::is_empty")]
    pub workflow: Workflow,
}

/// A project's feature settings, in its store file under `"workflow"`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Repositories features leave out, by folder name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// The branch features start from, by repository folder name, where
    /// `origin/HEAD` isn't the one.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub base: std::collections::BTreeMap<String, String>,
    /// How features ship, for every repository; without it, per
    /// repository by its `origin` (see `feature::ship::mode_of`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<crate::feature::ShipMode>,
    /// The agent each stage starts, by the picker's name ("Claude Code")
    /// or command (`claude`); without one, the first agent CLI installed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub agents: std::collections::BTreeMap<crate::feature::Stage, String>,
    /// The commands that set up a new feature's worktree, by repository
    /// folder name (see `feature::setup`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub setup: std::collections::BTreeMap<String, Vec<String>>,
    /// The checks Verify runs, by repository folder name or `_feature`
    /// (see `feature::verify`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub verify: std::collections::BTreeMap<String, Vec<String>>,
    /// How long one Verify check may run, in seconds; without it,
    /// `feature::verify::TIMEOUT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_timeout: Option<u64>,
}

impl Workflow {
    pub fn is_empty(&self) -> bool {
        self.ignore.is_empty()
            && self.base.is_empty()
            && self.ship.is_none()
            && self.agents.is_empty()
            && self.setup.is_empty()
            && self.verify.is_empty()
            && self.verify_timeout.is_none()
    }
}

/// What gerdr kept of a workspace of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// gerdr's id for it, stable for as long as it is kept: the herdr id
    /// it had first for one saved before records had their own (`w6`),
    /// else `g1`, `g2`… (see `Store::new_workspace_id`).
    #[serde(default)]
    pub id: String,
    /// The herdr workspace it mirrors (`w6`), while it has one: stable
    /// across a server restart. One with only apps has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_id: Option<String>,
    pub label: String,
    pub kind: Kind,
    pub dir: PathBuf,
    #[serde(default)]
    pub venv: VenvSetting,
    /// Its tabs' layouts, in tab order.
    #[serde(default)]
    pub tabs: Vec<TabRecord>,
    #[serde(default)]
    pub panes: Vec<PaneRecord>,
    /// Whether it was folded or unfolded by hand in the sidebar; `None`
    /// leaves it to fold while quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folded: Option<bool>,
    /// Whether restarting the workspace runs each command again, as the
    /// user chose: asked again only once a new command comes (see
    /// `restart::plan`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart_choices: Vec<RestartChoice>,
}

/// Whether restarting a workspace runs `command` again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartChoice {
    pub command: String,
    pub restart: bool,
}

/// What a workspace opens with: mirrors `WorkspaceKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Zed over a terminal. GitHub workspaces, which were once a kind of
    /// their own, open as one.
    #[serde(alias = "github")]
    Project,
    Terminal,
    /// The project's Cursor.
    Cursor,
}

/// Which Python venv the workspace's shells activate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VenvSetting {
    /// The `.venv` or `venv` folder found from the pane's folder up to the
    /// workspace's.
    #[default]
    Auto,
    Off,
    Path(PathBuf),
}

/// A tab's layout, including the panes herdr knows nothing about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabRecord {
    /// The herdr tab id (`w6:t2`), if it holds herdr panes.
    #[serde(default)]
    pub herdr_tab: Option<String>,
    pub layout: SavedNode,
    /// The name the tab was given, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// What ran in a herdr pane, to run again when its server restarts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneRecord {
    /// The herdr pane id (`w6:p3`): stable across a server restart.
    pub pane_id: String,
    /// Fallback key if the id is gone: the tab's place in the workspace and
    /// the pane's place in the tab, in herdr's order.
    #[serde(default)]
    pub tab: usize,
    #[serde(default)]
    pub index: usize,
    /// The foreground command last seen running, shell-quoted; `None` once
    /// the shell has been idle for a while.
    #[serde(default)]
    pub command: Option<String>,
    /// Where the command ran, if not in the pane's own folder.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The command's environment worth restoring: `VIRTUAL_ENV`,
    /// `MISE_*_VERSION`, `MISE_ENV`.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Commands the user said not to run again in this pane.
    #[serde(default)]
    pub skip: Vec<String>,
}

/// A pane's foreground command, as read from herdr and `/proc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Store {
    /// The saved store; the first time, the old single file `projects.json`
    /// split into a file per project, or the session gerdr used before it
    /// had projects (see `migrate`); else an empty one. Says which.
    pub fn load() -> (Self, Loaded) {
        Self::load_from(&data_dir(), Self::first_run)
    }

    /// `load`, from the files under `root`.
    fn load_from(root: &Path, first_run: impl FnOnce() -> Option<Self>) -> (Self, Loaded) {
        let books = Books {
            root: Some(root.to_owned()),
            ..Books::default()
        };
        let dir = projects_in(root);
        match project_files(&dir) {
            Ok(files) => {
                let mut store = Self {
                    books,
                    ..Self::empty()
                };
                store.load_state(&root.join("state.json"));
                for (id, path) in files {
                    store.load_project(&id, &path);
                }
                (store, Loaded::File)
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                Self::load_legacy(&root.join("projects.json"), books, first_run)
            }
            Err(err) => {
                crate::problems::report(
                    "store",
                    format!("Couldn't read the projects in {}: {err}", dir.display()),
                );
                let mut store = Self {
                    books,
                    ..Self::empty()
                };
                store.load_state(&root.join("state.json"));
                (store, Loaded::Failed)
            }
        }
    }

    /// Take `state.json` in; one that can't be read is set aside.
    fn load_state(&mut self, path: &Path) {
        match read_state(path) {
            Ok(Some((state, written))) => {
                self.set_state(state);
                self.books.state = Some(written);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                set_aside(path);
            }
        }
    }

    fn set_state(&mut self, state: State) {
        self.version = state.version;
        self.last_active = state.last_active;
        self.quit_choice = state.quit_choice;
        self.stop_picks = state.stop_picks;
        self.cursor_history = state.cursor_history;
        self.t3_hidden = state.t3_hidden;
        self.t3_finished = state.t3_finished;
        self.ci_watched = state.ci_watched;
        self.repo_grouping = state.repo_grouping;
        self.sidebar_width = state.sidebar_width;
        self.window_size = state.window_size;
        self.service_anchors = state.service_anchors;
        self.service_errors = state.service_errors;
        self.ignored_errors = state.ignored_errors;
        self.nav = state.nav;
    }

    fn state(&self) -> State {
        State {
            version: self.version,
            last_active: self.last_active.clone(),
            quit_choice: self.quit_choice,
            stop_picks: self.stop_picks,
            cursor_history: self.cursor_history.clone(),
            t3_hidden: self.t3_hidden.clone(),
            t3_finished: self.t3_finished.clone(),
            ci_watched: self.ci_watched.clone(),
            repo_grouping: self.repo_grouping,
            sidebar_width: self.sidebar_width,
            window_size: self.window_size,
            service_anchors: self.service_anchors.clone(),
            service_errors: self.service_errors.clone(),
            ignored_errors: self.ignored_errors.clone(),
            nav: self.nav.clone(),
        }
    }

    /// Take project file `path` in. One that can't be read is set aside, or,
    /// if even that failed, never saved over.
    fn load_project(&mut self, id: &str, path: &Path) {
        match read_project(id, path) {
            Ok((project, written)) => {
                let disk = Disk {
                    written: Some(written),
                    ..Disk::default()
                };
                self.books.files.insert(project.id.clone(), disk);
                self.projects.push(project);
            }
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                if !set_aside(path) && valid_id(id) {
                    self.books.frozen.insert(id.to_owned());
                }
            }
        }
    }

    /// The store in the old single file at `path`, written out as a file per
    /// project, the old one kept as `projects.json.migrated` once they all
    /// are; without one, `first_run`'s.
    fn load_legacy(
        path: &Path,
        books: Books,
        first_run: impl FnOnce() -> Option<Self>,
    ) -> (Self, Loaded) {
        let failed = |err: &dyn std::fmt::Display| {
            tracing::warn!("reading {}: {err}", path.display());
            let books = Books {
                legacy_frozen: !set_aside(path),
                ..books.clone()
            };
            let store = Self {
                books,
                ..Self::empty()
            };
            (store, Loaded::Failed)
        };
        match std::fs::read(path) {
            Ok(raw) => match serde_json::from_slice::<Self>(&raw) {
                Ok(mut store) => {
                    store
                        .projects
                        .iter_mut()
                        .for_each(Project::fill_workspace_ids);
                    let mut store = Self {
                        books: books.clone(),
                        ..store
                    };
                    if store.write() {
                        let mut migrated = path.as_os_str().to_owned();
                        migrated.push(".migrated");
                        if let Err(err) = std::fs::rename(path, &migrated) {
                            tracing::warn!("moving {} aside: {err}", path.display());
                        }
                    } else {
                        tracing::warn!("{} not all written out: kept", path.display());
                    }
                    (store, Loaded::Migrated)
                }
                Err(err) => failed(&err),
            },
            Err(err) if err.kind() == ErrorKind::NotFound => match first_run() {
                Some(store) => (Self { books, ..store }, Loaded::Migrated),
                None => (
                    Self {
                        books,
                        ..Self::empty()
                    },
                    Loaded::Empty,
                ),
            },
            Err(err) => failed(&err),
        }
    }

    /// The ids of the projects whose file changed outside gerdr since the
    /// user was last told; each once.
    pub fn take_conflicts(&mut self) -> Vec<String> {
        let mut ids = Vec::new();
        for (id, disk) in &mut self.books.files {
            if disk.conflict && !disk.told {
                disk.told = true;
                ids.push(id.clone());
            }
        }
        ids
    }

    /// The store in the files now, for a reload. A file that can't be read
    /// is left where it is, to be fixed and reloaded: its project stays as
    /// it is in `self` and its saves stay paused, and `errors` names it. A
    /// project whose file is gone was deleted. What isn't any one project's
    /// stays as it is in `self`: `state.json` isn't for editing.
    pub fn reread(&self) -> Reread {
        let dir = projects_in(&self.books.root());
        let files = match project_files(&dir) {
            Ok(files) => files,
            Err(err) => {
                return Reread {
                    store: self.clone(),
                    errors: vec![format!("{}: {err}", dir.display())],
                };
            }
        };
        let mut store = Self {
            projects: Vec::new(),
            books: Books {
                files: BTreeMap::new(),
                ..self.books.clone()
            },
            ..self.clone()
        };
        let mut errors = Vec::new();
        for (id, path) in files {
            match read_project(&id, &path) {
                Ok((project, written)) => {
                    let disk = Disk {
                        written: Some(written),
                        ..Disk::default()
                    };
                    // Readable again: fixed.
                    store.books.frozen.remove(&project.id);
                    store.books.files.insert(project.id.clone(), disk);
                    store.projects.push(project);
                }
                Err(err) => {
                    errors.push(format!("{id}.json: {err}"));
                    let Some(project) = self.project(&id) else {
                        continue;
                    };
                    // Told by `errors`.
                    let disk = Disk {
                        conflict: true,
                        told: true,
                        ..self.books.files.get(&id).cloned().unwrap_or_default()
                    };
                    store.books.files.insert(id, disk);
                    store.projects.push(project.clone());
                }
            }
        }
        Reread { store, errors }
    }

    fn empty() -> Self {
        Self {
            version: VERSION,
            ..Self::default()
        }
    }

    /// The store before one was ever saved: the legacy session as a project,
    /// if there is one.
    fn first_run() -> Option<Self> {
        let dir = herdr::session_dir(LEGACY);
        if !dir.exists() {
            return None;
        }
        let live = herdr::is_running(LEGACY)
            .then(|| herdr::Herdr::on_socket(herdr::socket_path(LEGACY)).workspace_list())
            .and_then(|list| list.map_err(|err| tracing::warn!("{err}")).ok());
        let session_json = std::fs::read_to_string(dir.join("session.json")).ok();
        let project = migrate(live.as_deref(), session_json.as_deref())?;
        Some(Self {
            last_active: Some(project.id.clone()),
            projects: vec![project],
            ..Self::empty()
        })
    }

    /// Write out what changed, each file atomically (temporary file, then
    /// rename). A project's file that changed outside gerdr since gerdr last
    /// read or wrote it isn't saved over, nor removed once the project is
    /// gone; the other projects save on.
    pub fn save(&mut self) {
        self.write();
    }

    /// `save`; whether all of it is on disk.
    fn write(&mut self) -> bool {
        let root = self.books.root();
        if self.books.legacy_frozen {
            let legacy = root.join("projects.json");
            tracing::warn!("not saving while {} can't be read", legacy.display());
            return false;
        }
        let mut all = true;
        let state = serde_json::to_vec_pretty(&self.state()).map_err(std::io::Error::other);
        let state = state.and_then(|json| {
            let written = digest(&json);
            if self.books.state != Some(written) {
                write_atomic(&root.join("state.json"), &json)?;
            }
            Ok(written)
        });
        match state {
            Ok(written) => self.books.state = Some(written),
            Err(err) => {
                tracing::warn!("saving state.json: {err}");
                all = false;
            }
        }
        let dir = projects_in(&root);
        for project in &self.projects {
            let id = &project.id;
            if !valid_id(id) {
                tracing::warn!("not saving project {id:?}: it isn't a valid id");
                all = false;
                continue;
            }
            let path = dir.join(format!("{id}.json"));
            if self.books.frozen.contains(id) {
                tracing::warn!("not saving over the unreadable {}", path.display());
                all = false;
                continue;
            }
            let json = match serde_json::to_vec_pretty(project) {
                Ok(json) => json,
                Err(err) => {
                    tracing::warn!("saving {}: {err}", path.display());
                    all = false;
                    continue;
                }
            };
            let disk = self.books.files.entry(id.clone()).or_default();
            let written = digest(&json);
            if disk.written == Some(written) {
                continue;
            }
            if on_disk(&path, disk.written) != disk.written {
                if !disk.conflict {
                    tracing::warn!(
                        "{} changed outside gerdr: not saving over it",
                        path.display()
                    );
                }
                disk.conflict = true;
                all = false;
                continue;
            }
            match write_atomic(&path, &json) {
                Ok(()) => {
                    *disk = Disk {
                        written: Some(written),
                        ..Disk::default()
                    };
                }
                Err(err) => {
                    tracing::warn!("saving {}: {err}", path.display());
                    all = false;
                }
            }
        }
        // The files of the projects gone, unless edited outside gerdr.
        let kept: BTreeSet<&str> = self.projects.iter().map(|p| p.id.as_str()).collect();
        self.books.files.retain(|id, disk| {
            if kept.contains(id.as_str()) {
                return true;
            }
            let path = dir.join(format!("{id}.json"));
            if on_disk(&path, disk.written) != disk.written {
                tracing::warn!("{} changed outside gerdr: not removing it", path.display());
                return false;
            }
            match std::fs::remove_file(&path) {
                Err(err) if err.kind() != ErrorKind::NotFound => {
                    tracing::warn!("removing {}: {err}", path.display());
                    all = false;
                }
                _ => {}
            }
            false
        });
        all
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn project_mut(&mut self, id: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.id == id)
    }

    /// Put project `project`'s workspaces in `order` (record ids), those
    /// not in it last; whether that moved any.
    pub fn order_workspaces(&mut self, project: &str, order: &[&str]) -> bool {
        let Some(project) = self.project_mut(project) else {
            return false;
        };
        let place = |id: &str| order.iter().position(|o| *o == id).unwrap_or(usize::MAX);
        if project.workspaces.is_sorted_by_key(|r| place(&r.id)) {
            return false;
        }
        project.workspaces.sort_by_key(|r| place(&r.id));
        true
    }

    /// Project `project`'s workspace record `id`.
    /// Project `project`'s record of workspace `id`.
    pub fn record(&self, project: &str, id: &str) -> Option<&WorkspaceRecord> {
        self.project(project)?
            .workspaces
            .iter()
            .find(|w| w.id == id)
    }

    pub fn record_mut(&mut self, project: &str, id: &str) -> Option<&mut WorkspaceRecord> {
        self.project_mut(project)?
            .workspaces
            .iter_mut()
            .find(|w| w.id == id)
    }

    /// Project `project`'s record of herdr workspace `herdr_id`.
    pub fn herdr_record_mut(
        &mut self,
        project: &str,
        herdr_id: &str,
    ) -> Option<&mut WorkspaceRecord> {
        self.project_mut(project)?
            .workspaces
            .iter_mut()
            .find(|w| w.mirrors(herdr_id))
    }
}

impl Project {
    /// Give the workspace records saved before they had ids of their own
    /// one: their herdr workspace's id, else a new one.
    fn fill_workspace_ids(&mut self) {
        for record in &mut self.workspaces {
            if record.id.is_empty() {
                record.id = record.herdr_id.clone().unwrap_or_default();
            }
        }
        for i in 0..self.workspaces.len() {
            if self.workspaces[i].id.is_empty() {
                self.workspaces[i].id = self.new_workspace_id();
            }
        }
    }

    /// An id for a new workspace record, unlike any it has.
    pub fn new_workspace_id(&self) -> String {
        (1..)
            .map(|n| format!("g{n}"))
            .find(|id| self.workspaces.iter().all(|r| r.id != *id))
            .unwrap()
    }

    /// Its record of herdr workspace `herdr_id`.
    pub fn herdr_record(&self, herdr_id: &str) -> Option<&WorkspaceRecord> {
        self.workspaces.iter().find(|r| r.mirrors(herdr_id))
    }
}

impl WorkspaceRecord {
    /// Whether it mirrors herdr workspace `herdr_id`.
    pub fn mirrors(&self, herdr_id: &str) -> bool {
        self.herdr_id.as_deref() == Some(herdr_id)
    }
}

/// What `Store::reread` found.
#[derive(Debug)]
pub struct Reread {
    pub store: Store,
    /// The files that couldn't be read, each with why, ready to show.
    pub errors: Vec<String>,
}

/// The folder of gerdr's files.
pub(crate) fn data_dir() -> PathBuf {
    home_dir().unwrap_or_default().join(".local/share/gerdr")
}

/// The folder holding a file per project.
pub fn projects_dir() -> PathBuf {
    projects_in(&data_dir())
}

fn projects_in(root: &Path) -> PathBuf {
    root.join("projects")
}

/// Project `id`'s file.
pub fn project_file(id: &str) -> PathBuf {
    projects_dir().join(format!("{id}.json"))
}

/// Whether `id` can be a project's, and so a file's name: lowercase
/// `[a-z0-9-]`, not starting with a dash, as `slugify` makes them.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The project files in `dir`, as (id, path), by name: `<id>.json`, not
/// hidden. Leftovers (`.json.tmp`, `.json.bad-<unix time>`) aren't.
fn project_files(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if let Some(id) = name.strip_suffix(".json") {
            files.push((id.to_owned(), entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

/// The project in file `path`, and a digest of its bytes; it must be
/// project `id`'s.
fn read_project(id: &str, path: &Path) -> Result<(Project, u64), String> {
    let raw = std::fs::read(path).map_err(|err| err.to_string())?;
    let mut project: Project = serde_json::from_slice(&raw).map_err(|err| err.to_string())?;
    project.fill_workspace_ids();
    if !valid_id(&project.id) {
        return Err(format!("{:?} isn't a valid id", project.id));
    }
    if project.id != id {
        return Err(format!("id {:?} doesn't match the file name", project.id));
    }
    Ok((project, digest(&raw)))
}

/// What `state.json` at `path` holds, and a digest of its bytes; `None` if
/// there is none.
fn read_state(path: &Path) -> Result<Option<(State, u64)>, String> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    let state = serde_json::from_slice(&raw).map_err(|err| err.to_string())?;
    Ok(Some((state, digest(&raw))))
}

/// A digest of the file at `path` now: `written` if it is gone, as there is
/// nothing to lose; `None` if it can't be read.
fn on_disk(path: &Path, written: Option<u64>) -> Option<u64> {
    match std::fs::read(path) {
        Ok(raw) => Some(digest(&raw)),
        Err(err) if err.kind() == ErrorKind::NotFound => written,
        Err(_) => None,
    }
}

/// Write `bytes` to `path` atomically: to `<path>.tmp`, then renamed.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Move the unreadable file at `path` out of the way of the next save, as
/// `<path>.bad-<unix time>`; whether it is out of the way.
///
/// What it did goes to the problems list (see `problems`): the user may
/// want what was in it.
pub(crate) fn set_aside(path: &Path) -> bool {
    let bad = bad_name(path, now());
    match std::fs::rename(path, &bad) {
        Ok(()) => {
            let name = bad.file_name().unwrap_or_default().to_string_lossy();
            crate::problems::report(
                "store",
                format!(
                    "{} couldn't be read: it is kept as {name} (see the log)",
                    path.display()
                ),
            );
            true
        }
        Err(err) => {
            tracing::warn!("setting {} aside: {err}", path.display());
            crate::problems::report(
                "store",
                format!(
                    "{} couldn't be read nor set aside: it won't be saved over",
                    path.display()
                ),
            );
            false
        }
    }
}

fn bad_name(path: &Path, now: u64) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bad-{now}"));
    PathBuf::from(name)
}

impl PaneRecord {
    /// The command to run when the pane comes back, unless the user said not
    /// to or it is never run again (see `DENY`).
    pub fn restart_command(&self) -> Option<&str> {
        self.command
            .as_deref()
            .filter(|command| !self.skip.iter().any(|s| s == command))
            .filter(|command| !denied_command(command))
    }
}

/// A unique id for a project called `name`: lowercase `[a-z0-9-]`, with
/// `-2`, `-3`, … added if `taken` has it already.
pub fn slugify(name: &str, taken: &[&str]) -> String {
    let mut slug = String::new();
    // Anything else, however long a run of it, becomes one dash between
    // words and nothing at the ends.
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    if slug.is_empty() {
        slug = "project".to_owned();
    }
    if !taken.contains(&slug.as_str()) {
        return slug;
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .unwrap_or(slug)
}

/// The herdr session of a new project: `gerdr-<id>`, apart from the user's
/// own sessions.
pub fn session_name(id: &str) -> String {
    format!("gerdr-{id}")
}

/// What runs in the foreground of a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    /// Its shell waits at its prompt (or a bare shell runs in it).
    Idle,
    /// A command worth running again, led by process `pid`.
    Busy { pid: i32, captured: Captured },
    /// Something led by process `pid` that isn't to be run again: `denied`
    /// if it is never run again (see `DENY`), else its command line can't be
    /// read back (a process title rewritten by setproctitle, like gunicorn's
    /// `gunicorn: master [app]` or celery's `[celeryd: …]`).
    Untracked { pid: i32, denied: bool },
}

/// When running a program again is out of the question.
enum Deny {
    Always,
    /// When its first word (flags aside) is one of these: it changes what
    /// is installed.
    FirstArg(&'static [&'static str]),
    /// When it runs no script: an interactive prompt.
    NoScript,
}

const NODE_CHANGES: &[&str] = &[
    "install",
    "i",
    "add",
    "remove",
    "uninstall",
    "update",
    "upgrade",
    "ci",
    "publish",
];
const PACKAGE_CHANGES: &[&str] = &[
    "install",
    "add",
    "remove",
    "uninstall",
    "update",
    "upgrade",
    "sync",
    "lock",
];

/// Programs never run again after a restart, by name: they page, reach
/// other machines, or change files or the system.
const DENY: &[(&[&str], Deny)] = &[
    // Pagers.
    (&["less", "more", "man", "most", "bat"], Deny::Always),
    // Other machines.
    (
        &["ssh", "mosh", "scp", "sftp", "rsync", "telnet"],
        Deny::Always,
    ),
    // Version control.
    (&["git", "lazygit", "gh", "tig"], Deny::Always),
    // Files.
    (&["rm", "mv", "cp", "dd", "shred"], Deny::Always),
    // Privileges.
    (&["sudo", "doas", "su"], Deny::Always),
    (
        &["npm", "pnpm", "yarn", "bun"],
        Deny::FirstArg(NODE_CHANGES),
    ),
    (
        &[
            "pip", "pip3", "uv", "poetry", "pipx", "cargo", "brew", "apt", "apt-get", "dnf", "yum",
            "pacman", "flatpak", "snap", "mise", "rustup",
        ],
        Deny::FirstArg(PACKAGE_CHANGES),
    ),
    (&["python", "python3", "node", "ipython"], Deny::NoScript),
];

/// Interpreters whose script names the program (`python3 ~/.local/bin/pip`).
const INTERPRETERS: [&str; 5] = ["python", "python3", "node", "bun", "deno"];

/// The last part of a path.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `python3.12` is `python3`, `python3` is `python3`.
fn program_name(name: &str) -> &str {
    let base = basename(name);
    match base.strip_prefix("python3.") {
        Some(minor) if !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit()) => "python3",
        _ => base,
    }
}

/// Whether `program` run with `args` is never run again (see `DENY`).
fn denied_program(program: &str, args: &[&str]) -> bool {
    let program = program_name(program);
    let Some((_, when)) = DENY.iter().find(|(names, _)| names.contains(&program)) else {
        return false;
    };
    match when {
        Deny::Always => true,
        Deny::FirstArg(changes) => args
            .iter()
            .find(|a| !a.starts_with('-'))
            .is_some_and(|a| changes.contains(a)),
        Deny::NoScript => args.iter().all(|a| a.starts_with('-')),
    }
}

/// Whether the command line `argv` (of a process called `name`) is never
/// run again: by its program, or the script or module an interpreter runs.
fn denied(name: &str, argv: &[&str]) -> bool {
    // The process name can't tell a script from an interactive prompt.
    let always = |name: &str| {
        DENY.iter()
            .any(|(names, when)| matches!(when, Deny::Always) && names.contains(&name))
    };
    if always(program_name(name.trim_start_matches('-'))) {
        return true;
    }
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    if denied_program(program, args) {
        return true;
    }
    if !INTERPRETERS.contains(&program_name(program)) {
        return false;
    }
    match args {
        ["-m", module, rest @ ..] => denied_program(module, rest),
        [script, rest @ ..] if !script.starts_with('-') => denied_program(script, rest),
        _ => false,
    }
}

/// Whether `argv0` reads like a program, not a process title rewritten by
/// the program itself (`gunicorn: master [app]`, `[celeryd: …]`), which
/// wouldn't run.
fn usable_argv0(argv0: &str) -> bool {
    !argv0.is_empty()
        && !argv0.contains(':')
        && !argv0.starts_with('[')
        && !argv0.contains(char::is_whitespace)
}

/// What npm was run with, from the title it gives its process in place of
/// its command line (`npm exec nx run app:serve` for `npx nx run
/// app:serve`). The title holds only npm's positional arguments, unquoted,
/// so it is read back only where little is lost: `npm exec` (or `npm x`)
/// comes back as `npx`, which hands the rest to the command as it is;
/// another npm command only if no word is an option, which the title would
/// have taken out from behind its `--`. Never with secrets npm blanked out.
fn npm_title_argv(title: &str) -> Option<Vec<&str>> {
    let words: Vec<&str> = title.split_whitespace().collect();
    let ["npm", command, args @ ..] = words.as_slice() else {
        return None;
    };
    if words.iter().any(|word| word.contains("***")) {
        return None;
    }
    match (*command, args) {
        ("exec" | "x", [program, ..]) if !program.starts_with('-') => {
            Some(std::iter::once("npx").chain(args.iter().copied()).collect())
        }
        ("exec" | "x", _) => None,
        _ if args.iter().any(|arg| arg.starts_with('-')) => None,
        _ => Some(words),
    }
}

/// The words of `command`, as `shell_quote` quoted them: single quotes and
/// backslashes undone.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let word = word.get_or_insert_default();
                word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '\\' => word.get_or_insert_default().extend(chars.next()),
            c if c.is_whitespace() => words.extend(word.take()),
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    words
}

/// Whether the saved `command` is never to be run again, as `DENY` says:
/// for records made before it did.
fn denied_command(command: &str) -> bool {
    let words = split_command(command);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    words.first().is_some_and(|program| denied(program, &words))
}

/// What runs in the foreground of a pane: its idle shell, or the process
/// group leader's command line, where it runs (if not in `pane_cwd`), and
/// its environment worth restoring (from `/proc`). `None` if herdr can't
/// tell.
pub fn foreground_command(
    info: &herdr::ProcessInfo,
    pane_cwd: Option<&Path>,
) -> Option<Foreground> {
    let shell = info.shell_pid?;
    let group = info.foreground_process_group_id?;
    if group == shell {
        return Some(Foreground::Idle);
    }
    // In a pipeline the group is named after its first command.
    let leader = info
        .foreground_processes
        .iter()
        .find(|p| p.pid == group)
        .or(info.foreground_processes.first())?;
    // Login shells show up as `-zsh`; one running a script isn't idle.
    let name = leader.name.trim_start_matches('-');
    if SHELLS.contains(&name) && (name == "herdr" || !crate::prepare::runs_script(&leader.argv)) {
        return Some(Foreground::Idle);
    }
    let pid = leader.pid;
    let untracked = |denied| Some(Foreground::Untracked { pid, denied });
    // Without argv, the command line is trusted only as a single word: its
    // spaces can't be told from those inside an argument.
    let cmdline = leader.cmdline.trim();
    let title = match leader.argv.as_slice() {
        [title] => title.as_str(),
        [] => cmdline,
        _ => "",
    };
    let argv: Vec<&str> = if let Some(argv) = npm_title_argv(title) {
        argv
    } else if leader.argv.is_empty() {
        if cmdline.contains(char::is_whitespace) {
            let rough: Vec<&str> = cmdline.split_whitespace().collect();
            return untracked(denied(&leader.name, &rough));
        }
        Some(cmdline)
            .filter(|c| !c.is_empty())
            .into_iter()
            .collect()
    } else {
        leader.argv.iter().map(String::as_str).collect()
    };
    if denied(&leader.name, &argv) {
        return untracked(true);
    }
    if !argv.first().is_some_and(|argv0| usable_argv0(argv0)) {
        return untracked(false);
    }
    let words: Vec<_> = argv.iter().map(|arg| shell_quote(arg)).collect();
    let cwd = leader
        .cwd
        .clone()
        .filter(|cwd| Some(cwd.as_path()) != pane_cwd);
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    Some(Foreground::Busy {
        pid,
        captured: Captured {
            command: words.join(" "),
            cwd,
            env: environ_whitelist(&environ),
        },
    })
}

impl Store {
    /// The project whose herdr session a process runs in, from its raw
    /// `/proc/<pid>/environ`: herdr gives the processes in its panes
    /// `HERDR_SOCKET_PATH` and `HERDR_SESSION`. `None` if it has neither;
    /// `Some(None)` if they name no project of gerdr's.
    pub fn project_of_environ(&self, raw: &[u8]) -> Option<Option<&str>> {
        let mut socket = None;
        let mut session = None;
        for var in raw.split(|&b| b == 0) {
            let Some((key, value)) = std::str::from_utf8(var)
                .ok()
                .and_then(|v| v.split_once('='))
            else {
                continue;
            };
            match key {
                "HERDR_SOCKET_PATH" => socket = Some(PathBuf::from(value)),
                "HERDR_SESSION" => session = Some(value),
                _ => {}
            }
        }
        if socket.is_none() && session.is_none() {
            return None;
        }
        // The socket says which server; the session name only without one.
        let found = self.projects.iter().find(|p| match &socket {
            Some(socket) => *socket == herdr::socket_path(&p.session),
            None => session == Some(p.session.as_str()),
        });
        Some(found.map(|p| p.id.as_str()))
    }
}

/// The variables of a raw `/proc/<pid>/environ` worth restoring.
pub fn environ_whitelist(raw: &[u8]) -> Vec<(String, String)> {
    let keep = |key: &str| {
        key == "VIRTUAL_ENV"
            || key == "MISE_ENV"
            || key
                .strip_prefix("MISE_")
                .and_then(|tool| tool.strip_suffix("_VERSION"))
                .is_some_and(|tool| !tool.is_empty())
    };
    let mut vars: Vec<_> = raw
        .split(|&b| b == 0)
        .filter_map(|var| {
            let (key, value) = std::str::from_utf8(var).ok()?.split_once('=')?;
            keep(key).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect();
    vars.sort();
    vars
}

/// How long ago `then` was, for people: "just now", "5m ago", "3h ago",
/// "yesterday", "4d ago", …
pub fn ago(now: u64, then: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    // A clock set back puts `then` in the future.
    match now.saturating_sub(then) {
        s if s < MINUTE => "just now".to_owned(),
        s if s < HOUR => format!("{}m ago", s / MINUTE),
        s if s < DAY => format!("{}h ago", s / HOUR),
        s if s < 2 * DAY => "yesterday".to_owned(),
        s if s < 7 * DAY => format!("{}d ago", s / DAY),
        s if s < 35 * DAY => format!("{}w ago", s / (7 * DAY)),
        s => format!("{}mo ago", s / (30 * DAY)),
    }
}

/// The notice on switching to project `new` (just made, if `created`)
/// from project `old`, which keeps running.
pub fn switch_notice(new: &str, old: &str, created: bool) -> String {
    format!("{} {new} · {old} keeps running", switched(created))
}

/// The notice after switching to project `new` (just made, if `created`)
/// from `old`, which stops.
pub fn stop_notice(new: &str, old: &str, created: bool) -> String {
    format!("{} {new} · stopping {old}", switched(created))
}

/// How a switch notice starts: one notice says both that the project was
/// made and shown.
fn switched(created: bool) -> &'static str {
    if created {
        "Created and switched to"
    } else {
        "Switched to"
    }
}

/// The greeting for the local `hour` (0–23).
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Burning the midnight oil",
    }
}

/// The local hour (0–23).
pub fn local_hour() -> u32 {
    // SAFETY: `localtime_r` writes only to the `tm` it is given, and an
    // all-zero `tm` is a valid one.
    let (tm, ok) = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        let ok = !libc::localtime_r(&now, &mut tm).is_null();
        (tm, ok)
    };
    if ok {
        u32::try_from(tm.tm_hour).unwrap_or(12)
    } else {
        12
    }
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The project made of the session gerdr used before it had projects
/// (`gerdr`): its workspaces from the running server's metadata (`live`),
/// else from its saved `session.json`. `None` if neither has any.
pub fn migrate(
    live: Option<&[herdr::WorkspaceInfo]>,
    session_json: Option<&str>,
) -> Option<Project> {
    let from_live = live
        .map(|list| list.iter().filter_map(live_record).collect::<Vec<_>>())
        .filter(|records| !records.is_empty());
    let workspaces = from_live
        .or_else(|| session_json.map(saved_records))
        .filter(|records| !records.is_empty())?;
    let now = now();
    Some(Project {
        id: LEGACY.to_owned(),
        name: LEGACY.to_owned(),
        session: LEGACY.to_owned(),
        created: now,
        last_used: now,
        state: RunState::Kept,
        workspaces,
        before_start: String::new(),
        after_stop: String::new(),
        tickets: Vec::new(),
        workflow: Workflow::default(),
    })
}

fn record(herdr_id: String, label: String, kind: Kind, dir: PathBuf) -> WorkspaceRecord {
    WorkspaceRecord {
        id: herdr_id.clone(),
        herdr_id: Some(herdr_id),
        label,
        kind,
        dir,
        venv: VenvSetting::Auto,
        tabs: Vec::new(),
        panes: Vec::new(),
        folded: None,
        restart_choices: Vec::new(),
    }
}

/// A workspace gerdr opened, from the metadata it gave the server.
fn live_record(info: &herdr::WorkspaceInfo) -> Option<WorkspaceRecord> {
    if !info.is_gerdr() {
        return None;
    }
    let kind = if info.is_project() {
        Kind::Project
    } else {
        Kind::Terminal
    };
    Some(record(
        info.workspace_id.clone(),
        info.label.clone(),
        kind,
        info.dir()?,
    ))
}

/// The workspaces herdr saved in `session.json`. It keeps no gerdr metadata,
/// so each counts as a project.
fn saved_records(session_json: &str) -> Vec<WorkspaceRecord> {
    let saved: Value = match serde_json::from_str(session_json) {
        Ok(saved) => saved,
        Err(err) => {
            tracing::warn!("reading the gerdr session's session.json: {err}");
            return Vec::new();
        }
    };
    let workspaces = saved["workspaces"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    workspaces
        .iter()
        .filter_map(|ws| {
            let id = ws["id"].as_str()?;
            let dir = PathBuf::from(ws["identity_cwd"].as_str().filter(|d| !d.is_empty())?);
            let label = ws["custom_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .or_else(|| Some(dir.file_name()?.to_string_lossy().into_owned()))
                .unwrap_or_else(|| dir.display().to_string());
            Some(record(id.to_owned(), label, Kind::Project, dir))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Dir, SavedLeaf};
    use serde_json::json;

    // Above the kernel's pid limit, so `/proc` has nothing for them.
    const NO_PID: i32 = 2_000_000_000;

    #[test]
    fn a_saved_github_workspace_opens_as_a_project() {
        assert_eq!(
            serde_json::from_value::<Kind>(json!("github")).unwrap(),
            Kind::Project
        );
        assert_eq!(
            serde_json::to_value(Kind::Project).unwrap(),
            json!("project")
        );
    }

    fn process(pid: i32, name: &str, argv: &[&str], cmdline: &str, cwd: &str) -> herdr::Process {
        herdr::Process {
            pid,
            name: name.into(),
            argv: argv.iter().map(|&a| a.into()).collect(),
            cmdline: cmdline.into(),
            cwd: Some(PathBuf::from(cwd)),
        }
    }

    fn info(shell: i32, group: i32, processes: Vec<herdr::Process>) -> herdr::ProcessInfo {
        herdr::ProcessInfo {
            shell_pid: Some(shell),
            foreground_process_group_id: Some(group),
            foreground_processes: processes,
        }
    }

    fn sample() -> Store {
        let layout = SavedNode::Split {
            dir: Dir::Right,
            ratio: 0.6,
            first: Box::new(SavedNode::Leaf(SavedLeaf::Zed)),
            second: Box::new(SavedNode::Split {
                dir: Dir::Down,
                ratio: 0.5,
                first: Box::new(SavedNode::Leaf(SavedLeaf::Herdr {
                    pane_id: "w1:p1".into(),
                })),
                second: Box::new(SavedNode::Leaf(SavedLeaf::App {
                    command: "firefox https://a.b".into(),
                })),
            }),
        };
        Store {
            version: VERSION,
            last_active: Some("api".into()),
            quit_choice: Some(QuitChoice::Stops(Stops {
                session: false,
                apps: true,
                t3: false,
            })),
            stop_picks: Some(Stops {
                session: true,
                apps: false,
                t3: false,
            }),
            cursor_history: History::default(),
            t3_hidden: Default::default(),
            t3_finished: Default::default(),
            ci_watched: Default::default(),
            repo_grouping: Grouping::Folders,
            sidebar_width: Some(280),
            window_size: Some((1600, 1000)),
            service_anchors: BTreeMap::new(),
            service_errors: Vec::new(),
            ignored_errors: Vec::new(),
            nav: {
                let mut nav = NavHistory::default();
                nav.visit(
                    Some(Visit::Parked {
                        project: "api".into(),
                        workspace: "w1".into(),
                        herdr_tab: Some("w1:t1".into()),
                        index: 0,
                        label: "api › 1 zsh".into(),
                    }),
                    |_| true,
                );
                nav
            },
            books: Books::default(),
            projects: vec![Project {
                id: "api".into(),
                name: "API".into(),
                session: session_name("api"),
                created: 100,
                last_used: 200,
                state: RunState::Stopped,
                workspaces: vec![WorkspaceRecord {
                    id: "w1".into(),
                    herdr_id: Some("w1".into()),
                    label: "services-api".into(),
                    kind: Kind::Project,
                    dir: "/work/services-api".into(),
                    venv: VenvSetting::Path("/work/services-api/.venv".into()),
                    tabs: vec![TabRecord {
                        herdr_tab: Some("w1:t1".into()),
                        layout,
                        label: None,
                    }],
                    panes: vec![PaneRecord {
                        pane_id: "w1:p1".into(),
                        tab: 0,
                        index: 1,
                        command: Some("npm run dev".into()),
                        cwd: Some("/work/services-api/web".into()),
                        env: vec![("VIRTUAL_ENV".into(), "/v".into())],
                        skip: vec!["rm -rf build".into()],
                    }],
                    folded: None,
                    restart_choices: Vec::new(),
                }],
                before_start: "docker compose up -d".into(),
                after_stop: "docker compose down".into(),
                tickets: Vec::new(),
                workflow: Default::default(),
            }],
        }
    }

    #[test]
    fn workspaces_take_the_order_given() {
        let mut store = sample();
        let workspaces = &mut store.projects[0].workspaces;
        for id in ["w2", "w3"] {
            workspaces.push(record(id.into(), id.into(), Kind::Project, "/w".into()));
        }
        assert!(!store.order_workspaces("api", &["w1", "w2", "w3"]));
        assert!(store.order_workspaces("api", &["w3", "w1"]));
        let ids: Vec<&str> = store.projects[0]
            .workspaces
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids, ["w3", "w1", "w2"]);
        assert!(!store.order_workspaces("api", &["w3", "w1"]));
    }

    #[test]
    fn store_round_trips() {
        let store = sample();
        let json = serde_json::to_string(&store).unwrap();
        assert_eq!(serde_json::from_str::<Store>(&json).unwrap(), store);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let old = json!({
            "version": 1,
            "projects": [{
                "id": "a",
                "name": "A",
                "session": "gerdr-a",
                "created": 1,
                "last_used": 2,
                "workspaces": [{
                    "herdr_id": "w1",
                    "label": "a",
                    "kind": "terminal",
                    "dir": "/a",
                    "panes": [{"pane_id": "w1:p1"}],
                }],
            }],
        });
        let store: Store = serde_json::from_value(old).unwrap();
        assert_eq!(store.last_active, None);
        assert_eq!(store.quit_choice, None);
        let project = store.project("a").unwrap();
        assert_eq!(project.state, RunState::Kept);
        assert_eq!(project.before_start, "");
        assert_eq!(project.after_stop, "");
        let ws = &project.workspaces[0];
        assert_eq!(ws.venv, VenvSetting::Auto);
        assert!(ws.tabs.is_empty());
        assert_eq!(
            ws.panes[0],
            PaneRecord {
                pane_id: "w1:p1".into(),
                ..PaneRecord::default()
            }
        );
    }

    #[test]
    fn records_get_ids_of_their_own() {
        let old = json!({
            "version": 1,
            "projects": [{
                "id": "a",
                "name": "A",
                "session": "gerdr-a",
                "created": 1,
                "last_used": 2,
                "workspaces": [
                    {"herdr_id": "w1", "label": "a", "kind": "terminal", "dir": "/a"},
                    {"label": "b", "kind": "project", "dir": "/b"},
                    {"id": "g7", "label": "c", "kind": "project", "dir": "/c"},
                ],
            }],
        });
        let mut store: Store = serde_json::from_value(old).unwrap();
        store
            .projects
            .iter_mut()
            .for_each(Project::fill_workspace_ids);
        let records = &store.project("a").unwrap().workspaces;
        let ids: Vec<_> = records
            .iter()
            .map(|r| (r.id.as_str(), r.herdr_id.as_deref()))
            .collect();
        assert_eq!(ids, [("w1", Some("w1")), ("g1", None), ("g7", None)]);
        assert_eq!(serde_json::to_value(Kind::Cursor).unwrap(), json!("cursor"));
        // Without a herdr workspace, it saves without one.
        let json = serde_json::to_value(&records[1]).unwrap();
        assert!(json.get("herdr_id").is_none());
        let back: WorkspaceRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, records[1]);
        assert_eq!(
            store
                .project("a")
                .unwrap()
                .herdr_record("w1")
                .map(|r| &r.id),
            Some(&"w1".to_owned())
        );
    }

    #[test]
    fn finds_records() {
        let mut store = sample();
        assert!(store.project("nope").is_none());
        store.project_mut("api").unwrap().name = "Api".into();
        assert_eq!(store.project("api").unwrap().name, "Api");
        store.record_mut("api", "w1").unwrap().venv = VenvSetting::Off;
        assert_eq!(store.projects[0].workspaces[0].venv, VenvSetting::Off);
        assert!(store.record_mut("api", "w2").is_none());
        assert!(store.record_mut("nope", "w1").is_none());
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Project", &[]), "my-project");
        assert_eq!(slugify("  --Services  API__v2!! ", &[]), "services-api-v2");
        assert_eq!(slugify("Café Ünïcode", &[]), "caf-n-code");
        assert_eq!(slugify("日本語", &[]), "project");
        assert_eq!(slugify("", &["project"]), "project-2");
        assert_eq!(slugify("api", &["api", "api-2"]), "api-3");
        assert_eq!(slugify("api", &["api-2"]), "api");
        assert_eq!(session_name("api"), "gerdr-api");
    }

    #[test]
    fn restart_command_honours_skip() {
        let mut pane = PaneRecord {
            command: Some("npm run dev".into()),
            ..PaneRecord::default()
        };
        assert_eq!(pane.restart_command(), Some("npm run dev"));
        pane.skip.push("cargo watch".into());
        assert_eq!(pane.restart_command(), Some("npm run dev"));
        pane.skip.push("npm run dev".into());
        assert_eq!(pane.restart_command(), None);
        pane.command = None;
        assert_eq!(pane.restart_command(), None);
        // Saved before such commands were left out.
        pane.command = Some("'/usr/bin/less' app.log".into());
        assert_eq!(pane.restart_command(), None);
        pane.command = Some("sudo dnf upgrade".into());
        assert_eq!(pane.restart_command(), None);
    }

    /// The command `fg` would run again.
    fn busy(fg: Option<Foreground>) -> Captured {
        match fg {
            Some(Foreground::Busy { captured, .. }) => captured,
            other => panic!("not busy: {other:?}"),
        }
    }

    #[test]
    fn idle_shell_runs_nothing() {
        let idle = info(
            NO_PID,
            NO_PID,
            vec![process(
                NO_PID,
                "zsh",
                &["/usr/bin/zsh"],
                "/usr/bin/zsh",
                "/w",
            )],
        );
        assert_eq!(foreground_command(&idle, None), Some(Foreground::Idle));
        // herdr can't tell: neither idle nor busy.
        assert_eq!(
            foreground_command(&herdr::ProcessInfo::default(), None),
            None
        );
        let unknown = herdr::ProcessInfo {
            foreground_process_group_id: None,
            ..info(NO_PID, NO_PID + 1, Vec::new())
        };
        assert_eq!(foreground_command(&unknown, None), None);
        assert_eq!(
            foreground_command(&info(NO_PID, NO_PID + 1, Vec::new()), None),
            None
        );
        // A shell started from the shell is no command to run again.
        let nested = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "-bash", &["-bash"], "-bash", "/w")],
        );
        assert_eq!(foreground_command(&nested, None), Some(Foreground::Idle));
        let herdr = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "herdr", &["herdr"], "herdr", "/w")],
        );
        assert_eq!(foreground_command(&herdr, None), Some(Foreground::Idle));
    }

    #[test]
    fn shell_running_a_script_is_busy() {
        let script = info(
            NO_PID,
            NO_PID + 1,
            vec![process(
                NO_PID + 1,
                "sh",
                &["sh", "runlocal.sh"],
                "sh runlocal.sh",
                "/w",
            )],
        );
        assert!(matches!(
            foreground_command(&script, None),
            Some(Foreground::Busy { captured, .. }) if captured.command == "sh runlocal.sh"
        ));
    }

    #[test]
    fn busy_pane_runs_its_leader() {
        let dir = "/home/me/services-api";
        let busy_info = info(
            NO_PID,
            NO_PID + 1,
            vec![process(
                NO_PID + 1,
                "sleep",
                &["sleep", "999"],
                "sleep 999",
                dir,
            )],
        );
        assert_eq!(
            foreground_command(&busy_info, Some(Path::new(dir))),
            Some(Foreground::Busy {
                pid: NO_PID + 1,
                captured: Captured {
                    command: "sleep 999".into(),
                    cwd: None,
                    env: Vec::new(),
                }
            })
        );
        // Elsewhere than the pane's folder, it runs there again.
        let moved = busy(foreground_command(&busy_info, Some(Path::new("/home/me"))));
        assert_eq!(moved.cwd, Some(PathBuf::from(dir)));
        assert_eq!(
            busy(foreground_command(&busy_info, None)).cwd,
            Some(dir.into())
        );

        let quoted = info(
            NO_PID,
            NO_PID + 1,
            vec![process(
                NO_PID + 1,
                "python",
                &["python", "-c", "print('hi there')", "a b"],
                "",
                dir,
            )],
        );
        assert_eq!(
            busy(foreground_command(&quoted, None)).command,
            r"python -c 'print('\''hi there'\'')' 'a b'"
        );
    }

    #[test]
    fn pipeline_runs_its_group_leader() {
        let pipeline = info(
            NO_PID,
            NO_PID + 2,
            vec![
                process(NO_PID + 3, "grep", &["grep", "x"], "grep x", "/w"),
                process(
                    NO_PID + 2,
                    "tail",
                    &["tail", "-f", "log"],
                    "tail -f log",
                    "/w",
                ),
            ],
        );
        assert_eq!(
            busy(foreground_command(&pipeline, None)).command,
            "tail -f log"
        );
        // No leader among them: the first one stands for the group.
        let leaderless = info(NO_PID, NO_PID + 9, pipeline.foreground_processes.clone());
        assert_eq!(
            busy(foreground_command(&leaderless, None)).command,
            "grep x"
        );
    }

    #[test]
    fn falls_back_to_a_one_word_cmdline() {
        let one = |cmdline: &str| {
            let fg = info(
                NO_PID,
                NO_PID + 1,
                vec![process(NO_PID + 1, "server", &[], cmdline, "/w")],
            );
            foreground_command(&fg, None)
        };
        assert_eq!(busy(one(" ./server ")).command, "./server");
        // Its spaces can't be told from those in an argument.
        let untracked = Some(Foreground::Untracked {
            pid: NO_PID + 1,
            denied: false,
        });
        assert_eq!(one("node server.js --port 3000"), untracked);
        assert_eq!(one(""), untracked);
        assert_eq!(one("gunicorn:"), untracked);
        assert_eq!(
            one("less notes.md"),
            Some(Foreground::Untracked {
                pid: NO_PID + 1,
                denied: true,
            })
        );
    }

    #[test]
    fn rewritten_process_titles_are_untracked() {
        let titled = |name: &str, argv: &[&str]| {
            let fg = info(
                NO_PID,
                NO_PID + 1,
                vec![process(NO_PID + 1, name, argv, "", "/w")],
            );
            foreground_command(&fg, None)
        };
        let untracked = Some(Foreground::Untracked {
            pid: NO_PID + 1,
            denied: false,
        });
        assert_eq!(
            titled("gunicorn", &["gunicorn: master [app.wsgi]"]),
            untracked
        );
        assert_eq!(
            titled(
                "celery",
                &["[celeryd: w1@host:MainProcess] -active- (worker)"]
            ),
            untracked
        );
        assert_eq!(titled("postgres", &["postgres: writer"]), untracked);
        assert!(usable_argv0("/usr/bin/gunicorn"));
        assert!(usable_argv0("./manage.py"));
        assert!(!usable_argv0(""));
        assert!(!usable_argv0("[kworker]"));
        assert!(!usable_argv0("nginx: master"));
        assert!(!usable_argv0("my app"));
    }

    #[test]
    fn npm_titles_are_read_back() {
        let titled = |argv: &[&str], cmdline: &str| {
            let fg = info(
                NO_PID,
                NO_PID + 1,
                vec![process(NO_PID + 1, "npm exec nx run", argv, cmdline, "/w")],
            );
            match foreground_command(&fg, None) {
                Some(Foreground::Busy { captured, .. }) => Some(captured.command),
                _ => None,
            }
        };
        let title = "npm exec nx run ao-app:serve:local";
        assert_eq!(
            titled(&[title], title).as_deref(),
            Some("npx nx run ao-app:serve:local")
        );
        // Known by the command line alone too.
        assert_eq!(
            titled(&[], title).as_deref(),
            Some("npx nx run ao-app:serve:local")
        );
        assert_eq!(
            titled(&["npm exec vite --port 3000"], "").as_deref(),
            Some("npx vite --port 3000")
        );
        assert_eq!(titled(&["npm run dev"], "").as_deref(), Some("npm run dev"));
        // `npm run dev -- --port 3000`: without its `--`, npm takes the flag.
        assert_eq!(titled(&["npm run dev --port 3000"], ""), None);
        assert_eq!(titled(&["npm exec"], ""), None);
        assert_eq!(titled(&["npm install"], ""), None);
        assert_eq!(titled(&["npm exec x https://***@host"], ""), None);
    }

    #[test]
    fn denies_what_must_not_run_again() {
        let no = |argv: &[&str]| denied(argv[0], argv);
        for argv in [
            &["less", "+F", "log"][..],
            &["/usr/bin/less", "log"],
            &["bat", "README.md"],
            &["ssh", "host"],
            &["git", "rebase", "-i"],
            &["rm", "-rf", "build"],
            &["sudo", "make", "install"],
            &["npm", "install"],
            &["npm", "i", "left-pad"],
            &["pnpm", "--silent", "add", "x"],
            &["bun", "publish"],
            &["cargo", "install", "ripgrep"],
            &["uv", "sync"],
            &["pip3", "install", "-r", "requirements.txt"],
            &["apt-get", "upgrade"],
            &["mise", "install"],
            &["python3"],
            &["python3.12", "-q"],
            &["node", "--inspect"],
            &["ipython"],
            // Scripts and modules run by an interpreter.
            &["python3", "/home/me/.local/bin/pip", "install", "x"],
            &["node", "/usr/lib/node_modules/.bin/npm", "install"],
            &["python", "-m", "pip", "install", "x"],
        ] {
            assert!(no(argv), "{argv:?}");
        }
        for argv in [
            &["npm", "run", "dev"][..],
            &["pnpm", "dev"],
            &["cargo", "run", "--release"],
            &["cargo", "watch", "-x", "test"],
            &["uv", "run", "manage.py", "runserver"],
            &["mise", "run", "dev"],
            &["python", "manage.py", "runserver"],
            &["python3", "-m", "http.server"],
            &["python", "-c", "print(1)"],
            &["node", "server.js"],
            &["watch", "-n1", "date"],
            &["celery", "-A", "app", "worker"],
            &["tail", "-f", "log"],
            &["gitk"],
            &["npm"],
            // Editors, full-screen tools and coding agents come back.
            &["nvim", "src/main.rs"],
            &["htop"],
            &["tmux", "attach"],
            &["claude", "--resume"],
            &["cursor-agent"],
            &["python3", "/home/me/.local/bin/aider", "--model", "x"],
            &["node", "/usr/lib/node_modules/.bin/codex"],
        ] {
            assert!(!no(argv), "{argv:?}");
        }
        // By the process name alone, too.
        assert!(denied("less", &["/opt/pager-wrapper"]));
        // But a name can't tell a script from a prompt.
        assert!(!denied("python3", &["[celeryd: x]"]));

        let denied_fg = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "less", &["less", "x.log"], "", "/w")],
        );
        assert_eq!(
            foreground_command(&denied_fg, None),
            Some(Foreground::Untracked {
                pid: NO_PID + 1,
                denied: true,
            })
        );
    }

    #[test]
    fn splits_quoted_commands() {
        assert_eq!(
            split_command(r"python -c 'print('\''hi there'\'')' 'a b'  c\ d"),
            ["python", "-c", "print('hi there')", "a b", "c d"]
        );
        assert!(split_command("  ").is_empty());
        assert_eq!(split_command("''"), [""]);
        assert!(denied_command("'less' x"));
        assert!(!denied_command("'npm' run dev"));
        assert!(!denied_command(""));
    }

    #[test]
    fn finds_the_project_of_a_process() {
        let mut store = sample();
        store.projects.push(Project {
            id: "old".into(),
            session: LEGACY.into(),
            ..store.projects[0].clone()
        });
        let socket = herdr::socket_path(&session_name("api"));
        let environ = |vars: &[&str]| {
            let mut raw = b"PATH=/usr/bin\0HOME=/home/me\0".to_vec();
            for var in vars {
                raw.extend_from_slice(var.as_bytes());
                raw.push(0);
            }
            raw
        };
        let api = format!("HERDR_SOCKET_PATH={}", socket.display());
        assert_eq!(
            store.project_of_environ(&environ(&[&api, "HERDR_SESSION=x"])),
            Some(Some("api"))
        );
        assert_eq!(
            store.project_of_environ(&environ(&["HERDR_SESSION=gerdr"])),
            Some(Some("old"))
        );
        // Another herdr session's: not one of gerdr's projects.
        assert_eq!(
            store.project_of_environ(&environ(&["HERDR_SOCKET_PATH=/tmp/herdr.sock"])),
            Some(None)
        );
        assert_eq!(store.project_of_environ(&environ(&[])), None);
        assert_eq!(store.project_of_environ(b""), None);
    }

    /// A folder of its own for a test, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `store`, kept under `root`.
    fn at(root: &Path, store: Store) -> Store {
        Store {
            books: Books {
                root: Some(root.to_owned()),
                ..Books::default()
            },
            ..store
        }
    }

    /// `sample` with a second project.
    fn two() -> Store {
        let mut store = sample();
        let web = Project {
            id: "web".into(),
            name: "Web".into(),
            session: session_name("web"),
            ..store.projects[0].clone()
        };
        store.projects.push(web);
        store
    }

    /// The names in folder `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn inode(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).unwrap().ino()
    }

    #[test]
    fn loading_says_what_it_found() {
        let root = scratch("load");
        let dir = root.join("projects");
        let none = || None;

        assert_eq!(
            Store::load_from(&root, none),
            (Store::empty(), Loaded::Empty)
        );
        let (migrated, loaded) = Store::load_from(&root, || Some(sample()));
        assert_eq!((migrated, loaded), (sample(), Loaded::Migrated));
        // On disk only once saved.
        assert!(!dir.exists());

        at(&root, two()).save();
        assert_eq!(names(&dir), ["api.json", "web.json"]);
        assert_eq!(
            std::fs::read(dir.join("api.json")).unwrap(),
            serde_json::to_vec_pretty(&two().projects[0]).unwrap()
        );
        assert_eq!(Store::load_from(&root, none), (two(), Loaded::File));

        // Leftovers and hidden files aren't projects.
        for name in ["x.json.tmp", "x.json.bad-1", ".x.json", "x.txt"] {
            std::fs::write(dir.join(name), b"{ not json").unwrap();
        }
        assert_eq!(Store::load_from(&root, none), (two(), Loaded::File));
        assert_eq!(names(&dir).len(), 6);

        // Unreadable: set aside, the rest loaded.
        std::fs::write(dir.join("web.json"), b"{ not json").unwrap();
        let (store, loaded) = Store::load_from(&root, none);
        assert_eq!((store, loaded), (sample(), Loaded::File));
        let set_aside: Vec<_> = names(&dir)
            .into_iter()
            .filter(|name| name.starts_with("web.json.bad-"))
            .collect();
        assert_eq!(set_aside.len(), 1);
        assert!(!dir.join("web.json").exists());
        assert_eq!(
            std::fs::read(dir.join(&set_aside[0])).unwrap(),
            b"{ not json"
        );

        // So is `state.json`, and the rest comes back as it was.
        std::fs::write(root.join("state.json"), b"[]").unwrap();
        let (store, loaded) = Store::load_from(&root, none);
        assert_eq!(loaded, Loaded::File);
        assert_eq!((store.last_active, store.quit_choice), (None, None));
        assert_eq!(store.projects, sample().projects);
        assert!(!root.join("state.json").exists());
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(
            bad_name(Path::new("/d/projects/api.json"), 42),
            Path::new("/d/projects/api.json.bad-42")
        );
    }

    #[test]
    fn migrates_the_old_single_file() {
        let root = scratch("migrate");
        let dir = root.join("projects");
        let legacy = root.join("projects.json");
        let old = serde_json::to_vec_pretty(&two()).unwrap();
        std::fs::write(&legacy, &old).unwrap();

        let (mut store, loaded) = Store::load_from(&root, || None);
        assert_eq!((&store, loaded), (&two(), Loaded::Migrated));
        assert_eq!(names(&dir), ["api.json", "web.json"]);
        assert!(!legacy.exists());
        assert_eq!(
            std::fs::read(root.join("projects.json.migrated")).unwrap(),
            old
        );
        // Saving it again, as a migrated store is, changes nothing.
        let files = ["projects/api.json", "projects/web.json", "state.json"];
        let before: Vec<_> = files.iter().map(|f| inode(&root.join(f))).collect();
        store.save();
        let after: Vec<_> = files.iter().map(|f| inode(&root.join(f))).collect();
        assert_eq!(before, after);
        assert_eq!(Store::load_from(&root, || None), (two(), Loaded::File));
        let _ = std::fs::remove_dir_all(&root);

        // A project that can't have a file: the old file stays.
        let root = scratch("migrate-bad-id");
        let mut bad = sample();
        bad.projects[0].id = "../x".into();
        let old = serde_json::to_vec(&bad).unwrap();
        std::fs::write(root.join("projects.json"), &old).unwrap();
        let (store, loaded) = Store::load_from(&root, || None);
        assert_eq!((store, loaded), (bad, Loaded::Migrated));
        assert_eq!(std::fs::read(root.join("projects.json")).unwrap(), old);
        assert!(!root.join("projects").exists());
        assert!(!root.join("x.json").exists());
        let _ = std::fs::remove_dir_all(&root);

        // Unreadable: set aside, nothing migrated.
        let root = scratch("migrate-broken");
        std::fs::write(root.join("projects.json"), b"{ not json").unwrap();
        let (store, loaded) = Store::load_from(&root, || Some(sample()));
        assert_eq!((store, loaded), (Store::empty(), Loaded::Failed));
        assert!(!root.join("projects").exists());
        let left = names(&root);
        assert_eq!(left.len(), 1);
        assert!(left[0].starts_with("projects.json.bad-"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_must_be_its_projects() {
        for id in ["api", "gerdr", "project-2", "2fa"] {
            assert!(valid_id(id), "{id}");
        }
        for id in [
            "", "-api", "Api", "../x", "a/b", "a b", "a_b", "café", "a.json",
        ] {
            assert!(!valid_id(id), "{id}");
        }

        let root = scratch("ids");
        let dir = root.join("projects");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, id: &str| {
            let project = Project {
                id: id.into(),
                ..sample().projects[0].clone()
            };
            let path = dir.join(name);
            std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
            path
        };
        let web = write("web.json", "api");
        assert_eq!(
            read_project("web", &web).unwrap_err(),
            "id \"api\" doesn't match the file name"
        );
        let up = write("up.json", "../up");
        assert_eq!(
            read_project("up", &up).unwrap_err(),
            "\"../up\" isn't a valid id"
        );
        let caps = write("Api.json", "Api");
        assert_eq!(
            read_project("Api", &caps).unwrap_err(),
            "\"Api\" isn't a valid id"
        );
        let (store, loaded) = Store::load_from(&root, || None);
        assert_eq!(loaded, Loaded::File);
        assert!(store.projects.is_empty());
        assert!(names(&dir).iter().all(|name| name.contains(".json.bad-")));

        // Never written out, wherever it would land.
        let mut store = at(&root, sample());
        store.projects[0].id = "../escaped".into();
        assert!(!store.write());
        assert!(!root.join("escaped.json").exists());
        assert_eq!(names(&dir).len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_store_that_cant_be_set_aside_is_never_saved_over() {
        // Root moves it anyway.
        let root_user = unsafe { libc::geteuid() } == 0;
        let mode = |dir: &Path, mode| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        };

        // A project file that can't be read (a folder), nor moved: frozen.
        let root = scratch("frozen");
        let dir = root.join("projects");
        let api = dir.join("api.json");
        std::fs::create_dir_all(api.join("x")).unwrap();
        mode(&dir, 0o500);
        let (mut store, loaded) = Store::load_from(&root, || None);
        mode(&dir, 0o700);
        assert_eq!(loaded, Loaded::File);
        assert_eq!(store.books.frozen.contains("api"), !root_user);
        store.projects = sample().projects;
        store.save();
        assert_eq!(api.is_dir(), !root_user);
        store.projects.clear();
        store.save();
        assert_eq!(api.is_dir(), !root_user);
        let _ = std::fs::remove_dir_all(&root);

        // The old single file, likewise: nothing saved, not even beside it.
        let root = scratch("frozen-legacy");
        std::fs::create_dir_all(root.join("projects.json/x")).unwrap();
        mode(&root, 0o500);
        let (mut store, loaded) = Store::load_from(&root, || None);
        mode(&root, 0o700);
        assert_eq!(loaded, Loaded::Failed);
        assert_eq!(store.books.legacy_frozen, !root_user);
        store.projects = sample().projects;
        store.save();
        assert_eq!(root.join("projects").exists(), root_user);
        assert!(!set_aside(&root.join("nowhere/api.json")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edit_made_outside_gerdr_isnt_saved_over() {
        let root = scratch("edited");
        let api = root.join("projects/api.json");
        let web = root.join("projects/web.json");
        let mut store = at(&root, two());
        store.save();
        assert!(store.take_conflicts().is_empty());

        let edited = std::fs::read_to_string(&api)
            .unwrap()
            .replace("\"API\"", "\"Site\"");
        std::fs::write(&api, &edited).unwrap();
        for project in &mut store.projects {
            project.last_used = 300;
        }
        store.save();
        // Only that project's saves wait.
        assert_eq!(std::fs::read_to_string(&api).unwrap(), edited);
        assert_eq!(read_project("web", &web).unwrap().0.last_used, 300);
        assert_eq!(store.take_conflicts(), ["api"]);
        // Said once.
        store.save();
        assert!(store.take_conflicts().is_empty());

        // Reloaded: saved again, from what was read.
        let Reread {
            store: mut reloaded,
            errors,
        } = store.reread();
        assert!(errors.is_empty());
        assert_eq!(reloaded.projects[0].name, "Site");
        assert_eq!(reloaded.projects[0].last_used, 200);
        reloaded.projects[0].last_used = 400;
        reloaded.save();
        assert!(reloaded.take_conflicts().is_empty());
        assert_eq!(read_project("api", &api).unwrap().0.last_used, 400);

        // Put back as gerdr wrote it: saves go on.
        let ours = std::fs::read(&api).unwrap();
        std::fs::write(&api, b"{}").unwrap();
        reloaded.projects[0].last_used = 500;
        reloaded.save();
        assert_eq!(reloaded.take_conflicts(), ["api"]);
        std::fs::write(&api, &ours).unwrap();
        reloaded.save();
        assert!(reloaded.take_conflicts().is_empty());
        assert_eq!(read_project("api", &api).unwrap().0.last_used, 500);

        // Deleted: nothing to lose.
        std::fs::remove_file(&api).unwrap();
        reloaded.projects[0].last_used = 600;
        reloaded.save();
        assert_eq!(read_project("api", &api).unwrap().0.last_used, 600);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unchanged_project_file_isnt_rewritten() {
        let root = scratch("unchanged");
        let api = root.join("projects/api.json");
        let web = root.join("projects/web.json");
        let state = root.join("state.json");
        let mut store = at(&root, two());
        store.save();
        let (api_was, web_was, state_was) = (inode(&api), inode(&web), inode(&state));

        store.projects[1].last_used = 300;
        store.save();
        assert_eq!(inode(&api), api_was);
        assert_ne!(inode(&web), web_was);
        assert_eq!(inode(&state), state_was);

        let web_was = inode(&web);
        store.last_active = Some("web".into());
        store.save();
        assert_eq!((inode(&api), inode(&web)), (api_was, web_was));
        assert_ne!(inode(&state), state_was);
        let (store, _) = Store::load_from(&root, || None);
        assert_eq!(store.last_active.as_deref(), Some("web"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_forgotten_projects_file_is_removed() {
        let root = scratch("forgotten");
        let web = root.join("projects/web.json");
        let mut store = at(&root, two());
        store.save();
        let gone = store.projects.pop().unwrap();
        store.save();
        assert!(!web.exists());
        assert_eq!(names(&root.join("projects")), ["api.json"]);

        // Unless edited outside gerdr.
        store.projects.push(gone.clone());
        store.save();
        std::fs::write(&web, b"{ edited").unwrap();
        store.projects.pop();
        store.save();
        assert_eq!(std::fs::read(&web).unwrap(), b"{ edited");
        // Nor saved over when it comes back.
        store.projects.push(gone);
        store.save();
        assert_eq!(std::fs::read(&web).unwrap(), b"{ edited");
        assert_eq!(store.take_conflicts(), ["web"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rereading_a_broken_file_leaves_it_alone() {
        let root = scratch("broken");
        let dir = root.join("projects");
        let api = dir.join("api.json");
        let mut store = at(&root, two());
        store.save();
        std::fs::write(&api, b"{ not json").unwrap();

        let Reread {
            store: mut reread,
            errors,
        } = store.reread();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("api.json: "), "{errors:?}");
        assert!(errors[0].contains("line 1"), "{errors:?}");
        // Its project as it was, its saves paused; the rest saves on.
        assert_eq!(reread, two());
        assert_eq!(std::fs::read(&api).unwrap(), b"{ not json");
        reread.projects[0].last_used = 300;
        reread.projects[1].last_used = 300;
        reread.save();
        assert_eq!(std::fs::read(&api).unwrap(), b"{ not json");
        assert_eq!(
            read_project("web", &dir.join("web.json"))
                .unwrap()
                .0
                .last_used,
            300
        );
        // Already told.
        assert!(reread.take_conflicts().is_empty());

        // A file deleted: its project is gone.
        std::fs::remove_file(dir.join("web.json")).unwrap();
        let Reread { store: gone, .. } = store.reread();
        assert_eq!(gone.projects.len(), 1);
        assert_eq!(gone.last_active, store.last_active);

        // The whole folder gone: nothing changes.
        std::fs::remove_dir_all(&dir).unwrap();
        let Reread {
            store: kept,
            errors,
        } = store.reread();
        assert_eq!(kept, store);
        assert_eq!(errors.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keeps_worthwhile_environment() {
        let raw = b"PATH=/usr/bin\0VIRTUAL_ENV=/w/.venv\0MISE_NODE_VERSION=20\0MISE_ENV=dev\0\
MISE_VERSION=x\0MISE_PYTHON_VERSION=3.12=rc\0HOME=/home/me\0garbage\0";
        assert_eq!(
            environ_whitelist(raw),
            [
                ("MISE_ENV".to_owned(), "dev".to_owned()),
                ("MISE_NODE_VERSION".to_owned(), "20".to_owned()),
                ("MISE_PYTHON_VERSION".to_owned(), "3.12=rc".to_owned()),
                ("VIRTUAL_ENV".to_owned(), "/w/.venv".to_owned()),
            ]
        );
        assert!(environ_whitelist(b"").is_empty());
    }

    #[test]
    fn ago_buckets() {
        let now = 10_000_000;
        let cases = [
            (0, "just now"),
            (59, "just now"),
            (60, "1m ago"),
            (59 * 60, "59m ago"),
            (3600, "1h ago"),
            (23 * 3600, "23h ago"),
            (24 * 3600, "yesterday"),
            (47 * 3600, "yesterday"),
            (2 * 86400, "2d ago"),
            (6 * 86400, "6d ago"),
            (7 * 86400, "1w ago"),
            (34 * 86400, "4w ago"),
            (35 * 86400, "1mo ago"),
            (90 * 86400, "3mo ago"),
        ];
        for (secs, text) in cases {
            assert_eq!(ago(now, now - secs), text, "{secs}s");
        }
        assert_eq!(ago(now, now + 500), "just now");
    }

    #[test]
    fn greetings() {
        assert_eq!(greeting(4), "Burning the midnight oil");
        assert_eq!(greeting(5), "Good morning");
        assert_eq!(greeting(11), "Good morning");
        assert_eq!(greeting(12), "Good afternoon");
        assert_eq!(greeting(17), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(greeting(22), "Good evening");
        assert_eq!(greeting(23), "Burning the midnight oil");
        assert_eq!(greeting(0), "Burning the midnight oil");
        assert!(local_hour() < 24);
        assert!(now() > 1_700_000_000);
    }

    #[test]
    fn switching_says_the_old_project_keeps_running() {
        assert_eq!(
            switch_notice("gerdr", "aft", false),
            "Switched to gerdr · aft keeps running"
        );
    }

    #[test]
    fn migrates_saved_session() {
        let saved = json!({
            "version": 3,
            "workspaces": [
                {"id": "w1", "custom_name": "aft", "identity_cwd": "/home/me/ao-app/aft-backend"},
                {"id": "w2", "custom_name": null, "identity_cwd": "/home/me/gerdr"},
                {"id": "w3", "custom_name": "no folder"},
            ],
        })
        .to_string();
        let project = migrate(None, Some(&saved)).unwrap();
        assert_eq!(
            (
                project.id.as_str(),
                project.name.as_str(),
                project.session.as_str()
            ),
            ("gerdr", "gerdr", "gerdr")
        );
        assert_eq!(project.state, RunState::Kept);
        assert_eq!(project.created, project.last_used);
        assert_eq!(
            project.workspaces,
            [
                record(
                    "w1".into(),
                    "aft".into(),
                    Kind::Project,
                    "/home/me/ao-app/aft-backend".into()
                ),
                record(
                    "w2".into(),
                    "gerdr".into(),
                    Kind::Project,
                    "/home/me/gerdr".into()
                ),
            ]
        );
        assert_eq!(
            migrate(None, Some(r#"{"version":3,"workspaces":[]}"#)),
            None
        );
        assert_eq!(migrate(None, Some("not json")), None);
        assert_eq!(migrate(None, None), None);
    }

    #[test]
    fn migrates_live_session() {
        let live: Vec<herdr::WorkspaceInfo> = serde_json::from_value(json!([
            {"workspace_id": "w1", "label": "api", "tokens": {
                "gerdr_kind": "project", "gerdr_dir_0": "/work/serv", "gerdr_dir_1": "ices-api",
            }},
            {"workspace_id": "w2", "label": "shell", "tokens": {
                "gerdr_kind": "terminal", "gerdr_dir_0": "/home/me",
            }},
            {"workspace_id": "w3", "label": "someone else's"},
            {"workspace_id": "w4", "label": "no folder", "tokens": {"gerdr_kind": "project"}},
        ]))
        .unwrap();
        let saved = r#"{"workspaces":[{"id":"w9","identity_cwd":"/x"}]}"#;
        let project = migrate(Some(&live), Some(saved)).unwrap();
        assert_eq!(
            project.workspaces,
            [
                record(
                    "w1".into(),
                    "api".into(),
                    Kind::Project,
                    "/work/services-api".into()
                ),
                record(
                    "w2".into(),
                    "shell".into(),
                    Kind::Terminal,
                    "/home/me".into()
                ),
            ]
        );
        // Nothing of gerdr's running: what was saved.
        let project = migrate(Some(&live[2..]), Some(saved)).unwrap();
        assert_eq!(project.workspaces[0].herdr_id.as_deref(), Some("w9"));
        assert_eq!(migrate(Some(&[]), None), None);
    }

    #[test]
    fn quit_choices_before_picks_still_read() {
        let read = |text: &str| serde_json::from_str::<QuitChoice>(text).unwrap().stops();
        assert_eq!(read(r#""keep""#), Stops::default());
        assert_eq!(read(r#""stop""#), Stops::ALL);
        assert_eq!(
            read(r#"{"stops":{"apps":true}}"#),
            Stops {
                session: false,
                apps: true,
                t3: false,
            }
        );
    }
}
