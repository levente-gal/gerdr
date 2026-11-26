//! The chat of a run among many, by folder and time.

use std::{collections::HashSet, path::PathBuf};

use crate::ai_work::{Chat, locate::Span};

/// What a chat's start says: its id, folder and start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Begun {
    pub id: String,
    pub cwd: PathBuf,
    pub started: u64,
}

/// Of `metas`, the one of `chat`'s run: in one of its folders, begun in
/// `span`, not `claimed`; the nearest in time.
pub fn nearest<'a, T>(
    chat: &Chat,
    span: Span,
    claimed: &HashSet<String>,
    metas: impl Iterator<Item = (T, &'a Begun)>,
) -> Option<T> {
    (metas)
        .filter(|(_, meta)| (span.from..=span.to).contains(&meta.started))
        .filter(|(_, meta)| !claimed.contains(&meta.id))
        .filter(|(_, meta)| chat.folders.iter().any(|f| meta.cwd.starts_with(f)))
        .min_by_key(|(_, meta)| meta.started.abs_diff(span.near))
        .map(|(it, _)| it)
}
