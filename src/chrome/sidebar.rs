//! The workspace sidebar: the views of workspaces and agent cards, where
//! they go, and how they are drawn.
//!
//! A workspace row is two lines: its name with a status chip at the right
//! (one dot and a word for the most pressing agent, a filled badge with a
//! count when agents are blocked), then its branch and place, and at the
//! right how many other worktrees its repository has; then a line per
//! worktree that needs a look (a few at most). Under it, a
//! compact card per agent: a header line and a detail line, the most
//! pressing first. The header holds the project's name, a button opening
//! the repositories panel and a "+" button opening the menu of what to
//! add. What moves (hover pills, the pulse on a blocked card, a name being
//! renamed) comes in through `SidebarState`.

use super::{
    BAR_HEIGHT, BRANCH_ICON_GAP, BRANCH_ICON_WIDTH, CARD_GAP, CARD_HEADER, CARD_INDENT, CARD_LINE,
    CARD_PAD, CARD_RADIUS, CHEVRON_WIDTH, CLOSE_WIDTH, Canvas, DETAIL_FONT_SIZE, Density,
    FONT_SIZE, FieldSpot, GAP, HEADER_BUTTON, HEADER_MARGIN, HEADER_NAME_END, HEADER_NAME_GAP,
    LABEL_FONT_SIZE, PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette, ROW_BOTTOM, ROW_HEIGHT,
    ROW_ICON_GAP, ROW_ICON_WIDTH, ROW_INSET, ROW_PICTURE_SIZE, ROW_RADIUS, ROW_TITLE_HEIGHT,
    ROWS_TOP, Rect, SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, TAB_LINE, TINY_FONT_SIZE, TabView,
    app, band, chevron_down, cross, distinct, folder, git_branch, icon_sdf, mix, new_tab, physical,
    rect_px, status_mark, terminal,
};
use crate::WorkspaceKind;
use crate::agents::AgentKey;
use crate::app_icons::AppIcon;
use crate::cursor_agents::CursorChat;
use crate::git::{GitStatus, Worktree};
use crate::herdr::{AgentInfo, AgentStatus, PaneState};
use crate::paths::{repo_root, short_path, tidy_title};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};
use std::path::PathBuf;

/// How many steps a hover or pulse alpha in `SidebarState` has: the value
/// `anim::quantize` gives 1.0.
const ALPHA_STEPS: f32 = 16.0;
/// Room for the status chip at the right of a row's name line, which it
/// shares with the "×": while the row is hovered the chip shrinks to its
/// dot and the "×" shows, so the name never moves.
const CHIP_WIDTH: i32 = 66;
/// The chip's own part of that room, left of the "×".
const CHIP_SLOT: i32 = CHIP_WIDTH - CLOSE_WIDTH;
/// The chip's inner padding, and the gap between its dot and its word.
const CHIP_PADDING: i32 = 6;
/// Room for the count of other worktrees at the right end of line 2.
const WORKTREE_COUNT_WIDTH: i32 = 30;
/// How far the count's hover pill reaches past it on each side.
const COUNT_PILL_PAD: i32 = 4;
/// The most lines a row gives worktrees that need a look; a last line
/// counts the rest.
const WORKTREE_LINES: usize = 3;
const CHIP_DOT: i32 = 8;
const CHIP_GAP: i32 = 4;
/// The gap between the header's name pill and its repositories button,
/// and between that and the "+".
const HEADER_GAP: i32 = 4;
const HEADER_BUTTON_GAP: i32 = 2;
/// A row in compact density: just the name line.
const ROW_COMPACT: i32 = 28;
/// Card padding in compact density: half the comfortable one.
const CARD_PAD_COMPACT: i32 = CARD_PAD / 2;
/// The field a row's name turns into while renaming: its height and how
/// far it reaches left of the name.
const RENAME_HEIGHT: i32 = 24;
const RENAME_INSET: i32 = 5;
/// How much of the warning colour a blocked card's header carries at the
/// top of its pulse.
const BLOCKED_TINT: f32 = 0.15;
/// How much of the tint a blocked card's header keeps at the bottom of its
/// breathing; the rest comes and goes with the pulse.
const BLOCKED_REST: f32 = 0.4;

/// The header band of a blocked agent card at `card`, as `agent_card`
/// tints it: the part the pulse breathes over.
pub fn blocked_band(card: Rect, density: Density) -> Rect {
    Rectangle::new(
        (card.loc.x + 1, card.loc.y + 1).into(),
        (
            (card.size.w - 2).max(0),
            (CARD_HEADER + card_pad(density) - 1).max(0),
        )
            .into(),
    )
}

/// How opaque the pulse band over a blocked card's header is drawn when
/// the pulse stands at `pulse` (0 to 1).
pub fn pulse_alpha(pulse: f32) -> f32 {
    BLOCKED_TINT * (1.0 - BLOCKED_REST) * pulse.clamp(0.0, 1.0)
}

/// One line of an agent card below its header.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentLine {
    /// What the agent is on, from its terminal title.
    Title(String),
    /// A labelled value.
    Field(String, String),
}

