//! Where the Antigravity CLI keeps a chat: a conversation's transcripts in
//! `<home>/brain/<id>/.system_generated/logs/`, found by its id or, by
//! folder and time, in `history.jsonl`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{Form, Found, Homes, Span, found, plain_id},
    },
    chat::source,
    integrations::common::pick::{Begun, nearest},
};

/// The whole transcript's name; herdr's hook names the cut one beside it.
pub(super) const FULL: &str = "transcript_full.jsonl";

/// The folder of conversation `id`'s transcripts under home `home`:
/// `brain/<id>/.system_generated/logs`.
pub(super) fn logs(home: &Path, id: &str) -> PathBuf {
    (home.join("brain").join(id)).join(".system_generated/logs")
}

/// The home a transcript at `path` (in `logs`) is under.
pub(super) fn home_of(path: &Path) -> Option<&Path> {
    path.ancestors().nth(5)
}

/// The conversation a transcript at `path` (in `logs`) is of.
pub(super) fn id_of(path: &Path) -> Option<&str> {
    (path.ancestors().nth(3)).and_then(|dir| dir.file_name()?.to_str())
}

/// The Antigravity CLI's transcript of conversation `id`, the whole one:
/// `brain/<id>/.system_generated/logs/transcript_full.jsonl`.
pub fn transcript(home: &Path, id: &str) -> PathBuf {
    logs(home, id).join(FULL)
}

/// Antigravity CLI conversation `id`.
pub(super) fn by_id(home: &Path, id: &str) -> Option<Found> {
    let main = transcript(home, id);
    main.is_file()
        .then(|| found(AgentKind::Antigravity, id, main, Form::Lines))
}

/// The Antigravity CLI conversation of `chat`'s run, by folder and time:
/// its `history.jsonl` has a line per prompt typed, with the conversation
/// and the folder (`{timestamp, workspace, conversationId}`); a
/// conversation begins with its first.
pub(super) fn by_time(
    home: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let history = std::fs::read_to_string(home.join("history.jsonl")).ok()?;
    let mut metas: Vec<Begun> = Vec::new();
    for line in history.lines() {
        let Ok(line) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(cwd), Some(ms)) = (
            line["conversationId"].as_str(),
            line["workspace"].as_str(),
            line["timestamp"].as_u64(),
        ) else {
            continue;
        };
        if !metas.iter().any(|m| m.id == id) {
            let (id, cwd, started) = (id.to_owned(), PathBuf::from(cwd), ms / 1000);
            metas.push(Begun { id, cwd, started });
        }
    }
    let meta = nearest(chat, span, claimed, metas.iter().map(|m| (m, m)))?;
    plain_id(&meta.id).then(|| by_id(home, &meta.id)).flatten()
}

/// The whole transcript of the Antigravity CLI conversation whose
/// transcript is at `path` (herdr's hook names the cut one), if it is one
/// under its home's `brain/`.
pub(super) fn transcript_at(path: &str, homes: &Homes) -> Option<PathBuf> {
    let brain = homes.antigravity.join("brain");
    let real = source::transcript(path, std::slice::from_ref(&brain), |_| true)?;
    let full = real.with_file_name(FULL);
    Some(if full.is_file() { full } else { real })
}
