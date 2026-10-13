//! gerdr's own UI: the workspace sidebar on the left (each workspace with
//! what its tabs show put together, and a card per coding agent herdr sees
//! in it), the tab strip above the apps
//! (each tab with what runs in it, as herdr reports;
//! the light/dark switch at its right end), and the key help
//! panel shown while the prefix is armed, and the switcher; the welcome
//! screen covering it all, and the quit dialog over that. All are drawn on the CPU into memory buffers at physical
//! resolution (crisp at fractional scales) and only redrawn when they change.
//!
//! The look follows Zed's default themes, One Dark and One Light.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    utils::{Logical, Point, Rectangle, Size, Transform},
};

use crate::{
    WorkspaceKind,
    git::GitStatus,
    herdr::{AgentInfo, AgentStatus, PaneState, TabDetail},
    home_dir,
    icons::AppIcon,
};

/// Height of the sidebar header, in logical pixels.
pub const BAR_HEIGHT: i32 = 34;
/// The tab strip: a title row, then a line for what runs in the tab.
pub const TAB_STRIP_HEIGHT: i32 = TAB_TITLE_HEIGHT + TAB_LINE + TAB_BOTTOM;
const TAB_TITLE_HEIGHT: i32 = 28;
const TAB_LINE: i32 = 15;
const TAB_BOTTOM: i32 = 5;
/// Room for the branch icon left of the branch name.
const BRANCH_ICON_WIDTH: i32 = 10;
const BRANCH_ICON_GAP: i32 = 5;
/// Room for each Git mark after a workspace's branch (uncommitted changes,
/// ahead, behind), and the gap before it.
const GIT_MARK_WIDTH: i32 = 8;
const GIT_MARK_GAP: i32 = 4;
/// Width of the workspace sidebar, in logical pixels: at start, and the
/// range it can be dragged to.
pub const SIDEBAR_WIDTH: i32 = 200;
pub const SIDEBAR_MIN_WIDTH: i32 = 120;
pub const SIDEBAR_MAX_WIDTH: i32 = 480;
/// A workspace row: just its name, or its name in a shorter title band,
/// then the same detail lines as a tab.
const ROW_HEIGHT: i32 = 32;
const ROW_TITLE_HEIGHT: i32 = 28;
const ROW_BOTTOM: i32 = 6;
const ROWS_TOP: i32 = BAR_HEIGHT + 6;
/// Horizontal inset of the rounded sidebar row highlight.
const ROW_INSET: i32 = 6;
const ROW_RADIUS: f32 = 6.0;
const CLOSE_WIDTH: i32 = 28;
const TAB_MAX_WIDTH: i32 = 360;
const BUTTON_WIDTH: i32 = 36;
/// Side of the rounded square behind the tab strip's "+".
const NEW_TAB_SIZE: i32 = 28;
/// The sidebar header's buttons: "add project", and the split "add terminal"
/// button with its menu chevron.
const HEADER_BUTTON: i32 = 28;
const CHEVRON_WIDTH: i32 = 16;
/// Space between the chevron and the sidebar's right edge.
const HEADER_MARGIN: i32 = 4;
/// Room for the workspace kind icon left of each sidebar row's name.
const ROW_ICON_WIDTH: i32 = 14;
const ROW_ICON_GAP: i32 = 8;
/// An app's own icon in a switcher row: bigger than the drawn ones, reaching
/// a little into the room around them.
const APP_ICON_SIZE: i32 = 20;
const GAP: i32 = 1;
/// Agent cards, under their workspace's row: indent past the row's icon,
/// space around them, inner padding, the header line and each detail line.
const CARD_INDENT: i32 = 14;
const CARD_GAP: i32 = 4;
const CARD_PAD: i32 = 7;
const CARD_HEADER: i32 = 20;
const CARD_LINE: i32 = 17;
const CARD_RADIUS: f32 = 6.0;
/// Room for the status mark left of the agent's name.
const STATUS_WIDTH: i32 = 10;
const STATUS_GAP: i32 = 7;
/// The status pill at the right of a card's header.
const PILL_HEIGHT: i32 = 16;
const PILL_PADDING: i32 = 6;
/// Gap between a detail's label and its value.
const FIELD_GAP: i32 = 8;
const FONT_SIZE: f32 = 15.0;
/// Tab labels and workspace names.
const LABEL_FONT_SIZE: f32 = 17.0;
const SMALL_FONT_SIZE: f32 = 13.0;
/// Agent card details, and their ids and status pill.
const DETAIL_FONT_SIZE: f32 = 12.0;
const TINY_FONT_SIZE: f32 = 11.0;
const KEY_FONT_SIZE: f32 = 13.0;
const ICON_SIZE: f32 = 20.0;
const PADDING: f32 = 10.0;
const HELP_MARGIN: i32 = 16;
const HELP_PADDING: i32 = 14;
const HELP_RADIUS: f32 = 8.0;
/// Room around popups (help panel, menu) for their drop shadow.
const SHADOW: i32 = 12;
const HELP_TITLE_HEIGHT: i32 = 40;
const HELP_ROW_HEIGHT: i32 = 30;
const HELP_KEY_GAP: i32 = 16;
const HELP_COLUMN_GAP: i32 = 32;
/// Keycaps: text padding, gap around the "/" between alternatives, corner radius.
const CAP_PADDING: i32 = 6;
const CAP_GAP: i32 = 5;
const CAP_RADIUS: f32 = 4.0;
/// The menu under the terminal button's chevron.
const MENU_WIDTH: i32 = 180;
const MENU_PADDING: i32 = 4;
const MENU_ITEM_HEIGHT: i32 = 30;
const MENU_RADIUS: f32 = 6.0;
pub const MENU_ITEMS: &[&str] = &["Open folder…"];

/// The switcher, at the top of the app area.
const SWITCHER_WIDTH: i32 = 640;
const SWITCHER_TOP: i32 = 24;
const SWITCHER_INPUT: i32 = 44;
const SWITCHER_PADDING: i32 = 6;
const SWITCHER_ROW: i32 = 34;
/// The most rows it shows; it scrolls for more.
const SWITCHER_ROWS: i32 = 12;
/// The caret after the query.
const CARET_WIDTH: i32 = 2;
const CARET_HEIGHT: i32 = 18;

/// The welcome screen: a column at most this wide, centred on the output.
const WELCOME_WIDTH: i32 = 560;
const GREETING_FONT_SIZE: f32 = 28.0;
/// From the top: greeting, subtitle, a gap, the "Recent projects" label,
/// the rows, a gap, the button and the note under it.
const GREETING_HEIGHT: i32 = 40;
const SUBTITLE_HEIGHT: i32 = 24;
const WELCOME_GAP: i32 = 24;
const WELCOME_LABEL: i32 = 24;
/// A project row: its name, detail and age on one line.
const WELCOME_ROW: i32 = 44;
/// The most rows it shows; it scrolls for more.
const WELCOME_ROWS: i32 = 8;
const WELCOME_BUTTON: i32 = 36;
const WELCOME_BUTTON_WIDTH: i32 = 200;
const NOTE_GAP: i32 = 6;
const NOTE_HEIGHT: i32 = 20;
/// The key hints, a margin above the output's bottom edge.
const HINTS_HEIGHT: i32 = 24;
/// Where the middle of the welcome screen sits, as a share of the output's height.
const WELCOME_MIDDLE: f64 = 0.42;
/// Space between the hints' entries, and between each key and its action.
const HINT_GAP: i32 = 18;
const HINT_KEY_GAP: i32 = 6;
/// Width of the focus ring around the selected button, and its gap to it.
const FOCUS_RING: f32 = 1.5;
const FOCUS_GAP: f32 = 2.0;

/// The quit dialog, centred on the output: title, question, the "Don't ask
/// again" checkbox, and the buttons at the bottom right.
const QUIT_WIDTH: i32 = 420;
const QUIT_HEIGHT: i32 = 190;
const QUIT_PADDING: i32 = 20;
const QUIT_TITLE: i32 = 24;
const QUIT_BODY: i32 = 22;
const QUIT_LINE_GAP: i32 = 10;
/// The checkbox row: the box and its label, all of it clickable.
const CHECKBOX_ROW: i32 = 24;
const CHECKBOX_WIDTH: i32 = 150;
const CHECKBOX_SIZE: i32 = 16;
const CHECKBOX_GAP: i32 = 8;
const QUIT_BUTTON: i32 = 32;
const QUIT_BUTTON_GAP: i32 = 8;
/// Cancel, Stop project, Keep running: labels and widths.
const QUIT_BUTTONS: [(&str, i32); 3] =
    [("Cancel", 84), ("Stop project", 116), ("Keep running", 124)];
const BUTTON_RADIUS: f32 = 6.0;

type Rgb = [u8; 3];

const fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Light or dark look, switched with the button at the top right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn toggled(self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }

    /// What shows between and behind the apps: Zed's workspace background.
    pub fn clear_color(self) -> [f32; 4] {
        let [r, g, b] = self.palette().background;
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    }

    fn palette(self) -> &'static Palette {
        match self {
            Theme::Dark => &ONE_DARK,
            Theme::Light => &ONE_LIGHT,
        }
    }
}

/// The colours the chrome uses, named after Zed's theme keys.
struct Palette {
    background: Rgb,
    panel: Rgb,
    tab_bar: Rgb,
    tab_active: Rgb,
    elevated: Rgb,
    element: Rgb,
    selected: Rgb,
    border: Rgb,
    border_variant: Rgb,
    text: Rgb,
    muted: Rgb,
    placeholder: Rgb,
    accent: Rgb,
    on_accent: Rgb,
    success: Rgb,
    warning: Rgb,
    shadow: f32,
}

const ONE_DARK: Palette = Palette {
    background: hex(0x3b414d),
    panel: hex(0x2f343e),
    tab_bar: hex(0x2f343e),
    tab_active: hex(0x282c33),
    elevated: hex(0x2f343e),
    element: hex(0x3b414d),
    selected: hex(0x454a56),
    border: hex(0x464b57),
    border_variant: hex(0x363c46),
    text: hex(0xdce0e5),
    muted: hex(0xa9afbc),
    placeholder: hex(0x878a98),
    accent: hex(0x74ade8),
    on_accent: hex(0x282c33),
    success: hex(0xa1c181),
    warning: hex(0xdec184),
    shadow: 0.45,
};

const ONE_LIGHT: Palette = Palette {
    background: hex(0xdcdcdd),
    panel: hex(0xebebec),
    tab_bar: hex(0xebebec),
    tab_active: hex(0xfafafa),
    elevated: hex(0xebebec),
    element: hex(0xfafafa),
    selected: hex(0xd3d3d4),
    border: hex(0xc9c9ca),
    border_variant: hex(0xdfdfe0),
    text: hex(0x242529),
    muted: hex(0x58585a),
    placeholder: hex(0x7e8086),
    accent: hex(0x5c78e2),
    on_accent: hex(0xfafafa),
    success: hex(0x669f59),
    warning: hex(0xa48819),
    shadow: 0.18,
};

impl Palette {
    fn status(&self, status: AgentStatus) -> Rgb {
        match status {
            AgentStatus::Working => self.accent,
            AgentStatus::Blocked => self.warning,
            AgentStatus::Done => self.success,
            AgentStatus::Idle => self.muted,
            AgentStatus::Unknown => self.placeholder,
        }
    }
}

/// `a` moved `t` of the way to `b`.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

/// One line of an agent card below its header.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentLine {
    /// What the agent is on, from its terminal title.
    Title(String),
    /// A labelled value.
    Field(String, String),
}

/// What an agent card shows: everything herdr reports about the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentView {
    /// The herdr pane it runs in; a click on the card shows it.
    pub pane_id: String,
    pub status: AgentStatus,
    /// How herdr names its kind, and the name it was given.
    pub kind: String,
    pub name: Option<String>,
    pub lines: Vec<AgentLine>,
}

impl AgentView {
    /// The card for `info`, running in herdr pane `pane`.
    pub fn new(info: &AgentInfo, pane: Option<&PaneState>) -> Self {
        let mut lines = Vec::new();
        let field = |label: &str, value: String| AgentLine::Field(label.into(), value);
        let title = info
            .title
            .as_deref()
            .map(tidy_title)
            .filter(|t| !t.is_empty());
        let term_title = info
            .terminal_title_stripped
            .as_deref()
            .or(info.terminal_title.as_deref())
            .map(tidy_title);
        let term_title = term_title.filter(|t| !t.is_empty() && Some(t) != title.as_ref());
        match (title, term_title) {
            (Some(title), term) => {
                lines.push(AgentLine::Title(title));
                lines.extend(term.map(|t| field("terminal", t)));
            }
            (None, Some(term)) => lines.push(AgentLine::Title(term)),
            (None, None) => {}
        }
        let kind = info
            .display_agent
            .clone()
            .or_else(|| info.agent.clone())
            .unwrap_or_else(|| "agent".into());
        if let Some(agent) = info.agent.as_ref().filter(|a| **a != kind) {
            lines.push(field("kind", agent.clone()));
        }
        let root = pane.and_then(repo_root);
        if let Some(cwd) = &info.cwd {
            lines.push(field("cwd", short_path(cwd, root)));
        }
        if let Some(fg) = info
            .foreground_cwd
            .as_ref()
            .filter(|fg| Some(*fg) != info.cwd.as_ref())
        {
            lines.push(field("fg cwd", short_path(fg, root)));
        }
        lines.extend(info.state_labels.iter().map(|(k, v)| field(k, v.clone())));
        if let Some(session) = &info.agent_session {
            lines.push(field(
                "session",
                format!("{} {}", session.kind, session.value),
            ));
            lines.push(field("source", session.source.clone()));
            if Some(&session.agent) != info.agent.as_ref() {
                lines.push(field("agent", session.agent.clone()));
            }
        }
        lines.extend(info.tokens.iter().map(|(k, v)| field(k, v.clone())));
        Self {
            pane_id: info.pane_id.clone(),
            status: info.agent_status,
            kind,
            name: info.name.clone(),
            lines,
        }
    }

    /// The card's height, in logical pixels.
    pub fn height(&self) -> i32 {
        2 * CARD_PAD + CARD_HEADER + self.lines.len() as i32 * CARD_LINE
    }
}

pub fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// A terminal title without the spinner or status glyph agents put in front.
fn tidy_title(title: &str) -> String {
    let glyph = |c: char| c.is_whitespace() || matches!(c, '\u{2190}'..='\u{2BFF}' | '·' | '•');
    title.trim_start_matches(glyph).trim_end().to_string()
}

/// The name of the repository pane `p` is in, and its worktree's folder.
fn repo_root(p: &PaneState) -> Option<(&str, &std::path::Path)> {
    p.repo
        .as_deref()
        .zip(p.worktree.as_ref().map(|w| w.path.as_path()))
}

