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
//!
//! The bar keeps out of the way of an idle shell: it shows while a job
//! runs or settles, while a command other than the shell runs in front,
//! while the pointer is near the pane's top edge or over the bar, and for
//! `FOCUS_HOLD` after the pane gains focus (see `bar_wanted`). Hidden, it
//! gives its room to the window. Each bar fades and slides as it shows
//! and hides (`BarStates`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::{
        channel,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Rectangle},
};

use crate::{
    Gerdr, Tab,
    anim::{self, Fade},
    chrome::{self, BarButton, BarHit, BarStatus, Help, PaneBarView, Rect},
    herdr::{Herdr, PaneState, Process, ProcessInfo},
    layout::Dir,
    menu::{MenuAction, MenuItem, MenuSpec},
    prepare::{self, Plan, Prep, Shell, Step, Tools},
    projects::{Captured, Foreground, VenvSetting},
    tooltip::Tooltip,
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

/// How long a pane's bar stays out after the pane gains focus.
pub const FOCUS_HOLD: Duration = Duration::from_secs(1);

/// How much a pane's bar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarWant {
    Hidden,
    /// Drawn over the top of the window, which keeps its room: a passing
    /// look.
    Over,
    /// Out with room of its own, the window below it.
    Room,
}

/// Whether a pane's bar shows: with room while its command is `status` (a
/// job, or something other than the shell, running) or while the pointer
/// asks for it (`pointer`, see `chrome::pointer_reveals`); over the window
/// for `FOCUS_HOLD` after the pane gained focus (`focused_for` while it
/// has it), so a focus change moves no window.
pub fn bar_wanted(status: BarStatus, pointer: bool, focused_for: Option<Duration>) -> BarWant {
    if matches!(status, BarStatus::Running | BarStatus::Working) || pointer {
        BarWant::Room
    } else if focused_for.is_some_and(|since| since < FOCUS_HOLD) {
        BarWant::Over
    } else {
        BarWant::Hidden
    }
}

/// Where one pane's bar stands between hidden and shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BarState {
    /// The bar is out (or on its way out).
    visible: bool,
    /// The bar takes room over its window.
    room: bool,
    /// The fade it is in, while it still moves.
    fade: Option<Fade>,
    /// Since when the pane has had the focus, while it has.
    focused_since: Option<Instant>,
}

impl BarState {
    /// Its opacity at `now`.
    fn alpha(&self, now: Instant) -> f32 {
        match self.fade {
            Some(fade) if !fade.done(now) => fade.value(now),
            _ if self.visible => 1.0,
            _ => 0.0,
        }
    }

    /// Start showing or hiding at `now`, going on from wherever a fade
    /// under way had got to.
    fn flip(&mut self, visible: bool, now: Instant) {
        let from = self.alpha(now);
        self.visible = visible;
        self.fade = Some(Fade::from_value(now, anim::SLOW, visible, from));
    }
}

/// How a pane's bar is to be drawn right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarLook {
    /// The bar is out, or still fading out.
    pub visible: bool,
    /// The bar takes room over its window.
    pub shown: bool,
    /// Its opacity, in `anim::quantize` steps; 1 at rest.
    pub alpha: f32,
    /// How far it is still under its pane's top edge; 0 at rest.
    pub slide: i32,
    /// It just took or gave back its room: the window's room changes.
    pub flipped: bool,
}

/// The show/hide state of every pane's bar, by herdr pane id, moved on
/// each time the bars are looked at.
#[derive(Debug, Default)]
pub struct BarStates(Mutex<HashMap<String, BarState>>);

impl BarStates {
    /// Pane `key`'s bar at `now`: whether the pane is `focused`, and how
    /// much of the bar is wanted given how long the pane has had the
    /// focus and whether the bar is out already (`want`).
    pub fn update(
        &self,
        key: &str,
        focused: bool,
        now: Instant,
        want: impl FnOnce(Option<Duration>, bool) -> BarWant,
    ) -> BarLook {
        let mut states = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let state = states.entry(key.to_owned()).or_default();
        state.focused_since = match (focused, state.focused_since) {
            (false, _) => None,
            (true, Some(since)) => Some(since),
            (true, None) => Some(now),
        };
        let focused_for = state.focused_since.map(|since| now.duration_since(since));
        let wanted = want(focused_for, state.visible);
        let visible = wanted != BarWant::Hidden;
        let room = wanted == BarWant::Room;
        if visible != state.visible {
            state.flip(visible, now);
        }
        let flipped = room != state.room;
        state.room = room;
        if state.fade.is_some_and(|fade| fade.done(now)) {
            state.fade = None;
        }
        let alpha = anim::quantize(state.alpha(now)) as f32 / 16.0;
        BarLook {
            visible: state.visible,
            shown: state.room,
            alpha,
            slide: ((1.0 - alpha) * chrome::BAR_SLIDE as f32).round() as i32,
            flipped,
        }
    }

