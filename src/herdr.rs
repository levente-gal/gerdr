//! herdr as the terminal backend: gerdr keeps its terminals in a herdr session
//! of its own, so shells and servers outlive gerdr. Each terminal pane is a
//! herdr pane shown through `herdr terminal attach` in an Alacritty window.
//!
//! The socket API speaks one JSON object per line: `{"id", "method",
//! "params"}` in, `{"id", "result"}` or `{"id", "error"}` back. A second
//! connection subscribes to events, so changes made from the herdr TUI or CLI
//! show up in gerdr too.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{home_dir, layout::Dir, state_dir};

/// Who gerdr's workspace metadata is reported as.
const SOURCE: &str = "gerdr";
/// How long a request may take before gerdr gives up on it.
const TIMEOUT: Duration = Duration::from_secs(3);
/// How long a ping may take when only checking whether a server runs.
const PROBE: Duration = Duration::from_millis(300);
/// How long a server gerdr started may take to answer.
const STARTUP: Duration = Duration::from_secs(5);
/// A request that takes longer is logged: it may hold up the compositor.
const SLOW_CALL: Duration = Duration::from_millis(10);
/// How long after typing a line to an agent to press Enter. Any gap did on
/// Cursor when probed; this one leaves room for a busy machine.
const ENTER_AFTER: Duration = Duration::from_millis(50);
/// herdr's caller context, inherited when gerdr itself runs inside herdr; it
/// must not leak into gerdr's children.
const INHERITED: [&str; 7] = [
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_ENV",
    "HERDR_WORKSPACE_ID",
    "HERDR_TAB_ID",
    "HERDR_PANE_ID",
];
/// Workspace metadata tokens: what kind of gerdr workspace it is, and its
/// folder, in pieces `gerdr_dir_0`, `gerdr_dir_1`, … as herdr caps their size.
const KIND_TOKEN: &str = "gerdr_kind";
const DIR_TOKEN: &str = "gerdr_dir_";
const TOKEN_MAX: usize = 80;

/// The most bytes a socket's path may have for gerdr to use it: a Unix
/// socket path is capped at 108 bytes (`sun_path`, with its NUL), and the
/// server's session folder holds a `herdr-client.sock` too.
const SOCKET_PATH_MAX: usize = 100;
/// The log a server started by gerdr writes to, in gerdr's state folder.
const SERVER_LOG: &str = "herdr-server.log";
/// A server log bigger than this is started over.
const SERVER_LOG_MAX: u64 = 1 << 20;

/// herdr's config folder's parent: `$XDG_CONFIG_HOME`, by default
/// `~/.config`. herdr keeps its sessions under it.
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home_dir().unwrap_or_default().join(".config"))
}

/// `$XDG_RUNTIME_DIR`, the user's own private folder for sockets; none
/// without it (a world-writable temp folder is no place for a config
/// link another user could plant first).
pub(crate) fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

/// The socket of `session` under the config folder `config`, as the
/// server puts it (herdr 0.9 takes no socket path: `HERDR_SOCKET_PATH` is
/// read by its clients only, so the server's can only be moved by giving
/// it another `XDG_CONFIG_HOME`).
fn socket_under(config: &Path, session: &str) -> PathBuf {
    config
        .join("herdr/sessions")
        .join(session)
        .join("herdr.sock")
}

/// The config folder `session`'s server is told, and gerdr reaches its
/// socket through: the real one, or when the socket's path under it would
/// be too long (a deep home folder), `<runtime>/gerdr/config`, a short
/// symlink to the real one, so herdr still finds the same config and
/// sessions. With whether it is the short one. Without a runtime folder,
/// or with one too deep itself, the real folder is kept and the server
/// is left to fail with its own words.
fn pick_config_home(session: &str, config: &Path, runtime: Option<&Path>) -> (PathBuf, bool) {
    let fits = |dir: &Path| socket_under(dir, session).as_os_str().len() <= SOCKET_PATH_MAX;
    if fits(config) {
        return (config.to_path_buf(), false);
    }
    match runtime.map(|runtime| runtime.join("gerdr/config")) {
        Some(short) if fits(&short) => (short, true),
        Some(short) => {
            tracing::warn!(
                "the herdr socket path is too long under {} and under {}",
                config.display(),
                short.display()
            );
            (config.to_path_buf(), false)
        }
        None => {
            tracing::warn!(
                "the herdr socket path is too long under {} and XDG_RUNTIME_DIR is not set",
                config.display()
            );
            (config.to_path_buf(), false)
        }
    }
}

/// The config folder for `session`, as `pick_config_home` picks it, with
/// the short symlink made (or pointed right) when it is that one.
fn config_home_for(session: &str) -> (PathBuf, bool) {
    let config = config_home();
    let runtime = runtime_dir();
    let (picked, short) = pick_config_home(session, &config, runtime.as_deref());
    if short && let Err(err) = link_config(&picked, &config) {
        tracing::warn!(
            "linking {} to {}: {err}",
            picked.display(),
            config.display()
        );
    }
    (picked, short)
}

