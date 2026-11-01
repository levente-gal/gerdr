//! The tab strip: what each tab shows, where the tabs, the "+" button, the
//! chips and the watch button at the right end and the theme switch go,
//! and how the strip is drawn. Every rect the layout hands out is the rect the painter draws,
//! so a click lands where the pill is.

use super::{
    Badge, Canvas, Density, Dot, FONT_SIZE, FieldSpot, GAP, LABEL_FONT_SIZE, NEW_TAB_SIZE, PADDING,
    Painter, Palette, Rect, Rgb, SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, STROKE, TAB_MAX_WIDTH,
    Theme, ThemeMode, agent_icon, chevron_side, cross, dot_slot, eye, field_text, icon_sdf, mix,
    moon, new_tab, no_restart, physical, rounded_rect, row_dot, segment, status_dot, status_label,
    sun, tab_strip_height, tray, zoom_corners,
};
use crate::agent_icons::AgentIcon;
use crate::app_icons::github_icon;
use crate::herdr::{AgentInfo, AgentStatus, PaneState, Process, TabDetail};
use crate::paths::{repo_root, short_path, tidy_title};
use crate::prepare::is_interactive_shell;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// What a tab shows: its title, and everything herdr reports about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabView {
    /// Its name, or what runs in it (see `TabView::derive`).
    pub title: String,
    /// The most pressing status among its agents, if it has any.
    pub status: Option<AgentStatus>,
    /// The Git branches its panes are on.
    pub branch: Option<String>,
    /// The repository and the folders its panes are in.
    pub place: String,
    /// What runs in its panes: agents, foreground processes, terminal
    /// titles; the distinct ones, split by "  |  ". Idle shells say nothing.
    pub activity: String,
    /// Whether its focused terminal's command runs again after a reboot:
    /// `None` when nothing runs in it.
    pub restart: Option<bool>,
    /// Whether its focused pane shows a GitHub page: its tab carries
    /// GitHub's mark.
    pub github: bool,
    /// The icon of the agent its status is of: the one in its focused
    /// pane, else the first standing as the tab does.
    pub agent: Option<AgentIcon>,
    /// One of its panes is zoomed to fill it: it carries a mark that
    /// unzooms it.
    pub zoomed: bool,
}

impl TabView {
    /// The view of a tab whose first window is titled `title`, mirroring
    /// herdr tab `detail` (if any), whose agents are among `agents`. The
    /// title is only a fallback: see `derive`, which also takes the name
    /// the tab was given.
    pub fn new(title: String, detail: Option<&TabDetail>, agents: &[AgentInfo]) -> Self {
        Self::derive(None, Some(&title), detail, agents)
    }

    /// The view of a tab named `label` (if it was given a name) whose first
    /// window is titled `window` (if it has one), mirroring herdr tab
    /// `detail` (if any), whose agents are among `agents`. Its title is the
    /// name; else what the focused pane runs; else the folder it is in;
    /// else the window's title, tidied. The window title of a bare
    /// terminal ("Alacritty") says nothing and is never used.
    pub fn derive(
        label: Option<&str>,
        window: Option<&str>,
        detail: Option<&TabDetail>,
        agents: &[AgentInfo],
    ) -> Self {
        let label = label.map(str::trim).filter(|l| !l.is_empty());
        let Some(TabDetail { tab, panes, .. }) = detail else {
            let title = label
                .map(str::to_owned)
                .or_else(|| window_title(window))
                .unwrap_or_else(|| if window.is_some() { "terminal" } else { "…" }.into());
            return Self {
                title,
                ..Default::default()
            };
        };
        let focused = panes.iter().find(|p| p.focused).or(panes.first());
        let title = label
            .map(str::to_owned)
            .or_else(|| focused.and_then(|p| foreground_program(p, agents)))
            .or_else(|| focused.and_then(folder_name))
            .or_else(|| window_title(window))
            .unwrap_or_else(|| "terminal".into());
        let status = (tab.agent_status != AgentStatus::Unknown).then_some(tab.agent_status);
        // The agent the status is of: in the focused pane, else the first
        // standing as the tab does, else any; known by herdr's id, else by
        // name.
        let has_agent = |p: &&PaneState| p.agent.is_some();
        let standing = |p: &&PaneState| has_agent(p) && p.agent_status == tab.agent_status;
        let agent = focused
            .filter(standing)
            .or_else(|| panes.iter().find(standing))
            .or_else(|| focused.filter(has_agent))
            .or_else(|| panes.iter().find(has_agent))
            .map(|pane| {
                agents
                    .iter()
                    .find(|a| a.pane_id == pane.pane_id)
                    .map(AgentIcon::from_info)
                    .filter(|icon| *icon != AgentIcon::Generic)
                    .unwrap_or_else(|| AgentIcon::from_agent(pane.agent.as_deref().unwrap_or("")))
            });
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
        let activity = unique(
            panes
                .iter()
                .map(|p| pane_activity(p, &title, agents))
                .filter(|a| !a.is_empty()),
        );
        Self {
            title,
            status,
            branch,
            place,
            activity: activity.join("  |  "),
            restart: None,
            github: false,
            agent,
            zoomed: false,
        }
    }

    /// The tooltip of its tab: the title, then where it is, then what its
    /// "⊘" mark says, if it has one.
    pub fn tooltip(&self) -> String {
        let mut parts = vec![self.title.as_str()];
        if !self.place.is_empty() {
            parts.push(&self.place);
        }
        parts.extend(self.branch.as_deref());
        if self.restart == Some(false) {
            parts.push("⊘ won't run again after a reboot");
        }
        parts.join(" · ")
    }
}

/// `window`, a window's title, tidied: `None` when it says nothing (a
/// terminal's default title is its app's name).
fn window_title(window: Option<&str>) -> Option<String> {
    let title = tidy_title(window?);
    let lower = title.to_lowercase();
    (!title.is_empty()
        && !["alacritty", "ghostty"]
            .iter()
            .any(|app| lower.contains(app)))
    .then_some(title)
}

/// The program in front in pane `p`: its agent, by name, else the leader
/// of its foreground process group unless that is the shell at its prompt.
fn foreground_program(p: &PaneState, agents: &[AgentInfo]) -> Option<String> {
    let agent = agents.iter().find(|a| a.pane_id == p.pane_id);
    agent
        .and_then(|a| a.display_agent.clone())
        .or_else(|| p.agent.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| running_leader(p).map(program_name))
}

/// The process in front in pane `p`, if it is not the shell at its prompt.
fn running_leader(p: &PaneState) -> Option<&Process> {
    let info = p.process.as_ref()?;
    let group = info.foreground_process_group_id?;
    if Some(group) == info.shell_pid {
        return None;
    }
    let leader = info
        .foreground_processes
        .iter()
        .find(|p| p.pid == group)
        .or(info.foreground_processes.first())?;
    (!is_interactive_shell(leader)).then_some(leader)
}

/// A process by the name it was started as: "/usr/bin/cargo build" is "cargo".
fn program_name(process: &Process) -> String {
    let argv0 = process.argv.first().map(String::as_str);
    let name = argv0
        .filter(|a| !a.is_empty())
        .unwrap_or(&process.name)
        .trim_start_matches('-');
    name.rsplit('/').next().unwrap_or(name).to_owned()
}

/// The name of the folder pane `p` is in: "~" for home.
fn folder_name(p: &PaneState) -> Option<String> {
    let path = p.cwd.as_deref().or(p.foreground_cwd.as_deref())?;
    if crate::home_dir().is_some_and(|home| home == path) {
        return Some("~".into());
    }
    Some(match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    })
}

