//! The stats panel: what the feature's AI work came to, read from its index
//! (see `ai_work::stats`), by feature, by agent and by check.

use crate::{Gerdr, ai_work::stats};

impl Gerdr {
    /// Open the stats panel, or close it again.
    pub(crate) fn toggle_ai_stats(&mut self) {}

    /// The index was read for `scope`.
    pub(crate) fn ai_stats_read(
        &mut self,
        scope: stats::Scope,
        result: Result<stats::View, String>,
    ) {
        let _ = (scope, result);
    }
}
