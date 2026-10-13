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
        let path = store_file();
        match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|err| {
                // Left as it is until the next save, so it can be rescued.
                tracing::warn!(
                    "reading {}: {err}, starting without projects",
                    path.display()
                );
                Self::empty()
            }),
            Err(err) if err.kind() == ErrorKind::NotFound => Self::first_run(),
            Err(err) => {
                tracing::warn!("reading {}: {err}", path.display());
                Self::empty()
            }
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
    fn first_run() -> Self {
        let dir = herdr::session_dir(LEGACY);
        if !dir.exists() {
            return Self::empty();
        }
        let live = herdr::is_running(LEGACY)
            .then(|| herdr::Herdr::on_socket(herdr::socket_path(LEGACY)).workspace_list())
            .and_then(|list| list.map_err(|err| tracing::warn!("{err}")).ok());
        let session_json = std::fs::read_to_string(dir.join("session.json")).ok();
        match migrate(live.as_deref(), session_json.as_deref()) {
            Some(project) => Self {
                version: VERSION,
                last_active: Some(project.id.clone()),
                quit_choice: None,
                projects: vec![project],
            },
            None => Self::empty(),
        }
    }

    /// Write it out atomically (temporary file, then rename).
    pub fn save(&self) {
        let path = store_file();
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

impl PaneRecord {
    /// The command to run when the pane comes back, unless the user said not
    /// to.
    pub fn restart_command(&self) -> Option<&str> {
        self.command
            .as_deref()
            .filter(|command| !self.skip.iter().any(|s| s == command))
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

/// What runs in the foreground of a pane, if anything but its idle shell:
/// the process group leader's command line, where it runs (if not in
/// `pane_cwd`), and its environment worth restoring (from `/proc`).
pub fn foreground_command(info: &herdr::ProcessInfo, pane_cwd: Option<&Path>) -> Option<Captured> {
    let shell = info.shell_pid?;
    let group = info.foreground_process_group_id?;
    if group == shell {
        return None;
    }
    // In a pipeline the group is named after its first command.
    let leader = info
        .foreground_processes
        .iter()
        .find(|p| p.pid == group)
        .or(info.foreground_processes.first())?;
    // Login shells show up as `-zsh`.
    if SHELLS.contains(&leader.name.trim_start_matches('-')) {
        return None;
    }
    let command = if leader.argv.is_empty() {
        leader.cmdline.trim().to_owned()
    } else {
        let words: Vec<_> = leader.argv.iter().map(|arg| shell_quote(arg)).collect();
        words.join(" ")
    };
    if command.is_empty() {
        return None;
    }
    let cwd = leader
        .cwd
        .clone()
        .filter(|cwd| Some(cwd.as_path()) != pane_cwd);
    let environ = std::fs::read(format!("/proc/{}/environ", leader.pid)).unwrap_or_default();
    Some(Captured {
        command,
        cwd,
        env: environ_whitelist(&environ),
    })
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
        assert_eq!(foreground_command(&idle, None), None);
        assert_eq!(
            foreground_command(&herdr::ProcessInfo::default(), None),
            None
        );
        let unknown = herdr::ProcessInfo {
            foreground_process_group_id: None,
            ..info(NO_PID, NO_PID + 1, Vec::new())
        };
        assert_eq!(foreground_command(&unknown, None), None);
        // A shell started from the shell is no command to run again.
        let nested = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "-bash", &["-bash"], "-bash", "/w")],
        );
        assert_eq!(foreground_command(&nested, None), None);
        let herdr = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "herdr", &["herdr"], "herdr", "/w")],
        );
        assert_eq!(foreground_command(&herdr, None), None);
    }

    #[test]
    fn busy_pane_runs_its_leader() {
        let dir = "/home/me/services-api";
        let busy = info(
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
            foreground_command(&busy, Some(Path::new(dir))),
            Some(Captured {
                command: "sleep 999".into(),
                cwd: None,
                env: Vec::new(),
            })
        );
        // Elsewhere than the pane's folder, it runs there again.
        let moved = foreground_command(&busy, Some(Path::new("/home/me"))).unwrap();
        assert_eq!(moved.cwd, Some(PathBuf::from(dir)));
        assert_eq!(
            foreground_command(&busy, None).unwrap().cwd,
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
            foreground_command(&quoted, None).unwrap().command,
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
            foreground_command(&pipeline, None).unwrap().command,
            "tail -f log"
        );
        // No leader among them: the first one stands for the group.
        let leaderless = info(NO_PID, NO_PID + 9, pipeline.foreground_processes.clone());
        assert_eq!(
            foreground_command(&leaderless, None).unwrap().command,
            "grep x"
        );
    }

    #[test]
    fn falls_back_to_cmdline() {
        let bare = info(
            NO_PID,
            NO_PID + 1,
            vec![process(
                NO_PID + 1,
                "node",
                &[],
                "node server.js --port 3000",
                "/w",
            )],
        );
        assert_eq!(
            foreground_command(&bare, None).unwrap().command,
            "node server.js --port 3000"
        );
        let blank = info(
            NO_PID,
            NO_PID + 1,
            vec![process(NO_PID + 1, "node", &[], "", "/w")],
        );
        assert_eq!(foreground_command(&blank, None), None);
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
