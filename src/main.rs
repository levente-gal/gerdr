//! gerdr: a nested Wayland compositor that shows apps inside a single host
//! window. A sidebar lists workspaces (project folders); each workspace has
//! tabs. A workspace's first tab tiles Zed (top 70%) over a terminal (bottom
//! 30%); further tabs start with one terminal. Like herdr, any pane can be
//! split right or down; the new half gets a terminal in the workspace folder.
//! Terminals live in a herdr session of gerdr's own (see `herdr`), so they
//! keep running when gerdr quits and come back when it starts again. Any
//! other Wayland app can be launched into a split too (see `launch`).

mod agents;
mod anim;
mod app_icons;
mod chrome;
mod clipboard;
mod config;
mod cursor_agents;
mod desktop;
mod frecency;
mod fuzzy;
mod git;
mod git_ops;
mod handlers;
mod herdr;
mod hooks;
mod input;
mod keys;
mod launch;
mod layout;
mod menu;
mod notify;
mod pane_control;
mod pane_drag;
mod pane_moves;
mod paths;
mod prepare;
mod project_list;
mod project_menu;
mod projects;
mod quit;
mod reload;
mod rename;
mod repo_batches;
mod repos;
mod sidebar;
mod switcher;
mod system_theme;
mod tab_strip;
mod text_field;
mod toast;
mod tooltip;
mod web_pages;
mod welcome;
mod worktrees;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use chrome::{
    AgentView, BarButton, Chrome, ChromeView, Density, PlaceholderView, SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH, TabView, Theme, ThemeMode, WorkspaceView,
};
use frecency::Frecency;
use hooks::Hook;
use layout::{Dir, Node, SavedLeaf, SavedNode, Side};
use menu::{ContextMenu, MenuAction};
use paths::tilde;
use projects::{PaneRecord, RunState, TabRecord, VenvSetting, WorkspaceRecord};
use rename::Rename;
use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{
            ImportDma, damage::OutputDamageTracker,
            element::memory::MemoryRenderBufferRenderElement, gles::GlesRenderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState, keyboard::Keycode, pointer::CursorIcon},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, LoopSignal, PostAction, RegistrationToken,
            channel::{self, Channel},
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason, ObjectId},
            protocol::wl_surface::WlSurface,
        },
        winit::{platform::wayland::WindowAttributesExtWayland, window::WindowAttributes},
    },
    utils::{Logical, Point, Rectangle, Size, Transform},
    wayland::{
        compositor::{
            CompositorClientState, CompositorState, TraversalAction, with_states,
            with_surface_tree_downward,
        },
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufFeedbackBuilder, DmabufState},
        fractional_scale::{FractionalScaleManagerState, with_fractional_scale},
        output::OutputManagerState,
        selection::{
            data_device::DataDeviceState,
            ext_data_control::DataControlState as ExtDataControlState,
            primary_selection::PrimarySelectionState, wlr_data_control::DataControlState,
        },
        shell::xdg::{XdgShellState, XdgToplevelSurfaceData, decoration::XdgDecorationState},
        shm::ShmState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
};
use toast::{Level, Toast};
use tooltip::TooltipState;

/// Share of the content height given to Zed in the first tab, at start.
const TOP_RATIO: f64 = 0.7;
/// Share a split pane keeps; the new pane gets the rest.
const SPLIT_RATIO: f64 = 0.5;
/// Gap between the panes, in logical pixels.
const DIVIDER: i32 = 2;
/// How far from a divider (sidebar edge or pane gap) the pointer can grab it.
const GRAB: f64 = 4.0;
/// How far a held workspace row or tab must move before it is dragged.
const REORDER_START: f64 = 4.0;
/// Smallest pane size a divider drag leaves, and the app area width the
/// sidebar always leaves.
const MIN_PANE: i32 = 80;
const REAP_INTERVAL: Duration = Duration::from_millis(500);
/// How often the agent cards are refreshed from herdr, for what changes
/// without an event (terminal titles).
const AGENT_INTERVAL: Duration = Duration::from_secs(1);
/// A frame that takes longer to draw is logged.
const SLOW_FRAME: Duration = Duration::from_millis(20);
/// How often the Git status of the workspace folders is read again.
const GIT_INTERVAL: Duration = Duration::from_secs(3);
/// How often the web pages' windows are asked where they are now (see
/// `web_pages`).
const WEB_INTERVAL: Duration = Duration::from_secs(3);
/// How long a window may still show up after the process we started exited
/// (the `zed` CLI hands the window off to an already running editor and quits).
const HANDOFF_GRACE: Duration = Duration::from_secs(10);
/// How long a pane's shell must stay idle (or the pane gone) before the
/// command it ran is forgotten: panes dying while the machine shuts down
/// mustn't count.
const FORGET_IDLE: Duration = Duration::from_secs(10);
/// How long the tab layouts must differ from what was saved of them before
/// they are saved again: windows closing as the machine shuts down mustn't
/// count.
const TABS_SETTLE: Duration = Duration::from_secs(3);
/// How often the workspace records are brought up to date (see
/// `Gerdr::keep_records`).
const RECORDS_INTERVAL: Duration = Duration::from_secs(1);
/// How long the windows of a project switched away from (or of gerdr
/// quitting) have to close, say to save their work, before their processes
/// are ended.
const CLOSE_GRACE: Duration = Duration::from_millis(1500);
/// How long a new project's Zed waits, after that, for the old Zed to exit:
/// a `zed -n` would hand its window to it.
const ZED_EXIT_WAIT: Duration = Duration::from_secs(3);
/// How long a window started for a tab may take to show up before the
/// placeholder kept for it says so.
pub const EXPECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an agent stays blocked, out of sight, before its notification
/// is sent again.
const BLOCKED_REMINDER: Duration = Duration::from_secs(60);
/// How long a restarted server may take to bring back all its workspaces.
const RESTORE_WAIT: Duration = Duration::from_secs(5);
/// How long after a server restart the commands saved for its panes are
/// kept whatever the panes look like: they are still being run again.
const RESTORE_PROTECT: Duration = Duration::from_secs(30);
/// How long a key is held before it repeats (ms), and how often it then
/// repeats (per second), for the apps and for gerdr's own popups.
pub const REPEAT_DELAY: u32 = 300;
pub const REPEAT_RATE: u32 = 30;

pub struct Pane {
    /// The pane's leaf in the tab's layout.
    pub id: u32,
    pub window: Window,
    /// The herdr pane it shows, for a terminal.
    pub herdr: Option<herdr::PaneInfo>,
    /// The command it was launched with from the run prompt, for an app;
    /// for a web page, the link to where it is now (see `web_pages`).
    pub app_command: Option<String>,
    /// Neither started by gerdr nor by what it started: opened from a shell
    /// of the project's. Held for the project when switching away.
    pub stray: bool,
}

/// A process we started for a tab whose window hasn't shown up yet.
pub struct Expected {
    /// `None` while queued: windows told apart by app_id only are started
    /// one at a time (see `launch_next`).
    pub pid: Option<u32>,
    /// Lowercase substring of the app_id, for windows not opened by `pid`
    /// or a process it started (the `zed` CLI hands off to a separate editor
    /// process, Ghostty and Alacritty to gerdr's own; see `launch`). Empty
    /// for any other app launched by command: only its own processes'
    /// windows count. A web app's names its site (see `launch`).
    pub app_id: String,
    /// The layout leaf kept free for the window.
    pub pane: u32,
    /// Whether the window takes keyboard focus when it shows up.
    pub focus: bool,
    /// When `pid` exited; the expectation is dropped `HANDOFF_GRACE` later.
    pub exited: Option<Instant>,
    /// The herdr pane the window will show, for a terminal.
    pub herdr: Option<herdr::PaneInfo>,
    /// The command it was launched with from the run prompt, for an app.
    pub app_command: Option<String>,
    /// When the process was started (or queued).
    pub started: Instant,
    /// The process has run for `EXPECT_TIMEOUT` without a window, or (a
    /// Zed's) exited without one (see `reap_children`); the placeholder says
    /// so and offers to retry or close. Without a pid, it is waited for no
    /// more (see `Expected::is_stopped`).
    pub timed_out: bool,
    /// A Zed window of the pane went away without gerdr closing it (see
    /// `remove_window`): its placeholder says so.
    pub closed: bool,
}

impl Expected {
    fn new(pid: Option<u32>, app_id: &str, pane: u32, focus: bool) -> Self {
        Self {
            pid,
            app_id: app_id.to_owned(),
            pane,
            focus,
            exited: None,
            herdr: None,
            app_command: None,
            started: Instant::now(),
            timed_out: false,
            closed: false,
        }
    }

    /// Whether nothing is started for it, nor will be, until its placeholder's
    /// Retry: a Zed that didn't start or whose window went away. Its leaf is
    /// kept so the layout stays.
    fn is_stopped(&self) -> bool {
        self.pid.is_none() && self.timed_out
    }

    /// Whether, at `now`, the process has run for `EXPECT_TIMEOUT` with
    /// no window yet: it was started (not queued), still runs, and the
    /// wait is up.
    fn is_overdue(&self, now: Instant) -> bool {
        self.pid.is_some()
            && self.exited.is_none()
            && now.duration_since(self.started) > EXPECT_TIMEOUT
    }

    /// What the placeholder for its window says is starting.
    fn label(&self) -> String {
        let program = |command: &str| {
            command
                .split_whitespace()
                .next()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
        };
        match self.app_id.as_str() {
            "zed" => "Zed".into(),
            "alacritty" | "ghostty" => "terminal".into(),
            "" => self
                .app_command
                .as_deref()
                .and_then(program)
                .unwrap_or_else(|| "app".into()),
            other => other.to_owned(),
        }
    }

    /// A terminal started as `pid`, showing the herdr pane `herdr` if any.
    fn terminal(pid: u32, pane: u32, focus: bool, herdr: Option<herdr::PaneInfo>) -> Self {
        Self {
            herdr,
            ..Self::new(Some(pid), "alacritty", pane, focus)
        }
    }
}

#[derive(Default)]
pub struct Tab {
    /// Where the panes go: one leaf per window, shown or still expected.
    pub layout: Option<Node>,
    pub panes: Vec<Pane>,
    pub expected: Vec<Expected>,
    /// Window that gets keyboard focus when the tab is shown.
    pub focused: Option<Window>,
    /// The name it was given, shown ahead of the title derived from what
    /// runs in it.
    pub label: Option<String>,
}

impl Tab {
    /// A tab holding one terminal, started as `pid`, in pane `id`.
    fn terminal(pid: u32, id: u32, herdr: Option<herdr::PaneInfo>) -> Self {
        Self {
            layout: Some(Node::Leaf(id)),
            expected: vec![Expected::terminal(pid, id, true, herdr)],
            ..Default::default()
        }
    }

    /// The herdr panes of the tab: shown, then still starting.
    fn herdr_panes(&self) -> impl Iterator<Item = &herdr::PaneInfo> {
        self.panes
            .iter()
            .filter_map(|p| p.herdr.as_ref())
            .chain(self.expected.iter().filter_map(|e| e.herdr.as_ref()))
    }

    /// The herdr tab this one mirrors, while it has herdr panes.
    fn herdr_tab_id(&self) -> Option<&str> {
        self.herdr_panes().next().map(|p| p.tab_id.as_str())
    }

    /// The herdr pane a new split goes next to: the focused one if it is a
    /// terminal, else the last one.
    fn herdr_split_target(&self) -> Option<&herdr::PaneInfo> {
        let focused = self
            .focused
            .as_ref()
            .and_then(|w| self.panes.iter().find(|p| &p.window == w));
        focused
            .and_then(|p| p.herdr.as_ref())
            .or_else(|| self.herdr_panes().last())
    }

    /// The herdr pane the tab's restart marker and toggle are about: the
    /// focused one if it is a terminal, else the first.
    fn restart_target(&self) -> Option<&herdr::PaneInfo> {
        let focused = self
            .focused
            .as_ref()
            .and_then(|w| self.panes.iter().find(|p| &p.window == w));
        focused
            .and_then(|p| p.herdr.as_ref())
            .or_else(|| self.herdr_panes().next())
    }

    /// What to remember of the tab: its layout, with each leaf named by what
    /// fills it. Windows gerdr can't bring back are left out.
    fn save(&self) -> Option<TabRecord> {
        let leaf = |id: u32| match self.panes.iter().find(|p| p.id == id) {
            Some(p) => saved_leaf(
                p.herdr.as_ref(),
                p.app_command.as_deref(),
                is_zed(&p.window),
            ),
            None => {
                let e = self.expected.iter().find(|e| e.pane == id)?;
                saved_leaf(
                    e.herdr.as_ref(),
                    e.app_command.as_deref(),
                    e.app_id == "zed",
                )
            }
        };
        Some(TabRecord {
            herdr_tab: self.herdr_tab_id().map(str::to_owned),
            layout: self.layout.as_ref()?.save(&leaf)?,
            label: self.label.clone(),
        })
    }

    /// The herdr pane pane `id` shows, if it is a terminal.
    fn herdr_pane(&self, id: u32) -> Option<&herdr::PaneInfo> {
        self.panes.iter().find(|p| p.id == id)?.herdr.as_ref()
    }

    /// Whether pane `id` has a bar over it: a terminal's, to restart and
    /// stop what runs in it (see `pane_control`).
    fn has_bar(&self, id: u32) -> bool {
        self.herdr_pane(id).is_some()
    }

    fn window(&self, id: u32) -> Option<&Window> {
        self.panes.iter().find(|p| p.id == id).map(|p| &p.window)
    }

    fn focused_id(&self) -> Option<u32> {
        let focused = self.focused.as_ref()?;
        self.panes
            .iter()
            .find(|p| &p.window == focused)
            .map(|p| p.id)
    }

    /// Panes that have a window, in layout order.
    fn shown(&self) -> Vec<u32> {
        self.layout
            .as_ref()
            .map_or(Vec::new(), Node::leaves)
            .into_iter()
            .filter(|id| self.window(*id).is_some())
            .collect()
    }

    /// Where the panes that have a window go in `area`.
    fn shown_rects(&self, area: Rectangle<i32, Logical>) -> Vec<(u32, Rectangle<i32, Logical>)> {
        let rects = self
            .layout
            .as_ref()
            .map_or(Vec::new(), |l| l.rects(area, DIVIDER));
        rects
            .into_iter()
            .filter(|(id, _)| self.window(*id).is_some())
            .collect()
    }

    /// Where the panes still waiting for their window go in `area`, with
    /// what is expected there.
    fn expected_rects(
        &self,
        area: Rectangle<i32, Logical>,
    ) -> Vec<(&Expected, Rectangle<i32, Logical>)> {
        let rects = self
            .layout
            .as_ref()
            .map_or(Vec::new(), |l| l.rects(area, DIVIDER));
        rects
            .into_iter()
            .filter(|(id, _)| self.window(*id).is_none())
            .filter_map(|(id, rect)| {
                let expected = self.expected.iter().find(|e| e.pane == id)?;
                Some((expected, rect))
            })
            .collect()
    }

    /// Split pane `target` (the focused one if `None`, else the last) and
    /// give the new half to `new`; an empty tab just gets `new`. `dir` `None`:
    /// split along the longer side.
    fn split(
        &mut self,
        target: Option<u32>,
        new: u32,
        dir: Option<Dir>,
        area: Rectangle<i32, Logical>,
    ) {
        let target = target
            .or_else(|| self.focused_id())
            .or_else(|| self.layout.as_ref()?.leaves().last().copied());
        let Some((target, layout)) = target.zip(self.layout.as_mut()) else {
            self.layout = Some(Node::Leaf(new));
            return;
        };
        let dir = dir.unwrap_or_else(|| {
            let rect = layout
                .rects(area, DIVIDER)
                .into_iter()
                .find(|(id, _)| *id == target)
                .map(|(_, r)| r);
            if rect.is_some_and(|r| r.size.h > r.size.w) {
                Dir::Down
            } else {
                Dir::Right
            }
        });
        if !layout.split(target, new, dir, SPLIT_RATIO) {
            // No such pane: give the new one half of the whole tab.
            let old = std::mem::replace(layout, Node::Leaf(new));
            *layout = Node::Split {
                dir,
                ratio: SPLIT_RATIO,
                first: Box::new(old),
                second: Box::new(Node::Leaf(new)),
            };
        }
    }
}

/// How a leaf is remembered: by the herdr pane it shows, the command an app
/// was launched with, or as the workspace's Zed. A plain terminal (without
/// herdr) can't come back.
fn saved_leaf(
    herdr: Option<&herdr::PaneInfo>,
    app_command: Option<&str>,
    zed: bool,
) -> Option<SavedLeaf> {
    if let Some(pane) = herdr {
        Some(SavedLeaf::Herdr {
            pane_id: pane.pane_id.clone(),
        })
    } else if let Some(command) = app_command {
        Some(SavedLeaf::App {
            command: command.to_owned(),
        })
    } else {
        zed.then_some(SavedLeaf::Zed)
    }
}

/// Whether `command` is a web page's link, which opens as a web app (see
/// `launch::web_app`).
fn is_web_link(command: &str) -> bool {
    let command = command.trim();
    let web =
        |scheme: &str| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    !command.contains(char::is_whitespace)
        && command
            .split_once("://")
            .is_some_and(|(scheme, _)| web(scheme))
}

/// The title `window` goes by.
fn window_title(window: &Window) -> Option<String> {
    with_states(window.toplevel()?.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .unwrap()
            .title
            .clone()
    })
}

/// Whether `window` is a Zed window.
fn is_zed(window: &Window) -> bool {
    with_states(window.toplevel().unwrap().wl_surface(), |states| {
        let data = states.data_map.get::<XdgToplevelSurfaceData>()?;
        let app_id = data.lock().unwrap().app_id.clone()?;
        Some(app_id.to_ascii_lowercase().contains("zed"))
    })
    .unwrap_or(false)
}

/// A tab rebuilt from its saved layout.
#[derive(Debug, Default, PartialEq)]
struct RestoredTab {
    layout: Option<Node>,
    /// The leaves kept for Zed windows.
    zed: Vec<u32>,
    /// The leaves kept for apps, with the command to launch into each.
    apps: Vec<(u32, String)>,
}

/// Rebuild a tab from `saved`: each herdr pane in the leaf `ids` gives it,
/// Zed and apps in new leaves from `next`. Leaves with nothing to fill them
/// leave their space to their sibling.
fn restore_tab(saved: &SavedNode, ids: &HashMap<String, u32>, next: &mut u32) -> RestoredTab {
    let mut restored = RestoredTab::default();
    let mut new_leaf = || {
        *next += 1;
        *next
    };
    restored.layout = saved.restore(&mut |leaf| match leaf {
        SavedLeaf::Herdr { pane_id } => ids.get(pane_id).copied(),
        SavedLeaf::Zed => {
            let id = new_leaf();
            restored.zed.push(id);
            Some(id)
        }
        SavedLeaf::App { command } => {
            let id = new_leaf();
            restored.apps.push((id, command.clone()));
            Some(id)
        }
    });
    restored
}

/// Whether `saved` holds exactly the herdr panes `live`.
fn same_panes(saved: &SavedNode, live: &HashSet<&str>) -> bool {
    let panes = saved.herdr_panes();
    panes.len() == live.len() && panes.iter().all(|p| live.contains(p))
}

/// The commands of the apps in `saved`, in layout order.
fn saved_apps(saved: &SavedNode) -> Vec<&str> {
    match saved {
        SavedNode::Leaf(SavedLeaf::App { command }) => vec![command.as_str()],
        SavedNode::Leaf(_) => Vec::new(),
        SavedNode::Split { first, second, .. } => {
            let mut apps = saved_apps(first);
            apps.extend(saved_apps(second));
            apps
        }
    }
}

/// The record of herdr pane `pane_id`: by its id, else by its place (tab
/// `tab`, pane `index` in it) if the record there is of no pane in `live`.
fn find_pane_record<'a>(
    records: &'a [PaneRecord],
    pane_id: &str,
    tab: usize,
    index: usize,
    live: &HashSet<&str>,
) -> Option<&'a PaneRecord> {
    records.iter().find(|r| r.pane_id == pane_id).or_else(|| {
        records
            .iter()
            .find(|r| r.tab == tab && r.index == index && !live.contains(r.pane_id.as_str()))
    })
}

/// What a round of herdr's reports changes in a pane's saved command.
#[derive(Clone, Debug, PartialEq)]
enum Note {
    /// Nothing.
    Keep,
    /// This, if it has none.
    Fill(projects::Captured),
    /// This instead.
    Set(projects::Captured),
    /// It runs nothing worth running again.
    Clear,
}

/// What `foreground` (`None`: herdr couldn't tell) changes in a pane's
/// saved command, given the process `leader` that led its foreground when
/// it was last noted, and whether its shell has been idle long enough to
/// forget its command (`idle_long`); and the leader to remember now.
/// A command is noted once per process: some rewrite their command line
/// once running (setproctitle). One that can't be read back then keeps
/// what was noted, as it may be the same program run again; one never run
/// again (see `projects::DENY`) is another program, and clears it.
fn leader_note(
    foreground: Option<projects::Foreground>,
    leader: Option<i32>,
    idle_long: bool,
) -> (Note, Option<i32>) {
    use projects::Foreground::{Busy, Idle, Untracked};
    match foreground {
        None => (Note::Keep, leader),
        Some(Idle) if idle_long => (Note::Clear, None),
        Some(Idle) => (Note::Keep, None),
        Some(Busy { pid, captured }) if leader == Some(pid) => (Note::Fill(captured), leader),
        Some(Busy { pid, captured }) => (Note::Set(captured), Some(pid)),
        Some(Untracked { pid, .. }) if leader == Some(pid) => (Note::Keep, leader),
        Some(Untracked { pid, denied: true }) => (Note::Clear, Some(pid)),
        Some(Untracked { pid, denied: false }) => (Note::Keep, Some(pid)),
    }
}

/// Bring the record of herdr pane `pane_id` (tab `tab`, pane `index` in it)
/// up to date with what runs in it, as `note` says. Returns whether it
/// changed.
fn note_pane(
    records: &mut Vec<PaneRecord>,
    pane_id: &str,
    (tab, index): (usize, usize),
    note: Note,
) -> bool {
    let existing = records.iter_mut().find(|r| r.pane_id == pane_id);
    let record = match (existing, &note) {
        (Some(record), _) => record,
        (None, Note::Fill(_) | Note::Set(_)) => {
            records.push(PaneRecord {
                pane_id: pane_id.to_owned(),
                ..Default::default()
            });
            records.last_mut().unwrap()
        }
        (None, Note::Keep | Note::Clear) => return false,
    };
    let before = record.clone();
    record.tab = tab;
    record.index = index;
    match note {
        Note::Fill(_) if record.command.is_some() => {}
        Note::Fill(captured) | Note::Set(captured) => {
            record.command = Some(captured.command);
            record.cwd = captured.cwd;
            record.env = captured.env;
        }
        Note::Clear => {
            record.command = None;
            record.cwd = None;
            record.env.clear();
        }
        Note::Keep => {}
    }
    *record != before
}

