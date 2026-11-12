//! Verify: gerdr runs the project's checks on a feature's code and writes
//! what they said to `evidence.md` in the feature data dir.
//!
//! The checks come from the project's store file (the project commands
//! dialog edits them, see `project_commands`), by repository name or
//! `_feature` (`"workflow": {"verify": {"services-api": ["npm test"],
//! "_feature": ["./e2e.sh"]}}`): each touched repository's run in its
//! worktree, and `_feature`'s in the feature folder.
//! An untouched repository's checks don't run. A linked project's
//! repository takes its own project's checks, by its name; `_feature` is
//! the home project's alone.
//!
//! A feature borrows those, and can add its own (`Feature::checks`, by
//! `Repo::key` or `_feature`): each key's run beside what it borrows, a
//! command it borrows already not running twice. It can also skip what it
//! borrows (`Feature::skip_checks`), by the command's words: reworded in the
//! project, a check runs again. The evidence says what was skipped.
//!
//! Verify fails when:
//! - the code reads dirty, or an untouched repository has changes
//!   (`patch::Reading::dirty`, `stray`): nothing runs;
//! - a check exits non-zero or runs past the timeout; the others go on, so
//!   the evidence is whole;
//! - the code reads differently after the checks than before: a check
//!   changed it (a formatter, a snapshot update).
//!
//! Every check runs at the same time, each in a terminal of its own in the
//! feature's workspace: `feature_checks` opens them, with the Node and venv
//! a new terminal there gets. What runs is a small POSIX script (`script`),
//! so it reads the same in any shell. It runs the check with `sh -c`, stdin
//! closed so nothing waits on a watch mode or a question, and writes the exit
//! code to a file: herdr knows none. `watch` waits for those files, each
//! check for at most the timeout, and the evidence keeps the last `TAIL`
//! lines of what its terminal shows.
//!
//! A check interrupted (stopped, its terminal closed, its shell never
//! starting) says nothing of the code: Verify fails, but its failures don't
//! go to the Build agent.
//!
//! Run again after fixes, only what can say something new runs: the checks
//! that didn't pass, and those of a repository whose `HEAD` moved. A passed
//! check of a repository that didn't move stands (`VerifyRun::holds`), though
//! it never ran beside the new code of the others. A `_feature` check reads
//! them all, so it runs again whenever any moved. Run checks runs them all.
//!
//! Verify can run on another machine (`Stage::Verify`'s place, as for an
//! agent): the same script in a terminal there, its files read and its
//! processes signalled over the link (`Files`, `machine_checks`). What is judged
//! stays the code read here, which goes there by fast-forward first. What can't
//! be told over a lost link decides nothing.
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{
    Feature, PerProject,
    layout::git,
    now, patch,
    setup::{kill_group, minutes},
};
use crate::desktop::shell_quote;

/// The checks' key for the feature folder as a whole.
pub const FEATURE_KEY: &str = "_feature";

/// The evidence file's name, in the feature data dir.
pub const EVIDENCE: &str = "evidence.md";

/// How long one check may run, unless the store file says otherwise
/// (`"workflow": {"verify_timeout": 900}`, seconds). Generous: all checks run
/// at once, often beside busy agents, and a test run takes far longer then.
pub const TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How many of a check's last output lines the evidence keeps.
pub const TAIL: usize = 40;

/// How many of a check's output lines its card on the Verify page shows.
pub const PREVIEW_LINES: usize = 4;

/// One check to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The repository's `Repo::key`, or `FEATURE_KEY`.
    pub repo: String,
    /// Where it runs: the repository's worktree, or the feature folder.
    pub dir: PathBuf,
    pub command: String,
    /// Only this feature runs it: it isn't borrowed from a project.
    pub own: bool,
}

impl Check {
    /// What it is, in a line of the Verify page.
    pub fn line(&self) -> String {
        let repo = match self.repo.as_str() {
            FEATURE_KEY => "whole feature",
            repo => repo,
        };
        let only = if self.own { " (this feature only)" } else { "" };
        format!("{repo}: {}{only}", self.command)
    }

    /// What it is, in a line, when the feature skips it.
    pub fn skipped_line(&self) -> String {
        format!("{} (skipped for this feature)", self.line())
    }
}

/// What one check did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub repo: String,
    pub command: String,
    /// Its exit code; `None` when it was killed (timed out) or never ran
    /// to its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// How it ended when it has no exit code to say so: "stopped".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether it was cut short by something other than the code: stopped,
    /// or its terminal gone. It says nothing of the code.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// How long it ran, in seconds.
    #[serde(default)]
    pub secs: u64,
    /// The last lines its terminal showed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tail: String,
    /// When the code failed it: the lines of its output that say why (see
    /// `failure_lines`), which may lie before `tail`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excerpt: Vec<String>,
    /// All its terminal kept, to keep with the AI work (see `Panes::keep_log`)
    /// and then let go of. Never saved: the record keeps `tail`.
    #[serde(skip)]
    pub log: Option<String>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.exit == Some(0)
    }

    /// What its card previews: why it failed when that was found, else the
    /// last lines it printed.
    pub fn preview(&self) -> Vec<String> {
        match self.excerpt.is_empty() {
            true => last_lines(&self.tail),
            false => self.excerpt.clone(),
        }
    }
}

/// What Verify found: what `evidence.md` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// Unix seconds it started.
    pub at: u64,
    /// The feature's patch-id the checks ran on; empty when it couldn't
    /// be read.
    pub patch: String,
    /// Each touched repository's `HEAD` commit, by `Repo::key`.
    pub heads: Vec<(String, String)>,
    pub outcomes: Vec<Outcome>,
    /// What failed outside the checks, one line each: "uncommitted
    /// changes in services-api", "services-web isn't touched but has
    /// changes", "the checks changed the code in services-api",
    /// "couldn't read the code: …".
    pub problems: Vec<String>,
    /// Where the checks ran, in words, if not on this machine.
    pub ran_on: Option<String>,
}

impl Evidence {
    /// Whether every check passed and nothing else failed.
    pub fn passed(&self) -> bool {
        self.problems.is_empty() && self.outcomes.iter().all(Outcome::passed)
    }

    /// Whether a check was cut short: what failed may not be the code's.
    pub fn interrupted(&self) -> bool {
        self.outcomes.iter().any(|o| o.interrupted)
    }

    /// What failed, one short line each, problems first:
    /// "services-api: `npm test` exited with 1".
    pub fn failures(&self) -> Vec<String> {
        let failed = self
            .outcomes
            .iter()
            .filter(|o| !o.passed())
            .map(|o| format!("{}: `{}` {}", o.repo, o.command, o.how()));
        self.problems.iter().cloned().chain(failed).collect()
    }
}

impl Outcome {
    /// How it ended, after its command: "exited with 1", "timed out after
    /// 60 min", "stopped".
    pub fn how(&self) -> String {
        if self.timed_out {
            format!(
                "timed out after {}",
                minutes(Duration::from_secs(self.secs))
            )
        } else if let Some(why) = &self.error {
            why.clone()
        } else if let Some(code) = self.exit {
            format!("exited with {code}")
        } else {
            "ended without an exit code".into()
        }
    }
}

/// Each project's checks, by repository name or `FEATURE_KEY`: the store
/// file's `"workflow": {"verify": ...}`.
pub type Config = PerProject<BTreeMap<String, Vec<String>>>;

/// The checks `config` gives `feature`, in the order they are listed.
pub fn checks(feature: &Feature, config: &Config) -> Vec<Check> {
    split(feature, config).0
}

/// The checks `config` gives `feature` that it skips.
pub fn skipped(feature: &Feature, config: &Config) -> Vec<Check> {
    split(feature, config).1
}

/// A check `config` gives a feature, as its Verify page lists it whether or
/// not a run got to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCheck {
    pub repo: String,
    pub command: String,
    /// The feature skips it.
    pub skipped: bool,
    /// Only this feature runs it: it isn't borrowed, so can't be skipped.
    pub own: bool,
}

/// `repo` (a `Repo::key` or `FEATURE_KEY`) as the user knows it.
pub fn place_of(repo: &str) -> &str {
    match repo {
        FEATURE_KEY => "feature folder",
        repo => repo,
    }
}

/// All the checks `config` gives `feature`, skipped or not, as they are
/// listed: a skip leaves its check where it stands.
pub fn plan_cards(feature: &Feature, config: &Config) -> Vec<PlannedCheck> {
    (listed(feature, config).into_iter())
        .map(|(check, skipped)| PlannedCheck {
            repo: check.repo,
            command: check.command,
            skipped,
            own: check.own,
        })
        .collect()
}

/// What `feature` runs of what `config` gives it, in order, and what it
/// skips.
fn split(feature: &Feature, config: &Config) -> (Vec<Check>, Vec<Check>) {
    let (skips, runs): (Vec<_>, Vec<_>) =
        (listed(feature, config).into_iter()).partition(|(_, skipped)| *skipped);
    let checks = |of: Vec<(Check, bool)>| of.into_iter().map(|(check, _)| check).collect();
    (checks(runs), checks(skips))
}

/// Each check `config` gives `feature` in its order, and if it is skipped.
fn listed(feature: &Feature, config: &Config) -> Vec<(Check, bool)> {
    let commands = |config: Option<&BTreeMap<String, Vec<String>>>, key: &str| {
        config
            .and_then(|c| c.get(key))
            .into_iter()
            .flatten()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    // What `key` borrows, skipped or not, then what is its own and not
    // borrowed and run already.
    let of = |borrowed: Vec<String>, key: String, dir: &PathBuf| {
        let skip = commands(Some(&feature.skip_checks), &key);
        let own: Vec<String> = commands(Some(&feature.checks), &key)
            .into_iter()
            .filter(|c| !borrowed.contains(c) || skip.contains(c))
            .collect();
        let check = |command: String, own: bool| Check {
            repo: key.clone(),
            dir: dir.clone(),
            command,
            own,
        };
        (borrowed.into_iter())
            .map(|command| {
                let skipped = skip.contains(&command);
                (check(command, false), skipped)
            })
            .chain(own.into_iter().map(|command| (check(command, true), false)))
            .collect::<Vec<_>>()
    };
    let mut repos: Vec<_> = feature.touched().collect();
    repos.sort_by_key(|r| r.key());
    let mut checks = Vec::new();
    for repo in repos {
        let borrowed = commands(config.of(repo), &repo.name);
        checks.extend(of(borrowed, repo.key(), &repo.worktree));
    }
    let borrowed = commands(Some(&config.home), FEATURE_KEY);
    checks.extend(of(borrowed, FEATURE_KEY.into(), &feature.dir));
    checks
}

/// The code a run of checks starts on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Begun {
    /// The feature's patch-id.
    pub patch: String,
    /// Each touched repository's `HEAD` commit, by `Repo::key`.
    pub heads: Vec<(String, String)>,
}

/// Read `feature`'s code ahead of its checks. The evidence, when nothing is
/// to run: the code can't be read, or has changes it shouldn't.
pub fn begin(feature: &Feature) -> Result<Begun, Box<Evidence>> {
    let mut evidence = Evidence {
        at: now(),
        ..Evidence::default()
    };
    let before = match patch::read(feature) {
        Ok(reading) => reading,
        Err(err) => {
            evidence
                .problems
                .push(format!("couldn't read the code: {err}"));
            return Err(Box::new(evidence));
        }
    };
    evidence.patch = before.patch.clone();
    for name in &before.dirty {
        evidence
            .problems
            .push(format!("uncommitted changes in {name}"));
    }
    for name in &before.stray {
        evidence.problems.push(format!(
            "{name} isn't touched by the feature but has changes"
        ));
    }
    if !evidence.problems.is_empty() {
        return Err(Box::new(evidence));
    }
    let mut touched: Vec<_> = feature.touched().collect();
    touched.sort_by_key(|r| r.key());
    let mut heads = Vec::new();
    for repo in touched {
        match git(&repo.worktree, &["rev-parse", "HEAD"]) {
            Ok(head) => heads.push((repo.key(), head.trim().to_owned())),
            Err(err) => {
                evidence
                    .problems
                    .push(format!("couldn't read {}'s HEAD: {err}", repo.key()));
                return Err(Box::new(evidence));
            }
        }
    }
    Ok(Begun {
        patch: before.patch,
        heads,
    })
}

/// The evidence of checks that ended with `outcomes`, on the code `begun`
/// read: they mustn't have changed it.
pub fn finish(feature: &Feature, begun: &Begun, at: u64, outcomes: Vec<Outcome>) -> Evidence {
    let mut evidence = Evidence {
        at,
        patch: begun.patch.clone(),
        heads: begun.heads.clone(),
        outcomes,
        problems: Vec::new(),
        ran_on: None,
    };
    match patch::read(feature) {
        Err(err) => evidence
            .problems
            .push(format!("couldn't read the code after the checks: {err}")),
        Ok(after) => {
            let changed: Vec<&str> = after
                .dirty
                .iter()
                .chain(&after.stray)
                .map(String::as_str)
                .collect();
            if !changed.is_empty() {
                evidence.problems.push(format!(
                    "the checks changed the code in {}",
                    changed.join(", ")
                ));
            }
            if after.patch != begun.patch {
                evidence
                    .problems
                    .push("the checks changed the code in the feature's commits".into());
            }
        }
    }
    evidence
}

