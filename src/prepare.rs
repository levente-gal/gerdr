//! Getting a herdr pane's shell ready: activate the repository's Python venv,
//! bring back `mise` version overrides, and (when the pane comes back after
//! its server restarted) run again what ran in it. Each pane gets one line,
//! typed in as soon as its shell exists: the pane is brand new, so nobody
//! typed in it yet, and the shell keeps the line as type-ahead until its
//! first prompt. It runs after the shell's rc files (and `mise activate`),
//! so the venv's `bin` comes first in PATH.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    desktop::shell_quote,
    herdr::{Herdr, Process, ProcessInfo},
    projects::VenvSetting,
};

/// How often the panes are asked about.
const POLL: Duration = Duration::from_millis(150);
/// How long the panes are waited for, all together. A pane whose shell
/// didn't start by then is left alone.
const DEADLINE: Duration = Duration::from_secs(10);
/// Shells that can lead a pane's foreground without anything else running
/// (one started in place of the first, or from its rc files).
const SHELLS: [&str; 16] = [
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "oksh", "yash", "tcsh", "csh", "nu",
    "elvish", "xonsh", "ion", "pwsh",
];

/// What to do in one pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prep {
    pub pane_id: String,
    /// The pane's folder.
    pub pane_cwd: PathBuf,
    /// The workspace's folder: `find_venv` looks no higher.
    pub ws_dir: PathBuf,
    pub venv: VenvSetting,
    /// The environment captured from what ran in the pane
    /// (`PaneRecord::env`): its `VIRTUAL_ENV` wins over `venv`.
    pub captured_env: Vec<(String, String)>,
    /// Where to `cd` first, if the command ran elsewhere than `pane_cwd`.
    pub cd: Option<PathBuf>,
    /// What to run, after a restart.
    pub command: Option<String>,
}

/// The venv a shell in `pane_cwd` uses: the first `.venv` or `venv` folder
/// with a `bin/activate`, looking from `pane_cwd` up to `ws_dir`.
pub fn find_venv(pane_cwd: &Path, ws_dir: &Path) -> Option<PathBuf> {
    // A pane outside its workspace's folder only looks in its own.
    let dirs: Vec<&Path> = if pane_cwd.starts_with(ws_dir) {
        pane_cwd
            .ancestors()
            .take_while(|dir| dir.starts_with(ws_dir))
            .collect()
    } else {
        vec![pane_cwd]
    };
    dirs.into_iter()
        .flat_map(|dir| [dir.join(".venv"), dir.join("venv")])
        .find(|venv| is_venv(venv))
}

/// Whether `dir` is a venv that can be activated.
fn is_venv(dir: &Path) -> bool {
    dir.join("bin/activate").is_file()
}

/// `path` as one word for `sh`.
fn quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

/// Whether `name` can be assigned to in `sh`.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The venv to activate in the pane, if any.
fn venv_of(prep: &Prep) -> Option<PathBuf> {
    if prep.venv == VenvSetting::Off {
        return None;
    }
    let captured = prep
        .captured_env
        .iter()
        .find(|(key, _)| key == "VIRTUAL_ENV")
        .map(|(_, dir)| PathBuf::from(dir))
        .filter(|dir| is_venv(dir));
    if captured.is_some() {
        return captured;
    }
    match &prep.venv {
        // A relative path is the workspace's.
        VenvSetting::Path(path) => Some(prep.ws_dir.join(path)).filter(|dir| is_venv(dir)),
        VenvSetting::Auto => find_venv(&prep.pane_cwd, &prep.ws_dir),
        VenvSetting::Off => None,
    }
}

/// The line to type into the pane, or `None` if there's nothing to do. It
/// starts with a space, so shells ignoring such lines keep it out of their
/// history.
pub fn prepare_line(prep: &Prep) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(venv) = venv_of(prep) {
        parts.push(format!("source {}", quote_path(&venv.join("bin/activate"))));
    }
    // mise's hook leaves these alone, so versions picked by hand stay.
    let mise: Vec<String> = prep
        .captured_env
        .iter()
        .filter(|(key, _)| key.starts_with("MISE_") && is_name(key))
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect();
    if !mise.is_empty() {
        parts.push(format!("export {}", mise.join(" ")));
    }
    if let Some(cd) = &prep.cd {
        parts.push(format!("cd {}", quote_path(cd)));
    }
    if let Some(command) = &prep.command {
        parts.push(command.clone());
    }
    (!parts.is_empty()).then(|| format!(" {}", parts.join(" && ")))
}

/// What to do next with a pane waiting for its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Its shell isn't there yet.
    Wait,
    Send,
    /// Something other than its shell runs in it already.
    Busy,
    /// It's gone.
    Drop,
}

/// Whether `process` is a shell, by its name (a login shell's starts with
/// `-`) or else its first argument.
fn is_shell(process: &Process) -> bool {
    let argv0 = process
        .argv
        .first()
        .and_then(|arg| Path::new(arg).file_name()?.to_str());
    [Some(process.name.as_str()), argv0]
        .into_iter()
        .flatten()
        .any(|name| SHELLS.contains(&name.trim_start_matches('-')))
}