/// Whether process group `group` still has processes.
fn group_alive(group: i32) -> bool {
    // Safety: signal 0 only checks.
    let found = unsafe { libc::kill(-group, 0) } == 0;
    found || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The process group of `pid`.
fn group_of(pid: u32) -> i32 {
    // Safety: reads only.
    unsafe { libc::getpgid(pid as i32) }
}

/// The deepest folder holding all of `dirs`.
fn common_dir(dirs: &[&Path]) -> Option<PathBuf> {
    let (first, rest) = dirs.split_first()?;
    let mut common = first.to_path_buf();
    for dir in rest {
        while !dir.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    Some(common)
}

/// What the notice says of a workspace's venv setting, once changed.
fn venv_notice(setting: &VenvSetting) -> String {
    match setting {
        VenvSetting::Auto => "New shells here activate the venv they find".into(),
        VenvSetting::Off => "New shells here activate no venv".into(),
        VenvSetting::Path(path) => format!("New shells here activate {}", path.display()),
    }
}

/// What the welcome screen says of a project with workspaces in `dirs`:
/// "3 repos · ~/Work/aft", or "empty".
fn project_detail(dirs: &[&Path], home: Option<&Path>) -> String {
    let count = match dirs.len() {
        0 => return "empty".into(),
        1 => "1 repo".to_owned(),
        n => format!("{n} repos"),
    };
    match common_dir(dirs).filter(|d| d.parent().is_some()) {
        Some(dir) => format!(
            "{count} · {}",
            paths::display(&dir, home, project_list::PATH_CHARS)
        ),
        None => count,
    }
}

/// The project named with `--project` (`--project <name>` or
/// `--project=<name>`), and the folders to open.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> (Option<String>, Vec<PathBuf>) {
    let mut project = None;
    let mut folders = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--project" {
            project = args.next().map(|a| a.to_string_lossy().into_owned());
        } else if let Some(name) = text.strip_prefix("--project=") {
            project = Some(name.to_owned());
        } else {
            folders.push(PathBuf::from(arg));
        }
    }
    (project, folders)
}

/// What the folder picker is open for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickFor {
    /// A workspace of this kind, opened in the project shown.
    Workspace(WorkspaceKind),
    /// The folder of the project menu's form.
    MenuFolder,
    /// The venv of a project's herdr workspace.
    Venv { project: String, workspace: String },
}

/// What a workspace opens with: Zed over a terminal, just a terminal, or
/// the project's Cursor (see `Gerdr::open_cursor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    Project,
    Terminal,
    Cursor,
}

impl WorkspaceKind {
    fn from_record(kind: projects::Kind) -> Self {
        match kind {
            projects::Kind::Project => Self::Project,
            projects::Kind::Terminal => Self::Terminal,
            projects::Kind::Cursor => Self::Cursor,
        }
    }

    fn to_record(self) -> projects::Kind {
        match self {
            Self::Project => projects::Kind::Project,
            Self::Terminal => projects::Kind::Terminal,
            Self::Cursor => projects::Kind::Cursor,
        }
    }
}

/// A folder with its own windows and tabs: a project (Zed and terminals) or
/// a plain terminal workspace.
pub struct Workspace {
    pub dir: PathBuf,
    pub kind: WorkspaceKind,
    /// The folder's name, shown in the sidebar.
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// The id of the shown project's record of it (see
    /// `Gerdr::new_workspace`); empty without a project.
    pub record: String,
}

impl Workspace {
    /// The herdr workspace this one mirrors, while it has herdr panes.
    fn herdr_id(&self) -> Option<&str> {
        self.tabs
            .iter()
            .flat_map(Tab::herdr_panes)
            .next()
            .map(|p| p.workspace_id.as_str())
    }

    fn new(dir: PathBuf, kind: WorkspaceKind) -> Self {
        let name = if home_dir().is_some_and(|home| home == dir) {
            "~".into()
        } else {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string())
        };
        Self {
            dir,
            kind,
            name,
            tabs: Vec::new(),
            active_tab: 0,
            record: String::new(),
        }
    }
}

/// A project to show once its start commands are done, and the folders to
/// open in it (see `enter_project`).
#[derive(Debug)]
pub struct PendingEnter {
    id: String,
    pick: bool,
    folders: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    GotoTab(usize),
    NewProject,
    /// A terminal workspace in the home folder.
    NewTerminal,
    CloseWorkspace,
    NextWorkspace,
    PrevWorkspace,
    /// Split the focused pane and open a terminal in the new half.
    Split(Dir),
    /// Close the focused pane.
    ClosePane,
    /// Focus the pane next to the focused one.
    FocusSide(Side),
    /// Move focus to the next (1) or previous (-1) pane of the tab.
    CyclePane(isize),
    /// Switch the chrome between light and dark.
    ToggleTheme,
    /// Keep the key help open, or close it again.
    Shortcuts,
    /// Open the switcher, to search for what to switch to.
    Switcher,
    /// Open the run prompt, for an app to launch into a split.
    Run,
    /// Open the run prompt, for an app to launch into a new tab.
    RunTab,
    /// Open the project menu (the welcome screen without a project), to
    /// switch, start or manage projects.
    Projects,
    /// Show the project shown before this one.
    LastProject,
    /// Whether the focused terminal's command runs again after a restart.
    ToggleRestart,
    /// Whether new shells of the active workspace activate a venv.
    ToggleVenv,
    /// Pick the venv new shells of the active workspace activate.
    PickVenv,
    /// Run the shown project's start commands again.
    RunStartCommands,
    /// Stop what runs in the focused terminal and run its last command again.
    RestartPane,
    /// Stop what runs in the focused terminal.
    StopPane,
    /// Move the focused pane to `side`: beside its neighbour there, or to
    /// the tab's edge (see `pane_moves`).
    MovePane(Side),
    /// Swap the focused pane with its neighbour on `side`.
    SwapPane(Side),
    /// Turn the split the focused pane is in: side by side becomes
    /// stacked and back.
    FlipPane,
    /// Move the focused pane to a new tab of the workspace.
    PaneToNewTab,
    /// Open the worktrees popup, to review, merge and delete the
    /// repositories' worktrees.
    Worktrees,
    /// Open the repositories panel, to branch, commit, push and sync the
    /// project's repositories as one.
    Repos,
    /// Read the projects file again, after it was edited by hand.
    ReloadProjects,
    /// Ask whether to keep the project running, and quit.
    Quit,
}

/// A divider that can be dragged with the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drag {
    /// The sidebar's right edge.
    Sidebar,
    /// The gap between two panes of the shown tab: the split at `path`.
    Split { path: Vec<bool>, dir: Dir },
}

/// The active workspace's row, the active tab or a pane, held with the
/// left button to drag it to another place.
#[derive(Clone, Copy, Debug)]
pub struct Reorder {
    pub what: Reorderable,
    /// Where the button went down.
    pub from: Point<f64, Logical>,
    /// The pointer moved far enough from `from` to drag.
    pub moving: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reorderable {
    Workspace,
    Tab,
    /// Pane `id` of the shown tab, held by its bar; it lands where it is
    /// dropped (see `pane_drag`).
    Pane(u32),
}

pub struct Gerdr {
    pub start_time: Instant,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, Gerdr>,
    pub backend: WinitGraphicsBackend<GlesRenderer>,
    pub output: Output,
    pub space: Space<Window>,
    pub popups: PopupManager,

    /// Toplevels created but not yet committed (app_id unknown until first commit).
    pub unplaced: Vec<Window>,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub chrome: Chrome,
    /// The look chosen: the system's, dark or light.
    pub theme_mode: ThemeMode,
    /// Whether the system prefers a dark look (see `system_theme`).
    pub system_dark: bool,
    /// How much room the chrome takes, from the config file.
    pub density: Density,
    pub sidebar_width: i32,
    /// The next pane id to hand out.
    pub next_pane: u32,
    /// The divider being dragged, while the left button holds it.
    pub drag: Option<Drag>,
    /// Where the held pane (see `reorder`) would land if dropped now.
    pub pane_drop: Option<pane_drag::PaneDrop>,
    /// The workspace row or tab held, while the left button holds it.
    pub reorder: Option<Reorder>,
    /// The cursor shape gerdr last set over its chrome.
    pub chrome_cursor: CursorIcon,
    /// Keys whose press was a gerdr shortcut; their release is swallowed too.
    pub suppressed_keys: Vec<Keycode>,
    /// The prefix was pressed; the next key is a gerdr command.
    pub prefix_armed: bool,
    /// The key help stays open until closed again.
    pub help_pinned: bool,
    /// The prefix and the keys after it, from the config file.
    pub keymap: keys::Keymap,
    /// Where the pointer is, in window coordinates.
    pub pointer: Point<f64, Logical>,
    /// The last press anywhere (when, where, which button), to tell a
    /// double click.
    pub last_press: Option<(Instant, Point<f64, Logical>, u32)>,
    /// A workspace or tab being renamed in place.
    pub rename: Option<Rename>,
    /// What the pointer is over in the tab strip, and how far it has lit
    /// up (or faded since the pointer left).
    pub tab_hover: anim::Hover<chrome::TabHit>,
    /// What the pointer is over in the sidebar, with its fades.
    pub sidebar_hover: anim::Hover<chrome::SidebarHit>,
    /// How often and recently things were switched to, for ordering lists.
    pub frecency: Frecency,
    /// Where each pane bar stands between hidden and shown.
    pub bar_states: pane_control::BarStates,
    /// Where a window goes once it has drawn a new size, by its surface:
    /// the size it had when asked, and its place then. A window that is
    /// to move and change size at once (a pane bar taking or giving back
    /// its room) stays put until then, so its old buffer never shows at
    /// the new place and jumps again when the new one comes.
    pub pending_place: HashMap<ObjectId, (Size<i32, Logical>, Point<i32, Logical>)>,

    pub socket_name: OsString,
    /// Zed's data dir, shared by all workspaces so they run in one Zed process.
    pub zed_data: PathBuf,
    pub children: Vec<Child>,
    /// gerdr's herdr session, holding the terminals; `None` without herdr.
    pub herdr: Option<herdr::Herdr>,
    /// The coding agents herdr sees in the session, shown in the sidebar.
    pub agents: Vec<herdr::AgentInfo>,
    /// What herdr reports about the tabs of gerdr's workspaces, by herdr tab
    /// id, shown in the tab strip.
    pub herdr_tabs: HashMap<String, herdr::TabDetail>,
    /// The Git status of the workspace folders in a repository, by folder.
    pub git: HashMap<PathBuf, git::GitStatus>,
    /// A read of where the web pages are is under way (see `watch_web_pages`).
    pub web_pending: bool,
    /// A round of reading the Git status is still running.
    pub git_pending: bool,
    /// Another round is wanted once it is done.
    pub git_again: bool,
    /// Hands a round's Git status to the event loop (see `watch_git`).
    git_read: Option<channel::Sender<HashMap<PathBuf, git::GitStatus>>>,
    /// The worktrees popup and what is being done to worktrees.
    pub worktrees: worktrees::Worktrees,
    /// The repositories panel and its batches.
    pub repos: repos::Repos,
    /// gerdr's connection to the host's notification daemon; `None` without
    /// a session bus.
    pub notifier: Option<notify::Notifier>,
    /// Since when each blocked agent was last notified of (or seen to be
    /// blocked): a reminder goes after `BLOCKED_REMINDER`.
    pub blocked_since: HashMap<agents::AgentKey, Instant>,
    /// What each agent was doing when last looked at, to tell news by.
    pub agent_statuses: HashMap<agents::AgentKey, herdr::AgentStatus>,
    /// What gerdr knows of its Cursor's chats (see `cursor_agents`).
    pub cursor: cursor_agents::Chats,
    /// Those doing something or with news, shown as cards.
    pub cursor_chats: Vec<cursor_agents::CursorChat>,
    /// Asks for Cursor's database to be read now.
    cursor_poke: Option<std::sync::mpsc::Sender<()>>,
    /// Where Cursor's hooks reach this gerdr.
    cursor_socket: Option<PathBuf>,
    /// The Cursor chat last shown from its card, whose card is filled
    /// while Cursor has the keyboard.
    pub cursor_opened: Option<String>,
    /// gerdr's own window has the host's keyboard focus.
    pub host_focused: bool,
    /// Process groups we started, all terminated when gerdr exits.
    pub process_groups: Vec<i32>,
    /// The folder picker, while it is open, and what it is open for.
    pub picker: Option<(Child, PickFor)>,
    /// The context menu open, if one is.
    pub context_menu: Option<ContextMenu>,
    /// The tooltip offered for what is under the pointer, and the one shown.
    pub tooltip: TooltipState,
    /// The switcher, while it is open.
    pub switcher: Option<switcher::Switcher>,
    /// The folders zoxide knows, the highest ranked first.
    pub recent_dirs: Vec<PathBuf>,
    /// Where a read of zoxide's folders sends them; `None` until set up.
    pub zoxide: Option<channel::Sender<Vec<PathBuf>>>,
    /// The key held over an overlay, and the timer repeating it.
    pub key_repeat: Option<(Keycode, RegistrationToken)>,
    /// The apps named in the config file, to launch into a split.
    pub apps: Vec<launch::App>,
    /// The apps installed on the system, offered in the run prompt.
    pub installed_apps: Vec<launch::App>,
    /// Where a read of the installed apps sends them; `None` until set up.
    pub installed_apps_tx: Option<channel::Sender<Vec<launch::App>>>,

    /// The projects gerdr knows, and what it remembers of them.
    pub projects: projects::Store,
    /// The project shown, by id; `None` before one is open.
    pub project: Option<String>,
    /// The welcome screen, while it is shown.
    pub welcome: Option<welcome::Welcome>,
    /// Whether each project's session runs, by id, as last checked.
    pub running: HashMap<String, bool>,
    /// Where a check of which projects' sessions run sends its answer (by
    /// project id); `None` until set up.
    pub running_status: Option<channel::Sender<Vec<(String, bool)>>>,
    /// Where stopping a project's session in the background sends whether
    /// it stopped (by project id); `None` until set up.
    pub stop_status: Option<channel::Sender<(String, bool)>>,
    /// Where a project's start or stop commands send how they went; `None`
    /// until set up.
    pub hook_status: Option<channel::Sender<hooks::Done>>,
    /// The projects whose start commands run, by id.
    pub starting: HashSet<String>,
    /// The project to show once its start commands are done.
    pub pending_enter: Option<PendingEnter>,
    /// Brings the clipboard's text read for a paste back (see `clipboard`).
    pub paste: Option<channel::Sender<(input::Overlay, String)>>,
    /// The text copied from one of gerdr's own text fields, while gerdr
    /// owns the clipboard with it (see `clipboard`).
    pub copied: Option<String>,
    /// The last presses in an overlay's text field (when, where, how many
    /// in a row), to tell double and triple clicks.
    pub field_clicks: Option<(std::time::Instant, Point<f64, Logical>, u32)>,
    /// A press in an overlay's text field is held: moving selects.
    pub field_drag: bool,
    /// The projects whose sessions are being stopped, by id: not shown
    /// until they have.
    pub stopping: HashSet<String>,
    /// The project menu, while it is open.
    pub project_menu: Option<project_menu::ProjectMenu>,
    /// The pointer is over the sidebar header's project name.
    pub header_hover: bool,
    /// The project shown before the one shown now, by id, for
    /// `Action::LastProject`.
    pub previous_project: Option<String>,
    /// The notes shown over the apps, oldest first (see `toast`).
    pub toasts: VecDeque<Toast>,
    /// The quit dialog, while it is open.
    pub quit: Option<quit::QuitDialog>,
    /// The event loop sources following the project's herdr session, removed
    /// when switching to another project.
    pub herdr_sources: Vec<RegistrationToken>,
    /// Asks the thread polling the project's herdr session for a round now.
    herdr_poke: Option<std::sync::mpsc::Sender<()>>,
    /// Since when each herdr pane's shell has been idle, by pane id: its
    /// command is forgotten only after a while, so panes dying on shutdown
    /// don't count.
    pub idle_since: HashMap<String, Instant>,
    /// Process groups of projects switched away from, until they are gone;
    /// their late windows are closed instead of placed.
    pub retired_groups: Vec<i32>,
    /// Counts the times gerdr's processes were ended, to name the Ghostty
    /// and Alacritty gerdr starts next apart from those on their way out
    /// (see `launch`).
    pub shared_generation: u32,
    /// Since when the tab layouts differ from what was saved of them.
    pub tabs_changed: Option<Instant>,
    /// The projects whose file was reloaded while they weren't running: they
    /// take its workspace order and labels when they start.
    pub order_pending: HashSet<String>,
    /// The process leading each herdr pane's foreground when its command was
    /// last noted, by pane id: a command is noted once per process, as
    /// some rewrite their command line once running.
    pub pane_leaders: HashMap<String, i32>,
    /// The last command run in each herdr pane, by pane id, to run again
    /// from its bar.
    pub last_commands: HashMap<String, pane_control::Remembered>,
    /// The panes being stopped or restarted, by herdr pane id.
    pub pane_jobs: HashMap<String, pane_control::Job>,
    /// Where those jobs report; `None` until set up.
    pub pane_job_tx: Option<channel::Sender<pane_control::JobEvent>>,
    /// The pane bar button under the pointer, by pane.
    pub pane_bar_hover: Option<(u32, BarButton)>,
    /// Since when each workspace herdr said closed has been, by herdr id:
    /// forgotten only after a while, so a server going down doesn't count.
    pub closed_workspaces: HashMap<String, Instant>,
    /// Since when each record without a herdr workspace has had no
    /// workspace open, by id (see `keep_records`).
    pub gone_since: HashMap<String, Instant>,
    /// The records of the workspaces closed on purpose, to forget once
    /// they are gone (see `keep_records`).
    pub closing_records: HashSet<String>,
    /// When the shown project's server was restarted and brought its
    /// workspaces back, if it was.
    pub restored_at: Option<Instant>,
    /// The project's workspaces, by herdr id, that the restarted server
    /// hasn't brought back yet: what ran in them runs again once it does.
    pub replay_pending: HashSet<String>,
    /// Windows opened by the shells of projects not shown, by project id
    /// (`""` for windows of no known project while none is shown): shown
    /// once their project is.
    pub held: HashMap<String, Vec<Window>>,
    /// Windows of panes gerdr was asked to close (see `close_panes`): a Zed
    /// one leaves no placeholder when it goes (see `remove_window`).
    pub closing: Vec<Window>,
    /// The process groups of the Zed launches of the shown project.
    pub zed_groups: Vec<i32>,
    /// Those of the project switched away from, while they may still take
    /// the next `zed -n`, and until when they are waited for.
    pub old_zed: Vec<i32>,
    pub old_zed_until: Option<Instant>,
    /// The address of each workspace folder's repository page, once read
    /// (see `git::web_url`).
    pub repo_pages: HashMap<PathBuf, Option<String>>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Gerdr>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub ext_data_control_state: ExtDataControlState,
    /// Clipboard bridge to the host; `None` if the host doesn't support it.
    pub host_clipboard: Option<clipboard::HostClipboard>,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub cursor_shape_state: CursorShapeManagerState,
    pub seat: Seat<Gerdr>,
}

impl Gerdr {
    pub fn output_size(&self) -> Size<i32, Logical> {
        let mode = self.output.current_mode().unwrap();
        mode.size.to_f64().to_logical(self.scale()).to_i32_round()
    }

    pub fn scale(&self) -> f64 {
        self.output.current_scale().fractional_scale()
    }

    /// The look in use: the mode chosen, by what the system prefers.
    pub fn theme(&self) -> Theme {
        self.theme_mode.resolve(self.system_dark)
    }

    /// How high the tab strip is, at the density chosen.
    pub fn tab_strip_height(&self) -> i32 {
        chrome::tab_strip_height(self.density)
    }

    /// Where the panes go: right of the sidebar, below the tab strip.
    fn content_rect(&self) -> Rectangle<i32, Logical> {
        let size = self.output_size();
        let (x, y) = (self.sidebar_width, self.tab_strip_height());
        Rectangle::new(
            (x, y).into(),
            ((size.w - x).max(1), (size.h - y).max(1)).into(),
        )
    }

    /// The divider within grabbing distance of `pos`, if any.
    pub fn divider_at(&self, pos: Point<f64, Logical>) -> Option<Drag> {
        if (pos.x - self.sidebar_width as f64).abs() <= GRAB {
            return Some(Drag::Sidebar);
        }
        let layout = self.tab()?.layout.as_ref()?;
        let divider = layout
            .dividers(self.content_rect(), DIVIDER)
            .into_iter()
            .find(|d| d.near(pos, GRAB))?;
        Some(Drag::Split {
            path: divider.path,
            dir: divider.dir,
        })
    }

    /// Move the dragged divider to the pointer at `pos` and resize the panes.
    pub fn drag_to(&mut self, drag: &Drag, pos: Point<f64, Logical>) {
        let size = self.output_size();
        match drag {
            Drag::Sidebar => {
                let max = SIDEBAR_MAX_WIDTH.min(size.w - MIN_PANE);
                self.sidebar_width = (pos.x.round() as i32).min(max).max(SIDEBAR_MIN_WIDTH);
            }
            Drag::Split { path, dir } => {
                let area = self.content_rect();
                let ws = &mut self.workspaces[self.active_workspace];
                let Some(layout) = ws
                    .tabs
                    .get_mut(ws.active_tab)
                    .and_then(|t| t.layout.as_mut())
                else {
                    return;
                };
                let Some(split) = layout
                    .dividers(area, DIVIDER)
                    .into_iter()
                    .find(|d| &d.path == path)
                else {
                    return;
                };
                let (start, len, at) = match dir {
                    Dir::Right => (split.area.loc.x, split.area.size.w, pos.x),
                    Dir::Down => (split.area.loc.y, split.area.size.h, pos.y),
                };
                // Leave both sides at least MIN_PANE, or half each without room for that.
                let room = (len - DIVIDER).max(1);
                let first = (at.round() as i32 - start)
                    .clamp(MIN_PANE.min(room / 2), (room - MIN_PANE).max(room / 2));
                layout.set_ratio(path, first as f64 / room as f64);
            }
        }
        self.arrange();
    }

    /// Move the held workspace or tab (the active one) to the place under
    /// the pointer at `pos`.
    pub fn reorder_to(&mut self, what: Reorderable, pos: Point<f64, Logical>) {
        match what {
            Reorderable::Workspace => {
                let layout = self.sidebar_layout();
                let to = chrome::workspace_slot(&layout.blocks, self.active_workspace, pos.y);
                move_item(&mut self.workspaces, &mut self.active_workspace, to);
            }
            Reorderable::Tab => {
                let to = self.tab_slot_at(pos.x);
                let Some(ws) = self.workspaces.get_mut(self.active_workspace) else {
                    return;
                };
                move_item(&mut ws.tabs, &mut ws.active_tab, to);
            }
            // A pane only lands when dropped; meanwhile, where it would.
            Reorderable::Pane(id) => self.pane_drop = self.pane_drop_at(id, pos),
        }
    }

    /// Tell herdr where the active workspace or tab went, so the order holds
    /// when gerdr starts again.
    pub fn reorder_herdr(&mut self, what: Reorderable) {
        let Some(ws) = self.workspaces.get(self.active_workspace) else {
            return;
        };
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        let moved = match what {
            // A dropped pane tells herdr itself (see `pane_moves`).
            Reorderable::Pane(_) => return,
            Reorderable::Workspace => {
                let Some(id) = ws.herdr_id() else { return };
                let before = self.workspaces[self.active_workspace + 1..]
                    .iter()
                    .find_map(Workspace::herdr_id);
                herdr.workspace_move(id, before)
            }
            Reorderable::Tab => {
                let Some((workspace_id, id)) = ws
                    .herdr_id()
                    .zip(ws.tabs.get(ws.active_tab).and_then(Tab::herdr_tab_id))
                else {
                    return;
                };
                let before = ws.tabs[ws.active_tab + 1..]
                    .iter()
                    .find_map(Tab::herdr_tab_id);
                herdr.tab_move(workspace_id, id, before)
            }
        };
        if let Err(err) = moved {
            tracing::warn!("herdr: {err}");
        }
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_workspace)
    }

