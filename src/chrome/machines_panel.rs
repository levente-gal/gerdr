//! The machines panel: the machines gerdr reaches over SSH listed on the
//! left (and a row adding one), the selected one on the right: how its link
//! stands, what of this gerdr's features runs there with buttons to show or
//! stop it, and buttons acting on the machine (see `machines_panel`).

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{
    ButtonStyle, Canvas, DETAIL_FONT_SIZE, DOT, Dot, FONT_SIZE, HELP_MARGIN, HELP_PADDING,
    HELP_TITLE_HEIGHT, HINTS_HEIGHT, LABEL_FONT_SIZE, ListBar, POPUP_RADIUS, Painter, Palette,
    QUIT_BUTTON, QUIT_BUTTON_GAP, Rect, Rgb, SHADOW, SMALL_FONT_SIZE, Thumb, band, button_width,
    dot_shape, hint_spots, hint_strs, icon_sdf, measure, phys,
};

/// How wide the panel is, at the most; it shrinks with the output, keeping
/// `MARGIN` around it.
const PANEL_WIDTH: i32 = 860;
const MARGIN: i32 = HELP_MARGIN;
/// Room inside the panel's edges and the detail's.
const PAD: i32 = HELP_PADDING;
const TITLE_H: i32 = HELP_TITLE_HEIGHT;
/// The list's column, at the most a third of the panel.
const LIST_W: i32 = 240;
/// Room around the list's rows.
const LIST_PAD: i32 = 8;
/// A list row or a run row: its title line and its detail line.
const ROW_H: i32 = 46;
const ROW_GAP: i32 = 2;
/// A row's dot column and the gap after it.
const ROW_INSET: i32 = 8;
const ROW_DOT: i32 = 14;
const DOT_GAP: i32 = 6;
/// A two-line row's lines, as (top in the row, height): as the tasks
/// panel's.
const TOP_LINE: (i32, i32) = (6, 20);
const LOW_LINE: (i32, i32) = (26, 16);
/// The detail, from the top: room, its heading, a line per fact, the
/// problem and what to do about it, the runs' subheading, the runs.
const DETAIL_TOP: i32 = 10;
const HEAD_H: i32 = 28;
const FACT_H: i32 = 20;
const LINE_H: i32 = 20;
const SECTION_GAP: i32 = 10;
const SUBHEAD_H: i32 = 26;
const EMPTY_H: i32 = 30;
/// A run's buttons, right-aligned on its row.
const RUN_BUTTON_H: i32 = 26;
const RUN_BUTTON_PAD: i32 = 10;
const RUN_BUTTON_GAP: i32 = 6;
/// Room above the machine's buttons.
const BUTTONS_GAP: i32 = 12;
/// The key hints' band along the bottom.
const FOOTER_H: i32 = 36;
/// The scrollbar's room right of the rows, when not all fit.
const BAR_ROOM: i32 = 8;
/// The most lines the problem, or what to do about it, wraps to.
const MOST_LINES: usize = 3;

/// What the detail says on the add row.
const ADD_NOTE: &str = "Connect a machine over SSH to run agents and checks there";
const ADD_LABEL: &str = "Add machine\u{2026}";
const RUNS_HEAD: &str = "Runs there";

/// What the machines panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinesPanelView {
    /// The machines saved, by name. The row adding one comes after them.
    pub rows: Vec<MachineRowView>,
    /// The row under the keyboard's cursor: `rows.len()` is the add row.
    pub selected: usize,
    /// The first list row shown, when not all fit.
    pub first: usize,
    /// The selected machine; none on the add row.
    pub detail: Option<MachineDetailView>,
    /// What the keyboard is on.
    pub focus: MachinesFocus,
    /// The first run shown in the detail, when not all fit.
    pub runs_first: usize,
    pub hover: Option<MachinesHit>,
    /// The key hints along the bottom: keys, what they do.
    pub hints: Vec<(String, String)>,
}

/// A machine's row in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRowView {
    pub name: String,
    /// The address its link uses, or the one saved.
    pub address: String,
    /// How its link stands, shortly: "connected", "away", "off"...
    pub state: String,
    pub dot: Dot,
    /// Del was pressed once on it: the row says "Press Del again".
    pub armed: bool,
}

/// The selected machine, on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineDetailView {
    pub name: String,
    pub dot: Dot,
    /// Lines about its link, top first: its address, then "up 2h · herdr
    /// 0.9.3 · load 1.2 / 8", then its id.
    pub facts: Vec<String>,
    /// Why it isn't connected, and what to do about it, when it isn't.
    pub problem: Option<String>,
    pub how: Option<String>,
    /// What of this gerdr's features runs there.
    pub runs: Vec<MachineRunView>,
    /// What the runs list says when it is empty: "Nothing runs there",
    /// "Connect it to see what runs there".
    pub no_runs: String,
    /// The buttons acting on the machine, left to right; the last is the
    /// primary one.
    pub buttons: Vec<String>,
}

