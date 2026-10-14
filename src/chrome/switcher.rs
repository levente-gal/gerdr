//! The switcher: its rows, where they go, and how it is drawn.

use super::{
    CARET_WIDTH, Canvas, FIELD_GAP, FONT_SIZE, FieldSpot, HELP_MARGIN, HELP_PADDING, HELP_RADIUS,
    MENU_RADIUS, PADDING, Painter, Palette, ROW_ICON_GAP, ROW_ICON_WIDTH, Rect, SHADOW,
    SMALL_FONT_SIZE, SWITCHER_INPUT, SWITCHER_PADDING, SWITCHER_ROW, SWITCHER_ROWS, SWITCHER_TOP,
    SWITCHER_WIDTH, app, folder, folder_plus, icon_sdf, physical, status_mark, tab, terminal,
};
use crate::WorkspaceKind;
use crate::app_icons::AppIcon;
use crate::herdr::AgentStatus;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

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
pub(super) fn switcher_parts(
    rows: usize,
    loc: Point<i32, Logical>,
    w: i32,
) -> (Rect, Rect, Vec<Rect>) {
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

/// Where the text of the switcher's query, in its `input` rect, goes.
pub(super) fn switcher_text(input: Rect, scale: f64) -> (f32, f32) {
    let px = |v: i32| (v as f64 * scale).round() as f32;
    (
        px(input.loc.x + HELP_PADDING),
        px(input.loc.x + input.size.w - HELP_PADDING) - px(CARET_WIDTH + 1),
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

impl Painter {
    /// The switcher `w` wide inside a margin of `SHADOW` holding its drop
    /// shadow: the query (or what to type) with a caret, then the rows.
    pub(super) fn switcher(
        &self,
        view: &SwitcherView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
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
}