    /// The shown tab.
    pub fn tab(&self) -> Option<&Tab> {
        self.workspace().and_then(|ws| ws.tabs.get(ws.active_tab))
    }

    fn new_pane_id(&mut self) -> u32 {
        self.next_pane += 1;
        self.next_pane
    }

    /// Start a child on our socket, in its own process group; what it
    /// writes to its stderr goes to `app`'s log (see `launch::log_file`).
    fn spawn(&mut self, mut cmd: Command, dir: &Path, app: &str) -> Option<u32> {
        let socket = self.herdr.as_ref().map(|h| h.socket().to_path_buf());
        herdr::scrub_env(&mut cmd, socket.as_deref());
        cmd.env("WAYLAND_DISPLAY", &self.socket_name)
            .env_remove("DISPLAY") // no XWayland escape to the host
            .current_dir(dir)
            .process_group(0);
        if let Some(log) = launch::log_file(app) {
            cmd.stderr(Stdio::from(log));
        }
        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                self.children.push(child);
                self.process_groups.push(pid as i32);
                Some(pid)
            }
            Err(err) => {
                tracing::error!("spawn {:?} failed: {err}", cmd.get_program());
                None
            }
        }
    }

    /// Start a terminal for workspace `w`: a new herdr pane shown in
    /// Alacritty, or a plain Alacritty without herdr. The pane splits `split`
    /// off tab `t`'s focused terminal, or opens a herdr tab (or workspace)
    /// when there is none.
    fn start_terminal(
        &mut self,
        w: usize,
        t: Option<usize>,
        split: Dir,
    ) -> Option<(u32, Option<herdr::PaneInfo>)> {
        self.start_terminal_running(w, t, split, None)
    }

    /// `start_terminal`, its shell running `run`'s command in `run`'s
    /// folder.
    fn start_terminal_running(
        &mut self,
        w: usize,
        t: Option<usize>,
        split: Dir,
        run: Option<(PathBuf, String)>,
    ) -> Option<(u32, Option<herdr::PaneInfo>)> {
        let dir = match &run {
            Some((cd, _)) => cd.clone(),
            None => self.workspaces[w].dir.clone(),
        };
        let pane = self.new_herdr_pane(w, t, split, run.clone());
        let pid = match &pane {
            Some(pane) => self.attach(pane, &dir),
            None => {
                let mut term = Command::new("alacritty");
                term.arg("--working-directory")
                    .arg(&dir)
                    .args(self.theme().palette().alacritty_options());
                if let Some((_, command)) = &run {
                    term.args(["-e", "sh", "-c", command]);
                }
                self.spawn(term, &dir, "alacritty")
            }
        };
        if pid.is_none()
            && let Some((herdr, pane)) = self.herdr.as_mut().zip(pane.as_ref())
        {
            let _ = herdr.pane_close(&pane.pane_id);
        }
        Some((pid?, pane))
    }

    fn new_herdr_pane(
        &mut self,
        w: usize,
        t: Option<usize>,
        split: Dir,
        run: Option<(PathBuf, String)>,
    ) -> Option<herdr::PaneInfo> {
        let env = self.herdr.as_ref()?.pane_env(&self.socket_name);
        let ws = &self.workspaces[w];
        let target = t
            .and_then(|t| ws.tabs.get(t)?.herdr_split_target())
            .map(|p| p.pane_id.clone());
        let workspace_id = ws.herdr_id().map(str::to_owned);
        let (dir, name, kind) = (ws.dir.clone(), ws.name.clone(), ws.kind);
        let record = ws.record.clone();
        let new_workspace = target.is_none() && workspace_id.is_none();
        let herdr = self.herdr.as_mut()?;
        let pane = match (target, workspace_id) {
            (Some(target), _) => herdr.pane_split(&target, split, &dir, &env),
            (None, Some(workspace_id)) => herdr.tab_create(&workspace_id, &dir, &env),
            (None, None) => {
                herdr.workspace_create(&dir, &name, kind == WorkspaceKind::Project, &env)
            }
        };
        let pane = pane
            .inspect_err(|err| tracing::warn!("herdr: {err}; starting a plain terminal"))
            .ok()?;
        let socket = herdr.socket().to_path_buf();
        if new_workspace {
            self.mirror_herdr(&record, &pane.workspace_id);
        }
        // The new shell gets the workspace's venv.
        let venv = self
            .herdr_record(&pane.workspace_id)
            .map(|r| r.venv.clone())
            .unwrap_or_default();
        let prep = prepare::Prep {
            pane_id: pane.pane_id.clone(),
            pane_cwd: pane.cwd.clone().unwrap_or_else(|| dir.clone()),
            ws_dir: dir,
            venv,
            captured_env: Vec::new(),
            cd: run.as_ref().map(|(cd, _)| cd.clone()),
            command: run.map(|(_, command)| command),
        };
        prepare::spawn_prepare(socket, vec![prep]);
        Some(pane)
    }

    /// The shown project's record of herdr workspace `herdr_id`.
    fn herdr_record(&self, herdr_id: &str) -> Option<&WorkspaceRecord> {
        let project = self.projects.project(self.project.as_deref()?)?;
        project.herdr_record(herdr_id)
    }

    /// The shown project's record `id`.
    fn record(&self, id: &str) -> Option<&WorkspaceRecord> {
        let project = self.projects.project(self.project.as_deref()?)?;
        project.workspaces.iter().find(|r| r.id == id)
    }

    /// The shown project's record of herdr pane `pane`.
    fn pane_record(&self, pane: &herdr::PaneInfo) -> Option<&PaneRecord> {
        let record = self.herdr_record(&pane.workspace_id)?;
        record.panes.iter().find(|r| r.pane_id == pane.pane_id)
    }

    /// A new workspace of `kind` in `dir`, with a record of it in the shown
    /// project, for it to come back after a restart whatever it holds.
    fn new_workspace(&mut self, dir: PathBuf, kind: WorkspaceKind) -> Workspace {
        let mut ws = Workspace::new(dir, kind);
        ws.record = self.remember_workspace(&ws.name, kind, &ws.dir, None);
        ws
    }

    /// Add a workspace to the shown project's records, mirroring herdr
    /// workspace `herdr_id` if given (the one there already is, if any):
    /// the record's id; empty without a project.
    fn remember_workspace(
        &mut self,
        label: &str,
        kind: WorkspaceKind,
        dir: &Path,
        herdr_id: Option<&str>,
    ) -> String {
        let Some(project) = self
            .project
            .clone()
            .and_then(|id| self.projects.project_mut(&id))
        else {
            return String::new();
        };
        if let Some(record) = herdr_id.and_then(|id| project.herdr_record(id)) {
            return record.id.clone();
        }
        let id = project.new_workspace_id();
        project.workspaces.push(WorkspaceRecord {
            id: id.clone(),
            herdr_id: herdr_id.map(str::to_owned),
            label: label.to_owned(),
            kind: kind.to_record(),
            dir: dir.to_path_buf(),
            venv: Default::default(),
            tabs: Vec::new(),
            panes: Vec::new(),
        });
        self.projects.save();
        id
    }

    /// Forget the shown project's record `id`.
    fn forget_workspace(&mut self, id: &str) {
        if let Some(project) = self
            .project
            .clone()
            .and_then(|p| self.projects.project_mut(&p))
        {
            let before = project.workspaces.len();
            project.workspaces.retain(|r| r.id != id);
            if project.workspaces.len() != before {
                self.projects.save();
            }
        }
    }

    /// Record `id` of the shown project mirrors herdr workspace
    /// `herdr_id` from now on.
    fn mirror_herdr(&mut self, id: &str, herdr_id: &str) {
        let Some(project) = self.project.clone() else {
            return;
        };
        if let Some(record) = self.projects.record_mut(&project, id)
            && !record.mirrors(herdr_id)
        {
            record.herdr_id = Some(herdr_id.to_owned());
            self.projects.save();
        }
    }

    /// Show herdr pane `pane` in an Alacritty window.
    fn attach(&mut self, pane: &herdr::PaneInfo, dir: &Path) -> Option<u32> {
        let dir = pane
            .cwd
            .as_deref()
            .filter(|d| d.is_dir())
            .unwrap_or(dir)
            .to_path_buf();
        let options = self.theme().palette().alacritty_options();
        let term = herdr::attach_command(&pane.terminal_id, &dir, &options);
        self.spawn(term, &dir, "alacritty")
    }

    /// Connect to herdr session `session` (starting it if needed), show what
    /// it already holds and follow its changes. Without herdr, terminals are
    /// plain Alacritty windows. A restarted server brings its workspaces back
    /// by itself, as fresh shells: what ran in them is run again. A fresh
    /// one has none.
    fn connect_herdr(&mut self, session: &str) {
        // Read before a server starting may save over it.
        let saved = herdr::saved_workspace_count(session);
        let (mut herdr, start) = match herdr::Herdr::connect_or_start(session, &self.socket_name) {
            Ok(connected) => connected,
            Err(err) => {
                tracing::warn!("{err}; terminals won't persist");
                return self.toast(err, Level::Error);
            }
        };
        let mut workspaces = herdr.workspace_list().unwrap_or_default();
        // A restarted server answers before it has brought back all it
        // saved. Those that come later are shown and run again as they come
        // (see `capture_commands`).
        if start == herdr::Start::Restored {
            let deadline = Instant::now() + RESTORE_WAIT;
            while workspaces.len() < saved && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(list) = herdr.workspace_list() {
                    workspaces = list;
                }
            }
        }
        let subscribed = herdr.subscribe();
        let socket = herdr.socket().to_path_buf();
        self.herdr = Some(herdr);
        // Let go of the workspaces closed while gerdr wasn't looking: their
        // apps come back without them (see `restore_saved`). An empty
        // list may be a server still restoring: keep them then, unless it
        // had nothing saved to restore (its new workspaces would take the
        // ids of the old ones). A restarted one must have brought back all
        // it saved.
        let complete = match start {
            herdr::Start::Fresh => true,
            herdr::Start::Attached => !workspaces.is_empty(),
            herdr::Start::Restored => workspaces.len() >= saved,
        };
        if complete
            && let Some(project) = self
                .project
                .clone()
                .and_then(|id| self.projects.project_mut(&id))
        {
            let mut changed = false;
            for record in &mut project.workspaces {
                if let Some(id) = &record.herdr_id
                    && workspaces.iter().all(|ws| ws.workspace_id != *id)
                {
                    record.herdr_id = None;
                    changed = true;
                }
            }
            if changed {
                self.projects.save();
            }
        }
        if complete
            && let Some(id) = self.project.clone()
            && self.order_pending.remove(&id)
            && let (Some(herdr), Some(project)) = (self.herdr.as_mut(), self.projects.project(&id))
        {
            for problem in reload::apply_order(herdr, project) {
                tracing::warn!("projects.json order: {problem}");
            }
            if let Ok(list) = herdr.workspace_list() {
                workspaces = list;
            }
        }
        for ws in &workspaces {
            self.sync_herdr_workspace(&ws.workspace_id);
        }
        self.restore_saved(false);
        if start == herdr::Start::Restored {
            self.restored_at = Some(Instant::now());
            self.replay_pending = self
                .project
                .as_deref()
                .and_then(|id| self.projects.project(id))
                .map_or_else(HashSet::new, |p| {
                    p.workspaces
                        .iter()
                        .filter_map(|r| r.herdr_id.clone())
                        .filter(|id| workspaces.iter().all(|ws| ws.workspace_id != *id))
                        .collect()
                });
            self.replay(&workspaces);
        }
        self.refresh_agents();
        // The events' source goes last: it removes itself when the server goes.
        let tabs = self.watch_herdr_tabs(socket);
        self.herdr_sources.extend(tabs);
        let sub = match subscribed {
            Ok(sub) => sub,
            Err(err) => {
                return tracing::warn!(
                    "herdr events: {err}; changes made outside gerdr won't show"
                );
            }
        };
        let source = Generic::new(
            sub,
            Interest::READ,
            smithay::reexports::calloop::Mode::Level,
        );
        let inserted = self.loop_handle.insert_source(source, |_, sub, state| {
            // Safety: the subscription is never dropped while the loop runs.
            match unsafe { sub.get_mut() }.read() {
                Ok(events) => events
                    .into_iter()
                    .for_each(|event| state.on_herdr_event(event)),
                Err(()) => {
                    tracing::warn!("herdr server went away; new terminals won't persist");
                    state.toast(
                        "herdr server went away; new terminals won't persist".into(),
                        Level::Error,
                    );
                    state.herdr = None;
                    state.herdr_poke = None;
                    state.agents.clear();
                    state.notify_agents();
                    state.herdr_tabs.clear();
                    // Closed as it went down: they come back with it.
                    state.closed_workspaces.clear();
                    // This source is the last one and removes itself.
                    state.herdr_sources.pop();
                    for token in std::mem::take(&mut state.herdr_sources) {
                        state.loop_handle.remove(token);
                    }
                    return Ok(PostAction::Remove);
                }
            }
            Ok(PostAction::Continue)
        });
        match inserted {
            Ok(token) => self.herdr_sources.push(token),
            Err(err) => tracing::warn!("herdr events: {err}"),
        }
    }

    /// Run again what ran in the panes of `workspaces`, just restored by a
    /// server restart, each in its venv; the panes without a command just
    /// get their venv. A pane is found in the records by its id, else by its
    /// place in the workspace.
    fn replay(&mut self, workspaces: &[herdr::WorkspaceInfo]) {
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        let socket = herdr.socket().to_path_buf();
        let project = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id));
        let mut preps = Vec::new();
        for ws in workspaces {
            let order = herdr.tab_list(&ws.workspace_id).unwrap_or_default();
            let panes = herdr.pane_list(&ws.workspace_id).unwrap_or_default();
            let live: HashSet<&str> = panes.iter().map(|p| p.pane_id.as_str()).collect();
            let record = project.and_then(|p| p.herdr_record(&ws.workspace_id));
            for pane in &panes {
                let tab = order.iter().position(|t| *t == pane.tab_id).unwrap_or(0);
                let index = panes
                    .iter()
                    .filter(|p| p.tab_id == pane.tab_id)
                    .position(|p| p.pane_id == pane.pane_id)
                    .unwrap_or(0);
                let ws_dir = record
                    .map(|r| r.dir.clone())
                    .or_else(|| ws.dir())
                    .or_else(|| pane.cwd.clone())
                    .unwrap_or_default();
                let found = record
                    .and_then(|r| find_pane_record(&r.panes, &pane.pane_id, tab, index, &live));
                preps.push(prepare::Prep {
                    pane_id: pane.pane_id.clone(),
                    pane_cwd: pane.cwd.clone().unwrap_or_else(|| ws_dir.clone()),
                    ws_dir,
                    venv: record.map(|r| r.venv.clone()).unwrap_or_default(),
                    captured_env: found.map(|r| r.env.clone()).unwrap_or_default(),
                    cd: found.and_then(|r| r.cwd.clone()),
                    command: found.and_then(|r| r.restart_command()).map(str::to_owned),
                });
            }
        }
        prepare::spawn_prepare(socket, preps);
    }

    fn on_herdr_event(&mut self, event: herdr::Event) {
        tracing::debug!("herdr event {event:?}");
        let agents_changed = matches!(
            event,
            herdr::Event::AgentsChanged | herdr::Event::PaneGone { .. }
        );
        match event {
            herdr::Event::AgentsChanged => {}
            herdr::Event::Created { workspace_id } => self.sync_herdr_workspace(&workspace_id),
            // Its window closes by itself when the attach ends.
            herdr::Event::PaneGone { pane_id } => {
                self.forget_herdr_pane(&pane_id);
            }
            herdr::Event::PaneMoved {
                previous_pane_id,
                pane,
            } => {
                // herdr keeps the pane's id on a move. One gerdr moved
                // itself (see `pane_moves`) shows in its new tab already.
                let shown = self
                    .workspaces
                    .iter()
                    .flat_map(|ws| &ws.tabs)
                    .flat_map(Tab::herdr_panes)
                    .any(|p| p.pane_id == pane.pane_id && p.tab_id == pane.tab_id);
                if shown {
                    return;
                }
                // Shown elsewhere now: close the old window, then show the
                // pane where it went.
                for (window, pid) in self.forget_herdr_pane(&previous_pane_id) {
                    match window {
                        Some(window) => window.toplevel().unwrap().send_close(),
                        None => unsafe {
                            libc::kill(pid as i32, libc::SIGTERM);
                        },
                    }
                }
                self.sync_herdr_workspace(&pane.workspace_id);
            }
            // Forgotten if it stays closed (see `capture_commands`).
            herdr::Event::WorkspaceClosed { workspace_id } => {
                self.closed_workspaces
                    .entry(workspace_id)
                    .or_insert_with(Instant::now);
            }
            herdr::Event::WorkspaceRenamed {
                workspace_id,
                label,
            } => {
                if let Some(project) = self.project.clone()
                    && let Some(record) = self.projects.herdr_record_mut(&project, &workspace_id)
                    && record.label != label
                {
                    record.label = label.clone();
                    self.projects.save();
                }
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.herdr_id() == Some(workspace_id.as_str()))
                {
                    ws.name = label;
                }
            }
        }
        if agents_changed {
            self.poke_herdr();
        }
    }

    /// Whether the commands saved for the panes are kept whatever the panes
    /// look like: the server restarted a moment ago, and they are still
    /// being run again.
    fn restoring(&self) -> bool {
        self.restored_at
            .is_some_and(|at| at.elapsed() < RESTORE_PROTECT)
    }

    /// Run again what ran in the workspaces `ids` that a restarted server
    /// brought back late.
    fn replay_late(&mut self, ids: Vec<String>) {
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        let workspaces: Vec<_> = ids
            .iter()
            .filter_map(|id| herdr.workspace_get(id).ok())
            .collect();
        for ws in &workspaces {
            self.replay_pending.remove(&ws.workspace_id);
        }
        if !workspaces.is_empty() {
            self.replay(&workspaces);
        }
    }

    /// Follow what herdr reports about the tabs and agents, polled off the
    /// event loop, and remember what runs in the panes. Returns the event
    /// loop source.
    fn watch_herdr_tabs(&mut self, socket: PathBuf) -> Option<RegistrationToken> {
        let (tx, rx): (_, Channel<herdr::Round>) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            // Rounds are dropped once the server is gone.
            if let channel::Event::Msg(round) = event
                && state.herdr.is_some()
            {
                state.herdr_tabs = round.tabs;
                state.capture_commands();
                if state.projects.take_conflict() {
                    state.toast_with(
                        "projects.json edited; saves paused".into(),
                        Level::Warning,
                        Some(("Reload".into(), MenuAction::ReloadProjects)),
                    );
                }
                if let Some(agents) = round.agents {
                    state.agents = agents;
                    state.notify_agents();
                }
            }
        });
        match inserted {
            Ok(token) => {
                // The thread stops once the source is removed.
                let poke =
                    herdr::watch_tabs(socket, AGENT_INTERVAL, move |round| tx.send(round).is_ok());
                self.herdr_poke = Some(poke);
                Some(token)
            }
            Err(err) => {
                tracing::warn!("herdr tabs: {err}");
                None
            }
        }
    }

    /// Remember what runs in each pane of the shown project, to run it again
    /// after a server restart, and the layout of its tabs. A command is
    /// forgotten once its shell has been idle (or its pane gone) for
    /// `FORGET_IDLE`, a workspace once it has been closed that long. A round
    /// with no tabs at all says nothing: the server may be on its way down.
    /// Nothing is forgotten while the server is being restored, and the
    /// workspaces it brings back late have their commands run again.
    fn capture_commands(&mut self) {
        if self.herdr_tabs.is_empty() {
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let restoring = self.restoring();
        if !restoring {
            self.replay_pending.clear();
        }
        let now = Instant::now();
        let mut changed = false;
        let mut live: HashSet<&str> = HashSet::new();
        let mut seen_workspaces: HashSet<&str> = HashSet::new();
        for detail in self.herdr_tabs.values() {
            for (index, pane) in detail.panes.iter().enumerate() {
                live.insert(&pane.pane_id);
                seen_workspaces.insert(&pane.workspace_id);
                // Without an answer about the process, nothing is known.
                let foreground = pane
                    .process
                    .as_ref()
                    .and_then(|info| projects::foreground_command(info, pane.cwd.as_deref()));
                pane_control::remember(
                    &mut self.last_commands,
                    &pane.pane_id,
                    foreground.as_ref(),
                    pane.cwd.as_deref(),
                );
                let idle_long = match foreground {
                    Some(projects::Foreground::Idle) => {
                        let since = self.idle_since.entry(pane.pane_id.clone()).or_insert(now);
                        now - *since >= FORGET_IDLE && !restoring
                    }
                    _ => {
                        self.idle_since.remove(&pane.pane_id);
                        false
                    }
                };
                let leader = self.pane_leaders.get(&pane.pane_id).copied();
                let (mut note, leader) = leader_note(foreground, leader, idle_long);
                match leader {
                    Some(pid) => self.pane_leaders.insert(pane.pane_id.clone(), pid),
                    None => self.pane_leaders.remove(&pane.pane_id),
                };
                if restoring && note == Note::Clear {
                    note = Note::Keep;
                }
                if let Some(record) = self.projects.herdr_record_mut(&project, &pane.workspace_id) {
                    let place = (detail.index, index);
                    changed |= note_pane(&mut record.panes, &pane.pane_id, place, note);
                }
            }
        }
        self.pane_leaders.retain(|id, _| live.contains(id.as_str()));
        self.last_commands
            .retain(|id, _| live.contains(id.as_str()));
        let late: Vec<String> = self
            .replay_pending
            .iter()
            .filter(|id| seen_workspaces.contains(id.as_str()))
            .cloned()
            .collect();
        // Closed for a while, and not back.
        let closed: Vec<String> = self
            .closed_workspaces
            .iter()
            .filter(|(_, since)| now - **since >= FORGET_IDLE && !restoring)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &closed {
            self.closed_workspaces.remove(id);
        }
        let closed: HashSet<&str> = closed
            .iter()
            .map(String::as_str)
            .filter(|id| !seen_workspaces.contains(id))
            .collect();
        // A workspace still open in gerdr, with its apps, keeps its record.
        let open: HashSet<&str> = self
            .workspaces
            .iter()
            .map(|ws| ws.record.as_str())
            .collect();
        // Panes gone from a workspace still there.
        if let Some(p) = self.projects.project_mut(&project) {
            let before = p.workspaces.len();
            p.workspaces.retain(|r| {
                !r.herdr_id
                    .as_deref()
                    .is_some_and(|id| closed.contains(id) && !open.contains(r.id.as_str()))
            });
            changed |= p.workspaces.len() != before;
            for record in &mut p.workspaces {
                if record
                    .herdr_id
                    .as_deref()
                    .is_some_and(|id| closed.contains(id))
                {
                    record.herdr_id = None;
                    record.panes.clear();
                    changed = true;
                }
                let seen = record
                    .herdr_id
                    .as_deref()
                    .is_some_and(|id| seen_workspaces.contains(id));
                if !seen || restoring {
                    continue;
                }
                let before = record.panes.len();
                record.panes.retain(|r| {
                    if live.contains(r.pane_id.as_str()) {
                        return true;
                    }
                    let since = self.idle_since.entry(r.pane_id.clone()).or_insert(now);
                    now - *since < FORGET_IDLE
                });
                changed |= record.panes.len() != before;
            }
            let recorded: HashSet<&str> = p
                .workspaces
                .iter()
                .flat_map(|r| &r.panes)
                .map(|r| r.pane_id.as_str())
                .collect();
            self.idle_since
                .retain(|id, _| live.contains(id.as_str()) || recorded.contains(id.as_str()));
        }
        if changed {
            self.projects.save();
        }
        if !late.is_empty() {
            self.replay_late(late);
        }
    }

    /// Keep the shown project's records up to date with its workspaces,
    /// herdr or not, every `RECORDS_INTERVAL`.
    fn watch_records(&mut self) {
        let timer = Timer::from_duration(RECORDS_INTERVAL);
        let keep = |_, _: &mut (), state: &mut Gerdr| {
            state.keep_records();
            TimeoutAction::ToDuration(RECORDS_INTERVAL)
        };
        if let Err(err) = self.loop_handle.insert_source(timer, keep) {
            tracing::warn!("keeping workspace records: {err}");
        }
    }

    /// Remember the shown project's tab layouts once they settled and its
    /// workspaces' order; mirror the herdr workspaces they took on; forget
    /// a record without one once its workspace has been gone `FORGET_IDLE`,
    /// and any once its workspace closed on purpose is gone.
    fn keep_records(&mut self) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let now = Instant::now();
        let mut changed = false;
        if self.changed_tabs().is_empty() {
            self.tabs_changed = None;
        } else if now - *self.tabs_changed.get_or_insert(now) >= TABS_SETTLE {
            changed |= self.record_tabs();
        }
        changed |= self.record_workspace_order();
        let mirrored: Vec<(String, String)> = self
            .workspaces
            .iter()
            .filter_map(|ws| Some((ws.record.clone(), ws.herdr_id()?.to_owned())))
            .collect();
        for (id, herdr_id) in mirrored {
            self.mirror_herdr(&id, &herdr_id);
        }
        let restoring = self.restoring();
        let open: HashSet<&str> = self
            .workspaces
            .iter()
            .map(|ws| ws.record.as_str())
            .collect();
        let closed: Vec<String> = self
            .closing_records
            .extract_if(|id| !open.contains(id.as_str()))
            .collect();
        if let Some(p) = self.projects.project_mut(&project)
            && !closed.is_empty()
        {
            p.workspaces.retain(|r| !closed.contains(&r.id));
            changed = true;
        }
        let gone: Vec<String> = self.projects.project(&project).map_or(Vec::new(), |p| {
            p.workspaces
                .iter()
                .filter(|r| r.herdr_id.is_none() && !open.contains(r.id.as_str()))
                .map(|r| r.id.clone())
                .collect()
        });
        self.gone_since.retain(|id, _| gone.contains(id));
        for id in gone {
            let since = *self.gone_since.entry(id.clone()).or_insert(now);
            if now - since >= FORGET_IDLE && !restoring {
                self.gone_since.remove(&id);
                if let Some(p) = self.projects.project_mut(&project) {
                    p.workspaces.retain(|r| r.id != id);
                    changed = true;
                }
            }
        }
        if changed {
            self.projects.save();
        }
    }

    /// The tab layouts of the shown project's workspaces that differ from
    /// what was saved of them, by record id.
    fn changed_tabs(&self) -> Vec<(String, Vec<TabRecord>)> {
        let mut changed = Vec::new();
        for ws in &self.workspaces {
            let Some(record) = self.record(&ws.record) else {
                continue;
            };
            let tabs: Vec<TabRecord> = ws.tabs.iter().filter_map(Tab::save).collect();
            if record.tabs != tabs {
                changed.push((record.id.clone(), tabs));
            }
        }
        changed
    }

    /// Remember the tab layouts of the shown project's workspaces; whether
    /// anything changed.
    fn record_tabs(&mut self) -> bool {
        self.tabs_changed = None;
        let changed = self.changed_tabs();
        let Some(project) = self.project.clone() else {
            return false;
        };
        let any = !changed.is_empty();
        for (id, tabs) in changed {
            if let Some(record) = self.projects.record_mut(&project, &id) {
                record.tabs = tabs;
            }
        }
        self.record_workspace_order() || any
    }

    /// Keep the shown project's workspaces in the sidebar's order, which
    /// herdr is told of but the projects file isn't; whether it changed.
    fn record_workspace_order(&mut self) -> bool {
        let Some(project) = self.project.as_deref() else {
            return false;
        };
        let order: Vec<&str> = self
            .workspaces
            .iter()
            .map(|ws| ws.record.as_str())
            .collect();
        self.projects.order_workspaces(project, &order)
    }

    /// Read the Git status of the workspace folders now and every
    /// `GIT_INTERVAL`, off the event loop.
    fn watch_git(&mut self) {
        let (tx, rx): (_, Channel<HashMap<PathBuf, git::GitStatus>>) = channel::channel();
        let read = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(git) = event {
                state.git = git;
                state.git_pending = false;
                state.worktrees_read();
                state.repos_read();
                if std::mem::take(&mut state.git_again) {
                    state.refresh_git();
                }
            }
        });
        self.git_read = Some(tx);
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), move |_, _, state| {
                // A round can outlast the interval in a big repository.
                if !state.git_pending {
                    state.refresh_git();
                }
                TimeoutAction::ToDuration(GIT_INTERVAL)
            });
        if let Err(err) = read
            .map_err(|e| e.to_string())
            .and(timer.map_err(|e| e.to_string()))
        {
            tracing::warn!("git status: {err}");
        }
    }

    /// Read the Git status of the workspace folders now, off the event
    /// loop; once the round under way is done, if there is one.
    pub fn refresh_git(&mut self) {
        let Some(tx) = self.git_read.clone() else {
            return;
        };
        if self.git_pending {
            self.git_again = true;
            return;
        }
        self.git_pending = true;
        let mut dirs: Vec<PathBuf> = self.workspaces.iter().map(|ws| ws.dir.clone()).collect();
        dirs.sort();
        dirs.dedup();
        git::read_all(dirs, move |git| {
            let _ = tx.send(git);
        });
    }

    /// Ask the browsers where the web pages' windows are now and every
    /// `WEB_INTERVAL`, off the event loop (see `web_pages`).
    fn watch_web_pages(&mut self) {
        let (tx, rx): (_, Channel<Vec<(web_pages::WebPane, String)>>) = channel::channel();
        let read = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(found) = event {
                state.web_pending = false;
                state.take_web_urls(found);
            }
        });
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), move |_, _, state| {
                if !state.web_pending {
                    let panes = state.web_panes();
                    if !panes.is_empty() {
                        state.web_pending = true;
                        let tx = tx.clone();
                        web_pages::read_all(panes, move |found| {
                            let _ = tx.send(found);
                        });
                    }
                }
                TimeoutAction::ToDuration(WEB_INTERVAL)
            });
        if let Err(err) = read
            .map_err(|e| e.to_string())
            .and(timer.map_err(|e| e.to_string()))
        {
            tracing::warn!("web pages: {err}");
        }
    }

    /// The panes showing a web page launched by its link, with their
    /// windows' titles.
    fn web_panes(&self) -> Vec<web_pages::WebPane> {
        let mut panes = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for pane in &tab.panes {
                    let Some(command) = pane.app_command.as_deref() else {
                        continue;
                    };
                    if !is_web_link(command) {
                        continue;
                    }
                    let Some(title) = window_title(&pane.window) else {
                        continue;
                    };
                    panes.push(web_pages::WebPane {
                        workspace: w,
                        tab: t,
                        pane: pane.id,
                        command: command.to_owned(),
                        title,
                    });
                }
            }
        }
        panes
    }

    /// Take in where the web pages are now: each pane still showing the
    /// link it was asked about now has the page's URL as its command, for
    /// the tab layouts to save (see `record_tabs`) and a restart to open.
    fn take_web_urls(&mut self, found: Vec<(web_pages::WebPane, String)>) {
        for (web, url) in found {
            let pane = self
                .workspaces
                .get_mut(web.workspace)
                .and_then(|ws| ws.tabs.get_mut(web.tab))
                .and_then(|tab| tab.panes.iter_mut().find(|p| p.id == web.pane));
            if let Some(pane) = pane
                && pane.app_command.as_deref() == Some(web.command.as_str())
            {
                tracing::debug!("web page {} now at {url}", web.command);
                pane.app_command = Some(url);
            }
        }
    }

    /// Ask herdr which agents run in the session now.
    fn refresh_agents(&mut self) {
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        match herdr.agent_list() {
            Ok(agents) => {
                self.agents = agents;
                self.notify_agents();
            }
            Err(err) => tracing::debug!("herdr: {err}"),
        }
    }

    /// Have the agents and tabs read again soon, off the event loop.
    fn poke_herdr(&self) {
        if let Some(poke) = &self.herdr_poke {
            let _ = poke.send(());
        }
    }

    /// Notify of each agent that just came to need input or finished where
    /// the user can't see it: in a hidden tab, or anywhere while gerdr's
    /// window is in the background (see `agents::agent_news`). herdr's own
    /// idea of what the user has seen follows its focused pane, not
    /// gerdr's, so a finished agent may come back `idle` rather than
    /// `done`: both count.
    fn notify_agents(&mut self) {
        let mut watched: Vec<agents::Watched> = self
            .agents
            .iter()
            .map(|agent| {
                let place = self.herdr_pane_place(&agent.pane_id);
                let mut body: Vec<&str> = place
                    .map(|(w, _)| self.workspaces[w].name.as_str())
                    .into_iter()
                    .collect();
                body.extend(
                    agent
                        .title
                        .as_deref()
                        .map(str::trim)
                        .filter(|t| !t.is_empty()),
                );
                agents::Watched {
                    key: agents::AgentKey::Herdr(agent.pane_id.clone()),
                    status: agent.agent_status,
                    label: agent.label().to_owned(),
                    body: body.join(" · "),
                    seen: self.host_focused && place.is_some_and(|(w, t)| self.is_visible(w, t)),
                }
            })
            .collect();
        watched.extend(self.cursor_watched());
        let news = agents::agent_news(
            &self.agent_statuses,
            &watched,
            &mut self.blocked_since,
            Instant::now(),
            BLOCKED_REMINDER,
        );
        self.agent_statuses = watched.into_iter().map(|a| (a.key, a.status)).collect();
        if let Some(notifier) = self.notifier.as_mut() {
            for news in news {
                notifier.send(&news.key.to_string(), &news.summary, &news.body);
            }
        }
    }

    /// Follow what look the system prefers (see `system_theme`), for
    /// `ThemeMode::System`, from `first`, the reading started at start if
    /// it wasn't waited for.
    fn watch_system_theme(&mut self, first: Option<system_theme::Reading>) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(dark) = event {
                state.system_theme_changed(dark);
            }
        });
        match inserted {
            Ok(_) => system_theme::watch(first, move |dark| {
                let _ = tx.send(dark);
            }),
            Err(err) => tracing::warn!("system theme: {err}"),
        }
    }

    /// Connect to the host's notification daemon, and show the agent of
    /// each notification the user clicks.
    fn connect_notifier(&mut self) {
        let notifier = match notify::Notifier::connect() {
            Ok(notifier) => notifier,
            Err(err) => return tracing::warn!("notifications: {err}"),
        };
        let fd = match notifier.fd() {
            Ok(fd) => fd,
            Err(err) => return tracing::warn!("notifications: {err}"),
        };
        let source = Generic::new(fd, Interest::READ, smithay::reexports::calloop::Mode::Level);
        let inserted = self.loop_handle.insert_source(source, |_, _, state| {
            let Some(notifier) = state.notifier.as_mut() else {
                return Ok(PostAction::Remove);
            };
            match notifier.dispatch() {
                Ok(clicked) => {
                    for key in clicked {
                        state.focus_agent(&agents::AgentKey::parse(&key));
                        state.backend.window().focus_window();
                    }
                    Ok(PostAction::Continue)
                }
                Err(()) => {
                    tracing::warn!("session bus went away; no more notifications");
                    state.notifier = None;
                    Ok(PostAction::Remove)
                }
            }
        });
        match inserted {
            Ok(_) => self.notifier = Some(notifier),
            Err(err) => tracing::warn!("notifications: {err}"),
        }
    }

    /// The workspace and tab herdr pane `pane_id` is in.
    fn herdr_pane_place(&self, pane_id: &str) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            Some((
                w,
                ws.tabs
                    .iter()
                    .position(|t| t.herdr_panes().any(|p| p.pane_id == pane_id))?,
            ))
        })
    }

    /// Show agent `key`: herdr's in its pane, a Cursor chat in Cursor.
    pub fn focus_agent(&mut self, key: &agents::AgentKey) {
        match key {
            agents::AgentKey::Herdr(pane_id) => self.focus_herdr_agent(pane_id),
            agents::AgentKey::Cursor(chat) => self.focus_cursor_chat(chat),
        }
    }

    /// Show the pane agent card `pane_id` stands for: switch to its workspace
    /// and tab and focus its window. herdr counts the agent as seen then.
    fn focus_herdr_agent(&mut self, pane_id: &str) {
        let shows = |p: &Pane| p.herdr.as_ref().is_some_and(|h| h.pane_id == pane_id);
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs.iter().enumerate().find_map(|(t, tab)| {
                Some((w, t, tab.panes.iter().find(|p| shows(p))?.window.clone()))
            })
        });
        let Some((w, t, window)) = found else { return };
        self.active_workspace = w;
        let ws = &mut self.workspaces[w];
        ws.active_tab = t;
        ws.tabs[t].focused = Some(window);
        self.show_active();
        if let Some(herdr) = self.herdr.as_mut()
            && let Err(err) = herdr.agent_focus(pane_id)
        {
            tracing::debug!("herdr: {err}");
        }
        self.poke_herdr();
    }

    /// The agent cards of each workspace, in sidebar order.
    pub fn agent_views(&self) -> Vec<Vec<AgentView>> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(w, ws)| {
                let mut views = self.herdr_cards(ws);
                views.extend(self.cursor_cards(w));
                views
            })
            .collect()
    }

    /// The cards of herdr's agents in workspace `ws`, by tab.
    fn herdr_cards(&self, ws: &Workspace) -> Vec<AgentView> {
        {
            {
                let Some(id) = ws.herdr_id() else {
                    return Vec::new();
                };
                let tab_of = |pane_id: &str| {
                    ws.tabs
                        .iter()
                        .position(|t| t.herdr_panes().any(|p| p.pane_id == pane_id))
                };
                let mut cards: Vec<_> = self
                    .agents
                    .iter()
                    .filter(|a| a.workspace_id == id)
                    .map(|a| (tab_of(&a.pane_id), a))
                    .collect();
                // By tab, keeping herdr's order within one.
                cards.sort_by_key(|(tab, _)| tab.unwrap_or(usize::MAX));
                let pane_of = |a: &herdr::AgentInfo| {
                    self.herdr_tabs
                        .get(&a.tab_id)?
                        .panes
                        .iter()
                        .find(|p| p.pane_id == a.pane_id)
                };
                cards
                    .into_iter()
                    .map(|(_, a)| AgentView::new(a, pane_of(a)))
                    .collect()
            }
        }
    }

    /// Unlink herdr pane `pane_id` from what shows it; returns its window, or
    /// the pid of the terminal still starting for it.
    fn forget_herdr_pane(&mut self, pane_id: &str) -> Vec<(Option<Window>, u32)> {
        let mut found = Vec::new();
        let is_it = |h: &Option<herdr::PaneInfo>| h.as_ref().is_some_and(|p| p.pane_id == pane_id);
        for tab in self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs) {
            for pane in tab.panes.iter_mut().filter(|p| is_it(&p.herdr)) {
                pane.herdr = None;
                found.push((Some(pane.window.clone()), 0));
            }
            for e in tab.expected.iter_mut().filter(|e| is_it(&e.herdr)) {
                e.herdr = None;
                found.extend(e.pid.map(|pid| (None, pid)));
            }
        }
        found
    }

    /// Show the panes of herdr workspace `workspace_id` that gerdr doesn't
    /// show yet: in the tab mirroring theirs, else in a new tab. A new tab
    /// holding just the panes a saved tab held is rebuilt as saved, with its
    /// Zed and apps; any other is laid out like herdr's, with the apps saved
    /// for it to the right. A new workspace takes its kind and folder from
    /// the project's record, else from what gerdr told herdr, and its active
    /// tab from herdr.
    fn sync_herdr_workspace(&mut self, workspace_id: &str) {
        let known: HashSet<String> = self
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(Tab::herdr_panes)
            .map(|p| p.pane_id.clone())
            .collect();
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        // Events replay when the subscription opens: what they name may be gone.
        let Ok(panes) = herdr.pane_list(workspace_id) else {
            return;
        };
        let panes: Vec<_> = panes
            .into_iter()
            .filter(|p| !known.contains(&p.pane_id))
            .collect();
        if panes.is_empty() {
            return;
        }
        // Or the one whose record it is, back after its panes went.
        let mirroring = self
            .project
            .as_deref()
            .and_then(|p| self.projects.project(p)?.herdr_record(workspace_id))
            .map(|r| r.id.as_str());
        let existing = self.workspaces.iter().position(|ws| {
            ws.herdr_id() == Some(workspace_id) || Some(ws.record.as_str()) == mirroring
        });
        let info = match existing {
            Some(_) => None,
            None => match herdr.workspace_get(workspace_id) {
                Ok(info) => Some(info),
                Err(_) => return,
            },
        };
        // The new panes by tab, in herdr's tab order, with herdr's layout for new tabs.
        let order = herdr.tabs(workspace_id).unwrap_or_default();
        let focused_tab = order.iter().find(|t| t.focused).map(|t| t.tab_id.clone());
        let mut tabs: Vec<(String, Vec<herdr::PaneInfo>, Vec<herdr::LayoutPane>)> = Vec::new();
        for tab_id in order.into_iter().map(|t| t.tab_id) {
            let in_tab: Vec<_> = panes
                .iter()
                .filter(|p| p.tab_id == tab_id)
                .cloned()
                .collect();
            if in_tab.is_empty() {
                continue;
            }
            let mirrored = existing.is_some_and(|w| {
                self.workspaces[w]
                    .tabs
                    .iter()
                    .any(|t| t.herdr_tab_id() == Some(&tab_id))
            });
            let cells = if mirrored {
                Vec::new()
            } else {
                herdr.pane_layout(&in_tab[0].pane_id).unwrap_or_default()
            };
            tabs.push((tab_id, in_tab, cells));
        }

        let record = self.herdr_record(workspace_id).cloned();
        let saved = record.as_ref().map_or(Vec::new(), |r| r.tabs.clone());
        let w = match existing {
            Some(w) => w,
            None => {
                let info = info.unwrap();
                let (dir, kind, id) = match &record {
                    Some(record) => (
                        record.dir.clone(),
                        WorkspaceKind::from_record(record.kind),
                        record.id.clone(),
                    ),
                    None => {
                        let dir = info
                            .dir()
                            .or_else(|| panes[0].cwd.clone())
                            .or_else(home_dir)
                            .unwrap_or_default();
                        let kind = if info.is_project() {
                            WorkspaceKind::Project
                        } else {
                            WorkspaceKind::Terminal
                        };
                        let id =
                            self.remember_workspace(&info.label, kind, &dir, Some(workspace_id));
                        (dir, kind, id)
                    }
                };
                let mut ws = Workspace::new(dir, kind);
                ws.name = info.label;
                ws.record = id;
                self.insert_workspace(ws)
            }
        };
        let dir = self.workspaces[w].dir.clone();
        let area = self.content_rect();
        // Whether a tab came back as saved: it brings its own Zed, if any.
        let mut restored_any = false;
        let mut zed_expected = false;
        for (tab_id, in_tab, cells) in tabs {
            let mirrored = self.workspaces[w]
                .tabs
                .iter()
                .position(|t| t.herdr_tab_id() == Some(&tab_id));
            let live: HashSet<&str> = in_tab.iter().map(|p| p.pane_id.as_str()).collect();
            let saved_tab = saved
                .iter()
                .find(|r| same_panes(&r.layout, &live))
                .filter(|_| mirrored.is_none());
            // Saved for this tab, but its panes changed since.
            let extra_apps: Vec<String> = match (mirrored, saved_tab) {
                (None, None) => saved
                    .iter()
                    .find(|r| r.herdr_tab.as_deref() == Some(tab_id.as_str()))
                    .map_or(Vec::new(), |r| {
                        saved_apps(&r.layout)
                            .into_iter()
                            .map(str::to_owned)
                            .collect()
                    }),
                _ => Vec::new(),
            };
            let mut started = Vec::new();
            for pane in in_tab {
                if let Some(pid) = self.attach(&pane, &dir) {
                    started.push((self.new_pane_id(), pid, pane));
                }
            }
            let mut restored = RestoredTab::default();
            let t = match mirrored {
                Some(t) => t,
                None => {
                    let layout = match saved_tab {
                        Some(saved_tab) => {
                            restored_any = true;
                            let ids = started
                                .iter()
                                .map(|(id, _, pane)| (pane.pane_id.clone(), *id))
                                .collect();
                            restored = restore_tab(&saved_tab.layout, &ids, &mut self.next_pane);
                            restored.layout.take()
                        }
                        None => {
                            let rects: Vec<_> = started
                                .iter()
                                .filter_map(|(id, _, pane)| {
                                    let c = cells.iter().find(|c| c.pane_id == pane.pane_id)?.rect;
                                    Some((
                                        *id,
                                        Rectangle::new(
                                            (c.x, c.y).into(),
                                            (c.width, c.height).into(),
                                        ),
                                    ))
                                })
                                .collect();
                            Node::from_rects(&rects)
                        }
                    };
                    let ws = &mut self.workspaces[w];
                    ws.tabs.push(Tab {
                        layout,
                        label: saved_tab.and_then(|r| r.label.clone()),
                        ..Default::default()
                    });
                    ws.tabs.len() - 1
                }
            };
            let tab = &mut self.workspaces[w].tabs[t];
            for (id, pid, pane) in started {
                if !tab
                    .layout
                    .as_ref()
                    .is_some_and(|l| l.leaves().contains(&id))
                {
                    tab.split(None, id, None, area);
                }
                tab.expected
                    .push(Expected::terminal(pid, id, false, Some(pane)));
            }
            for command in extra_apps {
                let id = self.new_pane_id();
                self.workspaces[w].tabs[t].split(None, id, Some(Dir::Right), area);
                restored.apps.push((id, command));
            }
            zed_expected |= self.fill_restored(w, t, restored);
        }

        if existing.is_none() {
            let (restored, zed) = self.restore_app_tabs(w, &saved, false);
            restored_any |= restored;
            zed_expected |= zed;
            let ws = &mut self.workspaces[w];
            if let Some(t) = focused_tab
                .and_then(|id| ws.tabs.iter().position(|t| t.herdr_tab_id() == Some(&id)))
            {
                ws.active_tab = t;
            }
        }

        // A project gerdr opened gets its Zed back over the first tab,
        // unless its tabs came back as saved.
        let ws = &self.workspaces[w];
        if existing.is_none()
            && !restored_any
            && ws.kind == WorkspaceKind::Project
            && !ws.tabs.is_empty()
        {
            let zed = self.new_pane_id();
            let tab = &mut self.workspaces[w].tabs[0];
            let below = tab.layout.take().map(Box::new);
            tab.layout = Some(match below {
                Some(second) => Node::Split {
                    dir: Dir::Down,
                    ratio: TOP_RATIO,
                    first: Box::new(Node::Leaf(zed)),
                    second,
                },
                None => Node::Leaf(zed),
            });
            tab.expected.push(Expected::new(None, "zed", zed, true));
            zed_expected = true;
        }
        if zed_expected {
            self.launch_next_zed();
        }
        // Once the whole workspace is back: an app that didn't start left
        // its leaf, and maybe its tab, empty.
        self.prune();
        self.arrange();
    }

    /// Bring back the saved tabs of workspace `w` without terminals (Zed
    /// alone, apps) where they were; with `all`, the others too, without
    /// their terminals. Whether any came back, and whether a Zed window was
    /// queued.
    fn restore_app_tabs(&mut self, w: usize, saved: &[TabRecord], all: bool) -> (bool, bool) {
        let mut restored_any = false;
        let mut zed_expected = false;
        for (i, saved_tab) in saved.iter().enumerate() {
            if !all && !saved_tab.layout.herdr_panes().is_empty() {
                continue;
            }
            let mut restored = restore_tab(&saved_tab.layout, &HashMap::new(), &mut self.next_pane);
            let Some(layout) = restored.layout.take() else {
                continue;
            };
            restored_any = true;
            let ws = &mut self.workspaces[w];
            let t = i.min(ws.tabs.len());
            ws.tabs.insert(
                t,
                Tab {
                    layout: Some(layout),
                    label: saved_tab.label.clone(),
                    ..Default::default()
                },
            );
            zed_expected |= self.fill_restored(w, t, restored);
        }
        (restored_any, zed_expected)
    }

    /// Bring back the shown project's workspaces herdr won't: those its
    /// records keep without a herdr workspace (all of them, if `all`: herdr
    /// isn't there), with their Zed and apps, minus their terminals, in
    /// their place in the records' order. One with nothing else to bring
    /// back is forgotten.
    fn restore_saved(&mut self, all: bool) {
        let Some(records) = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
        else {
            return;
        };
        let records: Vec<WorkspaceRecord> = records
            .workspaces
            .iter()
            .filter(|r| all || r.herdr_id.is_none())
            .filter(|r| self.workspaces.iter().all(|ws| ws.record != r.id))
            .cloned()
            .collect();
        let mut zed_expected = false;
        for record in records {
            if !record.tabs.iter().any(|t| t.layout.holds_apps()) {
                if record.herdr_id.is_none() {
                    self.forget_workspace(&record.id);
                }
                continue;
            }
            let mut ws =
                Workspace::new(record.dir.clone(), WorkspaceKind::from_record(record.kind));
            if !record.label.is_empty() {
                ws.name = record.label.clone();
            }
            ws.record = record.id.clone();
            let w = self.insert_workspace(ws);
            zed_expected |= self.restore_app_tabs(w, &record.tabs, true).1;
        }
        if zed_expected {
            self.launch_next_zed();
        }
        self.prune();
        self.arrange();
    }

    /// Add `ws` in its record's place in the shown project's order (last
    /// without one), keeping the active workspace; its index.
    fn insert_workspace(&mut self, ws: Workspace) -> usize {
        let order: Vec<&str> = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map_or(Vec::new(), |p| {
                p.workspaces.iter().map(|r| r.id.as_str()).collect()
            });
        let place = |id: &str| order.iter().position(|o| *o == id).unwrap_or(usize::MAX);
        let mine = place(&ws.record);
        let w = self
            .workspaces
            .iter()
            .filter(|open| place(&open.record) <= mine)
            .count();
        if w <= self.active_workspace && w < self.workspaces.len() {
            self.active_workspace += 1;
        }
        self.workspaces.insert(w, ws);
        w
    }

    /// Fill the leaves `restored` kept in tab `t` of workspace `w`: queue
    /// its Zed windows and launch its apps. Whether a Zed window was queued.
    fn fill_restored(&mut self, w: usize, t: usize, restored: RestoredTab) -> bool {
        let tab = &mut self.workspaces[w].tabs[t];
        for &zed in &restored.zed {
            tab.expected.push(Expected::new(None, "zed", zed, true));
        }
        for (id, command) in restored.apps {
            self.launch_into(w, t, Some(id), &command);
        }
        !restored.zed.is_empty()
    }

    /// Open `dir` as a project workspace (Zed on top, a terminal below) and
    /// show it. A folder already open as a project is just switched to.
    pub fn open_project(&mut self, dir: PathBuf) {
        let dir = dir.canonicalize().unwrap_or(dir);
        if let Some(idx) = self
            .workspaces
            .iter()
            .position(|ws| ws.kind == WorkspaceKind::Project && ws.dir == dir)
        {
            self.switch_workspace(idx);
            return;
        }

        switcher::remember(&dir);
        self.frecency.touch(&format!("folder:{}", dir.display()));
        self.frecency.save();
        let zed = self.new_pane_id();
        let mut tab = Tab {
            layout: Some(Node::Leaf(zed)),
            ..Default::default()
        };
        tab.expected.push(Expected::new(None, "zed", zed, true));
        let mut ws = self.new_workspace(dir, WorkspaceKind::Project);
        ws.tabs.push(tab);
        self.workspaces.push(ws);
        let w = self.workspaces.len() - 1;
        if let Some((pid, herdr)) = self.start_terminal(w, Some(0), Dir::Down) {
            let term = self.new_pane_id();
            let tab = &mut self.workspaces[w].tabs[0];
            tab.layout
                .as_mut()
                .unwrap()
                .split(zed, term, Dir::Down, TOP_RATIO);
            tab.expected
                .push(Expected::terminal(pid, term, false, herdr));
        }

        self.switch_workspace(w);
        self.launch_next_zed();
        self.prune();
    }

    /// Open a new terminal workspace in `dir` (one full-height terminal) and show it.
    pub fn open_terminal(&mut self, dir: PathBuf) {
        let dir = dir.canonicalize().unwrap_or(dir);
        switcher::remember(&dir);
        let ws = self.new_workspace(dir, WorkspaceKind::Terminal);
        self.workspaces.push(ws);
        let w = self.workspaces.len() - 1;
        let Some((pid, herdr)) = self.start_terminal(w, None, Dir::Right) else {
            if let Some(ws) = self.workspaces.pop() {
                self.forget_workspace(&ws.record);
            }
            return;
        };
        let id = self.new_pane_id();
        self.workspaces[w].tabs.push(Tab::terminal(pid, id, herdr));
        self.switch_workspace(w);
        self.prune();
    }

    /// Zed windows come from a shared editor process, so they can only be told
    /// apart by app_id. Start them one at a time: the next queued one once the
    /// previous window has shown up (or never will). A window restored with
    /// the project opens the workspace folder; one launched by command runs
    /// that command (with gerdr's data dir, see `launch::own_instance`).
    /// Whether a window that didn't start left its leaf, for the caller to
    /// prune.
    fn launch_next_zed(&mut self) -> bool {
        if self.old_zed_running() {
            return false; // reaping tries again
        }
        self.launch_next(launch::ZED_APP_ID, |gerdr, w, t, i| {
            let dir = gerdr.workspaces[w].dir.clone();
            let command = gerdr.workspaces[w].tabs[t].expected[i].app_command.clone();
            let pid = match command {
                Some(command) => gerdr.start_app(w, t, &command)?,
                None => {
                    // `-n`: a new Zed window, handed to the running editor if there is one.
                    let mut zed = Command::new("zed");
                    zed.arg("-n")
                        .arg("--user-data-dir")
                        .arg(&gerdr.zed_data)
                        .arg(&dir);
                    gerdr.spawn(zed, &dir, "zed")?
                }
            };
            gerdr.zed_groups.push(pid as i32);
            Some(pid)
        })
    }

    /// Start the first window queued under `app_id` (an expectation without
    /// a pid) with `start`, given its workspace, tab and place among the
    /// tab's expected, unless one is already on its way: such windows are
    /// told apart by app_id only, so they come one at a time. One that
    /// doesn't start is dropped with its leaf, and the next is tried:
    /// whether any was, for the caller to prune.
    fn launch_next(
        &mut self,
        app_id: &str,
        mut start: impl FnMut(&mut Self, usize, usize, usize) -> Option<u32>,
    ) -> bool {
        let mut dropped = false;
        loop {
            let mut queued = None;
            for (w, ws) in self.workspaces.iter().enumerate() {
                for (t, tab) in ws.tabs.iter().enumerate() {
                    for (i, e) in tab
                        .expected
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.app_id == app_id)
                    {
                        if e.pid.is_some() {
                            return dropped; // one is already on its way
                        }
                        if !e.is_stopped() {
                            queued.get_or_insert((w, t, i));
                        }
                    }
                }
            }
            let Some((w, t, i)) = queued else {
                return dropped;
            };
            let pid = start(self, w, t, i);
            let tab = &mut self.workspaces[w].tabs[t];
            match pid {
                Some(pid) => {
                    tab.expected[i].pid = Some(pid);
                    return dropped;
                }
                // A Zed keeps its leaf, for the layout, with a Retry.
                None if app_id == launch::ZED_APP_ID => {
                    let e = &mut tab.expected[i];
                    e.timed_out = true;
                    let (pane, label) = (e.pane, e.label());
                    self.toast_not_started(pane, &label);
                }
                None => {
                    let gone = tab.expected.remove(i);
                    layout::remove(&mut tab.layout, gone.pane);
                    dropped = true;
                }
            }
        }
    }

    /// Say `label`, for pane `pane`, didn't start, with a Retry.
    fn toast_not_started(&mut self, pane: u32, label: &str) {
        self.toast_with(
            format!("{label} didn't start"),
            Level::Error,
            Some(("Retry".into(), MenuAction::RetryExpected(pane))),
        );
    }

    /// Whether the Zed of a project switched away from is still on its way
    /// out: a `zed -n` would hand its window to it, to be ended with it.
    /// Waited for until `old_zed_until`.
    fn old_zed_running(&mut self) -> bool {
        self.old_zed.retain(|g| group_alive(*g));
        if self.old_zed_until.is_some_and(|t| Instant::now() >= t) {
            self.old_zed.clear();
        }
        !self.old_zed.is_empty()
    }

    /// Ask the user for a folder (on the host) to open as a new workspace of `kind`.
    pub fn pick_workspace(&mut self, kind: WorkspaceKind) {
        let start = self
            .workspace()
            .and_then(|ws| ws.dir.parent())
            .map(Path::to_path_buf)
            .or_else(home_dir);
        let title = match kind {
            WorkspaceKind::Project => "Open project folder",
            WorkspaceKind::Terminal | WorkspaceKind::Cursor => "Open terminal folder",
        };
        self.pick_folder(title, start, PickFor::Workspace(kind));
    }

    /// Ask zenity for a folder for the project menu's form, starting in
    /// `start` (else the home folder).
    pub fn pick_folder_for_menu(&mut self, start: Option<PathBuf>) {
        let start = start.or_else(home_dir);
        self.pick_folder("Choose the project's folder", start, PickFor::MenuFolder);
    }

    /// Ask zenity for a folder (on the host) under `title`, starting in
    /// `start`; `poll_picker` hands the answer on as `purpose` says. One
    /// open for the same purpose already stays; one open for another gives
    /// way, so the folder picked goes where it was last asked for.
    fn pick_folder(&mut self, title: &str, start: Option<PathBuf>, purpose: PickFor) {
        match self.picker.take() {
            Some(open) if open.1 == purpose => return self.picker = Some(open),
            Some((mut other, _)) => {
                let _ = other.kill();
                let _ = other.wait();
            }
            None => {}
        }
        let mut cmd = Command::new("zenity");
        cmd.args(["--file-selection", "--directory"])
            .arg(format!("--title={title}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(start) = start {
            cmd.arg(format!("--filename={}/", start.display()));
        }
        match cmd.spawn() {
            Ok(child) => self.picker = Some((child, purpose)),
            Err(err) => tracing::warn!("folder picker (zenity) failed to start: {err}"),
        }
    }

    fn poll_picker(&mut self) {
        let Some((picker, _)) = &mut self.picker else {
            return;
        };
        let status = match picker.try_wait() {
            Ok(None) => return,
            Ok(Some(status)) => status,
            Err(_) => {
                self.picker = None;
                return;
            }
        };
        // Already reaped by `try_wait` above.
        #[allow(clippy::zombie_processes)]
        let (mut picker, kind) = self.picker.take().unwrap();
        let mut out = String::new();
        if let Some(stdout) = picker.stdout.as_mut() {
            let _ = stdout.read_to_string(&mut out);
        }
        let path = out.trim_end_matches('\n');
        if status.success() && !path.is_empty() {
            match kind {
                PickFor::Workspace(WorkspaceKind::Project) => {
                    self.open_project(PathBuf::from(path))
                }
                PickFor::Workspace(WorkspaceKind::Terminal) => {
                    self.open_terminal(PathBuf::from(path))
                }
                PickFor::Workspace(WorkspaceKind::Cursor) => self.open_cursor(None),
                PickFor::MenuFolder => self.project_menu_folder(PathBuf::from(path)),
                PickFor::Venv { project, workspace } => {
                    self.set_venv(&project, &workspace, PathBuf::from(path))
                }
            }
        }
    }

    /// Close every pane of the workspace; it goes away once their windows do.
    pub fn close_workspace(&mut self, idx: usize) {
        if let Some(ws) = self.workspaces.get(idx) {
            // Closed on purpose: once gone, it doesn't come back.
            self.closing_records.insert(ws.record.clone());
            let tabs: Vec<_> = ws.tabs.iter().map(Self::closing).collect();
            tabs.into_iter().for_each(|panes| self.close_panes(panes));
            for t in 0..self.workspaces[idx].tabs.len() {
                self.drop_unstarted(idx, t);
            }
            self.prune();
            self.arrange();
        }
    }

    /// Give up on the windows of tab `t` of workspace `w`, closing, that
    /// nothing was started for (a Zed's that didn't start, say): no window
    /// would come to close.
    fn drop_unstarted(&mut self, w: usize, t: usize) {
        let tab = &mut self.workspaces[w].tabs[t];
        let (gone, kept) = std::mem::take(&mut tab.expected)
            .into_iter()
            .partition(|e| e.pid.is_none() && e.herdr.is_none());
        tab.expected = kept;
        for e in gone {
            layout::remove(&mut tab.layout, e.pane);
        }
    }

    /// What closing `tab` takes: each window with the herdr pane it shows,
    /// and the herdr panes whose windows haven't shown up yet.
    fn closing(tab: &Tab) -> Vec<(Option<Window>, Option<herdr::PaneInfo>)> {
        let shown = tab
            .panes
            .iter()
            .map(|p| (Some(p.window.clone()), p.herdr.clone()));
        shown
            .chain(
                tab.expected
                    .iter()
                    .filter(|e| e.herdr.is_some())
                    .map(|e| (None, e.herdr.clone())),
            )
            .collect()
    }

    /// Close herdr panes in herdr, which ends their attached windows; ask
    /// other windows to close.
    fn close_panes(&mut self, panes: Vec<(Option<Window>, Option<herdr::PaneInfo>)>) {
        for (window, pane) in panes {
            let closed = match (self.herdr.as_mut(), &pane) {
                (Some(herdr), Some(pane)) => herdr
                    .pane_close(&pane.pane_id)
                    .inspect_err(|err| tracing::warn!("herdr: {err}"))
                    .is_ok(),
                _ => false,
            };
            if let Some(window) = window {
                if !closed {
                    window.toplevel().unwrap().send_close();
                }
                self.closing.push(window);
            }
        }
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.active_workspace = idx;
            self.show_active();
        }
    }

    /// A new tab with a full-height terminal in the current workspace.
    pub fn new_tab(&mut self) {
        if self.workspace().is_none() {
            return;
        }
        let Some((pid, herdr)) = self.start_terminal(self.active_workspace, None, Dir::Right)
        else {
            return;
        };
        let id = self.new_pane_id();
        let ws = &mut self.workspaces[self.active_workspace];
        ws.tabs.push(Tab::terminal(pid, id, herdr));
        ws.active_tab = ws.tabs.len() - 1;
        self.show_active();
    }

    /// Open a tab in workspace `w` whose shell runs `command` in `cd`, and
    /// show it.
    pub fn new_tab_running(&mut self, w: usize, cd: &Path, command: String) {
        if w >= self.workspaces.len() {
            return;
        }
        if w != self.active_workspace {
            self.switch_workspace(w);
        }
        let run = Some((cd.to_path_buf(), command));
        let Some((pid, herdr)) = self.start_terminal_running(w, None, Dir::Right, run) else {
            return;
        };
        let id = self.new_pane_id();
        let ws = &mut self.workspaces[w];
        ws.tabs.push(Tab::terminal(pid, id, herdr));
        ws.active_tab = ws.tabs.len() - 1;
        self.show_active();
    }

    /// Split the focused pane like herdr: it keeps the left or top half, a
    /// new terminal in the workspace folder gets the other and the focus.
    pub fn split_pane(&mut self, dir: Dir) {
        let Some(ws) = self.workspace() else { return };
        if ws.tabs.is_empty() {
            return;
        }
        let (w, t) = (self.active_workspace, ws.active_tab);
        let Some((pid, herdr)) = self.start_terminal(w, Some(t), dir) else {
            return;
        };
        let (id, area) = (self.new_pane_id(), self.content_rect());
        let tab = &mut self.workspaces[w].tabs[t];
        tab.split(None, id, Some(dir), area);
        tab.expected.push(Expected::terminal(pid, id, true, herdr));
        self.arrange();
    }

    /// Close the focused pane; its space goes to its sibling once its window is gone.
    pub fn close_pane(&mut self) {
        let Some(tab) = self.tab() else { return };
        if let Some(pane) = tab
            .panes
            .iter()
            .find(|p| Some(&p.window) == tab.focused.as_ref())
        {
            self.close_panes(vec![(Some(pane.window.clone()), pane.herdr.clone())]);
        }
    }

    /// Close every pane in the tab; the tab goes away once their windows do.
    pub fn close_tab(&mut self, idx: usize) {
        if let Some(tab) = self.workspace().and_then(|ws| ws.tabs.get(idx)) {
            self.close_panes(Self::closing(tab));
            self.drop_unstarted(self.active_workspace, idx);
            self.prune();
            self.arrange();
        }
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if let Some(ws) = self.workspaces.get_mut(self.active_workspace)
            && idx < ws.tabs.len()
        {
            ws.active_tab = idx;
            self.show_active();
        }
    }

    pub fn run_action(&mut self, action: Action) {
        let tabs = self.workspace().map_or(0, |ws| ws.tabs.len());
        let tab = self.workspace().map_or(0, |ws| ws.active_tab);
        let (wss, ws) = (self.workspaces.len(), self.active_workspace);
        // Without a project, there is nothing to act on yet.
        let anywhere = matches!(
            action,
            Action::Projects
                | Action::ToggleTheme
                | Action::Shortcuts
                | Action::ReloadProjects
                | Action::Quit
        );
        if self.project.is_none() && !anywhere {
            return;
        }
        match action {
            Action::NewTab => self.new_tab(),
            Action::CloseTab => self.close_tab(tab),
            Action::NextTab if tabs > 0 => self.switch_tab((tab + 1) % tabs),
            Action::PrevTab if tabs > 0 => self.switch_tab((tab + tabs - 1) % tabs),
            Action::GotoTab(idx) => self.switch_tab(idx),
            Action::NewProject => self.pick_workspace(WorkspaceKind::Project),
            Action::NewTerminal => self.open_terminal(home_dir().unwrap_or_default()),
            Action::CloseWorkspace => self.close_workspace(ws),
            Action::NextWorkspace if wss > 0 => self.switch_workspace((ws + 1) % wss),
            Action::PrevWorkspace if wss > 0 => self.switch_workspace((ws + wss - 1) % wss),
            Action::Split(dir) => self.split_pane(dir),
            Action::ClosePane => self.close_pane(),
            Action::FocusSide(side) => {
                let area = self.content_rect();
                self.focus_pane(|tab| {
                    layout::neighbor(&tab.shown_rects(area), tab.focused_id()?, side)
                });
            }
            Action::ToggleTheme => self.toggle_theme(),
            Action::ReloadProjects => self.reload_projects(),
            Action::Shortcuts => self.help_pinned = !self.help_pinned,
            Action::Switcher => self.open_switcher(),
            Action::Run => self.open_run_prompt(launch::Place::Split),
            Action::RunTab => self.open_run_prompt(launch::Place::Tab),
            Action::Projects if self.project.is_some() => self.toggle_project_menu(),
            Action::Projects => self.open_welcome(true),
            Action::LastProject => self.switch_to_last_project(),
            Action::ToggleRestart => self.toggle_restart(),
            Action::ToggleVenv => self.toggle_venv(),
            Action::PickVenv => self.pick_venv(),
            Action::RunStartCommands => {
                if let Some(id) = self.project.clone() {
                    self.run_start_commands(&id);
                }
            }
            Action::RestartPane => self.restart_focused_pane(),
            Action::StopPane => self.stop_focused_pane(),
            Action::MovePane(side) => {
                if let Some(id) = self.tab().and_then(Tab::focused_id) {
                    self.move_pane(id, side);
                }
            }
            Action::SwapPane(side) => {
                if let Some(id) = self.tab().and_then(Tab::focused_id) {
                    self.swap_pane(id, side);
                }
            }
            Action::FlipPane => {
                if let Some(id) = self.tab().and_then(Tab::focused_id) {
                    self.flip_pane(id);
                }
            }
            Action::PaneToNewTab => {
                if let Some(id) = self.tab().and_then(Tab::focused_id) {
                    self.move_pane_to_tab(id, None);
                }
            }
            Action::Worktrees => self.open_worktrees(None),
            Action::Repos => self.open_repos(),
            // Nothing to keep running without a project.
            Action::Quit if self.project.is_none() => self.quit_app(false),
            Action::Quit => self.open_quit(),
            Action::CyclePane(step) => self.focus_pane(|tab| {
                let shown = tab.shown();
                let current = shown.iter().position(|id| Some(*id) == tab.focused_id())? as isize;
                Some(shown[(current + step).rem_euclid(shown.len() as isize) as usize])
            }),
            _ => {}
        }
    }

    /// Whether the command of the active tab's terminal (see
    /// `Tab::restart_target`) runs again after a restart: flip it, and say
    /// so.
    fn toggle_restart(&mut self) {
        let pane = self.tab().and_then(Tab::restart_target).cloned();
        let project = self.project.clone();
        let record = pane
            .as_ref()
            .zip(project)
            .and_then(|(pane, project)| {
                self.projects
                    .herdr_record_mut(&project, &pane.workspace_id)?
                    .panes
                    .iter_mut()
                    .find(|r| r.pane_id == pane.pane_id)
            })
            .filter(|r| r.command.is_some());
        let summary = match record {
            None => "Nothing running in this pane".to_owned(),
            Some(record) => {
                let command = record.command.clone().unwrap_or_default();
                if let Some(i) = record.skip.iter().position(|c| *c == command) {
                    record.skip.remove(i);
                    format!("Will restart `{command}` after reboot")
                } else {
                    record.skip.push(command.clone());
                    format!("Won't restart `{command}` after reboot")
                }
            }
        };
        self.projects.save();
        self.toast(summary, Level::Info);
    }

    /// The shown project and the active workspace's record: whose shells a
    /// venv setting is for.
    fn venv_target(&self) -> Option<(String, String)> {
        let project = self.project.clone()?;
        let workspace = self.workspace()?.record.clone();
        self.record(&workspace)?;
        Some((project, workspace))
    }

    /// Whether new shells of the active workspace activate a venv: flip
    /// between the one they find and none, and say so.
    fn toggle_venv(&mut self) {
        let record = self
            .venv_target()
            .and_then(|(project, workspace)| self.projects.record_mut(&project, &workspace));
        let Some(record) = record else {
            return self.show_notice("No shells here to set a venv for".into());
        };
        record.venv = match record.venv {
            VenvSetting::Off => VenvSetting::Auto,
            _ => VenvSetting::Off,
        };
        let text = venv_notice(&record.venv);
        self.projects.save();
        self.show_notice(text);
    }

    /// Ask zenity for the venv new shells of the active workspace activate,
    /// starting in the one set, else the workspace's folder.
    fn pick_venv(&mut self) {
        let Some((project, workspace)) = self.venv_target() else {
            return self.show_notice("No shells here to set a venv for".into());
        };
        let start = self
            .projects
            .project(&project)
            .and_then(|p| p.workspaces.iter().find(|r| r.id == workspace))
            .map(|r| match &r.venv {
                VenvSetting::Path(path) => r.dir.join(path),
                _ => r.dir.clone(),
            });
        let purpose = PickFor::Venv { project, workspace };
        self.pick_folder("Choose the workspace's Python venv", start, purpose);
    }

    /// zenity gave back `dir` for the venv of `project`'s `workspace`: the
    /// venv, or the `bin` folder in it.
    fn set_venv(&mut self, project: &str, workspace: &str, dir: PathBuf) {
        let venv = if dir.ends_with("bin") && !prepare::is_venv(&dir) {
            dir.parent().map(Path::to_path_buf)
        } else {
            Some(dir.clone())
        };
        let Some(venv) = venv.filter(|venv| prepare::is_venv(venv)) else {
            let dir = tilde(&dir, home_dir().as_deref());
            return self.show_notice(format!("Not a Python venv: {dir}"));
        };
        let Some(record) = self.projects.record_mut(project, workspace) else {
            return;
        };
        // Relative to the workspace's folder when in it, so it moves along.
        let path = venv
            .strip_prefix(&record.dir)
            .map_or_else(|_| venv.clone(), Path::to_path_buf);
        record.venv = VenvSetting::Path(path);
        let text = venv_notice(&record.venv);
        self.projects.save();
        self.show_notice(text);
    }

    /// Flip between light and dark, remembered for the next start.
    pub fn toggle_theme(&mut self) {
        self.set_theme_mode(self.theme().toggled().into());
    }

    /// Use `mode`, remembered for the next start.
    pub fn set_theme_mode(&mut self, mode: ThemeMode) {
        let before = self.theme();
        self.theme_mode = mode;
        if let Err(err) = std::fs::write(theme_file(), mode.name()) {
            tracing::warn!("saving theme: {err}");
        }
        self.theme_changed(before);
    }

    /// The system's preference came in (see `system_theme`): `dark` or not.
    pub fn system_theme_changed(&mut self, dark: bool) {
        let before = self.theme();
        self.system_dark = dark;
        self.theme_changed(before);
    }

    /// The look was `before` and may be another now: the terminals open
    /// keep their colours (they are passed on their command line), so say
    /// what new ones get.
    fn theme_changed(&mut self, before: Theme) {
        let now = self.theme();
        if now != before {
            self.toast(
                format!("New terminals use the {} theme", now.name()),
                Level::Info,
            );
        }
    }

    /// Focus the pane of the active tab that `pick` returns, if any.
    fn focus_pane(&mut self, pick: impl FnOnce(&Tab) -> Option<u32>) {
        let window = self.tab().and_then(|tab| tab.window(pick(tab)?).cloned());
        if window.is_some() {
            self.focus(window);
        }
    }

    /// Show the active workspace's active tab and focus its window.
    fn show_active(&mut self) {
        self.arrange();
        let target = self.tab().and_then(|tab| {
            tab.focused
                .clone()
                .or_else(|| tab.panes.first().map(|p| p.window.clone()))
        });
        self.focus(target);
        self.refresh_pointer_focus();
        let project = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map(|p| p.name.as_str());
        let title = self
            .workspace()
            .map(|ws| ws.name.as_str())
            .into_iter()
            .chain(project)
            .chain(["gerdr"])
            .collect::<Vec<_>>()
            .join(" — ");
        self.backend.window().set_title(&title);
    }

    /// Drop tabs with no windows left and nothing still starting, then
    /// workspaces with no tabs left.
    fn prune(&mut self) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            changed |= retain_active(&mut ws.tabs, &mut ws.active_tab, |t| {
                !t.panes.is_empty() || !t.expected.is_empty()
            });
        }
        changed |= retain_active(&mut self.workspaces, &mut self.active_workspace, |ws| {
            !ws.tabs.is_empty()
        });
        if changed {
            self.show_active();
            self.refresh_switcher();
        }
    }

    /// Forget children that exited, and windows that never showed up for them.
    fn reap_children(&mut self) {
        let mut exited = Vec::new();
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(None) => true,
            _ => {
                exited.push(child.id());
                false
            }
        });
        let now = Instant::now();
        let mut dropped = false;
        // A Zed that exited without a window: the next queued one may start.
        let mut stopped = false;
        // What took too long to show a window: its pane and what it is.
        let mut overdue = Vec::new();
        for tab in self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs) {
            let mut gone = Vec::new();
            tab.expected.retain_mut(|e| {
                if e.pid.is_some_and(|pid| exited.contains(&pid)) {
                    e.exited = Some(now);
                }
                if !e.timed_out && e.is_overdue(now) {
                    e.timed_out = true;
                    overdue.push((e.pane, e.label()));
                }
                // A launched app's window can't be told apart once its processes are gone.
                let grace = if e.app_id.is_empty() {
                    Duration::ZERO
                } else {
                    HANDOFF_GRACE
                };
                if e.exited.is_none_or(|t| now - t < grace) {
                    return true;
                }
                // A Zed keeps its leaf, for the layout, until Retry or Close.
                if e.app_id == launch::ZED_APP_ID {
                    if !e.timed_out {
                        overdue.push((e.pane, e.label()));
                    }
                    e.pid = None;
                    e.exited = None;
                    e.timed_out = true;
                    stopped = true;
                    return true;
                }
                gone.push(e.pane);
                false
            });
            // The space kept for a window that never came goes to the other panes.
            for pane in gone {
                layout::remove(&mut tab.layout, pane);
                dropped = true;
            }
        }
        if dropped || stopped || !self.old_zed.is_empty() {
            // The next queued windows, and the Zed waiting for the old one
            // to exit.
            dropped |= self.launch_next_zed();
        }
        if dropped {
            self.launch_next_apps();
            self.prune();
            self.arrange();
        }
        // Once per window that took too long or never came (see
        // `Expected::timed_out`).
        for (pane, label) in overdue {
            tracing::warn!("{label} didn't show a window");
            self.toast_not_started(pane, &label);
        }
        self.retired_groups.retain(|g| group_alive(*g));
        self.poll_picker();
    }

    /// Whether a window shown in a tab belongs to a process of `group`.
    fn group_shows_windows(&self, group: i32) -> bool {
        self.workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.panes)
            .filter_map(|p| self.window_pid(&p.window))
            .any(|pid| group_of(pid) == group)
    }

    /// The pid of the client showing `window`.
    fn window_pid(&self, window: &Window) -> Option<u32> {
        let dh = &self.display_handle;
        dh.get_client(window.toplevel()?.wl_surface().id())
            .ok()
            .and_then(|client| client.get_credentials(dh).ok())
            .map(|creds| creds.pid as u32)
    }

    /// Start again what pane `pane` (of any tab) waits for and took too
    /// long (see `reap_children`): the process started for it is ended (a
    /// `zed -n` would hand its window to the stuck editor), and a new one
    /// started the way the first was. One that doesn't start gives up its
    /// leaf.
    pub fn retry_expected(&mut self, pane: u32) {
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs.iter().enumerate().find_map(|(t, tab)| {
                let i = tab.expected.iter().position(|e| e.pane == pane)?;
                Some((w, t, i))
            })
        });
        let Some((w, t, i)) = found else {
            return;
        };
        if let Some(pid) = self.workspaces[w].tabs[t].expected[i].pid.take() {
            self.end_expected(pid);
        }
        let e = &mut self.workspaces[w].tabs[t].expected[i];
        e.exited = None;
        e.started = Instant::now();
        e.timed_out = false;
        e.closed = false;
        let (app_id, command, herdr_pane) =
            (e.app_id.clone(), e.app_command.clone(), e.herdr.clone());
        let dir = self.workspaces[w].dir.clone();
        let mut dropped = false;
        let started = match (app_id.as_str(), command, herdr_pane) {
            ("zed", ..) => {
                dropped = self.launch_next_zed();
                true
            }
            // Queued by app_id: started in its turn.
            (app_id, Some(_), _) if !app_id.is_empty() => {
                dropped = self.launch_next_apps();
                true
            }
            (_, Some(command), _) => match self.start_app(w, t, &command) {
                Some(pid) => {
                    self.workspaces[w].tabs[t].expected[i].pid = Some(pid);
                    true
                }
                None => false,
            },
            (_, None, Some(herdr_pane)) => match self.attach(&herdr_pane, &dir) {
                Some(pid) => {
                    self.workspaces[w].tabs[t].expected[i].pid = Some(pid);
                    true
                }
                None => false,
            },
            _ => false,
        };
        if !started {
            let tab = &mut self.workspaces[w].tabs[t];
            if let Some(i) = tab.expected.iter().position(|e| e.pane == pane) {
                let gone = tab.expected.remove(i);
                layout::remove(&mut tab.layout, gone.pane);
                dropped = true;
            }
        }
        if dropped {
            self.prune();
        }
        self.arrange();
    }

    /// End `pid`, started for a window that didn't show up: a `zed -n`
    /// would hand its window to the stuck editor.
    fn end_expected(&mut self, pid: u32) {
        self.zed_groups.retain(|g| *g != pid as i32);
        // A `zed` that started the editor leads its group, and the
        // editor may show other workspaces' windows by now: then only
        // the `zed` itself is ended, not the windows with it.
        let target = if self.group_shows_windows(pid as i32) {
            pid as i32
        } else {
            -(pid as i32)
        };
        // Safety: signals a process, or a process group, gerdr started
        // (see `spawn`).
        unsafe { libc::kill(target, libc::SIGTERM) };
    }

    /// Give up on what pane `pane` (of any tab) waits for (see
    /// `retry_expected`): end what was started for it, and the pane goes,
    /// its space to the others.
    pub fn close_expected(&mut self, pane: u32) {
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs.iter().enumerate().find_map(|(t, tab)| {
                let i = tab.expected.iter().position(|e| e.pane == pane)?;
                Some((w, t, i))
            })
        });
        let Some((w, t, i)) = found else {
            return;
        };
        let tab = &mut self.workspaces[w].tabs[t];
        let gone = tab.expected.remove(i);
        layout::remove(&mut tab.layout, gone.pane);
        if let Some(pid) = gone.pid {
            self.end_expected(pid);
        }
        if let Some(herdr_pane) = gone.herdr {
            self.close_panes(vec![(None, Some(herdr_pane))]);
        }
        // The next queued windows, if it was on its way.
        self.launch_next_zed();
        self.launch_next_apps();
        self.prune();
        self.arrange();
    }

    /// First commit of a toplevel: put it in the workspace, tab and pane it was started for.
    pub fn place(&mut self, window: Window, app_id: &str) {
        self.unplaced.retain(|w| w != &window);
        let pid = self.window_pid(&window);
        let app_id = app_id.to_ascii_lowercase();
        tracing::debug!("new window app_id={app_id} pid={pid:?}");

        let find = |pred: &dyn Fn(&Expected) -> bool| {
            self.workspaces.iter().enumerate().find_map(|(w, ws)| {
                ws.tabs
                    .iter()
                    .enumerate()
                    .find_map(|(t, tab)| tab.expected.iter().position(pred).map(|e| (w, t, e)))
            })
        };
        // The process we started nearest up the window's own, else its app_id.
        let lineage = pid.map(launch::lineage).unwrap_or_default();
        let ours = lineage
            .iter()
            .find_map(|pid| find(&|e| e.pid == Some(*pid)));
        // A Zed window the shown project waits for: Zed hands windows from
        // one process to another, so it may come from an old one.
        let zed = || find(&|e| e.pid.is_some() && e.app_id == "zed" && app_id.contains(&e.app_id));
        let claimed = ours.or_else(zed);
        // A late window of a project switched away from: its processes are
        // on their way out.
        let in_groups = |groups: &[i32]| {
            lineage.iter().any(|pid| {
                groups
                    .iter()
                    .any(|g| *g == *pid as i32 || *g == group_of(*pid))
            })
        };
        if claimed.is_none() && in_groups(&self.retired_groups) {
            window.toplevel().unwrap().send_close();
            return;
        }
        let found = claimed.or_else(|| {
            find(&|e| e.pid.is_some() && !e.app_id.is_empty() && app_id.contains(&e.app_id))
        });
        let Some((w, t, e)) = found else {
            // Not one we started: opened from a shell, maybe of a project
            // not shown, or by what gerdr started (a second Zed window).
            let stray = !in_groups(&self.process_groups);
            if stray && let Some(owner) = self.stray_owner(&lineage) {
                self.held.entry(owner).or_default().push(window);
                return;
            }
            return self.place_stray(window, stray);
        };
        let e = self.workspaces[w].tabs[t].expected.remove(e);
        let mut dropped = self.launch_next_zed();
        dropped |= self.launch_next_apps();
        let pane = Pane {
            id: e.pane,
            window,
            herdr: e.herdr,
            app_command: e.app_command,
            stray: false,
        };
        self.add_pane(w, t, pane, e.focus);
        if dropped {
            // A queued window that didn't start left its leaf, maybe its
            // tab, empty.
            self.prune();
            self.arrange();
        }
    }

    /// Show `window`, not one gerdr started, in the shown project: split
    /// the focused pane of the current tab along its longer side.
    fn place_stray(&mut self, window: Window, stray: bool) {
        if self.workspaces.is_empty() {
            let ws = self.new_workspace(home_dir().unwrap_or_default(), WorkspaceKind::Project);
            self.workspaces.push(ws);
            self.active_workspace = 0;
        }
        let w = self.active_workspace;
        let ws = &mut self.workspaces[w];
        if ws.tabs.is_empty() {
            ws.tabs.push(Tab::default());
            ws.active_tab = 0;
        }
        let t = ws.active_tab;
        let id = self.new_split(w, t);
        let pane = Pane {
            id,
            window,
            herdr: None,
            app_command: None,
            stray,
        };
        self.add_pane(w, t, pane, true);
    }

    /// A new leaf in tab `t` of workspace `w` for a window to come: split
    /// off the tab's focused pane along its longer side.
    fn new_split(&mut self, w: usize, t: usize) -> u32 {
        let (id, area) = (self.new_pane_id(), self.content_rect());
        self.workspaces[w].tabs[t].split(None, id, None, area);
        id
    }

    /// Put `pane` in tab `t` of workspace `w`, where its leaf is.
    fn add_pane(&mut self, w: usize, t: usize, pane: Pane, focus: bool) {
        let window = pane.window.clone();
        let tab = &mut self.workspaces[w].tabs[t];
        tab.panes.push(pane);
        // Zed and new splits get focus; the terminal under Zed only if nothing is focused yet.
        let takes_focus = focus || tab.focused.is_none();
        if takes_focus {
            tab.focused = Some(window.clone());
        }
        self.arrange();
        if takes_focus && self.is_visible(w, t) {
            self.focus(Some(window));
        }
    }

    /// The project to hold a stray window for, until it is shown, if not
    /// the one shown: the one whose herdr session the nearest process up
    /// `lineage` (from the window's own) runs in. A window of no known
    /// project waits for one to be shown (`""`).
    fn stray_owner(&self, lineage: &[u32]) -> Option<String> {
        let me = std::process::id();
        let owner = lineage
            .iter()
            .take_while(|pid| **pid != me)
            .find_map(|pid| {
                let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
                self.projects.project_of_environ(&environ)
            })
            .flatten();
        match (owner, self.project.as_deref()) {
            (Some(owner), shown) if shown != Some(owner) => Some(owner.to_owned()),
            (None, None) => Some(String::new()),
            _ => None,
        }
    }

    /// Show the windows held for project `id` (and those of no known
    /// project), now that it is shown.
    fn place_held(&mut self, id: &str) {
        let mut windows = self.held.remove(id).unwrap_or_default();
        windows.extend(self.held.remove("").unwrap_or_default());
        for window in windows {
            self.place_stray(window, true);
        }
    }

    fn is_visible(&self, workspace: usize, tab: usize) -> bool {
        workspace == self.active_workspace
            && self
                .workspaces
                .get(workspace)
                .is_some_and(|ws| ws.active_tab == tab)
    }

    /// Size every pane to its place in its tab's layout and tell the
    /// clients; show only the active tab.
    pub fn arrange(&mut self) {
        let scale = self.scale();
        let area = self.content_rect();
        // While a divider moves, clients may skip sizes between frames (Zed
        // otherwise rebuilds its swapchain for every one and falls far behind).
        let resizing = self.drag.is_some();
        let mut panes = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for (id, rect) in tab.shown_rects(area) {
                    // A terminal's bar takes the top of its place.
                    let rect = if tab.has_bar(id) {
                        chrome::pane_bar_rects_shown(rect, self.pane_bar_shown(tab, id)).1
                    } else {
                        rect
                    };
                    panes.push((w, t, tab.window(id).unwrap().clone(), rect));
                }
            }
        }
        for (w, t, window, rect) in panes {
            let toplevel = window.toplevel().unwrap();
            toplevel.with_pending_state(|state| {
                state.size = Some(rect.size);
                state.bounds = Some(rect.size);
                for s in [
                    xdg_toplevel::State::Maximized,
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                    xdg_toplevel::State::TiledTop,
                    xdg_toplevel::State::TiledBottom,
                ] {
                    state.states.set(s);
                }
                if resizing {
                    state.states.set(xdg_toplevel::State::Resizing);
                } else {
                    state.states.unset(xdg_toplevel::State::Resizing);
                }
            });
            toplevel.send_pending_configure();
            set_preferred_scale(toplevel.wl_surface(), scale);
            let id = toplevel.wl_surface().id();
            if !self.is_visible(w, t) {
                self.pending_place.remove(&id);
                self.space.unmap_elem(&window);
                continue;
            }
            // A window both moving and changing size moves once the client
            // has drawn the new size (see `pending_place`); while a divider
            // moves, it follows the divider.
            let size = window.geometry().size;
            let moving = self.space.element_location(&window) != Some(rect.loc);
            if !resizing && moving && size != rect.size && size.w > 0 && size.h > 0 {
                self.pending_place.insert(id, (size, rect.loc));
            } else {
                self.pending_place.remove(&id);
                self.space.map_element(window, rect.loc, false);
            }
        }
    }

    pub fn focus(&mut self, window: Option<Window>) {
        for tab in self.workspaces.iter_mut().flat_map(|ws| ws.tabs.iter_mut()) {
            for pane in &tab.panes {
                let active = Some(&pane.window) == window.as_ref();
                if active {
                    tab.focused = Some(pane.window.clone());
                }
                if pane.window.set_activated(active) {
                    pane.window.toplevel().unwrap().send_pending_configure();
                }
            }
        }
        let surface = window.map(|w| w.toplevel().unwrap().wl_surface().clone());
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, surface, smithay::utils::SERIAL_COUNTER.next_serial());
    }

    pub fn remove_window(&mut self, surface: &WlSurface) {
        self.pending_place.remove(&surface.id());
        let not_it = |w: &Window| w.toplevel().unwrap().wl_surface() != surface;
        self.unplaced.retain(not_it);
        for windows in self.held.values_mut() {
            windows.retain(not_it);
        }
        self.held.retain(|_, windows| !windows.is_empty());
        let is_it = |p: &Pane| p.window.toplevel().unwrap().wl_surface() == surface;
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs
                .iter()
                .enumerate()
                .find_map(|(t, tab)| tab.panes.iter().position(is_it).map(|p| (w, t, p)))
        });
        let asked = self.closing.iter().any(|w| !not_it(w));
        self.closing.retain(not_it);
        let Some((w, t, p)) = found else { return };

        let tab = &mut self.workspaces[w].tabs[t];
        let pane = tab.panes.remove(p);
        self.space.unmap_elem(&pane.window);
        // A Zed that crashed or quit keeps its leaf, for the layout, with a
        // Retry and a Close: Zed runs every workspace's window in one
        // process, so they all go at once.
        if is_zed(&pane.window) && !asked {
            tracing::warn!("a Zed window went away");
            let mut e = Expected::new(None, launch::ZED_APP_ID, pane.id, false);
            e.focus = tab.focused.as_ref() == Some(&pane.window);
            e.app_command = pane.app_command;
            e.timed_out = true;
            e.closed = true;
            tab.expected.push(e);
            if tab.focused.as_ref() == Some(&pane.window) {
                tab.focused = tab.panes.first().map(|p| p.window.clone());
            }
            self.arrange();
            if self.is_visible(w, t) {
                self.focus(self.workspaces[w].tabs[t].focused.clone());
            }
            return;
        }
        // Like herdr: the pane that takes over the space takes the focus too.
        let next = layout::remove(&mut tab.layout, pane.id);
        if tab.focused.as_ref() == Some(&pane.window) {
            let next = next
                .and_then(|id| tab.window(id))
                .or(tab.panes.first().map(|p| &p.window));
            tab.focused = next.cloned();
        }
        self.arrange();
        if self.is_visible(w, t) {
            self.focus(self.workspaces[w].tabs[t].focused.clone());
        }
        self.prune();
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.unplaced
            .iter()
            .chain(self.held.values().flatten())
            .chain(
                self.workspaces
                    .iter()
                    .flat_map(|ws| &ws.tabs)
                    .flat_map(|t| t.panes.iter().map(|p| &p.window)),
            )
            .find(|w| w.toplevel().unwrap().wl_surface() == surface)
            .cloned()
    }

    /// The client surface that gets the pointer at `pos`: none over a divider
    /// or while one, a workspace row or a tab is dragged.
    pub fn pointer_target(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let over_menu = self
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.layout().0.to_f64().contains(pos));
        let reordering = self.reorder.is_some_and(|r| r.moving);
        // These take every click.
        if self.quit.is_some()
            || self.welcome.is_some()
            || self.project_menu.is_some()
            || self.worktrees.is_open()
            || self.repos.is_open()
        {
            return None;
        }
        if over_menu || self.drag.is_some() || reordering || self.divider_at(pos).is_some() {
            return None;
        }
        self.surface_under(pos)
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
    }

    /// What the tabs of `ws` show.
    fn tab_views(&self, ws: &Workspace) -> Vec<TabView> {
        ws.tabs
            .iter()
            .map(|tab| {
                // A tab is named after its first window's title.
                let main = tab.shown().first().and_then(|id| tab.window(*id));
                let title: Option<String> = main.and_then(|window| {
                    with_states(window.toplevel().unwrap().wl_surface(), |states| {
                        let data = states
                            .data_map
                            .get::<XdgToplevelSurfaceData>()?
                            .lock()
                            .unwrap();
                        data.title.clone().or_else(|| data.app_id.clone())
                    })
                });
                let detail = tab.herdr_tab_id().and_then(|id| self.herdr_tabs.get(id));
                let mut view =
                    TabView::derive(tab.label.as_deref(), title.as_deref(), detail, &self.agents);
                let record = tab.restart_target().and_then(|p| self.pane_record(p));
                view.restart = record.and_then(|r| {
                    let command = r.command.as_ref()?;
                    Some(!r.skip.contains(command))
                });
                view
            })
            .collect()
    }

    /// What each workspace row in the sidebar shows, with the tabs of each.
    fn workspace_views(&self) -> Vec<(WorkspaceView, Vec<TabView>)> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(w, ws)| {
                let tabs = self.tab_views(ws);
                let mut view = WorkspaceView::new(ws.name.clone(), ws.kind, &tabs);
                if ws.kind == WorkspaceKind::Cursor {
                    view.icon = self.cursor_icon();
                }
                // Cursor's chats under it count as its tabs' agents do.
                view.status = self
                    .cursor_chats
                    .iter()
                    .filter(|chat| self.cursor_chat_place(chat) == Some(w))
                    .map(|chat| chat.status)
                    .chain(view.status)
                    .max_by_key(|s| chrome::urgency(*s));
                if let Some(git) = self.git.get(&ws.dir) {
                    // Without herdr the panes don't report a branch: take the folder's.
                    view.branch = view.branch.or_else(|| {
                        Some(git.branch.clone().unwrap_or_else(|| "detached HEAD".into()))
                    });
                    view.git = Some(git.clone());
                }
                (view, tabs)
            })
            .collect()
    }

    /// Show project `id`: the project shown now keeps running in the
    /// background. A project without workspaces asks for a folder.
    pub fn switch_project(&mut self, id: &str) {
        self.enter_project(id, true, Vec::new());
    }

    /// Show project `id` as `enter_project_now` does, opening `folders` in
    /// it; when its session doesn't run and it has start commands, once
    /// they are done (see `hook_done`). The project shown stays meanwhile.
    fn enter_project(&mut self, id: &str, pick: bool, folders: Vec<PathBuf>) {
        if self.project.as_deref() != Some(id)
            && !self.stopping.contains(id)
            && let Some(project) = self.projects.project(id)
        {
            let name = project.name.clone();
            let cold = || {
                !Hook::BeforeStart.line(project).is_empty() && !herdr::is_running(&project.session)
            };
            let still_starting = self.starting.contains(id);
            if still_starting || cold() {
                self.pending_enter = Some(PendingEnter {
                    id: id.to_owned(),
                    pick,
                    folders,
                });
                if still_starting {
                    return self.show_notice(format!("{name} is still starting"));
                }
                if self.run_hook(id, Hook::BeforeStart) {
                    return self.show_notice(format!("Starting {name}…"));
                }
                // They couldn't run: start it anyway.
                let pending = self.pending_enter.take();
                return self.enter_project_now(
                    id,
                    pick,
                    pending.map(|p| p.folders).unwrap_or_default(),
                );
            }
        }
        self.enter_project_now(id, pick, folders);
    }

    /// Show project `id`, opening `folders` in it; if it has no workspaces
    /// and `pick`, ask for a folder to open in it. The project left, if any,
    /// is remembered for `switch_to_last_project`, and a notice says it
    /// keeps running.
    fn enter_project_now(&mut self, id: &str, pick: bool, folders: Vec<PathBuf>) {
        // Another project was asked for since.
        self.pending_enter = None;
        self.show_project(id, pick);
        for folder in folders {
            self.open_project(folder);
        }
    }

    /// Show project `id`; if it has no workspaces and `pick`, ask for a
    /// folder to open in it.
    fn show_project(&mut self, id: &str, pick: bool) {
        if self.project.as_deref() == Some(id) {
            self.welcome = None;
            return;
        }
        let Some((session, name)) = self
            .projects
            .project(id)
            .map(|p| (p.session.clone(), p.name.clone()))
        else {
            return tracing::warn!("no project {id}");
        };
        // Its server is going down: attaching now would lose it.
        if self.stopping.contains(id) {
            return self.show_notice(format!("{name} is still stopping"));
        }
        let left = self.project.clone();
        self.detach_project();
        self.welcome = None;
        self.quit = None;
        self.project = Some(id.to_owned());
        self.projects.last_active = Some(id.to_owned());
        if let Some(project) = self.projects.project_mut(id) {
            project.last_used = projects::now();
            project.state = RunState::Kept;
        }
        self.projects.save();
        self.frecency.touch(&format!("project:{id}"));
        self.connect_herdr(&session);
        // Without herdr, what comes back without it still does.
        if self.herdr.is_none() {
            self.restore_saved(true);
        }
        self.running.insert(id.to_owned(), true);
        self.place_held(id);
        self.show_active();
        if let Some(left) = left {
            let name = |id: &str| self.projects.project(id).map(|p| p.name.clone());
            if let (Some(new), Some(old)) = (name(id), name(&left)) {
                self.show_notice(projects::switch_notice(&new, &old));
            }
            self.previous_project = Some(left);
        }
        if pick && self.workspaces.is_empty() {
            self.pick_workspace(WorkspaceKind::Project);
        }
    }

    /// Let go of the project shown: stop following its session, close the
    /// windows gerdr started for it (its terminals keep running in herdr)
    /// and end their processes a moment later, hold those its shells opened
    /// for when it is shown again, and forget what was shown. Children are
    /// still reaped.
    fn detach_project(&mut self) {
        if self.record_tabs() {
            self.projects.save();
        }
        for token in std::mem::take(&mut self.herdr_sources) {
            self.loop_handle.remove(token);
        }
        self.herdr_poke = None;
        let project = self.project.clone().unwrap_or_default();
        let panes: Vec<Pane> = self
            .workspaces
            .drain(..)
            .flat_map(|ws| ws.tabs)
            .flat_map(|tab| tab.panes)
            .collect();
        for pane in panes {
            if pane.stray {
                self.held
                    .entry(project.clone())
                    .or_default()
                    .push(pane.window);
            } else {
                pane.window.toplevel().unwrap().send_close();
            }
        }
        self.retire_children();
        if let Some((mut picker, _)) = self.picker.take() {
            let _ = picker.kill();
            let _ = picker.wait();
        }
        let windows: Vec<Window> = self.space.elements().cloned().collect();
        for window in windows {
            self.space.unmap_elem(&window);
        }
        self.agents.clear();
        self.blocked_since.clear();
        self.agent_statuses.clear();
        self.herdr_tabs.clear();
        self.git.clear();
        self.idle_since.clear();
        self.tabs_changed = None;
        self.pane_leaders.clear();
        self.last_commands.clear();
        self.pane_jobs.clear();
        self.pane_bar_hover = None;
        self.closed_workspaces.clear();
        self.gone_since.clear();
        self.closing_records.clear();
        self.restored_at = None;
        self.replay_pending.clear();
        self.herdr = None;
        self.active_workspace = 0;
        self.drag = None;
        self.reorder = None;
        self.pane_drop = None;
        self.context_menu = None;
        self.tooltip.hide();
        self.rename = None;
        self.stop_key_repeat();
        self.switcher = None;
        self.project_menu = None;
        self.worktrees.close();
        self.repos.close();
        self.header_hover = false;
        self.project = None;
        self.focus(None);
    }

    /// End the process groups gerdr started for the project shown, once
    /// their windows had `CLOSE_GRACE` to close: each child led its own, so
    /// this takes down what it handed off to too (Zed's editor). Their
    /// windows that still show up are closed (see `place`), and the next
    /// Zed waits for the old one to exit (see `launch_next_zed`).
    fn retire_children(&mut self) {
        let groups = std::mem::take(&mut self.process_groups);
        self.retired_groups.extend(&groups);
        self.retire_shared();
        self.old_zed.append(&mut self.zed_groups);
        self.old_zed_until = Some(Instant::now() + CLOSE_GRACE + ZED_EXIT_WAIT);
        let end = move |_, _: &mut (), _: &mut Gerdr| {
            for group in &groups {
                // Safety: signals a process group gerdr started.
                unsafe { libc::kill(-group, libc::SIGTERM) };
            }
            TimeoutAction::Drop
        };
        let timer = Timer::from_duration(CLOSE_GRACE);
        if let Err(err) = self.loop_handle.insert_source(timer, end) {
            tracing::warn!("ending the old project's processes: {err}");
            // Its windows had no time to close.
            self.kill_children();
        }
    }

    /// End every process group gerdr started, and those retired still
    /// waiting to be ended.
    fn kill_children(&mut self) {
        let groups = std::mem::take(&mut self.process_groups);
        self.retire_shared();
        for group in groups.iter().chain(&self.retired_groups) {
            // Safety: signals a process group gerdr started.
            unsafe { libc::kill(-group, libc::SIGTERM) };
        }
        self.retired_groups.extend(groups);
    }

    /// Ask the windows of the project shown to close, and give them
    /// `CLOSE_GRACE` to, say to save their work, running `event_loop`
    /// meanwhile.
    fn close_windows(&mut self, event_loop: &mut EventLoop<'static, Gerdr>) {
        let windows: Vec<Window> = self
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.panes)
            .filter(|p| !p.stray)
            .map(|p| p.window.clone())
            .collect();
        for window in &windows {
            window.toplevel().unwrap().send_close();
        }
        let _ = self.display_handle.flush_clients();
        let deadline = Instant::now() + CLOSE_GRACE;
        let open = |state: &Gerdr| {
            windows.iter().any(|w| {
                state
                    .workspaces
                    .iter()
                    .flat_map(|ws| &ws.tabs)
                    .any(|tab| tab.panes.iter().any(|p| p.window == *w))
            })
        };
        while open(self) && Instant::now() < deadline {
            if let Err(err) = event_loop.dispatch(Some(Duration::from_millis(50)), self) {
                return tracing::warn!("closing windows: {err}");
            }
        }
    }

    /// Start a new, empty project called `name` and show it.
    pub fn create_project(&mut self, name: &str) {
        let id = self.add_project(name);
        self.switch_project(&id);
    }

    /// A new, empty project called `name`; returns its id. A herdr session
    /// left behind by a forgotten project isn't reused.
    fn add_project(&mut self, name: &str) -> String {
        let name = name.trim();
        let mut taken: Vec<String> = self
            .projects
            .projects
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let id = loop {
            let refs: Vec<&str> = taken.iter().map(String::as_str).collect();
            let id = projects::slugify(name, &refs);
            let left_behind = herdr::session_dir(&projects::session_name(&id)).exists();
            if !left_behind || taken.len() > 1000 {
                break id;
            }
            taken.push(id);
        };
        let now = projects::now();
        self.projects.projects.push(projects::Project {
            id: id.clone(),
            name: name.to_owned(),
            session: projects::session_name(&id),
            created: now,
            last_used: now,
            state: RunState::Kept,
            workspaces: Vec::new(),
            before_start: String::new(),
            after_stop: String::new(),
        });
        self.projects.save();
        id
    }

    /// Start a new project called `name` and show it, opening `folder` in
    /// it as a workspace of `kind`, if given; without one, it starts empty.
    pub fn create_project_with(
        &mut self,
        name: &str,
        folder: Option<PathBuf>,
        kind: WorkspaceKind,
    ) {
        let id = self.add_project(name);
        self.enter_project(&id, false, Vec::new());
        match (folder, kind) {
            (Some(folder), WorkspaceKind::Project) => self.open_project(folder),
            (Some(folder), WorkspaceKind::Terminal) => self.open_terminal(folder),
            (_, WorkspaceKind::Cursor) => self.open_cursor(None),
            (None, _) => {}
        }
        // After the switch's notice, so it wins.
        self.show_notice(format!("Created {}", name.trim()));
    }

    /// Rename project `id` to `name` (trimmed; not to nothing); its id and
    /// session stay.
    pub fn rename_project(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(project) = self.projects.project_mut(id) else {
            return tracing::warn!("no project {id} to rename");
        };
        project.name = name.to_owned();
        self.projects.save();
        self.refresh_project_menu();
    }

    /// Keep project `id`'s start and stop commands, `before` and `after`.
    pub fn set_project_commands(&mut self, id: &str, before: String, after: String) {
        let Some(project) = self.projects.project_mut(id) else {
            return tracing::warn!("no project {id} to keep commands for");
        };
        if project.before_start == before && project.after_stop == after {
            return;
        }
        project.before_start = before;
        project.after_stop = after;
        let name = project.name.clone();
        self.projects.save();
        self.show_notice(format!("Saved {name}'s start and stop commands"));
    }

    /// Stop project `id`'s herdr session, off the event loop; not the shown
    /// one. `project_stopped` takes it from there.
    pub fn stop_project(&mut self, id: &str) {
        if self.project.as_deref() == Some(id) {
            return tracing::warn!("not stopping project {id}: it is the one shown");
        }
        let Some(session) = self.projects.project(id).map(|p| p.session.clone()) else {
            return tracing::warn!("no project {id} to stop");
        };
        if !self.stopping.insert(id.to_owned()) {
            return;
        }
        // Neither running nor stopped until it answers.
        self.running.remove(id);
        if self.stop_status.is_none() {
            let (tx, rx) = channel::channel::<(String, bool)>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((id, stopped)) = event {
                    state.project_stopped(&id, stopped);
                }
            });
            match inserted {
                Ok(_) => self.stop_status = Some(tx),
                Err(err) => {
                    self.stopping.remove(id);
                    return tracing::warn!("stopping project {id}: {err}");
                }
            }
        }
        let Some(tx) = self.stop_status.clone() else {
            self.stopping.remove(id);
            return;
        };
        let id = id.to_owned();
        std::thread::spawn(move || {
            let stopped = match herdr::stop_server(&session) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!("stopping project {id}'s herdr session: {err}");
                    false
                }
            };
            let _ = tx.send((id, stopped));
        });
    }

    /// Project `id`'s session was asked to stop, and `stopped` says whether
    /// it did: then it counts as stopped, and the windows held for it close.
    fn project_stopped(&mut self, id: &str, stopped: bool) {
        self.stopping.remove(id);
        let Some(name) = self.projects.project(id).map(|p| p.name.clone()) else {
            return;
        };
        if !stopped {
            // Whether it still runs is found out again.
            self.check_running();
            return self.show_notice(format!("Couldn't stop {name}"));
        }
        self.running.insert(id.to_owned(), false);
        if let Some(project) = self.projects.project_mut(id) {
            project.state = RunState::Stopped;
        }
        self.projects.save();
        self.run_hook(id, Hook::AfterStop);
        // They go from `held` once closed (see `remove_window`).
        for window in self.held.get(id).into_iter().flatten() {
            window.toplevel().unwrap().send_close();
        }
        self.refresh_project_menu();
        self.show_notice(format!("Stopped {name}"));
    }

    /// Show the project shown before this one, if it is still there.
    pub fn switch_to_last_project(&mut self) {
        let Some(id) = self.previous_project.clone() else {
            return;
        };
        if self.projects.project(&id).is_some() {
            self.switch_project(&id);
        }
    }

    /// Forget project `id`; its herdr session stays. Not the shown one.
    pub fn forget_project(&mut self, id: &str) {
        if self.project.as_deref() == Some(id) {
            return tracing::warn!("not forgetting project {id}: it is the one shown");
        }
        self.projects.projects.retain(|p| p.id != id);
        if self.projects.last_active.as_deref() == Some(id) {
            self.projects.last_active = None;
        }
        if self.previous_project.as_deref() == Some(id) {
            self.previous_project = None;
        }
        self.projects.save();
        self.refresh_project_menu();
    }

    /// The projects for the welcome screen, most recently used first, by id;
    /// `running` is left `None`.
    pub fn project_rows(&self) -> Vec<(String, chrome::ProjectRow)> {
        let now = projects::now();
        let home = home_dir();
        let mut projects: Vec<_> = self.projects.projects.iter().collect();
        projects.sort_by_key(|p| std::cmp::Reverse(p.last_used));
        projects
            .into_iter()
            .map(|p| {
                let dirs: Vec<&Path> = p.workspaces.iter().map(|w| w.dir.as_path()).collect();
                let row = chrome::ProjectRow {
                    name: p.name.clone(),
                    detail: project_detail(&dirs, home.as_deref()),
                    ago: projects::ago(now, p.last_used),
                    running: None,
                    current: self.project.as_deref() == Some(p.id.as_str()),
                };
                (p.id.clone(), row)
            })
            .collect()
    }

    /// End gerdr, stopping the project's session if `stop`. It counts as
    /// stopped only once it is.
    pub fn quit_app(&mut self, stop: bool) {
        self.take_web_urls(web_pages::read_now(self.web_panes()));
        self.record_tabs();
        let stopped = stop && self.stop_session();
        if stopped
            && let Some(project) = self
                .project
                .as_deref()
                .and_then(|id| self.projects.project(id))
        {
            let line = Hook::AfterStop.line(project);
            if !line.is_empty() {
                let dir = hooks::project_dir(project);
                hooks::spawn_detached(&project.id, Hook::AfterStop, line, &dir);
            }
        }
        if let Some(project) = self
            .project
            .clone()
            .and_then(|id| self.projects.project_mut(&id))
        {
            project.state = if stopped {
                RunState::Stopped
            } else {
                RunState::Kept
            };
        }
        self.projects.save();
        self.close_cursor_socket();
        self.loop_signal.stop();
    }

    /// Stop the shown project's herdr session, which saves its layout
    /// first, even if gerdr lost touch with it; whether it is stopped now.
    fn stop_session(&mut self) -> bool {
        let Some(session) = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map(|p| p.session.clone())
        else {
            return false;
        };
        let stopped = match self.herdr.as_mut() {
            Some(herdr) => herdr.stop(),
            None if herdr::is_running(&session) => {
                herdr::Herdr::on_socket(herdr::socket_path(&session)).stop()
            }
            // Nothing runs.
            None => return true,
        };
        match stopped {
            Ok(()) => true,
            // It may go before it answers.
            Err(_) if !herdr::is_running(&session) => true,
            Err(err) => {
                tracing::warn!("stopping the project's herdr session: {err}");
                false
            }
        }
    }

    /// Show what gerdr was started for: project `name` (by id or name,
    /// made if there is none), opening `folders` in it; else, with folders,
    /// the last project shown (or a new one named after the first folder);
    /// else the last project shown if it was left running, or the welcome
    /// screen. Without herdr, projects can't run: the welcome screen says
    /// so.
    fn start_project(&mut self, name: Option<&str>, folders: Vec<PathBuf>) {
        if !herdr::is_installed() {
            tracing::warn!("herdr isn't installed: projects can't run");
            return self.open_welcome(false);
        }
        let last = self
            .projects
            .last_active
            .clone()
            .filter(|id| self.projects.project(id).is_some());
        let id = match name {
            Some(name) => {
                let found = self
                    .projects
                    .projects
                    .iter()
                    .find(|p| p.id == name || p.name == name);
                match found {
                    Some(project) => project.id.clone(),
                    None => self.add_project(name),
                }
            }
            None if !folders.is_empty() => last.unwrap_or_else(|| {
                let dir = folders[0].canonicalize().unwrap_or(folders[0].clone());
                let name = dir
                    .file_name()
                    .map_or("project".into(), |n| n.to_string_lossy().into_owned());
                self.add_project(&name)
            }),
            None => match last {
                Some(id)
                    if self
                        .projects
                        .project(&id)
                        .is_some_and(|p| p.state == RunState::Kept) =>
                {
                    id
                }
                _ => return self.open_welcome(false),
            },
        };
        self.enter_project(&id, folders.is_empty(), folders);
    }

    /// Run project `id`'s `hook` commands off the event loop, if it has
    /// any (start commands not twice at once); `hook_done` takes it from
    /// there. Whether they started.
    fn run_hook(&mut self, id: &str, hook: Hook) -> bool {
        let Some(project) = self.projects.project(id) else {
            return false;
        };
        let line = hook.line(project).to_owned();
        if line.is_empty() || (hook == Hook::BeforeStart && self.starting.contains(id)) {
            return false;
        }
        let dir = hooks::project_dir(project);
        if self.hook_status.is_none() {
            let (tx, rx) = channel::channel::<hooks::Done>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((id, hook, result)) = event {
                    state.hook_done(&id, hook, result);
                }
            });
            match inserted {
                Ok(_) => self.hook_status = Some(tx),
                Err(err) => {
                    tracing::warn!("running project {id}'s {}: {err}", hook.label());
                    return false;
                }
            }
        }
        let Some(tx) = self.hook_status.clone() else {
            return false;
        };
        if hook == Hook::BeforeStart {
            self.starting.insert(id.to_owned());
        }
        hooks::spawn(id.to_owned(), hook, line, dir, tx);
        true
    }

    /// Run project `id`'s start commands again, whether its session runs or
    /// not.
    pub fn run_start_commands(&mut self, id: &str) {
        let Some(project) = self.projects.project(id) else {
            return;
        };
        let name = project.name.clone();
        if Hook::BeforeStart.line(project).is_empty() {
            self.show_notice(format!("{name} has no start commands"));
        } else if self.starting.contains(id) {
            self.show_notice(format!("{name}'s start commands are running"));
        } else if self.run_hook(id, Hook::BeforeStart) {
            self.show_notice(format!("Running {name}'s start commands…"));
        }
    }

    /// Project `id`'s `hook` commands ended as `result` says. Once its start
    /// commands are done, the project waiting for them is shown.
    fn hook_done(&mut self, id: &str, hook: Hook, result: Result<(), String>) {
        let mut entered = false;
        if hook == Hook::BeforeStart {
            self.starting.remove(id);
            if self.pending_enter.as_ref().is_some_and(|p| p.id == id)
                && let Some(PendingEnter { id, pick, folders }) = self.pending_enter.take()
            {
                self.enter_project_now(&id, pick, folders);
                entered = true;
            }
        }
        let Some(name) = self.projects.project(id).map(|p| p.name.clone()) else {
            return;
        };
        // After the switch's notice, so it wins.
        match result {
            Err(err) => self.show_notice(format!("{name}'s {} failed: {err}", hook.label())),
            Ok(()) if hook == Hook::BeforeStart && !entered => {
                self.show_notice(format!("{name}'s start commands are done"));
            }
            Ok(()) => {}
        }
    }

    /// What the sidebar and tab strip should show right now. Taken once
    /// per frame: what moves with time (the tooltip's wait, the toasts'
    /// stay) is moved on here first.
    fn chrome_view(&mut self) -> ChromeView {
        let now = Instant::now();
        self.tooltip.tick(now);
        self.expire_toasts(now);
        let tab_strip = self.tab_hover_tick(now);
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        let agents = self.agent_views();
        let sidebar_state = self.sidebar_state(now, &workspaces, &agents);
        let ws = self.workspace();
        let active = ws.map(|_| self.active_workspace);
        let area = self.content_rect();
        let tab = self.tab();
        let focused_pane = tab.and_then(|tab| {
            let focused = tab.focused_id()?;
            let (id, rect) = tab
                .shown_rects(area)
                .into_iter()
                .find(|(id, _)| *id == focused)?;
            Some(if tab.has_bar(id) {
                chrome::pane_bar_rects_shown(rect, self.pane_bar_shown(tab, id)).1
            } else {
                rect
            })
        });
        let placeholders = tab.map_or(Vec::new(), |tab| {
            tab.expected_rects(area)
                .into_iter()
                .map(|(expected, rect)| PlaceholderView {
                    rect,
                    label: expected.label(),
                    since: expected.started,
                    timed_out: expected.timed_out,
                    closed: expected.closed,
                })
                .collect()
        });
        let project = self
            .project
            .as_deref()
            .and_then(|id| self.projects.project(id))
            .map(|p| p.name.clone());
        ChromeView {
            now,
            pointer: self.pointer,
            help_pinned: self.help_pinned,
            sidebar: chrome::SidebarView {
                workspaces,
                agents,
                active,
                project,
                header_lit: self.header_hover || self.project_menu.is_some(),
                state: sidebar_state,
            },
            pulse: anim::pulse(self.start_time, now),
            tab_strip,
            focused_pane,
            pane_drop: self.pane_drop_rect(),
            placeholders,
            context_menu: self.context_menu.as_ref().map(ContextMenu::view),
            tooltip: self.tooltip.view(),
            switcher: self.switcher_view(),
            prefix: (self.prefix_armed || self.help_pinned).then(|| self.keymap.help()),
            welcome: self.welcome_view(),
            quit: self.quit_view(),
            project_menu: self.project_menu_view(),
            worktrees: self.worktrees_view(),
            repos: self.repos_view(),
            toasts: self.toast_views(),
            pane_bars: self.pane_bar_views(),
            theme: self.theme(),
            density: self.density,
            sidebar_width: self.sidebar_width,
            size: self.output_size(),
            scale: self.scale(),
        }
    }
}