/// Make `link` a symlink to `config`, replacing one pointing elsewhere.
fn link_config(link: &Path, config: &Path) -> std::io::Result<()> {
    if std::fs::read_link(link).is_ok_and(|target| target == config) {
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::remove_file(link) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    std::os::unix::fs::symlink(config, link)
}

/// Where herdr keeps named session `session`: its socket, log and saved
/// layout.
pub fn session_dir(session: &str) -> PathBuf {
    config_home_for(session)
        .0
        .join("herdr/sessions")
        .join(session)
}

pub fn socket_path(session: &str) -> PathBuf {
    session_dir(session).join("herdr.sock")
}

/// Where a server started by gerdr writes what it has to say.
pub fn server_log() -> PathBuf {
    state_dir().join(SERVER_LOG)
}

/// The last line with anything on it in `log`, if any.
fn last_line(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// The server log, open for appending; started over once too big.
fn open_server_log() -> std::io::Result<std::fs::File> {
    let path = server_log();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let big = std::fs::metadata(&path).is_ok_and(|m| m.len() > SERVER_LOG_MAX);
    std::fs::OpenOptions::new()
        .create(true)
        .append(!big)
        .write(true)
        .truncate(big)
        .open(&path)
}

/// How many workspaces `session` saved for its server to restore on start:
/// 0 without a `session.json` (herdr keeps one even with none in it).
pub fn saved_workspace_count(session: &str) -> usize {
    std::fs::read(session_dir(session).join("session.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|saved| saved["workspaces"].as_array().map(Vec::len))
        .unwrap_or(0)
}

/// Whether `session` saved workspaces that its server restores on start.
pub fn has_saved_layout(session: &str) -> bool {
    saved_workspace_count(session) > 0
}

/// Where the `herdr` executable is, looking through the folders of `path`
/// (as in `PATH`).
pub fn find_on_path(path: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("herdr"))
        .find(|file| {
            std::fs::metadata(file)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Whether herdr can be found on `PATH`.
pub fn is_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|path| find_on_path(&path).is_some())
}

/// Whether a server answers for `session`. A socket file alone says nothing:
/// stopped sessions leave theirs behind.
pub fn is_running(session: &str) -> bool {
    let mut herdr = Herdr::on_socket(socket_path(session));
    herdr.timeout = PROBE;
    herdr.call("ping", json!({})).is_ok()
}

/// Stop `session`'s server, which saves its layout first. One that goes
/// before it answers counts as stopped.
pub fn stop_server(session: &str) -> Result<(), String> {
    match Herdr::on_socket(socket_path(session)).stop() {
        Err(_) if !is_running(session) => Ok(()),
        stopped => stopped,
    }
}

/// Drop herdr's caller context from `cmd` and point it at `socket`, the
/// active project's session, if any.
pub fn scrub_env(cmd: &mut Command, socket: Option<&Path>) {
    strip_inherited(cmd);
    if let Some(socket) = socket {
        cmd.env("HERDR_SOCKET_PATH", socket);
    }
}

fn strip_inherited(cmd: &mut Command) {
    for var in INHERITED {
        cmd.env_remove(var);
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub label: String,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

impl WorkspaceInfo {
    /// Whether gerdr opened it (rather than the server or someone else).
    pub fn is_gerdr(&self) -> bool {
        self.tokens.contains_key(KIND_TOKEN)
    }

    /// Whether gerdr opened it as a project (Zed over a terminal).
    pub fn is_project(&self) -> bool {
        self.tokens.get(KIND_TOKEN).is_some_and(|k| k == "project")
    }

    /// The folder gerdr opened it for.
    pub fn dir(&self) -> Option<PathBuf> {
        let pieces = (0..).map_while(|i| self.tokens.get(&format!("{DIR_TOKEN}{i}")));
        let dir: String = pieces.map(String::as_str).collect();
        (!dir.is_empty()).then(|| PathBuf::from(dir))
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Cells {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Deserialize)]
pub struct LayoutPane {
    pub pane_id: String,
    pub rect: Cells,
}

/// What herdr makes of the coding agent in a pane.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Ready for input, and seen since it got there.
    Idle,
    Working,
    /// Showing an approval prompt or a question.
    Blocked,
    /// Ready for input after unseen background work.
    Done,
    #[default]
    #[serde(other)]
    Unknown,
}

/// The agent's own session, for resuming it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AgentSession {
    /// Who reported it (an integration such as `herdr:opencode`).
    pub source: String,
    pub agent: String,
    /// `id` or `path`.
    pub kind: String,
    pub value: String,
}

/// A coding agent herdr recognised in a pane, as `agent.list` reports it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AgentInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// The agent kind (`claude`, `codex`, …) and how herdr names it.
    pub agent: Option<String>,
    pub display_agent: Option<String>,
    /// The name it was given with `herdr agent start` or `rename`.
    pub name: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    pub agent_session: Option<AgentSession>,
    pub cwd: Option<PathBuf>,
    /// Where the foreground process is, if it moved away from `cwd`.
    pub foreground_cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    /// Extra state the agent's integration reports, by label.
    #[serde(default)]
    pub state_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub interactive_ready: bool,
    #[serde(default)]
    pub launch_pending: bool,
    #[serde(default)]
    pub screen_detection_skipped: bool,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub state_change_seq: u64,
}

impl AgentInfo {
    /// What to call it: the name it was given, else its kind.
    pub fn label(&self) -> &str {
        self.name
            .as_ref()
            .or(self.display_agent.as_ref())
            .or(self.agent.as_ref())
            .map_or("Agent", |s| s.as_str())
    }
}

/// A tab as `tab.list` reports it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TabInfo {
    pub tab_id: String,
    /// Its place and name in herdr's own tab bar.
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub pane_count: u32,
    /// The most pressing status among its agents.
    #[serde(default)]
    pub agent_status: AgentStatus,
}

/// How far a pane is scrolled back, in rows.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Scroll {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

/// A process in a pane's foreground process group.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Process {
    pub pid: i32,
    pub name: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cmdline: String,
    pub cwd: Option<PathBuf>,
}

/// What runs in a pane, as `pane.process_info` reports it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ProcessInfo {
    pub shell_pid: Option<i32>,
    /// Equal to `shell_pid` while the shell waits at its prompt.
    #[serde(default)]
    pub foreground_process_group_id: Option<i32>,
    #[serde(default)]
    pub foreground_processes: Vec<Process>,
}

/// A Git worktree of a workspace's repository, as `worktree.list` reports it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    #[serde(default)]
    pub is_detached: bool,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

/// A pane as `pane.list` reports it, with what runs in it and the worktree
/// it is in.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PaneState {
    pub pane_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub focused: bool,
    pub cwd: Option<PathBuf>,
    pub foreground_cwd: Option<PathBuf>,
    pub agent: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    pub scroll: Option<Scroll>,
    #[serde(default)]
    pub revision: u64,
    #[serde(skip)]
    pub process: Option<ProcessInfo>,
    #[serde(skip)]
    pub worktree: Option<Worktree>,
    /// The name of the repository `worktree` belongs to.
    #[serde(skip)]
    pub repo: Option<String>,
}

/// Everything herdr reports about a tab and its panes, in herdr's pane order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabDetail {
    pub tab: TabInfo,
    /// Its place among its workspace's tabs.
    pub index: usize,
    pub panes: Vec<PaneState>,
}

/// A change made in the session, by gerdr or anyone else.
#[derive(Debug)]
pub enum Event {
    /// Something appeared in the workspace: it, a tab or a pane.
    Created {
        workspace_id: String,
    },
    /// A pane went away (closed or its shell exited).
    PaneGone {
        pane_id: String,
    },
    PaneMoved {
        previous_pane_id: String,
        pane: PaneInfo,
    },
    WorkspaceRenamed {
        workspace_id: String,
        label: String,
    },
    WorkspaceClosed {
        workspace_id: String,
    },
    /// An agent showed up, left, or changed state or title.
    AgentsChanged,
}