/// A run on the machine: an agent, a feature's checks, a sandbox, what a
/// gone feature left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRunView {
    /// "asset-tag · Build agent".
    pub title: String,
    /// "Claude · working · 12m".
    pub detail: String,
    pub dot: Dot,
    pub buttons: Vec<String>,
}

/// What the keyboard is on in the machines panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MachinesFocus {
    /// The list: Up/Down move through the machines.
    #[default]
    List,
    /// Run `.0`'s button `.1`.
    Run(usize, usize),
    /// The machine's button `.0`.
    Button(usize),
}

/// What the pointer is over in the machines panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachinesHit {
    Panel,
    /// The list's row `.0`; `rows.len()` is the add row.
    Row(usize),
    /// Run `.0`, off its buttons.
    Run(usize),
    /// Run `.0`'s button `.1`.
    RunButton(usize, usize),
    /// The machine's button `.0`.
    Button(usize),
    /// Key hint `.0`: a click presses its key.
    Hint(usize),
}

/// Where the machines panel's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinesLayout {
    pub panel: Rect,
    pub title: Rect,
    /// The list's column.
    pub list: Rect,
    /// The list row shown first: `rows[0]` is row `first`.
    pub first: usize,
    /// The list rows shown, from `first`; the add row is among them when
    /// it shows.
    pub rows: Vec<Rect>,
    /// The list's scrollbar track, when not all rows fit.
    pub track: Option<Rect>,
    /// The detail's column.
    pub detail: Rect,
    /// The detail's heading: the machine's dot and name, or on the add row
    /// what adding one is for.
    pub heading: Rect,
    /// A line per fact about the machine.
    pub facts: Vec<Rect>,
    /// The problem's lines, wrapped, then what to do about it's.
    pub problem: Vec<(String, Rect)>,
    pub how: Vec<(String, Rect)>,
    /// The "Runs there" subheading, and where the runs list says it is
    /// empty, when it is.
    pub runs_head: Rect,
    pub no_runs: Option<Rect>,
    /// The run shown first: `runs[0]` is run `runs_first`.
    pub runs_first: usize,
    /// The runs shown, from `runs_first`.
    pub runs: Vec<Rect>,
    /// Each shown run's buttons, in order.
    pub run_buttons: Vec<Vec<Rect>>,
    /// The runs' scrollbar track, when not all fit.
    pub runs_track: Option<Rect>,
    /// The machine's buttons, in order.
    pub buttons: Vec<Rect>,
    /// The key hints' band, with a line on top.
    pub footer: Rect,
    /// The key hints, as (index into the view's hints, rect).
    pub hints: Vec<(usize, Rect)>,
}

fn text_w(text: &str, size: f32) -> i32 {
    measure(text, size)
        .unwrap_or(text.chars().count() as f32 * size * 0.6)
        .ceil() as i32
}

/// The panel's width on an output `w` wide, and its list column's.
fn widths(w: i32) -> (i32, i32) {
    let panel = PANEL_WIDTH.min(w - 2 * MARGIN).max(1);
    (panel, list_width(panel))
}

/// The list column's width in a panel `panel` wide.
fn list_width(panel: i32) -> i32 {
    LIST_W.min(panel / 3)
}

/// How high the list and the detail can be on an output `h` high.
fn body_room(h: i32) -> i32 {
    h - 2 * MARGIN - TITLE_H - FOOTER_H
}

/// How wide the detail's text is in a panel `panel` wide.
fn detail_inner(panel: i32) -> i32 {
    (panel - list_width(panel) - 2 * PAD).max(1)
}

/// `text` wrapped to `width` at `size`, at most `MOST_LINES` lines.
fn wrap(text: &str, size: f32, width: i32) -> Vec<String> {
    let mut lines = super::features::wrap_all(text, size, width);
    lines.truncate(MOST_LINES);
    lines
}

/// The detail's lines: the problem's, then the how's.
fn detail_lines(d: &MachineDetailView, width: i32) -> (Vec<String>, Vec<String>) {
    let problem = (d.problem.as_deref()).map_or_else(Vec::new, |t| wrap(t, FONT_SIZE, width));
    let how = (d.how.as_deref()).map_or_else(Vec::new, |t| wrap(t, SMALL_FONT_SIZE, width));
    (problem, how)
}

/// How high the detail is without its runs (or its empty line).
fn detail_fixed(d: &MachineDetailView, width: i32) -> i32 {
    let (problem, how) = detail_lines(d, width);
    let lines = (problem.len() + how.len()) as i32;
    let problem_h = if lines > 0 {
        SECTION_GAP / 2 + lines * LINE_H
    } else {
        0
    };
    let buttons = if d.buttons.is_empty() {
        0
    } else {
        BUTTONS_GAP + QUIT_BUTTON
    };
    DETAIL_TOP
        + HEAD_H
        + d.facts.len() as i32 * FACT_H
        + problem_h
        + SECTION_GAP
        + SUBHEAD_H
        + buttons
        + PAD
}