/// `path`, short: from the repository named `repo` when it is inside that
/// repository's worktree, from "~" when it is inside the home folder, with
/// every folder between that start and the last cut to its first letter (fish
/// style): "~/Personal/gerdr" is "~/P/gerdr", "<worktree>/src/bin" is
/// "gerdr/s/bin".
fn short_path(path: &std::path::Path, root: Option<(&str, &std::path::Path)>) -> String {
    let in_repo = root.and_then(|(repo, worktree)| Some((repo, path.strip_prefix(worktree).ok()?)));
    let in_home = || home_dir().and_then(|home| Some(path.strip_prefix(home).ok()?.to_path_buf()));
    let (start, rest) = match in_repo {
        Some((repo, rest)) => (repo.to_string(), rest.to_path_buf()),
        None => match in_home() {
            Some(rest) => ("~".into(), rest),
            None => (
                String::new(),
                path.strip_prefix("/").unwrap_or(path).to_path_buf(),
            ),
        },
    };
    let names: Vec<String> = rest
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let Some((last, between)) = names.split_last() else {
        return if start.is_empty() { "/".into() } else { start };
    };
    let initial = |name: &String| {
        let skip = name.chars().take_while(|&c| c == '.').count();
        name.chars().take(skip + 1).collect::<String>()
    };
    let mut parts = vec![start];
    parts.extend(between.iter().map(initial));
    parts.push(last.clone());
    parts.join("/")
}

/// What a tab shows: its title, and everything herdr reports about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabView {
    /// Its first window's title.
    pub title: String,
    /// The most pressing status among its agents, if it has any.
    pub status: Option<AgentStatus>,
    /// The Git branches its panes are on.
    pub branch: Option<String>,
    /// The repository and the folders its panes are in.
    pub place: String,
    /// What runs in its panes: agents, foreground processes, terminal titles.
    pub activity: String,
    /// Whether its focused terminal's command runs again after a restart:
    /// `None` when nothing runs in it.
    pub restart: Option<bool>,
}

impl TabView {
    /// The view of a tab titled `title`, mirroring herdr tab `detail` (if
    /// any), whose agents are among `agents`.
    pub fn new(title: String, detail: Option<&TabDetail>, agents: &[AgentInfo]) -> Self {
        let Some(TabDetail { tab, panes, .. }) = detail else {
            return Self {
                title,
                ..Default::default()
            };
        };
        let status = (tab.agent_status != AgentStatus::Unknown).then_some(tab.agent_status);
        let branch = distinct(panes.iter().filter_map(|p| {
            let worktree = p.worktree.as_ref()?;
            let name = match (&worktree.branch, worktree.is_detached) {
                (Some(branch), false) => branch.clone(),
                _ => "detached HEAD".into(),
            };
            Some(if worktree.is_linked_worktree {
                format!("{name} (linked worktree)")
            } else {
                name
            })
        }));
        let place = distinct(panes.iter().filter_map(|p| {
            let root = repo_root(p);
            let cwd = p.cwd.as_deref().map(|cwd| short_path(cwd, root));
            let fg = p
                .foreground_cwd
                .as_ref()
                .filter(|fg| Some(*fg) != p.cwd.as_ref())
                .map(|fg| short_path(fg, root));
            match (cwd, fg) {
                (Some(cwd), Some(fg)) => Some(format!("{cwd} → {fg}")),
                (cwd, fg) => cwd.or(fg),
            }
        }))
        .unwrap_or_default();
        let activity = panes
            .iter()
            .map(|p| pane_activity(p, &title, agents))
            .filter(|a| !a.is_empty())
            .collect::<Vec<_>>();
        Self {
            title,
            status,
            branch,
            place,
            activity: activity.join("  |  "),
            restart: None,
        }
    }
}

/// What a workspace row shows: its name and kind, and what its tabs show,
/// put together.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceView {
    pub name: String,
    pub kind: WorkspaceKind,
    /// The most pressing status among its tabs.
    pub status: Option<AgentStatus>,
    pub branch: Option<String>,
    /// Where its folder's repository stands, shown after `branch`.
    pub git: Option<GitStatus>,
    pub place: String,
    /// How many tabs it has and what runs in them; empty without herdr.
    pub activity: String,
}

impl WorkspaceView {
    pub fn new(name: String, kind: WorkspaceKind, tabs: &[TabView]) -> Self {
        let status = tabs
            .iter()
            .filter_map(|t| t.status)
            .max_by_key(|s| urgency(*s));
        let branch = distinct(
            tabs.iter()
                .filter_map(|t| t.branch.as_deref())
                .flat_map(|b| b.split(", "))
                .map(String::from),
        );
        let place = distinct(
            tabs.iter()
                .flat_map(|t| t.place.split(", "))
                .filter(|p| !p.is_empty())
                .map(String::from),
        )
        .unwrap_or_default();
        let runs = tabs
            .iter()
            .flat_map(|t| t.activity.split("  |  "))
            .filter(|a| !a.is_empty())
            .map(String::from);
        let activity = match (tabs.len(), distinct(runs)) {
            (_, None) if branch.is_none() && place.is_empty() => String::new(),
            (1, runs) => format!("1 tab{}", runs.map_or(String::new(), |r| format!(" · {r}"))),
            (n, runs) => format!(
                "{n} tabs{}",
                runs.map_or(String::new(), |r| format!(" · {r}"))
            ),
        };
        Self {
            name,
            kind,
            status,
            branch,
            git: None,
            place,
            activity,
        }
    }

    /// How many detail lines it shows below its name.
    fn detail_lines(&self) -> i32 {
        (self.branch.is_some() || !self.place.is_empty()) as i32 + !self.activity.is_empty() as i32
    }

    /// The height of the band holding its name, in logical pixels.
    fn title_height(&self) -> i32 {
        if self.detail_lines() == 0 {
            ROW_HEIGHT
        } else {
            ROW_TITLE_HEIGHT
        }
    }

    /// The row's height, in logical pixels.
    pub fn height(&self) -> i32 {
        self.title_height()
            + self.detail_lines() * TAB_LINE
            + if self.detail_lines() == 0 {
                0
            } else {
                ROW_BOTTOM
            }
    }
}

/// How pressing `status` is, for picking the one a workspace shows.
fn urgency(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

/// What runs in pane `p` of a tab titled `title`: its agent and its status,
/// else its foreground processes; its terminal title when that says more;
/// how far it is scrolled back.
fn pane_activity(p: &PaneState, title: &str, agents: &[AgentInfo]) -> String {
    let mut parts = Vec::new();
    let agent = agents.iter().find(|a| a.pane_id == p.pane_id);
    if let Some(kind) = agent
        .and_then(|a| a.display_agent.clone())
        .or_else(|| p.agent.clone())
    {
        let name = agent
            .and_then(|a| a.name.as_ref())
            .map_or(String::new(), |n| format!(" \"{n}\""));
        parts.push(format!("{kind}{name} {}", status_label(p.agent_status)));
    }
    let processes = p.process.iter().flat_map(|info| &info.foreground_processes);
    let commands =
        processes.map(|proc| command_line(&proc.cmdline).unwrap_or_else(|| proc.name.clone()));
    let commands = commands.collect::<Vec<_>>().join(" | ");
    if !commands.is_empty() && !parts.iter().any(|part| part.starts_with(&commands)) {
        parts.push(commands);
    }
    let term_title = p
        .terminal_title_stripped
        .as_deref()
        .or(p.terminal_title.as_deref())
        .map(tidy_title);
    if let Some(term_title) = term_title.filter(|t| {
        !t.is_empty() && t != title && !parts.iter().any(|part| part.contains(t.as_str()))
    }) {
        parts.push(format!("\"{term_title}\""));
    }
    if let Some(agent_title) = agent
        .and_then(|a| a.title.as_deref())
        .map(tidy_title)
        .filter(|t| !t.is_empty())
    {
        parts.push(agent_title);
    }
    if let Some(scroll) = p.scroll.filter(|s| s.offset_from_bottom > 0) {
        parts.push(format!(
            "scrolled back {}/{}",
            scroll.offset_from_bottom, scroll.max_offset_from_bottom
        ));
    }
    parts.join(" · ")
}

/// A command line with the program's folder left out: "/usr/bin/zsh -l" is
/// "zsh -l".
fn command_line(cmdline: &str) -> Option<String> {
    let mut words = cmdline.split_whitespace();
    let program = words.next()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    Some(
        std::iter::once(program)
            .chain(words)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The distinct values of `items` in order, joined; `None` if there are none.
fn distinct(items: impl Iterator<Item = String>) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    (!seen.is_empty()).then(|| seen.join(", "))
}

/// The icon of a switcher row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemIcon {
    Workspace(WorkspaceKind),
    Tab,
    Agent(AgentStatus),
    /// A folder to open.
    Folder,
    /// An app or command to launch.
    App,
    /// An app's own icon.
    Picture(AppIcon),
}

/// One row of the switcher, with the positions (in chars) of the characters
/// the query matched in its label and detail.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherRow {
    pub icon: ItemIcon,
    pub label: String,
    pub detail: String,
    pub label_hits: Vec<usize>,
    pub detail_hits: Vec<usize>,
}

/// What the switcher shows: the query (or `hint` without one), the rows in
/// view, and which of them is selected (none when nothing matches).
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherView {
    pub query: String,
    pub hint: &'static str,
    pub rows: Vec<SwitcherRow>,
    pub selected: Option<usize>,
}

/// What the help panel shows: the prefix, and the keys to press after it
/// as (keys, action). Alternative keys are split by " / ".
#[derive(Debug, PartialEq, Eq)]
pub struct Help {
    pub prefix: String,
    pub rows: Vec<(String, String)>,
}

/// What the welcome screen shows: a greeting, the projects, and the button
/// to start a new one.
#[derive(Clone, Debug, PartialEq)]
pub struct WelcomeView {
    /// "Good evening, lgal".
    pub greeting: String,
    pub subtitle: String,
    /// What was typed to filter the projects.
    pub query: String,
    /// The projects in view, most recently used first.
    pub rows: Vec<ProjectRow>,
    /// The selected row; `None` when nothing matches or the button is.
    pub selected: Option<usize>,
    pub hover: Option<WelcomeHit>,
    /// Set while naming a new project: the name typed so far.
    pub naming: Option<String>,
    /// The row asking to be confirmed for forgetting.
    pub confirm_forget: Option<usize>,
    /// A line under the button, such as "herdr isn't installed".
    pub note: Option<String>,
}

/// A project on the welcome screen.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    /// "3 repos · ~/Work/aft".
    pub detail: String,
    /// "2h ago".
    pub ago: String,
    /// Whether its session runs; `None` until known.
    pub running: Option<bool>,
    /// It is the project shown behind the welcome screen.
    pub current: bool,
}

/// What the pointer is over on the welcome screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeHit {
    Row(usize),
    Button,
}

/// Where the parts of the welcome screen go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WelcomeLayout {
    pub greeting: Rect,
    pub subtitle: Rect,
    /// The "Recent projects" label over the list.
    pub label: Rect,
    pub list: Rect,
    pub rows: Vec<Rect>,
    /// The "New project" button, or the name field while naming.
    pub button: Rect,
    pub note: Rect,
    /// The key hints at the bottom.
    pub hints: Rect,
}

/// What the quit dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitView {
    /// The project's name.
    pub project: String,
    /// The selected button: 0 Cancel, 1 Stop project, 2 Keep running.
    pub selected: usize,
    pub dont_ask: bool,
    pub hover: Option<QuitHit>,
}

/// What the pointer is over in the quit dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitHit {
    /// Index as in `QuitView::selected`.
    Button(usize),
    Checkbox,
}

/// Where the parts of the quit dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    pub checkbox: Rect,
    /// Cancel, Stop project, Keep running.
    pub buttons: [Rect; 3],
}

/// Where the welcome screen's parts go on an output of `size` with `rows`
/// projects in view: a column centred on it, its middle at
/// `WELCOME_MIDDLE` of the height as far as it fits, the hints at the bottom.
/// While `naming`, the name field takes the button's place, as wide as the column.
pub fn welcome_layout(size: Size<i32, Logical>, rows: usize, naming: bool) -> WelcomeLayout {
    let w = WELCOME_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let x = (size.w - w) / 2;
    let rect = |y: i32, h: i32| Rectangle::new((x, y).into(), (w, h).into());
    let hints = rect((size.h - HELP_MARGIN - HINTS_HEIGHT).max(0), HINTS_HEIGHT);
    let list_h = rows as i32 * WELCOME_ROW;
    let block = WELCOME_FIXED + list_h;
    let middle = (size.h as f64 * WELCOME_MIDDLE).round() as i32;
    let top = (middle - block / 2)
        .min(hints.loc.y - HELP_MARGIN - block)
        .max(HELP_MARGIN.min(size.h / 2));

    let greeting = rect(top, GREETING_HEIGHT);
    let subtitle = rect(greeting.loc.y + GREETING_HEIGHT, SUBTITLE_HEIGHT);
    let label = rect(
        subtitle.loc.y + SUBTITLE_HEIGHT + WELCOME_GAP,
        WELCOME_LABEL,
    );
    let list = rect(label.loc.y + WELCOME_LABEL, list_h);
    let row_rects = (0..rows as i32)
        .map(|i| rect(list.loc.y + i * WELCOME_ROW, WELCOME_ROW))
        .collect();
    let button_y = list.loc.y + list_h + WELCOME_GAP;
    let button = if naming {
        rect(button_y, WELCOME_BUTTON)
    } else {
        let bw = WELCOME_BUTTON_WIDTH.min(w);
        Rectangle::new(
            ((size.w - bw) / 2, button_y).into(),
            (bw, WELCOME_BUTTON).into(),
        )
    };
    let note = rect(button_y + WELCOME_BUTTON + NOTE_GAP, NOTE_HEIGHT);
    WelcomeLayout {
        greeting,
        subtitle,
        label,
        list,
        rows: row_rects,
        button,
        note,
        hints,
    }
}

/// The welcome screen's height above the hints without its rows.
const WELCOME_FIXED: i32 = GREETING_HEIGHT
    + SUBTITLE_HEIGHT
    + WELCOME_GAP
    + WELCOME_LABEL
    + WELCOME_GAP
    + WELCOME_BUTTON
    + NOTE_GAP
    + NOTE_HEIGHT;

/// How many project rows fit on the welcome screen of an output of `size`.
pub fn welcome_page(size: Size<i32, Logical>) -> usize {
    let room = size.h - WELCOME_FIXED - 3 * HELP_MARGIN - HINTS_HEIGHT;
    (room / WELCOME_ROW).clamp(1, WELCOME_ROWS) as usize
}

