//! The Build and Verify stages of features, run by gerdr.
//!
//! Setup: once a feature's worktrees are made, the store file's setup
//! commands run off the thread (see `feature::setup`), shown in the tasks
//! panel. Starting the Build agent while the setup failed or never ran
//! runs it first, and starts the agent once it is done.
//!
//! Build: its agent (see `feature_agents`) builds the plan's milestones
//! one at a time in the touched worktrees, commits each, ticks it in
//! `build.md`, and stops: the feature waits at a milestone gate until the
//! user lets it go on (`continue_stage_agent`). With every milestone
//! ticked and the agent done, gerdr reads the code: nothing uncommitted,
//! no change in an untouched repository, at least one commit. Then gerdr
//! marks Build done on that code and runs Verify.
//!
//! Verify: the store file's checks (see `feature::verify`) run off the
//! thread; `evidence.md` is written either way. Passing marks Verify done
//! on the code the checks ran on. Failing sends the failures to the Build
//! agent, in its tab while it lives, else to a fresh one, up to
//! `verify::MAX_FIXES` times in a row; its fix commits send the feature
//! back to Build (the patch watch), and the loop goes round. A project
//! without checks keeps Verify a manual mark.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Gerdr,
    feature::{
        self, Feature, Stage, Status,
        patch::{self, Reading},
        setup::{self, SetupRun},
        store,
        verify::{self, Evidence, VerifyRun},
    },
    feature_agents::{self, Stamp, Watch},
    features::{Busy, Done, Job},
    herdr::AgentStatus,
};

/// A feature: its project and slug.
type Key = (String, String);

/// What Build and Verify keep while gerdr runs; not saved.
#[derive(Debug, Default)]
pub(crate) struct BuildState {
    /// Features whose Build agent starts once their setup ends.
    pub(crate) pending: BTreeSet<Key>,
    /// Features being set up now.
    setting_up: BTreeSet<Key>,
    /// Features whose Verify checks run now.
    verifying: BTreeSet<Key>,
    /// Features whose Build agent was sent Verify's failures to fix: the
    /// code is probed once it rests, not at the patch watch's next round.
    fixing: BTreeMap<Key, Fixing>,
}

/// A Build agent fixing Verify's failures.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fixing {
    pane: String,
    /// herdr saw it working since it was sent the failures.
    worked: bool,
}

/// Whether the Build agent's start waits for the feature's setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupFirst {
    /// Start it now.
    No,
    /// The setup runs: the agent starts once it is done.
    Wait,
    /// Run the setup, then start it.
    Run,
}

/// Whether a Build agent starting (`fresh`: Ctrl+R, or the first) waits
/// for the setup: `has_commands` whether the project gives the feature
/// any, `setup` where it stands, `running` whether it runs now. One never
/// run, or cut short by gerdr quitting, runs first; a failed one runs
/// again on a fresh start.
fn setup_first(
    has_commands: bool,
    setup: Option<&SetupRun>,
    running: bool,
    fresh: bool,
) -> SetupFirst {
    if !has_commands {
        return SetupFirst::No;
    }
    if running {
        return SetupFirst::Wait;
    }
    match setup {
        None => SetupFirst::Run,
        Some(run) if run.finished.is_none() => SetupFirst::Run,
        Some(run) if run.failed.is_some() && fresh => SetupFirst::Run,
        Some(_) => SetupFirst::No,
    }
}

/// What reading the code says of a Build agent done with every
/// milestone.
#[derive(Clone, Debug, PartialEq, Eq)]
enum BuildCheck {
    /// Touched repositories with uncommitted changes.
    Dirty(Vec<String>),
    /// Untouched repositories with changes.
    Stray(Vec<String>),
    /// No touched repository has a commit.
    NoCommits,
    Holds,
}

impl BuildCheck {
    fn of(reading: &Reading) -> Self {
        if !reading.dirty.is_empty() {
            Self::Dirty(reading.dirty.clone())
        } else if !reading.stray.is_empty() {
            Self::Stray(reading.stray.clone())
        } else if reading.commits.values().all(|n| *n == 0) {
            Self::NoCommits
        } else {
            Self::Holds
        }
    }

    /// Why Build isn't done, for the agent's line: "not ready: …".
    fn why(&self) -> String {
        match self {
            Self::Dirty(repos) => format!("uncommitted changes in {}", repos.join(", ")),
            Self::Stray(repos) => format!(
                "Build changed {}, which the feature doesn't touch",
                repos.join(", ")
            ),
            Self::NoCommits => "no commits yet".into(),
            Self::Holds => String::new(),
        }
    }

