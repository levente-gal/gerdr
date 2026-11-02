//! The footer along the bottom of the window, under everything else: a
//! chip per background task (T3 Code's server, a git fetch, an app kept
//! running...) with its dot, and at its right end how many there are. A
//! click opens the tasks panel over it: each task with what it is up to
//! and buttons to act on it.

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{
    BUTTON_RADIUS, DETAIL_FONT_SIZE, DOT, Density, Dot, FONT_SIZE, HELP_RADIUS, LABEL_FONT_SIZE,
    Painter, Palette, Rect, SHADOW, SMALL_FONT_SIZE, band, dot_shape, icon_sdf, measure, mix, phys,
    physical, rect_px,
};

/// How high the footer is at `density`.
pub fn status_bar_height(density: Density) -> i32 {
    match density {
        Density::Comfortable => 24,
        Density::Compact => 20,
    }
}

/// Room left of the first chip and right of the count.
const EDGE: i32 = 8;
/// A chip's room around its dot and label, and between chips.
const CHIP_PAD: i32 = 8;
const CHIP_GAP: i32 = 2;
const DOT_GAP: i32 = 6;
/// The widest a chip's label gets.
const CHIP_MAX: i32 = 220;
/// The chips stop this far before the count.
const COUNT_GAP: i32 = 16;

/// A background task, as the footer and the tasks panel show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskView {
    pub title: String,
    /// What its footer chip says.
    pub chip: String,
    /// What it is up to: a version and port, what runs.
    pub detail: String,
    pub dot: Dot,
    /// Its buttons' labels, in order.
    pub buttons: Vec<String>,
}

/// What the footer shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FooterView {
    pub tasks: Vec<TaskView>,
    pub hover: Option<FooterHit>,
}

/// What the pointer is over in the footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterHit {
    /// A task's chip, by index.
    Task(usize),
    /// The count at the right end, with the chips left out.
    Count,
}

/// Where the footer's parts go, in footer coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FooterLayout {
    /// One per task shown, in order: the first tasks that fit.
    pub chips: Vec<Rect>,
    pub count: Rect,
}

fn text_w(text: &str, size: f32) -> i32 {
    measure(text, size)
        .unwrap_or(text.chars().count() as f32 * size * 0.6)
        .ceil() as i32
}

/// What the count at the right end says.
fn count_text(tasks: usize) -> String {
    match tasks {
        0 => "No background tasks".to_owned(),
        1 => "1 background task".to_owned(),
        n => format!("{n} background tasks"),
    }
}

/// Where the footer's chips and count go on a footer of `size`.
pub fn footer_layout(view: &FooterView, size: Size<i32, Logical>) -> FooterLayout {
    let h = size.h;
    let count_w = text_w(&count_text(view.tasks.len()), SMALL_FONT_SIZE) + 2 * CHIP_PAD;
    let count = Rectangle::new(
        ((size.w - EDGE - count_w).max(0), 0).into(),
        (count_w, h).into(),
    );
    let end = count.loc.x - COUNT_GAP;
    let mut x = EDGE;
    let mut chips = Vec::new();
    for task in &view.tasks {
        let label = text_w(&task.chip, SMALL_FONT_SIZE).min(CHIP_MAX);
        let w = CHIP_PAD + DOT + DOT_GAP + label + CHIP_PAD;
        if x + w > end {
            break;
        }
        chips.push(Rectangle::new((x, 0).into(), (w, h).into()));
        x += w + CHIP_GAP;
    }
    FooterLayout { chips, count }
}

impl FooterLayout {
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<FooterHit> {
        if let Some(i) = self.chips.iter().position(|r| r.to_f64().contains(pos)) {
            return Some(FooterHit::Task(i));
        }
        self.count
            .to_f64()
            .contains(pos)
            .then_some(FooterHit::Count)
    }
}

/// Where a chip's dot goes.
fn chip_dot(chip: Rect) -> Rect {
    Rectangle::new(
        (chip.loc.x + CHIP_PAD, chip.loc.y + (chip.size.h - DOT) / 2).into(),
        (DOT, DOT).into(),
    )
}

/// The dots that blink, in footer coordinates: the busy tasks'.
pub fn footer_blinkers(view: &FooterView, size: Size<i32, Logical>) -> Vec<Rect> {
    let layout = footer_layout(view, size);
    layout
        .chips
        .iter()
        .zip(&view.tasks)
        .filter(|(_, task)| task.dot.blinks())
        .map(|(chip, _)| chip_dot(*chip))
        .collect()
}