/// Where the quit dialog's parts go, centred on an output of `size`.
pub fn quit_layout(size: Size<i32, Logical>) -> QuitLayout {
    let panel = Rectangle::new(
        (
            ((size.w - QUIT_WIDTH) / 2).max(0),
            ((size.h - QUIT_HEIGHT) / 2).max(0),
        )
            .into(),
        (QUIT_WIDTH, QUIT_HEIGHT).into(),
    );
    let (x, y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = QUIT_WIDTH - 2 * QUIT_PADDING;
    let title = Rectangle::new((x, y).into(), (inner, QUIT_TITLE).into());
    let body_y = y + QUIT_TITLE + QUIT_LINE_GAP;
    let body = Rectangle::new((x, body_y).into(), (inner, QUIT_BODY).into());
    let checkbox = Rectangle::new(
        (x, body_y + QUIT_BODY + QUIT_LINE_GAP).into(),
        (CHECKBOX_WIDTH, CHECKBOX_ROW).into(),
    );
    // Right-aligned, in order.
    let total = QUIT_BUTTONS.iter().map(|(_, w)| w).sum::<i32>()
        + (QUIT_BUTTONS.len() as i32 - 1) * QUIT_BUTTON_GAP;
    let left = panel.loc.x + QUIT_WIDTH - QUIT_PADDING - total;
    let button_y = panel.loc.y + QUIT_HEIGHT - QUIT_PADDING - QUIT_BUTTON;
    let buttons = std::array::from_fn(|i| {
        let before = QUIT_BUTTONS[..i]
            .iter()
            .map(|(_, w)| w + QUIT_BUTTON_GAP)
            .sum::<i32>();
        Rectangle::new(
            (left + before, button_y).into(),
            (QUIT_BUTTONS[i].1, QUIT_BUTTON).into(),
        )
    });
    QuitLayout {
        panel,
        title,
        body,
        checkbox,
        buttons,
    }
}

type Rect = Rectangle<i32, Logical>;

/// Where the tabs (each with its "×" close button in its title row) and the
/// "+" button sit in a strip `width` wide (strip coordinates). The theme
/// switch keeps the right end.
pub fn tab_layout(count: usize, width: i32) -> (Vec<(Rect, Rect)>, Rect) {
    let room = (width - 2 * BUTTON_WIDTH).max(0);
    let tab_w = if count == 0 {
        0
    } else {
        (room / count as i32).min(TAB_MAX_WIDTH)
    };
    let tabs = (0..count as i32)
        .map(|i| {
            let w = (tab_w - GAP).max(0);
            let tab = Rectangle::new((i * tab_w, 0).into(), (w, TAB_STRIP_HEIGHT).into());
            let close_w = BUTTON_WIDTH.min(w);
            let close = Rectangle::new(
                (i * tab_w + w - close_w, 0).into(),
                (close_w, TAB_TITLE_HEIGHT).into(),
            );
            (tab, close)
        })
        .collect();
    // Full height, so the "+" sits in the middle of the strip as in browsers.
    let plus = Rectangle::new(
        (count as i32 * tab_w, 0).into(),
        (BUTTON_WIDTH, TAB_STRIP_HEIGHT).into(),
    );
    (tabs, plus)
}

/// The light/dark switch at the right end of a strip `width` wide (strip coordinates).
pub fn theme_toggle(width: i32) -> Rect {
    Rectangle::new(
        (width - BUTTON_WIDTH, 0).into(),
        (BUTTON_WIDTH, TAB_TITLE_HEIGHT).into(),
    )
}

pub struct SidebarLayout {
    /// Header button: pick a folder to open as a project workspace.
    pub project: Rect,
    /// Header split button: a terminal workspace in the home folder, and the
    /// chevron opening the menu to pick another folder.
    pub terminal: Rect,
    pub chevron: Rect,
    /// One row per workspace, with its "×" close button.
    pub rows: Vec<(Rect, Rect)>,
    /// Per workspace, its agent cards, below its row.
    pub cards: Vec<Vec<Rect>>,
    /// Per workspace, all it takes: its row, its cards and the gap after them.
    pub blocks: Vec<Rect>,
}

/// Where the header buttons, the rows of `workspaces` and their `agents`
/// cards sit in a sidebar `width` wide.
pub fn sidebar_layout(
    workspaces: &[WorkspaceView],
    agents: &[Vec<AgentView>],
    width: i32,
) -> SidebarLayout {
    let button = |x: i32, w: i32| Rectangle::new((x, 0).into(), (w, BAR_HEIGHT).into());
    let chevron = button(width - GAP - HEADER_MARGIN - CHEVRON_WIDTH, CHEVRON_WIDTH);
    let terminal = button(chevron.loc.x - HEADER_BUTTON, HEADER_BUTTON);
    let project = button(terminal.loc.x - HEADER_BUTTON, HEADER_BUTTON);
    let (mut rows, mut card_rects, mut blocks) = (Vec::new(), Vec::new(), Vec::new());
    let mut y = ROWS_TOP;
    let card_x = ROW_INSET + CARD_INDENT;
    let card_w = (width - GAP - ROW_INSET - card_x).max(0);
    let none = Vec::new();
    for (i, ws) in workspaces.iter().enumerate() {
        let row = Rectangle::new((0, y).into(), (width - GAP, ws.height()).into());
        let close_x = width - GAP - ROW_INSET - CLOSE_WIDTH;
        let close = Rectangle::new((close_x, y).into(), (CLOSE_WIDTH, ws.title_height()).into());
        rows.push((row, close));
        y += ws.height();
        let cards = agents.get(i).unwrap_or(&none);
        let ws_cards = cards
            .iter()
            .map(|card| {
                let card = Rectangle::new((card_x, y).into(), (card_w, card.height()).into());
                y += card.size.h + CARD_GAP;
                card
            })
            .collect();
        card_rects.push(ws_cards);
        if !cards.is_empty() {
            y += CARD_GAP;
        }
        blocks.push(Rectangle::new(row.loc, (row.size.w, y - row.loc.y).into()));
    }
    SidebarLayout {
        project,
        terminal,
        chevron,
        rows,
        cards: card_rects,
        blocks,
    }
}

/// Where workspace `held` goes when dragged to height `y`: the first place
/// where its block would hold `y`, the first or last above or below them
/// all. `held` stays put while `y` falls between two such places.
pub fn workspace_slot(blocks: &[Rect], held: usize, y: f64) -> usize {
    let Some(first) = blocks.first() else {
        return held;
    };
    let others: Vec<_> = (0..blocks.len()).filter(|i| *i != held).collect();
    let height = blocks[held].size.h as f64;
    let mut top = first.loc.y as f64;
    for to in 0..blocks.len() {
        if y < top + height {
            return if y >= top || to == 0 { to } else { held };
        }
        if let Some(other) = others.get(to) {
            top += blocks[*other].size.h as f64;
        }
    }
    blocks.len() - 1
}

/// Where a tab dragged to `x` goes, in a strip `width` wide holding `count`
/// tabs (strip coordinates): the place under `x`, the first or last beside them all.
pub fn tab_slot(count: usize, width: i32, x: f64) -> usize {
    let (tabs, _) = tab_layout(count, width);
    tabs.iter()
        .rposition(|(tab, _)| x >= tab.loc.x as f64)
        .unwrap_or(0)
}

/// The terminal button's menu, below it, for a sidebar `width` wide (window
/// coordinates; it may reach over the apps): the panel and one rect per item.
pub fn terminal_menu(width: i32) -> (Rect, Vec<Rect>) {
    let x = sidebar_layout(&[], &[], width).terminal.loc.x;
    let h = 2 * MENU_PADDING + MENU_ITEMS.len() as i32 * MENU_ITEM_HEIGHT;
    let panel = Rectangle::new((x, BAR_HEIGHT + 2).into(), (MENU_WIDTH, h).into());
    let items = (0..MENU_ITEMS.len() as i32)
        .map(|i| {
            let y = panel.loc.y + MENU_PADDING + i * MENU_ITEM_HEIGHT;
            Rectangle::new(
                (x + MENU_PADDING, y).into(),
                (MENU_WIDTH - 2 * MENU_PADDING, MENU_ITEM_HEIGHT).into(),
            )
        })
        .collect();
    (panel, items)
}

/// The switcher showing `rows` rows in the app `area` (window coordinates):
/// the panel, its query field and the rows.
pub fn switcher_layout(rows: usize, area: Rect) -> (Rect, Rect, Vec<Rect>) {
    let w = SWITCHER_WIDTH.min(area.size.w - 2 * HELP_MARGIN).max(1);
    switcher_parts(
        rows,
        (
            area.loc.x + (area.size.w - w) / 2,
            area.loc.y + SWITCHER_TOP,
        )
            .into(),
        w,
    )
}

/// How many rows the switcher shows at most in the app `area`.
pub fn switcher_page(area: Rect) -> usize {
    let room = area.size.h - SWITCHER_TOP - SWITCHER_INPUT - 2 * SWITCHER_PADDING - HELP_MARGIN;
    (room / SWITCHER_ROW).clamp(1, SWITCHER_ROWS) as usize
}

/// `switcher_layout` for a panel `w` wide at `loc`; it keeps room for one
/// row without any.
fn switcher_parts(rows: usize, loc: Point<i32, Logical>, w: i32) -> (Rect, Rect, Vec<Rect>) {
    let h = SWITCHER_INPUT + 2 * SWITCHER_PADDING + rows.max(1) as i32 * SWITCHER_ROW;
    let panel = Rectangle::new(loc, (w, h).into());
    let input = Rectangle::new(loc, (w, SWITCHER_INPUT).into());
    let rows = (0..rows as i32)
        .map(|i| {
            let y = loc.y + SWITCHER_INPUT + SWITCHER_PADDING + i * SWITCHER_ROW;
            Rectangle::new(
                (loc.x + SWITCHER_PADDING, y).into(),
                (w - 2 * SWITCHER_PADDING, SWITCHER_ROW).into(),
            )
        })
        .collect();
    (panel, input, rows)
}

/// Everything the chrome shows; a change in any of it triggers a redraw.
pub struct ChromeView {
    pub workspaces: Vec<WorkspaceView>,
    /// Per workspace, the agents running in it.
    pub agents: Vec<Vec<AgentView>>,
    pub active_workspace: Option<usize>,
    pub tabs: Vec<TabView>,
    pub active_tab: usize,
    /// Set while the terminal menu is open: the item under the pointer.
    pub menu: Option<Option<usize>>,
    /// Set while the switcher is open.
    pub switcher: Option<SwitcherView>,
    /// Set while waiting for the key after the prefix: the bindings to list.
    pub prefix: Option<std::sync::Arc<Help>>,
    /// Set while the welcome screen covers everything.
    pub welcome: Option<WelcomeView>,
    /// Set while the quit dialog is open, over everything.
    pub quit: Option<QuitView>,
    pub theme: Theme,
    pub sidebar_width: i32,
    pub size: Size<i32, Logical>,
    pub scale: f64,
}

/// Sidebar inputs: workspaces, their agents, the active one, whether the
/// terminal menu is open, theme, size, scale.
type SidebarKey = (
    Vec<WorkspaceView>,
    Vec<Vec<AgentView>>,
    Option<usize>,
    bool,
    Theme,
    Size<i32, Logical>,
    f64,
);

pub struct Chrome {
    painter: Painter,
    sidebar: Cached<SidebarKey>,
    tabs: Cached<(Vec<TabView>, usize, bool, Theme, i32, f64)>,
    help: Cached<(HelpLayout, Theme, f64)>,
    menu: Cached<(Option<usize>, Theme, f64)>,
    switcher: Cached<(SwitcherView, i32, Theme, f64)>,
    welcome: Cached<(WelcomeView, Size<i32, Logical>, Theme, f64)>,
    quit: Cached<(QuitView, Size<i32, Logical>, Theme, f64)>,
}

impl Chrome {
    pub fn new() -> Self {
        Self {
            painter: Painter::new(),
            sidebar: Cached::default(),
            tabs: Cached::default(),
            help: Cached::default(),
            menu: Cached::default(),
            switcher: Cached::default(),
            welcome: Cached::default(),
            quit: Cached::default(),
        }
    }

    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        view: ChromeView,
    ) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let scale = view.scale;
        let theme = view.theme;
        let sidebar_w = view.sidebar_width;
        let output: Size<i32, Logical> = (view.size.w.max(1), view.size.h.max(1)).into();
        let painter = &self.painter;

        // Front to back: the quit dialog covers everything, the welcome
        // screen all the rest.
        let mut parts = Vec::new();
        if let Some(quit) = view.quit {
            let panel = quit_layout(output).panel;
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self
                .quit
                .get((quit, output, theme, scale), |(quit, output, ..)| {
                    painter.quit_dialog(quit, *output, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
        if let Some(welcome) = view.welcome {
            let buffer = self
                .welcome
                .get((welcome, output, theme, scale), |(welcome, output, ..)| {
                    painter.welcome(welcome, *output, scale, theme.palette())
                });
            parts.push((buffer, Point::from((0, 0)), output));
            return render_parts(renderer, parts, scale);
        }

        let sidebar_size: Size<i32, Logical> = (sidebar_w, view.size.h.max(1)).into();
        let strip_size: Size<i32, Logical> =
            ((view.size.w - sidebar_w).max(1), TAB_STRIP_HEIGHT).into();
        let menu_open = view.menu.is_some();
        let sidebar_key = (
            view.workspaces,
            view.agents,
            view.active_workspace,
            menu_open,
            theme,
            sidebar_size,
            scale,
        );
        let sidebar = self.sidebar.get(
            sidebar_key,
            |(workspaces, agents, active, menu_open, ..)| {
                painter.sidebar(
                    workspaces,
                    agents,
                    *active,
                    *menu_open,
                    sidebar_size,
                    scale,
                    theme.palette(),
                )
            },
        );
        let strip = self.tabs.get(
            (
                view.tabs,
                view.active_tab,
                view.prefix.is_some(),
                theme,
                strip_size.w,
                scale,
            ),
            |(tabs, active, prefix, ..)| {
                painter.tab_strip(tabs, *active, *prefix, theme, strip_size, scale)
            },
        );

        // Then the switcher, the menu and the help panel cover the apps.
        if let Some(switcher) = view.switcher {
            let area = Rectangle::new(
                (sidebar_w, TAB_STRIP_HEIGHT).into(),
                (strip_size.w, (view.size.h - TAB_STRIP_HEIGHT).max(1)).into(),
            );
            let (panel, _, _) = switcher_layout(switcher.rows.len(), area);
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self.switcher.get(
                (switcher, panel.size.w, theme, scale),
                |(switcher, w, ..)| painter.switcher(switcher, *w, scale, theme.palette()),
            );
            parts.push((buffer, loc, size));
        }
        if let Some(hover) = view.menu {
            let (panel, _) = terminal_menu(sidebar_w);
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let menu = self.menu.get((hover, theme, scale), |(hover, _, _)| {
                painter.menu(*hover, scale, theme.palette())
            });
            parts.push((menu, loc, size));
        }
        if let Some(help) = view.prefix.as_deref() {
            // Centred at the bottom of the app area.
            let (area_w, area_h) = (view.size.w - sidebar_w, view.size.h - TAB_STRIP_HEIGHT);
            let layout = painter.help_layout(help, area_w - 2 * HELP_MARGIN, scale);
            // The buffer reaches past the panel by the shadow on every side.
            let loc = Point::from((
                sidebar_w + (area_w - layout.size.w) / 2 - SHADOW,
                (TAB_STRIP_HEIGHT + area_h - layout.size.h - HELP_MARGIN).max(TAB_STRIP_HEIGHT)
                    - SHADOW,
            ));
            let size = (layout.size.w + 2 * SHADOW, layout.size.h + 2 * SHADOW).into();
            let panel = self.help.get((layout, theme, scale), |(layout, _, _)| {
                painter.help_panel(help, layout, scale, theme.palette())
            });
            parts.push((panel, loc, size));
        }
        parts.push((sidebar, Point::from((0, 0)), sidebar_size));
        parts.push((strip, Point::from((sidebar_w, 0)), strip_size));
        render_parts(renderer, parts, scale)
    }
}

/// A drawn buffer, where it goes on the output and the logical size it covers.
type Part<'a> = (
    &'a MemoryRenderBuffer,
    Point<i32, Logical>,
    Size<i32, Logical>,
);

/// Render elements for buffers of physical pixels, each covering a logical
/// size at a logical place on the output.
fn render_parts(
    renderer: &mut GlesRenderer,
    parts: Vec<Part<'_>>,
    scale: f64,
) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
    parts
        .into_iter()
        .filter_map(|(buffer, loc, size)| {
            // The buffer holds physical pixels at buffer scale 1: sample all of it
            // and squeeze it into the logical size.
            let src = Rectangle::from_size(physical(size, scale).to_f64());
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc.to_f64().to_physical(scale),
                buffer,
                None,
                Some(src),
                Some(size),
                Kind::Unspecified,
            )
            .ok()
        })
        .collect()
}