    /// What the agent is told, if it can mend it itself.
    fn nudge(&self, branch: &str) -> Option<String> {
        match self {
            Self::Dirty(repos) => Some(format!(
                "gerdr found uncommitted changes in {}. Please commit them on {branch}, then stop.",
                repos.join(", ")
            )),
            Self::NoCommits => Some(format!(
                "gerdr found no commits on {branch}. Please commit each milestone's work, then stop."
            )),
            Self::Stray(_) | Self::Holds => None,
        }
    }
}

/// What gerdr does with Verify's failures, `fixes` sent in a row so far,
/// the Build agent's tab there (`alive`) or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fix {
    /// Send them into its tab.
    Send,
    /// Start a fresh Build agent with them.
    Launch,
    /// Enough: the user decides (the Verify gate).
    Stop,
}

fn fix_step(fixes: u32, alive: bool) -> Fix {
    if fixes >= verify::MAX_FIXES {
        Fix::Stop
    } else if alive {
        Fix::Send
    } else {
        Fix::Launch
    }
}

/// What a Build agent fixing Verify's failures comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixingNext {
    Wait,
    /// It works on them.
    Worked,
    /// It is done, or gone: probe the code.
    Probe,
}

/// What the fixing agent (`worked` since it was sent them) comes to with
/// herdr's `status` for its pane, its pane there (`alive`) or not.
fn fixing_next(worked: bool, status: Option<AgentStatus>, alive: bool) -> FixingNext {
    match status {
        Some(AgentStatus::Working | AgentStatus::Blocked) => FixingNext::Worked,
        Some(AgentStatus::Idle | AgentStatus::Done) if worked => FixingNext::Probe,
        _ if !alive => FixingNext::Probe,
        _ => FixingNext::Wait,
    }
}

/// Whether Verify's checks can run on `feature` now (`running`: they run
/// already, `checks`: how many are set); else why not.
fn verify_refusal(feature: &Feature, running: bool, checks: usize) -> Result<(), String> {
    let title = &feature.title;
    if feature.status != Status::Open {
        return Err(format!("\u{201c}{title}\u{201d} isn't open"));
    }
    match feature.current() {
        Some(Stage::Verify) => {}
        _ if feature.is_done(Stage::Verify) => {
            return Err(format!("Verify of \u{201c}{title}\u{201d} is done"));
        }
        Some(current) => return Err(format!("Mark {} done first", current.label())),
        None => return Err("Nothing left to do".into()),
    }
    if running {
        return Err(format!("Verify of \u{201c}{title}\u{201d} runs already"));
    }
    if checks == 0 {
        return Err(format!(
            "No checks set for \u{201c}{title}\u{201d}: add \"workflow\": {{\"verify\": \u{2026}}} to the project's file, or mark Verify done"
        ));
    }
    Ok(())
}

/// Whether `evidence` marks Verify of `feature` done: it passed, Verify is
/// current, and the checks ran on the code Build was marked done on.
fn verify_holds(feature: &Feature, evidence: &Evidence) -> bool {
    feature.status == Status::Open
        && feature.current() == Some(Stage::Verify)
        && evidence.passed()
        && !evidence.patch.is_empty()
        && feature
            .marks
            .get(&Stage::Build)
            .and_then(|m| m.patch.as_deref())
            == Some(evidence.patch.as_str())
}

/// Whether Verify's `evidence` of `feature` is on other code than Build
/// was marked done on, with Verify current: it says nothing of that code.
fn outdated(feature: &Feature, evidence: &Evidence) -> bool {
    let build = feature
        .marks
        .get(&Stage::Build)
        .and_then(|m| m.patch.as_deref());
    feature.status == Status::Open
        && feature.current() == Some(Stage::Verify)
        && !evidence.patch.is_empty()
        && build.is_some_and(|b| b != evidence.patch)
}

/// The setup failure's words for a toast: "Setup of “T” failed: why (see
/// setup.log)".
fn setup_failed_text(title: &str, why: &str) -> String {
    format!(
        "Setup of \u{201c}{title}\u{201d} failed: {why} (see {})",
        setup::LOG
    )
}

impl Gerdr {
    // ------------------------------------------------------------ setup

