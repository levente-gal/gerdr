//! Restarting and stopping what runs in a terminal pane, from the bar over
//! it (see `chrome::pane_bar_rects`) or the keys (`restart-pane`,
//! `stop-pane`).
//!
//! Stopping presses Ctrl+C in the pane; if what runs there is still there
//! a while later, its process group gets SIGTERM, then SIGKILL. Restarting
//! stops it the same way, then, once the shell is back at its prompt, types
//! in the pane's last command again, in the folder and with the venv and
//! `mise` versions it ran with. That works after the command exited too:
//! each pane's last command is remembered (in memory only) apart from what
//! is saved to run again after a reboot, which is forgotten once the shell
//! has been idle a while. Programs never run again (see `projects::DENY`)
//! don't take its place.
//!
//! The steps run on a thread of their own per pane, asking herdr how the
//! pane is every `POLL`; they report back over a channel to the event loop.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, BarButton, BarHit, BarStatus, PaneBarView},
    herdr::{Herdr, PaneState, Process, ProcessInfo},
    prepare::{self, Plan, Prep, Shell, Step, Tools},
    projects::{Captured, Foreground, VenvSetting},
};

/// How often a pane is asked about while it is stopped.
const POLL: Duration = Duration::from_millis(150);
/// How long what runs in a pane has after Ctrl+C, before SIGTERM; then
/// after SIGTERM, before SIGKILL; then after SIGKILL, before giving up.
const INTERRUPT_GRACE: Duration = Duration::from_secs(3);
const TERM_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(1);
/// How long a job may take in all: a shell that never comes back to its
/// prompt (one still starting, say) isn't waited for forever.
const JOB_DEADLINE: Duration = Duration::from_secs(15);
/// How long a restarted pane's bar still says so: until herdr shows the
/// command running again.
const SETTLE: Duration = Duration::from_millis(1500);

/// A pane's last command, and the process that ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remembered {
    pub pid: i32,
    pub captured: Captured,
}

/// Bring the last command of herdr pane `pane_id` (in folder `pane_cwd`) up
/// to date with what runs in it: a new command takes the place of the last,
/// once per process (some rewrite their command line once running). An
/// idle shell, a program never run again or one whose command line can't be
/// read back keeps it.
pub fn remember(
    memory: &mut HashMap<String, Remembered>,
    pane_id: &str,
    foreground: Option<&Foreground>,
    pane_cwd: Option<&Path>,
) {
    let Some(Foreground::Busy { pid, captured }) = foreground else {
        return;
    };
    if memory.get(pane_id).is_some_and(|r| r.pid == *pid) {
        return;
    }
    let mut captured = captured.clone();
    // Always where it ran: the shell may have moved since.
    captured.cwd = captured.cwd.or_else(|| pane_cwd.map(Path::to_path_buf));
    memory.insert(
        pane_id.to_owned(),
        Remembered {
            pid: *pid,
            captured,
        },
    );
}

/// What to type in pane `pane_id` to run `captured` again: back to its
/// folder, its `mise` versions and its venv (if it ran in one), then it.
pub fn restart_prep(pane_id: &str, captured: &Captured) -> Prep {
    let dir = captured.cwd.clone().unwrap_or_default();
    let in_venv = captured.env.iter().any(|(key, _)| key == "VIRTUAL_ENV");
    Prep {
        pane_id: pane_id.to_owned(),
        pane_cwd: dir.clone(),
        ws_dir: dir,
        venv: if in_venv {
            VenvSetting::Auto
        } else {
            VenvSetting::Off
        },
        captured_env: captured.env.clone(),
        cd: captured.cwd.clone(),
        command: Some(captured.command.clone()),
    }
}

/// How a pane is, as a job sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Its shell is at its prompt.
    Prompt,
    /// Its shell isn't there yet.
    Starting,
    /// Something runs in it, in this process group.
    Running(i32),
    Gone,
}

