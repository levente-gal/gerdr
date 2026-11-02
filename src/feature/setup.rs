//! Setting up a feature's worktrees after they are made: the commands the
//! project's store file gives per repository (`"workflow": {"setup":
//! {"services-api": ["npm ci"]}}`), run in that repository's worktree.
//!
//! Before a repository's commands, each folder of `REFLINK` its main
//! worktree has and its feature worktree lacks is copied over with
//! `cp -a --reflink=always`: on a copy-on-write file system that is
//! instant and takes no space, and `npm ci` then has little to do. Where
//! the file system can't, the copy fails, its leftovers go, and the
//! commands run as they would.
//!
//! Every command runs with `sh -c`, stdin closed, its output appended to
//! `setup.log` in the feature data dir, for at most `TIMEOUT`. The first
//! one that fails stops the setup: its repository, command and exit go in
//! the error. Each command runs in a process group of its own, and the
//! group is killed when it ends or times out, so nothing it started
//! lingers (`shell`, which Verify's checks run with too).
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{Feature, layout::shown, now};

/// The folders copied from a repository's main worktree before its setup.
pub const REFLINK: &[&str] = &["node_modules"];

/// How long one setup command may run.
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// The setup log's name, in the feature data dir.
pub const LOG: &str = "setup.log";

/// Where a feature's setup stands, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRun {
    /// Unix seconds.
    pub started: u64,
    /// When it ended, either way (unix seconds); `None` while it runs, or
    /// when gerdr quit before it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// Why it failed: "services-api: `npm ci` exited with 1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// Whether `commands` (the store file's setup, by repository name) give
/// `feature` anything to run, `REFLINK` copies aside.
pub fn has_commands(feature: &Feature, commands: &BTreeMap<String, Vec<String>>) -> bool {
    feature
        .repos
        .iter()
        .any(|repo| repo.worktree.is_dir() && !commands_of(commands, &repo.name).is_empty())
}

/// The commands `commands` give repository `name`, blank ones left out.
fn commands_of<'a>(commands: &'a BTreeMap<String, Vec<String>>, name: &str) -> Vec<&'a str> {
    commands
        .get(name)
        .into_iter()
        .flatten()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect()
}

/// Set up every repository of `feature` with a worktree: the `REFLINK`
/// copies, then its `commands`, logging to `log`. `progress` is told
/// what runs, in words: "services-api: npm ci (2 of 3)".
pub fn run(
    feature: &Feature,
    commands: &BTreeMap<String, Vec<String>>,
    log: &Path,
    progress: &mut dyn FnMut(String),
) -> Result<(), String> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("couldn't make {}: {err}", shown(parent)))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|err| format!("couldn't open {}: {err}", shown(log)))?;
    let mut repos: Vec<_> = feature
        .repos
        .iter()
        .filter(|r| r.worktree.is_dir())
        .collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    let total: usize = repos
        .iter()
        .map(|r| commands_of(commands, &r.name).len())
        .sum();
    let mut i = 0;
    for repo in repos {
        let name = &repo.name;
        for folder in REFLINK {
            let from = repo.main.join(folder);
            let to = repo.worktree.join(folder);
            if !from.exists() || to.symlink_metadata().is_ok() {
                continue;
            }
            progress(format!("{name}: copying {folder}"));
            let copied = Command::new("cp")
                .arg("-a")
                .arg("--reflink=always")
                .arg(&from)
                .arg(&to)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output();
            let note = match copied {
                Ok(out) if out.status.success() => format!("copied {folder} from {}", shown(&from)),
                failed => {
                    // Leftovers of a half copy would only confuse the
                    // commands: they make the folder themselves.
                    let _ = std::fs::remove_dir_all(&to);
                    let _ = std::fs::remove_file(&to);
                    let why = match failed {
                        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                        Err(err) => err.to_string(),
                    };
                    format!(
                        "couldn't copy {folder} from {} ({why}); going on",
                        shown(&from)
                    )
                }
            };
            let _ = writeln!(file, "== {} {name}: {note}", super::verify::utc(now()));
        }
        for command in commands_of(commands, name) {
            i += 1;
            progress(format!("{name}: {command} ({i} of {total})"));
            let _ = writeln!(file, "== {} {name}: {command}", super::verify::utc(now()));
            let _ = file.flush();
            let out = file
                .try_clone()
                .map_err(|err| format!("couldn't write to {}: {err}", shown(log)))?;
            let err = file
                .try_clone()
                .map_err(|err| format!("couldn't write to {}: {err}", shown(log)))?;
            let started = Instant::now();
            let exit = shell(
                command,
                &repo.worktree,
                &feature.slug,
                Stdio::from(out),
                Stdio::from(err),
                TIMEOUT,
            );
            let secs = started.elapsed().as_secs();
            let _ = writeln!(file, "== {} after {secs} s\n", exit.describe());
            if !exit.passed() {
                return Err(format!("{name}: `{command}` {}", exit.describe()));
            }
        }
    }
    Ok(())
}