/// Move the item at `active` to place `to`, keeping `active` on it.
fn move_item<T>(items: &mut Vec<T>, active: &mut usize, to: usize) {
    if *active != to && *active < items.len() && to < items.len() {
        let item = items.remove(*active);
        items.insert(to, item);
        *active = to;
    }
}

/// Remove the items failing `keep`, keeping `active` on the same item (or on
/// the one that took its place). Returns whether anything was removed.
fn retain_active<T>(items: &mut Vec<T>, active: &mut usize, keep: impl Fn(&T) -> bool) -> bool {
    let before = items.len();
    let mut new_active = *active;
    let mut idx = 0;
    items.retain(|item| {
        let kept = keep(item);
        if !kept && idx < *active {
            new_active -= 1;
        }
        idx += 1;
        kept
    });
    *active = new_active.min(items.len().saturating_sub(1));
    items.len() != before
}

/// Where the theme choice (system, dark or light) is kept between runs.
fn theme_file() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/theme")
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Puts the user's bin folders (`~/.local/bin`, `~/.cargo/bin`) on `PATH`
/// when they are missing: started from an app launcher, gerdr gets the
/// session's bare `PATH`, where herdr and Zed are not found, and so would
/// every app it starts.
fn extend_path() {
    let Some(home) = home_dir() else { return };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let missing: Vec<PathBuf> = [".local/bin", ".cargo/bin"]
        .map(|d| home.join(d))
        .into_iter()
        .filter(|d| !dirs.contains(d))
        .collect();
    if missing.is_empty() {
        return;
    }
    dirs.splice(0..0, missing);
    if let Ok(path) = std::env::join_paths(dirs) {
        // SAFETY: called at the start of `main`, before any other thread.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// gerdr's state folder, for logs: `$XDG_STATE_HOME/gerdr`, by default
/// `~/.local/state/gerdr`.
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home_dir().unwrap_or_default().join(".local/state"))
        .join("gerdr")
}

