//! The screenshot viewer: one of a Manual test criterion's screenshots over
//! the whole output, as big as fits, with what it is of and how many there
//! are over it (see `crate::shot_viewer`).

use std::path::PathBuf;

use super::{
    ButtonStyle, FONT_SIZE, POPUP_RADIUS, Painter, Palette, QUIT_BUTTON, QUIT_BUTTON_GAP,
    QUIT_PADDING, Rect, SHADOW, band, button_width, phys, rect_px,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The room left around the viewer on the output.
const MARGIN: i32 = 24;

/// What the viewer shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotViewerView {
    pub path: PathBuf,
    /// Its width and height in pixels; `None` when its file can't be read.
    pub size: Option<(u32, u32)>,
    /// What it is a screenshot of: the criterion's id and title.
    pub title: String,
    /// Which of how many of the criterion's screenshots it is, from 0.
    pub at: usize,
    pub of: usize,
    pub hover: Option<ShotViewerHit>,
}

/// A button of the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotViewerHit {
    Prev,
    Next,
    Delete,
    Close,
}

impl ShotViewerHit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prev => "Previous",
            Self::Next => "Next",
            Self::Delete => "Delete",
            Self::Close => "Close",
        }
    }
}

/// Where the viewer's parts go on an output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotViewerLayout {
    pub panel: Rect,
    /// The line saying what it is, left of the buttons.
    pub title: Rect,
    /// The screenshot, as big as fits under the line, its shape kept; no
    /// bigger than its pixels.
    pub image: Rect,
    pub buttons: Vec<(ShotViewerHit, Rect)>,
}