/// What runs in pane `p` of a tab titled `title`: its agent and its status,
/// else its foreground processes (not a shell at its prompt); its terminal
/// title when that says more; how far it is scrolled back.
pub(super) fn pane_activity(p: &PaneState, title: &str, agents: &[AgentInfo]) -> String {
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
    let processes = p
        .process
        .iter()
        .flat_map(|info| &info.foreground_processes)
        .filter(|proc| !is_interactive_shell(proc));
    let commands =
        processes.map(|proc| command_line(&proc.cmdline).unwrap_or_else(|| proc.name.clone()));
    let commands = unique(commands).join(" | ");
    if !commands.is_empty() && !parts.iter().any(|part| part.starts_with(&commands)) {
        parts.push(commands);
    }
    let term_title = p
        .terminal_title_stripped
        .as_deref()
        .or(p.terminal_title.as_deref())
        .map(tidy_title);
    if let Some(term_title) = term_title.filter(|t| {
        !t.is_empty()
            && t != title
            && window_title(Some(t)).is_some()
            && !parts.iter().any(|part| part.contains(t.as_str()))
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

/// The distinct values of `items`, in order of first appearance.
fn unique(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

/// The distinct values of `items` in order, joined; `None` if there are none.
pub(super) fn distinct(items: impl Iterator<Item = String>) -> Option<String> {
    let seen = unique(items);
    (!seen.is_empty()).then(|| seen.join(", "))
}

/// Something in the tab strip the pointer can be over or press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHit {
    Tab(usize),
    /// The "×" of tab `i`.
    Close(usize),
    /// The zoom mark of tab `i`, whose pane fills it.
    Unzoom(usize),
    /// The "+" button.
    Plus,
    /// The chip holding the tabs that don't fit.
    More,
    /// The chip saying other workspaces need attention.
    Overflow,
    /// The badge showing the prefix is armed.
    Prefix,
    /// The theme switch.
    Toggle,
    /// The button watching (or no longer watching) the CI of the pull
    /// request the focused pane shows.
    Watch,
    /// The inbox button.
    Inbox,
    /// The button listing what went wrong (see `crate::problems`).
    Problems,
}

/// Everything the tab strip is drawn from.
#[derive(Clone, Debug, PartialEq)]
pub struct TabStripView {
    pub tabs: Vec<TabView>,
    pub active: usize,
    /// The prefix is armed (or the help pinned): the PREFIX badge shows.
    pub prefix: bool,
    /// The theme mode the switch shows.
    pub mode: ThemeMode,
    /// What the pointer is over, if anything.
    pub hover: Option<TabHit>,
    /// How far the hover highlight has faded in, in `anim::quantize` steps
    /// (0 to 16).
    pub hover_alpha: u8,
    /// The tab being renamed, and the name typed so far.
    pub renaming: Option<(usize, Line)>,
    /// The workspaces not shown, and how many of them have a blocked agent.
    pub elsewhere: (usize, usize),
    /// Whether the CI of the pull request the focused pane shows is
    /// watched; `None` hides the watch button (not a pull request's page).
    pub watch: Option<bool>,
    /// How many items the inbox button counts, its badge; `None` hides the
    /// button.
    pub inbox: Option<usize>,
    /// How many problems the problems button counts; `None` hides it (there
    /// are none).
    pub problems: Option<usize>,
    /// A workspace is shown, so the "+" can open a tab in it, even with
    /// no tabs left.
    pub plus: bool,
}

impl TabStripView {
    /// A view of `tabs` with `active` shown, the prefix badge when
    /// `prefix`, and nothing else going on.
    pub fn plain(tabs: Vec<TabView>, active: usize, prefix: bool, mode: ThemeMode) -> Self {
        Self {
            tabs,
            active,
            prefix,
            mode,
            hover: None,
            hover_alpha: 0,
            renaming: None,
            elsewhere: (0, 0),
            watch: None,
            inbox: None,
            problems: None,
            plus: true,
        }
    }

    /// Where everything in it sits on a strip `size` big: the tabs that
    /// fit taking in the active one (see `tab_layout_at`).
    pub fn layout(&self, size: Size<i32, Logical>) -> TabLayout {
        tab_layout_at(
            self.tabs.len(),
            self.active,
            self.plus,
            size,
            self.prefix,
            self.overflow(),
            self.watch.is_some(),
            self.inbox.is_some(),
            self.problems.is_some(),
        )
    }

    /// Whether the overflow chip shows: some workspace not shown is blocked.
    pub fn overflow(&self) -> bool {
        self.elsewhere.1 > 0
    }

    /// The hover highlight's opacity, 0 to 1.
    fn alpha(&self) -> f32 {
        f32::from(self.hover_alpha.min(16)) / 16.0
    }
}

/// The "+" button, the theme switch and the overflow chip are square
/// pills this big, vertically centred in the strip.
const PILL: i32 = NEW_TAB_SIZE;
const PILL_RADIUS: f32 = 6.0;
/// Room between the last tab and the "+".
const PLUS_GAP: i32 = 4;
/// Room between the chips at the right end, and the room left of them.
const CHIP_GAP: i32 = 6;
/// Inset of the theme switch from the strip's right edge.
const EDGE: i32 = 4;
/// The overflow chip ("⋯ N" with its warning dot) and the PREFIX badge.
const OVERFLOW_WIDTH: i32 = 44;
const PREFIX_WIDTH: i32 = 64;
const PREFIX_HEIGHT: i32 = 20;
/// The watch button: an eye and "Watch CI" (or "Watching"), and the gap
/// between them.
const WATCH_WIDTH: i32 = 96;
const WATCH_ICON: i32 = 16;
const WATCH_GAP: i32 = 5;
/// How far the inbox button's count badge sticks out of its top right
/// corner.
const BADGE_OUT: i32 = 4;
/// The "×" of a tab: its square, and its inset from the tab's right edge.
const CLOSE: i32 = 22;
const CLOSE_INSET: i32 = 5;
/// A tab narrower than this has no "×".
const CLOSE_MIN_TAB: i32 = 2 * PADDING as i32 + CLOSE + 40;
/// The narrowest a tab gets: past that, the tabs that don't fit hide
/// behind the "more tabs" chip, after those shown.
const TAB_MIN_WIDTH: i32 = CLOSE_MIN_TAB + 24;
/// The "more tabs" chip: a chevron and how many it holds.
const MORE_WIDTH: i32 = 44;
/// How many tabs have a number, and a key going to them.
const NUMBERED_TABS: usize = 9;
/// The rename field inside a tab: this far in from the tab's edges.
const RENAME_INSET: i32 = 4;
/// How big GitHub's mark is on a tab showing a GitHub page, as in the
/// sidebar's GitHub card.
const GITHUB_ICON_SIZE: i32 = 12;

/// Where everything in a strip sits (strip coordinates): the tabs (each
/// one's rect runs up to and including its 1 px border on the right, so
/// the strip has no dead gaps; an empty one for a tab that doesn't fit),
/// their "×" buttons (empty rects on tabs too narrow for one), the "more
/// tabs" chip while some don't fit, the "+" pill (empty while no
/// workspace is shown), the overflow chip, the PREFIX badge, the watch
/// button and the inbox button when shown, and the theme switch at the
/// right end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabLayout {
    pub tabs: Vec<Rect>,
    pub closes: Vec<Rect>,
    /// The chip holding the tabs that don't fit, while some don't.
    pub more: Option<Rect>,
    pub plus: Rect,
    pub overflow: Option<Rect>,
    pub prefix: Option<Rect>,
    /// The watch button, when shown.
    pub watch: Option<Rect>,
    /// The inbox button, when shown.
    pub inbox: Option<Rect>,
    /// The problems button, left of the inbox button, when shown.
    pub problems: Option<Rect>,
    pub toggle: Rect,
}

impl TabLayout {
    /// The tabs that don't fit, behind the "more tabs" chip.
    pub fn hidden(&self) -> Vec<usize> {
        (0..self.tabs.len())
            .filter(|&i| self.tabs[i].size.w == 0)
            .collect()
    }

    /// What is at `(x, y)` (strip coordinates), if anything.
    pub fn hit(&self, x: i32, y: i32) -> Option<TabHit> {
        let inside = |r: &Rect| r.contains((x, y));
        if inside(&self.toggle) {
            return Some(TabHit::Toggle);
        }
        if self.inbox.as_ref().is_some_and(inside) {
            return Some(TabHit::Inbox);
        }
        if self.problems.as_ref().is_some_and(inside) {
            return Some(TabHit::Problems);
        }
        if self.overflow.as_ref().is_some_and(inside) {
            return Some(TabHit::Overflow);
        }
        if self.prefix.as_ref().is_some_and(inside) {
            return Some(TabHit::Prefix);
        }
        if self.watch.as_ref().is_some_and(inside) {
            return Some(TabHit::Watch);
        }
        if let Some(i) = self.closes.iter().position(inside) {
            return Some(TabHit::Close(i));
        }
        if let Some(i) = self.tabs.iter().position(inside) {
            return Some(TabHit::Tab(i));
        }
        if self.more.as_ref().is_some_and(inside) {
            return Some(TabHit::More);
        }
        inside(&self.plus).then_some(TabHit::Plus)
    }

    /// Where the field renaming tab `t` goes.
    pub fn rename_field(&self, t: usize) -> Option<Rect> {
        let tab = self.tabs.get(t)?;
        Some(Rectangle::new(
            (tab.loc.x + RENAME_INSET, self.plus.loc.y).into(),
            ((tab.size.w - GAP - 2 * RENAME_INSET).max(0), PILL).into(),
        ))
    }
}

/// The layout of a strip of `size` holding `count` tabs, the first ones
/// shown if not all fit, with the "+": see `tab_layout_at`, which takes
/// in the active tab. Tests use it; the chrome lays a strip out with
/// `TabStripView::layout`.
// `#[cfg(test)]` once chrome/mod.rs lays its hit rects out with
// `strip.layout(..)`, as it does after the wave-2 merge.
#[cfg_attr(not(test), allow(dead_code))]
pub fn tab_layout(
    count: usize,
    size: Size<i32, Logical>,
    prefix: bool,
    overflow: bool,
    watch: bool,
    inbox: bool,
) -> TabLayout {
    tab_layout_at(count, 0, true, size, prefix, overflow, watch, inbox, false)
}

/// The layout of a strip of `size` holding `count` tabs, `active` among
/// them, with the "+" when `plus`, the PREFIX badge when `prefix`, the
/// overflow chip when `overflow`, the watch button when `watch`, the
/// inbox button, by the switch, when `inbox` and the problems button left
/// of it when `problems`. The chips, the buttons and
/// the switch keep the right end, the PREFIX badge's room kept whether it
/// shows or not, so it moves nothing; the tabs share what is left of the
/// "+", at most `TAB_MAX_WIDTH` each. Past `TAB_MIN_WIDTH` each, those
/// that fit show, taking in `active`, and the rest go behind the "more
/// tabs" chip between them and the "+".
#[allow(clippy::too_many_arguments)]
pub fn tab_layout_at(
    count: usize,
    active: usize,
    plus: bool,
    size: Size<i32, Logical>,
    prefix: bool,
    overflow: bool,
    watch: bool,
    inbox: bool,
    problems: bool,
) -> TabLayout {
    let (width, height) = (size.w, size.h);
    let top = (height - PILL) / 2;
    let toggle = Rectangle::new((width - EDGE - PILL, top).into(), (PILL, PILL).into());
    let mut right = toggle.loc.x;
    let inbox = inbox.then(|| {
        right -= CHIP_GAP + PILL;
        Rectangle::new((right, top).into(), (PILL, PILL).into())
    });
    let problems = problems.then(|| {
        right -= CHIP_GAP + PILL;
        Rectangle::new((right, top).into(), (PILL, PILL).into())
    });
    right -= CHIP_GAP + PREFIX_WIDTH;
    let prefix = prefix.then(|| {
        Rectangle::new(
            (right, (height - PREFIX_HEIGHT) / 2).into(),
            (PREFIX_WIDTH, PREFIX_HEIGHT).into(),
        )
    });
    let overflow = overflow.then(|| {
        right -= CHIP_GAP + OVERFLOW_WIDTH;
        Rectangle::new((right, top).into(), (OVERFLOW_WIDTH, PILL).into())
    });
    let watch = watch.then(|| {
        right -= CHIP_GAP + WATCH_WIDTH;
        Rectangle::new((right, top).into(), (WATCH_WIDTH, PILL).into())
    });
    let room = (right - CHIP_GAP - PILL - PLUS_GAP).max(0);
    // How many show, from which: all, or as many as fit at the narrowest
    // beside the "more tabs" chip, the active one last if it is past them.
    let all_fit = count as i32 * TAB_MIN_WIDTH <= room;
    let (first, shown, room) = if all_fit {
        (0, count, room)
    } else {
        let room = (room - MORE_WIDTH - PLUS_GAP).max(0);
        let shown = ((room / TAB_MIN_WIDTH) as usize).clamp(1, count);
        let first = (active + 1).saturating_sub(shown).min(count - shown);
        (first, shown, room)
    };
    let tab_w = if shown == 0 {
        0
    } else {
        (room / shown as i32).min(TAB_MAX_WIDTH)
    };
    let end = shown as i32 * tab_w;
    let tabs: Vec<Rect> = (0..count)
        .map(|i| {
            if (first..first + shown).contains(&i) {
                let x = (i - first) as i32 * tab_w;
                Rectangle::new((x, 0).into(), (tab_w, height).into())
            } else {
                // Not shown: no room, before or after those that are.
                let x = if i < first { 0 } else { end };
                Rectangle::new((x, 0).into(), (0, height).into())
            }
        })
        .collect();
    let more =
        (!all_fit).then(|| Rectangle::new((end + PLUS_GAP, top).into(), (MORE_WIDTH, PILL).into()));
    let plus_x = more.map_or(end, |m| m.loc.x + m.size.w) + PLUS_GAP;
    let closes = tabs
        .iter()
        .map(|tab| {
            let end = tab.loc.x + tab.size.w - GAP;
            if tab.size.w < CLOSE_MIN_TAB {
                Rectangle::new((end, top).into(), (0, 0).into())
            } else {
                Rectangle::new(
                    (end - CLOSE_INSET - CLOSE, (height - CLOSE) / 2).into(),
                    (CLOSE, CLOSE).into(),
                )
            }
        })
        .collect();
    let plus = Rectangle::new(
        (plus_x, top).into(),
        if plus { (PILL, PILL) } else { (0, 0) }.into(),
    );
    TabLayout {
        tabs,
        closes,
        more,
        plus,
        overflow,
        prefix,
        watch,
        inbox,
        problems,
        toggle,
    }
}

/// Where a tab dragged to `x` (strip coordinates) goes in a strip laid
/// out as `layout`: the place under `x`, the first or last beside them all.
pub fn tab_slot(layout: &TabLayout, x: f64) -> usize {
    layout
        .tabs
        .iter()
        .rposition(|tab| x >= tab.loc.x as f64)
        .unwrap_or(0)
}

/// The field renaming a tab, for clicks into it: `strip` is where the
/// strip sits in the window.
pub fn tab_rename_field(view: &TabStripView, strip: Rect, scale: f64) -> Option<FieldSpot> {
    let (t, line) = view.renaming.as_ref()?;
    let layout = view.layout(strip.size);
    let mut area = layout.rename_field(*t)?;
    area.loc += strip.loc;
    Some(FieldSpot {
        size: FONT_SIZE,
        area,
        text: field_text(area, scale),
        line: line.clone(),
    })
}

/// Signed distance to a monitor icon centred on the origin: a screen on a
/// stand. The theme follows the system.
fn monitor(x: f32, y: f32, s: f32) -> f32 {
    let screen =
        rounded_rect(x, y, -7.0 * s, -6.0 * s, 14.0 * s, 10.0 * s, 1.5 * s).abs() - STROKE * s;
    let neck = segment(x, y, 0.0, 4.0 * s, 0.0, 6.5 * s) - STROKE * s;
    let foot = segment(x, y, -3.5 * s, 6.5 * s, 3.5 * s, 6.5 * s) - STROKE * s;
    screen.min(neck).min(foot)
}

/// Signed distance to three dots in a row centred on the origin: more.
fn ellipsis(x: f32, y: f32, s: f32) -> f32 {
    [-4.5, 0.0, 4.5]
        .into_iter()
        .map(|cx: f32| (x - cx * s).hypot(y) - 1.4 * s)
        .fold(f32::MAX, f32::min)
}

/// Signed distance to a "!" in a ring centred on the origin: something
/// went wrong.
fn alert(x: f32, y: f32, s: f32) -> f32 {
    let ring = (x.hypot(y) - 7.0 * s).abs() - STROKE * s;
    let bar = segment(x, y, 0.0, -3.6 * s, 0.0, 0.8 * s) - 1.1 * s;
    let dot = x.hypot(y - 3.4 * s) - 1.2 * s;
    ring.min(bar).min(dot)
}

/// The theme mode's icon, as the switch shows it.
fn mode_icon(mode: ThemeMode) -> fn(f32, f32, f32) -> f32 {
    match mode {
        ThemeMode::System => monitor,
        ThemeMode::Dark => moon,
        ThemeMode::Light => sun,
    }
}

impl Painter {
    /// The tab strip: one line per tab that fits (status mark, or GitHub's
    /// on a tab showing a GitHub page; number for the first nine, title,
    /// "×" on the active tab and under the pointer), the "more tabs" chip
    /// while some don't fit, the "+" pill after them, and
    /// at the right end the watch button, the overflow chip, the PREFIX
    /// badge, the inbox button with its count and the theme switch. A strip
    /// lower than the comfortable height is compact.
    pub(super) fn tab_strip_view(
        &self,
        view: &TabStripView,
        theme: Theme,
        size: Size<i32, Logical>,
        scale: f64,
    ) -> MemoryRenderBuffer {
        let p = theme.palette();
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let pad = (PADDING * s).round();
        let line = px(1).max(1);
        let compact = size.h < tab_strip_height(Density::Comfortable);
        let font = if compact { FONT_SIZE } else { LABEL_FONT_SIZE };
        let alpha = view.alpha();
        let lit = |hit: TabHit| view.hover == Some(hit) && alpha > 0.0;
        c.fill(0, 0, c.w, c.h, p.tab_bar);
        c.fill(0, c.h - line, c.w, line, p.border);

        let layout = view.layout(size);
        let band = (0, c.h);
        for (i, (rect, tab)) in layout.tabs.iter().zip(&view.tabs).enumerate() {
            if rect.size.w == 0 {
                continue;
            }
            let (x, w) = (
                px(rect.loc.x),
                px(rect.loc.x + rect.size.w) - px(rect.loc.x),
            );
            let is_active = i == view.active;
            let hovered = lit(TabHit::Tab(i)) || lit(TabHit::Close(i)) || lit(TabHit::Unzoom(i));
            // Like Zed: the active tab opens into the editor below it; a
            // tab under the pointer lifts a little.
            if is_active {
                c.fill(x, 0, w - line, c.h, p.tab_active);
            } else if hovered {
                let fill = mix(p.tab_bar, p.element, alpha);
                c.fill(x, 0, w - line, c.h - line, fill);
            }
            c.fill(x + w - line, 0, line, c.h, p.border);

            if let Some((_, name)) = view.renaming.as_ref().filter(|(t, _)| *t == i)
                && let Some(field) = layout.rename_field(i)
            {
                self.field(&mut c, name, "Tab name", field, true, scale, p);
                continue;
            }

            let close = layout.closes[i];
            let show_close = close.size.w > 0 && (is_active || hovered);
            let mut right = if show_close {
                px(close.loc.x) as f32 - px(PLUS_GAP) as f32
            } else {
                px(rect.loc.x + rect.size.w - GAP) as f32 - pad
            };
            // Its agent's dot, before the "×" (where it goes, shown or not).
            let badge = tab_badge(tab);
            if let Some(badge) = badge {
                let dot = tab_dot(*rect, close, size.h);
                status_dot(&mut c, dot, badge.dot, scale, p);
                right = right.min(px(dot_slot(tab_dot_line(*rect, close, size.h)).loc.x) as f32);
            }
            // A pane fills the tab: its mark, before the dot, unzooms it.
            if let Some(mark) = tab_zoom_mark(tab, *rect, close, size.h) {
                if lit(TabHit::Unzoom(i)) {
                    let behind = if is_active { p.tab_active } else { p.element };
                    self.pill(&mut c, mark, 4.0, mix(behind, p.selected, alpha), scale);
                }
                let color = if lit(TabHit::Unzoom(i)) {
                    p.text
                } else {
                    p.muted
                };
                icon_sdf(&mut c, mark, scale, color, |x, y| {
                    zoom_corners(true, x, y, s)
                });
                right = right.min(px(mark.loc.x) as f32);
            }
            // What runs in the tab won't run again after a reboot: say so
            // before the "×" and the dot.
            if tab.restart == Some(false) {
                let mark_w = px(STATUS_WIDTH) as f32;
                let mark = Rectangle::new(
                    (((right - mark_w) / s).round() as i32, 0).into(),
                    (STATUS_WIDTH, size.h).into(),
                );
                icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                    no_restart(x, y, s)
                });
                right = px(mark.loc.x) as f32 - px(STATUS_GAP) as f32;
            }
            // Before the number, a mark: its agent's icon when its status
            // says something, else GitHub's when the tab shows a GitHub page.
            let mut text_x = x as f32 + pad;
            let mark = Rectangle::new(
                (rect.loc.x + PADDING as i32, 0).into(),
                (STATUS_WIDTH, size.h).into(),
            );
            let github = tab.github.then(github_icon).flatten();
            if let Some(badge) = badge {
                let tint = if is_active { p.muted } else { p.placeholder };
                let icon = tab_icon_rect(*rect, size.h);
                agent_icon(&mut c, icon, TAB_ICON, badge.icon, tint, scale);
                text_x += ((TAB_ICON + STATUS_GAP) as f32 * s).round();
            } else if let Some(icon) = &github {
                let tint = if is_active { p.muted } else { p.placeholder };
                c.picture_sized(icon, mark, scale, tint, GITHUB_ICON_SIZE);
                text_x += ((STATUS_WIDTH + STATUS_GAP) as f32 * s).round();
            }
            // Only the tabs a key goes to have a number.
            let after = if i < NUMBERED_TABS {
                let number = (i + 1).to_string();
                let end = self.text(
                    &mut c,
                    &number,
                    font,
                    text_x,
                    right,
                    band,
                    scale,
                    if is_active { p.muted } else { p.placeholder },
                );
                end + px(8) as f32
            } else {
                text_x
            };
            self.text(
                &mut c,
                &tab.title,
                font,
                after,
                right,
                band,
                scale,
                if is_active { p.text } else { p.muted },
            );
            if show_close {
                if lit(TabHit::Close(i)) {
                    let behind = if is_active { p.tab_active } else { p.element };
                    self.pill(&mut c, close, 4.0, mix(behind, p.selected, alpha), scale);
                }
                let color = if lit(TabHit::Close(i)) {
                    p.text
                } else {
                    p.muted
                };
                icon_sdf(&mut c, close, scale, color, |x, y| cross(x, y, s));
            }
        }
        if let Some(chip) = layout.more {
            let fill = if lit(TabHit::More) {
                mix(p.element, p.selected, alpha)
            } else {
                p.element
            };
            self.pill(&mut c, chip, PILL_RADIUS, fill, scale);
            let arrow = Rectangle::new(chip.loc, (PILL - 8, PILL).into());
            icon_sdf(&mut c, arrow, scale, p.muted, |x, y| {
                chevron_side(false, x, y, s)
            });
            let count = Rectangle::new(
                (arrow.loc.x + arrow.size.w - 2, chip.loc.y).into(),
                (chip.size.w - arrow.size.w, chip.size.h).into(),
            );
            let hidden = layout.hidden().len().to_string();
            self.centered(&mut c, &hidden, SMALL_FONT_SIZE, count, scale, p.text);
        }
        if layout.plus.size.w > 0 {
            // A soft rounded square with a large "+", centred in the strip.
            let fill = if lit(TabHit::Plus) {
                mix(p.element, p.selected, alpha)
            } else {
                p.element
            };
            self.pill(&mut c, layout.plus, PILL_RADIUS, fill, scale);
            icon_sdf(&mut c, layout.plus, scale, p.text, |x, y| new_tab(x, y, s));
        }

        if let Some(chip) = layout.overflow {
            let fill = if lit(TabHit::Overflow) {
                mix(p.element, p.selected, alpha)
            } else {
                p.element
            };
            self.pill(&mut c, chip, PILL_RADIUS, fill, scale);
            let dots = Rectangle::new(chip.loc, (PILL - 6, PILL).into());
            icon_sdf(&mut c, dots, scale, p.muted, |x, y| ellipsis(x, y, s));
            let count = Rectangle::new(
                (dots.loc.x + dots.size.w, chip.loc.y).into(),
                (chip.size.w - dots.size.w, chip.size.h).into(),
            );
            self.centered(
                &mut c,
                &view.elsewhere.0.to_string(),
                SMALL_FONT_SIZE,
                count,
                scale,
                p.text,
            );
            // The warning dot, a badge on the chip's top right corner.
            let badge = Rectangle::new(
                (chip.loc.x + chip.size.w - 9, chip.loc.y + 1).into(),
                (8, 8).into(),
            );
            icon_sdf(&mut c, badge, scale, p.warning, |x, y| x.hypot(y) - 3.2 * s);
        }

        if let (Some(button), Some(watching)) = (layout.watch, view.watch) {
            self.watch_button(
                &mut c,
                button,
                watching,
                lit(TabHit::Watch),
                alpha,
                scale,
                p,
            );
        }

        if let (Some(button), Some(count)) = (layout.inbox, view.inbox) {
            self.inbox_button(&mut c, button, count, lit(TabHit::Inbox), alpha, scale, p);
        }

        if let (Some(button), Some(count)) = (layout.problems, view.problems) {
            let lit = lit(TabHit::Problems);
            self.problems_button(&mut c, button, count, lit, alpha, scale, p);
        }

        if let Some(badge) = layout.prefix {
            let (bx, by, bw, bh) = super::rect_px(badge, scale);
            c.rounded(bx, by, bw, bh, bh / 2.0, p.accent);
            self.centered(&mut c, "PREFIX", SMALL_FONT_SIZE, badge, scale, p.on_accent);
        }

        // The switch shows the mode in use: a monitor while following the
        // system, else the moon or the sun.
        if lit(TabHit::Toggle) {
            let fill = mix(p.tab_bar, p.element, alpha);
            self.pill(&mut c, layout.toggle, PILL_RADIUS, fill, scale);
        }
        let icon = mode_icon(view.mode);
        icon_sdf(&mut c, layout.toggle, scale, p.muted, |x, y| icon(x, y, s));
        c.into_buffer(true)
    }

    /// The button watching the CI of the pull request shown, at `rect`: an
    /// outlined pill saying "Watch CI", or while `watching` a filled one
    /// saying "Watching"; lighter while `lit` (fading in by `alpha`).
    #[allow(clippy::too_many_arguments)]
    fn watch_button(
        &self,
        c: &mut Canvas,
        rect: Rect,
        watching: bool,
        lit: bool,
        alpha: f32,
        scale: f64,
        p: &Palette,
    ) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (label, content) = if watching {
            let fill = if lit {
                mix(p.accent, p.text, 0.15 * alpha)
            } else {
                p.accent
            };
            self.pill(c, rect, PILL_RADIUS, fill, scale);
            ("Watching", p.on_accent)
        } else {
            let b = px(1).max(1) as f32;
            let fill = if lit {
                mix(p.tab_bar, p.element, alpha)
            } else {
                p.tab_bar
            };
            let (x, y, w, h) = super::rect_px(rect, scale);
            let r = PILL_RADIUS * s;
            c.rounded(x, y, w, h, r, p.border);
            c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
            ("Watch CI", if lit { p.text } else { p.muted })
        };
        // The eye and the label, centred together.
        let text_w = self.text_width(label, SMALL_FONT_SIZE, scale) / s;
        let content_w = (WATCH_ICON + WATCH_GAP) as f32 + text_w;
        let start = rect.loc.x + ((rect.size.w as f32 - content_w) / 2.0).round() as i32;
        let icon = Rectangle::new((start, rect.loc.y).into(), (WATCH_ICON, rect.size.h).into());
        icon_sdf(c, icon, scale, content, |x, y| eye(x, y, s));
        self.text(
            c,
            label,
            SMALL_FONT_SIZE,
            px(start + WATCH_ICON + WATCH_GAP) as f32,
            px(rect.loc.x + rect.size.w) as f32,
            (px(rect.loc.y), px(rect.size.h)),
            scale,
            content,
        );
    }

    /// The inbox button at `rect`: a tray, lit while `lit` (fading in by
    /// `alpha`), and while `count` isn't 0 a badge with it on the top
    /// right corner.
    #[allow(clippy::too_many_arguments)]
    fn inbox_button(
        &self,
        c: &mut Canvas,
        rect: Rect,
        count: usize,
        lit: bool,
        alpha: f32,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        if lit {
            self.pill(
                c,
                rect,
                PILL_RADIUS,
                mix(p.tab_bar, p.element, alpha),
                scale,
            );
        }
        let color = if lit { p.text } else { p.muted };
        icon_sdf(c, rect, scale, color, |x, y| tray(x, y, s));
        if count == 0 {
            return;
        }
        let colors = (p.tab_bar, p.accent, p.on_accent);
        self.count_badge(c, rect, count, 99, BADGE_OUT, colors, scale);
    }

    /// The problems button at `rect`: a "!" in a ring in the warning
    /// colour, lit while `lit` (fading in by `alpha`), with a badge saying
    /// how many there are (`count`) on its top right corner.
    #[allow(clippy::too_many_arguments)]
    fn problems_button(
        &self,
        c: &mut Canvas,
        rect: Rect,
        count: usize,
        lit: bool,
        alpha: f32,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        if lit {
            let fill = mix(p.tab_bar, p.element, alpha);
            self.pill(c, rect, PILL_RADIUS, fill, scale);
        }
        icon_sdf(c, rect, scale, p.warning, |x, y| alert(x, y, s));
        let colors = (p.tab_bar, p.warning, p.on_accent);
        self.count_badge(c, rect, count, 99, BADGE_OUT, colors, scale);
    }

    /// A rounded pill filling `rect` (logical) with corners of `radius`.
    fn pill(&self, c: &mut Canvas, rect: Rect, radius: f32, color: Rgb, scale: f64) {
        let (x, y, w, h) = super::rect_px(rect, scale);
        c.rounded(x, y, w, h, radius * scale as f32, color);
    }
}