/// How pane is, from what herdr says of it.
pub fn seen(info: &Result<ProcessInfo, String>) -> Seen {
    match prepare::step(info) {
        Step::Send => Seen::Prompt,
        Step::Wait => Seen::Starting,
        Step::Drop => Seen::Gone,
        Step::Busy => match info {
            Ok(ProcessInfo {
                foreground_process_group_id: Some(group),
                ..
            }) => Seen::Running(*group),
            _ => Seen::Starting,
        },
    }
}

/// How hard a job has asked what runs in a pane to end, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Ctrl+C.
    Interrupt,
    /// SIGTERM.
    Terminate,
    /// SIGKILL.
    Kill,
}

/// What a job does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Wait,
    /// Ask process group `.1` harder.
    Signal(Phase, i32),
    /// Type the command in again.
    Rerun,
    Done,
    /// It wouldn't end.
    GaveUp,
}

/// What a job does next, seeing the pane as `seen`, having last sent
/// `sent` (and how long ago), and whether it runs the command again.
pub fn next(seen: Seen, sent: Option<(Phase, Duration)>, rerun: bool) -> Next {
    match seen {
        Seen::Gone => Next::Done,
        Seen::Prompt if rerun => Next::Rerun,
        Seen::Prompt => Next::Done,
        Seen::Starting => Next::Wait,
        Seen::Running(group) => match sent {
            None => Next::Signal(Phase::Interrupt, group),
            Some((Phase::Interrupt, since)) if since >= INTERRUPT_GRACE => {
                Next::Signal(Phase::Terminate, group)
            }
            Some((Phase::Terminate, since)) if since >= TERM_GRACE => {
                Next::Signal(Phase::Kill, group)
            }
            Some((Phase::Kill, since)) if since >= KILL_GRACE => Next::GaveUp,
            Some(_) => Next::Wait,
        },
    }
}

/// How a job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Stopped,
    /// The command was typed in again.
    Restarted,
    /// The pane went away.
    Gone,
    /// What ran wouldn't end.
    GaveUp,
    /// The command couldn't be typed in again.
    Failed,
}

/// What a job reports, by herdr pane id.
#[derive(Debug)]
pub enum JobEvent {
    Phase(String, Phase),
    Finished(String, Outcome),
}

/// A pane being stopped or restarted.
#[derive(Debug)]
pub struct Job {
    /// Whether the command runs again once stopped: the other button flips
    /// it while the job runs.
    pub rerun: Arc<AtomicBool>,
    /// Whether there's a command to run again.
    pub can_rerun: bool,
    /// What it stops, for the bar and the notices.
    pub command: String,
    /// The last step taken.
    pub phase: Option<Phase>,
    /// When the command was typed in again.
    pub restarted: Option<Instant>,
}

/// What a pane's bar shows: see `PaneBarView`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarContent {
    pub label: String,
    pub note: Option<&'static str>,
    pub status: BarStatus,
    pub can_restart: bool,
    pub can_stop: bool,
}

/// The process leading what runs in a pane, if not its shell.
fn running_leader(info: &ProcessInfo) -> Option<&Process> {
    let group = info.foreground_process_group_id?;
    if Some(group) == info.shell_pid {
        return None;
    }
    let leader = info
        .foreground_processes
        .iter()
        .find(|p| p.pid == group)
        .or(info.foreground_processes.first())?;
    (!prepare::is_interactive_shell(leader)).then_some(leader)
}

/// What a job's bar says it is doing.
fn phase_label(rerun: bool, phase: Option<Phase>, restarted: bool) -> &'static str {
    match (restarted, rerun, phase) {
        (true, ..) => "restarting…",
        (false, _, Some(Phase::Terminate)) => "terminating…",
        (false, _, Some(Phase::Kill)) => "killing…",
        (false, true, _) => "restarting…",
        (false, false, _) => "stopping…",
    }
}

