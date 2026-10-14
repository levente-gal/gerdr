//! The placeholder over a pane whose window has not shown up yet.

use std::time::Instant;

use super::Rect;

/// What a placeholder shows: where the window goes, what is starting
/// there ("Zed"), since when, and whether it took too long.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceholderView {
    pub rect: Rect,
    pub label: String,
    pub since: Instant,
    pub timed_out: bool,
}