    /// Whether pane `key`'s bar takes room over its window.
    pub fn shown(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|state| state.room)
    }

    /// Whether pane `key`'s bar is out, with room or over its window.
    pub fn drawn(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|state| state.visible)
    }

    /// Forget every pane but `keep`.
    pub fn retain(&self, keep: &[String]) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|key, _| keep.contains(key));
    }
}

/// What the help calls each thing a bar offers: its own description, its
/// group's, and its place in the group (see `keys::HELP_LINES`).
const SPLIT_RIGHT_HELP: (&str, &str, usize) = ("split pane right", "split pane right / down", 0);
const SPLIT_DOWN_HELP: (&str, &str, usize) = ("split pane down", "split pane right / down", 1);
const RESTART_HELP: (&str, &str, usize) = (
    "restart the pane's command",
    "restart / stop the pane's command",
    0,
);
const STOP_HELP: (&str, &str, usize) = (
    "stop the pane's command",
    "restart / stop the pane's command",
    1,
);
const CLOSE_PANE_HELP: (&str, &str, usize) = ("close pane", "close pane", 0);

/// The key for `what` (one of the `_HELP` triples), if `help` lists one.
/// The key for one of the bar's actions, as the help lists it: on its own
/// line (`own`) or as the `index`th of its group's line (`group`).
fn hint_for(help: &Help, (own, group, index): (&str, &str, usize)) -> Option<String> {
    help.key_for(own, 0).or_else(|| help.key_for(group, index))
}

/// What a bar button says when the pointer rests on it at `rect`.
pub fn button_tooltip(button: BarButton, rect: Rect, help: &Help) -> Tooltip {
    let (text, what) = match button {
        BarButton::Restart => ("Restart", RESTART_HELP),
        BarButton::Stop => ("Stop", STOP_HELP),
    };
    let tip = Tooltip::new(text, rect);
    match hint_for(help, what) {
        Some(key) => tip.key(&key),
        None => tip,
    }
}

