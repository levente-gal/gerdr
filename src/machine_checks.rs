//! Verify's checks and a feature's setup on other machines. The other half of
//! `machine_work`: those threads do short jobs in turn, and these wait for as
//! long as a test run takes, each in a thread of its own.
//!
//! A check there is what it is here (`verify::script`, in a terminal, leaving
//! its files), only what reads those files and signals its processes goes over
//! the link (`CheckFiles`): a pid there means nothing on this machine, whose
//! `kill` never hears of it. What can't be told while the link is down decides
//! nothing, and the checks go on there. Their time runs by the clock there.
//!
//! Code goes there first, by the same fast-forward as for an agent: the checks
//! run on the commits read here. Whatever they leave uncommitted there comes
//! up in the evidence, as it does here. Nothing comes back but what they said.
//!
//! The setup there brings small ignored files like `.env` over, and runs the
//! project's commands in a terminal; environments are for those to build. That
//! terminal closes when it ends: the setup log says all, and it comes back as
//! it grows.

use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use smithay::reexports::calloop::channel;

use crate::{
    Gerdr,
    desktop::shell_quote,
    feature::{
        Feature,
        remote::{self, Look, Paths, Place, Reach, Trouble},
        setup::{self, PerRepo, Report, StepThere},
        store,
        verify::{self, Begun, CheckRun, Evidence, Files, VerifyRun},
    },
    feature_checks::not_started,
    features::{Done, Job},
    herdr::Herdr,
    machine_work::{self, open_there, qualified, split_pane},
    prepare,
};

/// How often files there are looked at.
const LOOK_EVERY: Duration = Duration::from_secs(2);
/// How long a setup waits for a machine out of reach, as it may never be back.
/// It goes on there, and the next setup closes what's left of it.
const SETUP_LOST: Duration = Duration::from_secs(2 * 60);
/// How many times the machine is asked how the code stands after the checks.
const AFTER_TRIES: u32 = 30;

/// A setup to do on a machine.
pub struct SetupAsked<'a> {
    pub feature: &'a Feature,
    pub commands: &'a PerRepo,
    pub extras: &'a PerRepo,
    /// The terminal of a setup before it, which may still be there.
    pub old: Option<&'a str>,
    /// The setup log here.
    pub log: &'a Path,
    /// Told what runs, in words.
    pub progress: &'a dyn Fn(String),
    /// Told its terminal, as soon as there is one.
    pub on_pane: &'a dyn Fn(String),
}

/// What a thread needs of a machine that is up.
#[derive(Clone, Debug)]
pub struct Remote {
    pub name: String,
    pub id: String,
    pub reach: Reach,
    /// The forward to the worker session's herdr there.
    pub socket: PathBuf,
    pub paths: Paths,
    /// The feature's data folder there.
    pub data: String,
}

/// A check run's files on another machine, as last looked at.
pub struct CheckFiles {
    reach: Reach,
    dir: String,
    name: String,
    look: Look,
}

impl Files for CheckFiles {
    fn look(&mut self) -> bool {
        let script = remote::look_script(&self.dir, "*.pid *.queued *.start *.exit");
        let said = (self.reach)
            .script(&script, &AtomicBool::new(false))
            .ok()
            .and_then(|text| Look::parse(&text));
        match said {
            Some(look) => {
                self.look = look;
                true
            }
            None => false,
        }
    }

    fn number(&self, i: usize, ext: &str) -> Option<i32> {
        self.look.number(&format!("{i}.{ext}"))
    }

    fn written(&self, i: usize, ext: &str) -> Option<SystemTime> {
        let at = self.look.written(&format!("{i}.{ext}"))?;
        Some(UNIX_EPOCH + Duration::from_secs(at))
    }

    fn age(&self, at: SystemTime) -> Duration {
        let now = UNIX_EPOCH + Duration::from_secs(self.look.now);
        now.duration_since(at).unwrap_or_default()
    }

    fn kill_group(&mut self, group: i32, signal: i32) {
        self.signal(&remote::kill_group_script(group, signal_name(signal)));
    }