/// What an agent card shows: everything herdr (or Cursor) reports about
/// the agent. The card draws its header and one detail line; the rest is
/// for its tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentView {
    /// Which agent; a click on the card shows it.
    pub key: AgentKey,
    /// The folder it works in, for "Copy path".
    pub dir: Option<PathBuf>,
    pub status: AgentStatus,
    /// How herdr names its kind, and the name it was given.
    pub kind: String,
    pub name: Option<String>,
    /// The model it runs, as its status line reports it to herdr.
    pub model: Option<String>,
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
                lines.extend(term.map(|t| field("Terminal title", t)));
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
            lines.push(field("Agent", agent.clone()));
        }
        let root = pane.and_then(repo_root);
        if let Some(cwd) = &info.cwd {
            lines.push(field(FOLDER, short_path(cwd, root)));
        }
        if let Some(fg) = info
            .foreground_cwd
            .as_ref()
            .filter(|fg| Some(*fg) != info.cwd.as_ref())
        {
            lines.push(field("Running in", short_path(fg, root)));
        }
        lines.extend(
            info.state_labels
                .iter()
                .map(|(k, v)| field(&sentence_case(k), v.clone())),
        );
        let model = info.tokens.get("model").filter(|m| !m.is_empty()).cloned();
        lines.extend(
            info.tokens
                .iter()
                .filter(|(k, _)| *k != "model")
                .map(|(k, v)| field(&sentence_case(k), v.clone())),
        );
        Self {
            key: AgentKey::Herdr(info.pane_id.clone()),
            dir: info.cwd.clone(),
            status: info.agent_status,
            kind,
            name: info.name.clone(),
            model,
            lines,
        }
    }

    /// The card for Cursor chat `chat`.
    pub fn cursor(chat: &CursorChat) -> Self {
        let row = &chat.row;
        let field = |label: &str, value: String| AgentLine::Field(label.into(), value);
        let mut lines = Vec::new();
        lines.extend(
            row.subtitle
                .as_deref()
                .map(tidy_title)
                .filter(|t| !t.is_empty())
                .map(AgentLine::Title),
        );
        let dir = row.paths.first().cloned();
        if let Some(dir) = &dir {
            lines.push(field(FOLDER, short_path(dir, None)));
        }
        if let Some(pct) = row.context_pct {
            lines.push(field("Context used", format!("{pct:.0}%")));
        }
        if let Some(files) = row.files.filter(|n| *n > 0) {
            let plural = if files == 1 { "" } else { "s" };
            let (added, removed) = (row.added.unwrap_or(0), row.removed.unwrap_or(0));
            lines.push(field(
                "Changes",
                format!("{files} file{plural}, {added} lines added, {removed} removed"),
            ));
        }
        if row.worktree {
            lines.push(field("Worktree", "its own".into()));
        }
        lines.extend(chat.subagents.iter().map(|task| {
            field(SUBAGENT, task.clone().unwrap_or_else(|| "Working".into()))
        }));
        Self {
            key: AgentKey::Cursor(row.id.clone()),
            dir,
            status: chat.status,
            kind: "cursor".into(),
            name: row.name.clone().filter(|n| !n.trim().is_empty()),
            model: chat.model.clone(),
            lines,
        }
    }

    /// What it is on: its title line, if any.
    pub fn title(&self) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            AgentLine::Title(title) => Some(title.as_str()),
            _ => None,
        })
    }

    /// What the card calls the agent: its kind, then its name and its
    /// model.
    pub fn label(&self) -> String {
        [Some(&self.kind), self.name.as_ref(), self.model.as_ref()]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The card's detail line: the folder it is in (its last component),
    /// how many subagents of its run, and what it is on.
    pub fn detail(&self) -> String {
        let cwd = self.lines.iter().find_map(|l| match l {
            AgentLine::Field(label, value) if label == FOLDER => Some(basename(value)),
            _ => None,
        });
        let subagents = self
            .lines
            .iter()
            .filter(|l| matches!(l, AgentLine::Field(label, _) if label == SUBAGENT))
            .count();
        let subagents = match subagents {
            0 => None,
            1 => Some("1 subagent".to_owned()),
            n => Some(format!("{n} subagents")),
        };
        [cwd.map(str::to_owned), subagents, self.title().map(str::to_owned)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Everything the card knows, for its tooltip under `label`: a line
    /// each, its status first, said in full.
    pub fn tooltip_fields(&self) -> Vec<(String, String)> {
        let mut fields = vec![("Status".to_owned(), status_meaning(self.status).to_owned())];
        fields.extend(self.lines.iter().map(|l| match l {
            AgentLine::Title(title) => ("Working on".to_owned(), title.clone()),
            AgentLine::Field(label, value) => (label.clone(), value.clone()),
        }));
        fields
    }

    /// The card's height at `density`, in logical pixels.
    pub fn height(&self, density: Density) -> i32 {
        let detail = if self.detail().is_empty() {
            0
        } else {
            CARD_LINE
        };
        2 * card_pad(density) + CARD_HEADER + detail
    }
}

/// The label of the field holding the folder an agent works in.
const FOLDER: &str = "Folder";
/// The label of a field holding what a subagent that runs is to do.
const SUBAGENT: &str = "Subagent";

/// `key` as a label: its words split on `_` and `-`, the first capitalised.
fn sentence_case(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
    }
}

/// The last component of a path shown as text.
fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|b| !b.is_empty())
        .unwrap_or(path)
}

fn card_pad(density: Density) -> i32 {
    match density {
        Density::Comfortable => CARD_PAD,
        Density::Compact => CARD_PAD_COMPACT,
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

/// What `status` means, said in full for a tooltip.
pub fn status_meaning(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "Idle, waiting for a prompt",
        AgentStatus::Working => "Working",
        AgentStatus::Blocked => "Blocked, needs your answer",
        AgentStatus::Done => "Done, not looked at yet",
        AgentStatus::Unknown => "Unknown",
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
    /// The tab strip shows this, so the row leaves it to its tooltip.
    pub activity: String,
    /// The app icon its row shows in place of its kind's, if any.
    pub icon: Option<AppIcon>,
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
            icon: None,
        }
    }

    /// Whether it has a second line: a branch or a place.
    fn has_detail(&self) -> bool {
        self.branch.is_some() || !self.place.is_empty()
    }

    /// The height of the band holding its name, in logical pixels.
    fn title_height(&self, density: Density) -> i32 {
        match density {
            Density::Compact => ROW_COMPACT,
            Density::Comfortable if self.has_detail() => ROW_TITLE_HEIGHT,
            Density::Comfortable => ROW_HEIGHT,
        }
    }

    /// The row's height at `density`, in logical pixels.
    pub fn height(&self, density: Density) -> i32 {
        let title = self.title_height(density);
        if density == Density::Comfortable && self.has_detail() {
            title + TAB_LINE * (1 + self.attention_lines() as i32) + ROW_BOTTOM
        } else {
            title
        }
    }

    /// The other worktrees of its folder's repository.
    pub fn worktrees(&self) -> &[Worktree] {
        self.git.as_ref().map_or(&[], |g| &g.worktrees)
    }

    /// Those of them that need a look: unmerged, changed or gone.
    pub fn needing_attention(&self) -> Vec<&Worktree> {
        let all = self.worktrees().iter();
        all.filter(|w| w.needs_attention()).collect()
    }

    /// How many lines those take under line 2: one each up to
    /// `WORKTREE_LINES`, and one counting the rest.
    fn attention_lines(&self) -> usize {
        let n = self.needing_attention().len();
        if n > WORKTREE_LINES {
            WORKTREE_LINES + 1
        } else {
            n
        }
    }
}

/// How pressing `status` is, for picking the one a workspace shows and
/// ordering the cards.
pub fn urgency(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

/// The order `cards` show in: the most pressing first, and among equals
/// as given (by tab). Indices into `cards`.
pub fn card_order(cards: &[AgentView]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cards.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(urgency(cards[*i].status)));
    order
}

/// How many of `cards` are blocked.
pub fn blocked_count(cards: &[AgentView]) -> usize {
    cards
        .iter()
        .filter(|c| c.status == AgentStatus::Blocked)
        .count()
}

/// What moves in a workspace row: its hover pill's alpha (0 to 16) and
/// whether the pointer is on its "×", on its worktree count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowState {
    pub hover: u8,
    pub close: bool,
    pub worktrees: bool,
}

/// What moves in an agent card: its hover pill's alpha and, while blocked,
/// (0 to 16). A blocked card's pulse is not here: it is composed over the
/// sidebar as its own part (see `blocked_band`), so it never redraws it.
/// Also whether its agent is open: its pane has the keyboard in the shown
/// tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardState {
    pub hover: u8,
    pub open: bool,
}

/// What the sidebar shows beyond its data: the density, the hover and
/// pulse alphas, whether the "+" menu is open, and the name being renamed.
/// Alphas are quantized so a buffer drawn from it is drawn again only when
/// a step changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarState {
    pub density: Density,
    /// Per workspace row.
    pub rows: Vec<RowState>,
    /// Per workspace, per card as the data lists them (not as drawn).
    pub cards: Vec<Vec<CardState>>,
    /// The "+" button's hover alpha, and whether its menu is open.
    pub plus: u8,
    pub plus_open: bool,
    /// The repositories button's hover alpha, and whether the panel is
    /// open.
    pub repos: u8,
    pub repos_open: bool,
    /// The workspace being renamed, and the name typed so far.
    pub rename: Option<(usize, Line)>,
}

