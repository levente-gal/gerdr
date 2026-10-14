//! gerdr's own UI: the workspace sidebar on the left (each workspace with
//! what its tabs show put together, and a card per coding agent herdr sees
//! in it), the tab strip above the apps
//! (each tab with what runs in it, as herdr reports;
//! the light/dark switch at its right end), a bar over each terminal pane
//! (what runs in it, and buttons to restart and stop it), and the key help
//! panel shown while the prefix is armed, the switcher, the project menu
//! under the sidebar header and a notice over the apps; the welcome
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
    text_field::Line,
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
/// Side of the rounded square behind the tab strip's "+", which reaches on
/// over its menu chevron, and the room left of it.
const NEW_TAB_SIZE: i32 = 28;
const NEW_TAB_INSET: i32 = (BUTTON_WIDTH - NEW_TAB_SIZE) / 2;
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

/// The sidebar header's project name: the gap between it and its chevron,
/// and the room after the chevron inside the highlight.
const HEADER_NAME_GAP: i32 = 2;
const HEADER_NAME_END: i32 = 4;

/// The project menu: a panel at most this wide, just below the sidebar
/// header, reaching over the apps.
const PROJECT_MENU_WIDTH: i32 = 440;
const PROJECT_MENU_TOP: i32 = BAR_HEIGHT + 2;
/// The query field (the title in form mode), the space around the list,
/// and the footer with the key hints.
const PROJECT_MENU_INPUT: i32 = 44;
const PROJECT_MENU_PADDING: i32 = 6;
const PROJECT_MENU_FOOTER: i32 = 36;
/// A project, "New project" or folder row, and a section label.
const PROJECT_MENU_ROW: i32 = 40;
const PROJECT_MENU_SECTION: i32 = 22;
/// The fewest and most entries the list shows; it scrolls for more.
const PROJECT_MENU_MIN_ROWS: i32 = 3;
const PROJECT_MENU_MAX_ROWS: i32 = 12;
/// Room for the running dot or the icon left of each row's label, and the
/// gap after it.
const MENU_MARK_WIDTH: i32 = 14;
const MENU_MARK_GAP: i32 = 8;
/// The square buttons on a project row under the pointer: their side, the
/// gap between them and the room after the last.
const ROW_BUTTON: i32 = 26;
const ROW_BUTTON_GAP: i32 = 2;
const ROW_BUTTON_INSET: i32 = 6;
/// The field a project row's name turns into while renaming: its height,
/// and how far it reaches left of the name.
const RENAME_FIELD: i32 = 28;
const RENAME_INSET: i32 = 6;

/// The project menu's form: its inner padding, the column of labels, a row
/// and the field in it, and the gap between parts side by side.
const FORM_PADDING: i32 = 14;
const FORM_LABEL_WIDTH: i32 = 64;
const FORM_ROW: i32 = 40;
const FORM_FIELD: i32 = 32;
const FORM_GAP: i32 = 8;
/// The Browse button right of the folder field.
const BROWSE_WIDTH: i32 = 84;
/// A folder offered under the folder field.
const SUGGESTION_ROW: i32 = 28;
/// The "Opens" choices: what each opens, its label and its width; their
/// height, and the side of the round mark before the label.
const KIND_CHOICES: [(WorkspaceKind, &str, i32); 2] = [
    (WorkspaceKind::Project, "Zed + terminal", 150),
    (WorkspaceKind::Terminal, "Terminal only", 130),
];
const KIND_HEIGHT: i32 = 28;
const RADIO_SIZE: i32 = 16;
/// The line saying why the project can't be created.
const ERROR_HEIGHT: i32 = 22;
/// Cancel and Create & switch: labels and widths.
const FORM_BUTTONS: [(&str, i32); 2] = [("Cancel", 84), ("Create & switch", 136)];
/// Cancel, Run now and Save in the project settings form: labels and widths.
const SETTINGS_BUTTONS: [(&str, i32); 3] = [("Cancel", 84), ("Run now", 96), ("Save", 84)];

/// The notice: a pill this high, its text this far in from its ends, this
/// far below the tab strip.
const NOTICE_HEIGHT: i32 = 30;
const NOTICE_PADDING: i32 = 16;
const NOTICE_TOP: i32 = 8;

/// The bar over each terminal pane: this high, with its restart and stop
/// buttons this wide at its right end, this far apart and in from the end.
pub const PANE_BAR_HEIGHT: i32 = 24;
const BAR_BUTTON: i32 = 24;
const BAR_BUTTON_GAP: i32 = 2;
const BAR_INSET: i32 = 4;
const BAR_NOTE_GAP: i32 = 8;

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
pub fn command_line(cmdline: &str) -> Option<String> {
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
    pub query: Line,
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
    pub query: Line,
    /// The projects in view, most recently used first.
    pub rows: Vec<ProjectRow>,
    /// The selected row; `None` when nothing matches or the button is.
    pub selected: Option<usize>,
    pub hover: Option<WelcomeHit>,
    /// Set while naming a new project: the name typed so far.
    pub naming: Option<Line>,
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

/// What the project menu shows: in list mode the query, the entries in view
/// and the selection; in form mode the new project's form.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectMenuView {
    /// What was typed to filter the projects; empty in form mode.
    pub query: Line,
    /// The entries in view, scrolled.
    pub entries: Vec<MenuEntry>,
    /// The selected entry, by index into `entries`.
    pub selected: Option<usize>,
    pub hover: Option<MenuHit>,
    /// Set in form mode.
    pub form: Option<FormView>,
    /// Set in settings mode.
    pub settings: Option<SettingsView>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
}

/// An entry of the project menu's list.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A section label ("Current", "Recent"); not selectable.
    Section(String),
    Project {
        row: ProjectRow,
        /// The Alt+digit that opens it, 1 to 9.
        digit: Option<u8>,
        /// Set while renaming it: the name typed so far.
        renaming: Option<Line>,
        /// Delete was pressed on it once.
        confirm_forget: bool,
    },
    /// Start a new project: the label, as `New project "blo"…`.
    New(String),
    /// Start a new project in this folder ("~/Sites/blog").
    Folder(String),
}

/// The project menu's form for a new project.
#[derive(Clone, Debug, PartialEq)]
pub struct FormView {
    pub name: Line,
    /// The folder as typed; empty for none.
    pub folder: Line,
    /// What the folder opens as.
    pub kind: WorkspaceKind,
    pub focus: FormField,
    /// The folders offered for what was typed in the folder field, as
    /// "~/Sites/blog"; empty when the list is closed.
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    /// Why the project can't be created as it is.
    pub error: Option<String>,
}

/// A part of the project menu's form that takes the keyboard, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormField {
    Name,
    Folder,
    Browse,
    Kind,
    Cancel,
    Create,
}

/// The project menu's form for a project's start and stop commands.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsView {
    /// The project's name.
    pub name: String,
    /// Its start commands, as typed.
    pub before: Line,
    /// Its stop commands, as typed.
    pub after: Line,
    /// Where they run, as "~/Work/app".
    pub dir: String,
    pub focus: SettingsField,
}

/// A part of the project settings form that takes the keyboard, in Tab
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsField {
    Before,
    After,
    Cancel,
    Run,
    Save,
}

/// A button shown on a project row of the menu under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowButton {
    /// Its start and stop commands.
    Settings,
    Rename,
    Stop,
    Forget,
}

/// What the pointer is over in the project menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuHit {
    Query,
    /// An entry, by index into `ProjectMenuView::entries`.
    Entry(usize),
    /// A button on an entry's row.
    Button(usize, RowButton),
    Field(FormField),
    /// A part of the settings form.
    Setting(SettingsField),
    /// One of the form's two "Opens" choices.
    Kind(WorkspaceKind),
    Suggestion(usize),
    /// Inside the panel, on nothing.
    Panel,
}

/// Where the project menu's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMenuLayout {
    pub panel: Rect,
    /// The query field (list mode) or the title (form mode).
    pub query: Rect,
    /// Per entry, its rect and its row buttons' (none when not hovered).
    pub entries: Vec<(Rect, Vec<(RowButton, Rect)>)>,
    pub footer: Rect,
    /// Set in form mode.
    pub form: Option<FormLayout>,
    /// Set in settings mode.
    pub settings: Option<SettingsLayout>,
}

/// Where the project menu's form parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormLayout {
    pub name: Rect,
    pub folder: Rect,
    pub browse: Rect,
    pub kinds: [(WorkspaceKind, Rect); 2],
    pub suggestions: Vec<Rect>,
    pub error: Rect,
    pub cancel: Rect,
    pub create: Rect,
}

/// Where the project menu's settings form parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLayout {
    pub before: Rect,
    pub after: Rect,
    /// The line saying where the commands run.
    pub note: Rect,
    pub cancel: Rect,
    pub run: Rect,
    pub save: Rect,
}

/// Where the parts of the project menu showing `view` go, on an output of
/// `size`: below the sidebar header, reaching over the apps. The query field
/// (the title in form mode), the entries or the form, then the footer. On an
/// output too short for it all, the entries that don't fit are cut to nothing.
pub fn project_menu_layout(view: &ProjectMenuView, size: Size<i32, Logical>) -> ProjectMenuLayout {
    let w = PROJECT_MENU_WIDTH.min(size.w - 2 * HELP_MARGIN).max(1);
    let (x, top) = (ROW_INSET, PROJECT_MENU_TOP);
    let query = Rectangle::new((x, top).into(), (w, PROJECT_MENU_INPUT).into());
    let mut y = top + PROJECT_MENU_INPUT + PROJECT_MENU_PADDING;
    let (mut entries, form, settings) = match (&view.form, &view.settings) {
        (Some(form), _) => (Vec::new(), Some(form_layout(form, x, w, &mut y)), None),
        (None, Some(_)) => (Vec::new(), None, Some(settings_layout(x, w, &mut y))),
        (None, None) => (menu_entries(view, x, w, &mut y), None, None),
    };
    let bottom = (y + PROJECT_MENU_PADDING + PROJECT_MENU_FOOTER)
        .min(size.h - HELP_MARGIN)
        .max(top + PROJECT_MENU_INPUT + PROJECT_MENU_FOOTER);
    let footer = Rectangle::new(
        (x, bottom - PROJECT_MENU_FOOTER).into(),
        (w, PROJECT_MENU_FOOTER).into(),
    );
    let limit = footer.loc.y - PROJECT_MENU_PADDING;
    for (rect, buttons) in &mut entries {
        if rect.loc.y + rect.size.h > limit {
            *rect = Rectangle::new(
                (rect.loc.x, rect.loc.y.min(limit)).into(),
                (rect.size.w, 0).into(),
            );
            buttons.clear();
        }
    }
    ProjectMenuLayout {
        panel: Rectangle::new((x, top).into(), (w, bottom - top).into()),
        query,
        entries,
        footer,
        form,
        settings,
    }
}