    fn kill_group_of(&mut self, pid: i32, signal: i32) {
        self.signal(&remote::kill_group_of_script(pid, signal_name(signal)));
    }

    fn every(&self) -> Duration {
        LOOK_EVERY
    }

    fn out_of_reach(&self) -> String {
        format!(
            "stopped here: {} can't be reached, so it may still run there",
            self.name
        )
    }
}

impl CheckFiles {
    fn signal(&self, script: &str) {
        if let Err(trouble) = self.reach.script(script, &AtomicBool::new(false)) {
            tracing::warn!("signalling a check on {}: {}", self.name, trouble.text());
        }
    }
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGKILL => "KILL",
        _ => "TERM",
    }
}

/// What it says of the worktrees there before a run, on the commits `begun`
/// read here: anything but those, as they are, isn't what the checks are for.
fn standing_problems(said: &[remote::Standing], begun: &Begun, name: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for standing in said {
        if standing.dirty {
            problems.push(format!("uncommitted changes in {} on {name}", standing.key));
        }
        let here = begun.heads.iter().find(|(key, _)| *key == standing.key);
        if here.is_some_and(|(_, head)| *head != standing.head) {
            problems.push(format!(
                "{} on {name} has other commits than here: they didn't come back",
                standing.key
            ));
        }
    }
    problems
}

/// What it says of the worktrees there after a run on the commits `begun`
/// read here: as `verify::finish` says it of the code here.
fn changed_problems(said: &[remote::Standing], begun: &Begun, name: &str) -> Vec<String> {
    let dirty: Vec<&str> = (said.iter())
        .filter(|s| s.dirty)
        .map(|s| s.key.as_str())
        .collect();
    let mut problems = Vec::new();
    if !dirty.is_empty() {
        problems.push(format!(
            "the checks changed the code in {} on {name}",
            dirty.join(", ")
        ));
    }
    let moved = said
        .iter()
        .any(|s| (begun.heads.iter()).any(|(key, head)| *key == s.key && *head != s.head));
    if moved {
        problems.push(format!(
            "the checks changed the code in the feature's commits on {name}"
        ));
    }
    problems
}

impl Remote {
    pub fn files(&self, n: u32) -> CheckFiles {
        CheckFiles {
            reach: self.reach.clone(),
            dir: self.run_dir(n),
            name: self.name.clone(),
            look: Look::default(),
        }
    }

    fn run_dir(&self, n: u32) -> String {
        verify::run_dir(Path::new(&self.data), n)
            .to_string_lossy()
            .into_owned()
    }

    /// Put the code there and start the checks of `run` yet to run. A row that
    /// couldn't start gets its outcome. What keeps them all from running on
    /// that code is the evidence to go with: then none did.
    pub fn open_checks(
        &self,
        feature: &Feature,
        (project, slug, n): (&str, &str, u32),
        run: &mut VerifyRun,
        begun: &Begun,
        tx: &channel::Sender<Done>,
    ) -> Option<Evidence> {
        let stop = AtomicBool::new(false);
        let pending: Vec<usize> = (0..run.checks.len())
            .filter(|i| run.checks[*i].outcome.is_none())
            .collect();
        let progress = |what: String| {
            // As for `CheckEnded`: it fails only once gerdr quits.
            let _ = tx.send(Done::Progress {
                project: project.to_owned(),
                slug: slug.to_owned(),
                job: Job::Verify,
                what: format!("Verifying \u{201c}{}\u{201d}: {what}", feature.title),
            });
        };
        let tell = |panes: Vec<(usize, Result<String, String>)>| {
            let _ = tx.send(Done::ChecksOpened {
                project: project.to_owned(),
                slug: slug.to_owned(),
                n,
                panes,
            });
        };
        if !pending.is_empty() {
            progress(format!("bringing the code to {}", self.name));
        }
        let placed = match pending.is_empty() {
            true => Ok(Vec::new()),
            false => self.place_code(feature, &stop),
        };
        let said = match placed {
            Ok(said) => said,
            Err(trouble) => {
                let why = format!("couldn't run on {}: {}", self.name, trouble.text());
                tracing::warn!("{project}/{slug}: {why}");
                for i in pending {
                    let row = &mut run.checks[i];
                    let outcome = not_started(row, why.clone());
                    row.outcome = Some(outcome.clone());
                    let _ = tx.send(Done::CheckEnded {
                        project: project.to_owned(),
                        slug: slug.to_owned(),
                        n,
                        index: i,
                        outcome,
                    });
                }
                tell(Vec::new());
                return None;
            }
        };
        let problems = standing_problems(&said, begun, &self.name);
        if !problems.is_empty() {
            return Some(Evidence {
                at: run.started,
                patch: begun.patch.clone(),
                heads: begun.heads.clone(),
                problems,
                ran_on: Some(self.name.clone()),
                ..Evidence::default()
            });
        }
        let opened = self.open_panes(project, slug, n, run, &pending, &stop);
        for (i, pane) in &opened {
            let row = &mut run.checks[*i];
            match pane {
                Ok(pane) => row.pane = Some(pane.clone()),
                Err(why) => {
                    let outcome = not_started(row, why.clone());
                    row.outcome = Some(outcome.clone());
                    let _ = tx.send(Done::CheckEnded {
                        project: project.to_owned(),
                        slug: slug.to_owned(),
                        n,
                        index: *i,
                        outcome,
                    });
                }
            }
        }
        tell(opened);
        None
    }

