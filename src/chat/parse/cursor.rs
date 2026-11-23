//! A Cursor chat, read out of its `store.db` a message at a time.

use serde_json::Value;

use super::Reader;

impl Reader {
    /// Read message `row`, the `n`th of the chat (see `super::snapshot`).
    // Wave 1 (OpenCode and Cursor parsers) fills this in.
    pub(super) fn cursor_row(&mut self, _n: usize, _row: &Value) {}
}