/// How a session's server came to answer `connect_or_start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// It was already running.
    Attached,
    /// Just started, and it brought back the workspaces it saved (as fresh
    /// shells: what ran in them is gone).
    Restored,
    /// Just started, with nothing saved: no workspaces.
    Fresh,
}

/// Requests to a herdr session's server. The server answers one request per
/// connection, so each call connects anew.
pub struct Herdr {
    next_id: u64,
    socket: PathBuf,
    timeout: Duration,
    /// The config folder the server's shells get back, when the server was
    /// told the short one (see `pick_config_home`).
    shell_config_home: Option<PathBuf>,
}

impl Herdr {
    /// Requests to the server on `socket`, without checking it answers.
    pub fn on_socket(socket: PathBuf) -> Self {
        Self {
            next_id: 0,
            socket,
            timeout: TIMEOUT,
            shell_config_home: None,
        }
    }

    /// The environment of a new pane's shell: gerdr's display, and the
    /// user's config folder in place of the short one the server runs
    /// with, which lives in the runtime folder and only stands in for the
    /// socket's sake. Shells the server starts on its own (restoring its
    /// layout) still get the short one: a link to the same folder.
    pub fn pane_env(&self, wayland_display: &OsStr) -> Value {
        let mut env = json!({"WAYLAND_DISPLAY": wayland_display.to_string_lossy()});
        if let Some(config) = &self.shell_config_home {
            env["XDG_CONFIG_HOME"] = config.to_string_lossy().into();
        }
        env
    }

    fn connect(socket: &Path) -> Result<Self, String> {
        let mut herdr = Self::on_socket(socket.to_path_buf());
        herdr.call("ping", json!({}))?;
        Ok(herdr)
    }

    /// The socket of the session it talks to.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Connect to `session`, starting its server if none runs. What the
    /// server said last (see `server_log`) when it doesn't come up, or
    /// why it didn't start.
    pub fn connect_or_start(
        session: &str,
        wayland_display: &OsStr,
    ) -> Result<(Self, Start), String> {
        let (config, short) = config_home_for(session);
        let socket = socket_path(session);
        let (herdr, start) =
            Self::connect_or_start_server(session, wayland_display, &config, short, &socket)?;
        let shell_config_home = short.then(config_home);
        Ok((
            Self {
                shell_config_home,
                ..herdr
            },
            start,
        ))
    }

