//! Getting a herdr pane's shell ready: switch to the Node version the
//! repository pins, activate its Python venv, bring back `mise` version
//! overrides, and (when the pane comes back after its server restarted) run
//! again what ran in it. Each pane gets one line, typed in as soon as its
//! shell exists: the pane is brand new, so nobody typed in it yet, and the
//! shell keeps the line as type-ahead until its first prompt. It runs after
//! the shell's rc files (and `mise activate`), so the venv's `bin` comes
//! first in PATH. The line is written for the shell that gets it: POSIX
//! shells, fish, csh, nu and PowerShell each get their own syntax and their
//! own venv activation script; other shells get nothing.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    desktop::shell_quote,
    herdr::{Herdr, Process, ProcessInfo},
    home_dir,
    projects::VenvSetting,
};

/// How often the panes are asked about.
const POLL: Duration = Duration::from_millis(150);
/// How long the panes are waited for, all together. A pane whose shell
/// didn't start by then is left alone.
const DEADLINE: Duration = Duration::from_secs(10);
/// How long a pane that looks busy is asked about again before it's left
/// alone: a shell just started runs its rc files' commands in the
/// foreground for a moment, and a command typed then would be lost.
const START_GRACE: Duration = Duration::from_secs(3);
/// How long `poetry` or `pipenv` may take to say where a venv is.
const TOOL_TIME: Duration = Duration::from_secs(3);
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

impl Prep {
    /// What to do in the brand new pane `pane_id`, opened in `opened_in`.
    /// `run` is the folder its line moves to and what it runs there: its venv
    /// and Node are that folder's, as a repository can be below the workspace's.
    pub fn new_pane(
        pane_id: String,
        opened_in: PathBuf,
        ws_dir: PathBuf,
        venv: VenvSetting,
        run: Option<(PathBuf, Option<String>)>,
    ) -> Self {
        let (cd, command) = run.unzip();
        Self {
            pane_id,
            pane_cwd: cd.clone().unwrap_or(opened_in),
            ws_dir,
            venv,
            captured_env: Vec::new(),
            cd,
            command: command.flatten(),
        }
    }
}

/// The version managers and venv tools installed, as far as gerdr can tell
/// from its own `PATH` and the usual places.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    /// pyenv's root: its `versions` holds pyenv-virtualenv's venvs.
    pub pyenv_root: Option<PathBuf>,
    pub poetry: Option<PathBuf>,
    pub pipenv: Option<PathBuf>,
    /// mise is there: its hook picks the Node version, gerdr doesn't.
    pub mise: bool,
    pub fnm: bool,
    /// nvm for POSIX shells (`nvm.sh`).
    pub nvm_sh: bool,
    /// nvm.fish.
    pub nvm_fish: bool,
    /// Where mise, fnm and nvm's `nvm.sh` are: a shell without rc files has
    /// none of their hooks (see `background_command`).
    pub mise_bin: Option<PathBuf>,
    pub fnm_bin: Option<PathBuf>,
    pub nvm_script: Option<PathBuf>,
}

impl Tools {
    pub fn detect() -> Self {
        let home = home_dir();
        let under_home = |rel: &str| home.as_ref().map(|home| home.join(rel));
        let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from);
        let pyenv_root = env_dir("PYENV_ROOT")
            .or_else(|| under_home(".pyenv"))
            .filter(|root| root.join("versions").is_dir());
        let nvm_dir = env_dir("NVM_DIR").or_else(|| under_home(".nvm"));
        let fish_functions = env_dir("XDG_CONFIG_HOME")
            .or_else(|| under_home(".config"))
            .map(|config| config.join("fish/functions"));
        let mise_bin = find_program("mise");
        let fnm_bin = find_program("fnm");
        let nvm_script = nvm_dir
            .map(|dir| dir.join("nvm.sh"))
            .filter(|script| script.is_file());
        Self {
            pyenv_root,
            poetry: find_program("poetry"),
            pipenv: find_program("pipenv"),
            mise: mise_bin.is_some(),
            fnm: fnm_bin.is_some(),
            nvm_sh: nvm_script.is_some(),
            nvm_fish: fish_functions.is_some_and(|dir| dir.join("nvm.fish").is_file()),
            mise_bin,
            fnm_bin,
            nvm_script,
        }
    }
}