/// How a shell command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Exit {
    /// It exited with this code; 128 + the signal when a signal killed it,
    /// as shells say.
    Code(i32),
    /// It ran past its time and was killed, after this long.
    TimedOut(Duration),
    /// It couldn't start, for this reason.
    Failed(String),
}

impl Exit {
    /// Whether it exited with 0.
    pub(super) fn passed(&self) -> bool {
        *self == Exit::Code(0)
    }

    /// How it ended, after its command: "exited with 1", "timed out
    /// after 20 min", "couldn't start: …".
    pub(super) fn describe(&self) -> String {
        match self {
            Exit::Code(code) => format!("exited with {code}"),
            Exit::TimedOut(after) => format!("timed out after {}", minutes(*after)),
            Exit::Failed(why) => format!("couldn't start: {why}"),
        }
    }
}

/// A timeout in words: "20 min", or "45 s" under a minute.
pub(super) fn minutes(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min", (secs + 30) / 60)
    }
}

/// Run `command` with `sh -c` in `dir`, stdin closed, its output to
/// `stdout` and `stderr`, `GERDR_FEATURE` set to `slug`, for at most
/// `timeout`. It runs in a process group of its own: past the timeout
/// the whole group is killed, so what it started dies with it, and so
/// does anything it left running in the background once it exits.
pub(super) fn shell(
    command: &str,
    dir: &Path,
    slug: &str,
    stdout: Stdio,
    stderr: Stdio,
    timeout: Duration,
) -> Exit {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .env("GERDR_FEATURE", slug)
        .process_group(0);
    let child = cmd.spawn();
    // Our copies of the output's ends go, so a reader sees its end once
    // the command's do.
    drop(cmd);
    let mut child = match child {
        Ok(child) => child,
        Err(err) => return Exit::Failed(err.to_string()),
    };
    let group = child.id() as i32;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        kill_group(group, libc::SIGTERM);
        let grace = Instant::now();
        while grace.elapsed() < Duration::from_secs(2) {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        kill_group(group, libc::SIGKILL);
        let _ = child.wait();
        return Exit::TimedOut(timeout);
    };
    // What it left behind in the background.
    kill_group(group, libc::SIGKILL);
    match (status.code(), status.signal()) {
        (Some(code), _) => Exit::Code(code),
        (None, Some(signal)) => Exit::Code(128 + signal),
        (None, None) => Exit::Code(-1),
    }
}

