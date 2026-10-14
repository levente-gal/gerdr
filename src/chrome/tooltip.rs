//! The tooltip over the thing under the pointer.

use super::Rect;

/// What the tooltip shows: its text, the key doing the same, and what it
/// is about (window coordinates), to sit below or above it.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipView {
    pub text: String,
    pub key: Option<String>,
    pub anchor: Rect,
}
