//! A scrollbar as it stands, for the pointer to hold its thumb: where
//! its track and its thumb are, and where the list stands for any place
//! of the thumb along the track.

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    utils::{Logical, Point, Rectangle, Size},
};

use super::{Canvas, Painter, Palette, Rect, mix, physical};

/// How far beside a scrollbar's track the pointer still takes it, in
/// logical pixels: the track is only a few wide.
const REACH: i32 = 3;
/// How wide a scrollbar in the gutter right of a popup's rows is, and how
/// far right of them it stands.
const GUTTER_BAR: i32 = 4;
const GUTTER_GAP: i32 = 1;
/// The shortest a thumb gets.
const THUMB_MIN: f64 = 24.0;

/// Where a scrolled list of rows of any height stands, in logical
/// pixels: how far down it is scrolled and how high all its rows are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListBar {
    pub down: f64,
    pub total: f64,
}

/// The track of a scrollbar in the gutter right of rows ending at
/// `right`, down all of `list`.
pub(super) fn gutter_track(right: i32, list: Rect) -> Rect {
    Rectangle::new(
        (right + GUTTER_GAP, list.loc.y).into(),
        (GUTTER_BAR, list.size.h).into(),
    )
}

impl Painter {
    /// A scrollbar's thumb, `size` big.
    pub(super) fn bar_thumb(
        &self,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let mut c = Canvas::new(physical(size, scale));
        let (w, h) = (c.w as f32, c.h as f32);
        c.rounded(0.0, 0.0, w, h, w / 2.0, mix(p.elevated, p.muted, 0.45));
        c.into_buffer(false)
    }
}

/// The scrollbar of a list that does not all show, in output
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumb {
    pub track: Rect,
    /// The thumb's top, to the fraction of a pixel.
    pub top: f64,
    /// The thumb's height.
    pub h: f64,
    /// Where the list stands, in its own units, with the thumb at the
    /// track's end; with it at the track's start, it stands at 0.
    pub last: f64,
}

impl Thumb {
    /// The scrollbar in `track` of a list `view` high standing as `bar`;
    /// none while it all shows.
    pub fn of(track: Rect, view: f64, bar: ListBar) -> Option<Self> {
        let last = bar.total - view;
        let room = track.size.h as f64;
        if last < 1.0 || view <= 0.0 || room <= 0.0 {
            return None;
        }
        let h = (room * view / bar.total)
            .max(THUMB_MIN.min(room))
            .min(room)
            .round();
        let top = track.loc.y as f64 + (room - h) * bar.down.clamp(0.0, last) / last;
        Some(Self {
            track,
            top,
            h,
            last,
        })
    }

    /// Whether `pos` is on the track, or within `REACH` beside it.
    pub fn takes(&self, pos: Point<f64, Logical>) -> bool {
        let t = self.track;
        let x = (t.loc.x - REACH) as f64..(t.loc.x + t.size.w + REACH) as f64;
        let y = t.loc.y as f64..(t.loc.y + t.size.h) as f64;
        x.contains(&pos.x) && y.contains(&pos.y)
    }

    /// How far below the thumb's top `y` is, when it is on the thumb.
    pub fn on_thumb(&self, y: f64) -> Option<f64> {
        let by = y - self.top;
        (0.0..self.h).contains(&by).then_some(by)
    }

    /// Where the list stands with the thumb's top at `top`: the thumb
    /// goes no further than the track's ends.
    pub fn pos_at(&self, top: f64) -> f64 {
        let room = self.track.size.h as f64 - self.h;
        if room <= 0.0 {
            return 0.0;
        }
        ((top - self.track.loc.y as f64) / room).clamp(0.0, 1.0) * self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb() -> Thumb {
        Thumb {
            track: Rect::new((100, 50).into(), (6, 200).into()),
            top: 90.0,
            h: 40.0,
            last: 32.0,
        }
    }

    #[test]
    fn the_thumb_along_the_track_puts_the_list_as_far_along() {
        let t = thumb();
        assert_eq!(t.pos_at(50.0), 0.0);
        assert_eq!(t.pos_at(130.0), 16.0);
        assert_eq!(t.pos_at(210.0), 32.0);
        // No further than either end.
        assert_eq!(t.pos_at(0.0), 0.0);
        assert_eq!(t.pos_at(400.0), 32.0);
    }

    #[test]
    fn a_list_scrolled_by_pixels_puts_its_thumb_as_far_along() {
        let track = Rect::new((100, 50).into(), (4, 200).into());
        let at = |down| Thumb::of(track, 100.0, ListBar { down, total: 400.0 }).unwrap();
        // A quarter of the rows show: a quarter of the track.
        assert_eq!((at(0.0).top, at(0.0).h, at(0.0).last), (50.0, 50.0, 300.0));
        assert_eq!(at(150.0).top, 125.0);
        assert_eq!(at(300.0).top, 200.0);
        // Pulled past an end, it stays at that end.
        assert_eq!(at(-30.0).top, 50.0);
        assert_eq!(at(340.0).top, 200.0);
        // Dragged back, the list goes where the thumb is.
        assert_eq!(at(0.0).pos_at(125.0), 150.0);
        let fits = ListBar {
            down: 0.0,
            total: 100.0,
        };
        assert_eq!(Thumb::of(track, 100.0, fits), None);
    }

    #[test]
    fn the_track_takes_the_pointer_a_little_beside_it() {
        let t = thumb();
        assert!(t.takes((98.0, 60.0).into()));
        assert!(t.takes((108.5, 249.0).into()));
        assert!(!t.takes((96.5, 60.0).into()));
        assert!(!t.takes((103.0, 250.0).into()));
    }

    #[test]
    fn a_press_on_the_thumb_holds_it_where_it_is_pressed() {
        let t = thumb();
        assert_eq!(t.on_thumb(100.0), Some(10.0));
        assert_eq!(t.on_thumb(89.0), None);
        assert_eq!(t.on_thumb(130.0), None);
    }
}
