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
//! and while the pointer is near the pane's top edge or over the bar (see
//! `bar_wanted`). Shown or hidden, it takes no room from the window: its
//! grip (`chrome::PANE_GRIP`) and its pill float over the window's top.
//! Each bar drops from its grip as it shows, and back as it hides
//! (`BarStates`). A focus change alone doesn't bring it out: the grip
//! shows which pane has the focus.
//!
//! An app's window and a web page have a bar too, shown and hidden the
//! same way: an app's with its title, a web page's with its address and
//! where the page can go. Every bar is there to drag the pane by (see
//! `pane_drag`), for its menu, and to zoom and close the pane.

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
    Action, Gerdr, Pane, Tab,
    anim::{self, Fade},
    chrome::{self, BarButton, BarButtons, BarHit, BarStatus, Help, PaneBarView, Rect},
    herdr::{AgentStatus, Herdr, PaneState, Process, ProcessInfo},
    layout::Dir,
    menu::{MenuAction, MenuItem, MenuSpec},
    prepare::{self, Plan, Prep, Shell, Step, Tools},
    projects::{Captured, Foreground, VenvSetting},
    toast::Level,
    tooltip::Tooltip,
    web_control::WebStep,
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

/// How long a command must run to count as a service: one restarting a
/// workspace runs again after it exited (crashed, say).
pub const LONG_RUN: Duration = Duration::from_secs(60);

/// A pane's last command, and the process that ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remembered {
    /// 0 while no process has run it yet (see `Remembered::service`).
    pub pid: i32,
    pub captured: Captured,
    /// When its process was first seen.
    pub since: Instant,
    /// Whether it is a service: it ran for `LONG_RUN`, or gerdr ran it
    /// again (after a reboot, or restarting its workspace), or it is the
    /// same command as a service before it in the pane (one restarted
    /// after a crash may crash again before it has run long).
    pub long: bool,
}

impl Remembered {
    /// `captured`, a service about to be run again in its pane.
    pub fn service(captured: Captured, now: Instant) -> Self {
        Self {
            pid: 0,
            captured,
            since: now,
            long: true,
        }
    }
}

/// Bring the last command of herdr pane `pane_id` (in folder `pane_cwd`) up
/// to date with what runs in it at `now`: a new command takes the place of
/// the last, once per process (some rewrite their command line once
/// running). An idle shell, a program never run again or one whose command
/// line can't be read back keeps it.
pub fn remember(
    memory: &mut HashMap<String, Remembered>,
    pane_id: &str,
    foreground: Option<&Foreground>,
    pane_cwd: Option<&Path>,
    now: Instant,
) {
    let Some(Foreground::Busy { pid, captured }) = foreground else {
        return;
    };
    if let Some(r) = memory.get_mut(pane_id).filter(|r| r.pid == *pid) {
        r.long |= now.duration_since(r.since) >= LONG_RUN;
        return;
    }
    let mut captured = captured.clone();
    // Always where it ran: the shell may have moved since.
    captured.cwd = captured.cwd.or_else(|| pane_cwd.map(Path::to_path_buf));
    let long = memory
        .get(pane_id)
        .is_some_and(|r| r.long && r.captured.command == captured.command);
    memory.insert(
        pane_id.to_owned(),
        Remembered {
            pid: *pid,
            captured,
            since: now,
            long,
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
    /// Run again: a terminal's command, or a web page loaded again.
    pub can_restart: bool,
    pub can_stop: bool,
    /// A web page's history goes back, or forward, from where it is.
    pub can_back: bool,
    pub can_forward: bool,
    pub buttons: BarButtons,
    /// The workspace's other tabs, by index, with their titles: where the
    /// menu can move the pane. Only the menu fills it in.
    pub other_tabs: Vec<(usize, String)>,
    /// How many panes share the pane's tab. Only the menu fills it in.
    pub pane_count: usize,
    /// The pane fills its tab, zoomed. Only the menu fills it in.
    pub zoomed: bool,
}

/// How a bar stands with `agent`, where herdr sees one, in its pane: the
/// agent's process runs between prompts too, so it is only `Running` at work.
fn status_with_agent(status: BarStatus, agent: Option<AgentStatus>) -> BarStatus {
    match (status, agent) {
        (BarStatus::Running, Some(agent)) if agent != AgentStatus::Working => {
            BarStatus::Agent(agent)
        }
        _ => status,
    }
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
            can_back: false,
            can_forward: false,
            buttons: BarButtons::Terminal,
            other_tabs: Vec::new(),
            pane_count: 0,
            zoomed: false,
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
            can_back: false,
            can_forward: false,
            buttons: BarButtons::Terminal,
            other_tabs: Vec::new(),
            pane_count: 0,
            zoomed: false,
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
            can_back: false,
            can_forward: false,
            buttons: BarButtons::Terminal,
            other_tabs: Vec::new(),
            pane_count: 0,
            zoomed: false,
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
        can_back: false,
        can_forward: false,
        buttons: BarButtons::Terminal,
        other_tabs: Vec::new(),
        pane_count: 0,
        zoomed: false,
    }
}

/// What the bar over an app's window (`pane`) shows: its title, and
/// nothing running in it to restart or stop.
fn app_bar_content(pane: &Pane) -> BarContent {
    BarContent {
        label: crate::window_title(&pane.window)
            .filter(|title| !title.trim().is_empty())
            .or_else(|| pane.app_command.clone())
            .unwrap_or_else(|| "app".to_owned()),
        note: None,
        status: BarStatus::Idle,
        can_restart: false,
        can_stop: false,
        can_back: false,
        can_forward: false,
        buttons: BarButtons::None,
        other_tabs: Vec::new(),
        pane_count: 0,
        zoomed: false,
    }
}

/// The address of the web page `pane` shows, if it shows one.
fn web_page(pane: &Pane) -> Option<&str> {
    let url = pane.app_command.as_deref()?;
    (pane.herdr.is_none() && crate::is_web_link(url)).then_some(url)
}

/// What the state of `pane`'s bar is kept by: the herdr pane it shows, for
/// a terminal, else its own leaf.
fn bar_key(pane: &Pane) -> String {
    match &pane.herdr {
        Some(herdr) => herdr.pane_id.clone(),
        None => format!("app:{}", pane.id),
    }
}

/// Whether a pane's bar shows: while its command is `status` (a job, or
/// something other than the shell, running), and while the pointer asks
/// for it (`pointer`, see `chrome::pointer_reveals`). Either way it floats
/// over the window, so it never resizes or moves one.
pub fn bar_wanted(status: BarStatus, pointer: bool) -> bool {
    pointer || status.runs()
}

/// Where one pane's bar stands between hidden and shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BarState {
    /// The bar is out (or on its way out).
    visible: bool,
    /// The fade it is in, while it still moves.
    fade: Option<Fade>,
    /// The pointer is over its pill.
    lit: bool,
    /// The fade between not lit and lit it is in, while that moves.
    lit_fade: Option<Fade>,
}

