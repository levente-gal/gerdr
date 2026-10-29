//! The workspace sidebar: the views of workspaces and agents, where they
//! go, and how they are drawn.
//!
//! It reads as a tree. A workspace is a one-line header: a chevron folding
//! it (when it has anything under it), its kind's icon, its name, its
//! branch with the marks of where its repository stands, and at the right
//! the count of its repository's other worktrees, the mark of its CI while
//! that needs no look, and its status while it is folded or an agent in it
//! is blocked. Under it, unless folded: a line per worktree that needs a
//! look (a few at most), a flat row per agent (the most pressing first:
//! its status mark, its name or kind, what it is on, a line per subagent
//! that runs), a "+N more" line, then the CI and pull requests of its
//! folder's repository once something there needs a look (a run failed, a
//! review waits): a line per group, each a label and a chip per branch or
//! pull request, and a count of the other pull requests. Quiet things stay
//! quiet: an idle agent is muted, a green CI is one mark on the header,
//! and a workspace with nothing going on folds itself (see
//! `WorkspaceView::is_quiet`). Above the workspaces, a "Needs you" list
//! gathers the agents of the other workspaces that are blocked or done. The
//! active workspace's block is tinted. The header of the sidebar holds the
//! project's name, a button opening the repositories panel and a "+"
//! button opening the menu of what to add. What moves (hover pills, the
//! pulse on a blocked agent, a name being renamed) comes in through
//! `SidebarState`.

use super::repos::scrollbar;
use super::{
    BAR_HEIGHT, BRANCH_ICON_GAP, BRANCH_ICON_WIDTH, CARD_LINE, CHEVRON_WIDTH, Canvas,
    DETAIL_FONT_SIZE, Density, FONT_SIZE, FieldSpot, GAP, HEADER_BUTTON, HEADER_MARGIN,
    HEADER_NAME_END, HEADER_NAME_GAP, PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette,
    ROW_ICON_WIDTH, ROW_INSET, ROW_PICTURE_SIZE, ROW_RADIUS, ROWS_TOP, Rect, Rgb, SMALL_FONT_SIZE,
    STATUS_GAP, STATUS_WIDTH, TAB_LINE, TINY_FONT_SIZE, TabView, app, band, chevron_down, ci_mark,
    cross, distinct, folder, git_branch, icon_sdf, measure, mix, new_tab, physical, rect_px,
    restart_arrow, status_mark, terminal,
};
use crate::WorkspaceKind;
use crate::agents::AgentKey;
use crate::app_icons::{AppIcon, github_icon};
use crate::ci::{CiState, Group};
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
/// A workspace header's height, in comfortable and compact density.
const HEADER_ROW: i32 = 30;
const HEADER_ROW_COMPACT: i32 = 26;
/// A header with a branch: the band of its name, then the line under it
/// of its branch, in comfortable and compact density.
const NAME_LINE: i32 = 24;
const NAME_LINE_COMPACT: i32 = 22;
const BRANCH_LINE: i32 = 16;
const BRANCH_LINE_COMPACT: i32 = 13;
/// An agent row's first line, in comfortable and compact density.
const AGENT_ROW: i32 = 24;
const AGENT_ROW_COMPACT: i32 = 22;
/// The gap between the rows under a header.
const CHILD_GAP: i32 = 2;
/// The room after a workspace's block, in comfortable and compact density.
const BLOCK_GAP: i32 = 8;
const BLOCK_GAP_COMPACT: i32 = 4;
/// The fold chevron's column, left of the kind's icon.
const FOLD_X: i32 = ROW_INSET + 2;
const FOLD_WIDTH: i32 = 12;
/// Where a header's kind icon starts, and its name.
const ICON_X: i32 = FOLD_X + FOLD_WIDTH;
const NAME_X: i32 = ICON_X + ROW_ICON_WIDTH + 7;
/// How far a row under a header reaches left of its status mark, which
/// sits under the header's name; and where the row starts.
const CHILD_PAD: i32 = 6;
const CHILD_X: i32 = NAME_X - CHILD_PAD;
/// Where a row's text starts under a header: after its status mark.
const CHILD_TEXT_X: i32 = NAME_X + STATUS_WIDTH + STATUS_GAP;
/// The "×" at the right end of a header, and of a deletable agent row.
const CLOSE_SLOT: i32 = 22;
/// The restart button left of a header's "×".
const RESTART_SLOT: i32 = 22;
/// The room of the header's CI mark.
const CI_SLOT: i32 = 18;
/// The room of the header's status mark, and the gap before the count of
/// agents a folded workspace hides.
const STATUS_SLOT: i32 = 16;
const COUNT_GAP: i32 = 2;
/// The gap between a header's name and what is right of it.
const NAME_GAP: i32 = 4;
/// Room for the count of other worktrees on the header.
const WORKTREE_COUNT_WIDTH: i32 = 30;
/// How far a hover pill on the header's count or CI mark reaches past it.
const COUNT_PILL_PAD: i32 = 4;
/// The most lines a workspace gives worktrees that need a look; a last
/// line counts the rest.
const WORKTREE_LINES: usize = 3;
/// The room a header's worktree lines leave under them.
const ATTENTION_BOTTOM: i32 = 4;
/// The gap between the header's name pill and its repositories button,
/// and between that and the "+".
const HEADER_GAP: i32 = 4;
const HEADER_BUTTON_GAP: i32 = 2;
/// How far the repositories button's count badge sticks out of its top
/// right corner: up only, the "+" being right next to it.
const REPOS_BADGE_OUT: i32 = 1;
/// The field a header's name turns into while renaming: its height and
/// how far it reaches left of the name.
const RENAME_HEIGHT: i32 = 24;
const RENAME_INSET: i32 = 5;
/// How much of the warning colour a blocked agent's row carries at the
/// top of its pulse.
const BLOCKED_TINT: f32 = 0.15;
/// How much of the tint a blocked row keeps at the bottom of its
/// breathing; the rest comes and goes with the pulse.
const BLOCKED_REST: f32 = 0.4;
/// How much of the selection colour tints the active workspace's block.
const ACTIVE_TINT: f32 = 0.45;
/// The "Needs you" list: its heading's height, and the room after it.
const INBOX_TITLE: i32 = 20;
const INBOX_GAP: i32 = 6;
/// The most of an inbox row its workspace's name takes, as a part of it.
const INBOX_PLACE: f32 = 0.4;
/// The gap between an agent row's name and what it is on.
const TITLE_GAP: i32 = 6;
/// A GitHub section's chip: the room around its mark and label, and the
/// gap between them; the gap between chips.
const CI_CHIP_PAD: i32 = 4;
const CI_MARK_GAP: i32 = 4;
const CI_CHIP_GAP: i32 = 4;
const CI_PILL_RADIUS: f32 = 4.0;
/// How big GitHub's mark is at the start of a GitHub section.
const CI_ICON_SIZE: i32 = 12;
/// The gap between the GitHub section's widest line label and its chips.
const CI_LABEL_GAP: i32 = 8;
/// The room above and below a GitHub section's lines.
const CI_PAD: i32 = 2;
/// How far the GitHub section's count of other pull requests tints its
/// pill toward the muted text colour at rest; under the pointer, twice as
/// far.
const CI_COUNT_TINT: f32 = 0.15;
/// The room left under the last workspace when the sidebar is scrolled to
/// its end, and the scrollbar's width and its gap from the right edge.
const SCROLL_END: i32 = 8;
const SCROLLBAR_WIDTH: i32 = 4;
const SCROLLBAR_INSET: i32 = 1;
/// Without a font, how wide a character is guessed to be, as a part of
/// its size.
const GUESS_CHAR: f32 = 0.55;

fn header_height(density: Density) -> i32 {
    match density {
        Density::Comfortable => HEADER_ROW,
        Density::Compact => HEADER_ROW_COMPACT,
    }
}

fn agent_row(density: Density) -> i32 {
    match density {
        Density::Comfortable => AGENT_ROW,
        Density::Compact => AGENT_ROW_COMPACT,
    }
}

fn block_gap(density: Density) -> i32 {
    match density {
        Density::Comfortable => BLOCK_GAP,
        Density::Compact => BLOCK_GAP_COMPACT,
    }
}

/// The first line of an agent row at `row`, as `agent_row` tints it when
/// blocked: the part the pulse breathes over.
pub fn blocked_band(row: Rect, density: Density) -> Rect {
    Rectangle::new(
        row.loc,
        (row.size.w, agent_row(density).min(row.size.h)).into(),
    )
}

/// How opaque the pulse band over a blocked agent's row is drawn when the
/// pulse stands at `pulse` (0 to 1).
pub fn pulse_alpha(pulse: f32) -> f32 {
    BLOCKED_TINT * (1.0 - BLOCKED_REST) * pulse.clamp(0.0, 1.0)
}

/// One line of what is known of an agent, past its name.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentLine {
    /// What the agent is on, from its terminal title.
    Title(String),
    /// A labelled value.
    Field(String, String),
}

/// What an agent row shows: everything herdr (or Cursor) reports about
/// the agent. The row shows its name and what it is on, and a line per
/// subagent that runs; the rest is for its tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentView {
    /// Which agent; a click on the row shows it.
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
    /// Whether it is history (a Cursor chat gone idle) that a "×" on the
    /// row deletes from gerdr.
    pub deletable: bool,
    /// Its subagents that run.
    pub subagents: Vec<SubagentView>,
    /// Whether it also shows in the "Needs you" list at the top.
    pub inbox: bool,
}

/// A subagent on its parent's row: what it is on, and the agent a click
/// on its line shows, once known.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentView {
    pub key: Option<AgentKey>,
    pub task: String,
}

impl AgentView {
    /// The view of `info`, running in herdr pane `pane`.
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
            deletable: false,
            subagents: Vec::new(),
            inbox: false,
        }
    }

    /// The view of Cursor chat `chat`.
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
        lines.push(field("chat", row.id.clone()));
        let subagents = chat
            .subagents
            .iter()
            .map(|run| SubagentView {
                key: run.chat.clone().map(AgentKey::Cursor),
                task: run.task.clone().unwrap_or_else(|| "Working".into()),
            })
            .collect();
        // A chat not in agent mode says which it is in.
        let kind = match row.mode_name() {
            Some(mode) => format!("cursor {mode}"),
            None => "cursor".into(),
        };
        Self {
            key: AgentKey::Cursor(row.id.clone()),
            dir,
            status: chat.status,
            kind,
            name: row.name.clone().filter(|n| !n.trim().is_empty()),
            model: chat.model.clone(),
            lines,
            deletable: false,
            subagents,
            inbox: false,
        }
    }

    /// What it is on: its title line, if any.
    pub fn title(&self) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            AgentLine::Title(title) => Some(title.as_str()),
            _ => None,
        })
    }

    /// What its tooltip calls the agent: its kind, then its name and its
    /// model.
    pub fn label(&self) -> String {
        [Some(&self.kind), self.name.as_ref(), self.model.as_ref()]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// What its row calls the agent: the name it was given, else its kind.
    pub fn heading(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.kind)
    }

    /// How many lines its subagents take below its first: one each up to
    /// `SUBAGENT_LINES`, and one counting the rest.
    pub fn subagent_lines(&self) -> usize {
        let n = self.subagents.len();
        if n > SUBAGENT_LINES {
            SUBAGENT_LINES + 1
        } else {
            n
        }
    }

    /// Where line `j` of its subagents sits on its row at `row`: from a
    /// pill's padding before the text to the row's padding.
    pub fn subagent_line(&self, row: Rect, density: Density, j: usize) -> Rect {
        let x = row.loc.x + CHILD_TEXT_X - CHILD_X - PILL_PADDING;
        let y = row.loc.y + agent_row(density) + j as i32 * CARD_LINE;
        let w = (row.loc.x + row.size.w - CHILD_PAD / 2 - x).max(0);
        Rectangle::new((x, y).into(), (w, CARD_LINE).into())
    }

    /// Everything known of it, for its tooltip under `label`: a line each,
    /// its status first, said in full.
    pub fn tooltip_fields(&self) -> Vec<(String, String)> {
        let mut fields = vec![("Status".to_owned(), status_meaning(self.status).to_owned())];
        fields.extend(self.lines.iter().map(|l| match l {
            AgentLine::Title(title) => ("Working on".to_owned(), title.clone()),
            AgentLine::Field(label, value) => (label.clone(), value.clone()),
        }));
        let subagents = self.subagents.iter();
        fields.extend(subagents.map(|sub| (SUBAGENT.to_owned(), sub.task.clone())));
        fields
    }

    /// Its row's height at `density`, in logical pixels: a line, and one
    /// per subagent line.
    pub fn height(&self, density: Density) -> i32 {
        agent_row(density) + self.subagent_lines() as i32 * CARD_LINE
    }
}

/// The label of the field holding the folder an agent works in.
const FOLDER: &str = "Folder";
/// The label of a field holding what a subagent that runs is to do.
const SUBAGENT: &str = "Subagent";
/// How many subagents a row shows a line each at most.
pub const SUBAGENT_LINES: usize = 5;
/// How far a subagent's line starts its text in, past its elbow.
const SUBAGENT_INDENT: i32 = 12;

/// `key` as a label: its words split on `_` and `-`, the first capitalised.
fn sentence_case(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
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

/// What a workspace header shows: its name and kind, and what its tabs
/// show, put together.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceView {
    pub name: String,
    pub kind: WorkspaceKind,
    /// The most pressing status among its tabs.
    pub status: Option<AgentStatus>,
    pub branch: Option<String>,
    /// Where its folder's repository stands, shown after `branch`.
    pub git: Option<GitStatus>,
    /// Where its tabs are; the header leaves it to its tooltip.
    pub place: String,
    /// How many tabs it has and what runs in them; empty without herdr.
    /// The tab strip shows this, so the header leaves it to its tooltip.
    pub activity: String,
    /// The app icon its header shows in place of its kind's, if any.
    pub icon: Option<AppIcon>,
    /// What the agents kept under it that its rows leave out are called:
    /// a "+N more" line after the rows counts them.
    pub more_agents: Vec<String>,
    /// The CI and pull requests of its folder's repository: a mark on its
    /// header, or a GitHub section after its agents once it needs a look.
    pub ci: Option<CiView>,
    /// Whether it shows just its header: folded by hand, or quiet.
    pub folded: bool,
    /// Whether it has commands that run again after a reboot: its header
    /// then has a button restarting them all.
    pub restart: bool,
}