/// What the bar of a pane shows, given what runs in it (`process`, as herdr
/// last said), its last command and the job stopping it, if any.
pub fn bar_content(
    process: Option<&ProcessInfo>,
    remembered: Option<&Remembered>,
    job: Option<&Job>,
) -> BarContent {
    if let Some(job) = job {
        let rerun = job.rerun.load(Ordering::Relaxed);
        let settling = job.restarted.is_some();
        return BarContent {
            label: job.command.clone(),
            note: Some(phase_label(rerun, job.phase, settling)),
            status: BarStatus::Working,
            can_restart: !settling && !rerun && job.can_rerun,
            can_stop: !settling && rerun,
        };
    }
    if let Some(leader) = process.and_then(running_leader) {
        return BarContent {
            label: chrome::command_line(&leader.cmdline).unwrap_or_else(|| leader.name.clone()),
            note: None,
            status: BarStatus::Running,
            // Only what it remembers: the leader may be a program never run
            // again, run in front of the last command.
            can_restart: remembered.is_some_and(|r| r.pid == leader.pid),
            can_stop: true,
        };
    }
    if let Some(remembered) = remembered {
        let command = &remembered.captured.command;
        return BarContent {
            label: chrome::command_line(command).unwrap_or_else(|| command.clone()),
            note: Some("exited"),
            status: BarStatus::Exited,
            can_restart: true,
            can_stop: false,
        };
    }
    let shell = process
        .and_then(|info| info.foreground_processes.first())
        .map_or("shell", |p| p.name.trim_start_matches('-'));
    BarContent {
        label: shell.to_owned(),
        note: None,
        status: BarStatus::Idle,
        can_restart: false,
        can_stop: false,
    }
}

/// Stop what runs in herdr pane `pane_id`, on a thread of its own talking to
/// the server on `socket`, then type `line` in, written for the pane's
/// shell, if `rerun` says so by then.
/// Reports to `tx`.
fn spawn_job(
    socket: PathBuf,
    pane_id: String,
    line: Option<Plan>,
    rerun: Arc<AtomicBool>,
    tx: channel::Sender<JobEvent>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("pane job".into())
        .spawn(move || {
            let mut herdr = Herdr::on_socket(socket);
            let outcome = run_job(&mut herdr, &pane_id, line.as_ref(), &rerun, &tx);
            let _ = tx.send(JobEvent::Finished(pane_id, outcome));
        })
        .map(drop)
}

fn run_job(
    herdr: &mut Herdr,
    pane_id: &str,
    line: Option<&Plan>,
    rerun: &AtomicBool,
    tx: &channel::Sender<JobEvent>,
) -> Outcome {
    let started = Instant::now();
    let mut sent: Option<(Phase, Instant)> = None;
    loop {
        let info = herdr.process_info(pane_id);
        let seen = seen(&info);
        let rerun = line.is_some() && rerun.load(Ordering::Relaxed);
        match next(seen, sent.map(|(phase, at)| (phase, at.elapsed())), rerun) {
            Next::Wait => {}
            Next::Signal(phase, group) => {
                signal(herdr, pane_id, phase, group);
                sent = Some((phase, Instant::now()));
                let _ = tx.send(JobEvent::Phase(pane_id.to_owned(), phase));
            }
            Next::Rerun => {
                let shell = info.as_ref().map_or(Shell::Posix, prepare::shell_of);
                let Some(line) = line.and_then(|plan| plan.line(shell)) else {
                    tracing::debug!("pane {pane_id}: no line for {shell:?}, left stopped");
                    return Outcome::Stopped;
                };
                return match herdr.send_line(pane_id, &line) {
                    Ok(()) => Outcome::Restarted,
                    Err(err) => {
                        tracing::warn!("restarting pane {pane_id}: {err}");
                        Outcome::Failed
                    }
                };
            }
            Next::Done if seen == Seen::Gone => return Outcome::Gone,
            Next::Done => return Outcome::Stopped,
            Next::GaveUp => return Outcome::GaveUp,
        }
        if started.elapsed() >= JOB_DEADLINE {
            return Outcome::GaveUp;
        }
        std::thread::sleep(POLL);
    }
}

