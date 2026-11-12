//! The bar over each pane: what it shows, its buttons and how it is drawn;
//! and the strip marking the focused pane, whatever it shows.
//!
//! Every pane has its bar, always: a flat band along its top, as wide as
//! the pane and `PANE_BAR_HEIGHT` high. The window starts under it (see
//! `pane_bar_rects`), so the bar covers nothing of it. A terminal's says
//! what runs, after a mark for how it is; a web page's has its address
//! between its steps; an app's has its title. Its buttons sit at its ends,
//! always in sight; the rest of it is its label, which holds the pane to
//! move it.
//!
//! An agent runs as a command, but it only blinks while it works: at
//! rest, its mark says where it stands, as its card's dot does.

use super::{
    BAR_BUTTON, BAR_BUTTON_GAP, BAR_INSET, BAR_NOTE_GAP, Canvas, DETAIL_FONT_SIZE, Dot,
    PANE_BAR_HEIGHT, Painter, Palette, ROW_HIGHLIGHT_RADIUS, Rect, Rgb, SHADOW, STATUS_GAP,
    STATUS_WIDTH, band, centred_dot, chevron_side, copy, cross, icon_sdf, mix, physical, rect_px,
    restart_arrow, send, status_dot, status_mark, stop, text_lines, zoom_corners,
};
use crate::herdr::AgentStatus;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The strip along the top edge of the focused pane's bar: this high.
pub const FOCUS_STRIP: i32 = 2;
/// How opaque the unzoom button is while the pointer is away from it: the
/// window under it shows through a little. Under the pointer it fades to
/// whole.
pub(super) const UNZOOM_REST: f32 = 0.7;
/// How far a bar's buttons keep from its top and bottom, at least: they
/// are smaller than `BAR_BUTTON` in a bar too low for that.
const BUTTON_INSET_Y: i32 = 2;
/// How wide a web page's address field gets at most: in a wider bar it
/// sits in the middle of the room between the buttons.
const FIELD_MAX: i32 = 760;

/// A pane's place split in two: its bar on top, `PANE_BAR_HEIGHT` high,
/// and its window below it, at least a pixel high. Everything that places
/// a pane's bar or its window goes through this.
pub fn pane_bar_rects(pane: Rect) -> (Rect, Rect) {
    let h = PANE_BAR_HEIGHT.min(pane.size.h - 1).max(0);
    let bar = Rectangle::new(pane.loc, (pane.size.w, h).into());
    let client = Rectangle::new(
        (pane.loc.x, pane.loc.y + h).into(),
        (pane.size.w, (pane.size.h - h).max(1)).into(),
    );
    (bar, client)
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
/// They sit inside `bar`, square, `BAR_INSET` in from its ends.
pub fn pane_bar_buttons(bar: Rect, kind: BarButtons, zoom: Option<bool>) -> Vec<(BarButton, Rect)> {
    let size = BAR_BUTTON.min(bar.size.h - 2 * BUTTON_INSET_Y).max(0);
    let inset_y = ((bar.size.h - size) / 2).max(0);
    let bar: Rect = Rectangle::new(
        (bar.loc.x, bar.loc.y + inset_y).into(),
        (bar.size.w, bar.size.h - 2 * inset_y).into(),
    );
    let (leading, trailing) = kind.buttons().split_at(kind.leading());
    let trailing: Vec<_> = trailing
        .iter()
        .copied()
        .chain(zoom_button(zoom))
        .chain([BarButton::Close])
        .collect();
    let step = size + BAR_BUTTON_GAP;
    let at = |x: i32| Rectangle::new((x, bar.loc.y).into(), (size, bar.size.h).into());
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
/// `kind`'s buttons and the zoom button if `zoom`: the room between the
/// buttons at its start and those at its end, in the middle of it once
/// that is wider than `FIELD_MAX`. None for a bar that shows no address,
/// or has no room for one.
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
    // The field is a little lower than the buttons either side of it.
    let inset = buttons
        .first()
        .map_or(BAR_FIELD_INSET, |(_, r)| r.loc.y - bar.loc.y + 1);
    let h = bar.size.h - 2 * inset;
    let w = (right - left).min(FIELD_MAX);
    let left = left + (right - left - w) / 2;
    (w >= BAR_BUTTON && h > 0).then(|| Rectangle::new((left, bar.loc.y + inset).into(), (w, h).into()))
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
    /// Tell the agent waiting in this terminal to read its stage's prompt.
    SendPrompt,
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
    /// A terminal whose stage agent waits for its prompt: that to send too.
    Waiting,
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
            Self::Waiting => &[
                BarButton::SendPrompt,
                BarButton::Scrollback,
                BarButton::Restart,
                BarButton::Stop,
            ],
            Self::Web { history: true } => &[
                BarButton::Back,
                BarButton::Forward,
                BarButton::Reload,
                BarButton::CopyUrl,
            ],
            Self::Web { history: false } => &[BarButton::Reload, BarButton::CopyUrl],
        }
    }

    /// How many of the buttons lead the bar, at its start; the rest are
    /// at its end.
    pub fn leading(self) -> usize {
        match self {
            Self::Web { history: true } => 3,
            Self::Web { history: false } => 1,
            Self::None | Self::Terminal | Self::Waiting => 0,
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
    /// What runs is an agent, and this is where it stands, other than at
    /// work (that is `Running`): waiting for a prompt, say.
    Agent(AgentStatus),
    /// The command ran and is done: it can run again.
    Exited,
    /// Being stopped or restarted.
    Working,
}

impl BarStatus {
    /// Whether something runs in the pane, or is still being stopped.
    pub fn runs(self) -> bool {
        matches!(self, Self::Running | Self::Agent(_) | Self::Working)
    }
}

/// The bar over a pane: what runs in it or what it shows, and its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneBarView {
    /// Where it goes on the output: along its pane's top (see
    /// `pane_bar_rects`).
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
    /// The pane has keyboard focus: its label is plain, else muted.
    pub focused: bool,
    /// The button under the pointer.
    pub hover: Option<BarButton>,
}