/// Help panel geometry, in logical pixels.
#[derive(PartialEq)]
struct HelpLayout {
    size: Size<i32, Logical>,
    rows: i32,
    key_width: i32,
    column_width: i32,
}

/// A buffer plus the inputs it was drawn from.
struct Cached<K> {
    key: Option<K>,
    buffer: Option<MemoryRenderBuffer>,
}

impl<K> Default for Cached<K> {
    fn default() -> Self {
        Self {
            key: None,
            buffer: None,
        }
    }
}

impl<K: PartialEq> Cached<K> {
    fn get(&mut self, key: K, draw: impl FnOnce(&K) -> MemoryRenderBuffer) -> &MemoryRenderBuffer {
        if self.key.as_ref() != Some(&key) || self.buffer.is_none() {
            self.buffer = Some(draw(&key));
            self.key = Some(key);
        }
        self.buffer.as_ref().unwrap()
    }
}

struct Painter {
    font: Option<FontVec>,
}

impl Painter {
    fn new() -> Self {
        let font = find_font().and_then(|path| {
            let data = std::fs::read(&path).ok()?;
            FontVec::try_from_vec(data).ok()
        });
        if font.is_none() {
            tracing::warn!("chrome: no usable font found, labels will be blank");
        }
        Self { font }
    }