/// Paint `dot` at `spot`, blinking or not: a still one.
fn paint_dot(c: &mut super::Canvas, spot: Rect, dot: Dot, scale: f64, p: &Palette) {
    let d = DOT as f32 * scale as f32;
    icon_sdf(c, spot, scale, dot.color(p), |x, y| dot_shape(dot, x, y, d));
}

impl Painter {
    /// The footer, `size` big: a line on top, the chips, the count.
    pub(super) fn footer(
        &self,
        view: &FooterView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = super::Canvas::new(physical(size, scale));
        let (w, h) = (
            (size.w as f64 * scale).round() as i32,
            (size.h as f64 * scale).round() as i32,
        );
        c.fill(0, 0, w, h, p.tab_bar);
        c.fill(0, 0, w, (scale.round() as i32).max(1), p.border);
        let layout = footer_layout(view, size);
        let s = scale as f32;
        let pill = |c: &mut super::Canvas, rect: Rect, color| {
            let inner = Rectangle::new(
                (rect.loc.x, rect.loc.y + 3).into(),
                (rect.size.w, rect.size.h - 5).into(),
            );
            let (x, y, w, h) = rect_px(inner, scale);
            c.rounded(x, y, w, h, BUTTON_RADIUS * s, color);
        };
        let lit = mix(p.tab_bar, p.selected, 0.7);
        for (i, (chip, task)) in layout.chips.iter().zip(&view.tasks).enumerate() {
            if view.hover == Some(FooterHit::Task(i)) {
                pill(&mut c, *chip, lit);
            }
            if !task.dot.blinks() {
                paint_dot(&mut c, chip_dot(*chip), task.dot, scale, p);
            }
            let x = chip.loc.x + CHIP_PAD + DOT + DOT_GAP;
            let (start, end) = phys(
                Rectangle::new(
                    (x, 0).into(),
                    (chip.loc.x + chip.size.w - CHIP_PAD - x, 1).into(),
                ),
                scale,
            );
            self.text(
                &mut c,
                &task.chip,
                SMALL_FONT_SIZE,
                start,
                end,
                band(*chip, scale),
                scale,
                p.text,
            );
        }
        if view.hover == Some(FooterHit::Count) {
            pill(&mut c, layout.count, lit);
        }
        let more = view.tasks.len() > layout.chips.len();
        let count = count_text(view.tasks.len());
        let color = if more || view.hover == Some(FooterHit::Count) {
            p.text
        } else {
            p.muted
        };
        self.centered(&mut c, &count, SMALL_FONT_SIZE, layout.count, scale, color);
        c.into_buffer(true)
    }
}

/// How wide the tasks panel is, at the most.
const PANEL_WIDTH: i32 = 560;
/// Room around its edges, and between it and the footer.
const PANEL_PAD: i32 = 12;
const PANEL_MARGIN: i32 = 6;
const TITLE_H: i32 = 22;
const TITLE_GAP: i32 = 8;
const ROW_H: i32 = 46;
const ROW_GAP: i32 = 2;
const EMPTY_H: i32 = 30;
const BUTTON_H: i32 = 26;
const BUTTON_PAD: i32 = 10;
const BUTTON_GAP: i32 = 6;
/// A row's dot column and the gap after it.
const ROW_INSET: i32 = 8;
const ROW_DOT: i32 = 14;

/// What the tasks panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TasksView {
    pub rows: Vec<TaskView>,
    /// The row under the keyboard's cursor.
    pub selected: Option<usize>,
    pub hover: Option<TasksHit>,
}

/// What the pointer is over in the tasks panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TasksHit {
    Panel,
    Row(usize),
    /// Row `.0`'s button `.1`.
    Button(usize, usize),
}

/// Where the tasks panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TasksLayout {
    pub panel: Rect,
    pub title: Rect,
    pub rows: Vec<Rect>,
    /// Each row's buttons, in order.
    pub buttons: Vec<Vec<Rect>>,
}

/// How wide a button labelled `label` is.
fn button_w(label: &str) -> i32 {
    text_w(label, FONT_SIZE) + 2 * BUTTON_PAD
}

