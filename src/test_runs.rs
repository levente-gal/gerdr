//! Test runs in herdr's panes, shown as cards next to the agents: a pane
//! whose foreground command has "test" in its name runs tests. Once it
//! leaves the foreground, the end of the pane's text says how it went:
//! herdr reports no exit codes.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::{
    Gerdr, Workspace, agents,
    chrome::{AgentLine, AgentView},
    ci::CiState,
    herdr::{AgentStatus, PaneState, ProcessInfo, TabDetail},
    paths::{repo_root, short_path},
};

/// How many lines at the end of a pane's text are read for how a run went.
const READ_LINES: usize = 400;

/// A test run seen in a herdr pane, running or finished.
#[derive(Clone, Debug, PartialEq)]
pub struct TestRun {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// The process group leader that runs it.
    pub pid: i32,
    /// Its command line, and the words of it that name it (`cargo test`).
    pub command: String,
    pub short: String,
    pub dir: Option<PathBuf>,
    /// Its folder, as a card shows it.
    pub folder: Option<String>,
    pub started: Instant,
    /// How it went, once finished.
    pub outcome: Option<Outcome>,
    /// Whether the user has seen it since it finished.
    pub seen: bool,
}

/// How a finished run went.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// `Passed`, `Failed`, or `Unknown` when its text doesn't say.
    pub state: CiState,
    /// How many tests passed and failed, when its text counts them.
    pub summary: Option<String>,
    pub took: Duration,
}

/// A test command in the foreground of a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub pid: i32,
    pub command: String,
    pub short: String,
}

/// `word` without a package's version, as in `t2@latest` or
/// `vitest@1.2.3`; a scope's leading `@` stays.
fn unversioned(word: &str) -> &str {
    match word.rfind('@') {
        Some(at) if at > 0 => &word[..at],
        _ => word,
    }
}

/// Whether `word` names tests: it has "test" in it, not counting a
/// package's version (`latest`).
fn testy(word: &str) -> bool {
    unversioned(word).to_lowercase().contains("test")
}

/// Whether `program` runs the script its argument names, so that a path
/// there says what is running (`python3 /usr/bin/pytest`).
fn interpreter(program: &str) -> bool {
    const NAMES: [&str; 13] = [
        "sh", "bash", "dash", "zsh", "ksh", "fish", "node", "nodejs", "bun", "deno", "ruby",
        "perl", "php",
    ];
    NAMES.contains(&program) || program.starts_with("python") || program.starts_with("pypy")
}

/// Whether `word` is a path, not a scoped package (`@playwright/test`).
fn path_like(word: &str) -> bool {
    word.contains('/') && !word.starts_with('@')
}

/// The test command `info` has in the foreground, if any: one whose
/// program has "test" in its name (`pytest`, `vitest`), or whose first or
/// second argument that isn't an option does (`cargo test`, `npm run
/// test:e2e`, `python -m pytest`, `uv run pytest`). A file, as in `nvim
/// test_foo.py`, doesn't count: an argument counts by its base name, which
/// must have no dot. Neither does what only sits in an argument: a folder
/// given to anything but an interpreter (`claude --add-dir /f/test-x`), or
/// text of several words (an agent's prompt).
pub fn test_command(info: &ProcessInfo) -> Option<Found> {
    let group = info.foreground_process_group_id?;
    if info.shell_pid == Some(group) {
        return None;
    }
    // In a pipeline the group is named after its first command.
    let leader = info
        .foreground_processes
        .iter()
        .find(|p| p.pid == group)
        .or(info.foreground_processes.first())?;
    // A title rewritten as one word, like npm's `npm test`, is split.
    let words: Vec<&str> = match leader.argv.as_slice() {
        [title] => title.split_whitespace().collect(),
        [] => leader.cmdline.split_whitespace().collect(),
        argv => argv.iter().map(String::as_str).collect(),
    };
    let base = |word: &str| word.rsplit('/').next().unwrap_or(word).to_owned();
    let (argv0, args) = words.split_first()?;
    let program = base(argv0);
    // Its words from the script an interpreter runs, if it runs one by its
    // path, absolute paths shortened to their base names.
    let command = |from: &[&str]| {
        let shown: Vec<String> = from
            .iter()
            .map(|w| {
                if w.starts_with('/') {
                    base(w)
                } else {
                    w.to_string()
                }
            })
            .collect();
        shown.join(" ")
    };
    let (short, command) = if testy(&program) {
        (program, command(&words))
    } else {
        let named: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| !a.starts_with('-'))
            .collect();
        let script_allowed = interpreter(&program);
        let at = named.iter().take(2).position(|a| {
            !a.contains(char::is_whitespace)
                && (script_allowed || !path_like(a))
                && testy(&base(a))
                && !unversioned(&base(a)).contains('.')
        })?;
        // A script run by its path (`python3 /usr/bin/pytest`) is named alone.
        if path_like(named[at]) {
            let script = args.iter().position(|a| *a == named[at]).unwrap_or(0);
            (base(named[at]), command(&args[script..]))
        } else {
            let mut short = vec![program];
            short.extend(named[..=at].iter().map(|a| a.to_string()));
            (short.join(" "), command(&words))
        }
    };
    Some(Found {
        pid: leader.pid,
        command,
        short,
    })
}

