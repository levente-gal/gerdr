//! Projects: a named set of workspaces (usually one per repository) that
//! lives in a herdr session of its own, so it can keep running while gerdr is
//! closed and come back after a reboot. herdr restores the layout and the
//! folders by itself; what ran in the panes, the venvs and the apps gerdr
//! launched outside herdr are remembered here, in
//! `~/.local/share/gerdr/projects.json`.

#![allow(dead_code)] // contract: implemented by the project store work

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{herdr, layout::SavedNode};

/// Everything gerdr remembers about the projects.
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
    #[serde(default)]
    pub projects: Vec<Project>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitChoice {
    /// Keep the project's session running in the background.
    Keep,
    /// Stop the project's session.
    Stop,
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
}

/// What gerdr kept of a workspace of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// The herdr workspace id (`w6`): stable across a server restart.
    pub herdr_id: String,
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
}

/// What a workspace opens with: mirrors `WorkspaceKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Zed over a terminal.
    Project,
    Terminal,
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
    /// The saved store; on first run, the session gerdr used before it had
    /// projects (see `migrate`); else an empty one.
    pub fn load() -> Self {
        todo!("project store")
    }

    /// Write it out atomically (temporary file, then rename).
    pub fn save(&self) {
        todo!("project store")
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        let _ = id;
        todo!("project store")
    }

    pub fn project_mut(&mut self, id: &str) -> Option<&mut Project> {
        let _ = id;
        todo!("project store")
    }

    /// Project `project`'s record of herdr workspace `herdr_id`.
    pub fn record_mut(&mut self, project: &str, herdr_id: &str) -> Option<&mut WorkspaceRecord> {
        let _ = (project, herdr_id);
        todo!("project store")
    }
}

impl PaneRecord {
    /// The command to run when the pane comes back, unless the user said not
    /// to.
    pub fn restart_command(&self) -> Option<&str> {
        todo!("project store")
    }
}

/// A unique id for a project called `name`: lowercase `[a-z0-9-]`, with
/// `-2`, `-3`, … added if `taken` has it already.
pub fn slugify(name: &str, taken: &[&str]) -> String {
    let _ = (name, taken);
    todo!("project store")
}

/// The herdr session of a new project: `gerdr-<id>`, apart from the user's
/// own sessions.
pub fn session_name(id: &str) -> String {
    let _ = id;
    todo!("project store")
}

/// What runs in the foreground of a pane, if anything but its idle shell:
/// the process group leader's command line, where it runs (if not in
/// `pane_cwd`), and its environment worth restoring (from `/proc`).
pub fn foreground_command(info: &herdr::ProcessInfo, pane_cwd: Option<&Path>) -> Option<Captured> {
    let _ = (info, pane_cwd);
    todo!("project store")
}

/// The variables of a raw `/proc/<pid>/environ` worth restoring.
pub fn environ_whitelist(raw: &[u8]) -> Vec<(String, String)> {
    let _ = raw;
    todo!("project store")
}

/// How long ago `then` was, for people: "just now", "5m ago", "3h ago",
/// "yesterday", "4d ago", …
pub fn ago(now: u64, then: u64) -> String {
    let _ = (now, then);
    todo!("project store")
}

/// The greeting for the local `hour` (0–23).
pub fn greeting(hour: u32) -> &'static str {
    let _ = hour;
    todo!("project store")
}

/// The local hour (0–23).
pub fn local_hour() -> u32 {
    todo!("project store")
}

/// Unix seconds now.
pub fn now() -> u64 {
    todo!("project store")
}

/// The project made of the session gerdr used before it had projects
/// (`gerdr`): its workspaces from the running server's metadata (`live`),
/// else from its saved `session.json`. `None` if neither has any.
pub fn migrate(
    live: Option<&[herdr::WorkspaceInfo]>,
    session_json: Option<&str>,
) -> Option<Project> {
    let _ = (live, session_json);
    todo!("project store")
}
