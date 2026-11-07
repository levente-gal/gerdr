//! What happened to a feature, told from its record: each save is held up
//! against a `Digest` of the one before, and what changed between them
//! becomes events. Saving is the one place every change passes through.

use super::Event;
use crate::feature::Feature;

/// What is kept of a record to tell the next save's changes by: small, and
/// quick to take on the loop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digest {
    /// The history's length.
    pub history: usize,
}

/// The digest of `feature`.
pub fn digest(feature: &Feature) -> Digest {
    Digest {
        history: feature.history.len(),
    }
}

/// What happened between `prev` and `feature`. With no `prev` (the feature
/// is new to this gerdr) nothing is told.
pub fn diff(prev: Option<&Digest>, feature: &Feature) -> Vec<Event> {
    let _ = (prev, feature);
    Vec::new()
}
