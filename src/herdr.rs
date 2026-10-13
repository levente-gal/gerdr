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

use crate::{home_dir, layout::Dir};

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

/// Where herdr keeps named session `session`: its socket, log and saved
/// layout.
pub fn session_dir(session: &str) -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".config/herdr/sessions")
        .join(session)
}

pub fn socket_path(session: &str) -> PathBuf {
    session_dir(session).join("herdr.sock")
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
}

impl Herdr {
    /// Requests to the server on `socket`, without checking it answers.
    pub fn on_socket(socket: PathBuf) -> Self {
        Self {
            next_id: 0,
            socket,
            timeout: TIMEOUT,
        }
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

    /// Connect to `session`, starting its server if none runs. `None` if
    /// herdr isn't there.
    pub fn connect_or_start(session: &str, wayland_display: &OsStr) -> Option<(Self, Start)> {
        let socket = socket_path(session);
        if let Ok(herdr) = Self::connect(&socket) {
            return Some((herdr, Start::Attached));
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
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Safety: setsid is async-signal-safe.
        unsafe {
            server.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Err(err) = server.spawn() {
            tracing::warn!("herdr server failed to start, terminals won't persist: {err}");
            return None;
        }
        // The socket answers before the server is done restoring; a server
        // with a saved layout is ready once its workspaces are back. A
        // server started without one has none.
        let start = Instant::now();
        while start.elapsed() < STARTUP {
            std::thread::sleep(Duration::from_millis(50));
            let Ok(mut herdr) = Self::connect(&socket) else {
                continue;
            };
            if !saved {
                return Some((herdr, Start::Fresh));
            }
            if herdr.workspace_list().is_ok_and(|list| !list.is_empty()) {
                return Some((herdr, Start::Restored));
            }
        }
        // A server that answers but restored nothing (or not yet) is still
        // the session's: gerdr mustn't run without it and leave it behind.
        if saved && let Ok(herdr) = Self::connect(&socket) {
            tracing::warn!("herdr server didn't restore its workspaces in time");
            return Some((herdr, Start::Restored));
        }
        tracing::warn!("herdr server didn't come up, terminals won't persist");
        None
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
        let direction = match dir {
            Dir::Right => "right",
            Dir::Down => "down",
        };
        let params =
            json!({"target_pane_id": target, "direction": direction, "cwd": cwd, "env": env});
        self.get("pane.split", params, "pane")
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
/// window, taking it over from any other client.
pub fn attach_command(terminal_id: &str, dir: &Path) -> Command {
    let mut term = Command::new("alacritty");
    term.arg("--working-directory").arg(dir).args([
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn insert_index_counts_places_before_the_move() {
        let order = ["a".to_string(), "b".into(), "c".into()];
        assert_eq!(insert_index(&order, Some("b")), 1);
        assert_eq!(insert_index(&order, None), 3);
    }
}