/// Where `fade` stands at `now`, or, done or none, where it ends: 1 when
/// `on`, else 0.
fn fade_at(fade: Option<Fade>, on: bool, now: Instant) -> f32 {
    match fade {
        Some(fade) if !fade.done(now) => fade.value(now),
        _ if on => 1.0,
        _ => 0.0,
    }
}

impl BarState {
    /// Its opacity at `now`.
    fn alpha(&self, now: Instant) -> f32 {
        fade_at(self.fade, self.visible, now)
    }

    /// How lit it is at `now`, 0 to 1.
    fn lit_level(&self, now: Instant) -> f32 {
        fade_at(self.lit_fade, self.lit, now)
    }

    /// Start lighting up or going back at `now`, from wherever a fade
    /// under way had got to.
    fn light(&mut self, lit: bool, now: Instant) {
        let from = self.lit_level(now);
        self.lit = lit;
        self.lit_fade = Some(Fade::from_value(now, anim::FAST, lit, from));
    }

    /// Start showing or hiding at `now`, going on from wherever a fade
    /// under way had got to; at once while nothing is to move (see
    /// `anim::motion`).
    fn flip(&mut self, visible: bool, now: Instant) {
        let from = self.alpha(now);
        self.visible = visible;
        let dur = match anim::motion() {
            anim::Motion::Off => Duration::ZERO,
            anim::Motion::Full | anim::Motion::Reduced => anim::SLOW,
        };
        self.fade = Some(Fade::from_value(now, dur, visible, from));
    }
}

/// How a pane's bar is to be drawn right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarLook {
    /// The bar is out, or still fading out.
    pub visible: bool,
    /// The bar is out and the pointer is over its pill.
    pub lit: bool,
    /// How lit it is, 0 to 1, in `anim::quantize` steps: it follows `lit`
    /// over `anim::FAST`.
    pub lit_level: f32,
    /// How far out it is, 0 (its grip) to 1, in `anim::quantize` steps.
    pub alpha: f32,
    /// How far it is still under its pane's top edge: 0 out,
    /// `chrome::BAR_SLIDE` at its grip.
    pub slide: i32,
}

/// The show/hide state of every pane's bar, by `bar_key`, moved on
/// each time the bars are looked at.
#[derive(Debug, Default)]
pub struct BarStates(Mutex<HashMap<String, BarState>>);

impl BarStates {
    /// Pane `key`'s bar at `now`: whether it is wanted given whether it is
    /// out already (`want`), and whether the pointer is over its pill
    /// (`over`).
    pub fn update(
        &self,
        key: &str,
        now: Instant,
        over: bool,
        want: impl FnOnce(bool) -> bool,
    ) -> BarLook {
        let mut states = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let state = states.entry(key.to_owned()).or_default();
        let visible = want(state.visible);
        if visible != state.visible {
            state.flip(visible, now);
        }
        let lit = visible && over;
        if lit != state.lit {
            state.light(lit, now);
        }
        if state.lit_fade.is_some_and(|fade| fade.done(now)) {
            state.lit_fade = None;
        }
        if state.fade.is_some_and(|fade| fade.done(now)) {
            state.fade = None;
        }
        let alpha = anim::quantize(state.alpha(now)) as f32 / 16.0;
        BarLook {
            visible: state.visible,
            lit: state.lit,
            lit_level: anim::quantize(state.lit_level(now)) as f32 / 16.0,
            alpha,
            slide: ((1.0 - alpha) * chrome::BAR_SLIDE as f32).round() as i32,
        }
    }

    /// Whether pane `key`'s bar is out over its window.
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

/// The key for one of the bar's actions, as the help lists it: the first
/// that runs `action`.
fn hint_for(help: &Help, action: Action) -> Option<String> {
    help.key_of(action, 0)
}

/// Why `button` can't be pressed on a bar showing `content`, if it can't.
pub fn button_unavailable(button: BarButton, content: &BarContent) -> Option<&'static str> {
    let web = matches!(content.buttons, BarButtons::Web { .. });
    // A web page's reload is there whenever the extension follows it.
    let follows = content.can_restart;
    match button {
        BarButton::Back | BarButton::Forward | BarButton::Reload if web && !follows => {
            Some("gerdr's browser extension doesn't follow this page")
        }
        BarButton::Back if !content.can_back => Some("nothing to go back to"),
        BarButton::Forward if !content.can_forward => Some("nothing to go forward to"),
        BarButton::Restart if !content.can_restart => Some(match content.status {
            BarStatus::Working => "it is being stopped",
            BarStatus::Running | BarStatus::Agent(_) => {
                "it wasn't started in this pane since gerdr started"
            }
            BarStatus::Idle | BarStatus::Exited => "nothing has run here to run again",
        }),
        BarButton::Stop if !content.can_stop => Some(match content.status {
            BarStatus::Working => "it is being restarted",
            _ => "nothing runs here",
        }),
        _ => None,
    }
}