    /// Whether the Build agent of feature `slug` waits for its setup (run
    /// now, or running): then it starts once the setup is done, and the
    /// user is told.
    pub(crate) fn setup_before_build(&mut self, project: &str, slug: &str, fresh: bool) -> bool {
        let Some(feature) = self.feature(project, slug) else {
            return false;
        };
        let has = self
            .projects
            .project(project)
            .is_some_and(|p| setup::has_commands(feature, &p.workflow.setup));
        let key = (project.to_owned(), slug.to_owned());
        let running = self.features.build.setting_up.contains(&key);
        let first = setup_first(has, feature.setup.as_ref(), running, fresh);
        let title = feature.title.clone();
        if first == SetupFirst::No {
            return false;
        }
        self.features.build.pending.insert(key);
        // A fresh agent takes over once the setup ends, carrying on from
        // build.md: the old agent's milestone stop is over, so the row
        // stops offering Continue meanwhile.
        if let Some(run) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&Stage::Build))
            .filter(|r| r.stop.is_some())
        {
            run.stop = None;
            self.save_feature(project, slug);
        }
        if first == SetupFirst::Run {
            self.start_setup(project, slug);
        }
        self.show_warning(format!(
            "Setting up \u{201c}{title}\u{201d}; the Build agent starts once it is done"
        ));
        true
    }

    /// Set up feature `slug`'s worktrees off the thread, if its project
    /// gives setup commands and it isn't set up or being set up.
    pub fn start_setup(&mut self, project: &str, slug: &str) {
        let Some(commands) = self
            .projects
            .project(project)
            .map(|p| p.workflow.setup.clone())
        else {
            return;
        };
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        if !setup::has_commands(&feature, &commands) {
            return;
        }
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.build.setting_up.insert(key) {
            return;
        }
        let now = feature::now();
        if let Some(f) = self.feature_mut(project, slug) {
            // A failure an earlier setup put in the problems list goes: this
            // run fixes it or reports its own.
            if let Some(old) = f.setup.as_ref().and_then(|s| s.failed.as_deref()) {
                crate::problems::forget("features", &setup_failed_text(&f.title, old));
            }
            f.setup = Some(SetupRun {
                started: now,
                ..SetupRun::default()
            });
            f.log(now, "Setup started");
            self.save_feature(project, slug);
        }
        let title = feature.title.clone();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Setup,
            repo: None,
            what: format!("Setting up \u{201c}{title}\u{201d}"),
        };
        let log = setup::log_path(&store::dir(None, project, slug));
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |tx| {
            let mut progress = |what: String| {
                // It fails only once gerdr quits: nothing is left to show
                // the progress in.
                let _ = tx.send(Done::Progress {
                    project: project.clone(),
                    slug: slug.clone(),
                    job: Job::Setup,
                    what: format!("Setting up \u{201c}{title}\u{201d}: {what}"),
                });
            };
            let result = setup::run(&feature, &commands, &log, &mut progress);
            Done::SetUp {
                project,
                slug,
                result,
            }
        });
    }

    /// The setup of feature `slug` ended: kept, and a Build agent waiting
    /// on it starts.
    pub(crate) fn setup_done(&mut self, project: &str, slug: &str, result: Result<(), String>) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.setting_up.remove(&key);
        let pending = self.features.build.pending.remove(&key);
        let now = feature::now();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let run = feature.setup.get_or_insert_with(|| SetupRun {
            started: now,
            ..SetupRun::default()
        });
        run.finished = Some(now);
        run.failed = result.as_ref().err().cloned();
        match &result {
            Ok(()) => feature.log(now, "Set up"),
            Err(why) => feature.log(now, format!("Setup failed: {why}")),
        }
        self.save_feature(project, slug);
        match &result {
            Ok(()) => self.show_success(format!("\u{201c}{title}\u{201d} is set up")),
            Err(why) => {
                let mut text = setup_failed_text(&title, why);
                crate::problems::report("features", text.clone());
                if pending {
                    text.push_str("; starting the Build agent anyway");
                }
                self.show_warning(text);
            }
        }
        if !pending {
            return;
        }
        let startable = self
            .feature(project, slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(Stage::Build));
        if startable {
            self.launch_stage_agent(project, slug, Stage::Build, &[]);
        }
    }

    // ------------------------------------------------------------ build

    /// Read the code of feature `slug` off the thread for run `n` of its
    /// Build agent, whose `build.md` has `stamp` (see
    /// `build_code_checked`).
    pub(crate) fn check_build_code(&mut self, project: &str, slug: &str, n: u32, stamp: Stamp) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(None, move |_| Done::BuildChecked {
            result: patch::read(&feature),
            project,
            slug,
            n,
            stamp,
        });
    }

    /// The code of feature `slug` was read for run `n` of its Build agent,
    /// whose `build.md` had `stamp`: Build is marked done on it if it
    /// holds, and Verify runs; else the agent is told what is missing.
    pub(crate) fn build_code_checked(
        &mut self,
        project: &str,
        slug: &str,
        n: u32,
        stamp: Stamp,
        result: Result<Reading, String>,
    ) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let key = (project.to_owned(), slug.to_owned(), Stage::Build);
        let Some(watch) = self.features.agents.get_mut(&key).filter(|w| w.n == n) else {
            return;
        };
        watch.checking = None;
        watch.refused_at = Some(std::time::Instant::now());
        if feature.status != Status::Open || feature.current() != Some(Stage::Build) {
            return;
        }
        let reading = match result {
            Ok(reading) => reading,
            Err(why) => {
                // Read again now and then; said once until it changes.
                let line = format!("couldn't read the code: {why}");
                if !refuse(watch, stamp, line) {
                    return;
                }
                tracing::warn!("reading the code of feature {project}/{slug}: {why}");
                return self.show_error(format!("Couldn't check the Build agent's code: {why}"));
            }
        };
        let check = BuildCheck::of(&reading);
        if check == BuildCheck::Holds {
            watch.refused = None;
            return self.build_done(project, slug, &reading.patch);
        }
        let why = check.why();
        if !refuse(watch, stamp, why.clone()) {
            return;
        }
        let title = &feature.title;
        tracing::warn!("feature {project}/{slug}: Build isn't done: {why}");
        let told = check
            .nudge(&feature.branch())
            .map(|line| self.tell_build_agent(&feature, &line));
        let text = match (&check, told) {
            (BuildCheck::Stray(repos), _) => format!(
                "Build of \u{201c}{title}\u{201d} changed {}, which the feature doesn't touch; undo it",
                repos.join(", ")
            ),
            (_, Some(Ok(()))) => {
                format!("Build of \u{201c}{title}\u{201d} isn't done: {why}; told the agent")
            }
            (_, Some(Err(err))) => {
                tracing::warn!("telling the Build agent of {project}/{slug}: {err}");
                format!(
                    "Build of \u{201c}{title}\u{201d} isn't done: {why}; the agent couldn't be told ({err})"
                )
            }
            (_, None) => format!("Build of \u{201c}{title}\u{201d} isn't done: {why}"),
        };
        if let Some(f) = self.feature_mut(project, slug) {
            f.log(feature::now(), format!("Build not done: {why}"));
            self.save_feature(project, slug);
        }
        self.show_warning(text);
    }

    /// Send `line` to the Build agent's tab of `feature`, if it is there.
    fn tell_build_agent(&mut self, feature: &Feature, line: &str) -> Result<(), String> {
        let pane = feature
            .runs
            .get(&Stage::Build)
            .and_then(|r| r.pane.clone())
            .filter(|p| self.herdr_pane_place(p).is_some())
            .ok_or_else(|| String::from("its tab is gone"))?;
        match self.herdr.as_mut() {
            Some(herdr) => herdr.send_line(&pane, line),
            None => Err("herdr isn't connected".into()),
        }
    }

    /// The Build of feature `slug` holds on the code `patch`: marked done
    /// by gerdr, and Verify runs.
    fn build_done(&mut self, project: &str, slug: &str, patch: &str) {
        let now = feature::now();
        let config = self
            .projects
            .project(project)
            .map(|p| p.workflow.verify.clone())
            .unwrap_or_default();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        if feature.status != Status::Open || feature.current() != Some(Stage::Build) {
            return;
        }
        feature.tick(now, "gerdr", Some(patch));
        if let Some(run) = feature.runs.get_mut(&Stage::Build) {
            run.ready = Some(now);
            run.stop = None;
            run.nudged = false;
        }
        feature.log(now, "Build done: every milestone ticked and committed");
        let title = feature.title.clone();
        let checks = verify::checks(feature, &config).len();
        self.save_feature(project, slug);
        self.feature_cursor_to_current_of(project, slug);
        if checks == 0 {
            return self.show_warning(format!(
                "Build of \u{201c}{title}\u{201d} is done; no checks are set: add \"workflow\": {{\"verify\": \u{2026}}} to the project's file, or mark Verify done"
            ));
        }
        self.show_success(format!(
            "Build of \u{201c}{title}\u{201d} is done; verifying"
        ));
        // Checks running already run again on this code once they end.
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.build.verifying.contains(&key) {
            self.run_verify(project, slug);
        }
    }

    /// Let `stage`'s agent of feature `slug` go on after a milestone stop.
    pub fn continue_stage_agent(&mut self, project: &str, slug: &str, stage: Stage) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let title = &feature.title;
        let run = feature
            .runs
            .get(&stage)
            .filter(|_| feature.status == Status::Open && feature.current() == Some(stage));
        let Some((run, stop)) = run.and_then(|r| Some((r.clone(), r.stop?))) else {
            return self.show_warning(format!(
                "The {} agent of \u{201c}{title}\u{201d} isn't stopped at a milestone",
                stage.label()
            ));
        };
        let alive = run
            .pane
            .as_deref()
            .is_some_and(|p| self.herdr_pane_place(p).is_some());
        let mut sent = Err(String::from("its tab is gone"));
        if alive {
            let data = store::dir(None, project, slug);
            let project_name = self
                .projects
                .project(project)
                .map_or_else(|| project.to_owned(), |p| p.name.clone());
            let values = feature_agents::vars(&feature, &project_name, &data, stage);
            sent = crate::prompts::render_builtin("continue", &values)
                .map_err(|e| e.to_string())
                .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        }
        if let Err(why) = sent {
            if alive {
                tracing::warn!("letting the Build agent of {project}/{slug} go on: {why}");
            }
            // A fresh agent goes on from build.md.
            self.launch_stage_agent(project, slug, stage, &[]);
            return;
        }
        let now = feature::now();
        if let Some(r) = self
            .feature_mut(project, slug)
            .and_then(|f| f.runs.get_mut(&stage))
        {
            r.stop = None;
            r.nudged = false;
        }
        if let Some(f) = self.feature_mut(project, slug) {
            f.log(
                now,
                format!(
                    "{} goes on after milestone {} of {}",
                    stage.label(),
                    stop.done,
                    stop.total
                ),
            );
            self.save_feature(project, slug);
        }
        // It waits for the agent to work again.
        let key = (project.to_owned(), slug.to_owned(), stage);
        let mut watch = Watch::of(run.n);
        watch.went_on = Some(stop.done);
        self.features.agents.insert(key, watch);
        self.show_success(format!(
            "{} of \u{201c}{title}\u{201d} goes on with milestone {} of {}",
            stage.label(),
            stop.done + 1,
            stop.total
        ));
    }

    // ----------------------------------------------------------- verify

    /// Run Verify's checks on feature `slug` off the thread.
    pub fn run_verify(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let (config, timeout) = self
            .projects
            .project(project)
            .map(|p| (p.workflow.verify.clone(), p.workflow.verify_timeout))
            .unwrap_or_default();
        let checks = verify::checks(&feature, &config);
        let key = (project.to_owned(), slug.to_owned());
        let running = self.features.build.verifying.contains(&key);
        if let Err(why) = verify_refusal(&feature, running, checks.len()) {
            return self.show_warning(why);
        }
        let timeout = timeout.map_or(verify::TIMEOUT, std::time::Duration::from_secs);
        let now = feature::now();
        let last = feature.verify.as_ref();
        let n = last.map_or(1, |v| v.n + 1);
        let fixes = last.map_or(0, |v| v.fixes);
        if let Some(f) = self.feature_mut(project, slug) {
            f.verify = Some(VerifyRun {
                n,
                started: now,
                fixes,
                ..VerifyRun::default()
            });
            f.log(now, format!("Verify run {n} started"));
            self.save_feature(project, slug);
        }
        self.features.build.verifying.insert(key);
        let title = feature.title.clone();
        let busy = Busy {
            project: project.to_owned(),
            slug: slug.to_owned(),
            job: Job::Verify,
            repo: None,
            what: format!("Verifying \u{201c}{title}\u{201d}"),
        };
        let data = store::dir(None, project, slug);
        let (project, slug) = (project.to_owned(), slug.to_owned());
        self.feature_work(Some(busy), move |tx| {
            let mut progress = |what: String| {
                // It fails only once gerdr quits: nothing is left to show
                // the progress in.
                let _ = tx.send(Done::Progress {
                    project: project.clone(),
                    slug: slug.clone(),
                    job: Job::Verify,
                    what: format!("Verifying \u{201c}{title}\u{201d}: {what}"),
                });
            };
            let mut evidence = verify::run(&feature, &checks, timeout, &mut progress);
            let path = data.join(verify::EVIDENCE);
            let written = std::fs::create_dir_all(&data)
                .and_then(|()| std::fs::write(&path, verify::markdown(&feature, &evidence)));
            if let Err(err) = written {
                tracing::warn!("writing {}: {err}", path.display());
                evidence
                    .problems
                    .push(format!("couldn't write {}: {err}", verify::EVIDENCE));
            }
            Done::Verified {
                project,
                slug,
                n,
                evidence,
            }
        });
    }

    /// Run `n` of Verify's checks of feature `slug` came back with
    /// `evidence`: written, then marked done, or the failures sent to the
    /// Build agent.
    pub(crate) fn verify_done(&mut self, project: &str, slug: &str, n: u32, evidence: Evidence) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.verifying.remove(&key);
        let now = feature::now();
        let passed = evidence.passed();
        let failures = evidence.failures();
        let Some(feature) = self.feature_mut(project, slug) else {
            return;
        };
        let Some(run) = feature.verify.as_mut().filter(|v| v.n == n) else {
            return;
        };
        run.finished = Some(now);
        run.patch = Some(evidence.patch.clone()).filter(|p| !p.is_empty());
        run.passed = Some(passed);
        run.failures = failures.clone();
        let fixes = run.fixes;
        let first = failures
            .first()
            .cloned()
            .unwrap_or_else(|| "see evidence.md".into());
        if passed {
            feature.log(now, format!("Verify run {n} passed"));
        } else {
            feature.log(now, format!("Verify run {n} failed: {first}"));
        }
        let title = feature.title.clone();
        let snapshot = feature.clone();
        self.save_feature(project, slug);
        if outdated(&snapshot, &evidence) {
            // Build was marked done on other code while the checks ran:
            // they run again on it.
            tracing::info!("feature {project}/{slug}: Verify ran on older code; again");
            return self.run_verify(project, slug);
        }
        if passed {
            if !verify_holds(&snapshot, &evidence) {
                return self.show_warning(format!(
                    "\u{201c}{title}\u{201d} passed Verify, but its code couldn't be read; run the checks again"
                ));
            }
            if let Some(f) = self.feature_mut(project, slug) {
                f.tick(now, "gerdr", Some(&evidence.patch));
                if let Some(v) = f.verify.as_mut() {
                    v.fixes = 0;
                }
                self.save_feature(project, slug);
            }
            self.features.build.fixing.remove(&key);
            self.feature_cursor_to_current_of(project, slug);
            return self.show_success(format!("\u{201c}{title}\u{201d} passed Verify"));
        }
        if snapshot.status != Status::Open || snapshot.current() != Some(Stage::Verify) {
            return self.show_warning(format!("Verify of \u{201c}{title}\u{201d} failed: {first}"));
        }
        let alive = snapshot
            .runs
            .get(&Stage::Build)
            .and_then(|r| r.pane.as_deref())
            .is_some_and(|p| self.herdr_pane_place(p).is_some());
        match fix_step(fixes, alive) {
            Fix::Stop => self.show_warning(format!(
                "Verify of \u{201c}{title}\u{201d} failed again: {first}; gerdr stopped sending it to the Build agent after {fixes} fixes"
            )),
            step => self.send_fix(project, slug, &snapshot, &failures, step == Fix::Send),
        }
    }

    /// Send Verify's `failures` of `feature` to its Build agent: into its
    /// tab (`into_pane`), else to a fresh agent.
    fn send_fix(
        &mut self,
        project: &str,
        slug: &str,
        feature: &Feature,
        failures: &[String],
        into_pane: bool,
    ) {
        let title = &feature.title;
        let data = store::dir(None, project, slug);
        let project_name = self
            .projects
            .project(project)
            .map_or_else(|| project.to_owned(), |p| p.name.clone());
        let joined = failures.join("\n");
        let mut sent = Err(String::from("its tab is gone"));
        if into_pane {
            let mut values = feature_agents::vars(feature, &project_name, &data, Stage::Build);
            values.insert("evidence".into(), joined.clone());
            sent = crate::prompts::render_builtin("fix", &values)
                .map_err(|e| e.to_string())
                .and_then(|line| self.tell_build_agent(feature, line.trim()));
            if let Err(why) = &sent {
                tracing::warn!("sending Verify's failures of {project}/{slug} to its agent: {why}");
            }
        }
        let fresh = sent.is_err();
        if fresh {
            // Build stays done while Verify is current: the fix's commits
            // send the feature back to Build, and its agent's run is seen
            // as back there.
            let build_at = feature.marks.get(&Stage::Build).map(|m| m.at);
            if !self.launch_stage_agent(project, slug, Stage::Build, &[("evidence", joined)]) {
                return;
            }
            if let Some(r) = self
                .feature_mut(project, slug)
                .and_then(|f| f.runs.get_mut(&Stage::Build))
            {
                r.ready = build_at;
                self.save_feature(project, slug);
            }
        }
        let now = feature::now();
        let Some(f) = self.feature_mut(project, slug) else {
            return;
        };
        let fixes = f.verify.as_mut().map_or(0, |v| {
            v.fixes += 1;
            v.fixes
        });
        let pane = f.runs.get(&Stage::Build).and_then(|r| r.pane.clone());
        f.log(
            now,
            format!(
                "Verify's failures sent to the Build agent (fix {fixes} of {})",
                verify::MAX_FIXES
            ),
        );
        self.save_feature(project, slug);
        if let Some(pane) = pane {
            self.features.build.fixing.insert(
                (project.to_owned(), slug.to_owned()),
                Fixing {
                    pane,
                    worked: false,
                },
            );
        }
        let first = failures.first().map_or("see evidence.md", String::as_str);
        let to = if fresh {
            "a fresh Build agent"
        } else {
            "the Build agent"
        };
        self.show_warning(format!(
            "Verify of \u{201c}{title}\u{201d} failed: {first}; sent to {to} (fix {fixes} of {})",
            verify::MAX_FIXES
        ));
    }

    /// Look at the Build agents of `project` fixing Verify's failures: one
    /// that worked and rests has its code probed now.
    pub(crate) fn fixing_tick(&mut self, project: &str) {
        let keys: Vec<Key> = self
            .features
            .build
            .fixing
            .keys()
            .filter(|(p, _)| p == project)
            .cloned()
            .collect();
        for key in keys {
            let (project, slug) = (&key.0, &key.1);
            let verifying = self
                .feature(project, slug)
                .is_some_and(|f| f.status == Status::Open && f.current() == Some(Stage::Verify));
            let Some(fixing) = self.features.build.fixing.get(&key).cloned() else {
                continue;
            };
            let status = self
                .agents
                .iter()
                .find(|a| a.pane_id == fixing.pane)
                .map(|a| a.agent_status);
            let alive = status.is_some() || self.herdr_pane_place(&fixing.pane).is_some();
            if !verifying {
                self.features.build.fixing.remove(&key);
                continue;
            }
            match fixing_next(fixing.worked, status, alive) {
                FixingNext::Wait => {}
                FixingNext::Worked => {
                    if let Some(f) = self.features.build.fixing.get_mut(&key) {
                        f.worked = true;
                    }
                }
                FixingNext::Probe => {
                    self.features.build.fixing.remove(&key);
                    self.probe_feature(project, slug);
                }
            }
        }
    }
}

