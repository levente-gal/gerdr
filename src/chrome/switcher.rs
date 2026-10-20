//! The switcher: its rows, where they go, and how it is drawn.
//!
//! Under the query, a row of category chips ("All", then a chip per
//! section listed) narrows the rows to one section. The rows come in
//! sections (Workspaces, Tabs, Agents, …), each under an upper case
//! heading. A row has two lines: what it is on the first, more about it in
//! muted text on the second, with a small pill at the right naming its
//! kind. The key hints go in a footer, as in the project menu.

use super::{
    BUTTON_RADIUS, CARET_WIDTH, Canvas, FONT_SIZE, FOOTER_PAD, FOOTER_ROW, FieldSpot, HELP_MARGIN,
    HELP_PADDING, HELP_RADIUS, MENU_RADIUS, PADDING, PILL_HEIGHT, PILL_PADDING, Painter, Palette,
    ROW_ICON_GAP, ROW_ICON_WIDTH, ROW_PICTURE_SIZE, Rect, SHADOW, SMALL_FONT_SIZE, SWITCHER_INPUT,
    SWITCHER_PADDING, SWITCHER_ROWS, SWITCHER_SECTION, SWITCHER_TOP, SWITCHER_WIDTH,
    TINY_FONT_SIZE, app, folder, folder_plus, footer_height, footer_rows, icon_sdf, mix, pencil,
    physical, prompt, rect_px, status_mark, tab, terminal,
};
use crate::WorkspaceKind;
use crate::app_icons::{AppIcon, github_icon};
use crate::herdr::AgentStatus;
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// A row: two lines of text.
const ROW_HEIGHT: i32 = 40;
/// A section's heading: the gap above it and its text.
const SECTION_HEIGHT: i32 = SWITCHER_SECTION;
/// The heading's text sits in the lower part of its band.
const SECTION_TEXT: i32 = 22;
/// The gap between the two lines' bands and the row's edges.
const LINE_INSET: i32 = 4;
/// The first line's band; the second takes the rest.
const LABEL_LINE: i32 = 18;
/// The pill at the right of a row and the gap before it.
const PILL_GAP: i32 = 10;
/// A category chip's height, and the gap between chips.
const CHIP_HEIGHT: i32 = 24;
const CHIP_GAP: i32 = 4;
/// A chip's room per character of its label, and around the label: an
/// estimate of the text's width, as the layout has no font to measure with.
const CHIP_CHAR: i32 = 7;
const CHIP_INSET: i32 = 12;
/// The row of chips: a chip and the gap under it.
const CHIPS_ROW: i32 = CHIP_HEIGHT + SWITCHER_PADDING;

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
    /// Something gerdr does itself: the shortcuts, the project menu.
    Command,
    /// A file to edit: the settings.
    Settings,
}

/// One row of the switcher, with the positions (in chars) of the characters
/// the query matched in its label and detail. `section` is set on the first
/// row of each section shown: the heading drawn above it.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherRow {
    pub icon: ItemIcon,
    pub label: String,
    pub detail: String,
    /// What kind of thing it is, for the pill at the right.
    pub kind: &'static str,
    pub section: Option<&'static str>,
    pub label_hits: Vec<usize>,
    pub detail_hits: Vec<usize>,
}

/// What the switcher shows: the query (or `hint` without one), the
/// category chips, the rows in view, which of them is selected (none when
/// nothing matches), which the pointer is over, and the key hints.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherView {
    pub query: Line,
    pub hint: &'static str,
    /// The category chips' labels, "All" first; none without a choice.
    pub categories: Vec<&'static str>,
    /// The chip on, by index into `categories`.
    pub category: usize,
    /// The chip under the pointer.
    pub category_hover: Option<usize>,
    pub rows: Vec<SwitcherRow>,
    pub selected: Option<usize>,
    pub hover: Option<usize>,
    /// The footer's key hints, as (key, what it does).
    pub hints: Vec<(String, String)>,
}

/// Where the switcher's parts go: the panel, its query field, a rect per
/// category chip, a rect per row shown, the section headings with their
/// text, and the footer.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherLayout {
    pub panel: Rect,
    pub input: Rect,
    pub chips: Vec<Rect>,
    pub rows: Vec<Rect>,
    pub sections: Vec<(Rect, &'static str)>,
    pub footer: Rect,
}