/// The list's entries in a panel `w` wide at `x`, from `y` down (moved past
/// them), with the row buttons of the project row under the pointer. It
/// keeps room for one row without any.
fn menu_entries(
    view: &ProjectMenuView,
    x: i32,
    w: i32,
    y: &mut i32,
) -> Vec<(Rect, Vec<(RowButton, Rect)>)> {
    let hovered = match view.hover {
        Some(MenuHit::Entry(i) | MenuHit::Button(i, _)) => Some(i),
        _ => None,
    };
    let (x, w) = (
        x + PROJECT_MENU_PADDING,
        (w - 2 * PROJECT_MENU_PADDING).max(0),
    );
    let start = *y;
    let entries = view
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let h = match entry {
                MenuEntry::Section(_) => PROJECT_MENU_SECTION,
                _ => PROJECT_MENU_ROW,
            };
            let rect = Rectangle::new((x, *y).into(), (w, h).into());
            *y += h;
            let buttons = match entry {
                MenuEntry::Project { row, .. } if hovered == Some(i) => {
                    row_button_rects(&row_buttons(row), rect)
                }
                _ => Vec::new(),
            };
            (rect, buttons)
        })
        .collect();
    *y = (*y).max(start + PROJECT_MENU_ROW);
    entries
}

/// The buttons a project row shows under the pointer, left to right:
/// settings and rename always; stop while its session may run, and forget,
/// unless it is the current project.
fn row_buttons(row: &ProjectRow) -> Vec<RowButton> {
    let mut buttons = vec![RowButton::Settings, RowButton::Rename];
    if !row.current && row.running != Some(false) {
        buttons.push(RowButton::Stop);
    }
    if !row.current {
        buttons.push(RowButton::Forget);
    }
    buttons
}

/// Square `buttons` at the right end of a row in `rect`, vertically centred.
fn row_button_rects(buttons: &[RowButton], rect: Rect) -> Vec<(RowButton, Rect)> {
    let n = buttons.len() as i32;
    let right = rect.loc.x + rect.size.w - ROW_BUTTON_INSET;
    let left = right - n * ROW_BUTTON - (n - 1).max(0) * ROW_BUTTON_GAP;
    let top = rect.loc.y + (rect.size.h - ROW_BUTTON) / 2;
    buttons
        .iter()
        .enumerate()
        .map(|(i, button)| {
            let x = left + i as i32 * (ROW_BUTTON + ROW_BUTTON_GAP);
            (
                *button,
                Rectangle::new((x, top).into(), (ROW_BUTTON, ROW_BUTTON).into()),
            )
        })
        .collect()
}

/// The form's parts in a panel `w` wide at `x`, from `y` down (moved past
/// them): a row each for the name, the folder (with Browse at its right,
/// then the suggestions under it) and the "Opens" choices, the error line,
/// then Cancel and Create at the right.
fn form_layout(form: &FormView, x: i32, w: i32, y: &mut i32) -> FormLayout {
    let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w.max(0), h).into());
    let right = x + w - FORM_PADDING;
    let field_x = x + FORM_PADDING + FORM_LABEL_WIDTH;
    let field_w = (right - field_x).max(0);
    let in_row = |y: i32, h: i32| y + (FORM_ROW - h) / 2;

    let name = rect(field_x, in_row(*y, FORM_FIELD), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let browse_w = BROWSE_WIDTH.min(field_w);
    let folder_w = field_w - browse_w - FORM_GAP;
    let folder = rect(field_x, in_row(*y, FORM_FIELD), folder_w, FORM_FIELD);
    let browse = rect(right - browse_w, folder.loc.y, browse_w, FORM_FIELD);
    *y += FORM_ROW;
    let suggestions = (0..form.suggestions.len() as i32)
        .map(|i| rect(field_x, *y + i * SUGGESTION_ROW, folder_w, SUGGESTION_ROW))
        .collect::<Vec<_>>();
    if !suggestions.is_empty() {
        *y += suggestions.len() as i32 * SUGGESTION_ROW + FORM_GAP;
    }
    let mut kind_x = field_x;
    let kinds = KIND_CHOICES.map(|(kind, _, kind_w)| {
        let kind_w = kind_w.min((right - kind_x).max(0));
        let r = rect(kind_x, in_row(*y, KIND_HEIGHT), kind_w, KIND_HEIGHT);
        kind_x += kind_w + FORM_GAP;
        (kind, r)
    });
    *y += FORM_ROW;
    let error = rect(field_x, *y, field_w, ERROR_HEIGHT);
    *y += ERROR_HEIGHT;
    let [(_, cancel_w), (_, create_w)] = FORM_BUTTONS;
    let create = rect(right - create_w, *y, create_w, QUIT_BUTTON);
    let cancel = rect(
        create.loc.x - QUIT_BUTTON_GAP - cancel_w,
        *y,
        cancel_w,
        QUIT_BUTTON,
    );
    *y += QUIT_BUTTON + FORM_GAP;
    FormLayout {
        name,
        folder,
        browse,
        kinds,
        suggestions,
        error,
        cancel,
        create,
    }
}

/// The settings form's parts in a panel `w` wide at `x`, from `y` down
/// (moved past them): a row each for the start and stop commands, the line
/// saying where they run, then Cancel, Run now and Save at the right.
fn settings_layout(x: i32, w: i32, y: &mut i32) -> SettingsLayout {
    let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w.max(0), h).into());
    let right = x + w - FORM_PADDING;
    let field_x = x + FORM_PADDING + FORM_LABEL_WIDTH;
    let field_w = (right - field_x).max(0);
    let in_row = |y: i32| y + (FORM_ROW - FORM_FIELD) / 2;

    let before = rect(field_x, in_row(*y), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let after = rect(field_x, in_row(*y), field_w, FORM_FIELD);
    *y += FORM_ROW;
    let note = rect(field_x, *y, field_w, ERROR_HEIGHT);
    *y += ERROR_HEIGHT + FORM_GAP;
    let [(_, cancel_w), (_, run_w), (_, save_w)] = SETTINGS_BUTTONS;
    let save = rect(right - save_w, *y, save_w, QUIT_BUTTON);
    let run = rect(save.loc.x - QUIT_BUTTON_GAP - run_w, *y, run_w, QUIT_BUTTON);
    let cancel = rect(
        run.loc.x - QUIT_BUTTON_GAP - cancel_w,
        *y,
        cancel_w,
        QUIT_BUTTON,
    );
    *y += QUIT_BUTTON + FORM_GAP;
    SettingsLayout {
        before,
        after,
        note,
        cancel,
        run,
        save,
    }
}

/// How many entries the project menu's list shows at most on an output of
/// `size`: as many full rows as fit above its bottom margin, within bounds.
pub fn project_menu_page(size: Size<i32, Logical>) -> usize {
    let room = size.h
        - HELP_MARGIN
        - PROJECT_MENU_TOP
        - PROJECT_MENU_INPUT
        - 2 * PROJECT_MENU_PADDING
        - PROJECT_MENU_FOOTER;
    (room / PROJECT_MENU_ROW).clamp(PROJECT_MENU_MIN_ROWS, PROJECT_MENU_MAX_ROWS) as usize
}

/// What `pos` is over in the project menu showing `view` on an output of
/// `size`; `None` outside its panel.
pub fn project_menu_hit(
    view: &ProjectMenuView,
    size: Size<i32, Logical>,
    pos: Point<f64, Logical>,
) -> Option<MenuHit> {
    let layout = project_menu_layout(view, size);
    let over = |rect: Rect| rect.to_f64().contains(pos);
    if !over(layout.panel) {
        return None;
    }
    if let Some(form) = &layout.form {
        let fields = [
            (form.name, FormField::Name),
            (form.folder, FormField::Folder),
            (form.browse, FormField::Browse),
            (form.cancel, FormField::Cancel),
            (form.create, FormField::Create),
        ];
        let hit = fields
            .iter()
            .find(|(rect, _)| over(*rect))
            .map(|(_, field)| MenuHit::Field(*field))
            .or_else(|| {
                let i = form.suggestions.iter().position(|rect| over(*rect))?;
                Some(MenuHit::Suggestion(i))
            })
            .or_else(|| {
                let (kind, _) = form.kinds.iter().find(|(_, rect)| over(*rect))?;
                Some(MenuHit::Kind(*kind))
            });
        return hit.or(Some(MenuHit::Panel));
    }
    if let Some(settings) = &layout.settings {
        let fields = [
            (settings.before, SettingsField::Before),
            (settings.after, SettingsField::After),
            (settings.cancel, SettingsField::Cancel),
            (settings.run, SettingsField::Run),
            (settings.save, SettingsField::Save),
        ];
        let hit = fields
            .iter()
            .find(|(rect, _)| over(*rect))
            .map(|(_, field)| MenuHit::Setting(*field));
        return hit.or(Some(MenuHit::Panel));
    }
    if over(layout.query) {
        return Some(MenuHit::Query);
    }
    for (i, (rect, buttons)) in layout.entries.iter().enumerate() {
        if let Some((button, _)) = buttons.iter().find(|(_, rect)| over(*rect)) {
            return Some(MenuHit::Button(i, *button));
        }
        if over(*rect) {
            return Some(MenuHit::Entry(i));
        }
    }
    Some(MenuHit::Panel)
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

/// Where the tabs (each with its "×" close button in its title row), the
/// "+" button and its menu chevron sit in a strip `width` wide (strip
/// coordinates). The theme switch keeps the right end.
pub fn tab_layout(count: usize, width: i32) -> (Vec<(Rect, Rect)>, Rect, Rect) {
    let room = (width - 2 * BUTTON_WIDTH - CHEVRON_WIDTH).max(0);
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
        (NEW_TAB_INSET + NEW_TAB_SIZE, TAB_STRIP_HEIGHT).into(),
    );
    let chevron = Rectangle::new(
        (plus.loc.x + plus.size.w, 0).into(),
        (CHEVRON_WIDTH, TAB_STRIP_HEIGHT).into(),
    );
    (tabs, plus, chevron)
}

/// A pane's place split in two: its bar on top, and the rest for its
/// window, which keeps at least a pixel.
pub fn pane_bar_rects(pane: Rect) -> (Rect, Rect) {
    let h = PANE_BAR_HEIGHT.min(pane.size.h - 1).max(0);
    let bar = Rectangle::new(pane.loc, (pane.size.w, h).into());
    let client = Rectangle::new(
        (pane.loc.x, pane.loc.y + h).into(),
        (pane.size.w, (pane.size.h - h).max(1)).into(),
    );
    (bar, client)
}

/// Where the restart and stop buttons sit in a pane's bar, left to right.
pub fn pane_bar_buttons(bar: Rect) -> [(BarButton, Rect); 2] {
    let right = bar.loc.x + bar.size.w - BAR_INSET;
    let at = |i: i32| {
        Rectangle::new(
            (right - (i + 1) * BAR_BUTTON - i * BAR_BUTTON_GAP, bar.loc.y).into(),
            (BAR_BUTTON, bar.size.h).into(),
        )
    };
    [(BarButton::Restart, at(1)), (BarButton::Stop, at(0))]
}

/// What of a pane's bar is at `pos`, if the bar is.
pub fn pane_bar_hit(bar: Rect, pos: Point<f64, Logical>) -> Option<BarHit> {
    if !bar.to_f64().contains(pos) {
        return None;
    }
    let button = pane_bar_buttons(bar)
        .into_iter()
        .find(|(_, rect)| rect.to_f64().contains(pos));
    Some(button.map_or(BarHit::Label, |(button, _)| BarHit::Button(button)))
}

