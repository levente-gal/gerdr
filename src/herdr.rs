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
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{home_dir, layout::Dir};

/// The herdr session gerdr's terminals live in, apart from the user's own.
const SESSION: &str = "gerdr";
/// How long a request may take before gerdr gives up on it.
const TIMEOUT: Duration = Duration::from_secs(3);
/// How long a server gerdr started may take to answer.
const STARTUP: Duration = Duration::from_secs(5);
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

pub fn socket_path() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".config/herdr/sessions")
        .join(SESSION)
        .join("herdr.sock")
}

/// Drop herdr's caller context from `cmd` and point it at gerdr's session.
pub fn scrub_env(cmd: &mut Command) {
    strip_inherited(cmd);
    cmd.env("HERDR_SOCKET_PATH", socket_path());
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
    pub cmdline: String,
    pub cwd: Option<PathBuf>,
}

/// What runs in a pane, as `pane.process_info` reports it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ProcessInfo {
    pub shell_pid: Option<i32>,
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
    /// An agent showed up, left, or changed state or title.
    AgentsChanged,
}

/// Requests to gerdr's herdr server. The server answers one request per
/// connection, so each call connects anew.
pub struct Herdr {
    next_id: u64,
}

impl Herdr {
    fn connect() -> Result<Self, String> {
        let mut herdr = Self { next_id: 0 };
        herdr.call("ping", json!({}))?;
        Ok(herdr)
    }

    /// Connect to gerdr's session, starting its server if none runs. The
    /// bool says whether it was just started. `None` if herdr isn't there.
    pub fn connect_or_start(wayland_display: &OsStr) -> Option<(Self, bool)> {
        if let Ok(herdr) = Self::connect() {
            return Some((herdr, false));
        }
        // Its own session, out of gerdr's process groups: it outlives gerdr.
        // Shells it starts on its own still open their windows in gerdr.
        let mut server = Command::new("herdr");
        server.args(["--session", SESSION, "server"]);
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
        // The socket answers before the server is done starting; it is
        // ready once its startup workspace is there.
        let start = Instant::now();
        while start.elapsed() < STARTUP {
            std::thread::sleep(Duration::from_millis(50));
            if let Ok(mut herdr) = Self::connect()
                && herdr.workspace_list().is_ok_and(|list| !list.is_empty())
            {
                return Some((herdr, true));
            }
        }
        tracing::warn!("herdr server didn't come up, terminals won't persist");
        None
    }

    /// Send a request and wait for its result.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let fail = |e: &dyn std::fmt::Display| format!("{method}: {e}");
        let mut stream = UnixStream::connect(socket_path()).map_err(|e| fail(&e))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|e| fail(&e))?;
        stream
            .set_write_timeout(Some(TIMEOUT))
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
        let params =
            json!({"workspace_id": pane.workspace_id, "source": SESSION, "tokens": tokens});
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

    /// Everything herdr reports about the tabs of gerdr's workspaces, by tab id.
    pub fn tab_details(&mut self) -> Result<HashMap<String, TabDetail>, String> {
        let mut details = HashMap::new();
        for ws in self
            .workspace_list()?
            .into_iter()
            .filter(WorkspaceInfo::is_gerdr)
        {
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
            for tab in tabs {
                let in_tab = panes
                    .iter()
                    .filter(|p| p.tab_id == tab.tab_id)
                    .cloned()
                    .collect();
                details.insert(tab.tab_id.clone(), TabDetail { tab, panes: in_tab });
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
    pub fn subscribe() -> std::io::Result<Subscription> {
        let mut stream = UnixStream::connect(socket_path())?;
        let subscriptions: Vec<Value> = [
            "workspace.created",
            "workspace.renamed",
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

/// Ask herdr about the tabs of gerdr's workspaces every `interval`, on a
/// thread of its own so a slow answer never holds up the compositor, and hand
/// each round to `send` (empty while the server is gone). Stops once `send`
/// returns false.
/// herdr's `insert_index` for a move before `before` in `order`: it counts
/// places in the order before the move.
fn insert_index(order: &[String], before: Option<&str>) -> usize {
    before
        .and_then(|b| order.iter().position(|id| id == b))
        .unwrap_or(order.len())
}

pub fn watch_tabs(
    interval: Duration,
    mut send: impl FnMut(HashMap<String, TabDetail>) -> bool + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut herdr = Herdr { next_id: 0 };
        loop {
            let details = herdr
                .tab_details()
                .inspect_err(|err| tracing::debug!("herdr: {err}"))
                .unwrap_or_default();
            if !send(details) {
                return;
            }
            std::thread::sleep(interval);
        }
    });
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