/// What a GitHub section shows: a chip per branch or pull request, a line
/// per group of them, and how many other pull requests the repository has
/// (a count that opens its pull requests page).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CiView {
    pub chips: Vec<CiChip>,
    pub other_prs: usize,
}

/// A chip of the GitHub section: a status mark and a label, `main` or
/// `#42`.
#[derive(Clone, Debug, PartialEq)]
pub struct CiChip {
    pub label: String,
    pub state: CiState,
    /// A pull request's title, for the tooltip.
    pub title: Option<String>,
    /// Watched by the user, so it can be unwatched.
    pub watched: bool,
    /// Its checks, by name, for the tooltip.
    pub checks: Vec<(String, CiState)>,
    /// The line it shows on; a view's chips come line by line.
    pub group: Group,
}

impl CiView {
    /// Whether it has anything to show, neither a chip nor another pull
    /// request: nothing shows otherwise.
    pub fn is_empty(&self) -> bool {
        self.chips.is_empty() && self.other_prs == 0
    }

    /// Whether it needs no look: no run failed and no review waits. It is
    /// then one mark on its workspace's header, not a section.
    pub fn quiet(&self) -> bool {
        !self
            .chips
            .iter()
            .any(|c| c.state == CiState::Failed || c.group == Group::Review)
    }

    /// Where its runs stand together, as the header's mark shows it:
    /// failed if one did, else running if one runs, else passed if one
    /// passed.
    pub fn rollup(&self) -> CiState {
        let any = |state| self.chips.iter().any(|c| c.state == state);
        [CiState::Failed, CiState::Running, CiState::Passed]
            .into_iter()
            .find(|s| any(*s))
            .unwrap_or(CiState::Unknown)
    }
}

/// The count of `n` other pull requests, as the GitHub section says it.
fn prs_label(n: usize) -> String {
    format!("+{n} PRs")
}

/// The word the GitHub section's lines of `group` start with.
pub fn group_label(group: Group) -> &'static str {
    match group {
        Group::Branch => "CI",
        Group::Mine => "Mine",
        Group::Review => "Review",
    }
}

/// How wide `text` is at `size` in logical pixels, measured with the UI
/// font; without one, a guess.
fn text_extent(text: &str, size: f32) -> i32 {
    let guess = || text.chars().count() as f32 * size * GUESS_CHAR;
    measure(text, size).unwrap_or_else(guess).ceil() as i32
}

/// How wide the GitHub section's chip labelled `label` is: its mark and
/// its label, with room around them.
fn ci_chip_width(label: &str) -> i32 {
    2 * CI_CHIP_PAD + STATUS_WIDTH + CI_MARK_GAP + text_extent(label, DETAIL_FONT_SIZE)
}

/// The colour of the mark of a CI run standing at `state`: a done agent's
/// when it passed, the error colour when it failed, a working agent's
/// while it runs.
fn ci_color(state: CiState, p: &Palette) -> Rgb {
    match state {
        CiState::Passed => p.success,
        CiState::Failed => p.error,
        CiState::Running => p.accent,
        CiState::Unknown => p.placeholder,
    }
}

/// A right-pointing chevron: a folded workspace's.
fn chevron_right(x: f32, y: f32, s: f32) -> f32 {
    chevron_down(y, x, s)
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
            more_agents: Vec::new(),
            ci: None,
            folded: false,
            restart: false,
        }
    }

    /// Whether nothing goes on in it: no agent past idle, and no CI that
    /// needs a look. Such a workspace folds itself unless it is the active
    /// one or was unfolded by hand.
    pub fn is_quiet(&self) -> bool {
        self.status
            .is_none_or(|s| urgency(s) <= urgency(AgentStatus::Idle))
            && self.ci.as_ref().is_none_or(CiView::quiet)
    }

    /// Its header's height and the worktree lines under it, at `density`.
    pub fn height(&self, density: Density) -> i32 {
        let lines = self.attention_lines(density) as i32;
        let bottom = if lines > 0 { ATTENTION_BOTTOM } else { 0 };
        let (name, branch) = self.header_lines(density);
        name + branch + lines * TAB_LINE + bottom
    }

    /// The heights of its header's name band and of the line under it
    /// holding its branch: with a branch, it has both, so a long name
    /// keeps the width and the branch always shows.
    fn header_lines(&self, density: Density) -> (i32, i32) {
        match (self.branch.is_some(), density) {
            (false, _) => (header_height(density), 0),
            (true, Density::Comfortable) => (NAME_LINE, BRANCH_LINE),
            (true, Density::Compact) => (NAME_LINE_COMPACT, BRANCH_LINE_COMPACT),
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

    /// How many lines those take under its header at `density`: one each
    /// up to `WORKTREE_LINES`, and one counting the rest; none while it is
    /// folded or compact.
    fn attention_lines(&self, density: Density) -> usize {
        if self.folded || density == Density::Compact {
            return 0;
        }
        let n = self.needing_attention().len();
        if n > WORKTREE_LINES {
            WORKTREE_LINES + 1
        } else {
            n
        }
    }

    /// Whether its CI shows as the mark on its header: while it needs no
    /// look, or the workspace is folded.
    fn ci_mark(&self) -> bool {
        self.ci
            .as_ref()
            .is_some_and(|ci| !ci.is_empty() && (self.folded || ci.quiet()))
    }

    /// Its GitHub section, while it needs a look and the workspace shows
    /// it.
    fn ci_section(&self) -> Option<&CiView> {
        let ci = self.ci.as_ref()?;
        (!self.folded && !ci.is_empty() && !ci.quiet()).then_some(ci)
    }
}

/// How pressing `status` is, for picking the one a workspace shows and
/// ordering the rows.
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

/// The agents the "Needs you" list shows, as (workspace, agent) indices:
/// those marked for it, the most pressing first, and among equals by
/// workspace and as each workspace orders them.
pub fn inbox_order(agents: &[Vec<AgentView>]) -> Vec<(usize, usize)> {
    let mut entries: Vec<(usize, usize)> = agents
        .iter()
        .enumerate()
        .flat_map(|(i, cards)| {
            card_order(cards)
                .into_iter()
                .filter(|k| cards[*k].inbox)
                .map(move |k| (i, k))
        })
        .collect();
    entries.sort_by_key(|(i, k)| std::cmp::Reverse(urgency(agents[*i][*k].status)));
    entries
}

/// What moves in a workspace header: its hover pill's alpha (0 to 16) and
/// whether the pointer is on its "×", its restart button, its worktree
/// count, its fold chevron, its CI mark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowState {
    pub hover: u8,
    pub close: bool,
    pub restart: bool,
    pub worktrees: bool,
    pub fold: bool,
    pub ci: bool,
}

/// What moves in an agent row: its hover pill's alpha (0 to 16). A blocked
/// row's pulse is not here: it is composed over the sidebar as its own
/// part (see `blocked_band`), so it never redraws it. Also whether its
/// agent is open (its pane has the keyboard in the shown tab), whether the
/// pointer is on its "×", and the hover alphas of its subagents' lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardState {
    pub hover: u8,
    pub open: bool,
    pub close: bool,
    /// The hover alpha of each of its subagents' lines.
    pub subagents: [u8; SUBAGENT_LINES],
}

/// What moves in a GitHub section: the hover alphas (0 to 16) of the
/// section off its chips and count, of each chip (as `CiView::chips`
/// lists them) and of its count of other pull requests. A part lights
/// alone: the section fades while the pointer is on a chip, as
/// `SidebarHit::pill` has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiCardState {
    pub hover: u8,
    pub chips: Vec<u8>,
    pub others: u8,
}

impl CiCardState {
    fn chip(&self, k: usize) -> u8 {
        self.chips.get(k).copied().unwrap_or(0)
    }
}

/// What the sidebar shows beyond its data: the density, the hover and
/// pulse alphas, whether the "+" menu is open, and the name being renamed.
/// Alphas are quantized so a buffer drawn from it is drawn again only when
/// a step changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarState {
    pub density: Density,
    /// Per workspace header.
    pub rows: Vec<RowState>,
    /// Per workspace, per agent as the data lists them (not as drawn).
    pub cards: Vec<Vec<CardState>>,
    /// Per workspace, its GitHub section.
    pub ci: Vec<CiCardState>,
    /// The hover alpha of each row of the "Needs you" list, as
    /// `inbox_order` lists them.
    pub inbox: Vec<u8>,
    /// The "+" button's hover alpha, and whether its menu is open.
    pub plus: u8,
    pub plus_open: bool,
    /// The repositories button's hover alpha, and whether the panel is
    /// open.
    pub repos: u8,
    pub repos_open: bool,
    /// How many of the project's repositories have uncommitted changes or
    /// are stopped half-way (a merge, a rebase, a conflict), the button's
    /// badge; and whether any is stopped, which turns it red.
    pub repos_changed: usize,
    pub repos_stuck: bool,
    /// The workspace being renamed, and the name typed so far.
    pub rename: Option<(usize, Line)>,
    /// How far the rows are scrolled up under the header, in logical
    /// pixels.
    pub scroll: i32,
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

    fn ci(&self, i: usize) -> CiCardState {
        self.ci.get(i).cloned().unwrap_or_default()
    }

    fn inbox(&self, n: usize) -> u8 {
        self.inbox.get(n).copied().unwrap_or(0)
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

/// Where the parts of a workspace header sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    /// The header and its worktree lines.
    pub rect: Rect,
    /// The band holding its name.
    pub title: Rect,
    /// Where its name goes: under the "×" and the restart button too
    /// while nothing else is right of them, since they show only under
    /// the pointer.
    pub name: Rect,
    /// The line under the name holding its branch, when it has one.
    pub branch: Option<Rect>,
    /// Its fold chevron, when it has anything to fold.
    pub fold: Option<Rect>,
    /// Its status, when it shows: folded, or an agent blocked.
    pub chip: Option<Rect>,
    /// The mark of its CI, while that shows on the header: at the right
    /// end of the branch's line, else of the name's band.
    pub ci: Option<Rect>,
    /// Its "×" close button, at the right end.
    pub close: Rect,
    /// Its button restarting its commands, left of the "×", when it has
    /// commands to restart.
    pub restart: Option<Rect>,
    /// The count of its other worktrees, when it has any.
    pub worktrees: Option<Rect>,
    /// The lines under the header of the worktrees needing a look, when
    /// it shows any.
    pub attention: Option<Rect>,
}

impl RowLayout {
    /// Where the buttons showing only under the pointer start: the
    /// restart button, else the "×".
    pub fn buttons_x(&self) -> i32 {
        self.restart.unwrap_or(self.close).loc.x
    }

    /// The field the name turns into while renaming: over the name and
    /// the branch.
    pub fn rename_field(&self) -> Rect {
        let x = self.name.loc.x - RENAME_INSET;
        let y = self.title.loc.y + (self.title.size.h - RENAME_HEIGHT) / 2;
        let end = [self.worktrees, self.ci, self.chip]
            .into_iter()
            .flatten()
            .filter(|r| r.loc.y == self.title.loc.y)
            .map(|r| r.loc.x)
            .min()
            .unwrap_or(self.buttons_x())
            - 2;
        Rectangle::new((x, y).into(), ((end - x).max(0), RENAME_HEIGHT).into())
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
    /// The "Needs you" heading, when the list shows, and its rows: each
    /// with its agent's workspace and index among that workspace's agents.
    pub inbox_title: Option<Rect>,
    pub inbox: Vec<(usize, usize, Rect)>,
    /// One per workspace.
    pub rows: Vec<RowLayout>,
    /// Per workspace, its agent rows as drawn (the most pressing first):
    /// each with its index among the workspace's agents as the data lists
    /// them. None while folded.
    pub cards: Vec<Vec<(usize, Rect)>>,
    /// Per workspace, the "×" of each agent row that has one, by its index.
    pub card_closes: Vec<Vec<(usize, Rect)>>,
    /// Per workspace, the lines of the subagents a click shows: each with
    /// its agent's index and its own among the agent's subagents.
    pub subagents: Vec<Vec<(usize, usize, Rect)>>,
    /// Per workspace, the "+N more" line after its agent rows, if it has
    /// one.
    pub more: Vec<Option<Rect>>,
    /// Per workspace, its GitHub section, if it shows one.
    pub ci: Vec<Option<CiLayout>>,
    /// Per workspace, all it takes: its header, what is under it and the
    /// gap after them.
    pub blocks: Vec<Rect>,
    /// The gap closing each block.
    pub block_gap: i32,
    /// How far everything under the header is scrolled up (see `scrolled`).
    pub scroll: i32,
}

/// Where the parts of a GitHub section sit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLayout {
    pub card: Rect,
    /// One per chip, as `CiView::chips` lists them.
    pub chips: Vec<Rect>,
    /// The label starting each line of chips, by the group it names, in
    /// the band of that line's chips.
    pub labels: Vec<(Group, Rect)>,
    /// The count of other pull requests, when there are any.
    pub others: Option<Rect>,
}

/// Where the "×" of an agent row at `row` sits: at the right end of its
/// first line.
pub fn card_close(row: Rect, density: Density) -> Rect {
    Rectangle::new(
        (row.loc.x + row.size.w - CLOSE_SLOT, row.loc.y).into(),
        (CLOSE_SLOT, agent_row(density)).into(),
    )
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
    /// Workspace `i`: its header off any part below, its name, its status,
    /// its "×", its restart button, the count of its other worktrees, its
    /// fold chevron, the mark of its CI.
    Row(usize),
    Name(usize),
    Chip(usize),
    Close(usize),
    Restart(usize),
    Worktrees(usize),
    Fold(usize),
    CiMark(usize),
    /// Line `k` of workspace `i`'s worktrees needing a look: the worktree
    /// `k` of `WorkspaceView::needing_attention`, or the "+N more" line.
    Attention(usize, usize),
    /// Agent `k` (as the data lists them) of workspace `i`: its row, its
    /// "×", and the line of its subagent `j`.
    Card(usize, usize),
    CardClose(usize, usize),
    Subagent(usize, usize, usize),
    /// The "Needs you" list's heading: it opens the inbox.
    InboxTitle,
    /// Agent `k` of workspace `i` in the "Needs you" list.
    Inbox(usize, usize),
    /// The "+N more" line after workspace `i`'s agent rows.
    More(usize),
    /// Workspace `i`'s GitHub section off its chips and count (its mark,
    /// its line labels, the gaps), chip `k` of it (as `CiView::chips` lists
    /// them), and its count of other pull requests.
    Ci(usize),
    CiChip(usize, usize),
    CiPrs(usize),
}