/// Ask process group `group`, leading what runs in herdr pane `pane_id`, to
/// end as hard as `phase` says. Ctrl+C goes through the pane's terminal, as
/// if typed: it reaches programs that read raw keys too.
fn signal(herdr: &mut Herdr, pane_id: &str, phase: Phase, group: i32) {
    if phase == Phase::Interrupt {
        match herdr.send_keys(pane_id, &["ctrl+c"]) {
            Ok(()) => return,
            Err(err) => tracing::warn!("pressing Ctrl+C in pane {pane_id}: {err}"),
        }
    }
    let signal = match phase {
        Phase::Interrupt => libc::SIGINT,
        Phase::Terminate => libc::SIGTERM,
        Phase::Kill => libc::SIGKILL,
    };
    // Never gerdr's nor init's group: `seen` only gives one that isn't the
    // pane's shell's.
    if group > 1 {
        // Safety: sends a signal only.
        unsafe { libc::kill(-group, signal) };
    }
}

impl Gerdr {
    /// What herdr last said of pane `pane_id`.
    fn pane_state(&self, pane_id: &str) -> Option<&PaneState> {
        self.herdr_tabs
            .values()
            .flat_map(|detail| &detail.panes)
            .find(|p| p.pane_id == pane_id)
    }

    /// What the bar of herdr pane `pane_id` shows.
    fn pane_bar_content(&self, pane_id: &str) -> BarContent {
        bar_content(
            self.pane_state(pane_id).and_then(|p| p.process.as_ref()),
            self.last_commands.get(pane_id),
            self.pane_jobs.get(pane_id),
        )
    }