/// Where `view`'s parts go on an output of `size`: the panel over all of
/// it but a margin, its buttons at the top right, Previous and Next only
/// with more than one screenshot.
pub fn shot_viewer_layout(view: &ShotViewerView, size: Size<i32, Logical>) -> ShotViewerLayout {
    let panel = Rectangle::new(
        (MARGIN.min(size.w / 8), MARGIN.min(size.h / 8)).into(),
        (
            (size.w - 2 * MARGIN.min(size.w / 8)).max(1),
            (size.h - 2 * MARGIN.min(size.h / 8)).max(1),
        )
            .into(),
    );
    let (x, y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let right = panel.loc.x + panel.size.w - QUIT_PADDING;
    let hits: &[ShotViewerHit] = match view.of > 1 {
        true => &[
            ShotViewerHit::Prev,
            ShotViewerHit::Next,
            ShotViewerHit::Delete,
            ShotViewerHit::Close,
        ],
        false => &[ShotViewerHit::Delete, ShotViewerHit::Close],
    };
    let mut end = right;
    let mut buttons: Vec<(ShotViewerHit, Rect)> = (hits.iter().rev())
        .map(|hit| {
            let w = button_width(hit.label());
            end -= w;
            let rect = Rectangle::new((end, y).into(), (w, QUIT_BUTTON).into());
            end -= QUIT_BUTTON_GAP;
            (*hit, rect)
        })
        .collect();
    buttons.reverse();
    let title = Rectangle::new((x, y).into(), ((end - x).max(1), QUIT_BUTTON).into());
    let top = y + QUIT_BUTTON + QUIT_PADDING;
    let room = Rectangle::new(
        (x, top).into(),
        (
            (right - x).max(1),
            (panel.loc.y + panel.size.h - QUIT_PADDING - top).max(1),
        )
            .into(),
    );
    ShotViewerLayout {
        panel,
        title,
        image: fit(view.size, room),
        buttons,
    }
}

/// A picture of `size` pixels as big as fits in `room`, centred, its shape
/// kept and no bigger than it is; one of no size, all of `room`.
fn fit(size: Option<(u32, u32)>, room: Rect) -> Rect {
    let Some((w, h)) = size.filter(|(w, h)| *w > 0 && *h > 0) else {
        return room;
    };
    let (w, h) = (w as f64, h as f64);
    let k = (room.size.w as f64 / w)
        .min(room.size.h as f64 / h)
        .min(1.0);
    let (fw, fh) = (
        ((w * k).round() as i32).max(1),
        ((h * k).round() as i32).max(1),
    );
    Rectangle::new(
        (
            room.loc.x + (room.size.w - fw) / 2,
            room.loc.y + (room.size.h - fh) / 2,
        )
            .into(),
        (fw, fh).into(),
    )
}

/// The viewer's button at `pos`, if one is.
pub fn shot_viewer_hit(
    layout: &ShotViewerLayout,
    pos: Point<f64, Logical>,
) -> Option<ShotViewerHit> {
    (layout.buttons.iter())
        .find(|(_, r)| r.to_f64().contains(pos))
        .map(|(hit, _)| *hit)
}

impl Painter {
    /// The screenshot viewer for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow.
    pub(super) fn shot_viewer(
        &self,
        view: &ShotViewerView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = shot_viewer_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let title = shift(layout.title);
        let place = format!("{} of {}", view.at + 1, view.of);
        let place_w = (self.text_width(&place, FONT_SIZE, scale) / scale as f32).ceil() as i32;
        // The title gets up to half the line, the place and keys the rest.
        let title_w =
            (self.text_width(&view.title, FONT_SIZE, scale) / scale as f32).ceil() as i32 + 1;
        let title_end = title.loc.x + title_w.min(title.size.w / 2);
        let (x, _) = phys(title, scale);
        let end = (title_end as f64 * scale).round() as f32;
        self.text(
            &mut c,
            &view.title,
            FONT_SIZE,
            x,
            end,
            band(title, scale),
            scale,
            p.text,
        );
        let place_rect = Rectangle::new(
            (title_end + QUIT_BUTTON_GAP, title.loc.y).into(),
            (place_w + 1, title.size.h).into(),
        );
        let (px, pend) = phys(place_rect, scale);
        self.text(
            &mut c,
            &place,
            FONT_SIZE,
            px,
            pend,
            band(place_rect, scale),
            scale,
            p.muted,
        );
        // Its keys after that, as far as they fit before its buttons.
        let hints: &[(&str, &str)] = match view.of > 1 {
            true => &[
                ("Left", "previous"),
                ("Right", "next"),
                ("Del", "delete"),
                ("Esc", "close"),
            ],
            false => &[("Del", "delete"), ("Esc", "close")],
        };
        let keys_x = place_rect.loc.x + place_rect.size.w + 2 * QUIT_BUTTON_GAP;
        let row = Rectangle::new((keys_x, title.loc.y).into(), (0, title.size.h).into());
        let first =
            (layout.buttons.first()).map_or(title.loc.x + title.size.w, |(_, r)| shift(*r).loc.x);
        self.dialog_keys(&mut c, hints, None, row, first - QUIT_BUTTON_GAP, scale, p);
        for (hit, rect) in &layout.buttons {
            let style = match hit {
                ShotViewerHit::Close => ButtonStyle::Primary,
                _ => ButtonStyle::Plain,
            };
            let lit = (view.hover == Some(*hit), false);
            self.button(&mut c, hit.label(), shift(*rect), lit, style, scale, p);
        }
        let image = shift(layout.image);
        let (ix, iy, iw, ih) = rect_px(image, scale);
        let picture = (view.size.is_some())
            .then(|| crate::shot_images::picture(&view.path, iw.max(ih).round() as usize))
            .flatten();
        match picture {
            Some(picture) => {
                let side = image.size.w.max(image.size.h);
                c.picture_sized(&picture, image, scale, p.text, side);
                let b = (scale as f32).round().max(1.0) as i32;
                c.outline(ix as i32, iy as i32, iw as i32, ih as i32, b, p.border);
            }
            None => {
                let text = "This screenshot's file is gone";
                let w = (self.text_width(text, FONT_SIZE, scale) / scale as f32).ceil() as i32 + 1;
                let line = Rectangle::new(
                    (
                        image.loc.x + (image.size.w - w) / 2,
                        image.loc.y + image.size.h / 2 - QUIT_BUTTON / 2,
                    )
                        .into(),
                    (w, QUIT_BUTTON).into(),
                );
                let (lx, lend) = phys(line, scale);
                self.text(
                    &mut c,
                    text,
                    FONT_SIZE,
                    lx,
                    lend,
                    band(line, scale),
                    scale,
                    p.muted,
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

    fn view(size: Option<(u32, u32)>, of: usize) -> ShotViewerView {
        ShotViewerView {
            path: "/nonexistent/AC-1-1.png".into(),
            size,
            title: "AC-1 · Export holds both invoices".into(),
            at: 0,
            of,
            hover: None,
        }
    }

    #[test]
    fn the_screenshot_fills_what_it_can_its_shape_kept_and_no_bigger_than_it_is() {
        let output: Size<i32, Logical> = (1920, 1080).into();
        let big = shot_viewer_layout(&view(Some((3840, 2160)), 2), output);
        assert!(inside(big.panel, big.image) && inside(big.panel, big.title));
        assert!(big.image.loc.y >= big.title.loc.y + big.title.size.h);
        let (w, h) = (big.image.size.w as f64, big.image.size.h as f64);
        assert!((w / h - 16.0 / 9.0).abs() < 0.01);
        let small = shot_viewer_layout(&view(Some((400, 300)), 1), output);
        assert_eq!((small.image.size.w, small.image.size.h), (400, 300));
        // Previous and Next only with more than one, all right of the line.
        let hits = |l: &ShotViewerLayout| l.buttons.iter().map(|(h, _)| *h).collect::<Vec<_>>();
        assert_eq!(hits(&small), [ShotViewerHit::Delete, ShotViewerHit::Close]);
        assert_eq!(
            hits(&big),
            [
                ShotViewerHit::Prev,
                ShotViewerHit::Next,
                ShotViewerHit::Delete,
                ShotViewerHit::Close
            ]
        );
        for (hit, rect) in &big.buttons {
            assert!(rect.loc.x >= big.title.loc.x + big.title.size.w);
            let middle = (rect.loc.x as f64 + 2.0, rect.loc.y as f64 + 2.0).into();
            assert_eq!(shot_viewer_hit(&big, middle), Some(*hit));
        }
        assert_eq!(shot_viewer_hit(&big, (5.0, 5.0).into()), None);
    }
}