/// Which of a feature's checks to run.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    /// Them all.
    #[default]
    All,
    /// These, by `CheckRun::key`. What the last run said of the others
    /// stands, where their code didn't change (`VerifyRun::holds`).
    Only(Vec<(String, String)>),
}

impl Scope {
    /// What a run after `last` runs when the code changed: what didn't pass.
    /// The checks of what changed run as well, as `plan` keeps none of them.
    pub fn after_changes(last: Option<&VerifyRun>) -> Self {
        last.map_or(Self::All, |run| Self::Only(run.not_passed()))
    }
}

/// What run `n` of `checks` is to do: a row for each, those that `scope`
/// keeps with their outcomes from the `last` run if `holds` says that still
/// goes for their repository. Every row keeps its terminal, to run in again.
pub fn plan(
    checks: &[Check],
    last: &[CheckRun],
    holds: &dyn Fn(&str) -> bool,
    scope: &Scope,
    n: u32,
) -> Vec<CheckRun> {
    checks
        .iter()
        .map(|check| {
            let key = (check.repo.clone(), check.command.clone());
            let was = last.iter().find(|c| c.key() == key);
            let again = match scope {
                Scope::All => true,
                Scope::Only(keys) => keys.contains(&key),
            };
            let kept = was
                .filter(|_| holds(&check.repo) && !again)
                .and_then(|c| Some((c.outcome.clone()?, c.run)));
            CheckRun {
                repo: check.repo.clone(),
                command: check.command.clone(),
                dir: check.dir.clone(),
                pane: was.and_then(|c| c.pane.clone()),
                run: kept.as_ref().map_or(n, |(_, run)| *run),
                outcome: kept.map(|(outcome, _)| outcome),
                card_off: false,
                last_secs: was.and_then(CheckRun::took_last),
                queued: false,
                began: None,
                stopping: false,
            }
        })
        .collect()
}

/// The folder a run's scripts and what they leave go in, under the feature
/// data dir.
pub fn run_dir(data: &Path, n: u32) -> PathBuf {
    data.join("checks").join(n.to_string())
}

/// Remove what runs before `n` left. A script of one still running writes
/// nowhere, and is noticed by nobody.
pub fn prune_runs(data: &Path, n: u32) {
    let Ok(entries) = std::fs::read_dir(data.join("checks")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != n.to_string().as_str()
            && let Err(err) = std::fs::remove_dir_all(entry.path())
        {
            tracing::warn!("removing {}: {err}", entry.path().display());
        }
    }
}

/// Where check `i`'s script is, in its run's folder. The name tells it
/// from what the user runs (see `runs_check`).
pub fn script_path(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("{SCRIPT}{i}"))
}

pub(crate) const SCRIPT: &str = "gerdr-check-";

/// The variable a check's script exports to all it starts, set to
/// `marker_of` its check: how its process group is told from another that
/// took its pid since (see `group_ours`).
pub(crate) const MARKER: &str = "GERDR_CHECK";

/// What `MARKER` is set to for check `i` of the run in `dir`.
fn marker_value(dir: &Path, i: usize) -> String {
    format!("{}/{i}", dir.to_string_lossy())
}

/// `MARKER` of check `i` of the run in `dir`, as it reads in a process's
/// environment.
pub(crate) fn marker_of(dir: &Path, i: usize) -> String {
    format!("{MARKER}={}", marker_value(dir, i))
}

/// Whether a command line is a check's script: what it runs ends by itself,
/// and its card tells how it went.
pub fn runs_check(command: &str) -> bool {
    command.contains(SCRIPT)
}

/// The start of the line a check's script prints before it.
pub(crate) const HEAD: &str = "\u{25b6} Check ";
/// The start of the line it prints after it.
const END: &str = "\u{25a0} Check ";

/// What a check's terminal says while another check has its folder.
const QUEUED: &str = "Waiting for another check in this folder to end";
/// What it says when nothing can make it wait.
const NO_FLOCK: &str =
    "flock isn't installed, so this check doesn't wait for others in this folder";

/// The script of check `i` of `check`, which leaves its pid and then its exit
/// code in `dir`, each file whole or not at all. It outlives an interrupt,
/// as what it runs may take time to stop and its exit code is to be told.
/// It clears the screen first: the line that started it is long, and the
/// terminal may be narrow.
///
/// Checks of one folder take turns, by a lock on the folder, as they share
/// what they write there: two `nx` starting together in a new worktree crash
/// on the file they both copy. The script holds the lock itself, not what it
/// runs: a daemon left behind doesn't keep it, and the kernel drops it however
/// the script ends. It leaves `queued` when it has to wait, and `start` when
/// the check starts; its time counts from there.
pub fn script(dir: &Path, i: usize, check: &CheckRun, slug: &str) -> String {
    script_with(dir, i, check, slug, "")
}

/// `script`, with `prelude` run right before the check: lines of `sh`, for what
/// a terminal on another machine doesn't get by itself.
pub fn script_with(dir: &Path, i: usize, check: &CheckRun, slug: &str, prelude: &str) -> String {
    let path = |ext: &str| dir.join(format!("{i}.{ext}"));
    let quoted = |p: PathBuf| shell_quote(&p.to_string_lossy());
    let leave = |what: &str, ext: &str| {
        let (new, file) = (quoted(path(&format!("{ext}.new"))), quoted(path(ext)));
        format!("echo \"{what}\" > {new} && mv {new} {file}\n")
    };
    let first = check.command.lines().next().unwrap_or_default();
    let more = if check.command.lines().nth(1).is_some() {
        " \u{2026}"
    } else {
        ""
    };
    let head = format!("{HEAD}{}: {first}{more}", check.repo);
    // Past a wait the screen starts over, so the output starts at the check.
    // A lock that fails for any reason but a signal doesn't fail the check.
    format!(
        "trap : INT TERM\n{pid}printf '\\033[H\\033[2J%s\\n' {head}\nGERDR_FEATURE={slug}\nexport GERDR_FEATURE\n\
         {MARKER}={marker}\nexport {MARKER}\n\
         if cd {cwd}; then\n  code=0\n  if [ -r . ] && command -v flock >/dev/null 2>&1; then\n    exec 9<.\n    \
         if ! flock -n 9; then\n      {queued}      printf '%s\\n' {waiting}\n      flock 9\n      code=$?\n      \
         if [ \"$code\" -le 128 ]; then\n        code=0\n        printf '\\033[H\\033[2J%s\\n' {head}\n      fi\n    fi\n  \
         else\n    printf '%s\\n' {no_flock}\n  fi\n  if [ \"$code\" -eq 0 ]; then\n    {start}    {prelude}\
         sh -c {command} </dev/null 9<&-\n    code=$?\n  fi\nelse\n  code=127\nfi\n\
         if [ \"$code\" -eq 0 ]; then\n  printf '%s\\n' {passed}\nelse\n  printf '%s\\n' \"{end}exited with $code\"\nfi\n{exit}",
        pid = leave("$$", "pid"),
        marker = shell_quote(&marker_value(dir, i)),
        queued = leave("$$", "queued"),
        waiting = shell_quote(QUEUED),
        no_flock = shell_quote(NO_FLOCK),
        start = leave("$$", "start"),
        head = shell_quote(&head),
        slug = shell_quote(slug),
        cwd = shell_quote(&check.dir.to_string_lossy()),
        command = shell_quote(&check.command),
        passed = shell_quote(&format!("{END}passed")),
        end = END,
        exit = leave("$code", "exit"),
    )
}

/// The checks' terminals, as `watch` needs them.
pub trait Panes {
    /// Whether pane `pane` is gone. A server too busy to answer doesn't say
    /// that: its terminals go on.
    fn gone(&mut self, pane: &str) -> bool;
    /// What pane `pane` shows, its last lines, those it wrapped joined.
    fn text(&mut self, pane: &str) -> Result<String, Unread>;
    /// Press Ctrl+C in pane `pane`.
    fn interrupt(&mut self, pane: &str);
    /// All pane `pane` keeps of what it printed, oldest first, for the AI
    /// work; none while that isn't kept.
    fn keep_log(&mut self, _pane: &str) -> Option<String> {
        None
    }
}

/// The files a run's checks leave, and the processes they name: on this
/// machine, or on another one with its own pids and clock.
pub trait Files {
    /// Look at them again. False: nothing can be told now, so nothing is
    /// decided from them.
    fn look(&mut self) -> bool;
    /// The number check `i` left in its file `ext`, once it is there whole.
    fn number(&self, i: usize, ext: &str) -> Option<i32>;
    /// When check `i` wrote its file `ext`, if it did.
    fn written(&self, i: usize, ext: &str) -> Option<SystemTime>;
    /// How long ago `at` was, by the clock of where the check runs.
    fn age(&self, at: SystemTime) -> Duration;
    /// Send `signal` to process group `group`, from check `i`'s pid file.
    fn kill_group(&mut self, i: usize, group: i32, signal: i32);
    /// Send `signal` to the process group `pid` is in, from check `i`'s pid
    /// file.
    fn kill_group_of(&mut self, i: usize, pid: i32, signal: i32);
    /// Whether `pid`, from check `i`'s pid file, may still be its script's,
    /// so its group can be signalled: a file left from before a reboot or a
    /// crash names whatever took the pid since.
    fn still_check(&self, _i: usize, _pid: i32) -> bool {
        true
    }
    /// How often to look.
    fn every(&self) -> Duration;
    /// How a check told to stop reads when it can't be reached to stop it.
    fn out_of_reach(&self) -> String;
}

/// A run's files on this machine, in its folder.
pub struct LocalFiles<'a>(pub &'a Path);

impl LocalFiles<'_> {
    fn path(&self, i: usize, ext: &str) -> PathBuf {
        self.0.join(format!("{i}.{ext}"))
    }
}

impl Files for LocalFiles<'_> {
    fn look(&mut self) -> bool {
        true
    }

    fn number(&self, i: usize, ext: &str) -> Option<i32> {
        read_number(&self.path(i, ext))
    }

    fn written(&self, i: usize, ext: &str) -> Option<SystemTime> {
        modified(&self.path(i, ext))
    }

    fn age(&self, at: SystemTime) -> Duration {
        at.elapsed().unwrap_or_default()
    }

    fn kill_group(&mut self, _i: usize, group: i32, signal: i32) {
        kill_group(group, signal);
    }

    fn kill_group_of(&mut self, _i: usize, pid: i32, signal: i32) {
        // SAFETY: getpgid takes no pointers; a pid gone gives -1, which
        // `kill_group` leaves alone.
        kill_group(unsafe { libc::getpgid(pid) }, signal);
    }

    fn still_check(&self, i: usize, pid: i32) -> bool {
        // A pid file from before this boot names nothing of it.
        let fresh = (self.written(i, "pid").zip(booted())).is_some_and(|(at, booted)| at >= booted);
        fresh && group_ours(pid, &members(pid), &marker_of(self.0, i))
    }

    fn every(&self) -> Duration {
        WATCH_POLL
    }

    fn out_of_reach(&self) -> String {
        "stopped".to_owned()
    }
}

/// A process of a process group, as `/proc` showed it: what it holds is
/// `None` when it couldn't be read.
struct Member {
    pid: i32,
    zombie: bool,
    environ: Option<Vec<u8>>,
    cmdline: Option<Vec<u8>>,
}

/// Whether process group `group` is the one of the check whose `marker_of`
/// is `marker`: a process of it alive is the check's, with the marker in its
/// environment, or the leader running a check's script (which exports the
/// marker after it starts). One is enough: a pid is never handed out again
/// while a group of that id lives, so a group holding the check's process
/// is the check's, its other members too (a server that cleared its
/// environment). A zombie is passed over, as a signal does nothing to it.
fn group_ours(group: i32, members: &[Member], marker: &str) -> bool {
    (members.iter().filter(|m| !m.zombie)).any(|m| {
        let marked = (m.environ.as_deref())
            .is_some_and(|env| env.split(|&b| b == 0).any(|var| var == marker.as_bytes()));
        let script = m.pid == group
            && (m.cmdline.as_deref()).is_some_and(|c| runs_check(&String::from_utf8_lossy(c)));
        marked || script
    })
}

/// A process's state and process group, from its `/proc/<pid>/stat`.
fn state_and_group(stat: &str) -> Option<(char, i32)> {
    // Its name, in parentheses, may hold anything: the state, parent and
    // group come right after the last.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let state = fields.next()?.chars().next()?;
    Some((state, fields.nth(1)?.parse().ok()?))
}