/// The number before each `word` in `line` (`212 passed; 0 failed`).
fn counted(line: &str, word: &str) -> u64 {
    let words: Vec<&str> = line.split_whitespace().collect();
    words
        .windows(2)
        .filter(|w| w[1].trim_end_matches([',', ';', '.', ')']) == word)
        .filter_map(|w| w[0].trim_start_matches('(').parse::<u64>().ok())
        .sum()
}

/// How a run of `short` went, from `text`, the end of its pane: what
/// follows the last line naming it (the prompt it was typed at), else all
/// of it. It failed if a test runner's summary says so (cargo, pytest,
/// jest, vitest, go) or the build broke, passed if one says it passed;
/// otherwise its text doesn't say. Also how many passed and failed, when
/// counted.
pub fn verdict(text: &str, short: &str) -> (CiState, Option<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let from = lines
        .iter()
        .rposition(|l| l.contains(short))
        .map_or(0, |i| i + 1);
    let (mut failed, mut passed) = (false, false);
    let (mut failures, mut passes) = (0, 0);
    for line in &lines[from..] {
        let line = line.trim();
        failed |= line.starts_with("test result: FAILED")
            || line.starts_with("FAIL")
            || line.contains("error: test failed")
            || line.contains("error: could not compile")
            || line.starts_with("npm ERR!")
            || line.starts_with("npm error");
        passed |= line.starts_with("test result: ok")
            || line.starts_with("ok ")
            || line.starts_with("ok\t")
            || line.starts_with("PASS");
        // jest and vitest count the files too, before the tests.
        if line.starts_with("Test Suites:") || line.starts_with("Test Files") {
            continue;
        }
        failures += counted(line, "failed");
        passes += counted(line, "passed");
    }
    let state = if failed || failures > 0 {
        CiState::Failed
    } else if passed || passes > 0 {
        CiState::Passed
    } else {
        CiState::Unknown
    };
    let summary = match (failures, passes) {
        (0, 0) => None,
        (0, p) => Some(format!("{p} passed")),
        (f, p) => Some(format!("{f} failed, {p} passed")),
    };
    (state, summary)
}

