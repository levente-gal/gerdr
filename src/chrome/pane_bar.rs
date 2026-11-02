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
//!
//! A web page's or an app's bar floats instead (see `BarButtons::floats`):
//! hidden, its grip is a bare band with a handle in its middle; out, it is
//! a pill dropped from the handle over the top of the window, as wide as
//! it needs, so the page's own header still shows either side of it.

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
/// The handle in the middle of a floating bar's grip: this wide and high.
const HANDLE_W: i32 = 32;
const HANDLE_H: i32 = 3;
/// How far an unfocused pane's handle fades toward its grip, 0 to 1.
const HANDLE_FADE: f32 = 0.3;
/// How high a floating bar's pill is, how far below its pane's top edge
/// it hangs, and how far in from the pill's ends its buttons sit.
const BAR_PILL_HEIGHT: i32 = 30;
const PILL_TOP: i32 = 3;
const PILL_PAD: i32 = 4;
/// How wide a web page's pill is: this share of its pane, no less than
/// `PILL_WEB_MIN` nor more than `PILL_WEB_MAX`; an app's is `PILL_APP`.
const PILL_WEB_SHARE: f64 = 0.6;
const PILL_WEB_MIN: i32 = 420;
const PILL_WEB_MAX: i32 = 760;
const PILL_APP: i32 = 340;
/// How far a pill keeps from its pane's sides, and how far it drops as
/// it shows.
const PILL_MARGIN: i32 = 8;
const PILL_DROP: i32 = 6;

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

/// Where the bar of the pane at `pane` with `kind`'s buttons is when out:
/// along its top for a terminal (see `pane_bar_rects`); a pill hanging in
/// the middle of its top edge for a bar that floats.
pub fn pane_bar_rect(pane: Rect, kind: BarButtons) -> Rect {
    if !kind.floats() {
        return pane_bar_rects(pane).0;
    }
    let room = (pane.size.w - 2 * PILL_MARGIN).max(0);
    let want = match kind {
        BarButtons::Web { .. } => {
            ((pane.size.w as f64 * PILL_WEB_SHARE).round() as i32).clamp(PILL_WEB_MIN, PILL_WEB_MAX)
        }
        _ => PILL_APP,
    };
    let w = want.min(room);
    let h = BAR_PILL_HEIGHT.min(pane.size.h - PILL_TOP - 1).max(0);
    Rectangle::new(
        (pane.loc.x + (pane.size.w - w) / 2, pane.loc.y + PILL_TOP).into(),
        (w, h).into(),
    )
}

/// The grip of the pane at `pane`: the band along its top edge a hidden
/// bar leaves (see `pane_bar_rects_shown`).
pub fn pane_grip_rect(pane: Rect) -> Rect {
    pane_bar_rects_shown(pane, false).0
}