impl SidebarState {
    fn row(&self, i: usize) -> RowState {
        self.rows.get(i).copied().unwrap_or_default()
    }

    fn card(&self, i: usize, k: usize) -> CardState {
        self.cards
            .get(i)
            .and_then(|cards| cards.get(k))
            .copied()
            .unwrap_or_default()
    }
}

/// Everything the sidebar is drawn from.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarView {
    pub workspaces: Vec<WorkspaceView>,
    /// Per workspace, the agents running in it.
    pub agents: Vec<Vec<AgentView>>,
    pub active: Option<usize>,
    /// The project shown, by name, and whether its name is lit (under the
    /// pointer, or its menu open).
    pub project: Option<String>,
    pub header_lit: bool,
    pub state: SidebarState,
}

/// Where the parts of a workspace row sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    /// The whole row.
    pub rect: Rect,
    /// The band holding its name.
    pub title: Rect,
    /// Where its name goes, up to the chip (or the "×").
    pub name: Rect,
    /// The status chip, when it has agents.
    pub chip: Option<Rect>,
    /// Its "×" close button.
    pub close: Rect,
    /// The count of its other worktrees on line 2, when it has any and
    /// line 2 shows.
    pub worktrees: Option<Rect>,
    /// The lines under line 2 of the worktrees needing a look, when it
    /// shows any.
    pub attention: Option<Rect>,
}

impl RowLayout {
    /// The field the name turns into while renaming.
    pub fn rename_field(&self) -> Rect {
        let x = self.name.loc.x - RENAME_INSET;
        let y = self.title.loc.y + (self.title.size.h - RENAME_HEIGHT) / 2;
        let end = self.chip.map_or(self.close.loc.x, |c| c.loc.x) - 2;
        Rectangle::new((x, y).into(), (end - x, RENAME_HEIGHT).into())
    }
}

pub struct SidebarLayout {
    /// The project's name, opening the project menu: the pill at the left
    /// of the header, exactly as it lights up.
    pub header: Rect,
    /// The "+" button at the right of the header, opening the menu of what
    /// to add.
    pub plus: Rect,
    /// The button left of the "+", opening the repositories panel.
    pub repos: Rect,
    /// One per workspace.
    pub rows: Vec<RowLayout>,
    /// Per workspace, its agent cards as drawn (the most pressing first):
    /// each with its index among the workspace's cards as the data lists
    /// them.
    pub cards: Vec<Vec<(usize, Rect)>>,
    /// Per workspace, all it takes: its row, its cards and the gap after them.
    pub blocks: Vec<Rect>,
}

/// What is under a point of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarHit {
    /// The project's name.
    Header,
    /// The "+" button.
    Plus,
    /// The repositories button.
    Repos,
    /// Workspace `i`: its row off any part below, its name, its status
    /// chip, its "×", the count of its other worktrees.
    Row(usize),
    Name(usize),
    Chip(usize),
    Close(usize),
    Worktrees(usize),
    /// Line `k` of workspace `i`'s worktrees needing a look: the worktree
    /// `k` of `WorkspaceView::needing_attention`, or the "+N more" line.
    Attention(usize, usize),
    /// Card `k` (as the data lists it) of workspace `i`.
    Card(usize, usize),
}

impl SidebarHit {
    /// The row a hit on any part of workspace `i` belongs to.
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Row(i)
            | Self::Name(i)
            | Self::Chip(i)
            | Self::Close(i)
            | Self::Worktrees(i)
            | Self::Attention(i, _) => Some(i),
            _ => None,
        }
    }

    /// What lights up under this hit: a row for any part of it.
    pub fn pill(self) -> Self {
        match self.row() {
            Some(i) => Self::Row(i),
            None => self,
        }
    }
}

impl SidebarLayout {
    /// What is at `pos`.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<SidebarHit> {
        let inside = |r: Rect| r.to_f64().contains(pos);
        if inside(self.plus) {
            return Some(SidebarHit::Plus);
        }
        if inside(self.repos) {
            return Some(SidebarHit::Repos);
        }
        if inside(self.header) {
            return Some(SidebarHit::Header);
        }
        for (i, cards) in self.cards.iter().enumerate() {
            if let Some((k, _)) = cards.iter().find(|(_, rect)| inside(*rect)) {
                return Some(SidebarHit::Card(i, *k));
            }
        }
        let (i, row) = self.rows.iter().enumerate().find(|(_, r)| inside(r.rect))?;
        Some(if inside(row.close) {
            SidebarHit::Close(i)
        } else if row.worktrees.is_some_and(inside) {
            SidebarHit::Worktrees(i)
        } else if let Some(lines) = row.attention.filter(|r| inside(*r)) {
            SidebarHit::Attention(i, ((pos.y - lines.loc.y as f64) / TAB_LINE as f64) as usize)
        } else if row.chip.is_some_and(inside) {
            SidebarHit::Chip(i)
        } else if inside(row.name) {
            SidebarHit::Name(i)
        } else {
            SidebarHit::Row(i)
        })
    }
}

/// Where the header's parts, the rows of `workspaces` and their `agents`
/// cards sit in a sidebar `width` wide, in comfortable density.
#[cfg(test)]
pub fn sidebar_layout(
    workspaces: &[WorkspaceView],
    agents: &[Vec<AgentView>],
    width: i32,
) -> SidebarLayout {
    sidebar_layout_at(workspaces, agents, width, Density::Comfortable)
}

