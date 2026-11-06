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
//! Verify: the store file's checks (see `feature::verify`) run in terminals
//! (see `feature_checks`), and are waited for off the
//! thread; `evidence.md` is written either way. Passing marks Verify done
//! on the code the checks ran on. Failing waits for the user, who runs the
//! checks again or plans the fixes (see `feature_fix`); the fixes' commits
//! send the feature back to Build (the patch watch), and the loop goes
//! round. A project without checks keeps Verify a manual mark.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use crate::{
    Gerdr,
    agents::AgentKey,
    chrome::{CommandsTab, Dot},
    feature::{
        self, Feature, Stage, Status,
        patch::{self, Reading},
        setup::{self, SetupRun},
        store,
        verify::{self, Evidence, Scope, VerifyRun},
    },
    feature_agents::{self, Stamp, Watch},
    feature_checks::checks_running,
    features::{Busy, Done, Job},
    herdr::AgentStatus,
    menu::MenuAction,
    toast::Level,
};

/// A feature: its project and slug.
type Key = (String, String);

/// Why a line couldn't reach a stage's agent, its tab closed.
pub(crate) const TAB_GONE: &str = "its tab is gone";
/// Why a line couldn't reach a stage's agent, its CLI gone from its tab.
pub(crate) const AGENT_QUIT: &str = "its agent has quit";
/// Who gets fixes when the last Build agent can't.
pub(crate) const FRESH_BUILD_AGENT: &str = "a fresh Build agent";

/// How long an agent just sent work counts as running before herdr sees it
/// at work.
const FIXING_STARTS: Duration = Duration::from_secs(30);

/// What Build and Verify keep while gerdr runs; not saved.
#[derive(Debug, Default)]
pub(crate) struct BuildState {
    /// Features whose Build agent starts once their setup ends.
    pub(crate) pending: BTreeSet<Key>,
    /// Features whose Verify checks run once their setup ends.
    pending_verify: BTreeSet<Key>,
    /// Features being set up now.
    setting_up: BTreeSet<Key>,
    /// Features whose Verify checks run now.
    pub(crate) verifying: BTreeSet<Key>,
    /// Which checks the next run of a feature's runs, while its code is read.
    pub(crate) scopes: BTreeMap<Key, Scope>,
    /// What stops the running checks of a feature, by their index.
    pub(crate) stops: BTreeMap<Key, Arc<Vec<AtomicBool>>>,
    /// What stops a feature's sandbox being started; there too while it is
    /// being stopped, so one thing happens to it at a time.
    pub(crate) sandbox_stops: BTreeMap<Key, Arc<AtomicBool>>,
    /// Features whose Build agent was sent Verify's failures to fix: the
    /// code is probed once it rests, not at the patch watch's next round.
    fixing: BTreeMap<Key, Fixing>,
    /// The open fix rounds' watches (see `feature_fix`).
    pub(crate) fix_watches: BTreeMap<Key, crate::feature_fix::FixWatch>,
}

/// A Build agent fixing Verify's failures.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fixing {
    agent: AgentKey,
    /// It was seen working since it was sent the failures.
    worked: bool,
    /// When it was sent them, or last still waited for its prompt.
    since: Instant,
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

/// How many milestones the plan a prompt's `values` point at has: none if
/// it can't be read or has none, as nothing can be said from it then.
pub(crate) fn planned_milestones(values: &crate::prompts::render::Vars) -> Option<usize> {
    let plan = std::fs::read_to_string(values.get("plan.path")?).ok()?;
    let (_, total) = feature::artifacts::milestones(&plan);
    (total > 0).then_some(total)
}

/// The toast's button that opens the checks of feature `slug`.
fn set_checks(project: &str, slug: &str) -> (String, MenuAction) {
    let action = MenuAction::EditCommands {
        project: project.to_owned(),
        slug: Some(slug.to_owned()),
        tab: CommandsTab::Checks,
    };
    ("Set checks".to_owned(), action)
}

