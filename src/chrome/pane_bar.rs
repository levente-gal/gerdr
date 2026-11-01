//! The bar over each pane: what it shows, its buttons, how it hides and
//! shows, and how it is drawn; and the strip marking the focused pane,
//! whatever it shows.
//!
//! A hidden bar leaves its grip behind: a thin band along the pane's top
//! edge with room of its own, a dot for what runs, the title small and a
//! faint close button, so every pane can be found, closed and held. The
//! pointer on the grip brings the whole bar out, grown from it. A bar out
//! has all its buttons, whatever the pane shows, faint until the pointer
//! is over it.

use super::{
    BAR_BUTTON, BAR_BUTTON_GAP, BAR_INSET, BAR_NOTE_GAP, Canvas, DETAIL_FONT_SIZE, PADDING,
    PANE_BAR_HEIGHT, Painter, Palette, Rect, Rgb, SHADOW, STATUS_GAP, STATUS_WIDTH, band,
    centred_dot, chevron_side, copy, cross, icon_sdf, mix, physical, rect_px, restart_arrow,
    status_mark, stop, text_lines, zoom_corners,
};
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How far from a pane's top edge, above or below it, the pointer brings
/// its hidden bar out.
pub const BAR_REVEAL: i32 = 8;
/// How far a bar slides from under the top edge of its pane as it shows,
/// and back as it hides: all but its grip (see `PANE_GRIP`).
pub const BAR_SLIDE: i32 = PANE_BAR_HEIGHT - PANE_GRIP;
/// The strip along the top edge of the focused pane: this high.
pub const FOCUS_STRIP: i32 = 2;
/// How high a hidden bar's grip is (see the module doc).
pub const PANE_GRIP: i32 = 8;
/// The size of a grip's title.
const GRIP_FONT_SIZE: f32 = 7.5;
/// How big a grip's dot is.
const GRIP_DOT: i32 = 4;

/// A pane's place split in two: its bar on top, and the rest for its
/// window, which keeps at least a pixel. The bar as it is when shown; see
/// `pane_bar_rects_shown` for one that may be hidden.
pub fn pane_bar_rects(pane: Rect) -> (Rect, Rect) {
    pane_bar_rects_shown(pane, true)
}

/// A pane's place split in two: its bar on top, and the rest for its
/// window, which keeps at least a pixel. A hidden bar (`shown` false)
/// keeps only its grip, `PANE_GRIP` high, and the window takes the rest.
pub fn pane_bar_rects_shown(pane: Rect, shown: bool) -> (Rect, Rect) {
    let h = if shown { PANE_BAR_HEIGHT } else { PANE_GRIP };
    let h = h.min(pane.size.h - 1).max(0);
    let bar = Rectangle::new(pane.loc, (pane.size.w, h).into());
    let client = Rectangle::new(
        (pane.loc.x, pane.loc.y + h).into(),
        (pane.size.w, (pane.size.h - h).max(1)).into(),
    );
    (bar, client)
}

/// Whether the pointer at `pos` asks for the bar of the pane at `pane`:
/// within `BAR_REVEAL` of the pane's top edge, or, while the bar is
/// `shown`, anywhere over it.
pub fn pointer_reveals(pane: Rect, shown: bool, pos: Point<f64, Logical>) -> bool {
    let (x, y) = (pos.x, pos.y);
    let (left, right) = (pane.loc.x as f64, (pane.loc.x + pane.size.w) as f64);
    if x < left || x >= right {
        return false;
    }
    let top = pane.loc.y as f64;
    let reach = if shown {
        pane_bar_rects(pane).0.size.h.max(BAR_REVEAL)
    } else {
        BAR_REVEAL
    };
    y >= top - BAR_REVEAL as f64 && y < top + reach as f64
}

/// Where the strip marking the focused pane goes: along the top edge of
/// `pane` (the pane's client rect), never taller than the pane.
pub fn focus_strip_rect(pane: Rect) -> Rect {
    Rectangle::new(
        pane.loc,
        (pane.size.w, FOCUS_STRIP.min(pane.size.h).max(0)).into(),
    )
}

/// The strip marking the focused pane, `size` big: a line in the accent.
pub fn focus_strip(size: Size<i32, Logical>, scale: f64, p: &Palette) -> MemoryRenderBuffer {
    let mut c = Canvas::new(physical(size, scale));
    c.fill(0, 0, c.w, c.h, p.accent);
    c.into_buffer(false)
}

/// The round button floating over a zoomed pane to unzoom it: this big,
/// and this far in from the pane's bottom right corner.
pub const UNZOOM_BUTTON: i32 = 36;
pub const UNZOOM_MARGIN: i32 = 16;

/// Where the button unzooming the pane at `pane` floats: in its bottom
/// right corner, or nowhere when the pane is too small to hold it.
pub fn unzoom_button_rect(pane: Rect) -> Option<Rect> {
    let room = UNZOOM_BUTTON + 2 * UNZOOM_MARGIN;
    (pane.size.w >= room && pane.size.h >= room).then(|| {
        let edge = UNZOOM_MARGIN + UNZOOM_BUTTON;
        Rectangle::new(
            (
                pane.loc.x + pane.size.w - edge,
                pane.loc.y + pane.size.h - edge,
            )
                .into(),
            (UNZOOM_BUTTON, UNZOOM_BUTTON).into(),
        )
    })
}