/// Follow the test runs in `tabs`, a round of what herdr reports: a test
/// command new to a pane starts its run (the pane's last one gives way); a
/// run whose pane went back to its prompt or on to something else is
/// finished, though how it went is left for its text to say; one whose
/// pane is gone goes. A pane herdr couldn't say what runs in changes
/// nothing, nor does a round with no tabs at all (the server may be on its
/// way down). Returns the runs that just finished, by index.
pub fn step(
    runs: &mut Vec<TestRun>,
    tabs: &HashMap<String, TabDetail>,
    now: Instant,
) -> Vec<usize> {
    if tabs.is_empty() {
        return Vec::new();
    }
    let mut panes: Vec<&PaneState> = tabs.values().flat_map(|d| &d.panes).collect();
    panes.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
    runs.retain(|r| panes.iter().any(|p| p.pane_id == r.pane_id));
    let mut finished = Vec::new();
    for pane in panes {
        let Some(info) = &pane.process else { continue };
        let found = test_command(info);
        let at = runs.iter().position(|r| r.pane_id == pane.pane_id);
        match (found, at) {
            (Some(found), Some(i)) if runs[i].pid == found.pid => {
                runs[i].tab_id = pane.tab_id.clone();
            }
            (Some(found), at) => {
                let dir = pane.foreground_cwd.clone().or_else(|| pane.cwd.clone());
                let run = TestRun {
                    pane_id: pane.pane_id.clone(),
                    workspace_id: pane.workspace_id.clone(),
                    tab_id: pane.tab_id.clone(),
                    pid: found.pid,
                    command: found.command,
                    short: found.short,
                    folder: dir.as_deref().map(|d| short_path(d, repo_root(pane))),
                    dir,
                    started: now,
                    outcome: None,
                    seen: false,
                };
                match at {
                    Some(i) => runs[i] = run,
                    None => runs.push(run),
                }
            }
            (None, Some(i)) if runs[i].outcome.is_none() => {
                runs[i].outcome = Some(Outcome {
                    state: CiState::Unknown,
                    summary: None,
                    took: now.saturating_duration_since(runs[i].started),
                });
                finished.push(i);
            }
            _ => {}
        }
    }
    finished
}

/// `took` as a card says it: `42s`, `3m 12s`, `1h 5m`.
fn took(took: Duration) -> String {
    let s = took.as_secs();
    match s {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s / 60 % 60),
    }
}

impl TestRun {
    /// Working while it runs; done once finished, until seen, then idle.
    pub fn status(&self) -> AgentStatus {
        match (&self.outcome, self.seen) {
            (None, _) => AgentStatus::Working,
            (Some(_), false) => AgentStatus::Done,
            (Some(_), true) => AgentStatus::Idle,
        }
    }

    /// What it did, once finished, as a notification says it.
    fn finished_word(&self) -> Option<&'static str> {
        Some(match self.outcome.as_ref()?.state {
            CiState::Passed => "passed",
            CiState::Failed => "failed",
            _ => "finished",
        })
    }

    /// What its card says it is on: its command while it runs (or
    /// "running" when that is all its name), then how it went and how
    /// long it took.
    fn title(&self) -> String {
        match &self.outcome {
            None if self.command == self.short => "running".into(),
            None => self.command.clone(),
            Some(outcome) => {
                let how = outcome
                    .summary
                    .clone()
                    .or(self.finished_word().map(str::to_owned))
                    .unwrap_or_default();
                format!("{how} · {}", took(outcome.took))
            }
        }
    }

    /// Its card.
    pub fn view(&self) -> AgentView {
        let field = |label: &str, value: String| AgentLine::Field(label.into(), value);
        let mut lines = vec![AgentLine::Title(self.title())];
        lines.push(field("Command", self.command.clone()));
        lines.extend(self.folder.clone().map(|f| field("Folder", f)));
        AgentView {
            key: agents::AgentKey::Test(self.pane_id.clone()),
            dir: self.dir.clone(),
            status: self.status(),
            kind: self.short.clone(),
            name: None,
            model: None,
            lines,
            closable: true,
            subagents: Vec::new(),
            inbox: false,
            test: Some(self.outcome.as_ref().map_or(CiState::Running, |o| o.state)),
            icon: crate::agent_icons::AgentIcon::Test,
        }
    }
}

impl Gerdr {
    /// Follow the test runs in what herdr just reported (see `step`), and
    /// read how each that just finished went from its pane's text. One
    /// finished in sight is seen.
    pub(crate) fn track_tests(&mut self) {
        let finished = step(&mut self.test_runs, &self.herdr_tabs, Instant::now());
        for i in finished {
            let run = &self.test_runs[i];
            let Some(herdr) = self.herdr.as_mut() else {
                break;
            };
            let text = herdr
                .pane_read(&run.pane_id, "recent", READ_LINES)
                .inspect_err(|err| tracing::debug!("herdr: {err}"));
            if let Ok(text) = text {
                let (state, summary) = verdict(&text, &run.short);
                if let Some(outcome) = self.test_runs[i].outcome.as_mut() {
                    outcome.state = state;
                    outcome.summary = summary;
                }
            }
        }
        let shown: Vec<bool> = self
            .test_runs
            .iter()
            .map(|r| self.test_run_shown(&r.pane_id))
            .collect();
        for (run, shown) in self.test_runs.iter_mut().zip(shown) {
            run.seen |= run.outcome.is_some() && shown;
        }
    }