/// Where the tasks panel's parts go on an output of `output`, over a
/// footer `footer` high: at the bottom left, over the footer.
pub fn tasks_layout(view: &TasksView, output: Size<i32, Logical>, footer: i32) -> TasksLayout {
    let rows_h = if view.rows.is_empty() {
        EMPTY_H
    } else {
        view.rows.len() as i32 * (ROW_H + ROW_GAP) - ROW_GAP
    };
    let height = 2 * PANEL_PAD + TITLE_H + TITLE_GAP + rows_h;
    let width = PANEL_WIDTH.min(output.w - 2 * PANEL_MARGIN).max(1);
    let y = (output.h - footer - PANEL_MARGIN - height).max(PANEL_MARGIN);
    let panel = Rectangle::new((PANEL_MARGIN, y).into(), (width, height).into());
    let (x, inner) = (panel.loc.x + PANEL_PAD, width - 2 * PANEL_PAD);
    let title = Rectangle::new((x, y + PANEL_PAD).into(), (inner, TITLE_H).into());
    let rows_y = title.loc.y + TITLE_H + TITLE_GAP;
    let rows: Vec<Rect> = (0..view.rows.len() as i32)
        .map(|i| {
            Rectangle::new(
                (x, rows_y + i * (ROW_H + ROW_GAP)).into(),
                (inner, ROW_H).into(),
            )
        })
        .collect();
    let buttons = rows
        .iter()
        .zip(&view.rows)
        .map(|(row, task)| {
            let mut right = row.loc.x + row.size.w - ROW_INSET;
            let by = row.loc.y + (ROW_H - BUTTON_H) / 2;
            let mut rects: Vec<Rect> = task
                .buttons
                .iter()
                .rev()
                .map(|label| {
                    let w = button_w(label);
                    right -= w;
                    let rect = Rectangle::new((right, by).into(), (w, BUTTON_H).into());
                    right -= BUTTON_GAP;
                    rect
                })
                .collect();
            rects.reverse();
            rects
        })
        .collect();
    TasksLayout {
        panel,
        title,
        rows,
        buttons,
    }
}

impl TasksLayout {
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<TasksHit> {
        if !self.panel.to_f64().contains(pos) {
            return None;
        }
        for (i, buttons) in self.buttons.iter().enumerate() {
            if let Some(b) = buttons.iter().position(|r| r.to_f64().contains(pos)) {
                return Some(TasksHit::Button(i, b));
            }
        }
        let row = self.rows.iter().position(|r| r.to_f64().contains(pos));
        Some(row.map_or(TasksHit::Panel, TasksHit::Row))
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        self.buttons.iter().flatten().copied().collect()
    }
}