/// Where the buttons of a bar with `kind`'s sit, left to right: those
/// leading it at its start (see `BarButtons::leading`), the rest at its
/// right end; then the zoom button, if `zoom` (see `zoom_button`); last
/// the close button, on every bar.
pub fn pane_bar_buttons(bar: Rect, kind: BarButtons, zoom: Option<bool>) -> Vec<(BarButton, Rect)> {
    let (leading, trailing) = kind.buttons().split_at(kind.leading());
    let trailing: Vec<_> = trailing
        .iter()
        .copied()
        .chain(zoom_button(zoom))
        .chain([BarButton::Close])
        .collect();
    let step = BAR_BUTTON + BAR_BUTTON_GAP;
    let at = |x: i32| Rectangle::new((x, bar.loc.y).into(), (BAR_BUTTON, bar.size.h).into());
    let left = bar.loc.x + BAR_INSET;
    let n = trailing.len() as i32;
    let right = bar.loc.x + bar.size.w - BAR_INSET - n * step + BAR_BUTTON_GAP;
    let leading = (0..)
        .zip(leading)
        .map(|(i, &button)| (button, at(left + i * step)));
    let trailing = (0..)
        .zip(trailing)
        .map(|(i, button)| (button, at(right + i * step)));
    leading.chain(trailing).collect()
}

/// How far the address field of a web page's bar keeps from the buttons
/// either side of it, and from the bar's top and bottom.
const BAR_FIELD_GAP: i32 = 4;
const BAR_FIELD_INSET: i32 = 3;

/// Where the read-only field with a web page's address sits in a bar with
/// `kind`'s buttons and the zoom button if `zoom`: all the room between
/// the buttons at its start and those at its end. None for a bar that
/// shows no address, or has no room for one.
pub fn pane_bar_field(bar: Rect, kind: BarButtons, zoom: Option<bool>) -> Option<Rect> {
    if kind != BarButtons::Web {
        return None;
    }
    let buttons = pane_bar_buttons(bar, kind, zoom);
    let lead = kind.leading();
    let left = buttons[..lead]
        .last()
        .map_or(bar.loc.x + BAR_INSET, |(_, r)| r.loc.x + r.size.w)
        + BAR_FIELD_GAP;
    let right = buttons
        .get(lead)
        .map_or(bar.loc.x + bar.size.w - BAR_INSET, |(_, r)| r.loc.x)
        - BAR_FIELD_GAP;
    let h = bar.size.h - 2 * BAR_FIELD_INSET;
    (right - left >= BAR_BUTTON && h > 0).then(|| {
        Rectangle::new(
            (left, bar.loc.y + BAR_FIELD_INSET).into(),
            (right - left, h).into(),
        )
    })
}

/// The zoom button of a bar whose pane is zoomed (`Some(true)`) or can be
/// (`Some(false)`); none for a pane alone in its tab (`None`).
fn zoom_button(zoom: Option<bool>) -> Option<BarButton> {
    zoom.map(|zoomed| {
        if zoomed {
            BarButton::Unzoom
        } else {
            BarButton::Zoom
        }
    })
}

/// What of a pane's bar, with `kind`'s buttons and the zoom button if
/// `zoom`, is at `pos`, if the bar is.
pub fn pane_bar_hit(
    bar: Rect,
    kind: BarButtons,
    zoom: Option<bool>,
    pos: Point<f64, Logical>,
) -> Option<BarHit> {
    if !bar.to_f64().contains(pos) {
        return None;
    }
    let button = pane_bar_buttons(bar, kind, zoom)
        .into_iter()
        .find(|(_, rect)| rect.to_f64().contains(pos));
    Some(button.map_or(BarHit::Label, |(button, _)| BarHit::Button(button)))
}

/// A button in a pane's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButton {
    Scrollback,
    Restart,
    Stop,
    Back,
    Forward,
    Reload,
    CopyUrl,
    /// Zoom the pane to fill its tab.
    Zoom,
    /// Give the tab's other panes their places back.
    Unzoom,
    /// Close the pane.
    Close,
}

/// Which buttons a pane's bar has, by what the pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarButtons {
    /// An app's window: none.
    #[default]
    None,
    /// A terminal: its scrollback, and restart and stop what runs in it.
    Terminal,
    /// A web page: back, forward and reload at the start, its address in
    /// a field filling the rest, and copy it at the end.
    Web,
}

impl BarButtons {
    /// The buttons, left to right.
    pub fn buttons(self) -> &'static [BarButton] {
        match self {
            Self::None => &[],
            Self::Terminal => &[BarButton::Scrollback, BarButton::Restart, BarButton::Stop],
            Self::Web => &[
                BarButton::Back,
                BarButton::Forward,
                BarButton::Reload,
                BarButton::CopyUrl,
            ],
        }
    }

    /// How many of the buttons lead the bar, at its start; the rest are
    /// at its end.
    pub fn leading(self) -> usize {
        match self {
            Self::Web => 3,
            Self::None | Self::Terminal => 0,
        }
    }
}