/// Write `evidence` of `feature` to its file in `data`; a file that can't be
/// written is one more thing that failed.
pub(crate) fn write_evidence(feature: &Feature, data: &Path, mut evidence: Evidence) -> Evidence {
    let path = data.join(verify::EVIDENCE);
    let written = std::fs::create_dir_all(data)
        .and_then(|()| std::fs::write(&path, verify::markdown(feature, &evidence)));
    if let Err(err) = written {
        tracing::warn!("writing {}: {err}", path.display());
        evidence
            .problems
            .push(format!("couldn't write {}: {err}", verify::EVIDENCE));
    }
    evidence
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
            "No checks for the repositories of \u{201c}{title}\u{201d}: set some, or mark Verify done"
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

/// What couldn't be copied, in words for a toast: "Setup of “T” couldn't
/// copy api: .venv (exited with 1) (see setup.log)".
fn setup_copies_text(title: &str, not_copied: &[String]) -> String {
    format!(
        "Setup of \u{201c}{title}\u{201d} couldn't copy {} (see {})",
        not_copied.join("; "),
        setup::LOG
    )
}

/// The setup failure's words for a toast: "Setup of “T” failed: why (see
/// setup.log)".
fn setup_failed_text(title: &str, why: &str) -> String {
    format!(
        "Setup of \u{201c}{title}\u{201d} failed: {why} (see {})",
        setup::LOG
    )
}

/// What a setup that ended well says it did, in words for a toast: a command
/// that ran counts even when it changed nothing.
fn setup_done_text(title: &str, report: &setup::Report) -> String {
    let what = match report.ran.len() {
        0 if report.copied.is_empty() => ": nothing was missing".to_owned(),
        0 => String::new(),
        1 => ": ran 1 command".to_owned(),
        n => format!(": ran {n} commands"),
    };
    format!("\u{201c}{title}\u{201d} is set up{what}")
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
        let has = setup::has_work(feature);
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

    /// Whether feature `slug` is being set up.
    pub(crate) fn setting_up(&self, project: &str, slug: &str) -> bool {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.setting_up.contains(&key)
    }

    /// Set up feature `slug`'s worktrees off the thread, unless it is being
    /// set up.
    pub fn start_setup(&mut self, project: &str, slug: &str) -> bool {
        self.start_setup_of(project, slug, None)
    }

    /// Whether feature `slug`'s checks are being started or run.
    fn checks_under_way(&self, project: &str, slug: &str) -> bool {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.verifying.contains(&key)
            || (self.feature(project, slug)).is_some_and(checks_running)
    }

    /// Whether feature `slug`'s checks are being started or run, or wait for
    /// its setup to end.
    pub(crate) fn checks_busy(&self, project: &str, slug: &str) -> bool {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.pending_verify.contains(&key) || self.checks_under_way(project, slug)
    }

    /// Why the code of feature `slug` can't be changed now: its checks run on
    /// it. A key can still ask, with no button showing.
    pub(crate) fn checks_refusal(&self, project: &str, slug: &str) -> Option<String> {
        let title = &self.feature(project, slug)?.title;
        if self.checks_under_way(project, slug) {
            return Some(format!(
                "The checks of \u{201c}{title}\u{201d} are running: stop them first"
            ));
        }
        self.checks_busy(project, slug).then(|| {
            format!("The checks of \u{201c}{title}\u{201d} run once its setup ends: wait for them")
        })
    }

    /// The user asks to set feature `slug` up again: it copies what is
    /// missing and runs the commands. It says so when it can't start.
    pub(crate) fn set_up_again(&mut self, project: &str, slug: &str) {
        if self.checks_under_way(project, slug)
            && let Some(why) = self.checks_refusal(project, slug)
        {
            return self.show_warning(why);
        }
        if self.start_setup(project, slug) {
            return;
        }
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let key = (project.to_owned(), slug.to_owned());
        self.show_warning(if self.features.build.setting_up.contains(&key) {
            format!("\u{201c}{title}\u{201d} is being set up")
        } else {
            format!("\u{201c}{title}\u{201d} has no worktree to set up")
        });
    }

    /// `start_setup` of its repositories `only` (`Repo::key`), one that
    /// joined later, or all. Whether a setup started.
    pub fn start_setup_of(&mut self, project: &str, slug: &str, only: Option<Vec<String>>) -> bool {
        if self.projects.project(project).is_none() {
            return false;
        }
        let Some(mut feature) = self.feature(project, slug).cloned() else {
            return false;
        };
        if let Some(only) = only {
            feature.repos.retain(|r| only.contains(&r.key()));
        }
        if !setup::has_work(&feature) {
            return false;
        }
        let commands = self.projects.workflows(&feature, |w| w.setup.clone());
        let extras = self.projects.workflows(&feature, |w| w.copy.clone());
        let venv = self.feature_venv(project, slug);
        let key = (project.to_owned(), slug.to_owned());
        if !self.features.build.setting_up.insert(key) {
            return false;
        }
        let now = feature::now();
        if let Some(f) = self.feature_mut(project, slug) {
            // A failure an earlier setup put in the problems list goes: this
            // run fixes it or reports its own.
            if let Some(old) = f.setup.as_ref() {
                if let Some(why) = old.failed.as_deref() {
                    crate::problems::forget("features", &setup_failed_text(&f.title, why));
                }
                if !old.not_copied.is_empty() {
                    let text = setup_copies_text(&f.title, &old.not_copied);
                    crate::problems::forget("features", &text);
                }
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
            let report = setup::run(&feature, &commands, &extras, &venv, &log, &mut progress);
            Done::SetUp {
                project,
                slug,
                report,
            }
        });
        true
    }

    /// The setup of feature `slug` ended: kept, and a Build agent waiting
    /// on it starts.
    pub(crate) fn setup_done(&mut self, project: &str, slug: &str, report: setup::Report) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.setting_up.remove(&key);
        let pending = self.features.build.pending.remove(&key);
        let checks = self.features.build.pending_verify.remove(&key);
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
        run.failed = report.failed.clone();
        run.copied = report.copied.clone();
        run.not_copied = report.not_copied.clone();
        run.ran = report.ran.clone();
        match &report.failed {
            None => feature.log(now, "Set up"),
            Some(why) => feature.log(now, format!("Setup failed: {why}")),
        }
        self.save_feature(project, slug);
        let log = || {
            let action = MenuAction::SetupLog(project.to_owned(), slug.to_owned());
            Some(("Show log".to_owned(), action))
        };
        if !report.not_copied.is_empty() {
            let text = setup_copies_text(&title, &report.not_copied);
            crate::problems::report("features", text.clone());
            self.toast_with(text, Level::Warning, log());
        }
        match &report.failed {
            None if report.not_copied.is_empty() => {
                self.show_success(setup_done_text(&title, &report));
            }
            None => {}
            Some(why) => {
                let mut text = setup_failed_text(&title, why);
                crate::problems::report("features", text.clone());
                if pending {
                    text.push_str("; starting the Build agent anyway");
                }
                self.toast_with(text, Level::Warning, log());
            }
        }
        if checks {
            self.run_verify(project, slug);
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
    pub(crate) fn tell_build_agent(&mut self, feature: &Feature, line: &str) -> Result<(), String> {
        let run = feature.runs.get(&Stage::Build);
        if run.is_some_and(|r| r.held) {
            return Err("its prompt wasn't sent yet".into());
        }
        if let Some(run) = run.filter(|r| r.t3.is_some()) {
            let id = run
                .t3_thread()
                .ok_or_else(|| String::from("its T3 Code draft wasn't sent yet"))?;
            if !self.t3_thread_state(id).1 {
                return Err("its T3 Code thread is gone".into());
            }
            return self.t3_send(id, line);
        }
        let pane = run
            .and_then(|r| r.pane.clone())
            .filter(|p| self.herdr_pane_place(p).is_some())
            .ok_or_else(|| String::from(TAB_GONE))?;
        self.tell_agent_pane(&pane, line)
    }

    /// Type `line` into `pane`, where a stage's agent ran, if something
    /// still runs there: a pane back at its shell's prompt would run the
    /// line as a command.
    pub(crate) fn tell_agent_pane(&mut self, pane: &str, line: &str) -> Result<(), String> {
        let Some(herdr) = self.herdr.as_mut() else {
            return Err("herdr isn't connected".into());
        };
        if crate::prepare::step(&herdr.process_info(pane)) != crate::prepare::Step::Busy {
            return Err(AGENT_QUIT.into());
        }
        herdr.send_agent_line(pane, line)
    }

    /// The Build of feature `slug` holds on the code `patch`: marked done
    /// by gerdr, and Verify runs.
    fn build_done(&mut self, project: &str, slug: &str, patch: &str) {
        let now = feature::now();
        let config = match self.feature(project, slug) {
            Some(feature) => self.projects.workflows(feature, |w| w.verify.clone()),
            None => return,
        };
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
            return self.toast_with(
                format!(
                    "Build of \u{201c}{title}\u{201d} is done; no checks for its repositories: set some, or mark Verify done"
                ),
                Level::Warning,
                Some(set_checks(project, slug)),
            );
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
        // Past the plan's milestones what's left is fixes an agent wrote in as
        // milestones: they go as the round they came from, not one by one.
        let names = self.projects.names(&feature);
        let values =
            feature_agents::vars(&feature, &names, &store::dir(None, project, slug), stage);
        if feature.fix_round.is_none()
            && feature.last_fix_round.is_some()
            && planned_milestones(&values).is_some_and(|n| stop.done as usize >= n)
        {
            self.make_fixes(project, slug);
            return;
        }
        let alive = run.agent_key().is_some() && self.stage_run_state(&run).1;
        let fresh = feature.fresh_milestones;
        let mut sent = Err(String::from(TAB_GONE));
        if fresh {
            sent = Err(String::from("each milestone gets a fresh agent"));
        } else if alive {
            sent = crate::prompts::render_builtin("continue", &values)
                .map_err(|e| e.to_string())
                .and_then(|line| self.tell_build_agent(&feature, line.trim()));
        }
        if let Err(why) = sent {
            if alive && !fresh {
                tracing::warn!("letting the Build agent of {project}/{slug} go on: {why}");
            }
            // A fresh agent goes on from build.md.
            if !self.launch_stage_agent(project, slug, stage, &[]) {
                return;
            }
            // Until it ticks more, the log still says the stop it went on from.
            let n = self
                .feature(project, slug)
                .and_then(|f| f.runs.get(&stage))
                .map_or(0, |r| r.n);
            let mut watch = Watch::of(n);
            watch.went_on = Some(stop.done);
            let key = (project.to_owned(), slug.to_owned(), stage);
            self.features.agents.insert(key, watch);
            // The one before has done its milestone: its idle chat goes,
            // or a tab would pile up per milestone. One at work stays.
            let working = matches!(
                self.stage_run_state(&run).0,
                Some(AgentStatus::Working | AgentStatus::Blocked)
            );
            if fresh
                && alive
                && !working
                && let Some(pane) = run.pane.as_deref()
            {
                self.close_herdr_pane(pane);
            }
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

    /// Run all of Verify's checks on feature `slug`.
    pub fn run_verify(&mut self, project: &str, slug: &str) {
        self.run_checks(project, slug, Scope::All);
    }

    /// Run `scope`'s checks of Verify on feature `slug`, in terminals.
    pub(crate) fn run_checks(&mut self, project: &str, slug: &str, scope: Scope) {
        let Some(feature) = self.feature(project, slug).cloned() else {
            return;
        };
        let config = self.projects.workflows(&feature, |w| w.verify.clone());
        let checks = verify::checks(&feature, &config);
        let skipped: Vec<String> = (verify::skipped(&feature, &config).iter())
            .map(verify::Check::skipped_line)
            .collect();
        let key = (project.to_owned(), slug.to_owned());
        let running = self.features.build.verifying.contains(&key);
        if let Err(why) = verify_refusal(&feature, running, checks.len()) {
            // Fine with a check to run: setting some is the way on.
            let fix = verify_refusal(&feature, running, 1).is_ok();
            let action = fix.then(|| set_checks(project, slug));
            return self.toast_with(why, Level::Warning, action);
        }
        // They run in terminals, of the project shown.
        if self.project.as_deref() != Some(project) {
            let title = &feature.title;
            return self.show_error(format!(
                "\u{201c}{title}\u{201d} is a feature of another project: switch to it to run its checks"
            ));
        }
        if self.herdr.is_none() {
            return self.show_error(
                "The checks run in herdr's terminals, and herdr isn't connected".into(),
            );
        }
        // The checks need what the setup copies. A feature made before it
        // copied anything hasn't had one.
        let setting_up = self.features.build.setting_up.contains(&key);
        let has = setup::has_work(&feature);
        let first = setup_first(has, feature.setup.as_ref(), setting_up, false);
        let waits = match first {
            SetupFirst::No => false,
            SetupFirst::Wait => true,
            SetupFirst::Run => self.start_setup(project, slug),
        };
        if waits {
            self.features.build.pending_verify.insert(key);
            let title = &feature.title;
            return self.show_warning(format!(
                "Setting up \u{201c}{title}\u{201d}; its checks run once it is done"
            ));
        }
        let now = feature::now();
        let last = feature.verify.as_ref();
        let n = last.map_or(1, |v| v.n + 1);
        // What the last run said stays until its code is read: then the
        // checks that are not run again keep it, on the same code.
        let (checks, patch) =
            last.map_or((Vec::new(), None), |v| (v.checks.clone(), v.patch.clone()));
        if let Some(f) = self.feature_mut(project, slug) {
            f.verify = Some(VerifyRun {
                n,
                started: now,
                checks,
                patch,
                skipped,
                ..VerifyRun::default()
            });
            f.log(now, format!("Verify run {n} started"));
            self.save_feature(project, slug);
        }
        self.features.build.scopes.insert(key.clone(), scope);
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
        self.feature_work(Some(busy), move |_| match verify::begin(&feature) {
            Ok(begun) => Done::ChecksBegun {
                project,
                slug,
                n,
                begun,
            },
            Err(evidence) => Done::Verified {
                project,
                slug,
                n,
                evidence: write_evidence(&feature, &data, *evidence),
            },
        });
    }

    /// Run `n` of Verify's checks of feature `slug` came back with
    /// `evidence`: written, then marked done, or the failures sent to the
    /// Build agent.
    pub(crate) fn verify_done(&mut self, project: &str, slug: &str, n: u32, evidence: Evidence) {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.verifying.remove(&key);
        self.features.build.stops.remove(&key);
        self.features.build.scopes.remove(&key);
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
        // Nothing ran on this code: the last run's say nothing of it.
        if evidence.outcomes.is_empty() {
            for check in &mut run.checks {
                check.outcome = None;
            }
        }
        // A run on older code goes again: that one will say.
        if !outdated(feature, &evidence) {
            feature::fix_round::verify_ended(&mut feature.verify_fixes, n, &evidence);
        }
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
                self.save_feature(project, slug);
            }
            self.features.build.fixing.remove(&key);
            self.feature_cursor_to_current_of(project, slug);
            return self.start_reviewer(project, slug, &title);
        }
        if snapshot.status != Status::Open || snapshot.current() != Some(Stage::Verify) {
            return self.show_warning(format!("Verify of \u{201c}{title}\u{201d} failed: {first}"));
        }
        // A check cut short says nothing of the code: no fix to ask for.
        if let Some(cut) = evidence.outcomes.iter().find(|o| o.interrupted) {
            let (command, how) = (&cut.command, cut.how());
            return self.show_warning(format!(
                "The checks of \u{201c}{title}\u{201d} didn't all run: `{command}` {how}"
            ));
        }
        self.show_warning(format!("Verify of \u{201c}{title}\u{201d} failed: {first}"));
    }

    /// Verify of feature `slug` (`title`) passed: AI review's page shows and
    /// its agent starts on it behind that, and the toast says so. A reviewer
    /// that can't start says why itself, and the panel's Enter starts it
    /// later.
    fn start_reviewer(&mut self, project: &str, slug: &str, title: &str) {
        let reviewing = self
            .feature(project, slug)
            .is_some_and(|f| f.status == Status::Open && f.current() == Some(Stage::AiReview));
        if !reviewing {
            return self.show_success(format!("\u{201c}{title}\u{201d} passed Verify"));
        }
        self.show_stage_page(project, slug);
        if !self.launch_stage_agent(project, slug, Stage::AiReview, &[]) {
            return;
        }
        let run = (self.feature(project, slug)).and_then(|f| f.runs.get(&Stage::AiReview));
        let agent = run.map_or("an agent", |r| r.agent.as_str());
        let what = match run.is_some_and(|r| r.held) {
            true => format!("{agent} waits for its prompt"),
            false => format!("{agent} is reviewing it"),
        };
        self.show_success(format!("\u{201c}{title}\u{201d} passed Verify; {what}"));
    }

    /// Watch the Build agent in `pane` of feature `slug`, sent findings or
    /// comments to fix: once it worked and rests, its code is probed (see
    /// `fixing_tick`).
    pub(crate) fn watch_fixing(&mut self, project: &str, slug: &str, agent: AgentKey) {
        self.features.build.fixing.insert(
            (project.to_owned(), slug.to_owned()),
            Fixing {
                agent,
                worked: false,
                since: Instant::now(),
            },
        );
    }

    /// The dot of `feature`'s Build agent while it runs again with Build
    /// done: just sent work, or seen at work (what was sent isn't known
    /// after a restart). A line it never acted on stops counting.
    pub(crate) fn build_agent_fixing(&self, feature: &Feature) -> Option<Dot> {
        let run = feature.runs.get(&Stage::Build).filter(|r| !r.held)?;
        if !feature.is_done(Stage::Build) || !run.live() {
            return None;
        }
        let key = (feature.project.clone(), feature.slug.clone());
        let sent = (self.features.build.fixing.get(&key))
            .is_some_and(|f| f.worked || f.since.elapsed() < FIXING_STARTS);
        match self.stage_run_state(run).0 {
            Some(status @ (AgentStatus::Working | AgentStatus::Blocked)) => {
                Some(Dot::of_status(status))
            }
            _ => sent.then_some(Dot::Working),
        }
    }

    /// The Build agent watched as fixing feature `slug`, if any, is now
    /// `agent`.
    pub(crate) fn fixing_moved(&mut self, project: &str, slug: &str, agent: AgentKey) {
        let key = (project.to_owned(), slug.to_owned());
        if let Some(fixing) = self.features.build.fixing.get_mut(&key) {
            fixing.agent = agent;
        }
    }

    /// Look at the Build agents of `project` fixing what Verify or a review
    /// found: one that worked and rests has its code probed now. Watched
    /// while Build is done and a later stage is current.
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
            let verifying = self.feature(project, slug).is_some_and(|f| {
                f.status == Status::Open
                    && f.is_done(Stage::Build)
                    && f.current().is_some_and(|c| c > Stage::Build)
            });
            let Some(fixing) = self.features.build.fixing.get(&key).cloned() else {
                continue;
            };
            let (status, alive) = self.agent_key_state(&fixing.agent);
            if !verifying {
                self.features.build.fixing.remove(&key);
                continue;
            }
            // Not fixing yet: picking its model isn't work, nor its tab
            // closing, as Start agent opens one again.
            let held = (self.feature(project, slug))
                .and_then(|f| f.runs.get(&Stage::Build))
                .is_some_and(|r| r.held);
            if held {
                // Its work starts once its prompt goes.
                if let Some(f) = self.features.build.fixing.get_mut(&key) {
                    f.since = Instant::now();
                }
                continue;
            }
            // Not seen at work in time: it was too quick to see, or it never
            // took the line. Reading its code tells either way, and a fix
            // round must not wait on it for good.
            let lost = !fixing.worked && fixing.since.elapsed() >= FIXING_STARTS;
            match fixing_next(fixing.worked, status, alive) {
                FixingNext::Wait if lost => {
                    self.features.build.fixing.remove(&key);
                    self.probe_feature(project, slug);
                }
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
    fn a_setup_says_it_ran_its_commands_even_when_nothing_was_copied() {
        let text = |copied: &[&str], ran: &[&str]| {
            let list = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
            let report = setup::Report {
                copied: list(copied),
                ran: list(ran),
                ..setup::Report::default()
            };
            setup_done_text("T", &report)
        };
        let t = "\u{201c}T\u{201d} is set up";
        assert_eq!(text(&[], &[]), format!("{t}: nothing was missing"));
        assert_eq!(text(&["api: .env"], &[]), t);
        assert_eq!(text(&[], &["api: true"]), format!("{t}: ran 1 command"));
        assert_eq!(
            text(&["api: .env"], &["api: true", "api: false"]),
            format!("{t}: ran 2 commands")
        );
    }

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
        let building = feature("a", vec![repo("api", true)], 3);
        assert_eq!(
            verify_refusal(&building, false, 2),
            Err("Mark Build done first".into())
        );
        let f = feature("a", vec![repo("api", true)], 4);
        assert_eq!(verify_refusal(&f, false, 2), Ok(()));
        assert_eq!(
            verify_refusal(&f, true, 2),
            Err("Verify of \u{201c}Feature a\u{201d} runs already".into())
        );
        assert!(
            verify_refusal(&f, false, 0)
                .unwrap_err()
                .starts_with("No checks for the repositories of \u{201c}Feature a\u{201d}")
        );
        let verified = feature("a", vec![repo("api", true)], 5);
        assert_eq!(
            verify_refusal(&verified, false, 2),
            Err("Verify of \u{201c}Feature a\u{201d} is done".into())
        );
    }

    #[test]
    fn a_pass_holds_on_the_code_build_was_done_on() {
        let mut f = feature("a", vec![repo("api", true)], 4);
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