    /// `connect_or_start` once the config folder and socket are picked.
    fn connect_or_start_server(
        session: &str,
        wayland_display: &OsStr,
        config: &Path,
        short: bool,
        socket: &Path,
    ) -> Result<(Self, Start), String> {
        if let Ok(herdr) = Self::connect(socket) {
            return Ok((herdr, Start::Attached));
        }
        let saved = has_saved_layout(session);
        // Its own session, out of gerdr's process groups: it outlives gerdr.
        // Shells it starts on its own still open their windows in gerdr.
        let mut server = Command::new("herdr");
        server.args(["--session", session, "server"]);
        strip_inherited(&mut server);
        server
            .env("WAYLAND_DISPLAY", wayland_display)
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        // The server puts its socket under its config folder (see
        // `pick_config_home`); the short name keeps the path short enough.
        if short {
            server.env("XDG_CONFIG_HOME", config);
        }
        match open_server_log() {
            Ok(log) => {
                server.stdout(log.try_clone().map_or(Stdio::null(), Stdio::from));
                server.stderr(log);
            }
            Err(err) => {
                tracing::warn!("herdr server log: {err}");
                server.stderr(Stdio::null());
            }
        }
        // Safety: setsid is async-signal-safe.
        unsafe {
            server.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Err(err) = server.spawn() {
            return Err(format!("herdr server failed to start: {err}"));
        }
        // The socket answers before the server is done restoring; a server
        // with a saved layout is ready once its workspaces are back. A
        // server started without one has none.
        let start = Instant::now();
        while start.elapsed() < STARTUP {
            std::thread::sleep(Duration::from_millis(50));
            let Ok(mut herdr) = Self::connect(socket) else {
                continue;
            };
            if !saved {
                return Ok((herdr, Start::Fresh));
            }
            if herdr.workspace_list().is_ok_and(|list| !list.is_empty()) {
                return Ok((herdr, Start::Restored));
            }
        }
        // A server that answers but restored nothing (or not yet) is still
        // the session's: gerdr mustn't run without it and leave it behind.
        if saved && let Ok(herdr) = Self::connect(socket) {
            tracing::warn!("herdr server didn't restore its workspaces in time");
            return Ok((herdr, Start::Restored));
        }
        Err(match last_line(&server_log()) {
            Some(line) => format!("herdr server didn't come up: {line}"),
            None => "herdr server didn't come up".to_owned(),
        })
    }

    /// Send a request and wait for its result.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let start = Instant::now();
        let result = self.request(method, params);
        let took = start.elapsed();
        if took > SLOW_CALL {
            tracing::debug!("herdr {method} took {took:?}");
        }
        result
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let fail = |e: &dyn std::fmt::Display| format!("{method}: {e}");
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| fail(&e))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| fail(&e))?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|e| fail(&e))?;
        let mut line =
            serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).unwrap();
        line.push(b'\n');
        stream.write_all(&line).map_err(|e| fail(&e))?;
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .map_err(|e| fail(&e))?;
        let mut reply: Value = serde_json::from_str(&reply).map_err(|e| fail(&e))?;
        if let Some(err) = reply.get("error") {
            return Err(fail(&err["message"].as_str().unwrap_or("failed")));
        }
        Ok(reply["result"].take())
    }

    fn get<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Value,
        key: &str,
    ) -> Result<T, String> {
        let mut result = self.call(method, params)?;
        serde_json::from_value(result[key].take()).map_err(|e| format!("{method}: {e}"))
    }

    /// A new workspace in `dir`, labelled `label` and marked as gerdr's;
    /// returns its first pane.
    pub fn workspace_create(
        &mut self,
        dir: &Path,
        label: &str,
        project: bool,
        env: &Value,
    ) -> Result<PaneInfo, String> {
        let pane: PaneInfo = self.get(
            "workspace.create",
            json!({"cwd": dir, "label": label, "env": env}),
            "root_pane",
        )?;
        let mut tokens = json!({KIND_TOKEN: if project { "project" } else { "terminal" }});
        let mut pieces = vec![String::new()];
        for c in dir.to_string_lossy().chars() {
            if pieces.last().unwrap().len() + c.len_utf8() > TOKEN_MAX {
                pieces.push(String::new());
            }
            pieces.last_mut().unwrap().push(c);
        }
        for (i, piece) in pieces.into_iter().enumerate() {
            tokens[format!("{DIR_TOKEN}{i}")] = piece.into();
        }
        let params = json!({"workspace_id": pane.workspace_id, "source": SOURCE, "tokens": tokens});
        if let Err(err) = self.call("workspace.report_metadata", params) {
            tracing::warn!("{err}");
        }
        Ok(pane)
    }

    /// A new tab in `workspace_id`; returns its first pane.
    pub fn tab_create(
        &mut self,
        workspace_id: &str,
        dir: &Path,
        env: &Value,
    ) -> Result<PaneInfo, String> {
        self.get(
            "tab.create",
            json!({"workspace_id": workspace_id, "cwd": dir, "env": env}),
            "root_pane",
        )
    }

    /// Split `target`; the new pane goes right of or below it.
    pub fn pane_split(
        &mut self,
        target: &str,
        dir: Dir,
        cwd: &Path,
        env: &Value,
    ) -> Result<PaneInfo, String> {
        let params =
            json!({"target_pane_id": target, "direction": split_name(dir), "cwd": cwd, "env": env});
        self.get("pane.split", params, "pane")
    }

    /// Swap the panes `source` and `target` (of one tab).
    pub fn pane_swap(&mut self, source: &str, target: &str) -> Result<(), String> {
        let params = json!({"source_pane_id": source, "target_pane_id": target});
        let result = self.call("pane.swap", params)?;
        let swap = &result["swap"];
        if swap["changed"].as_bool().unwrap_or(false) {
            return Ok(());
        }
        let reason = swap["reason"].as_str().unwrap_or("not swapped");
        Err(format!("pane.swap: {reason}"))
    }

    /// Move `pane_id` into the tab `tab_id`, split off `target` (right of
    /// or below it; `None`: wherever herdr puts it) with `target` keeping
    /// `ratio` of the space. Returns the pane as herdr has it now (its tab
    /// may have changed).
    pub fn pane_move(
        &mut self,
        pane_id: &str,
        tab_id: &str,
        target: Option<&str>,
        dir: Dir,
        ratio: f64,
    ) -> Result<PaneInfo, String> {
        let destination = json!({
            "type": "tab",
            "tab_id": tab_id,
            "split": split_name(dir),
            "target_pane_id": target,
            "ratio": ratio,
        });
        let mut result = self.call("pane.move", move_params(pane_id, destination))?;
        moved_pane(result["move_result"].take())
    }

    /// Move `pane_id` into a new tab of `workspace_id`. Returns the pane as
    /// herdr has it now (in the new tab).
    pub fn pane_move_to_new_tab(
        &mut self,
        pane_id: &str,
        workspace_id: &str,
    ) -> Result<PaneInfo, String> {
        let destination = json!({"type": "new_tab", "workspace_id": workspace_id, "label": null});
        let mut result = self.call("pane.move", move_params(pane_id, destination))?;
        moved_pane(result["move_result"].take())
    }

    /// Close a pane, ending what runs in it.
    pub fn pane_close(&mut self, pane_id: &str) -> Result<(), String> {
        self.call("pane.close", json!({"pane_id": pane_id}))
            .map(drop)
    }

    pub fn workspace_close(&mut self, workspace_id: &str) -> Result<(), String> {
        self.call("workspace.close", json!({"workspace_id": workspace_id}))
            .map(drop)
    }

    pub fn workspace_list(&mut self) -> Result<Vec<WorkspaceInfo>, String> {
        self.get("workspace.list", json!({}), "workspaces")
    }

    pub fn workspace_rename(&mut self, workspace_id: &str, label: &str) -> Result<(), String> {
        self.call(
            "workspace.rename",
            json!({"workspace_id": workspace_id, "label": label}),
        )
        .map(drop)
    }

    pub fn workspace_get(&mut self, workspace_id: &str) -> Result<WorkspaceInfo, String> {
        self.get(
            "workspace.get",
            json!({"workspace_id": workspace_id}),
            "workspace",
        )
    }

    /// The tab ids of a workspace, in order.
    pub fn tab_list(&mut self, workspace_id: &str) -> Result<Vec<String>, String> {
        Ok(self
            .tabs(workspace_id)?
            .into_iter()
            .map(|t| t.tab_id)
            .collect())
    }

    /// The tabs of a workspace, in order.
    pub fn tabs(&mut self, workspace_id: &str) -> Result<Vec<TabInfo>, String> {
        self.get("tab.list", json!({"workspace_id": workspace_id}), "tabs")
    }

    /// Move workspace `workspace_id` before workspace `before`, or to the end.
    pub fn workspace_move(
        &mut self,
        workspace_id: &str,
        before: Option<&str>,
    ) -> Result<(), String> {
        let order: Vec<_> = self
            .workspace_list()?
            .into_iter()
            .map(|ws| ws.workspace_id)
            .collect();
        let params =
            json!({"workspace_id": workspace_id, "insert_index": insert_index(&order, before)});
        self.call("workspace.move", params).map(drop)
    }

    /// Move tab `tab_id` of `workspace_id` before tab `before`, or to the end.
    pub fn tab_move(
        &mut self,
        workspace_id: &str,
        tab_id: &str,
        before: Option<&str>,
    ) -> Result<(), String> {
        let order = self.tab_list(workspace_id)?;
        self.call(
            "tab.move",
            json!({"tab_id": tab_id, "insert_index": insert_index(&order, before)}),
        )
        .map(drop)
    }

    /// The Git worktrees of the repository `workspace_id` is in, and the
    /// repository's name.
    pub fn worktree_list(
        &mut self,
        workspace_id: &str,
    ) -> Result<(Option<String>, Vec<Worktree>), String> {
        let mut result = self.call("worktree.list", json!({"workspace_id": workspace_id}))?;
        let repo = result["source"]["repo_name"].as_str().map(str::to_owned);
        let worktrees = serde_json::from_value(result["worktrees"].take())
            .map_err(|e| format!("worktree.list: {e}"))?;
        Ok((repo, worktrees))
    }

    pub fn process_info(&mut self, pane_id: &str) -> Result<ProcessInfo, String> {
        self.get(
            "pane.process_info",
            json!({"pane_id": pane_id}),
            "process_info",
        )
    }

    /// Stop the session's server, ending everything that runs in it. It
    /// saves its layout first.
    pub fn stop(&mut self) -> Result<(), String> {
        self.call("server.stop", json!({})).map(drop)
    }

    /// Type `text` into pane `pane_id` and press Enter. A shell still
    /// starting keeps it and runs it at its first prompt.
    pub fn send_line(&mut self, pane_id: &str, text: &str) -> Result<(), String> {
        self.call(
            "pane.send_input",
            json!({"pane_id": pane_id, "text": text, "keys": ["Enter"]}),
        )
        .map(drop)
    }

    /// Type `text` into the agent CLI running in pane `pane_id`, then press
    /// Enter apart from it. Cursor (2026.11.05, probed) keeps text and Enter
    /// that come together in its input box and runs nothing.
    pub fn send_agent_line(&mut self, pane_id: &str, text: &str) -> Result<(), String> {
        self.call(
            "pane.send_input",
            json!({"pane_id": pane_id, "text": text, "keys": []}),
        )?;
        std::thread::sleep(ENTER_AFTER);
        self.send_keys(pane_id, &["Enter"])
    }

    /// Run `line` in the shell of pane `pane_id`, which is at its prompt or
    /// still starting. What is on its input line goes first, or `line` would
    /// land behind it and run mangled: a `herdr terminal attach` client asks
    /// its terminal about itself as it starts, and a reply coming late is
    /// passed on to the pane as if typed (`1016;0$y6c`). As a key: `text` is
    /// pasted, so a control character in it stays literal.
    pub fn send_shell_line(&mut self, pane_id: &str, line: &str) -> Result<(), String> {
        if let Err(err) = self.send_keys(pane_id, &["ctrl+u"]) {
            // An empty line needs nothing, and the line itself tells.
            tracing::warn!("clearing the input line of pane {pane_id}: {err}");
        }
        self.send_line(pane_id, line)
    }

    /// Press `keys` in pane `pane_id`, as typed on its terminal: `ctrl+c`
    /// interrupts what runs in it. herdr checks every key before sending
    /// any, and refuses one it doesn't know (`invalid_key`).
    pub fn send_keys(&mut self, pane_id: &str, keys: &[&str]) -> Result<(), String> {
        self.call("pane.send_keys", json!({"pane_id": pane_id, "keys": keys}))
            .map(drop)
    }

    /// Everything herdr reports about the tabs of the session, by tab id.
    pub fn tab_details(&mut self) -> Result<HashMap<String, TabDetail>, String> {
        let mut details = HashMap::new();
        for ws in self.workspace_list()? {
            // It may close while being asked about.
            let id = &ws.workspace_id;
            let panes =
                self.get::<Vec<PaneState>>("pane.list", json!({"workspace_id": id}), "panes");
            let (Ok(tabs), Ok(mut panes)) = (self.tabs(id), panes) else {
                continue;
            };
            // Not a Git repository: no worktrees.
            let (repo, worktrees) = self.worktree_list(id).unwrap_or_default();
            for pane in &mut panes {
                pane.process = self.process_info(&pane.pane_id).ok();
                let dir = pane.foreground_cwd.as_ref().or(pane.cwd.as_ref());
                pane.worktree = dir.and_then(|dir| {
                    let holding = worktrees.iter().filter(|w| dir.starts_with(&w.path));
                    holding.max_by_key(|w| w.path.as_os_str().len()).cloned()
                });
                pane.repo = pane.worktree.as_ref().and(repo.clone());
            }
            for (index, tab) in tabs.into_iter().enumerate() {
                let in_tab = panes
                    .iter()
                    .filter(|p| p.tab_id == tab.tab_id)
                    .cloned()
                    .collect();
                details.insert(
                    tab.tab_id.clone(),
                    TabDetail {
                        tab,
                        index,
                        panes: in_tab,
                    },
                );
            }
        }
        Ok(details)
    }

    pub fn pane_list(&mut self, workspace_id: &str) -> Result<Vec<PaneInfo>, String> {
        self.get("pane.list", json!({"workspace_id": workspace_id}), "panes")
    }

    /// Where the panes of `pane_id`'s tab are, in terminal cells.
    pub fn pane_layout(&mut self, pane_id: &str) -> Result<Vec<LayoutPane>, String> {
        #[derive(Deserialize)]
        struct Layout {
            panes: Vec<LayoutPane>,
        }
        let layout: Layout = self.get("pane.layout", json!({"pane_id": pane_id}), "layout")?;
        Ok(layout.panes)
    }

    /// The coding agents running in the session's panes.
    pub fn agent_list(&mut self) -> Result<Vec<AgentInfo>, String> {
        self.get("agent.list", json!({}), "agents")
    }

    /// The text of pane `pane_id`, its last `lines` lines of `source`:
    /// `visible` (the screen as it is, for apps on the alternate screen,
    /// which have no scrollback), `recent` (the scrollback's end, at most
    /// 1000 lines), `recent_unwrapped` or `detection`.
    pub fn pane_read(
        &mut self,
        pane_id: &str,
        source: &str,
        lines: usize,
    ) -> Result<String, String> {
        let mut result = self.call(
            "pane.read",
            json!({"pane_id": pane_id, "source": source, "lines": lines}),
        )?;
        match result["read"]["text"].take() {
            Value::String(text) => Ok(text),
            _ => Err("pane.read: no text in the reply".into()),
        }
    }

    /// The last `lines` lines of pane `pane_id`'s output as its terminal
    /// draws them, with the escape sequences that colour them, the lines it
    /// wrapped joined again. herdr reads the colours only from the end of
    /// the scrollback (`pane.selection.read`, which reads all of it, gives
    /// plain text only), so there may be fewer.
    pub fn pane_styled(&mut self, pane_id: &str, lines: usize) -> Result<String, String> {
        let mut result = self.call(
            "pane.read",
            json!({
                "pane_id": pane_id,
                "source": "recent_unwrapped",
                "lines": lines.min(u32::MAX as usize),
                "format": "ansi",
                "strip_ansi": false,
            }),
        )?;
        match result["read"]["text"].take() {
            Value::String(text) => Ok(text),
            _ => Err("pane.read: no text in the reply".into()),
        }
    }

    /// How many rows pane `pane_id` keeps above its screen: none while it
    /// shows the alternate screen, whatever it kept before.
    pub fn pane_scrollback_rows(&mut self, pane_id: &str) -> Result<u64, String> {
        let pane: PaneState = self.get("pane.get", json!({"pane_id": pane_id}), "pane")?;
        Ok(pane
            .scroll
            .ok_or("pane.get: no scroll")?
            .max_offset_from_bottom)
    }

    /// All that pane `pane_id` keeps of its output, oldest first, the lines
    /// it wrapped joined again; and how many rows up from the bottom its
    /// view stands. Read as a selection of every row, as `pane.read` gives
    /// no more than its last 1000 lines.
    pub fn pane_scrollback(&mut self, pane_id: &str) -> Result<(String, u64), String> {
        let pane: PaneState = self.get("pane.get", json!({"pane_id": pane_id}), "pane")?;
        let scroll = pane.scroll.ok_or("pane.get: no scroll")?;
        let last = (scroll.max_offset_from_bottom + scroll.viewport_rows).saturating_sub(1);
        // A column past the row's end is refused: up to its last character.
        let end = self.call(
            "pane.copy_motion",
            json!({
                "pane_id": pane_id,
                "cursor": {"row": last, "col": 0},
                "motion": "line_end",
            }),
        )?;
        let col = end["cursor"]["col"].as_u64().unwrap_or(0);
        let mut result = self.call(
            "pane.selection.read",
            json!({
                "pane_id": pane_id,
                "anchor": {"row": 0, "col": 0},
                "cursor": {"row": last, "col": col},
            }),
        )?;
        match result["text"].take() {
            Value::String(text) => Ok((text, scroll.offset_from_bottom)),
            _ => Err("pane.selection.read: no text in the reply".into()),
        }
    }

    /// Mark the agent in `pane_id` as seen, turning `done` into `idle`.
    pub fn agent_focus(&mut self, pane_id: &str) -> Result<(), String> {
        self.call("agent.focus", json!({"target": pane_id}))
            .map(drop)
    }

    /// Open the event connection.
    pub fn subscribe(&self) -> std::io::Result<Subscription> {
        let mut stream = UnixStream::connect(&self.socket)?;
        let subscriptions: Vec<Value> = [
            "workspace.created",
            "workspace.renamed",
            "workspace.closed",
            "tab.created",
            "pane.created",
            "pane.closed",
            "pane.exited",
            "pane.moved",
            "pane.updated",
            "pane.agent_detected",
        ]
        .into_iter()
        .map(|t| json!({"type": t}))
        .collect();
        let request = json!({"id": "events", "method": "events.subscribe", "params": {"subscriptions": subscriptions}});
        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.set_nonblocking(true)?;
        Ok(Subscription {
            stream,
            buf: Vec::new(),
        })
    }
}