/// What a click on a pane's bar lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Label,
    Button(BarButton),
}

/// What a pane's bar says of its command, by the mark before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStatus {
    /// The shell waits at its prompt, and nothing ran in it to run again.
    Idle,
    Running,
    /// The command ran and is done: it can run again.
    Exited,
    /// Being stopped or restarted.
    Working,
}

/// The bar over a pane: what runs in it or what it shows, its buttons, and
/// where it stands between hidden and shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneBarView {
    /// Where the bar goes on the output when shown in full; no height while
    /// it is hidden and still.
    pub rect: Rect,
    /// What runs, or a web page's address.
    pub label: String,
    /// A word after the label, such as "exited" or "stopping…".
    pub note: Option<&'static str>,
    pub status: BarStatus,
    pub can_restart: bool,
    pub can_stop: bool,
    /// A web page's history goes back, or forward, from where it is.
    pub can_back: bool,
    pub can_forward: bool,
    /// Which buttons it has, by what the pane shows.
    pub buttons: BarButtons,
    /// Its pane is zoomed (`Some(true)`) or can be: the zoom button.
    pub zoom: Option<bool>,
    /// The pane has keyboard focus.
    pub focused: bool,
    /// The pointer is over the bar: its buttons show.
    pub lit: bool,
    /// The button under the pointer.
    pub hover: Option<BarButton>,
    /// The bar is out: with room of its own over its window (see
    /// `pane_bar_rects_shown`) or drawn over the window's top. While
    /// false with a height, it is still fading out.
    pub visible: bool,
    /// Its opacity, 0 to 1, from the fade showing or hiding it; 1 at rest.
    pub alpha: f32,
    /// How far it is still under its pane's top edge, in logical pixels,
    /// while it slides in or out; 0 at rest.
    pub slide: i32,
}

impl PaneBarView {
    /// Where the bar is drawn right now, how big, and how opaque: slid
    /// under the top edge by `slide`, so only the rest shows.
    pub fn placement(&self) -> (Point<i32, Logical>, Size<i32, Logical>, f32) {
        let h = (self.rect.size.h - self.slide).max(0);
        (self.rect.loc, (self.rect.size.w, h).into(), self.alpha)
    }

    /// Whether no more than its grip shows: it is drawn as the grip.
    pub fn is_grip(&self) -> bool {
        let h = self.placement().1.h;
        h > 0 && h <= PANE_GRIP && self.rect.size.h > PANE_GRIP
    }

    /// Where its mark goes on the output: at its start, `slide` up with
    /// the rest of it.
    fn mark(&self) -> Rect {
        Rectangle::new(
            (
                self.rect.loc.x + PADDING as i32,
                self.rect.loc.y - self.slide,
            )
                .into(),
            (STATUS_WIDTH, self.rect.size.h).into(),
        )
    }

    /// Where its blinking dot goes on the output, while its command runs
    /// and the dot is not slid under its pane's top edge.
    pub fn blinker(&self) -> Option<Rect> {
        let dot = centred_dot(self.mark());
        (self.status == BarStatus::Running
            && self.rect.size.h > 0
            && !self.is_grip()
            && dot.loc.y >= self.rect.loc.y)
            .then_some(dot)
    }

    /// The same bar at full opacity: what its buffer looks like, for a
    /// cache key (the alpha is applied when it is composed).
    pub fn still(&self) -> Self {
        Self {
            alpha: 1.0,
            ..self.clone()
        }
    }
}

impl Painter {
    /// The button floating over a zoomed pane: a round panel with its
    /// shadow and the unzoom icon, lit while `hovered`.
    pub(super) fn unzoom_button(
        &self,
        hovered: bool,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = Size::from((UNZOOM_BUTTON, UNZOOM_BUTTON));
        let mut c = self.popup(size, UNZOOM_BUTTON as f32 / 2.0, scale, p);
        // The icon a little bigger than on the bars, to fill the disc.
        let s = scale as f32 * 1.25;
        let button = Rectangle::new((SHADOW, SHADOW).into(), size);
        if hovered {
            let inset = Rectangle::new(
                (SHADOW + 3, SHADOW + 3).into(),
                (UNZOOM_BUTTON - 6, UNZOOM_BUTTON - 6).into(),
            );
            let (x, y, w, h) = rect_px(inset, scale);
            c.rounded(x, y, w, h, w / 2.0, p.selected);
        }
        let color = if hovered { p.text } else { p.muted };
        icon_sdf(&mut c, button, scale, color, |x, y| {
            zoom_corners(true, x, y, s)
        });
        c.into_buffer(false)
    }