/// Where the icon of the switcher row in `row` goes.
pub fn switcher_icon(row: Rect) -> Rect {
    Rectangle::new(
        (row.loc.x + PADDING as i32, row.loc.y).into(),
        (ROW_ICON_WIDTH, row.size.h).into(),
    )
}

/// The switcher showing `view` in the app `area` (window coordinates).
pub fn switcher_geometry(view: &SwitcherView, area: Rect) -> SwitcherLayout {
    switcher_parts(view, panel_origin(area), panel_width(area))
}

/// The switcher showing `rows` rows without any section heading, category
/// chip or key hint in the app `area`: the panel, its query field and the
/// rows, for tests with only a count (`switcher_geometry` knows the rest).
#[cfg(test)]
pub fn switcher_layout(rows: usize, area: Rect) -> (Rect, Rect, Vec<Rect>) {
    let layout = parts(
        &vec![None; rows],
        &[],
        0,
        panel_origin(area),
        panel_width(area),
    );
    (layout.panel, layout.input, layout.rows)
}

fn panel_width(area: Rect) -> i32 {
    SWITCHER_WIDTH.min(area.size.w - 2 * HELP_MARGIN).max(1)
}

fn panel_origin(area: Rect) -> Point<i32, Logical> {
    (
        area.loc.x + (area.size.w - panel_width(area)) / 2,
        area.loc.y + SWITCHER_TOP,
    )
        .into()
}

/// How many rows the switcher shows at most in the app `area`, keeping
/// room for the headings of `sections` sections, the category chips (when
/// there are `chips`) and the footer.
pub fn switcher_page(area: Rect, sections: usize, chips: bool) -> usize {
    let room = area.size.h
        - SWITCHER_TOP
        - SWITCHER_INPUT
        - 2 * SWITCHER_PADDING
        - HELP_MARGIN
        - if chips { CHIPS_ROW } else { 0 }
        - footer_height(SWITCHER_HINTS);
    let headings = (sections.min(SWITCHER_ROWS as usize) as i32) * SECTION_HEIGHT;
    ((room - headings) / ROW_HEIGHT).clamp(1, SWITCHER_ROWS) as usize
}

/// The most key hints the switcher shows: one footer row's worth.
pub const SWITCHER_HINTS: usize = 4;

/// `switcher_geometry` for a panel `w` wide at `loc`.
pub(super) fn switcher_parts(
    view: &SwitcherView,
    loc: Point<i32, Logical>,
    w: i32,
) -> SwitcherLayout {
    let sections: Vec<_> = view.rows.iter().map(|row| row.section).collect();
    parts(&sections, &view.categories, view.hints.len(), loc, w)
}

/// The chips of `labels` in a row from `x` to `right` at `y`: each as
/// wide as its label needs, all narrowed alike when they don't fit.
fn chip_rects(labels: &[&str], x: i32, y: i32, right: i32) -> Vec<Rect> {
    let natural: Vec<i32> = labels
        .iter()
        .map(|label| label.chars().count() as i32 * CHIP_CHAR + 2 * CHIP_INSET)
        .collect();
    let gaps = CHIP_GAP * (labels.len() as i32 - 1).max(0);
    let room = (right - x - gaps).max(0);
    let total: i32 = natural.iter().sum();
    let mut left = x;
    natural
        .into_iter()
        .map(|w| {
            let w = if total > room { w * room / total } else { w };
            let rect = Rectangle::new((left, y).into(), (w, CHIP_HEIGHT).into());
            left += w + CHIP_GAP;
            rect
        })
        .collect()
}