impl Herdr {
    /// Open a connection ringing whenever an output line matching
    /// `pattern` (a regex) comes into pane `pane_id`'s last `lines` lines,
    /// from none there (herdr's `pane.output_matched`, edge-triggered). It
    /// takes no other subscription: a second `events.subscribe` on it
    /// silences it.
    pub fn watch_output(
        &self,
        pane_id: &str,
        lines: usize,
        pattern: &str,
    ) -> std::io::Result<OutputWatch> {
        let mut stream = UnixStream::connect(&self.socket)?;
        let subscription = json!({
            "type": "pane.output_matched",
            "pane_id": pane_id,
            "source": "recent",
            "lines": lines,
            "match": {"type": "regex", "value": pattern},
            "strip_ansi": true,
        });
        let request = json!({"id": "output", "method": "events.subscribe", "params": {"subscriptions": [subscription]}});
        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.set_nonblocking(true)?;
        Ok(OutputWatch {
            stream,
            buf: Vec::new(),
        })
    }
}

/// A connection ringing on matching output in one pane (see
/// `Herdr::watch_output`).
pub struct OutputWatch {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl AsFd for OutputWatch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl OutputWatch {
    /// How many times it rang since last read; `Err` once the connection
    /// is gone, or herdr refused the subscription.
    pub fn read(&mut self) -> Result<usize, String> {
        let mut chunk = [0u8; 65536];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err("closed".into()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut rang = 0;
        while let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                return Err(error.to_string());
            }
            if value.get("event").is_some() {
                rang += 1;
            }
        }
        Ok(rang)
    }
}