/// The processes of process group `group` now; one gone while it is read
/// is left out.
fn members(group: i32) -> Vec<Member> {
    let entries = std::fs::read_dir("/proc").into_iter().flatten().flatten();
    let pids = entries.filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok());
    pids.filter_map(|pid| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (state, of) = state_and_group(&stat)?;
        if of != group {
            return None;
        }
        let read = |what: &str| std::fs::read(format!("/proc/{pid}/{what}")).ok();
        let (environ, cmdline) = (read("environ"), read("cmdline"));
        if environ.is_none() && !Path::new(&format!("/proc/{pid}")).exists() {
            return None;
        }
        Some(Member {
            pid,
            zombie: state == 'Z',
            environ,
            cmdline,
        })
    })
    .collect()
}

/// When this machine booted, to the second.
fn booted() -> Option<SystemTime> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let secs = stat.lines().find_map(|l| l.strip_prefix("btime "))?;
    Some(UNIX_EPOCH + Duration::from_secs(secs.trim().parse().ok()?))
}

/// Why a terminal's text wasn't read, and what the error said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unread {
    /// The terminal is gone, and its text with it.
    Gone(String),
    /// The server didn't answer in time: it may yet.
    Busy(String),
}

/// A check `watch` waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waited {
    /// Its index in the run's rows, and of its files.
    pub index: usize,
    pub repo: String,
    pub command: String,
    pub pane: String,
}

/// How long a shell may take to start its check, from the time it was told
/// to. `prepare` gives up on a shell long before this.
const START_WAIT: Duration = Duration::from_secs(30);
/// How long a check has to end after Ctrl+C, before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// How long the files of a killed check are waited for.
const KILL_GRACE: Duration = Duration::from_secs(1);
/// How long a check told to stop waits for a machine out of reach to come
/// back, before it ends here and goes on there.
const OUT_OF_REACH_STOP: Duration = if cfg!(test) {
    Duration::from_millis(30)
} else {
    Duration::from_secs(15)
};
/// How often the files are looked at.
const WATCH_POLL: Duration = Duration::from_millis(200);
/// How often the panes are asked about.
const PANE_POLL: Duration = Duration::from_secs(1);
/// What 128 + SIGINT means: Ctrl+C, the user's or `watch`'s.
const INTERRUPTED: i32 = 130;

/// Why `watch` cut a check short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    Stopped,
    TimedOut,
}

/// A check being waited for.
struct Waiting {
    check: Waited,
    /// When it was cut short, and why.
    cut: Option<(Instant, Cut)>,
    killed: bool,
    /// What has been told of its turn.
    queued: bool,
    began: bool,
    /// Since when it was told to stop with nobody to tell it to.
    unheard: Option<Instant>,
}

/// Where a check stands in its folder's queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Another check has its folder: it waits.
    Queued,
    /// It started, at these unix seconds.
    Began(u64),
}

/// Wait for the checks `waited` of the run in `dir`, each for at most
/// `timeout` from when it starts; `turn` hears when one has to wait and when
/// it starts, `ended` of each as it ends. `stops[index]` cuts one short, and is
/// looked at all along.
pub fn watch(
    waited: Vec<Waited>,
    dir: &Path,
    timeout: Duration,
    stops: &[AtomicBool],
    panes: &mut dyn Panes,
    turn: &mut dyn FnMut(usize, Turn),
    ended: &mut dyn FnMut(usize, Outcome),
) {
    let mut files = LocalFiles(dir);
    watch_files(waited, &mut files, timeout, stops, panes, turn, ended);
}

/// `watch`, over `files` wherever they are. What can't be looked at decides
/// nothing: not that a check started, passed or ran out of time. Only one told
/// to stop ends then, after a while, as nothing else would end it if the
/// machine never comes back.
pub fn watch_files(
    waited: Vec<Waited>,
    files: &mut dyn Files,
    timeout: Duration,
    stops: &[AtomicBool],
    panes: &mut dyn Panes,
    turn: &mut dyn FnMut(usize, Turn),
    ended: &mut dyn FnMut(usize, Outcome),
) {
    let mut told = Instant::now();
    let mut asked = Instant::now();
    let mut waiting: Vec<Waiting> = waited
        .into_iter()
        .map(|check| Waiting {
            check,
            cut: None,
            killed: false,
            queued: false,
            began: false,
            unheard: None,
        })
        .collect();
    let mut was_out = false;
    while !waiting.is_empty() {
        if !files.look() {
            let word = files.out_of_reach();
            waiting.retain_mut(|w| {
                let i = w.check.index;
                let stop = stops.get(i).is_some_and(|s| s.load(Ordering::Relaxed));
                if !stop {
                    return true;
                }
                let since = *w.unheard.get_or_insert_with(Instant::now);
                if since.elapsed() < OUT_OF_REACH_STOP {
                    return true;
                }
                let outcome = Outcome {
                    repo: w.check.repo.clone(),
                    command: w.check.command.clone(),
                    error: Some(word.clone()),
                    interrupted: true,
                    ..Outcome::default()
                };
                ended(i, outcome);
                false
            });
            was_out = true;
            std::thread::sleep(files.every());
            continue;
        }
        // Back: a shell gets its time from now, as nobody knows what it had.
        if std::mem::take(&mut was_out) {
            told = Instant::now();
            for w in &mut waiting {
                w.unheard = None;
            }
        }
        let ask = asked.elapsed() >= PANE_POLL;
        if ask {
            asked = Instant::now();
        }
        waiting.retain_mut(|w| {
            let i = w.check.index;
            let pid = files.number(i, "pid");
            let began = files.written(i, "start");
            if let (false, Some(at)) = (w.began, began) {
                w.began = true;
                let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                turn(i, Turn::Began(secs));
            } else if !w.began && !w.queued && files.written(i, "queued").is_some() {
                w.queued = true;
                turn(i, Turn::Queued);
            }
            let outcome =
                |exit: Option<i32>, error: Option<&str>, interrupted: bool, secs| Outcome {
                    repo: w.check.repo.clone(),
                    command: w.check.command.clone(),
                    exit,
                    error: error.map(str::to_owned),
                    interrupted,
                    secs,
                    ..Outcome::default()
                };
            if let Some(code) = files.number(i, "exit") {
                let from = began.or_else(|| files.written(i, "pid"));
                let secs = took(from, files.written(i, "exit"));
                let mut o = match (w.cut.map(|(_, cut)| cut), code) {
                    (Some(Cut::TimedOut), _) => outcome(None, None, false, timeout.as_secs()),
                    (Some(Cut::Stopped), _) | (None, INTERRUPTED) => {
                        outcome(None, Some("stopped"), true, secs)
                    }
                    _ => outcome(Some(code), None, false, secs),
                };
                o.timed_out = w.cut.is_some_and(|(_, cut)| cut == Cut::TimedOut);
                o.keep_output(panes, &w.check.pane);
                // What it left running in the background goes with it, as
                // nothing else would stop it: a server would hold its port.
                if let Some(pid) = pid.filter(|&p| files.still_check(i, p)) {
                    files.kill_group(i, pid, libc::SIGTERM);
                }
                ended(i, o);
                return false;
            }
            if ask && panes.gone(&w.check.pane) {
                if let Some(pid) = pid.filter(|&p| files.still_check(i, p)) {
                    files.kill_group(i, pid, libc::SIGTERM);
                }
                ended(i, outcome(None, Some("its terminal was closed"), true, 0));
                return false;
            }
            if pid.is_none() && told.elapsed() >= START_WAIT {
                let why = "its terminal's shell didn't start it";
                ended(i, outcome(None, Some(why), true, 0));
                return false;
            }
            let Some(pid) = pid else {
                return true;
            };
            match w.cut {
                None => {
                    let stop = stops.get(i).is_some_and(|s| s.load(Ordering::Relaxed));
                    // A script from before queues had no `start`; one in the
                    // queue hasn't started, so its time hasn't begun.
                    let past = match began {
                        Some(at) => files.age(at) >= timeout,
                        None => {
                            !w.queued
                                && files
                                    .written(i, "pid")
                                    .is_some_and(|at| files.age(at) >= timeout)
                        }
                    };
                    if stop || past {
                        let cut = if stop { Cut::Stopped } else { Cut::TimedOut };
                        w.cut = Some((Instant::now(), cut));
                        panes.interrupt(&w.check.pane);
                    }
                }
                Some((at, _)) if !w.killed && at.elapsed() >= STOP_GRACE => {
                    w.killed = true;
                    if files.still_check(i, pid) {
                        files.kill_group_of(i, pid, libc::SIGKILL);
                    }
                }
                Some((at, cut))
                    if w.killed && at.elapsed() >= STOP_GRACE + KILL_GRACE + files.every() =>
                {
                    let mut o = match cut {
                        Cut::Stopped => outcome(None, Some("stopped"), true, 0),
                        Cut::TimedOut => outcome(None, None, false, timeout.as_secs()),
                    };
                    o.timed_out = cut == Cut::TimedOut;
                    o.keep_output(panes, &w.check.pane);
                    ended(i, o);
                    return false;
                }
                Some(_) => {}
            }
            true
        });
        if !waiting.is_empty() {
            std::thread::sleep(files.every());
        }
    }
}