    /// The code and files of `feature` there, and how its worktrees stand.
    fn place_code(
        &self,
        feature: &Feature,
        stop: &AtomicBool,
    ) -> Result<Vec<remote::Standing>, Trouble> {
        if let Some(missing) = self.reach.missing("sh", stop)? {
            return Err(Trouble::Work(format!("{missing} isn't installed there")));
        }
        self.reach
            .push_code(feature, &self.paths, &self.data, stop)?;
        let local = store::dir(None, &feature.project, &feature.slug);
        self.reach.push_data(&local, &self.data, false, stop)?;
        let places: Vec<Place> = remote::places(feature, &self.paths);
        let text = self.reach.script(&remote::status_script(&places), stop)?;
        Ok(remote::parse_status(&text))
    }

    /// Write the scripts of `run`'s checks at `pending` there, and start each
    /// in a terminal. Each one's qualified pane, or why it has none.
    fn open_panes(
        &self,
        project: &str,
        slug: &str,
        n: u32,
        run: &VerifyRun,
        pending: &[usize],
        stop: &AtomicBool,
    ) -> Vec<(usize, Result<String, String>)> {
        let dir = self.run_dir(n);
        let mut scripts = format!(
            "d={dir}\nmkdir -p \"$d\" || exit 1\nfor old in \"$(dirname \"$d\")\"/*; do [ \"$old\" = \"$d\" ] || rm -rf \"$old\"; done\n",
            dir = shell_quote(&dir),
        );
        for i in pending {
            let row = self.row_there(&run.checks[*i]);
            let body = verify::script_with(Path::new(&dir), *i, &row, slug, remote::ACTIVATE);
            let path = verify::script_path(Path::new(&dir), *i);
            let path = shell_quote(&path.to_string_lossy());
            scripts.push_str(&format!(
                "cat > {path} <<'GERDR_CHECK_SCRIPT'\n{body}\nGERDR_CHECK_SCRIPT\n"
            ));
        }
        if let Err(trouble) = self.reach.script(&scripts, stop) {
            let why = format!("couldn't write its script: {}", trouble.text());
            return pending.iter().map(|i| (*i, Err(why.clone()))).collect();
        }
        let mut herdr = Herdr::on_socket(self.socket.clone());
        let mut used: Vec<String> = Vec::new();
        (pending.iter())
            .map(|i| {
                let row = &run.checks[*i];
                let cwd = self.paths.there(&row.dir);
                let script = verify::script_path(Path::new(&dir), *i);
                let line = format!("sh {}", shell_quote(&script.to_string_lossy()));
                let pane = self.terminal_for(&mut herdr, project, row, &cwd, &mut used, stop);
                let told = pane.and_then(|(pane, info)| {
                    let line = prepare::line_there(&info, &cwd, &line);
                    herdr
                        .send_shell_line(&pane, &line)
                        .map_err(Trouble::Link)
                        .map(|()| qualified(&self.id, &pane))
                });
                (*i, told.map_err(|t| t.text().to_owned()))
            })
            .collect()
    }