/// Whether the pointer at `pos` asks for the bar of the pane at `pane`
/// with `kind`'s buttons: within `BAR_REVEAL` of the pane's top edge, or,
/// while the bar is `shown`, anywhere over it; for a floating bar, over
/// its pill and the shadow round it.
pub fn pointer_reveals(
    pane: Rect,
    kind: BarButtons,
    shown: bool,
    pos: Point<f64, Logical>,
) -> bool {
    let (x, y) = (pos.x, pos.y);
    let (left, right) = (pane.loc.x as f64, (pane.loc.x + pane.size.w) as f64);
    if x < left || x >= right {
        return false;
    }
    let top = pane.loc.y as f64;
    if shown && kind.floats() {
        let pill = pane_bar_rect(pane, kind);
        let around = Rectangle::new(
            (pill.loc.x - PILL_MARGIN, pane.loc.y).into(),
            (
                pill.size.w + 2 * PILL_MARGIN,
                pill.loc.y - pane.loc.y + pill.size.h + PILL_MARGIN,
            )
                .into(),
        );
        let reach = PANE_GRIP.max(BAR_REVEAL) as f64;
        return around.to_f64().contains(pos) || y >= top - BAR_REVEAL as f64 && y < top + reach;
    }
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
///
/// A floating bar's sit inside its pill, square, clear of its round ends.
pub fn pane_bar_buttons(bar: Rect, kind: BarButtons, zoom: Option<bool>) -> Vec<(BarButton, Rect)> {
    let bar = if kind.floats() {
        let inset_y = ((bar.size.h - BAR_BUTTON) / 2).max(0);
        Rectangle::new(
            (bar.loc.x + PILL_PAD, bar.loc.y + inset_y).into(),
            ((bar.size.w - 2 * PILL_PAD).max(0), bar.size.h - 2 * inset_y).into(),
        )
    } else {
        bar
    };
    lay_out_buttons(bar, kind, zoom)
}

/// The buttons of a bar with `kind`'s, laid out edge to edge over `bar`
/// (see `pane_bar_buttons`).
fn lay_out_buttons(bar: Rect, kind: BarButtons, zoom: Option<bool>) -> Vec<(BarButton, Rect)> {
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
    if !matches!(kind, BarButtons::Web { .. }) {
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
    // In a pill, the field is as high as the buttons either side of it.
    let inset = buttons
        .first()
        .map_or(BAR_FIELD_INSET, |(_, r)| r.loc.y - bar.loc.y + 1);
    let h = bar.size.h - 2 * inset;
    (right - left >= BAR_BUTTON && h > 0)
        .then(|| Rectangle::new((left, bar.loc.y + inset).into(), (right - left, h).into()))
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

/// Where a hidden terminal bar's grip has its close button: at its end,
/// where the bar's is.
fn grip_close(grip: Rect) -> Rect {
    lay_out_buttons(grip, BarButtons::None, None)
        .into_iter()
        .find(|(b, _)| *b == BarButton::Close)
        .map_or(grip, |(_, r)| r)
}

/// What of a hidden terminal bar's `grip` is at `pos`, if the grip is: its
/// close button, or else its label.
pub fn pane_grip_hit(grip: Rect, pos: Point<f64, Logical>) -> Option<BarHit> {
    if !grip.to_f64().contains(pos) {
        return None;
    }
    Some(if grip_close(grip).to_f64().contains(pos) {
        BarHit::Button(BarButton::Close)
    } else {
        BarHit::Label
    })
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
    /// A web page: back and forward (while its page has a `history` to
    /// go through) and reload at the start, its address in a field
    /// filling the rest, and copy it at the end.
    Web { history: bool },
}

impl BarButtons {
    /// The buttons, left to right.
    pub fn buttons(self) -> &'static [BarButton] {
        match self {
            Self::None => &[],
            Self::Terminal => &[BarButton::Scrollback, BarButton::Restart, BarButton::Stop],
            Self::Web { history: true } => &[
                BarButton::Back,
                BarButton::Forward,
                BarButton::Reload,
                BarButton::CopyUrl,
            ],
            Self::Web { history: false } => &[BarButton::Reload, BarButton::CopyUrl],
        }
    }

    /// Whether the bar floats over its window as a pill, leaving only a
    /// handle while hidden: any but a terminal's.
    pub fn floats(self) -> bool {
        self != Self::Terminal
    }

    /// How many of the buttons lead the bar, at its start; the rest are
    /// at its end.
    pub fn leading(self) -> usize {
        match self {
            Self::Web { history: true } => 3,
            Self::Web { history: false } => 1,
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
    /// it is hidden and still. A floating bar's pill (see `pane_bar_rect`).
    pub rect: Rect,
    /// Where a floating bar's grip goes, drawn on its own under the pill
    /// (see `grip`).
    pub grip: Rect,
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
    ///
    /// A floating bar's pill is drawn whole with its shadow round it,
    /// fading in as it drops from a little higher; nothing while its fade
    /// has it gone.
    pub fn placement(&self) -> (Point<i32, Logical>, Size<i32, Logical>, f32) {
        if self.buttons.floats() {
            if self.alpha <= 0.0 || self.rect.size.h <= 0 {
                return (self.rect.loc, Size::from((0, 0)), 0.0);
            }
            let drop = (PILL_DROP * self.slide).checked_div(BAR_SLIDE).unwrap_or(0);
            let loc = self.rect.loc - Point::from((SHADOW, SHADOW + drop));
            let size = self.rect.size + Size::from((2 * SHADOW, 2 * SHADOW));
            return (loc, size, self.alpha);
        }
        let h = (self.rect.size.h - self.slide).max(0);
        (self.rect.loc, (self.rect.size.w, h).into(), self.alpha)
    }

    /// Where a floating bar's grip is drawn, under its pill; none for a
    /// terminal's, whose grip is the bar itself slid away.
    pub fn grip(&self) -> Option<Rect> {
        (self.buttons.floats() && self.grip.size.w > 0 && self.grip.size.h > 0).then_some(self.grip)
    }

    /// Whether no more than its grip shows: it is drawn as the grip.
    pub fn is_grip(&self) -> bool {
        if self.buttons.floats() {
            return false;
        }
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
    /// A pill looks the same wherever it is on its way, so its slide is
    /// left out too.
    pub fn still(&self) -> Self {
        Self {
            alpha: 1.0,
            slide: if self.buttons.floats() { 0 } else { self.slide },
            ..self.clone()
        }
    }
}

/// The fill of a pill's address field: a shade off the pill's own.
fn field_fill(p: &Palette) -> Rgb {
    mix(p.elevated, p.element, 0.7)
}

/// The colour of a floating bar's handle: the accent while its pane is
/// `focused`, else a mark faint enough to rest but still 3:1 on its grip.
fn handle_color(focused: bool, p: &Palette) -> Rgb {
    if focused {
        p.accent
    } else {
        mix(p.muted, p.tab_bar, HANDLE_FADE)
    }
}

/// A web page's address split for its bar: the host, without the
/// scheme, and the rest from its first slash, none when that is a bare
/// "/".
fn split_address(url: &str) -> (&str, &str) {
    let bare = ["https://", "http://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
        .unwrap_or(url);
    match bare.find('/') {
        Some(i) if &bare[i..] == "/" => (&bare[..i], ""),
        Some(i) => bare.split_at(i),
        None => (bare, ""),
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
        if view.buttons.floats() {
            return self.pane_pill(view, scale, p);
        }
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

    /// A floating bar's pill (see `BarButtons::floats`): a round panel with
    /// its shadow; a web page's with its back, forward and reload, its
    /// address in a soft field, the host plain and the path muted and cut
    /// in the middle, then copy, zoom and close; an app's with its title,
    /// then zoom and close.
    fn pane_pill(&self, view: &PaneBarView, scale: f64, p: &Palette) -> MemoryRenderBuffer {
        let size = view.rect.size;
        let mut c = self.popup(size, size.h as f32 / 2.0, scale, p);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let bar = Rectangle::new((SHADOW, SHADOW).into(), size);
        let buttons = pane_bar_buttons(bar, view.buttons, view.zoom);
        if let Some(field) = pane_bar_field(bar, view.buttons, view.zoom) {
            let (x, y, w, h) = rect_px(field, scale);
            // Round like the pill round it.
            c.rounded(x, y, w, h, h / 2.0, field_fill(p));
            let pad = px(8) as f32;
            let band = band(field, scale);
            let (left, right) = (x + pad, x + w - pad);
            let (host, path) = split_address(&view.label);
            let host = self.cut_middle(host, DETAIL_FONT_SIZE, right - left, scale);
            let end = self.text(
                &mut c,
                &host,
                DETAIL_FONT_SIZE,
                left,
                right,
                band,
                scale,
                p.text,
            );
            if !path.is_empty() && end < right {
                let path = self.cut_middle(path, DETAIL_FONT_SIZE, right - end, scale);
                self.text(
                    &mut c,
                    &path,
                    DETAIL_FONT_SIZE,
                    end,
                    right,
                    band,
                    scale,
                    p.muted,
                );
            }
        } else if !matches!(view.buttons, BarButtons::Web { .. }) {
            let left = px(bar.loc.x + PILL_PAD + BAR_NOTE_GAP) as f32;
            let end_x = buttons
                .first()
                .map_or(bar.loc.x + bar.size.w - PILL_PAD, |(_, first)| first.loc.x);
            let right = px(end_x - BAR_NOTE_GAP / 2) as f32;
            let label = self.cut_middle(&view.label, DETAIL_FONT_SIZE, right - left, scale);
            let band = band(bar, scale);
            self.text(
                &mut c,
                &label,
                DETAIL_FONT_SIZE,
                left,
                right,
                band,
                scale,
                p.text,
            );
        }
        self.bar_buttons(&mut c, view, buttons, p.elevated, scale, p);
        c.into_buffer(false)
    }

    /// A floating bar's grip, `size` big: a bare band with a short handle
    /// in its middle, in the accent while its pane is `focused` (the line
    /// along the window's top says so too), else faint over a hairline.
    pub(super) fn pane_handle(
        &self,
        size: Size<i32, Logical>,
        focused: bool,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        c.fill(0, 0, c.w, c.h, p.tab_bar);
        if !focused {
            let line = px(1).max(1);
            c.fill(0, c.h - line, c.w, line, p.border_variant);
        }
        let w = HANDLE_W.min(size.w);
        let h = HANDLE_H.min(size.h);
        let at = Rectangle::new(((size.w - w) / 2, (size.h - h) / 2).into(), (w, h).into());
        let (x, y, w, h) = rect_px(at, scale);
        c.rounded(x, y, w, h, h / 2.0, handle_color(focused, p));
        c.into_buffer(false)
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
        let close = grip_close(grip);
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
                // Round in a pill, like the pill.
                let r = if view.buttons.floats() {
                    w.min(h) / 2.0
                } else {
                    4.0 * s
                };
                c.rounded(x, y, w, h, r, p.selected);
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
        BAR_PILL_HEIGHT, BAR_REVEAL, BAR_SLIDE, BarButton, BarButtons, BarHit, BarStatus,
        FOCUS_STRIP, PANE_GRIP, PILL_APP, PILL_MARGIN, PILL_PAD, PILL_TOP, PILL_WEB_MAX,
        PaneBarView, SHADOW, UNZOOM_BUTTON, UNZOOM_MARGIN, field_fill, focus_strip,
        focus_strip_rect, handle_color, pane_bar_buttons, pane_bar_field, pane_bar_hit,
        pane_bar_rect, pane_bar_rects, pane_bar_rects_shown, pane_grip_hit, pane_grip_rect,
        pointer_reveals, split_address, unzoom_button_rect,
    };
    use crate::chrome::{BAR_INSET, PANE_BAR_HEIGHT, Painter, Rect, Theme};
    use smithay::utils::{Point, Size};

    fn view(w: i32) -> PaneBarView {
        PaneBarView {
            rect: Rect::new((0, 0).into(), (w, PANE_BAR_HEIGHT).into()),
            grip: Rect::default(),
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
            pane_grip_hit(grip, end),
            Some(BarHit::Button(BarButton::Close))
        );
        assert_eq!(
            pane_grip_hit(grip, (300.0, 52.0).into()),
            Some(BarHit::Label)
        );
        assert_eq!(pane_grip_hit(grip, (300.0, 60.0).into()), None);
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
        const T: BarButtons = BarButtons::Terminal;
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let at = |x: f64, y: f64| (x, y).into();
        // Hidden: a band around the edge, inside the pane's width.
        assert!(pointer_reveals(pane, T, false, at(300.0, 50.0)));
        assert!(pointer_reveals(
            pane,
            T,
            false,
            at(300.0, 50.0 - BAR_REVEAL as f64)
        ));
        assert!(pointer_reveals(pane, T, false, at(300.0, 57.9)));
        assert!(!pointer_reveals(pane, T, false, at(300.0, 58.0)));
        assert!(!pointer_reveals(pane, T, false, at(300.0, 41.0)));
        assert!(!pointer_reveals(pane, T, false, at(199.0, 50.0)));
        assert!(!pointer_reveals(pane, T, false, at(800.0, 50.0)));
        // Shown: the whole bar keeps it, and the band above still counts.
        assert!(pointer_reveals(pane, T, true, at(300.0, 73.0)));
        assert!(pointer_reveals(pane, T, true, at(300.0, 45.0)));
        assert!(!pointer_reveals(pane, T, true, at(300.0, 74.0)));
        // A pane too short for a bar still has its band.
        let short = Rect::new((0, 100).into(), (300, 4).into());
        assert!(pointer_reveals(short, T, true, at(10.0, 105.0)));
        assert!(!pointer_reveals(short, T, true, at(10.0, 108.0)));
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

    const WEB: BarButtons = BarButtons::Web { history: true };

    #[test]
    fn a_web_bar_leads_with_its_steps_and_fills_the_rest_with_its_address() {
        let pane = Rect::new((200, 50).into(), (1000, 400).into());
        let pill = pane_bar_rect(pane, WEB);
        for zoom in [None, Some(false)] {
            let buttons = pane_bar_buttons(pill, WEB, zoom);
            let (_, reload) = buttons[2];
            assert_eq!(buttons[0].1.loc.x, pill.loc.x + PILL_PAD + BAR_INSET);
            assert_eq!(buttons[3].0, BarButton::CopyUrl);
            let field = pane_bar_field(pill, WEB, zoom).unwrap();
            assert!(field.loc.x >= reload.loc.x + reload.size.w);
            assert!(field.loc.x + field.size.w <= buttons[3].1.loc.x);
            assert!(field.loc.y > pill.loc.y);
            assert!(field.loc.y + field.size.h < pill.loc.y + pill.size.h);
            // The field is the label: a click there holds the pane.
            let mid = field.to_f64().loc + field.to_f64().size.downscale(2.0).to_point();
            assert_eq!(pane_bar_hit(pill, WEB, zoom, mid), Some(BarHit::Label));
        }
        // Copy sits just before the close button, which ends the pill.
        let buttons = pane_bar_buttons(pill, WEB, None);
        let order: Vec<_> = buttons[3..].iter().map(|(b, _)| *b).collect();
        assert_eq!(order, [BarButton::CopyUrl, BarButton::Close]);
        let (_, close) = buttons.last().unwrap();
        assert_eq!(
            close.loc.x + close.size.w,
            pill.loc.x + pill.size.w - PILL_PAD - BAR_INSET
        );
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        assert_eq!(pane_bar_field(bar, BarButtons::Terminal, None), None);
        let narrow = Rect::new((0, 0).into(), (120, PANE_BAR_HEIGHT).into());
        assert_eq!(pane_bar_field(narrow, WEB, Some(false)), None);
    }

    #[test]
    fn a_web_page_without_history_has_only_reload_before_its_address() {
        let kind = BarButtons::Web { history: false };
        assert_eq!(kind.buttons(), [BarButton::Reload, BarButton::CopyUrl]);
        assert_eq!(kind.leading(), 1);
        let pane = Rect::new((0, 0).into(), (1000, 400).into());
        let pill = pane_bar_rect(pane, kind);
        let buttons = pane_bar_buttons(pill, kind, None);
        let field = pane_bar_field(pill, kind, None).unwrap();
        assert_eq!(buttons[0].0, BarButton::Reload);
        assert!(field.loc.x >= buttons[0].1.loc.x + buttons[0].1.size.w);
    }

    #[test]
    fn a_floating_bar_hangs_as_a_pill_in_the_middle_of_its_top_edge() {
        let pane = Rect::new((200, 50).into(), (1000, 400).into());
        assert_eq!(
            pane_bar_rect(pane, BarButtons::Terminal),
            pane_bar_rects(pane).0
        );
        for kind in [WEB, BarButtons::None] {
            assert!(kind.floats());
            let pill = pane_bar_rect(pane, kind);
            assert_eq!(pill.loc.y, 50 + PILL_TOP);
            assert_eq!(pill.size.h, BAR_PILL_HEIGHT);
            // Centred, inside the pane.
            let (left, right) = (pill.loc.x - 200, 1200 - pill.loc.x - pill.size.w);
            assert!((left - right).abs() <= 1, "{left} {right}");
            assert!(left >= PILL_MARGIN);
            // Its buttons are square and inside it.
            for (_, r) in pane_bar_buttons(pill, kind, Some(false)) {
                assert_eq!(r.size.w, r.size.h);
                assert!(r.loc.y >= pill.loc.y && r.loc.y + r.size.h <= pill.loc.y + pill.size.h);
                assert!(r.loc.x > pill.loc.x && r.loc.x + r.size.w < pill.loc.x + pill.size.w);
            }
        }
        // A web page's grows with its pane, within bounds; an app's doesn't.
        let wide = Rect::new((0, 0).into(), (3000, 400).into());
        assert_eq!(pane_bar_rect(wide, WEB).size.w, PILL_WEB_MAX);
        assert_eq!(pane_bar_rect(wide, BarButtons::None).size.w, PILL_APP);
        let narrow = Rect::new((0, 0).into(), (300, 400).into());
        assert_eq!(pane_bar_rect(narrow, WEB).size.w, 300 - 2 * PILL_MARGIN);
        // The grip is the band a hidden bar leaves.
        assert_eq!(pane_grip_rect(pane), pane_bar_rects_shown(pane, false).0);
    }

    #[test]
    fn a_floating_bar_stays_out_over_its_pill_and_its_grip_only() {
        let pane = Rect::new((200, 50).into(), (1000, 400).into());
        let pill = pane_bar_rect(pane, WEB);
        let at = |x: i32, y: i32| (x as f64, y as f64).into();
        let mid = pill.loc.x + pill.size.w / 2;
        let bottom = pill.loc.y + pill.size.h;
        // Hidden, only the band along the edge brings it out.
        assert!(pointer_reveals(pane, WEB, false, at(mid, 52)));
        assert!(!pointer_reveals(pane, WEB, false, at(mid, bottom - 2)));
        // Out, the pill and the shadow round it keep it, and the band.
        assert!(pointer_reveals(pane, WEB, true, at(mid, bottom - 2)));
        assert!(pointer_reveals(
            pane,
            WEB,
            true,
            at(mid, bottom + PILL_MARGIN - 1)
        ));
        assert!(!pointer_reveals(
            pane,
            WEB,
            true,
            at(mid, bottom + PILL_MARGIN)
        ));
        assert!(pointer_reveals(pane, WEB, true, at(210, 52)));
        // Beside the pill, below the band, the page is the page's.
        assert!(!pointer_reveals(pane, WEB, true, at(210, bottom - 2)));
    }

    #[test]
    fn a_floating_bar_fades_its_pill_in_over_its_grip() {
        let pane = Rect::new((200, 50).into(), (1000, 400).into());
        let mut v = view(600);
        v.buttons = WEB;
        v.rect = pane_bar_rect(pane, WEB);
        v.grip = pane_grip_rect(pane);
        v.status = BarStatus::Idle;
        assert!(!v.is_grip());
        assert_eq!(v.grip(), Some(v.grip));
        assert_eq!(v.blinker(), None);
        // Out: drawn whole with its shadow round it.
        let (loc, size, alpha) = v.placement();
        assert_eq!(loc, v.rect.loc - Point::from((SHADOW, SHADOW)));
        assert_eq!(size, v.rect.size + Size::from((2 * SHADOW, 2 * SHADOW)));
        assert_eq!(alpha, 1.0);
        // On its way, a little higher and fainter; gone, not drawn.
        v.alpha = 0.5;
        v.slide = BAR_SLIDE / 2;
        let (half, _, alpha) = v.placement();
        assert!(half.y < loc.y && alpha == 0.5);
        v.alpha = 0.0;
        assert_eq!(v.placement().1.h, 0);
        // A terminal's has no grip of its own: it slides into it.
        assert_eq!(view(600).grip(), None);
        let painter = Painter::new();
        for theme in [Theme::Dark, Theme::Light] {
            for focused in [true, false] {
                painter.pane_handle(v.grip.size, focused, 2.0, theme.palette());
            }
            for kind in [WEB, BarButtons::Web { history: false }, BarButtons::None] {
                let mut v = v.clone();
                v.alpha = 1.0;
                v.buttons = kind;
                v.label = "http://localhost:3773/7a88cf7d-c216-4867/d0734377".into();
                painter.pane_bar(&v, 1.5, theme.palette());
            }
        }
    }

    #[test]
    fn a_pill_and_its_handle_read_in_both_themes() {
        use crate::chrome::sidebar::contrast;
        for theme in [Theme::Dark, Theme::Light] {
            let p = theme.palette();
            let field = field_fill(p);
            for (what, colour, floor) in [("host", p.text, 4.5), ("path", p.muted, 4.5)] {
                let ratio = contrast(colour, field);
                assert!(ratio >= floor, "{theme:?}: {what} {ratio:.2}:1");
            }
            for focused in [true, false] {
                let ratio = contrast(handle_color(focused, p), p.tab_bar);
                assert!(ratio >= 3.0, "{theme:?}: handle {focused} {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn an_address_splits_into_its_host_and_path() {
        assert_eq!(
            split_address("http://localhost:3773/a/b?c"),
            ("localhost:3773", "/a/b?c")
        );
        assert_eq!(split_address("https://example.com/"), ("example.com", ""));
        assert_eq!(split_address("https://example.com"), ("example.com", ""));
        assert_eq!(split_address("file:///tmp/x"), ("file:", "///tmp/x"));
    }

    #[test]
    fn pane_bar_buttons_do_not_overlap() {
        let bar = Rect::new((200, 50).into(), (600, PANE_BAR_HEIGHT).into());
        let kinds = [BarButtons::Terminal, WEB, BarButtons::None];
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
            let pad = if kind.floats() { PILL_PAD } else { 0 };
            assert_eq!(last.loc.x + last.size.w, 200 + 600 - pad - BAR_INSET);
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
        for kind in [BarButtons::Terminal, WEB] {
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
                                WEB,
                                BarButtons::None,
                                BarButtons::Web { history: false },
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