    #[allow(clippy::too_many_arguments)]
    fn sidebar(
        &self,
        workspaces: &[WorkspaceView],
        agents: &[Vec<AgentView>],
        active: Option<usize>,
        menu_open: bool,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let line = px(1).max(1);
        c.fill(0, 0, c.w, c.h, p.panel);
        c.fill(0, px(BAR_HEIGHT) - line, c.w, line, p.border);
        c.fill(c.w - px(GAP).max(1), 0, px(GAP).max(1), c.h, p.border);

        let layout = sidebar_layout(workspaces, agents, size.w);
        let s = scale as f32;
        let header_x = px(ROW_INSET) as f32 + PADDING * s;
        self.text(
            &mut c,
            "Workspaces",
            SMALL_FONT_SIZE,
            header_x,
            px(layout.project.loc.x) as f32,
            (0, px(BAR_HEIGHT)),
            scale,
            p.muted,
        );
        if menu_open {
            let chevron = layout.chevron;
            let (x, w) = (px(chevron.loc.x), px(chevron.size.w));
            c.rounded(
                x as f32,
                px(5) as f32,
                w as f32,
                px(BAR_HEIGHT - 10) as f32,
                4.0 * s,
                p.selected,
            );
        }
        icon_sdf(&mut c, layout.project, scale, p.muted, |x, y| {
            folder_plus(x, y, s)
        });
        icon_sdf(&mut c, layout.terminal, scale, p.muted, |x, y| {
            terminal_plus(x, y, s)
        });
        icon_sdf(&mut c, layout.chevron, scale, p.muted, |x, y| {
            chevron_down(x, y, s)
        });
        // A thin line splits the terminal button from its chevron.
        c.fill(
            px(layout.chevron.loc.x),
            px(10),
            line,
            px(BAR_HEIGHT - 20),
            p.border,
        );

        let name_x = px(ROW_INSET) as f32
            + PADDING * s
            + ((ROW_ICON_WIDTH + ROW_ICON_GAP) as f32 * s).round();
        // Where the name starts, in logical pixels, for the icons lined up with it.
        let name_lx = ROW_INSET + PADDING as i32 + ROW_ICON_WIDTH + ROW_ICON_GAP;
        for (i, ((row, close), ws)) in layout.rows.iter().zip(workspaces).enumerate() {
            let is_active = Some(i) == active;
            let (y, h) = (px(row.loc.y), px(row.loc.y + row.size.h) - px(row.loc.y));
            if is_active {
                let (x, w) = (px(ROW_INSET), px(row.size.w - 2 * ROW_INSET));
                let (inset_y, inset_h) = (y + px(2), h - 2 * px(2));
                c.rounded(
                    x as f32,
                    inset_y as f32,
                    w as f32,
                    inset_h as f32,
                    ROW_RADIUS * scale as f32,
                    p.selected,
                );
            }
            let color = if is_active { p.text } else { p.muted };
            let title = (y, px(row.loc.y + close.size.h) - y);
            let icon = Rectangle::new(
                (ROW_INSET + PADDING as i32, row.loc.y).into(),
                (ROW_ICON_WIDTH, close.size.h).into(),
            );
            let small = 0.8 * s;
            match ws.kind {
                WorkspaceKind::Project => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    folder(x, y, small)
                }),
                WorkspaceKind::Terminal => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    terminal(x, y, small)
                }),
            }
            let mut ws_name_x = name_x;
            if let Some(status) = ws.status {
                let mark = Rectangle::new(
                    (name_lx, row.loc.y).into(),
                    (STATUS_WIDTH, close.size.h).into(),
                );
                icon_sdf(&mut c, mark, scale, p.status(status), |x, y| {
                    status_mark(status, x, y, s)
                });
                ws_name_x += ((STATUS_WIDTH + STATUS_GAP) as f32 * s).round();
            }
            self.text(
                &mut c,
                &ws.name,
                LABEL_FONT_SIZE,
                ws_name_x,
                px(close.loc.x) as f32,
                title,
                scale,
                color,
            );
            self.icon(&mut c, '×', *close, scale, p.placeholder);

            // Details, as on a tab: Git branch and folders, then what runs.
            let right = px(row.loc.x + row.size.w - ROW_INSET) as f32 - PADDING * s;
            let mut line_y = row.loc.y + close.size.h;
            if ws.branch.is_some() || !ws.place.is_empty() {
                let band = (px(line_y), px(line_y + TAB_LINE) - px(line_y));
                let mut place_x = name_x;
                if let Some(branch) = &ws.branch {
                    let icon = Rectangle::new(
                        (name_lx, line_y).into(),
                        (BRANCH_ICON_WIDTH, TAB_LINE).into(),
                    );
                    icon_sdf(&mut c, icon, scale, p.accent, |x, y| {
                        git_branch(x, y, 0.75 * s)
                    });
                    let branch_x =
                        name_x + ((BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32 * s).round();
                    place_x = self.text(
                        &mut c,
                        branch,
                        DETAIL_FONT_SIZE,
                        branch_x,
                        right,
                        band,
                        scale,
                        p.accent,
                    );
                    if let Some(git) = &ws.git {
                        place_x = self.git_marks(&mut c, git, place_x, right, band, scale, p);
                    }
                    if !ws.place.is_empty() {
                        place_x = self.text(
                            &mut c,
                            " · ",
                            DETAIL_FONT_SIZE,
                            place_x,
                            right,
                            band,
                            scale,
                            p.placeholder,
                        );
                    }
                }
                self.text(
                    &mut c,
                    &ws.place,
                    DETAIL_FONT_SIZE,
                    place_x,
                    right,
                    band,
                    scale,
                    p.muted,
                );
                line_y += TAB_LINE;
            }
            if !ws.activity.is_empty() {
                let band = (px(line_y), px(line_y + TAB_LINE) - px(line_y));
                self.text(
                    &mut c,
                    &ws.activity,
                    DETAIL_FONT_SIZE,
                    name_x,
                    right,
                    band,
                    scale,
                    color,
                );
            }
        }
        for (rects, cards) in layout.cards.iter().zip(agents) {
            for (rect, card) in rects.iter().zip(cards) {
                self.agent_card(&mut c, card, *rect, scale, p);
            }
        }
        c.into_buffer(true)
    }

    /// An agent card: a status mark, the agent's kind and name and a status
    /// pill, then a line per detail. A stripe down its left edge carries the
    /// status colour.
    fn agent_card(&self, c: &mut Canvas, card: &AgentView, rect: Rect, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = (
            px(rect.loc.x),
            px(rect.loc.y),
            px(rect.loc.x + rect.size.w) - px(rect.loc.x),
            px(rect.size.h),
        );
        let (b, r) = (px(1).max(1) as f32, CARD_RADIUS * s);
        let status = p.status(card.status);
        let fill = mix(p.panel, p.element, 0.6);
        let edge = if card.status == AgentStatus::Blocked {
            mix(p.border_variant, status, 0.6)
        } else {
            p.border_variant
        };
        c.rounded(x as f32, y as f32, w as f32, h as f32, r, edge);
        c.rounded(
            x as f32 + b,
            y as f32 + b,
            w as f32 - 2.0 * b,
            h as f32 - 2.0 * b,
            r - b,
            fill,
        );
        c.rounded(
            x as f32 + 3.0 * s,
            (y + px(CARD_PAD)) as f32,
            2.0 * s,
            (h - 2 * px(CARD_PAD)) as f32,
            s,
            status,
        );

        // Header: status mark, kind and name, status pill.
        let inner_x = rect.loc.x + CARD_PAD;
        let right = px(rect.loc.x + rect.size.w - CARD_PAD) as f32;
        let header_y = rect.loc.y + CARD_PAD;
        let band = (px(header_y), px(CARD_HEADER));
        let mark = Rectangle::new(
            (inner_x, header_y).into(),
            (STATUS_WIDTH, CARD_HEADER).into(),
        );
        icon_sdf(c, mark, scale, status, |x, y| {
            status_mark(card.status, x, y, s)
        });
        let label = status_label(card.status);
        let pill_w = self.text_width(label, TINY_FONT_SIZE, scale) + 2.0 * PILL_PADDING as f32 * s;
        let pill_x = (right - pill_w).round();
        let pill_y = (band.0 as f32 + (band.1 - px(PILL_HEIGHT)) as f32 / 2.0).round();
        let pill_h = px(PILL_HEIGHT) as f32;
        c.rounded(
            pill_x,
            pill_y,
            pill_w.round(),
            pill_h,
            pill_h / 2.0,
            mix(fill, status, 0.2),
        );
        self.text(
            c,
            label,
            TINY_FONT_SIZE,
            pill_x + PILL_PADDING as f32 * s,
            f32::MAX,
            (pill_y as i32, pill_h as i32),
            scale,
            status,
        );
        let kind_x = px(inner_x + STATUS_WIDTH + STATUS_GAP) as f32;
        let heading_end = pill_x - px(6) as f32;
        let after = self.text(
            c,
            &card.kind,
            FONT_SIZE,
            kind_x,
            heading_end,
            band,
            scale,
            p.text,
        );
        if let Some(name) = &card.name {
            self.text(
                c,
                &format!("· {name}"),
                FONT_SIZE,
                after + px(5) as f32,
                heading_end,
                band,
                scale,
                p.accent,
            );
        }

        // Details: labels in one column, values lined up after the widest.
        let text_x = kind_x;
        let label_w = card
            .lines
            .iter()
            .filter_map(|l| match l {
                AgentLine::Field(label, _) => Some(self.text_width(label, DETAIL_FONT_SIZE, scale)),
                _ => None,
            })
            .fold(0.0, f32::max)
            .min((right - text_x) * 0.4);
        let value_x = (text_x + label_w + FIELD_GAP as f32 * s).round();
        for (i, line) in card.lines.iter().enumerate() {
            let band = (
                px(header_y + CARD_HEADER + i as i32 * CARD_LINE),
                px(CARD_LINE),
            );
            match line {
                AgentLine::Title(title) => {
                    self.text(
                        c,
                        title,
                        DETAIL_FONT_SIZE,
                        text_x,
                        right,
                        band,
                        scale,
                        p.text,
                    );
                }
                AgentLine::Field(label, value) => {
                    self.text(
                        c,
                        label,
                        DETAIL_FONT_SIZE,
                        text_x,
                        value_x - px(4) as f32,
                        band,
                        scale,
                        p.placeholder,
                    );
                    self.text(
                        c,
                        value,
                        DETAIL_FONT_SIZE,
                        value_x,
                        right,
                        band,
                        scale,
                        p.muted,
                    );
                }
            }
        }
    }

    /// The tab strip: per tab a title row (status mark, number, title, "×"),
    /// then what runs.
    fn tab_strip(
        &self,
        views: &[TabView],
        active: usize,
        prefix: bool,
        theme: Theme,
        size: Size<i32, Logical>,
        scale: f64,
    ) -> MemoryRenderBuffer {
        let p = theme.palette();
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let pad = PADDING * s;
        let line = px(1).max(1);
        c.fill(0, 0, c.w, c.h, p.tab_bar);
        c.fill(0, c.h - line, c.w, line, p.border);

        let (tabs, plus) = tab_layout(views.len(), size.w);
        for (i, ((rect, close), view)) in tabs.iter().zip(views).enumerate() {
            let (x, w) = (
                px(rect.loc.x),
                px(rect.loc.x + rect.size.w) - px(rect.loc.x),
            );
            let is_active = i == active;
            // Like Zed: the active tab opens into the editor below it.
            if is_active {
                c.fill(x, 0, w, c.h, p.tab_active);
            }
            c.fill(x + w, 0, line, c.h, p.border);

            let band = (0, px(TAB_TITLE_HEIGHT));
            let max_x = px(close.loc.x) as f32;
            let mut title_x = x as f32 + pad;
            if let Some(status) = view.status {
                let mark = Rectangle::new(
                    (rect.loc.x + PADDING as i32, 0).into(),
                    (STATUS_WIDTH, TAB_TITLE_HEIGHT).into(),
                );
                icon_sdf(&mut c, mark, scale, p.status(status), |x, y| {
                    status_mark(status, x, y, s)
                });
                title_x += ((STATUS_WIDTH + STATUS_GAP) as f32 * s).round();
            }
            let number = (i + 1).to_string();
            let after = self.text(
                &mut c,
                &number,
                LABEL_FONT_SIZE,
                title_x,
                max_x,
                band,
                scale,
                p.placeholder,
            );
            self.text(
                &mut c,
                &view.title,
                LABEL_FONT_SIZE,
                after + px(8) as f32,
                max_x,
                band,
                scale,
                if is_active { p.text } else { p.muted },
            );
            self.icon(&mut c, '×', *close, scale, p.placeholder);

            // What runs in the tab, on a line of its own, cut off at the tab's
            // edge, or before the mark saying it won't run again after a restart.
            let (text_x, mut right) = (x as f32 + pad, (x + w) as f32 - pad);
            let band = (px(TAB_TITLE_HEIGHT), px(TAB_LINE));
            if view.restart == Some(false) {
                let mark = Rectangle::new(
                    (
                        rect.loc.x + rect.size.w - PADDING as i32 - STATUS_WIDTH,
                        TAB_TITLE_HEIGHT,
                    )
                        .into(),
                    (STATUS_WIDTH, TAB_LINE).into(),
                );
                icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                    no_restart(x, y, s)
                });
                right = px(mark.loc.x - 4) as f32;
            }
            self.text(
                &mut c,
                &view.activity,
                DETAIL_FONT_SIZE,
                text_x,
                right,
                band,
                scale,
                p.text,
            );
        }
        if !views.is_empty() {
            // A soft rounded square with a large "+", centred in the strip.
            let side = px(NEW_TAB_SIZE) as f32;
            let (bx, by) = (
                px(plus.loc.x) as f32 + (px(plus.size.w) as f32 - side) / 2.0,
                (px(plus.size.h) as f32 - side) / 2.0,
            );
            c.rounded(bx.round(), by.round(), side, side, 6.0 * s, p.element);
            icon_sdf(&mut c, plus, scale, p.text, |x, y| new_tab(x, y, s));
        }

        let toggle = theme_toggle(size.w);
        // Show where a click goes: the moon in light mode, the sun in dark.
        match theme {
            Theme::Dark => icon_sdf(&mut c, toggle, scale, p.muted, |x, y| sun(x, y, s)),
            Theme::Light => icon_sdf(&mut c, toggle, scale, p.muted, |x, y| moon(x, y, s)),
        }

        if prefix {
            // Badge left of the theme switch, over any tabs reaching that far.
            let width = (self.text_width("PREFIX", SMALL_FONT_SIZE, scale) / scale as f32) as i32
                + 2 * PADDING as i32;
            let badge = Rectangle::new(
                (toggle.loc.x - width - 4, 5).into(),
                (width, TAB_TITLE_HEIGHT - 10).into(),
            );
            c.fill(
                px(badge.loc.x - 6),
                0,
                px(width + 10),
                c.h - line,
                p.tab_bar,
            );
            let (bx, by, bw, bh) = (
                px(badge.loc.x),
                px(badge.loc.y),
                px(badge.size.w),
                px(badge.size.h),
            );
            c.rounded(
                bx as f32,
                by as f32,
                bw as f32,
                bh as f32,
                bh as f32 / 2.0,
                p.accent,
            );
            self.centered(&mut c, "PREFIX", SMALL_FONT_SIZE, badge, scale, p.on_accent);
        }
        c.into_buffer(true)
    }

    /// Size the help panel: two columns when they fit in `max_width`, else one.
    fn help_layout(&self, help: &Help, max_width: i32, scale: f64) -> HelpLayout {
        let widest = |widths: &mut dyn Iterator<Item = f32>| {
            // Measured at the output scale, where glyph advances are rounded differently.
            widths
                .map(|w| (w / scale as f32).ceil() as i32 + 1)
                .max()
                .unwrap_or(0)
        };
        let key_width = widest(
            &mut help
                .rows
                .iter()
                .map(|(keys, _)| self.keycaps_width(keys, scale)),
        );
        let action_width = widest(
            &mut help
                .rows
                .iter()
                .map(|(_, action)| self.text_width(action, FONT_SIZE, scale)),
        );
        let column_width = key_width + HELP_KEY_GAP + action_width;
        let columns = if 2 * column_width + HELP_COLUMN_GAP + 2 * HELP_PADDING <= max_width {
            2
        } else {
            1
        };
        let rows = help.rows.len().div_ceil(columns) as i32;
        let w = columns as i32 * column_width
            + (columns as i32 - 1) * HELP_COLUMN_GAP
            + 2 * HELP_PADDING;
        let h = HELP_TITLE_HEIGHT + rows * HELP_ROW_HEIGHT + HELP_PADDING;
        HelpLayout {
            size: (w, h).into(),
            rows,
            key_width,
            column_width,
        }
    }

    /// The panel inside a margin of `SHADOW` holding its drop shadow.
    fn help_panel(
        &self,
        help: &Help,
        layout: &HelpLayout,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let outer = Size::from((layout.size.w + 2 * SHADOW, layout.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);
        let (ox, oy) = (px(SHADOW), px(SHADOW));
        c.popup(layout.size, HELP_RADIUS, scale, p);

        let (pad, max_x) = (ox + px(HELP_PADDING), ox + px(layout.size.w - HELP_PADDING));
        let title_band = (
            oy + px(HELP_PADDING / 2),
            px(HELP_TITLE_HEIGHT - HELP_PADDING / 2),
        );
        let after = self.keycaps(&mut c, &help.prefix, pad as f32, title_band, scale, p);
        self.text(
            &mut c,
            "then press a key",
            SMALL_FONT_SIZE,
            after + px(10) as f32,
            max_x as f32,
            title_band,
            scale,
            p.muted,
        );
        c.fill(
            ox + border,
            oy + px(HELP_TITLE_HEIGHT) - border,
            px(layout.size.w) - 2 * border,
            border,
            p.border_variant,
        );

        for (i, (keys, action)) in help.rows.iter().enumerate() {
            let (column, row) = (i as i32 / layout.rows, i as i32 % layout.rows);
            let x = SHADOW + HELP_PADDING + column * (layout.column_width + HELP_COLUMN_GAP);
            let y = SHADOW + HELP_TITLE_HEIGHT + row * HELP_ROW_HEIGHT;
            let band = (px(y), px(HELP_ROW_HEIGHT));
            let column_end = px(x + layout.column_width) as f32;
            self.keycaps(&mut c, keys, px(x) as f32, band, scale, p);
            let action_x = px(x + layout.key_width + HELP_KEY_GAP) as f32;
            self.text(
                &mut c, action, FONT_SIZE, action_x, column_end, band, scale, p.text,
            );
        }
        c.into_buffer(false)
    }

    /// The terminal button's menu inside a margin of `SHADOW` holding its drop
    /// shadow, `hover` item highlighted.
    fn menu(&self, hover: Option<usize>, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let (panel, items) = terminal_menu(0);
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        c.popup(panel.size, MENU_RADIUS, scale, p);
        // Item rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        for (i, (item, label)) in items.iter().zip(MENU_ITEMS).enumerate() {
            let item = Rectangle::new(item.loc + shift, item.size);
            let (x, y, w, h) = (
                px(item.loc.x),
                px(item.loc.y),
                px(item.size.w),
                px(item.size.h),
            );
            if hover == Some(i) {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * scale as f32,
                    p.selected,
                );
            }
            let text_x = x as f32 + PADDING * scale as f32;
            self.text(
                &mut c,
                label,
                FONT_SIZE,
                text_x,
                (x + w) as f32,
                (y, h),
                scale,
                p.text,
            );
        }
        c.into_buffer(false)
    }

    /// The switcher `w` wide inside a margin of `SHADOW` holding its drop
    /// shadow: the query (or what to type) with a caret, then the rows.
    fn switcher(&self, view: &SwitcherView, w: i32, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let (panel, input, slots) = switcher_parts(view.rows.len(), (SHADOW, SHADOW).into(), w);
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        c.popup(panel.size, HELP_RADIUS, scale, p);

        let band = (px(input.loc.y), px(input.size.h));
        let (text_x, max_x) = (
            px(input.loc.x + HELP_PADDING) as f32,
            px(input.loc.x + input.size.w - HELP_PADDING) as f32,
        );
        let caret_x = if view.query.is_empty() {
            self.text(
                &mut c,
                view.hint,
                FONT_SIZE,
                text_x + px(CARET_WIDTH + 2) as f32,
                max_x,
                band,
                scale,
                p.placeholder,
            );
            text_x
        } else {
            self.text(
                &mut c,
                &view.query,
                FONT_SIZE,
                text_x,
                max_x,
                band,
                scale,
                p.text,
            ) + px(1) as f32
        };
        let caret_h = px(CARET_HEIGHT);
        c.fill(
            caret_x.round() as i32,
            band.0 + (band.1 - caret_h) / 2,
            px(CARET_WIDTH).max(1),
            caret_h,
            p.accent,
        );
        let line_y = px(input.loc.y + input.size.h) - border;
        c.fill(
            px(panel.loc.x) + border,
            line_y,
            px(panel.size.w) - 2 * border,
            border,
            p.border_variant,
        );

        if view.rows.is_empty() {
            let (_, _, slots) = switcher_parts(1, (SHADOW, SHADOW).into(), w);
            let band = (px(slots[0].loc.y), px(slots[0].size.h));
            self.text(
                &mut c,
                "Nothing matches",
                FONT_SIZE,
                px(slots[0].loc.x) as f32 + PADDING * s,
                max_x,
                band,
                scale,
                p.placeholder,
            );
        }
        for (i, (slot, row)) in slots.iter().zip(&view.rows).enumerate() {
            let (x, y, w, h) = (
                px(slot.loc.x),
                px(slot.loc.y),
                px(slot.size.w),
                px(slot.size.h),
            );
            if view.selected == Some(i) {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * s,
                    p.selected,
                );
            }
            let icon = Rectangle::new(
                (slot.loc.x + PADDING as i32, slot.loc.y).into(),
                (ROW_ICON_WIDTH, slot.size.h).into(),
            );
            let small = 0.8 * s;
            match &row.icon {
                ItemIcon::Workspace(WorkspaceKind::Project) => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                        folder(x, y, small)
                    })
                }
                ItemIcon::Workspace(WorkspaceKind::Terminal) => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                        terminal(x, y, small)
                    })
                }
                ItemIcon::Tab => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| tab(x, y, small))
                }
                &ItemIcon::Agent(status) => {
                    icon_sdf(&mut c, icon, scale, p.status(status), |x, y| {
                        status_mark(status, x, y, s)
                    })
                }
                ItemIcon::Folder => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    folder_plus(x, y, small)
                }),
                ItemIcon::App => {
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| app(x, y, small))
                }
                ItemIcon::Picture(picture) => c.picture(picture, icon, scale, p.placeholder),
            }
            let band = (y, h);
            let right = (x + w) as f32 - PADDING * s;
            let label_x = px(icon.loc.x + ROW_ICON_WIDTH + ROW_ICON_GAP) as f32;
            let end = self.marked(
                &mut c,
                &row.label,
                &row.label_hits,
                FONT_SIZE,
                label_x,
                right,
                band,
                scale,
                (p.text, p.accent),
            );
            let detail_x = end + (FIELD_GAP as f32 * s).round();
            self.marked(
                &mut c,
                &row.detail,
                &row.detail_hits,
                SMALL_FONT_SIZE,
                detail_x,
                right,
                band,
                scale,
                (p.muted, p.accent),
            );
        }
        c.into_buffer(false)
    }

    /// The welcome screen over the whole output of `size`, opaque in the
    /// editor's background: the greeting, the projects, the button (or the
    /// name field while naming), a note and the key hints.
    fn welcome(
        &self,
        view: &WelcomeView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        c.fill(0, 0, c.w, c.h, p.tab_active);
        let layout = welcome_layout(size, view.rows.len(), view.naming.is_some());

        self.centered(
            &mut c,
            &view.greeting,
            GREETING_FONT_SIZE,
            layout.greeting,
            scale,
            p.text,
        );
        self.centered(
            &mut c,
            &view.subtitle,
            FONT_SIZE,
            layout.subtitle,
            scale,
            p.muted,
        );

        // The label over the list, or what stands in for the list without
        // rows; the filter typed so far, with its caret, at its right.
        let (label_x, right) = phys(layout.label, scale);
        let (text_x, right) = (label_x + PADDING * s, right - PADDING * s);
        let label_band = band(layout.label, scale);
        let label_end = if view.rows.is_empty() {
            let empty = if view.query.is_empty() {
                "No projects yet"
            } else {
                "Nothing matches"
            };
            self.text(
                &mut c,
                empty,
                FONT_SIZE,
                text_x,
                right,
                label_band,
                scale,
                p.placeholder,
            )
        } else {
            self.text(
                &mut c,
                "RECENT PROJECTS",
                SMALL_FONT_SIZE,
                text_x,
                right,
                label_band,
                scale,
                p.placeholder,
            )
        };
        if !view.query.is_empty() && view.naming.is_none() {
            let end = right - (px(CARET_WIDTH).max(1) + px(1)) as f32;
            let width = self.text_width(&view.query, FONT_SIZE, scale);
            let start = (end - width).max(label_end + px(16) as f32);
            let after = self.text(
                &mut c,
                &view.query,
                FONT_SIZE,
                start,
                end,
                label_band,
                scale,
                p.text,
            );
            caret(&mut c, after + px(1) as f32, label_band, scale, p.accent);
        }

        for (i, (rect, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            self.project_row(&mut c, view, i, row, *rect, scale, p);
        }

        let button = layout.button;
        match &view.naming {
            Some(name) => self.field(&mut c, name, "Project name", button, scale, p),
            None => {
                let (x, y, w, h) = rect_px(button, scale);
                if view.selected.is_none() {
                    focus_ring(&mut c, button, BUTTON_RADIUS, p.tab_active, scale, p.accent);
                }
                let fill = if view.hover == Some(WelcomeHit::Button) {
                    mix(p.accent, p.text, 0.12)
                } else {
                    p.accent
                };
                c.rounded(x, y, w, h, BUTTON_RADIUS * s, fill);
                // A "+" and the label, centred together.
                let label = "New project";
                let (arm, gap) = ((5.0 * s).round(), (8.0 * s).round());
                let content = 2.0 * arm + gap + self.text_width(label, FONT_SIZE, scale);
                let start = (x + (w - content) / 2.0).round();
                let (cx, cy) = (start + arm, y + h / 2.0);
                let reach = (arm + 2.0 * s).ceil() as i32;
                c.paint_in(
                    cx as i32 - reach,
                    cy as i32 - reach,
                    2 * reach + 1,
                    2 * reach + 1,
                    p.on_accent,
                    |x, y| coverage(plus(x, y, cx, cy, arm, s) - 0.2 * s),
                );
                self.text(
                    &mut c,
                    label,
                    FONT_SIZE,
                    start + 2.0 * arm + gap,
                    x + w,
                    band(button, scale),
                    scale,
                    p.on_accent,
                );
            }
        }

        if let Some(note) = &view.note {
            self.centered(&mut c, note, SMALL_FONT_SIZE, layout.note, scale, p.warning);
        }

        let hints: &[(&str, &str)] = if view.naming.is_some() {
            &[("Enter", "create"), ("Esc", "cancel")]
        } else {
            &[
                ("Up / Down", "select"),
                ("Enter", "open"),
                ("Ctrl+N", "new"),
                ("Del", "forget"),
                ("Esc", "back"),
            ]
        };
        self.hints(&mut c, hints, layout.hints, scale, p);
        c.into_buffer(true)
    }

    /// Row `i` of the welcome screen, in `rect`: a mark for whether its
    /// session runs, its name, its detail and how long ago it was used; or
    /// the question whether to forget it.
    #[allow(clippy::too_many_arguments)]
    fn project_row(
        &self,
        c: &mut Canvas,
        view: &WelcomeView,
        i: usize,
        row: &ProjectRow,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        // The highlight stops short of the rows above and below.
        let (y, h) = (y + px(2) as f32, h - 2.0 * px(2) as f32);
        let r = ROW_RADIUS * s;
        let row_band = band(rect, scale);
        let mark = Rectangle::new(
            (rect.loc.x + PADDING as i32, rect.loc.y).into(),
            (STATUS_WIDTH, rect.size.h).into(),
        );
        let name_x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let right = x + w - PADDING * s;
        let dot = move |x: f32, y: f32| x.hypot(y) - 4.2 * s;
        if view.confirm_forget == Some(i) {
            c.rounded(x, y, w, h, r, mix(p.tab_active, p.warning, 0.15));
            icon_sdf(c, mark, scale, p.warning, dot);
            self.text(
                c,
                &format!("Press Delete again to forget {}", row.name),
                FONT_SIZE,
                name_x,
                right,
                row_band,
                scale,
                p.warning,
            );
            return;
        }
        if view.selected == Some(i) {
            c.rounded(x, y, w, h, r, p.selected);
        } else if view.hover == Some(WelcomeHit::Row(i)) {
            c.rounded(x, y, w, h, r, mix(p.tab_active, p.selected, 0.5));
        }

        // A dot while its session runs, a ring while it doesn't, a fainter
        // ring until that is known.
        let ring = move |x: f32, y: f32| (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
        match row.running {
            Some(true) => icon_sdf(c, mark, scale, p.success, dot),
            Some(false) => icon_sdf(c, mark, scale, p.placeholder, ring),
            None => icon_sdf(c, mark, scale, p.border, ring),
        }

        let ago_x = (right - self.text_width(&row.ago, SMALL_FONT_SIZE, scale)).round();
        self.text(
            c,
            &row.ago,
            SMALL_FONT_SIZE,
            ago_x,
            f32::MAX,
            row_band,
            scale,
            p.placeholder,
        );
        let gap = (FIELD_GAP as f32 * s).round();
        let end = ago_x - gap;
        let mut after = self.marked(
            c,
            &row.name,
            &[],
            LABEL_FONT_SIZE,
            name_x,
            end,
            row_band,
            scale,
            (p.text, p.text),
        );
        if row.current {
            after = self.marked(
                c,
                "current",
                &[],
                SMALL_FONT_SIZE,
                after + gap,
                end,
                row_band,
                scale,
                (p.accent, p.accent),
            );
        }
        self.marked(
            c,
            &row.detail,
            &[],
            SMALL_FONT_SIZE,
            after + (12.0 * s).round(),
            end,
            row_band,
            scale,
            (p.muted, p.muted),
        );
    }

    /// A text field in `rect`, like the switcher's query: `text` (or
    /// `placeholder` without any) and a caret after it.
    fn field(
        &self,
        c: &mut Canvas,
        text: &str,
        placeholder: &str,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        let field_band = band(rect, scale);
        let text_x = x + PADDING * s;
        let max_x = x + w - PADDING * s - px(CARET_WIDTH + 1) as f32;
        let caret_x = if text.is_empty() {
            self.text(
                c,
                placeholder,
                FONT_SIZE,
                text_x + px(CARET_WIDTH + 2) as f32,
                max_x,
                field_band,
                scale,
                p.placeholder,
            );
            text_x
        } else {
            self.text(c, text, FONT_SIZE, text_x, max_x, field_band, scale, p.text) + px(1) as f32
        };
        caret(c, caret_x, field_band, scale, p.accent);
    }

    /// Key hints as keycaps, each followed by what it does, centred in `rect`.
    fn hints(&self, c: &mut Canvas, hints: &[(&str, &str)], rect: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let key_gap = (HINT_KEY_GAP as f32 * s).round();
        let gap = (HINT_GAP as f32 * s).round();
        let width = hints
            .iter()
            .map(|(keys, action)| {
                self.keycaps_width(keys, scale)
                    + key_gap
                    + self.text_width(action, SMALL_FONT_SIZE, scale)
            })
            .sum::<f32>()
            + gap * hints.len().saturating_sub(1) as f32;
        let (x, _, w, _) = rect_px(rect, scale);
        let hints_band = band(rect, scale);
        let mut pen = (x + (w - width) / 2.0).round();
        for (keys, action) in hints {
            let end = self.keycaps(c, keys, pen, hints_band, scale, p);
            pen = self.text(
                c,
                action,
                SMALL_FONT_SIZE,
                end + key_gap,
                f32::MAX,
                hints_band,
                scale,
                p.muted,
            ) + gap;
        }
    }

    /// The quit dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow: the question, the "Don't ask again"
    /// checkbox, and Cancel, Stop project and Keep running.
    fn quit_dialog(
        &self,
        view: &QuitView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = quit_layout(size);
        let panel = layout.panel;
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        c.popup(panel.size, HELP_RADIUS, scale, p);
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };

        let title = shift(layout.title);
        let (title_x, title_end) = phys(title, scale);
        self.text(
            &mut c,
            "Quit gerdr?",
            LABEL_FONT_SIZE,
            title_x,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        let body = shift(layout.body);
        let (body_x, body_end) = phys(body, scale);
        self.text(
            &mut c,
            &format!(
                "Keep \u{201c}{}\u{201d} running in the background?",
                view.project
            ),
            FONT_SIZE,
            body_x,
            body_end,
            band(body, scale),
            scale,
            p.muted,
        );

        // The checkbox: an outlined square, filled with a check mark when checked.
        let checkbox = shift(layout.checkbox);
        let side = px(CHECKBOX_SIZE) as f32;
        let (bx, by) = (
            px(checkbox.loc.x) as f32,
            (px(checkbox.loc.y) as f32 + (px(checkbox.size.h) as f32 - side) / 2.0).round(),
        );
        let (b, r) = (px(1).max(1) as f32, CAP_RADIUS * s);
        let hovered = view.hover == Some(QuitHit::Checkbox);
        if view.dont_ask {
            let fill = if hovered {
                mix(p.accent, p.text, 0.12)
            } else {
                p.accent
            };
            c.rounded(bx, by, side, side, r, fill);
            let (cx, cy) = (bx + side / 2.0, by + side / 2.0);
            c.paint_in(
                bx as i32,
                by as i32,
                side as i32,
                side as i32,
                p.on_accent,
                |x, y| coverage(check(x - cx, y - cy, s)),
            );
        } else {
            c.rounded(bx, by, side, side, r, p.border);
            let inner = if hovered { p.selected } else { p.elevated };
            c.rounded(bx + b, by + b, side - 2.0 * b, side - 2.0 * b, r - b, inner);
        }
        self.text(
            &mut c,
            "Don't ask again",
            FONT_SIZE,
            bx + side + (CHECKBOX_GAP as f32 * s).round(),
            phys(checkbox, scale).1,
            band(checkbox, scale),
            scale,
            p.text,
        );

        // The buttons; the last, the default, in the accent.
        for (i, (rect, (label, _))) in layout.buttons.iter().zip(QUIT_BUTTONS).enumerate() {
            let rect = shift(*rect);
            let (x, y, w, h) = rect_px(rect, scale);
            let r = BUTTON_RADIUS * s;
            let hovered = view.hover == Some(QuitHit::Button(i));
            if view.selected == i {
                focus_ring(&mut c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            let color = if i == 2 {
                let fill = if hovered {
                    mix(p.accent, p.text, 0.12)
                } else {
                    p.accent
                };
                c.rounded(x, y, w, h, r, fill);
                p.on_accent
            } else {
                let fill = if hovered { p.selected } else { p.element };
                c.rounded(x, y, w, h, r, p.border);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                if i == 1 { p.warning } else { p.text }
            };
            self.centered(&mut c, label, FONT_SIZE, rect, scale, color);
        }
        c.into_buffer(false)
    }

    /// `text` with the characters at `hits` (char positions) in the second
    /// of `colors`. Draws nothing where not even "…" fits.
    #[allow(clippy::too_many_arguments)]
    fn marked(
        &self,
        c: &mut Canvas,
        text: &str,
        hits: &[usize],
        size: f32,
        x: f32,
        max_x: f32,
        band: (i32, i32),
        scale: f64,
        (color, hit): (Rgb, Rgb),
    ) -> f32 {
        let chars: Vec<char> = text.chars().collect();
        let width = |n: usize| self.text_width(&chars[..n].iter().collect::<String>(), size, scale);
        let ellipsis = self.text_width("…", size, scale);
        let mut n = chars.len();
        let cut = x + width(n) > max_x;
        if cut {
            if x + ellipsis > max_x {
                return x;
            }
            while n > 0 && x + width(n) + ellipsis > max_x {
                n -= 1;
            }
        }
        let mut pen = x;
        let mut i = 0;
        while i < n {
            let is_hit = hits.contains(&i);
            let run = (i..n).find(|j| hits.contains(j) != is_hit).unwrap_or(n);
            let text: String = chars[i..run].iter().collect();
            pen = self.text(
                c,
                &text,
                size,
                pen,
                f32::MAX,
                band,
                scale,
                if is_hit { hit } else { color },
            );
            i = run;
        }
        if cut {
            pen = self.text(c, "…", size, pen, f32::MAX, band, scale, color);
        }
        pen
    }

    /// Width of `keys` drawn as keycaps, in physical pixels.
    fn keycaps_width(&self, keys: &str, scale: f64) -> f32 {
        let pad = 2.0 * CAP_PADDING as f32 * scale as f32;
        let sep = self.text_width("/", KEY_FONT_SIZE, scale) + 2.0 * CAP_GAP as f32 * scale as f32;
        let caps: Vec<f32> = keys
            .split(" / ")
            .map(|k| self.text_width(k, KEY_FONT_SIZE, scale) + pad)
            .collect();
        caps.iter().sum::<f32>() + sep * (caps.len() - 1) as f32
    }

    /// Draw `keys` as Zed-style keycaps from `x`, vertically centred in the band
    /// `(top, height)`, alternatives (" / ") split by a dim slash. Returns where they end.
    fn keycaps(
        &self,
        c: &mut Canvas,
        keys: &str,
        x: f32,
        (top, height): (i32, i32),
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let s = scale as f32;
        let (cap_h, border) = ((20.0 * s).round(), (s.round()).max(1.0));
        let cap_y = (top as f32 + (height as f32 - cap_h) / 2.0).round();
        let band = (cap_y as i32, cap_h as i32);
        let mut pen = x;
        for (i, key) in keys.split(" / ").enumerate() {
            if i > 0 {
                pen += CAP_GAP as f32 * s;
                self.text(
                    c,
                    "/",
                    KEY_FONT_SIZE,
                    pen,
                    f32::MAX,
                    band,
                    scale,
                    p.placeholder,
                );
                pen += self.text_width("/", KEY_FONT_SIZE, scale) + CAP_GAP as f32 * s;
            }
            let w =
                (self.text_width(key, KEY_FONT_SIZE, scale) + 2.0 * CAP_PADDING as f32 * s).round();
            let (cx, r) = (pen.round(), CAP_RADIUS * s);
            c.rounded(cx, cap_y, w, cap_h, r, p.border);
            c.rounded(
                cx + border,
                cap_y + border,
                w - 2.0 * border,
                cap_h - 2.0 * border,
                r - border,
                p.element,
            );
            self.text(
                c,
                key,
                KEY_FONT_SIZE,
                cx + CAP_PADDING as f32 * s,
                f32::MAX,
                band,
                scale,
                p.muted,
            );
            pen = cx + w;
        }
        pen
    }

    fn text_width(&self, text: &str, size: f32, scale: f64) -> f32 {
        let Some(font) = &self.font else { return 0.0 };
        let font = font.as_scaled(PxScale::from(size * scale as f32));
        text.chars().map(|c| font.h_advance(font.glyph_id(c))).sum()
    }

    /// Draw `text` centred in a logical rectangle.
    fn centered(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        rect: Rectangle<i32, Logical>,
        scale: f64,
        color: Rgb,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let x = px(rect.loc.x) as f32
            + (px(rect.size.w) as f32 - self.text_width(text, size, scale)) / 2.0;
        let band = (px(rect.loc.y), px(rect.size.h));
        self.text(
            c,
            text,
            size,
            x,
            px(rect.loc.x + rect.size.w) as f32,
            band,
            scale,
            color,
        );
    }

    /// Draw an icon glyph at `ICON_SIZE`, centred on its own ink in a logical
    /// rectangle ("×" and "+" sit off the cap-height centre `text` uses).
    fn icon(
        &self,
        c: &mut Canvas,
        ch: char,
        rect: Rectangle<i32, Logical>,
        scale: f64,
        color: Rgb,
    ) {
        let Some(font) = &self.font else { return };
        let scaled = font.as_scaled(PxScale::from(ICON_SIZE * scale as f32));
        let Some(outline) = font.outline_glyph(scaled.scaled_glyph(ch)) else {
            return;
        };
        let ink = outline.px_bounds();
        let px = |v: i32| (v as f64 * scale) as f32;
        let x = (px(rect.loc.x) + (px(rect.size.w) - ink.width()) / 2.0 - ink.min.x).round();
        let y = (px(rect.loc.y) + (px(rect.size.h) - ink.height()) / 2.0 - ink.min.y).round();
        let glyph = scaled.scaled_glyph(ch);
        let glyph = ab_glyph::Glyph {
            position: ab_glyph::point(x, y),
            ..glyph
        };
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                c.blend(
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                    color,
                    coverage,
                );
            });
        }
    }

    /// The marks after a branch: a dot for uncommitted changes, then arrows
    /// with the commits ahead of and behind the upstream. Drawn from `x` in
    /// the band `(top, height)`, as far as `max_x` leaves room. Returns where
    /// they end.
    #[allow(clippy::too_many_arguments)]
    fn git_marks(
        &self,
        c: &mut Canvas,
        git: &GitStatus,
        x: f32,
        max_x: f32,
        (top, height): (i32, i32),
        scale: f64,
        p: &Palette,
    ) -> f32 {
        let s = scale as f32;
        let (w, gap) = (
            (GIT_MARK_WIDTH as f32 * s).round(),
            (GIT_MARK_GAP as f32 * s).round(),
        );
        let mark = |c: &mut Canvas, x: f32, color: Rgb, shape: &dyn Fn(f32, f32) -> f32| {
            let x = x + gap;
            if x + w > max_x {
                return None;
            }
            let (cx, cy) = (x + w / 2.0, top as f32 + height as f32 / 2.0);
            c.paint_in(x as i32, top, w as i32, height, color, |px, py| {
                coverage(shape(px - cx, py - cy))
            });
            Some(x + w)
        };
        let mut x = x;
        if git.dirty {
            let Some(end) = mark(c, x, p.warning, &|x, y| x.hypot(y) - 2.5 * s) else {
                return x;
            };
            x = end;
        }
        for (count, up) in [(git.ahead, true), (git.behind, false)] {
            if count == 0 {
                continue;
            }
            let Some(end) = mark(c, x, p.muted, &|x, y| arrow(up, x, y, s)) else {
                return x;
            };
            x = self.text(
                c,
                &count.to_string(),
                DETAIL_FONT_SIZE,
                end + s,
                max_x,
                (top, height),
                scale,
                p.muted,
            );
        }
        x
    }

    /// Draw `text` from `x`, vertically centred in the band `(top, height)`, cut
    /// off with "…" before `max_x`. Coordinates are physical pixels. Returns
    /// where it ends.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        x: f32,
        max_x: f32,
        (top, height): (i32, i32),
        scale: f64,
        color: Rgb,
    ) -> f32 {
        let Some(font) = &self.font else { return x };
        let scaled = font.as_scaled(PxScale::from(size * scale as f32));
        // Centre on the cap height; the font's ascent/descent include line gap padding.
        let cap_height = font
            .outline_glyph(scaled.scaled_glyph('H'))
            .map_or(scaled.ascent(), |o| -o.px_bounds().min.y);
        let baseline = (top as f32 + (height as f32 + cap_height) / 2.0).round();

        let mut shown = text.to_string();
        if x + self.text_width(&shown, size, scale) > max_x {
            let ellipsis = self.text_width("…", size, scale);
            while !shown.is_empty() && x + self.text_width(&shown, size, scale) + ellipsis > max_x {
                shown.pop();
            }
            shown.push('…');
        }

        let mut pen = x;
        for ch in shown.chars() {
            let id = scaled.glyph_id(ch);
            let glyph = id.with_scale_and_position(scaled.scale(), ab_glyph::point(pen, baseline));
            pen += scaled.h_advance(id);
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    c.blend(
                        bounds.min.x as i32 + gx as i32,
                        bounds.min.y as i32 + gy as i32,
                        color,
                        coverage,
                    );
                });
            }
        }
        pen
    }
}