/// Where the strip's tabs are hit, for the `hover` of a `TabStripView`:
/// `pos` in strip coordinates.
/// How big an agent's badge is on its tab.
const TAB_ICON: i32 = GITHUB_ICON_SIZE;

/// The status a tab's mark shows: one that says something, or idle for
/// a tab whose agent is known, so a tab with an agent shows whose it is.
fn tab_status(tab: &TabView) -> Option<AgentStatus> {
    tab.status.filter(|s| match s {
        AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done => true,
        AgentStatus::Idle => tab.agent.is_some(),
        AgentStatus::Unknown => false,
    })
}

/// The badge a tab shows before its number, while its status says
/// something: its agent's icon (a generic one if not known) and dot.
fn tab_badge(tab: &TabView) -> Option<Badge> {
    tab_status(tab).map(|status| Badge {
        icon: tab.agent.unwrap_or(AgentIcon::Generic),
        dot: Dot::of_status(status),
    })
}

/// Where the badge of the tab at `rect` goes on a strip `h` high.
fn tab_icon_rect(rect: Rect, h: i32) -> Rect {
    Rectangle::new(
        (rect.loc.x + PADDING as i32, 0).into(),
        (TAB_ICON, h).into(),
    )
}

/// The line a tab at `rect` with its "×" at `close` (none when too
/// narrow) puts its dot at the end of: up to the "×".
fn tab_dot_line(rect: Rect, close: Rect, h: i32) -> Rect {
    let end = if close.size.w > 0 {
        close.loc.x - PLUS_GAP
    } else {
        rect.loc.x + rect.size.w - GAP - PADDING as i32
    };
    Rectangle::new(
        (rect.loc.x, 0).into(),
        ((end - rect.loc.x).max(0), h).into(),
    )
}