    /// `row` as it reads there.
    fn row_there(&self, row: &CheckRun) -> CheckRun {
        CheckRun {
            dir: self.paths.map(&row.dir),
            command: self.paths.text(&row.command),
            ..row.clone()
        }
    }

    /// A terminal for `row`: its last, at its prompt, or a new one, as the
    /// user may be at work in that one.
    fn terminal_for(
        &self,
        herdr: &mut Herdr,
        project: &str,
        row: &CheckRun,
        cwd: &str,
        used: &mut Vec<String>,
        stop: &AtomicBool,
    ) -> Result<(String, crate::herdr::ProcessInfo), Trouble> {
        let mine = (row.pane.as_deref())
            .and_then(split_pane)
            .filter(|(id, bare)| *id == self.id && !used.iter().any(|u| u == bare))
            .map(|(_, bare)| bare.to_owned());
        if let Some(pane) = mine
            && let Ok(info) = herdr.process_info(&pane)
            && prepare::step(&Ok(info.clone())) == prepare::Step::Send
        {
            used.push(pane.clone());
            return Ok((pane, info));
        }
        let (pane, _, info) = open_there(herdr, project, cwd, stop)?;
        used.push(pane.clone());
        Ok((pane, info))
    }

    /// Say in `evidence` what became of the code there. A machine that doesn't
    /// come back to say leaves that unknown: the code the checks read is here,
    /// so that doesn't fail a run that passed.
    pub fn after_checks(&self, feature: &Feature, begun: &Begun, evidence: &mut Evidence) {
        let places = remote::places(feature, &self.paths);
        let script = remote::status_script(&places);
        let stop = AtomicBool::new(false);
        for _ in 0..AFTER_TRIES {
            match self.reach.script(&script, &stop) {
                Ok(text) => {
                    let said = remote::parse_status(&text);
                    evidence
                        .problems
                        .extend(changed_problems(&said, begun, &self.name));
                    return;
                }
                Err(Trouble::Work(why)) => {
                    tracing::warn!("reading the code on {}: {why}", self.name);
                    break;
                }
                Err(Trouble::Link(_)) => std::thread::sleep(LOOK_EVERY),
            }
        }
        evidence.ran_on = Some(format!(
            "{}; it couldn't be asked afterwards whether the checks changed its code",
            self.name
        ));
    }

    /// Do what `asked` says, there.
    pub fn set_up(&self, asked: &SetupAsked) -> Report {
        let mut report = Report::default();
        let stop = AtomicBool::new(false);
        if let Err(why) = self.set_up_into(&mut report, asked, &stop) {
            let log = asked.log;
            let line = setup::failed_line(&why);
            let mut text = std::fs::read_to_string(log).unwrap_or_default();
            // The script there says so itself, once it got to run.
            if !text.trim_end().ends_with(&line) {
                text.push_str(&format!("{line}\n\n"));
                if let Err(err) = std::fs::write(log, text) {
                    tracing::warn!("writing {}: {err}", log.display());
                }
            }
            report.failed = Some(why);
        }
        report
    }

