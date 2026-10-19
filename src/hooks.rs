//! A project's start and stop commands: a shell line run before its session
//! starts, when it isn't running yet (`docker compose up -d`), and one run
//! after its session stops (`docker compose down`). Each runs with `sh -c`
//! in the project's folder, off the event loop, its output kept in a log
//! file under `~/.local/state/gerdr/hooks`; one that fails is told by the
//! last line it wrote.

use std::{
    fs::File,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use smithay::reexports::calloop::channel;

use crate::{common_dir, herdr, home_dir, projects::Project};

/// How long the start commands may take: pulling images is slow.
pub const BEFORE_START_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long the stop commands may take.
pub const AFTER_STOP_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// How often a running command is checked on.
const POLL: Duration = Duration::from_millis(100);

/// Which of a project's commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    BeforeStart,
    AfterStop,
}

impl Hook {
    /// What notices call it.
    pub fn label(self) -> &'static str {
        match self {
            Hook::BeforeStart => "start commands",
            Hook::AfterStop => "stop commands",
        }
    }

    /// Its log file's name part.
    fn slug(self) -> &'static str {
        match self {
            Hook::BeforeStart => "before-start",
            Hook::AfterStop => "after-stop",
        }
    }

    /// How long it may take.
    fn timeout(self) -> Duration {
        match self {
            Hook::BeforeStart => BEFORE_START_TIMEOUT,
            Hook::AfterStop => AFTER_STOP_TIMEOUT,
        }
    }

    /// Project `project`'s line for it; empty for none.
    pub fn line(self, project: &Project) -> &str {
        match self {
            Hook::BeforeStart => project.before_start.trim(),
            Hook::AfterStop => project.after_stop.trim(),
        }
    }
}

/// A finished run: the project's id, which commands, and why they failed.
pub type Done = (String, Hook, Result<(), String>);

/// Where `project`'s commands run: the folder holding all its workspaces,
/// else its first workspace's, else the home folder.
pub fn project_dir(project: &Project) -> PathBuf {
    let dirs: Vec<&Path> = project
        .workspaces
        .iter()
        .map(|ws| ws.dir.as_path())
        .collect();
    common_dir(&dirs)
        .filter(|dir| dir.parent().is_some() && dir.is_dir())
        .or_else(|| dirs.first().map(|dir| dir.to_path_buf()))
        .or_else(home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The log file of project `id`'s `hook`.
pub fn log_file(id: &str, hook: Hook) -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home_dir().map(|home| home.join(".local/state")))
        .unwrap_or_default();
    state
        .join("gerdr/hooks")
        .join(format!("{id}-{}.log", hook.slug()))
}

/// `sh -c line` in `dir`, its output to `log` (made anew), in a process
/// group of its own.
fn command(line: &str, dir: &Path, log: &Path) -> std::io::Result<Command> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = File::create(log)?;
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(line)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    herdr::scrub_env(&mut cmd, None);
    Ok(cmd)
}

/// Run `line` with `sh -c` in `dir`, its output to `log`, killing it (and
/// what it started) after `timeout`. An error says why it failed: the last
/// line it wrote, else how it ended.
pub fn run(line: &str, dir: &Path, log: &Path, timeout: Duration) -> Result<(), String> {
    let mut cmd = command(line, dir, log).map_err(|err| err.to_string())?;
    let mut child = cmd
        .process_group(0)
        .spawn()
        .map_err(|err| err.to_string())?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => {
                // Safety: signals the process group it leads.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            Err(err) => return Err(err.to_string()),
        }
    };
    if status.success() {
        return Ok(());
    }
    let written = std::fs::read_to_string(log).unwrap_or_default();
    Err(last_line(&written).map_or_else(|| status.to_string(), str::to_owned))
}

/// The last line of `text` with something on it.
fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

