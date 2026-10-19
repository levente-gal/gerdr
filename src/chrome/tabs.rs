//! The tab strip: what each tab shows, where the tabs, the "+" button, the
//! chips at the right end and the theme switch go, and how the strip is
//! drawn. Every rect the layout hands out is the rect the painter draws,
//! so a click lands where the pill is.

use super::{
    Canvas, Density, FONT_SIZE, FieldSpot, GAP, LABEL_FONT_SIZE, NEW_TAB_SIZE, PADDING, Painter,
    Rect, Rgb, SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, STROKE, TAB_MAX_WIDTH, Theme, ThemeMode,
    cross, field_text, icon_sdf, mix, moon, new_tab, no_restart, physical, rounded_rect, segment,
    status_label, status_mark, sun, tab_strip_height,
};
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
    /// Whether its focused terminal's command runs again after a restart:
    /// `None` when nothing runs in it.
    pub restart: Option<bool>,
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
        }
    }

    /// The tooltip of its tab: the title, then where it is.
    pub fn tooltip(&self) -> String {
        let mut parts = vec![self.title.as_str()];
        if !self.place.is_empty() {
            parts.push(&self.place);
        }
        parts.extend(self.branch.as_deref());
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
    /// The "+" button.
    Plus,
    /// The chip saying other workspaces need attention.
    Overflow,
    /// The badge showing the prefix is armed.
    Prefix,
    /// The theme switch.
    Toggle,
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
        }
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
/// The "×" of a tab: its square, and its inset from the tab's right edge.
const CLOSE: i32 = 22;
const CLOSE_INSET: i32 = 5;
/// A tab narrower than this has no "×".
const CLOSE_MIN_TAB: i32 = 2 * PADDING as i32 + CLOSE + 40;
/// The rename field inside a tab: this far in from the tab's edges.
const RENAME_INSET: i32 = 4;

/// Where everything in a strip sits (strip coordinates): the tabs (each
/// one's rect runs up to and including its 1 px border on the right, so
/// the strip has no dead gaps), their "×" buttons (empty rects on tabs too
/// narrow for one), the "+" pill, the overflow chip and the PREFIX badge
/// when shown, and the theme switch at the right end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabLayout {
    pub tabs: Vec<Rect>,
    pub closes: Vec<Rect>,
    pub plus: Rect,
    pub overflow: Option<Rect>,
    pub prefix: Option<Rect>,
    pub toggle: Rect,
}

