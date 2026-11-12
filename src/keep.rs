//! Apps that keep running when gerdr quits, as its terminals do in herdr:
//! those whose program the config file's `[keep] apps` names (Cursor by
//! default). A project's run on a Wayland server of their own that outlives
//! gerdr, wprs's `wprsd`; while the project is shown, a `wprsc` gerdr
//! starts shows their windows in it. Quitting gerdr ends only the `wprsc`:
//! the next gerdr starts another, and the windows come back to the panes
//! the project restores (see `Gerdr::take_kept`). Stopping the project ends
//! the server and its apps. Without `wprsd` and `wprsc` on PATH the apps
//! run on gerdr, as any other does.
//!
//! What a project keeps running is written down in
//! `$XDG_RUNTIME_DIR/gerdr/keep/<project>.json`, for the next gerdr.

use std::{
    ffi::OsString,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    Gerdr, cursor_agents,
    herdr::runtime_dir,
    home_dir, launch,
    process::{self, Process},
    toast::Level,
};

/// The wprs server, which outlives gerdr.
const SERVER: &str = "wprsd";
/// The wprs client, which shows the server's windows in gerdr.
const CLIENT: &str = "wprsc";

/// How soon a `wprsc` that ended is started again, at the most.
const CLIENT_RESTART: Duration = Duration::from_secs(10);

/// What `sh` runs first for an app or client of a server just started: wait
/// for its socket, `$1`, up to 5 s.
const WAIT_FOR_SOCKET: &str =
    r#"i=0; while [ ! -S "$1" ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done"#;

/// The shown project's server, and what gerdr has of it.
pub struct Keeper {
    /// The project's id.
    pub project: String,
    /// The server's Wayland display: its apps' WAYLAND_DISPLAY.
    pub display: String,
    /// The `wprsc` showing the server's windows in gerdr, while it runs.
    pub client: Option<u32>,
    /// When `client` was last started.
    client_started: Option<Instant>,
    /// The apps gerdr started on the server or gave a pane since the
    /// project was shown.
    pids: Vec<u32>,
    /// Where gerdr hears the hooks of a Cursor on the server (named by its
    /// display, see `cursor_agents::hook_socket`), while it does.
    hooks: Option<(PathBuf, smithay::reexports::calloop::RegistrationToken)>,
}

/// What a project keeps running, as written down.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    server: Option<Process>,
    apps: Vec<KeptApp>,
}

/// An app running on a project's server.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeptApp {
    /// The command it was launched with, as its pane remembers it.
    command: String,
    #[serde(flatten)]
    process: Process,
}

/// The server's Wayland display for project `id`.
pub fn display(id: &str) -> String {
    format!("gerdr-keep-{}", slug(id))
}

/// Project `id` as a file name.
fn slug(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where the servers' sockets and records go.
fn dir() -> Option<PathBuf> {
    Some(runtime_dir()?.join("gerdr/keep"))
}

/// The socket project `id`'s server and client talk over.
fn socket(id: &str) -> Option<PathBuf> {
    Some(dir()?.join(format!("{}.sock", slug(id))))
}

/// The control socket of project `id`'s client, which wprsc insists on.
fn control_socket(id: &str) -> Option<PathBuf> {
    Some(dir()?.join(format!("{}-ctrl.sock", slug(id))))
}

fn record_file(id: &str) -> Option<PathBuf> {
    Some(dir()?.join(format!("{}.json", slug(id))))
}

/// `name` in a folder of PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Whether wprs is installed.
pub fn available() -> bool {
    on_path(SERVER).is_some() && on_path(CLIENT).is_some()
}

/// Whether a Wayland server holds `display`: its lock file is locked.
fn server_runs(display: &str) -> bool {
    let Some(lock) = runtime_dir().map(|dir| dir.join(format!("{display}.lock"))) else {
        return false;
    };
    let Ok(file) = std::fs::File::open(lock) else {
        return false;
    };
    let fd = file.as_raw_fd();
    // Safety: a flock on a file we hold open; dropped with it.
    let free = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;
    !free
}

/// What project `id` keeps running: the apps that still do.
fn read(id: &str) -> Record {
    let Some(path) = record_file(id) else {
        return Record::default();
    };
    let mut record: Record = std::fs::read(&path)
        .ok()
        .and_then(|text| serde_json::from_slice(&text).ok())
        .unwrap_or_default();
    record.apps.retain(|app| app.process.runs());
    record.server = record.server.filter(Process::runs);
    record
}

fn write(id: &str, record: &Record) {
    let Some(path) = record_file(id) else {
        return;
    };
    let written = serde_json::to_vec(record)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, text)
        });
    if let Err(err) = written {
        tracing::warn!("writing {}: {err}", path.display());
    }
}

