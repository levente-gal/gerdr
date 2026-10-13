//! gerdr: a nested Wayland compositor that shows apps inside a single host
//! window. A sidebar lists workspaces (project folders); each workspace has
//! tabs. A workspace's first tab tiles Zed (top 70%) over a terminal (bottom
//! 30%); further tabs start with one terminal. Like herdr, any pane can be
//! split right or down; the new half gets a terminal in the workspace folder.
//! Terminals live in a herdr session of gerdr's own (see `herdr`), so they
//! keep running when gerdr quits and come back when it starts again. Any
//! other Wayland app can be launched into a split too (see `launch`).

mod chrome;
mod clipboard;
mod config;
mod desktop;
mod fuzzy;
mod git;
mod handlers;
mod herdr;
mod icons;
mod input;
mod keys;
mod launch;
mod layout;
mod notify;
mod switcher;

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use chrome::{
    AgentView, Chrome, ChromeView, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH,
    TAB_STRIP_HEIGHT, TabView, Theme, WorkspaceView,
};
use layout::{Dir, Node, Side};
use serde_json::json;
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
            backend::{ClientData, ClientId, DisconnectReason},
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
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::xdg::{XdgShellState, XdgToplevelSurfaceData, decoration::XdgDecorationState},
        shm::ShmState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
};

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
/// How often the Git status of the workspace folders is read again.
const GIT_INTERVAL: Duration = Duration::from_secs(3);
/// How long a window may still show up after the process we started exited
/// (the `zed` CLI hands the window off to an already running editor and quits).
const HANDOFF_GRACE: Duration = Duration::from_secs(10);
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
}

/// A process we started for a tab whose window hasn't shown up yet.
pub struct Expected {
    /// `None` while queued: Zed windows are started one at a time.
    pub pid: Option<u32>,
    /// Lowercase substring of the app_id, for windows not opened by `pid`
    /// or a process it started (the `zed` CLI hands off to a separate editor
    /// process). Empty for an app launched by command: only its own
    /// processes' windows count.
    pub app_id: &'static str,
    /// The layout leaf kept free for the window.
    pub pane: u32,
    /// Whether the window takes keyboard focus when it shows up.
    pub focus: bool,
    /// When `pid` exited; the expectation is dropped `HANDOFF_GRACE` later.
    pub exited: Option<Instant>,
    /// The herdr pane the window will show, for a terminal.
    pub herdr: Option<herdr::PaneInfo>,
}

impl Expected {
    fn new(pid: Option<u32>, app_id: &'static str, pane: u32, focus: bool) -> Self {
        Self {
            pid,
            app_id,
            pane,
            focus,
            exited: None,
            herdr: None,
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

/// What a workspace opens with: Zed over a terminal, or just a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    Project,
    Terminal,
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
        }
    }
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
    /// Open the switcher, to search for what to switch to.
    Switcher,
    /// Open the run prompt, for an app to launch into a split.
    Run,
}

/// A divider that can be dragged with the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drag {
    /// The sidebar's right edge.
    Sidebar,
    /// The gap between two panes of the shown tab: the split at `path`.
    Split { path: Vec<bool>, dir: Dir },
}