/// `sidebar_layout` at `density`.
pub fn sidebar_layout_at(
    workspaces: &[WorkspaceView],
    agents: &[Vec<AgentView>],
    width: i32,
    density: Density,
) -> SidebarLayout {
    let plus = Rectangle::new(
        (
            width - GAP - HEADER_MARGIN - HEADER_BUTTON,
            (BAR_HEIGHT - HEADER_BUTTON) / 2,
        )
            .into(),
        (HEADER_BUTTON, HEADER_BUTTON).into(),
    );
    let repos = Rectangle::new(
        (plus.loc.x - HEADER_BUTTON_GAP - HEADER_BUTTON, plus.loc.y).into(),
        plus.size,
    );
    let header = Rectangle::new(
        (ROW_INSET, 5).into(),
        (
            (repos.loc.x - HEADER_GAP - ROW_INSET).max(0),
            BAR_HEIGHT - 10,
        )
            .into(),
    );
    let (mut rows, mut card_rects, mut blocks) = (Vec::new(), Vec::new(), Vec::new());
    let mut y = ROWS_TOP;
    let name_x = ROW_INSET + PADDING as i32 + ROW_ICON_WIDTH + ROW_ICON_GAP;
    let card_x = ROW_INSET + CARD_INDENT;
    let card_w = (width - GAP - ROW_INSET - card_x).max(0);
    let none = Vec::new();
    for (i, ws) in workspaces.iter().enumerate() {
        let (h, title_h) = (ws.height(density), ws.title_height(density));
        let rect = Rectangle::new((0, y).into(), (width - GAP, h).into());
        let title = Rectangle::new((0, y).into(), (width - GAP, title_h).into());
        let close_x = width - GAP - ROW_INSET - CLOSE_WIDTH;
        let close = Rectangle::new((close_x, y).into(), (CLOSE_WIDTH, title_h).into());
        let chip = ws
            .status
            .map(|_| Rectangle::new((close_x - CHIP_SLOT, y).into(), (CHIP_SLOT, title_h).into()));
        let name_end = chip.map_or(close_x, |c| c.loc.x) - 4;
        let name = Rectangle::new(
            (name_x, y).into(),
            ((name_end - name_x).max(0), title_h).into(),
        );
        let worktrees =
            (density == Density::Comfortable && ws.has_detail() && !ws.worktrees().is_empty())
                .then(|| {
                    let x = width - GAP - ROW_INSET - PADDING as i32 - WORKTREE_COUNT_WIDTH;
                    Rectangle::new(
                        (x, y + title_h).into(),
                        (WORKTREE_COUNT_WIDTH, TAB_LINE).into(),
                    )
                });
        let lines = ws.attention_lines() as i32;
        let attention =
            (density == Density::Comfortable && ws.has_detail() && lines > 0).then(|| {
                Rectangle::new(
                    (ROW_INSET, y + title_h + TAB_LINE).into(),
                    (width - GAP - 2 * ROW_INSET, lines * TAB_LINE).into(),
                )
            });
        rows.push(RowLayout {
            rect,
            title,
            name,
            chip,
            close,
            worktrees,
            attention,
        });
        y += h;
        let cards = agents.get(i).unwrap_or(&none);
        let ws_cards = card_order(cards)
            .into_iter()
            .map(|k| {
                let card = Rectangle::new(
                    (card_x, y).into(),
                    (card_w, cards[k].height(density)).into(),
                );
                y += card.size.h + CARD_GAP;
                (k, card)
            })
            .collect();
        card_rects.push(ws_cards);
        if !cards.is_empty() {
            y += CARD_GAP;
        }
        blocks.push(Rectangle::new(
            rect.loc,
            (rect.size.w, y - rect.loc.y).into(),
        ));
    }
    SidebarLayout {
        header,
        plus,
        repos,
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

/// Roughly whether `name` at the row's label size fits in `width` logical
/// pixels, without a font to measure it: for offering the full name as a
/// tooltip.
pub fn name_fits(name: &str, width: i32) -> bool {
    let estimate = name.chars().count() as f32 * LABEL_FONT_SIZE * 0.55;
    estimate <= width as f32
}

/// A quantized alpha from `SidebarState` as a fraction.
fn alpha(step: u8) -> f32 {
    (step as f32 / ALPHA_STEPS).clamp(0.0, 1.0)
}

impl Painter {
    /// The sidebar: the header (project name and "+"), a row per workspace
    /// and the agent cards under it, with what moves in `view.state`.
    pub(super) fn sidebar_view(
        &self,
        view: &SidebarView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let line = px(1).max(1);
        c.fill(0, 0, c.w, c.h, p.panel);
        c.fill(0, px(BAR_HEIGHT) - line, c.w, line, p.border);
        c.fill(c.w - px(GAP).max(1), 0, px(GAP).max(1), c.h, p.border);

        let state = &view.state;
        let density = state.density;
        let layout = sidebar_layout_at(&view.workspaces, &view.agents, size.w, density);
        match &view.project {
            Some(name) => self.header_name(&mut c, name, view.header_lit, layout.header, scale, p),
            None => {
                self.text(
                    &mut c,
                    "Workspaces",
                    SMALL_FONT_SIZE,
                    px(ROW_INSET) as f32 + PADDING * s,
                    px(layout.header.loc.x + layout.header.size.w) as f32,
                    (0, px(BAR_HEIGHT)),
                    scale,
                    p.muted,
                );
            }
        }
        let plus = (alpha(state.plus), state.plus_open);
        self.header_button(&mut c, layout.plus, plus, scale, p, |x, y| {
            new_tab(x, y, 0.9 * s)
        });
        let repos = (alpha(state.repos), state.repos_open);
        self.header_button(&mut c, layout.repos, repos, scale, p, |x, y| {
            git_branch(x, y, 1.05 * s)
        });

        for (i, (row, ws)) in layout.rows.iter().zip(&view.workspaces).enumerate() {
            let is_active = Some(i) == view.active;
            let row_state = state.row(i);
            let hover = alpha(row_state.hover);
            let fill = match (is_active, hover > 0.0) {
                (true, true) => Some(mix(p.selected, p.text, 0.05 * hover)),
                (true, false) => Some(p.selected),
                (false, true) => Some(mix(p.panel, p.element, hover)),
                (false, false) => None,
            };
            if let Some(fill) = fill {
                let (x, y, w, h) = rect_px(row.rect, scale);
                c.rounded(
                    x + px(ROW_INSET) as f32,
                    y + px(2) as f32,
                    w - 2.0 * px(ROW_INSET) as f32,
                    h - 2.0 * px(2) as f32,
                    ROW_RADIUS * s,
                    fill,
                );
            }
            let color = if is_active { p.text } else { p.muted };
            let title_band = band(row.title, scale);
            let icon = Rectangle::new(
                (ROW_INSET + PADDING as i32, row.title.loc.y).into(),
                (ROW_ICON_WIDTH, row.title.size.h).into(),
            );
            let small = 0.8 * s;
            match ws.kind {
                WorkspaceKind::Project => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    folder(x, y, small)
                }),
                WorkspaceKind::Terminal => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    terminal(x, y, small)
                }),
                WorkspaceKind::Cursor => match &ws.icon {
                    Some(picture) => {
                        c.picture_sized(picture, icon, scale, p.placeholder, ROW_PICTURE_SIZE)
                    }
                    None => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| app(x, y, small)),
                },
            }
            let name_x = px(row.name.loc.x) as f32;
            let name_end = px(row.name.loc.x + row.name.size.w) as f32;
            match state.rename.as_ref().filter(|(w, _)| *w == i) {
                Some((_, typed)) => {
                    self.rename_field(&mut c, typed, row.rename_field(), scale, p);
                }
                None => {
                    self.text(
                        &mut c,
                        &ws.name,
                        LABEL_FONT_SIZE,
                        name_x,
                        name_end,
                        title_band,
                        scale,
                        color,
                    );
                }
            }
            if let (Some(chip), Some(status)) = (row.chip, ws.status) {
                let blocked = view.agents.get(i).map_or(0, |cards| blocked_count(cards));
                // The whole room, or just its own part while the "×" shows.
                let room = if hover > 0.0 {
                    chip
                } else {
                    Rectangle::new(
                        chip.loc,
                        (chip.size.w + row.close.size.w, chip.size.h).into(),
                    )
                };
                self.status_chip(&mut c, status, blocked, room, hover == 0.0, scale, p);
            }
            if hover > 0.0 {
                let color = if row_state.close {
                    p.text
                } else {
                    mix(p.panel, p.placeholder, hover)
                };
                icon_sdf(&mut c, row.close, scale, color, |x, y| cross(x, y, 0.8 * s));
            }

            // The count of its other worktrees at the right end of line 2;
            // hovering it lists them, on a pill as it opens the popup.
            if let Some(count) = row.worktrees {
                let n = ws.worktrees().len().to_string();
                let n_w = (self.text_width(&n, DETAIL_FONT_SIZE, scale) / s).ceil() as i32;
                let end = count.loc.x + count.size.w;
                let icon = Rectangle::new(
                    (end - n_w - BRANCH_ICON_GAP - BRANCH_ICON_WIDTH, count.loc.y).into(),
                    (BRANCH_ICON_WIDTH, count.size.h).into(),
                );
                let on = row_state.worktrees;
                if on {
                    let pill = Rectangle::new(
                        (icon.loc.x - COUNT_PILL_PAD, count.loc.y).into(),
                        (end - icon.loc.x + 2 * COUNT_PILL_PAD, count.size.h).into(),
                    );
                    let (x, y, w, h) = rect_px(pill, scale);
                    let under = fill.unwrap_or(p.panel);
                    c.rounded(x, y, w, h, 4.0 * s, mix(under, p.text, 0.1));
                }
                let (icon_color, n_color) = if on {
                    (p.text, p.text)
                } else {
                    (p.placeholder, p.muted)
                };
                icon_sdf(&mut c, icon, scale, icon_color, |x, y| {
                    git_branch(x, y, 0.75 * s)
                });
                self.text(
                    &mut c,
                    &n,
                    DETAIL_FONT_SIZE,
                    px(end - n_w) as f32,
                    px(end) as f32,
                    band(count, scale),
                    scale,
                    n_color,
                );
            }

            // Under line 2, a line per worktree that needs a look: its name,
            // a dot when changed, and why ("unmerged", "gone").
            if density == Density::Comfortable && ws.has_detail() {
                let right = px(row.rect.loc.x + row.rect.size.w - ROW_INSET) as f32 - PADDING * s;
                let text_x = name_x + ((BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32 * s).round();
                let needing = ws.needing_attention();
                for k in 0..ws.attention_lines() {
                    let line_y = row.title.loc.y + row.title.size.h + TAB_LINE * (1 + k as i32);
                    let band = (px(line_y), px(line_y + TAB_LINE) - px(line_y));
                    let Some(wt) = needing.get(k).filter(|_| k < WORKTREE_LINES) else {
                        let more = format!("+{} more", needing.len() - WORKTREE_LINES);
                        self.text(
                            &mut c,
                            &more,
                            DETAIL_FONT_SIZE,
                            text_x,
                            right,
                            band,
                            scale,
                            p.placeholder,
                        );
                        break;
                    };
                    let icon = Rectangle::new(
                        (row.name.loc.x, line_y).into(),
                        (BRANCH_ICON_WIDTH, TAB_LINE).into(),
                    );
                    icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                        git_branch(x, y, 0.75 * s)
                    });
                    let x = self.text(
                        &mut c,
                        &wt.name(),
                        DETAIL_FONT_SIZE,
                        text_x,
                        right,
                        band,
                        scale,
                        p.muted,
                    );
                    let dirty = GitStatus {
                        dirty: wt.dirty,
                        ..GitStatus::default()
                    };
                    let x = self.git_marks(&mut c, &dirty, x, right, band, scale, p);
                    if let Some(tag) = wt.attention_tag() {
                        self.text(
                            &mut c,
                            &format!("  {tag}"),
                            DETAIL_FONT_SIZE,
                            x,
                            right,
                            band,
                            scale,
                            p.placeholder,
                        );
                    }
                }
            }

            // Line 2: Git branch and marks, then the place, in comfortable density.
            if density == Density::Comfortable && ws.has_detail() {
                let right = match row.worktrees {
                    Some(count) => px(count.loc.x - BRANCH_ICON_GAP) as f32,
                    None => px(row.rect.loc.x + row.rect.size.w - ROW_INSET) as f32 - PADDING * s,
                };
                let line_y = row.title.loc.y + row.title.size.h;
                let band = (px(line_y), px(line_y + TAB_LINE) - px(line_y));
                let mut place_x = name_x;
                if let Some(branch) = &ws.branch {
                    let icon = Rectangle::new(
                        (row.name.loc.x, line_y).into(),
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
                        // The separator only where a bit of the place fits after it.
                        let sep = self.text_width(" · ", DETAIL_FONT_SIZE, scale);
                        let some = self.text_width("…", DETAIL_FONT_SIZE, scale) * 2.0;
                        if place_x + sep + some > right {
                            continue;
                        }
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
            }
        }
        for (i, (rects, cards)) in layout.cards.iter().zip(&view.agents).enumerate() {
            for (k, rect) in rects {
                self.agent_card(
                    &mut c,
                    &cards[*k],
                    *rect,
                    state.card(i, *k),
                    density,
                    scale,
                    p,
                );
            }
        }
        c.into_buffer(true)
    }

    /// The project's `name` in the sidebar `header` pill, cut to fit, with
    /// a chevron at the pill's right end; the pill shows when `lit` (under
    /// the pointer, or its menu open). The pill is `header` itself, so a
    /// press anywhere it shows opens the menu.
    pub(super) fn header_name(
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
        let (x, y, w, h) = rect_px(header, scale);
        if lit {
            c.rounded(x, y, w, h, 4.0 * s, p.selected);
        }
        let header_end = header.loc.x + header.size.w;
        let chevron = Rectangle::new(
            (header_end - HEADER_NAME_END - CHEVRON_WIDTH, 0).into(),
            (CHEVRON_WIDTH, BAR_HEIGHT).into(),
        );
        let text_x = px(ROW_INSET) as f32 + PADDING * s;
        let max_x = px(chevron.loc.x - HEADER_NAME_GAP) as f32;
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

    /// A header button showing `shape`: on a pill when hovered (`hover`,
    /// fading in) or what it opens is `open`.
    fn header_button(
        &self,
        c: &mut Canvas,
        rect: Rect,
        (hover, open): (f32, bool),
        scale: f64,
        p: &Palette,
        shape: impl Fn(f32, f32) -> f32,
    ) {
        let s = scale as f32;
        let fill = if open {
            Some(p.selected)
        } else if hover > 0.0 {
            Some(mix(p.panel, p.element, hover))
        } else {
            None
        };
        if let Some(fill) = fill {
            let (x, y, w, h) = rect_px(rect, scale);
            c.rounded(x, y, w, h, 6.0 * s, fill);
        }
        let color = if open || hover > 0.0 {
            mix(p.muted, p.text, hover.max(open as u8 as f32))
        } else {
            p.muted
        };
        icon_sdf(c, rect, scale, color, shape);
    }

    /// The status chip at the right of a row's name line: a dot and a word
    /// in the status colour; blocked agents make it a filled warning badge
    /// with their count. Right-aligned in `slot`; the word goes when it
    /// does not fit, or when `words` is off (the "×" has the room).
    #[allow(clippy::too_many_arguments)]
    fn status_chip(
        &self,
        c: &mut Canvas,
        status: AgentStatus,
        blocked: usize,
        slot: Rect,
        words: bool,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let color = p.status(status);
        let word = match (status, blocked) {
            (AgentStatus::Blocked, n) if n > 1 => format!("{n} blocked"),
            _ => status_label(status).to_owned(),
        };
        let pad = (CHIP_PADDING as f32 * s).round();
        let dot = (CHIP_DOT as f32 * s).round();
        let gap = (CHIP_GAP as f32 * s).round();
        let word_w = self.text_width(&word, TINY_FONT_SIZE, scale);
        let (x0, _, slot_w, _) = rect_px(slot, scale);
        let right = x0 + slot_w - (2.0 * s).round();
        let full = 2.0 * pad + dot + gap + word_w;
        let with_word = words && full <= slot_w;
        let w = if with_word { full } else { 2.0 * pad + dot };
        let x = (right - w).round();
        let h = px(PILL_HEIGHT) as f32;
        let y = (px(slot.loc.y) as f32 + (px(slot.size.h) as f32 - h) / 2.0).round();
        let filled = status == AgentStatus::Blocked;
        let fill = if filled {
            color
        } else {
            mix(p.panel, color, 0.18)
        };
        c.rounded(x, y, w.round(), h, h / 2.0, fill);
        let dot_color = if filled { p.on_accent } else { color };
        let (cx, cy) = (x + pad + dot / 2.0, y + h / 2.0);
        c.paint_in(
            (x + pad) as i32,
            y as i32,
            dot as i32 + 1,
            h as i32,
            dot_color,
            |px, py| super::coverage((px - cx).hypot(py - cy) - dot / 2.0),
        );
        if with_word {
            self.text(
                c,
                &word,
                TINY_FONT_SIZE,
                x + pad + dot + gap,
                f32::MAX,
                (y as i32, h as i32),
                scale,
                if filled { p.on_accent } else { color },
            );
        }
    }

    /// The field a row's name turns into while renaming, showing `typed`
    /// with its selection and caret.
    fn rename_field(&self, c: &mut Canvas, typed: &Line, rect: Rect, scale: f64, p: &Palette) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = (scale.round() as f32).max(1.0);
        c.rounded(x, y, w, h, 4.0 * s, p.accent);
        c.rounded(
            x + b,
            y + b,
            w - 2.0 * b,
            h - 2.0 * b,
            4.0 * s - b,
            p.element,
        );
        let pad = (RENAME_INSET as f32 * s).round();
        self.edit_line(
            c,
            typed,
            "",
            (x + pad, x + w - pad),
            band(rect, scale),
            true,
            FONT_SIZE,
            scale,
            p,
        );
    }

    /// An agent card: a header with the status mark, the agent's kind and
    /// name and a status pill, then one detail line. A blocked card's
    /// header carries the warning colour (its breathing is composed on
    /// top, see `pulse_band`); the card lightens under the pointer. The
    /// open agent's card is filled as the active workspace row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn agent_card(
        &self,
        c: &mut Canvas,
        card: &AgentView,
        rect: Rect,
        state: CardState,
        density: Density,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let (b, r) = (px(1).max(1) as f32, CARD_RADIUS * s);
        let status = p.status(card.status);
        let blocked = card.status == AgentStatus::Blocked;
        let hover = alpha(state.hover);
        let fill = if state.open {
            mix(p.selected, p.text, 0.05 * hover)
        } else {
            mix(mix(p.panel, p.element, 0.6), p.selected, hover)
        };
        let edge = if blocked {
            mix(p.border_variant, status, 0.6)
        } else {
            mix(p.border_variant, p.border, hover)
        };
        c.rounded(x, y, w, h, r, edge);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let pad = card_pad(density);
        let header_y = rect.loc.y + pad;
        if blocked {
            // The header band in the warning colour, at the bottom of its
            // breathing.
            let tint = BLOCKED_TINT * BLOCKED_REST;
            let band_h = px(CARD_HEADER + pad) as f32;
            c.rounded(
                x + b,
                y + b,
                w - 2.0 * b,
                band_h - b,
                r - b,
                mix(fill, p.warning, tint),
            );
            // Square off the band's bottom corners against the body.
            c.fill(
                (x + b) as i32,
                (y + band_h - r) as i32,
                (w - 2.0 * b) as i32,
                r as i32,
                mix(fill, p.warning, tint),
            );
        }
        c.rounded(
            x + 3.0 * s,
            (px(rect.loc.y) + px(pad)) as f32,
            2.0 * s,
            (px(rect.size.h) - 2 * px(pad)) as f32,
            s,
            status,
        );

        // Header: status mark, kind, name and model, status pill.
        let inner_x = rect.loc.x + CARD_PAD;
        let right = px(rect.loc.x + rect.size.w - CARD_PAD) as f32;
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
        let after = match &card.name {
            Some(name) => self.text(
                c,
                &format!("· {name}"),
                FONT_SIZE,
                after + px(5) as f32,
                heading_end,
                band,
                scale,
                p.accent,
            ),
            None => after,
        };
        if let Some(model) = &card.model {
            self.text(
                c,
                &format!("· {model}"),
                FONT_SIZE,
                after + px(5) as f32,
                heading_end,
                band,
                scale,
                p.muted,
            );
        }

        // The detail line: where it is and what it is on.
        let detail = card.detail();
        if !detail.is_empty() {
            let band = (px(header_y + CARD_HEADER), px(CARD_LINE));
            self.text(
                c,
                &detail,
                DETAIL_FONT_SIZE,
                kind_x,
                right,
                band,
                scale,
                p.muted,
            );
        }
    }
}