/// The panel `w` wide at `loc`, with the chips of `categories` (none
/// without any), a row per entry of `sections` (the heading over it, if
/// any) and a footer for `hints` key hints; it keeps room for one row
/// without any.
fn parts(
    sections: &[Option<&'static str>],
    categories: &[&str],
    hints: usize,
    loc: Point<i32, Logical>,
    w: i32,
) -> SwitcherLayout {
    let mut y = loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
    let x = loc.x + SWITCHER_PADDING;
    let row_w = w - 2 * SWITCHER_PADDING;
    let chips = chip_rects(categories, x, y, x + row_w);
    if !chips.is_empty() {
        y += CHIPS_ROW;
    }
    let mut rows = Vec::with_capacity(sections.len());
    let mut headings = Vec::new();
    for section in sections {
        if let Some(label) = section {
            headings.push((
                Rectangle::new((x, y).into(), (row_w, SECTION_HEIGHT).into()),
                *label,
            ));
            y += SECTION_HEIGHT;
        }
        rows.push(Rectangle::new((x, y).into(), (row_w, ROW_HEIGHT).into()));
        y += ROW_HEIGHT;
    }
    if sections.is_empty() {
        y += ROW_HEIGHT;
    }
    let footer_h = footer_height(hints);
    let footer = Rectangle::new((loc.x, y + SWITCHER_PADDING).into(), (w, footer_h).into());
    let h = y + SWITCHER_PADDING + footer_h - loc.y;
    SwitcherLayout {
        panel: Rectangle::new(loc, (w, h).into()),
        input: Rectangle::new(loc, (w, SWITCHER_INPUT).into()),
        chips,
        rows,
        sections: headings,
        footer,
    }
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
    let input = switcher_geometry(view, area).input;
    FieldSpot {
        size: FONT_SIZE,
        area: input,
        text: switcher_text(input, scale),
        line: view.query.clone(),
    }
}

impl Painter {
    /// The switcher `w` wide inside a margin of `SHADOW` holding its drop
    /// shadow: the query (or what to type) with a caret, the category
    /// chips, the rows under their section headings, then the key hints.
    pub(super) fn switcher(
        &self,
        view: &SwitcherView,
        w: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = switcher_parts(view, (SHADOW, SHADOW).into(), w);
        let (panel, input) = (layout.panel, layout.input);
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
            FONT_SIZE,
            scale,
            p,
        );
        let divider = |c: &mut Canvas, y: i32| {
            c.fill(
                px(panel.loc.x) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };
        divider(&mut c, px(input.loc.y + input.size.h) - border);

        for (i, (rect, label)) in layout.chips.iter().zip(&view.categories).enumerate() {
            self.category_chip(&mut c, view, i, label, *rect, scale, p);
        }

        if view.rows.is_empty() {
            let empty = parts(
                &[None],
                &view.categories,
                view.hints.len(),
                (SHADOW, SHADOW).into(),
                w,
            );
            let slot = empty.rows[0];
            let band = (px(slot.loc.y), px(slot.size.h));
            self.text(
                &mut c,
                "Nothing matches",
                FONT_SIZE,
                px(slot.loc.x) as f32 + PADDING * s,
                px(input.loc.x + input.size.w - HELP_PADDING) as f32,
                band,
                scale,
                p.placeholder,
            );
        }
        for (rect, label) in &layout.sections {
            let band = (
                px(rect.loc.y + rect.size.h - SECTION_TEXT),
                px(SECTION_TEXT),
            );
            self.text(
                &mut c,
                &label.to_uppercase(),
                TINY_FONT_SIZE,
                px(rect.loc.x) as f32 + PADDING * s,
                px(rect.loc.x + rect.size.w) as f32 - PADDING * s,
                band,
                scale,
                p.placeholder,
            );
        }
        for (i, (slot, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            let (x, y, w, h) = (
                px(slot.loc.x),
                px(slot.loc.y),
                px(slot.size.w),
                px(slot.size.h),
            );
            let fill = if view.selected == Some(i) {
                Some(p.selected)
            } else if view.hover == Some(i) {
                Some(p.element)
            } else {
                None
            };
            if let Some(fill) = fill {
                c.rounded(
                    x as f32,
                    y as f32,
                    w as f32,
                    h as f32,
                    (MENU_RADIUS - 2.0) * s,
                    fill,
                );
            }
            let icon = switcher_icon(*slot);
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
                ItemIcon::Workspace(kind @ (WorkspaceKind::Cursor | WorkspaceKind::GitHub)) => {
                    // GitHub's mark for its workspaces, as the sidebar shows them.
                    let mark = github_icon().filter(|_| *kind == WorkspaceKind::GitHub);
                    match mark {
                        Some(mark) => {
                            c.picture_sized(&mark, icon, scale, p.placeholder, ROW_PICTURE_SIZE)
                        }
                        None => {
                            icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| app(x, y, small))
                        }
                    }
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
                ItemIcon::Command => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    prompt(x, y, small)
                }),
                ItemIcon::Settings => icon_sdf(&mut c, icon, scale, p.placeholder, |x, y| {
                    pencil(x, y, small)
                }),
            }

            // The kind pill at the right; the text stops before it.
            let pill_w =
                self.text_width(row.kind, TINY_FONT_SIZE, scale) + 2.0 * PILL_PADDING as f32 * s;
            let pill_x = ((x + w) as f32 - PADDING * s - pill_w).round();
            let pill_h = px(PILL_HEIGHT) as f32;
            let pill_y = (y as f32 + (h as f32 - pill_h) / 2.0).round();
            c.rounded(
                pill_x,
                pill_y,
                pill_w.round(),
                pill_h,
                pill_h / 2.0,
                mix(fill.unwrap_or(p.elevated), p.muted, 0.18),
            );
            self.text(
                &mut c,
                row.kind,
                TINY_FONT_SIZE,
                pill_x + PILL_PADDING as f32 * s,
                f32::MAX,
                (pill_y as i32, pill_h as i32),
                scale,
                p.muted,
            );

            let right = pill_x - px(PILL_GAP) as f32;
            let text_x = px(icon.loc.x + ROW_ICON_WIDTH + ROW_ICON_GAP) as f32;
            let label_band = (y + px(LINE_INSET), px(LABEL_LINE));
            self.marked(
                &mut c,
                &row.label,
                &row.label_hits,
                FONT_SIZE,
                text_x,
                right,
                label_band,
                scale,
                (p.text, p.accent),
            );
            let detail_top = y + px(LINE_INSET + LABEL_LINE);
            let detail_band = (detail_top, y + h - px(LINE_INSET) - detail_top);
            self.marked(
                &mut c,
                &row.detail,
                &row.detail_hits,
                SMALL_FONT_SIZE,
                text_x,
                right,
                detail_band,
                scale,
                (p.muted, p.accent),
            );
        }

        let footer = layout.footer;
        divider(&mut c, px(footer.loc.y));
        let hints: Vec<(&str, &str)> = view
            .hints
            .iter()
            .map(|(keys, action)| (keys.as_str(), action.as_str()))
            .collect();
        let inner_w = (footer.size.w - 2 * HELP_PADDING).max(0);
        let rows = self.hint_rows(&hints, inner_w, scale, footer_rows(hints.len()) as usize);
        for (i, row) in rows.into_iter().enumerate() {
            let inner = Rectangle::new(
                (
                    footer.loc.x + HELP_PADDING,
                    footer.loc.y + FOOTER_PAD + i as i32 * FOOTER_ROW,
                )
                    .into(),
                (inner_w, FOOTER_ROW).into(),
            );
            self.hints(&mut c, &hints[row], inner, scale, p);
        }
        c.into_buffer(false)
    }

    /// Category chip `i`, `label`, in `rect`: an outlined pill, filled and
    /// outlined in the accent when it is the category shown, lighter under
    /// the pointer.
    #[allow(clippy::too_many_arguments)]
    fn category_chip(
        &self,
        c: &mut Canvas,
        view: &SwitcherView,
        i: usize,
        label: &str,
        rect: Rect,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        let (x, y, w, h) = rect_px(rect, scale);
        let b = ((scale.round() as i32).max(1)) as f32;
        let r = BUTTON_RADIUS * s;
        let on = view.category == i;
        let fill = if on {
            p.selected
        } else if view.category_hover == Some(i) {
            mix(p.element, p.selected, 0.5)
        } else {
            p.element
        };
        c.rounded(x, y, w, h, r, if on { p.accent } else { p.border });
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
        let color = if on { p.text } else { p.muted };
        self.centered(c, label, SMALL_FONT_SIZE, rect, scale, color);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHIP_GAP, CHIP_HEIGHT, CHIPS_ROW, ItemIcon, ROW_HEIGHT, SECTION_HEIGHT, SWITCHER_INPUT,
        SWITCHER_PADDING, SwitcherRow, SwitcherView, chip_rects, footer_height, switcher_geometry,
        switcher_layout, switcher_page,
    };
    use smithay::utils::Rectangle;

    fn row(section: Option<&'static str>) -> SwitcherRow {
        SwitcherRow {
            icon: ItemIcon::Tab,
            label: "a".into(),
            detail: "b".into(),
            kind: "tab",
            section,
            label_hits: Vec::new(),
            detail_hits: Vec::new(),
        }
    }

    #[test]
    fn headings_take_their_own_band_above_the_first_row_of_a_section() {
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let view = SwitcherView {
            query: "".into(),
            hint: "",
            categories: Vec::new(),
            category: 0,
            category_hover: None,
            rows: vec![row(Some("Workspaces")), row(None), row(Some("Apps"))],
            selected: Some(0),
            hover: None,
            hints: Vec::new(),
        };
        let layout = switcher_geometry(&view, area);
        let top = layout.panel.loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
        assert_eq!(layout.sections.len(), 2);
        assert_eq!(layout.sections[0].0.loc.y, top);
        assert_eq!(layout.sections[0].1, "Workspaces");
        assert_eq!(layout.rows[0].loc.y, top + SECTION_HEIGHT);
        assert_eq!(layout.rows[1].loc.y, top + SECTION_HEIGHT + ROW_HEIGHT);
        assert_eq!(
            layout.sections[1].0.loc.y,
            top + SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        assert_eq!(
            layout.rows[2].loc.y,
            top + 2 * SECTION_HEIGHT + 2 * ROW_HEIGHT
        );
        let footer = footer_height(0);
        assert_eq!(
            layout.panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 2 * SECTION_HEIGHT + 3 * ROW_HEIGHT + footer
        );
        // The footer closes the panel.
        assert_eq!(
            layout.footer.loc.y + layout.footer.size.h,
            layout.panel.loc.y + layout.panel.size.h
        );
        // The same rows without headings, from the count alone.
        let (panel, input, rows) = switcher_layout(3, area);
        assert_eq!(
            panel.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + 3 * ROW_HEIGHT + footer
        );
        assert_eq!((panel.loc, input.loc), (layout.panel.loc, layout.input.loc));
        assert_eq!(rows[0].loc.y, top);
        // An empty list keeps room for the "Nothing matches" row.
        assert_eq!(
            switcher_layout(0, area).0.size.h,
            SWITCHER_INPUT + 2 * SWITCHER_PADDING + ROW_HEIGHT + footer
        );
    }

    #[test]
    fn the_category_chips_go_between_the_query_and_the_rows() {
        let area = Rectangle::new((200, 36).into(), (1000, 800).into());
        let view = SwitcherView {
            query: "".into(),
            hint: "",
            categories: vec!["All", "Workspaces", "Apps"],
            category: 0,
            category_hover: None,
            rows: vec![row(Some("Workspaces"))],
            selected: Some(0),
            hover: None,
            hints: vec![("Esc".into(), "close".into())],
        };
        let layout = switcher_geometry(&view, area);
        let top = layout.panel.loc.y + SWITCHER_INPUT + SWITCHER_PADDING;
        assert_eq!(layout.chips.len(), 3);
        assert!(layout.chips.iter().all(|chip| chip.loc.y == top));
        // Wider for a longer label, in order, apart.
        assert!(layout.chips[1].size.w > layout.chips[0].size.w);
        for pair in layout.chips.windows(2) {
            assert_eq!(pair[0].loc.x + pair[0].size.w + CHIP_GAP, pair[1].loc.x);
        }
        assert_eq!(layout.sections[0].0.loc.y, top + CHIPS_ROW);
        assert_eq!(layout.rows[0].loc.y, top + CHIPS_ROW + SECTION_HEIGHT);
    }

    #[test]
    fn chips_too_wide_together_are_narrowed_to_fit() {
        let labels = [
            "All",
            "Workspaces",
            "Tabs",
            "Agents",
            "Projects",
            "Apps",
            "Folders",
        ];
        let chips = chip_rects(&labels, 10, 0, 310);
        let last = chips.last().unwrap();
        assert!(last.loc.x + last.size.w <= 310);
        assert!(chips.iter().all(|chip| chip.size.h == CHIP_HEIGHT));
        assert!(chip_rects(&[], 0, 0, 100).is_empty());
    }

    #[test]
    fn the_page_keeps_room_for_the_headings() {
        let area = Rectangle::new((0, 0).into(), (1000, 600).into());
        assert!(switcher_page(area, 7, false) < switcher_page(area, 1, false));
        assert!(switcher_page(area, 1, true) <= switcher_page(area, 1, false));
        assert!(switcher_page(area, 7, true) >= 1);
        assert_eq!(
            switcher_page(Rectangle::new((0, 0).into(), (1000, 100).into()), 7, true),
            1
        );
        assert_eq!(
            switcher_page(Rectangle::new((0, 0).into(), (1000, 2000).into()), 7, true),
            12
        );
    }
}