/// The light/dark switch at the right end of a strip `width` wide (strip coordinates).
pub fn theme_toggle(width: i32) -> Rect {
    Rectangle::new(
        (width - BUTTON_WIDTH, 0).into(),
        (BUTTON_WIDTH, TAB_TITLE_HEIGHT).into(),
    )
}

pub struct SidebarLayout {
    /// The project's name, opening the project menu: the header left of
    /// its buttons.
    pub header: Rect,
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
        header: Rectangle::new((0, 0).into(), (project.loc.x.max(0), BAR_HEIGHT).into()),
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
    let (tabs, _, _) = tab_layout(count, width);
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
/// A button in a terminal pane's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButton {
    Restart,
    Stop,
}

/// What a click on a pane's bar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Label,
    Button(BarButton),
}

/// What a pane's bar says of its command, by the mark before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStatus {
    /// The shell waits at its prompt, and nothing ran in it to run again.
    Idle,
    Running,
    /// The command ran and is done: it can run again.
    Exited,
    /// Being stopped or restarted.
    Working,
}

/// The bar over a terminal pane: what runs in it, and its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneBarView {
    /// Where the bar goes on the output.
    pub rect: Rect,
    pub label: String,
    /// A word after the label, such as "exited" or "stopping…".
    pub note: Option<&'static str>,
    pub status: BarStatus,
    pub can_restart: bool,
    pub can_stop: bool,
    /// The pane has keyboard focus.
    pub focused: bool,
    /// The button under the pointer.
    pub hover: Option<BarButton>,
}

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
    /// The project shown, by name, for the sidebar header.
    pub project: Option<String>,
    /// The pointer is over the sidebar header's project name.
    pub header_hover: bool,
    /// Set while the project menu is open.
    pub project_menu: Option<ProjectMenuView>,
    /// A short note at the top of the app area, such as "Switched to gerdr".
    pub notice: Option<String>,
    /// The bars over the active tab's terminal panes.
    pub pane_bars: Vec<PaneBarView>,
    pub theme: Theme,
    pub sidebar_width: i32,
    pub size: Size<i32, Logical>,
    pub scale: f64,
}

/// Sidebar inputs: workspaces, their agents, the active one, whether the
/// terminal menu is open, the project's name and whether it is lit (under
/// the pointer or its menu open), theme, size, scale.
type SidebarKey = (
    Vec<WorkspaceView>,
    Vec<Vec<AgentView>>,
    Option<usize>,
    bool,
    Option<String>,
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
    project_menu: Cached<(ProjectMenuView, Size<i32, Logical>, Theme, f64)>,
    /// The notice's text and width.
    notice: Cached<(String, i32, Theme, f64)>,
    /// One per bar over a terminal pane.
    pane_bars: Vec<Cached<(PaneBarView, Theme, f64)>>,
}