impl Painter {
    /// The band composed over a blocked card's header as its pulse: the
    /// warning colour, `size` big, rounded at the top like the card and
    /// square at the bottom where the header meets the body. Its opacity
    /// is given when it is composed (`pulse_alpha`).
    pub(super) fn pulse_band(
        &self,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let s = scale as f32;
        let b = (scale.round() as f32).max(1.0);
        let r = CARD_RADIUS * s - b;
        let (w, h) = (c.w as f32, c.h as f32);
        c.rounded(0.0, 0.0, w, h, r, p.warning);
        c.fill(0, (h - r).max(0.0) as i32, c.w, r.ceil() as i32, p.warning);
        c.into_buffer(false)
    }
}

/// The field renaming a workspace, drawn in `area` (see
/// `RowLayout::rename_field`) and showing `line`, for clicks into it.
pub fn sidebar_rename_field(area: Rect, line: &Line, scale: f64) -> FieldSpot {
    let (x, _, w, _) = rect_px(area, scale);
    let pad = (RENAME_INSET as f32 * scale as f32).round();
    FieldSpot {
        size: FONT_SIZE,
        area,
        text: (x + pad, x + w - pad),
        line: line.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentLine, AgentView, CARD_GAP, CARD_HEADER, CARD_LINE, CARD_PAD, CARD_PAD_COMPACT,
        CHIP_SLOT, CLOSE_WIDTH, Density, FOLDER, GAP, HEADER_BUTTON, ROW_BOTTOM, ROW_COMPACT,
        ROW_HEIGHT, ROW_INSET, ROW_TITLE_HEIGHT, ROWS_TOP, SidebarHit, TAB_LINE, WorkspaceView,
        card_order, sentence_case, sidebar_layout, sidebar_layout_at, workspace_slot,
    };
    use crate::WorkspaceKind;
    use crate::chrome::{BAR_HEIGHT, TabView};
    use crate::git::{GitStatus, Worktree};
    use crate::herdr::AgentStatus;

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

    fn workspace(name: &str, status: Option<AgentStatus>, branch: Option<&str>) -> WorkspaceView {
        WorkspaceView {
            name: name.into(),
            kind: WorkspaceKind::Project,
            status,
            branch: branch.map(String::from),
            git: None,
            place: String::new(),
            activity: String::new(),
            icon: None,
        }
    }

    /// The sidebar with one workspace of three agents, their cards open
    /// or not by `open`, painted as bytes.
    fn painted_cards(open: [bool; 3], scale: f64) -> Vec<u8> {
        use super::{CardState, Painter, SidebarState, SidebarView};
        let agents = vec![vec![
            agent("1", AgentStatus::Working, Some("~/src/gerdr")),
            agent("2", AgentStatus::Idle, Some("~/src/gerdr")),
            agent("3", AgentStatus::Done, Some("~/src/gerdr")),
        ]];
        let view = SidebarView {
            workspaces: vec![workspace("gerdr", None, Some("main"))],
            agents,
            active: Some(0),
            project: None,
            header_lit: false,
            state: SidebarState {
                cards: vec![open.map(|open| CardState { hover: 0, open }).to_vec()],
                ..SidebarState::default()
            },
        };
        let palette = crate::chrome::Theme::Dark.palette();
        let mut buffer = Painter::new().sidebar_view(&view, (240, 600).into(), scale, palette);
        let mut bytes = Vec::new();
        let _ = buffer.render().draw(|mem| {
            bytes.extend_from_slice(mem);
            Ok::<_, ()>(Vec::new())
        });
        bytes
    }

    #[test]
    fn the_open_agents_card_stands_out() {
        for scale in [1.0, 2.0] {
            let plain = painted_cards([false; 3], scale);
            let first = painted_cards([true, false, false], scale);
            let second = painted_cards([false, true, false], scale);
            assert_ne!(plain, first);
            assert_ne!(plain, second);
            assert_ne!(first, second);
            // The same states paint the same pixels.
            assert_eq!(painted_cards([false; 3], scale), plain);
        }
    }

    fn agent(pane: &str, status: AgentStatus, cwd: Option<&str>) -> AgentView {
        AgentView {
            key: crate::agents::AgentKey::Herdr(pane.into()),
            dir: None,
            status,
            kind: "claude".into(),
            name: None,
            model: None,
            lines: cwd
                .map(|cwd| AgentLine::Field(FOLDER.into(), cwd.into()))
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn worktrees_show_as_a_count_on_line_two() {
        let mut ws = workspace("gerdr", None, Some("main"));
        let plain = sidebar_layout_at(&[ws.clone()], &[], 240, Density::Comfortable);
        assert_eq!(plain.rows[0].worktrees, None);
        let done = Worktree {
            merged: true,
            ..Worktree::default()
        };
        ws.git = Some(GitStatus {
            worktrees: vec![done.clone(); 5],
            ..GitStatus::default()
        });
        // Merged and clean: only counted.
        let height = ROW_TITLE_HEIGHT + TAB_LINE + ROW_BOTTOM;
        assert_eq!(ws.height(Density::Comfortable), height);
        // Those needing a look get a line each, up to a count of the rest.
        let worktrees = &mut ws.git.as_mut().unwrap().worktrees;
        worktrees[0].dirty = true;
        worktrees[1].merged = false;
        assert_eq!(ws.height(Density::Comfortable), height + 2 * TAB_LINE);
        for w in &mut ws.git.as_mut().unwrap().worktrees {
            w.merged = false;
        }
        assert_eq!(ws.height(Density::Comfortable), height + 4 * TAB_LINE);
        assert_eq!(ws.height(Density::Compact), ROW_COMPACT);
        let layout = sidebar_layout_at(&[ws.clone()], &[], 240, Density::Comfortable);
        let count = layout.rows[0].worktrees.expect("a count");
        assert_eq!(count.loc.y, ROWS_TOP + ROW_TITLE_HEIGHT);
        let mid = (count.loc.x as f64 + 5.0, count.loc.y as f64 + 5.0).into();
        assert_eq!(layout.hit(mid), Some(SidebarHit::Worktrees(0)));
        assert_eq!(SidebarHit::Worktrees(0).pill(), SidebarHit::Row(0));
        // Compact rows have no line 2 to put it on.
        let compact = sidebar_layout_at(&[ws], &[], 240, Density::Compact);
        assert_eq!(compact.rows[0].worktrees, None);
    }

    #[test]
    fn the_label_names_the_model() {
        let mut card = agent("p1", AgentStatus::Working, None);
        assert_eq!(card.label(), "claude");
        card.model = Some("Opus 5.5".into());
        assert_eq!(card.label(), "claude · Opus 5.5");
        card.name = Some("reviewer".into());
        assert_eq!(card.label(), "claude · reviewer · Opus 5.5");
    }

    #[test]
    fn a_cursor_card_shows_its_subagents() {
        use crate::cursor_agents::{ChatRow, CursorChat};
        let mut chat = CursorChat {
            row: ChatRow {
                id: "c1".into(),
                subtitle: Some("Read main.rs".into()),
                paths: vec!["/src/gerdr".into()],
                ..ChatRow::default()
            },
            status: AgentStatus::Working,
            model: None,
            subagents: vec![Some("Audit the plan".into()), None],
        };
        let card = AgentView::cursor(&chat);
        assert_eq!(card.detail(), "gerdr · 2 subagents · Read main.rs");
        let fields = card.tooltip_fields();
        assert!(fields.contains(&("Subagent".into(), "Audit the plan".into())));
        assert!(fields.contains(&("Subagent".into(), "Working".into())));
        chat.subagents.pop();
        assert_eq!(
            AgentView::cursor(&chat).detail(),
            "gerdr · 1 subagent · Read main.rs"
        );
        chat.subagents.clear();
        assert_eq!(AgentView::cursor(&chat).detail(), "gerdr · Read main.rs");
    }

    #[test]
    fn rows_have_two_lines_in_comfortable_and_one_in_compact() {
        let with = workspace("gerdr", Some(AgentStatus::Working), Some("main"));
        let plain = workspace("~", None, None);
        assert_eq!(
            with.height(Density::Comfortable),
            ROW_TITLE_HEIGHT + TAB_LINE + ROW_BOTTOM
        );
        assert_eq!(plain.height(Density::Comfortable), ROW_HEIGHT);
        assert_eq!(with.height(Density::Compact), ROW_COMPACT);
        assert_eq!(plain.height(Density::Compact), ROW_COMPACT);
    }

    #[test]
    fn cards_are_a_header_and_a_detail_line() {
        let with = agent("p1", AgentStatus::Idle, Some("gerdr/src"));
        let bare = agent("p2", AgentStatus::Idle, None);
        assert_eq!(with.detail(), "src");
        assert_eq!(bare.detail(), "");
        assert_eq!(
            with.height(Density::Comfortable),
            2 * CARD_PAD + CARD_HEADER + CARD_LINE
        );
        assert_eq!(
            bare.height(Density::Comfortable),
            2 * CARD_PAD + CARD_HEADER
        );
        assert_eq!(
            with.height(Density::Compact),
            2 * CARD_PAD_COMPACT + CARD_HEADER + CARD_LINE
        );
        assert_eq!(
            with.tooltip_fields(),
            [
                ("Status".into(), "Idle, waiting for a prompt".into()),
                ("Folder".into(), "gerdr/src".into()),
            ]
        );
        assert_eq!(sentence_case("context_tokens"), "Context tokens");
    }

    #[test]
    fn layout_stacks_rows_and_cards_in_both_densities() {
        let workspaces = vec![
            workspace("gerdr", Some(AgentStatus::Blocked), Some("main")),
            workspace("~", None, None),
        ];
        let agents = vec![
            vec![
                agent("p1", AgentStatus::Idle, Some("gerdr")),
                agent("p2", AgentStatus::Blocked, Some("gerdr/src")),
            ],
            vec![],
        ];
        for density in [Density::Comfortable, Density::Compact] {
            let layout = sidebar_layout_at(&workspaces, &agents, 200, density);
            // The "+" is a 28 px square centred in the header; the name
            // pill fills the rest of the header, and ends before it.
            assert_eq!(layout.plus.size, (HEADER_BUTTON, HEADER_BUTTON).into());
            assert_eq!(layout.plus.loc.y, (BAR_HEIGHT - HEADER_BUTTON) / 2);
            assert_eq!(layout.header.loc.x, ROW_INSET);
            assert_eq!(layout.repos.size, layout.plus.size);
            assert!(layout.repos.loc.x + layout.repos.size.w < layout.plus.loc.x);
            assert!(layout.header.loc.x + layout.header.size.w < layout.repos.loc.x);
            assert_eq!(layout.header.size.h, BAR_HEIGHT - 10);

            let row0 = layout.rows[0];
            let h0 = workspaces[0].height(density);
            assert_eq!(row0.rect.loc.y, ROWS_TOP);
            assert_eq!(row0.rect.size, (200 - GAP, h0).into());
            assert_eq!(row0.title.size.h, workspaces[0].title_height(density));
            assert_eq!(row0.close.size.w, CLOSE_WIDTH);
            assert_eq!(row0.close.loc.x, 200 - GAP - ROW_INSET - CLOSE_WIDTH);
            let chip = row0.chip.expect("a chip for a workspace with agents");
            assert_eq!(chip.loc.x + chip.size.w, row0.close.loc.x);
            assert_eq!(chip.size.w, CHIP_SLOT);
            assert!(row0.name.loc.x + row0.name.size.w <= chip.loc.x);
            assert!(layout.rows[1].chip.is_none());
            assert_eq!(
                layout.rows[1].name.loc.x + layout.rows[1].name.size.w,
                layout.rows[1].close.loc.x - 4
            );

            // Cards: the blocked one first, each as high as it says, with
            // a gap between and after them.
            let cards = &layout.cards[0];
            assert_eq!(cards.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [1, 0]);
            assert_eq!(cards[0].1.loc.y, ROWS_TOP + h0);
            assert_eq!(cards[0].1.size.h, agents[0][1].height(density));
            assert_eq!(
                cards[1].1.loc.y,
                cards[0].1.loc.y + cards[0].1.size.h + CARD_GAP
            );
            let block_end = cards[1].1.loc.y + cards[1].1.size.h + 2 * CARD_GAP;
            assert_eq!(layout.blocks[0].size.h, block_end - ROWS_TOP);
            assert_eq!(layout.rows[1].rect.loc.y, block_end);
            assert!(layout.cards[1].is_empty());
            assert_eq!(layout.blocks[1].size.h, workspaces[1].height(density));
        }
        let comfortable = sidebar_layout(&workspaces, &agents, 200);
        let compact = sidebar_layout_at(&workspaces, &agents, 200, Density::Compact);
        assert!(compact.blocks[1].loc.y < comfortable.blocks[1].loc.y);
    }

    #[test]
    fn hits_name_every_part() {
        let workspaces = vec![
            workspace("gerdr", Some(AgentStatus::Working), Some("main")),
            workspace("~", None, None),
        ];
        let agents = vec![vec![agent("p1", AgentStatus::Idle, None)], vec![]];
        let layout = sidebar_layout(&workspaces, &agents, 200);
        let at = |r: crate::chrome::Rect| {
            (
                r.loc.x as f64 + r.size.w as f64 / 2.0,
                r.loc.y as f64 + r.size.h as f64 / 2.0,
            )
                .into()
        };
        assert_eq!(layout.hit(at(layout.plus)), Some(SidebarHit::Plus));
        assert_eq!(layout.hit(at(layout.repos)), Some(SidebarHit::Repos));
        assert_eq!(layout.hit(at(layout.header)), Some(SidebarHit::Header));
        let row = layout.rows[0];
        assert_eq!(layout.hit(at(row.name)), Some(SidebarHit::Name(0)));
        assert_eq!(layout.hit(at(row.chip.unwrap())), Some(SidebarHit::Chip(0)));
        assert_eq!(layout.hit(at(row.close)), Some(SidebarHit::Close(0)));
        // Line 2 is the row itself.
        let line2 = (
            row.name.loc.x as f64,
            (row.title.loc.y + row.title.size.h + 2) as f64,
        );
        assert_eq!(layout.hit(line2.into()), Some(SidebarHit::Row(0)));
        assert_eq!(
            layout.hit(at(layout.cards[0][0].1)),
            Some(SidebarHit::Card(0, 0))
        );
        assert_eq!(
            layout.hit(at(layout.rows[1].name)),
            Some(SidebarHit::Name(1))
        );
        assert_eq!(layout.hit((100.0, 10_000.0).into()), None);
        assert_eq!(SidebarHit::Close(3).pill(), SidebarHit::Row(3));
        assert_eq!(SidebarHit::Card(1, 2).pill(), SidebarHit::Card(1, 2));
    }

    #[test]
    fn renaming_edits_in_the_name_band() {
        let workspaces = vec![workspace("gerdr", Some(AgentStatus::Idle), Some("main"))];
        let layout = sidebar_layout(&workspaces, &[], 200);
        let row = layout.rows[0];
        let field = row.rename_field();
        assert!(field.loc.x < row.name.loc.x);
        assert!(field.loc.y >= row.title.loc.y);
        assert!(field.loc.y + field.size.h <= row.title.loc.y + row.title.size.h);
        assert!(field.loc.x + field.size.w < row.chip.unwrap().loc.x);
    }

    #[test]
    fn cards_go_by_urgency_then_as_given() {
        let cards = vec![
            agent("a", AgentStatus::Idle, None),
            agent("b", AgentStatus::Done, None),
            agent("c", AgentStatus::Blocked, None),
            agent("d", AgentStatus::Working, None),
            agent("e", AgentStatus::Blocked, None),
            agent("f", AgentStatus::Unknown, None),
        ];
        assert_eq!(card_order(&cards), [2, 4, 1, 3, 0, 5]);
        assert_eq!(card_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn a_held_row_slots_between_the_others() {
        let workspaces = vec![
            workspace("a", None, None),
            workspace("b", None, None),
            workspace("c", None, None),
        ];
        let layout = sidebar_layout(&workspaces, &[], 200);
        let blocks = &layout.blocks;
        assert_eq!(workspace_slot(blocks, 0, blocks[0].loc.y as f64 + 1.0), 0);
        let last = blocks[2].loc.y + blocks[2].size.h;
        assert_eq!(workspace_slot(blocks, 0, last as f64 + 50.0), 2);
    }
}
