//! A list scrolled by the wheel or a touchpad: whole rows move the list,
//! and the part of a row left over shows as the list moved up by that part
//! of its first row, so a touchpad scrolls it smoothly.

use std::cell::Cell;

/// The wheel's turn not yet a whole row, and the first row shown then.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wheel {
    /// In rows, from 0 up to (not including) 1. Dropped when the list is
    /// seen moved (see `part`), so it can't come back with the list.
    part: Cell<f64>,
    /// The first row shown when `part` was left.
    at: usize,
}

impl Wheel {
    /// The wheel turned `rows` rows (down positive) with `first` the first
    /// row shown: the whole rows to scroll the list by. Settle with
    /// `settle` once the list scrolled.
    pub fn turn(&mut self, rows: f64, first: usize) -> i64 {
        let part = self.part(first) + rows;
        let whole = part.floor();
        self.part.set(part - whole);
        self.at = first;
        whole as i64
    }

    /// The list scrolled to `first` after `turn` asked for `wanted`, and
    /// can go down to `last` (the first row with the end of the list
    /// shown): the part of a row left is kept only where the scroll went
    /// as asked and there is a row under the list to show it with.
    pub fn settle(&mut self, wanted: i64, first: usize, last: usize) {
        if wanted != first as i64 || first >= last {
            self.part.set(0.0);
        }
        self.at = first;
    }

    /// The part of row `first`, the first row shown, scrolled past: none
    /// once the list moved otherwise (by keys, say), and none from then on
    /// even if it moves back. Ask for it with every view of the list.
    pub fn part(&self, first: usize) -> f64 {
        if self.at != first {
            self.part.set(0.0);
        }
        self.part.get()
    }
}

/// How far up a list scrolled `part` of its first row, `height` logical
/// pixels high, is moved: less than the whole row.
pub fn shift(part: f64, height: i32) -> i32 {
    ((part * height as f64).round() as i32).clamp(0, (height - 1).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scroll a list of `len` rows, `page` shown, from `first` by `rows`.
    fn scroll(wheel: &mut Wheel, first: &mut usize, rows: f64, len: usize, page: usize) {
        let last = len.saturating_sub(page);
        let whole = wheel.turn(rows, *first);
        let wanted = *first as i64 + whole;
        *first = wanted.clamp(0, last as i64) as usize;
        wheel.settle(wanted, *first, last);
    }

    #[test]
    fn parts_of_a_row_add_up_and_show() {
        let (mut wheel, mut first) = (Wheel::default(), 0);
        scroll(&mut wheel, &mut first, 0.25, 10, 4);
        assert_eq!((first, wheel.part(first)), (0, 0.25));
        scroll(&mut wheel, &mut first, 1.0, 10, 4);
        assert_eq!((first, wheel.part(first)), (1, 0.25));
        // Up past the row's top shows the row above, partly.
        scroll(&mut wheel, &mut first, -0.5, 10, 4);
        assert_eq!((first, wheel.part(first)), (0, 0.75));
    }

    #[test]
    fn the_ends_hold_the_list_whole() {
        let (mut wheel, mut first) = (Wheel::default(), 0);
        scroll(&mut wheel, &mut first, -0.5, 10, 4);
        assert_eq!((first, wheel.part(first)), (0, 0.0));
        scroll(&mut wheel, &mut first, 6.5, 10, 4);
        assert_eq!((first, wheel.part(first)), (6, 0.0));
        scroll(&mut wheel, &mut first, 9.0, 10, 4);
        assert_eq!((first, wheel.part(first)), (6, 0.0));
        // A list that fits doesn't move.
        let (mut wheel, mut first) = (Wheel::default(), 0);
        scroll(&mut wheel, &mut first, 0.5, 3, 4);
        assert_eq!((first, wheel.part(first)), (0, 0.0));
    }

    #[test]
    fn a_list_moved_away_and_back_drops_the_part() {
        let (mut wheel, mut first) = (Wheel::default(), 2);
        scroll(&mut wheel, &mut first, 0.5, 10, 4);
        assert_eq!(wheel.part(2), 0.5);
        // Seen at the end by keys, then back where the part was left.
        assert_eq!(wheel.part(6), 0.0);
        assert_eq!(wheel.part(2), 0.0);
    }

    #[test]
    fn the_shift_stays_within_the_row() {
        assert_eq!(shift(0.0, 40), 0);
        assert_eq!(shift(0.5, 40), 20);
        assert_eq!(shift(0.999, 40), 39);
    }
}