impl Chrome {
    /// The byte of `field`'s text nearest output x `x`, as it is drawn on
    /// an output of `scale`: where a click there puts the caret.
    pub fn caret_at(&self, field: &FieldSpot, x: f64, scale: f64) -> usize {
        self.painter
            .caret_at(&field.line, field.text, (x * scale) as f32, scale)
    }

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
            project_menu: Cached::default(),
            notice: Cached::default(),
            pane_bars: Vec::new(),
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
        let header_lit = view.header_hover || view.project_menu.is_some();
        let sidebar_key = (
            view.workspaces,
            view.agents,
            view.active_workspace,
            menu_open,
            view.project,
            header_lit,
            theme,
            sidebar_size,
            scale,
        );
        let sidebar = self.sidebar.get(
            sidebar_key,
            |(workspaces, agents, active, menu_open, project, lit, ..)| {
                painter.sidebar(
                    workspaces,
                    agents,
                    *active,
                    *menu_open,
                    (project.as_deref(), *lit),
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

        // Then the project menu, the switcher, the terminal menu and the
        // help panel cover the apps; the project menu the sidebar too.
        if let Some(menu) = view.project_menu {
            let panel = project_menu_layout(&menu, output).panel;
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self
                .project_menu
                .get((menu, output, theme, scale), |(menu, output, ..)| {
                    painter.project_menu(menu, *output, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
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
        if let Some(notice) = view.notice {
            // Centred at the top of the app area, just below the tab strip.
            let w = painter.notice_width(&notice, strip_size.w - 2 * HELP_MARGIN, scale);
            let loc = Point::from((
                sidebar_w + (strip_size.w - w) / 2 - SHADOW,
                TAB_STRIP_HEIGHT + NOTICE_TOP - SHADOW,
            ));
            let size = (w + 2 * SHADOW, NOTICE_HEIGHT + 2 * SHADOW).into();
            let buffer = self
                .notice
                .get((notice, w, theme, scale), |(notice, w, ..)| {
                    painter.notice(notice, *w, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
        // The pane bars sit on the apps, under everything else.
        self.pane_bars
            .resize_with(view.pane_bars.len(), Cached::default);
        for (cached, bar) in self.pane_bars.iter_mut().zip(view.pane_bars) {
            let (loc, size) = (bar.rect.loc, bar.rect.size);
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let buffer = cached.get((bar, theme, scale), |(bar, ..)| {
                painter.pane_bar(bar, scale, theme.palette())
            });
            parts.push((buffer, loc, size));
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
        (project, lit): (Option<&str>, bool),
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
        match project {
            Some(name) => self.header_name(&mut c, name, lit, layout.header, scale, p),
            None => {
                self.text(
                    &mut c,
                    "Workspaces",
                    SMALL_FONT_SIZE,
                    px(ROW_INSET) as f32 + PADDING * s,
                    px(layout.project.loc.x) as f32,
                    (0, px(BAR_HEIGHT)),
                    scale,
                    p.muted,
                );
            }
        }
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

    /// The project's `name` in the sidebar `header`, cut to fit, and a
    /// chevron after it; on a rounded highlight when `lit` (under the
    /// pointer, or its menu open).
    fn header_name(
        &self,
        c: &mut Canvas,
        name: &str,
        lit: bool,
        header: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let header_end = header.loc.x + header.size.w - HEADER_MARGIN;
        let text_x = px(ROW_INSET) as f32 + PADDING * s;
        let max_x = px(header_end - HEADER_NAME_END - CHEVRON_WIDTH - HEADER_NAME_GAP) as f32;
        let text_end = (text_x + self.text_width(name, FONT_SIZE, scale)).min(max_x);
        // The chevron right after the name, in logical pixels.
        let chevron_x = (text_end / s).ceil() as i32 + HEADER_NAME_GAP;
        let chevron = Rectangle::new((chevron_x, 0).into(), (CHEVRON_WIDTH, BAR_HEIGHT).into());
        if lit {
            let x = px(ROW_INSET) as f32;
            let right = px((chevron_x + CHEVRON_WIDTH + HEADER_NAME_END).min(header_end)) as f32;
            c.rounded(
                x,
                px(5) as f32,
                right - x,
                px(BAR_HEIGHT - 10) as f32,
                4.0 * s,
                p.selected,
            );
        }
        self.text(
            c,
            name,
            FONT_SIZE,
            text_x,
            max_x,
            (0, px(BAR_HEIGHT)),
            scale,
            p.text,
        );
        icon_sdf(c, chevron, scale, p.muted, |x, y| chevron_down(x, y, s));
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

        let (tabs, plus, chevron) = tab_layout(views.len(), size.w);
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
            // A soft rounded square with a large "+", centred in the strip,
            // stretched over the chevron of the menu of apps to open instead.
            let side = px(NEW_TAB_SIZE) as f32;
            let square = Rectangle::new(
                (plus.loc.x + NEW_TAB_INSET, 0).into(),
                (NEW_TAB_SIZE, plus.size.h).into(),
            );
            let (bx, by) = (
                px(square.loc.x) as f32,
                ((px(plus.size.h) as f32 - side) / 2.0).round(),
            );
            let w = (px(chevron.loc.x + chevron.size.w) - px(square.loc.x)) as f32;
            c.rounded(bx, by, w, side, 6.0 * s, p.element);
            icon_sdf(&mut c, square, scale, p.text, |x, y| new_tab(x, y, s));
            icon_sdf(&mut c, chevron, scale, p.muted, |x, y| {
                chevron_down(x, y, s)
            });
            // A thin line splits the "+" from its chevron.
            c.fill(
                px(chevron.loc.x),
                by as i32 + px(6),
                line,
                side as i32 - 2 * px(6),
                p.border,
            );
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
        self.edit_line(
            &mut c,
            &view.query,
            view.hint,
            switcher_text(input, scale),
            band,
            true,
            scale,
            p,
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
                px(input.loc.x + input.size.w - HELP_PADDING) as f32,
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
            self.edit_line(
                &mut c,
                &view.query,
                "",
                (start, end),
                label_band,
                true,
                scale,
                p,
            );
        }

        for (i, (rect, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            self.project_row(&mut c, view, i, row, *rect, scale, p);
        }

        let button = layout.button;
        match &view.naming {
            Some(name) => self.field(&mut c, name, "Project name", button, true, scale, p),
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

    /// A text field in `rect`, like the switcher's query: `line` (or
    /// `placeholder` without any) and, when `focused`, its caret and
    /// selection.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &self,
        c: &mut Canvas,
        line: &Line,
        placeholder: &str,
        rect: Rect,
        focused: bool,
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
        self.edit_line(
            c,
            line,
            placeholder,
            field_text(rect, scale),
            field_band,
            focused,
            scale,
            p,
        );
    }

    /// The text of a field being edited, `line`, between `x` and `max_x`
    /// (physical pixels) in the band `(top, height)`: `placeholder` without
    /// any text, and when `focused`, the selection behind it and the caret.
    /// A line too long to fit scrolls to keep the caret in view; otherwise
    /// it is cut off with "…".
    #[allow(clippy::too_many_arguments)]
    fn edit_line(
        &self,
        c: &mut Canvas,
        line: &Line,
        placeholder: &str,
        (x, max_x): (f32, f32),
        field_band: (i32, i32),
        focused: bool,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        if line.is_empty() {
            let shown = x + px(CARET_WIDTH + 2) as f32;
            self.text(
                c,
                placeholder,
                FONT_SIZE,
                shown,
                max_x,
                field_band,
                scale,
                p.placeholder,
            );
            if focused {
                caret(c, x, field_band, scale, p.accent);
            }
            return;
        }
        let width = |text: &str| self.text_width(text, FONT_SIZE, scale);
        let to_caret = width(&line[..line.caret]);
        // How far the text moves left for the caret to show.
        let scrolled = (to_caret - (max_x - x)).max(0.0);
        let start = x - scrolled;
        let selection = line.selection();
        if focused && !selection.is_empty() {
            let from = (start + width(&line[..selection.start])).max(x);
            let to = (start + width(&line[..selection.end])).min(max_x);
            let (top, height) = field_band;
            let inset = px(3) as f32;
            c.rounded(
                from - s,
                top as f32 + inset,
                to - from + 2.0 * s,
                height as f32 - 2.0 * inset,
                2.0 * s,
                p.selected,
            );
        }
        if scrolled > 0.0 {
            self.clipped_text(c, line, start, (x, max_x), field_band, scale, p.text);
        } else {
            self.text(c, line, FONT_SIZE, x, max_x, field_band, scale, p.text);
        }
        if focused {
            let nudge = if line.caret > 0 { px(1) as f32 } else { 0.0 };
            caret(c, start + to_caret + nudge, field_band, scale, p.accent);
        }
    }

    /// Key hints as keycaps, each followed by what it does, centred in
    /// `rect`: as many of the first as fit.
    fn hints(&self, c: &mut Canvas, hints: &[(&str, &str)], rect: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let key_gap = (HINT_KEY_GAP as f32 * s).round();
        let gap = (HINT_GAP as f32 * s).round();
        let (x, _, w, _) = rect_px(rect, scale);
        let mut width = -gap;
        for (n, (keys, action)) in hints.iter().enumerate() {
            let more = gap
                + self.keycaps_width(keys, scale)
                + key_gap
                + self.text_width(action, SMALL_FONT_SIZE, scale);
            if width + more > w {
                return self.hints(c, &hints[..n], rect, scale, p);
            }
            width += more;
        }
        let width = width.max(0.0);
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

    /// The project menu for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow: the query field and the entries, or the
    /// form under its title; then the key hints.
    fn project_menu(
        &self,
        view: &ProjectMenuView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = project_menu_layout(view, size);
        let panel = layout.panel;
        let outer = Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let border = px(1).max(1);
        c.popup(panel.size, HELP_RADIUS, scale, p);
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        // A line across the panel, at `y` physical pixels.
        let divider = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };

        let query = shift(layout.query);
        match (&layout.form, &layout.settings) {
            (None, Some(settings)) => {
                if let Some(settings_view) = &view.settings {
                    let (x, right) = phys(query, scale);
                    self.text(
                        &mut c,
                        &format!("{} · start and stop", settings_view.name),
                        LABEL_FONT_SIZE,
                        x + (FORM_PADDING as f32 * s).round(),
                        right,
                        band(query, scale),
                        scale,
                        p.text,
                    );
                    self.menu_settings(&mut c, settings_view, view.hover, settings, by, scale, p);
                }
            }
            (Some(form), _) => {
                let (x, right) = phys(query, scale);
                self.text(
                    &mut c,
                    "New project",
                    LABEL_FONT_SIZE,
                    x + (FORM_PADDING as f32 * s).round(),
                    right,
                    band(query, scale),
                    scale,
                    p.text,
                );
                if let Some(form_view) = &view.form {
                    self.menu_form(&mut c, form_view, view.hover, form, by, scale, p);
                }
            }
            (None, None) => {
                let renaming = view.entries.iter().any(|e| {
                    matches!(
                        e,
                        MenuEntry::Project {
                            renaming: Some(_),
                            ..
                        }
                    )
                });
                self.menu_query(&mut c, &view.query, !renaming, query, scale, p);
                if view.entries.is_empty() {
                    let first = Rectangle::new(
                        (
                            query.loc.x,
                            query.loc.y + query.size.h + PROJECT_MENU_PADDING,
                        )
                            .into(),
                        (query.size.w, PROJECT_MENU_ROW).into(),
                    );
                    let (x, right) = phys(first, scale);
                    self.text(
                        &mut c,
                        "Nothing matches",
                        FONT_SIZE,
                        x + px(PROJECT_MENU_PADDING) as f32 + PADDING * s,
                        right,
                        band(first, scale),
                        scale,
                        p.placeholder,
                    );
                }
                for (i, ((rect, buttons), entry)) in
                    layout.entries.iter().zip(&view.entries).enumerate()
                {
                    if rect.size.h == 0 {
                        continue;
                    }
                    let rect = shift(*rect);
                    let buttons: Vec<_> = buttons.iter().map(|(b, r)| (*b, shift(*r))).collect();
                    // A line between the projects and the rows starting new ones.
                    let after_project = i
                        .checked_sub(1)
                        .is_some_and(|j| matches!(view.entries[j], MenuEntry::Project { .. }));
                    if matches!(entry, MenuEntry::New(_)) && after_project {
                        divider(&mut c, px(rect.loc.y));
                    }
                    self.menu_entry(&mut c, view, i, entry, rect, &buttons, scale, p);
                }
            }
        }
        divider(&mut c, px(query.loc.y + query.size.h) - border);

        let footer = shift(layout.footer);
        divider(&mut c, px(footer.loc.y));
        let hints: Vec<(&str, &str)> = view
            .hints
            .iter()
            .map(|(keys, action)| (keys.as_str(), action.as_str()))
            .collect();
        let inner = Rectangle::new(
            (footer.loc.x + HELP_PADDING, footer.loc.y).into(),
            ((footer.size.w - 2 * HELP_PADDING).max(0), footer.size.h).into(),
        );
        self.hints(&mut c, &hints, inner, scale, p);
        c.into_buffer(false)
    }

    /// The project menu's query field in `rect`: a magnifier, then `query`
    /// (or what to type) with a caret after it when `focused`.
    fn menu_query(
        &self,
        c: &mut Canvas,
        query: &Line,
        focused: bool,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        // Lined up with the marks and labels of the rows below.
        let mark_x = rect.loc.x + PROJECT_MENU_PADDING + PADDING as i32;
        let mark = Rectangle::new(
            (mark_x, rect.loc.y).into(),
            (MENU_MARK_WIDTH, rect.size.h).into(),
        );
        icon_sdf(c, mark, scale, p.placeholder, |x, y| magnifier(x, y, s));
        let query_band = band(rect, scale);
        let placeholder = "Find or name a project…";
        self.edit_line(
            c,
            query,
            placeholder,
            menu_query_text(rect, scale),
            query_band,
            focused,
            scale,
            p,
        );
    }

    /// Entry `i` of the project menu in `rect` (buffer coordinates), with
    /// the row `buttons` it shows under the pointer.
    #[allow(clippy::too_many_arguments)]
    fn menu_entry(
        &self,
        c: &mut Canvas,
        view: &ProjectMenuView,
        i: usize,
        entry: &MenuEntry,
        rect: Rect,
        buttons: &[(RowButton, Rect)],
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let row_band = band(rect, scale);
        let mark = Rectangle::new(
            (rect.loc.x + PADDING as i32, rect.loc.y).into(),
            (MENU_MARK_WIDTH, rect.size.h).into(),
        );
        let label_lx = mark.loc.x + MENU_MARK_WIDTH + MENU_MARK_GAP;
        let label_x = px(label_lx) as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let right = x + w - PADDING * s;
        let gap = (FIELD_GAP as f32 * s).round();
        if let MenuEntry::Section(label) = entry {
            self.text(
                c,
                &label.to_uppercase(),
                DETAIL_FONT_SIZE,
                px(mark.loc.x) as f32,
                right,
                row_band,
                scale,
                p.placeholder,
            );
            return;
        }

        // The highlight stops short of the rows above and below.
        let (y, h) = (y + px(2) as f32, h - 2.0 * px(2) as f32);
        let r = (MENU_RADIUS - 2.0) * s;
        let hovered = matches!(
            view.hover,
            Some(MenuHit::Entry(j) | MenuHit::Button(j, _)) if j == i
        );
        let confirm = matches!(
            entry,
            MenuEntry::Project {
                confirm_forget: true,
                ..
            }
        );
        if confirm {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.warning, 0.15));
        } else if view.selected == Some(i) {
            c.rounded(x, y, w, h, r, p.selected);
        } else if hovered {
            c.rounded(x, y, w, h, r, mix(p.elevated, p.selected, 0.5));
        }

        // A keycap at the right end of the row; returns where it starts.
        let keycap = |c: &mut Canvas, keys: &str| {
            let cap_x = (right - self.keycaps_width(keys, scale)).round();
            self.keycaps(c, keys, cap_x, row_band, scale, p);
            cap_x
        };
        match entry {
            MenuEntry::Section(_) => {}
            MenuEntry::Project {
                row,
                digit,
                renaming,
                ..
            } => {
                let dot = move |x: f32, y: f32| x.hypot(y) - 4.2 * s;
                let ring = move |x: f32, y: f32| (x.hypot(y) - 3.6 * s).abs() - 1.1 * s;
                match (confirm, row.running) {
                    (true, _) => icon_sdf(c, mark, scale, p.warning, dot),
                    (false, Some(true)) => icon_sdf(c, mark, scale, p.success, dot),
                    (false, Some(false)) => icon_sdf(c, mark, scale, p.placeholder, ring),
                    (false, None) => {}
                }
                let mut end = right;
                if let Some((_, first)) = buttons.first() {
                    self.row_buttons(c, view, i, buttons, confirm, scale, p);
                    end = px(first.loc.x) as f32 - gap;
                } else if !confirm && renaming.is_none() {
                    if let Some(digit) = digit {
                        end = keycap(c, &format!("Alt+{digit}")) - gap;
                    }
                    let ago_x = (end - self.text_width(&row.ago, SMALL_FONT_SIZE, scale)).round();
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
                    end = ago_x - gap;
                }
                if confirm {
                    self.text(
                        c,
                        &format!("Press Delete again to forget {}", row.name),
                        FONT_SIZE,
                        label_x,
                        end,
                        row_band,
                        scale,
                        p.warning,
                    );
                } else if let Some(name) = renaming {
                    let field = rename_field(rect, buttons);
                    self.field(c, name, "Project name", field, true, scale, p);
                } else {
                    let after = self.text(
                        c, &row.name, FONT_SIZE, label_x, end, row_band, scale, p.text,
                    );
                    self.text(
                        c,
                        &row.detail,
                        SMALL_FONT_SIZE,
                        after + (12.0 * s).round(),
                        end,
                        row_band,
                        scale,
                        p.muted,
                    );
                }
            }
            MenuEntry::New(label) => {
                icon_sdf(c, mark, scale, p.muted, |x, y| {
                    plus(x, y, 0.0, 0.0, 5.0 * s, s)
                });
                let end = keycap(c, "Ctrl+N") - gap;
                self.text(c, label, FONT_SIZE, label_x, end, row_band, scale, p.text);
            }
            MenuEntry::Folder(path) => {
                let small = 0.8 * s;
                icon_sdf(c, mark, scale, p.placeholder, |x, y| folder(x, y, small));
                let suffix = "new project";
                let suffix_w = self.text_width(suffix, SMALL_FONT_SIZE, scale);
                let after = self.text(
                    c,
                    path,
                    FONT_SIZE,
                    label_x,
                    right - suffix_w - gap,
                    row_band,
                    scale,
                    p.text,
                );
                self.text(
                    c,
                    suffix,
                    SMALL_FONT_SIZE,
                    after + gap,
                    right,
                    row_band,
                    scale,
                    p.placeholder,
                );
            }
        }
    }

    /// The `buttons` of entry `i`: icons, the one under the pointer on a
    /// rounded square; forget in the warning colour while it asks to be
    /// confirmed.
    #[allow(clippy::too_many_arguments)]
    fn row_buttons(
        &self,
        c: &mut Canvas,
        view: &ProjectMenuView,
        i: usize,
        buttons: &[(RowButton, Rect)],
        confirm: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        for &(button, rect) in buttons {
            let hot = view.hover == Some(MenuHit::Button(i, button));
            if hot {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, CAP_RADIUS * s, mix(p.selected, p.text, 0.12));
            }
            let color = match button {
                RowButton::Forget if confirm => p.warning,
                _ if hot => p.text,
                _ => p.muted,
            };
            icon_sdf(c, rect, scale, color, |x, y| match button {
                RowButton::Settings => commands_prompt(x, y, s),
                RowButton::Rename => pencil(x, y, s),
                RowButton::Stop => stop(x, y, s),
                RowButton::Forget => cross(x, y, s),
            });
        }
    }

    /// The project menu's settings form, its parts at `layout` moved `by`
    /// into the buffer: the labelled start and stop command fields, where
    /// they run, and the buttons.
    #[allow(clippy::too_many_arguments)]
    fn menu_settings(
        &self,
        c: &mut Canvas,
        form: &SettingsView,
        hover: Option<MenuHit>,
        layout: &SettingsLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let focused = |field: SettingsField| form.focus == field;
        let hovered = |hit: MenuHit| hover == Some(hit);
        for (label, text, placeholder, rect, field) in [
            (
                "Start",
                &form.before,
                "e.g. docker compose up -d",
                layout.before,
                SettingsField::Before,
            ),
            (
                "Stop",
                &form.after,
                "e.g. docker compose down",
                layout.after,
                SettingsField::After,
            ),
        ] {
            let rect = shift(rect);
            let x = px(rect.loc.x - FORM_LABEL_WIDTH) as f32;
            let end = px(rect.loc.x - FORM_GAP) as f32;
            let label_band = band(rect, scale);
            self.text(c, label, FONT_SIZE, x, end, label_band, scale, p.muted);
            if focused(field) {
                focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            self.field(c, text, placeholder, rect, focused(field), scale, p);
        }

        let note = shift(layout.note);
        let (x, right) = phys(note, scale);
        self.text(
            c,
            &format!("Run with sh -c in {}", form.dir),
            SMALL_FONT_SIZE,
            x,
            right,
            band(note, scale),
            scale,
            p.muted,
        );

        let [(cancel, _), (run, _), (save, _)] = SETTINGS_BUTTONS;
        for (label, rect, field, primary) in [
            (cancel, layout.cancel, SettingsField::Cancel, false),
            (run, layout.run, SettingsField::Run, false),
            (save, layout.save, SettingsField::Save, true),
        ] {
            let state = (hovered(MenuHit::Setting(field)), focused(field));
            self.button(c, label, shift(rect), state, primary, scale, p);
        }
    }

    /// The project menu's form, its parts at `layout` moved `by` into the
    /// buffer: the labelled name and folder fields, Browse, the folders
    /// offered, the "Opens" choices, the error line and the buttons.
    #[allow(clippy::too_many_arguments)]
    fn menu_form(
        &self,
        c: &mut Canvas,
        form: &FormView,
        hover: Option<MenuHit>,
        layout: &FormLayout,
        by: Point<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let (name, folder_rect) = (shift(layout.name), shift(layout.folder));
        let kinds = layout.kinds.map(|(kind, rect)| (kind, shift(rect)));
        // A label in the column left of the row holding `rect`.
        let label = |c: &mut Canvas, text: &str, rect: Rect| {
            let x = px(rect.loc.x - FORM_LABEL_WIDTH) as f32;
            let end = px(rect.loc.x - FORM_GAP) as f32;
            self.text(
                c,
                text,
                FONT_SIZE,
                x,
                end,
                band(rect, scale),
                scale,
                p.muted,
            );
        };
        let focused = |field: FormField| form.focus == field;
        let hovered = |hit: MenuHit| hover == Some(hit);

        label(c, "Name", name);
        if focused(FormField::Name) {
            focus_ring(c, name, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let name_focus = focused(FormField::Name);
        self.field(c, &form.name, "Project name", name, name_focus, scale, p);

        label(c, "Folder", folder_rect);
        let folder_focus = focused(FormField::Folder);
        if folder_focus {
            focus_ring(c, folder_rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let placeholder = "None — add later";
        self.field(
            c,
            &form.folder,
            placeholder,
            folder_rect,
            folder_focus,
            scale,
            p,
        );
        self.button(
            c,
            "Browse…",
            shift(layout.browse),
            (
                hovered(MenuHit::Field(FormField::Browse)),
                focused(FormField::Browse),
            ),
            false,
            scale,
            p,
        );

        // The folders offered, in a box hanging under the folder field.
        if let (Some(first), Some(last)) = (layout.suggestions.first(), layout.suggestions.last()) {
            let (first, last) = (shift(*first), shift(*last));
            let (x, y, w, _) = rect_px(first, scale);
            let h = px(last.loc.y + last.size.h) as f32 - y;
            let (b, r) = (px(1).max(1) as f32, BUTTON_RADIUS * s);
            c.rounded(x, y, w, h, r, p.border);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
            for (i, (rect, path)) in layout.suggestions.iter().zip(&form.suggestions).enumerate() {
                let rect = shift(*rect);
                let (x, y, w, h) = rect_px(rect, scale);
                let (x, y, w, h) = (x + 2.0 * b, y + 2.0 * b, w - 4.0 * b, h - 4.0 * b);
                if form.suggestion == Some(i) {
                    c.rounded(x, y, w, h, r - 2.0 * b, p.selected);
                } else if hovered(MenuHit::Suggestion(i)) {
                    c.rounded(x, y, w, h, r - 2.0 * b, mix(p.element, p.selected, 0.5));
                }
                let mark = Rectangle::new(
                    (rect.loc.x + FORM_GAP, rect.loc.y).into(),
                    (MENU_MARK_WIDTH, rect.size.h).into(),
                );
                let small = 0.7 * s;
                icon_sdf(c, mark, scale, p.placeholder, |x, y| folder(x, y, small));
                let text_x = px(mark.loc.x + MENU_MARK_WIDTH + FORM_GAP) as f32;
                let end = phys(rect, scale).1 - PADDING * s;
                self.text(
                    c,
                    path,
                    SMALL_FONT_SIZE,
                    text_x,
                    end,
                    band(rect, scale),
                    scale,
                    p.text,
                );
            }
        }

        // What the folder opens as: a pair of round choices, dimmed without
        // a folder.
        label(c, "Opens", kinds[0].1);
        let enabled = !form.folder.trim().is_empty();
        for ((kind, rect), (_, text, _)) in kinds.iter().zip(KIND_CHOICES) {
            let (x, y, w, h) = rect_px(*rect, scale);
            let chosen = form.kind == *kind;
            if focused(FormField::Kind) && chosen {
                focus_ring(c, *rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
            }
            if enabled && hovered(MenuHit::Kind(*kind)) {
                c.rounded(
                    x,
                    y,
                    w,
                    h,
                    BUTTON_RADIUS * s,
                    mix(p.elevated, p.selected, 0.5),
                );
            }
            let side = px(RADIO_SIZE) as f32;
            let (bx, by) = (x + px(6) as f32, (y + (h - side) / 2.0).round());
            let b = px(1).max(1) as f32;
            let accent = if enabled {
                p.accent
            } else {
                mix(p.accent, p.elevated, 0.5)
            };
            if chosen {
                c.rounded(bx, by, side, side, side / 2.0, accent);
                let dot = (6.0 * s).round();
                let (dx, dy) = (bx + (side - dot) / 2.0, by + (side - dot) / 2.0);
                c.rounded(dx, dy, dot, dot, dot / 2.0, p.on_accent);
            } else {
                let edge = if enabled { p.border } else { p.border_variant };
                c.rounded(bx, by, side, side, side / 2.0, edge);
                let inner = side - 2.0 * b;
                c.rounded(bx + b, by + b, inner, inner, inner / 2.0, p.element);
            }
            self.text(
                c,
                text,
                FONT_SIZE,
                bx + side + (FORM_GAP as f32 * s).round(),
                x + w,
                band(*rect, scale),
                scale,
                if enabled { p.text } else { p.placeholder },
            );
        }

        if let Some(error) = &form.error {
            let error_rect = shift(layout.error);
            let (x, right) = phys(error_rect, scale);
            self.text(
                c,
                error,
                SMALL_FONT_SIZE,
                x,
                right,
                band(error_rect, scale),
                scale,
                p.warning,
            );
        }

        let [(cancel_label, _), (create_label, _)] = FORM_BUTTONS;
        for (label, rect, field, primary) in [
            (cancel_label, layout.cancel, FormField::Cancel, false),
            (create_label, layout.create, FormField::Create, true),
        ] {
            let state = (hovered(MenuHit::Field(field)), focused(field));
            self.button(c, label, shift(rect), state, primary, scale, p);
        }
    }

    /// A button in `rect` like the quit dialog's, labelled `label`: the
    /// `primary` one in the accent. `(hovered, focused)` lightens it and
    /// rings it.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        c: &mut Canvas,
        label: &str,
        rect: Rect,
        (hovered, focused): (bool, bool),
        primary: bool,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (((scale).round() as f32).max(1.0), BUTTON_RADIUS * s);
        if focused {
            focus_ring(c, rect, BUTTON_RADIUS, p.elevated, scale, p.accent);
        }
        let color = if primary {
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
            p.text
        };
        self.centered(c, label, FONT_SIZE, rect, scale, color);
    }

    /// How wide the notice showing `text` is, at most `max`.
    fn notice_width(&self, text: &str, max: i32, scale: f64) -> i32 {
        let text_w = (self.text_width(text, FONT_SIZE, scale) / scale as f32).ceil() as i32;
        (text_w + 2 * NOTICE_PADDING).min(max).max(NOTICE_HEIGHT)
    }

    /// The notice: `text` on a pill `w` wide, inside a margin of `SHADOW`
    /// holding its drop shadow.
    fn notice(&self, text: &str, w: i32, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = Size::from((w, NOTICE_HEIGHT));
        let outer = Size::from((w + 2 * SHADOW, NOTICE_HEIGHT + 2 * SHADOW));
        let mut c = Canvas::new(physical(outer, scale));
        c.popup(size, NOTICE_HEIGHT as f32 / 2.0, scale, p);
        let rect = Rectangle::new(
            (SHADOW + NOTICE_PADDING, SHADOW).into(),
            ((w - 2 * NOTICE_PADDING).max(0), NOTICE_HEIGHT).into(),
        );
        let (x, right) = phys(rect, scale);
        // A pixel of slack for the rounding of the measured width.
        let right = right + scale.ceil() as f32;
        self.text(
            &mut c,
            text,
            FONT_SIZE,
            x,
            right,
            band(rect, scale),
            scale,
            p.text,
        );
        c.into_buffer(false)
    }

    /// A terminal pane's bar: a mark saying how its command is, the command
    /// and a note after it, then the restart and stop buttons.
    fn pane_bar(&self, view: &PaneBarView, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = view.rect.size;
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let line = px(1).max(1);
        let background = if view.focused {
            p.tab_active
        } else {
            p.tab_bar
        };
        c.fill(0, 0, c.w, c.h, background);
        c.fill(0, c.h - line, c.w, line, p.border_variant);

        let bar = Rectangle::from_size(size);
        let buttons = pane_bar_buttons(bar);
        let mark = Rectangle::new((PADDING as i32, 0).into(), (STATUS_WIDTH, size.h).into());
        match view.status {
            BarStatus::Idle => {}
            BarStatus::Running => {
                icon_sdf(&mut c, mark, scale, p.success, |x, y| x.hypot(y) - 3.4 * s)
            }
            BarStatus::Exited => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                (x.hypot(y) - 3.4 * s).abs() - STROKE * s
            }),
            BarStatus::Working => icon_sdf(&mut c, mark, scale, p.warning, |x, y| {
                status_mark(AgentStatus::Working, x, y, s)
            }),
        }

        // The note keeps its room; the label is cut short before it.
        let band = (0, px(size.h));
        let x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let right = px(buttons[0].1.loc.x - BAR_NOTE_GAP) as f32;
        let note_w = view.note.map_or(0.0, |note| {
            self.text_width(note, DETAIL_FONT_SIZE, scale) + px(BAR_NOTE_GAP) as f32
        });
        let (label_color, note_color) = match view.status {
            BarStatus::Running => (p.text, p.muted),
            BarStatus::Working => (p.text, p.warning),
            BarStatus::Idle | BarStatus::Exited => (p.muted, p.placeholder),
        };
        let end = self.text(
            &mut c,
            &view.label,
            DETAIL_FONT_SIZE,
            x,
            (right - note_w).max(x),
            band,
            scale,
            label_color,
        );
        if let Some(note) = view.note {
            self.text(
                &mut c,
                note,
                DETAIL_FONT_SIZE,
                end + px(BAR_NOTE_GAP) as f32,
                right,
                band,
                scale,
                note_color,
            );
        }

        for (button, rect) in buttons {
            let enabled = match button {
                BarButton::Restart => view.can_restart,
                BarButton::Stop => view.can_stop,
            };
            let hovered = enabled && view.hover == Some(button);
            if hovered {
                let inset = Rectangle::new(
                    (rect.loc.x + 2, rect.loc.y + 2).into(),
                    (rect.size.w - 4, (rect.size.h - 4).max(0)).into(),
                );
                let (x, y, w, h) = rect_px(inset, scale);
                c.rounded(x, y, w, h, 4.0 * s, p.selected);
            }
            let color = if !enabled {
                mix(p.placeholder, background, 0.5)
            } else if hovered {
                p.text
            } else {
                p.muted
            };
            match button {
                BarButton::Restart => {
                    icon_sdf(&mut c, rect, scale, color, |x, y| restart_arrow(x, y, s))
                }
                BarButton::Stop => icon_sdf(&mut c, rect, scale, color, |x, y| stop(x, y, s)),
            }
        }
        c.into_buffer(true)
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

        let (mut shown, mut cut) = (text, false);
        if x + self.text_width(text, size, scale) > max_x {
            // The longest start of `text` that fits with an ellipsis after it.
            let ellipsis = self.text_width("…", size, scale);
            let mut width = 0.0;
            let mut end = 0;
            for (i, ch) in text.char_indices() {
                width += scaled.h_advance(scaled.glyph_id(ch));
                if x + width + ellipsis > max_x {
                    break;
                }
                end = i + ch.len_utf8();
            }
            (shown, cut) = (&text[..end], true);
        }

        let mut pen = x;
        for ch in shown.chars().chain(cut.then_some('…')) {
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

    /// The byte of `line`, drawn by `edit_line` between `x` and `max_x`,
    /// nearest `at` (physical pixels).
    fn caret_at(&self, line: &Line, (x, max_x): (f32, f32), at: f32, scale: f64) -> usize {
        let width = |text: &str| self.text_width(text, FONT_SIZE, scale);
        let scrolled = (width(&line[..line.caret]) - (max_x - x)).max(0.0);
        let mut pen = x - scrolled;
        for (i, ch) in line.char_indices() {
            let advance = width(ch.encode_utf8(&mut [0; 4]));
            if at < pen + advance / 2.0 {
                return i;
            }
            pen += advance;
        }
        line.len()
    }

    /// Draw `text` from `x` as `text` does, but only the characters wholly
    /// between `min_x` and `max_x`: a line scrolled in a field.
    #[allow(clippy::too_many_arguments)]
    fn clipped_text(
        &self,
        c: &mut Canvas,
        text: &str,
        x: f32,
        (min_x, max_x): (f32, f32),
        (top, height): (i32, i32),
        scale: f64,
        color: Rgb,
    ) {
        let Some(font) = &self.font else { return };
        let scaled = font.as_scaled(PxScale::from(FONT_SIZE * scale as f32));
        let cap_height = font
            .outline_glyph(scaled.scaled_glyph('H'))
            .map_or(scaled.ascent(), |o| -o.px_bounds().min.y);
        let baseline = (top as f32 + (height as f32 + cap_height) / 2.0).round();
        let mut pen = x;
        for ch in text.chars() {
            let id = scaled.glyph_id(ch);
            let advance = scaled.h_advance(id);
            if pen + advance > max_x {
                break;
            }
            let glyph = id.with_scale_and_position(scaled.scale(), ab_glyph::point(pen, baseline));
            if pen >= min_x
                && let Some(outline) = font.outline_glyph(glyph)
            {
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
            pen += advance;
        }
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

/// The focused text field of an overlay: the area a click in it lands in,
/// where its text goes (physical pixels, from and up to), and what it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpot {
    pub area: Rect,
    pub text: (f32, f32),
    pub line: Line,
}

/// Where the text of a field drawn in `rect` (by `Painter::field`) goes.
fn field_text(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let (x, _, w, _) = rect_px(rect, scale);
    let pad = PADDING * scale as f32;
    (x + pad, x + w - pad - px(CARET_WIDTH + 1))
}

/// Where the text of the project menu's query field in `rect` goes, after
/// its magnifier.
fn menu_query_text(rect: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    let mark_x = rect.loc.x + PROJECT_MENU_PADDING + PADDING as i32;
    (
        px(mark_x + MENU_MARK_WIDTH + MENU_MARK_GAP),
        px(rect.loc.x + rect.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
    )
}

/// Where the text of the switcher's query, in its `input` rect, goes.
fn switcher_text(input: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (
        px(input.loc.x + HELP_PADDING),
        px(input.loc.x + input.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
    )
}

/// The field a project menu entry in `rect` turns into while its project is
/// renamed: over the name, up to the row's `buttons` if it shows them.
fn rename_field(rect: Rect, buttons: &[(RowButton, Rect)]) -> Rect {
    let x = rect.loc.x + PADDING as i32 + MENU_MARK_WIDTH + MENU_MARK_GAP - RENAME_INSET;
    let end = buttons
        .first()
        .map_or(rect.loc.x + rect.size.w - PADDING as i32, |(_, b)| {
            b.loc.x - FIELD_GAP
        });
    Rectangle::new(
        (x, rect.loc.y + (rect.size.h - RENAME_FIELD) / 2).into(),
        ((end - x).max(0), RENAME_FIELD).into(),
    )
}

/// The switcher's query field, in the app `area`.
pub fn switcher_field(view: &SwitcherView, area: Rect, scale: f64) -> FieldSpot {
    let (_, input, _) = switcher_layout(view.rows.len(), area);
    FieldSpot {
        area: input,
        text: switcher_text(input, scale),
        line: view.query.clone(),
    }
}

/// The welcome screen's text field with the keyboard, on an output of
/// `size`: the new project's name, while asked for. (The filter, typed
/// over the list, is no field to click in.)
pub fn welcome_field(
    view: &WelcomeView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let line = view.naming.clone()?;
    let area = welcome_layout(size, view.rows.len(), true).button;
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        line,
    })
}

/// The project menu's text field with the keyboard, on an output of `size`.
pub fn project_menu_field(
    view: &ProjectMenuView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let layout = project_menu_layout(view, size);
    let boxed = |area: Rect, line: &Line| FieldSpot {
        area,
        text: field_text(area, scale),
        line: line.clone(),
    };
    if let (Some(form), Some(rects)) = (&view.form, &layout.form) {
        return match form.focus {
            FormField::Name => Some(boxed(rects.name, &form.name)),
            FormField::Folder => Some(boxed(rects.folder, &form.folder)),
            _ => None,
        };
    }
    if let (Some(settings), Some(rects)) = (&view.settings, &layout.settings) {
        return match settings.focus {
            SettingsField::Before => Some(boxed(rects.before, &settings.before)),
            SettingsField::After => Some(boxed(rects.after, &settings.after)),
            _ => None,
        };
    }
    let renamed = view
        .entries
        .iter()
        .zip(&layout.entries)
        .find_map(|(entry, (rect, buttons))| match entry {
            MenuEntry::Project {
                renaming: Some(name),
                ..
            } => Some(boxed(rename_field(*rect, buttons), name)),
            _ => None,
        });
    renamed.or_else(|| {
        Some(FieldSpot {
            area: layout.query,
            text: menu_query_text(layout.query, scale),
            line: view.query.clone(),
        })
    })
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

/// Signed distance to a magnifying glass centred on the origin: a ring and
/// its handle to the bottom right.
fn magnifier(x: f32, y: f32, s: f32) -> f32 {
    let ring = ((x + 1.0 * s).hypot(y + 1.0 * s) - 4.5 * s).abs() - STROKE * s;
    let handle = segment(x, y, 2.4 * s, 2.4 * s, 5.5 * s, 5.5 * s) - 1.1 * s;
    ring.min(handle)
}

/// Signed distance to a pencil centred on the origin, pointing to the
/// bottom left: an outlined body and its pointed tip.
fn pencil(x: f32, y: f32, s: f32) -> f32 {
    // Along the pencil (towards the top right) and across it.
    let (along, across) = (
        (x - y) * std::f32::consts::FRAC_1_SQRT_2,
        (x + y) * std::f32::consts::FRAC_1_SQRT_2,
    );
    let body = rounded_rect(along, across, -2.5 * s, -2.2 * s, 8.5 * s, 4.4 * s, 0.6 * s).abs()
        - STROKE * s;
    let tip = segment(along, across, -2.5 * s, -2.2 * s, -6.2 * s, 0.0).min(segment(
        along,
        across,
        -2.5 * s,
        2.2 * s,
        -6.2 * s,
        0.0,
    )) - STROKE * s;
    body.min(tip)
}

/// Signed distance to a prompt, ">_", centred on the origin: a project's
/// commands.
fn commands_prompt(x: f32, y: f32, s: f32) -> f32 {
    let chevron = segment(x, y, -4.2 * s, -3.6 * s, -0.6 * s, 0.0).min(segment(
        x,
        y,
        -0.6 * s,
        0.0,
        -4.2 * s,
        3.6 * s,
    ));
    let line = segment(x, y, 1.0 * s, 3.6 * s, 4.6 * s, 3.6 * s);
    chevron.min(line) - STROKE * s
}

/// Signed distance to a filled square centred on the origin: stop.
fn stop(x: f32, y: f32, s: f32) -> f32 {
    rounded_rect(x, y, -3.8 * s, -3.8 * s, 7.6 * s, 7.6 * s, 1.2 * s)
}

/// Signed distance to a circling arrow centred on the origin: run again. A
/// ring open at its top right, its top end an arrow head pointing into the
/// gap.
fn restart_arrow(x: f32, y: f32, s: f32) -> f32 {
    let r = 4.2 * s;
    let ring = ((x.hypot(y) - r).abs() - STROKE * s).max(x.min(-y));
    let (tip_x, tip_y) = (1.4 * s, -r);
    let head = segment(x, y, tip_x, tip_y, tip_x - 2.4 * s, tip_y - 2.4 * s).min(segment(
        x,
        y,
        tip_x,
        tip_y,
        tip_x - 2.4 * s,
        tip_y + 2.4 * s,
    )) - STROKE * s;
    ring.min(head)
}

/// Signed distance to a "×" centred on the origin: forget.
fn cross(x: f32, y: f32, s: f32) -> f32 {
    let arm = 3.8 * s;
    segment(x, y, -arm, -arm, arm, arm).min(segment(x, y, -arm, arm, arm, -arm)) - STROKE * s
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
        let (x0, x1) = (x.max(0), (x + w).min(self.w));
        if x0 >= x1 {
            return;
        }
        for py in y.max(0)..(y + h).min(self.h) {
            let row = ((py * self.w + x0) * 4) as usize..((py * self.w + x1) * 4) as usize;
            for pixel in self.px[row].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&[b, g, r, 0xff]);
            }
        }
    }

    /// Lay `color` over the pixel at `coverage` opacity.
    fn blend(&mut self, x: i32, y: i32, [r, g, b]: Rgb, coverage: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let a = coverage.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        if a >= 1.0 {
            self.px[i..i + 4].copy_from_slice(&[b, g, r, 0xff]);
            return;
        }
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
        let at = |px: i32, py: i32| {
            coverage(rounded_rect(
                px as f32 + 0.5,
                py as f32 + 0.5,
                x,
                y,
                w,
                h,
                r,
            ))
        };
        let (x0, x1) = (
            (x.floor() as i32).max(0),
            ((x + w).ceil() as i32).min(self.w),
        );
        for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(self.h) {
            // The shape is convex: blend the edges in from both ends of the
            // row up to the first fully covered pixel, and fill between.
            let mut left = x0;
            while left < x1 {
                let a = at(left, py);
                if a >= 1.0 {
                    break;
                }
                if a > 0.0 {
                    self.blend(left, py, color, a);
                }
                left += 1;
            }
            let mut right = x1;
            while right > left {
                let a = at(right - 1, py);
                if a >= 1.0 {
                    break;
                }
                if a > 0.0 {
                    self.blend(right - 1, py, color, a);
                }
                right -= 1;
            }
            self.fill(left, py, right - left, 1, color);
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
        BAR_HEIGHT, BAR_INSET, BarButton, BarHit, BarStatus, FormField, FormView, HELP_MARGIN,
        Line, MenuEntry, MenuHit, NOTICE_HEIGHT, PANE_BAR_HEIGHT, PROJECT_MENU_WIDTH, Painter,
        PaneBarView, ProjectMenuView, ProjectRow, QuitHit, QuitView, ROW_INSET, Rect, RowButton,
        SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH, SettingsField, SettingsView, TabView,
        Theme, WelcomeHit, WelcomeView, WorkspaceView, pane_bar_buttons, pane_bar_hit,
        pane_bar_rects, project_menu_field, project_menu_hit, project_menu_layout,
        project_menu_page, quit_layout, short_path, welcome_layout, welcome_page,
    };
    use crate::{WorkspaceKind, herdr::AgentStatus};
    use smithay::utils::{Logical, Point, Size};
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
    fn pane_bar_takes_the_top_and_leaves_the_rest_to_the_window() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects(pane);
        assert_eq!(
            bar,
            Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into())
        );
        assert_eq!(
            client,
            Rect::new(
                (200, 50 + PANE_BAR_HEIGHT).into(),
                (600, 400 - PANE_BAR_HEIGHT).into()
            )
        );
    }

    #[test]
    fn tiny_pane_keeps_a_window() {
        for h in [0, 1, 10, PANE_BAR_HEIGHT] {
            let pane = Rect::new((0, 0).into(), (300, h).into());
            let (bar, client) = pane_bar_rects(pane);
            assert!(client.size.h >= 1, "{h}");
            assert!(bar.size.h >= 0 && bar.size.h < h.max(1), "{h}");
        }
    }

    #[test]
    fn pane_bar_buttons_sit_right_and_do_not_overlap() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        let [(first, restart), (second, stop)] = pane_bar_buttons(bar);
        assert_eq!((first, second), (BarButton::Restart, BarButton::Stop));
        assert_eq!(stop.loc.x + stop.size.w, 200 + 600 - BAR_INSET);
        assert!(restart.loc.x + restart.size.w <= stop.loc.x);
        assert!(restart.loc.y == 50 && stop.size.h == PANE_BAR_HEIGHT);
    }

    #[test]
    fn pane_bar_hit_agrees_with_the_buttons() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        for (button, rect) in pane_bar_buttons(bar) {
            let centre = rect.to_f64().loc + rect.to_f64().size.downscale(2.0).to_point();
            assert_eq!(pane_bar_hit(bar, centre), Some(BarHit::Button(button)));
        }
        assert_eq!(pane_bar_hit(bar, (210.0, 60.0).into()), Some(BarHit::Label));
        assert_eq!(pane_bar_hit(bar, (210.0, 80.0).into()), None);
        assert_eq!(pane_bar_hit(bar, (100.0, 60.0).into()), None);
    }

    #[test]
    fn pane_bars_paint() {
        let painter = Painter::new();
        let statuses = [
            BarStatus::Idle,
            BarStatus::Running,
            BarStatus::Exited,
            BarStatus::Working,
        ];
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for w in [600, 60, 1] {
                    for (i, status) in statuses.into_iter().enumerate() {
                        let view = PaneBarView {
                            rect: Rect::new((0, 0).into(), (w, PANE_BAR_HEIGHT).into()),
                            label: "npm run dev -- --port 3000 --host 0.0.0.0".into(),
                            note: (i % 2 == 0).then_some("restarting…"),
                            status,
                            can_restart: i < 2,
                            can_stop: i % 2 == 1,
                            focused: i == 0,
                            hover: Some(BarButton::Restart),
                        };
                        painter.pane_bar(&view, scale, theme.palette());
                    }
                }
            }
        }
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
        view.naming = Some(Line::default());
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

    fn menu_row(name: &str, running: Option<bool>, current: bool) -> ProjectRow {
        ProjectRow {
            name: name.into(),
            detail: "3 repos · ~/Work/aft".into(),
            ago: "2h ago".into(),
            running,
            current,
        }
    }

    fn project(row: ProjectRow, digit: Option<u8>) -> MenuEntry {
        MenuEntry::Project {
            row,
            digit,
            renaming: None,
            confirm_forget: false,
        }
    }

    /// The list: the current project, three recent ones (running, stopped,
    /// not known yet), then the rows starting new ones.
    fn list_view() -> ProjectMenuView {
        ProjectMenuView {
            query: "blo".into(),
            entries: vec![
                MenuEntry::Section("Current".into()),
                project(menu_row("aft", Some(true), true), None),
                MenuEntry::Section("Recent".into()),
                project(menu_row("gerdr", Some(true), false), Some(1)),
                project(menu_row("blog", Some(false), false), Some(2)),
                project(menu_row("notes", None, false), Some(3)),
                MenuEntry::New("New project \"blo\"…".into()),
                MenuEntry::Folder("~/Sites/blogging".into()),
            ],
            selected: Some(3),
            hover: None,
            form: None,
            settings: None,
            hints: vec![
                ("Enter".into(), "open".into()),
                ("Ctrl+R".into(), "rename".into()),
                ("Ctrl+S".into(), "stop".into()),
                ("Del".into(), "forget".into()),
                ("Esc".into(), "close".into()),
            ],
        }
    }

    fn form_view(suggestions: usize, error: bool) -> ProjectMenuView {
        ProjectMenuView {
            query: Line::default(),
            entries: Vec::new(),
            selected: None,
            hover: None,
            form: Some(FormView {
                name: "blog-v2".into(),
                folder: "~/Sites/blog".into(),
                kind: WorkspaceKind::Project,
                focus: FormField::Folder,
                suggestions: (0..suggestions)
                    .map(|i| format!("~/Sites/blog{i}"))
                    .collect(),
                suggestion: (suggestions > 0).then_some(0),
                error: error.then(|| "A project named blog-v2 exists.".into()),
            }),
            settings: None,
            hints: vec![
                ("Tab".into(), "next".into()),
                ("Enter".into(), "create".into()),
                ("Esc".into(), "back".into()),
            ],
        }
    }

    fn settings_view(before: &str) -> ProjectMenuView {
        ProjectMenuView {
            query: Line::default(),
            entries: Vec::new(),
            selected: None,
            hover: None,
            form: None,
            settings: Some(SettingsView {
                name: "blog".into(),
                before: before.into(),
                after: Line::default(),
                dir: "~/Sites/blog".into(),
                focus: SettingsField::Before,
            }),
            hints: vec![("Esc".into(), "back".into())],
        }
    }

    fn below(upper: Rect, lower: Rect) -> bool {
        upper.loc.y + upper.size.h <= lower.loc.y
    }

    fn centre(rect: Rect) -> Point<f64, Logical> {
        (
            rect.loc.x as f64 + rect.size.w as f64 / 2.0,
            rect.loc.y as f64 + rect.size.h as f64 / 2.0,
        )
            .into()
    }

    #[test]
    fn project_menu_layout_fits_the_output() {
        for (w, h) in [(800, 600), (1920, 1080)] {
            let size: Size<i32, Logical> = (w, h).into();
            let margin = Rect::new((0, 0).into(), (w, h - HELP_MARGIN).into());
            // A full page of rows, with a section label at the top.
            let mut full = list_view();
            full.entries.truncate(1);
            while full.entries.len() < project_menu_page(size) {
                full.entries.push(project(menu_row("x", None, false), None));
            }
            full.hover = Some(MenuHit::Entry(2));
            let mut views = vec![list_view(), full];
            for (suggestions, error) in [(0, false), (5, false), (0, true), (5, true)] {
                views.push(form_view(suggestions, error));
            }
            for view in &views {
                let l = project_menu_layout(view, size);
                assert!(inside(margin, l.panel), "{:?} outside {w}x{h}", l.panel);
                assert!(l.panel.size.w <= PROJECT_MENU_WIDTH);
                assert_eq!(l.panel.loc, (ROW_INSET, BAR_HEIGHT + 2).into());
                assert!(inside(l.panel, l.query) && inside(l.panel, l.footer));
                assert!(below(l.query, l.footer));
                assert_eq!(
                    l.footer.loc.y + l.footer.size.h,
                    l.panel.loc.y + l.panel.size.h
                );
                for (rect, buttons) in &l.entries {
                    assert!(rect.size.h > 0, "{rect:?} cut off at {w}x{h}");
                    assert!(
                        inside(l.panel, *rect) && below(l.query, *rect) && below(*rect, l.footer)
                    );
                    assert!(buttons.iter().all(|(_, b)| inside(*rect, *b)));
                }
                assert!(l.entries.windows(2).all(|e| below(e[0].0, e[1].0)));
                match (&l.form, &view.form) {
                    (None, None) => assert_eq!(l.entries.len(), view.entries.len()),
                    (Some(f), Some(form)) => {
                        assert!(l.entries.is_empty());
                        assert_eq!(f.suggestions.len(), form.suggestions.len());
                        let mut parts =
                            vec![f.name, f.folder, f.browse, f.error, f.cancel, f.create];
                        parts.extend(f.kinds.iter().map(|(_, r)| *r));
                        parts.extend(&f.suggestions);
                        for part in parts {
                            assert!(inside(l.panel, part), "{part:?} outside the panel");
                        }
                        assert!(below(l.query, f.name) && below(f.name, f.folder));
                        assert_eq!(f.folder.loc.y, f.browse.loc.y);
                        assert!(f.folder.loc.x + f.folder.size.w < f.browse.loc.x);
                        let last = f.suggestions.last().copied().unwrap_or(f.folder);
                        assert!(below(f.folder, last) || last == f.folder);
                        assert!(f.suggestions.windows(2).all(|s| below(s[0], s[1])));
                        let [(_, zed), (_, term)] = f.kinds;
                        assert!(below(last, zed) && zed.loc.x + zed.size.w < term.loc.x);
                        assert!(below(zed, f.error) && below(f.error, f.create));
                        assert_eq!(f.cancel.loc.y, f.create.loc.y);
                        assert!(f.cancel.loc.x + f.cancel.size.w < f.create.loc.x);
                        assert!(below(f.create, l.footer));
                    }
                    _ => panic!("form layout without a form, or the other way round"),
                }
            }
        }
    }

    #[test]
    fn project_menu_page_stays_in_bounds() {
        let mut last = 0;
        for h in (100..2200).step_by(10) {
            let page = project_menu_page((1000, h).into());
            assert!((3..=12).contains(&page) && page >= last);
            last = page;
        }
        assert_eq!(project_menu_page((1000, 100).into()), 3);
        assert_eq!(project_menu_page((1920, 1080).into()), 12);
        assert!(project_menu_page((800, 600).into()) > 3);
    }

    #[test]
    fn project_menu_hit_agrees_with_the_layout() {
        let size: Size<i32, Logical> = (1280, 800).into();
        let mut view = list_view();
        let l = project_menu_layout(&view, size);
        let hit = |view: &ProjectMenuView, pos| project_menu_hit(view, size, pos);
        assert!(l.entries.iter().all(|(_, buttons)| buttons.is_empty()));
        assert_eq!(hit(&view, centre(l.query)), Some(MenuHit::Query));
        assert_eq!(hit(&view, centre(l.footer)), Some(MenuHit::Panel));
        assert_eq!(hit(&view, (1200.0, 700.0).into()), None);
        assert_eq!(hit(&view, (2.0, 60.0).into()), None);
        for (i, (rect, _)) in l.entries.iter().enumerate() {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Entry(i)));
        }

        // Row buttons: only on the hovered project row, and which depend on
        // whether it is current and runs.
        use RowButton::{Forget, Rename, Settings, Stop};
        let expected: [&[RowButton]; 8] = [
            &[],
            &[Settings, Rename],
            &[],
            &[Settings, Rename, Stop, Forget],
            &[Settings, Rename, Forget],
            &[Settings, Rename, Stop, Forget],
            &[],
            &[],
        ];
        for (i, want) in expected.iter().enumerate() {
            for hover in [MenuHit::Entry(i), MenuHit::Button(i, Rename)] {
                view.hover = Some(hover);
                let l = project_menu_layout(&view, size);
                for (j, (rect, buttons)) in l.entries.iter().enumerate() {
                    let shown: Vec<_> = buttons.iter().map(|(b, _)| *b).collect();
                    assert_eq!(shown, if i == j { want.to_vec() } else { vec![] });
                    for (button, b) in buttons {
                        assert_eq!(hit(&view, centre(*b)), Some(MenuHit::Button(j, *button)));
                    }
                    // Left of the buttons is still the row.
                    let left = (rect.loc.x as f64 + 20.0, centre(*rect).y).into();
                    assert_eq!(hit(&view, left), Some(MenuHit::Entry(j)));
                }
                // Right-aligned in order, side by side.
                let buttons = &l.entries[i].1;
                assert!(
                    buttons
                        .windows(2)
                        .all(|b| b[0].1.loc.x + b[0].1.size.w <= b[1].1.loc.x)
                );
            }
        }

        let view = form_view(3, true);
        let l = project_menu_layout(&view, size);
        let f = l.form.as_ref().unwrap();
        let fields = [
            (f.name, FormField::Name),
            (f.folder, FormField::Folder),
            (f.browse, FormField::Browse),
            (f.cancel, FormField::Cancel),
            (f.create, FormField::Create),
        ];
        for (rect, field) in fields {
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Field(field)));
        }
        for (kind, rect) in f.kinds {
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Kind(kind)));
        }
        for (i, rect) in f.suggestions.iter().enumerate() {
            assert_eq!(hit(&view, centre(*rect)), Some(MenuHit::Suggestion(i)));
        }
        for nothing in [l.query, f.error, l.footer] {
            assert_eq!(hit(&view, centre(nothing)), Some(MenuHit::Panel));
        }
        let past = l.panel.loc.y as f64 + l.panel.size.h as f64 + 1.0;
        assert_eq!(hit(&view, (100.0, past).into()), None);

        let view = settings_view("docker compose up -d");
        let l = project_menu_layout(&view, size);
        assert!(l.entries.is_empty() && l.form.is_none());
        let f = l.settings.as_ref().unwrap();
        assert!(below(l.query, f.before) && below(f.before, f.after));
        assert!(below(f.after, f.note) && below(f.note, f.save));
        assert!(below(f.save, l.footer));
        assert!(f.cancel.loc.x + f.cancel.size.w < f.run.loc.x);
        assert!(f.run.loc.x + f.run.size.w < f.save.loc.x);
        let fields = [
            (f.before, SettingsField::Before),
            (f.after, SettingsField::After),
            (f.cancel, SettingsField::Cancel),
            (f.run, SettingsField::Run),
            (f.save, SettingsField::Save),
        ];
        for (rect, field) in fields {
            assert!(inside(l.panel, rect), "{rect:?} outside the panel");
            assert_eq!(hit(&view, centre(rect)), Some(MenuHit::Setting(field)));
        }
        for nothing in [l.query, f.note, l.footer] {
            assert_eq!(hit(&view, centre(nothing)), Some(MenuHit::Panel));
        }
    }

    #[test]
    fn project_menu_header_and_notice_paint() {
        let painter = Painter::new();
        let mut list = list_view();
        list.hover = Some(MenuHit::Button(4, RowButton::Forget));
        if let MenuEntry::Project { confirm_forget, .. } = &mut list.entries[4] {
            *confirm_forget = true;
        }
        let mut renaming = list_view();
        renaming.hover = Some(MenuHit::Entry(3));
        if let MenuEntry::Project { renaming: name, .. } = &mut renaming.entries[3] {
            *name = Some("gerdr renamed".into());
        }
        let mut empty = list_view();
        empty.entries.clear();
        empty.query = Line::default();
        let mut views = vec![
            list,
            renaming,
            empty,
            form_view(5, true),
            form_view(0, false),
        ];
        let mut no_folder = form_view(0, false);
        if let Some(form) = &mut no_folder.form {
            form.folder = Line::default();
            form.focus = FormField::Kind;
        }
        views.push(no_folder);
        views.push(settings_view(""));
        let mut long_line = settings_view(&"docker compose up -d && ".repeat(8));
        long_line.hover = Some(MenuHit::Setting(SettingsField::Run));
        views.push(long_line);
        let long = "a project with a very long name indeed, longer than the header";
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for (w, h) in [(800, 600), (1920, 1080), (120, 90)] {
                    let size = (w, h).into();
                    for view in &views {
                        painter.project_menu(view, size, scale, theme.palette());
                    }
                    painter.notice(
                        "Switched to gerdr · aft keeps running",
                        w / 2,
                        scale,
                        theme.palette(),
                    );
                }
                for (name, lit) in [(Some(long), true), (Some("aft"), false), (None, false)] {
                    for width in [SIDEBAR_MIN_WIDTH, SIDEBAR_WIDTH, SIDEBAR_MAX_WIDTH] {
                        let size = (width, 400).into();
                        painter.sidebar(
                            &[],
                            &[],
                            None,
                            false,
                            (name, lit),
                            size,
                            scale,
                            theme.palette(),
                        );
                    }
                }
            }
        }
        assert!(painter.notice_width("Created blog-v2", 10, 1.0) >= NOTICE_HEIGHT);
    }

    #[test]
    fn the_focused_field_is_found_where_it_is_drawn() {
        let size = Size::from((1280, 800));
        let field = |view: &ProjectMenuView| project_menu_field(view, size, 1.0).unwrap();
        let list = list_view();
        let l = project_menu_layout(&list, size);
        assert_eq!(field(&list).area, l.query);
        assert_eq!(field(&list).line, "blo");

        let mut renaming = list_view();
        renaming.hover = Some(MenuHit::Entry(3));
        if let MenuEntry::Project { renaming: name, .. } = &mut renaming.entries[3] {
            *name = Some("gerdr x".into());
        }
        let r = project_menu_layout(&renaming, size);
        let (row, buttons) = &r.entries[3];
        let spot = field(&renaming);
        assert_eq!(spot.line, "gerdr x");
        assert!(row.contains(spot.area.loc) && spot.area.size.w > 0);
        // It stops short of the row's buttons.
        let first = buttons.first().expect("the hovered row shows buttons").1;
        assert!(spot.area.loc.x + spot.area.size.w < first.loc.x);

        let mut form = form_view(0, false);
        let f = project_menu_layout(&form, size).form.unwrap();
        form.form.as_mut().unwrap().focus = FormField::Name;
        assert_eq!(field(&form).area, f.name);
        form.form.as_mut().unwrap().focus = FormField::Folder;
        assert_eq!(field(&form).area, f.folder);
        form.form.as_mut().unwrap().focus = FormField::Create;
        assert_eq!(project_menu_field(&form, size, 1.0), None);

        let mut settings = settings_view("make up");
        let st = project_menu_layout(&settings, size).settings.unwrap();
        assert_eq!(field(&settings).area, st.before);
        settings.settings.as_mut().unwrap().focus = SettingsField::After;
        assert_eq!(field(&settings).area, st.after);
    }

    #[test]
    fn a_click_puts_the_caret_by_the_nearest_character() {
        let painter = Painter::new();
        if painter.font.is_none() {
            return;
        }
        let width = |text: &str| painter.text_width(text, super::FONT_SIZE, 1.0);
        let line = Line::from("abc def");
        let (x, max_x) = (100.0, 1000.0);
        let at = |px: f32| painter.caret_at(&line, (x, max_x), px, 1.0);
        assert_eq!(at(0.0), 0);
        assert_eq!(at(x + width("ab") + 1.0), 2);
        assert_eq!(at(x + width("abc ") - 1.0), 4);
        assert_eq!(at(2000.0), 7);
        // Scrolled to keep the caret in view, the text starts left of `x`.
        let long = Line::from("a long line typed past the end of a narrow field");
        let narrow = (x, x + width("a long line"));
        let end = painter.caret_at(&long, narrow, narrow.1, 1.0);
        assert_eq!(end, long.len());
        assert!(painter.caret_at(&long, narrow, x, 1.0) > 0);
    }
}
