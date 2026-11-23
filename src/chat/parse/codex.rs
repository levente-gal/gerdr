//! Codex's rollout (`sessions/<y>/<m>/<d>/rollout-<time>-<id>.jsonl`): a
//! JSON line per thing said or done, `{timestamp, type, payload}`.

use serde_json::Value;

use super::Reader;

impl Reader {
    /// Read rollout line `line`.
    // Wave 1 (Codex parser) fills this in.
    pub(super) fn codex_line(&mut self, _line: &Value) {}
}