    fn set_up_into(
        &self,
        report: &mut Report,
        asked: &SetupAsked,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let SetupAsked {
            feature,
            commands,
            extras,
            old,
            log,
            progress,
            ..
        } = *asked;
        let name = &self.name;
        if let Some(parent) = log.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("couldn't make {}: {err}", parent.display()))?;
        }
        let started = setup::started_line(crate::feature::now());
        let mut text = std::fs::read_to_string(log).unwrap_or_default();
        text.push_str(&format!("{started}\n"));
        std::fs::write(log, text)
            .map_err(|err| format!("couldn't write {}: {err}", log.display()))?;
        let lost = |t: Trouble| format!("{name}: {}", t.text());
        let mut herdr = Herdr::on_socket(self.socket.clone());
        if let Some((_, bare)) = old.and_then(split_pane)
            && let Err(err) = herdr.pane_close(bare)
            && !Herdr::says_no_pane(&err)
        {
            tracing::warn!("closing the last setup's terminal on {name}: {err}");
        }
        progress(format!("bringing the code to {name}"));
        if let Some(missing) = self.reach.missing("sh", stop).map_err(lost)? {
            return Err(format!("{missing} isn't installed on {name}"));
        }
        self.reach
            .push_code(feature, &self.paths, &self.data, stop)
            .map_err(lost)?;
        let mut notes = Vec::new();
        let mut repos: Vec<_> = feature.repos.iter().collect();
        repos.sort_by_key(|r| r.key());
        for repo in repos {
            let key = repo.key();
            let worktree = self.paths.there(&repo.worktree);
            let items = setup::for_machine(repo, extras);
            let has_commands = (commands.of(repo).and_then(|c| c.get(&repo.name)))
                .is_some_and(|lines| lines.iter().any(|l| !l.trim().is_empty()));
            report
                .not_copied
                .extend((items.problems.iter()).map(|what| format!("{key}: {what}")));
            for env in items.environments {
                notes.push(format!("== {key}: left out {env}, built for this machine"));
                if !has_commands {
                    report.not_copied.push(format!(
                        "{key}: {env} (it's built for this machine: add a setup command that builds it on {name})"
                    ));
                }
            }
            if items.send.is_empty() {
                continue;
            }
            // What is there already stays, as it does here.
            let out = self
                .reach
                .script(&remote::missing_script(&worktree, &items.send), stop)
                .map_err(lost)?;
            let send: Vec<String> = out.lines().map(str::to_owned).collect();
            for item in items.send.iter().filter(|item| !send.contains(item)) {
                notes.push(format!("== {key}: kept {item}, already there"));
            }
            if send.is_empty() {
                continue;
            }
            progress(format!("{key}: copying ignored files to {name}"));
            self.reach
                .push_ignored(&repo.main, &worktree, &send, stop)
                .map_err(lost)?;
            let out = self
                .reach
                .script(&remote::unignored_script(&worktree, &send), stop)
                .map_err(lost)?;
            let left: Vec<&str> = out.lines().collect();
            for what in &left {
                report.not_copied.push(format!(
                    "{key}: {what} (git doesn't ignore it on this branch)"
                ));
            }
            let copied: Vec<&str> = (send.iter())
                .map(String::as_str)
                .filter(|item| !left.contains(item))
                .collect();
            if !copied.is_empty() {
                let list = copied.join(", ");
                notes.push(format!("== {key}: copied {list} to {name}"));
                report.copied.push(format!("{key}: {list}"));
            }
        }
        let steps =
            setup::steps_there(feature, commands, &|path| self.paths.there(path), &|line| {
                self.paths.text(line)
            });
        self.run_steps(asked, &steps, &notes, report, &mut herdr, stop)
    }

    /// Run `steps` in a terminal there, and wait for them.
    fn run_steps(
        &self,
        asked: &SetupAsked,
        steps: &[StepThere],
        notes: &[String],
        report: &mut Report,
        herdr: &mut Herdr,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let SetupAsked { feature, log, .. } = *asked;
        let name = &self.name;
        let lost = |t: Trouble| format!("{name}: {}", t.text());
        let dir = format!("{}/{}", self.data, setup::RUN_THERE);
        let log_there = format!("{}/{}", self.data, setup::LOG);
        let script_path = format!("{dir}.sh");
        let body = setup::script_there(
            &dir,
            &log_there,
            steps,
            &feature.slug,
            notes,
            remote::ACTIVATE,
        );
        let write = format!(
            "mkdir -p {data} || exit 1\nrm -rf {dir}\ncat > {path} <<'GERDR_SETUP_SCRIPT'\n{body}\nGERDR_SETUP_SCRIPT\n",
            data = shell_quote(&self.data),
            dir = shell_quote(&dir),
            path = shell_quote(&script_path),
        );
        self.reach.script(&write, stop).map_err(lost)?;
        let cwd = self.paths.there(&feature.dir);
        let (pane, _, info) = open_there(herdr, &feature.project, &cwd, stop).map_err(lost)?;
        (asked.on_pane)(qualified(&self.id, &pane));
        let line = format!("sh {}", shell_quote(&script_path));
        herdr
            .send_shell_line(&pane, &prepare::line_there(&info, &cwd, &line))
            .map_err(|err| format!("{name}: {err}"))?;
        let ended = self.wait_for_steps(
            steps,
            (&dir, &log_there, log),
            &pane,
            asked.progress,
            herdr,
            stop,
        );
        let (at, how) = ended;
        if let Err(err) = herdr.pane_close(&pane)
            && !Herdr::says_no_pane(&err)
        {
            tracing::warn!("closing the setup's terminal on {name}: {err}");
        }
        // Whatever came of it, what it wrote is worth having.
        if let Err(trouble) = self.reach.pull_file(log, &log_there, stop) {
            tracing::warn!("bringing the setup log back: {}", trouble.text());
        }
        let (ran, failed) = setup::report_there(steps, at, how);
        report.ran = ran;
        match failed {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    /// Wait for the script in `pane`: the command it got to, and how it ended.
    /// With a setup's own time for each.
    fn wait_for_steps(
        &self,
        steps: &[StepThere],
        (dir, log_there, log): (&str, &str, &Path),
        pane: &str,
        progress: &dyn Fn(String),
        herdr: &mut Herdr,
        stop: &AtomicBool,
    ) -> (usize, Result<(), String>) {
        let name = &self.name;
        let script = remote::look_script(dir, "pid step exit");
        let start = Instant::now();
        let mut at = 0;
        let mut heard = Instant::now();
        let mut pid = None;
        loop {
            let said = (self.reach.script(&script, stop).ok()).and_then(|t| Look::parse(&t));
            let Some(look) = said else {
                if heard.elapsed() >= SETUP_LOST {
                    let how = format!("{name} can't be reached, so it may still run there");
                    return (at, Err(how));
                }
                std::thread::sleep(LOOK_EVERY);
                continue;
            };
            heard = Instant::now();
            pid = look.number("pid").or(pid);
            if let Some(now) = look.number("step").and_then(|n| usize::try_from(n).ok())
                && now != at
            {
                at = now;
                if let Some(step) = at.checked_sub(1).and_then(|i| steps.get(i)) {
                    progress(format!(
                        "{}: {} ({at} of {}) on {name}",
                        step.repo,
                        step.command,
                        steps.len()
                    ));
                }
            }
            if let Err(trouble) = self.reach.pull_file(log, log_there, stop) {
                tracing::debug!("following the setup log: {}", trouble.text());
            }
            if let Some(code) = look.number("exit") {
                return match code {
                    0 => (at, Ok(())),
                    code => (at, Err(format!("exited with {code}"))),
                };
            }
            let since = look.written("step").map(|w| look.now.saturating_sub(w));
            if since.is_some_and(|s| s >= setup::TIMEOUT.as_secs()) {
                if let Some(pid) = pid {
                    let kill = remote::kill_group_of_script(pid, "KILL");
                    if let Err(trouble) = self.reach.script(&kill, stop) {
                        tracing::warn!("stopping a setup on {name}: {}", trouble.text());
                    }
                }
                return (at, Err(setup::timed_out()));
            }
            match herdr.process_info(pane) {
                Err(err) if Herdr::says_no_pane(&err) => {
                    return (at, Err("its terminal was closed".to_owned()));
                }
                _ => {}
            }
            // No pid soon after it was told means its shell never ran it.
            if pid.is_none() && start.elapsed() >= Duration::from_secs(30) {
                return (at, Err("its terminal's shell didn't start it".to_owned()));
            }
            std::thread::sleep(LOOK_EVERY);
        }
    }
}

impl Gerdr {
    /// What a thread needs of machine `id` for `feature`, if it is up.
    pub(crate) fn machine_remote(&self, id: &str, feature: &Feature) -> Option<Remote> {
        let work = self.work_of(id)?;
        let paths = self.machine_paths(id, Some(feature))?;
        let data = paths.there(&store::dir(None, &feature.project, &feature.slug));
        Some(Remote {
            name: self.machine_name(id),
            id: id.to_owned(),
            reach: work.reach(),
            socket: work.info.socket.clone(),
            paths,
            data,
        })
    }

    /// Where `feature`'s setup goes, for what it is at now: Verify's place at
    /// Verify, else Build's. `Err`: on a machine out of reach, by that name.
    pub(crate) fn setup_place(&self, feature: &Feature) -> Result<Option<String>, String> {
        let stage = match feature.current() {
            Some(crate::feature::Stage::Verify) => crate::feature::Stage::Verify,
            _ => crate::feature::Stage::Build,
        };
        match self.placed(feature, stage) {
            machine_work::Placed::Here => Ok(None),
            machine_work::Placed::There(id) => Ok(Some(id)),
            machine_work::Placed::Away(name) => Err(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::remote::Standing;

    fn standing(key: &str, head: &str, dirty: bool) -> Standing {
        Standing {
            key: key.into(),
            head: head.into(),
            dirty,
        }
    }

    fn begun() -> Begun {
        Begun {
            patch: "p".into(),
            heads: vec![("api".into(), "c1".into())],
        }
    }

    #[test]
    fn the_code_there_must_be_the_code_read_here() {
        let clean = [standing("api", "c1", false), standing("web", "z", false)];
        assert!(standing_problems(&clean, &begun(), "box").is_empty());
        let dirty = [standing("web", "z", true)];
        assert_eq!(
            standing_problems(&dirty, &begun(), "box"),
            ["uncommitted changes in web on box"]
        );
        let ahead = [standing("api", "c2", false)];
        let said = standing_problems(&ahead, &begun(), "box");
        assert!(said[0].contains("other commits"), "{said:?}");
    }

    #[test]
    fn what_checks_change_there_fails_them_as_it_does_here() {
        let same = [standing("api", "c1", false)];
        assert!(changed_problems(&same, &begun(), "box").is_empty());
        let dirty = [standing("api", "c1", true)];
        assert_eq!(
            changed_problems(&dirty, &begun(), "box"),
            ["the checks changed the code in api on box"]
        );
        let moved = [standing("api", "c2", false)];
        assert_eq!(
            changed_problems(&moved, &begun(), "box"),
            ["the checks changed the code in the feature's commits on box"]
        );
    }

    #[test]
    fn signals_keep_to_the_two_they_are_meant_to_be() {
        assert_eq!(signal_name(libc::SIGKILL), "KILL");
        assert_eq!(signal_name(libc::SIGTERM), "TERM");
    }

    #[test]
    fn files_there_read_as_the_ones_here_do() {
        let reach = Reach {
            target: "ana@box".into(),
            control: "/run/x.ctl".into(),
        };
        let mut files = CheckFiles {
            reach,
            dir: "/d".into(),
            name: "box".into(),
            look: Look::parse("1000\n0.pid\t990\t77\n0.exit\t999\t3\n").unwrap(),
        };
        assert_eq!(files.number(0, "pid"), Some(77));
        assert_eq!(files.number(0, "exit"), Some(3));
        assert_eq!(files.number(1, "pid"), None);
        let at = files.written(0, "pid").unwrap();
        assert_eq!(files.age(at), Duration::from_secs(10));
        assert!(files.out_of_reach().contains("may still run"));
        // By the clock there: one a little ahead isn't a long time ago.
        files.look = Look::parse("1000\n0.start\t1002\t\n").unwrap();
        let ahead = files.written(0, "start").unwrap();
        assert_eq!(files.age(ahead), Duration::ZERO);
    }
}