    /// Whether the user sees herdr pane `pane_id` now.
    fn test_run_shown(&self, pane_id: &str) -> bool {
        self.host_focused
            && self
                .herdr_pane_place(pane_id)
                .is_some_and(|(w, t)| self.is_visible(w, t))
    }

    /// The cards of the test runs in workspace `ws`, by tab.
    pub(crate) fn test_cards(&self, ws: &Workspace) -> Vec<AgentView> {
        let Some(id) = ws.herdr_id() else {
            return Vec::new();
        };
        let tab_of = |pane_id: &str| {
            ws.tabs
                .iter()
                .position(|t| t.herdr_panes().any(|p| p.pane_id == pane_id))
        };
        let mut runs: Vec<_> = self
            .test_runs
            .iter()
            .filter(|r| r.workspace_id == id)
            .map(|r| (tab_of(&r.pane_id), r))
            .collect();
        runs.sort_by_key(|(tab, _)| tab.unwrap_or(usize::MAX));
        runs.into_iter().map(|(_, r)| r.view()).collect()
    }

    /// The test runs, as notifications see them.
    pub(crate) fn test_watched(&self) -> Vec<agents::Watched> {
        self.test_runs
            .iter()
            .map(|run| {
                let place = self.herdr_pane_place(&run.pane_id);
                let mut body: Vec<&str> = place
                    .map(|(w, _)| self.workspaces[w].name.as_str())
                    .into_iter()
                    .collect();
                body.extend(run.outcome.as_ref().and_then(|o| o.summary.as_deref()));
                agents::Watched {
                    key: agents::AgentKey::Test(run.pane_id.clone()),
                    status: run.status(),
                    label: run.short.clone(),
                    body: body.join(" · "),
                    seen: self.test_run_shown(&run.pane_id),
                    finished: run.finished_word(),
                }
            })
            .collect()
    }

    /// Show the pane of the test run in `pane_id`; a finished run is seen.
    pub(crate) fn focus_test_run(&mut self, pane_id: &str) {
        if !self.show_herdr_pane(pane_id) {
            return;
        }
        if let Some(run) = self.test_runs.iter_mut().find(|r| r.pane_id == pane_id) {
            run.seen |= run.outcome.is_some();
        }
    }