/// Send `signal` to every process of process group `group`.
fn kill_group(group: i32, signal: i32) {
    if group > 0 {
        // SAFETY: kill takes no pointers; a group already gone is ESRCH.
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

/// The log of the feature with data dir `data`.
pub fn log_path(data: &Path) -> PathBuf {
    data.join(LOG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::tests::{Temp, feature_over};

    fn map(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.iter().map(|c| (*c).into()).collect()))
            .collect()
    }

    #[test]
    fn commands_count_for_repos_with_a_worktree() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &["web"]);
        assert!(has_commands(&f, &map(&[("web", &["npm ci"])])));
        assert!(!has_commands(&f, &map(&[("web", &[" "])])));
        assert!(!has_commands(&f, &map(&[("other", &["npm ci"])])));
        std::fs::remove_dir_all(&f.repos[1].worktree).unwrap();
        assert!(!has_commands(&f, &map(&[("web", &["npm ci"])])));
    }

    #[test]
    fn commands_run_in_order_and_the_first_failure_stops() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web", "zzz"], &[]);
        let log = t.0.join("data/setup.log");
        let commands = map(&[
            (
                "api",
                &["echo $GERDR_FEATURE > slug", "echo out; echo err >&2"],
            ),
            ("web", &["exit 3"]),
            ("zzz", &["touch never"]),
        ]);
        let mut said = Vec::new();
        let result = run(&f, &commands, &log, &mut |s| said.push(s));
        assert_eq!(result, Err("web: `exit 3` exited with 3".to_owned()));
        assert_eq!(
            said,
            [
                "api: echo $GERDR_FEATURE > slug (1 of 4)",
                "api: echo out; echo err >&2 (2 of 4)",
                "web: exit 3 (3 of 4)",
            ]
        );
        let api = &f.repos[0].worktree;
        assert_eq!(std::fs::read_to_string(api.join("slug")).unwrap(), "f\n");
        assert!(!f.repos[2].worktree.join("never").exists());
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains("api: echo out; echo err >&2\nout\nerr\n"),
            "{text}"
        );
        assert!(text.contains("== exited with 3 after"), "{text}");
        // A second run appends.
        run(&f, &map(&[("api", &["true"])]), &log, &mut |_| {}).unwrap();
        let again = std::fs::read_to_string(&log).unwrap();
        assert!(again.starts_with(&text) && again.len() > text.len());
    }

    #[test]
    fn folders_are_copied_from_main_or_left_to_the_commands() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        let (api, web) = (&f.repos[0], &f.repos[1]);
        std::fs::create_dir_all(api.main.join("node_modules/x")).unwrap();
        std::fs::write(api.main.join("node_modules/x/i.js"), "1").unwrap();
        // A worktree that has the folder keeps its own.
        std::fs::create_dir_all(web.main.join("node_modules")).unwrap();
        std::fs::create_dir_all(web.worktree.join("node_modules")).unwrap();
        std::fs::write(web.worktree.join("node_modules/own"), "").unwrap();
        let log = t.0.join("setup.log");
        let mut said = Vec::new();
        run(&f, &BTreeMap::new(), &log, &mut |s| said.push(s)).unwrap();
        assert_eq!(said, ["api: copying node_modules"]);
        let text = std::fs::read_to_string(&log).unwrap();
        let copied = api.worktree.join("node_modules/x/i.js");
        // Copy-on-write or not, depending on the file system under /tmp.
        if text.contains("couldn't copy node_modules") {
            assert!(!api.worktree.join("node_modules").exists());
        } else {
            assert!(text.contains("api: copied node_modules"), "{text}");
            assert_eq!(std::fs::read_to_string(copied).unwrap(), "1");
        }
        assert!(web.worktree.join("node_modules/own").exists());
    }

    #[test]
    fn a_command_past_its_time_dies_with_what_it_started() {
        let t = Temp::new();
        let started = Instant::now();
        let exit = shell(
            "sleep 30 & echo $! > bg; sleep 30",
            &t.0,
            "f",
            Stdio::null(),
            Stdio::null(),
            Duration::from_millis(300),
        );
        assert_eq!(exit, Exit::TimedOut(Duration::from_millis(300)));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(exit.describe(), "timed out after 0 s");
        let bg = std::fs::read_to_string(t.0.join("bg")).unwrap();
        let proc = PathBuf::from(format!("/proc/{}", bg.trim()));
        let gone = Instant::now();
        // Killed; init reaps it soon after.
        while proc.exists() && gone.elapsed() < Duration::from_secs(5) {
            let state = std::fs::read_to_string(proc.join("stat")).unwrap_or_default();
            if state.contains(") Z") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let state = std::fs::read_to_string(proc.join("stat")).unwrap_or_default();
        assert!(state.is_empty() || state.contains(") Z"), "{state}");
    }

    #[test]
    fn exits_in_words() {
        let t = Temp::new();
        let run = |c: &str| shell(c, &t.0, "f", Stdio::null(), Stdio::null(), TIMEOUT);
        assert!(run("true").passed());
        assert_eq!(run("exit 4"), Exit::Code(4));
        assert_eq!(run("kill -9 $$"), Exit::Code(137));
        let gone = shell(
            "true",
            &t.0.join("nope"),
            "f",
            Stdio::null(),
            Stdio::null(),
            TIMEOUT,
        );
        assert!(gone.describe().starts_with("couldn't start: "), "{gone:?}");
        assert_eq!(minutes(TIMEOUT), "20 min");
        assert_eq!(minutes(Duration::from_secs(45)), "45 s");
    }
}