/// Where `program` is installed: in `$PATH`, else where installers put it
/// for a user (gerdr's `PATH` may lack those).
fn find_program(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let usual = home_dir()
        .into_iter()
        .flat_map(|home| [".local/bin", ".cargo/bin", ".local/share/fnm"].map(|d| home.join(d)));
    std::env::split_paths(&path)
        .chain(usual)
        .map(|dir| dir.join(program))
        .find(|file| {
            use std::os::unix::fs::PermissionsExt;
            file.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The folders searched for a venv or a Node version file, nearest first:
/// from `pane_cwd` up to `ws_dir`. A pane outside its workspace's folder
/// only looks in its own.
fn search_dirs<'a>(pane_cwd: &'a Path, ws_dir: &Path) -> Vec<&'a Path> {
    if pane_cwd.starts_with(ws_dir) {
        pane_cwd
            .ancestors()
            .take_while(|dir| dir.starts_with(ws_dir))
            .collect()
    } else {
        vec![pane_cwd]
    }
}

/// The venv a shell in `pane_cwd` uses: the first folder from `pane_cwd` up
/// to `ws_dir` with one (see `venv_in`).
pub fn find_venv(pane_cwd: &Path, ws_dir: &Path, tools: &Tools) -> Option<PathBuf> {
    search_dirs(pane_cwd, ws_dir)
        .into_iter()
        .find_map(|dir| venv_in(dir, tools))
}

/// The venv of the project in `dir`: its `.venv` or `venv` folder, the
/// pyenv-virtualenv its `.python-version` names, or the one pipenv or
/// poetry keeps for it elsewhere.
fn venv_in(dir: &Path, tools: &Tools) -> Option<PathBuf> {
    [".venv", "venv"]
        .map(|name| dir.join(name))
        .into_iter()
        .find(|venv| is_venv(venv))
        .or_else(|| pyenv_venv(dir, tools.pyenv_root.as_deref()?))
        .or_else(|| {
            let pipenv = tools.pipenv.as_ref()?;
            dir.join("Pipfile").is_file().then_some(())?;
            ask_venv(pipenv, &["--venv"], dir)
        })
        .or_else(|| {
            let poetry = tools.poetry.as_ref()?;
            is_poetry_project(dir).then_some(())?;
            ask_venv(poetry, &["env", "info", "--path"], dir)
        })
}

/// The pyenv-virtualenv venv that `dir`'s `.python-version` names, under
/// pyenv's `root`. A plain Python version (`3.12`) isn't a venv.
fn pyenv_venv(dir: &Path, root: &Path) -> Option<PathBuf> {
    let names = std::fs::read_to_string(dir.join(".python-version")).ok()?;
    names
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('#') && !name.contains('/'))
        .map(|name| root.join("versions").join(name))
        .find(|venv| is_venv(venv))
}

/// Whether `dir` holds a project poetry manages.
pub(crate) fn is_poetry_project(dir: &Path) -> bool {
    dir.join("poetry.lock").is_file()
        || std::fs::read_to_string(dir.join("pyproject.toml"))
            .is_ok_and(|pyproject| pyproject.contains("[tool.poetry"))
}

/// Run `program` with `args` in `dir`, for the path of a venv it prints;
/// one that takes longer than `TOOL_TIME` is killed.
fn ask_venv(program: &Path, args: &[&str], dir: &Path) -> Option<PathBuf> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env_remove("VIRTUAL_ENV")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + TOOL_TIME;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL / 5),
            _ => {
                tracing::debug!("{} took too long, killed", program.display());
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let venv = PathBuf::from(out.trim());
    (status.success() && venv.is_absolute() && is_venv(&venv)).then_some(venv)
}

/// Whether `dir` is a venv that can be activated.
pub fn is_venv(dir: &Path) -> bool {
    dir.join("bin/activate").is_file()
}

/// Whether `name` can be assigned to in `sh`.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The venv to activate in the pane, if any.
fn venv_of(prep: &Prep, tools: &Tools) -> Option<PathBuf> {
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
        VenvSetting::Auto => find_venv(&prep.pane_cwd, &prep.ws_dir, tools),
        VenvSetting::Off => None,
    }
}