/// Where the dot of a tab at `rect` with its "×" at `close` goes.
fn tab_dot(rect: Rect, close: Rect, h: i32) -> Rect {
    row_dot(tab_dot_line(rect, close, h))
}

/// Where the zoom mark of `tab`, at `rect` with its "×" at `close`, goes:
/// before its dot's place, shown or not, so it stays put. None while no
/// pane of it is zoomed, or on a tab too narrow for a "×".
fn tab_zoom_mark(tab: &TabView, rect: Rect, close: Rect, h: i32) -> Option<Rect> {
    if !tab.zoomed || close.size.w == 0 {
        return None;
    }
    let slot = dot_slot(tab_dot_line(rect, close, h));
    Some(Rectangle::new(
        (slot.loc.x - CLOSE, (h - CLOSE) / 2).into(),
        (CLOSE, CLOSE).into(),
    ))
}

/// Where the blinking dots of `view`'s tabs go on its strip, `size` big
/// (strip coordinates): one per tab at work, but the one being renamed.
pub fn tab_blinkers(view: &TabStripView, size: Size<i32, Logical>) -> Vec<Rect> {
    let layout = view.layout(size);
    let renaming = view.renaming.as_ref().map(|(i, _)| *i);
    layout
        .tabs
        .iter()
        .zip(&layout.closes)
        .zip(&view.tabs)
        .enumerate()
        .filter(|(i, ((rect, _), tab))| {
            Some(*i) != renaming
                && rect.size.w > TAB_ICON
                && tab_badge(tab).is_some_and(|b| b.dot.blinks())
        })
        .map(|(_, ((rect, close), _))| tab_dot(*rect, *close, size.h))
        .collect()
}