impl PaneBarView {
    /// Whether it starts with a mark saying how its command is: a
    /// terminal's, once something has run in it.
    fn has_mark(&self) -> bool {
        matches!(self.buttons, BarButtons::Terminal | BarButtons::Waiting)
            && self.status != BarStatus::Idle
    }

    /// Where its mark goes, in the bar at `bar`.
    fn mark_in(bar: Rect) -> Rect {
        Rectangle::new(
            (bar.loc.x + BAR_INSET, bar.loc.y).into(),
            (STATUS_WIDTH, bar.size.h).into(),
        )
    }

    /// Where its blinking dot goes on the output, while its command runs:
    /// on its mark.
    pub fn blinker(&self) -> Option<Rect> {
        (self.status == BarStatus::Running && self.has_mark() && self.rect.size.h > 0)
            .then(|| centred_dot(Self::mark_in(self.rect)))
    }
}

/// The fill of a bar's address field: a shade off the popups' own.
fn field_fill(p: &Palette) -> Rgb {
    mix(p.elevated, p.element, 0.7)
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

    /// A pane's bar: a flat band with a hairline under it, its label plain
    /// while its pane is focused, else muted. A web page's has its back,
    /// forward and reload, its address in a soft field, the host first
    /// and the path muted and cut in the middle, then copy, zoom and close;
    /// a terminal's a mark saying how its command is (a running one's is a
    /// blinking dot, composed over it: see `blinker`), the command cut in
    /// the middle and a note after it, then its buttons; an app's its
    /// title, then zoom and close.
    pub(super) fn pane_bar(
        &self,
        view: &PaneBarView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let size = view.rect.size;
        let mut c = Canvas::new(physical(size, scale));
        let px = |v: i32| (v as f64 * scale).round() as i32;
        c.fill(0, 0, c.w, c.h, p.tab_bar);
        let line = px(1).max(1);
        c.fill(0, c.h - line, c.w, line, p.border_variant);
        let bar = Rectangle::from_size(size);
        let ink = if view.focused { p.text } else { p.muted };
        let buttons = pane_bar_buttons(bar, view.buttons, view.zoom);
        if let Some(field) = pane_bar_field(bar, view.buttons, view.zoom) {
            let (x, y, w, h) = rect_px(field, scale);
            c.rounded(x, y, w, h, ROW_HIGHLIGHT_RADIUS * scale as f32, field_fill(p));
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
                ink,
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
            let mark = PaneBarView::mark_in(bar);
            // The marks the sidebar's agent chips use: a ring for done, an
            // open ring in the accent for busy.
            let s = scale as f32;
            match view.status {
                _ if !view.has_mark() => {}
                BarStatus::Idle | BarStatus::Running => {}
                BarStatus::Agent(status) => {
                    let dot = Dot::of_status(status);
                    status_dot(&mut c, centred_dot(mark), dot, scale, p);
                }
                BarStatus::Exited => icon_sdf(&mut c, mark, scale, p.placeholder, |x, y| {
                    status_mark(AgentStatus::Idle, x, y, s)
                }),
                BarStatus::Working => icon_sdf(&mut c, mark, scale, p.accent, |x, y| {
                    status_mark(AgentStatus::Working, x, y, s)
                }),
            }
            let left = if view.has_mark() {
                px(mark.loc.x + STATUS_WIDTH + STATUS_GAP)
            } else {
                px(bar.loc.x + BAR_NOTE_GAP)
            } as f32;
            let end_x = buttons
                .first()
                .map_or(bar.loc.x + bar.size.w - BAR_INSET, |(_, first)| first.loc.x);
            let right = px(end_x - BAR_NOTE_GAP / 2) as f32;
            // The note keeps its room; the label is cut short before it.
            let note_w = view.note.map_or(0.0, |note| {
                self.text_width(note, DETAIL_FONT_SIZE, scale) + px(BAR_NOTE_GAP) as f32
            });
            let label_right = (right - note_w).max(left);
            let label = self.cut_middle(&view.label, DETAIL_FONT_SIZE, label_right - left, scale);
            let band = band(bar, scale);
            let end = self.text(
                &mut c,
                &label,
                DETAIL_FONT_SIZE,
                left,
                label_right,
                band,
                scale,
                ink,
            );
            if let Some(note) = view.note {
                let color = if view.status == BarStatus::Working {
                    p.accent
                } else {
                    p.muted
                };
                self.text(
                    &mut c,
                    note,
                    DETAIL_FONT_SIZE,
                    end + px(BAR_NOTE_GAP) as f32,
                    right,
                    band,
                    scale,
                    color,
                );
            }
        }
        self.bar_buttons(&mut c, view, buttons, p.tab_bar, scale, p);
        c.into_buffer(true)
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
        for (button, rect) in buttons {
            let enabled = match button {
                BarButton::Restart => view.can_restart,
                BarButton::Stop => view.can_stop,
                BarButton::Back => view.can_back,
                BarButton::Forward => view.can_forward,
                BarButton::SendPrompt
                | BarButton::Scrollback
                | BarButton::Reload
                | BarButton::CopyUrl
                | BarButton::Zoom
                | BarButton::Unzoom
                | BarButton::Close => true,
            };
            let hovered = enabled && view.hover == Some(button);
            if hovered {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, ROW_HIGHLIGHT_RADIUS * s, p.selected);
            }
            let color = if !enabled {
                mix(p.placeholder, background, 0.5)
            } else if hovered {
                p.text
            } else if button == BarButton::SendPrompt {
                // What the agent waits for stands out.
                p.accent
            } else {
                p.muted
            };
            match button {
                BarButton::SendPrompt => icon_sdf(c, rect, scale, color, |x, y| send(x, y, s)),
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
        BarButton, BarButtons, BarHit, BarStatus, FIELD_MAX, FOCUS_STRIP, PaneBarView,
        UNZOOM_BUTTON, UNZOOM_MARGIN, field_fill, focus_strip, focus_strip_rect, pane_bar_buttons,
        pane_bar_field, pane_bar_hit, pane_bar_rects, split_address, unzoom_button_rect,
    };
    use crate::chrome::{BAR_BUTTON, BAR_INSET, PANE_BAR_HEIGHT, Painter, Rect, Theme};
    use crate::herdr::AgentStatus;

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
            hover: None,
        }
    }

    /// The pixels `painter` draws for `view`.
    fn drawn(painter: &Painter, view: &PaneBarView, theme: Theme) -> Vec<u8> {
        let mut buffer = painter.pane_bar(view, 1.0, theme.palette());
        let mut bytes = Vec::new();
        let _ = buffer.render().draw(|mem| {
            bytes.extend_from_slice(mem);
            Ok::<_, ()>(Vec::new())
        });
        bytes
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
    fn a_bar_takes_the_top_of_its_pane_and_the_window_the_rest() {
        let pane = Rect::new((200, 50).into(), (600, 400).into());
        let (bar, client) = pane_bar_rects(pane);
        // As wide as the pane, and the window right under it: no overlap.
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
        assert!(bar.intersection(client).is_none_or(|both| both.is_empty()));
        // A pane too small for a bar keeps its window.
        let tiny = Rect::new((0, 0).into(), (300, 1).into());
        assert_eq!(pane_bar_rects(tiny).1, tiny);
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
        let pane = Rect::new((200, 50).into(), (700, 400).into());
        let bar = pane_bar_rects(pane).0;
        for zoom in [None, Some(false)] {
            let buttons = pane_bar_buttons(bar, WEB, zoom);
            let (_, reload) = buttons[2];
            assert_eq!(buttons[0].1.loc.x, bar.loc.x + BAR_INSET);
            assert_eq!(buttons[3].0, BarButton::CopyUrl);
            let field = pane_bar_field(bar, WEB, zoom).unwrap();
            assert!(field.loc.x >= reload.loc.x + reload.size.w);
            assert!(field.loc.x + field.size.w <= buttons[3].1.loc.x);
            assert!(field.loc.y > bar.loc.y);
            assert!(field.loc.y + field.size.h < bar.loc.y + bar.size.h);
            // The field is the label: a click there holds the pane.
            let mid = field.to_f64().loc + field.to_f64().size.downscale(2.0).to_point();
            assert_eq!(pane_bar_hit(bar, WEB, zoom, mid), Some(BarHit::Label));
        }
        // Copy sits just before the close button, which ends the bar.
        let buttons = pane_bar_buttons(bar, WEB, None);
        let order: Vec<_> = buttons[3..].iter().map(|(b, _)| *b).collect();
        assert_eq!(order, [BarButton::CopyUrl, BarButton::Close]);
        let (_, close) = buttons.last().unwrap();
        assert_eq!(
            close.loc.x + close.size.w,
            bar.loc.x + bar.size.w - BAR_INSET
        );
        assert_eq!(pane_bar_field(bar, BarButtons::Terminal, None), None);
        let narrow = Rect::new((0, 0).into(), (120, PANE_BAR_HEIGHT).into());
        assert_eq!(pane_bar_field(narrow, WEB, Some(false)), None);
    }

    #[test]
    fn a_wide_web_bar_keeps_its_address_in_the_middle() {
        let bar = Rect::new((0, 0).into(), (3000, PANE_BAR_HEIGHT).into());
        let buttons = pane_bar_buttons(bar, WEB, None);
        let field = pane_bar_field(bar, WEB, None).unwrap();
        assert_eq!(field.size.w, FIELD_MAX);
        let before = field.loc.x - (buttons[2].1.loc.x + buttons[2].1.size.w);
        let after = buttons[3].1.loc.x - (field.loc.x + field.size.w);
        assert!((before - after).abs() <= 1, "{before} {after}");
    }

    #[test]
    fn a_web_page_without_history_has_only_reload_before_its_address() {
        let kind = BarButtons::Web { history: false };
        assert_eq!(kind.buttons(), [BarButton::Reload, BarButton::CopyUrl]);
        assert_eq!(kind.leading(), 1);
        let pane = Rect::new((0, 0).into(), (1000, 400).into());
        let bar = pane_bar_rects(pane).0;
        let buttons = pane_bar_buttons(bar, kind, None);
        let field = pane_bar_field(bar, kind, None).unwrap();
        assert_eq!(buttons[0].0, BarButton::Reload);
        assert!(field.loc.x >= buttons[0].1.loc.x + buttons[0].1.size.w);
    }

    #[test]
    fn a_bar_reads_in_both_themes() {
        use crate::chrome::sidebar::contrast;
        for theme in [Theme::Dark, Theme::Light] {
            let p = theme.palette();
            let field = field_fill(p);
            for (what, colour, floor) in [
                ("host", p.text, 4.5),
                ("unfocused host", p.muted, 4.5),
                ("path", p.muted, 4.5),
            ] {
                let ratio = contrast(colour, field);
                assert!(ratio >= floor, "{theme:?}: {what} {ratio:.2}:1");
            }
            // On the bar itself: its label, its note, its marks, its buttons.
            for (what, colour, floor) in [
                ("label", p.text, 4.5),
                ("unfocused label", p.muted, 4.5),
                ("note", p.muted, 4.5),
                ("working note", p.accent, 4.5),
                ("exited mark", p.placeholder, 3.0),
                ("button", p.muted, 3.0),
                ("waiting arrow", p.accent, 3.0),
            ] {
                let ratio = contrast(colour, p.tab_bar);
                assert!(ratio >= floor, "{theme:?}: {what} {ratio:.2}:1");
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
            assert_eq!(last.loc.x + last.size.w, 200 + 600 - BAR_INSET);
            for pair in buttons.windows(2) {
                assert!(pair[0].1.loc.x + pair[0].1.size.w <= pair[1].1.loc.x);
            }
            // Square, and inside the bar.
            let top = 50 + (PANE_BAR_HEIGHT - BAR_BUTTON) / 2;
            assert!(buttons.iter().all(|(_, r)| r.loc.y == top
                && r.size.h == BAR_BUTTON
                && r.size.w == BAR_BUTTON
                && r.loc.x >= bar.loc.x));
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
        // A waiting agent's bar leads what stays at its end with the arrow.
        assert_eq!(order(BarButtons::Waiting)[0], BarButton::SendPrompt,);
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
        assert_eq!(pane_bar_hit(bar, kind, None, (210.0, 90.0).into()), None);
        assert_eq!(pane_bar_hit(bar, kind, None, (100.0, 60.0).into()), None);
        // A bar with no height has nothing to hit.
        let flat = Rect::new((200, 50).into(), (600, 0).into());
        assert_eq!(pane_bar_hit(flat, kind, None, (210.0, 50.0).into()), None);
    }

    #[test]
    fn a_running_bar_blinks_its_dot_where_its_mark_is() {
        let mut v = view(600);
        v.rect.loc = (200, 50).into();
        v.status = BarStatus::Running;
        let dot = v.blinker().expect("a running bar blinks");
        assert_eq!(dot.size, (crate::chrome::DOT, crate::chrome::DOT).into());
        let mark_end = 200 + BAR_INSET + super::STATUS_WIDTH;
        assert!(dot.loc.x >= 200 + BAR_INSET && dot.loc.x + dot.size.w <= mark_end);
        let mid = dot.loc.y + dot.size.h / 2;
        assert!((mid - (50 + PANE_BAR_HEIGHT / 2)).abs() <= 1);
        v.status = BarStatus::Exited;
        assert_eq!(v.blinker(), None);
        // An agent blinks only at work, which is `Running`.
        for status in [
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            v.status = BarStatus::Agent(status);
            assert_eq!(v.blinker(), None);
        }
        // Only a terminal's runs a command.
        v.status = BarStatus::Running;
        v.buttons = BarButtons::None;
        assert_eq!(v.blinker(), None);
        // A bar with no height has no dot.
        v.buttons = BarButtons::Terminal;
        v.rect.size.h = 0;
        assert_eq!(v.blinker(), None);
    }

    #[test]
    fn an_agent_at_rest_paints_its_dot_where_the_blink_goes() {
        let painter = Painter::new();
        let drawn = |status| {
            let view = PaneBarView {
                status,
                ..view(600)
            };
            drawn(&painter, &view, Theme::Dark)
        };
        // A blink is composed over the bar, so a running bar's mark is empty.
        let idle = drawn(BarStatus::Agent(AgentStatus::Idle));
        assert_ne!(idle, drawn(BarStatus::Running));
        assert_ne!(idle, drawn(BarStatus::Agent(AgentStatus::Blocked)));
    }

    #[test]
    fn a_bar_says_whether_its_pane_is_focused_and_shows_its_buttons_either_way() {
        let painter = Painter::new();
        for theme in [Theme::Dark, Theme::Light] {
            let focused = view(600);
            let unfocused = PaneBarView {
                focused: false,
                ..view(600)
            };
            let (a, b) = (
                drawn(&painter, &focused, theme),
                drawn(&painter, &unfocused, theme),
            );
            assert_ne!(a, b);
            // The buttons are drawn the same, pointer or no pointer near:
            // the last columns of the bar, where they sit, don't change.
            let w = 600 * 4;
            let from = (600 - 120) * 4;
            let ends = |bytes: &[u8]| -> Vec<u8> {
                bytes.chunks(w).flat_map(|row| row[from..].to_vec()).collect()
            };
            assert_eq!(ends(&a), ends(&b));
            let empty = PaneBarView {
                label: String::new(),
                buttons: BarButtons::None,
                status: BarStatus::Idle,
                ..view(600)
            };
            let blank = PaneBarView {
                zoom: Some(false),
                ..empty.clone()
            };
            assert_ne!(ends(&drawn(&painter, &empty, theme)), ends(&drawn(&painter, &blank, theme)));
        }
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
                            hover: Some(BarButton::Restart),
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