/// How many list rows (machines and the add row) fit on an output of
/// `output`: at least one.
pub fn machines_shown(rows: usize, output: Size<i32, Logical>) -> usize {
    let room = body_room(output.h) - 2 * LIST_PAD + ROW_GAP;
    let fit = (room / (ROW_H + ROW_GAP)).max(1) as usize;
    rows.min(fit).max(1)
}

/// How many runs fit in the detail on an output of `output`: at least one.
pub fn machine_runs_shown(view: &MachinesPanelView, output: Size<i32, Logical>) -> usize {
    let Some(d) = &view.detail else {
        return 1;
    };
    let (panel, _) = widths(output.w);
    let room = body_room(output.h) - detail_fixed(d, detail_inner(panel)) + ROW_GAP;
    let fit = (room / (ROW_H + ROW_GAP)).max(1) as usize;
    d.runs.len().min(fit).max(1)
}

/// Where `buttons` go right-aligned on `row`, each as wide as its label.
fn run_buttons(row: Rect, buttons: &[String]) -> Vec<Rect> {
    let mut right = row.loc.x + row.size.w - ROW_INSET;
    let y = row.loc.y + (ROW_H - RUN_BUTTON_H) / 2;
    let mut rects: Vec<Rect> = (buttons.iter().rev())
        .map(|label| {
            let w = text_w(label, FONT_SIZE) + 2 * RUN_BUTTON_PAD;
            right -= w;
            let rect = Rectangle::new((right, y).into(), (w, RUN_BUTTON_H).into());
            right -= RUN_BUTTON_GAP;
            rect
        })
        .collect();
    rects.reverse();
    rects
}

/// Rows `h` high from `top`, `ROW_GAP` apart, `count` of them.
fn stack(x: i32, top: i32, w: i32, count: usize) -> Vec<Rect> {
    (0..count as i32)
        .map(|i| Rectangle::new((x, top + i * (ROW_H + ROW_GAP)).into(), (w, ROW_H).into()))
        .collect()
}

/// How high `count` rows stacked are.
fn stack_h(count: usize) -> i32 {
    (count as i32 * (ROW_H + ROW_GAP) - ROW_GAP).max(0)
}