impl SidebarHit {
    /// The workspace whose header a hit is on any part of.
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Row(i)
            | Self::Name(i)
            | Self::Chip(i)
            | Self::Close(i)
            | Self::Restart(i)
            | Self::Worktrees(i)
            | Self::Fold(i)
            | Self::CiMark(i)
            | Self::Attention(i, _) => Some(i),
            _ => None,
        }
    }

    /// What lights up under this hit: a header for any part of it, an
    /// agent row for its "×"; anything else (a GitHub chip, its count, an
    /// inbox row) itself.
    pub fn pill(self) -> Self {
        match (self.row(), self) {
            (Some(i), _) => Self::Row(i),
            (None, Self::CardClose(i, k)) => Self::Card(i, k),
            (None, _) => self,
        }
    }
}

impl SidebarLayout {
    /// What is at `pos`. Rows scrolled under the header are not.
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
        if pos.y < BAR_HEIGHT as f64 {
            return None;
        }
        if self.inbox_title.is_some_and(inside) {
            return Some(SidebarHit::InboxTitle);
        }
        if let Some((i, k, _)) = self.inbox.iter().find(|(_, _, rect)| inside(*rect)) {
            return Some(SidebarHit::Inbox(*i, *k));
        }
        for (i, closes) in self.card_closes.iter().enumerate() {
            if let Some((k, _)) = closes.iter().find(|(_, rect)| inside(*rect)) {
                return Some(SidebarHit::CardClose(i, *k));
            }
        }
        for (i, lines) in self.subagents.iter().enumerate() {
            if let Some((k, j, _)) = lines.iter().find(|(_, _, rect)| inside(*rect)) {
                return Some(SidebarHit::Subagent(i, *k, *j));
            }
        }
        for (i, cards) in self.cards.iter().enumerate() {
            if let Some((k, _)) = cards.iter().find(|(_, rect)| inside(*rect)) {
                return Some(SidebarHit::Card(i, *k));
            }
        }
        if let Some(i) = self.more.iter().position(|m| m.is_some_and(inside)) {
            return Some(SidebarHit::More(i));
        }
        for (i, ci) in self.ci.iter().enumerate() {
            let Some(ci) = ci.as_ref().filter(|ci| inside(ci.card)) else {
                continue;
            };
            return Some(if ci.others.is_some_and(inside) {
                SidebarHit::CiPrs(i)
            } else if let Some(k) = ci.chips.iter().position(|r| inside(*r)) {
                SidebarHit::CiChip(i, k)
            } else {
                SidebarHit::Ci(i)
            });
        }
        let (i, row) = self.rows.iter().enumerate().find(|(_, r)| inside(r.rect))?;
        Some(if inside(row.close) {
            SidebarHit::Close(i)
        } else if row.restart.is_some_and(inside) {
            SidebarHit::Restart(i)
        } else if row.fold.is_some_and(inside) {
            SidebarHit::Fold(i)
        } else if row.worktrees.is_some_and(inside) {
            SidebarHit::Worktrees(i)
        } else if row.ci.is_some_and(inside) {
            SidebarHit::CiMark(i)
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

impl SidebarLayout {
    /// How far the rows can scroll in a sidebar `height` tall: what of
    /// them (and the room after the last) does not fit under the header.
    pub fn max_scroll(&self, height: i32) -> i32 {
        let end = self
            .blocks
            .last()
            .map_or(ROWS_TOP, |b| b.loc.y + b.size.h + self.scroll);
        (end + SCROLL_END - height).max(0)
    }

    /// The scroll nearest `scroll` showing all of workspace `i`'s block (its
    /// top when it is taller than the room) in a sidebar `height` tall.
    pub fn showing(&self, i: usize, scroll: i32, height: i32) -> i32 {
        let Some(block) = self.blocks.get(i) else {
            return scroll;
        };
        let top = block.loc.y + self.scroll;
        let (first, last) = (top - ROWS_TOP, top + block.size.h + SCROLL_END - height);
        scroll
            .max(last)
            .min(first)
            .clamp(0, self.max_scroll(height))
    }

    /// This layout with everything under the header moved up by `scroll`,
    /// held to what a sidebar `height` tall can scroll (`max_scroll`).
    pub fn scrolled(mut self, scroll: i32, height: i32) -> Self {
        let by = scroll.clamp(0, self.max_scroll(height)) - self.scroll;
        if by == 0 {
            return self;
        }
        let up = |r: &mut Rect| r.loc.y -= by;
        self.inbox_title.iter_mut().for_each(up);
        for (_, _, r) in &mut self.inbox {
            up(r);
        }
        for row in &mut self.rows {
            for r in [&mut row.rect, &mut row.title, &mut row.name, &mut row.close] {
                up(r);
            }
            for r in [
                &mut row.restart,
                &mut row.branch,
                &mut row.fold,
                &mut row.chip,
                &mut row.ci,
                &mut row.worktrees,
                &mut row.attention,
            ]
            .into_iter()
            .flatten()
            {
                up(r);
            }
        }
        for (_, r) in self.cards.iter_mut().chain(&mut self.card_closes).flatten() {
            up(r);
        }
        for (_, _, r) in self.subagents.iter_mut().flatten() {
            up(r);
        }
        self.more.iter_mut().flatten().for_each(up);
        for ci in self.ci.iter_mut().flatten() {
            up(&mut ci.card);
            ci.chips.iter_mut().for_each(up);
            ci.labels.iter_mut().for_each(|(_, r)| up(r));
            ci.others.iter_mut().for_each(up);
        }
        self.blocks.iter_mut().for_each(up);
        self.scroll += by;
        self
    }
}

/// Where the header's parts, the "Needs you" list, the headers of
/// `workspaces`, their `agents` rows and their GitHub sections sit in a
/// sidebar `width` wide, in comfortable density.
#[cfg(test)]
pub fn sidebar_layout(
    workspaces: &[WorkspaceView],
    agents: &[Vec<AgentView>],
    width: i32,
) -> SidebarLayout {
    sidebar_layout_at(workspaces, agents, width, Density::Comfortable)
}

/// The width of workspace `ws`'s status on its header, when it shows one:
/// a badge counting its blocked agents, else while folded its status mark
/// and how many agents it hides.
fn status_extent(ws: &WorkspaceView, cards: &[AgentView]) -> Option<i32> {
    let blocked = blocked_count(cards);
    if blocked > 0 {
        let w = text_extent(&blocked.to_string(), TINY_FONT_SIZE) + 2 * PILL_PADDING;
        return Some(w.max(PILL_HEIGHT));
    }
    ws.status.filter(|_| ws.folded)?;
    Some(match cards.len() {
        0 => STATUS_SLOT,
        n => STATUS_SLOT + COUNT_GAP + text_extent(&n.to_string(), TINY_FONT_SIZE),
    })
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
    let right = width - GAP - ROW_INSET;
    let child_w = (right - CHILD_X).max(0);
    let child = |y: i32, h: i32| Rectangle::new((CHILD_X, y).into(), (child_w, h).into());
    let mut y = ROWS_TOP;
    let none = Vec::new();

    // The "Needs you" list: a heading, then a one-line row per agent.
    let entries = inbox_order(agents);
    let inbox_title = (!entries.is_empty()).then(|| {
        let title = Rectangle::new((0, y).into(), (width - GAP, INBOX_TITLE).into());
        y += INBOX_TITLE;
        title
    });
    let inbox = entries
        .into_iter()
        .map(|(i, k)| {
            let rect = child(y, agent_row(density));
            y += rect.size.h + CHILD_GAP;
            (i, k, rect)
        })
        .collect();
    if inbox_title.is_some() {
        y += INBOX_GAP;
    }

    let (mut rows, mut card_rects, mut blocks) = (Vec::new(), Vec::new(), Vec::new());
    let (mut card_closes, mut more, mut ci) = (Vec::new(), Vec::new(), Vec::new());
    let mut subagents = Vec::new();
    for (i, ws) in workspaces.iter().enumerate() {
        let cards = agents.get(i).unwrap_or(&none);
        let attention_lines = ws.attention_lines(density) as i32;
        let foldable = !cards.is_empty()
            || !ws.more_agents.is_empty()
            || ws.ci.as_ref().is_some_and(|ci| !ci.is_empty())
            || !ws.needing_attention().is_empty();
        let folded = ws.folded && foldable;

        // The header: from the right, its "×", its restart button, its
        // status, its CI mark and its worktree count, its name in the rest. With a branch, the
        // branch goes on a line under the name, with the CI mark and the
        // count at that line's right end.
        let h = ws.height(density);
        let (title_h, branch_h) = ws.header_lines(density);
        let rect = Rectangle::new((0, y).into(), (width - GAP, h).into());
        let title = Rectangle::new((0, y).into(), (width - GAP, title_h).into());
        let slot = |x: i32, w: i32| Rectangle::new((x, y).into(), (w, title_h).into());
        let below = |x: i32, w: i32| Rectangle::new((x, y + title_h).into(), (w, branch_h).into());
        let close = slot(right - CLOSE_SLOT, CLOSE_SLOT);
        let restart = ws
            .restart
            .then(|| slot(close.loc.x - RESTART_SLOT, RESTART_SLOT));
        let buttons = restart.unwrap_or(close).loc.x;
        let mut end = buttons;
        let chip = status_extent(ws, cards).map(|w| {
            end -= w;
            slot(end, w)
        });
        // The CI mark and the count: at the right end of the branch's
        // line, else of the header.
        let mut branch_end = right;
        let mut take = |w: i32| {
            if branch_h > 0 {
                branch_end -= w;
                below(branch_end, w)
            } else {
                end -= w;
                slot(end, w)
            }
        };
        let ci_mark = ws.ci_mark().then(|| take(CI_SLOT));
        let worktrees = (!ws.worktrees().is_empty()).then(|| take(WORKTREE_COUNT_WIDTH));
        // The name may reach under the "×" and the restart button, which
        // show only under the pointer, while nothing else is right of them.
        let name_end = if end == buttons { right } else { end };
        let name = slot(NAME_X, (name_end - NAME_GAP - NAME_X).max(0));
        let branch = ws
            .branch
            .is_some()
            .then(|| below(NAME_X, (branch_end - NAME_GAP - NAME_X).max(0)));
        let fold = foldable.then(|| slot(ROW_INSET, ICON_X - ROW_INSET));
        let attention = (attention_lines > 0).then(|| {
            Rectangle::new(
                (ROW_INSET, y + title_h + branch_h).into(),
                (width - GAP - 2 * ROW_INSET, attention_lines * TAB_LINE).into(),
            )
        });
        rows.push(RowLayout {
            rect,
            title,
            name,
            branch,
            fold,
            chip,
            ci: ci_mark,
            close,
            restart,
            worktrees,
            attention,
        });
        y += h;

        // What is under it, unless folded: the agent rows, the "+N more"
        // line, the GitHub section.
        let start = y;
        let ws_cards: Vec<(usize, Rect)> = if folded {
            Vec::new()
        } else {
            card_order(cards)
                .into_iter()
                .map(|k| {
                    let row = child(y, cards[k].height(density));
                    y += row.size.h + CHILD_GAP;
                    (k, row)
                })
                .collect()
        };
        card_closes.push(
            ws_cards
                .iter()
                .filter(|(k, _)| cards[*k].deletable)
                .map(|(k, row)| (*k, card_close(*row, density)))
                .collect(),
        );
        subagents.push(
            ws_cards
                .iter()
                .flat_map(|(k, rect)| {
                    let card = &cards[*k];
                    let shown = card.subagents.iter().take(SUBAGENT_LINES).enumerate();
                    shown
                        .filter(|(_, sub)| sub.key.is_some())
                        .map(|(j, _)| (*k, j, card.subagent_line(*rect, density, j)))
                        .collect::<Vec<_>>()
                })
                .collect(),
        );
        card_rects.push(ws_cards);
        let ws_more = (!folded && !ws.more_agents.is_empty()).then(|| {
            let line = child(y, CARD_LINE);
            y += CARD_LINE + CHILD_GAP;
            line
        });
        more.push(ws_more);
        let ws_ci = ws.ci_section().filter(|_| !folded).map(|view| {
            let section = ci_layout(view, (CHILD_X, y), child_w);
            y += section.card.size.h + CHILD_GAP;
            section
        });
        ci.push(ws_ci);
        if y > start {
            y += CHILD_GAP;
        }
        y += block_gap(density);
        blocks.push(Rectangle::new(
            rect.loc,
            (rect.size.w, y - rect.loc.y).into(),
        ));
    }
    SidebarLayout {
        header,
        plus,
        repos,
        inbox_title,
        inbox,
        rows,
        cards: card_rects,
        card_closes,
        subagents,
        more,
        ci,
        blocks,
        block_gap: block_gap(density),
        scroll: 0,
    }
}

/// Where the parts of the GitHub section showing `view` sit, its top left
/// corner at `(x, y)` and `width` wide: a line per group of chips, the
/// first with the count of other pull requests at its right end. Each line
/// starts with its group's label where an agent row's text starts, GitHub's
/// mark left of the first; the chips follow in a column as wide as the
/// widest label present and a gap, left to right, on a line under it (in
/// the same column) when the next one does not fit. An empty group takes
/// no line. A chip wider than the room is cut to fit.
fn ci_layout(view: &CiView, (x, y): (i32, i32), width: i32) -> CiLayout {
    let top = y + CI_PAD;
    let end = x + width - CHILD_PAD;
    let others = (view.other_prs > 0).then(|| {
        let w = text_extent(&prs_label(view.other_prs), TINY_FONT_SIZE) + 2 * PILL_PADDING;
        Rectangle::new(
            (end - w, top + (CARD_LINE - PILL_HEIGHT) / 2).into(),
            (w, PILL_HEIGHT).into(),
        )
    });
    let label_x = x + CHILD_TEXT_X - CHILD_X;
    let label_w = view
        .chips
        .iter()
        .map(|chip| text_extent(group_label(chip.group), DETAIL_FONT_SIZE))
        .max()
        .unwrap_or(0);
    let left = label_x + label_w + CI_LABEL_GAP - CI_CHIP_PAD;
    let right = end + CI_CHIP_PAD;
    // The first line leaves the count its room.
    let bound = |line: i32| match others {
        Some(o) if line == 0 => o.loc.x - CI_CHIP_GAP,
        _ => right,
    };
    let line_y = |line: i32| top + line * CARD_LINE;
    let (mut chip_x, mut line, mut group) = (left, -1, None);
    let mut labels = Vec::new();
    let chips = view
        .chips
        .iter()
        .map(|chip| {
            let full = ci_chip_width(&chip.label);
            if group != Some(chip.group) {
                (chip_x, line, group) = (left, line + 1, Some(chip.group));
                let label =
                    Rectangle::new((label_x, line_y(line)).into(), (label_w, CARD_LINE).into());
                labels.push((chip.group, label));
            } else if chip_x > left && chip_x + full.min(bound(line) - left) > bound(line) {
                (chip_x, line) = (left, line + 1);
            }
            let w = full.min((bound(line) - left).max(0));
            let rect = Rectangle::new((chip_x, line_y(line)).into(), (w, CARD_LINE).into());
            chip_x += w + CI_CHIP_GAP;
            rect
        })
        .collect();
    let lines = (line + 1).max(others.is_some() as i32);
    let height = 2 * CI_PAD + lines * CARD_LINE;
    CiLayout {
        card: Rectangle::new((x, y).into(), (width, height).into()),
        chips,
        labels,
        others,
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

/// A quantized alpha from `SidebarState` as a fraction.
fn alpha(step: u8) -> f32 {
    (step as f32 / ALPHA_STEPS).clamp(0.0, 1.0)
}

impl Painter {
    /// The sidebar: the header (project name and "+"), the "Needs you"
    /// list, a header per workspace and what is under it, with what moves
    /// in `view.state`.
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

        let state = &view.state;
        let density = state.density;
        let layout = sidebar_layout_at(&view.workspaces, &view.agents, size.w, density)
            .scrolled(state.scroll, size.h);
        self.sidebar_inbox(&mut c, view, &layout, scale, p);
        self.sidebar_rows(&mut c, view, &layout, scale, p);

        // The header over the rows scrolled under it.
        c.fill(0, 0, c.w, px(BAR_HEIGHT), p.panel);
        c.fill(0, px(BAR_HEIGHT) - line, c.w, line, p.border);
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
        if state.repos_changed > 0 {
            let fill = if state.repos_stuck { p.error } else { p.accent };
            let colors = (p.panel, fill, p.on_accent);
            let (rect, n) = (layout.repos, state.repos_changed);
            self.count_badge(&mut c, rect, n, 9, REPOS_BADGE_OUT, colors, scale);
        }

        // Where the rows are scrolled to, when they do not all fit.
        let shown = size.h - BAR_HEIGHT;
        let total = shown + layout.max_scroll(size.h);
        let track = Rectangle::new(
            (
                size.w - GAP - SCROLLBAR_INSET - SCROLLBAR_WIDTH,
                BAR_HEIGHT + SCROLLBAR_INSET,
            )
                .into(),
            (SCROLLBAR_WIDTH, shown - 2 * SCROLLBAR_INSET).into(),
        );
        let first = layout.scroll as usize;
        scrollbar(
            &mut c,
            track,
            (first, shown.max(0) as usize, total.max(0) as usize),
            scale,
            p,
        );
        c.fill(c.w - px(GAP).max(1), 0, px(GAP).max(1), c.h, p.border);
        c.into_buffer(true)
    }

    /// The "Needs you" list where `layout` has it: a muted heading, then
    /// a row per agent of another workspace that is blocked or done, with
    /// its workspace's name at the right.
    fn sidebar_inbox(
        &self,
        c: &mut Canvas,
        view: &SidebarView,
        layout: &SidebarLayout,
        scale: f64,
        p: &Palette,
    ) {
        let Some(title) = layout.inbox_title else {
            return;
        };
        let px = |v: i32| (v as f64 * scale).round() as i32;
        self.text(
            c,
            "Needs you",
            TINY_FONT_SIZE,
            px(NAME_X - CHILD_PAD) as f32,
            px(title.loc.x + title.size.w - ROW_INSET) as f32,
            band(title, scale),
            scale,
            p.muted,
        );
        let density = view.state.density;
        for (n, (i, k, rect)) in layout.inbox.iter().enumerate() {
            let (Some(card), Some(ws)) = (
                view.agents.get(*i).and_then(|cards| cards.get(*k)),
                view.workspaces.get(*i),
            ) else {
                continue;
            };
            let state = CardState {
                hover: view.state.inbox(n),
                ..CardState::default()
            };
            let place = Some(ws.name.as_str());
            self.agent_row(c, card, *rect, state, p.panel, place, density, scale, p);
        }
    }

    /// The sidebar's workspace headers and what is under them, where
    /// `layout` has them.
    fn sidebar_rows(
        &self,
        c: &mut Canvas,
        view: &SidebarView,
        layout: &SidebarLayout,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let state = &view.state;
        let density = state.density;
        let none = Vec::new();
        for (i, (row, ws)) in layout.rows.iter().zip(&view.workspaces).enumerate() {
            let is_active = Some(i) == view.active;
            let cards = view.agents.get(i).unwrap_or(&none);
            // The active workspace's block, tinted from edge to edge of
            // what it holds; what lies on it mixes from the tint.
            let under = if is_active {
                mix(p.panel, p.selected, ACTIVE_TINT)
            } else {
                p.panel
            };
            if is_active && let Some(block) = layout.blocks.get(i) {
                let h = (block.size.h - layout.block_gap).max(0);
                let inner = Rectangle::new(block.loc, (block.size.w, h).into());
                let (x, y, w, h) = rect_px(inner, scale);
                let inset = px(ROW_INSET) as f32;
                c.rounded(x + inset, y, w - 2.0 * inset, h, ROW_RADIUS * s, under);
            }
            let row_state = state.row(i);
            let hover = alpha(row_state.hover);
            let fill = (hover > 0.0).then(|| mix(under, p.element, hover));
            if let Some(fill) = fill {
                let (x, y, w, h) = rect_px(row.rect, scale);
                let inset = px(ROW_INSET) as f32;
                c.rounded(x + inset, y, w - 2.0 * inset, h, ROW_RADIUS * s, fill);
            }
            let surface = fill.unwrap_or(under);
            self.workspace_header(c, ws, cards, row, row_state, i, surface, scale, view, p);

            for (k, rect) in layout.cards.get(i).unwrap_or(&Vec::new()) {
                let card_state = state.card(i, *k);
                self.agent_row(
                    c, &cards[*k], *rect, card_state, under, None, density, scale, p,
                );
            }
            if let Some(Some(line)) = layout.more.get(i) {
                let n = ws.more_agents.len();
                self.text(
                    c,
                    &format!("+{n} more"),
                    DETAIL_FONT_SIZE,
                    px(line.loc.x + CHILD_TEXT_X - CHILD_X) as f32,
                    px(line.loc.x + line.size.w) as f32,
                    band(*line, scale),
                    scale,
                    p.placeholder,
                );
            }
            if let (Some(Some(ci)), Some(ci_view)) = (layout.ci.get(i), ws.ci.as_ref()) {
                self.ci_section(c, ci_view, ci, &state.ci(i), under, scale, p);
            }
        }
    }

    /// Workspace `ws`'s header laid out as `row` on `surface`: its fold
    /// chevron, kind icon, name, branch and marks, worktree count, CI
    /// mark and status, the "×" while hovered, and the worktree lines
    /// under it.
    #[allow(clippy::too_many_arguments)]
    fn workspace_header(
        &self,
        c: &mut Canvas,
        ws: &WorkspaceView,
        cards: &[AgentView],
        row: &RowLayout,
        row_state: RowState,
        i: usize,
        surface: Rgb,
        scale: f64,
        view: &SidebarView,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let hover = alpha(row_state.hover);
        let title_band = band(row.title, scale);
        let at = |x: i32, w: i32| {
            Rectangle::new((x, row.title.loc.y).into(), (w, row.title.size.h).into())
        };
        let within =
            |r: Rect, x: i32, w: i32| Rectangle::new((x, r.loc.y).into(), (w, r.size.h).into());

        if let Some(fold) = row.fold {
            let color = if row_state.fold {
                p.text
            } else {
                mix(p.placeholder, p.muted, hover)
            };
            let icon = Rectangle::new(
                (FOLD_X, fold.loc.y).into(),
                (FOLD_WIDTH, fold.size.h).into(),
            );
            if ws.folded {
                icon_sdf(c, icon, scale, color, |x, y| chevron_right(x, y, s));
            } else {
                icon_sdf(c, icon, scale, color, |x, y| chevron_down(x, y, s));
            }
        }
        let icon = at(ICON_X, ROW_ICON_WIDTH);
        let small = 0.8 * s;
        match ws.kind {
            WorkspaceKind::Project => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| folder(x, y, small))
            }
            WorkspaceKind::Terminal => {
                icon_sdf(c, icon, scale, p.placeholder, |x, y| terminal(x, y, small))
            }
            WorkspaceKind::Cursor => match &ws.icon {
                Some(picture) => {
                    c.picture_sized(picture, icon, scale, p.placeholder, ROW_PICTURE_SIZE)
                }
                None => icon_sdf(c, icon, scale, p.placeholder, |x, y| app(x, y, small)),
            },
        }
        match view.state.rename.as_ref().filter(|(w, _)| *w == i) {
            Some((_, typed)) => self.rename_field(c, typed, row.rename_field(), scale, p),
            None => {
                // Under the pointer the "×" and the restart button show,
                // and a name reaching under them stops short of them.
                let mut end = row.name.loc.x + row.name.size.w;
                if hover > 0.0 {
                    end = end.min(row.buttons_x() - NAME_GAP);
                }
                self.text(
                    c,
                    &ws.name,
                    FONT_SIZE,
                    px(row.name.loc.x) as f32,
                    px(end) as f32,
                    title_band,
                    scale,
                    p.text,
                );
                if let (Some(line), Some(branch)) = (row.branch, &ws.branch) {
                    let line_band = band(line, scale);
                    let icon = within(line, line.loc.x, BRANCH_ICON_WIDTH);
                    icon_sdf(c, icon, scale, p.accent, |x, y| git_branch(x, y, 0.75 * s));
                    let x = px(line.loc.x + BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32;
                    let end = px(line.loc.x + line.size.w) as f32;
                    let after = self.text(
                        c,
                        branch,
                        DETAIL_FONT_SIZE,
                        x,
                        end,
                        line_band,
                        scale,
                        p.accent,
                    );
                    if let Some(git) = &ws.git {
                        self.git_marks(c, git, after, end, line_band, scale, p);
                    }
                }
            }
        }

        // The count of its other worktrees; hovering it lists them, on a
        // pill as it opens the popup.
        if let Some(count) = row.worktrees {
            let n = ws.worktrees().len().to_string();
            let n_w = (self.text_width(&n, DETAIL_FONT_SIZE, scale) / s).ceil() as i32;
            let end = count.loc.x + count.size.w - COUNT_PILL_PAD;
            let icon = within(
                count,
                end - n_w - BRANCH_ICON_GAP - BRANCH_ICON_WIDTH,
                BRANCH_ICON_WIDTH,
            );
            let on = row_state.worktrees;
            if on {
                self.lit_pill(c, icon.loc.x, end, count, surface, scale, p);
            }
            let (icon_color, n_color) = if on {
                (p.text, p.text)
            } else {
                (p.placeholder, p.muted)
            };
            icon_sdf(c, icon, scale, icon_color, |x, y| {
                git_branch(x, y, 0.75 * s)
            });
            self.text(
                c,
                &n,
                DETAIL_FONT_SIZE,
                px(end - n_w) as f32,
                px(end) as f32,
                band(count, scale),
                scale,
                n_color,
            );
        }

        // The mark of its CI, while that needs no look or it is folded.
        if let (Some(mark), Some(ci)) = (row.ci, &ws.ci) {
            let state = ci.rollup();
            let icon = within(
                mark,
                mark.loc.x + (CI_SLOT - STATUS_WIDTH) / 2,
                STATUS_WIDTH,
            );
            if row_state.ci {
                let end = icon.loc.x + icon.size.w;
                self.lit_pill(c, icon.loc.x, end, mark, surface, scale, p);
            }
            icon_sdf(c, icon, scale, ci_color(state, p), |x, y| {
                ci_mark(state, x, y, s)
            });
        }

        if let (Some(chip), Some(status)) = (row.chip, ws.status) {
            let blocked = blocked_count(cards);
            let hidden = if ws.folded { cards.len() } else { 0 };
            self.header_status(c, status, blocked, hidden, chip, scale, p);
        }
        if hover > 0.0 {
            let color = if row_state.close {
                p.text
            } else {
                mix(surface, p.placeholder, hover)
            };
            icon_sdf(c, row.close, scale, color, |x, y| cross(x, y, 0.8 * s));
            if let Some(restart) = row.restart {
                let color = if row_state.restart {
                    p.text
                } else {
                    mix(surface, p.placeholder, hover)
                };
                icon_sdf(c, restart, scale, color, |x, y| {
                    restart_arrow(x, y, 0.8 * s)
                });
            }
        }

        // Under the header, a line per worktree that needs a look: its
        // name, a dot when changed, and why ("unmerged", "gone").
        let Some(lines) = row.attention else {
            return;
        };
        let right = px(lines.loc.x + lines.size.w) as f32 - PADDING * s;
        let text_x = px(NAME_X + BRANCH_ICON_WIDTH + BRANCH_ICON_GAP) as f32;
        let needing = ws.needing_attention();
        for k in 0..ws.attention_lines(view.state.density) {
            let line_y = lines.loc.y + TAB_LINE * k as i32;
            let band = (px(line_y), px(line_y + TAB_LINE) - px(line_y));
            let Some(wt) = needing.get(k).filter(|_| k < WORKTREE_LINES) else {
                let more = format!("+{} more", needing.len() - WORKTREE_LINES);
                self.text(
                    c,
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
                (NAME_X, line_y).into(),
                (BRANCH_ICON_WIDTH, TAB_LINE).into(),
            );
            icon_sdf(c, icon, scale, p.placeholder, |x, y| {
                git_branch(x, y, 0.75 * s)
            });
            let x = self.text(
                c,
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
            let x = self.git_marks(c, &dirty, x, right, band, scale, p);
            if let Some(tag) = wt.attention_tag() {
                self.text(
                    c,
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

    /// The pill lighting a header part from `x` to `end` under the
    /// pointer, `COUNT_PILL_PAD` past it each side, in the band `title`.
    #[allow(clippy::too_many_arguments)]
    fn lit_pill(
        &self,
        c: &mut Canvas,
        x: i32,
        end: i32,
        title: Rect,
        surface: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let h = PILL_HEIGHT + 4;
        let pill = Rectangle::new(
            (x - COUNT_PILL_PAD, title.loc.y + (title.size.h - h) / 2).into(),
            (end - x + 2 * COUNT_PILL_PAD, h).into(),
        );
        let (x, y, w, h) = rect_px(pill, scale);
        c.rounded(x, y, w, h, 4.0 * s, mix(surface, p.text, 0.1));
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

    /// A workspace header's status in `slot`, right-aligned: a filled
    /// warning badge counting `blocked` agents; else `status`'s mark in its
    /// colour, then how many agents a fold `hidden`.
    #[allow(clippy::too_many_arguments)]
    fn header_status(
        &self,
        c: &mut Canvas,
        status: AgentStatus,
        blocked: usize,
        hidden: usize,
        slot: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let band = band(slot, scale);
        if blocked > 0 {
            let h = px(PILL_HEIGHT) as f32;
            let (x0, _, w, _) = rect_px(slot, scale);
            let y = (band.0 as f32 + (band.1 as f32 - h) / 2.0).round();
            c.rounded(x0, y, w, h, h / 2.0, p.warning);
            let text = blocked.to_string();
            let text_w = self.text_width(&text, TINY_FONT_SIZE, scale);
            let x = (x0 + (w - text_w) / 2.0).round();
            let pill_band = (y as i32, h as i32);
            self.text(
                c,
                &text,
                TINY_FONT_SIZE,
                x,
                f32::MAX,
                pill_band,
                scale,
                p.on_accent,
            );
            return;
        }
        let mark = Rectangle::new(slot.loc, (STATUS_SLOT, slot.size.h).into());
        let color = p.status(status);
        icon_sdf(c, mark, scale, color, |x, y| status_mark(status, x, y, s));
        if hidden > 0 {
            let x = px(slot.loc.x + STATUS_SLOT + COUNT_GAP) as f32;
            let end = px(slot.loc.x + slot.size.w) as f32 + 1.0;
            self.text(
                c,
                &hidden.to_string(),
                TINY_FONT_SIZE,
                x,
                end,
                band,
                scale,
                p.muted,
            );
        }
    }

    /// The field a header's name turns into while renaming, showing
    /// `typed` with its selection and caret.
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

    /// An agent row on `under`: its status mark, its name or kind and what
    /// it is on, and at the right a "blocked" pill (an inbox row: its
    /// workspace's name `place` instead; a deletable row under the pointer:
    /// its "×"), then a line per subagent that runs. Idle agents are muted;
    /// a blocked one's line carries the warning colour (its breathing is
    /// composed on top, see `pulse_band`). The row lightens under the
    /// pointer; the open agent's is filled as a selection, with an accent
    /// bar at its left.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn agent_row(
        &self,
        c: &mut Canvas,
        card: &AgentView,
        rect: Rect,
        state: CardState,
        under: Rgb,
        place: Option<&str>,
        density: Density,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let r = ROW_RADIUS * s;
        let status = p.status(card.status);
        let blocked = card.status == AgentStatus::Blocked;
        let hover = alpha(state.hover);
        let fill = if state.open {
            Some(mix(p.selected, p.text, 0.05 * hover))
        } else {
            (hover > 0.0).then(|| mix(under, p.element, hover))
        };
        let base = fill.unwrap_or(under);
        if let Some(fill) = fill {
            let (x, y, w, h) = rect_px(rect, scale);
            c.rounded(x, y, w, h, r, fill);
        }
        let first = blocked_band(rect, density);
        if blocked {
            // The first line in the warning colour, at the bottom of its
            // breathing.
            let (x, y, w, h) = rect_px(first, scale);
            c.rounded(
                x,
                y,
                w,
                h,
                r,
                mix(base, p.warning, BLOCKED_TINT * BLOCKED_REST),
            );
        }
        let surface = if blocked {
            mix(base, p.warning, BLOCKED_TINT * BLOCKED_REST)
        } else {
            base
        };
        if state.open {
            let (x, y, _, h) = rect_px(rect, scale);
            let inset = (4.0 * s).round();
            c.rounded(
                x + 2.0 * s,
                y + inset,
                2.0 * s,
                h - 2.0 * inset,
                s,
                p.accent,
            );
        }

        let line_band = band(first, scale);
        let mark = Rectangle::new(
            (rect.loc.x + CHILD_PAD, rect.loc.y).into(),
            (STATUS_WIDTH, first.size.h).into(),
        );
        icon_sdf(c, mark, scale, status, |x, y| {
            status_mark(card.status, x, y, s)
        });
        let text_x = px(rect.loc.x + CHILD_TEXT_X - CHILD_X) as f32;
        let right = px(rect.loc.x + rect.size.w - CHILD_PAD) as f32;
        let end = if card.deletable && hover > 0.0 {
            let close = card_close(rect, density);
            let color = if state.close {
                p.text
            } else {
                mix(surface, p.placeholder, hover)
            };
            icon_sdf(c, close, scale, color, |x, y| cross(x, y, 0.8 * s));
            px(close.loc.x) as f32
        } else if let Some(place) = place {
            let most = (rect.size.w as f32 * INBOX_PLACE * s).round();
            let w = self.text_width(place, DETAIL_FONT_SIZE, scale).min(most);
            let x = (right - w).round();
            self.text(
                c,
                place,
                DETAIL_FONT_SIZE,
                x,
                right + 1.0,
                line_band,
                scale,
                p.placeholder,
            );
            x - px(TITLE_GAP) as f32
        } else if blocked {
            let label = status_label(card.status);
            let pill_w =
                self.text_width(label, TINY_FONT_SIZE, scale) + 2.0 * PILL_PADDING as f32 * s;
            let pill_h = px(PILL_HEIGHT) as f32;
            let pill_x = (right - pill_w).round();
            let pill_y = (line_band.0 as f32 + (line_band.1 as f32 - pill_h) / 2.0).round();
            let pill_fill = mix(surface, p.warning, 0.25);
            c.rounded(
                pill_x,
                pill_y,
                pill_w.round(),
                pill_h,
                pill_h / 2.0,
                pill_fill,
            );
            let pill_band = (pill_y as i32, pill_h as i32);
            let x = pill_x + PILL_PADDING as f32 * s;
            self.text(
                c,
                label,
                TINY_FONT_SIZE,
                x,
                f32::MAX,
                pill_band,
                scale,
                p.warning,
            );
            pill_x - px(TITLE_GAP) as f32
        } else {
            right
        };
        let (heading_color, title_color) = match card.status {
            AgentStatus::Idle | AgentStatus::Unknown => (p.muted, p.placeholder),
            AgentStatus::Working => (p.text, p.muted),
            AgentStatus::Blocked | AgentStatus::Done => (p.text, p.text),
        };
        let after = self.text(
            c,
            card.heading(),
            SMALL_FONT_SIZE,
            text_x,
            end,
            line_band,
            scale,
            heading_color,
        );
        // What it is on, where a few characters of it fit.
        let x = after + px(TITLE_GAP) as f32;
        let some = self.text_width("abc…", SMALL_FONT_SIZE, scale);
        if let Some(title) = card.title().filter(|_| x + some <= end) {
            self.text(
                c,
                title,
                SMALL_FONT_SIZE,
                x,
                end,
                line_band,
                scale,
                title_color,
            );
        }
        if place.is_some() {
            return;
        }

        // A line per subagent that runs, after an elbow, what it is on, up
        // to `SUBAGENT_LINES`, then one counting the rest. One under the
        // pointer gets a pill, exactly its rect.
        let task_x = text_x + px(SUBAGENT_INDENT) as f32;
        for (j, sub) in card.subagents.iter().take(SUBAGENT_LINES).enumerate() {
            let line = card.subagent_line(rect, density, j);
            let lit = alpha(state.subagents[j]);
            if lit > 0.0 {
                let (lx, ly, lw, lh) = rect_px(line, scale);
                c.rounded(lx, ly, lw, lh, lh / 2.0, mix(base, p.text, 0.1 * lit));
            }
            let color = mix(p.muted, p.text, lit);
            let (ex, thin) = (text_x as i32 + px(2), px(1).max(1));
            let (top, mid) = (px(line.loc.y + 3), px(line.loc.y + CARD_LINE / 2));
            c.fill(ex, top, thin, mid - top + thin, color);
            c.fill(ex, mid, px(SUBAGENT_INDENT - 5), thin, color);
            self.text(
                c,
                &sub.task,
                DETAIL_FONT_SIZE,
                task_x,
                right,
                (px(line.loc.y), px(CARD_LINE)),
                scale,
                color,
            );
        }
        if let Some(rest) = card.subagents.len().checked_sub(SUBAGENT_LINES + 1) {
            let line = card.subagent_line(rect, density, SUBAGENT_LINES);
            self.text(
                c,
                &format!("+{} more", rest + 1),
                DETAIL_FONT_SIZE,
                task_x,
                right,
                (px(line.loc.y), px(CARD_LINE)),
                scale,
                p.muted,
            );
        }
    }
}

impl Painter {
    /// A GitHub section laid out as `layout` on `under`: GitHub's mark,
    /// then a line per group, its label and a chip per branch or pull
    /// request (its status mark and label), and the count of other pull
    /// requests on a quiet pill at the first line's right end. The section
    /// lightens under the pointer as an agent row does; a chip under it
    /// gets a pill, exactly its rect, and its count a stronger tint.
    #[allow(clippy::too_many_arguments)]
    fn ci_section(
        &self,
        c: &mut Canvas,
        view: &CiView,
        layout: &CiLayout,
        state: &CiCardState,
        under: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let rect = layout.card;
        let hover = alpha(state.hover);
        let fill = mix(under, p.element, hover);
        if hover > 0.0 {
            let (x, y, w, h) = rect_px(rect, scale);
            c.rounded(x, y, w, h, ROW_RADIUS * s, fill);
        }
        let mark = Rectangle::new(
            (rect.loc.x + CHILD_PAD, rect.loc.y + CI_PAD).into(),
            (STATUS_WIDTH, CARD_LINE).into(),
        );
        if let Some(icon) = github_icon() {
            c.picture_sized(&icon, mark, scale, p.muted, CI_ICON_SIZE);
        }
        if let Some(others) = layout.others {
            let lit = alpha(state.others);
            let (ox, oy, ow, oh) = rect_px(others, scale);
            c.rounded(
                ox,
                oy,
                ow,
                oh,
                oh / 2.0,
                mix(fill, p.muted, CI_COUNT_TINT * (1.0 + lit)),
            );
            self.text(
                c,
                &prs_label(view.other_prs),
                TINY_FONT_SIZE,
                ox + PILL_PADDING as f32 * s,
                ox + ow,
                (oy as i32, oh as i32),
                scale,
                mix(p.muted, p.text, lit),
            );
        }

        // The lines' labels, in the band of their chips.
        for (group, rect) in &layout.labels {
            self.text(
                c,
                group_label(*group),
                DETAIL_FONT_SIZE,
                px(rect.loc.x) as f32,
                px(rect.loc.x + rect.size.w) as f32 + 1.0,
                band(*rect, scale),
                scale,
                p.placeholder,
            );
        }

        // The chips: a mark and a label each, cut to fit.
        for (k, (chip, rect)) in view.chips.iter().zip(&layout.chips).enumerate() {
            let lit = alpha(state.chip(k));
            if lit > 0.0 {
                let (cx, cy, cw, ch) = rect_px(*rect, scale);
                c.rounded(
                    cx,
                    cy,
                    cw,
                    ch,
                    CI_PILL_RADIUS * s,
                    mix(fill, p.text, 0.1 * lit),
                );
            }
            let mark = Rectangle::new(
                (rect.loc.x + CI_CHIP_PAD, rect.loc.y).into(),
                (STATUS_WIDTH, rect.size.h).into(),
            );
            icon_sdf(c, mark, scale, ci_color(chip.state, p), |x, y| {
                ci_mark(chip.state, x, y, s)
            });
            // A pixel past the padding for rounding: a label measured to
            // fit is never cut.
            let end = px(rect.loc.x + rect.size.w - CI_CHIP_PAD) as f32 + 1.0;
            self.text(
                c,
                &chip.label,
                DETAIL_FONT_SIZE,
                px(mark.loc.x + STATUS_WIDTH + CI_MARK_GAP) as f32,
                end,
                band(*rect, scale),
                scale,
                mix(p.muted, p.text, lit),
            );
        }
    }

    /// The band composed over a blocked agent's row as its pulse: the
    /// warning colour, `size` big, rounded as the row. Its opacity is
    /// given when it is composed (`pulse_alpha`).
    pub(super) fn pulse_band(
        &self,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let r = ROW_RADIUS * scale as f32;
        let (w, h) = (c.w as f32, c.h as f32);
        c.rounded(0.0, 0.0, w, h, r, p.warning);
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
        AGENT_ROW, AGENT_ROW_COMPACT, ATTENTION_BOTTOM, AgentLine, AgentView, BLOCK_GAP,
        BRANCH_LINE, BRANCH_LINE_COMPACT, CARD_LINE, CHILD_GAP, CHILD_TEXT_X, CHILD_X, CI_CHIP_GAP,
        CI_LABEL_GAP, CI_PAD, CLOSE_SLOT, CiChip, CiLayout, CiView, DETAIL_FONT_SIZE, Density,
        FOLDER, GAP, HEADER_BUTTON, HEADER_ROW, HEADER_ROW_COMPACT, INBOX_GAP, INBOX_TITLE,
        NAME_LINE, NAME_LINE_COMPACT, NAME_X, ROW_INSET, ROWS_TOP, SCROLL_END, SidebarHit,
        SidebarLayout, TAB_LINE, WorkspaceView, card_order, ci_chip_width, inbox_order,
        sentence_case, sidebar_layout, sidebar_layout_at, text_extent, workspace_slot,
    };
    use crate::WorkspaceKind;
    use crate::chrome::{BAR_HEIGHT, TabView};
    use crate::ci::{CiState, Group};
    use crate::git::{GitStatus, Worktree};
    use crate::herdr::AgentStatus;
    use smithay::utils::Rectangle;

    /// A header with a branch: its name's band and its branch's line.
    const BRANCHED: i32 = NAME_LINE + BRANCH_LINE;
    const BRANCHED_COMPACT: i32 = NAME_LINE_COMPACT + BRANCH_LINE_COMPACT;

    #[test]
    fn workspace_puts_its_tabs_together() {
        let tab = |status, branch: &str, place: &str, activity: &str| TabView {
            title: String::new(),
            status,
            branch: Some(branch.into()),
            place: place.into(),
            activity: activity.into(),
            restart: None,
            github: false,
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
            more_agents: Vec::new(),
            ci: None,
            folded: false,
            restart: false,
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
            deletable: false,
            subagents: Vec::new(),
            inbox: false,
        }
    }

    fn mid(r: crate::chrome::Rect) -> smithay::utils::Point<f64, smithay::utils::Logical> {
        (
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        )
            .into()
    }

    #[test]
    fn rows_scroll_under_the_header() {
        let workspaces: Vec<_> = (0..10)
            .map(|i| workspace(&format!("w{i}"), None, Some("main")))
            .collect();
        let layout = sidebar_layout(&workspaces, &[], 240);
        let end = layout.blocks.last().map(|b| b.loc.y + b.size.h).unwrap();
        let height = 200;
        let max = layout.max_scroll(height);
        assert_eq!(max, end + SCROLL_END - height);
        assert_eq!(sidebar_layout(&workspaces, &[], 240).max_scroll(end * 2), 0);

        // Held to what it can scroll, the header staying put.
        let scrolled = sidebar_layout(&workspaces, &[], 240).scrolled(max + 100, height);
        assert_eq!(scrolled.scroll, max);
        assert_eq!(scrolled.max_scroll(height), max);
        assert_eq!(scrolled.header, layout.header);
        assert_eq!(scrolled.rows[3].rect.loc.y, layout.rows[3].rect.loc.y - max);
        assert_eq!(scrolled.blocks[3].loc.y, layout.blocks[3].loc.y - max);
        let back = scrolled.scrolled(10, height);
        assert_eq!(back.rows[0].rect.loc.y, layout.rows[0].rect.loc.y - 10);

        // A row scrolled under the header is not hit there.
        let under = sidebar_layout(&workspaces, &[], 240).scrolled(ROWS_TOP, height);
        assert!(under.rows[0].rect.loc.y < BAR_HEIGHT);
        let pos = (100.0, (BAR_HEIGHT - 1) as f64).into();
        assert_eq!(under.hit(pos), None);
        let name = layout.rows[0].name;
        assert_eq!(
            layout.hit((name.loc.x as f64 + 2.0, ROWS_TOP as f64 + 1.0).into()),
            Some(SidebarHit::Name(0))
        );
    }

    #[test]
    fn scrolling_shows_a_workspace() {
        let workspaces: Vec<_> = (0..10)
            .map(|i| workspace(&format!("w{i}"), None, Some("main")))
            .collect();
        let height = 200;
        let layout = sidebar_layout(&workspaces, &[], 240);
        // Already in view: no move.
        assert_eq!(layout.showing(0, 0, height), 0);
        // Below: scrolled until its bottom fits; above: until its top does.
        let last = layout.blocks[9];
        let to = layout.showing(9, 0, height);
        assert_eq!(to, layout.max_scroll(height));
        assert_eq!(last.loc.y + last.size.h + SCROLL_END - to, height);
        let scrolled = sidebar_layout(&workspaces, &[], 240).scrolled(to, height);
        assert_eq!(
            scrolled.showing(2, to, height),
            layout.blocks[2].loc.y - ROWS_TOP
        );
    }

    /// The sidebar with one workspace of three agents, their rows open or
    /// not by `open`, painted as bytes.
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
                cards: vec![
                    open.map(|open| CardState {
                        open,
                        ..CardState::default()
                    })
                    .to_vec(),
                ],
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
    fn the_open_agents_row_stands_out() {
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

    #[test]
    fn worktrees_show_as_a_count_on_the_header() {
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
        assert_eq!(ws.height(Density::Comfortable), BRANCHED);
        // Those needing a look get a line each, up to a count of the rest.
        let worktrees = &mut ws.git.as_mut().unwrap().worktrees;
        worktrees[0].dirty = true;
        worktrees[1].merged = false;
        let lines = |n: i32| BRANCHED + n * TAB_LINE + ATTENTION_BOTTOM;
        assert_eq!(ws.height(Density::Comfortable), lines(2));
        for w in &mut ws.git.as_mut().unwrap().worktrees {
            w.merged = false;
        }
        assert_eq!(ws.height(Density::Comfortable), lines(4));
        assert_eq!(ws.height(Density::Compact), BRANCHED_COMPACT);
        let layout = sidebar_layout_at(&[ws.clone()], &[], 240, Density::Comfortable);
        let row = layout.rows[0];
        // On the branch's line, at its right end.
        let count = row.worktrees.expect("a count");
        let branch = row.branch.expect("a branch");
        assert_eq!(count.loc.y, ROWS_TOP + NAME_LINE);
        assert_eq!(count.loc.y, branch.loc.y);
        assert!(branch.loc.x + branch.size.w < count.loc.x);
        assert_eq!(count.loc.x + count.size.w, row.close.loc.x + CLOSE_SLOT);
        assert_eq!(layout.hit(mid(count)), Some(SidebarHit::Worktrees(0)));
        assert_eq!(SidebarHit::Worktrees(0).pill(), SidebarHit::Row(0));
        let lines = row.attention.expect("worktree lines");
        assert_eq!(lines.loc.y, ROWS_TOP + BRANCHED);
        assert_eq!(layout.hit(mid(lines)), Some(SidebarHit::Attention(0, 2)));
        // Compact headers count them, and have no lines.
        let compact = sidebar_layout_at(&[ws.clone()], &[], 240, Density::Compact);
        assert!(compact.rows[0].worktrees.is_some());
        assert_eq!(compact.rows[0].attention, None);
        // Folded: the lines go, the count stays.
        ws.folded = true;
        assert_eq!(ws.height(Density::Comfortable), BRANCHED);
        let folded = sidebar_layout(&[ws], &[], 240);
        assert!(folded.rows[0].worktrees.is_some());
        assert_eq!(folded.rows[0].attention, None);
    }

    #[test]
    fn the_label_names_the_model_and_the_row_its_name() {
        let mut card = agent("p1", AgentStatus::Working, None);
        assert_eq!(card.label(), "claude");
        assert_eq!(card.heading(), "claude");
        card.model = Some("Opus 5.5".into());
        assert_eq!(card.label(), "claude · Opus 5.5");
        card.name = Some("reviewer".into());
        assert_eq!(card.label(), "claude · reviewer · Opus 5.5");
        assert_eq!(card.heading(), "reviewer");
    }

    #[test]
    fn a_row_shows_five_subagents_then_counts_the_rest() {
        use super::{SUBAGENT_LINES, SubagentView};
        use smithay::utils::Point;
        let mut card = agent("1", AgentStatus::Working, Some("~/src/gerdr"));
        card.subagents = (0..7)
            .map(|n| SubagentView {
                // The third's chat isn't known yet.
                key: (n != 2).then(|| super::AgentKey::Cursor(format!("s{n}"))),
                task: format!("Task {n}"),
            })
            .collect();
        assert_eq!(card.subagent_lines(), SUBAGENT_LINES + 1);
        assert_eq!(card.height(Density::Comfortable), AGENT_ROW + 6 * CARD_LINE);
        let ws = workspace("gerdr", Some(AgentStatus::Working), Some("main"));
        let agents = vec![vec![card.clone()]];
        let layout = sidebar_layout_at(&[ws], &agents, 240, Density::Comfortable);
        // A line each but the unknown one's and the count's.
        let lines: Vec<_> = layout.subagents[0].iter().map(|l| (l.0, l.1)).collect();
        assert_eq!(lines, [(0, 0), (0, 1), (0, 3), (0, 4)]);
        let row = layout.cards[0][0].1;
        let at = |r: super::Rect| Point::from((r.loc.x as f64 + 4.0, r.loc.y as f64 + 4.0));
        let first = card.subagent_line(row, Density::Comfortable, 0);
        assert_eq!(first.loc.y, row.loc.y + AGENT_ROW);
        assert_eq!(layout.hit(at(first)), Some(SidebarHit::Subagent(0, 0, 0)));
        // The unknown one's line and the count are the row's.
        for j in [2, SUBAGENT_LINES] {
            let line = card.subagent_line(row, Density::Comfortable, j);
            assert_eq!(layout.hit(at(line)), Some(SidebarHit::Card(0, 0)));
        }
    }

    #[test]
    fn a_cursor_row_shows_its_subagents() {
        use crate::cursor_agents::{ChatRow, CursorChat, SubagentRun};
        let mut chat = CursorChat {
            row: ChatRow {
                id: "c1".into(),
                subtitle: Some("Read main.rs".into()),
                paths: vec!["/src/gerdr".into()],
                ..ChatRow::default()
            },
            status: AgentStatus::Working,
            model: None,
            subagents: vec![
                SubagentRun {
                    chat: Some("s1".into()),
                    task: Some("Audit the plan".into()),
                },
                SubagentRun::default(),
            ],
        };
        let card = AgentView::cursor(&chat);
        assert_eq!(card.title(), Some("Read main.rs"));
        let tasks: Vec<_> = card.subagents.iter().map(|s| s.task.as_str()).collect();
        assert_eq!(tasks, ["Audit the plan", "Working"]);
        assert_eq!(
            card.subagents[0].key,
            Some(super::AgentKey::Cursor("s1".into()))
        );
        assert_eq!(card.subagents[1].key, None);
        let fields = card.tooltip_fields();
        assert!(fields.contains(&("Subagent".into(), "Audit the plan".into())));
        assert!(fields.contains(&("Subagent".into(), "Working".into())));
        assert_eq!(card.height(Density::Comfortable), AGENT_ROW + 2 * CARD_LINE);
        chat.subagents.clear();
        let card = AgentView::cursor(&chat);
        assert_eq!(card.subagent_lines(), 0);
        assert_eq!(card.height(Density::Comfortable), AGENT_ROW);
    }

    #[test]
    fn headers_are_a_line_and_one_for_the_branch() {
        let with = workspace("gerdr", Some(AgentStatus::Working), Some("main"));
        let plain = workspace("~", None, None);
        assert_eq!(plain.height(Density::Comfortable), HEADER_ROW);
        assert_eq!(plain.height(Density::Compact), HEADER_ROW_COMPACT);
        assert_eq!(with.height(Density::Comfortable), BRANCHED);
        assert_eq!(with.height(Density::Compact), BRANCHED_COMPACT);
        let with = agent("p1", AgentStatus::Idle, Some("gerdr/src"));
        assert_eq!(with.height(Density::Comfortable), AGENT_ROW);
        assert_eq!(with.height(Density::Compact), AGENT_ROW_COMPACT);
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
    fn the_branch_has_a_line_under_the_name() {
        let layout = sidebar_layout(&[workspace("gerdr", None, Some("main"))], &[], 200);
        let row = layout.rows[0];
        let branch = row.branch.expect("a line for the branch");
        assert_eq!((row.name.loc.x, branch.loc.x), (NAME_X, NAME_X));
        assert_eq!(row.title.size.h, NAME_LINE);
        assert_eq!(branch.loc.y, row.name.loc.y + NAME_LINE);
        assert_eq!(branch.size.h, BRANCH_LINE);
        // A long name keeps the width, under the "×" too: that shows
        // only under the pointer.
        let long = "a-workspace-with-a-very-long-name-indeed";
        let layout = sidebar_layout(&[workspace(long, None, Some("main"))], &[], 200);
        let row = layout.rows[0];
        assert!(row.branch.is_some());
        assert_eq!(
            row.name.loc.x + row.name.size.w,
            row.close.loc.x + CLOSE_SLOT - 4
        );
        assert_eq!(layout.hit(mid(row.close)), Some(SidebarHit::Close(0)));
        // A status right of it bounds it.
        let busy = workspace(long, Some(AgentStatus::Idle), Some("main"));
        let agents = vec![vec![agent("p1", AgentStatus::Blocked, None)]];
        let layout = sidebar_layout(&[busy], &agents, 200);
        let row = layout.rows[0];
        let chip = row.chip.expect("a blocked badge");
        assert_eq!(row.name.loc.x + row.name.size.w, chip.loc.x - 4);
        // Without a branch, one line.
        let layout = sidebar_layout(&[workspace("~", None, None)], &[], 200);
        assert_eq!(layout.rows[0].branch, None);
        assert_eq!(layout.rows[0].rect.size.h, HEADER_ROW);
        // On the header, the branch is the row's.
        let layout = sidebar_layout(&[workspace("gerdr", None, Some("main"))], &[], 240);
        let branch = layout.rows[0].branch.unwrap();
        assert_eq!(layout.hit(mid(branch)), Some(SidebarHit::Row(0)));
    }

    #[test]
    fn a_workspace_with_commands_has_a_restart_button_left_of_its_close() {
        let mut ws = workspace("gerdr", None, Some("main"));
        let layout = sidebar_layout(std::slice::from_ref(&ws), &[], 200);
        assert_eq!(layout.rows[0].restart, None);
        ws.restart = true;
        let layout = sidebar_layout(&[ws.clone()], &[], 200);
        let row = layout.rows[0];
        let restart = row.restart.expect("a restart button");
        assert_eq!(restart.loc.x + restart.size.w, row.close.loc.x);
        assert_eq!(restart.loc.y, row.close.loc.y);
        assert_eq!(row.buttons_x(), restart.loc.x);
        assert_eq!(layout.hit(mid(restart)), Some(SidebarHit::Restart(0)));
        assert_eq!(SidebarHit::Restart(0).pill(), SidebarHit::Row(0));
        // A name may still reach under both, which show only under the
        // pointer; a status right of them bounds it at the button.
        assert_eq!(
            row.name.loc.x + row.name.size.w,
            row.close.loc.x + CLOSE_SLOT - 4
        );
        ws.status = Some(AgentStatus::Idle);
        let agents = vec![vec![agent("p1", AgentStatus::Blocked, None)]];
        let layout = sidebar_layout(&[ws], &agents, 200);
        let row = layout.rows[0];
        let chip = row.chip.expect("a blocked badge");
        assert_eq!(chip.loc.x + chip.size.w, row.restart.unwrap().loc.x);
        // Scrolled, it moves with its header.
        let mut many = vec![workspace("x", None, None); 40];
        many[1].restart = true;
        let row = sidebar_layout(&many, &[], 200).scrolled(50, 300).rows[1];
        assert_eq!(row.restart.unwrap().loc.y, row.close.loc.y);
    }

    #[test]
    fn layout_stacks_headers_and_rows_in_both_densities() {
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
            assert_eq!(layout.inbox_title, None);

            let row0 = layout.rows[0];
            let h0 = workspaces[0].height(density);
            assert_eq!(row0.rect.loc.y, ROWS_TOP);
            assert_eq!(row0.rect.size, (200 - GAP, h0).into());
            assert_eq!(row0.close.size.w, CLOSE_SLOT);
            assert_eq!(row0.close.loc.x, 200 - GAP - ROW_INSET - CLOSE_SLOT);
            // A blocked agent shows its badge; nothing else does unfolded.
            let chip = row0.chip.expect("a badge for a blocked agent");
            assert_eq!(chip.loc.x + chip.size.w, row0.close.loc.x);
            assert!(row0.name.loc.x + row0.name.size.w <= chip.loc.x);
            assert!(layout.rows[1].chip.is_none());
            assert!(row0.fold.is_some());
            assert!(layout.rows[1].fold.is_none());

            // Rows: the blocked one first, each as high as it says, under
            // the header's name, with a gap between and after them.
            let cards = &layout.cards[0];
            assert_eq!(cards.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [1, 0]);
            assert_eq!(cards[0].1.loc, (CHILD_X, ROWS_TOP + h0).into());
            assert_eq!(cards[0].1.size.h, agents[0][1].height(density));
            assert_eq!(
                cards[1].1.loc.y,
                cards[0].1.loc.y + cards[0].1.size.h + CHILD_GAP
            );
            let gap = super::block_gap(density);
            let block_end = cards[1].1.loc.y + cards[1].1.size.h + 2 * CHILD_GAP + gap;
            assert_eq!(layout.blocks[0].size.h, block_end - ROWS_TOP);
            assert_eq!(layout.rows[1].rect.loc.y, block_end);
            assert!(layout.cards[1].is_empty());
            assert_eq!(layout.blocks[1].size.h, workspaces[1].height(density) + gap);
        }
        let comfortable = sidebar_layout(&workspaces, &agents, 200);
        let compact = sidebar_layout_at(&workspaces, &agents, 200, Density::Compact);
        assert!(compact.blocks[1].loc.y < comfortable.blocks[1].loc.y);
    }

    #[test]
    fn a_folded_workspace_is_its_header() {
        let mut ws = workspace("gerdr", Some(AgentStatus::Idle), Some("main"));
        ws.more_agents = vec!["cursor ask · A".into()];
        ws.ci = Some(ci_view(&["main", "#42"], 1));
        let agents = vec![vec![
            agent("p1", AgentStatus::Idle, None),
            agent("p2", AgentStatus::Idle, None),
        ]];
        let open = sidebar_layout(&[ws.clone()], &agents, 240);
        assert_eq!(open.cards[0].len(), 2);
        assert!(open.more[0].is_some() && open.ci[0].is_some());
        assert_eq!(open.rows[0].chip, None);
        ws.folded = true;
        let layout = sidebar_layout(&[ws], &agents, 240);
        assert!(layout.cards[0].is_empty());
        assert_eq!((layout.more[0], layout.ci[0].clone()), (None, None));
        assert_eq!(layout.blocks[0].size.h, BRANCHED + BLOCK_GAP);
        // Its status shows, with how many agents it hides, and its CI as
        // the mark.
        let row = layout.rows[0];
        let chip = row.chip.expect("the status of a folded workspace");
        let count = text_extent("2", crate::chrome::TINY_FONT_SIZE);
        assert_eq!(chip.size.w, super::STATUS_SLOT + super::COUNT_GAP + count);
        assert!(row.ci.is_some());
        let fold = row.fold.expect("a chevron");
        assert_eq!(layout.hit(mid(fold)), Some(SidebarHit::Fold(0)));
        assert_eq!(SidebarHit::Fold(0).pill(), SidebarHit::Row(0));
        // With nothing under it, there is nothing to fold.
        let mut bare = workspace("~", Some(AgentStatus::Idle), None);
        bare.folded = true;
        let layout = sidebar_layout(&[bare], &[], 240);
        assert_eq!(layout.rows[0].fold, None);
    }

    #[test]
    fn quiet_workspaces_have_nothing_going_on() {
        let mut ws = workspace("gerdr", None, None);
        assert!(ws.is_quiet());
        ws.status = Some(AgentStatus::Idle);
        assert!(ws.is_quiet());
        ws.status = Some(AgentStatus::Working);
        assert!(!ws.is_quiet());
        ws.status = Some(AgentStatus::Idle);
        ws.ci = Some(passing(&["main", "dev"]));
        assert!(ws.is_quiet());
        ws.ci = Some(ci_view(&["main", "#42"], 0));
        assert!(!ws.is_quiet());
    }

    #[test]
    fn a_green_ci_is_a_mark_on_the_header() {
        let mut ws = workspace("gerdr", None, Some("main"));
        ws.ci = Some(passing(&["main", "dev"]));
        assert_eq!(ws.ci.as_ref().unwrap().rollup(), CiState::Passed);
        let layout = sidebar_layout(&[ws.clone()], &[], 240);
        assert_eq!(layout.ci[0], None);
        let mark = layout.rows[0].ci.expect("a CI mark");
        assert_eq!(layout.hit(mid(mark)), Some(SidebarHit::CiMark(0)));
        // At the right end of the branch's line.
        let row = layout.rows[0];
        assert_eq!(mark.loc.y, row.branch.unwrap().loc.y);
        assert_eq!(mark.loc.x + mark.size.w, row.close.loc.x + CLOSE_SLOT);
        assert_eq!(layout.blocks[0].size.h, BRANCHED + BLOCK_GAP);
        // A review waiting makes it a section, and the mark goes.
        ws.ci.as_mut().unwrap().chips[1].group = Group::Review;
        let layout = sidebar_layout(&[ws.clone()], &[], 240);
        assert!(layout.ci[0].is_some());
        assert_eq!(layout.rows[0].ci, None);
        // So does a run failing; the mark says so while folded.
        let mut ci = passing(&["main", "dev"]);
        ci.chips[0].state = CiState::Failed;
        assert_eq!(ci.rollup(), CiState::Failed);
        ws.ci = Some(ci);
        assert!(sidebar_layout(&[ws.clone()], &[], 240).ci[0].is_some());
        ws.folded = true;
        let layout = sidebar_layout(&[ws], &[], 240);
        assert_eq!(layout.ci[0], None);
        assert!(layout.rows[0].ci.is_some());
    }

    #[test]
    fn needs_you_lists_the_marked_agents_first() {
        let workspaces = vec![
            workspace("gerdr", Some(AgentStatus::Working), None),
            workspace("aft", Some(AgentStatus::Blocked), None),
        ];
        let mut done = agent("p1", AgentStatus::Done, None);
        done.inbox = true;
        let mut blocked = agent("p3", AgentStatus::Blocked, None);
        blocked.inbox = true;
        let agents = vec![
            vec![agent("p0", AgentStatus::Working, None), done],
            vec![agent("p2", AgentStatus::Idle, None), blocked],
        ];
        assert_eq!(inbox_order(&agents), [(1, 1), (0, 1)]);
        let layout = sidebar_layout(&workspaces, &agents, 240);
        let title = layout.inbox_title.expect("a heading");
        assert_eq!(title.loc.y, ROWS_TOP);
        let rows: Vec<_> = layout.inbox.iter().map(|(i, k, _)| (*i, *k)).collect();
        assert_eq!(rows, [(1, 1), (0, 1)]);
        let (_, _, first) = layout.inbox[0];
        assert_eq!(first.loc, (CHILD_X, ROWS_TOP + INBOX_TITLE).into());
        assert_eq!(first.size.h, AGENT_ROW);
        assert_eq!(layout.hit(mid(first)), Some(SidebarHit::Inbox(1, 1)));
        assert_eq!(SidebarHit::Inbox(1, 1).pill(), SidebarHit::Inbox(1, 1));
        let (_, _, last) = layout.inbox[1];
        let end = last.loc.y + last.size.h + CHILD_GAP + INBOX_GAP;
        assert_eq!(layout.rows[0].rect.loc.y, end);
        // They stay in their workspaces too.
        assert_eq!(layout.cards[1].len(), 2);
    }

    #[test]
    fn hits_name_every_part() {
        let workspaces = vec![
            workspace("gerdr", Some(AgentStatus::Working), Some("main")),
            workspace("~", None, None),
        ];
        let agents = vec![vec![agent("p1", AgentStatus::Idle, None)], vec![]];
        let layout = sidebar_layout(&workspaces, &agents, 200);
        assert_eq!(layout.hit(mid(layout.plus)), Some(SidebarHit::Plus));
        assert_eq!(layout.hit(mid(layout.repos)), Some(SidebarHit::Repos));
        assert_eq!(layout.hit(mid(layout.header)), Some(SidebarHit::Header));
        let row = layout.rows[0];
        assert_eq!(layout.hit(mid(row.name)), Some(SidebarHit::Name(0)));
        assert_eq!(layout.hit(mid(row.close)), Some(SidebarHit::Close(0)));
        assert_eq!(
            layout.hit(mid(layout.cards[0][0].1)),
            Some(SidebarHit::Card(0, 0))
        );
        assert_eq!(
            layout.hit(mid(layout.rows[1].name)),
            Some(SidebarHit::Name(1))
        );
        assert_eq!(layout.hit((100.0, 10_000.0).into()), None);
        assert_eq!(SidebarHit::Close(3).pill(), SidebarHit::Row(3));
        assert_eq!(SidebarHit::Card(1, 2).pill(), SidebarHit::Card(1, 2));
        // A blocked agent's badge is the chip.
        let blocked = vec![vec![agent("p1", AgentStatus::Blocked, None)], vec![]];
        let layout = sidebar_layout(&workspaces, &blocked, 200);
        let chip = layout.rows[0].chip.unwrap();
        assert_eq!(layout.hit(mid(chip)), Some(SidebarHit::Chip(0)));
    }

    #[test]
    fn history_rows_have_a_close_and_the_rest_a_count() {
        let mut gerdr = workspace("gerdr", None, None);
        gerdr.more_agents = vec!["cursor plan · A".into(), "cursor ask · B".into()];
        let workspaces = vec![gerdr, workspace("~", None, None)];
        let mut history = agent("p2", AgentStatus::Idle, None);
        history.deletable = true;
        let agents = vec![vec![agent("p1", AgentStatus::Working, None), history]];
        let layout = sidebar_layout(&workspaces, &agents, 200);
        // Only the deletable row has a "×", at its right end.
        assert_eq!(layout.card_closes[0].len(), 1);
        let (k, close) = layout.card_closes[0][0];
        let card = layout.cards[0].iter().find(|(c, _)| *c == k).unwrap().1;
        assert_eq!(k, 1);
        assert!(card.contains(close.loc) && close.loc.x > card.loc.x + card.size.w / 2);
        assert_eq!(layout.hit(mid(close)), Some(SidebarHit::CardClose(0, 1)));
        assert_eq!(SidebarHit::CardClose(0, 1).pill(), SidebarHit::Card(0, 1));
        // The count goes after the rows, before the next header.
        let more = layout.more[0].unwrap();
        assert!(more.loc.y >= card.loc.y + card.size.h);
        assert!(more.loc.y + more.size.h <= layout.rows[1].rect.loc.y);
        assert_eq!(layout.hit(mid(more)), Some(SidebarHit::More(0)));
        assert_eq!(layout.more[1], None);
    }

    #[test]
    fn renaming_edits_in_the_name_band() {
        let workspaces = vec![workspace("gerdr", Some(AgentStatus::Blocked), Some("main"))];
        let agents = vec![vec![agent("p1", AgentStatus::Blocked, None)]];
        let layout = sidebar_layout(&workspaces, &agents, 200);
        let row = layout.rows[0];
        let field = row.rename_field();
        assert!(field.loc.x < row.name.loc.x);
        assert!(field.loc.y >= row.title.loc.y);
        assert!(field.loc.y + field.size.h <= row.title.loc.y + row.title.size.h);
        assert!(field.loc.x + field.size.w < row.chip.unwrap().loc.x);
    }

    #[test]
    fn rows_go_by_urgency_then_as_given() {
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

    /// GitHub chips of branches labelled `labels`, their states going
    /// passed, failed, running, unknown, with `others` other pull requests.
    fn ci_view(labels: &[&str], others: usize) -> CiView {
        let chips: Vec<_> = labels.iter().map(|l| (Group::Branch, *l)).collect();
        grouped_ci_view(&chips, others)
    }

    /// GitHub chips of branches labelled `labels`, all passed.
    fn passing(labels: &[&str]) -> CiView {
        let mut ci = ci_view(labels, 0);
        ci.chips.iter_mut().for_each(|c| c.state = CiState::Passed);
        ci
    }

    /// GitHub chips of `chips`, each its group and label, with `others`
    /// other pull requests: one failed, so they show as a section.
    fn grouped_ci_view(chips: &[(Group, &str)], others: usize) -> CiView {
        let states = [
            CiState::Failed,
            CiState::Passed,
            CiState::Running,
            CiState::Unknown,
        ];
        CiView {
            chips: chips
                .iter()
                .zip(states.into_iter().cycle())
                .map(|((group, label), state)| CiChip {
                    label: (*label).into(),
                    state,
                    title: None,
                    watched: false,
                    checks: Vec::new(),
                    group: *group,
                })
                .collect(),
            other_prs: others,
        }
    }

    #[test]
    fn the_ci_section_follows_the_agent_rows_and_grows_the_block() {
        let mut gerdr = workspace("gerdr", Some(AgentStatus::Working), Some("main"));
        gerdr.more_agents = vec!["cursor ask · A".into()];
        let agents = vec![vec![agent("p1", AgentStatus::Working, None)], vec![]];
        let plain = vec![gerdr.clone(), workspace("~", None, None)];
        gerdr.ci = Some(ci_view(&["main", "#42"], 2));
        let workspaces = vec![gerdr, workspace("~", None, None)];
        for density in [Density::Comfortable, Density::Compact] {
            let without = sidebar_layout_at(&plain, &agents, 240, density);
            let layout = sidebar_layout_at(&workspaces, &agents, 240, density);
            assert_eq!(without.ci, [None, None]);
            let section = layout.ci[0].as_ref().expect("a GitHub section").card;
            // After the "+N more" line, as wide as the agent rows.
            let more = layout.more[0].unwrap();
            let row = layout.cards[0][0].1;
            assert_eq!(section.loc.y, more.loc.y + more.size.h + CHILD_GAP);
            assert_eq!((section.loc.x, section.size.w), (row.loc.x, row.size.w));
            // A line of chips; the block takes it and the gap after it,
            // and the next header moves down by as much.
            assert_eq!(section.size.h, 2 * CI_PAD + CARD_LINE);
            let end = section.loc.y + section.size.h + 2 * CHILD_GAP + super::block_gap(density);
            assert_eq!(layout.blocks[0].size.h, end - ROWS_TOP);
            assert_eq!(layout.rows[1].rect.loc.y, end);
            assert_eq!(
                layout.rows[1].rect.loc.y - without.rows[1].rect.loc.y,
                section.size.h + CHILD_GAP
            );
            assert_eq!(layout.ci[1], None);
        }
        // Nothing to show: no section, no mark.
        let mut alone = workspace("gerdr", None, None);
        alone.ci = Some(CiView::default());
        let layout = sidebar_layout(&[alone.clone()], &[], 240);
        assert_eq!(layout.ci[0], None);
        assert_eq!(layout.rows[0].ci, None);
        assert_eq!(
            layout.blocks[0].size.h,
            alone.height(Density::Comfortable) + BLOCK_GAP
        );
    }

    #[test]
    fn ci_chips_flow_and_wrap_inside_the_section() {
        let mut ws = workspace("gerdr", None, None);
        let labels = [
            "main", "dev", "#101", "#102", "#103", "#104", "#105", "#106",
        ];
        ws.ci = Some(ci_view(&labels, 0));
        let layout = sidebar_layout(&[ws], &[], 200);
        let ci = layout.ci[0].as_ref().unwrap();
        assert_eq!(ci.others, None);
        assert_eq!(ci.chips.len(), labels.len());
        let (card, chips) = (ci.card, &ci.chips);
        let first_line = card.loc.y + CI_PAD;
        assert_eq!(chips[0].loc.y, first_line);
        // One label, "CI", on the first line, where a row's text starts;
        // the chips in a column after it.
        let label_w = text_extent("CI", DETAIL_FONT_SIZE);
        let label_x = card.loc.x + CHILD_TEXT_X - CHILD_X;
        let label = Rectangle::new((label_x, first_line).into(), (label_w, CARD_LINE).into());
        assert_eq!(ci.labels, [(Group::Branch, label)]);
        assert_eq!(chips[0].loc.x, label_x + label_w + CI_LABEL_GAP - 4);
        let mut lines = 1;
        for (pair, label) in chips.windows(2).zip(&labels[1..]) {
            let (a, b) = (pair[0], pair[1]);
            assert_eq!(b.size, (ci_chip_width(label), CARD_LINE).into());
            if b.loc.y == a.loc.y {
                // Straight after the last on its line.
                assert_eq!(b.loc.x, a.loc.x + a.size.w + CI_CHIP_GAP);
            } else {
                // A line down, in the column, as the next did not fit.
                lines += 1;
                assert_eq!(b.loc.y, a.loc.y + CARD_LINE);
                assert_eq!(b.loc.x, chips[0].loc.x);
                assert!(a.loc.x + a.size.w + CI_CHIP_GAP + b.size.w > card.loc.x + card.size.w);
            }
        }
        assert!(lines > 1, "eight chips take more than a line at 200 px");
        assert_eq!(card.size.h, 2 * CI_PAD + lines * CARD_LINE);
        for chip in chips {
            assert!(crate::chrome::inside(card, *chip));
        }

        // A label longer than the room after the column is cut to it.
        let mut long = workspace("gerdr", None, None);
        long.ci = Some(ci_view(&["a-branch-with-a-name-far-too-long-for-it"], 0));
        let layout = sidebar_layout(&[long], &[], 200);
        let ci = layout.ci[0].as_ref().unwrap();
        let card_end = ci.card.loc.x + ci.card.size.w - super::CHILD_PAD + 4;
        assert_eq!(ci.chips[0].loc.x + ci.chips[0].size.w, card_end);
        assert_eq!(ci.card.size.h, 2 * CI_PAD + CARD_LINE);
    }

    #[test]
    fn ci_hits_name_the_chip_the_count_and_the_section() {
        let mut ws = workspace("gerdr", Some(AgentStatus::Idle), Some("main"));
        ws.ci = Some(ci_view(&["main", "#42"], 4));
        let workspaces = vec![workspace("~", None, None), ws];
        let agents = vec![vec![], vec![agent("p1", AgentStatus::Idle, None)]];
        let layout = sidebar_layout(&workspaces, &agents, 240);
        let ci = layout.ci[1].as_ref().unwrap();
        let others = ci.others.expect("a count of other pull requests");
        // The count at the first line's right end, after its chips.
        assert!(others.loc.x + others.size.w <= ci.card.loc.x + ci.card.size.w);
        assert!(ci.chips[1].loc.x + ci.chips[1].size.w < others.loc.x);
        assert_eq!(layout.hit(mid(others)), Some(SidebarHit::CiPrs(1)));
        assert_eq!(layout.hit(mid(ci.chips[0])), Some(SidebarHit::CiChip(1, 0)));
        assert_eq!(layout.hit(mid(ci.chips[1])), Some(SidebarHit::CiChip(1, 1)));
        // Between the chips, and GitHub's mark, are the section.
        let between = (
            (ci.chips[0].loc.x + ci.chips[0].size.w + 1) as f64,
            mid(ci.chips[0]).y,
        );
        assert_eq!(layout.hit(between.into()), Some(SidebarHit::Ci(1)));
        let mark = (ci.card.loc.x as f64 + 8.0, ci.card.loc.y as f64 + 6.0);
        assert_eq!(layout.hit(mark.into()), Some(SidebarHit::Ci(1)));
        // So is a line's label.
        let (_, label) = ci.labels[0];
        assert_eq!(layout.hit(mid(label)), Some(SidebarHit::Ci(1)));
        assert_eq!(
            layout.hit(mid(layout.cards[1][0].1)),
            Some(SidebarHit::Card(1, 0))
        );
        // None of it is the header's, and each part lights itself.
        for hit in [
            SidebarHit::Ci(1),
            SidebarHit::CiChip(1, 1),
            SidebarHit::CiPrs(1),
        ] {
            assert_eq!(hit.row(), None);
            assert_eq!(hit.pill(), hit);
        }
    }

    /// The pixel at `(x, y)` of `buffer`, `w` pixels wide.
    fn pixel(
        mut buffer: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
        w: i32,
        x: i32,
        y: i32,
    ) -> crate::chrome::Rgb {
        let mut out = [0; 3];
        let _ = buffer.render().draw(|mem| {
            let i = ((y * w + x) * 4) as usize;
            out = [mem[i + 2], mem[i + 1], mem[i]];
            Ok::<_, ()>(Vec::new())
        });
        out
    }

    #[test]
    fn ci_hover_pills_are_painted_exactly_on_the_hit_rects() {
        use super::{CI_COUNT_TINT, CiCardState, Painter, SidebarState, SidebarView};
        use crate::chrome::{Theme, mix};
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let mut ws = workspace("gerdr", None, None);
        ws.ci = Some(ci_view(&["main", "#42", "dev"], 2));
        let mut view = SidebarView {
            workspaces: vec![ws],
            agents: vec![vec![]],
            active: None,
            project: None,
            header_lit: false,
            state: SidebarState::default(),
        };
        let size = smithay::utils::Size::from((240, 300));
        let layout = sidebar_layout(&view.workspaces, &view.agents, size.w);
        let ci = layout.ci[0].clone().unwrap();
        let at = |view: &SidebarView, x, y| {
            pixel(painter.sidebar_view(view, size, 1.0, p), size.w, x, y)
        };
        let under = p.panel;
        let chip = ci.chips[1];
        let y = chip.loc.y + chip.size.h / 2;
        let (left, right) = (chip.loc.x + 1, chip.loc.x + chip.size.w - 2);
        // Nothing lit: the chip is on the panel.
        assert_eq!(at(&view, left, y), under);
        assert_eq!(at(&view, right, y), under);
        // Lit: a pill from edge to edge of its rect, and no further.
        view.state.ci = vec![CiCardState {
            chips: vec![0, 16],
            ..CiCardState::default()
        }];
        let lit = mix(under, p.text, 0.1);
        assert_eq!(at(&view, left, y), lit);
        assert_eq!(at(&view, right, y), lit);
        assert_eq!(at(&view, chip.loc.x - 1, y), under);
        assert_eq!(at(&view, chip.loc.x + chip.size.w, y), under);
        assert_eq!(at(&view, left, chip.loc.y - 1), under);
        // The first chip stays unlit.
        let first = ci.chips[0];
        assert_eq!(at(&view, first.loc.x + 1, y), under);
        // The count's quiet pill tints more under the pointer, inside its
        // rect.
        let others = ci.others.unwrap();
        let oy = others.loc.y + others.size.h / 2;
        let ox = others.loc.x + 2;
        assert_eq!(at(&view, ox, oy), mix(under, p.muted, CI_COUNT_TINT));
        view.state.ci = vec![CiCardState {
            others: 16,
            ..CiCardState::default()
        }];
        assert_eq!(at(&view, ox, oy), mix(under, p.muted, 2.0 * CI_COUNT_TINT));
        assert_eq!(at(&view, others.loc.x - 1, oy), under);
        // Painted at twice the scale and in the light look too.
        painter.sidebar_view(&view, size, 2.0, Theme::Light.palette());
    }

    /// The layout of a lone workspace's GitHub section of `chips`, `width`
    /// wide, and the section itself.
    fn ci_card_of(chips: &[(Group, &str)], width: i32) -> (SidebarLayout, CiLayout) {
        let mut ws = workspace("gerdr", None, None);
        ws.ci = Some(grouped_ci_view(chips, 0));
        let layout = sidebar_layout(&[ws], &[], width);
        let ci = layout.ci[0].clone().expect("a GitHub section");
        (layout, ci)
    }

    #[test]
    fn each_group_starts_a_line_with_its_label() {
        let chips = [
            (Group::Branch, "main"),
            (Group::Branch, "dev"),
            (Group::Mine, "#42"),
            (Group::Review, "#70"),
        ];
        let (_, ci) = ci_card_of(&chips, 240);
        let first_line = ci.card.loc.y + CI_PAD;
        let ys: Vec<_> = ci.chips.iter().map(|c| c.loc.y).collect();
        assert_eq!(
            ys,
            [
                first_line,
                first_line,
                first_line + CARD_LINE,
                first_line + 2 * CARD_LINE
            ]
        );
        // A label per line, in its chips' band, as wide as the widest.
        let widest = ["CI", "Mine", "Review"]
            .map(|l| text_extent(l, DETAIL_FONT_SIZE))
            .into_iter()
            .max()
            .unwrap();
        let groups: Vec<_> = ci.labels.iter().map(|(g, _)| *g).collect();
        assert_eq!(groups, [Group::Branch, Group::Mine, Group::Review]);
        for ((_, label), chip) in ci.labels.iter().zip([0, 2, 3]) {
            assert_eq!(label.loc.x, ci.card.loc.x + CHILD_TEXT_X - CHILD_X);
            assert_eq!(label.size, (widest, CARD_LINE).into());
            assert_eq!(label.loc.y, ci.chips[chip].loc.y);
            assert!(label.loc.x + label.size.w < ci.chips[chip].loc.x);
        }
        // The first chip of each line in the same column.
        assert_eq!(ci.chips[2].loc.x, ci.chips[0].loc.x);
        assert_eq!(ci.chips[3].loc.x, ci.chips[0].loc.x);
        assert_eq!(ci.card.size.h, 2 * CI_PAD + 3 * CARD_LINE);
    }

    #[test]
    fn a_wrapped_chip_stays_in_the_label_column() {
        let chips = [
            (Group::Branch, "main"),
            (Group::Mine, "#101"),
            (Group::Mine, "#102"),
            (Group::Mine, "#103"),
            (Group::Mine, "#104"),
            (Group::Mine, "#105"),
            (Group::Mine, "#106"),
        ];
        let (_, ci) = ci_card_of(&chips, 200);
        let mine = &ci.chips[1..];
        let wrapped = mine
            .iter()
            .find(|c| c.loc.y > mine[0].loc.y)
            .expect("six chips take more than a line at 200 px");
        assert_eq!(wrapped.loc.y, mine[0].loc.y + CARD_LINE);
        assert_eq!(wrapped.loc.x, mine[0].loc.x);
        assert_eq!(wrapped.loc.x, ci.chips[0].loc.x);
        // No label on the wrapped line: one per group.
        assert_eq!(ci.labels.len(), 2);
        let lines = 1 + (mine[mine.len() - 1].loc.y - mine[0].loc.y) / CARD_LINE + 1;
        assert_eq!(ci.card.size.h, 2 * CI_PAD + lines * CARD_LINE);
    }

    #[test]
    fn an_empty_group_takes_no_line() {
        let (_, ci) = ci_card_of(&[(Group::Branch, "main"), (Group::Review, "#70")], 240);
        let groups: Vec<_> = ci.labels.iter().map(|(g, _)| *g).collect();
        assert_eq!(groups, [Group::Branch, Group::Review]);
        assert_eq!(ci.chips[1].loc.y, ci.chips[0].loc.y + CARD_LINE);
        assert_eq!(ci.card.size.h, 2 * CI_PAD + 2 * CARD_LINE);
        // The column fits the labels present only.
        let (_, mine) = ci_card_of(&[(Group::Mine, "#42")], 240);
        let column = |w: i32| ci.card.loc.x + CHILD_TEXT_X - CHILD_X + w + CI_LABEL_GAP - 4;
        assert_eq!(
            mine.chips[0].loc.x,
            column(text_extent("Mine", DETAIL_FONT_SIZE))
        );
        assert_eq!(
            ci.chips[0].loc.x,
            column(text_extent("Review", DETAIL_FONT_SIZE))
        );
    }
}
