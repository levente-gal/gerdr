//! T3 Code's server, run by gerdr in the background rather than in a tab:
//! in a session of its own, so it outlives gerdr as herdr's server does,
//! writing what it says to `~/.local/state/gerdr/launch/t3.log`. What gerdr
//! started is written down in `$XDG_RUNTIME_DIR/gerdr/t3-server.json`, so
//! every gerdr tells it from a server started elsewhere (a terminal, say)
//! and finds its log. The footer lists it with the other background tasks
//! (see `tasks`); quitting stops it if asked (see `quit`).

use std::{
    io::Write,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{Gerdr, launch, process, t3_agents::Server, toast::Level};

/// The name its log goes by (see `launch::log_path`).
pub const LOG: &str = "t3";
/// How long the server is given to stop before it is killed.
const STOP_TIME: Duration = Duration::from_secs(10);
/// How often a stopping server is looked at.
const STOP_POLL: Duration = Duration::from_millis(250);
/// A server ending this soon after it started didn't start: said, with
/// what its log says last.
const EARLY: Duration = Duration::from_secs(10);
/// How soon after starting a server T3 Code's home is looked for, when it
/// had none: the server makes it.
const HOME_LOOK: Duration = Duration::from_secs(3);

/// A server gerdr started, as written down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    /// Its process, which leads its session and process group.
    #[serde(flatten)]
    pub process: process::Process,
    /// Where it writes what it says.
    pub log: PathBuf,
    /// When it started, in seconds since 1970.
    pub at: u64,
}

impl Started {
    fn runs(&self) -> bool {
        self.process.runs()
    }
}

fn record_file() -> Option<PathBuf> {
    Some(crate::herdr::runtime_dir()?.join("gerdr/t3-server.json"))
}

/// The server gerdr started, while it runs.
pub fn started() -> Option<Started> {
    let text = std::fs::read(record_file()?).ok()?;
    let started: Started = serde_json::from_slice(&text).ok()?;
    started.runs().then_some(started)
}

fn write(started: &Started) {
    let Some(path) = record_file() else {
        return;
    };
    let written = serde_json::to_vec(started)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::projects::write_atomic(&path, &text)
        });
    if let Err(err) = written {
        tracing::warn!("writing {}: {err}", path.display());
    }
}

fn forget() {
    if let Some(path) = record_file() {
        let _ = std::fs::remove_file(path);
    }
}


/// The last line of the log at `path` that says something.
pub fn last_log_line(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read(path).ok()?;
    let tail = &text[text.len().saturating_sub(4096)..];
    String::from_utf8_lossy(tail)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with("--- gerdr"))
        .map(str::to_owned)
}

/// A shell script killing `target` (a pid, or a process group as a
/// negative one) unless it is gone within `STOP_TIME`.
fn kill_later(target: i32) -> String {
    let polls = STOP_TIME.as_millis() / STOP_POLL.as_millis();
    let poll = STOP_POLL.as_secs_f64();
    format!(
        "i=0; while kill -0 -- {target} 2>/dev/null; do \
         i=$((i+1)); [ $i -ge {polls} ] && exec kill -s KILL -- {target}; \
         sleep {poll}; done"
    )
}

/// What to signal to stop the server: the session gerdr started it in
/// (as a negative pid), else its own process when it is a `t3`.
fn target(server: Option<&Server>) -> Option<i32> {
    if let Some(started) = started() {
        // Never init's group, should the record say pid 1: `kill_later`
        // signals it from a shell, without `kill_group`'s guard.
        return i32::try_from(started.process.pid)
            .ok()
            .filter(|&pid| pid > 1)
            .map(|pid| -pid);
    }
    let server = server.filter(|s| s.alive())?;
    if !crate::t3_update::is_t3(server.pid) {
        tracing::warn!("t3 code's server: pid {} isn't t3's", server.pid);
        return None;
    }
    i32::try_from(server.pid).ok()
}

/// Send `signal` to `target`: the server's session gerdr started (a
/// negative group id), or the server's process, checked to be t3's.
fn signal(target: i32, signal: i32) {
    if target < 0 {
        process::kill_group(-target, signal);
    } else {
        process::kill_pid(target as u32, signal);
    }
}