/// The Node version pinned by the nearest `.nvmrc` or `.node-version`, from
/// `pane_cwd` up to `ws_dir`.
fn node_version(pane_cwd: &Path, ws_dir: &Path) -> Option<String> {
    let pinned = |file: PathBuf| {
        let text = std::fs::read_to_string(file).ok()?;
        let version = text.lines().next()?.split('#').next()?.trim();
        let valid = !version.is_empty()
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-/*".contains(c));
        valid.then(|| version.to_owned())
    };
    search_dirs(pane_cwd, ws_dir)
        .into_iter()
        .find_map(|dir| pinned(dir.join(".nvmrc")).or_else(|| pinned(dir.join(".node-version"))))
}

/// A Node version to switch to, and the managers that can.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeUse {
    version: String,
    fnm: bool,
    nvm_sh: bool,
    nvm_fish: bool,
}

impl NodeUse {
    /// For a pinned Node version, unless mise is there (its hook reads the
    /// version files itself) or no manager is.
    fn new(prep: &Prep, tools: &Tools) -> Option<Self> {
        if tools.mise || !(tools.fnm || tools.nvm_sh || tools.nvm_fish) {
            return None;
        }
        Some(Self {
            version: node_version(&prep.pane_cwd, &prep.ws_dir)?,
            fnm: tools.fnm,
            nvm_sh: tools.nvm_sh,
            nvm_fish: tools.nvm_fish,
        })
    }

    /// The command switching `shell` to the version: fnm's, else nvm's.
    fn command(&self, shell: Shell) -> Option<String> {
        let manager = match shell {
            Shell::Posix | Shell::Fish | Shell::Pwsh if self.fnm => "fnm",
            Shell::Posix if self.nvm_sh => "nvm",
            Shell::Fish if self.nvm_fish => "nvm",
            _ => return None,
        };
        Some(format!("{manager} use {}", shell.quote(&self.version)))
    }
}

/// The kind of shell a pane runs, by the syntax its line needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// sh, bash, zsh, dash, ksh and the like.
    Posix,
    Fish,
    /// csh and tcsh.
    Csh,
    Nu,
    /// PowerShell.
    Pwsh,
    /// elvish, xonsh, ion: nothing is typed in.
    Other,
}

impl Shell {
    /// The kind of the shell called `name`, if it's one of `SHELLS`.
    fn named(name: &str) -> Option<Self> {
        let name = name.trim_start_matches('-');
        SHELLS.contains(&name).then_some(match name {
            "fish" => Self::Fish,
            "csh" | "tcsh" => Self::Csh,
            "nu" => Self::Nu,
            "pwsh" => Self::Pwsh,
            "elvish" | "xonsh" | "ion" => Self::Other,
            _ => Self::Posix,
        })
    }

    /// `word` as one word for this shell.
    fn quote(self, word: &str) -> String {
        let plain = !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%".contains(c));
        match self {
            Self::Posix | Self::Other => shell_quote(word),
            Self::Fish | Self::Csh if plain => word.to_owned(),
            Self::Fish => format!("'{}'", word.replace('\\', r"\\").replace('\'', r"\'")),
            Self::Csh => format!("'{}'", word.replace('\'', r"'\''").replace('!', r"\!")),
            Self::Nu if !word.contains('\'') => format!("'{word}'"),
            // A raw string, with more `#`s than any `'#…` inside.
            Self::Nu => {
                let mut hashes = "#".to_owned();
                while word.contains(&format!("'{hashes}")) {
                    hashes.push('#');
                }
                format!("r{hashes}'{word}'{hashes}")
            }
            Self::Pwsh => format!("'{}'", word.replace('\'', "''")),
        }
    }

    /// The separator running the next command only if this one worked,
    /// where the shell has one.
    fn and(self) -> &'static str {
        match self {
            Self::Nu | Self::Pwsh => "; ",
            _ => " && ",
        }
    }

    /// How this shell sources a venv's activation script, and the script.
    fn activate(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Posix => Some((".", "activate")),
            Self::Fish => Some(("source", "activate.fish")),
            Self::Csh => Some(("source", "activate.csh")),
            Self::Nu => Some(("overlay use", "activate.nu")),
            Self::Pwsh => Some((".", "Activate.ps1")),
            Self::Other => None,
        }
    }

    /// The commands exporting `vars` (checked names) in this shell.
    fn export(self, vars: &[(String, String)]) -> Vec<String> {
        let each = |pattern: fn(&str, String) -> String| {
            vars.iter()
                .map(|(key, value)| pattern(key, self.quote(value)))
                .collect()
        };
        match self {
            Self::Posix | Self::Other => {
                let vars: Vec<_> = vars
                    .iter()
                    .map(|(key, value)| format!("{key}={}", self.quote(value)))
                    .collect();
                vec![format!("export {}", vars.join(" "))]
            }
            Self::Fish => each(|key, value| format!("set -gx {key} {value}")),
            Self::Csh => each(|key, value| format!("setenv {key} {value}")),
            Self::Pwsh => each(|key, value| format!("$env:{key} = {value}")),
            Self::Nu => {
                let vars: Vec<_> = vars
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", self.quote(value)))
                    .collect();
                vec![format!("load-env {{{}}}", vars.join(", "))]
            }
        }
    }
}

/// The shell that gets the line typed in a pane: the one leading the
/// foreground, else the pane's own; known by its name in herdr's report,
/// else in `/proc`. One that can't be told is taken for a POSIX shell.
pub fn shell_of(info: &ProcessInfo) -> Shell {
    let Some(pid) = info.foreground_process_group_id.or(info.shell_pid) else {
        return Shell::Posix;
    };
    let reported = info
        .foreground_processes
        .iter()
        .find(|process| process.pid == pid)
        .and_then(shell_name);
    let from_proc = || {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Shell::named(comm.trim())
    };
    reported.or_else(from_proc).unwrap_or(Shell::Posix)
}

/// What a pane gets, worked out before its shell is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    node: Option<NodeUse>,
    venv: Option<PathBuf>,
    /// `mise` overrides to export, with names `sh` takes.
    mise: Vec<(String, String)>,
    cd: Option<PathBuf>,
    command: Option<String>,
}

impl Plan {
    pub fn new(prep: &Prep, tools: &Tools) -> Self {
        Self {
            node: NodeUse::new(prep, tools),
            venv: venv_of(prep, tools),
            // mise's hook leaves these alone, so versions picked by hand stay.
            mise: prep
                .captured_env
                .iter()
                .filter(|(key, _)| key.starts_with("MISE_") && is_name(key))
                .cloned()
                .collect(),
            cd: prep.cd.clone(),
            command: prep.command.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The line to type into a pane running `shell`, or `None` if there's
    /// nothing to do. It starts with a space, so shells ignoring such lines
    /// keep it out of their history. Switching Node comes first, apart: a
    /// version that isn't installed mustn't stop the rest.
    pub fn line(&self, shell: Shell) -> Option<String> {
        if shell == Shell::Other {
            return None;
        }
        let path = |path: &Path| shell.quote(&path.to_string_lossy());
        let mut parts = Vec::new();
        if let (Some(venv), Some((source, script))) = (&self.venv, shell.activate()) {
            let script = venv.join("bin").join(script);
            if script.is_file() {
                parts.push(format!("{source} {}", path(&script)));
            } else {
                tracing::debug!("{} has no {}", venv.display(), script.display());
            }
        }
        if !self.mise.is_empty() {
            parts.extend(shell.export(&self.mise));
        }
        if let Some(cd) = &self.cd {
            parts.push(format!("cd {}", path(cd)));
        }
        if let Some(command) = &self.command {
            // Kept in `sh`'s syntax: another shell hands it to `sh`.
            parts.push(match shell {
                Shell::Posix => command.clone(),
                _ => format!("sh -c {}", shell.quote(command)),
            });
        }
        let node = self.node.as_ref().and_then(|node| node.command(shell));
        let rest = (!parts.is_empty()).then(|| parts.join(shell.and()));
        let line: Vec<String> = node.into_iter().chain(rest).collect();
        (!line.is_empty()).then(|| format!(" {}", line.join("; ")))
    }
}

/// `command` for a shell without rc files, which `sh -c` is: what a new
/// terminal in `dir` gets from its rc files and its line comes first, so it
/// runs on the same Node and venv. Like there, a Node version that can't be
/// switched to doesn't stop it, and the venv comes first in `PATH`. `command`
/// stays as it is when there is nothing to get.
pub fn background_command(
    command: &str,
    dir: &Path,
    ws_dir: &Path,
    venv: &VenvSetting,
    tools: &Tools,
) -> String {
    let prep = Prep {
        pane_id: String::new(),
        pane_cwd: dir.to_path_buf(),
        ws_dir: ws_dir.to_path_buf(),
        venv: venv.clone(),
        captured_env: Vec::new(),
        cd: None,
        command: None,
    };
    let plan = Plan::new(&prep, tools);
    let quoted = |path: &Path| shell_quote(&path.to_string_lossy());
    let hook = |bin: &Path, args: &str| format!("eval \"$({} {args})\"", quoted(bin));
    let mut first = Vec::new();
    if let Some(mise) = &tools.mise_bin {
        // Its hook reads the version files itself, and the rest it manages.
        first.push(hook(mise, "env -s bash"));
    } else if let Some(node) = &plan.node {
        let version = shell_quote(&node.version);
        if let Some(fnm) = &tools.fnm_bin {
            first.push(hook(fnm, "env --shell bash"));
            first.push(format!("{} use {version}", quoted(fnm)));
        } else if let Some(nvm) = &tools.nvm_script {
            first.push(format!(". {}", quoted(nvm)));
            first.push(format!("nvm use {version}"));
        }
    }
    let venv = (plan.venv.as_ref())
        .map(|venv| venv.join("bin/activate"))
        .filter(|script| script.is_file());
    if first.is_empty() && venv.is_none() {
        return command.to_owned();
    }
    let run = format!("sh -c {}", shell_quote(command));
    first.push(match venv {
        Some(script) => format!(". {} && {run}", quoted(&script)),
        None => run,
    });
    first.join("; ")
}

/// What to do next with a pane waiting for its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Its shell isn't there yet.
    Wait,
    Send,
    /// Something other than its shell runs in it already.
    Busy,
    /// It's gone.
    Drop,
}

/// Whether `process` is a shell waiting for what's typed in, by its name (a
/// login shell's starts with `-`) or else its first argument: not one
/// running a script or a `-c` command (see `runs_script`).
pub(crate) fn is_interactive_shell(process: &Process) -> bool {
    let argv0 = process
        .argv
        .first()
        .and_then(|arg| Path::new(arg).file_name()?.to_str());
    [Some(process.name.as_str()), argv0]
        .into_iter()
        .flatten()
        .any(|name| SHELLS.contains(&name.trim_start_matches('-')))
        && !runs_script(&process.argv)
}

/// Options of shells that take the next argument as their value.
const SHELL_VALUE_OPTIONS: [&str; 8] = [
    "-o",
    "+o",
    "-O",
    "+O",
    "--rcfile",
    "--init-file",
    "--config",
    "--env-config",
];

/// Whether a shell run as `argv` runs a script (`sh runlocal.sh`, or
/// `./runlocal.sh` with its `#!`) or a command (`bash -lc …`) rather than
/// waiting at its prompt: it has an argument that isn't an option.
pub(crate) fn runs_script(argv: &[String]) -> bool {
    let mut args = argv.iter().skip(1).map(String::as_str);
    while let Some(arg) = args.next() {
        if SHELL_VALUE_OPTIONS.contains(&arg) {
            args.next();
            continue;
        }
        let short = arg.strip_prefix('-').filter(|o| !o.starts_with('-'));
        match arg {
            "--" => return args.next().is_some(),
            _ if short.is_some_and(|o| o.contains('c')) => return true,
            _ if arg.starts_with('-') || arg.starts_with('+') => {}
            _ => return true,
        }
    }
    false
}

/// The kind of shell `process` is, if it's one, known as
/// `is_interactive_shell` knows.
fn shell_name(process: &Process) -> Option<Shell> {
    let argv0 = process
        .argv
        .first()
        .and_then(|arg| Path::new(arg).file_name()?.to_str());
    [Some(process.name.as_str()), argv0]
        .into_iter()
        .flatten()
        .find_map(Shell::named)
}

/// What to do with a pane, given what herdr says of it. The line goes in as
/// soon as the shell exists (it keeps the line until its first prompt),
/// unless the foreground is led by something other than a shell: then
/// something runs in the pane already.
pub(crate) fn step(info: &Result<ProcessInfo, String>) -> Step {
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
            if leader.is_some_and(is_interactive_shell) {
                Step::Send
            } else {
                Step::Busy
            }
        }
    }
}

/// Prepare `preps` on a thread of their own, talking to the server on
/// `socket`: each pane gets its line as soon as its shell exists, written
/// for that shell. A pane already running something is left alone.
pub fn spawn_prepare(socket: PathBuf, preps: Vec<Prep>) {
    if preps.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("prepare panes".into())
        .spawn(move || {
            // Asking poetry or pipenv takes a moment: not on the main thread.
            let tools = Tools::detect();
            let mut pending: Vec<(String, Plan)> = preps
                .iter()
                .map(|prep| (prep.pane_id.clone(), Plan::new(prep, &tools)))
                .filter(|(_, plan)| !plan.is_empty())
                .collect();
            let mut herdr = Herdr::on_socket(socket);
            let started = Instant::now();
            let deadline = started + DEADLINE;
            while !pending.is_empty() && Instant::now() < deadline {
                pending.retain(|(pane, plan)| {
                    // A slow server mustn't keep it going much longer.
                    if Instant::now() >= deadline {
                        return true;
                    }
                    let info = herdr.process_info(pane);
                    match step(&info) {
                        Step::Wait => true,
                        Step::Send => {
                            let shell = info.as_ref().map_or(Shell::Posix, shell_of);
                            let Some(line) = plan.line(shell) else {
                                tracing::debug!("pane {pane}: nothing to type for {shell:?}");
                                return false;
                            };
                            tracing::debug!("preparing pane {pane}:{line}");
                            if let Err(err) = herdr.send_line(pane, &line) {
                                tracing::warn!("pane {pane} couldn't be prepared: {err}");
                            }
                            false
                        }
                        Step::Busy if started.elapsed() < START_GRACE => true,
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

    /// No version manager, venv tool or pyenv.
    const NONE: Tools = Tools {
        pyenv_root: None,
        poetry: None,
        pipenv: None,
        mise: false,
        fnm: false,
        nvm_sh: false,
        nvm_fish: false,
        mise_bin: None,
        fnm_bin: None,
        nvm_script: None,
    };

    /// The line a POSIX shell gets, without any tools.
    fn posix_line(prep: &Prep) -> Option<String> {
        Plan::new(prep, &NONE).line(Shell::Posix)
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
    fn a_new_pane_gets_the_venv_of_the_folder_it_moves_to() {
        let tmp = TempDir::new();
        let ws = tmp.dir("ws");
        let app = tmp.dir("ws/app");
        let venv = tmp.venv("ws/app/.venv");
        let new_pane = |run| {
            Prep::new_pane(
                "w1:p1".into(),
                ws.clone(),
                ws.clone(),
                VenvSetting::Auto,
                run,
            )
        };
        // Opened in the workspace's folder, it runs in the repository below.
        let run = (app.clone(), Some("python manage.py".to_owned()));
        assert_eq!(
            posix_line(&new_pane(Some(run))),
            Some(format!(
                " . {}/bin/activate && cd {} && python manage.py",
                venv.display(),
                app.display()
            ))
        );
        // Told nowhere to go, it has only the workspace's own, which is none.
        assert_eq!(posix_line(&new_pane(None)), None);
        // A worktree outside the workspace doesn't get the workspace's venv.
        let main = tmp.venv("ws/.venv");
        let outside = tmp.dir("worktree");
        let line = posix_line(&new_pane(Some((outside.clone(), None))));
        assert_eq!(line, Some(format!(" cd {}", outside.display())));
        assert_eq!(
            posix_line(&new_pane(None)),
            Some(format!(" . {}/bin/activate", main.display()))
        );
    }

    #[test]
    fn a_background_command_gets_the_venv_and_node_of_a_new_terminal() {
        let tmp = TempDir::new();
        let dir = tmp.dir("ws/app");
        let ws = tmp.dir("ws");
        let run = |tools: &Tools, venv: &VenvSetting| {
            background_command("npm ci", &dir, &ws, venv, tools)
        };
        // Nothing to get: it stays as it is, not one more shell around it.
        assert_eq!(run(&NONE, &VenvSetting::Auto), "npm ci");
        let venv = tmp.venv("ws/app/.venv");
        let inner = "sh -c 'npm ci'";
        let activate = format!(". {} && {inner}", venv.join("bin/activate").display());
        assert_eq!(run(&NONE, &VenvSetting::Auto), activate);
        // The workspace's setting holds, as for its terminals.
        assert_eq!(run(&NONE, &VenvSetting::Off), "npm ci");
        // mise's hook comes first and may fail without stopping the rest.
        let mise = Tools {
            mise: true,
            mise_bin: Some("/m/mise".into()),
            ..NONE
        };
        assert_eq!(
            run(&mise, &VenvSetting::Auto),
            format!("eval \"$(/m/mise env -s bash)\"; {activate}")
        );
        // A pinned version: fnm's or else nvm's, the venv after it.
        std::fs::write(dir.join(".nvmrc"), "20.1\n").unwrap();
        let fnm = Tools {
            fnm: true,
            fnm_bin: Some("/f/fnm".into()),
            ..NONE
        };
        assert_eq!(
            run(&fnm, &VenvSetting::Off),
            format!("eval \"$(/f/fnm env --shell bash)\"; /f/fnm use 20.1; {inner}")
        );
        let nvm = Tools {
            nvm_sh: true,
            nvm_script: Some("/n/nvm.sh".into()),
            ..NONE
        };
        assert_eq!(
            run(&nvm, &VenvSetting::Auto),
            format!(". /n/nvm.sh; nvm use 20.1; {activate}")
        );
    }

    #[test]
    fn dot_venv_wins_over_venv() {
        let tmp = TempDir::new();
        let dot = tmp.venv("ws/.venv");
        tmp.venv("ws/venv");
        let ws = tmp.0.join("ws");
        assert_eq!(find_venv(&ws, &ws, &NONE), Some(dot));
    }

    #[test]
    fn nearest_venv_wins() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let near = tmp.venv("ws/app/venv");
        let cwd = tmp.dir("ws/app/src/pkg");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws"), &NONE), Some(near));
    }

    #[test]
    fn venv_search_stops_at_the_workspace() {
        let tmp = TempDir::new();
        tmp.venv(".venv");
        let cwd = tmp.dir("ws/app");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws"), &NONE), None);
        let top = tmp.venv("ws/venv");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws"), &NONE), Some(top));
    }

    #[test]
    fn venv_without_activate_is_skipped() {
        let tmp = TempDir::new();
        tmp.dir("ws/app/.venv/bin");
        let venv = tmp.venv("ws/app/venv");
        let cwd = tmp.0.join("ws/app");
        assert_eq!(find_venv(&cwd, &tmp.0.join("ws"), &NONE), Some(venv));
    }

    #[test]
    fn pane_outside_the_workspace_looks_only_in_its_folder() {
        let tmp = TempDir::new();
        tmp.venv(".venv");
        let ws = tmp.dir("ws");
        let other = tmp.dir("other");
        assert_eq!(find_venv(&other, &ws, &NONE), None);
        let own = tmp.venv("other/venv");
        assert_eq!(find_venv(&other, &ws, &NONE), Some(own));
    }

    #[test]
    fn nothing_to_do_gives_no_line() {
        let tmp = TempDir::new();
        assert_eq!(posix_line(&prep(&tmp)), None);
    }

    #[test]
    fn auto_activates_the_venv_found() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let line = posix_line(&prep(&tmp)).unwrap();
        assert_eq!(line, format!(" . {}/bin/activate", venv.display()));
    }

    #[test]
    fn off_activates_nothing() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Off;
        prep.captured_env = vec![("VIRTUAL_ENV".into(), venv.display().to_string())];
        assert_eq!(posix_line(&prep), None);
    }

    #[test]
    fn captured_venv_wins() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let captured = tmp.venv("elsewhere/env");
        let mut prep = prep(&tmp);
        prep.captured_env = vec![("VIRTUAL_ENV".into(), captured.display().to_string())];
        let line = posix_line(&prep).unwrap();
        assert_eq!(line, format!(" . {}/bin/activate", captured.display()));
        // One that's gone gives way to the setting.
        prep.captured_env = vec![(
            "VIRTUAL_ENV".into(),
            tmp.0.join("gone").display().to_string(),
        )];
        let line = posix_line(&prep).unwrap();
        assert!(line.ends_with("ws/.venv/bin/activate"));
    }

    #[test]
    fn path_setting_is_used() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let chosen = tmp.venv("ws/envs/py");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Path(chosen.clone());
        let line = posix_line(&prep).unwrap();
        assert_eq!(line, format!(" . {}/bin/activate", chosen.display()));
        prep.venv = VenvSetting::Path("envs/py".into());
        assert_eq!(posix_line(&prep).unwrap(), line);
        prep.venv = VenvSetting::Path(tmp.0.join("missing"));
        assert_eq!(posix_line(&prep), None);
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
            posix_line(&prep).unwrap(),
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
            " . {}/bin/activate && export MISE_ENV=dev && cd {}/ws/app && cargo run -- 'a b'",
            venv.display(),
            tmp.0.display()
        );
        assert_eq!(posix_line(&prep).unwrap(), expected);
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/my env");
        let mut prep = prep(&tmp);
        prep.venv = VenvSetting::Path(venv.clone());
        prep.cd = Some(tmp.0.join("ws/my app"));
        let expected = format!(
            " . '{}/bin/activate' && cd '{}/ws/my app'",
            venv.display(),
            tmp.0.display()
        );
        assert_eq!(posix_line(&prep).unwrap(), expected);
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
        let options = process(20, "bash", &["bash", "-o", "vi", "--rcfile", "rc"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(options))), Step::Send);
    }

    #[test]
    fn a_shell_running_a_script_is_busy() {
        // `./runlocal.sh`, run by its `#!`.
        let script = process(20, "runlocal.sh", &["/bin/bash", "./runlocal.sh"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(script))), Step::Busy);
        let script = process(20, "sh", &["sh", "-e", "runlocal.sh"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(script))), Step::Busy);
        let command = process(20, "bash", &["bash", "-lc", "make serve"]);
        assert_eq!(step(&info(Some(10), Some(20), Some(command))), Step::Busy);
        let after_dashes = process(20, "zsh", &["zsh", "--", "serve.zsh"]);
        assert_eq!(
            step(&info(Some(10), Some(20), Some(after_dashes))),
            Step::Busy
        );
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

    #[test]
    fn pyenv_virtualenv_named_in_python_version() {
        let tmp = TempDir::new();
        let root = tmp.dir("pyenv");
        let venv = tmp.venv("pyenv/versions/api-env");
        tmp.dir("pyenv/versions/3.12/bin");
        let ws = tmp.dir("ws");
        let tools = Tools {
            pyenv_root: Some(root),
            ..NONE
        };
        // A plain version isn't a venv.
        std::fs::write(ws.join(".python-version"), "3.12\n").unwrap();
        assert_eq!(find_venv(&ws, &ws, &tools), None);
        std::fs::write(ws.join(".python-version"), "api-env\n").unwrap();
        assert_eq!(find_venv(&ws, &ws, &tools), Some(venv));
        // The project's own folder comes first.
        let own = tmp.venv("ws/.venv");
        assert_eq!(find_venv(&ws, &ws, &tools), Some(own));
    }

    #[test]
    fn venv_tools_are_asked_only_for_their_projects() {
        let tmp = TempDir::new();
        let ws = tmp.dir("ws");
        let venv = tmp.venv("cache/api-x1");
        // Stands in for poetry and pipenv: prints the venv's path.
        let tool = tmp.0.join("tool");
        std::fs::write(&tool, format!("#!/bin/sh\necho {}\n", venv.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Tools {
            poetry: Some(tool.clone()),
            pipenv: Some(tool),
            ..NONE
        };
        assert_eq!(find_venv(&ws, &ws, &tools), None);
        std::fs::write(ws.join("pyproject.toml"), "[tool.poetry]\nname = 'api'\n").unwrap();
        assert_eq!(find_venv(&ws, &ws, &tools), Some(venv.clone()));
        std::fs::remove_file(ws.join("pyproject.toml")).unwrap();
        std::fs::write(ws.join("Pipfile"), "").unwrap();
        assert_eq!(find_venv(&ws, &ws, &tools), Some(venv));
    }

    #[test]
    fn shells_are_told_apart() {
        assert_eq!(Shell::named("-zsh"), Some(Shell::Posix));
        assert_eq!(Shell::named("dash"), Some(Shell::Posix));
        assert_eq!(Shell::named("fish"), Some(Shell::Fish));
        assert_eq!(Shell::named("tcsh"), Some(Shell::Csh));
        assert_eq!(Shell::named("nu"), Some(Shell::Nu));
        assert_eq!(Shell::named("pwsh"), Some(Shell::Pwsh));
        assert_eq!(Shell::named("xonsh"), Some(Shell::Other));
        assert_eq!(Shell::named("cargo"), None);
        // The foreground's leader gets the line, else the pane's shell.
        let fish = process(20, "fish", &["fish"]);
        let bash = process(10, "bash", &["bash"]);
        let both = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(20),
            foreground_processes: vec![bash.clone(), fish],
        };
        assert_eq!(shell_of(&both), Shell::Fish);
        let own = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: None,
            foreground_processes: vec![bash],
        };
        assert_eq!(shell_of(&own), Shell::Posix);
    }

    #[test]
    fn quoting_follows_the_shell() {
        let word = r"it's a\b!";
        assert_eq!(Shell::Posix.quote(word), r"'it'\''s a\b!'");
        assert_eq!(Shell::Fish.quote(word), r"'it\'s a\\b!'");
        assert_eq!(Shell::Csh.quote(word), r"'it'\''s a\b\!'");
        assert_eq!(Shell::Nu.quote(word), r"r#'it's a\b!'#");
        assert_eq!(Shell::Nu.quote("a'#b"), "r##'a'#b'##");
        assert_eq!(Shell::Nu.quote("/x/y"), "'/x/y'");
        assert_eq!(Shell::Pwsh.quote(word), r"'it''s a\b!'");
        assert_eq!(Shell::Fish.quote("/x/y"), "/x/y");
    }

    #[test]
    fn each_shell_gets_its_own_syntax() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        for script in [
            "activate.fish",
            "activate.csh",
            "activate.nu",
            "Activate.ps1",
        ] {
            std::fs::write(venv.join("bin").join(script), "").unwrap();
        }
        let mut prep = prep(&tmp);
        prep.captured_env = vec![
            ("MISE_ENV".into(), "dev".into()),
            ("MISE_NODE_VERSION".into(), "20".into()),
        ];
        prep.command = Some("cargo run".into());
        let plan = Plan::new(&prep, &NONE);
        let v = venv.join("bin").display().to_string();
        assert_eq!(
            plan.line(Shell::Fish).unwrap(),
            format!(
                " source {v}/activate.fish && set -gx MISE_ENV dev && \
                 set -gx MISE_NODE_VERSION 20 && sh -c 'cargo run'"
            )
        );
        assert_eq!(
            plan.line(Shell::Csh).unwrap(),
            format!(
                " source {v}/activate.csh && setenv MISE_ENV dev && \
                 setenv MISE_NODE_VERSION 20 && sh -c 'cargo run'"
            )
        );
        assert_eq!(
            plan.line(Shell::Nu).unwrap(),
            format!(
                " overlay use '{v}/activate.nu'; load-env {{MISE_ENV: 'dev', \
                 MISE_NODE_VERSION: '20'}}; sh -c 'cargo run'"
            )
        );
        assert_eq!(
            plan.line(Shell::Pwsh).unwrap(),
            format!(
                " . '{v}/Activate.ps1'; $env:MISE_ENV = 'dev'; \
                 $env:MISE_NODE_VERSION = '20'; sh -c 'cargo run'"
            )
        );
        assert_eq!(plan.line(Shell::Other), None);
    }

    #[test]
    fn a_shell_without_its_activation_script_skips_the_venv() {
        let tmp = TempDir::new();
        tmp.venv("ws/.venv");
        let mut prep = prep(&tmp);
        prep.command = Some("make".into());
        let plan = Plan::new(&prep, &NONE);
        assert_eq!(plan.line(Shell::Fish).unwrap(), " sh -c make");
    }

    #[test]
    fn node_version_files_pick_the_manager() {
        let tmp = TempDir::new();
        tmp.dir("ws/app");
        std::fs::write(tmp.0.join("ws/.nvmrc"), "lts/iron # the team's\n").unwrap();
        let prep = prep(&tmp);
        let with = |tools: Tools| Plan::new(&prep, &tools);
        // Nothing to switch with.
        assert!(with(NONE).is_empty());
        let fnm = Tools { fnm: true, ..NONE };
        assert_eq!(
            with(fnm.clone()).line(Shell::Posix).unwrap(),
            " fnm use lts/iron"
        );
        assert_eq!(with(fnm).line(Shell::Pwsh).unwrap(), " fnm use 'lts/iron'");
        let nvm = Tools {
            nvm_sh: true,
            ..NONE
        };
        assert_eq!(
            with(nvm.clone()).line(Shell::Posix).unwrap(),
            " nvm use lts/iron"
        );
        // nvm.sh doesn't work in fish.
        assert_eq!(with(nvm).line(Shell::Fish), None);
        // mise's hook picks the version itself.
        let mise = Tools {
            mise: true,
            fnm: true,
            ..NONE
        };
        assert!(with(mise).is_empty());
    }

    #[test]
    fn switching_node_doesnt_hold_up_the_rest() {
        let tmp = TempDir::new();
        let venv = tmp.venv("ws/.venv");
        let app = tmp.dir("ws/app");
        std::fs::write(app.join(".node-version"), "20.11.0\n").unwrap();
        let mut prep = prep(&tmp);
        prep.command = Some("npm run dev".into());
        let tools = Tools { fnm: true, ..NONE };
        assert_eq!(
            Plan::new(&prep, &tools).line(Shell::Posix).unwrap(),
            format!(
                " fnm use 20.11.0; . {}/bin/activate && npm run dev",
                venv.display()
            )
        );
    }
}
