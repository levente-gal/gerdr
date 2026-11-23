//! OpenCode's chat, read out of `opencode.db` a row at a time.

use serde_json::Value;

use super::Reader;

impl Reader {
    /// Read message row `row` of an OpenCode session (see `super::snapshot`).
    // Wave 1 (OpenCode and Cursor parsers) fills this in.
    pub(super) fn opencode_row(&mut self, _row: &Value) {}
}