impl TabLayout {
    /// What is at `(x, y)` (strip coordinates), if anything. The "+" only
    /// shows beside tabs.
    pub fn hit(&self, x: i32, y: i32) -> Option<TabHit> {
        let inside = |r: &Rect| r.contains((x, y));
        if inside(&self.toggle) {
            return Some(TabHit::Toggle);
        }
        if self.overflow.as_ref().is_some_and(inside) {
            return Some(TabHit::Overflow);
        }
        if self.prefix.as_ref().is_some_and(inside) {
            return Some(TabHit::Prefix);
        }
        if let Some(i) = self.closes.iter().position(inside) {
            return Some(TabHit::Close(i));
        }
        if let Some(i) = self.tabs.iter().position(inside) {
            return Some(TabHit::Tab(i));
        }
        (!self.tabs.is_empty() && inside(&self.plus)).then_some(TabHit::Plus)
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

/// The layout of a strip of `size` holding `count` tabs, with the PREFIX
/// badge when `prefix` and the overflow chip when `overflow`. The chips
/// and the switch keep the right end; the tabs share what is left of the
/// "+", at most `TAB_MAX_WIDTH` each.
pub fn tab_layout(
    count: usize,
    size: Size<i32, Logical>,
    prefix: bool,
    overflow: bool,
) -> TabLayout {
    let (width, height) = (size.w, size.h);
    let top = (height - PILL) / 2;
    let toggle = Rectangle::new((width - EDGE - PILL, top).into(), (PILL, PILL).into());
    let mut right = toggle.loc.x;
    let prefix = prefix.then(|| {
        right -= CHIP_GAP + PREFIX_WIDTH;
        Rectangle::new(
            (right, (height - PREFIX_HEIGHT) / 2).into(),
            (PREFIX_WIDTH, PREFIX_HEIGHT).into(),
        )
    });
    let overflow = overflow.then(|| {
        right -= CHIP_GAP + OVERFLOW_WIDTH;
        Rectangle::new((right, top).into(), (OVERFLOW_WIDTH, PILL).into())
    });
    let room = (right - CHIP_GAP - PILL - PLUS_GAP).max(0);
    let tab_w = if count == 0 {
        0
    } else {
        (room / count as i32).min(TAB_MAX_WIDTH)
    };
    let tabs: Vec<Rect> = (0..count as i32)
        .map(|i| Rectangle::new((i * tab_w, 0).into(), (tab_w, height).into()))
        .collect();
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
        (count as i32 * tab_w + PLUS_GAP, top).into(),
        (PILL, PILL).into(),
    );
    TabLayout {
        tabs,
        closes,
        plus,
        overflow,
        prefix,
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
    let layout = tab_layout(view.tabs.len(), strip.size, view.prefix, view.overflow());
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

/// The theme mode's icon, as the switch shows it.
fn mode_icon(mode: ThemeMode) -> fn(f32, f32, f32) -> f32 {
    match mode {
        ThemeMode::System => monitor,
        ThemeMode::Dark => moon,
        ThemeMode::Light => sun,
    }
}

impl Painter {
    /// The tab strip: one line per tab (status mark, number, title, "×" on
    /// the active tab and under the pointer), the "+" pill after them, and
    /// at the right end the overflow chip, the PREFIX badge and the theme
    /// switch. A strip lower than the comfortable height is compact.
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

        let layout = tab_layout(view.tabs.len(), size, view.prefix, view.overflow());
        let band = (0, c.h);
        for (i, (rect, tab)) in layout.tabs.iter().zip(&view.tabs).enumerate() {
            let (x, w) = (
                px(rect.loc.x),
                px(rect.loc.x + rect.size.w) - px(rect.loc.x),
            );
            let is_active = i == view.active;
            let hovered = lit(TabHit::Tab(i)) || lit(TabHit::Close(i));
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
            // What runs in the tab won't run again after a restart: say so
            // before the "×".
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
            let mut text_x = x as f32 + pad;
            if let Some(status) = tab.status.filter(|s| {
                matches!(
                    s,
                    AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done
                )
            }) {
                let mark = Rectangle::new(
                    (rect.loc.x + PADDING as i32, 0).into(),
                    (STATUS_WIDTH, size.h).into(),
                );
                icon_sdf(&mut c, mark, scale, p.status(status), |x, y| {
                    status_mark(status, x, y, s)
                });
                text_x += ((STATUS_WIDTH + STATUS_GAP) as f32 * s).round();
            }
            let number = (i + 1).to_string();
            let after = self.text(
                &mut c,
                &number,
                font,
                text_x,
                right,
                band,
                scale,
                if is_active { p.muted } else { p.placeholder },
            );
            self.text(
                &mut c,
                &tab.title,
                font,
                after + px(8) as f32,
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
        if !view.tabs.is_empty() {
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

    /// A rounded pill filling `rect` (logical) with corners of `radius`.
    fn pill(&self, c: &mut Canvas, rect: Rect, radius: f32, color: Rgb, scale: f64) {
        let (x, y, w, h) = super::rect_px(rect, scale);
        c.rounded(x, y, w, h, radius * scale as f32, color);
    }
}

/// Where the strip's tabs are hit, for the `hover` of a `TabStripView`:
/// `pos` in strip coordinates.
pub fn tab_strip_hit(
    view: &TabStripView,
    size: Size<i32, Logical>,
    pos: Point<i32, Logical>,
) -> Option<TabHit> {
    tab_layout(view.tabs.len(), size, view.prefix, view.overflow()).hit(pos.x, pos.y)
}

#[cfg(test)]
mod tests {
    use super::{
        CLOSE, EDGE, PILL, PLUS_GAP, TAB_MAX_WIDTH, TabHit, TabStripView, TabView, tab_layout,
        tab_rename_field, tab_slot,
    };
    use crate::chrome::{
        Density, Painter, Rect, Rgb, Theme, ThemeMode, inside, mix, tab_strip_height,
    };
    use crate::herdr::{
        AgentInfo, AgentStatus, PaneState, Process, ProcessInfo, TabDetail, TabInfo,
    };
    use crate::text_field::Line;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
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
            let l = tab_layout(3, size, true, true);
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
    fn squeezed_tabs_make_room_for_the_chips_and_lose_their_close() {
        let size = strip(Density::Comfortable);
        let free = tab_layout(8, size, false, false);
        let chips = tab_layout(8, size, true, true);
        assert!(chips.tabs[0].size.w < free.tabs[0].size.w);
        let last = chips.tabs[7];
        assert!(chips.plus.loc.x + PILL < chips.overflow.unwrap().loc.x);
        assert_eq!(chips.plus.loc.x, last.loc.x + last.size.w + PLUS_GAP);
        let narrow = tab_layout(20, (400, 36).into(), false, false);
        assert!(narrow.closes.iter().all(|c| c.size.w == 0));
        assert!(matches!(
            narrow.hit(narrow.tabs[3].loc.x + 2, 10),
            Some(TabHit::Tab(3))
        ));
        // Nothing to hit at all without tabs, but the switch.
        let none = tab_layout(0, size, false, false);
        assert!(none.tabs.is_empty());
        assert_eq!(none.hit(none.plus.loc.x + 2, none.plus.loc.y + 2), None);
        assert_eq!(none.hit(size.w - EDGE - 2, 10), Some(TabHit::Toggle));
    }

    #[test]
    fn hits_map_every_part_of_the_strip() {
        let size = strip(Density::Comfortable);
        let l = tab_layout(2, size, true, true);
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
        let strip = |count| tab_layout(count, (900, PILL).into(), false, false);
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
        let l = tab_layout(2, size, false, true);
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
        let l = tab_layout(4, size, true, false);
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
}