/// End project `id`'s server and its apps, and forget them. Each leads a
/// process group of its own (see `Gerdr::start_kept`).
pub fn stop(id: &str) {
    let record = read(id);
    let wayland = display(id);
    let groups = record
        .apps
        .iter()
        .map(|app| app.process.pid)
        .chain(record.server.map(|server| server.pid));
    for pid in groups {
        // A process group gerdr started on `wayland`: `read` keeps only
        // those whose pid and start time still match.
        process::kill_group(pid as i32, libc::SIGTERM);
    }
    for path in [record_file(id), socket(id), control_socket(id)]
        .into_iter()
        .flatten()
    {
        let _ = std::fs::remove_file(path);
    }
    if !record.apps.is_empty() || record.server.is_some() {
        tracing::info!("stopped {wayland}");
    }
}

/// The programs of the apps project `id` keeps running, each once and
/// named for people: "Cursor".
pub fn app_names(id: &str) -> Vec<String> {
    let mut names = Vec::new();
    for app in read(id).apps {
        let Some(program) = launch::program_name(&app.command) else {
            continue;
        };
        let mut chars = program.chars();
        let name: String = chars
            .next()
            .into_iter()
            .flat_map(char::to_uppercase)
            .chain(chars)
            .collect();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

impl Gerdr {
    /// Whether `command` keeps running when gerdr quits: its program is
    /// one `[keep] apps` names, and a project is shown.
    pub fn keeps(&self, command: &str) -> bool {
        self.project.is_some()
            && launch::program_name(command).is_some_and(|key| self.keep.apps.contains(&key))
    }

    /// Show the windows of the shown project's server, if it runs: start
    /// a client for it. The project was just shown.
    pub fn attach_keeper(&mut self) {
        let Some(id) = self.project.clone() else {
            return;
        };
        let display = display(&id);
        if !available() || !server_runs(&display) {
            return;
        }
        self.keep.keeper = Some(Keeper {
            project: id,
            display,
            client: None,
            client_started: None,
            pids: Vec::new(),
            hooks: None,
        });
        self.start_client();
        self.hear_kept_cursor();
    }

    /// Stop showing the server's windows: the project is left, or gerdr
    /// quits. Its client ends with gerdr's other children; the server
    /// and its apps run on.
    pub fn detach_keeper(&mut self) {
        let Some(keeper) = self.keep.keeper.take() else {
            return;
        };
        // Now, not with the other children: the next client may connect
        // to the server right away (see `reload_projects`).
        if let Some(client) = keeper.client {
            // The process group of a client gerdr started and hasn't reaped
            // yet (`client` is cleared when it ends), so its pid is still its.
            process::kill_group(client as i32, libc::SIGTERM);
        }
        if let Some((path, token)) = keeper.hooks {
            self.loop_handle.remove(token);
            let _ = std::fs::remove_file(path);
        }
    }

    /// End project `id`'s server and the apps on it.
    pub fn stop_kept(&mut self, id: &str) {
        if self.keep.keeper.as_ref().is_some_and(|k| k.project == id) {
            self.detach_keeper();
        }
        stop(id);
    }

    /// The shown project's server, started if it doesn't run, with a client
    /// showing it: its display. `None` without wprs (said once).
    fn ensure_keeper(&mut self) -> Option<String> {
        let id = self.project.clone()?;
        if !available() {
            if !self.keep.warned {
                self.keep.warned = true;
                let text = "Install wprs (wprsd and wprsc) to keep apps running when gerdr quits";
                crate::problems::report("wprs", text);
                self.toast(text.into(), Level::Warning);
            }
            return None;
        }
        let display = display(&id);
        if self.keep.keeper.as_ref().is_none_or(|k| k.project != id) {
            self.detach_keeper();
            self.keep.keeper = Some(Keeper {
                project: id.clone(),
                display: display.clone(),
                client: None,
                client_started: None,
                pids: Vec::new(),
                hooks: None,
            });
        }
        if !server_runs(&display) {
            let server = self.start_server(&id, &display)?;
            let mut record = read(&id);
            record.server = Some(Process::new(server));
            write(&id, &record);
            // The old client, if any, lost the old server.
            if let Some(keeper) = self.keep.keeper.as_mut() {
                keeper.client = None;
            }
        }
        if self
            .keep
            .keeper
            .as_ref()
            .is_some_and(|k| k.client.is_none())
        {
            self.start_client();
        }
        if self.keep.keeper.as_ref().is_some_and(|k| k.hooks.is_none()) {
            self.hear_kept_cursor();
        }
        Some(display)
    }

    /// Start project `id`'s server on Wayland display `wayland`, in a
    /// process group of its own, which gerdr doesn't end (see
    /// `retire_children`); still reaped while gerdr runs.
    fn start_server(&mut self, id: &str, wayland: &str) -> Option<u32> {
        let socket = socket(id)?;
        if let Some(dir) = socket.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("making {}: {err}", dir.display());
            return None;
        }
        let mut cmd = Command::new(SERVER);
        let mut arg = |name: &str, value: &dyn AsRef<std::ffi::OsStr>| {
            let mut text = OsString::from(format!("--{name}="));
            text.push(value);
            cmd.arg(text);
        };
        arg("wayland-display", &wayland);
        arg("socket", &socket);
        arg("enable-xwayland", &"false");
        arg("stderr-log-level", &"warn");
        cmd.env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .current_dir(home_dir().unwrap_or_else(|| "/".into()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0);
        if let Some(log) = launch::log_file(SERVER) {
            cmd.stderr(Stdio::from(log));
        }
        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                self.procs.children.push(child);
                tracing::info!("started {SERVER} on {wayland}");
                Some(pid)
            }
            Err(err) => {
                tracing::error!("spawn {SERVER} failed: {err}");
                None
            }
        }
    }

    /// Start a client showing the shown project's server in gerdr, once its
    /// socket is there. It is one of gerdr's children, ended with them.
    fn start_client(&mut self) {
        let Some(keeper) = self.keep.keeper.as_ref() else {
            return;
        };
        let (Some(socket), Some(control)) =
            (socket(&keeper.project), control_socket(&keeper.project))
        else {
            return;
        };
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                r#"{WAIT_FOR_SOCKET}; exec {CLIENT} --socket="$1" --control-socket="$2""#
            ))
            .arg(CLIENT)
            .arg(&socket)
            .arg(&control);
        let dir = home_dir().unwrap_or_else(|| "/".into());
        let pid = self.spawn(cmd, &dir, CLIENT);
        if let Some(keeper) = self.keep.keeper.as_mut() {
            keeper.client = pid;
            keeper.client_started = Some(Instant::now());
        }
    }

    /// The client of the shown project's server ended: start another if
    /// the server runs and the last didn't end right away.
    pub fn kept_client_exited(&mut self, exited: &[u32]) {
        let Some(keeper) = self.keep.keeper.as_mut() else {
            return;
        };
        if !keeper.client.is_some_and(|pid| exited.contains(&pid)) {
            return;
        }
        keeper.client = None;
        let soon = keeper
            .client_started
            .is_some_and(|t| t.elapsed() < CLIENT_RESTART);
        if !soon && server_runs(&keeper.display) {
            tracing::info!("{CLIENT} ended: starting it again");
            self.start_client();
        }
    }

    /// Start `cmd`, the prepared `command` of an app kept running, on the
    /// shown project's server: in a process group of its own, which gerdr
    /// doesn't end, written down for the next gerdr. Without wprs, it runs
    /// on gerdr like any other app.
    pub fn start_kept(
        &mut self,
        mut cmd: Command,
        dir: &Path,
        log: &str,
        command: &str,
    ) -> Option<u32> {
        let Some(display) = self.ensure_keeper() else {
            return self.spawn(cmd, dir, log);
        };
        let id = self.keep.keeper.as_ref()?.project.clone();
        let socket = runtime_dir()?.join(&display);
        // `sh -c <command>`: wait for the server's socket first.
        let args: Vec<OsString> = cmd.get_args().map(OsString::from).collect();
        let [flag, line] = args.as_slice() else {
            return self.spawn(cmd, dir, log);
        };
        let mut waiting = Command::new("sh");
        waiting
            .arg(flag)
            .arg(format!(
                "{WAIT_FOR_SOCKET}; shift; {}",
                line.to_string_lossy()
            ))
            .arg("sh")
            .arg(&socket);
        for (key, value) in cmd.get_envs() {
            match value {
                Some(value) => waiting.env(key, value),
                None => waiting.env_remove(key),
            };
        }
        cmd = waiting;
        let pid = self.spawn_kept(cmd, dir, log, &display)?;
        let mut record = read(&id);
        record.apps.push(KeptApp {
            command: command.to_owned(),
            process: Process::new(pid),
        });
        write(&id, &record);
        if let Some(keeper) = self.keep.keeper.as_mut() {
            keeper.pids.push(pid);
        }
        Some(pid)
    }

    /// The pid of an app the shown project's server runs for `command` and
    /// no pane has yet: a restored pane takes it instead of starting one.
    pub fn take_kept(&mut self, command: &str) -> Option<u32> {
        let keeper = self.keep.keeper.as_ref()?;
        let record = read(&keeper.project);
        let pid = record
            .apps
            .iter()
            .find(|app| app.command == command.trim() && !keeper.pids.contains(&app.process.pid))?
            .process
            .pid;
        self.keep.keeper.as_mut()?.pids.push(pid);
        Some(pid)
    }

    /// The pid of an app the shown project's server runs for `command`,
    /// whose other windows come back to their panes too.
    pub fn kept_pid(&self, command: &str) -> Option<u32> {
        let keeper = self.keep.keeper.as_ref()?;
        read(&keeper.project)
            .apps
            .iter()
            .find(|app| app.command == command.trim())
            .map(|app| app.process.pid)
    }

    /// The command that started the app on the shown project's server a
    /// window of app_id `app_id` belongs to: the one whose program the
    /// app_id names, else the first.
    pub fn kept_command(&self, app_id: &str) -> Option<String> {
        let keeper = self.keep.keeper.as_ref()?;
        let apps = read(&keeper.project).apps;
        let named = apps.iter().find(|app| {
            launch::program_name(&app.command)
                .is_some_and(|program| app_id.contains(&program.to_ascii_lowercase()))
        });
        named.or(apps.first()).map(|app| app.command.clone())
    }

    /// Whether `pid` is an app on the shown project's server.
    pub fn is_kept(&self, pid: Option<u32>) -> bool {
        self.keep
            .keeper
            .as_ref()
            .is_some_and(|k| pid.is_some_and(|pid| k.pids.contains(&pid)))
    }

    /// Whether `window` comes from the shown project's server, through its
    /// client: an app kept running.
    pub fn is_kept_window(&self, window: &smithay::desktop::Window) -> bool {
        let lineage = self
            .window_pid(window)
            .map(launch::lineage)
            .unwrap_or_default();
        self.from_keeper(&lineage)
    }

    /// Whether a window whose client descends from `lineage` comes from
    /// the shown project's server, through its client.
    pub fn from_keeper(&self, lineage: &[u32]) -> bool {
        self.keep
            .keeper
            .as_ref()
            .and_then(|k| k.client)
            .is_some_and(|client| lineage.contains(&client))
    }

    /// Hear the hooks of a Cursor on the shown project's server, which
    /// sends them by its display (see `cursor_agents::run_hook`).
    fn hear_kept_cursor(&mut self) {
        let Some(display) = self.keep.keeper.as_ref().map(|k| k.display.clone()) else {
            return;
        };
        let Some(path) = cursor_agents::hook_socket(std::ffi::OsStr::new(&display)) else {
            return;
        };
        if let Some(token) = self.listen_cursor_hooks(&path)
            && let Some(keeper) = self.keep.keeper.as_mut()
        {
            keeper.hooks = Some((path, token));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_is_known_right_after_it_starts_until_it_ends() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5; true"])
            .process_group(0)
            .spawn()
            .unwrap();
        let process = Process::new(child.id());
        assert!(process.runs());
        let _ = child.kill();
        let _ = child.wait();
        assert!(!process.runs());
        let reused = Process {
            start: process.start + 1,
            ..process
        };
        assert!(!reused.runs());
    }

    #[test]
    fn project_ids_make_safe_names() {
        assert_eq!(display("ao-app"), "gerdr-keep-ao-app");
        assert_eq!(slug("a b/c"), "a_b_c");
    }
}
