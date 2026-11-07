//! Setting up a feature's worktrees after they are made, so tests and builds
//! can run there: what a new worktree lacks is what git ignores.
//!
//! Each repository first gets a copy of what its main worktree has and its
//! feature worktree lacks (`to_copy`):
//! - its ignored files of at most `MAX_FILE` that aren't logs or compiled
//!   Python and aren't in an ignored folder: `.env`, `local_config.py`;
//! - its ignored environments: `node_modules`, any folder with a
//!   `pyvenv.cfg`. They copy with `cp -a --reflink=always`: on a
//!   copy-on-write file system that is instant and takes no space. Where
//!   that fails, for the file system or for what `-a` keeps, `copy_tree`
//!   copies it asking less, which takes its size on disk and some time;
//! - the paths the project's store file adds (`"workflow": {"copy":
//!   {"aft-backend": [".elasticbeanstalk"]}}`), however large.
//!
//! Nothing there already is overwritten, so a setup run again copies only
//! what is missing, and a copy doesn't follow later edits in main. A copy
//! that would show as a change in the worktree goes again (`git status`): a
//! file the feature's branch doesn't ignore must not fail Verify.
//!
//! A virtual environment names its folder inside it: copied as it is, its
//! `pytest` would run main's python, and an editable install would import
//! main's code, so tests would pass on the wrong code. `repoint` makes the
//! copy name the worktree instead.
//!
//! Then the repository's commands from the store file run, every time
//! (`"workflow": {"setup": {"services-api": ["npm ci"]}}`), in its worktree.
//! Every command runs with `sh -c`, on the Node and venv a new terminal
//! there gets (`prepare::background_command`), stdin closed, its output
//! appended to `setup.log` in the feature data dir, for at most `TIMEOUT`.
//! The log says what each command ran as and where, what was kept, and how
//! the run ended; the setup log viewer shows its last run (`shown_log`). The
//! first one that fails stops the setup: its repository, command and exit go
//! in the error. Each command runs in a process group of its own, and the
//! group is killed when it ends or times out, so nothing it started
//! lingers (`shell`, which Verify's checks run with too).
//!
//! On another machine the same setup differs in what it copies: only what's
//! small enough to send (`for_machine`). An environment isn't sent, as it is
//! built for this machine and its paths: the commands build it there. They run
//! in a terminal there, by a script (`script_there`), so they go on when the
//! link drops; `machine_checks` waits for it.
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
    Feature, PerProject, Repo,
    layout::{git, shown},
    now,
};
use crate::{
    prepare::{Tools, background_command},
    projects::VenvSetting,
};

/// The name of an ignored folder copied whatever it holds; any folder with
/// `VENV_MARK` is copied too.
const NODE_MODULES: &str = "node_modules";

/// The file that makes a folder a Python virtual environment.
const VENV_MARK: &str = "pyvenv.cfg";

/// The largest ignored file copied unasked: a local config is small, a
/// database dump isn't.
pub const MAX_FILE: u64 = 1024 * 1024;

/// How long one setup command, or one copy, may run.
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// The setup log's name, in the feature data dir.
pub const LOG: &str = "setup.log";

/// The line that starts a run in the log.
const RUN_START: &str = "== Setup started";
/// The line that ends a failed run in the log.
const RUN_FAILED: &str = "== Setup failed";

/// Where a feature's setup stands, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRun {
    /// The machine it runs on, by id; `None`: this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Its terminal there, as `machine_work::qualified` says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// Unix seconds.
    pub started: u64,
    /// When it ended, either way (unix seconds); `None` while it runs, or
    /// when gerdr quit before it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// Why it failed: "services-api: `npm ci` exited with 1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// What this run copied, a line per repository: "api: .env, .venv".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copied: Vec<String>,
    /// What it couldn't copy, a line each: "api: .venv (exited with 1)".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_copied: Vec<String>,
    /// The commands that passed, a line each: "api: npm ci".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ran: Vec<String>,
}

/// What a setup did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// As in `SetupRun::copied`.
    pub copied: Vec<String>,
    /// As in `SetupRun::not_copied`.
    pub not_copied: Vec<String>,
    /// As in `SetupRun::failed`.
    pub failed: Option<String>,
    /// As in `SetupRun::ran`.
    pub ran: Vec<String>,
}

/// Lines each project's store file gives, by repository name: the
/// `"workflow"` of the project a repository is in.
pub type PerRepo = PerProject<BTreeMap<String, Vec<String>>>;

/// The folder a setup's files are in, under the data folder there.
pub const RUN_THERE: &str = "setup-run";

/// What a repository's setup on another machine copies, and leaves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForMachine {
    /// By path, as `Item::path`.
    pub send: Vec<String>,
    /// Environments it has here, which aren't sent: built for this machine.
    pub environments: Vec<String>,
    /// What couldn't be told, a line each.
    pub problems: Vec<String>,
}

/// What `repo`'s setup on another machine copies from its main worktree.
pub fn for_machine(repo: &Repo, extras: &PerRepo) -> ForMachine {
    let mut out = ForMachine::default();
    for item in to_copy(repo, &lines_of(extras, repo), &mut out.problems) {
        match item.environment {
            true => out.environments.push(item.path),
            false => out.send.push(item.path),
        }
    }
    out
}

/// A command of a setup on another machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepThere {
    /// Its repository, by `Repo::key`.
    pub repo: String,
    /// Its worktree there.
    pub dir: String,
    /// As it reads here.
    pub command: String,
    /// As it reads there.
    pub line: String,
}

impl StepThere {
    /// Where it stands, as `Report` says it: "api: npm ci".
    pub fn said(&self) -> String {
        format!("{}: {}", self.repo, self.command)
    }