/// The menu a right press at `at` on the bar of pane `id` (showing
/// `content`) opens.
pub fn bar_menu(at: Point<f64, Logical>, id: u32, content: &BarContent, help: &Help) -> MenuSpec {
    let item = |label: &str, action, what, enabled| {
        let item = MenuItem::new(label, action).enabled(enabled);
        match hint_for(help, what) {
            Some(key) => item.hint(&key),
            None => item,
        }
    };
    MenuSpec {
        anchor: Rectangle::new(at.to_i32_round(), (0, 0).into()),
        items: vec![
            item(
                "Split right",
                MenuAction::SplitPane(id, Dir::Right),
                SPLIT_RIGHT_HELP,
                true,
            ),
            item(
                "Split down",
                MenuAction::SplitPane(id, Dir::Down),
                SPLIT_DOWN_HELP,
                true,
            ),
            item(
                "Restart",
                MenuAction::RestartPane(id),
                RESTART_HELP,
                content.can_restart,
            ),
            item(
                "Stop",
                MenuAction::StopPane(id),
                STOP_HELP,
                content.can_stop,
            ),
            item(
                "Close pane",
                MenuAction::ClosePane(id),
                CLOSE_PANE_HELP,
                true,
            ),
        ],
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

    /// Where the bars stand between hidden and shown.
    fn bar_states(&self) -> &BarStates {
        &self.bar_states
    }

    /// Whether the bar of pane `id` of `tab` takes room over its window:
    /// what `chrome::pane_bar_rects_shown` is given when the panes are
    /// arranged. A bar not looked at yet (a tab not shown) is hidden.
    pub fn pane_bar_shown(&self, tab: &Tab, id: u32) -> bool {
        tab.herdr_pane(id)
            .is_some_and(|pane| self.bar_states().shown(&pane.pane_id))
    }

    /// Whether the bar of pane `id` of `tab` is out, with room or over
    /// its window: there to see and to click.
    fn pane_bar_drawn(&self, tab: &Tab, id: u32) -> bool {
        tab.herdr_pane(id)
            .is_some_and(|pane| self.bar_states().drawn(&pane.pane_id))
    }

    /// The herdr pane ids of every pane in every workspace: the bars
    /// whose state is kept, shown or not.
    fn herdr_pane_ids(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| pane.herdr.as_ref().map(|h| h.pane_id.clone()))
            .collect()
    }

    /// Arrange the panes again once this event is done: a bar started
    /// showing or hiding, so a window's room changed.
    fn arrange_soon(&self) {
        let arrange = |_, _: &mut (), state: &mut Gerdr| {
            state.arrange();
            TimeoutAction::Drop
        };
        if let Err(err) = self.loop_handle.insert_source(Timer::immediate(), arrange) {
            tracing::warn!("pane bars: {err}");
        }
    }

    /// The bars over the active tab's terminal panes, moved on to now:
    /// a hidden one at rest has no height. The other tabs' bars keep
    /// their state meanwhile, so coming back moves nothing.
    pub fn pane_bar_views(&self) -> Vec<PaneBarView> {
        let states = self.bar_states();
        states.retain(&self.herdr_pane_ids());
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        let now = Instant::now();
        let pointer = self.pointer;
        let focused = tab.focused_id();
        let mut flipped = false;
        let views = tab
            .shown_rects(self.content_rect())
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = tab.panes.iter().find(|p| p.id == id)?;
                let key = pane.herdr.as_ref()?.pane_id.as_str();
                let content = self.pane_bar_content(key);
                let is_focused = focused == Some(id);
                let look = states.update(key, is_focused, now, |focused_for, out| {
                    let near = chrome::pointer_reveals(rect, out, pointer);
                    bar_wanted(content.status, near, focused_for)
                });
                flipped |= look.flipped;
                let full = chrome::pane_bar_rects(rect).0;
                let drawn = look.visible || look.alpha > 0.0;
                let bar = if drawn {
                    full
                } else {
                    Rectangle::new(full.loc, (full.size.w, 0).into())
                };
                Some(PaneBarView {
                    rect: bar,
                    label: content.label,
                    note: content.note,
                    status: content.status,
                    can_restart: content.can_restart,
                    can_stop: content.can_stop,
                    focused: is_focused,
                    lit: look.visible && full.to_f64().contains(pointer),
                    hover: self
                        .pane_bar_hover
                        .filter(|(hovered, _)| *hovered == id)
                        .map(|(_, button)| button),
                    visible: look.visible,
                    alpha: look.alpha,
                    slide: look.slide,
                })
            })
            .collect();
        if flipped {
            self.arrange_soon();
        }
        views
    }

    /// The pane of the active tab whose bar is at `pos`, and what of it.
    /// A hidden bar is not there to hit.
    pub fn pane_bar_at(&self, pos: Point<f64, Logical>) -> Option<(u32, BarHit)> {
        let tab = self.tab()?;
        tab.shown_rects(self.content_rect())
            .into_iter()
            .filter(|(id, _)| self.pane_bar_drawn(tab, *id))
            .find_map(|(id, rect)| {
                let hit = chrome::pane_bar_hit(chrome::pane_bar_rects(rect).0, pos)?;
                Some((id, hit))
            })
    }

    /// The pointer moved to `pos` over the app area with nothing open
    /// over it: the tooltip for the bar button under it, if one is.
    pub(crate) fn pane_bar_motion(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let (id, BarHit::Button(button)) = self.pane_bar_at(pos)? else {
            return None;
        };
        let tab = self.tab()?;
        let (_, rect) = tab
            .shown_rects(self.content_rect())
            .into_iter()
            .find(|(pane, _)| *pane == id)?;
        let bar = chrome::pane_bar_rects(rect).0;
        let (_, rect) = chrome::pane_bar_buttons(bar)
            .into_iter()
            .find(|(b, _)| *b == button)?;
        Some(button_tooltip(button, rect, &self.keymap.help()))
    }

    /// The menu a right press at `pos` on a pane's bar opens, if any.
    pub(crate) fn pane_bar_menu(&self, pos: Point<f64, Logical>) -> Option<MenuSpec> {
        let (id, _) = self.pane_bar_at(pos)?;
        let pane_id = &self.tab()?.herdr_pane(id)?.pane_id;
        let content = self.pane_bar_content(pane_id);
        Some(bar_menu(pos, id, &content, &self.keymap.help()))
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

    #[test]
    fn a_bar_shows_for_a_command_the_pointer_or_a_new_focus() {
        let ms = Duration::from_millis;
        assert_eq!(bar_wanted(BarStatus::Running, false, None), BarWant::Room);
        assert_eq!(bar_wanted(BarStatus::Working, false, None), BarWant::Room);
        assert_eq!(bar_wanted(BarStatus::Idle, false, None), BarWant::Hidden);
        assert_eq!(bar_wanted(BarStatus::Exited, false, None), BarWant::Hidden);
        assert_eq!(bar_wanted(BarStatus::Idle, true, None), BarWant::Room);
        // A new focus shows the bar over the window, taking no room.
        assert_eq!(
            bar_wanted(BarStatus::Exited, false, Some(ms(999))),
            BarWant::Over
        );
        assert_eq!(
            bar_wanted(BarStatus::Idle, true, Some(ms(999))),
            BarWant::Room
        );
        assert_eq!(
            bar_wanted(BarStatus::Idle, false, Some(FOCUS_HOLD)),
            BarWant::Hidden
        );
        assert_eq!(
            bar_wanted(BarStatus::Idle, false, Some(ms(5000))),
            BarWant::Hidden
        );
    }

    #[test]
    fn a_bar_fades_in_when_wanted_and_out_when_not() {
        let states = BarStates::default();
        let t0 = Instant::now();
        let out = |out: bool| if out { BarWant::Room } else { BarWant::Hidden };
        // Not wanted: hidden from the start, nothing to see.
        let look = states.update("p1", false, t0, |_, shown| {
            assert!(!shown);
            BarWant::Hidden
        });
        assert_eq!((look.shown, look.alpha, look.flipped), (false, 0.0, false));
        assert_eq!(look.slide, chrome::BAR_SLIDE);
        assert!(!states.shown("p1"));
        // Wanted: it starts showing, taking its room at once.
        let look = states.update("p1", false, t0, |_, _| BarWant::Room);
        assert!(look.shown && look.flipped && look.alpha == 0.0);
        assert!(states.shown("p1") && states.drawn("p1"));
        let look = states.update("p1", false, t0 + anim::SLOW / 2, |_, shown| out(shown));
        assert!(look.alpha > 0.3 && look.alpha < 0.7, "{}", look.alpha);
        assert!(look.slide > 0 && look.slide < chrome::BAR_SLIDE);
        assert!(!look.flipped);
        let look = states.update("p1", false, t0 + anim::SLOW, |_, _| BarWant::Room);
        assert_eq!((look.alpha, look.slide), (1.0, 0));
        // Not wanted any more: it gives its room back and fades out.
        let t1 = t0 + Duration::from_secs(2);
        let look = states.update("p1", false, t1, |_, _| BarWant::Hidden);
        assert!(!look.shown && look.flipped && look.alpha == 1.0);
        // On its way out it is neither shown nor there to click.
        assert!(!states.shown("p1") && !states.drawn("p1"));
        let look = states.update("p1", false, t1 + 2 * anim::SLOW, |_, _| BarWant::Hidden);
        assert_eq!((look.alpha, look.slide), (0.0, chrome::BAR_SLIDE));
    }

    #[test]
    fn a_bar_over_the_window_takes_no_room() {
        let states = BarStates::default();
        let t0 = Instant::now();
        // A new focus: out, seen, but the window keeps its room.
        let look = states.update("p1", true, t0, |_, _| BarWant::Over);
        assert!(look.visible && !look.shown && !look.flipped);
        assert!(states.drawn("p1") && !states.shown("p1"));
        // The pointer asks for it: it takes its room without fading again.
        let look = states.update("p1", true, t0 + anim::SLOW, |_, _| BarWant::Room);
        assert!(look.visible && look.shown && look.flipped);
        assert_eq!(look.alpha, 1.0);
        // Back to a passing look: the room goes, the bar stays.
        let look = states.update("p1", true, t0 + 2 * anim::SLOW, |_, _| BarWant::Over);
        assert!(look.visible && !look.shown && look.flipped);
        assert_eq!(look.alpha, 1.0);
    }

    #[test]
    fn a_fade_turned_around_goes_on_from_where_it_was() {
        let states = BarStates::default();
        let t0 = Instant::now();
        states.update("p1", false, t0, |_, _| BarWant::Room);
        let half = states.update("p1", false, t0 + anim::SLOW / 2, |_, _| BarWant::Room);
        let back = states.update("p1", false, t0 + anim::SLOW / 2, |_, _| BarWant::Hidden);
        assert!(back.flipped && !back.shown);
        assert!(
            (back.alpha - half.alpha).abs() < 0.1,
            "{} {}",
            back.alpha,
            half.alpha
        );
        let gone = states.update("p1", false, t0 + anim::SLOW, |_, _| BarWant::Hidden);
        assert_eq!(gone.alpha, 0.0);
    }

    #[test]
    fn focus_is_timed_from_when_it_was_gained() {
        let states = BarStates::default();
        let t0 = Instant::now();
        let seen = |focused, at| {
            let mut since = None;
            states.update("p1", focused, at, |focused_for, _| {
                since = focused_for;
                BarWant::Hidden
            });
            since
        };
        assert_eq!(seen(false, t0), None);
        assert_eq!(seen(true, t0), Some(Duration::ZERO));
        let ms = Duration::from_millis;
        assert_eq!(seen(true, t0 + ms(300)), Some(ms(300)));
        assert_eq!(seen(false, t0 + ms(400)), None);
        // Gained again: the clock starts over.
        assert_eq!(seen(true, t0 + ms(500)), Some(Duration::ZERO));
        // Forgotten panes start from nothing.
        states.retain(&[]);
        assert_eq!(seen(true, t0 + ms(900)), Some(Duration::ZERO));
        states.retain(&["p1".into()]);
        assert_eq!(seen(true, t0 + ms(1000)), Some(ms(100)));
    }

    #[test]
    fn key_hints_come_from_the_help() {
        let help = crate::keys::Keymap::default().help();
        assert_eq!(hint_for(&help, RESTART_HELP).as_deref(), Some("Ctrl+B A"));
        assert_eq!(hint_for(&help, STOP_HELP).as_deref(), Some("Ctrl+B S"));
        assert_eq!(
            hint_for(&help, SPLIT_RIGHT_HELP).as_deref(),
            Some("Ctrl+B V")
        );
        assert_eq!(
            hint_for(&help, SPLIT_DOWN_HELP).as_deref(),
            Some("Ctrl+B -")
        );
        assert_eq!(
            hint_for(&help, CLOSE_PANE_HELP).as_deref(),
            Some("Ctrl+B X")
        );
        // An action on a line of its own, with several keys: the first.
        let own = Help {
            prefix: "Ctrl+A".into(),
            rows: vec![("r / Shift+R".into(), "restart the pane's command".into())],
        };
        assert_eq!(hint_for(&own, RESTART_HELP).as_deref(), Some("Ctrl+A r"));
        assert_eq!(hint_for(&own, STOP_HELP), None);
    }

    #[test]
    fn bar_buttons_have_tooltips_with_their_keys() {
        let help = crate::keys::Keymap::default().help();
        let rect = Rect::new((500, 50).into(), (24, 24).into());
        let tip = button_tooltip(BarButton::Restart, rect, &help);
        assert_eq!(
            (tip.text.as_str(), tip.key.as_deref(), tip.anchor),
            ("Restart", Some("Ctrl+B A"), rect)
        );
        let tip = button_tooltip(BarButton::Stop, rect, &help);
        assert_eq!(
            (tip.text.as_str(), tip.key.as_deref()),
            ("Stop", Some("Ctrl+B S"))
        );
    }

    #[test]
    fn the_bar_menu_offers_what_the_pane_can_do() {
        let help = crate::keys::Keymap::default().help();
        let content = bar_content(Some(&at_prompt()), None, None);
        let menu = bar_menu((300.5, 60.2).into(), 7, &content, &help);
        assert_eq!(menu.anchor, Rect::new((301, 60).into(), (0, 0).into()));
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Split right", "Split down", "Restart", "Stop", "Close pane"]
        );
        let actions: Vec<&MenuAction> = menu.items.iter().map(|i| &i.action).collect();
        assert_eq!(
            actions,
            [
                &MenuAction::SplitPane(7, Dir::Right),
                &MenuAction::SplitPane(7, Dir::Down),
                &MenuAction::RestartPane(7),
                &MenuAction::StopPane(7),
                &MenuAction::ClosePane(7),
            ]
        );
        let enabled: Vec<bool> = menu.items.iter().map(|i| i.enabled).collect();
        assert_eq!(enabled, [true, true, false, false, true]);
        assert_eq!(menu.items[2].hint.as_deref(), Some("Ctrl+B A"));
        // With a command running: stop; with one exited: restart.
        let running = bar_content(Some(&running(9, "npm run dev")), None, None);
        let menu = bar_menu((0.0, 0.0).into(), 1, &running, &help);
        assert!(!menu.items[2].enabled && menu.items[3].enabled);
    }
}