/// Run project `id`'s `hook`, `line`, in `dir` on a thread of its own; `tx`
/// gets how it went.
pub fn spawn(id: String, hook: Hook, line: String, dir: PathBuf, tx: channel::Sender<Done>) {
    let spawned = std::thread::Builder::new()
        .name(format!("project {}", hook.slug()))
        .spawn(move || {
            let log = log_file(&id, hook);
            let result = run(&line, &dir, &log, hook.timeout());
            if let Err(err) = &result {
                tracing::warn!(
                    "project {id}'s {} failed: {err} (see {})",
                    hook.label(),
                    log.display()
                );
            }
            let _ = tx.send((id, hook, result));
        });
    if let Err(err) = spawned {
        tracing::warn!("running the project's {}: {err}", hook.label());
    }
}

/// Start project `id`'s `hook`, `line`, in `dir` in a session of its own
/// and leave it: gerdr is quitting.
pub fn spawn_detached(id: &str, hook: Hook, line: &str, dir: &Path) {
    let started = command(line, dir, &log_file(id, hook)).and_then(|mut cmd| {
        // Safety: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn()
    });
    if let Err(err) = started {
        tracing::warn!("running project {id}'s {}: {err}", hook.label());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::projects::{Kind, RunState, WorkspaceRecord};

    /// A folder of its own under the system's temporary one, removed when
    /// dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-hooks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const LONG: Duration = Duration::from_secs(10);

    #[test]
    fn runs_in_the_folder_and_logs_its_output() {
        let tmp = TempDir::new();
        let log = tmp.0.join("logs/run.log");
        run("echo hi && pwd > here", &tmp.0, &log, LONG).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "hi\n");
        let here = std::fs::read_to_string(tmp.0.join("here")).unwrap();
        assert_eq!(Path::new(here.trim()), tmp.0.canonicalize().unwrap());
    }

    #[test]
    fn a_failure_says_its_last_line() {
        let tmp = TempDir::new();
        let log = tmp.0.join("run.log");
        let err = run(
            "echo pulling; echo 'no such service: db' >&2; exit 3",
            &tmp.0,
            &log,
            LONG,
        );
        assert_eq!(err, Err("no such service: db".into()));
        let silent = run("exit 3", &tmp.0, &log, LONG).unwrap_err();
        assert!(silent.contains('3'), "{silent}");
    }

    #[test]
    fn one_that_takes_too_long_is_killed() {
        let tmp = TempDir::new();
        let log = tmp.0.join("run.log");
        let started = Instant::now();
        let err = run("sleep 30", &tmp.0, &log, Duration::from_millis(300)).unwrap_err();
        assert!(err.starts_with("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    fn project(dirs: &[&Path]) -> Project {
        let workspaces = dirs
            .iter()
            .map(|dir| WorkspaceRecord {
                id: "w1".into(),
                herdr_id: Some("w1".into()),
                label: "ws".into(),
                kind: Kind::Project,
                dir: dir.to_path_buf(),
                venv: Default::default(),
                tabs: Vec::new(),
                panes: Vec::new(),
            })
            .collect();
        Project {
            id: "a".into(),
            name: "A".into(),
            session: "gerdr-a".into(),
            created: 0,
            last_used: 0,
            state: RunState::Kept,
            workspaces,
            before_start: " docker compose up -d ".into(),
            after_stop: String::new(),
        }
    }

    #[test]
    fn commands_run_in_the_folder_holding_the_workspaces() {
        let tmp = TempDir::new();
        let (api, web) = (tmp.0.join("api"), tmp.0.join("web"));
        std::fs::create_dir_all(&api).unwrap();
        std::fs::create_dir_all(&web).unwrap();
        assert_eq!(project_dir(&project(&[&api, &web])), tmp.0);
        assert_eq!(project_dir(&project(&[&api])), api);
        // Nothing in common but the root: the first workspace's.
        let elsewhere = Path::new("/");
        assert_eq!(project_dir(&project(&[&api, elsewhere])), api);
    }

    #[test]
    fn a_line_is_trimmed_and_empty_for_none() {
        let p = project(&[]);
        assert_eq!(Hook::BeforeStart.line(&p), "docker compose up -d");
        assert_eq!(Hook::AfterStop.line(&p), "");
    }
}
