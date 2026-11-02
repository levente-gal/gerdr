//! Verify: gerdr runs the project's checks on a feature's code and writes
//! what they said to `evidence.md` in the feature data dir.
//!
//! The checks come from the project's store file, by repository name or
//! `_feature` (`"workflow": {"verify": {"services-api": ["npm test"],
//! "_feature": ["./e2e.sh"]}}`): each touched repository's run in its
//! worktree, in name order, then `_feature`'s in the feature folder. An
//! untouched repository's checks don't run.
//!
//! Verify fails when:
//! - the code reads dirty, or an untouched repository has changes
//!   (`patch::Reading::dirty`, `stray`): nothing runs;
//! - a check exits non-zero or runs past the timeout; every check still
//!   runs, so the evidence is whole;
//! - the code reads differently after the checks than before: a check
//!   changed it (a formatter, a snapshot update).
//!
//! Every check runs with `sh -c`, stdin closed, for at most the timeout;
//! the evidence keeps the last `TAIL` lines of its output.
//!
//! Runs processes: call it off the UI thread (checklist D3).

use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
    Feature,
    layout::git,
    now, patch,
    setup::{Exit, minutes, shell},
};

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
    /// The repository's name, or `FEATURE_KEY`.
    pub repo: String,
    /// Where it runs: the repository's worktree, or the feature folder.
    pub dir: PathBuf,
    pub command: String,
}

/// What one check did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub repo: String,
    pub command: String,
    /// Its exit code; `None` when it was killed (timed out) or couldn't
    /// start.
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// Why it couldn't start, if it couldn't.
    pub error: Option<String>,
    /// How long it ran, in seconds.
    pub secs: u64,
    /// Its output's last lines, stdout and stderr as they came.
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
    /// Each touched repository's `HEAD` commit, by name.
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
    /// 20 min", "couldn't start: …".
    fn how(&self) -> String {
        if self.timed_out {
            format!(
                "timed out after {}",
                minutes(Duration::from_secs(self.secs))
            )
        } else if let Some(why) = &self.error {
            format!("couldn't start: {why}")
        } else if let Some(code) = self.exit {
            format!("exited with {code}")
        } else {
            "ended without an exit code".into()
        }
    }
}

/// The checks `config` (by repository name or `FEATURE_KEY`) gives
/// `feature`, in the order they run.
pub fn checks(feature: &Feature, config: &BTreeMap<String, Vec<String>>) -> Vec<Check> {
    let commands = |key: &str| {
        config
            .get(key)
            .into_iter()
            .flatten()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut repos: Vec<_> = feature.touched().collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    let mut checks = Vec::new();
    for repo in repos {
        for command in commands(&repo.name) {
            checks.push(Check {
                repo: repo.name.clone(),
                dir: repo.worktree.clone(),
                command,
            });
        }
    }
    for command in commands(FEATURE_KEY) {
        checks.push(Check {
            repo: FEATURE_KEY.into(),
            dir: feature.dir.clone(),
            command,
        });
    }
    checks
}

/// Run `checks` on `feature`'s code, each for at most `timeout`;
/// `progress` is told what runs: "services-api: npm test (2 of 5)".
pub fn run(
    feature: &Feature,
    checks: &[Check],
    timeout: Duration,
    progress: &mut dyn FnMut(String),
) -> Evidence {
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
            return evidence;
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
        return evidence;
    }
    let mut touched: Vec<_> = feature.touched().collect();
    touched.sort_by(|a, b| a.name.cmp(&b.name));
    for repo in touched {
        match git(&repo.worktree, &["rev-parse", "HEAD"]) {
            Ok(head) => evidence
                .heads
                .push((repo.name.clone(), head.trim().to_owned())),
            Err(err) => {
                evidence
                    .problems
                    .push(format!("couldn't read {}'s HEAD: {err}", repo.name));
                return evidence;
            }
        }
    }
    let total = checks.len();
    for (i, check) in checks.iter().enumerate() {
        progress(format!(
            "{}: {} ({} of {total})",
            check.repo,
            check.command,
            i + 1
        ));
        evidence
            .outcomes
            .push(run_one(check, &feature.slug, timeout));
    }
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
            if after.patch != before.patch {
                evidence
                    .problems
                    .push("the checks changed the code in the feature's commits".into());
            }
        }
    }
    evidence
}

/// How many bytes of a check's output are kept while it runs: plenty
/// for `TAIL` lines, however much it prints.
const KEEP: usize = 256 * 1024;

/// How long a check's output may run on after it ended, as when
/// something it started still holds it open.
const DRAIN: Duration = Duration::from_secs(2);

/// Run `check`, its stdout and stderr into one pipe, keeping the end.
fn run_one(check: &Check, slug: &str, timeout: Duration) -> Outcome {
    let started = Instant::now();
    let mut outcome = Outcome {
        repo: check.repo.clone(),
        command: check.command.clone(),
        ..Outcome::default()
    };
    let (exit, raw, cut) = match std::io::pipe() {
        Err(err) => (
            Exit::Failed(format!("couldn't make a pipe: {err}")),
            Vec::new(),
            false,
        ),
        Ok((mut reader, writer)) => {
            let kept = Arc::new(Mutex::new((Vec::new(), false)));
            let (done, ended) = mpsc::channel();
            let into = Arc::clone(&kept);
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut kept = into.lock().unwrap_or_else(|e| e.into_inner());
                            kept.0.extend_from_slice(&chunk[..n]);
                            if kept.0.len() > KEEP {
                                let over = kept.0.len() - KEEP;
                                kept.0.drain(..over);
                                kept.1 = true;
                            }
                        }
                    }
                }
                // The other end gave up waiting (`DRAIN`): nobody to tell.
                let _ = done.send(());
            });
            let exit = match writer.try_clone() {
                Ok(err) => shell(
                    &check.command,
                    &check.dir,
                    slug,
                    Stdio::from(writer),
                    Stdio::from(err),
                    timeout,
                ),
                Err(err) => Exit::Failed(format!("couldn't make a pipe: {err}")),
            };
            // A timeout means something the check started still holds the
            // pipe: what was read so far is kept, and the check is done.
            let _ = ended.recv_timeout(DRAIN);
            let kept = kept.lock().unwrap_or_else(|e| e.into_inner());
            (exit, kept.0.clone(), kept.1)
        }
    };
    outcome.secs = started.elapsed().as_secs();
    match exit {
        Exit::Code(code) => outcome.exit = Some(code),
        Exit::TimedOut(after) => {
            outcome.timed_out = true;
            outcome.secs = after.as_secs();
        }
        Exit::Failed(why) => outcome.error = Some(why),
    }
    outcome.tail = tail(&raw, cut);
    outcome
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
}

/// How many failed Verify runs in a row gerdr sends to the Build agent.
pub const MAX_FIXES: u32 = 3;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::feature::tests::{Temp, commit, feature_over, run as git_run};

    fn config(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.iter().map(|c| (*c).into()).collect()))
            .collect()
    }

    fn check(repo: &str, dir: &Path, command: &str) -> Check {
        Check {
            repo: repo.into(),
            dir: dir.to_owned(),
            command: command.into(),
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
        assert!(checks(&f, &BTreeMap::new()).is_empty());
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
        let mut said = Vec::new();
        let e = run(&f, &checks(&f, &c), TIMEOUT, &mut |s| said.push(s));
        assert_eq!(said.len(), 3);
        assert_eq!(said[1], "web: echo broke >&2; exit 2 (2 of 3)");
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
