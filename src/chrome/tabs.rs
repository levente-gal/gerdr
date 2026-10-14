//! The tab strip: what each tab shows, where the tabs go, and how the
//! strip is drawn.

use super::{
    BUTTON_WIDTH, CHEVRON_WIDTH, Canvas, DETAIL_FONT_SIZE, GAP, LABEL_FONT_SIZE, NEW_TAB_INSET,
    NEW_TAB_SIZE, PADDING, Painter, Rect, SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, TAB_LINE,
    TAB_MAX_WIDTH, TAB_STRIP_HEIGHT, TAB_TITLE_HEIGHT, Theme, chevron_down, icon_sdf, moon,
    new_tab, no_restart, physical, status_label, status_mark, sun,
};
use crate::herdr::{AgentInfo, AgentStatus, PaneState, TabDetail};
use crate::paths::{repo_root, short_path, tidy_title};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size};

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

/// What runs in pane `p` of a tab titled `title`: its agent and its status,
/// else its foreground processes; its terminal title when that says more;
/// how far it is scrolled back.
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
pub(super) fn distinct(items: impl Iterator<Item = String>) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    (!seen.is_empty()).then(|| seen.join(", "))
}

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

/// The light/dark switch at the right end of a strip `width` wide (strip coordinates).
pub fn theme_toggle(width: i32) -> Rect {
    Rectangle::new(
        (width - BUTTON_WIDTH, 0).into(),
        (BUTTON_WIDTH, TAB_TITLE_HEIGHT).into(),
    )
}

/// Where a tab dragged to `x` goes, in a strip `width` wide holding `count`
/// tabs (strip coordinates): the place under `x`, the first or last beside them all.
pub fn tab_slot(count: usize, width: i32, x: f64) -> usize {
    let (tabs, _, _) = tab_layout(count, width);
    tabs.iter()
        .rposition(|(tab, _)| x >= tab.loc.x as f64)
        .unwrap_or(0)
}

impl Painter {
    /// The tab strip: per tab a title row (status mark, number, title, "×"),
    /// then what runs.
    pub(super) fn tab_strip(
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
}
