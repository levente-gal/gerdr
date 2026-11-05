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
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
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
/// (`"workflow": {"verify_timeout": 900}`, seconds).
pub const TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// How many of a check's last output lines the evidence keeps.
pub const TAIL: usize = 40;

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
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.exit == Some(0)
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
    /// 20 min", "stopped".
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

/// What the next run of `feature` does that its last run's cards don't
/// show, a line each: first what it skips, which no line count may hide,
/// then what it runs that isn't on a card yet.
pub fn planned(feature: &Feature, config: &Config) -> Vec<String> {
    let (runs, skips) = split(feature, config);
    let cards = feature.verify.as_ref().map_or(&[][..], |v| &v.checks);
    let on_card = |check: &Check| {
        (cards.iter()).any(|card| card.repo == check.repo && card.command == check.command)
    };
    (skips.iter().map(Check::skipped_line))
        .chain(runs.iter().filter(|c| !on_card(c)).map(Check::line))
        .collect()
}

/// What `feature` runs of what `config` gives it, in order, and what it
/// skips.
fn split(feature: &Feature, config: &Config) -> (Vec<Check>, Vec<Check>) {
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
    // What `key` borrows, then what is its own and not borrowed already;
    // and what it borrows and skips.
    let mut skips = Vec::new();
    let mut of = |borrowed: Vec<String>, key: String, dir: &PathBuf| {
        let skip = commands(Some(&feature.skip_checks), &key);
        let (skipped, borrowed): (Vec<_>, Vec<_>) =
            borrowed.into_iter().partition(|c| skip.contains(c));
        let own: Vec<String> = commands(Some(&feature.checks), &key)
            .into_iter()
            .filter(|c| !borrowed.contains(c))
            .collect();
        let tagged = |commands: Vec<String>, own: bool| {
            let (key, dir) = (key.clone(), dir.clone());
            (commands.into_iter()).map(move |command| Check {
                repo: key.clone(),
                dir: dir.clone(),
                command,
                own,
            })
        };
        skips.extend(tagged(skipped, false));
        tagged(borrowed, false)
            .chain(tagged(own, true))
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
    (checks, skips)
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
    /// stands, when it was on this code.
    Only(Vec<(String, String)>),
}

/// What run `n` of `checks` is to do: a row for each, those that `scope`
/// keeps with their outcomes from the `last` run if it was on this code
/// (`same_code`). Every row keeps its terminal, to run in again.
pub fn plan(
    checks: &[Check],
    last: &[CheckRun],
    same_code: bool,
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
                .filter(|_| same_code && !again)
                .and_then(|c| Some((c.outcome.clone()?, c.run)));
            CheckRun {
                repo: check.repo.clone(),
                command: check.command.clone(),
                dir: check.dir.clone(),
                pane: was.and_then(|c| c.pane.clone()),
                run: kept.as_ref().map_or(n, |(_, run)| *run),
                outcome: kept.map(|(outcome, _)| outcome),
                card_off: false,
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

const SCRIPT: &str = "gerdr-check-";

/// Whether a command line is a check's script: what it runs ends by itself,
/// and its card tells how it went.
pub fn runs_check(command: &str) -> bool {
    command.contains(SCRIPT)
}

/// The start of the line a check's script prints before it.
const HEAD: &str = "\u{25b6} Check ";
/// The start of the line it prints after it.
const END: &str = "\u{25a0} Check ";

/// The script of check `i` of `check`, which leaves its pid and then its exit
/// code in `dir`, each file whole or not at all. It outlives an interrupt,
/// as what it runs may take time to stop and its exit code is to be told.
/// It clears the screen first: the line that started it is long, and the
/// terminal may be narrow.
pub fn script(dir: &Path, i: usize, check: &CheckRun, slug: &str) -> String {
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
    format!(
        "trap : INT TERM\n{pid}printf '\\033[H\\033[2J%s\\n' {head}\nGERDR_FEATURE={slug}\nexport GERDR_FEATURE\n\
         if cd {cwd}; then\n  sh -c {command} </dev/null\n  code=$?\nelse\n  code=127\nfi\n\
         if [ \"$code\" -eq 0 ]; then\n  printf '%s\\n' {passed}\nelse\n  printf '%s\\n' \"{end}exited with $code\"\nfi\n{exit}",
        pid = leave("$$", "pid"),
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
    /// Whether pane `pane` is gone.
    fn gone(&mut self, pane: &str) -> bool;
    /// What pane `pane` shows, its last lines, those it wrapped joined.
    fn text(&mut self, pane: &str) -> Option<String>;
    /// Press Ctrl+C in pane `pane`.
    fn interrupt(&mut self, pane: &str);
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
}

/// Wait for the checks `waited` of the run in `dir`, each for at most
/// `timeout`; `ended` hears of each as it ends. `stops[index]` cuts one
/// short, and is looked at all along.
pub fn watch(
    waited: Vec<Waited>,
    dir: &Path,
    timeout: Duration,
    stops: &[AtomicBool],
    panes: &mut dyn Panes,
    ended: &mut dyn FnMut(usize, Outcome),
) {
    let told = Instant::now();
    let mut asked = Instant::now();
    let mut waiting: Vec<Waiting> = waited
        .into_iter()
        .map(|check| Waiting {
            check,
            cut: None,
            killed: false,
        })
        .collect();
    while !waiting.is_empty() {
        let ask = asked.elapsed() >= PANE_POLL;
        if ask {
            asked = Instant::now();
        }
        waiting.retain_mut(|w| {
            let i = w.check.index;
            let pid = read_number(&dir.join(format!("{i}.pid")));
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
            if let Some(code) = read_number(&dir.join(format!("{i}.exit"))) {
                let secs = took(
                    &dir.join(format!("{i}.pid")),
                    &dir.join(format!("{i}.exit")),
                );
                let mut o = match (w.cut.map(|(_, cut)| cut), code) {
                    (Some(Cut::TimedOut), _) => outcome(None, None, false, timeout.as_secs()),
                    (Some(Cut::Stopped), _) | (None, INTERRUPTED) => {
                        outcome(None, Some("stopped"), true, secs)
                    }
                    _ => outcome(Some(code), None, false, secs),
                };
                o.timed_out = w.cut.is_some_and(|(_, cut)| cut == Cut::TimedOut);
                o.tail = output_tail(panes, &w.check.pane);
                // What it left running in the background goes with it, as
                // nothing else would stop it: a server would hold its port.
                if let Some(pid) = pid {
                    kill_group(pid, libc::SIGTERM);
                }
                ended(i, o);
                return false;
            }
            if ask && panes.gone(&w.check.pane) {
                if let Some(pid) = pid {
                    kill_group(pid, libc::SIGTERM);
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
                    let past = since(&dir.join(format!("{i}.pid"))) >= timeout;
                    if stop || past {
                        let cut = if stop { Cut::Stopped } else { Cut::TimedOut };
                        w.cut = Some((Instant::now(), cut));
                        panes.interrupt(&w.check.pane);
                    }
                }
                Some((at, _)) if !w.killed && at.elapsed() >= STOP_GRACE => {
                    w.killed = true;
                    // SAFETY: getpgid takes no pointers; a pid gone gives -1,
                    // which `kill_group` leaves alone.
                    kill_group(unsafe { libc::getpgid(pid) }, libc::SIGKILL);
                }
                Some((at, cut)) if w.killed && at.elapsed() >= STOP_GRACE + KILL_GRACE => {
                    let mut o = match cut {
                        Cut::Stopped => outcome(None, Some("stopped"), true, 0),
                        Cut::TimedOut => outcome(None, None, false, timeout.as_secs()),
                    };
                    o.timed_out = cut == Cut::TimedOut;
                    o.tail = output_tail(panes, &w.check.pane);
                    ended(i, o);
                    return false;
                }
                Some(_) => {}
            }
            true
        });
        if !waiting.is_empty() {
            std::thread::sleep(WATCH_POLL);
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

/// How long ago `path` was written.
fn since(path: &Path) -> Duration {
    modified(path)
        .and_then(|at| at.elapsed().ok())
        .unwrap_or_default()
}

/// How many seconds lie between writing `from` and `to`.
fn took(from: &Path, to: &Path) -> u64 {
    modified(from)
        .zip(modified(to))
        .and_then(|(from, to)| to.duration_since(from).ok())
        .map_or(0, |d| d.as_secs())
}

/// How many lines a terminal is asked for: plenty for `TAIL`.
pub const READ_LINES: usize = 1000;
/// How many times a terminal is read for a check's last line, which it may
/// not have drawn yet.
const READ_TRIES: usize = 5;

/// The last lines of what the check in `pane` printed.
fn output_tail(panes: &mut dyn Panes, pane: &str) -> String {
    let mut text = String::new();
    for try_ in 0..READ_TRIES {
        if try_ > 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
        let Some(read) = panes.text(pane) else {
            break;
        };
        text = read;
        if text.contains(END) {
            break;
        }
    }
    tail(output_of(&text).as_bytes(), false)
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

/// The last `TAIL` lines of output `raw`, made readable: colour and other
/// terminal escapes off, of a line redrawn with `\r` only its last
/// drawing, long lines cut. `cut` says `raw` starts mid-output, so its
/// first line is partial and goes.
fn tail(raw: &[u8], cut: bool) -> String {
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
    let from = lines.len().saturating_sub(TAIL);
    lines[from..].join("\n")
}

/// `text` without terminal escapes (CSI such as colours, OSC such as
/// titles and links, two-character ones) or other control characters
/// than newline, carriage return and tab.
fn strip_escapes(text: &str) -> String {
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
        md.push_str(&format!("{how}, in {} s.\n", o.secs));
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
    /// How many failed runs in a row were sent to the Build agent to fix:
    /// gerdr stops sending after `MAX_FIXES`, and the user decides.
    #[serde(default)]
    pub fixes: u32,
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
}

impl CheckRun {
    /// What tells it from a feature's other checks.
    pub fn key(&self) -> (String, String) {
        (self.repo.clone(), self.command.clone())
    }

    /// Its repository as the user knows it.
    pub fn place(&self) -> &str {
        match self.repo.as_str() {
            FEATURE_KEY => "feature folder",
            repo => repo,
        }
    }
}

/// How many failed Verify runs in a row gerdr sends to the Build agent.
pub const MAX_FIXES: u32 = 3;

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

        fn text(&mut self, pane: &str) -> Option<String> {
            std::fs::read_to_string(self.out.get(pane)?).ok()
        }

        fn interrupt(&mut self, pane: &str) {
            kill_group(self.group[pane], libc::SIGINT);
        }
    }

    /// The rows of a run of `checks`.
    fn rows(checks: &[Check]) -> Vec<CheckRun> {
        plan(checks, &[], false, &Scope::All, 1)
    }

    /// A folder for a run's files, outside of any repository.
    fn run_folder() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gerdr-checks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
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
        watch(waited, &dir, timeout, &stops, &mut panes, &mut |i, o| {
            outcomes[i] = Some(o);
            ended(i);
        });
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
    fn what_a_run_will_do_says_only_what_its_cards_dont() {
        let t = Temp::new();
        let mut f = feature_over(&t, &["api"], &[]);
        let c = config(&[("api", &["slow", "fast"])]);
        f.skip_checks.insert("api".into(), vec!["slow".into()]);
        f.checks.insert("api".into(), vec!["mine".into()]);
        assert_eq!(
            planned(&f, &c),
            [
                "api: slow (skipped for this feature)",
                "api: fast",
                "api: mine (this feature only)"
            ]
        );
        let card = |command: &str| CheckRun {
            repo: "api".into(),
            command: command.into(),
            ..CheckRun::default()
        };
        // On a card already: what it skips still says so, even with a card
        // left from before it was skipped.
        f.verify = Some(VerifyRun {
            checks: vec![card("fast"), card("slow")],
            ..VerifyRun::default()
        });
        assert_eq!(
            planned(&f, &c),
            [
                "api: slow (skipped for this feature)",
                "api: mine (this feature only)"
            ]
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
    fn checks_run_at_the_same_time() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        // Each waits for the other: one after the other they'd time out.
        let wt = f.repos[0].worktree.display().to_string();
        let git_dir = git_run(&f.repos[0].worktree, &["rev-parse", "--absolute-git-dir"]);
        let meet = format!("{}/meet", git_dir.trim());
        let wait = |me: &str, other: &str| {
            format!("touch {meet}-{me}; until [ -e {meet}-{other} ]; do sleep 0.05; done; pwd")
        };
        let (a, b) = (wait("a", "b"), wait("b", "a"));
        let c = checks(&f, &config(&[("api", &[a.as_str(), b.as_str()])]));
        let e = run(&f, &c, Duration::from_secs(10), &mut |_| {});
        assert!(e.outcomes.iter().all(Outcome::passed), "{:?}", e.outcomes);
        assert_eq!(e.outcomes[1].tail, wt);
    }

    #[test]
    fn a_stopped_check_says_nothing_of_the_code() {
        let t = Temp::new();
        let f = feature_over(&t, &["api"], &[]);
        let c = checks(&f, &config(&[("api", &["echo up; sleep 30", "echo done"])]));
        let dir = run_folder();
        let (mut panes, waited) = start(&f, &c, &dir);
        let stops: Vec<AtomicBool> = c.iter().map(|_| AtomicBool::new(false)).collect();
        stops[0].store(true, Ordering::Relaxed);
        let mut ended = Vec::new();
        watch(waited, &dir, TIMEOUT, &stops, &mut panes, &mut |_, o| {
            ended.push(o);
        });
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
        watch(waited, &dir, TIMEOUT, &stops, &mut panes, &mut |_, o| {
            ended.push(o);
        });
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
        let mut last = plan(&listed, &[], false, &Scope::All, 1);
        for row in &mut last {
            row.pane = Some(format!("pane-{}", row.command));
            row.outcome = Some(Outcome {
                exit: Some(0),
                ..Outcome::default()
            });
        }
        let only_b = Scope::Only(vec![last[1].key()]);
        let same = plan(&listed, &last, true, &only_b, 2);
        assert!(same[0].outcome.is_some() && same[2].outcome.is_some());
        assert_eq!((same[0].run, same[2].run), (1, 1));
        assert!(same[1].outcome.is_none());
        assert_eq!(same[1].run, 2);
        assert_eq!(same[1].pane.as_deref(), Some("pane-b"));
        // On other code nothing stands.
        let other = plan(&listed, &last, false, &only_b, 2);
        assert!(
            other
                .iter()
                .all(|row| row.outcome.is_none() && row.run == 2)
        );
        assert!(other.iter().all(|row| row.pane.is_some()));
        // All of them: nothing stands either, and cards come back.
        last[0].card_off = true;
        let all = plan(&listed, &last, true, &Scope::All, 2);
        assert!(all.iter().all(|row| row.outcome.is_none() && !row.card_off));
        // A check no longer set goes, and a new one runs.
        let changed = [check("api", wt, "a"), check("api", wt, "new")];
        let row = plan(&changed, &last, true, &Scope::Only(Vec::new()), 2);
        assert_eq!(row.len(), 2);
        assert!(row[0].outcome.is_some() && row[1].outcome.is_none());
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