    /// A pane's bar: a mark saying how its command is, the command (or the
    /// app's title, cut in the middle so it keeps its end) and a note
    /// after it, then its buttons, faint unless the pointer is over the
    /// bar. A web page's has its address in a field between its buttons.
    /// Drawn `view.slide` pixels up, cut off at the top; no more than its
    /// grip showing, drawn as the grip (see `pane_grip`). A focused pane's
    /// ends in a line in the accent.
    pub(super) fn pane_bar(
        &self,
        view: &PaneBarView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let (_, size, _) = view.placement();
        let full = view.rect.size;
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let line = px(1).max(1);
        if view.is_grip() {
            self.pane_grip(&mut c, view, scale, p);
            return c.into_buffer(true);
        }
        let background = if view.focused {
            p.tab_active
        } else {
            p.tab_bar
        };
        c.fill(0, 0, c.w, c.h, background);
        if view.focused {
            let accent = px(FOCUS_STRIP).max(1);
            c.fill(0, c.h - accent, c.w, accent, p.accent);
        } else {
            c.fill(0, c.h - line, c.w, line, p.border_variant);
        }

        // Everything is laid out in the full bar, then moved up by the slide.
        let up = -view.slide;
        let bar = Rectangle::new((0, up).into(), full);
        let buttons = pane_bar_buttons(bar, view.buttons, view.zoom);
        if view.buttons == BarButtons::Web {
            if let Some(field) = pane_bar_field(bar, view.buttons, view.zoom) {
                let (x, y, w, h) = rect_px(field, scale);
                let (b, r) = (line as f32, 4.0 * s);
                c.rounded(x, y, w, h, r, p.border);
                let fill = mix(p.elevated, p.element, 0.6);
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, fill);
                let pad = px(6) as f32;
                let color = if view.focused { p.text } else { p.muted };
                let band = band(field, scale);
                let label = &view.label;
                self.text(
                    &mut c,
                    label,
                    DETAIL_FONT_SIZE,
                    x + pad,
                    x + w - pad,
                    band,
                    scale,
                    color,
                );
            }
            self.bar_buttons(&mut c, view, buttons, background, scale, p);
            return c.into_buffer(true);
        }
        let mark = Rectangle::new((PADDING as i32, up).into(), (STATUS_WIDTH, full.h).into());
        // The marks the sidebar's agent chips use: a ring for waiting, an
        // open ring for busy. A running command's is a blinking dot,
        // composed over the bar (see `blinker`).
        match view.status {
            BarStatus::Idle | BarStatus::Running => {}
            BarStatus::Exited => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                status_mark(AgentStatus::Idle, x, y, s)
            }),
            // Busy, as an agent busy is everywhere else: in the accent.
            BarStatus::Working => icon_sdf(&mut c, mark, scale, p.accent, |x, y| {
                status_mark(AgentStatus::Working, x, y, s)
            }),
        }

        // The note keeps its room; the label is cut short before it.
        let band = band(bar, scale);
        let x = px(mark.loc.x + STATUS_WIDTH + STATUS_GAP) as f32;
        let end_x = buttons
            .first()
            .map_or(full.w - BAR_INSET, |(_, first)| first.loc.x);
        let right = px(end_x - BAR_NOTE_GAP) as f32;
        let note_w = view.note.map_or(0.0, |note| {
            self.text_width(note, DETAIL_FONT_SIZE, scale) + px(BAR_NOTE_GAP) as f32
        });
        // An unfocused bar is all muted; a focused one says more.
        let (label_color, note_color) = match view.status {
            _ if !view.focused => (p.muted, p.placeholder),
            BarStatus::Running => (p.text, p.muted),
            BarStatus::Working => (p.text, p.accent),
            // Focused, it says what it is plainly enough to read.
            BarStatus::Idle | BarStatus::Exited => (p.text, p.muted),
        };
        let label_right = (right - note_w).max(x);
        let label = self.cut_middle(&view.label, DETAIL_FONT_SIZE, label_right - x, scale);
        let end = self.text(
            &mut c,
            &label,
            DETAIL_FONT_SIZE,
            x,
            label_right,
            band,
            scale,
            label_color,
        );
        if let Some(note) = view.note {
            self.text(
                &mut c,
                note,
                DETAIL_FONT_SIZE,
                end + px(BAR_NOTE_GAP) as f32,
                right,
                band,
                scale,
                note_color,
            );
        }

        self.bar_buttons(&mut c, view, buttons, background, scale, p);
        c.into_buffer(true)
    }

    /// A hidden pane's grip over the whole of `c`: a dot while something
    /// runs or has run, the title small and cut in the middle, and the
    /// close button faint at its end. A focused pane's is tinted with the
    /// accent and ends in an accent line, so the focus shows from afar.
    fn pane_grip(&self, c: &mut Canvas, view: &PaneBarView, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (_, size, _) = view.placement();
        let background = if view.focused {
            mix(p.tab_bar, p.accent, 0.22)
        } else {
            p.tab_bar
        };
        c.fill(0, 0, c.w, c.h, background);
        let line = px(1).max(1);
        let edge = if view.focused {
            p.accent
        } else {
            p.border_variant
        };
        c.fill(0, c.h - line, c.w, line, edge);
        let grip = Rectangle::new((0, 0).into(), size);
        let dot = match view.status {
            BarStatus::Running | BarStatus::Working => Some(p.accent),
            BarStatus::Exited => Some(p.placeholder),
            BarStatus::Idle => None,
        };
        if let Some(color) = dot {
            let at = Rectangle::new(
                (
                    PADDING as i32 + (STATUS_WIDTH - GRIP_DOT) / 2,
                    (size.h - GRIP_DOT) / 2,
                )
                    .into(),
                (GRIP_DOT, GRIP_DOT).into(),
            );
            let (x, y, w, h) = rect_px(at, scale);
            c.rounded(x, y, w, h, w / 2.0, color);
        }
        let close = pane_bar_buttons(grip, BarButtons::None, None)
            .into_iter()
            .find(|(b, _)| *b == BarButton::Close)
            .map(|(_, r)| r)
            .unwrap_or(grip);
        let x = px(PADDING as i32 + STATUS_WIDTH + STATUS_GAP) as f32;
        let right = px(close.loc.x - BAR_NOTE_GAP) as f32;
        let label = self.cut_middle(&view.label, GRIP_FONT_SIZE, right - x, scale);
        let color = if view.focused { p.text } else { p.muted };
        let band = band(grip, scale);
        self.text(c, &label, GRIP_FONT_SIZE, x, right, band, scale, color);
        let faint = mix(p.muted, background, 0.4);
        icon_sdf(c, close, scale, faint, |x, y| cross(x, y, 0.55 * s));
    }

    /// `text` cut to fit `width` (physical pixels) at `size`: whole if it
    /// fits, else its start and its end either side of "…", so a command
    /// keeps its last arguments as well as its name.
    fn cut_middle(&self, text: &str, size: f32, width: f32, scale: f64) -> String {
        if self.text_width(text, size, scale) <= width {
            return text.to_owned();
        }
        let chars: Vec<char> = text.chars().collect();
        let cut = |n: usize| -> String {
            let head = n.div_ceil(2);
            let tail = n - head;
            let mut s: String = chars[..head].iter().collect();
            s.push('…');
            s.extend(&chars[chars.len() - tail..]);
            s
        };
        (1..chars.len())
            .rev()
            .map(cut)
            .find(|s| self.text_width(s, size, scale) <= width)
            // Not even a character either side: the start, cut as `text` cuts.
            .unwrap_or_else(|| text.to_owned())
    }

    /// The buttons of a pane's bar, at `buttons`, over its `background`.
    fn bar_buttons(
        &self,
        c: &mut Canvas,
        view: &PaneBarView,
        buttons: Vec<(BarButton, Rect)>,
        background: Rgb,
        scale: f64,
        p: &Palette,
    ) {
        let s = scale as f32;
        // Faint until the pointer is over the bar, so a bar that stays
        // out over a running command doesn't shout.
        let rest = if view.lit {
            p.muted
        } else {
            mix(p.muted, background, 0.35)
        };
        for (button, rect) in buttons {
            let enabled = match button {
                BarButton::Restart => view.can_restart,
                BarButton::Stop => view.can_stop,
                BarButton::Back => view.can_back,
                BarButton::Forward => view.can_forward,
                BarButton::Scrollback
                | BarButton::Reload
                | BarButton::CopyUrl
                | BarButton::Zoom
                | BarButton::Unzoom
                | BarButton::Close => true,
            };
            let hovered = enabled && view.hover == Some(button);
            if hovered {
                let inset = Rectangle::new(
                    (rect.loc.x + 2, rect.loc.y + 2).into(),
                    (rect.size.w - 4, (rect.size.h - 4).max(0)).into(),
                );
                let (x, y, w, h) = rect_px(inset, scale);
                c.rounded(x, y, w, h, 4.0 * s, p.selected);
            }
            let color = if !enabled {
                mix(p.placeholder, background, 0.5)
            } else if hovered {
                p.text
            } else {
                rest
            };
            match button {
                BarButton::Restart | BarButton::Reload => {
                    icon_sdf(c, rect, scale, color, |x, y| restart_arrow(x, y, s))
                }
                BarButton::Stop => icon_sdf(c, rect, scale, color, |x, y| stop(x, y, s)),
                BarButton::Back | BarButton::Forward => {
                    let left = button == BarButton::Back;
                    icon_sdf(c, rect, scale, color, |x, y| chevron_side(left, x, y, s))
                }
                BarButton::CopyUrl => icon_sdf(c, rect, scale, color, |x, y| copy(x, y, s)),
                BarButton::Scrollback => {
                    icon_sdf(c, rect, scale, color, |x, y| text_lines(x, y, s))
                }
                BarButton::Zoom | BarButton::Unzoom => {
                    let unzoom = button == BarButton::Unzoom;
                    icon_sdf(c, rect, scale, color, |x, y| zoom_corners(unzoom, x, y, s))
                }
                BarButton::Close => icon_sdf(c, rect, scale, color, |x, y| cross(x, y, 0.8 * s)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BAR_REVEAL, BAR_SLIDE, BarButton, BarButtons, BarHit, BarStatus, FOCUS_STRIP, PANE_GRIP,
        PaneBarView, UNZOOM_BUTTON, UNZOOM_MARGIN, focus_strip, focus_strip_rect, pane_bar_buttons,
        pane_bar_field, pane_bar_hit, pane_bar_rects, pane_bar_rects_shown, pointer_reveals,
        unzoom_button_rect,
    };
    use crate::chrome::{BAR_INSET, PANE_BAR_HEIGHT, Painter, Rect, Theme};

    fn view(w: i32) -> PaneBarView {
        PaneBarView {
            rect: Rect::new((0, 0).into(), (w, PANE_BAR_HEIGHT).into()),
            label: "npm run dev -- --port 3000 --host 0.0.0.0".into(),
            note: None,
            status: BarStatus::Running,
            can_restart: true,
            can_stop: true,
            can_back: true,
            can_forward: false,
            buttons: BarButtons::Terminal,
            zoom: None,
            focused: true,
            lit: true,
            hover: None,
            visible: true,
            alpha: 1.0,
            slide: 0,
        }
    }

    #[test]
    fn pane_bar_takes_the_top_and_leaves_the_rest_to_the_window() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects(pane);
        assert_eq!(
            bar,
            Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into())
        );
        assert_eq!(
            client,
            Rect::new(
                (200, 50 + PANE_BAR_HEIGHT).into(),
                (600, 400 - PANE_BAR_HEIGHT).into()
            )
        );
        assert_eq!(pane_bar_rects_shown(pane, true), (bar, client));
    }

    #[test]
    fn the_unzoom_button_floats_in_the_bottom_right_corner() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let button = unzoom_button_rect(pane).unwrap();
        assert_eq!(button.size.w, UNZOOM_BUTTON);
        assert_eq!(button.loc.x + UNZOOM_BUTTON + UNZOOM_MARGIN, 800);
        assert_eq!(button.loc.y + UNZOOM_BUTTON + UNZOOM_MARGIN, 450);
        let tiny = Rect::new((0, 0).into(), (UNZOOM_BUTTON, 400).into());
        assert_eq!(unzoom_button_rect(tiny), None);
        let painter = Painter::new();
        for theme in [Theme::Dark, Theme::Light] {
            painter.unzoom_button(true, 2.0, theme.palette());
        }
    }

    #[test]
    fn a_hidden_bar_keeps_its_grip_and_the_window_takes_the_rest() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects_shown(pane, false);
        assert_eq!(bar, Rect::new((200, 50).into(), (600, PANE_GRIP).into()));
        assert_eq!(
            client,
            Rect::new((200, 50 + PANE_GRIP).into(), (600, 400 - PANE_GRIP).into())
        );
        // A pane too small for a grip keeps its window.
        let tiny = Rect::new((0, 0).into(), (300, 1).into());
        assert_eq!(pane_bar_rects_shown(tiny, false).1, tiny);
    }

    #[test]
    fn a_bar_at_rest_hidden_shows_as_its_grip() {
        let mut v = view(600);
        v.slide = BAR_SLIDE;
        assert_eq!(v.placement().1.h, PANE_GRIP);
        assert!(v.is_grip());
        assert_eq!(v.blinker(), None);
        v.slide = 0;
        assert!(!v.is_grip());
        // A grip's close button is where the bar's is.
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let grip = pane_bar_rects_shown(pane, false).0;
        let end = (200.0 + 600.0 - BAR_INSET as f64 - 4.0, 52.0).into();
        assert_eq!(
            pane_bar_hit(grip, BarButtons::None, None, end),
            Some(BarHit::Button(BarButton::Close))
        );
        assert_eq!(
            pane_bar_hit(grip, BarButtons::None, None, (300.0, 52.0).into()),
            Some(BarHit::Label)
        );
    }

    #[test]
    fn a_cut_label_keeps_its_end() {
        let painter = Painter::new();
        let text = "npm run dev -- --port 3000 --host 0.0.0.0";
        let full = painter.text_width(text, 12.0, 1.0);
        assert_eq!(painter.cut_middle(text, 12.0, full + 1.0, 1.0), text);
        if full > 0.0 {
            let cut = painter.cut_middle(text, 12.0, full / 2.0, 1.0);
            assert!(cut.starts_with("npm") && cut.ends_with("0.0"), "{cut}");
            assert!(cut.contains('…'));
            assert!(painter.text_width(&cut, 12.0, 1.0) <= full / 2.0);
        }
    }

    #[test]
    fn tiny_pane_keeps_a_window() {
        for h in [0, 1, 10, PANE_BAR_HEIGHT] {
            let pane = Rect::new((0, 0).into(), (300, h).into());
            let (bar, client) = pane_bar_rects(pane);
            assert!(client.size.h >= 1, "{h}");
            assert!(bar.size.h >= 0 && bar.size.h < h.max(1), "{h}");
        }
    }

    #[test]
    fn the_pointer_near_the_top_edge_reveals_the_bar() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let at = |x: f64, y: f64| (x, y).into();
        // Hidden: a band around the edge, inside the pane's width.
        assert!(pointer_reveals(pane, false, at(300.0, 50.0)));
        assert!(pointer_reveals(
            pane,
            false,
            at(300.0, 50.0 - BAR_REVEAL as f64)
        ));
        assert!(pointer_reveals(pane, false, at(300.0, 57.9)));
        assert!(!pointer_reveals(pane, false, at(300.0, 58.0)));
        assert!(!pointer_reveals(pane, false, at(300.0, 41.0)));
        assert!(!pointer_reveals(pane, false, at(199.0, 50.0)));
        assert!(!pointer_reveals(pane, false, at(800.0, 50.0)));
        // Shown: the whole bar keeps it, and the band above still counts.
        assert!(pointer_reveals(pane, true, at(300.0, 73.0)));
        assert!(pointer_reveals(pane, true, at(300.0, 45.0)));
        assert!(!pointer_reveals(pane, true, at(300.0, 74.0)));
        // A pane too short for a bar still has its band.
        let short = Rect::new((0, 100).into(), (300, 4).into());
        assert!(pointer_reveals(short, true, at(10.0, 105.0)));
        assert!(!pointer_reveals(short, true, at(10.0, 108.0)));
    }

    #[test]
    fn the_focus_strip_lies_along_the_top_edge() {
        let pane = Rect::new((200, 74).into(), (600, 376).into());
        assert_eq!(
            focus_strip_rect(pane),
            Rect::new((200, 74).into(), (600, FOCUS_STRIP).into())
        );
        let flat = Rect::new((0, 0).into(), (100, 1).into());
        assert_eq!(focus_strip_rect(flat).size.h, 1);
        assert_eq!(focus_strip_rect(Rect::default()).size.h, 0);
        for scale in [1.0, 1.5, 2.0] {
            for theme in [Theme::Dark, Theme::Light] {
                focus_strip((600, FOCUS_STRIP).into(), scale, theme.palette());
            }
        }
    }

    #[test]
    fn a_web_bar_leads_with_its_steps_and_fills_the_rest_with_its_address() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        for zoom in [None, Some(false)] {
            let buttons = pane_bar_buttons(bar, BarButtons::Web, zoom);
            let (_, reload) = buttons[2];
            assert_eq!(buttons[0].1.loc.x, 200 + BAR_INSET);
            assert_eq!(buttons[3].0, BarButton::CopyUrl);
            let field = pane_bar_field(bar, BarButtons::Web, zoom).unwrap();
            assert!(field.loc.x >= reload.loc.x + reload.size.w);
            assert!(field.loc.x + field.size.w <= buttons[3].1.loc.x);
            assert!(field.loc.y > 50 && field.loc.y + field.size.h < 50 + PANE_BAR_HEIGHT);
            // The field is the label: a click there holds the pane.
            let mid = field.to_f64().loc + field.to_f64().size.downscale(2.0).to_point();
            assert_eq!(
                pane_bar_hit(bar, BarButtons::Web, zoom, mid),
                Some(BarHit::Label)
            );
        }
        // Copy sits just before the close button, which ends the bar.
        let buttons = pane_bar_buttons(bar, BarButtons::Web, None);
        let order: Vec<_> = buttons[3..].iter().map(|(b, _)| *b).collect();
        assert_eq!(order, [BarButton::CopyUrl, BarButton::Close]);
        let (_, close) = buttons.last().unwrap();
        assert_eq!(close.loc.x + close.size.w, 200 + 600 - BAR_INSET);
        assert_eq!(pane_bar_field(bar, BarButtons::Terminal, None), None);
        let narrow = Rect::new((0, 0).into(), (120, PANE_BAR_HEIGHT).into());
        assert_eq!(pane_bar_field(narrow, BarButtons::Web, Some(false)), None);
    }

    #[test]
    fn pane_bar_buttons_do_not_overlap() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        let kinds = [BarButtons::Terminal, BarButtons::Web, BarButtons::None];
        for (kind, zoom) in kinds
            .into_iter()
            .flat_map(|k| [(k, Some(false)), (k, Some(true))])
        {
            let buttons = pane_bar_buttons(bar, kind, zoom);
            let order: Vec<_> = buttons.iter().map(|(b, _)| *b).collect();
            // The zoom button, then the close button at the bar's end.
            let zoom_button = if zoom == Some(true) {
                BarButton::Unzoom
            } else {
                BarButton::Zoom
            };
            assert_eq!(
                order,
                [kind.buttons(), &[zoom_button, BarButton::Close]].concat()
            );
            assert!(kind.leading() < order.len());
            let (_, last) = buttons.last().unwrap();
            assert_eq!(last.loc.x + last.size.w, 200 + 600 - BAR_INSET);
            for pair in buttons.windows(2) {
                assert!(pair[0].1.loc.x + pair[0].1.size.w <= pair[1].1.loc.x);
            }
            assert!(
                buttons
                    .iter()
                    .all(|(_, r)| r.loc.y == 50 && r.size.h == PANE_BAR_HEIGHT)
            );
        }
        // A pane alone in its tab has no zoom button; every pane can close.
        let order = |kind| -> Vec<_> {
            pane_bar_buttons(bar, kind, None)
                .into_iter()
                .map(|(b, _)| b)
                .collect()
        };
        assert_eq!(order(BarButtons::None), [BarButton::Close]);
        assert_eq!(
            order(BarButtons::Terminal),
            [BarButtons::Terminal.buttons(), &[BarButton::Close]].concat()
        );
    }

    #[test]
    fn pane_bar_hit_agrees_with_the_buttons() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        for kind in [BarButtons::Terminal, BarButtons::Web] {
            for zoom in [None, Some(false), Some(true)] {
                for (button, rect) in pane_bar_buttons(bar, kind, zoom) {
                    let centre = rect.to_f64().loc + rect.to_f64().size.downscale(2.0).to_point();
                    assert_eq!(
                        pane_bar_hit(bar, kind, zoom, centre),
                        Some(BarHit::Button(button))
                    );
                    // An app's bar has only its close button, at the end.
                    let app = match button {
                        BarButton::Close => BarHit::Button(BarButton::Close),
                        _ => BarHit::Label,
                    };
                    assert_eq!(pane_bar_hit(bar, BarButtons::None, None, centre), Some(app));
                }
            }
        }
        let kind = BarButtons::Terminal;
        assert_eq!(
            pane_bar_hit(bar, kind, None, (210.0, 60.0).into()),
            Some(BarHit::Label)
        );
        assert_eq!(pane_bar_hit(bar, kind, None, (210.0, 80.0).into()), None);
        assert_eq!(pane_bar_hit(bar, kind, None, (100.0, 60.0).into()), None);
        // A hidden bar has nothing to hit.
        let hidden = Rect::new((200, 50).into(), (600, 0).into());
        assert_eq!(pane_bar_hit(hidden, kind, None, (210.0, 50.0).into()), None);
    }

    #[test]
    fn a_sliding_bar_shows_only_what_is_past_the_edge() {
        let mut v = view(600);
        v.alpha = 0.5;
        v.slide = 10;
        let (loc, size, alpha) = v.placement();
        assert_eq!(loc, (0, 0).into());
        assert_eq!(size, (600, PANE_BAR_HEIGHT - 10).into());
        assert_eq!(alpha, 0.5);
        v.slide = PANE_BAR_HEIGHT + 5;
        assert_eq!(v.placement().1.h, 0);
        // The cache key forgets the alpha, not the slide.
        let still = v.still();
        assert_eq!((still.alpha, still.slide), (1.0, PANE_BAR_HEIGHT + 5));
    }

    #[test]
    fn a_running_bar_blinks_its_dot_where_its_mark_is() {
        let mut v = view(600);
        v.rect.loc = (200, 50).into();
        v.status = BarStatus::Running;
        let dot = v.blinker().expect("a running bar blinks");
        assert_eq!(dot.size, (crate::chrome::DOT, crate::chrome::DOT).into());
        assert!(dot.loc.x > 200 && dot.loc.x < 200 + super::PADDING as i32 + super::STATUS_WIDTH);
        let mid = dot.loc.y + dot.size.h / 2;
        assert!((mid - (50 + PANE_BAR_HEIGHT / 2)).abs() <= 1);
        // Slid up with the bar, while it stays past the edge.
        v.slide = 2;
        assert_eq!(v.blinker().map(|d| d.loc.y), Some(dot.loc.y - 2));
        v.slide = PANE_BAR_HEIGHT / 2;
        assert_eq!(v.blinker(), None);
        v.slide = 0;
        v.status = BarStatus::Exited;
        assert_eq!(v.blinker(), None);
    }

    #[test]
    fn pane_bars_paint() {
        let painter = Painter::new();
        let statuses = [
            BarStatus::Idle,
            BarStatus::Running,
            BarStatus::Exited,
            BarStatus::Working,
        ];
        for theme in [Theme::Dark, Theme::Light] {
            for scale in [1.0, 1.25, 2.0] {
                for w in [600, 60, 1] {
                    for (i, status) in statuses.into_iter().enumerate() {
                        let view = PaneBarView {
                            note: (i % 2 == 0).then_some("restarting…"),
                            status,
                            can_restart: i < 2,
                            can_stop: i % 2 == 1,
                            focused: i == 0,
                            buttons: [
                                BarButtons::Terminal,
                                BarButtons::Web,
                                BarButtons::None,
                                BarButtons::Web,
                            ][i],
                            zoom: [None, Some(false), Some(true), Some(false)][i],
                            can_back: i < 2,
                            lit: i != 3,
                            hover: Some(BarButton::Restart),
                            slide: [0, 7, BAR_SLIDE, PANE_BAR_HEIGHT][i],
                            ..view(w)
                        };
                        let buffer = painter.pane_bar(&view, scale, theme.palette());
                        drop(buffer);
                    }
                }
            }
        }
    }
}
