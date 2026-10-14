//! The workspace sidebar: the views of workspaces and agent cards, where
//! they go, and how they are drawn.

use super::{
    BAR_HEIGHT, BRANCH_ICON_GAP, BRANCH_ICON_WIDTH, CARD_GAP, CARD_HEADER, CARD_INDENT, CARD_LINE,
    CARD_PAD, CARD_RADIUS, CHEVRON_WIDTH, CLOSE_WIDTH, Canvas, DETAIL_FONT_SIZE, FIELD_GAP,
    FONT_SIZE, GAP, HEADER_BUTTON, HEADER_MARGIN, HEADER_NAME_END, HEADER_NAME_GAP,
    LABEL_FONT_SIZE, PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette, ROW_BOTTOM, ROW_HEIGHT,
    ROW_ICON_GAP, ROW_ICON_WIDTH, ROW_INSET, ROW_RADIUS, ROW_TITLE_HEIGHT, ROWS_TOP, Rect,
    SMALL_FONT_SIZE, STATUS_GAP, STATUS_WIDTH, TAB_LINE, TINY_FONT_SIZE, TabView, chevron_down,
    distinct, folder, folder_plus, git_branch, icon_sdf, mix, physical, status_mark, terminal,
    terminal_plus,
};
use crate::WorkspaceKind;
use crate::git::GitStatus;
use crate::herdr::{AgentInfo, AgentStatus, PaneState};
use crate::paths::{repo_root, short_path, tidy_title};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size};

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
pub(super) fn urgency(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
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

impl Painter {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sidebar(
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
    pub(super) fn agent_card(
        &self,
        c: &mut Canvas,
        card: &AgentView,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
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
}

#[cfg(test)]
mod tests {
    use super::WorkspaceView;
    use crate::WorkspaceKind;
    use crate::chrome::TabView;
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
}