/// The active workspace's row or the active tab, held with the left button
/// to drag it to another place.
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
    pub theme: Theme,
    pub sidebar_width: i32,
    /// The next pane id to hand out.
    pub next_pane: u32,
    /// The divider being dragged, while the left button holds it.
    pub drag: Option<Drag>,
    /// The workspace row or tab held, while the left button holds it.
    pub reorder: Option<Reorder>,
    /// The cursor shape gerdr last set over its chrome.
    pub chrome_cursor: CursorIcon,
    /// Keys whose press was a gerdr shortcut; their release is swallowed too.
    pub suppressed_keys: Vec<Keycode>,
    /// The prefix was pressed; the next key is a gerdr command.
    pub prefix_armed: bool,
    /// The prefix and the keys after it, from the config file.
    pub keymap: keys::Keymap,

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
    /// A round of reading the Git status is still running.
    pub git_pending: bool,
    /// gerdr's connection to the host's notification daemon; `None` without
    /// a session bus.
    pub notifier: Option<notify::Notifier>,
    /// gerdr's own window has the host's keyboard focus.
    pub host_focused: bool,
    /// Process groups we started, all terminated when gerdr exits.
    pub process_groups: Vec<i32>,
    /// The folder picker, while it is open, and the kind of workspace it opens.
    pub picker: Option<(Child, WorkspaceKind)>,
    /// The menu under the terminal button's chevron is open.
    pub terminal_menu: bool,
    /// The menu item under the pointer.
    pub menu_hover: Option<usize>,
    /// The switcher, while it is open.
    pub switcher: Option<switcher::Switcher>,
    /// The folders zoxide knows, the highest ranked first.
    pub recent_dirs: Vec<PathBuf>,
    /// Where a read of zoxide's folders sends them; `None` until set up.
    pub zoxide: Option<channel::Sender<Vec<PathBuf>>>,
    /// The key held in the switcher, and the timer repeating it.
    pub switcher_repeat: Option<(Keycode, RegistrationToken)>,
    /// The apps named in the config file, to launch into a split.
    pub apps: Vec<launch::App>,
    /// The apps installed on the system, offered in the run prompt.
    pub installed_apps: Vec<launch::App>,
    /// Where a read of the installed apps sends them; `None` until set up.
    pub installed_apps_tx: Option<channel::Sender<Vec<launch::App>>>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Gerdr>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
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

    /// Where the panes go: right of the sidebar, below the tab strip.
    fn content_rect(&self) -> Rectangle<i32, Logical> {
        let size = self.output_size();
        let (x, y) = (self.sidebar_width, TAB_STRIP_HEIGHT);
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
                let agents = self.agent_views();
                let workspaces: Vec<_> = self
                    .workspace_views()
                    .into_iter()
                    .map(|(ws, _)| ws)
                    .collect();
                let layout = chrome::sidebar_layout(&workspaces, &agents, self.sidebar_width);
                let to = chrome::workspace_slot(&layout.blocks, self.active_workspace, pos.y);
                move_item(&mut self.workspaces, &mut self.active_workspace, to);
            }
            Reorderable::Tab => {
                let width = self.output_size().w - self.sidebar_width;
                let Some(ws) = self.workspaces.get_mut(self.active_workspace) else {
                    return;
                };
                let to = chrome::tab_slot(ws.tabs.len(), width, pos.x - self.sidebar_width as f64);
                move_item(&mut ws.tabs, &mut ws.active_tab, to);
            }
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

    /// Start a child on our socket, in its own process group.
    fn spawn(&mut self, mut cmd: Command, dir: &Path) -> Option<u32> {
        herdr::scrub_env(&mut cmd);
        cmd.env("WAYLAND_DISPLAY", &self.socket_name)
            .env_remove("DISPLAY") // no XWayland escape to the host
            .current_dir(dir)
            .process_group(0);
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
        let dir = self.workspaces[w].dir.clone();
        let pane = self.new_herdr_pane(w, t, split);
        let pid = match &pane {
            Some(pane) => self.attach(pane, &dir),
            None => {
                let mut term = Command::new("alacritty");
                term.arg("--working-directory").arg(&dir);
                self.spawn(term, &dir)
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
    ) -> Option<herdr::PaneInfo> {
        let env = json!({"WAYLAND_DISPLAY": self.socket_name.to_string_lossy()});
        let ws = &self.workspaces[w];
        let target = t
            .and_then(|t| ws.tabs.get(t)?.herdr_split_target())
            .map(|p| p.pane_id.clone());
        let workspace_id = ws.herdr_id().map(str::to_owned);
        let (dir, name, project) = (
            ws.dir.clone(),
            ws.name.clone(),
            ws.kind == WorkspaceKind::Project,
        );
        let herdr = self.herdr.as_mut()?;
        let pane = match (target, workspace_id) {
            (Some(target), _) => herdr.pane_split(&target, split, &dir, &env),
            (None, Some(workspace_id)) => herdr.tab_create(&workspace_id, &dir, &env),
            (None, None) => herdr.workspace_create(&dir, &name, project, &env),
        };
        pane.inspect_err(|err| tracing::warn!("herdr: {err}; starting a plain terminal"))
            .ok()
    }

    /// Show herdr pane `pane` in an Alacritty window.
    fn attach(&mut self, pane: &herdr::PaneInfo, dir: &Path) -> Option<u32> {
        let dir = pane
            .cwd
            .as_deref()
            .filter(|d| d.is_dir())
            .unwrap_or(dir)
            .to_path_buf();
        self.spawn(herdr::attach_command(&pane.terminal_id, &dir), &dir)
    }

    /// Connect to gerdr's herdr session (starting it if needed), show what it
    /// already holds and follow its changes. Without herdr, terminals are
    /// plain Alacritty windows.
    fn connect_herdr(&mut self) {
        let Some((mut herdr, fresh)) = herdr::Herdr::connect_or_start(&self.socket_name) else {
            return;
        };
        let mut workspaces = herdr.workspace_list().unwrap_or_default();
        if fresh {
            // A new server opens a workspace of its own; gerdr opens its own.
            workspaces
                .retain(|ws| ws.is_gerdr() || herdr.workspace_close(&ws.workspace_id).is_err());
        }
        self.herdr = Some(herdr);
        for ws in workspaces {
            self.sync_herdr_workspace(&ws.workspace_id);
        }
        self.refresh_agents();
        self.watch_herdr_tabs();
        let sub = match herdr::Herdr::subscribe() {
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
                    state.herdr = None;
                    state.agents.clear();
                    state.herdr_tabs.clear();
                    return Ok(PostAction::Remove);
                }
            }
            Ok(PostAction::Continue)
        });
        if let Err(err) = inserted {
            tracing::warn!("herdr events: {err}");
        }
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
            herdr::Event::WorkspaceRenamed {
                workspace_id,
                label,
            } => {
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
            self.refresh_agents();
        }
    }

    /// Follow what herdr reports about the tabs, polled off the event loop.
    fn watch_herdr_tabs(&mut self) {
        let (tx, rx): (_, Channel<HashMap<String, herdr::TabDetail>>) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            // Rounds are dropped once the server is gone.
            if let channel::Event::Msg(tabs) = event
                && state.herdr.is_some()
            {
                state.herdr_tabs = tabs;
            }
        });
        match inserted {
            Ok(_) => herdr::watch_tabs(AGENT_INTERVAL, move |tabs| tx.send(tabs).is_ok()),
            Err(err) => tracing::warn!("herdr tabs: {err}"),
        }
    }

    /// Read the Git status of the workspace folders now and every
    /// `GIT_INTERVAL`, off the event loop.
    fn watch_git(&mut self) {
        let (tx, rx): (_, Channel<HashMap<PathBuf, git::GitStatus>>) = channel::channel();
        let read = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(git) = event {
                state.git = git;
                state.git_pending = false;
            }
        });
        let timer = self
            .loop_handle
            .insert_source(Timer::immediate(), move |_, _, state| {
                // A round can outlast the interval in a big repository.
                if !state.git_pending {
                    state.git_pending = true;
                    let mut dirs: Vec<PathBuf> =
                        state.workspaces.iter().map(|ws| ws.dir.clone()).collect();
                    dirs.sort();
                    dirs.dedup();
                    let tx = tx.clone();
                    git::read_all(dirs, move |git| {
                        let _ = tx.send(git);
                    });
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

    /// Ask herdr which agents run in the session now.
    fn refresh_agents(&mut self) {
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        match herdr.agent_list() {
            Ok(agents) => {
                self.notify_agents(&agents);
                self.agents = agents;
            }
            Err(err) => tracing::debug!("herdr: {err}"),
        }
    }

    /// Notify of each of `agents` that just came to need input or finished
    /// where the user can't see it: in a hidden tab, or anywhere while
    /// gerdr's window is in the background. herdr's own idea of what the
    /// user has seen follows its focused pane, not gerdr's, so a finished
    /// agent may come back `idle` rather than `done`: both count.
    fn notify_agents(&mut self, agents: &[herdr::AgentInfo]) {
        use herdr::AgentStatus::{Blocked, Done, Idle, Working};
        let mut news = Vec::new();
        for agent in agents {
            // Only a change is news; an agent seen for the first time (as all
            // are when gerdr starts) isn't.
            let Some(before) = self.agents.iter().find(|a| a.pane_id == agent.pane_id) else {
                continue;
            };
            let what = match (before.agent_status, agent.agent_status) {
                (Blocked, Blocked) => continue,
                (_, Blocked) => "needs input",
                (Working, Idle | Done) => "is done",
                _ => continue,
            };
            let place = self.herdr_pane_place(&agent.pane_id);
            if self.host_focused && place.is_some_and(|(w, t)| self.is_visible(w, t)) {
                continue;
            }
            let who = agent.label();
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
            news.push((&agent.pane_id, format!("{who} {what}"), body.join(" · ")));
        }
        if let Some(notifier) = self.notifier.as_mut() {
            for (pane_id, summary, body) in news {
                notifier.send(pane_id, &summary, &body);
            }
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
                    for pane_id in clicked {
                        state.focus_agent(&pane_id);
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

    /// Show the pane agent card `pane_id` stands for: switch to its workspace
    /// and tab and focus its window. herdr counts the agent as seen then.
    pub fn focus_agent(&mut self, pane_id: &str) {
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
        self.refresh_agents();
    }

    /// The agent cards of each workspace, in sidebar order.
    pub fn agent_views(&self) -> Vec<Vec<AgentView>> {
        self.workspaces
            .iter()
            .map(|ws| {
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
            })
            .collect()
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
    /// show yet: in the tab mirroring theirs, else in a new tab laid out like
    /// herdr's, in a new workspace if needed.
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
        let existing = self
            .workspaces
            .iter()
            .position(|ws| ws.herdr_id() == Some(workspace_id));
        let info = match existing {
            Some(_) => None,
            None => match herdr.workspace_get(workspace_id) {
                Ok(info) => Some(info),
                Err(_) => return,
            },
        };
        // The new panes by tab, in herdr's tab order, with herdr's layout for new tabs.
        let order = herdr.tab_list(workspace_id).unwrap_or_default();
        let mut tabs: Vec<(String, Vec<herdr::PaneInfo>, Vec<herdr::LayoutPane>)> = Vec::new();
        for tab_id in order {
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

        let w = existing.unwrap_or_else(|| {
            let info = info.unwrap();
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
            let mut ws = Workspace::new(dir, kind);
            ws.name = info.label;
            self.workspaces.push(ws);
            self.workspaces.len() - 1
        });
        let dir = self.workspaces[w].dir.clone();
        let area = self.content_rect();
        for (tab_id, in_tab, cells) in tabs {
            let mut started = Vec::new();
            for pane in in_tab {
                if let Some(pid) = self.attach(&pane, &dir) {
                    started.push((self.new_pane_id(), pid, pane));
                }
            }
            let ws = &mut self.workspaces[w];
            let t = match ws
                .tabs
                .iter()
                .position(|t| t.herdr_tab_id() == Some(&tab_id))
            {
                Some(t) => t,
                None => {
                    let rects: Vec<_> = started
                        .iter()
                        .filter_map(|(id, _, pane)| {
                            let c = cells.iter().find(|c| c.pane_id == pane.pane_id)?.rect;
                            Some((
                                *id,
                                Rectangle::new((c.x, c.y).into(), (c.width, c.height).into()),
                            ))
                        })
                        .collect();
                    ws.tabs.push(Tab {
                        layout: Node::from_rects(&rects),
                        ..Default::default()
                    });
                    ws.tabs.len() - 1
                }
            };
            let tab = &mut ws.tabs[t];
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
        }

        // A project gerdr opened gets its Zed back over the first tab.
        let ws = &self.workspaces[w];
        if existing.is_none() && ws.kind == WorkspaceKind::Project && !ws.tabs.is_empty() {
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
            self.launch_next_zed();
        }
        self.prune();
        self.arrange();
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
        let zed = self.new_pane_id();
        let mut tab = Tab {
            layout: Some(Node::Leaf(zed)),
            ..Default::default()
        };
        tab.expected.push(Expected::new(None, "zed", zed, true));
        let mut ws = Workspace::new(dir, WorkspaceKind::Project);
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
        self.workspaces
            .push(Workspace::new(dir, WorkspaceKind::Terminal));
        let w = self.workspaces.len() - 1;
        let Some((pid, herdr)) = self.start_terminal(w, None, Dir::Right) else {
            self.workspaces.pop();
            return;
        };
        let id = self.new_pane_id();
        self.workspaces[w].tabs.push(Tab::terminal(pid, id, herdr));
        self.switch_workspace(w);
        self.prune();
    }

    /// Zed windows come from a shared editor process, so they can only be told
    /// apart by app_id. Start them one at a time: the next queued one once the
    /// previous window has shown up (or never will).
    fn launch_next_zed(&mut self) {
        loop {
            let mut queued = None;
            for (w, ws) in self.workspaces.iter().enumerate() {
                for (t, tab) in ws.tabs.iter().enumerate() {
                    for (i, e) in tab
                        .expected
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.app_id == "zed")
                    {
                        if e.pid.is_some() {
                            return; // one is already on its way
                        }
                        queued.get_or_insert((w, t, i));
                    }
                }
            }
            let Some((w, t, i)) = queued else { return };

            // `-n`: a new Zed window, handed to the running editor if there is one.
            let dir = self.workspaces[w].dir.clone();
            let mut zed = Command::new("zed");
            zed.arg("-n")
                .arg("--user-data-dir")
                .arg(&self.zed_data)
                .arg(&dir);
            let pid = self.spawn(zed, &dir);
            let tab = &mut self.workspaces[w].tabs[t];
            match pid {
                Some(pid) => {
                    tab.expected[i].pid = Some(pid);
                    return;
                }
                None => {
                    let gone = tab.expected.remove(i);
                    layout::remove(&mut tab.layout, gone.pane);
                }
            }
        }
    }

    /// Ask the user for a folder (on the host) to open as a new workspace of `kind`.
    pub fn pick_workspace(&mut self, kind: WorkspaceKind) {
        if self.picker.is_some() {
            return;
        }
        let start = self
            .workspace()
            .and_then(|ws| ws.dir.parent())
            .map(Path::to_path_buf)
            .or_else(home_dir);
        let mut cmd = Command::new("zenity");
        let title = match kind {
            WorkspaceKind::Project => "--title=Open project folder",
            WorkspaceKind::Terminal => "--title=Open terminal folder",
        };
        cmd.args(["--file-selection", "--directory", title])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(start) = start {
            cmd.arg(format!("--filename={}/", start.display()));
        }
        match cmd.spawn() {
            Ok(child) => self.picker = Some((child, kind)),
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
                WorkspaceKind::Project => self.open_project(PathBuf::from(path)),
                WorkspaceKind::Terminal => self.open_terminal(PathBuf::from(path)),
            }
        }
    }

    /// Close every pane of the workspace; it goes away once their windows do.
    pub fn close_workspace(&mut self, idx: usize) {
        if let Some(ws) = self.workspaces.get(idx) {
            let tabs: Vec<_> = ws.tabs.iter().map(Self::closing).collect();
            tabs.into_iter().for_each(|panes| self.close_panes(panes));
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
            if !closed && let Some(window) = window {
                window.toplevel().unwrap().send_close();
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
            Action::Switcher => self.open_switcher(),
            Action::Run => self.open_run_prompt(),
            Action::CyclePane(step) => self.focus_pane(|tab| {
                let shown = tab.shown();
                let current = shown.iter().position(|id| Some(*id) == tab.focused_id())? as isize;
                Some(shown[(current + step).rem_euclid(shown.len() as isize) as usize])
            }),
            _ => {}
        }
    }

    /// Flip between light and dark, remembered for the next start.
    pub fn toggle_theme(&mut self) {
        self.theme = self.theme.toggled();
        if let Err(err) = std::fs::write(theme_file(), self.theme.name()) {
            tracing::warn!("saving theme: {err}");
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
        let title = self
            .workspace()
            .map_or("gerdr".into(), |ws| format!("{} — gerdr", ws.name));
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
        for tab in self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs) {
            let mut gone = Vec::new();
            tab.expected.retain_mut(|e| {
                if e.pid.is_some_and(|pid| exited.contains(&pid)) {
                    e.exited = Some(now);
                }
                // A launched app's window can't be told apart once its processes are gone.
                let grace = if e.app_id.is_empty() {
                    Duration::ZERO
                } else {
                    HANDOFF_GRACE
                };
                let keep = e.exited.is_none_or(|t| now - t < grace);
                if !keep {
                    gone.push(e.pane);
                }
                keep
            });
            // The space kept for a window that never came goes to the other panes.
            for pane in gone {
                layout::remove(&mut tab.layout, pane);
                dropped = true;
            }
        }
        if dropped {
            self.launch_next_zed();
            self.prune();
            self.arrange();
        }
        self.poll_picker();
    }

    /// First commit of a toplevel: put it in the workspace, tab and pane it was started for.
    pub fn place(&mut self, window: Window, app_id: &str) {
        self.unplaced.retain(|w| w != &window);
        let dh = &self.display_handle;
        let pid = dh
            .get_client(window.toplevel().unwrap().wl_surface().id())
            .ok()
            .and_then(|client| client.get_credentials(dh).ok())
            .map(|creds| creds.pid as u32);
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
        let found = lineage
            .iter()
            .find_map(|pid| find(&|e| e.pid == Some(*pid)))
            .or_else(|| {
                find(&|e| e.pid.is_some() && !e.app_id.is_empty() && app_id.contains(e.app_id))
            });
        let (w, t, id, focus, herdr) = match found {
            Some((w, t, e)) => {
                let e = self.workspaces[w].tabs[t].expected.remove(e);
                self.launch_next_zed();
                (w, t, e.pane, e.focus, e.herdr)
            }
            // Not one we started (e.g. a second Zed window): split the
            // focused pane of the current tab along its longer side.
            None => {
                if self.workspaces.is_empty() {
                    self.workspaces.push(Workspace::new(
                        home_dir().unwrap_or_default(),
                        WorkspaceKind::Project,
                    ));
                    self.active_workspace = 0;
                }
                let (id, area) = (self.new_pane_id(), self.content_rect());
                let ws = &mut self.workspaces[self.active_workspace];
                if ws.tabs.is_empty() {
                    ws.tabs.push(Tab::default());
                    ws.active_tab = 0;
                }
                ws.tabs[ws.active_tab].split(None, id, None, area);
                (self.active_workspace, ws.active_tab, id, true, None)
            }
        };

        let tab = &mut self.workspaces[w].tabs[t];
        tab.panes.push(Pane {
            id,
            window: window.clone(),
            herdr,
        });
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
            if self.is_visible(w, t) {
                self.space.map_element(window, rect.loc, false);
            } else {
                self.space.unmap_elem(&window);
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
        self.unplaced
            .retain(|w| w.toplevel().unwrap().wl_surface() != surface);
        let is_it = |p: &Pane| p.window.toplevel().unwrap().wl_surface() == surface;
        let found = self.workspaces.iter().enumerate().find_map(|(w, ws)| {
            ws.tabs
                .iter()
                .enumerate()
                .find_map(|(t, tab)| tab.panes.iter().position(is_it).map(|p| (w, t, p)))
        });
        let Some((w, t, p)) = found else { return };

        let tab = &mut self.workspaces[w].tabs[t];
        let pane = tab.panes.remove(p);
        self.space.unmap_elem(&pane.window);
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
        let over_menu = self.terminal_menu
            && chrome::terminal_menu(self.sidebar_width)
                .0
                .to_f64()
                .contains(pos);
        let reordering = self.reorder.is_some_and(|r| r.moving);
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
                let title = main
                    .and_then(|window| {
                        with_states(window.toplevel().unwrap().wl_surface(), |states| {
                            let data = states
                                .data_map
                                .get::<XdgToplevelSurfaceData>()?
                                .lock()
                                .unwrap();
                            data.title.clone().or_else(|| data.app_id.clone())
                        })
                    })
                    .unwrap_or_else(|| "…".into());
                let detail = tab.herdr_tab_id().and_then(|id| self.herdr_tabs.get(id));
                TabView::new(title, detail, &self.agents)
            })
            .collect()
    }

    /// What each workspace row in the sidebar shows, with the tabs of each.
    fn workspace_views(&self) -> Vec<(WorkspaceView, Vec<TabView>)> {
        self.workspaces
            .iter()
            .map(|ws| {
                let tabs = self.tab_views(ws);
                let mut view = WorkspaceView::new(ws.name.clone(), ws.kind, &tabs);
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

    /// What the sidebar and tab strip should show right now.
    fn chrome_view(&self) -> ChromeView {
        let ws = self.workspace();
        let active = ws.map(|_| self.active_workspace);
        let (workspaces, mut tabs): (Vec<_>, Vec<_>) = self.workspace_views().into_iter().unzip();
        let tabs = active.map_or(Vec::new(), |i| std::mem::take(&mut tabs[i]));
        ChromeView {
            workspaces,
            agents: self.agent_views(),
            active_workspace: active,
            tabs,
            active_tab: ws.map_or(0, |ws| ws.active_tab),
            menu: self.terminal_menu.then_some(self.menu_hover),
            switcher: self.switcher_view(),
            prefix: self.prefix_armed.then(|| self.keymap.help()),
            theme: self.theme,
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

/// Where the light/dark choice is kept between runs.
fn theme_file() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/theme")
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // Every argument is a folder to open as a workspace; default: the current dir.
    let mut folders: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if folders.is_empty() {
        folders.push(std::env::current_dir()?);
    }
    // Own data dir so Zed runs its own instance here instead of handing the
    // windows to a Zed already running on the host.
    let zed_data = home_dir()
        .unwrap_or_default()
        .join(".local/share/gerdr/zed");
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

    // Wayland socket for the children. A fixed name, so shells that outlive
    // gerdr in herdr still reach the next one.
    let socket = ListeningSocketSource::with_name("gerdr-0")
        .or_else(|_| ListeningSocketSource::new_auto())?;
    let socket_name = socket.socket_name().to_os_string();

    let (keymap, apps) = config::load();
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
        theme: std::fs::read_to_string(theme_file())
            .ok()
            .and_then(|s| Theme::from_name(s.trim()))
            .unwrap_or(Theme::Dark),
        sidebar_width: SIDEBAR_WIDTH,
        next_pane: 0,
        drag: None,
        reorder: None,
        chrome_cursor: CursorIcon::Default,
        suppressed_keys: Vec::new(),
        prefix_armed: false,
        keymap,
        socket_name: socket_name.clone(),
        zed_data,
        children: Vec::new(),
        herdr: None,
        agents: Vec::new(),
        herdr_tabs: HashMap::new(),
        git: HashMap::new(),
        git_pending: false,
        notifier: None,
        host_focused: true,
        process_groups: Vec::new(),
        picker: None,
        terminal_menu: false,
        menu_hover: None,
        switcher: None,
        recent_dirs: Vec::new(),
        zoxide: None,
        switcher_repeat: None,
        apps,
        installed_apps: Vec::new(),
        installed_apps_tx: None,
        compositor_state: CompositorState::new::<Gerdr>(&dh),
        xdg_shell_state: XdgShellState::new::<Gerdr>(&dh),
        xdg_decoration_state: XdgDecorationState::new::<Gerdr>(&dh),
        shm_state: ShmState::new::<Gerdr>(&dh, vec![]),
        dmabuf_state,
        output_manager_state: OutputManagerState::new_with_xdg_output::<Gerdr>(&dh),
        seat_state,
        data_device_state: DataDeviceState::new::<Gerdr>(&dh),
        primary_selection_state: PrimarySelectionState::new::<Gerdr>(&dh),
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
                let size = state.backend.window_size();
                let view = state.chrome_view();
                let clear_color = state.theme.clear_color();
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
                    state.stop_switcher_repeat();
                }
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
        })?;

    // Notice children that exit before showing a window, and the folder picker closing.
    event_loop
        .handle()
        .insert_source(Timer::from_duration(REAP_INTERVAL), |_, _, state| {
            state.reap_children();
            TimeoutAction::ToDuration(REAP_INTERVAL)
        })?;
    event_loop
        .handle()
        .insert_source(Timer::from_duration(AGENT_INTERVAL), |_, _, state| {
            state.refresh_agents();
            TimeoutAction::ToDuration(AGENT_INTERVAL)
        })?;

    state.connect_herdr();
    state.watch_git();
    state.connect_notifier();
    state.watch_zoxide();
    state.watch_installed_apps();
    for folder in folders {
        state.open_project(folder);
    }

    event_loop.run(None, &mut state, |_| {})?;

    for group in &state.process_groups {
        // Each child led its own process group; take down the whole group,
        // including processes it handed off to (Zed's editor).
        unsafe { libc::kill(-group, libc::SIGTERM) };
    }
    if let Some((mut picker, _)) = state.picker.take() {
        let _ = picker.kill();
    }
    Ok(())
}
