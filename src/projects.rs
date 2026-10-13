//! Projects: a named set of workspaces (usually one per repository) that
//! lives in a herdr session of its own, so it can keep running while gerdr is
//! closed and come back after a reboot. herdr restores the layout and the
//! folders by itself; what ran in the panes, the venvs and the apps gerdr
//! launched outside herdr are remembered here, in
//! `~/.local/share/gerdr/projects.json`.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{desktop::shell_quote, herdr, home_dir, layout::SavedNode};

/// The store's format.
const VERSION: u32 = 1;
/// The session gerdr used before it had projects, and the project made of it.
const LEGACY: &str = "gerdr";
/// What a pane's idle shell may be called: with one of these leading the
/// foreground process group, nothing worth running again runs.
const SHELLS: [&str; 7] = ["sh", "bash", "zsh", "fish", "dash", "nu", "herdr"];

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
    /// The file couldn't be read nor set aside: saving would lose it.
    #[serde(skip)]
    frozen: bool,
}

/// What `Store::load` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// The saved store.
    File,
    /// No store yet: the session gerdr used before it had projects, made a
    /// project. On disk only once saved.
    Migrated,
    /// No store yet, and nothing to make one of.
    Empty,
    /// A store that can't be read: set aside as `projects.json.bad-<unix
    /// time>` for the user to rescue, or, if even that failed, left where
    /// it is and never saved over.
    Failed,
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
    /// projects (see `migrate`); else an empty one. Says which.
    pub fn load() -> (Self, Loaded) {
        Self::load_from(&store_file(), Self::first_run)
    }

    fn load_from(path: &Path, first_run: impl FnOnce() -> Option<Self>) -> (Self, Loaded) {
        let failed = |err: &dyn std::fmt::Display| {
            tracing::warn!("reading {}: {err}", path.display());
            let store = Self {
                frozen: !set_aside(path),
                ..Self::empty()
            };
            (store, Loaded::Failed)
        };
        match std::fs::read(path) {
            Ok(raw) => match serde_json::from_slice(&raw) {
                Ok(store) => (store, Loaded::File),
                Err(err) => failed(&err),
            },
            Err(err) if err.kind() == ErrorKind::NotFound => match first_run() {
                Some(store) => (store, Loaded::Migrated),
                None => (Self::empty(), Loaded::Empty),
            },
            Err(err) => failed(&err),
        }
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

    /// Write it out atomically (temporary file, then rename).
    pub fn save(&self) {
        let path = store_file();
        if self.frozen {
            return tracing::warn!("not saving over the unreadable {}", path.display());
        }
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                path.parent().map_or(Ok(()), std::fs::create_dir_all)?;
                std::fs::write(&tmp, json)?;
                std::fs::rename(&tmp, &path)
            });
        if let Err(err) = written {
            tracing::warn!("saving {}: {err}", path.display());
        }
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn project_mut(&mut self, id: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.id == id)
    }

    /// Project `project`'s record of herdr workspace `herdr_id`.
    pub fn record_mut(&mut self, project: &str, herdr_id: &str) -> Option<&mut WorkspaceRecord> {
        self.project_mut(project)?
            .workspaces
            .iter_mut()
            .find(|w| w.herdr_id == herdr_id)
    }
}

fn store_file() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/projects.json")
}