/// Where the machines panel's parts go on an output of `output`: centred,
/// as tall as its list or its detail needs, at most the output's height;
/// the rows and runs past that scroll.
pub fn machines_layout(view: &MachinesPanelView, output: Size<i32, Logical>) -> MachinesLayout {
    let (width, list_w) = widths(output.w);
    let inner = detail_inner(width);
    let total = view.rows.len() + 1;
    let shown = machines_shown(total, output);
    let first = view.first.min(total - shown);
    let list_h = 2 * LIST_PAD + stack_h(shown);
    let (runs_total, runs_shown) = view.detail.as_ref().map_or((0, 0), |d| {
        let n = d.runs.len();
        (
            n,
            if n == 0 {
                0
            } else {
                machine_runs_shown(view, output)
            },
        )
    });
    let runs_first = view.runs_first.min(runs_total - runs_shown);
    let detail_h = match &view.detail {
        Some(d) => {
            let runs_h = if runs_shown == 0 {
                EMPTY_H
            } else {
                stack_h(runs_shown)
            };
            detail_fixed(d, inner) + runs_h
        }
        None => DETAIL_TOP + HEAD_H + PAD,
    };
    let body_h = list_h.max(detail_h);
    let height = (TITLE_H + body_h + FOOTER_H).min((output.h - 2 * MARGIN).max(1));
    let panel = Rectangle::new(
        (
            ((output.w - width) / 2).max(0),
            ((output.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, y) = (panel.loc.x, panel.loc.y);
    let title = Rectangle::new((x + PAD, y).into(), (width - 2 * PAD, TITLE_H).into());
    let body_top = y + TITLE_H;
    let body_h = (height - TITLE_H - FOOTER_H).max(0);
    let list = Rectangle::new((x, body_top).into(), (list_w, body_h).into());
    let detail = Rectangle::new(
        (x + list_w, body_top).into(),
        (width - list_w, body_h).into(),
    );

    // The list.
    let scrolls = shown < total;
    let row_x = x + LIST_PAD;
    let row_w = list_w - 2 * LIST_PAD - if scrolls { BAR_ROOM } else { 0 };
    let rows_top = body_top + LIST_PAD;
    let rows = stack(row_x, rows_top, row_w.max(1), shown);
    let track = scrolls.then(|| {
        super::gutter_track(
            row_x + row_w + 1,
            Rectangle::new((row_x, rows_top).into(), (row_w, stack_h(shown)).into()),
        )
    });

    // The detail, top down.
    let dx = detail.loc.x + PAD;
    let line = |top: i32, h: i32| Rectangle::new((dx, top).into(), (inner, h).into());
    let mut top = body_top + DETAIL_TOP;
    let heading = line(top, HEAD_H);
    top += HEAD_H;
    let mut facts = Vec::new();
    let mut problem = Vec::new();
    let mut how = Vec::new();
    let mut runs_head = line(top, 0);
    let mut no_runs = None;
    let mut runs = Vec::new();
    let mut runs_buttons = Vec::new();
    let mut runs_track = None;
    let mut buttons = Vec::new();
    if let Some(d) = &view.detail {
        for _ in &d.facts {
            facts.push(line(top, FACT_H));
            top += FACT_H;
        }
        let (problem_lines, how_lines) = detail_lines(d, inner);
        if !problem_lines.is_empty() || !how_lines.is_empty() {
            top += SECTION_GAP / 2;
        }
        for (lines, out) in [(problem_lines, &mut problem), (how_lines, &mut how)] {
            for text in lines {
                out.push((text, line(top, LINE_H)));
                top += LINE_H;
            }
        }
        top += SECTION_GAP;
        runs_head = line(top, SUBHEAD_H);
        top += SUBHEAD_H;
        if runs_shown == 0 {
            no_runs = Some(line(top, EMPTY_H));
        } else {
            let scrolls = runs_shown < runs_total;
            let run_w = inner - if scrolls { BAR_ROOM } else { 0 };
            runs = stack(dx, top, run_w, runs_shown);
            runs_buttons = (runs.iter())
                .zip(&d.runs[runs_first..])
                .map(|(row, run)| run_buttons(*row, &run.buttons))
                .collect();
            runs_track = scrolls.then(|| {
                super::gutter_track(
                    dx + run_w + 1,
                    Rectangle::new((dx, top).into(), (run_w, stack_h(runs_shown)).into()),
                )
            });
        }
        let labels: Vec<&str> = d.buttons.iter().map(String::as_str).collect();
        let bw: Vec<i32> = labels.iter().map(|l| button_width(l)).collect();
        let all = bw.iter().sum::<i32>() + (bw.len() as i32 - 1).max(0) * QUIT_BUTTON_GAP;
        let by = detail.loc.y + detail.size.h - PAD - QUIT_BUTTON;
        let mut bx = dx + inner - all;
        for w in bw {
            buttons.push(Rectangle::new((bx, by).into(), (w, QUIT_BUTTON).into()));
            bx += w + QUIT_BUTTON_GAP;
        }
    }

    let footer = Rectangle::new((x, y + height - FOOTER_H).into(), (width, FOOTER_H).into());
    let hints_rect = Rectangle::new(
        (x + PAD, footer.loc.y + (FOOTER_H - HINTS_HEIGHT) / 2).into(),
        (width - 2 * PAD, HINTS_HEIGHT).into(),
    );
    MachinesLayout {
        panel,
        title,
        list,
        first,
        rows,
        track,
        detail,
        heading,
        facts,
        problem,
        how,
        runs_head,
        no_runs,
        runs_first,
        runs,
        run_buttons: runs_buttons,
        runs_track,
        buttons,
        footer,
        hints: hint_spots(&hint_strs(&view.hints), hints_rect),
    }
}

impl MachinesLayout {
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<MachinesHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if !on(&self.panel) {
            return None;
        }
        if let Some((i, _)) = self.hints.iter().find(|(_, r)| on(r)) {
            return Some(MachinesHit::Hint(*i));
        }
        for (k, buttons) in self.run_buttons.iter().enumerate() {
            if let Some(b) = buttons.iter().position(on) {
                return Some(MachinesHit::RunButton(self.runs_first + k, b));
            }
        }
        if let Some(b) = self.buttons.iter().position(on) {
            return Some(MachinesHit::Button(b));
        }
        if let Some(k) = self.runs.iter().position(on) {
            return Some(MachinesHit::Run(self.runs_first + k));
        }
        if let Some(k) = self.rows.iter().position(on) {
            return Some(MachinesHit::Row(self.first + k));
        }
        Some(MachinesHit::Panel)
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        (self.buttons.iter())
            .chain(self.run_buttons.iter().flatten())
            .chain(self.rows.iter())
            .copied()
            .chain(self.hints.iter().map(|(_, r)| *r))
            .collect()
    }
}

/// How a machine's button labelled `label`, `i` of `n`, is painted: the
/// last primary, the rest plain. Remove is plain too, and never the
/// primary one: it offers Undo, or asks first.
fn button_style(label: &str, i: usize, n: usize) -> ButtonStyle {
    if i + 1 == n && label != "Remove" {
        ButtonStyle::Primary
    } else {
        ButtonStyle::Plain
    }
}

/// Paint `dot` still at `spot`.
fn paint_dot(c: &mut Canvas, spot: Rect, dot: Dot, scale: f64, p: &Palette) {
    let d = DOT as f32 * scale as f32;
    icon_sdf(c, spot, scale, dot.color(p), |x, y| dot_shape(dot, x, y, d));
}

/// Where a two-line row's dot goes, on its title line.
fn row_dot(row: Rect) -> Rect {
    Rectangle::new(
        (
            row.loc.x + ROW_INSET + (ROW_DOT - DOT) / 2,
            row.loc.y + TOP_LINE.0 + 2 + (TOP_LINE.1 - 2 - DOT) / 2,
        )
            .into(),
        (DOT, DOT).into(),
    )
}

impl Painter {
    /// `text` in `rect` (buffer coordinates), from its left, cut with "…"
    /// at its right.
    #[allow(clippy::too_many_arguments)]
    fn line_in(
        &self,
        c: &mut Canvas,
        text: &str,
        size: f32,
        rect: Rect,
        bold: bool,
        scale: f64,
        color: Rgb,
    ) {
        let (x, end) = phys(rect, scale);
        if bold {
            self.bold_text(c, text, size, x, end, band(rect, scale), scale, color);
        } else {
            self.text(c, text, size, x, end, band(rect, scale), scale, color);
        }
    }

    /// A two-line row in `row` (buffer coordinates): `dot`, `title` and
    /// under it `detail` in `detail_color`, ending at `end`.
    #[allow(clippy::too_many_arguments)]
    fn two_lines(
        &self,
        c: &mut Canvas,
        row: Rect,
        end: i32,
        dot: Dot,
        (title, bold): (&str, bool),
        (detail, detail_color): (&str, Rgb),
        scale: f64,
        p: &Palette,
    ) {
        paint_dot(c, row_dot(row), dot, scale, p);
        let text_x = row.loc.x + ROW_INSET + ROW_DOT + DOT_GAP;
        let w = (end - text_x).max(1);
        let first = Rectangle::new(
            (text_x, row.loc.y + TOP_LINE.0).into(),
            (w, TOP_LINE.1).into(),
        );
        self.line_in(c, title, FONT_SIZE, first, bold, scale, p.text);
        let second = Rectangle::new(
            (text_x, row.loc.y + LOW_LINE.0).into(),
            (w, LOW_LINE.1).into(),
        );
        self.line_in(
            c,
            detail,
            DETAIL_FONT_SIZE,
            second,
            false,
            scale,
            detail_color,
        );
    }

    /// The machines panel on an output of `output`, inside a margin of
    /// `SHADOW` holding its shadow.
    pub(super) fn machines_panel(
        &self,
        view: &MachinesPanelView,
        output: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = machines_layout(view, output);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let by = Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y));
        let shift = |r: Rect| Rectangle::new(r.loc + by, r.size);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let border = px(1).max(1);

        self.line_in(
            &mut c,
            "Machines",
            LABEL_FONT_SIZE,
            shift(layout.title),
            true,
            scale,
            p.text,
        );
        // The lines under the title, over the hints, and between the list
        // and the detail.
        let across = |c: &mut Canvas, y: i32| {
            c.fill(
                px(SHADOW) + border,
                y,
                px(panel.size.w) - 2 * border,
                border,
                p.border_variant,
            );
        };
        let list = shift(layout.list);
        across(&mut c, px(list.loc.y) - border);
        across(&mut c, px(shift(layout.footer).loc.y));
        c.fill(
            px(list.loc.x + list.size.w) - border,
            px(list.loc.y),
            border,
            px(list.size.h),
            p.border_variant,
        );

        let hints: Vec<(usize, Rect)> = layout.hints.iter().map(|(i, r)| (*i, shift(*r))).collect();
        let hint_hover = match view.hover {
            Some(MachinesHit::Hint(i)) => Some(i),
            _ => None,
        };
        self.hints(
            &mut c,
            &hint_strs(&view.hints),
            &hints,
            hint_hover,
            scale,
            p,
        );

        self.machines_list(&mut c, view, &layout, &shift, scale, p);
        match &view.detail {
            Some(d) => self.machine_detail(&mut c, view, d, &layout, &shift, scale, p),
            None => self.line_in(
                &mut c,
                ADD_NOTE,
                FONT_SIZE,
                shift(layout.heading),
                false,
                scale,
                p.muted,
            ),
        }
        c.into_buffer(false)
    }

    /// The machines panel's list, its rows shifted into `c` by `shift`.
    fn machines_list(
        &self,
        c: &mut Canvas,
        view: &MachinesPanelView,
        layout: &MachinesLayout,
        shift: &dyn Fn(Rect) -> Rect,
        scale: f64,
        p: &Palette,
    ) {
        if let Some(track) = layout.track {
            let bar = ListBar {
                down: layout.first as f64,
                total: (view.rows.len() + 1) as f64,
            };
            if let Some(thumb) = Thumb::of(shift(track), layout.rows.len() as f64, bar) {
                self.thumb_in(c, &thumb, scale, p);
            }
        }
        let on_list = view.focus == MachinesFocus::List;
        for (k, row) in layout.rows.iter().enumerate() {
            let i = layout.first + k;
            let row = shift(*row);
            let lit = view.hover == Some(MachinesHit::Row(i));
            let selected = view.selected == i;
            // Selected while the keyboard is in the detail: lit as a
            // hovered row, so the detail's own cursor stands out.
            let ground = match (selected, on_list) {
                (true, true) => (true, 0.0),
                (true, false) => (false, 1.0),
                _ => (false, if lit { 1.0 } else { 0.0 }),
            };
            self.row_highlight(c, row, ground, scale, p);
            let end = row.loc.x + row.size.w - ROW_INSET;
            let Some(machine) = view.rows.get(i) else {
                // The add row.
                let plus = Rectangle::new(
                    (row.loc.x + ROW_INSET, row.loc.y).into(),
                    (ROW_DOT, ROW_H).into(),
                );
                self.centered(c, "+", FONT_SIZE, plus, scale, p.muted);
                let text_x = row.loc.x + ROW_INSET + ROW_DOT + DOT_GAP;
                let label = Rectangle::new(
                    (text_x, row.loc.y).into(),
                    ((end - text_x).max(1), ROW_H).into(),
                );
                self.line_in(c, ADD_LABEL, FONT_SIZE, label, false, scale, p.text);
                continue;
            };
            let detail = if machine.armed {
                ("Press Del again".to_owned(), p.error)
            } else if machine.address.is_empty() {
                (machine.state.clone(), p.muted)
            } else {
                (
                    format!("{} \u{b7} {}", machine.address, machine.state),
                    p.muted,
                )
            };
            self.two_lines(
                c,
                row,
                end,
                machine.dot,
                (&machine.name, true),
                (&detail.0, detail.1),
                scale,
                p,
            );
        }
    }

    /// The selected machine's detail, shifted into `c` by `shift`.
    #[allow(clippy::too_many_arguments)]
    fn machine_detail(
        &self,
        c: &mut Canvas,
        view: &MachinesPanelView,
        d: &MachineDetailView,
        layout: &MachinesLayout,
        shift: &dyn Fn(Rect) -> Rect,
        scale: f64,
        p: &Palette,
    ) {
        let heading = shift(layout.heading);
        let spot = Rectangle::new(
            (
                heading.loc.x + (ROW_DOT - DOT) / 2,
                heading.loc.y + (HEAD_H - DOT) / 2,
            )
                .into(),
            (DOT, DOT).into(),
        );
        paint_dot(c, spot, d.dot, scale, p);
        let name_x = heading.loc.x + ROW_DOT + DOT_GAP;
        let name = Rectangle::new(
            (name_x, heading.loc.y).into(),
            ((heading.loc.x + heading.size.w - name_x).max(1), HEAD_H).into(),
        );
        self.line_in(c, &d.name, LABEL_FONT_SIZE, name, true, scale, p.text);
        for (i, (fact, rect)) in d.facts.iter().zip(&layout.facts).enumerate() {
            let color = if i == 0 { p.text } else { p.muted };
            self.line_in(c, fact, SMALL_FONT_SIZE, shift(*rect), false, scale, color);
        }
        for (text, rect) in &layout.problem {
            self.line_in(c, text, FONT_SIZE, shift(*rect), false, scale, p.error);
        }
        for (text, rect) in &layout.how {
            self.line_in(
                c,
                text,
                SMALL_FONT_SIZE,
                shift(*rect),
                false,
                scale,
                p.muted,
            );
        }
        let head = shift(layout.runs_head);
        self.line_in(c, RUNS_HEAD, SMALL_FONT_SIZE, head, true, scale, p.muted);
        if let Some(rect) = layout.no_runs {
            self.line_in(c, &d.no_runs, FONT_SIZE, shift(rect), false, scale, p.muted);
        }
        if let Some(track) = layout.runs_track {
            let bar = ListBar {
                down: layout.runs_first as f64,
                total: d.runs.len() as f64,
            };
            if let Some(thumb) = Thumb::of(shift(track), layout.runs.len() as f64, bar) {
                self.thumb_in(c, &thumb, scale, p);
            }
        }
        for (k, (row, buttons)) in layout.runs.iter().zip(&layout.run_buttons).enumerate() {
            let r = layout.runs_first + k;
            let Some(run) = d.runs.get(r) else {
                continue;
            };
            let row = shift(*row);
            let lit = matches!(
                view.hover,
                Some(MachinesHit::Run(h) | MachinesHit::RunButton(h, _)) if h == r
            );
            let focused = matches!(view.focus, MachinesFocus::Run(f, _) if f == r);
            self.row_highlight(c, row, (focused, if lit { 1.0 } else { 0.0 }), scale, p);
            let end = buttons
                .first()
                .map_or(row.loc.x + row.size.w - ROW_INSET, |b| {
                    shift(*b).loc.x - RUN_BUTTON_GAP
                });
            self.two_lines(
                c,
                row,
                end,
                run.dot,
                (&run.title, false),
                (&run.detail, p.muted),
                scale,
                p,
            );
            for (b, (rect, label)) in buttons.iter().zip(&run.buttons).enumerate() {
                let hovered = view.hover == Some(MachinesHit::RunButton(r, b));
                let on = view.focus == MachinesFocus::Run(r, b);
                self.button(c, label, shift(*rect), (hovered, on), false, scale, p);
            }
        }
        let n = d.buttons.len();
        for (b, (rect, label)) in layout.buttons.iter().zip(&d.buttons).enumerate() {
            let hovered = view.hover == Some(MachinesHit::Button(b));
            let on = view.focus == MachinesFocus::Button(b);
            self.button(
                c,
                label,
                shift(*rect),
                (hovered, on),
                button_style(label, b, n),
                scale,
                p,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::inside;

    fn row(name: &str) -> MachineRowView {
        MachineRowView {
            name: name.to_owned(),
            address: format!("ana@{name}"),
            state: "connected".to_owned(),
            dot: Dot::Done,
            armed: false,
        }
    }

    fn run(i: usize) -> MachineRunView {
        MachineRunView {
            title: format!("feature-{i} \u{b7} Build agent"),
            detail: "Claude \u{b7} working \u{b7} 12m".to_owned(),
            dot: Dot::Working,
            buttons: vec!["Show".to_owned(), "Stop".to_owned()],
        }
    }

    fn view(machines: usize, runs: usize) -> MachinesPanelView {
        MachinesPanelView {
            rows: (0..machines).map(|i| row(&format!("box-{i}"))).collect(),
            selected: 0,
            first: 0,
            detail: Some(MachineDetailView {
                name: "box-0".to_owned(),
                dot: Dot::Done,
                facts: vec![
                    "ana@box-0".to_owned(),
                    "up 2h \u{b7} herdr 0.9.3 \u{b7} load 1.2 / 8".to_owned(),
                ],
                problem: None,
                how: None,
                runs: (0..runs).map(run).collect(),
                no_runs: "Nothing runs there".to_owned(),
                buttons: vec!["Remove".to_owned(), "Disconnect".to_owned()],
            }),
            focus: MachinesFocus::List,
            runs_first: 0,
            hover: None,
            hints: vec![
                ("Tab".to_owned(), "next".to_owned()),
                ("Esc".to_owned(), "close".to_owned()),
            ],
        }
    }

    fn mid(r: Rect) -> Point<f64, Logical> {
        Point::from((
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        ))
    }

    /// Every part inside the panel, the list's left of the detail's.
    fn fits(l: &MachinesLayout, output: Size<i32, Logical>) {
        let screen = Rectangle::new((0, 0).into(), output);
        assert!(inside(screen, l.panel), "{:?} on {output:?}", l.panel);
        assert!(l.panel.loc.x >= MARGIN && l.panel.loc.y >= MARGIN);
        for r in l.rows.iter().chain(l.track.iter()) {
            assert!(inside(l.list, *r), "{r:?} out of {:?}", l.list);
        }
        let parts = (l.runs.iter())
            .chain(l.run_buttons.iter().flatten())
            .chain(l.runs_track.iter())
            .chain(&l.buttons)
            .chain(&l.facts)
            .chain(std::iter::once(&l.heading));
        for r in parts {
            assert!(inside(l.detail, *r), "{r:?} out of {:?}", l.detail);
        }
        for (k, buttons) in l.run_buttons.iter().enumerate() {
            for b in buttons {
                assert!(inside(l.runs[k], *b));
            }
        }
        if let (Some(last), Some(b)) = (l.runs.last(), l.buttons.first()) {
            assert!(last.loc.y + last.size.h <= b.loc.y);
        }
        for (_, h) in &l.hints {
            assert!(inside(l.footer, *h));
        }
    }

    #[test]
    fn it_fits_a_small_output_and_a_large_one() {
        for output in [(870, 600).into(), (1920, 1080).into()] {
            let l = machines_layout(&view(3, 3), output);
            fits(&l, output);
            assert!(l.panel.size.w <= PANEL_WIDTH);
            assert_eq!(l.rows.len(), 4);
            assert_eq!(l.runs.len(), 3);
            assert!(l.track.is_none() && l.runs_track.is_none());
            // Centred.
            assert_eq!(l.panel.loc.x, (output.w - l.panel.size.w) / 2);
        }
        let narrow: Size<i32, Logical> = (870, 600).into();
        let l = machines_layout(&view(3, 3), narrow);
        assert_eq!(l.panel.size.w, 870 - 2 * MARGIN);
        let wide = machines_layout(&view(3, 3), (1920, 1080).into());
        assert_eq!(wide.panel.size.w, PANEL_WIDTH);
        assert_eq!(wide.list.size.w, LIST_W);
        // As tall as it needs, no more.
        assert!(wide.panel.size.h < 1080 - 2 * MARGIN);
    }

    #[test]
    fn hits_map_rows_buttons_hints_and_the_add_row() {
        let mut v = view(30, 20);
        v.first = 10;
        v.runs_first = 5;
        let output = (870, 600).into();
        let l = machines_layout(&v, output);
        fits(&l, output);
        assert_eq!(l.first, 10);
        assert_eq!(l.runs_first, 5);
        assert_eq!(l.hit(mid(l.rows[0])), Some(MachinesHit::Row(10)));
        assert_eq!(l.hit(mid(l.rows[2])), Some(MachinesHit::Row(12)));
        let run = l.runs[1];
        let off = Point::from((run.loc.x as f64 + 3.0, run.loc.y as f64 + 3.0));
        assert_eq!(l.hit(off), Some(MachinesHit::Run(6)));
        assert_eq!(
            l.hit(mid(l.run_buttons[1][1])),
            Some(MachinesHit::RunButton(6, 1))
        );
        assert_eq!(l.hit(mid(l.buttons[1])), Some(MachinesHit::Button(1)));
        let (i, hint) = l.hints[1];
        assert_eq!(l.hit(mid(hint)), Some(MachinesHit::Hint(i)));
        assert_eq!(l.hit(mid(l.title)), Some(MachinesHit::Panel));
        assert_eq!(l.hit(Point::from((2.0, 2.0))), None);
        // Every press is among the controls.
        let controls = l.controls();
        for r in l
            .rows
            .iter()
            .chain(&l.buttons)
            .chain(l.run_buttons.iter().flatten())
        {
            assert!(controls.contains(r));
        }

        // Scrolled to the end, the add row shows last and is hit as
        // `rows.len()`.
        v.first = usize::MAX;
        let l = machines_layout(&v, output);
        let add = *l.rows.last().unwrap();
        assert_eq!(l.hit(mid(add)), Some(MachinesHit::Row(30)));
    }

    #[test]
    fn scrolling_clamps_to_what_shows() {
        let output: Size<i32, Logical> = (870, 600).into();
        let mut v = view(40, 30);
        v.first = 1000;
        v.runs_first = 1000;
        let l = machines_layout(&v, output);
        let shown = machines_shown(41, output);
        assert!(shown < 41 && l.track.is_some());
        assert_eq!(l.rows.len(), shown);
        assert_eq!(l.first, 41 - shown);
        let runs = machine_runs_shown(&v, output);
        assert!(runs < 30 && l.runs_track.is_some());
        assert_eq!(l.runs.len(), runs);
        assert_eq!(l.runs_first, 30 - runs);
        // Everything fits all the same.
        fits(&l, output);
        assert!(l.panel.size.h <= 600 - 2 * MARGIN);
        // A list that all shows doesn't scroll.
        v = view(2, 0);
        v.first = 5;
        v.runs_first = 5;
        let l = machines_layout(&v, output);
        assert_eq!((l.first, l.runs_first), (0, 0));
        assert!(l.runs.is_empty() && l.no_runs.is_some());
        assert_eq!(machines_shown(0, output), 1);
    }

    #[test]
    fn many_runs_scroll_and_a_tiny_output_shows_one_of_each() {
        let output: Size<i32, Logical> = (1920, 1080).into();
        let mut v = view(2, 60);
        v.runs_first = 20;
        let l = machines_layout(&v, output);
        assert!(l.runs.len() < 60 && l.runs_track.is_some());
        assert_eq!(l.runs_first, 20);
        assert_eq!(l.hit(mid(l.runs[0])), Some(MachinesHit::Run(20)));
        fits(&l, output);
        let tiny: Size<i32, Logical> = (400, 200).into();
        assert_eq!(machines_shown(10, tiny), 1);
        assert_eq!(machine_runs_shown(&v, tiny), 1);
    }

    #[test]
    fn the_add_row_has_no_detail_and_problems_wrap() {
        let output: Size<i32, Logical> = (870, 600).into();
        let mut v = view(1, 0);
        v.selected = 1;
        v.detail = None;
        let l = machines_layout(&v, output);
        assert!(l.buttons.is_empty() && l.runs.is_empty() && l.no_runs.is_none());
        assert_eq!(l.hit(mid(l.rows[1])), Some(MachinesHit::Row(1)));
        let mut v = view(1, 0);
        if let Some(d) = v.detail.as_mut() {
            d.problem = Some("It didn't accept any of your SSH keys. ".repeat(4));
            d.how = Some("Add a key with ssh-copy-id ana@box-0.".to_owned());
        }
        let l = machines_layout(&v, output);
        assert!(l.problem.len() > 1 && l.problem.len() <= MOST_LINES);
        assert_eq!(l.how.len(), 1);
        let below = l.how[0].1.loc.y > l.problem.last().unwrap().1.loc.y;
        assert!(below && l.runs_head.loc.y > l.how[0].1.loc.y);
        fits(&l, output);
    }

    #[test]
    fn remove_is_plain_and_never_the_primary() {
        assert_eq!(button_style("Remove", 0, 2), ButtonStyle::Plain);
        assert_eq!(button_style("Remove", 1, 2), ButtonStyle::Plain);
        assert_eq!(button_style("Connect", 1, 2), ButtonStyle::Primary);
        assert_eq!(button_style("Edit", 0, 2), ButtonStyle::Plain);
    }
}