pub fn set_preferred_scale(surface: &WlSurface, scale: f64) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| with_fractional_scale(states, |fs| fs.set_preferred_scale(scale)),
        |_, _, _| true,
    );
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Run by Cursor's hooks, many times a chat: in and out at once.
    if std::env::args().nth(1).as_deref() == Some(cursor_agents::HOOK_COMMAND) {
        cursor_agents::run_hook();
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    extend_path();
    // The portal can be slow to answer: asked now, alongside the setup.
    let system_theme = system_theme::read();

    // `--project <name>` picks the project; every other argument is a
    // folder to open as a workspace in it.
    let (project_arg, folders) = parse_args(std::env::args_os().skip(1));
    // Own data dir so Zed runs its own instance here instead of handing the
    // windows to a Zed already running on the host.
    let zed_data = launch::zed_data_dir(&home_dir().unwrap_or_default());
    let _ = std::fs::create_dir_all(&zed_data);

    let mut event_loop: EventLoop<'static, Gerdr> = EventLoop::try_new()?;
    let display: Display<Gerdr> = Display::new()?;
    let dh = display.handle();

    // Host window.
    let attrs = WindowAttributes::default()
        .with_title("gerdr")
        .with_name("gerdr", "gerdr")
        .with_inner_size(smithay::reexports::winit::dpi::LogicalSize::new(
            1400.0, 900.0,
        ));
    let (mut backend, winit_loop) = winit::init_from_attributes::<GlesRenderer>(attrs)?;

    // Output that mirrors the host window.
    let output = Output::new(
        "gerdr".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "gerdr".into(),
            model: "nested".into(),
        },
    );
    let _output_global = output.create_global::<Gerdr>(&dh);
    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(backend.window().scale_factor())),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    // linux-dmabuf with v4 feedback so EGL/Vulkan clients (Zed, Alacritty) render on the GPU.
    let mut dmabuf_state = DmabufState::new();
    let formats = backend.renderer().dmabuf_formats();
    let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .ok()
        .and_then(|dev| dev.try_get_render_node().ok().flatten());
    let feedback = render_node.and_then(|node| {
        DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .ok()
    });
    let _dmabuf_global = match feedback {
        Some(fb) => dmabuf_state.create_global_with_default_feedback::<Gerdr>(&dh, &fb),
        None => dmabuf_state.create_global::<Gerdr>(&dh, formats),
    };

    let mut seat_state = SeatState::new();
    let mut seat: Seat<Gerdr> = seat_state.new_wl_seat(&dh, "gerdr");
    seat.add_keyboard(Default::default(), REPEAT_DELAY as i32, REPEAT_RATE as i32)?;
    seat.add_pointer();

    let mut space = Space::default();
    space.map_output(&output, (0, 0));

    // Data control lets clipboard tools such as wl-copy set the selection
    // without a window of their own; without it they open one to get the
    // keyboard focus, and gerdr would tile it as a pane.
    let primary_selection_state = PrimarySelectionState::new::<Gerdr>(&dh);
    let data_control_state =
        DataControlState::new::<Gerdr, _>(&dh, Some(&primary_selection_state), |_| true);
    let ext_data_control_state =
        ExtDataControlState::new::<Gerdr, _>(&dh, Some(&primary_selection_state), |_| true);

    // Wayland socket for the children. A fixed name, so shells that outlive
    // gerdr in herdr still reach the next one.
    let socket = ListeningSocketSource::with_name("gerdr-0")
        .or_else(|_| ListeningSocketSource::new_auto())?;
    let socket_name = socket.socket_name().to_os_string();

    let (keymap, apps, ui) = config::load();
    let saved_theme = std::fs::read_to_string(theme_file())
        .ok()
        .and_then(|s| ThemeMode::from_name(&s));
    let mut state = Gerdr {
        start_time: Instant::now(),
        display_handle: dh.clone(),
        loop_signal: event_loop.get_signal(),
        loop_handle: event_loop.handle(),
        backend,
        output: output.clone(),
        space,
        popups: PopupManager::default(),
        unplaced: Vec::new(),
        workspaces: Vec::new(),
        active_workspace: 0,
        chrome: Chrome::new(),
        theme_mode: ui.theme.or(saved_theme).unwrap_or_default(),
        system_dark: true,
        density: ui.density,
        sidebar_width: SIDEBAR_WIDTH,
        next_pane: 0,
        drag: None,
        reorder: None,
        pane_drop: None,
        chrome_cursor: CursorIcon::Default,
        suppressed_keys: Vec::new(),
        prefix_armed: false,
        help_pinned: false,
        keymap,
        pointer: (0.0, 0.0).into(),
        last_press: None,
        rename: None,
        tab_hover: anim::Hover::default(),
        sidebar_hover: anim::Hover::default(),
        frecency: Frecency::load(),
        bar_states: pane_control::BarStates::default(),
        pending_place: HashMap::new(),
        socket_name: socket_name.clone(),
        zed_data,
        children: Vec::new(),
        herdr: None,
        agents: Vec::new(),
        herdr_tabs: HashMap::new(),
        git: HashMap::new(),
        git_pending: false,
        git_again: false,
        git_read: None,
        worktrees: worktrees::Worktrees::default(),
        repos: repos::Repos::default(),
        web_pending: false,
        notifier: None,
        blocked_since: HashMap::new(),
        agent_statuses: HashMap::new(),
        cursor: Default::default(),
        cursor_chats: Vec::new(),
        cursor_poke: None,
        cursor_socket: None,
        cursor_opened: None,
        host_focused: true,
        process_groups: Vec::new(),
        picker: None,
        context_menu: None,
        tooltip: TooltipState::default(),
        switcher: None,
        recent_dirs: Vec::new(),
        zoxide: None,
        key_repeat: None,
        apps,
        installed_apps: Vec::new(),
        installed_apps_tx: None,
        projects: projects::Store::default(),
        project: None,
        welcome: None,
        running: HashMap::new(),
        running_status: None,
        stop_status: None,
        hook_status: None,
        starting: HashSet::new(),
        pending_enter: None,
        paste: None,
        copied: None,
        field_clicks: None,
        field_drag: false,
        stopping: HashSet::new(),
        project_menu: None,
        header_hover: false,
        previous_project: None,
        toasts: VecDeque::new(),
        quit: None,
        herdr_sources: Vec::new(),
        herdr_poke: None,
        idle_since: HashMap::new(),
        retired_groups: Vec::new(),
        shared_generation: 0,
        tabs_changed: None,
        order_pending: HashSet::new(),
        pane_leaders: HashMap::new(),
        last_commands: HashMap::new(),
        pane_jobs: HashMap::new(),
        pane_job_tx: None,
        pane_bar_hover: None,
        closed_workspaces: HashMap::new(),
        gone_since: HashMap::new(),
        closing_records: HashSet::new(),
        restored_at: None,
        replay_pending: HashSet::new(),
        held: HashMap::new(),
        closing: Vec::new(),
        zed_groups: Vec::new(),
        old_zed: Vec::new(),
        old_zed_until: None,
        repo_pages: HashMap::new(),
        compositor_state: CompositorState::new::<Gerdr>(&dh),
        xdg_shell_state: XdgShellState::new::<Gerdr>(&dh),
        xdg_decoration_state: XdgDecorationState::new::<Gerdr>(&dh),
        shm_state: ShmState::new::<Gerdr>(&dh, vec![]),
        dmabuf_state,
        output_manager_state: OutputManagerState::new_with_xdg_output::<Gerdr>(&dh),
        seat_state,
        data_device_state: DataDeviceState::new::<Gerdr>(&dh),
        primary_selection_state,
        data_control_state,
        ext_data_control_state,
        host_clipboard: clipboard::HostClipboard::init(&event_loop.handle()),
        viewporter_state: ViewporterState::new::<Gerdr>(&dh),
        fractional_scale_state: FractionalScaleManagerState::new::<Gerdr>(&dh),
        cursor_shape_state: CursorShapeManagerState::new::<Gerdr>(&dh),
        seat,
    };

    event_loop
        .handle()
        .insert_source(socket, |stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("client insert failed: {err}");
            }
        })?;
    event_loop.handle().insert_source(
        Generic::new(
            display,
            Interest::READ,
            smithay::reexports::calloop::Mode::Level,
        ),
        |_, display, state| {
            // Safety: the display is never dropped while the loop runs.
            unsafe { display.get_mut().dispatch_clients(state).unwrap() };
            Ok(PostAction::Continue)
        },
    )?;

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    event_loop
        .handle()
        .insert_source(winit_loop, move |event, _, state| match event {
            WinitEvent::Resized { size, scale_factor } => {
                state.output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(scale_factor)),
                    None,
                );
                state.arrange();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let start = Instant::now();
                let size = state.backend.window_size();
                let view = state.chrome_view();
                let clear_color = state.theme().clear_color();
                {
                    let (renderer, mut framebuffer) = state.backend.bind().unwrap();
                    let chrome = state.chrome.elements(renderer, view);
                    let _ = smithay::desktop::space::render_output::<
                        _,
                        MemoryRenderBufferRenderElement<GlesRenderer>,
                        _,
                        _,
                    >(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&state.space],
                        &chrome,
                        &mut damage_tracker,
                        clear_color,
                    );
                }
                let _ = state.backend.submit(Some(&[Rectangle::from_size(size)]));
                let took = start.elapsed();
                if took > SLOW_FRAME {
                    tracing::debug!("frame took {took:?}");
                }

                let elapsed = state.start_time.elapsed();
                for window in state.space.elements() {
                    window.send_frame(&output, elapsed, Some(Duration::ZERO), |_, _| {
                        Some(output.clone())
                    });
                }
                state.space.refresh();
                state.popups.cleanup();
                let _ = state.display_handle.flush_clients();
                state.backend.window().request_redraw();
            }
            WinitEvent::Focus(focused) => {
                state.host_focused = focused;
                // The key's release goes elsewhere now.
                if !focused {
                    state.stop_key_repeat();
                    state.release_held_keys();
                }
            }
            WinitEvent::CloseRequested => state.request_quit(),
        })?;

    // Notice children that exit before showing a window, and the folder picker closing.
    event_loop
        .handle()
        .insert_source(Timer::from_duration(REAP_INTERVAL), |_, _, state| {
            state.reap_children();
            TimeoutAction::ToDuration(REAP_INTERVAL)
        })?;

    // The first run turns the session gerdr used before projects into one.
    let (store, loaded) = projects::Store::load();
    state.projects = store;
    if loaded == projects::Loaded::Migrated {
        state.projects.save();
    }
    // Before the first terminals, which take the look's colours with them,
    // when the look follows the system; any other look can do without it.
    let system_theme = if state.theme_mode == ThemeMode::System {
        if let Some(dark) = system_theme.wait() {
            state.system_dark = dark;
        }
        None
    } else {
        Some(system_theme)
    };
    state.watch_git();
    state.watch_records();
    state.watch_cursor_agents();
    state.watch_web_pages();
    state.connect_notifier();
    state.watch_system_theme(system_theme);
    state.watch_zoxide();
    state.watch_installed_apps();
    state.start_project(project_arg.as_deref(), folders);

    // Send what the loop queued for the clients (a paste's request to the
    // app that copied, say) right away: drawing, which flushes too, stops
    // while the host doesn't show gerdr.
    event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    })?;

    state.frecency.save();
    state.close_windows(&mut event_loop);
    state.kill_children();
    if let Some((mut picker, _)) = state.picker.take() {
        let _ = picker.kill();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn herdr_leaf(id: &str) -> SavedNode {
        SavedNode::Leaf(SavedLeaf::Herdr { pane_id: id.into() })
    }

    fn split(first: SavedNode, second: SavedNode) -> SavedNode {
        SavedNode::Split {
            dir: Dir::Down,
            ratio: 0.7,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn app(command: &str) -> SavedNode {
        SavedNode::Leaf(SavedLeaf::App {
            command: command.into(),
        })
    }

    fn record(pane_id: &str, tab: usize, index: usize, command: Option<&str>) -> PaneRecord {
        PaneRecord {
            pane_id: pane_id.into(),
            tab,
            index,
            command: command.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn restores_terminals_zed_and_apps_in_place() {
        let saved = split(
            SavedNode::Leaf(SavedLeaf::Zed),
            split(herdr_leaf("w1:p1"), app("firefox")),
        );
        let ids = HashMap::from([("w1:p1".to_owned(), 7)]);
        let mut next = 10;
        let restored = restore_tab(&saved, &ids, &mut next);
        assert_eq!(restored.zed, [11]);
        assert_eq!(restored.apps, [(12, "firefox".to_owned())]);
        assert_eq!(next, 12);
        assert_eq!(restored.layout.unwrap().leaves(), [11, 7, 12]);
    }

    #[test]
    fn a_terminal_that_is_gone_leaves_its_space_to_its_sibling() {
        let saved = split(herdr_leaf("w1:p1"), herdr_leaf("w1:p2"));
        let ids = HashMap::from([("w1:p2".to_owned(), 3)]);
        let mut next = 0;
        let restored = restore_tab(&saved, &ids, &mut next);
        assert_eq!(restored.layout.unwrap().leaves(), [3]);
        assert_eq!(next, 0);
    }

    #[test]
    fn a_saved_tab_matches_only_the_same_panes() {
        let saved = split(herdr_leaf("w1:p1"), split(herdr_leaf("w1:p2"), app("x")));
        assert!(same_panes(&saved, &HashSet::from(["w1:p2", "w1:p1"])));
        assert!(!same_panes(&saved, &HashSet::from(["w1:p1"])));
        assert!(!same_panes(
            &saved,
            &HashSet::from(["w1:p1", "w1:p2", "w1:p3"])
        ));
        assert_eq!(saved_apps(&saved), ["x"]);
    }

    #[test]
    fn leaves_are_named_by_what_fills_them() {
        let pane = herdr::PaneInfo {
            pane_id: "w1:p4".into(),
            terminal_id: "t".into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            cwd: None,
        };
        assert_eq!(
            saved_leaf(Some(&pane), None, false),
            Some(SavedLeaf::Herdr {
                pane_id: "w1:p4".into()
            })
        );
        assert_eq!(
            saved_leaf(None, Some("zathura"), false),
            Some(SavedLeaf::App {
                command: "zathura".into()
            })
        );
        assert_eq!(saved_leaf(None, None, true), Some(SavedLeaf::Zed));
        assert_eq!(saved_leaf(None, None, false), None);
    }

    #[test]
    fn pane_records_are_found_by_id_then_by_place() {
        let records = [
            record("w1:p1", 0, 0, Some("make run")),
            record("w1:p2", 0, 1, Some("celery worker")),
        ];
        let live = HashSet::from(["w1:p9", "w1:p2"]);
        let found = |id, tab, index| {
            find_pane_record(&records, id, tab, index, &live).map(|r| r.pane_id.as_str())
        };
        assert_eq!(found("w1:p2", 5, 5), Some("w1:p2"));
        // Its id is gone: the record at its place, unless a live pane owns it.
        assert_eq!(found("w1:p9", 0, 0), Some("w1:p1"));
        assert_eq!(found("w1:p9", 0, 1), None);
        assert_eq!(found("w1:p9", 1, 0), None);
    }

    fn captured(command: &str) -> projects::Captured {
        projects::Captured {
            command: command.into(),
            cwd: Some("/src/app".into()),
            env: vec![("MISE_NODE_VERSION".into(), "20".into())],
        }
    }

    #[test]
    fn commands_are_kept_until_forgotten() {
        let mut records = Vec::new();
        assert!(!note_pane(&mut records, "w1:p1", (0, 0), Note::Clear));
        assert!(!note_pane(&mut records, "w1:p1", (0, 0), Note::Keep));
        assert!(records.is_empty());

        let busy = Note::Set(captured("npm run dev"));
        assert!(note_pane(&mut records, "w1:p1", (1, 2), busy.clone()));
        assert_eq!(records[0].command.as_deref(), Some("npm run dev"));
        assert_eq!(records[0].cwd, Some("/src/app".into()));
        assert_eq!((records[0].tab, records[0].index), (1, 2));
        assert!(!note_pane(&mut records, "w1:p1", (1, 2), busy));

        // Filled only if empty.
        let fill = Note::Fill(captured("gunicorn: master"));
        assert!(!note_pane(&mut records, "w1:p1", (1, 2), fill.clone()));
        assert_eq!(records[0].command.as_deref(), Some("npm run dev"));

        // Idle for a moment: kept, though its place is followed.
        assert!(note_pane(&mut records, "w1:p1", (0, 0), Note::Keep));
        assert_eq!(records[0].command.as_deref(), Some("npm run dev"));

        records[0].skip.push("npm run dev".into());
        assert!(note_pane(&mut records, "w1:p1", (0, 0), Note::Clear));
        assert_eq!(records[0].command, None);
        assert_eq!(records[0].cwd, None);
        assert!(records[0].env.is_empty());
        assert_eq!(records[0].skip, ["npm run dev"]);

        assert!(note_pane(&mut records, "w1:p1", (0, 0), fill));
        assert_eq!(records[0].command.as_deref(), Some("gunicorn: master"));
        assert!(note_pane(
            &mut records,
            "w1:p2",
            (0, 1),
            Note::Fill(captured("x"))
        ));
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn a_command_is_noted_once_per_process() {
        use projects::Foreground::{Busy, Idle, Untracked};
        let busy = |pid, command: &str| {
            Some(Busy {
                pid,
                captured: captured(command),
            })
        };
        // herdr couldn't tell: nothing changes, the leader is kept.
        assert_eq!(leader_note(None, Some(7), true), (Note::Keep, Some(7)));
        // Idle: forgotten only once idle for long.
        assert_eq!(leader_note(Some(Idle), Some(7), false), (Note::Keep, None));
        assert_eq!(leader_note(Some(Idle), Some(7), true), (Note::Clear, None));
        // A new process: noted.
        assert_eq!(
            leader_note(busy(8, "gunicorn app.wsgi"), Some(7), false),
            (Note::Set(captured("gunicorn app.wsgi")), Some(8))
        );
        // The same one again: only if nothing was noted.
        assert_eq!(
            leader_note(busy(8, "gunicorn app.wsgi"), Some(8), false),
            (Note::Fill(captured("gunicorn app.wsgi")), Some(8))
        );
        // It rewrote its command line: what was noted of it stays.
        let rewritten = Some(Untracked {
            pid: 8,
            denied: false,
        });
        assert_eq!(
            leader_note(rewritten.clone(), Some(8), false),
            (Note::Keep, Some(8))
        );
        // Unreadable from the start: whatever was noted stays.
        assert_eq!(
            leader_note(rewritten, Some(7), false),
            (Note::Keep, Some(8))
        );
        // Another program, never run again: what ran before is gone.
        let editor = Some(Untracked {
            pid: 9,
            denied: true,
        });
        assert_eq!(
            leader_note(editor.clone(), Some(8), false),
            (Note::Clear, Some(9))
        );
        assert_eq!(leader_note(editor, Some(9), false), (Note::Keep, Some(9)));
    }

    #[test]
    fn project_detail_names_the_common_folder() {
        let home = Path::new("/home/me");
        let detail = |dirs: &[&str]| {
            let dirs: Vec<&Path> = dirs.iter().map(Path::new).collect();
            project_detail(&dirs, Some(home))
        };
        assert_eq!(detail(&[]), "empty");
        assert_eq!(detail(&["/home/me/Work/aft"]), "1 repo · ~/Work/aft");
        let aft = [
            "/home/me/Work/aft/api",
            "/home/me/Work/aft/app",
            "/home/me/Work/aft/api/sub",
        ];
        assert_eq!(detail(&aft), "3 repos · ~/Work/aft");
        assert_eq!(detail(&["/home/me/a", "/home/me/b"]), "2 repos · ~");
        assert_eq!(detail(&["/srv/a", "/opt/b"]), "2 repos");
        assert_eq!(detail(&["/srv/a", "/srv/b"]), "2 repos · /srv");
    }

    #[test]
    fn args_name_a_project_and_folders() {
        let args = |list: &[&str]| parse_args(list.iter().map(OsString::from));
        assert_eq!(args(&[]), (None, vec![]));
        assert_eq!(
            args(&["--project", "aft", "/a", "b"]),
            (
                Some("aft".into()),
                vec![PathBuf::from("/a"), PathBuf::from("b")]
            )
        );
        assert_eq!(
            args(&["/a", "--project=My work"]),
            (Some("My work".into()), vec![PathBuf::from("/a")])
        );
    }
}