/// Move the unreadable store at `path` out of the way of the next save, as
/// `<path>.bad-<unix time>`; whether it is out of the way.
fn set_aside(path: &Path) -> bool {
    let bad = bad_name(path, now());
    match std::fs::rename(path, &bad) {
        Ok(()) => {
            tracing::warn!(
                "starting without projects; the old store is {}",
                bad.display()
            );
            true
        }
        Err(err) => {
            tracing::warn!(
                "setting {} aside: {err}; it won't be saved over",
                path.display()
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

/// Programs never run again after a restart, by name: they edit, page,
/// reach other machines, change files or the system, take over the
/// terminal, or are better started by hand.
const DENY: &[(&[&str], Deny)] = &[
    // Editors.
    (
        &[
            "vim", "nvim", "vi", "nano", "emacs", "hx", "helix", "micro", "kak",
        ],
        Deny::Always,
    ),
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
    // Full-screen tools and multiplexers.
    (
        &[
            "top", "htop", "btop", "yazi", "ranger", "nnn", "lf", "tmux", "zellij", "herdr",
            "screen",
        ],
        Deny::Always,
    ),
    // Coding agents.
    (
        &[
            "claude",
            "codex",
            "opencode",
            "aider",
            "gemini",
            "goose",
            "cursor-agent",
            "amp",
        ],
        Deny::Always,
    ),
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

/// Interpreters whose script names the program (`python3 ~/.local/bin/aider`).
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
    // Login shells show up as `-zsh`.
    if SHELLS.contains(&leader.name.trim_start_matches('-')) {
        return Some(Foreground::Idle);
    }
    let pid = leader.pid;
    let untracked = |denied| Some(Foreground::Untracked { pid, denied });
    // Without argv, the command line is trusted only as a single word: its
    // spaces can't be told from those inside an argument.
    let cmdline = leader.cmdline.trim();
    let argv: Vec<&str> = if leader.argv.is_empty() {
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

/// The notice on switching to project `new` from project `old`, which keeps
/// running.
pub fn switch_notice(new: &str, old: &str) -> String {
    format!("Switched to {new} · {old} keeps running")
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
    })
}

fn record(herdr_id: String, label: String, kind: Kind, dir: PathBuf) -> WorkspaceRecord {
    WorkspaceRecord {
        herdr_id,
        label,
        kind,
        dir,
        venv: VenvSetting::Auto,
        tabs: Vec::new(),
        panes: Vec::new(),
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
            quit_choice: Some(QuitChoice::Keep),
            frozen: false,
            projects: vec![Project {
                id: "api".into(),
                name: "API".into(),
                session: session_name("api"),
                created: 100,
                last_used: 200,
                state: RunState::Stopped,
                workspaces: vec![WorkspaceRecord {
                    herdr_id: "w1".into(),
                    label: "services-api".into(),
                    kind: Kind::Project,
                    dir: "/work/services-api".into(),
                    venv: VenvSetting::Path("/work/services-api/.venv".into()),
                    tabs: vec![TabRecord {
                        herdr_tab: Some("w1:t1".into()),
                        layout,
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
                }],
            }],
        }
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
        pane.command = Some("'/usr/bin/nvim' main.rs".into());
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
            one("vim notes.md"),
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
    fn denies_what_must_not_run_again() {
        let no = |argv: &[&str]| denied(argv[0], argv);
        for argv in [
            &["nvim", "src/main.rs"][..],
            &["/usr/bin/vim"],
            &["less", "+F", "log"],
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
            &["htop"],
            &["tmux", "attach"],
            &["claude", "--resume"],
            &["cursor-agent"],
            &["python3"],
            &["python3.12", "-q"],
            &["node", "--inspect"],
            &["ipython"],
            // Scripts and modules run by an interpreter.
            &["python3", "/home/me/.local/bin/aider", "--model", "x"],
            &["node", "/usr/lib/node_modules/.bin/codex"],
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
        ] {
            assert!(!no(argv), "{argv:?}");
        }
        // By the process name alone, too.
        assert!(denied("nvim", &["/opt/editor-wrapper"]));
        // But a name can't tell a script from a prompt.
        assert!(!denied("python3", &["[celeryd: x]"]));

        let denied_fg = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "nvim", &["nvim", "x.rs"], "", "/w")],
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
        assert!(denied_command("'nvim' x"));
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

    #[test]
    fn loading_says_what_it_found() {
        let dir = scratch("load");
        let path = dir.join("projects.json");
        let none = || None;

        assert_eq!(
            Store::load_from(&path, none),
            (Store::empty(), Loaded::Empty)
        );
        let (migrated, loaded) = Store::load_from(&path, || Some(sample()));
        assert_eq!((migrated, loaded), (sample(), Loaded::Migrated));

        std::fs::write(&path, serde_json::to_vec(&sample()).unwrap()).unwrap();
        assert_eq!(Store::load_from(&path, none), (sample(), Loaded::File));

        // Unreadable: set aside, not saved over.
        std::fs::write(&path, b"{ not json").unwrap();
        let (store, loaded) = Store::load_from(&path, none);
        assert_eq!((loaded, store.frozen), (Loaded::Failed, false));
        assert!(store.projects.is_empty());
        assert!(!path.exists());
        let set_aside: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(set_aside.len(), 1);
        assert!(set_aside[0].starts_with("projects.json.bad-"));
        assert_eq!(
            std::fs::read(dir.join(&set_aside[0])).unwrap(),
            b"{ not json"
        );
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            bad_name(Path::new("/d/projects.json"), 42),
            Path::new("/d/projects.json.bad-42")
        );
    }

    #[test]
    fn a_store_that_cant_be_set_aside_is_never_saved_over() {
        // Can't be read (a folder), and can't be moved: frozen.
        let dir = scratch("frozen");
        let path = dir.join("projects.json");
        std::fs::create_dir_all(path.join("x")).unwrap();
        let readonly = |mode| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        readonly(0o500);
        let (store, loaded) = Store::load_from(&path, || None);
        readonly(0o700);
        assert_eq!(loaded, Loaded::Failed);
        // Root moves it anyway.
        let root = unsafe { libc::geteuid() } == 0;
        assert_eq!(store.frozen, !root);
        assert!(!set_aside(&dir.join("nowhere/projects.json")));
        let _ = std::fs::remove_dir_all(&dir);
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
            switch_notice("gerdr", "aft"),
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
        assert_eq!(project.workspaces[0].herdr_id, "w9");
        assert_eq!(migrate(Some(&[]), None), None);
    }
}