    /// How it failed, as `Report::failed` says it.
    pub fn failure(&self, how: &str) -> String {
        format!("{}: `{}` {how}", self.repo, self.command)
    }
}

/// `commands`' lines for `feature` on another machine, in the order of a setup
/// here; `there` translates each repository's worktree, `text` each command.
pub fn steps_there(
    feature: &Feature,
    commands: &PerRepo,
    there: &dyn Fn(&Path) -> String,
    text: &dyn Fn(&str) -> String,
) -> Vec<StepThere> {
    let mut repos: Vec<_> = feature.repos.iter().collect();
    repos.sort_by_key(|r| r.key());
    let mut steps = Vec::new();
    for repo in repos {
        for command in lines_of(commands, repo) {
            steps.push(StepThere {
                repo: repo.key(),
                dir: there(&repo.worktree),
                command: command.to_owned(),
                line: text(command),
            });
        }
    }
    steps
}

/// The script running `steps` in a terminal on another machine, its output
/// going to the log `log`, in a form `shown_log` reads. It keeps to its folder
/// `dir` what `machine_checks` waits for: `pid`, `step` (the command it is at,
/// from 1) and `exit`. Past a failure nothing more runs, as here.
pub fn script_there(
    dir: &str,
    log: &str,
    steps: &[StepThere],
    slug: &str,
    notes: &[String],
    prelude: &str,
) -> String {
    let q = crate::desktop::shell_quote;
    let mut script = format!(
        r#"trap : INT TERM
d={dir}
log={log}
GERDR_FEATURE={slug}
export GERDR_FEATURE
rm -rf "$d"
mkdir -p "$d" "$(dirname "$log")"
leave() {{ printf '%s\n' "$2" > "$d/$1.new" && mv "$d/$1.new" "$d/$1"; }}
leave pid "$$"
began=$(date +%s)
printf '%s\n' "{start} $(date -u '+%Y-%m-%d %H:%M UTC')" >> "$log"
"#,
        dir = q(dir),
        log = q(log),
        slug = q(slug),
        start = RUN_START,
    );
    for note in notes {
        script.push_str(&format!("printf '%s\\n' {} >> \"$log\"\n", q(note)));
    }
    script.push_str(&format!(
        r#"step() {{
  leave step "$1"
  printf '%s\n' "== $(date -u '+%Y-%m-%d %H:%M UTC') $2: $3" "== in $4" >> "$log"
  s=$(date +%s)
  rm -f "$d/rc"
  ( cd "$4" || exit 1
    {prelude}
    sh -c "$5" </dev/null
    echo $? > "$d/rc" ) 2>&1 | tee -a "$log"
  c=$(cat "$d/rc" 2>/dev/null)
  c=${{c:-1}}
  printf '== exited with %s after %s s\n\n' "$c" "$(($(date +%s) - s))" >> "$log"
  if [ "$c" -ne 0 ]; then
    printf '%s\n' "{failed}: $2: \`$3\` exited with $c" >> "$log"
    leave exit "$c"
    exit "$c"
  fi
}}
"#,
        prelude = prelude.trim_end(),
        failed = RUN_FAILED,
    ));
    for (i, step) in steps.iter().enumerate() {
        script.push_str(&format!(
            "step {} {} {} {} {}\n",
            i + 1,
            q(&step.repo),
            q(&step.command),
            q(&step.dir),
            q(&step.line),
        ));
    }
    script.push_str(
        r#"printf '%s\n\n' "== Setup finished after $(($(date +%s) - began)) s" >> "$log"
leave exit 0
"#,
    );
    script
}

/// What a setup there says it did: `at`, the command it got to (from 1), and
/// how it ended. `exit` is 0 past them all; `how` says how it was cut short.
pub fn report_there(
    steps: &[StepThere],
    at: usize,
    how: Result<(), String>,
) -> (Vec<String>, Option<String>) {
    let done = match &how {
        Ok(()) => steps.len(),
        Err(_) => at.saturating_sub(1).min(steps.len()),
    };
    let ran = steps[..done].iter().map(StepThere::said).collect();
    let failed = how
        .err()
        .map(|how| match at.checked_sub(1).and_then(|i| steps.get(i)) {
            Some(step) => step.failure(&how),
            None => how,
        });
    (ran, failed)
}

/// How a setup command that ran past its time reads.
pub fn timed_out() -> String {
    Exit::TimedOut(TIMEOUT).describe()
}

/// The line a log ends with when a setup couldn't get to run its commands.
pub fn failed_line(why: &str) -> String {
    format!("{RUN_FAILED}: {why}")
}

/// The line a log starts a run with, at `at`.
pub fn started_line(at: u64) -> String {
    format!("{RUN_START} {}", super::verify::utc(at))
}

/// Whether `feature` has a worktree to set up.
pub fn has_work(feature: &Feature) -> bool {
    feature.repos.iter().any(|repo| repo.worktree.is_dir())
}

/// The lines `lists` give `repo` (its own project's, by its name), blank
/// ones left out.
fn lines_of<'a>(lists: &'a PerRepo, repo: &Repo) -> Vec<&'a str> {
    lists
        .of(repo)
        .and_then(|c| c.get(&repo.name))
        .into_iter()
        .flatten()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect()
}

/// One path to copy from a repository's main worktree to its feature's.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    /// Relative to the repository, with no slash at its end.
    path: String,
    /// A large one: by reflink when that works, and said in the progress.
    environment: bool,
}