/// The number in `path`, once it is there whole.
fn read_number(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// How many seconds lie between two writes.
fn took(from: Option<SystemTime>, to: Option<SystemTime>) -> u64 {
    from.zip(to)
        .and_then(|(from, to)| to.duration_since(from).ok())
        .map_or(0, |d| d.as_secs())
}

/// How many lines a terminal is asked for: plenty for `TAIL`.
pub const READ_LINES: usize = 1000;
/// How many times a terminal is read for a check's last line, which it may
/// not have drawn yet.
const READ_TRIES: usize = 20;
/// How long a server that doesn't answer is asked for the text of a check that
/// ended: a machine under load stalls it for a minute or more, and the text is
/// all that says what the check found.
const READ_PATIENCE: Duration = Duration::from_secs(60);

impl Outcome {
    /// Keep the last lines of what the check in `pane` printed, and when the
    /// code failed it the lines that say why.
    fn keep_output(&mut self, panes: &mut dyn Panes, pane: &str) {
        let mut text = String::new();
        let from = Instant::now();
        let mut tries = 0;
        loop {
            match panes.text(pane) {
                Ok(read) => {
                    text = read;
                    tries += 1;
                    if ended(&text) || tries >= READ_TRIES {
                        break;
                    }
                }
                Err(Unread::Busy(why)) if from.elapsed() >= READ_PATIENCE => {
                    tracing::warn!("reading the check's pane {pane}, gave up: {why}");
                    break;
                }
                Err(Unread::Busy(_)) => {}
                Err(Unread::Gone(why)) => {
                    tracing::warn!("reading the check's pane {pane}: {why}");
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let output = output_of(&text);
        self.tail = tail(output.as_bytes(), false);
        if !self.passed() && !self.interrupted {
            self.excerpt = failure_lines(output);
        }
        self.log = panes.keep_log(pane);
    }
}

/// How many lines a terminal is asked for to preview it.
pub const PREVIEW_READ: usize = 200;

/// Whether the last check in a terminal showing `text` has ended there.
fn ended(text: &str) -> bool {
    let head = text.rfind(HEAD);
    text.rfind(END).is_some_and(|end| Some(end) > head)
}

/// What the card of a check running in a terminal showing `text` previews:
/// the last lines it printed. Nothing while the terminal still shows what it
/// did before the check started there. Once it `began`, output too long for
/// one read has lost its start line, but isn't that.
pub fn preview(text: &str, began: bool) -> Vec<String> {
    if ended(text) || (!began && !text.contains(HEAD)) {
        return Vec::new();
    }
    last_lines(&tail(output_of(text).as_bytes(), false))
}

/// The last `PREVIEW_LINES` lines of `text` with something on them.
pub fn last_lines(text: &str) -> Vec<String> {
    (crate::text::last_lines(text, PREVIEW_LINES).into_iter())
        .map(untabbed)
        .collect()
}

/// `line` with spaces for its tabs: a card draws them as nothing.
fn untabbed(line: &str) -> String {
    line.replace('\t', "    ")
}

/// What says outright that something went wrong.
const STRONG: [&str; 5] = ["error", "panicked", "traceback", "exception", "fatal"];
/// What says so too, but also names lists of tests and their summaries.
const WEAK: [&str; 3] = ["failed", "failure", "fail"];

/// The lines of output `raw` that say why a check failed: the first that
/// says so outright, else the first that says it failed, and what follows it,
/// `PREVIEW_LINES` in all. Nothing when no line does: its last lines then say
/// the most. A test run's last lines are its summary, not what broke.
fn failure_lines(raw: &str) -> Vec<String> {
    let lines: Vec<String> = clean_lines(raw.as_bytes(), false)
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let first = |words: &[&str]| lines.iter().position(|line| says_any(line, words));
    let Some(at) = first(&STRONG).or_else(|| first(&WEAK)) else {
        return Vec::new();
    };
    (lines[traceback_end(&lines, at)..].iter())
        // Python's `~~~^^^` marks under the code say nothing on a card.
        .filter(|line| !line.trim().chars().all(|c| matches!(c, '~' | '^')))
        .take(PREVIEW_LINES)
        .map(|line| untabbed(line))
        .collect()
}

/// Where the excerpt of a failure saying so at `lines[at]` starts. A Python
/// traceback's own lines (`Traceback (most recent call last):`, then
/// indented frames) say what was running; its first line not indented is
/// the error. That goes with the frame it came from, the last `File` line.
fn traceback_end(lines: &[String], at: usize) -> usize {
    if !lines[at]
        .trim_start()
        .starts_with("Traceback (most recent call last)")
    {
        return at;
    }
    let Some(error) = (at + 1..lines.len()).find(|&i| !lines[i].starts_with(char::is_whitespace))
    else {
        return at;
    };
    (at + 1..error)
        .rev()
        .find(|&i| lines[i].trim_start().starts_with("File "))
        .unwrap_or(error)
}

/// Whether `line` has one of `words` (in lower case) as a word of its own,
/// in any case, and not to say there are none of it. The end of a name like
/// `TypeError` counts; that of a path like `src/error.rs` or a test like
/// `no_error_here` doesn't.
fn says_any(line: &str, words: &[&str]) -> bool {
    // ASCII only, so every character stays at its place in `line`.
    let lower = line.to_ascii_lowercase();
    let joined = |c: char| c == '_' || c == '-' || c == '/' || c == '.';
    words.iter().any(|word| {
        lower.match_indices(word).any(|(at, _)| {
            let before = line[..at].chars().next_back();
            let start = match before {
                None => true,
                Some(c) if joined(c) => false,
                Some(c) if c.is_alphanumeric() => line[at..].starts_with(char::is_uppercase),
                Some(_) => true,
            };
            let rest = &line[at + word.len()..];
            let rest = rest.strip_prefix(['s', 'S']).unwrap_or(rest);
            let end = match rest.chars().next() {
                None => true,
                Some('.') => !rest[1..].starts_with(char::is_alphanumeric),
                Some(c) => !c.is_alphanumeric() && c != '_' && c != '-' && c != '/',
            };
            start && end && !says_none(&line[..at], rest)
        })
    })
}

/// Whether a word with `before` and `after` round it says there is none of
/// it: "0 failed", "no errors", "Errors: 0".
fn says_none(before: &str, after: &str) -> bool {
    let last = before.split_whitespace().next_back().unwrap_or_default();
    let count = after.trim_start_matches([':', '=', ' ']);
    let zero =
        count.starts_with('0') && !count[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.');
    zero || last == "0" || last.eq_ignore_ascii_case("no")
}

/// Of what a terminal shows, what its last check printed: from after the
/// line that starts it, up to the line that ends it.
fn output_of(text: &str) -> &str {
    let from = text.rfind(HEAD).map_or(0, |at| {
        text[at..].find('\n').map_or(text.len(), |nl| at + nl + 1)
    });
    let rest = &text[from..];
    &rest[..rest.rfind(END).unwrap_or(rest.len())]
}

/// How long a line of output the evidence keeps, in characters.
const LINE: usize = 500;

/// The last `TAIL` lines of output `raw`, made readable (see `clean_lines`).
fn tail(raw: &[u8], cut: bool) -> String {
    let lines = clean_lines(raw, cut);
    let from = lines.len().saturating_sub(TAIL);
    lines[from..].join("\n")
}

/// The lines of output `raw`, made readable: colour and other terminal
/// escapes off, of a line redrawn with `\r` only its last drawing, long lines
/// cut. `cut` says `raw` starts mid-output, so its first line is partial and
/// goes.
pub(super) fn clean_lines(raw: &[u8], cut: bool) -> Vec<String> {
    let text = strip_escapes(&String::from_utf8_lossy(raw));
    let mut lines: Vec<&str> = text.split('\n').collect();
    if cut && lines.len() > 1 {
        lines.remove(0);
    }
    let mut lines: Vec<String> = lines
        .into_iter()
        .map(|line| {
            let line = line
                .split('\r')
                .rfind(|part| !part.trim().is_empty())
                .unwrap_or("")
                .trim_end();
            if line.chars().count() > LINE {
                let mut short: String = line.chars().take(LINE).collect();
                short.push('…');
                short
            } else {
                line.to_owned()
            }
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// `text` without terminal escapes (CSI such as colours, OSC such as
/// titles and links, two-character ones) or other control characters
/// than newline, carriage return and tab.
pub(crate) fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\r' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `evidence.md`'s text for `evidence` of `feature`.
pub fn markdown(feature: &Feature, evidence: &Evidence) -> String {
    let mut md = format!("# Verify: {}\n\n", feature.title);
    md.push_str(&format!("- When: {}\n", utc(evidence.at)));
    md.push_str(&format!(
        "- Result: **{}**\n",
        if evidence.passed() {
            "PASSED"
        } else {
            "FAILED"
        }
    ));
    if evidence.patch.is_empty() {
        md.push_str("- Patch-id: unknown\n");
    } else {
        md.push_str(&format!("- Patch-id: `{}`\n", evidence.patch));
    }
    md.push_str(&format!("- Branch: `{}`\n", feature.branch()));
    if let Some(place) = &evidence.ran_on {
        md.push_str(&format!("- Ran on: {place}\n"));
    }
    if !evidence.heads.is_empty() {
        md.push_str("\n## Commits checked\n\n| Repository | HEAD |\n| --- | --- |\n");
        for (repo, head) in &evidence.heads {
            md.push_str(&format!("| {repo} | `{head}` |\n"));
        }
    }
    let failures = evidence.failures();
    if !failures.is_empty() {
        md.push_str("\n## What failed\n\n");
        for line in &failures {
            md.push_str(&format!("- {line}\n"));
        }
    }
    md.push_str("\n## Checks\n");
    if evidence.outcomes.is_empty() {
        md.push_str(if evidence.problems.is_empty() {
            "\nNo checks are set for this feature.\n"
        } else {
            "\nNone ran.\n"
        });
    }
    // Where nothing ran, nothing was left out of it.
    let skipped = feature.verify.as_ref().map_or(&[][..], |v| &v.skipped);
    if !skipped.is_empty() && !evidence.outcomes.is_empty() {
        md.push_str("\nSkipped for this feature:\n\n");
        for line in skipped {
            md.push_str(&format!("- {line}\n"));
        }
    }
    for o in &evidence.outcomes {
        md.push_str(&format!("\n### {}: {}\n\n", o.repo, o.command));
        let how = if o.passed() {
            "Passed".to_owned()
        } else {
            let how = o.how();
            let mut first = how.chars();
            first
                .next()
                .map(|c| c.to_uppercase().chain(first).collect())
                .unwrap_or(how)
        };
        // An earlier run's: its code didn't change since.
        let earlier = feature.verify.as_ref().and_then(|v| {
            let row = (v.checks.iter()).find(|c| c.repo == o.repo && c.command == o.command);
            row.filter(|c| c.run != v.n).map(|c| c.run)
        });
        match earlier {
            Some(run) => md.push_str(&format!(
                "{how}, in {} s, in run {run}: its code didn't change since.\n",
                o.secs
            )),
            None => md.push_str(&format!("{how}, in {} s.\n", o.secs)),
        }
        if o.tail.is_empty() {
            md.push_str("\nNo output.\n");
        } else {
            let fence = "`".repeat(longest_backticks(&o.tail).max(2) + 1);
            md.push_str(&format!(
                "\nLast lines of its output:\n\n{fence}text\n{}\n{fence}\n",
                o.tail
            ));
        }
    }
    md
}

/// The longest run of backticks in `text`.
fn longest_backticks(text: &str) -> usize {
    let (mut longest, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// Unix seconds `secs` as a UTC date and time: "2026-11-02 14:05 UTC".
pub(super) fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Days to a civil date, after Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest / 60 % 60
    )
}

/// A feature's latest Verify run, kept in its record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyRun {
    /// Which run: 1, 2…
    pub n: u32,
    /// Unix seconds.
    pub started: u64,
    /// When it ended (unix seconds); `None` while it runs, or when gerdr
    /// quit before it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<u64>,
    /// The patch-id it ran on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Whether it passed, once finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    /// What failed (`Evidence::failures`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// Its checks, each in its terminal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRun>,
    /// The borrowed checks the feature skipped, a line each, for the
    /// evidence. On the record, as a gerdr started later ends the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Each touched repository's `HEAD` commit it ran on, by `Repo::key`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heads: Vec<(String, String)>,
    /// Its terminals were told to run the checks: they go on without gerdr,
    /// and a gerdr started later waits for them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
    /// The machine it runs on, by id; `None`: this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl VerifyRun {
    /// The keys of its checks that didn't pass.
    pub fn not_passed(&self) -> Vec<(String, String)> {
        (self.checks.iter())
            .filter(|c| !c.outcome.as_ref().is_some_and(Outcome::passed))
            .map(CheckRun::key)
            .collect()
    }

    /// Whether what it said of `repo`'s checks (a `Repo::key` or
    /// `FEATURE_KEY`) goes for the code `begun` read: the whole feature's
    /// change is the same, or that repository's `HEAD` is. Each check it
    /// keeps ran on the `HEAD` it lists, as one that moved keeps none.
    pub fn holds(&self, begun: &Begun, repo: &str) -> bool {
        if !begun.patch.is_empty() && self.patch.as_deref() == Some(begun.patch.as_str()) {
            return true;
        }
        let head = |heads: &'_ [(String, String)]| {
            let found = heads.iter().find(|(key, _)| key == repo);
            found.map(|(_, head)| head.clone())
        };
        repo != FEATURE_KEY && head(&self.heads).is_some_and(|h| head(&begun.heads) == Some(h))
    }
}

/// One check of a feature's Verify run, as its record keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    /// The repository's `Repo::key`, or `FEATURE_KEY`.
    pub repo: String,
    pub command: String,
    /// Where it runs.
    pub dir: PathBuf,
    /// The herdr pane it ran in last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// The run that ran it: an earlier one for a check not run again.
    pub run: u32,
    /// How it went, once it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// The user took its card off the sidebar.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub card_off: bool,
    /// How many seconds it took the last time it ran to its end, in a run
    /// before the one that has it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_secs: Option<u64>,
    /// It waits for another check in its folder.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// When it started (unix seconds), past any wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub began: Option<u64>,
    /// The user stopped it and it hasn't ended yet. Not saved: what would
    /// stop it goes with the gerdr that was told, so a new one offers Stop.
    #[serde(skip)]
    pub stopping: bool,
}

impl CheckRun {
    /// How many seconds its next run may be told it took.
    fn took_last(&self) -> Option<u64> {
        let ran = (self.outcome.as_ref())
            .filter(|o| !o.interrupted && !o.timed_out)
            .map(|o| o.secs);
        ran.filter(|secs| *secs > 0).or(self.last_secs)
    }

    /// What tells it from a feature's other checks.
    pub fn key(&self) -> (String, String) {
        (self.repo.clone(), self.command.clone())
    }

    /// Its repository as the user knows it.
    pub fn place(&self) -> &str {
        place_of(&self.repo)
    }
}

/// The fix plan gerdr writes itself for Verify's `failures` of feature
/// `title`, `evidence` the file with their output: nothing of the user's
/// goes with them, so no plan is asked for, and each failure is one thing to
/// tick (`fix_round::items`).
pub fn fixes_plan(failures: &[String], title: &str, evidence: &Path) -> String {
    let mut md = super::fix_round::header(
        title,
        &format!(
            "Verify's failures: the user wrote nothing of their own, so no plan was asked for. Find each one's cause in the checks' output, in {}, before you change code.",
            evidence.display()
        ),
    );
    for failure in failures {
        // A line of its own: the heading is what is ticked.
        let first = failure.lines().next().unwrap_or_default();
        let one = first.trim_start_matches('#').trim();
        md.push_str(&format!("\n### {one}\n"));
    }
    md
}

/// What failed in `run` for the Build agent to fix: none while it runs, if
/// it passed, or if a check was cut short, which says nothing of the code.
pub fn to_fix(run: &VerifyRun) -> Option<Vec<String>> {
    let cut = (run.checks.iter()).any(|c| c.outcome.as_ref().is_some_and(|o| o.interrupted));
    if run.passed != Some(false) || run.finished.is_none() || cut {
        return None;
    }
    Some(match run.failures.is_empty() {
        true => vec![format!("see {EVIDENCE}")],
        false => run.failures.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        os::unix::process::CommandExt,
        path::Path,
        process::{Command, Stdio},
        sync::atomic::AtomicUsize,
    };

    use super::*;
    use crate::feature::tests::{Temp, commit, feature_over, link_over, run as git_run};

    /// Files as a look finds them: name, when written, the number in them.
    type Seen = Vec<(&'static str, u64, i32)>;

    /// A run's files on a machine: `script` says, look by look, what can be
    /// told then; past its end, the last goes on.
    struct FarFiles {
        script: Vec<Option<Seen>>,
        now: Vec<u64>,
        look: usize,
        kills: Vec<(i32, i32)>,
    }

    impl FarFiles {
        fn new(script: Vec<Option<Seen>>, now: Vec<u64>) -> Self {
            Self {
                script,
                now,
                look: 0,
                kills: Vec::new(),
            }
        }

        fn at(&self) -> usize {
            self.look.saturating_sub(1).min(self.script.len() - 1)
        }

        fn file(&self, i: usize, ext: &str) -> Option<(u64, i32)> {
            let name = format!("{i}.{ext}");
            let files = self.script[self.at()].as_ref()?;
            let file = files.iter().find(|(f, _, _)| *f == name)?;
            Some((file.1, file.2))
        }
    }

    impl Files for FarFiles {
        fn look(&mut self) -> bool {
            self.look += 1;
            self.script[self.at()].is_some()
        }

        fn number(&self, i: usize, ext: &str) -> Option<i32> {
            self.file(i, ext).map(|(_, n)| n)
        }

        fn written(&self, i: usize, ext: &str) -> Option<SystemTime> {
            let (at, _) = self.file(i, ext)?;
            Some(UNIX_EPOCH + Duration::from_secs(at))
        }

        fn age(&self, at: SystemTime) -> Duration {
            let now = self.now[self.at().min(self.now.len() - 1)];
            (UNIX_EPOCH + Duration::from_secs(now))
                .duration_since(at)
                .unwrap_or_default()
        }

        fn kill_group(&mut self, _i: usize, group: i32, signal: i32) {
            self.kills.push((group, signal));
        }

        fn kill_group_of(&mut self, _i: usize, pid: i32, signal: i32) {
            self.kills.push((pid, signal));
        }

        fn every(&self) -> Duration {
            Duration::from_millis(1)
        }

        fn out_of_reach(&self) -> String {
            "stopped here".to_owned()
        }
    }

    /// Terminals on a machine: they say nothing of themselves.
    #[derive(Default)]
    struct FarPanes {
        interrupted: Vec<String>,
    }

    impl Panes for FarPanes {
        fn gone(&mut self, _: &str) -> bool {
            false
        }

        fn text(&mut self, _: &str) -> Result<String, Unread> {
            Ok(format!("{HEAD}a: t\nsaid\n{END}passed\n"))
        }

        fn interrupt(&mut self, pane: &str) {
            self.interrupted.push(pane.to_owned());
        }
    }

    fn far_watch(files: &mut FarFiles, panes: &mut FarPanes, stop: bool) -> Vec<Outcome> {
        let waited = vec![Waited {
            index: 0,
            repo: "a".into(),
            command: "t".into(),
            pane: "p".into(),
        }];
        let stops = [AtomicBool::new(stop)];
        let mut outcomes = Vec::new();
        watch_files(
            waited,
            files,
            Duration::from_secs(100),
            &stops,
            panes,
            &mut |_, _| {},
            &mut |_, outcome| outcomes.push(outcome),
        );
        outcomes
    }

    #[test]
    fn a_plan_written_for_the_failures_has_each_as_a_fix() {
        use crate::feature::fix_round;
        let failures = [
            "api: `cargo test` exited with 101".to_owned(),
            "## web: `npm test` exited with 1\nmore".to_owned(),
        ];
        let plan = fixes_plan(&failures, "Export", Path::new("/d/evidence.md"));
        assert!(fix_round::missing(&plan, &[]).is_empty(), "{plan}");
        assert_eq!(
            fix_round::items(&plan, &[]),
            [
                "api: `cargo test` exited with 101",
                "web: `npm test` exited with 1"
            ]
        );
        assert!(plan.contains("/d/evidence.md"));
    }

    #[test]
    fn a_lost_link_decides_nothing_of_a_check() {
        // Past its time and with no word of it for a while: back, it passed.
        let mut files = FarFiles::new(
            vec![
                Some(vec![("0.pid", 10, 77), ("0.start", 10, 77)]),
                None,
                None,
                None,
                Some(vec![
                    ("0.pid", 10, 77),
                    ("0.start", 10, 77),
                    ("0.exit", 50, 0),
                ]),
            ],
            vec![20, 500, 500, 500, 500],
        );
        let mut panes = FarPanes::default();
        let outcomes = far_watch(&mut files, &mut panes, false);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].exit, Some(0));
        assert_eq!(outcomes[0].secs, 40);
        assert!(!outcomes[0].timed_out);
        assert!(panes.interrupted.is_empty());
    }

    #[test]
    fn time_there_cuts_a_check_short_once_it_can_be_told() {
        let running = vec![("0.pid", 10, 77), ("0.start", 10, 77)];
        let ended = vec![("0.pid", 10, 77), ("0.start", 10, 77), ("0.exit", 200, 130)];
        let mut files = FarFiles::new(vec![None, Some(running), Some(ended)], vec![0, 500, 500]);
        let mut panes = FarPanes::default();
        let outcomes = far_watch(&mut files, &mut panes, false);
        assert!(outcomes[0].timed_out);
        assert_eq!(panes.interrupted, ["p"]);
    }

    #[test]
    fn only_a_group_holding_its_check_is_signalled() {
        let marker = "GERDR_CHECK=/data/checks/3/0";
        let member = |pid: i32, environ: &[u8], cmdline: &[u8]| Member {
            pid,
            zombie: false,
            environ: Some(environ.to_vec()),
            cmdline: Some(cmdline.to_vec()),
        };
        let script = member(40, b"HOME=/h\0", b"sh\0/data/checks/3/gerdr-check-0\0");
        let marked = member(41, b"HOME=/h\0GERDR_CHECK=/data/checks/3/0\0", b"node\0");
        assert!(group_ours(40, &[script, marked], marker));
        // Its script ended: what it left is still the check's.
        let left = member(41, b"GERDR_CHECK=/data/checks/3/0\0", b"node\0");
        assert!(group_ours(40, &[left], marker));
        // Its pid taken by a leader that ended, its group left behind.
        let other = member(41, b"HOME=/h\0", b"vim\0");
        assert!(!group_ours(40, &[other], marker));
        // Another check's, or another run's.
        let near = member(41, b"GERDR_CHECK=/data/checks/3/01\0", b"node\0");
        assert!(!group_ours(40, &[near], marker));
        // A member that cleared its environment, beside one of the check's:
        // the group is still the check's.
        let cleared = member(42, b"", b"postgres: writer\0");
        let marked = member(41, b"GERDR_CHECK=/data/checks/3/0\0", b"node\0");
        assert!(group_ours(40, &[cleared, marked], marker));
        // A script's name on another than the leader isn't enough.
        let named = member(41, b"", b"sh\0/x/gerdr-check-0\0");
        assert!(!group_ours(40, &[named], marker));
        // One that can't be read, alone, isn't the check's.
        let unread = Member {
            environ: None,
            ..member(41, b"", b"")
        };
        assert!(!group_ours(40, &[unread], marker));
        // Nothing alive: nothing to signal.
        let zombie = Member {
            zombie: true,
            ..member(41, b"", b"")
        };
        assert!(!group_ours(40, &[zombie], marker));
        assert!(!group_ours(40, &[], marker));
    }

    #[test]
    fn a_process_state_and_group_are_read_after_its_name() {
        let stat = "123 (a ) b) (c) S 1 456 456 0 -1 4194560 9";
        assert_eq!(state_and_group(stat), Some(('S', 456)));
        assert_eq!(state_and_group("7 (z) Z 1 7 7"), Some(('Z', 7)));
        assert_eq!(state_and_group("garbage"), None);
    }

    #[test]
    fn a_checks_script_marks_all_it_starts() {
        let check = rows(&[check("a", Path::new("/r"), "make test")]).remove(0);
        let dir = Path::new("/data/checks/3");
        let script = script(dir, 0, &check, "slug");
        assert!(script.contains("\nGERDR_CHECK=/data/checks/3/0\nexport GERDR_CHECK\n"));
        assert_eq!(marker_of(dir, 0), "GERDR_CHECK=/data/checks/3/0");
    }

    #[test]
    fn a_check_stopped_out_of_reach_ends_here_and_kills_nothing() {
        let mut files = FarFiles::new(vec![None], vec![0]);
        let mut panes = FarPanes::default();
        let outcomes = far_watch(&mut files, &mut panes, true);
        assert_eq!(outcomes[0].error.as_deref(), Some("stopped here"));
        assert!(outcomes[0].interrupted);
        assert!(files.kills.is_empty());
    }

    #[test]
    fn what_a_check_left_there_is_killed_there() {
        let mut files = FarFiles::new(
            vec![Some(vec![
                ("0.pid", 10, 4_000_000),
                ("0.start", 10, 4_000_000),
                ("0.exit", 12, 1),
            ])],
            vec![20],
        );
        let mut panes = FarPanes::default();
        let outcomes = far_watch(&mut files, &mut panes, false);
        assert_eq!(outcomes[0].exit, Some(1));
        assert_eq!(files.kills, [(4_000_000, libc::SIGTERM)]);
    }

    /// Terminals made of plain processes, each with its output in a file.
    struct Direct {
        out: HashMap<String, PathBuf>,
        group: HashMap<String, i32>,
        closed: Vec<String>,
        children: Vec<std::process::Child>,
    }

    impl Drop for Direct {
        fn drop(&mut self) {
            for child in &mut self.children {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Panes for Direct {
        fn gone(&mut self, pane: &str) -> bool {
            self.closed.iter().any(|p| p == pane)
        }

        fn text(&mut self, pane: &str) -> Result<String, Unread> {
            let path = self.out.get(pane);
            (path.and_then(|path| std::fs::read_to_string(path).ok()))
                .ok_or(Unread::Gone("not there".into()))
        }

        fn interrupt(&mut self, pane: &str) {
            kill_group(self.group[pane], libc::SIGINT);
        }
    }

    /// The rows of a run of `checks`.
    fn rows(checks: &[Check]) -> Vec<CheckRun> {
        plan(checks, &[], &|_| false, &Scope::All, 1)
    }

    /// A folder for a run's files, outside of any repository.
    fn run_folder() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gerdr-checks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Named the same in a run before, as in a namespace where every run is
        // pid 1, it can hold what a script of that run wrote late.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Start the script of every check as a process, as a terminal would.
    fn start(feature: &Feature, checks: &[Check], dir: &Path) -> (Direct, Vec<Waited>) {
        let mut panes = Direct {
            out: HashMap::new(),
            group: HashMap::new(),
            closed: Vec::new(),
            children: Vec::new(),
        };
        let mut waited = Vec::new();
        for (index, row) in rows(checks).iter().enumerate() {
            let pane = format!("p{index}");
            let script = script_path(dir, index);
            std::fs::write(&script, super::script(dir, index, row, &feature.slug)).unwrap();
            let out = dir.join(format!("{index}.out"));
            let file = std::fs::File::create(&out).unwrap();
            let child = Command::new("sh")
                .arg(&script)
                .stdin(Stdio::null())
                .stdout(file.try_clone().unwrap())
                .stderr(file)
                .process_group(0)
                .spawn()
                .unwrap();
            panes.out.insert(pane.clone(), out);
            panes.group.insert(pane.clone(), child.id() as i32);
            panes.children.push(child);
            waited.push(Waited {
                index,
                repo: row.repo.clone(),
                command: row.command.clone(),
                pane,
            });
        }
        (panes, waited)
    }

    /// `checks` on `feature` as gerdr runs them, without the terminals.
    fn run(
        feature: &Feature,
        checks: &[Check],
        timeout: Duration,
        ended: &mut dyn FnMut(usize),
    ) -> Evidence {
        let begun = match begin(feature) {
            Ok(begun) => begun,
            Err(evidence) => return *evidence,
        };
        let dir = run_folder();
        let (mut panes, waited) = start(feature, checks, &dir);
        let stops: Vec<AtomicBool> = checks.iter().map(|_| AtomicBool::new(false)).collect();
        let mut outcomes: Vec<Option<Outcome>> = checks.iter().map(|_| None).collect();
        let mut turn = |_, _| {};
        watch(
            waited,
            &dir,
            timeout,
            &stops,
            &mut panes,
            &mut turn,
            &mut |i, o| {
                outcomes[i] = Some(o);
                ended(i);
            },
        );
        std::fs::remove_dir_all(&dir).unwrap();
        finish(
            feature,
            &begun,
            now(),
            outcomes.into_iter().flatten().collect(),
        )
    }

    fn by_name(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.iter().map(|c| (*c).into()).collect()))
            .collect()
    }

    fn config(pairs: &[(&str, &[&str])]) -> Config {
        PerProject::all(by_name(pairs))
    }

    #[test]
    fn linked_repos_check_with_their_own_projects_checks() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["web"], &[]);
        link_over(&t, &mut f, "other", &["api", "docs"], &["docs"]);
        let c = Config {
            home: by_name(&[
                ("web", &["w"]),
                ("api", &["home api"]),
                ("_feature", &["e2e"]),
            ]),
            links: [(
                "other".to_owned(),
                by_name(&[("api", &["a"]), ("docs", &["never"]), ("_feature", &["no"])]),
            )]
            .into(),
        };
        let wt = |i: usize| f.repos[i].worktree.clone();
        assert_eq!(
            checks(&f, &c),
            [
                check("other:api", &wt(1), "a"),
                check("web", &wt(0), "w"),
                check("_feature", &f.dir, "e2e"),
            ]
        );
        commit(&wt(1), "x", "x");
        let e = run(&f, &checks(&f, &c), TIMEOUT, &mut |_| {});
        let heads: Vec<&str> = e.heads.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(heads, ["other:api", "web"]);
    }

    fn check(repo: &str, dir: &Path, command: &str) -> Check {
        Check {
            repo: repo.into(),
            dir: dir.to_owned(),
            command: command.into(),
            own: false,
        }
    }

    #[test]
    fn touched_repos_check_in_name_order_then_the_feature() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "docs", "web"], &["docs"]);
        let c = config(&[
            ("_feature", &["./e2e.sh"]),
            ("web", &["w1", " ", "w2"]),
            ("docs", &["never"]),
            ("api", &["a"]),
        ]);
        let wt = |i: usize| f.repos[i].worktree.clone();
        assert_eq!(
            checks(&f, &c),
            [
                check("api", &wt(0), "a"),
                check("web", &wt(2), "w1"),
                check("web", &wt(2), "w2"),
                check("_feature", &f.dir, "./e2e.sh"),
            ]
        );
        assert!(checks(&f, &Config::default()).is_empty());
    }

    #[test]
    fn a_single_repo_takes_its_name_or_the_feature_key() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["app"], &[]);
        f.multi = false;
        f.dir = f.repos[0].worktree.clone();
        let c = config(&[("app", &["cargo test"]), ("_feature", &["./e2e.sh"])]);
        assert_eq!(
            checks(&f, &c),
            [
                check("app", &f.dir, "cargo test"),
                check("_feature", &f.dir, "./e2e.sh"),
            ]
        );
    }

    #[test]
    fn a_feature_adds_to_what_it_borrows_and_runs_nothing_twice() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api", "web"], &["web"]);
        let c = config(&[("api", &["cargo test"]), (FEATURE_KEY, &["./all.sh"])]);
        let of = |f: &Feature| {
            (checks(f, &c).into_iter())
                .map(|check| (check.repo, check.command, check.own))
                .collect::<Vec<_>>()
        };
        let plain = |repo: &str, command: &str, own| (repo.to_owned(), command.to_owned(), own);
        assert_eq!(
            of(&f),
            [
                plain("api", "cargo test", false),
                plain(FEATURE_KEY, "./all.sh", false)
            ]
        );
        for (key, commands) in [
            ("api", vec![" ./e2e.sh ", "", "cargo test"]),
            ("web", vec!["npm test"]),
            (FEATURE_KEY, vec!["./smoke.sh"]),
        ] {
            let commands = commands.into_iter().map(str::to_owned).collect();
            f.checks.insert(key.to_owned(), commands);
        }
        // Each after what its key borrows; an untouched repository's stay
        // out, and a command it borrows already doesn't run twice.
        assert_eq!(
            of(&f),
            [
                plain("api", "cargo test", false),
                plain("api", "./e2e.sh", true),
                plain(FEATURE_KEY, "./all.sh", false),
                plain(FEATURE_KEY, "./smoke.sh", true)
            ]
        );
        assert_eq!(checks(&f, &c)[1].dir, f.repos[0].worktree);
        assert_eq!(checks(&f, &c)[3].dir, f.dir);
        // With nothing to borrow, its own are what runs.
        let none = checks(&f, &Config::default());
        assert_eq!(none.len(), 3);
        assert!(none.iter().all(|check| check.own));
    }

    #[test]
    fn a_feature_skips_what_it_borrows_by_its_words() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let c = config(&[
            ("api", &["slow e2e", "cargo test"]),
            (FEATURE_KEY, &["./all.sh"]),
        ]);
        let words = |checks: Vec<Check>| {
            checks
                .into_iter()
                .map(|check| check.command)
                .collect::<Vec<_>>()
        };
        f.skip_checks.insert("api".into(), vec!["slow e2e".into()]);
        f.skip_checks
            .insert(FEATURE_KEY.into(), vec!["./all.sh".into(), "gone".into()]);
        assert_eq!(words(checks(&f, &c)), ["cargo test"]);
        assert_eq!(words(skipped(&f, &c)), ["slow e2e", "./all.sh"]);
        // Added back as its own, it runs, once.
        f.checks.insert("api".into(), vec!["slow e2e".into()]);
        assert_eq!(words(checks(&f, &c)), ["cargo test", "slow e2e"]);
        assert!(checks(&f, &c)[1].own);
        // Reworded in the project, it runs again.
        f.checks.clear();
        let c = config(&[("api", &["slow e2e --all"])]);
        assert_eq!(words(checks(&f, &c)), ["slow e2e --all"]);
        assert!(skipped(&f, &c).is_empty());
    }

    #[test]
    fn the_evidence_says_what_was_skipped_if_anything_ran() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let e = Evidence {
            outcomes: vec![Outcome::default()],
            ..Evidence::default()
        };
        assert!(!markdown(&f, &e).contains("Skipped"));
        f.verify = Some(VerifyRun {
            skipped: vec!["api: slow (skipped for this feature)".into()],
            ..VerifyRun::default()
        });
        assert!(markdown(&f, &e).contains("- api: slow (skipped for this feature)"));
        assert!(!markdown(&f, &Evidence::default()).contains("Skipped"));
    }

    #[test]
    fn the_page_lists_checks_where_they_stand_skipped_or_not() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let c = config(&[("api", &["slow", "fast"])]);
        f.skip_checks.insert("api".into(), vec!["slow".into()]);
        f.checks.insert("api".into(), vec!["mine".into()]);
        let listed: Vec<_> = (plan_cards(&f, &c).into_iter())
            .map(|p| (p.repo, p.command, p.skipped, p.own))
            .collect();
        let row =
            |command: &str, skipped, own| ("api".to_owned(), command.to_owned(), skipped, own);
        assert_eq!(
            listed,
            [
                row("slow", true, false),
                row("fast", false, false),
                row("mine", false, true),
            ]
        );
    }

    #[test]
    fn a_preview_is_of_what_the_check_prints_now() {
        let head = "\u{25b6} Check api: make";
        let end = "\u{25a0} Check passed";
        // The shell hasn't started it: no start line, and it hasn't begun.
        assert!(preview("$ sh gerdr-check-0\n", false).is_empty());
        let old: String = (0..PREVIEW_READ).map(|i| format!("{i}\n")).collect();
        assert!(preview(&old, false).is_empty());
        // What it did last time, in a terminal used again.
        let again = format!("{head}\nold\n{end}\n$ sh gerdr-check-0\n");
        assert!(preview(&again, false).is_empty());
        assert!(preview(&again, true).is_empty());
        // Its last lines, without the start line; a redrawn line is one line.
        let text = format!("$ prompt\n{head}\n1\n\n2\n3\n4\n50%\r100%\n");
        assert_eq!(preview(&text, false), ["2", "3", "4", "100%"]);
        assert_eq!(preview(&text, true), ["2", "3", "4", "100%"]);
        // So long that the read lost its start line. Long lines wrap, so the
        // read's rows may hold only a few of them.
        assert_eq!(preview("a\nb\nc\nd\ne\n", true), ["b", "c", "d", "e"]);
        assert_eq!(last_lines("a\tb\n\n"), ["a    b"]);
    }

    #[test]
    fn the_lines_of_a_failure_say_why() {
        let cargo_build = "   Compiling app v1\nerror[E0308]: mismatched types\n --> src/a.rs:1:1\n\nerror: could not compile\n";
        assert_eq!(
            failure_lines(cargo_build),
            [
                "error[E0308]: mismatched types",
                " --> src/a.rs:1:1",
                "error: could not compile"
            ]
        );
        let cargo_test = "test a ... ok\ntest b ... FAILED\n\n---- b stdout ----\nthread 'b' panicked at src/a.rs:1:\nboom\n\nfailures:\n    b\n\ntest result: FAILED. 1 passed; 1 failed\n\nerror: test failed\n";
        assert_eq!(
            failure_lines(cargo_test)[0],
            "thread 'b' panicked at src/a.rs:1:"
        );
        let pytest =
            "tests/a.py F\n=== FAILURES ===\nE   AssertionError: no\n=== 1 failed in 0.1s ===\n";
        assert_eq!(failure_lines(pytest)[0], "E   AssertionError: no");
        // Python's traceback: where it was, then what it was.
        let unittest = "F\n======\nFAIL: test_two (test_x.T.test_two)\n------\nTraceback (most recent call last):\n  File \"/a/test_x.py\", line 3, in helper\n    check()\n  File \"/a/test_x.py\", line 20, in test_two\n    self.assertEqual(got, \"10.50\")\n    ~~~~~~~~^^^^^\nAssertionError: '10.5' != '10.50'\n- 10.5\n+ 10.50\n\n------\nRan 1 test in 0.001s\n\nFAILED (failures=1)\n";
        assert_eq!(
            failure_lines(unittest),
            [
                "  File \"/a/test_x.py\", line 20, in test_two",
                "    self.assertEqual(got, \"10.50\")",
                "AssertionError: '10.5' != '10.50'",
                "- 10.5"
            ]
        );
        let jest = "PASS src/a.test.js\nFAIL src/b.test.js\n  \u{25cf} b works\n";
        assert_eq!(failure_lines(jest)[0], "FAIL src/b.test.js");
        // What says none, and what only names one.
        for clean in [
            "0 failed\nno errors\nErrors: 0, Failures: 0\n",
            "src/error.rs\ntest no_error_here ... ok\nmirror-errors/a\n",
            "something else\n",
        ] {
            assert_eq!(failure_lines(clean), Vec::<String>::new(), "{clean}");
        }
    }

    #[test]
    fn a_check_interrupted_or_passed_keeps_no_excerpt() {
        struct One(String);
        impl Panes for One {
            fn gone(&mut self, _: &str) -> bool {
                false
            }
            fn text(&mut self, _: &str) -> Result<String, Unread> {
                Ok(self.0.clone())
            }
            fn interrupt(&mut self, _: &str) {}
        }
        let head = "\u{25b6} Check api: t";
        let end = "\u{25a0} Check exited with 1";
        let mut panes = One(format!("{head}\nerror: boom\nmore\nlast\n{end}\n"));
        let mut failed = Outcome {
            exit: Some(1),
            ..Outcome::default()
        };
        failed.keep_output(&mut panes, "p");
        assert_eq!(failed.excerpt, ["error: boom", "more", "last"]);
        assert_eq!(failed.preview(), ["error: boom", "more", "last"]);
        let mut passed = Outcome {
            exit: Some(0),
            ..Outcome::default()
        };
        passed.keep_output(&mut panes, "p");
        assert!(passed.excerpt.is_empty());
        let mut stopped = Outcome {
            interrupted: true,
            ..Outcome::default()
        };
        stopped.keep_output(&mut panes, "p");
        assert!(stopped.excerpt.is_empty());
        assert_eq!(stopped.preview(), ["error: boom", "more", "last"]);
    }

    /// A terminal whose reads go through `reads`, in order, its last for good.
    struct Reads {
        reads: Vec<Result<String, Unread>>,
        asked: usize,
    }

    impl Panes for Reads {
        fn gone(&mut self, _: &str) -> bool {
            false
        }
        fn text(&mut self, _: &str) -> Result<String, Unread> {
            self.asked += 1;
            let at = (self.asked - 1).min(self.reads.len() - 1);
            self.reads[at].clone()
        }
        fn interrupt(&mut self, _: &str) {}
    }

    #[test]
    fn a_busy_server_doesnt_lose_what_a_check_printed() {
        let busy = || Err(Unread::Busy("timed out".into()));
        let text = "\u{25b6} Check api: t\nboom\n\u{25a0} Check exited with 1\n".to_owned();
        let mut panes = Reads {
            reads: vec![busy(), busy(), Ok(text)],
            asked: 0,
        };
        let mut failed = Outcome {
            exit: Some(1),
            ..Outcome::default()
        };
        failed.keep_output(&mut panes, "p");
        assert_eq!(failed.tail, "boom");
        assert_eq!(panes.asked, 3);
    }

    #[test]
    fn a_read_that_lags_waits_for_the_end_line_of_this_run() {
        let head = "\u{25b6} Check api: t";
        let end = "\u{25a0} Check passed";
        // The end line of the run before doesn't count.
        let lags = format!("{head}\nold\n{end}\n{head}\nnew\n");
        let done = format!("{lags}last\n{end}\n");
        let mut panes = Reads {
            reads: vec![Ok(lags), Ok(done)],
            asked: 0,
        };
        let mut passed = Outcome {
            exit: Some(0),
            ..Outcome::default()
        };
        passed.keep_output(&mut panes, "p");
        assert_eq!(passed.tail, "new\nlast");
    }

    #[test]
    fn a_terminal_gone_isnt_read_again() {
        let mut panes = Reads {
            reads: vec![Err(Unread::Gone("pane not found".into()))],
            asked: 0,
        };
        let mut stopped = Outcome {
            interrupted: true,
            ..Outcome::default()
        };
        stopped.keep_output(&mut panes, "p");
        assert_eq!(stopped.tail, "");
        assert_eq!(panes.asked, 1);
    }

    #[test]
    fn a_check_keeps_its_whole_log_only_when_its_terminal_gives_one() {
        // Terminals that keep no log, as by default.
        let text = format!("{HEAD}api: t\nboom\n{END}exited with 1\n");
        let mut panes = Reads {
            reads: vec![Ok(text.clone())],
            asked: 0,
        };
        let mut failed = Outcome {
            exit: Some(1),
            ..Outcome::default()
        };
        failed.keep_output(&mut panes, "p");
        assert_eq!(failed.log, None);

        struct Kept(String, Vec<String>);
        impl Panes for Kept {
            fn gone(&mut self, _: &str) -> bool {
                false
            }
            fn text(&mut self, _: &str) -> Result<String, Unread> {
                Ok(self.0.clone())
            }
            fn interrupt(&mut self, _: &str) {}
            fn keep_log(&mut self, pane: &str) -> Option<String> {
                self.1.push(pane.to_owned());
                Some(format!("before\n{}", self.0))
            }
        }
        let mut panes = Kept(text.clone(), Vec::new());
        failed.keep_output(&mut panes, "p7");
        assert_eq!(failed.log, Some(format!("before\n{text}")));
        assert_eq!(panes.1, ["p7"]);
        assert_eq!(failed.tail, "boom");
        // The record never holds it.
        let saved = serde_json::to_string(&failed).unwrap();
        assert!(!saved.contains("before"), "{saved}");
        let read: Outcome = serde_json::from_str(&saved).unwrap();
        assert_eq!(read.log, None);
    }

    #[test]
    fn a_run_remembers_how_long_a_check_took() {
        let wt = Path::new("/w");
        let listed = [check("api", wt, "a")];
        let mut last = plan(&listed, &[], &|_| false, &Scope::All, 1);
        assert_eq!(last[0].last_secs, None);
        last[0].outcome = Some(Outcome {
            exit: Some(1),
            secs: 90,
            ..Outcome::default()
        });
        let mut next = plan(&listed, &last, &|_| false, &Scope::All, 2);
        assert_eq!(next[0].last_secs, Some(90));
        // Stopped says nothing of how long it takes.
        next[0].outcome = Some(Outcome {
            interrupted: true,
            secs: 3,
            ..Outcome::default()
        });
        assert_eq!(
            plan(&listed, &next, &|_| false, &Scope::All, 3)[0].last_secs,
            Some(90)
        );
    }

    #[test]
    fn a_check_says_what_it_is_in_a_line() {
        let mut c = Check {
            repo: "api".into(),
            dir: PathBuf::new(),
            command: "make".into(),
            own: false,
        };
        assert_eq!(c.line(), "api: make");
        c.own = true;
        assert_eq!(c.line(), "api: make (this feature only)");
        c.repo = FEATURE_KEY.into();
        assert_eq!(c.line(), "whole feature: make (this feature only)");
    }

    #[test]
    fn every_check_runs_and_a_failure_fails() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        commit(&f.repos[0].worktree, "login", "log in");
        let c = config(&[
            (
                "api",
                &["echo $GERDR_FEATURE; printf '\\033[31mred\\033[0m\\n'"],
            ),
            ("web", &["echo broke >&2; exit 2", "pwd"]),
        ]);
        let mut ended = 0;
        let e = run(&f, &checks(&f, &c), TIMEOUT, &mut |_| ended += 1);
        assert_eq!(ended, 3);
        assert!(!e.passed());
        assert!(e.problems.is_empty(), "{:?}", e.problems);
        assert_eq!(e.patch, patch::read(&f).unwrap().patch);
        let head = git_run(&f.repos[0].worktree, &["rev-parse", "HEAD"]);
        assert_eq!(e.heads[0], ("api".to_owned(), head.trim().to_owned()));
        assert_eq!(e.heads.len(), 2);
        assert_eq!(e.outcomes[0].tail, "f\nred");
        assert!(e.outcomes[0].passed());
        assert_eq!(e.outcomes[1].exit, Some(2));
        assert_eq!(e.outcomes[1].tail, "broke");
        assert_eq!(
            e.outcomes[2].tail,
            f.repos[1].worktree.display().to_string()
        );
        assert_eq!(
            e.failures(),
            ["web: `echo broke >&2; exit 2` exited with 2"]
        );
        assert!(e.at > 0);
    }

    #[test]
    fn uncommitted_or_stray_changes_run_nothing() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "docs"], &["docs"]);
        let c = checks(&f, &config(&[("api", &["true"])]));
        std::fs::write(f.repos[0].worktree.join("new"), "x").unwrap();
        std::fs::write(f.repos[1].worktree.join("a"), "x").unwrap();
        let mut said = 0;
        let e = run(&f, &c, TIMEOUT, &mut |_| said += 1);
        assert_eq!(said, 0);
        assert!(e.outcomes.is_empty() && e.heads.is_empty());
        assert_eq!(
            e.failures(),
            [
                "uncommitted changes in api",
                "docs isn't touched by the feature but has changes",
            ]
        );
        std::fs::remove_dir_all(&f.repos[0].worktree).unwrap();
        let e = run(&f, &c, TIMEOUT, &mut |_| {});
        assert!(e.problems[0].starts_with("couldn't read the code: api: "));
        assert!(e.patch.is_empty());
    }

    #[test]
    fn checks_that_change_the_code_fail() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        let commit = "git -c user.name=t -c user.email=t@t commit -qam fmt";
        let c = config(&[
            ("api", &["echo more >> a"]),
            ("web", &[&format!("echo more >> a && {commit}")]),
        ]);
        let e = run(&f, &checks(&f, &c), TIMEOUT, &mut |_| {});
        assert!(e.outcomes.iter().all(Outcome::passed));
        assert_eq!(
            e.problems,
            [
                "the checks changed the code in api",
                "the checks changed the code in the feature's commits",
            ]
        );
        assert!(!e.passed());
    }

    #[test]
    fn a_check_past_its_time_fails() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(&f, &config(&[("api", &["echo started; sleep 30"])]));
        let e = run(&f, &c, Duration::from_millis(300), &mut |_| {});
        assert!(e.outcomes[0].timed_out);
        assert_eq!(e.outcomes[0].exit, None);
        assert_eq!(e.outcomes[0].tail, "started");
        assert_eq!(
            e.failures(),
            ["api: `echo started; sleep 30` timed out after 0 s"]
        );
    }

    #[test]
    fn checks_of_different_folders_run_at_the_same_time() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        // Each waits for the other: one after the other they'd time out.
        let git_dir = git_run(&f.repos[0].worktree, &["rev-parse", "--absolute-git-dir"]);
        let meet = format!("{}/meet", git_dir.trim());
        let wait = |me: &str, other: &str| {
            format!("touch {meet}-{me}; until [ -e {meet}-{other} ]; do sleep 0.05; done; pwd")
        };
        let (a, b) = (wait("a", "b"), wait("b", "a"));
        let c = checks(
            &f,
            &config(&[("api", &[a.as_str()]), ("web", &[b.as_str()])]),
        );
        let e = run(&f, &c, Duration::from_secs(10), &mut |_| {});
        assert!(e.outcomes.iter().all(Outcome::passed), "{:?}", e.outcomes);
        assert_eq!(
            e.outcomes[1].tail,
            f.repos[1].worktree.display().to_string()
        );
    }

    #[test]
    fn checks_of_one_folder_take_turns() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let git_dir = git_run(&f.repos[0].worktree, &["rev-parse", "--absolute-git-dir"]);
        let held = format!("{}/held", git_dir.trim());
        // Each finds the folder its own while it runs, or says so.
        let turn = format!(
            "if ! mkdir {held}; then echo clash; exit 9; fi; echo mine; sleep 0.3; rmdir {held}"
        );
        let (a, b) = (format!("{turn}; : a"), format!("{turn}; : b"));
        let c = checks(&f, &config(&[("api", &[a.as_str(), b.as_str()])]));
        let dir = run_folder();
        let (mut panes, waited) = start(&f, &c, &dir);
        let stops: Vec<AtomicBool> = c.iter().map(|_| AtomicBool::new(false)).collect();
        let (mut ended, mut turns) = (Vec::new(), Vec::new());
        let mut heard = |i, turn| turns.push((i, turn));
        watch(
            waited,
            &dir,
            TIMEOUT,
            &stops,
            &mut panes,
            &mut heard,
            &mut |_, o| {
                ended.push(o);
            },
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(ended.iter().all(Outcome::passed), "{ended:?}");
        // What the wait printed is no part of what the check printed.
        assert!(ended.iter().all(|o| o.tail == "mine"), "{ended:?}");
        let queued: Vec<usize> = (turns.iter())
            .filter(|(_, turn)| *turn == Turn::Queued)
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(queued.len(), 1, "{turns:?}");
        // It starts after the wait, and both are told.
        let began = |i: usize| {
            (turns.iter()).position(|(at, turn)| *at == i && matches!(turn, Turn::Began(_)))
        };
        let waiter = queued[0];
        let wait_at = (turns.iter()).position(|t| *t == (waiter, Turn::Queued));
        assert!(began(1 - waiter).is_some());
        assert!(began(waiter) > wait_at, "{turns:?}");
    }

    #[test]
    fn a_daemon_a_check_leaves_doesnt_keep_the_folder() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(
            &f,
            &config(&[("api", &["(sleep 8 >/dev/null 2>&1 &) ; true", "true"])]),
        );
        let at = Instant::now();
        let e = run(&f, &c, Duration::from_secs(5), &mut |_| {});
        assert!(e.outcomes.iter().all(Outcome::passed), "{:?}", e.outcomes);
        assert!(at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_time_of_a_check_in_the_queue_hasnt_begun() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        // Each fits its time, but the last's wait and run together don't.
        let c = checks(
            &f,
            &config(&[(
                "api",
                &[
                    "sleep 0.6; echo a",
                    "sleep 0.6; echo b",
                    "sleep 0.6; echo c",
                ],
            )]),
        );
        let e = run(&f, &c, Duration::from_secs(1), &mut |_| {});
        assert!(e.outcomes.iter().all(Outcome::passed), "{:?}", e.outcomes);
    }

    #[test]
    fn with_no_flock_a_check_still_runs_and_says_it_wont_wait() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(&f, &config(&[("api", &["echo ran"])]));
        let dir = run_folder();
        // A PATH with what the script needs but flock.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["sh", "mv"] {
            let found = on_path(tool);
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        let row = &rows(&c)[0];
        let script = script_path(&dir, 0);
        std::fs::write(&script, super::script(&dir, 0, row, &f.slug)).unwrap();
        let out = Command::new(bin.join("sh"))
            .arg(&script)
            .env("PATH", &bin)
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(printed.contains(NO_FLOCK), "{printed}");
        assert!(printed.contains("ran"), "{printed}");
        assert_eq!(read_number(&dir.join("0.exit")), Some(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Where `tool` is on the PATH.
    fn on_path(tool: &str) -> PathBuf {
        (std::env::split_paths(&std::env::var_os("PATH").unwrap()))
            .map(|dir| dir.join(tool))
            .find(|path| path.exists())
            .unwrap()
    }

    #[test]
    fn a_check_in_the_queue_can_stop() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(
            &f,
            &config(&[("api", &["echo held; sleep 30", "echo ran"])]),
        );
        let appears = |path: PathBuf| {
            let from = Instant::now();
            while !path.exists() {
                assert!(from.elapsed() < Duration::from_secs(10), "{path:?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let (holds, queues) = (run_folder(), run_folder());
        let (mut hold_panes, hold) = start(&f, &c[..1], &holds);
        appears(holds.join("0.start"));
        let (mut queue_panes, queue) = start(&f, &c[1..], &queues);
        appears(queues.join("0.queued"));

        let stops = [AtomicBool::new(true)];
        let mut ended = Vec::new();
        let mut turns = Vec::new();
        watch(
            queue,
            &queues,
            TIMEOUT,
            &stops,
            &mut queue_panes,
            &mut |i, turn| turns.push((i, turn)),
            &mut |_, o| ended.push(o),
        );
        assert_eq!(ended[0].how(), "stopped");
        assert!(ended[0].interrupted);
        assert_eq!(ended[0].tail, QUEUED);
        assert_eq!(turns, [(0, Turn::Queued)]);

        let mut turn = |_, _| {};
        watch(
            hold,
            &holds,
            TIMEOUT,
            &stops,
            &mut hold_panes,
            &mut turn,
            &mut |_, _| {},
        );
        std::fs::remove_dir_all(&holds).unwrap();
        std::fs::remove_dir_all(&queues).unwrap();
    }

    #[test]
    fn a_stopped_check_says_nothing_of_the_code() {
        let t = Temp::new();
        let f = feature_over(&t, &["api", "web"], &[]);
        let c = checks(
            &f,
            &config(&[("api", &["echo up; sleep 30"]), ("web", &["echo done"])]),
        );
        let dir = run_folder();
        let (mut panes, waited) = start(&f, &c, &dir);
        let stops: Vec<AtomicBool> = c.iter().map(|_| AtomicBool::new(false)).collect();
        stops[0].store(true, Ordering::Relaxed);
        let mut ended = Vec::new();
        let mut turn = |_, _| {};
        watch(
            waited,
            &dir,
            TIMEOUT,
            &stops,
            &mut panes,
            &mut turn,
            &mut |_, o| {
                ended.push(o);
            },
        );
        std::fs::remove_dir_all(&dir).unwrap();
        ended.sort_by_key(|o| o.command.clone());
        let (done, stopped) = (&ended[0], &ended[1]);
        assert_eq!(stopped.how(), "stopped");
        assert!(stopped.interrupted && !stopped.passed());
        assert_eq!(stopped.tail, "up");
        assert!(done.passed() && !done.interrupted);
        let e = Evidence {
            outcomes: ended,
            ..Evidence::default()
        };
        assert!(e.interrupted() && !e.passed());
    }

    #[test]
    fn a_closed_terminal_fails_at_once() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(&f, &config(&[("api", &["sleep 30"])]));
        let dir = run_folder();
        let (mut panes, waited) = start(&f, &c, &dir);
        panes.closed.push("p0".into());
        let stops = [AtomicBool::new(false)];
        let mut ended = Vec::new();
        let at = Instant::now();
        let mut turn = |_, _| {};
        watch(
            waited,
            &dir,
            TIMEOUT,
            &stops,
            &mut panes,
            &mut turn,
            &mut |_, o| {
                ended.push(o);
            },
        );
        assert!(at.elapsed() < Duration::from_secs(10));
        panes.interrupt("p0");
        // Its script may still be leaving its exit code: not this test's.
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ended[0].how(), "its terminal was closed");
        assert!(ended[0].interrupted);
    }

    #[test]
    fn what_a_check_printed_is_told_from_what_ran_before() {
        let text = "old\n\u{25b6} Check api: a\nold out\n\u{25a0} Check passed\n\
                    $ again\n\u{25b6} Check api: a\nnew out\n\u{25a0} Check exited with 1\n$ ";
        assert_eq!(output_of(text), "new out\n");
        // Scrolled off: what is left.
        assert_eq!(output_of("tail\n\u{25a0} Check passed\n"), "tail\n");
        // Its last line not drawn yet.
        assert_eq!(output_of("\u{25b6} Check a: b\nsome"), "some");
    }

    #[test]
    fn a_rerun_keeps_the_rest_only_on_the_same_code() {
        let wt = Path::new("/w");
        let listed = [
            check("api", wt, "a"),
            check("api", wt, "b"),
            check("api", wt, "c"),
        ];
        let mut last = plan(&listed, &[], &|_| false, &Scope::All, 1);
        for row in &mut last {
            row.pane = Some(format!("pane-{}", row.command));
            row.outcome = Some(Outcome {
                exit: Some(0),
                ..Outcome::default()
            });
        }
        let only_b = Scope::Only(vec![last[1].key()]);
        let same = plan(&listed, &last, &|_| true, &only_b, 2);
        assert!(same[0].outcome.is_some() && same[2].outcome.is_some());
        assert_eq!((same[0].run, same[2].run), (1, 1));
        assert!(same[1].outcome.is_none());
        assert_eq!(same[1].run, 2);
        assert_eq!(same[1].pane.as_deref(), Some("pane-b"));
        // On other code nothing stands.
        let other = plan(&listed, &last, &|_| false, &only_b, 2);
        assert!(
            other
                .iter()
                .all(|row| row.outcome.is_none() && row.run == 2)
        );
        assert!(other.iter().all(|row| row.pane.is_some()));
        // All of them: nothing stands either, and cards come back.
        last[0].card_off = true;
        let all = plan(&listed, &last, &|_| true, &Scope::All, 2);
        assert!(all.iter().all(|row| row.outcome.is_none() && !row.card_off));
        // A check no longer set goes, and a new one runs.
        let changed = [check("api", wt, "a"), check("api", wt, "new")];
        let row = plan(&changed, &last, &|_| true, &Scope::Only(Vec::new()), 2);
        assert_eq!(row.len(), 2);
        assert!(row[0].outcome.is_some() && row[1].outcome.is_none());
    }

    #[test]
    fn a_rerun_keeps_what_passed_where_the_code_didnt_move() {
        let wt = Path::new("/w");
        let listed = [
            check("api", wt, "a"),
            check("api", wt, "b"),
            check("web", wt, "lint"),
            check("web", wt, "test"),
            check("docs", wt, "build"),
            check(FEATURE_KEY, wt, "e2e"),
        ];
        let mut rows = plan(&listed, &[], &|_| false, &Scope::All, 1);
        for row in &mut rows {
            let failed = row.command == "a" || row.command == "lint";
            row.outcome = Some(Outcome {
                exit: Some(i32::from(failed)),
                ..Outcome::default()
            });
        }
        let heads = |api: &str| {
            vec![
                ("api".to_owned(), api.to_owned()),
                ("docs".to_owned(), "d".to_owned()),
                ("web".to_owned(), "w".to_owned()),
            ]
        };
        let last = VerifyRun {
            n: 1,
            patch: Some("old".into()),
            checks: rows,
            heads: heads("a1"),
            ..VerifyRun::default()
        };
        // The fix moved api alone.
        let begun = Begun {
            patch: "new".into(),
            heads: heads("a2"),
        };
        let scope = Scope::after_changes(Some(&last));
        let holds = |repo: &str| last.holds(&begun, repo);
        let next = plan(&listed, &last.checks, &holds, &scope, 2);
        let runs: Vec<_> = (next.iter())
            .filter(|row| row.outcome.is_none())
            .map(|row| row.command.as_str())
            .collect();
        // All of api's, web's that failed though web didn't move, and the
        // whole feature's. What passed elsewhere stands, from its run.
        assert_eq!(runs, ["a", "b", "lint", "e2e"]);
        let stands = |command: &str| next.iter().find(|row| row.command == command).unwrap().run;
        assert_eq!((stands("test"), stands("build")), (1, 1));
        assert_eq!((stands("a"), stands("e2e")), (2, 2));
        // A first run has nothing to keep.
        assert_eq!(Scope::after_changes(None), Scope::All);
    }

    #[test]
    fn what_a_run_said_holds_on_its_code_only() {
        let heads = |api: &str| vec![("api".to_owned(), api.to_owned())];
        let run = VerifyRun {
            patch: Some("p".into()),
            heads: heads("a1"),
            ..VerifyRun::default()
        };
        let begun = |patch: &str, api: &str| Begun {
            patch: patch.into(),
            heads: heads(api),
        };
        // The same change, rebased: all of it holds.
        assert!(run.holds(&begun("p", "a2"), "api"));
        assert!(run.holds(&begun("p", "a2"), FEATURE_KEY));
        // Another change: a repository holds by its HEAD alone.
        assert!(run.holds(&begun("q", "a1"), "api"));
        assert!(!run.holds(&begun("q", "a2"), "api"));
        // The whole feature's checks read all the code.
        assert!(!run.holds(&begun("q", "a1"), FEATURE_KEY));
        // A repository with no HEAD on either side says nothing.
        assert!(!run.holds(&begun("q", "a1"), "web"));
        assert!(!(VerifyRun::default()).holds(&begun("q", "a1"), "api"));
        // No patch read is no proof of the same change.
        let unread = VerifyRun::default();
        assert!(!unread.holds(&begun("", "a1"), FEATURE_KEY));
    }

    #[test]
    fn begun_reads_which_repository_a_fix_moved() {
        let tmp = Temp::new();
        let f = feature_over(&tmp, &["api", "web"], &[]);
        commit(&f.repos[0].worktree, "a.txt", "1");
        commit(&f.repos[1].worktree, "w.txt", "1");
        let before = begin(&f).unwrap();
        commit(&f.repos[0].worktree, "a.txt", "2");
        let after = begin(&f).unwrap();
        assert_ne!(before.patch, after.patch);
        let run = VerifyRun {
            patch: Some(before.patch),
            heads: before.heads,
            ..VerifyRun::default()
        };
        assert!(!run.holds(&after, &f.repos[0].key()));
        assert!(run.holds(&after, &f.repos[1].key()));
    }

    #[test]
    fn the_evidence_says_a_check_stands_from_an_earlier_run() {
        let tmp = Temp::new();
        let mut f = feature_over(&tmp, &["api"], &[]);
        let ran = |command: &str| Outcome {
            repo: "api".into(),
            command: command.into(),
            exit: Some(0),
            secs: 7,
            ..Outcome::default()
        };
        let row = |command: &str, run| CheckRun {
            repo: "api".into(),
            command: command.into(),
            run,
            outcome: Some(ran(command)),
            ..CheckRun::default()
        };
        f.verify = Some(VerifyRun {
            n: 3,
            checks: vec![row("old", 2), row("new", 3)],
            ..VerifyRun::default()
        });
        let md = markdown(
            &f,
            &Evidence {
                outcomes: vec![ran("old"), ran("new")],
                ..Evidence::default()
            },
        );
        assert!(
            md.contains("Passed, in 7 s, in run 2: its code didn't change since."),
            "{md}"
        );
        assert!(md.contains("Passed, in 7 s.\n"), "{md}");
    }

    #[test]
    fn the_tail_is_readable() {
        assert_eq!(tail(b"a\r\n\x1b[1;32mok\x1b[0m\r\n", false), "a\nok");
        assert_eq!(tail(b"10%\r50%\r100%\r\ndone\n\n\n", false), "100%\ndone");
        assert_eq!(
            tail(b"\x1b]0;title\x07x\x1b]8;;u\x1b\\y\x07z", false),
            "xyz"
        );
        assert_eq!(tail(b"partial\nwhole", true), "whole");
        let many: String = (0..100).map(|i| format!("{i}\n")).collect();
        let t = tail(many.as_bytes(), false);
        assert_eq!(t.lines().count(), TAIL);
        assert!(t.starts_with("60\n") && t.ends_with("99"));
        let long = "x".repeat(LINE + 10);
        assert_eq!(tail(long.as_bytes(), false).chars().count(), LINE + 1);
    }

    #[test]
    fn the_evidence_reads_as_markdown() {
        let mut f = crate::feature::tests::feature("bulk", 1);
        f.title = "Bulk edit".into();
        let e = Evidence {
            at: 1_793_628_300,
            patch: "abc".into(),
            heads: vec![("api".into(), "123".into())],
            outcomes: vec![
                Outcome {
                    repo: "api".into(),
                    command: "npm test".into(),
                    exit: Some(1),
                    secs: 3,
                    tail: "```\nboom".into(),
                    ..Outcome::default()
                },
                Outcome {
                    repo: "_feature".into(),
                    command: "./e2e.sh".into(),
                    exit: Some(0),
                    ..Outcome::default()
                },
            ],
            problems: Vec::new(),
            ran_on: None,
        };
        let md = markdown(&f, &e);
        for words in [
            "# Verify: Bulk edit\n",
            "- When: 2026-11-02 14:05 UTC\n",
            "- Result: **FAILED**\n",
            "- Patch-id: `abc`\n",
            "| api | `123` |\n",
            "## What failed\n\n- api: `npm test` exited with 1\n",
            "### api: npm test\n\nExited with 1, in 3 s.\n",
            "````text\n```\nboom\n````\n",
            "### _feature: ./e2e.sh\n\nPassed, in 0 s.\n\nNo output.\n",
        ] {
            assert!(md.contains(words), "{words}\n---\n{md}");
        }
        let none = markdown(&f, &Evidence::default());
        assert!(none.contains("**PASSED**") && none.contains("No checks are set"));
        assert!(none.contains("Patch-id: unknown"));
    }

    #[test]
    fn dates_read_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(951_868_740), "2000-02-29 23:59 UTC");
        assert_eq!(utc(1_793_628_300), "2026-11-02 14:05 UTC");
    }
}