/// Keep `why` as what `watch` refused the file with `stamp` for; whether
/// it is new (said once until it changes).
fn refuse(watch: &mut Watch, stamp: Stamp, why: String) -> bool {
    let again = watch.refused.as_ref() == Some(&(stamp, why.clone()));
    watch.refused = Some((stamp, why));
    !again
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        feature::Mark,
        features::tests::{feature, repo},
    };

    #[test]
    fn the_setup_runs_first_when_it_never_ended_or_failed_on_a_fresh_start() {
        let started = SetupRun {
            started: 1,
            ..SetupRun::default()
        };
        let done = SetupRun {
            finished: Some(2),
            ..started.clone()
        };
        let failed = SetupRun {
            failed: Some("api: `npm ci` exited with 1".into()),
            ..done.clone()
        };
        assert_eq!(setup_first(false, None, false, false), SetupFirst::No);
        assert_eq!(setup_first(true, None, false, false), SetupFirst::Run);
        // Cut short by gerdr quitting.
        assert_eq!(
            setup_first(true, Some(&started), false, false),
            SetupFirst::Run
        );
        assert_eq!(
            setup_first(true, Some(&started), true, false),
            SetupFirst::Wait
        );
        assert_eq!(setup_first(true, Some(&done), false, true), SetupFirst::No);
        assert_eq!(
            setup_first(true, Some(&failed), false, false),
            SetupFirst::No
        );
        assert_eq!(
            setup_first(true, Some(&failed), false, true),
            SetupFirst::Run
        );
    }

    fn reading(dirty: &[&str], stray: &[&str], commits: &[(&str, usize)]) -> Reading {
        Reading {
            patch: "p1".into(),
            dirty: dirty.iter().map(|s| (*s).to_owned()).collect(),
            stray: stray.iter().map(|s| (*s).to_owned()).collect(),
            commits: commits.iter().map(|(r, n)| ((*r).to_owned(), *n)).collect(),
        }
    }

    #[test]
    fn build_holds_with_commits_and_nothing_uncommitted_or_stray() {
        let check = BuildCheck::of(&reading(&["api"], &["web"], &[("api", 0)]));
        assert_eq!(check, BuildCheck::Dirty(vec!["api".into()]));
        assert_eq!(check.why(), "uncommitted changes in api");
        assert!(
            check
                .nudge("feature/a")
                .unwrap()
                .contains("commit them on feature/a")
        );
        let check = BuildCheck::of(&reading(&[], &["web"], &[("api", 2)]));
        assert_eq!(
            check.why(),
            "Build changed web, which the feature doesn't touch"
        );
        // The user undoes it, not the agent.
        assert_eq!(check.nudge("feature/a"), None);
        let check = BuildCheck::of(&reading(&[], &[], &[("api", 0), ("db", 0)]));
        assert_eq!(check, BuildCheck::NoCommits);
        assert_eq!(check.why(), "no commits yet");
        assert!(check.nudge("feature/a").is_some());
        assert_eq!(
            BuildCheck::of(&reading(&[], &[], &[("api", 0), ("db", 1)])),
            BuildCheck::Holds
        );
    }

    #[test]
    fn the_same_refusal_is_said_once() {
        let mut w = Watch::of(1);
        assert!(refuse(&mut w, (1, 1), "no commits yet".into()));
        assert!(!refuse(&mut w, (1, 1), "no commits yet".into()));
        assert!(refuse(&mut w, (1, 1), "uncommitted changes in api".into()));
        assert!(refuse(&mut w, (2, 1), "uncommitted changes in api".into()));
    }

    #[test]
    fn failures_go_to_the_pane_else_a_fresh_agent_until_too_many() {
        assert_eq!(fix_step(0, true), Fix::Send);
        assert_eq!(fix_step(0, false), Fix::Launch);
        assert_eq!(fix_step(verify::MAX_FIXES - 1, false), Fix::Launch);
        assert_eq!(fix_step(verify::MAX_FIXES, true), Fix::Stop);
    }

    #[test]
    fn a_fixing_agent_is_probed_once_it_worked_and_rests() {
        let idle = Some(AgentStatus::Idle);
        assert_eq!(fixing_next(false, idle, true), FixingNext::Wait);
        assert_eq!(
            fixing_next(false, Some(AgentStatus::Working), true),
            FixingNext::Worked
        );
        assert_eq!(fixing_next(true, idle, true), FixingNext::Probe);
        assert_eq!(fixing_next(true, None, true), FixingNext::Wait);
        assert_eq!(fixing_next(false, None, false), FixingNext::Probe);
    }

    fn mark(patch: &str) -> Mark {
        Mark {
            at: 5,
            by: "gerdr".into(),
            note: String::new(),
            patch: Some(patch.into()),
        }
    }

    #[test]
    fn verify_runs_only_when_current_with_checks() {
        let building = feature("a", vec![repo("api", true)], 2);
        assert_eq!(
            verify_refusal(&building, false, 2),
            Err("Mark Build done first".into())
        );
        let f = feature("a", vec![repo("api", true)], 3);
        assert_eq!(verify_refusal(&f, false, 2), Ok(()));
        assert_eq!(
            verify_refusal(&f, true, 2),
            Err("Verify of \u{201c}Feature a\u{201d} runs already".into())
        );
        assert!(
            verify_refusal(&f, false, 0)
                .unwrap_err()
                .starts_with("No checks set for \u{201c}Feature a\u{201d}")
        );
        let verified = feature("a", vec![repo("api", true)], 4);
        assert_eq!(
            verify_refusal(&verified, false, 2),
            Err("Verify of \u{201c}Feature a\u{201d} is done".into())
        );
    }

    #[test]
    fn a_pass_holds_on_the_code_build_was_done_on() {
        let mut f = feature("a", vec![repo("api", true)], 3);
        f.marks.insert(Stage::Build, mark("p1"));
        let pass = Evidence {
            patch: "p1".into(),
            ..Evidence::default()
        };
        assert!(verify_holds(&f, &pass));
        assert!(!outdated(&f, &pass));
        let moved = Evidence {
            patch: "p2".into(),
            ..Evidence::default()
        };
        assert!(!verify_holds(&f, &moved));
        assert!(outdated(&f, &moved));
        let unread = Evidence::default();
        assert!(!verify_holds(&f, &unread));
        assert!(!outdated(&f, &unread));
        let failed = Evidence {
            patch: "p1".into(),
            problems: vec!["uncommitted changes in api".into()],
            ..Evidence::default()
        };
        assert!(!verify_holds(&f, &failed));
        // Back at Build: nothing to say of Verify.
        f.marks.remove(&Stage::Build);
        assert!(!verify_holds(&f, &pass) && !outdated(&f, &moved));
    }

    #[test]
    fn a_failed_setup_names_its_log() {
        assert_eq!(
            setup_failed_text("Bulk", "api: `npm ci` exited with 1"),
            "Setup of \u{201c}Bulk\u{201d} failed: api: `npm ci` exited with 1 (see setup.log)"
        );
    }
}