/// What `listing` (`git ls-files --others --ignored --exclude-standard
/// --directory`, a folder with a slash at its end) has worth copying from
/// `main`: see the module's text.
fn picked(main: &Path, listing: &[&str]) -> Vec<Item> {
    let folders: Vec<&str> = (listing.iter().copied())
        .filter(|l| l.ends_with('/'))
        .collect();
    let mut items = Vec::new();
    for &entry in listing {
        if let Some(folder) = entry.strip_suffix('/') {
            let name = folder.rsplit('/').next().unwrap_or(folder);
            if name == NODE_MODULES || main.join(folder).join(VENV_MARK).is_file() {
                items.push(Item {
                    path: folder.to_owned(),
                    environment: true,
                });
            }
            continue;
        }
        let junk = entry.ends_with(".log") || entry.ends_with(".pyc");
        if junk || folders.iter().any(|f| entry.starts_with(f)) {
            continue;
        }
        let small = std::fs::symlink_metadata(main.join(entry))
            .is_ok_and(|meta| !meta.is_dir() && (!meta.is_file() || meta.len() <= MAX_FILE));
        if small {
            items.push(Item {
                path: entry.to_owned(),
                environment: false,
            });
        }
    }
    items
}

/// What to copy into `repo`'s worktree, by path, with `extras` from the
/// store file; `problems` is told what can't be.
fn to_copy(repo: &Repo, extras: &[&str], problems: &mut Vec<String>) -> Vec<Item> {
    let args = [
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ];
    let mut items = match git(&repo.main, &args) {
        Ok(out) => picked(
            &repo.main,
            &out.split('\0')
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>(),
        ),
        Err(err) => {
            problems.push(format!("ignored files (couldn't list them: {err})"));
            Vec::new()
        }
    };
    for &extra in extras {
        let path = extra.trim_end_matches('/');
        let inside = !path.is_empty()
            && Path::new(path)
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !inside {
            problems.push(format!("{extra} (not a path inside the repository)"));
        } else if repo.main.join(path).symlink_metadata().is_err() {
            problems.push(format!("{path} (not in {})", shown(&repo.main)));
        } else if !items.iter().any(|i| i.path == path) {
            items.push(Item {
                path: path.to_owned(),
                environment: false,
            });
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

/// Whether the `len` bytes of `text` at `at` are a path as a whole: not
/// `/x/api` in `/x/api-2` or in `/mnt/x/api`.
fn whole_path(text: &[u8], at: usize, len: usize) -> bool {
    let word = |b: &u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.');
    let before = at.checked_sub(1).map(|i| &text[i]);
    !before.is_some_and(|b| word(b) || *b == b'/') && !text.get(at + len).is_some_and(word)
}

/// `text` with the path `from` as `to`, if it names it.
fn replaced(text: &[u8], from: &[u8], to: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    let (mut at, mut last, mut any) = (0, 0, false);
    while at + from.len() <= text.len() {
        if text[at..].starts_with(from) && whole_path(text, at, from.len()) {
            out.extend_from_slice(&text[last..at]);
            out.extend_from_slice(to);
            at += from.len();
            last = at;
            any = true;
        } else {
            at += 1;
        }
    }
    any.then(|| {
        out.extend_from_slice(&text[last..]);
        out
    })
}

/// The files of the virtual environment `venv` that may name its folder:
/// `bin`'s, `pyvenv.cfg`, and what points `site-packages` at code.
fn venv_files(venv: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = vec![venv.join(VENV_MARK)];
    for entry in std::fs::read_dir(venv.join("bin"))? {
        files.push(entry?.path());
    }
    let Ok(libs) = std::fs::read_dir(venv.join("lib")) else {
        return Ok(files);
    };
    for lib in libs {
        let Ok(packages) = std::fs::read_dir(lib?.path().join("site-packages")) else {
            continue;
        };
        for entry in packages {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.ends_with(".pth") || name.starts_with("__editable__") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// Make the copied virtual environment `venv` name `worktree` where it
/// names `main`. Text only: a binary would break with a path of another
/// length. A link goes on being a link, to the same place in `worktree`.
fn repoint(venv: &Path, main: &Path, worktree: &Path) -> std::io::Result<()> {
    let (from, to) = (main.as_os_str().as_bytes(), worktree.as_os_str().as_bytes());
    for file in venv_files(venv)? {
        let meta = std::fs::symlink_metadata(&file)?;
        if meta.is_symlink() {
            let target = std::fs::read_link(&file)?;
            if let Ok(rest) = target.strip_prefix(main) {
                std::fs::remove_file(&file)?;
                std::os::unix::fs::symlink(worktree.join(rest), &file)?;
            }
        } else if meta.is_file() {
            let text = std::fs::read(&file)?;
            if !text.contains(&0)
                && let Some(new) = replaced(&text, from, to)
            {
                std::fs::write(&file, new)?;
            }
        }
    }
    Ok(())
}

/// Remove what a copy left at `path`, which wasn't there before it.
fn remove(path: &Path) {
    // One of the two fails for not being that kind of thing, and a copy
    // that can't go leaves only what `git status` or the commands then say.
    let _ = std::fs::remove_dir_all(path);
    let _ = std::fs::remove_file(path);
}

/// Copy `from` to `to`, which isn't there, for a command in `dir`; its errors
/// to `log`. Past a failure, whatever it was (no reflink here, an attribute
/// the file system refuses with "Operation not permitted"), it tries a copy
/// that asks less: a setup without its environment can't run tests at all.
fn copy_tree(
    from: &Path,
    to: &Path,
    environment: bool,
    dir: &Path,
    slug: &str,
    log: &File,
) -> Result<Exit, String> {
    let (from_text, to_text) = (from.to_string_lossy(), to.to_string_lossy());
    // The last keeps links and modes, and no times or owners: a Python
    // compiles its files again once, and that's all it costs.
    let ways: &[&str] = if environment {
        &[
            "-a --reflink=always",
            "-a --reflink=auto",
            "-dR --reflink=auto",
        ]
    } else {
        &["-a --reflink=auto", "-dR --reflink=auto"]
    };
    let mut exit = Exit::Code(-1);
    for (i, how) in ways.iter().enumerate() {
        if i > 0 {
            tracing::warn!("{from_text}: `cp {}` {}", ways[i - 1], exit.describe());
            // Else the next would copy into what this one left, not to its name.
            remove(to);
            let mut note = log;
            // The log is a help to read: see `run_into`.
            let _ = writeln!(note, "== that copy failed, so trying `cp {how}`");
        }
        let command = format!(
            "cp {how} -- {} {}",
            crate::desktop::shell_quote(&from_text),
            crate::desktop::shell_quote(&to_text),
        );
        let err = log.try_clone().map_err(|err| err.to_string())?;
        exit = shell(
            &command,
            dir,
            slug,
            Stdio::null(),
            Stdio::from(err),
            TIMEOUT,
        );
        // Another try wouldn't be any quicker than the one out of time.
        if exit.passed() || matches!(exit, Exit::TimedOut(_)) {
            break;
        }
    }
    Ok(exit)
}

/// Copy `item` into `repo`'s worktree; its errors to `log`. Why not, if it
/// didn't.
fn copy(repo: &Repo, item: &Item, slug: &str, log: &File) -> Result<(), String> {
    let (from, to) = (repo.main.join(&item.path), repo.worktree.join(&item.path));
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let exit = copy_tree(&from, &to, item.environment, &repo.worktree, slug, log)?;
    let checked = if !exit.passed() {
        Err(exit.describe())
    } else if from.join(VENV_MARK).is_file() {
        // Half repointed it would run main's code in places.
        repoint(&to, &repo.main, &repo.worktree)
            .map_err(|err| format!("couldn't repoint it: {err}"))
    } else {
        Ok(())
    };
    let shows = ["--literal-pathspecs", "status", "--porcelain", "--"];
    let checked = checked.and_then(|()| {
        let mut args = shows.to_vec();
        args.push(&item.path);
        match git(&repo.worktree, &args) {
            Ok(out) if out.trim().is_empty() => Ok(()),
            Ok(_) => Err("git doesn't ignore it on this branch".to_owned()),
            Err(err) => Err(err),
        }
    });
    if checked.is_err() {
        remove(&to);
    }
    checked
}

/// Set up every repository of `feature` with a worktree: the copies, with
/// the `extras` of the store file, then its `commands`, logging to `log`.
/// `progress` is told what runs, in words: "services-api: npm ci (2 of 3)".
pub fn run(
    feature: &Feature,
    commands: &PerRepo,
    extras: &PerRepo,
    venv: &VenvSetting,
    log: &Path,
    progress: &mut dyn FnMut(String),
) -> Report {
    let mut report = Report::default();
    let started = Instant::now();
    let result = run_into(&mut report, feature, commands, extras, venv, log, progress);
    let end = match &result {
        Ok(()) => format!("== Setup finished after {} s", started.elapsed().as_secs()),
        Err(why) => format!("{RUN_FAILED}: {why}"),
    };
    if let Err(why) = result {
        report.failed = Some(why);
    }
    // The log is a help to read, not the result: see `run_into`.
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) {
        let _ = writeln!(file, "{end}\n");
    }
    report
}

/// What the viewer shows of the setup log `raw`: its last run, without
/// terminal escapes, a line a program drew over and over only as it ended.
pub fn shown_log(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let run = (text.match_indices(RUN_START))
        .filter(|(at, _)| *at == 0 || text.as_bytes()[at - 1] == b'\n')
        .last()
        .map_or(0, |(at, _)| at);
    let plain = super::verify::strip_escapes(&text[run..]);
    let lines = plain.split('\n').map(|line| {
        let line = line.trim_end_matches('\r');
        line.rsplit('\r').next().unwrap_or(line)
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// `shown_log`'s text with the escapes that draw it: gerdr's own lines bold,
/// a failure red too, the commands' output plain (see `Scrollback::set_styles`).
pub fn log_styles(text: &str) -> String {
    let drawn = text.lines().map(|line| {
        if line.starts_with(RUN_FAILED) || (line.starts_with("== ") && line.contains(": couldn't "))
        {
            format!("\x1b[1;31m{line}\x1b[0m")
        } else if line.starts_with("== ") {
            format!("\x1b[1m{line}\x1b[0m")
        } else {
            line.to_owned()
        }
    });
    drawn.collect::<Vec<_>>().join("\n")
}

/// `run`, telling `report` as it goes; the error is what stopped it.
fn run_into(
    report: &mut Report,
    feature: &Feature,
    commands: &PerRepo,
    extras: &PerRepo,
    venv: &VenvSetting,
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
    let _ = writeln!(file, "{RUN_START} {}", super::verify::utc(now()));
    let mut repos: Vec<_> = feature
        .repos
        .iter()
        .filter(|r| r.worktree.is_dir())
        .collect();
    repos.sort_by_key(|r| r.key());
    let total: usize = repos.iter().map(|r| lines_of(commands, r).len()).sum();
    // Asking poetry or pipenv takes a moment: only with something to run.
    let tools = (total > 0).then(Tools::detect).unwrap_or_default();
    let mut i = 0;
    for repo in repos {
        let name = &repo.key();
        let mut problems = Vec::new();
        let mut copied = Vec::new();
        for item in to_copy(repo, &lines_of(extras, repo), &mut problems) {
            let path = &item.path;
            if repo.worktree.join(path).symlink_metadata().is_ok() {
                let _ = writeln!(file, "== {name}: kept {path}, already there");
                continue;
            }
            if item.environment {
                progress(format!("{name}: copying {path}"));
            }
            let note = match copy(repo, &item, &feature.slug, &file) {
                Ok(()) => {
                    copied.push(path.clone());
                    format!("copied {path} from {}", shown(&repo.main))
                }
                Err(why) => {
                    tracing::warn!("{name}: couldn't copy {path}: {why}");
                    problems.push(format!("{path} ({why})"));
                    format!("couldn't copy {path} from {} ({why})", shown(&repo.main))
                }
            };
            // The log is a help to read, not the result: a line that can't
            // be written doesn't stop the setup (its error is the result).
            let _ = writeln!(file, "== {} {name}: {note}", super::verify::utc(now()));
        }
        if !copied.is_empty() {
            report.copied.push(format!("{name}: {}", copied.join(", ")));
        }
        report
            .not_copied
            .extend(problems.iter().map(|what| format!("{name}: {what}")));
        for command in lines_of(commands, repo) {
            i += 1;
            progress(format!("{name}: {command} ({i} of {total})"));
            // As above: a log line lost doesn't stop the setup.
            let _ = writeln!(file, "== {} {name}: {command}", super::verify::utc(now()));
            let _ = writeln!(file, "== in {}", shown(&repo.worktree));
            let activated = background_command(command, &repo.worktree, &feature.dir, venv, &tools);
            if activated != command {
                let _ = writeln!(file, "== runs as: {activated}");
            }
            let _ = file.flush();
            let out = file
                .try_clone()
                .map_err(|err| format!("couldn't write to {}: {err}", shown(log)))?;
            let err = file
                .try_clone()
                .map_err(|err| format!("couldn't write to {}: {err}", shown(log)))?;
            let started = Instant::now();
            let exit = shell(
                &activated,
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
            report.ran.push(format!("{name}: {command}"));
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
    shell_until(
        command,
        dir,
        slug,
        stdout,
        stderr,
        timeout,
        &AtomicBool::new(false),
    )
}

/// `shell` that `stop` ends too, like the timeout does: then what it
/// returns says nothing, and the caller knows why it ended.
pub(super) fn shell_until(
    command: &str,
    dir: &Path,
    slug: &str,
    stdout: Stdio,
    stderr: Stdio,
    timeout: Duration,
    stop: &AtomicBool,
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
        // What it prints reaches its log as it prints it.
        .env("PYTHONUNBUFFERED", "1")
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
            Ok(None) if started.elapsed() >= timeout || stop.load(Ordering::Relaxed) => break None,
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
        // Only reaps it after SIGKILL: how it ended is already known.
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
pub(super) fn kill_group(group: i32, signal: i32) {
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
    use crate::feature::tests::{Temp, feature_over, link_over};

    fn step_there(repo: &str, dir: &Path, line: &str) -> StepThere {
        StepThere {
            repo: repo.into(),
            dir: dir.to_string_lossy().into_owned(),
            command: line.into(),
            line: line.into(),
        }
    }

    /// Run a setup script as a terminal there would: what it left, and its log.
    fn run_there(t: &Temp, steps: &[StepThere]) -> (PathBuf, String) {
        let dir = t.0.as_path().join("run");
        let log = t.0.as_path().join("data/setup.log");
        let script = script_there(
            &dir.to_string_lossy(),
            &log.to_string_lossy(),
            steps,
            "slug",
            &["== api: copied .env".to_owned()],
            crate::feature::remote::ACTIVATE,
        );
        let out = Command::new("sh").arg("-c").arg(&script).output().unwrap();
        assert!(out.stderr.is_empty(), "{out:?}");
        (dir, std::fs::read_to_string(&log).unwrap())
    }

    #[test]
    fn a_setup_there_runs_its_commands_in_their_folders_and_says_how_it_went() {
        let t = Temp::new();
        let (a, b) = (t.0.as_path().join("a"), t.0.as_path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let steps = [
            step_there("a", &a, "echo $GERDR_FEATURE > made; echo out"),
            step_there("b", &b, "pwd"),
        ];
        let (dir, log) = run_there(&t, &steps);
        assert_eq!(std::fs::read_to_string(a.join("made")).unwrap(), "slug\n");
        assert_eq!(std::fs::read_to_string(dir.join("exit")).unwrap(), "0\n");
        assert_eq!(std::fs::read_to_string(dir.join("step")).unwrap(), "2\n");
        assert!(log.contains("== api: copied .env"), "{log}");
        assert!(log.contains("\nout\n"), "{log}");
        assert!(log.contains("== Setup finished after"), "{log}");
        assert!(!log.contains(RUN_FAILED), "{log}");
        assert_eq!(
            report_there(&steps, 2, Ok(())),
            (vec![steps[0].said(), steps[1].said()], None)
        );
    }

    #[test]
    fn a_setup_there_stops_at_its_first_failure() {
        let t = Temp::new();
        let a = t.0.as_path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        let steps = [
            step_there("a", &a, "exit 3"),
            step_there("a", &a, "touch never"),
        ];
        let (dir, log) = run_there(&t, &steps);
        assert_eq!(std::fs::read_to_string(dir.join("exit")).unwrap(), "3\n");
        assert!(!a.join("never").exists());
        assert!(
            log.contains("== Setup failed: a: `exit 3` exited with 3"),
            "{log}"
        );
        let (ran, failed) = report_there(&steps, 1, Err("exited with 3".into()));
        assert!(ran.is_empty());
        assert_eq!(failed.as_deref(), Some("a: `exit 3` exited with 3"));
    }

    #[test]
    fn a_setup_there_without_its_folder_fails_and_runs_nothing_elsewhere() {
        let t = Temp::new();
        let steps = [step_there("a", &t.0.as_path().join("gone"), "touch here")];
        let dir = t.0.as_path().join("run");
        let script = script_there(
            &dir.to_string_lossy(),
            &t.0.as_path().join("l").to_string_lossy(),
            &steps,
            "s",
            &[],
            crate::feature::remote::ACTIVATE,
        );
        let out = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .current_dir(t.0.as_path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(!t.0.as_path().join("here").exists());
    }

    #[test]
    fn the_environment_built_there_is_on_for_the_next_command() {
        let t = Temp::new();
        let a = t.0.as_path().join("a");
        let bin = a.join(".venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(a.join(".venv/pyvenv.cfg"), "").unwrap();
        std::fs::write(bin.join("activate"), "ACTIVE=yes\nexport ACTIVE\n").unwrap();
        let steps = [step_there("a", &a, "echo $ACTIVE > said")];
        run_there(&t, &steps);
        assert_eq!(std::fs::read_to_string(a.join("said")).unwrap(), "yes\n");
    }

    fn by_name(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.iter().map(|c| (*c).into()).collect()))
            .collect()
    }

    fn map(pairs: &[(&str, &[&str])]) -> PerRepo {
        PerProject::all(by_name(pairs))
    }

    fn none() -> PerRepo {
        PerRepo::default()
    }

    /// Git ignores `lines` in `repo`'s main worktree only, as a local rule.
    fn ignore(repo: &Repo, lines: &str) {
        let info = repo.main.join(".git/info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("exclude"), lines).unwrap();
    }

    /// `path` of `dir` holding `text`, its folders made.
    fn put(dir: &Path, path: &str, text: &str) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn set_up(f: &Feature, extras: &PerRepo, t: &Temp) -> Report {
        run(
            f,
            &none(),
            extras,
            &VenvSetting::Auto,
            &t.0.join("setup.log"),
            &mut |_| {},
        )
    }

    #[test]
    fn a_linked_projects_repos_take_its_own_commands() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        link_over(&t, &mut f, "other", &["api"], &["api"]);
        let commands = PerRepo {
            home: by_name(&[("api", &["echo home > who"])]),
            links: [(
                "other".to_owned(),
                by_name(&[("api", &["echo other > who"])]),
            )]
            .into(),
        };
        let mut said = Vec::new();
        let report = run(
            &f,
            &commands,
            &none(),
            &VenvSetting::Auto,
            &t.0.join("setup.log"),
            &mut |s| said.push(s),
        );
        assert_eq!(report.failed, None);
        assert_eq!(
            said,
            [
                "api: echo home > who (1 of 2)",
                "other:api: echo other > who (2 of 2)"
            ]
        );
        let who = |r: usize| std::fs::read_to_string(f.repos[r].worktree.join("who")).unwrap();
        assert_eq!((who(0), who(1)), ("home\n".into(), "other\n".into()));
    }

    #[test]
    fn work_is_there_for_any_worktree_commands_or_not() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &["web"]);
        assert!(has_work(&f));
        std::fs::remove_dir_all(&f.repos[0].worktree).unwrap();
        assert!(has_work(&f));
        std::fs::remove_dir_all(&f.repos[1].worktree).unwrap();
        assert!(!has_work(&f));
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
        let report = run(&f, &commands, &none(), &VenvSetting::Auto, &log, &mut |s| {
            said.push(s)
        });
        assert_eq!(
            report.failed.as_deref(),
            Some("web: `exit 3` exited with 3")
        );
        assert_eq!(
            report.ran,
            [
                "api: echo $GERDR_FEATURE > slug",
                "api: echo out; echo err >&2",
            ]
        );
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
            text.contains("api: echo out; echo err >&2\n== in "),
            "{text}"
        );
        assert!(text.contains("\nout\nerr\n"), "{text}");
        assert!(text.contains("== exited with 3 after"), "{text}");
        assert!(
            text.contains("== Setup failed: web: `exit 3` exited with 3\n"),
            "{text}"
        );
        // A second run appends.
        run(
            &f,
            &map(&[("api", &["true"])]),
            &none(),
            &VenvSetting::Auto,
            &log,
            &mut |_| {},
        );
        let again = std::fs::read_to_string(&log).unwrap();
        assert!(again.starts_with(&text) && again.len() > text.len());
    }

    #[test]
    fn the_log_shows_its_last_run_as_it_ended() {
        let raw = b"== Setup started 1\nold\n== Setup started 2\n\x1b[32mgreen\x1b[0m\n10%\r50%\r100%\r\nsays == Setup started\n";
        assert_eq!(
            shown_log(raw),
            "== Setup started 2\ngreen\n100%\nsays == Setup started\n"
        );
        // A log from before runs were marked shows whole.
        assert_eq!(shown_log(b"a\nb\n"), "a\nb\n");
        assert_eq!(shown_log(b""), "");
    }

    #[test]
    fn the_log_draws_gerdrs_lines_apart_from_the_output() {
        assert_eq!(
            log_styles("== in ~/x\nout\n== Setup failed: no"),
            "\x1b[1m== in ~/x\x1b[0m\nout\n\x1b[1;31m== Setup failed: no\x1b[0m"
        );
    }

    #[test]
    fn a_rerun_says_what_it_kept_and_how_it_ended() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        put(&f.repos[0].main, ".env", "A=1\n");
        ignore(&f.repos[0], ".env\n");
        let log = t.0.join("data/setup.log");
        let commands = map(&[("api", &["true"])]);
        let again = |log: &Path| {
            run(&f, &commands, &none(), &VenvSetting::Auto, log, &mut |_| {});
            shown_log(&std::fs::read(log).unwrap())
        };
        assert!(again(&log).contains("copied .env"));
        let text = again(&log);
        assert!(text.starts_with("== Setup started"), "{text}");
        assert!(
            text.contains("== api: kept .env, already there\n"),
            "{text}"
        );
        assert!(text.contains("api: true\n== in "), "{text}");
        assert!(text.contains("== Setup finished after "), "{text}");
    }

    #[test]
    fn folders_are_copied_from_main_or_left_to_the_commands() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        let (api, web) = (&f.repos[0], &f.repos[1]);
        ignore(api, "node_modules/\n");
        ignore(web, "node_modules/\n");
        put(&api.main, "node_modules/x/i.js", "1");
        // A worktree that has the folder keeps its own.
        put(&web.main, "node_modules/x/i.js", "1");
        put(&web.worktree, "node_modules/own", "");
        let log = t.0.join("setup.log");
        let mut said = Vec::new();
        let report = run(&f, &none(), &none(), &VenvSetting::Auto, &log, &mut |s| {
            said.push(s)
        });
        assert_eq!(said, ["api: copying node_modules"]);
        let copied = api.worktree.join("node_modules/x/i.js");
        // Copy-on-write or not, depending on the file system under /tmp.
        assert_eq!(report.not_copied, Vec::<String>::new());
        assert_eq!(report.copied, ["api: node_modules"]);
        assert_eq!(std::fs::read_to_string(copied).unwrap(), "1");
        assert!(web.worktree.join("node_modules/own").exists());
        assert!(!web.worktree.join("node_modules/x").exists());
    }

    /// A reflink from another file system always fails, whichever two.
    #[test]
    fn an_environment_copies_plainly_where_a_reflink_fails() {
        use std::os::unix::fs::PermissionsExt;
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let t = Temp::new();
        let elsewhere = Temp(shm.join(format!("gerdr-setup-{}", std::process::id())));
        let from = elsewhere.0.join(".venv");
        put(&from, "pyvenv.cfg", "home = /usr\n");
        put(&from, "bin/pytest", "#!/bin/sh\n");
        let run = from.join("bin/pytest");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", from.join("bin/python")).unwrap();
        let log_path = t.0.join("copy.log");
        let log = File::create(&log_path).unwrap();
        let to = t.0.join("copied");
        let exit = copy_tree(&from, &to, true, &t.0, "s", &log).unwrap();
        assert_eq!(exit, Exit::Code(0));
        let meta = std::fs::metadata(to.join("bin/pytest")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o111, 0o111);
        let link = std::fs::read_link(to.join("bin/python")).unwrap();
        assert_eq!(link, Path::new("/usr/bin/python3"));
        // Into it rather than to its name, were the first try's left there.
        assert!(!to.join(".venv").exists());
        let said = std::fs::read_to_string(&log_path).unwrap();
        assert!(said.contains("so trying `cp -a --reflink=auto`"), "{said}");
    }

    #[test]
    fn small_ignored_files_are_copied_and_the_rest_left() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let api = &f.repos[0];
        ignore(
            api,
            ".env\nlocal_config.py\n*.log\n*.pyc\nbig.dump\ncache/\nsub/.env.test\n",
        );
        put(&api.main, ".env", "SECRET=1");
        put(&api.main, "local_config.py", "x = 1");
        put(&api.main, "sub/kept", "");
        crate::feature::tests::run(&api.main, &["add", "sub/kept"]);
        crate::feature::tests::run(&api.main, &["commit", "-q", "-m", "sub"]);
        // The feature's branch hasn't this folder: the copy makes it.
        put(&api.main, "sub/.env.test", "T=1");
        put(&api.main, "debug.log", "noise");
        put(&api.main, "sub/kept.cpython-313.pyc", "stale");
        put(&api.main, "cache/.env", "in an ignored folder");
        let big = "x".repeat(MAX_FILE as usize + 1);
        put(&api.main, "big.dump", &big);
        let report = set_up(&f, &none(), &t);
        assert_eq!(report.copied, ["api: .env, local_config.py, sub/.env.test"]);
        assert_eq!(report.not_copied, Vec::<String>::new());
        let w = &api.worktree;
        assert_eq!(std::fs::read_to_string(w.join(".env")).unwrap(), "SECRET=1");
        assert!(w.join("sub/.env.test").exists());
        assert!(!w.join("debug.log").exists());
        assert!(!w.join("sub/kept.cpython-313.pyc").exists());
        assert!(!w.join("big.dump").exists());
        assert!(!w.join("cache").exists());
        assert_eq!(
            crate::feature::tests::run(w, &["status", "--porcelain"]),
            ""
        );
    }

    #[test]
    fn nothing_there_is_overwritten_and_a_second_run_only_fills_in() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let api = &f.repos[0];
        ignore(api, ".env\n.env.test\n");
        put(&api.main, ".env", "from main");
        put(&api.worktree, ".env", "the feature's");
        assert_eq!(set_up(&f, &none(), &t).copied, Vec::<String>::new());
        let w = &api.worktree;
        assert_eq!(
            std::fs::read_to_string(w.join(".env")).unwrap(),
            "the feature's"
        );
        put(&api.main, ".env.test", "new");
        assert_eq!(set_up(&f, &none(), &t).copied, ["api: .env.test"]);
        assert_eq!(set_up(&f, &none(), &t), Report::default());
    }

    #[test]
    fn the_store_files_paths_are_copied_too_if_git_ignores_them() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let api = &f.repos[0];
        // A folder whose contents are ignored, not the folder: as in
        // `.elasticbeanstalk/*`.
        ignore(api, ".eb/*\nfixtures/\n");
        put(&api.main, ".eb/config.yml", "a");
        let big = "x".repeat(MAX_FILE as usize + 1);
        put(&api.main, "fixtures/large.bin", &big);
        put(&api.main, "shows.txt", "git sees this one");
        let extras = map(&[(
            "api",
            &[".eb/", "fixtures", "shows.txt", "gone", "../up", "/abs"],
        )]);
        let report = set_up(&f, &extras, &t);
        assert_eq!(report.copied, ["api: .eb, fixtures"]);
        let w = &api.worktree;
        assert_eq!(
            std::fs::read_to_string(w.join(".eb/config.yml")).unwrap(),
            "a"
        );
        assert!(w.join("fixtures/large.bin").exists());
        // It would fail Verify: it went again.
        assert!(!w.join("shows.txt").exists());
        assert_eq!(report.not_copied.len(), 4, "{report:?}");
        let said = report.not_copied.join("\n");
        assert!(said.contains("api: ../up (not a path inside the repository)"));
        assert!(said.contains("api: /abs (not a path inside the repository)"));
        assert!(said.contains("api: gone (not in "));
        assert!(said.contains("api: shows.txt (git doesn't ignore it on this branch)"));
        assert_eq!(
            crate::feature::tests::run(w, &["status", "--porcelain"]),
            ""
        );
    }

    /// A virtual environment in `main/.venv`, as `python -m venv` and pip
    /// leave one, with an editable install of `main`.
    fn venv_in(main: &Path) {
        let m = main.display();
        put(
            main,
            ".venv/pyvenv.cfg",
            &format!("home = /usr/bin\ncommand = /usr/bin/python -m venv {m}/.venv\n"),
        );
        put(
            main,
            ".venv/bin/pytest",
            &format!("#!{m}/.venv/bin/python\nimport pytest\n"),
        );
        put(
            main,
            ".venv/bin/activate",
            &format!("VIRTUAL_ENV=\"{m}/.venv\"\n"),
        );
        let packages = ".venv/lib/python3.13/site-packages";
        put(main, &format!("{packages}/app.pth"), &format!("{m}\n"));
        put(
            main,
            &format!("{packages}/near.pth"),
            &format!("{m}-2/x\n/mnt{m}\n"),
        );
        put(main, &format!("{packages}/pytest.py"), &format!("# {m}\n"));
        let mut binary = format!("{m}/.venv").into_bytes();
        binary.push(0);
        std::fs::write(main.join(".venv/bin/tool"), binary).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", main.join(".venv/bin/python")).unwrap();
        std::os::unix::fs::symlink(
            main.join(".venv/bin/pytest"),
            main.join(".venv/bin/py.test"),
        )
        .unwrap();
    }

    #[test]
    fn a_copied_venv_names_the_worktree_not_main() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let (main, w) = (&f.repos[0].main, &f.repos[0].worktree);
        venv_in(main);
        let (m, wt) = (main.display().to_string(), w.display().to_string());
        let before = std::fs::read_to_string(main.join(".venv/bin/pytest")).unwrap();
        repoint_copy(main, w);
        let read = |p: &str| std::fs::read_to_string(w.join(".venv").join(p)).unwrap();
        assert_eq!(
            read("bin/pytest"),
            format!("#!{wt}/.venv/bin/python\nimport pytest\n")
        );
        assert_eq!(
            read("bin/activate"),
            format!("VIRTUAL_ENV=\"{wt}/.venv\"\n")
        );
        assert!(read("pyvenv.cfg").ends_with(&format!("-m venv {wt}/.venv\n")));
        let packages = "lib/python3.13/site-packages";
        // An editable install imports the feature's code.
        assert_eq!(read(&format!("{packages}/app.pth")), format!("{wt}\n"));
        // What only looks like the path stays; so does what isn't read.
        assert_eq!(
            read(&format!("{packages}/near.pth")),
            format!("{m}-2/x\n/mnt{m}\n")
        );
        assert_eq!(read(&format!("{packages}/pytest.py")), format!("# {m}\n"));
        let binary = std::fs::read(w.join(".venv/bin/tool")).unwrap();
        assert!(binary.starts_with(m.as_bytes()));
        // The interpreter stays; a link into the folder follows it.
        let link = |p: &str| std::fs::read_link(w.join(".venv/bin").join(p)).unwrap();
        assert_eq!(link("python"), Path::new("/usr/bin/python3"));
        assert_eq!(link("py.test"), w.join(".venv/bin/pytest"));
        // Main's own stays as it was.
        let after = std::fs::read_to_string(main.join(".venv/bin/pytest")).unwrap();
        assert_eq!(after, before);
    }

    /// `cp -a` of `main`'s `.venv` to `worktree`, whatever the file system,
    /// then `repoint`.
    fn repoint_copy(main: &Path, worktree: &Path) {
        let done = Command::new("cp")
            .arg("-a")
            .arg(main.join(".venv"))
            .arg(worktree.join(".venv"))
            .status()
            .unwrap();
        assert!(done.success());
        repoint(&worktree.join(".venv"), main, worktree).unwrap();
    }

    #[test]
    fn a_venv_is_found_by_its_mark_whatever_its_name() {
        let t = Temp::new();
        put(&t.0, "env3/pyvenv.cfg", "");
        put(&t.0, "web/node_modules/x", "");
        put(&t.0, "build/x", "");
        let items = picked(&t.0, &["build/", "env3/", "web/node_modules/"]);
        let paths: Vec<_> = items
            .iter()
            .map(|i| (i.path.as_str(), i.environment))
            .collect();
        assert_eq!(paths, [("env3", true), ("web/node_modules", true)]);
    }

    #[test]
    fn a_path_is_replaced_only_as_a_whole() {
        let swap = |text: &str| {
            replaced(text.as_bytes(), b"/x/api", b"/y/f/api").map(|b| String::from_utf8(b).unwrap())
        };
        assert_eq!(
            swap("#!/x/api/.venv/bin/python").unwrap(),
            "#!/y/f/api/.venv/bin/python"
        );
        assert_eq!(swap("/x/api\n/x/api\n").unwrap(), "/y/f/api\n/y/f/api\n");
        assert_eq!(swap("a=\"/x/api\"").unwrap(), "a=\"/y/f/api\"");
        assert_eq!(swap("/x/api-2/a"), None);
        assert_eq!(swap("/x/api.old"), None);
        assert_eq!(swap("/mnt/x/api/a"), None);
        assert_eq!(swap("nothing"), None);
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
