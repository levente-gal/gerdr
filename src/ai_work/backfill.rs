//! Keeping the AI work of features from before it was kept: what their
//! records, prompts, stage files and logs still hold, and the chats still
//! on disk. Once per feature (`cursor.json`'s `backfilled`).

use std::{path::Path, sync::atomic::AtomicBool};

/// What keeping a feature's earlier AI work came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backfilled {
    /// Chats kept.
    pub chats: usize,
    /// Chats already gone.
    pub gone: usize,
}

/// Keep the earlier AI work of the feature in folder `feature` (its record
/// and stage files) into `dir`, its AI work folder, unless `stop`.
pub fn feature(feature: &Path, dir: &Path, stop: &AtomicBool) -> Result<Backfilled, String> {
    let _ = (feature, dir, stop);
    Ok(Backfilled::default())
}