/// What a bar's label says when the pointer rests on it at `rect`: all
/// of what runs, whether it runs again after a reboot (`restart`, when
/// the pane has a command kept for one), and what the pointer can do
/// there.
pub fn label_tooltip(label: &str, rect: Rect, restart: Option<bool>) -> Tooltip {
    let mut fields: Vec<(String, String)> = restart
        .map(|again| {
            let word = if again {
                "Runs again"
            } else {
                "Won't run again"
            };
            ("After a reboot".into(), word.into())
        })
        .into_iter()
        .collect();
    fields.extend([
        ("Drag".into(), "move the pane".into()),
        ("Double-click".into(), "zoom or unzoom".into()),
        ("Right-click".into(), "more".into()),
    ]);
    Tooltip::new(label, rect).fields(fields)
}

/// What a bar button says when the pointer rests on it at `rect`.
pub fn button_tooltip(button: BarButton, rect: Rect, help: &Help) -> Tooltip {
    let (text, what) = match button {
        BarButton::Scrollback => ("Select and copy", Some(Action::Scrollback)),
        BarButton::Restart => ("Restart", Some(Action::RestartPane)),
        BarButton::Stop => ("Stop", Some(Action::StopPane)),
        BarButton::Back => ("Back", None),
        BarButton::Forward => ("Forward", None),
        // The restart key loads a web page again.
        BarButton::Reload => ("Reload", Some(Action::RestartPane)),
        BarButton::CopyUrl => ("Copy URL", None),
        BarButton::Zoom => (
            "Zoom pane (or double-click the bar)",
            Some(Action::ZoomPane),
        ),
        BarButton::Unzoom => (
            "Unzoom pane (or double-click the bar)",
            Some(Action::ZoomPane),
        ),
        BarButton::Close => ("Close pane", Some(Action::ClosePane)),
    };
    let tip = Tooltip::new(text, rect);
    match what.and_then(|what| hint_for(help, what)) {
        Some(key) => tip.key(&key),
        None => tip,
    }
}