/// A logical rectangle's left and right edges in physical pixels.
fn phys(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (px(rect.loc.x), px(rect.loc.x + rect.size.w))
}

/// A logical rectangle in physical pixels (x, y, width, height), its edges
/// on whole pixels.
fn rect_px(rect: Rect, scale: f64) -> (f32, f32, f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let (x, y) = (px(rect.loc.x), px(rect.loc.y));
    (
        x,
        y,
        px(rect.loc.x + rect.size.w) - x,
        px(rect.loc.y + rect.size.h) - y,
    )
}

/// A logical rectangle's rows as the band `(top, height)` text is centred in.
fn band(rect: Rect, scale: f64) -> (i32, i32) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    (
        px(rect.loc.y),
        px(rect.loc.y + rect.size.h) - px(rect.loc.y),
    )
}

/// The text caret at `x`, vertically centred in the band `(top, height)`.
fn caret(c: &mut Canvas, x: f32, (top, height): (i32, i32), scale: f64, color: Rgb) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    let h = px(CARET_HEIGHT).min(height);
    c.fill(
        x.round() as i32,
        top + (height - h) / 2,
        px(CARET_WIDTH).max(1),
        h,
        color,
    );
}

/// A ring in `color` around a button in `rect` with corners of `radius`
/// (logical pixels), held off it by a gap showing `behind`.
fn focus_ring(c: &mut Canvas, rect: Rect, radius: f32, behind: Rgb, scale: f64, color: Rgb) {
    let s = scale as f32;
    let (x, y, w, h) = rect_px(rect, scale);
    let (gap, out) = (FOCUS_GAP * s, (FOCUS_GAP + FOCUS_RING) * s);
    let grown = |by: f32| (x - by, y - by, w + 2.0 * by, h + 2.0 * by, radius * s + by);
    let (rx, ry, rw, rh, rr) = grown(out);
    c.rounded(rx, ry, rw, rh, rr, color);
    let (gx, gy, gw, gh, gr) = grown(gap);
    c.rounded(gx, gy, gw, gh, gr, behind);
}