impl Painter {
    /// The tasks panel on an output of `output` over a footer `footer`
    /// high, inside a margin of `SHADOW` holding its shadow.
    pub(super) fn tasks_panel(
        &self,
        view: &TasksView,
        output: Size<i32, Logical>,
        footer: i32,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = tasks_layout(view, output, footer);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, HELP_RADIUS, scale, p);
        let s = scale as f32;
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let title = shift(layout.title);
        let (tx, tend) = phys(title, scale);
        self.text(
            &mut c,
            "Background tasks",
            LABEL_FONT_SIZE,
            tx,
            tend,
            band(title, scale),
            scale,
            p.text,
        );
        if view.rows.is_empty() {
            let empty = Rectangle::new(
                (title.loc.x, title.loc.y + TITLE_H + TITLE_GAP).into(),
                (title.size.w, EMPTY_H).into(),
            );
            let (x, end) = phys(empty, scale);
            self.text(
                &mut c,
                "Nothing runs in the background",
                FONT_SIZE,
                x,
                end,
                band(empty, scale),
                scale,
                p.muted,
            );
            return c.into_buffer(false);
        }
        for (i, ((row, task), buttons)) in layout
            .rows
            .iter()
            .zip(&view.rows)
            .zip(&layout.buttons)
            .enumerate()
        {
            let row = shift(*row);
            let lit =
                matches!(view.hover, Some(TasksHit::Row(r) | TasksHit::Button(r, _)) if r == i);
            let (x, y, w, h) = rect_px(row, scale);
            // Softer than `p.selected`, which a lit button takes.
            if view.selected == Some(i) {
                c.rounded(
                    x,
                    y,
                    w,
                    h,
                    BUTTON_RADIUS * s,
                    mix(p.elevated, p.selected, 0.6),
                );
            } else if lit {
                c.rounded(
                    x,
                    y,
                    w,
                    h,
                    BUTTON_RADIUS * s,
                    mix(p.elevated, p.selected, 0.3),
                );
            }
            let spot = Rectangle::new(
                (
                    row.loc.x + ROW_INSET + (ROW_DOT - DOT) / 2,
                    row.loc.y + 8 + (18 - DOT) / 2,
                )
                    .into(),
                (DOT, DOT).into(),
            );
            paint_dot(&mut c, spot, task.dot, scale, p);
            let text_x = row.loc.x + ROW_INSET + ROW_DOT + 6;
            let text_end = buttons
                .first()
                .map_or(row.loc.x + row.size.w - ROW_INSET, |b| b.loc.x - BUTTON_GAP);
            let line = |top: i32, h: i32| {
                Rectangle::new((text_x, top).into(), ((text_end - text_x).max(1), h).into())
            };
            let first = line(row.loc.y + 6, 20);
            let (x, end) = phys(first, scale);
            self.text(
                &mut c,
                &task.title,
                FONT_SIZE,
                x,
                end,
                band(first, scale),
                scale,
                p.text,
            );
            let second = line(row.loc.y + 26, 16);
            let (x, end) = phys(second, scale);
            self.text(
                &mut c,
                &task.detail,
                DETAIL_FONT_SIZE,
                x,
                end,
                band(second, scale),
                scale,
                p.muted,
            );
            for (b, (rect, label)) in buttons.iter().zip(&task.buttons).enumerate() {
                let hovered = view.hover == Some(TasksHit::Button(i, b));
                let focused = view.selected == Some(i) && b == 0;
                self.button(
                    &mut c,
                    label,
                    shift(*rect),
                    (hovered, focused),
                    false,
                    scale,
                    p,
                );
            }
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::inside;

    fn task(chip: &str, buttons: &[&str]) -> TaskView {
        TaskView {
            title: chip.to_owned(),
            chip: chip.to_owned(),
            detail: "detail".to_owned(),
            dot: Dot::Done,
            buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
        }
    }

    #[test]
    fn chips_go_left_to_right_and_stop_before_the_count() {
        let view = FooterView {
            tasks: (0..40).map(|i| task(&format!("task {i}"), &[])).collect(),
            hover: None,
        };
        let size = (900, 24).into();
        let layout = footer_layout(&view, size);
        assert!(!layout.chips.is_empty() && layout.chips.len() < 40);
        assert!(
            layout
                .chips
                .windows(2)
                .all(|c| c[0].loc.x + c[0].size.w < c[1].loc.x)
        );
        let last = layout.chips.last().unwrap();
        assert!(last.loc.x + last.size.w <= layout.count.loc.x);
        assert_eq!(layout.count.loc.x + layout.count.size.w, 900 - EDGE);
        let first = layout.chips[0];
        let middle = Point::from((first.loc.x as f64 + 2.0, 12.0));
        assert_eq!(layout.hit(middle), Some(FooterHit::Task(0)));
        let count = Point::from((layout.count.loc.x as f64 + 2.0, 12.0));
        assert_eq!(layout.hit(count), Some(FooterHit::Count));
    }

    #[test]
    fn the_panel_sits_over_the_footer_with_buttons_right_aligned() {
        let view = TasksView {
            rows: vec![
                task("T3 Code", &["Open", "Restart", "Stop"]),
                task("fetch", &[]),
            ],
            selected: Some(0),
            hover: None,
        };
        let output = (1600, 900).into();
        let l = tasks_layout(&view, output, 24);
        assert_eq!(l.panel.loc.y + l.panel.size.h, 900 - 24 - PANEL_MARGIN);
        assert_eq!(l.buttons[0].len(), 3);
        assert!(l.buttons[1].is_empty());
        for b in &l.buttons[0] {
            assert!(inside(l.rows[0], *b));
        }
        let last = l.buttons[0][2];
        assert_eq!(
            last.loc.x + last.size.w,
            l.rows[0].loc.x + l.rows[0].size.w - ROW_INSET
        );
        let on = Point::from((
            l.buttons[0][1].loc.x as f64 + 3.0,
            l.buttons[0][1].loc.y as f64 + 3.0,
        ));
        assert_eq!(l.hit(on), Some(TasksHit::Button(0, 1)));
        let row = Point::from((l.rows[1].loc.x as f64 + 3.0, l.rows[1].loc.y as f64 + 3.0));
        assert_eq!(l.hit(row), Some(TasksHit::Row(1)));
        assert_eq!(l.hit(Point::from((1500.0, 10.0))), None);
    }
}
