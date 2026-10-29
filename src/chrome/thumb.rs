//! A scrollbar as it stands, for the pointer to hold its thumb: where
//! its track and its thumb are, and where the list stands for any place
//! of the thumb along the track.

use smithay::utils::{Logical, Point};

use super::Rect;

/// How far beside a scrollbar's track the pointer still takes it, in
/// logical pixels: the track is only a few wide.
const REACH: i32 = 3;

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