    /// The bars over the active tab's terminal panes.
    pub fn pane_bar_views(&self) -> Vec<PaneBarView> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        let focused = tab.focused_id();
        tab.shown_rects(self.content_rect())
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = tab.panes.iter().find(|p| p.id == id)?;
                let herdr = pane.herdr.as_ref()?;
                let content = self.pane_bar_content(&herdr.pane_id);
                Some(PaneBarView {
                    rect: chrome::pane_bar_rects(rect).0,
                    label: content.label,
                    note: content.note,
                    status: content.status,
                    can_restart: content.can_restart,
                    can_stop: content.can_stop,
                    focused: focused == Some(id),
                    hover: self
                        .pane_bar_hover
                        .filter(|(hovered, _)| *hovered == id)
                        .map(|(_, button)| button),
                })
            })
            .collect()
    }

    /// The pane of the active tab whose bar is at `pos`, and what of it.
    pub fn pane_bar_at(&self, pos: Point<f64, Logical>) -> Option<(u32, BarHit)> {
        let tab = self.tab()?;
        tab.shown_rects(self.content_rect())
            .into_iter()
            .filter(|(id, _)| tab.has_bar(*id))
            .find_map(|(id, rect)| {
                let hit = chrome::pane_bar_hit(chrome::pane_bar_rects(rect).0, pos)?;
                Some((id, hit))
            })
    }

    /// Light up the bar button under the pointer at `pos`, if any.
    pub fn pane_bar_hover(&mut self, pos: Option<Point<f64, Logical>>) {
        self.pane_bar_hover = pos
            .and_then(|pos| self.pane_bar_at(pos))
            .and_then(|(id, hit)| match hit {
                BarHit::Button(button) => Some((id, button)),
                BarHit::Label => None,
            });
    }

    /// A click on pane `id`'s bar: it takes the focus, and its buttons do
    /// what they say.
    pub fn pane_bar_click(&mut self, id: u32, hit: BarHit) {
        self.focus_pane(|_| Some(id));
        let BarHit::Button(button) = hit else {
            return;
        };
        let Some(pane_id) = self.tab().and_then(|tab| tab.herdr_pane(id)) else {
            return;
        };
        let pane_id = pane_id.pane_id.clone();
        match button {
            BarButton::Restart => self.restart_pane(&pane_id),
            BarButton::Stop => self.stop_pane(&pane_id),
        }
    }

    /// The herdr pane of the active tab's focused pane, if it is a terminal.
    fn focused_terminal(&self) -> Option<String> {
        let tab = self.tab()?;
        tab.herdr_pane(tab.focused_id()?).map(|p| p.pane_id.clone())
    }

    pub fn restart_focused_pane(&mut self) {
        match self.focused_terminal() {
            Some(pane_id) => self.restart_pane(&pane_id),
            None => self.show_notice("Not a terminal pane".into()),
        }
    }

    pub fn stop_focused_pane(&mut self) {
        match self.focused_terminal() {
            Some(pane_id) => self.stop_pane(&pane_id),
            None => self.show_notice("Not a terminal pane".into()),
        }
    }

    /// Stop what runs in herdr pane `pane_id` and run its last command
    /// again; while it is being stopped, have it run again after all.
    pub fn restart_pane(&mut self, pane_id: &str) {
        let content = self.pane_bar_content(pane_id);
        if let Some(job) = self.pane_jobs.get(pane_id) {
            if content.can_restart {
                job.rerun.store(true, Ordering::Relaxed);
            }
            return;
        }
        if !content.can_restart {
            return self.show_notice("Nothing to restart in this pane".into());
        }
        self.start_job(pane_id, content.label, true);
    }

    /// Stop what runs in herdr pane `pane_id`; while it is being restarted,
    /// leave it stopped.
    pub fn stop_pane(&mut self, pane_id: &str) {
        let content = self.pane_bar_content(pane_id);
        if let Some(job) = self.pane_jobs.get(pane_id) {
            if content.can_stop {
                job.rerun.store(false, Ordering::Relaxed);
            }
            return;
        }
        if !content.can_stop {
            return self.show_notice("Nothing runs in this pane".into());
        }
        self.start_job(pane_id, content.label, false);
    }

    /// Stop what runs in herdr pane `pane_id`, called `command`, and run its
    /// last command again if `rerun`.
    fn start_job(&mut self, pane_id: &str, command: String, rerun: bool) {
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return;
        };
        let Some(tx) = self.job_sender() else {
            return;
        };
        // No tools: asking poetry or pipenv mustn't hold up the main thread.
        let line = self
            .last_commands
            .get(pane_id)
            .map(|r| Plan::new(&restart_prep(pane_id, &r.captured), &Tools::default()))
            .filter(|plan| !plan.is_empty());
        let flag = Arc::new(AtomicBool::new(rerun));
        let job = Job {
            rerun: flag.clone(),
            can_rerun: line.is_some(),
            command,
            phase: None,
            restarted: None,
        };
        match spawn_job(socket, pane_id.to_owned(), line, flag, tx) {
            Ok(()) => {
                self.pane_jobs.insert(pane_id.to_owned(), job);
            }
            Err(err) => tracing::warn!("stopping pane {pane_id}: {err}"),
        }
    }

    /// Where the jobs report, set up on first use.
    fn job_sender(&mut self) -> Option<channel::Sender<JobEvent>> {
        if self.pane_job_tx.is_none() {
            let (tx, rx) = channel::channel::<JobEvent>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.on_job_event(event);
                }
            });
            match inserted {
                Ok(_) => self.pane_job_tx = Some(tx),
                Err(err) => tracing::warn!("pane jobs: {err}"),
            }
        }
        self.pane_job_tx.clone()
    }

    /// A job's report. One of a project switched away from is about panes
    /// no longer known, and goes unheard.
    fn on_job_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Phase(pane_id, phase) => {
                if let Some(job) = self.pane_jobs.get_mut(&pane_id) {
                    job.phase = Some(phase);
                }
            }
            JobEvent::Finished(pane_id, outcome) => {
                let Some(job) = self.pane_jobs.get_mut(&pane_id) else {
                    return;
                };
                if outcome == Outcome::Restarted {
                    let at = Instant::now();
                    job.restarted = Some(at);
                    self.settle_job(pane_id, at);
                    return;
                }
                let command = self
                    .pane_jobs
                    .remove(&pane_id)
                    .map(|job| job.command)
                    .unwrap_or_default();
                match outcome {
                    Outcome::GaveUp => self.show_notice(format!("Couldn't stop {command}")),
                    Outcome::Failed => self.show_notice(format!("Couldn't restart {command}")),
                    _ => {}
                }
            }
        }
    }

    /// Forget the job of pane `pane_id`, which restarted its command `at`,
    /// in a moment.
    fn settle_job(&mut self, pane_id: String, at: Instant) {
        let forget = move |_, _: &mut (), state: &mut Gerdr| {
            if state
                .pane_jobs
                .get(&pane_id)
                .is_some_and(|job| job.restarted == Some(at))
            {
                state.pane_jobs.remove(&pane_id);
            }
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(SETTLE), forget)
        {
            tracing::warn!("pane jobs: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: i32 = 100;

    fn process(pid: i32, name: &str, cmdline: &str) -> Process {
        Process {
            pid,
            name: name.into(),
            argv: cmdline.split_whitespace().map(str::to_owned).collect(),
            cmdline: cmdline.into(),
            cwd: None,
        }
    }

    /// A pane whose shell is at its prompt.
    fn at_prompt() -> ProcessInfo {
        ProcessInfo {
            shell_pid: Some(SHELL),
            foreground_process_group_id: Some(SHELL),
            foreground_processes: vec![process(SHELL, "zsh", "/usr/bin/zsh")],
        }
    }

    /// A pane running `cmdline` as process `pid`, leading its group.
    fn running(pid: i32, cmdline: &str) -> ProcessInfo {
        let name = cmdline.split_whitespace().next().unwrap_or_default();
        ProcessInfo {
            shell_pid: Some(SHELL),
            foreground_process_group_id: Some(pid),
            foreground_processes: vec![process(pid, name, cmdline)],
        }
    }

    /// The line a POSIX shell gets to run `c` again.
    fn restart_line(c: &Captured) -> String {
        let prep = restart_prep("p1", c);
        Plan::new(&prep, &Tools::default())
            .line(Shell::Posix)
            .unwrap()
    }

    fn captured(command: &str, cwd: Option<&str>, env: &[(&str, &str)]) -> Captured {
        Captured {
            command: command.into(),
            cwd: cwd.map(PathBuf::from),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn busy(pid: i32, command: &str) -> Foreground {
        Foreground::Busy {
            pid,
            captured: captured(command, None, &[]),
        }
    }

    fn job(command: &str, rerun: bool) -> Job {
        Job {
            rerun: Arc::new(AtomicBool::new(rerun)),
            can_rerun: true,
            command: command.into(),
            phase: None,
            restarted: None,
        }
    }

    #[test]
    fn interrupt_comes_first_then_term_after_three_seconds_then_kill() {
        let run = Seen::Running(42);
        let secs = Duration::from_secs;
        assert_eq!(next(run, None, false), Next::Signal(Phase::Interrupt, 42));
        assert_eq!(
            next(run, Some((Phase::Interrupt, secs(2))), false),
            Next::Wait
        );
        assert_eq!(
            next(run, Some((Phase::Interrupt, secs(3))), true),
            Next::Signal(Phase::Terminate, 42)
        );
        assert_eq!(
            next(run, Some((Phase::Terminate, secs(1))), false),
            Next::Wait
        );
        assert_eq!(
            next(run, Some((Phase::Terminate, secs(2))), false),
            Next::Signal(Phase::Kill, 42)
        );
    }

    #[test]
    fn gives_up_a_second_after_kill() {
        let run = Seen::Running(42);
        let ms = Duration::from_millis;
        assert_eq!(next(run, Some((Phase::Kill, ms(900))), false), Next::Wait);
        assert_eq!(
            next(run, Some((Phase::Kill, ms(1000))), false),
            Next::GaveUp
        );
    }

    #[test]
    fn a_prompt_reruns_only_when_restarting() {
        assert_eq!(next(Seen::Prompt, None, true), Next::Rerun);
        assert_eq!(next(Seen::Prompt, None, false), Next::Done);
        let sent = Some((Phase::Interrupt, Duration::ZERO));
        assert_eq!(next(Seen::Prompt, sent, true), Next::Rerun);
        assert_eq!(next(Seen::Starting, sent, true), Next::Wait);
    }

    #[test]
    fn a_gone_pane_ends_the_job() {
        assert_eq!(next(Seen::Gone, None, true), Next::Done);
        assert_eq!(seen(&Err("no pane".into())), Seen::Gone);
    }

    #[test]
    fn seen_reads_prompt_busy_and_starting() {
        assert_eq!(seen(&Ok(at_prompt())), Seen::Prompt);
        assert_eq!(seen(&Ok(running(7, "npm run dev"))), Seen::Running(7));
        assert_eq!(seen(&Ok(ProcessInfo::default())), Seen::Starting);
        // A shell run from the first one is its prompt too.
        assert_eq!(seen(&Ok(running(7, "bash"))), Seen::Prompt);
    }

    #[test]
    fn a_new_leader_is_remembered_with_its_folder() {
        let mut memory = HashMap::new();
        let cwd = Path::new("/w/app");
        remember(&mut memory, "p1", Some(&busy(7, "npm run dev")), Some(cwd));
        let r = &memory["p1"];
        assert_eq!((r.pid, r.captured.command.as_str()), (7, "npm run dev"));
        assert_eq!(r.captured.cwd.as_deref(), Some(cwd));
        // Its own folder, when it ran elsewhere, wins.
        let elsewhere = Foreground::Busy {
            pid: 8,
            captured: captured("make", Some("/w/lib"), &[]),
        };
        remember(&mut memory, "p1", Some(&elsewhere), Some(cwd));
        assert_eq!(
            memory["p1"].captured.cwd.as_deref(),
            Some(Path::new("/w/lib"))
        );
    }

    #[test]
    fn the_same_leader_keeps_its_first_command() {
        let mut memory = HashMap::new();
        remember(&mut memory, "p1", Some(&busy(7, "uvicorn app:app")), None);
        remember(&mut memory, "p1", Some(&busy(7, "uvicorn worker")), None);
        assert_eq!(memory["p1"].captured.command, "uvicorn app:app");
    }

    #[test]
    fn idle_and_denied_keep_the_last_command() {
        let mut memory = HashMap::new();
        remember(&mut memory, "p1", Some(&busy(7, "npm run dev")), None);
        remember(&mut memory, "p1", Some(&Foreground::Idle), None);
        let denied = Foreground::Untracked {
            pid: 9,
            denied: true,
        };
        remember(&mut memory, "p1", Some(&denied), None);
        remember(&mut memory, "p1", None, None);
        assert_eq!(memory["p1"].captured.command, "npm run dev");
        assert_eq!(memory.len(), 1);
    }

    #[test]
    fn restart_line_cds_and_restores_the_venv_it_had() {
        let dir = std::env::temp_dir().join(format!("gerdr-restart-{}", std::process::id()));
        let venv = dir.join(".venv");
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("bin/activate"), "").unwrap();
        let venv_s = venv.to_string_lossy().into_owned();
        let dir_s = dir.to_string_lossy().into_owned();
        let c = captured(
            "python manage.py runserver",
            Some(&dir_s),
            &[("VIRTUAL_ENV", &venv_s), ("MISE_NODE_VERSION", "20")],
        );
        let line = restart_line(&c);
        assert_eq!(
            line,
            format!(
                " . {venv_s}/bin/activate && export MISE_NODE_VERSION=20 \
                 && cd {dir_s} && python manage.py runserver"
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restart_without_a_venv_activates_none() {
        // A venv right there isn't one the command ran in.
        let dir = std::env::temp_dir().join(format!("gerdr-novenv-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".venv/bin")).unwrap();
        std::fs::write(dir.join(".venv/bin/activate"), "").unwrap();
        let dir_s = dir.to_string_lossy().into_owned();
        let c = captured("npm run dev", Some(&dir_s), &[]);
        let line = restart_line(&c);
        assert_eq!(line, format!(" cd {dir_s} && npm run dev"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bar_content_of_the_remembered_command_running() {
        let remembered = Remembered {
            pid: 7,
            captured: captured("npm run dev", None, &[]),
        };
        let content = bar_content(
            Some(&running(7, "/usr/bin/npm run dev")),
            Some(&remembered),
            None,
        );
        assert_eq!(content.label, "npm run dev");
        assert_eq!(content.status, BarStatus::Running);
        assert!(content.can_restart && content.can_stop);
    }

    #[test]
    fn bar_content_of_something_else_running() {
        let remembered = Remembered {
            pid: 7,
            captured: captured("npm run dev", None, &[]),
        };
        let content = bar_content(Some(&running(9, "vim notes")), Some(&remembered), None);
        assert_eq!(content.label, "vim notes");
        assert!(!content.can_restart && content.can_stop);
    }

    #[test]
    fn bar_content_of_a_script_running() {
        let content = bar_content(Some(&running(9, "/bin/bash ./runlocal.sh")), None, None);
        assert_eq!(content.status, BarStatus::Running);
        assert!(content.can_stop);
    }

    #[test]
    fn bar_content_of_an_exited_command() {
        let remembered = Remembered {
            pid: 7,
            captured: captured("npm run dev", None, &[]),
        };
        let content = bar_content(Some(&at_prompt()), Some(&remembered), None);
        assert_eq!(
            (content.label.as_str(), content.note, content.status),
            ("npm run dev", Some("exited"), BarStatus::Exited)
        );
        assert!(content.can_restart && !content.can_stop);
    }

    #[test]
    fn bar_content_of_a_bare_shell() {
        let content = bar_content(Some(&at_prompt()), None, None);
        assert_eq!(
            (content.label.as_str(), content.status),
            ("zsh", BarStatus::Idle)
        );
        assert!(!content.can_restart && !content.can_stop);
        let unknown = bar_content(None, None, None);
        assert_eq!(unknown.label, "shell");
    }

    #[test]
    fn bar_content_of_a_job_offers_the_other_button() {
        let mut restarting = job("npm run dev", true);
        let content = bar_content(Some(&running(7, "npm run dev")), None, Some(&restarting));
        assert_eq!(
            (content.note, content.status),
            (Some("restarting…"), BarStatus::Working)
        );
        assert!(!content.can_restart && content.can_stop);

        restarting.phase = Some(Phase::Terminate);
        let content = bar_content(None, None, Some(&restarting));
        assert_eq!(content.note, Some("terminating…"));

        let stopping = job("npm run dev", false);
        let content = bar_content(None, None, Some(&stopping));
        assert_eq!(content.note, Some("stopping…"));
        assert!(content.can_restart && !content.can_stop);

        restarting.restarted = Some(Instant::now());
        let content = bar_content(None, None, Some(&restarting));
        assert_eq!(content.note, Some("restarting…"));
        assert!(!content.can_restart && !content.can_stop);
    }
}