/// Signed distance from (x, y) to a rounded rectangle: negative inside.
fn rounded_rect(x: f32, y: f32, left: f32, top: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let qx = (x - left - hw).abs() - (hw - r);
    let qy = (y - top - hh).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Signed distance to a sun icon centred on the origin, `s` pixels per logical pixel.
fn sun(x: f32, y: f32, s: f32) -> f32 {
    let disc = x.hypot(y) - 3.5 * s;
    let rays = (0..8)
        .map(|i| {
            let (sin, cos) = (i as f32 * std::f32::consts::FRAC_PI_4).sin_cos();
            segment(
                x,
                y,
                cos * 5.8 * s,
                sin * 5.8 * s,
                cos * 7.6 * s,
                sin * 7.6 * s,
            ) - 0.8 * s
        })
        .fold(f32::MAX, f32::min);
    disc.min(rays)
}

/// Signed distance to a crescent moon icon centred on the origin.
fn moon(x: f32, y: f32, s: f32) -> f32 {
    let disc = x.hypot(y) - 6.5 * s;
    let bite = (x - 3.6 * s).hypot(y + 3.0 * s) - 5.2 * s;
    disc.max(-bite)
}

/// Half the stroke width of the line icons below, in logical pixels.
const STROKE: f32 = 0.8;

/// Distance to a "+" of arm `arm` centred on (cx, cy), stroked.
fn plus(x: f32, y: f32, cx: f32, cy: f32, arm: f32, s: f32) -> f32 {
    let h = segment(x, y, cx - arm, cy, cx + arm, cy);
    let v = segment(x, y, cx, cy - arm, cx, cy + arm);
    h.min(v) - STROKE * s
}

/// The tab strip's "+": larger and a little bolder than the badge pluses.
fn new_tab(x: f32, y: f32, s: f32) -> f32 {
    plus(x, y, 0.0, 0.0, 6.0 * s, s) - 0.2 * s
}

/// Signed distance to a folder outline centred on the origin: a body with a
/// tab on its top left.
fn folder(x: f32, y: f32, s: f32) -> f32 {
    let body = rounded_rect(x, y, -8.0 * s, -5.0 * s, 16.0 * s, 12.0 * s, 1.5 * s);
    let tab = rounded_rect(x, y, -8.0 * s, -7.0 * s, 7.0 * s, 4.0 * s, 1.5 * s);
    body.min(tab).abs() - STROKE * s
}

/// Signed distance to a window outline with a bar across its top: a tab.
fn tab(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -6.5 * s, 16.0 * s, 13.0 * s, 2.0 * s).abs() - STROKE * s;
    frame.min(segment(x, y, -8.0 * s, -2.5 * s, 8.0 * s, -2.5 * s) - STROKE * s)
}

/// A folder with a "+" inside: add a project.
fn folder_plus(x: f32, y: f32, s: f32) -> f32 {
    folder(x, y, s).min(plus(x, y, 0.0, 1.0 * s, 2.8 * s, s))
}

/// Signed distance to a window outline with a "play" triangle inside:
/// launch an app.
fn app(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -6.5 * s, 16.0 * s, 13.0 * s, 2.0 * s).abs() - STROKE * s;
    let (a, b, c) = ((-2.0 * s, -3.5 * s), (-2.0 * s, 3.5 * s), (3.5 * s, 0.0));
    let play = segment(x, y, a.0, a.1, b.0, b.1)
        .min(segment(x, y, b.0, b.1, c.0, c.1))
        .min(segment(x, y, c.0, c.1, a.0, a.1))
        - STROKE * s;
    frame.min(play)
}

/// The ">" prompt of the terminal icons.
fn prompt(x: f32, y: f32, s: f32) -> f32 {
    segment(x, y, -4.5 * s, -2.5 * s, -1.5 * s, 0.0).min(segment(
        x,
        y,
        -1.5 * s,
        0.0,
        -4.5 * s,
        2.5 * s,
    )) - STROKE * s
}

/// Signed distance to a terminal window outline with a prompt and cursor,
/// centred on the origin.
fn terminal(x: f32, y: f32, s: f32) -> f32 {
    let frame =
        rounded_rect(x, y, -8.0 * s, -7.0 * s, 16.0 * s, 14.0 * s, 2.0 * s).abs() - STROKE * s;
    let cursor = segment(x, y, 0.0, 2.5 * s, 3.0 * s, 2.5 * s) - STROKE * s;
    frame.min(prompt(x, y, s)).min(cursor)
}

/// A terminal with a "+" badge cut into its bottom right corner: add a
/// terminal. Shifted up and left so the badge stays in the button.
fn terminal_plus(x: f32, y: f32, s: f32) -> f32 {
    let (x, y) = (x + 1.0 * s, y + 0.5 * s);
    let (bx, by) = (6.5 * s, 5.5 * s);
    let frame =
        rounded_rect(x, y, -8.0 * s, -7.0 * s, 16.0 * s, 14.0 * s, 2.0 * s).abs() - STROKE * s;
    let gap = (x - bx).hypot(y - by) - 5.0 * s;
    frame
        .max(-gap)
        .min(prompt(x, y, s))
        .min(plus(x, y, bx, by, 3.0 * s, s))
}

/// Signed distance to the mark for an agent's status, centred on the origin:
/// an open ring spinning while it works, a dot when it wants attention
/// (blocked, done), a ring while it waits.
fn status_mark(status: AgentStatus, x: f32, y: f32, s: f32) -> f32 {
    let ring = (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
    match status {
        // The ring without its top right quarter.
        AgentStatus::Working => ring.max(x.min(-y)),
        AgentStatus::Blocked | AgentStatus::Done => x.hypot(y) - 4.2 * s,
        AgentStatus::Idle => ring,
        AgentStatus::Unknown => (x.hypot(y) - 2.6 * s).abs() - 0.9 * s,
    }
}

/// Signed distance to a Git branch icon centred on the origin: a trunk
/// between two rings, and a ring at the top right forking off it.
fn git_branch(x: f32, y: f32, s: f32) -> f32 {
    let ring = |cx: f32, cy: f32| ((x - cx * s).hypot(y - cy * s) - 1.8 * s).abs() - STROKE * s;
    let trunk = segment(x, y, -3.0 * s, -2.2 * s, -3.0 * s, 2.2 * s) - STROKE * s;
    let fork = segment(x, y, 3.0 * s, -2.2 * s, 3.0 * s, -0.5 * s).min(segment(
        x,
        y,
        3.0 * s,
        -0.5 * s,
        -3.0 * s,
        2.0 * s,
    )) - STROKE * s;
    ring(-3.0, -4.0)
        .min(ring(-3.0, 4.0))
        .min(ring(3.0, -4.0))
        .min(trunk)
        .min(fork)
}

/// Signed distance to a small arrow centred on the origin, pointing up or down.
fn arrow(up: bool, x: f32, y: f32, s: f32) -> f32 {
    let y = if up { y } else { -y };
    let shaft = segment(x, y, 0.0, -3.5 * s, 0.0, 3.5 * s);
    let head =
        segment(x, y, -2.5 * s, -s, 0.0, -3.5 * s).min(segment(x, y, 2.5 * s, -s, 0.0, -3.5 * s));
    shaft.min(head) - STROKE * s
}

/// Signed distance to a check mark centred on the origin.
fn check(x: f32, y: f32, s: f32) -> f32 {
    let short = segment(x, y, -3.5 * s, 0.0, -s, 2.5 * s);
    let long = segment(x, y, -s, 2.5 * s, 3.8 * s, -3.0 * s);
    short.min(long) - 1.1 * s
}

/// Signed distance to a ring struck through, centred on the origin: what
/// runs in a tab won't run again after a restart.
fn no_restart(x: f32, y: f32, s: f32) -> f32 {
    let ring = (x.hypot(y) - 3.6 * s).abs() - STROKE * s;
    let slash = segment(x, y, -2.6 * s, 2.6 * s, 2.6 * s, -2.6 * s) - STROKE * s;
    ring.min(slash)
}

/// Signed distance to a small down-pointing chevron centred on the origin.
fn chevron_down(x: f32, y: f32, s: f32) -> f32 {
    let left = segment(x, y, -3.0 * s, -1.5 * s, 0.0, 1.5 * s);
    let right = segment(x, y, 0.0, 1.5 * s, 3.0 * s, -1.5 * s);
    left.min(right) - 0.75 * s
}

/// Paint the signed-distance `shape`, centred on a logical rectangle.
fn icon_sdf(c: &mut Canvas, rect: Rect, scale: f64, color: Rgb, shape: impl Fn(f32, f32) -> f32) {
    let px = |v: i32| (v as f64 * scale).round() as i32;
    let (x, y, w, h) = (
        px(rect.loc.x),
        px(rect.loc.y),
        px(rect.size.w),
        px(rect.size.h),
    );
    let (cx, cy) = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0);
    c.paint_in(x, y, w, h, color, |px, py| {
        coverage(shape(px - cx, py - cy))
    });
}