/// What `pos` (strip coordinates) is over in the strip drawn from `view`,
/// `size` big: a tab's zoom mark too.
pub fn tab_strip_hit(
    view: &TabStripView,
    size: Size<i32, Logical>,
    pos: Point<i32, Logical>,
) -> Option<TabHit> {
    let layout = view.layout(size);
    let hit = layout.hit(pos.x, pos.y)?;
    if let TabHit::Tab(i) = hit
        && let Some(mark) = tab_zoom_rect(view, &layout, i, size.h)
        && mark.contains(pos)
    {
        return Some(TabHit::Unzoom(i));
    }
    Some(hit)
}

/// Where the zoom mark of tab `i` of `view`, laid out as `layout` on a
/// strip `h` high, goes, if it has one.
pub fn tab_zoom_rect(view: &TabStripView, layout: &TabLayout, i: usize, h: i32) -> Option<Rect> {
    tab_zoom_mark(
        view.tabs.get(i)?,
        *layout.tabs.get(i)?,
        *layout.closes.get(i)?,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CHIP_GAP, CLOSE, EDGE, PILL, PLUS_GAP, TAB_MAX_WIDTH, TAB_MIN_WIDTH, TabHit, TabStripView,
        TabView, WATCH_WIDTH, tab_blinkers, tab_layout, tab_layout_at, tab_rename_field, tab_slot,
        tab_strip_hit, tab_zoom_rect,
    };
    use crate::agent_icons::AgentIcon;
    use crate::chrome::{
        Density, Painter, Rect, Rgb, Theme, ThemeMode, inside, mix, tab_strip_height,
    };
    use crate::herdr::{
        AgentInfo, AgentStatus, PaneState, Process, ProcessInfo, TabDetail, TabInfo,
    };
    use crate::text_field::Line;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
    use smithay::utils::{Logical, Point};
    use std::path::PathBuf;

    fn pane(cwd: Option<&str>, focused: bool) -> PaneState {
        PaneState {
            pane_id: "p1".into(),
            terminal_id: "t1".into(),
            workspace_id: "w1".into(),
            tab_id: "tab1".into(),
            focused,
            cwd: cwd.map(PathBuf::from),
            foreground_cwd: None,
            agent: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Unknown,
            scroll: None,
            revision: 0,
            process: None,
            worktree: None,
            repo: None,
        }
    }

    fn process(pid: i32, name: &str, argv0: &str) -> Process {
        Process {
            pid,
            name: name.into(),
            argv: vec![argv0.into()],
            cmdline: argv0.into(),
            cwd: None,
        }
    }

    fn detail(panes: Vec<PaneState>, status: AgentStatus) -> TabDetail {
        TabDetail {
            tab: TabInfo {
                tab_id: "tab1".into(),
                number: 1,
                label: String::new(),
                focused: true,
                pane_count: panes.len() as u32,
                agent_status: status,
            },
            index: 0,
            panes,
        }
    }

    #[test]
    fn a_tab_is_titled_by_its_name_program_folder_or_window() {
        let title = |label: Option<&str>, window: Option<&str>, detail: Option<&TabDetail>| {
            TabView::derive(label, window, detail, &[]).title
        };
        // No herdr: the window's title, but never a terminal's app name.
        assert_eq!(title(None, Some("Alacritty"), None), "terminal");
        assert_eq!(
            title(None, Some("org.alacritty.Alacritty"), None),
            "terminal"
        );
        assert_eq!(
            title(None, Some("main.rs — gerdr"), None),
            "main.rs — gerdr"
        );
        assert_eq!(title(None, Some("✳ claude"), None), "claude");
        assert_eq!(title(None, None, None), "…");
        assert_eq!(title(Some("docs"), Some("Alacritty"), None), "docs");
        assert_eq!(title(Some("  "), Some("Alacritty"), None), "terminal");

        // With herdr: the focused pane's folder while its shell waits.
        let mut idle = pane(Some("/home/x/Personal/gerdr"), true);
        idle.process = Some(ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(10),
            foreground_processes: vec![process(10, "bash", "-bash")],
        });
        let d = detail(
            vec![pane(Some("/tmp/other"), false), idle.clone()],
            AgentStatus::Unknown,
        );
        assert_eq!(title(None, Some("Alacritty"), Some(&d)), "gerdr");
        assert_eq!(title(Some("docs"), Some("Alacritty"), Some(&d)), "docs");

        // The program in front wins over the folder.
        let mut busy = idle.clone();
        busy.process = Some(ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(20),
            foreground_processes: vec![process(20, "cargo", "/usr/bin/cargo")],
        });
        let d = detail(vec![busy.clone()], AgentStatus::Unknown);
        assert_eq!(title(None, Some("Alacritty"), Some(&d)), "cargo");
        // An agent by its name, ahead of its process.
        let mut agent = busy.clone();
        agent.agent = Some("claude".into());
        let d = detail(vec![agent], AgentStatus::Working);
        let view = TabView::derive(None, Some("Alacritty"), Some(&d), &[]);
        assert_eq!(view.title, "claude");
        assert_eq!(view.status, Some(AgentStatus::Working));
        assert_eq!(view.agent, Some(AgentIcon::Claude));
        // A pane with no folder yet falls back to the window.
        let d = detail(vec![pane(None, true)], AgentStatus::Unknown);
        assert_eq!(title(None, Some("Alacritty"), Some(&d)), "terminal");
        assert_eq!(title(None, Some("vim notes"), Some(&d)), "vim notes");
        // The root folder has a name too.
        let d = detail(vec![pane(Some("/"), true)], AgentStatus::Unknown);
        assert_eq!(title(None, None, Some(&d)), "/");
    }

    #[test]
    fn activity_folds_idle_shells_and_duplicates() {
        let shell = |pid| ProcessInfo {
            shell_pid: Some(pid),
            foreground_process_group_id: Some(pid),
            foreground_processes: vec![process(pid, "bash", "bash")],
        };
        let mut a = pane(Some("/w"), true);
        a.process = Some(shell(1));
        let mut b = pane(Some("/w"), false);
        b.process = Some(shell(2));
        let mut c = pane(Some("/w"), false);
        c.process = Some(ProcessInfo {
            shell_pid: Some(3),
            foreground_process_group_id: Some(4),
            foreground_processes: vec![process(4, "cargo", "cargo"), process(5, "rustc", "rustc")],
        });
        let d = detail(vec![a, b, c.clone(), c], AgentStatus::Unknown);
        let view = TabView::derive(None, None, Some(&d), &[]);
        assert_eq!(view.activity, "cargo | rustc");
        let mut agent = pane(Some("/w"), true);
        agent.agent = Some("claude".into());
        agent.agent_status = AgentStatus::Blocked;
        let mut info = agent_info();
        info.agent = Some("claude".into());
        info.name = Some("auth".into());
        info.agent_status = AgentStatus::Blocked;
        let agents = [info];
        let d = detail(vec![agent], AgentStatus::Blocked);
        let view = TabView::derive(None, None, Some(&d), &agents);
        assert_eq!(view.activity, "claude \"auth\" blocked");
        assert_eq!(view.tooltip(), "claude · /w");
    }

    fn agent_info() -> AgentInfo {
        serde_json::from_value(serde_json::json!({
            "pane_id": "p1",
            "terminal_id": "t1",
            "workspace_id": "w1",
            "tab_id": "tab1",
            "agent": null,
            "display_agent": null,
            "name": null,
            "agent_session": null,
            "cwd": null,
            "foreground_cwd": null,
            "title": null,
            "terminal_title": null,
            "terminal_title_stripped": null
        }))
        .expect("an agent with defaults")
    }

    fn strip(density: Density) -> smithay::utils::Size<i32, smithay::utils::Logical> {
        (900, tab_strip_height(density)).into()
    }

    #[test]
    fn the_strip_is_36_comfortable_and_30_compact() {
        assert_eq!(tab_strip_height(Density::Comfortable), 36);
        assert_eq!(tab_strip_height(Density::Compact), 30);
    }

    #[test]
    fn the_layout_tiles_the_strip_without_gaps_at_both_densities() {
        for density in [Density::Comfortable, Density::Compact] {
            let size = strip(density);
            let l = tab_layout(3, size, true, true, false, false);
            let h = size.h;
            // Tabs run from the left edge, each straight after the last,
            // the full height of the strip.
            assert_eq!(l.tabs[0].loc.x, 0);
            for pair in l.tabs.windows(2) {
                assert_eq!(pair[1].loc.x, pair[0].loc.x + pair[0].size.w);
            }
            assert!(
                l.tabs
                    .iter()
                    .all(|t| t.size.h == h && t.size.w == TAB_MAX_WIDTH.min(t.size.w))
            );
            // The "+" is one pill, vertically centred, right after the tabs.
            let last = l.tabs[2];
            assert_eq!(l.plus.size, (PILL, PILL).into());
            assert_eq!(l.plus.loc.y, (h - PILL) / 2);
            assert_eq!(l.plus.loc.x, last.loc.x + last.size.w + PLUS_GAP);
            // The switch keeps the right end; the chips sit left of it,
            // none overlapping.
            assert_eq!(l.toggle.size, (PILL, PILL).into());
            assert_eq!(l.toggle.loc.x + l.toggle.size.w, size.w - EDGE);
            let prefix = l.prefix.unwrap();
            let overflow = l.overflow.unwrap();
            assert!(prefix.loc.x + prefix.size.w < l.toggle.loc.x);
            assert!(overflow.loc.x + overflow.size.w < prefix.loc.x);
            assert!(l.plus.loc.x + l.plus.size.w < overflow.loc.x);
            // Each "×" is inside its tab, and inside the hit rect of a
            // pointer over it.
            for (tab, close) in l.tabs.iter().zip(&l.closes) {
                assert_eq!(close.size, (CLOSE, CLOSE).into());
                assert!(inside(*tab, *close));
                let (cx, cy) = (close.loc.x + 1, close.loc.y + 1);
                assert!(matches!(l.hit(cx, cy), Some(TabHit::Close(_))));
            }
        }
    }

    #[test]
    fn a_zoomed_tab_has_a_mark_that_unzooms_it() {
        let size = strip(Density::Comfortable);
        let zoomed = TabView {
            title: "zsh".into(),
            zoomed: true,
            ..Default::default()
        };
        let view = TabStripView::plain(
            vec![TabView::default(), zoomed],
            1,
            false,
            ThemeMode::System,
        );
        let l = tab_layout(2, size, false, false, false, false);
        assert_eq!(tab_zoom_rect(&view, &l, 0, size.h), None);
        let mark = tab_zoom_rect(&view, &l, 1, size.h).unwrap();
        // Inside its tab, before the "×" and clear of it.
        assert!(inside(l.tabs[1], mark));
        assert!(mark.loc.x + mark.size.w <= l.closes[1].loc.x);
        let mid = |r: Rect| -> Point<i32, Logical> {
            (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2).into()
        };
        assert_eq!(
            tab_strip_hit(&view, size, mid(mark)),
            Some(TabHit::Unzoom(1))
        );
        assert_eq!(
            tab_strip_hit(&view, size, mid(l.closes[1])),
            Some(TabHit::Close(1))
        );
        // The same place on a tab not zoomed is the tab.
        let shifted = mid(mark) - Point::from((l.tabs[1].loc.x - l.tabs[0].loc.x, 0));
        assert_eq!(tab_strip_hit(&view, size, shifted), Some(TabHit::Tab(0)));
        // A tab too narrow for a "×" has no mark either.
        let narrow = tab_layout(20, (400, 36).into(), false, false, false, false);
        let many = TabStripView::plain(vec![view.tabs[1].clone(); 20], 0, false, ThemeMode::System);
        assert_eq!(tab_zoom_rect(&many, &narrow, 3, 36), None);
        let painter = Painter::new();
        for hover in [None, Some(TabHit::Unzoom(1))] {
            let view = TabStripView {
                hover,
                hover_alpha: 16,
                ..view.clone()
            };
            painter.tab_strip_view(&view, Theme::Dark, size, 1.5);
        }
    }

    #[test]
    fn squeezed_tabs_make_room_for_the_chips() {
        let size = strip(Density::Comfortable);
        let free = tab_layout(6, size, false, false, false, false);
        let chips = tab_layout(6, size, true, true, false, false);
        assert!(chips.tabs[0].size.w < free.tabs[0].size.w);
        assert_eq!(chips.more, None);
        let last = chips.tabs[5];
        assert!(chips.plus.loc.x + PILL < chips.overflow.unwrap().loc.x);
        assert_eq!(chips.plus.loc.x, last.loc.x + last.size.w + PLUS_GAP);
        // The PREFIX badge's room is kept while it doesn't show: pressing
        // the prefix moves nothing.
        let armed = tab_layout(6, size, true, false, false, false);
        let unarmed = tab_layout(6, size, false, false, false, false);
        assert_eq!((armed.tabs, armed.plus), (unarmed.tabs, unarmed.plus));
        assert_eq!(unarmed.prefix, None);
        // Nothing to hit without tabs but the "+" and the switch; without
        // a workspace, not even the "+".
        let none = tab_layout(0, size, false, false, false, false);
        assert!(none.tabs.is_empty());
        let (x, y) = (none.plus.loc.x + 2, none.plus.loc.y + 2);
        assert_eq!(none.hit(x, y), Some(TabHit::Plus));
        assert_eq!(none.hit(size.w - EDGE - 2, 10), Some(TabHit::Toggle));
        let bare = tab_layout_at(0, 0, false, size, false, false, false, false, false);
        assert_eq!(bare.hit(x, y), None);
    }

    #[test]
    fn tabs_that_dont_fit_go_behind_the_more_chip_but_the_active_one() {
        let size = (400, 36).into();
        for active in [0, 1, 7, 19] {
            let l = tab_layout_at(20, active, true, size, false, false, false, false, false);
            let shown: Vec<usize> = (0..20).filter(|&i| l.tabs[i].size.w > 0).collect();
            assert!(!shown.is_empty() && shown.contains(&active), "{active}");
            assert_eq!(l.hidden().len(), 20 - shown.len());
            // Each shown one keeps its "×", and they run on from the left.
            for &i in &shown {
                assert!(l.tabs[i].size.w >= TAB_MIN_WIDTH);
                assert!(l.closes[i].size.w > 0);
            }
            assert_eq!(l.tabs[shown[0]].loc.x, 0);
            let mid = |r: Rect| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2);
            assert_eq!(l.hit(mid(l.tabs[active]).0, 10), Some(TabHit::Tab(active)));
            // The chip after them, then the "+", clear of the switch.
            let more = l.more.expect("a more chip");
            let end = l.tabs[*shown.last().unwrap()];
            assert!(more.loc.x >= end.loc.x + end.size.w);
            assert_eq!(l.hit(mid(more).0, mid(more).1), Some(TabHit::More));
            assert!(more.loc.x + more.size.w < l.plus.loc.x);
            assert!(l.plus.loc.x + PILL < l.toggle.loc.x);
        }
        let painter = Painter::new();
        let tabs = vec![TabView::default(); 20];
        let mut view = TabStripView::plain(tabs, 12, true, ThemeMode::System);
        view.hover = Some(TabHit::More);
        view.hover_alpha = 16;
        painter.tab_strip_view(&view, Theme::Light, size, 1.5);
    }

    #[test]
    fn hits_map_every_part_of_the_strip() {
        let size = strip(Density::Comfortable);
        let l = tab_layout(2, size, true, true, false, false);
        let mid = |r: Rect| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2);
        let at = |(x, y)| l.hit(x, y);
        assert_eq!(at(mid(l.tabs[0])), Some(TabHit::Tab(0)));
        assert_eq!(at(mid(l.tabs[1])), Some(TabHit::Tab(1)));
        // The border pixel belongs to the tab on its left.
        assert_eq!(at((l.tabs[1].loc.x - 1, 1)), Some(TabHit::Tab(0)));
        assert_eq!(at(mid(l.closes[1])), Some(TabHit::Close(1)));
        assert_eq!(at(mid(l.plus)), Some(TabHit::Plus));
        assert_eq!(at(mid(l.overflow.unwrap())), Some(TabHit::Overflow));
        assert_eq!(at(mid(l.prefix.unwrap())), Some(TabHit::Prefix));
        assert_eq!(at(mid(l.toggle)), Some(TabHit::Toggle));
        // Just above the "+" pill is nothing.
        assert_eq!(at((l.plus.loc.x + 2, l.plus.loc.y - 1)), None);
        assert_eq!(at((l.plus.loc.x + 2, size.h - 1)), None);
    }

    #[test]
    fn a_drag_finds_the_slot_under_the_pointer() {
        let strip = |count| tab_layout(count, (900, PILL).into(), false, false, false, false);
        assert_eq!(tab_slot(&strip(3), -5.0), 0);
        assert_eq!(tab_slot(&strip(3), 10.0), 0);
        assert_eq!(tab_slot(&strip(3), 300.0), 1);
        assert_eq!(tab_slot(&strip(3), 890.0), 2);
        assert_eq!(tab_slot(&strip(0), 100.0), 0);
    }

    /// The pixel at `(x, y)` of `buffer`.
    fn pixel(mut buffer: MemoryRenderBuffer, w: i32, x: i32, y: i32) -> Rgb {
        let mut out = [0; 3];
        let _ = buffer.render().draw(|mem| {
            let i = ((y * w + x) * 4) as usize;
            out = [mem[i + 2], mem[i + 1], mem[i]];
            Ok::<_, ()>(Vec::new())
        });
        out
    }

    #[test]
    fn hover_pills_are_painted_exactly_on_the_hit_rects() {
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let size = strip(Density::Comfortable);
        let tabs = vec![TabView::default(), TabView::default()];
        let mut view = TabStripView::plain(tabs, 0, false, ThemeMode::System);
        view.elsewhere = (2, 1);
        view.hover_alpha = 16;
        let l = tab_layout(2, size, false, true, false, false);
        let paint = |view: &TabStripView| painter.tab_strip_view(view, Theme::Dark, size, 1.0);
        let at = |view: &TabStripView, x, y| pixel(paint(view), size.w, x, y);

        // The "+" pill: lit inside its rect, the strip just outside it.
        view.hover = Some(TabHit::Plus);
        let lit = mix(p.element, p.selected, 1.0);
        assert_eq!(at(&view, l.plus.loc.x + 2, l.plus.loc.y + 2), lit);
        assert_eq!(at(&view, l.plus.loc.x - 1, l.plus.loc.y + 10), p.tab_bar);
        assert_eq!(at(&view, l.plus.loc.x + 10, l.plus.loc.y - 1), p.tab_bar);
        assert_eq!(at(&view, l.plus.loc.x + 10, l.plus.loc.y + PILL), p.tab_bar);

        // An inactive tab under the pointer: lit from its left edge to the
        // border on its right, which stays the border.
        view.hover = Some(TabHit::Tab(1));
        let tab = l.tabs[1];
        let lit = mix(p.tab_bar, p.element, 1.0);
        assert_eq!(at(&view, tab.loc.x, 2), lit);
        assert_eq!(at(&view, tab.loc.x + tab.size.w - 2, 2), lit);
        assert_eq!(at(&view, tab.loc.x + tab.size.w - 1, 2), p.border);
        assert_eq!(at(&view, tab.loc.x - 1, 2), p.border);
        // The active tab stays its own colour.
        assert_eq!(at(&view, 2, 2), p.tab_active);

        // The switch and the overflow chip light inside their rects only.
        view.hover = Some(TabHit::Toggle);
        let t = l.toggle;
        assert_eq!(
            at(&view, t.loc.x + 3, t.loc.y + 3),
            mix(p.tab_bar, p.element, 1.0)
        );
        assert_eq!(at(&view, t.loc.x - 1, t.loc.y + 10), p.tab_bar);
        view.hover = Some(TabHit::Overflow);
        let o = l.overflow.unwrap();
        assert_eq!(
            at(&view, o.loc.x + 3, o.loc.y + 10),
            mix(p.element, p.selected, 1.0)
        );
        assert_eq!(at(&view, o.loc.x - 1, o.loc.y + 10), p.tab_bar);
        // The warning dot sits on the chip.
        assert_eq!(at(&view, o.loc.x + o.size.w - 5, o.loc.y + 5), p.warning);

        // With no hover nothing is lit, at either scale or theme.
        view.hover = None;
        assert_eq!(at(&view, l.plus.loc.x + 2, l.plus.loc.y + 2), p.element);
        assert_eq!(at(&view, tab.loc.x, 2), p.tab_bar);
        painter.tab_strip_view(&view, Theme::Light, size, 2.0);
    }

    #[test]
    fn the_prefix_badge_and_rename_field_stay_off_the_tabs() {
        let painter = Painter::new();
        let p = Theme::Light.palette();
        let size = strip(Density::Compact);
        let tabs = vec![TabView::default(); 4];
        let mut view = TabStripView::plain(tabs, 1, true, ThemeMode::Light);
        let l = tab_layout(4, size, true, false, false, false);
        let badge = l.prefix.unwrap();
        let buffer = painter.tab_strip_view(&view, Theme::Light, size, 1.0);
        let at = |x, y| pixel(buffer.clone(), size.w, x, y);
        assert_eq!(
            at(badge.loc.x + badge.size.w / 2, badge.loc.y + 3),
            p.accent
        );
        assert_eq!(
            at(badge.loc.x - 2, badge.loc.y + badge.size.h / 2),
            p.tab_bar
        );
        let last = l.tabs[3];
        assert!(last.loc.x + last.size.w + PLUS_GAP + PILL < badge.loc.x);

        // Renaming tab 2 draws a field inside it, and clicks find it.
        view.renaming = Some((2, Line::from("docs")));
        let strip_rect = Rect::new((200, 0).into(), size);
        let field = tab_rename_field(&view, strip_rect, 1.0).unwrap();
        let mut inside_tab = l.tabs[2];
        inside_tab.loc.x += 200;
        assert!(inside(inside_tab, field.area));
        assert_eq!(field.line, Line::from("docs"));
        let buffer = painter.tab_strip_view(&view, Theme::Light, size, 1.0);
        let at = |x, y| pixel(buffer.clone(), size.w, x, y);
        let f = l.rename_field(2).unwrap();
        assert_eq!(at(f.loc.x + f.size.w / 2, f.loc.y + 2), p.element);
        assert_eq!(
            tab_rename_field(
                &TabStripView::plain(Vec::new(), 0, false, ThemeMode::Dark),
                strip_rect,
                1.0
            ),
            None
        );
    }

    #[test]
    fn the_watch_button_takes_room_left_of_the_chips_off_the_tabs() {
        let size = strip(Density::Comfortable);
        let l = tab_layout(5, size, true, true, true, false);
        let without = tab_layout(5, size, true, true, false, false);
        assert_eq!(without.watch, None);
        let watch = l.watch.expect("a watch button");
        assert_eq!(watch.size, (WATCH_WIDTH, PILL).into());
        assert_eq!(watch.loc.y, l.plus.loc.y);
        assert_eq!(
            watch.loc.x + watch.size.w + CHIP_GAP,
            l.overflow.unwrap().loc.x
        );
        // The tabs and the "+" make room for it.
        assert!(l.tabs[0].size.w < without.tabs[0].size.w);
        assert!(l.plus.loc.x + PILL < watch.loc.x);
        let last = l.tabs[4];
        assert!(last.loc.x + last.size.w < watch.loc.x);
        let mid = |r: Rect| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2);
        let (x, y) = mid(watch);
        assert_eq!(l.hit(x, y), Some(TabHit::Watch));
        assert_eq!(l.hit(watch.loc.x, y), Some(TabHit::Watch));
        assert_eq!(l.hit(watch.loc.x - 1, y), None);
        assert_eq!(l.hit(x, watch.loc.y - 1), None);
        let (x, y) = mid(l.overflow.unwrap());
        assert_eq!(l.hit(x, y), Some(TabHit::Overflow));
    }

    #[test]
    fn the_problems_button_sits_left_of_the_inbox_while_there_are_some() {
        let painter = Painter::new();
        let size = strip(Density::Comfortable);
        let mut view = TabStripView::plain(vec![TabView::default(); 3], 0, false, ThemeMode::Dark);
        view.inbox = Some(0);
        assert_eq!(view.layout(size).problems, None);
        view.problems = Some(2);
        let l = view.layout(size);
        let (button, inbox) = (l.problems.expect("a problems button"), l.inbox.unwrap());
        assert_eq!(button.size, (PILL, PILL).into());
        assert_eq!(button.loc.x + PILL + CHIP_GAP, inbox.loc.x);
        let mid = (button.loc.x + PILL / 2, button.loc.y + PILL / 2);
        assert_eq!(l.hit(mid.0, mid.1), Some(TabHit::Problems));
        // The "!" in the warning colour, in both looks, at its bar.
        for theme in [Theme::Dark, Theme::Light] {
            let buffer = painter.tab_strip_view(&view, theme, size, 2.0);
            let at = pixel(buffer, 2 * size.w, 2 * mid.0, 2 * mid.1 - 4);
            assert_eq!(at, theme.palette().warning, "{theme:?}");
        }
    }

    #[test]
    fn the_inbox_button_sits_by_the_switch() {
        let size = (1200, PILL).into();
        let l = tab_layout(8, size, true, true, true, true);
        let without = tab_layout(8, size, true, true, true, false);
        assert_eq!(without.inbox, None);
        let inbox = l.inbox.expect("an inbox button");
        assert_eq!(inbox.size, (PILL, PILL).into());
        assert_eq!(inbox.loc.x + PILL + CHIP_GAP, l.toggle.loc.x);
        assert_eq!(
            l.prefix.unwrap().loc.x + l.prefix.unwrap().size.w + CHIP_GAP,
            inbox.loc.x
        );
        // It takes room from the tabs, not from the other chips.
        assert!(l.tabs[0].size.w <= without.tabs[0].size.w);
        let mid = (inbox.loc.x + PILL / 2, inbox.loc.y + PILL / 2);
        assert_eq!(l.hit(mid.0, mid.1), Some(TabHit::Inbox));
    }

    #[test]
    fn the_watch_button_lights_on_its_hit_rect_and_fills_while_watching() {
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let size = strip(Density::Comfortable);
        let mut view = TabStripView::plain(vec![TabView::default()], 0, false, ThemeMode::System);
        view.watch = Some(false);
        view.hover_alpha = 16;
        let w = tab_layout(1, size, false, false, true, false)
            .watch
            .unwrap();
        let at = |view: &TabStripView, x, y| {
            pixel(
                painter.tab_strip_view(view, Theme::Dark, size, 1.0),
                size.w,
                x,
                y,
            )
        };
        let y = w.loc.y + w.size.h / 2;
        // Not watching: an outline around the strip's colour, the
        // element's under the pointer, inside the rect only.
        assert_eq!(at(&view, w.loc.x + 3, y), p.tab_bar);
        assert_eq!(at(&view, w.loc.x, y), p.border);
        view.hover = Some(TabHit::Watch);
        assert_eq!(at(&view, w.loc.x + 3, y), p.element);
        assert_eq!(at(&view, w.loc.x + w.size.w - 3, y), p.element);
        assert_eq!(at(&view, w.loc.x - 1, y), p.tab_bar);
        assert_eq!(at(&view, w.loc.x + w.size.w, y), p.tab_bar);
        // Watching: filled with the accent.
        view.watch = Some(true);
        view.hover = None;
        assert_eq!(at(&view, w.loc.x + 3, y), p.accent);
        assert_eq!(at(&view, w.loc.x - 1, y), p.tab_bar);
        // Hidden: nothing there.
        view.watch = None;
        assert_eq!(at(&view, w.loc.x + 3, y), p.tab_bar);
        painter.tab_strip_view(&view, Theme::Light, size, 2.0);
    }

    #[test]
    fn a_github_tab_carries_githubs_mark_unless_its_status_shows() {
        use crate::chrome::{PADDING, STATUS_WIDTH};
        let painter = Painter::new();
        let p = Theme::Dark.palette();
        let size = strip(Density::Comfortable);
        let l = tab_layout(2, size, false, false, false, false);
        // The mark's slot of tab `i`, before its number, off the strip's
        // bottom border.
        let slot = |tabs: Vec<TabView>, i: usize| -> Vec<Rgb> {
            let view = TabStripView::plain(tabs, 0, false, ThemeMode::Dark);
            let buffer = painter.tab_strip_view(&view, Theme::Dark, size, 1.0);
            let x = l.tabs[i].loc.x + PADDING as i32;
            (x..x + STATUS_WIDTH)
                .flat_map(|x| (1..size.h - 1).map(move |y| (x, y)))
                .map(|(x, y)| pixel(buffer.clone(), size.w, x, y))
                .collect()
        };
        let github = TabView {
            title: "PR #42".into(),
            github: true,
            ..TabView::default()
        };
        // Active and not: the mark shows, the number moves past it.
        let tabs = vec![github.clone(), github.clone()];
        assert!(slot(tabs.clone(), 0).iter().any(|c| *c != p.tab_active));
        assert!(slot(tabs, 1).iter().any(|c| *c != p.tab_bar));
        // An agent's status takes the slot.
        let working = TabView {
            status: Some(AgentStatus::Working),
            ..github.clone()
        };
        let plain = TabView {
            github: false,
            ..working.clone()
        };
        assert_eq!(
            slot(vec![working.clone(), working.clone()], 1),
            slot(vec![plain.clone(), plain], 1)
        );
        // Painted at twice the scale and in the light look too.
        let view = TabStripView::plain(vec![github, working], 1, false, ThemeMode::Light);
        painter.tab_strip_view(&view, Theme::Light, size, 2.0);
    }

    #[test]
    fn a_working_tab_blinks_its_dot_but_not_while_renamed() {
        let size = (900, tab_strip_height(Density::Comfortable)).into();
        let working = TabView {
            title: "gerdr".into(),
            status: Some(AgentStatus::Working),
            agent: Some(AgentIcon::Codex),
            ..TabView::default()
        };
        let done = TabView {
            status: Some(AgentStatus::Done),
            ..working.clone()
        };
        let mut view = TabStripView::plain(
            vec![done, working.clone(), working],
            0,
            false,
            ThemeMode::Dark,
        );
        let dots = tab_blinkers(&view, size);
        let l = tab_layout(3, size, false, false, false, false);
        assert_eq!(dots.len(), 2);
        assert!(inside(l.tabs[1], dots[0]) && inside(l.tabs[2], dots[1]));
        // At the tab's right end, before its "×".
        assert!(dots[0].loc.x > l.tabs[1].loc.x + l.tabs[1].size.w / 2);
        assert!(dots[0].loc.x + dots[0].size.w <= l.closes[1].loc.x);
        view.renaming = Some((1, Line::default()));
        assert_eq!(tab_blinkers(&view, size).len(), 1);
    }
}