/// The menu a right press at `at` on the bar of pane `id` (showing
/// `content`) opens: splits, moves to other tabs, then what runs in it or
/// where its web page goes.
pub fn bar_menu(at: Point<f64, Logical>, id: u32, content: &BarContent, help: &Help) -> MenuSpec {
    let item = |label: &str, action, what, enabled| {
        let item = MenuItem::new(label, action).enabled(enabled);
        match hint_for(help, what) {
            Some(key) => item.hint(&key),
            None => item,
        }
    };
    let mut items = vec![
        item(
            "Split right",
            MenuAction::SplitPane(id, Dir::Right),
            Action::Split(Dir::Right),
            true,
        ),
        item(
            "Split down",
            MenuAction::SplitPane(id, Dir::Down),
            Action::Split(Dir::Down),
            true,
        ),
        item(
            "Flip split",
            MenuAction::FlipPane(id),
            Action::FlipPane,
            content.pane_count > 1,
        ),
        item(
            if content.zoomed {
                "Unzoom pane"
            } else {
                "Zoom pane"
            },
            MenuAction::ZoomPane(id),
            Action::ZoomPane,
            content.pane_count > 1,
        ),
        item(
            "Move to new tab",
            MenuAction::PaneToTab(id, None),
            Action::PaneToNewTab,
            // Alone in its tab, it has a tab of its own already.
            content.pane_count > 1,
        ),
    ];
    items.extend(content.other_tabs.iter().map(|(t, title)| {
        MenuItem::new(
            &format!("Move to tab: {title}"),
            MenuAction::PaneToTab(id, Some(*t)),
        )
    }));
    match content.buttons {
        BarButtons::Web { .. } => items.extend([
            MenuItem::new("Back", MenuAction::WebStep(id, WebStep::Back)).enabled(content.can_back),
            MenuItem::new("Forward", MenuAction::WebStep(id, WebStep::Forward))
                .enabled(content.can_forward),
            item(
                "Reload",
                MenuAction::WebStep(id, WebStep::Reload),
                Action::RestartPane,
                content.can_restart,
            ),
            MenuItem::new("Copy URL", MenuAction::CopyPaneUrl(id)),
        ]),
        _ => items.extend([
            item(
                "Select and copy",
                MenuAction::Scrollback(id),
                Action::Scrollback,
                true,
            ),
            item(
                "Restart",
                MenuAction::RestartPane(id),
                Action::RestartPane,
                content.can_restart,
            ),
            item(
                "Stop",
                MenuAction::StopPane(id),
                Action::StopPane,
                content.can_stop,
            ),
        ]),
    }
    items.push(item(
        "Close pane",
        MenuAction::ClosePane(id),
        Action::ClosePane,
        true,
    ));
    MenuSpec {
        anchor: Rectangle::new(at.to_i32_round(), (0, 0).into()),
        items,
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
    pub(crate) fn pane_bar_content(&self, pane_id: &str) -> BarContent {
        let pane = self.pane_state(pane_id);
        let mut content = bar_content(
            pane.and_then(|p| p.process.as_ref()),
            self.last_commands.get(pane_id),
            self.pane_jobs.get(pane_id),
        );
        let agent = pane.filter(|p| p.agent.is_some()).map(|p| p.agent_status);
        content.status = status_with_agent(content.status, agent);
        content
    }

    /// What runs in pane `id` of the active tab, as its bar says it, if
    /// a command runs there or is being stopped.
    pub fn running_in_pane(&self, id: u32) -> Option<String> {
        let pane = self.tab()?.panes.iter().find(|p| p.id == id)?;
        self.running_in(pane)
    }

    /// What runs in `pane`, as its bar says it, if a command runs there
    /// or is being stopped.
    pub fn running_in(&self, pane: &Pane) -> Option<String> {
        let content = self.pane_bar_of(pane);
        content.status.runs().then_some(content.label)
    }

    /// What the bar of `pane` shows: what runs in it, for a terminal;
    /// its address and where its page can go, for a web page the extension
    /// follows (one it doesn't can only have its address copied).
    fn pane_bar_of(&self, pane: &Pane) -> BarContent {
        if let Some(herdr) = &pane.herdr {
            return self.pane_bar_content(&herdr.pane_id);
        }
        let content = app_bar_content(pane);
        let Some(url) = web_page(pane) else {
            return content;
        };
        let follows = self.web_control.follows(pane.id);
        // Where the page couldn't say, the way is left open.
        let history = self.web_control.history(pane.id);
        let can_back = follows && history.back != Some(false);
        let can_forward = follows && history.forward != Some(false);
        BarContent {
            label: url.to_owned(),
            can_restart: follows,
            can_back,
            can_forward,
            // Back and forward only while there is somewhere to go.
            buttons: BarButtons::Web {
                history: can_back || can_forward,
            },
            ..content
        }
    }

    /// Where the bars stand between hidden and shown.
    fn bar_states(&self) -> &BarStates {
        &self.bar_states
    }

    /// Where the pill of pane `id` of `tab`, placed at `rect`, is when
    /// out (see `chrome::pane_bar_rect`).
    pub(crate) fn pane_pill_rect(&self, tab: &Tab, id: u32, rect: Rect) -> Rect {
        let kind = tab
            .panes
            .iter()
            .find(|p| p.id == id)
            .map_or(BarButtons::None, |pane| self.pane_bar_of(pane).buttons);
        chrome::pane_bar_rect(rect, kind)
    }

    /// Whether the bar of pane `id` of `tab` is out over its window:
    /// there to see and to click.
    fn pane_bar_drawn(&self, tab: &Tab, id: u32) -> bool {
        tab.panes
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|pane| self.bar_states().drawn(&bar_key(pane)))
    }

    /// The bar keys of every pane in every workspace: the bars whose
    /// state is kept, shown or not.
    fn bar_keys(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.iter())
            .map(bar_key)
            .collect()
    }

    /// The bars over the active tab's panes, moved on to now:
    /// a hidden one at rest has no height. The other tabs' bars keep
    /// their state meanwhile, so coming back moves nothing.
    pub fn pane_bar_views(&self) -> Vec<PaneBarView> {
        let states = self.bar_states();
        states.retain(&self.bar_keys());
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        let now = Instant::now();
        let pointer = self.pointer;
        let focused = tab.focused_id();
        tab.shown_rects(self.content_rect())
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = tab.panes.iter().find(|p| p.id == id)?;
                let key = bar_key(pane);
                let content = self.pane_bar_of(pane);
                let is_focused = focused == Some(id);
                let kind = content.buttons;
                // Its grip is drawn always, its pill over it.
                let full = chrome::pane_bar_rect(rect, kind);
                let over = full.to_f64().contains(pointer);
                let look = states.update(&key, now, over, |out| {
                    let near = chrome::pointer_reveals(rect, kind, out, pointer);
                    bar_wanted(content.status, near)
                });
                Some(PaneBarView {
                    rect: full,
                    grip: chrome::pane_bar_rects(rect).0,
                    label: content.label,
                    note: content.note,
                    status: content.status,
                    can_restart: content.can_restart,
                    can_stop: content.can_stop,
                    can_back: content.can_back,
                    can_forward: content.can_forward,
                    buttons: content.buttons,
                    zoom: tab.bar_zoom(id),
                    focused: is_focused,
                    lit: look.lit,
                    lit_level: look.lit_level,
                    hover: self
                        .pane_bar_hover
                        .filter(|(hovered, _)| *hovered == id)
                        .map(|(_, button)| button),
                    visible: look.visible,
                    // Its pill fades in over its band.
                    alpha: look.alpha,
                    slide: look.slide,
                })
            })
            .collect()
    }

    /// The pane of the active tab whose bar is at `pos`, and what of it.
    /// A bar's grip is all label, out or not; its pill, while out, has its
    /// buttons.
    pub fn pane_bar_at(&self, pos: Point<f64, Logical>) -> Option<(u32, BarHit)> {
        let tab = self.tab()?;
        tab.shown_rects(self.content_rect())
            .into_iter()
            .find_map(|(id, rect)| {
                let pane = tab.panes.iter().find(|p| p.id == id)?;
                let kind = self.pane_bar_of(pane).buttons;
                let hit = self.pane_bar_drawn(tab, id).then(|| {
                    let bar = chrome::pane_bar_rect(rect, kind);
                    chrome::pane_bar_hit(bar, kind, tab.bar_zoom(id), pos)
                });
                let band = chrome::pane_bar_rects(rect).0;
                let hit = hit
                    .flatten()
                    .or_else(|| band.to_f64().contains(pos).then_some(BarHit::Label))?;
                Some((id, hit))
            })
    }

    /// The zoomed pane of the active tab, and where the button floating
    /// over it to unzoom it is, if one is.
    pub fn unzoom_button(&self) -> Option<(u32, Rect)> {
        let tab = self.tab()?;
        let id = tab.zoomed()?;
        let (_, rect) = tab
            .shown_rects(self.content_rect())
            .into_iter()
            .find(|(pane, _)| *pane == id)?;
        Some((id, chrome::unzoom_button_rect(rect)?))
    }

    /// The zoomed pane whose unzoom button is at `pos`, if it is.
    pub fn unzoom_button_at(&self, pos: Point<f64, Logical>) -> Option<u32> {
        self.unzoom_button()
            .filter(|(_, rect)| rect.to_f64().contains(pos))
            .map(|(id, _)| id)
    }

    /// The pointer moved to `pos` over the app area with nothing open
    /// over it: the tooltip for the bar button or the unzoom button under
    /// it, if one is.
    pub(crate) fn pane_bar_motion(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        if let Some((_, rect)) = self
            .unzoom_button()
            .filter(|(_, rect)| rect.to_f64().contains(pos))
        {
            return Some(button_tooltip(BarButton::Unzoom, rect, &self.keymap.help()));
        }
        let (id, hit) = self.pane_bar_at(pos)?;
        let tab = self.tab()?;
        let pane = tab.panes.iter().find(|p| p.id == id)?;
        let content = self.pane_bar_of(pane);
        let kind = content.buttons;
        let (_, rect) = tab
            .shown_rects(self.content_rect())
            .into_iter()
            .find(|(pane, _)| *pane == id)?;
        let bar = chrome::pane_bar_rect(rect, kind);
        let BarHit::Button(button) = hit else {
            // Whether its command runs again after a reboot: what the
            // toggle's toast said, kept in sight.
            let restart = pane
                .herdr
                .as_ref()
                .and_then(|herdr| self.pane_record(herdr))
                .and_then(|r| Some(!r.skip.contains(r.command.as_ref()?)));
            return Some(label_tooltip(&content.label, bar, restart));
        };
        let (_, rect) = chrome::pane_bar_buttons(bar, kind, tab.bar_zoom(id))
            .into_iter()
            .find(|(b, _)| *b == button)?;
        let tip = button_tooltip(button, rect, &self.keymap.help());
        Some(match button_unavailable(button, &content) {
            Some(why) => tip.fields(vec![("Unavailable".into(), why.into())]),
            None => tip,
        })
    }

    /// The menu a right press at `pos` on a pane's bar opens, if any.
    pub(crate) fn pane_bar_menu(&self, pos: Point<f64, Logical>) -> Option<MenuSpec> {
        let (id, _) = self.pane_bar_at(pos)?;
        let tab = self.tab()?;
        let pane = tab.panes.iter().find(|p| p.id == id)?;
        let mut content = self.pane_bar_of(pane);
        // Here rather than in `pane_bar_content`, which every bar calls
        // every frame.
        let ws = self.workspace()?;
        content.other_tabs = self
            .tab_views(ws)
            .into_iter()
            .enumerate()
            .filter(|(t, _)| *t != ws.active_tab)
            .map(|(t, view)| (t, view.title))
            .collect();
        content.pane_count = tab.layout.as_ref().map_or(0, |l| l.leaves().len());
        content.zoomed = tab.zoomed() == Some(id);
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
        let step = match button {
            BarButton::Back => WebStep::Back,
            BarButton::Forward => WebStep::Forward,
            BarButton::Reload => WebStep::Reload,
            BarButton::CopyUrl => return self.copy_pane_url(id),
            BarButton::Scrollback => return self.open_scrollback(id),
            BarButton::Zoom | BarButton::Unzoom => return self.zoom_pane(id),
            BarButton::Close => return self.close_pane(),
            BarButton::Restart | BarButton::Stop => {
                let Some(pane_id) = self.tab().and_then(|tab| tab.herdr_pane(id)) else {
                    return;
                };
                let pane_id = pane_id.pane_id.clone();
                if button == BarButton::Restart {
                    self.restart_pane(&pane_id);
                } else {
                    self.stop_pane(&pane_id);
                }
                return;
            }
        };
        self.web_pane_step(id, step);
    }

    /// Copy the address of the web page pane `id` of the active tab shows.
    pub fn copy_pane_url(&mut self, id: u32) {
        let url = self
            .tab()
            .and_then(|tab| tab.panes.iter().find(|p| p.id == id))
            .and_then(web_page)
            .map(str::to_owned);
        match url {
            Some(url) => {
                self.copy_text(url);
                self.toast("Copied the link".into(), Level::Info);
            }
            None => self.show_notice("Not a web page".into()),
        }
    }

    /// The herdr pane of the active tab's focused pane, if it is a terminal.
    fn focused_terminal(&self) -> Option<String> {
        let tab = self.tab()?;
        tab.herdr_pane(tab.focused_id()?).map(|p| p.pane_id.clone())
    }

    /// Run the focused terminal's command again, or load the focused web
    /// page again.
    pub fn restart_focused_pane(&mut self) {
        if let Some(pane_id) = self.focused_terminal() {
            return self.restart_pane(&pane_id);
        }
        let page = self.tab().and_then(|tab| {
            let id = tab.focused_id()?;
            tab.panes
                .iter()
                .find(|p| p.id == id)
                .filter(|p| web_page(p).is_some())?;
            Some(id)
        });
        match page {
            Some(id) if self.web_pane_step(id, WebStep::Reload) => {}
            Some(_) => self.show_notice("gerdr can't reload this page".into()),
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
        let captured = self.last_commands.get(pane_id).map(|r| r.captured.clone());
        self.start_job(pane_id, content.label, captured.as_ref(), true);
    }

    /// The commands of workspace `w` to restart, each with its herdr
    /// pane, in the folder and environment it ran with: those that run
    /// again after a reboot (see `PaneRecord::restart_command`), and the
    /// services that exited since (see `Remembered::long`) unless the
    /// user said not to run them again. A pane where something else runs
    /// in front of its command (an editor, a pager) is left alone.
    pub fn workspace_restarts(&self, w: usize) -> Vec<(String, Captured)> {
        let Some(ws) = self.workspaces.get(w) else {
            return Vec::new();
        };
        ws.tabs
            .iter()
            .flat_map(Tab::herdr_panes)
            .filter_map(|pane| {
                let content = self.pane_bar_content(&pane.pane_id);
                if matches!(content.status, BarStatus::Running | BarStatus::Agent(_))
                    && !content.can_restart
                {
                    return None;
                }
                let record = self.pane_record(pane);
                let remembered = self.last_commands.get(&pane.pane_id);
                if let Some(record) = record
                    && let Some(command) = record.restart_command()
                {
                    let captured = match remembered {
                        Some(r) if r.captured.command == command => r.captured.clone(),
                        _ => Captured {
                            command: command.to_owned(),
                            cwd: record.cwd.clone().or_else(|| pane.cwd.clone()),
                            env: record.env.clone(),
                        },
                    };
                    return Some((pane.pane_id.clone(), captured));
                }
                let service = remembered.filter(|r| r.long)?;
                let skipped = record.is_some_and(|r| r.skip.contains(&service.captured.command));
                (content.status == BarStatus::Exited && !skipped)
                    .then(|| (pane.pane_id.clone(), service.captured.clone()))
            })
            .collect()
    }

    /// Stop each of `restarts` (commands, by herdr pane) and run it again,
    /// as a reboot would: after a pull or a checkout, for what doesn't
    /// reload by itself (see `restart`).
    pub fn restart_commands(&mut self, restarts: Vec<(String, Captured)>) {
        let count = restarts.len();
        let now = Instant::now();
        for (pane_id, captured) in restarts {
            // A service from now on, crashing again or not.
            match self.last_commands.get_mut(&pane_id) {
                Some(r) if r.captured.command == captured.command => r.long = true,
                _ => {
                    let service = Remembered::service(captured.clone(), now);
                    self.last_commands.insert(pane_id.clone(), service);
                }
            }
            if let Some(job) = self.pane_jobs.get(&pane_id) {
                if job.can_rerun {
                    job.rerun.store(true, Ordering::Relaxed);
                }
                continue;
            }
            let label =
                chrome::command_line(&captured.command).unwrap_or_else(|| captured.command.clone());
            self.start_job(&pane_id, label, Some(&captured), true);
        }
        self.show_notice(match count {
            1 => "Restarting 1 command".to_owned(),
            n => format!("Restarting {n} commands"),
        });
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
        let captured = self.last_commands.get(pane_id).map(|r| r.captured.clone());
        self.start_job(pane_id, content.label, captured.as_ref(), false);
    }

    /// Stop what runs in herdr pane `pane_id`, called `command`, and run
    /// `captured` (its last command) again if `rerun`.
    fn start_job(
        &mut self,
        pane_id: &str,
        command: String,
        captured: Option<&Captured>,
        rerun: bool,
    ) {
        let Some(socket) = self.herdr.as_ref().map(|h| h.socket().to_path_buf()) else {
            return;
        };
        let Some(tx) = self.job_sender() else {
            return;
        };
        // No tools: asking poetry or pipenv mustn't hold up the main thread.
        let line = captured
            .map(|c| Plan::new(&restart_prep(pane_id, c), &Tools::default()))
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
                    Outcome::GaveUp => self.show_error(format!("Couldn't stop {command}")),
                    Outcome::Failed => self.show_error(format!("Couldn't restart {command}")),
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
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "npm run dev")),
            Some(cwd),
            Instant::now(),
        );
        let r = &memory["p1"];
        assert_eq!((r.pid, r.captured.command.as_str()), (7, "npm run dev"));
        assert_eq!(r.captured.cwd.as_deref(), Some(cwd));
        // Its own folder, when it ran elsewhere, wins.
        let elsewhere = Foreground::Busy {
            pid: 8,
            captured: captured("make", Some("/w/lib"), &[]),
        };
        remember(
            &mut memory,
            "p1",
            Some(&elsewhere),
            Some(cwd),
            Instant::now(),
        );
        assert_eq!(
            memory["p1"].captured.cwd.as_deref(),
            Some(Path::new("/w/lib"))
        );
    }

    #[test]
    fn a_command_running_long_is_a_service_and_its_reruns_too() {
        let mut memory = HashMap::new();
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "celery worker")),
            None,
            start,
        );
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "celery worker")),
            None,
            at(59),
        );
        assert!(!memory["p1"].long);
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "celery worker")),
            None,
            at(60),
        );
        assert!(memory["p1"].long);
        // It crashed; run again, it is one at once, however soon it ends.
        remember(&mut memory, "p1", Some(&Foreground::Idle), None, at(70));
        remember(
            &mut memory,
            "p1",
            Some(&busy(8, "celery worker")),
            None,
            at(80),
        );
        assert!(memory["p1"].long);
        // Another command isn't, until it has run long itself.
        remember(&mut memory, "p1", Some(&busy(9, "git pull")), None, at(90));
        assert!(!memory["p1"].long);
        remember(
            &mut memory,
            "p1",
            Some(&busy(10, "celery worker")),
            None,
            at(95),
        );
        assert!(!memory["p1"].long);
    }

    #[test]
    fn a_service_run_again_by_gerdr_stays_one() {
        let mut memory = HashMap::new();
        let now = Instant::now();
        let service = Remembered::service(captured("npm run dev", None, &[]), now);
        assert_eq!((service.pid, service.long), (0, true));
        memory.insert("p1".to_owned(), service);
        remember(&mut memory, "p1", Some(&busy(7, "npm run dev")), None, now);
        assert_eq!((memory["p1"].pid, memory["p1"].long), (7, true));
    }

    #[test]
    fn the_same_leader_keeps_its_first_command() {
        let mut memory = HashMap::new();
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "uvicorn app:app")),
            None,
            Instant::now(),
        );
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "uvicorn worker")),
            None,
            Instant::now(),
        );
        assert_eq!(memory["p1"].captured.command, "uvicorn app:app");
    }

    #[test]
    fn idle_and_denied_keep_the_last_command() {
        let mut memory = HashMap::new();
        remember(
            &mut memory,
            "p1",
            Some(&busy(7, "npm run dev")),
            None,
            Instant::now(),
        );
        remember(
            &mut memory,
            "p1",
            Some(&Foreground::Idle),
            None,
            Instant::now(),
        );
        let denied = Foreground::Untracked {
            pid: 9,
            denied: true,
        };
        remember(&mut memory, "p1", Some(&denied), None, Instant::now());
        remember(&mut memory, "p1", None, None, Instant::now());
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
            since: Instant::now(),
            long: false,
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
    fn an_agent_runs_only_at_work_as_its_bar_says() {
        use AgentStatus::{Blocked, Done, Idle, Unknown, Working};
        let running = BarStatus::Running;
        assert_eq!(status_with_agent(running, None), running);
        assert_eq!(status_with_agent(running, Some(Working)), running);
        for agent in [Idle, Blocked, Done, Unknown] {
            let status = status_with_agent(running, Some(agent));
            assert_eq!(status, BarStatus::Agent(agent));
            // Still something to ask about before closing, in sight.
            assert!(status.runs());
            assert!(bar_wanted(status, false));
        }
        // Gone or being stopped, the agent's word doesn't count.
        for other in [BarStatus::Idle, BarStatus::Exited, BarStatus::Working] {
            assert_eq!(status_with_agent(other, Some(Idle)), other);
        }
    }

    #[test]
    fn bar_content_of_something_else_running() {
        let remembered = Remembered {
            pid: 7,
            captured: captured("npm run dev", None, &[]),
            since: Instant::now(),
            long: false,
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
            since: Instant::now(),
            long: false,
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
    fn a_bar_shows_for_a_command_or_the_pointer() {
        assert!(bar_wanted(BarStatus::Running, false));
        assert!(bar_wanted(BarStatus::Working, false));
        assert!(!bar_wanted(BarStatus::Idle, false));
        assert!(!bar_wanted(BarStatus::Exited, false));
        assert!(bar_wanted(BarStatus::Idle, true));
        assert!(bar_wanted(BarStatus::Exited, true));
        assert!(bar_wanted(BarStatus::Running, true));
    }

    #[test]
    fn a_bar_fades_in_when_wanted_and_out_when_not() {
        let states = BarStates::default();
        let t0 = Instant::now();
        let out = |out: bool| out;
        // Not wanted: hidden from the start, nothing to see.
        let look = states.update("p1", t0, false, |shown| {
            assert!(!shown);
            false
        });
        assert_eq!((look.visible, look.alpha), (false, 0.0));
        assert_eq!(look.slide, chrome::BAR_SLIDE);
        assert!(!states.drawn("p1"));
        // Wanted: it starts showing.
        let look = states.update("p1", t0, false, |_| true);
        assert!(look.visible && look.alpha == 0.0);
        assert!(states.drawn("p1"));
        let look = states.update("p1", t0 + anim::SLOW / 2, false, out);
        assert!(look.alpha > 0.3 && look.alpha < 0.7, "{}", look.alpha);
        assert!(look.slide > 0 && look.slide < chrome::BAR_SLIDE);
        let look = states.update("p1", t0 + anim::SLOW, false, |_| true);
        assert_eq!((look.alpha, look.slide), (1.0, 0));
        // Not wanted any more: it fades out.
        let t1 = t0 + Duration::from_secs(2);
        let look = states.update("p1", t1, false, |_| false);
        assert!(!look.visible && look.alpha == 1.0);
        // On its way out it is not there to click.
        assert!(!states.drawn("p1"));
        let look = states.update("p1", t1 + 2 * anim::SLOW, false, |_| false);
        assert_eq!((look.alpha, look.slide), (0.0, chrome::BAR_SLIDE));
    }

    #[test]
    fn a_bar_lights_up_under_the_pointer_over_a_fade() {
        let states = BarStates::default();
        let t0 = Instant::now();
        let look = states.update("p1", t0, false, |_| true);
        assert_eq!((look.lit, look.lit_level), (false, 0.0));
        // The pointer comes over its pill: lit at once, its level after.
        let look = states.update("p1", t0, true, |_| true);
        assert!(look.lit && look.lit_level == 0.0);
        let look = states.update("p1", t0 + anim::FAST / 2, true, |_| true);
        assert!(
            look.lit_level > 0.2 && look.lit_level < 0.8,
            "{}",
            look.lit_level
        );
        let look = states.update("p1", t0 + anim::FAST, true, |_| true);
        assert_eq!(look.lit_level, 1.0);
        // It leaves: back down over the same while.
        let t1 = t0 + Duration::from_secs(1);
        let look = states.update("p1", t1, false, |_| true);
        assert!(!look.lit && look.lit_level == 1.0);
        let look = states.update("p1", t1 + anim::FAST, false, |_| true);
        assert_eq!(look.lit_level, 0.0);
        // A bar not out isn't lit, the pointer over it or not.
        let look = states.update("p1", t1 + anim::FAST, true, |_| false);
        assert!(!look.lit);
    }

    #[test]
    fn a_fade_turned_around_goes_on_from_where_it_was() {
        let states = BarStates::default();
        let t0 = Instant::now();
        states.update("p1", t0, false, |_| true);
        let half = states.update("p1", t0 + anim::SLOW / 2, false, |_| true);
        let back = states.update("p1", t0 + anim::SLOW / 2, false, |_| false);
        assert!(!back.visible);
        assert!(
            (back.alpha - half.alpha).abs() < 0.1,
            "{} {}",
            back.alpha,
            half.alpha
        );
        let gone = states.update("p1", t0 + anim::SLOW, false, |_| false);
        assert_eq!(gone.alpha, 0.0);
    }

    #[test]
    fn forgotten_bars_start_hidden() {
        let states = BarStates::default();
        let t0 = Instant::now();
        states.update("p1", t0, false, |_| true);
        states.retain(&["p1".into()]);
        assert!(states.drawn("p1"));
        states.retain(&[]);
        assert!(!states.drawn("p1"));
    }

    #[test]
    fn unavailable_buttons_say_why() {
        let idle = bar_content(Some(&at_prompt()), None, None);
        assert_eq!(
            button_unavailable(BarButton::Restart, &idle),
            Some("nothing has run here to run again")
        );
        assert_eq!(
            button_unavailable(BarButton::Stop, &idle),
            Some("nothing runs here")
        );
        assert_eq!(button_unavailable(BarButton::Scrollback, &idle), None);
        let web = BarContent {
            buttons: BarButtons::Web { history: true },
            ..idle.clone()
        };
        let why = button_unavailable(BarButton::Back, &web).unwrap();
        assert!(why.contains("extension"), "{why}");
        assert!(button_unavailable(BarButton::Reload, &web).is_some());
        let followed = BarContent {
            can_restart: true,
            can_forward: true,
            ..web
        };
        assert_eq!(
            button_unavailable(BarButton::Back, &followed),
            Some("nothing to go back to")
        );
        assert_eq!(button_unavailable(BarButton::Forward, &followed), None);
        assert_eq!(button_unavailable(BarButton::Reload, &followed), None);
        let tip = label_tooltip("npm run dev", Rect::default(), None);
        assert_eq!(tip.text, "npm run dev");
        assert!(tip.fields.iter().any(|(k, _)| k == "Double-click"));
        assert!(!tip.fields.iter().any(|(k, _)| k == "After a reboot"));
        let skipped = label_tooltip("npm run dev", Rect::default(), Some(false));
        assert_eq!(
            skipped.fields[0],
            ("After a reboot".into(), "Won't run again".into())
        );
        let kept = label_tooltip("npm run dev", Rect::default(), Some(true));
        assert_eq!(kept.fields[0].1, "Runs again");
    }

    #[test]
    fn key_hints_come_from_the_help() {
        let help = crate::keys::Keymap::default().help();
        assert_eq!(
            hint_for(&help, Action::RestartPane).as_deref(),
            Some("Ctrl+B A")
        );
        assert_eq!(
            hint_for(&help, Action::StopPane).as_deref(),
            Some("Ctrl+B S")
        );
        assert_eq!(
            hint_for(&help, Action::Split(Dir::Right)).as_deref(),
            Some("Ctrl+B V")
        );
        assert_eq!(
            hint_for(&help, Action::Split(Dir::Down)).as_deref(),
            Some("Ctrl+B -")
        );
        assert_eq!(
            hint_for(&help, Action::ClosePane).as_deref(),
            Some("Ctrl+B X")
        );
        // An action on a line of its own, with several keys: the first.
        let restart = Some(crate::chrome::HelpPress::Action(Action::RestartPane));
        let own = Help {
            prefix: "Ctrl+A".into(),
            rows: vec![("r / Shift+R".into(), "Restart the pane's command".into())],
            presses: vec![vec![restart; 2]],
        };
        assert_eq!(
            hint_for(&own, Action::RestartPane).as_deref(),
            Some("Ctrl+A r")
        );
        assert_eq!(hint_for(&own, Action::StopPane), None);
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
        let content = BarContent {
            other_tabs: vec![(0, "zsh".into()), (2, "vim notes".into())],
            pane_count: 2,
            ..bar_content(Some(&at_prompt()), None, None)
        };
        let menu = bar_menu((300.5, 60.2).into(), 7, &content, &help);
        assert_eq!(menu.anchor, Rect::new((301, 60).into(), (0, 0).into()));
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Split right",
                "Split down",
                "Flip split",
                "Zoom pane",
                "Move to new tab",
                "Move to tab: zsh",
                "Move to tab: vim notes",
                "Select and copy",
                "Restart",
                "Stop",
                "Close pane"
            ]
        );
        let actions: Vec<&MenuAction> = menu.items.iter().map(|i| &i.action).collect();
        assert_eq!(
            actions,
            [
                &MenuAction::SplitPane(7, Dir::Right),
                &MenuAction::SplitPane(7, Dir::Down),
                &MenuAction::FlipPane(7),
                &MenuAction::ZoomPane(7),
                &MenuAction::PaneToTab(7, None),
                &MenuAction::PaneToTab(7, Some(0)),
                &MenuAction::PaneToTab(7, Some(2)),
                &MenuAction::Scrollback(7),
                &MenuAction::RestartPane(7),
                &MenuAction::StopPane(7),
                &MenuAction::ClosePane(7),
            ]
        );
        let enabled: Vec<bool> = menu.items.iter().map(|i| i.enabled).collect();
        assert_eq!(
            enabled,
            [
                true, true, true, true, true, true, true, true, false, false, true
            ]
        );
        let hints: Vec<Option<&str>> = menu.items.iter().map(|i| i.hint.as_deref()).collect();
        assert_eq!(
            hints[2..5],
            [Some("Ctrl+B Space"), Some("Ctrl+B Z"), Some("Ctrl+B !")]
        );
        assert_eq!(hints[5..7], [None, None]);
        assert_eq!(hints[7..9], [Some("Ctrl+B ["), Some("Ctrl+B A")]);
        // With a command running: stop; with one exited: restart. A pane
        // alone in its tab has no split to flip, nothing to zoom over, and
        // no other tab to go to.
        let running = bar_content(Some(&running(9, "npm run dev")), None, None);
        let menu = bar_menu((0.0, 0.0).into(), 1, &running, &help);
        assert!(!menu.items[2].enabled && !menu.items[3].enabled);
        assert_eq!(menu.items[6].label, "Restart");
        assert!(!menu.items[6].enabled && menu.items[7].enabled);
        // A zoomed pane's menu unzooms it.
        let zoomed = BarContent {
            zoomed: true,
            ..content
        };
        let menu = bar_menu((0.0, 0.0).into(), 7, &zoomed, &help);
        assert_eq!(menu.items[3].label, "Unzoom pane");
        assert_eq!(menu.items[3].action, MenuAction::ZoomPane(7));
    }

    #[test]
    fn a_web_pages_bar_menu_goes_back_forward_and_copies() {
        let help = crate::keys::Keymap::default().help();
        let content = BarContent {
            can_restart: true,
            can_back: true,
            can_forward: false,
            buttons: BarButtons::Web { history: true },
            ..bar_content(None, None, None)
        };
        let menu = bar_menu((0.0, 0.0).into(), 3, &content, &help);
        let tail: Vec<(&str, &MenuAction, bool)> = menu.items[5..]
            .iter()
            .map(|i| (i.label.as_str(), &i.action, i.enabled))
            .collect();
        assert_eq!(
            tail,
            [
                ("Back", &MenuAction::WebStep(3, WebStep::Back), true),
                ("Forward", &MenuAction::WebStep(3, WebStep::Forward), false),
                ("Reload", &MenuAction::WebStep(3, WebStep::Reload), true),
                ("Copy URL", &MenuAction::CopyPaneUrl(3), true),
                ("Close pane", &MenuAction::ClosePane(3), true),
            ]
        );
        // Reload is what the restart key does on a web page.
        assert_eq!(menu.items[7].hint.as_deref(), Some("Ctrl+B A"));
    }
}