    /// Take the card of the test run in `pane_id` off the sidebar.
    pub(crate) fn dismiss_test(&mut self, pane_id: &str) {
        self.test_runs.retain(|r| r.pane_id != pane_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{Process, TabInfo};

    fn info(argv: &[&str]) -> ProcessInfo {
        ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(20),
            foreground_processes: vec![Process {
                pid: 20,
                name: argv[0].rsplit('/').next().unwrap().into(),
                argv: argv.iter().map(|a| a.to_string()).collect(),
                cmdline: argv.join(" "),
                cwd: None,
            }],
        }
    }

    fn short(argv: &[&str]) -> Option<String> {
        test_command(&info(argv)).map(|f| f.short)
    }

    #[test]
    fn test_commands_are_told_by_their_name() {
        assert_eq!(
            short(&["cargo", "test", "-p", "gerdr"]).as_deref(),
            Some("cargo test")
        );
        assert_eq!(short(&["npm test"]).as_deref(), Some("npm test"));
        assert_eq!(
            short(&["npm", "run", "test:e2e"]).as_deref(),
            Some("npm run test:e2e")
        );
        assert_eq!(
            short(&["/usr/bin/python3", "/usr/bin/pytest", "-x"]).as_deref(),
            Some("pytest")
        );
        assert_eq!(
            short(&["python3", "-m", "pytest"]).as_deref(),
            Some("python3 pytest")
        );
        assert_eq!(short(&["go", "test", "./..."]).as_deref(), Some("go test"));
        assert_eq!(
            short(&["/home/me/.cargo/bin/cargo-nextest", "run"]).as_deref(),
            Some("cargo-nextest")
        );
        let found = test_command(&info(&["cargo", "test", "-p", "gerdr"])).unwrap();
        assert_eq!(
            (found.pid, found.command.as_str()),
            (20, "cargo test -p gerdr")
        );
        // A script its interpreter runs is shown from the script on.
        let found = test_command(&info(&["/bin/sh", "/home/me/bin/fake-test", "12"])).unwrap();
        assert_eq!(
            (found.short.as_str(), found.command.as_str()),
            ("fake-test", "fake-test 12")
        );
        // Not tests: a file named after them, anything else, an idle shell.
        assert_eq!(short(&["nvim", "test_foo.py"]), None);
        assert_eq!(short(&["nvim", "src/tests/mod.rs"]), None);
        assert_eq!(short(&["cargo", "build"]), None);
        // Not tests: a folder or a prompt given to an agent.
        assert_eq!(
            short(&[
                "claude",
                "--add-dir",
                "/d/features/app/test-feature",
                "You are the agent for test-feature, see /d/test-feature",
            ]),
            None
        );
        assert_eq!(short(&["claude", "write a test"]), None);
        assert_eq!(short(&["nvim", "src/tests"]), None);
        // A script by a relative path, a scoped package: still tests.
        assert_eq!(
            short(&["bash", "scripts/run-tests"]).as_deref(),
            Some("run-tests")
        );
        assert_eq!(
            short(&["npx", "@playwright/test"]).as_deref(),
            Some("npx @playwright/test")
        );
        // A package's version isn't its name: `latest` names no tests.
        assert_eq!(short(&["npm", "exec", "t2@latest"]), None);
        assert_eq!(short(&["npm exec t2@latest"]), None);
        assert_eq!(
            short(&["npx", "vitest@1.2.3", "run"]).as_deref(),
            Some("npx vitest@1.2.3")
        );
        let mut idle = info(&["cargo", "test"]);
        idle.foreground_process_group_id = Some(10);
        assert_eq!(test_command(&idle), None);
    }

    #[test]
    fn cargo_runs_say_how_they_went() {
        let ok = "❯ cargo test\n   Compiling gerdr\nrunning 3 tests\ntest a ... ok\n\
                  test result: ok. 3 passed; 0 failed; 0 ignored\n\n\
                  test result: ok. 2 passed; 0 failed; 0 ignored\n❯ ";
        assert_eq!(
            verdict(ok, "cargo test"),
            (CiState::Passed, Some("5 passed".into()))
        );
        let failed = "❯ cargo test\ntest a ... FAILED\nfailures:\n\
                      test result: FAILED. 1 passed; 2 failed; 0 ignored\n\
                      error: test failed, to rerun pass `--bin gerdr`\n❯ ";
        assert_eq!(
            verdict(failed, "cargo test"),
            (CiState::Failed, Some("2 failed, 1 passed".into()))
        );
        let broken = "❯ cargo test\nerror[E0308]: mismatched types\n\
                      error: could not compile `gerdr` (bin \"gerdr\" test)\n❯ ";
        assert_eq!(verdict(broken, "cargo test"), (CiState::Failed, None));
    }

    #[test]
    fn only_the_last_run_counts() {
        let text = "❯ cargo test\ntest result: FAILED. 1 passed; 2 failed\n\
                    ❯ cargo test\ntest result: ok. 3 passed; 0 failed\n❯ ";
        assert_eq!(
            verdict(text, "cargo test"),
            (CiState::Passed, Some("3 passed".into()))
        );
    }

    #[test]
    fn other_runners_say_how_they_went() {
        let pytest = "$ pytest\n==== test session starts ====\nplatform linux, pytest-8.0\n\
                      tests/test_a.py .F.\n==== 1 failed, 2 passed in 0.12s ====\n$ ";
        assert_eq!(
            verdict(pytest, "pytest"),
            (CiState::Failed, Some("1 failed, 2 passed".into()))
        );
        let jest = "$ npm test\n> jest\nPASS src/a.test.js\n\
                    Test Suites: 2 passed, 2 total\nTests:       7 passed, 7 total\n$ ";
        assert_eq!(
            verdict(jest, "npm test"),
            (CiState::Passed, Some("7 passed".into()))
        );
        let go = "$ go test ./...\nok  \tgerdr/a\t0.01s\n--- FAIL: TestB\nFAIL\tgerdr/b\t0.02s\n$ ";
        assert_eq!(verdict(go, "go test"), (CiState::Failed, None));
        let quiet = "$ make test\nall good\n$ ";
        assert_eq!(verdict(quiet, "make test"), (CiState::Unknown, None));
    }

    fn pane(id: &str, process: Option<ProcessInfo>) -> PaneState {
        let mut pane: PaneState = serde_json::from_value(serde_json::json!({
            "pane_id": id,
            "terminal_id": "t",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "cwd": "/work/gerdr",
            "foreground_cwd": null,
            "agent": null,
            "terminal_title": null,
            "terminal_title_stripped": null,
            "scroll": null,
        }))
        .unwrap();
        pane.process = process;
        pane
    }

    fn round(panes: Vec<PaneState>) -> HashMap<String, TabDetail> {
        let tab: TabInfo = serde_json::from_value(serde_json::json!({"tab_id": "w1:t1"})).unwrap();
        HashMap::from([(
            "w1:t1".into(),
            TabDetail {
                tab,
                index: 0,
                panes,
            },
        )])
    }

    #[test]
    fn runs_start_finish_give_way_and_go() {
        let start = Instant::now();
        let later = start + Duration::from_secs(42);
        let mut runs = Vec::new();
        let testing = info(&["cargo", "test"]);
        let mut idle = testing.clone();
        idle.foreground_process_group_id = Some(10);

        assert!(
            step(
                &mut runs,
                &round(vec![pane("p1", Some(testing.clone()))]),
                start
            )
            .is_empty()
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status(), AgentStatus::Working);
        // Still running; then back at the prompt: finished.
        assert!(
            step(
                &mut runs,
                &round(vec![pane("p1", Some(testing.clone()))]),
                later
            )
            .is_empty()
        );
        assert_eq!(
            step(
                &mut runs,
                &round(vec![pane("p1", Some(idle.clone()))]),
                later
            ),
            [0]
        );
        assert_eq!(
            runs[0].outcome.as_ref().unwrap().took,
            Duration::from_secs(42)
        );
        assert_eq!(runs[0].status(), AgentStatus::Done);
        // Finished stays, and isn't finished again; a herdr that can't
        // tell and an empty round change nothing.
        assert!(
            step(
                &mut runs,
                &round(vec![pane("p1", Some(idle.clone()))]),
                later
            )
            .is_empty()
        );
        assert!(step(&mut runs, &round(vec![pane("p1", None)]), later).is_empty());
        assert!(step(&mut runs, &HashMap::new(), later).is_empty());
        assert_eq!(runs.len(), 1);
        // A new run in the pane takes the last one's place.
        let mut again = testing.clone();
        again.foreground_process_group_id = Some(30);
        again.foreground_processes[0].pid = 30;
        step(&mut runs, &round(vec![pane("p1", Some(again))]), later);
        assert_eq!(
            (runs.len(), runs[0].pid, runs[0].outcome.is_none()),
            (1, 30, true)
        );
        // Its pane gone: it goes.
        step(&mut runs, &round(vec![pane("p2", Some(idle))]), later);
        assert!(runs.is_empty());
    }

    #[test]
    fn cards_say_how_a_run_went() {
        let mut runs = Vec::new();
        let now = Instant::now();
        step(
            &mut runs,
            &round(vec![pane("p1", Some(info(&["cargo", "test", "-q"])))]),
            now,
        );
        let card = runs[0].view();
        assert_eq!(
            (card.kind.as_str(), card.title()),
            ("cargo test", Some("cargo test -q"))
        );
        assert_eq!(card.test, Some(CiState::Running));
        assert!(card.closable && card.running());
        assert_eq!(card.close_label(), "Close pane");
        runs[0].outcome = Some(Outcome {
            state: CiState::Failed,
            summary: Some("2 failed, 1 passed".into()),
            took: Duration::from_secs(192),
        });
        let card = runs[0].view();
        assert_eq!(card.title(), Some("2 failed, 1 passed · 3m 12s"));
        assert_eq!(card.test, Some(CiState::Failed));
        assert!(card.closable && !card.running());
        assert_eq!(card.close_label(), "Dismiss");
    }
}