/// What to do with a pane, given what herdr says of it. The line goes in as
/// soon as the shell exists (it keeps the line until its first prompt),
/// unless the foreground is led by something other than a shell: then
/// something runs in the pane already.
fn step(info: &Result<ProcessInfo, String>) -> Step {
    let Ok(info) = info else {
        return Step::Drop;
    };
    let Some(shell) = info.shell_pid else {
        return Step::Wait;
    };
    match info.foreground_process_group_id {
        // At its prompt or still starting, or a herdr that doesn't say.
        None => Step::Send,
        Some(group) if group == shell => Step::Send,
        Some(group) => {
            let leader = info.foreground_processes.iter().find(|p| p.pid == group);
            if leader.is_some_and(is_shell) {
                Step::Send
            } else {
                Step::Busy
            }
        }
    }
}

/// Prepare `preps` on a thread of their own, talking to the server on
/// `socket`: each pane gets its line as soon as its shell exists. A pane
/// already running something is left alone.
pub fn spawn_prepare(socket: PathBuf, preps: Vec<Prep>) {
    // Each pane with its line.
    let mut pending: Vec<(String, String)> = preps
        .iter()
        .filter_map(|prep| Some((prep.pane_id.clone(), prepare_line(prep)?)))
        .collect();
    if pending.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("prepare panes".into())
        .spawn(move || {
            let mut herdr = Herdr::on_socket(socket);
            let deadline = Instant::now() + DEADLINE;
            while !pending.is_empty() && Instant::now() < deadline {
                pending.retain(|(pane, line)| {
                    // A slow server mustn't keep it going much longer.
                    if Instant::now() >= deadline {
                        return true;
                    }
                    match step(&herdr.process_info(pane)) {
                        Step::Wait => true,
                        Step::Send => {
                            tracing::debug!("preparing pane {pane}:{line}");
                            if let Err(err) = herdr.send_line(pane, line) {
                                tracing::warn!("pane {pane} couldn't be prepared: {err}");
                            }
                            false
                        }
                        Step::Busy => {
                            tracing::debug!("pane {pane} is busy, not preparing it");
                            false
                        }
                        Step::Drop => {
                            tracing::debug!("pane {pane} is gone, not preparing it");
                            false
                        }
                    }
                });
                if !pending.is_empty() {
                    std::thread::sleep(POLL);
                }
            }
            for (pane, _) in &pending {
                tracing::debug!("pane {pane}'s shell didn't start, not preparing it");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("panes couldn't be prepared: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A folder of its own under the system's temporary one, removed when
    /// dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gerdr-prepare-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// Make `rel` a venv that can be activated.
        fn venv(&self, rel: &str) -> PathBuf {
            let venv = self.0.join(rel);
            std::fs::create_dir_all(venv.join("bin")).unwrap();
            std::fs::write(venv.join("bin/activate"), "").unwrap();
            venv
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let dir = self.0.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn prep(tmp: &TempDir) -> Prep {
        Prep {
            pane_id: "w1:p1".into(),
            pane_cwd: tmp.dir("ws/app"),
            ws_dir: tmp.dir("ws"),
            venv: VenvSetting::Auto,
            captured_env: Vec::new(),
            cd: None,
            command: None,
        }
    }

    #[test]
    fn dot_venv_wins_over_venv() {
        let tmp = TempDir::new();
        let dot = tmp.venv("ws/.venv");
        tmp.venv("ws/venv");
        let ws = tmp.0.join("ws");
        assert_eq!(find_venv(&ws, &ws), Some(dot));
    }

    #[test]
    fn nearest_venv_wins() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let near = tmp.venv("ws/app/venv");
        let cwd = tmp.dir("ws/app/src/pkg");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws")), Some(near));
    }

    #[test]
    fn venv_search_stops_at_the_workspace() {
        let tmp = TempDir::new();
        tmp.venv(".venv");
        let cwd = tmp.dir("ws/app");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws")), None);
        let top = tmp.venv("ws/venv");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws")), Some(top));
    }

    #[test]
    fn venv_without_activate_is_skipped() {
        let tmp = TempDir::new();
        tmp.dir("ws/app/.venv/bin");
        let venv = tmp.venv("ws/app/venv");
        let cwd = tmp.0.join("ws/app");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws")), Some(venv));
    }

    #[test]
    fn pane_outside_the_workspace_looks_only_in_its_folder() {
        let tmp = TempDir::new();
        tmp.venv(".venv");
        let ws = tmp.dir("ws");
        let other = tmp.dir("other");
        assert_eq!(find_venv(&other, &ws), None);
        let own = tmp.venv("other/venv");
        assert_eq!(find_venv(&other, &ws), Some(own));
    }

    #[test]
    fn nothing_to_do_gives_no_line() {
        let tmp = TempDir::new();
        assert_eq!(prepare_line(&prep(&tmp)), None);
    }

    #[test]
    fn auto_activates_the_venv_found() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let line = prepare_line(&prep(&tmp)).unwrap();
        assert_eq!(line, format!(" source {}/bin/activate", venv.display()));
    }

    #[test]
    fn off_activates_nothing() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Off;
        prep.captured_env = vec![("VIRTUAL_ENV".into(), venv.display().to_string())];
        assert_eq!(prepare_line(&prep), None);
    }

    #[test]
    fn captured_venv_wins() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let captured = tmp.venv("elsewhere/env");
        let mut prep = prep(&tmp);
        prep.captured_env = vec![("VIRTUAL_ENV".into(), captured.display().to_string())];
        let line = prepare_line(&prep).unwrap();
        assert_eq!(line, format!(" source {}/bin/activate", captured.display()));
        // One that's gone gives way to the setting.
        prep.captured_env = vec![(
            "VIRTUAL_ENV".into(),
            tmp.0.join("gone").display().to_string(),
        )];
        let line = prepare_line(&prep).unwrap();
        assert!(line.ends_with("ws/.venv/bin/activate"));
    }

    #[test]
    fn path_setting_is_used() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let chosen = tmp.venv("ws/envs/py");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Path(chosen.clone());
        let line = prepare_line(&prep).unwrap();
        assert_eq!(line, format!(" source {}/bin/activate", chosen.display()));
        prep.venv = VenvSetting::Path("envs/py".into());
        assert_eq!(prepare_line(&prep).unwrap(), line);
        prep.venv = VenvSetting::Path(tmp.0.join("missing"));
        assert_eq!(prepare_line(&prep), None);
    }

    #[test]
    fn mise_overrides_are_exported() {
        let tmp = TempDir::new();
        let mut prep = prep(&tmp);
        prep.captured_env = vec![
            ("PATH".into(), "/usr/bin".into()),
            ("MISE_PYTHON_VERSION".into(), "3.12".into()),
            ("MISE_ENV".into(), "dev test".into()),
        ];
        assert_eq!(
            prepare_line(&prep).unwrap(),
            " export MISE_PYTHON_VERSION=3.12 MISE_ENV='dev test'"
        );
    }

    #[test]
    fn parts_come_in_order() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let mut prep = prep(&tmp);
        prep.captured_env = vec![("MISE_ENV".into(), "dev".into())];
        prep.cd = Some(tmp.0.join("ws/app"));
        prep.command = Some("cargo run -- 'a b'".into());
        let expected = format!(
            " source {}/bin/activate && export MISE_ENV=dev && cd {}/ws/app && cargo run -- 'a b'",
            venv.display(),
            tmp.0.display()
        );
        assert_eq!(prepare_line(&prep).unwrap(), expected);
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/my env");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Path(venv.clone());
        prep.cd = Some(tmp.0.join("ws/my app"));
        let expected = format!(
            " source '{}/bin/activate' && cd '{}/ws/my app'",
            venv.display(),
            tmp.0.display()
        );
        assert_eq!(prepare_line(&prep).unwrap(), expected);
    }

    fn process(pid: i32, name: &str, argv: &[&str]) -> Process {
        Process {
            pid,
            name: name.into(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            cmdline: argv.join(" "),
            cwd: None,
        }
    }

    fn info(
        shell: Option<i32>,
        group: Option<i32>,
        leader: Option<Process>,
    ) -> Result<ProcessInfo, String> {
        Ok(ProcessInfo {
            shell_pid: shell,
            foreground_process_group_id: group,
            foreground_processes: leader.into_iter().collect(),
        })
    }

    #[test]
    fn line_is_sent_as_soon_as_the_shell_exists() {
        assert_eq!(step(&info(None, None, None)), Step::Wait);
        let zsh = process(10, "zsh", &["zsh"]);
        assert_eq!(step(&info(Some(10), Some(10), Some(zsh))), Step::Send);
        // A herdr that doesn't report the foreground.
        assert_eq!(step(&info(Some(10), None, None)), Step::Send);
    }

    #[test]
    fn a_shell_leading_the_foreground_is_not_busy() {
        let login = process(20, "-zsh", &["-zsh"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(login))), Step::Send);
        // Known by its first argument when its name isn't a shell's.
        let bash = process(20, "x", &["/usr/bin/bash", "-l"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(bash))), Step::Send);
    }

    #[test]
    fn something_else_running_is_busy() {
        let server = process(20, "cargo", &["cargo", "run"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(server))), Step::Busy);
        // A shell that doesn't lead the foreground doesn't count.
        let zsh = process(21, "zsh", &["zsh"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(zsh))), Step::Busy);
        // Nothing known of the leader.
        assert_eq!(step(&info(Some(10), Some(20), None)), Step::Busy);
    }

    #[test]
    fn gone_pane_is_dropped() {
        let gone = Err("pane.process_info: no such pane".to_string());
        assert_eq!(step(&gone), Step::Drop);
    }
}