/// Whether what `target` signals still runs.
fn runs(target: i32, server: Option<&Server>) -> bool {
    if target < 0 {
        started().is_some() || server.is_some_and(Server::alive)
    } else {
        Server::alive_pid(target as u32)
    }
}

impl Gerdr {
    /// Whether T3 Code's server runs, wherever it was started.
    pub fn t3_runs(&self) -> bool {
        self.t3.alive || started().is_some()
    }

    /// Start T3 Code's server in the background, at `port` if any (where a
    /// restarted one ran), unless one runs.
    pub fn spawn_t3(&mut self, port: Option<u16>) {
        if self.t3_runs() {
            return self.toast("T3 Code is running".to_owned(), Level::Info);
        }
        let command = self.t3_command(port);
        let log_path = launch::log_path(LOG);
        let Some(mut log) = launch::append_log(&log_path) else {
            let text = "T3 Code can't start: its log can't be written".to_owned();
            return self.toast(text, Level::Error);
        };
        let _ = writeln!(log, "--- gerdr starts T3 Code: {command}");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!("exec {command}"));
        crate::herdr::scrub_env(&mut cmd, None);
        cmd.current_dir(crate::home_dir().unwrap_or_else(|| "/".into()))
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_or(Stdio::null(), Stdio::from))
            .stderr(Stdio::from(log));
        // Its own session, out of gerdr's process groups: it outlives
        // gerdr, and one signal to the group stops it with its helpers.
        // Safety: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                self.procs.children.push(child);
                write(&Started {
                    process: process::Process::new(pid),
                    log: log_path,
                    at: crate::clock::now_secs(),
                });
                self.t3.started_at = Some(Instant::now());
                self.look_at_kept();
                tracing::info!("started t3 code's server: {command}");
                self.toast("Starting T3 Code".to_owned(), Level::Info);
                if self.t3.home.is_none() {
                    self.look_for_t3_home();
                }
            }
            Err(err) => {
                tracing::error!("starting t3 code's server: {err}");
                self.toast(format!("T3 Code can't start: {err}"), Level::Error);
            }
        }
    }

    /// Follow T3 Code once the server just started made its home.
    fn look_for_t3_home(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let look = |_, _: &mut (), state: &mut Gerdr| {
            if state.t3.home.is_none() && crate::t3_agents::home().is_some() {
                state.watch_t3_agents();
            }
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(HOME_LOOK), look)
        {
            tracing::warn!("t3 code's home: {err}");
        }
    }

    /// Children of gerdr's that ended: say so when it was T3 Code's server
    /// just started.
    pub fn t3_child_exited(&mut self, exited: &[u32]) {
        let Some(at) = self.t3.started_at else {
            return;
        };
        let record = record_file()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|text| serde_json::from_slice::<Started>(&text).ok());
        let Some(record) = record.filter(|r| exited.contains(&r.process.pid)) else {
            return;
        };
        self.t3.started_at = None;
        forget();
        self.t3_started = None;
        // Stopped by gerdr: nothing to tell.
        if self.t3.stopping {
            return;
        }
        let why = last_log_line(&record.log).unwrap_or_else(|| "it ended".to_owned());
        let text = if at.elapsed() < EARLY {
            format!("T3 Code didn't start: {why}")
        } else {
            format!("T3 Code's server stopped: {why}")
        };
        tracing::warn!("{text}");
        let action = (
            "Open log".to_owned(),
            crate::menu::MenuAction::ShowLog(record.log),
        );
        self.toast_with(text, Level::Error, Some(action));
    }

    /// Stop T3 Code's server: asked, killed after `STOP_TIME`; then `then`.
    /// Waits off the event loop's back, by a timer.
    pub fn stop_t3(&mut self, then: impl FnOnce(&mut Gerdr) + 'static) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let server = self.t3.server.clone();
        let Some(target) = target(server.as_ref()) else {
            // A server gerdr can't tell is T3 Code's is left alone.
            if self.t3.alive {
                let text = "Couldn't stop T3 Code's server: it doesn't run as t3. \
                            Stop it where it was started";
                let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
                return self.toast_with(text.to_owned(), Level::Error, Some(open_log));
            }
            return then(self);
        };
        self.t3.stopping = true;
        signal(target, libc::SIGTERM);
        let asked_at = Instant::now();
        let mut then = Some(then);
        let wait = move |_, _: &mut (), state: &mut Gerdr| {
            if runs(target, server.as_ref()) {
                if asked_at.elapsed() < STOP_TIME {
                    return TimeoutAction::ToDuration(STOP_POLL);
                }
                signal(target, libc::SIGKILL);
            }
            state.t3_stopped();
            if let Some(then) = then.take() {
                then(state);
            }
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(STOP_POLL), wait)
        {
            tracing::warn!("t3 code's server: {err}");
        }
    }

    /// The same while gerdr quits: asked here, killed after `STOP_TIME`
    /// by a helper in a session of its own, so quitting doesn't wait.
    pub fn stop_t3_now(&mut self) {
        let server = self.t3.server.clone();
        let Some(target) = target(server.as_ref()) else {
            return;
        };
        signal(target, libc::SIGTERM);
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(kill_later(target))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Safety: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Err(err) = cmd.spawn() {
            tracing::warn!("t3 code's server: no helper to kill it later: {err}");
        }
        self.t3_stopped();
    }

    /// The server is gone.
    fn t3_stopped(&mut self) {
        forget();
        self.t3_started = None;
        self.t3.stopping = false;
        self.t3.started_at = None;
        if self.t3.alive {
            self.t3.alive = false;
            self.t3_changed();
        }
    }

    /// Stop the server and start it again, at the version pinned; at the
    /// version waiting for it when there is one.
    pub fn restart_t3(&mut self) {
        let version = self.t3.restart_at.take();
        let port = self.t3.server.as_ref().and_then(Server::port);
        // A server gerdr can't tell is T3 Code's is left alone.
        if self.t3.alive && target(self.t3.server.as_ref()).is_none() {
            let text = match &version {
                Some(version) => format!("Couldn't restart T3 Code at {version}"),
                None => "Couldn't restart T3 Code".to_owned(),
            };
            let open_log = ("Open log".to_owned(), crate::menu::MenuAction::OpenLog);
            return self.toast_with(text, Level::Error, Some(open_log));
        }
        let text = match &version {
            Some(version) => format!("Restarting T3 Code at {version}"),
            None => "Restarting T3 Code".to_owned(),
        };
        self.toast(text, Level::Info);
        self.stop_t3(move |state| state.spawn_t3(port));
    }

    /// Show T3 Code's log in a new tab of the shown workspace.
    pub fn show_log(&mut self, path: PathBuf) {
        let w = self.active_workspace;
        let Some(dir) = self.workspaces.get(w).map(|ws| ws.dir.clone()) else {
            return;
        };
        let path = crate::desktop::shell_quote(&path.to_string_lossy());
        self.new_tab_running(w, &dir, format!("less +F {path}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_line_skips_blanks_and_gerdrs_own() {
        let dir = std::env::temp_dir().join(format!("gerdr-t3-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t3.log");
        std::fs::write(
            &path,
            "--- gerdr starts T3 Code: t3 serve\nerror: port taken\n\n",
        )
        .unwrap();
        assert_eq!(last_log_line(&path).as_deref(), Some("error: port taken"));
        std::fs::write(&path, "--- gerdr starts T3 Code: t3 serve\n").unwrap();
        assert_eq!(last_log_line(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_left_running_is_killed_by_the_helper() {
        let mut group = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let target = -(group.id() as i32);
        // Ten seconds would make a slow test: the script is the same.
        let script = kill_later(target).replace("-ge 40", "-ge 1");
        assert_ne!(script, kill_later(target));
        let status = Command::new("sh").arg("-c").arg(script).status().unwrap();
        assert!(status.success());
        assert!(!group.wait().unwrap().success());
    }

    #[test]
    fn a_record_reads_back() {
        let started = Started {
            process: process::Process::new(std::process::id()),
            log: "/tmp/t3.log".into(),
            at: 5,
        };
        let text = serde_json::to_vec(&started).unwrap();
        let back: Started = serde_json::from_slice(&text).unwrap();
        assert_eq!(back, started);
        assert!(back.runs());
        let gone = Started {
            process: process::Process::new(u32::MAX - 1),
            ..started
        };
        assert!(!gone.runs());
    }
}