/// Distance from (x, y) to the segment from (ax, ay) to (bx, by).
fn segment(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
    let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - dx * t).hypot(py - dy * t)
}

/// Anti-aliased pixel coverage for a signed distance in pixels.
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// ARGB8888 pixels with premultiplied alpha, stored little-endian as B, G, R, A.
struct Canvas {
    w: i32,
    h: i32,
    px: Vec<u8>,
}

impl Canvas {
    fn new(size: Size<i32, Logical>) -> Self {
        Self {
            w: size.w,
            h: size.h,
            px: vec![0; (size.w * size.h * 4) as usize],
        }
    }

    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, [r, g, b]: Rgb) {
        for py in y.max(0)..(y + h).min(self.h) {
            for px in x.max(0)..(x + w).min(self.w) {
                let i = ((py * self.w + px) * 4) as usize;
                self.px[i..i + 4].copy_from_slice(&[b, g, r, 0xff]);
            }
        }
    }

    /// Lay `color` over the pixel at `coverage` opacity.
    fn blend(&mut self, x: i32, y: i32, [r, g, b]: Rgb, coverage: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let a = coverage.clamp(0.0, 1.0);
        for (dst, src) in self.px[i..i + 4].iter_mut().zip([b, g, r, 0xff]) {
            *dst = (*dst as f32 * (1.0 - a) + src as f32 * a).round() as u8;
        }
    }

    /// Blend `color` over every pixel at the coverage `shape` gives for its centre.
    fn paint(&mut self, color: Rgb, shape: impl Fn(f32, f32) -> f32) {
        self.paint_in(0, 0, self.w, self.h, color, shape);
    }

    /// `paint`, limited to a rectangle (physical pixels).
    fn paint_in(
        &mut self,
        x0: i32,
        y0: i32,
        w: i32,
        h: i32,
        color: Rgb,
        shape: impl Fn(f32, f32) -> f32,
    ) {
        for y in y0.max(0)..(y0 + h).min(self.h) {
            for x in x0.max(0)..(x0 + w).min(self.w) {
                let a = shape(x as f32 + 0.5, y as f32 + 0.5);
                if a > 0.0 {
                    self.blend(x, y, color, a);
                }
            }
        }
    }

    /// `icon` `APP_ICON_SIZE` big, centred on `rect` (logical pixels); a
    /// symbolic one in `tint`.
    fn picture(&mut self, icon: &AppIcon, rect: Rect, scale: f64, tint: Rgb) {
        let size = (APP_ICON_SIZE as f64 * scale).round() as usize;
        let (w, h, rgba) = icon.scaled(size);
        let centre = |start: i32, len: i32| (start as f64 + len as f64 / 2.0) * scale;
        let x0 = (centre(rect.loc.x, rect.size.w) - w as f64 / 2.0).round() as i32;
        let y0 = (centre(rect.loc.y, rect.size.h) - h as f64 / 2.0).round() as i32;
        for (i, &[r, g, b, a]) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (x0 + (i % w) as i32, y0 + (i / w) as i32);
            if a == 0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
                continue;
            }
            if icon.symbolic() {
                self.blend(x, y, tint, a as f32 / 255.0);
                continue;
            }
            // Premultiplied: the icon's colour, then what shows through.
            let j = ((y * self.w + x) * 4) as usize;
            let keep = 1.0 - a as f32 / 255.0;
            for (dst, src) in self.px[j..j + 4].iter_mut().zip([b, g, r, a]) {
                *dst = (src as f32 + *dst as f32 * keep).round().min(255.0) as u8;
            }
        }
    }

    /// A filled, anti-aliased rounded rectangle (physical pixels).
    fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgb) {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(self.h) {
            for px in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(self.w) {
                let a = coverage(rounded_rect(
                    px as f32 + 0.5,
                    py as f32 + 0.5,
                    x,
                    y,
                    w,
                    h,
                    r,
                ));
                if a > 0.0 {
                    self.blend(px, py, color, a);
                }
            }
        }
    }

    /// A popup panel of logical `size` with a soft drop shadow, drawn `SHADOW`
    /// in from the top left.
    fn popup(&mut self, size: Size<i32, Logical>, radius: f32, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (ox, oy) = (px(SHADOW) as f32, px(SHADOW) as f32);
        let (w, h) = (px(size.w) as f32, px(size.h) as f32);
        let radius = radius * s;

        // Soft shadow, dropped slightly below the panel.
        let blur = (SHADOW as f32 - 2.0) * s;
        let shadow_y = oy + 3.0 * s;
        self.paint([0, 0, 0], |x, y| {
            let d = rounded_rect(x, y, ox, shadow_y, w, h, radius);
            let t = 1.0 - (d / blur).clamp(0.0, 1.0);
            p.shadow * t * t
        });
        self.rounded(ox, oy, w, h, radius, p.border);
        let b = px(1).max(1) as f32;
        self.rounded(
            ox + b,
            oy + b,
            w - 2.0 * b,
            h - 2.0 * b,
            radius - b,
            p.elevated,
        );
    }

    /// `opaque`: every pixel is fully opaque, so the compositor need not draw behind it.
    fn into_buffer(self, opaque: bool) -> MemoryRenderBuffer {
        let opaque = opaque.then(|| vec![Rectangle::from_size((self.w, self.h).into())]);
        MemoryRenderBuffer::from_slice(
            &self.px,
            Fourcc::Argb8888,
            (self.w, self.h),
            1,
            Transform::Normal,
            opaque,
        )
    }
}

/// Pixel size of a buffer covering `size` logical pixels.
fn physical(size: Size<i32, Logical>, scale: f64) -> Size<i32, Logical> {
    let px = |v: i32| ((v as f64 * scale).round() as i32).max(1);
    (px(size.w), px(size.h)).into()
}

fn find_font() -> Option<String> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    (!path.is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::{
        Painter, ProjectRow, QuitHit, QuitView, Rect, TabView, Theme, WelcomeHit, WelcomeView,
        WorkspaceView, quit_layout, short_path, welcome_layout, welcome_page,
    };
    use crate::{WorkspaceKind, herdr::AgentStatus};
    use smithay::utils::{Logical, Size};
    use std::path::Path;

    fn inside(outer: Rect, inner: Rect) -> bool {
        inner.loc.x >= outer.loc.x
            && inner.loc.y >= outer.loc.y
            && inner.loc.x + inner.size.w <= outer.loc.x + outer.size.w
            && inner.loc.y + inner.size.h <= outer.loc.y + outer.size.h
    }

    #[test]
    fn short_path_cuts_folders_between_start_and_last() {
        assert_eq!(
            short_path(Path::new("/usr/local/share/fonts"), None),
            "/u/l/s/fonts"
        );
        assert_eq!(short_path(Path::new("/etc/.config/x"), None), "/e/.c/x");
        assert_eq!(short_path(Path::new("/"), None), "/");
        assert_eq!(short_path(Path::new("/tmp"), None), "/tmp");
    }

    #[test]
    fn short_path_starts_from_repository() {
        let root = Some(("gerdr", Path::new("/work/gerdr")));
        assert_eq!(short_path(Path::new("/work/gerdr"), root), "gerdr");
        assert_eq!(
            short_path(Path::new("/work/gerdr/src/bin/tool"), root),
            "gerdr/s/b/tool"
        );
        assert_eq!(short_path(Path::new("/elsewhere/dir"), root), "/e/dir");
    }

    #[test]
    fn workspace_puts_its_tabs_together() {
        let tab = |status, branch: &str, place: &str, activity: &str| TabView {
            title: String::new(),
            status,
            branch: Some(branch.into()),
            place: place.into(),
            activity: activity.into(),
            restart: None,
        };
        let tabs = [
            tab(
                Some(AgentStatus::Working),
                "main",
                "gerdr",
                "claude working  |  zsh",
            ),
            tab(
                Some(AgentStatus::Blocked),
                "main",
                "gerdr, gerdr/src",
                "zsh",
            ),
        ];
        let ws = WorkspaceView::new("gerdr".into(), WorkspaceKind::Project, &tabs);
        assert_eq!(ws.status, Some(AgentStatus::Blocked));
        assert_eq!(ws.branch.as_deref(), Some("main"));
        assert_eq!(ws.place, "gerdr, gerdr/src");
        assert_eq!(ws.activity, "2 tabs · claude working, zsh");

        let plain = WorkspaceView::new("~".into(), WorkspaceKind::Terminal, &[TabView::default()]);
        assert_eq!(plain.activity, "");
    }

    #[test]
    fn welcome_layout_fits_the_output() {
        for (w, h) in [(800, 600), (1920, 1080), (1280, 720)] {
            let size: Size<i32, Logical> = (w, h).into();
            let output = Rect::from_size(size);
            let rows = welcome_page(size);
            for naming in [false, true] {
                let l = welcome_layout(size, rows, naming);
                for part in [
                    l.greeting, l.subtitle, l.label, l.list, l.button, l.note, l.hints,
                ] {
                    assert!(inside(output, part), "{part:?} outside {w}x{h}");
                }
                assert_eq!(l.rows.len(), rows);
                assert!(l.rows.iter().all(|r| inside(l.list, *r)));
                assert!(
                    l.rows
                        .windows(2)
                        .all(|r| r[0].loc.y + r[0].size.h <= r[1].loc.y)
                );
                assert!(l.subtitle.loc.y >= l.greeting.loc.y + l.greeting.size.h);
                assert!(l.label.loc.y + l.label.size.h <= l.list.loc.y);
                assert!(l.button.loc.y >= l.list.loc.y + l.list.size.h);
                assert!(l.note.loc.y >= l.button.loc.y + l.button.size.h);
                assert!(l.hints.loc.y >= l.note.loc.y + l.note.size.h);
                // The hints sit at the bottom; the column is centred.
                assert!(h - (l.hints.loc.y + l.hints.size.h) <= 24);
                assert_eq!(l.list.loc.x, w - (l.list.loc.x + l.list.size.w));
            }
        }
    }

    #[test]
    fn welcome_layout_without_rows_keeps_the_label() {
        let size: Size<i32, Logical> = (800, 600).into();
        let l = welcome_layout(size, 0, false);
        assert!(l.rows.is_empty());
        assert_eq!(l.list.size.h, 0);
        assert!(l.label.size.h > 0);
        assert!(l.button.loc.y >= l.label.loc.y + l.label.size.h);
    }

    #[test]
    fn naming_widens_the_button_to_the_column() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let button = welcome_layout(size, 3, false).button;
        let field = welcome_layout(size, 3, true).button;
        let list = welcome_layout(size, 3, true).list;
        assert!(field.size.w > button.size.w);
        assert_eq!((field.loc.x, field.size.w), (list.loc.x, list.size.w));
        assert_eq!(field.loc.y, button.loc.y);
        // The button is centred on the output.
        assert_eq!(button.loc.x, 1920 - (button.loc.x + button.size.w));
    }

    #[test]
    fn welcome_page_grows_with_the_height() {
        let mut last = 0;
        for h in (100..2000).step_by(10) {
            let rows = welcome_page((1000, h).into());
            assert!(rows >= 1 && rows >= last);
            last = rows;
        }
        assert_eq!(welcome_page((1000, 100).into()), 1);
        assert!(last > 1);
    }

    #[test]
    fn quit_layout_is_centred_with_buttons_in_order() {
        let size: Size<i32, Logical> = (1920, 1080).into();
        let l = quit_layout(size);
        let p = l.panel;
        assert_eq!(p.loc.x, 1920 - (p.loc.x + p.size.w));
        assert_eq!(p.loc.y, 1080 - (p.loc.y + p.size.h));
        for part in [l.title, l.body, l.checkbox] {
            assert!(inside(p, part));
        }
        assert!(l.buttons.iter().all(|b| inside(p, *b)));
        assert!(
            l.buttons
                .windows(2)
                .all(|b| b[0].loc.x + b[0].size.w < b[1].loc.x)
        );
        assert!(
            l.buttons
                .iter()
                .all(|b| b.loc.y >= l.checkbox.loc.y + l.checkbox.size.h)
        );
    }

    #[test]
    fn welcome_and_quit_paint() {
        let painter = Painter::new();
        let row = |name: &str, running, current| ProjectRow {
            name: name.into(),
            detail: "3 repos · ~/Work/aft".into(),
            ago: "2h ago".into(),
            running,
            current,
        };
        let mut view = WelcomeView {
            greeting: "Good evening, lgal".into(),
            subtitle: "Pick up where you left off".into(),
            query: "af".into(),
            rows: vec![
                row("aft", Some(true), true),
                row(
                    "a project with a very long name indeed, longer than the row",
                    Some(false),
                    false,
                ),
                row("gerdr", None, false),
            ],
            selected: Some(1),
            hover: Some(WelcomeHit::Row(2)),
            naming: None,
            confirm_forget: Some(2),
            note: Some("herdr isn't installed".into()),
        };
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for (w, h) in [(800, 600), (1920, 1080), (60, 40)] {
                    let size = (w, h).into();
                    painter.welcome(&view, size, scale, theme.palette());
                    let quit = QuitView {
                        project: "aft".into(),
                        selected: 2,
                        dont_ask: scale > 1.0,
                        hover: Some(QuitHit::Button(1)),
                    };
                    painter.quit_dialog(&quit, size, scale, theme.palette());
                }
            }
        }
        view.naming = Some(String::new());
        view.rows.clear();
        view.selected = None;
        painter.welcome(&view, (800, 600).into(), 1.5, Theme::Dark.palette());
        view.naming = None;
        view.hover = Some(WelcomeHit::Button);
        painter.welcome(&view, (800, 600).into(), 1.0, Theme::Light.palette());

        let tabs = [
            TabView {
                title: "zsh".into(),
                activity: "cargo watch".into(),
                restart: Some(false),
                ..Default::default()
            },
            TabView::default(),
        ];
        painter.tab_strip(&tabs, 0, false, Theme::Dark, (900, 48).into(), 1.25);
    }
}