/// herdr's `insert_index` for a move before `before` in `order`: it counts
/// places in the order before the move.
fn insert_index(order: &[String], before: Option<&str>) -> usize {
    before
        .and_then(|b| order.iter().position(|id| id == b))
        .unwrap_or(order.len())
}

/// What the server said about its tabs and agents in one round.
pub struct Round {
    pub tabs: HashMap<String, TabDetail>,
    /// `None` if the server didn't answer.
    pub agents: Option<Vec<AgentInfo>>,
}

/// Ask the server on `socket` about its tabs and agents every `interval`, on
/// a thread of its own so a slow answer never holds up the compositor, and
/// hand each round to `send` (no tabs while the server is gone). A message
/// on the returned sender asks for a round now. Stops once `send` returns
/// false or the sender is dropped.
pub fn watch_tabs(
    socket: PathBuf,
    interval: Duration,
    mut send: impl FnMut(Round) -> bool + Send + 'static,
) -> mpsc::Sender<()> {
    let (poke, poked) = mpsc::channel();
    std::thread::spawn(move || {
        let mut herdr = Herdr::on_socket(socket);
        loop {
            let tabs = herdr
                .tab_details()
                .inspect_err(|err| tracing::debug!("herdr: {err}"))
                .unwrap_or_default();
            let agents = herdr
                .agent_list()
                .inspect_err(|err| tracing::debug!("herdr: {err}"))
                .ok();
            if !send(Round { tabs, agents }) {
                return;
            }
            match poked.recv_timeout(interval) {
                // Pokes that came in the meantime ask for the same round.
                Ok(()) => while poked.try_recv().is_ok() {},
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    poke
}

/// The command that shows herdr terminal `terminal_id` in an Alacritty
/// window, taking it over from any other client, with Alacritty's
/// `options` (see `Gerdr::terminal_options`).
pub fn attach_command(terminal_id: &str, dir: &Path, options: &[String]) -> Command {
    let mut term = Command::new("alacritty");
    term.arg("--working-directory")
        .arg(dir)
        .args(options)
        .args([
            "-e",
            "herdr",
            "terminal",
            "attach",
            terminal_id,
            "--takeover",
        ]);
    term
}

/// The event connection; herdr replays recent events when it opens, so
/// handlers must check what still exists.
pub struct Subscription {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl AsFd for Subscription {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl Subscription {
    /// The events that arrived; `Err` once the server is gone.
    pub fn read(&mut self) -> Result<Vec<Event>, ()> {
        let mut chunk = [0u8; 65536];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(()),
            }
        }
        let mut events = Vec::new();
        while let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            match serde_json::from_slice::<Value>(&line) {
                Ok(value) => events.extend(parse_event(value)),
                Err(err) => tracing::warn!("herdr event: {err}"),
            }
        }
        Ok(events)
    }
}

fn parse_event(mut value: Value) -> Option<Event> {
    let event = value["event"].as_str()?.to_owned();
    let data = &mut value["data"];
    let string = |v: &Value| v.as_str().map(str::to_owned);
    Some(match event.as_str() {
        "workspace_created" => Event::Created {
            workspace_id: string(&data["workspace"]["workspace_id"])?,
        },
        "tab_created" => Event::Created {
            workspace_id: string(&data["tab"]["workspace_id"])?,
        },
        "pane_created" => Event::Created {
            workspace_id: string(&data["pane"]["workspace_id"])?,
        },
        "pane_closed" | "pane_exited" => Event::PaneGone {
            pane_id: string(&data["pane_id"])?,
        },
        "pane_moved" => Event::PaneMoved {
            previous_pane_id: string(&data["previous_pane_id"])?,
            pane: serde_json::from_value(data["pane"].take()).ok()?,
        },
        "workspace_closed" => Event::WorkspaceClosed {
            workspace_id: string(&data["workspace_id"])?,
        },
        "workspace_renamed" => Event::WorkspaceRenamed {
            workspace_id: string(&data["workspace_id"])?,
            label: string(&data["label"])?,
        },
        "pane_updated" | "pane_agent_detected" | "pane_agent_status_changed" => {
            Event::AgentsChanged
        }
        _ => return None,
    })
}

/// The split direction as herdr names it.
fn split_name(dir: Dir) -> &'static str {
    match dir {
        Dir::Right => "right",
        Dir::Down => "down",
    }
}

/// The params of a `pane.move` of `pane_id` to `destination`. Focus stays
/// where it is herdr-side: gerdr decides what is focused.
fn move_params(pane_id: &str, destination: Value) -> Value {
    json!({"pane_id": pane_id, "focus": false, "destination": destination})
}

/// The pane a `pane.move` reply's `move_result` leaves behind. A move herdr turned down as
/// `same_tab` is still a success: within a tab gerdr's tree is the source
/// of truth and the pane is where it was; any other refusal is an error.
fn moved_pane(mut result: Value) -> Result<PaneInfo, String> {
    let changed = result["changed"].as_bool().unwrap_or(false);
    let reason = result["reason"].as_str();
    if !changed && reason != Some("same_tab") {
        return Err(format!("pane.move: {}", reason.unwrap_or("not moved")));
    }
    serde_json::from_value(result["pane"].take()).map_err(|e| format!("pane.move: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_line_clears_the_input_line_first() {
        use std::os::unix::net::UnixListener;
        let socket = std::env::temp_dir().join(format!("gerdr-clear-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            let refused = &b"{\"error\":{\"message\":\"no\"}}\n"[..];
            for reply in [refused, b"{\"result\":{}}\n"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                stream.write_all(reply).unwrap();
                asked.push(serde_json::from_str::<Value>(&line).unwrap());
            }
            asked
        });

        // Not clearing it doesn't lose the line.
        let sent = Herdr::on_socket(socket.clone()).send_shell_line("p1", "sh check");
        let _ = std::fs::remove_file(&socket);
        sent.unwrap();

        let asked = server.join().unwrap();
        assert_eq!(asked[0]["method"], "pane.send_keys");
        assert_eq!(asked[0]["params"]["keys"], json!(["ctrl+u"]));
        assert_eq!(asked[1]["method"], "pane.send_input");
        assert_eq!(asked[1]["params"]["text"], "sh check");
        assert_eq!(asked[1]["params"]["keys"], json!(["Enter"]));
    }

    #[test]
    fn an_agents_line_gets_its_enter_apart() {
        use std::os::unix::net::UnixListener;
        let socket = std::env::temp_dir().join(format!("gerdr-enter-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                stream.write_all(b"{\"result\":{}}\n").unwrap();
                asked.push((request, Instant::now()));
            }
            asked
        });

        let sent = Herdr::on_socket(socket.clone()).send_agent_line("p1", "/summarize");
        let _ = std::fs::remove_file(&socket);
        sent.unwrap();

        let asked = server.join().unwrap();
        let (typed, typed_at) = &asked[0];
        assert_eq!(typed["method"], "pane.send_input");
        assert_eq!(typed["params"]["text"], "/summarize");
        assert_eq!(typed["params"]["keys"], json!([]));
        let (enter, enter_at) = &asked[1];
        assert_eq!(enter["method"], "pane.send_keys");
        assert_eq!(enter["params"]["pane_id"], "p1");
        assert_eq!(enter["params"]["keys"], json!(["Enter"]));
        assert!(*enter_at - *typed_at >= ENTER_AFTER);
    }

    /// Needs a scratch herdr server and `GERDR_PROBE_PANE` running a CLI
    /// agent at its empty prompt. Cursor in an empty chat says "No active
    /// session." and clears the box; the line left there is the bug.
    #[test]
    #[ignore]
    fn probe_an_agent_runs_the_line() {
        let socket = PathBuf::from(std::env::var("GERDR_PROBE_SOCKET").unwrap());
        let pane = std::env::var("GERDR_PROBE_PANE").unwrap();
        let line = std::env::var("GERDR_PROBE_LINE").unwrap_or_else(|_| "/summarize".into());
        let mut herdr = Herdr::on_socket(socket);
        herdr.send_agent_line(&pane, &line).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let screen = herdr
            .call("pane.read", json!({"pane_id": pane, "source": "visible"}))
            .unwrap();
        let text = screen["read"]["text"].as_str().unwrap_or_default();
        assert!(
            !text.lines().any(|l| l.trim().starts_with("\u{2192} /")),
            "{text}"
        );
    }

    #[test]
    fn closed_workspaces_are_reported() {
        let event = json!({"event": "workspace_closed", "data": {"type": "workspace_closed", "workspace_id": "w3", "workspace": null}});
        assert!(matches!(
            parse_event(event),
            Some(Event::WorkspaceClosed { workspace_id }) if workspace_id == "w3"
        ));
    }

    #[test]
    fn process_info_tells_a_busy_pane_from_an_idle_one() {
        let busy: ProcessInfo = serde_json::from_value(json!({
            "pane_id": "w1:p2",
            "shell_pid": 10,
            "foreground_process_group_id": 20,
            "foreground_processes": [{"pid": 20, "name": "sleep", "argv": ["sleep", "999"], "cmdline": "sleep 999", "cwd": "/tmp"}]
        }))
        .unwrap();
        assert_eq!(busy.foreground_process_group_id, Some(20));
        assert_eq!(busy.foreground_processes[0].argv, ["sleep", "999"]);
        let old: ProcessInfo =
            serde_json::from_value(json!({"shell_pid": 10, "foreground_processes": []})).unwrap();
        assert_eq!(old.foreground_process_group_id, None);
    }

    #[test]
    fn herdr_is_found_only_as_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("gerdr-herdr-path-{}", std::process::id()));
        let (plain, exec, folder) = (tmp.join("plain"), tmp.join("exec"), tmp.join("folder"));
        for dir in [&plain, &exec, &folder] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(plain.join("herdr"), "").unwrap();
        std::fs::write(exec.join("herdr"), "").unwrap();
        std::fs::set_permissions(exec.join("herdr"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::create_dir_all(folder.join("herdr")).unwrap();
        let path = std::env::join_paths([&folder, &plain, &tmp.join("missing"), &exec]).unwrap();
        assert_eq!(find_on_path(&path), Some(exec.join("herdr")));
        let path = std::env::join_paths([&folder, &plain]).unwrap();
        assert_eq!(find_on_path(&path), None);
        assert_eq!(find_on_path(OsStr::new("")), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_long_socket_path_moves_under_the_runtime_dir() {
        let runtime = Some(Path::new("/run/user/1000"));
        let short = Path::new("/home/me/.config");
        assert_eq!(
            pick_config_home("gerdr-x", short, runtime),
            (short.to_path_buf(), false)
        );
        assert_eq!(
            socket_under(short, "gerdr-x"),
            PathBuf::from("/home/me/.config/herdr/sessions/gerdr-x/herdr.sock")
        );
        let deep = PathBuf::from(format!("/home/{}/.config", "a".repeat(90)));
        let (picked, moved) = pick_config_home("gerdr-x", &deep, runtime);
        assert!(moved);
        assert_eq!(picked, PathBuf::from("/run/user/1000/gerdr/config"));
        assert!(socket_under(&picked, "gerdr-x").as_os_str().len() <= SOCKET_PATH_MAX);
        // Right at the limit, the real folder is kept; a byte over, not.
        let config = |n: usize| PathBuf::from(format!("/{}", "b".repeat(n)));
        let n = (1..200)
            .find(|&n| socket_under(&config(n), "s").as_os_str().len() == SOCKET_PATH_MAX)
            .unwrap();
        assert!(!pick_config_home("s", &config(n), runtime).1);
        assert!(pick_config_home("s", &config(n + 1), runtime).1);
        // Without a runtime folder, or with one too deep itself, the real
        // folder stays.
        assert_eq!(
            pick_config_home("gerdr-x", &deep, None),
            (deep.clone(), false)
        );
        let deep_runtime = PathBuf::from(format!("/run/{}", "r".repeat(90)));
        assert_eq!(
            pick_config_home("gerdr-x", &deep, Some(&deep_runtime)),
            (deep.clone(), false)
        );
    }

    #[test]
    fn the_short_config_link_points_at_the_real_folder() {
        let tmp = std::env::temp_dir().join(format!("gerdr-herdr-link-{}", std::process::id()));
        let (real, other) = (tmp.join("real"), tmp.join("other"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let link = tmp.join("runtime/gerdr/config");
        link_config(&link, &real).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), real);
        // Pointed elsewhere, it is pointed right again; already right, kept.
        link_config(&link, &other).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), other);
        link_config(&link, &other).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), other);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn the_last_line_of_the_server_log_is_what_it_said() {
        let tmp = std::env::temp_dir().join(format!("gerdr-herdr-log-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let log = tmp.join("herdr-server.log");
        assert_eq!(last_line(&log), None);
        std::fs::write(&log, "started\nError: socket in use\n\n  \n").unwrap();
        assert_eq!(last_line(&log).as_deref(), Some("Error: socket in use"));
        std::fs::write(&log, "\n").unwrap();
        assert_eq!(last_line(&log), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn attach_passes_the_colours_before_the_command() {
        let options = ["-o".to_owned(), "colors.normal.red=\"#e06c75\"".to_owned()];
        let cmd = attach_command("t1", Path::new("/my dir"), &options);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--working-directory",
                "/my dir",
                "-o",
                "colors.normal.red=\"#e06c75\"",
                "-e",
                "herdr",
                "terminal",
                "attach",
                "t1",
                "--takeover"
            ]
        );
    }

    #[test]
    fn insert_index_counts_places_before_the_move() {
        let order = ["a".to_string(), "b".into(), "c".into()];
        assert_eq!(insert_index(&order, Some("b")), 1);
        assert_eq!(insert_index(&order, None), 3);
    }

    #[test]
    fn a_pane_move_never_takes_the_focus_with_it() {
        let to_tab = json!({"type": "tab", "tab_id": "w1:t2", "split": split_name(Dir::Down), "target_pane_id": Value::Null, "ratio": 0.5});
        assert_eq!(
            move_params("w1:p1", to_tab),
            json!({"pane_id": "w1:p1", "focus": false, "destination": {"type": "tab", "tab_id": "w1:t2", "split": "down", "target_pane_id": null, "ratio": 0.5}})
        );
        assert_eq!(split_name(Dir::Right), "right");
    }

    #[test]
    fn a_move_within_the_tab_is_still_the_pane_as_herdr_has_it() {
        let pane = json!({"pane_id": "w1:p1", "terminal_id": "t1", "workspace_id": "w1", "tab_id": "w1:t1", "cwd": "/tmp"});
        let moved = moved_pane(json!({"changed": true, "reason": null, "pane": pane})).unwrap();
        assert_eq!(moved.tab_id, "w1:t1");
        let same =
            moved_pane(json!({"changed": false, "reason": "same_tab", "pane": pane})).unwrap();
        assert_eq!(same.pane_id, "w1:p1");
        let zoomed = moved_pane(json!({"changed": false, "reason": "zoomed_tab", "pane": pane}));
        assert_eq!(zoomed.unwrap_err(), "pane.move: zoomed_tab");
        assert!(moved_pane(json!({"changed": false, "pane": pane})).is_err());
    }
}
