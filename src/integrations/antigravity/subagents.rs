//! The Antigravity CLI's subagents: each a conversation of its own
//! (`brain/<id>/…`), read under the call that started it.

use std::path::{Path, PathBuf};

use super::find::{home_of, logs};
use crate::{
    chat::{Source, parse::Reader},
    integrations::common::{self, files::SubFiles},
};

/// What the Antigravity CLI's subagents wrote since the last read: each a
/// conversation of its own (`brain/<id>/…`), the one an `invoke_subagent`
/// call's result names; else a child conversation of the chat's, as the
/// summaries database says, whose first input is a prompt the call gave.
pub(super) fn read(
    chat: &Source,
    reader: &mut Reader,
    files: &mut SubFiles,
    chunk: u64,
) -> std::io::Result<()> {
    let path = &chat.path;
    let (Some(home), Some(name)) = (home_of(path), path.file_name()) else {
        return Ok(());
    };
    let transcript = |id: &str| logs(home, id).join(name);
    for child in reader.named_subagents() {
        if child != chat.id && transcript(&child).is_file() {
            reader.take_subagent(&child, "");
        }
    }
    if reader.unfound_subagents() > 0 {
        let db = home.join("conversation_summaries.db");
        find_subagents(&db, &chat.id, reader, files, transcript);
    }
    for child in reader.subagents() {
        let path = transcript(&child);
        if let Err(err) = files.read(reader, &path, &child, "", chunk) {
            // One subagent unread leaves the chat as it is.
            tracing::warn!("agent chat: can't read {}: {err}", path.display());
        }
    }
    Ok(())
}

/// Take the child conversations summaries database `db` has of chat `id`
/// as subagents, by their first input; looked in again when it changed or
/// more are to find.
fn find_subagents(
    db: &Path,
    id: &str,
    reader: &mut Reader,
    files: &mut SubFiles,
    transcript: impl Fn(&str) -> PathBuf,
) {
    let Ok(stamps) = common::files::with_wal(db) else {
        return;
    };
    let now = (stamps, reader.unfound_subagents());
    if files.looked == Some(now) {
        return;
    }
    files.looked = Some(now);
    let children = match children(db, id) {
        Ok(children) => children,
        Err(err) => {
            tracing::warn!("agent chat: can't read {}: {err}", db.display());
            return;
        }
    };
    let taken = reader.subagents();
    for child in children.iter().filter(|c| !taken.contains(c)) {
        match common::files::first_line(&transcript(child), u64::MAX, true) {
            Some(first) => _ = reader.take_subagent(child, &first),
            // Not written yet: looked for again.
            None => files.looked = None,
        }
    }
}

/// The conversations under conversation `id`, as summaries database `db`
/// has them.
fn children(db: &Path, id: &str) -> rusqlite::Result<Vec<String>> {
    let conn = crate::cursor_agents::open_read_only(db)?;
    let mut rows = conn.prepare(
        "SELECT conversation_id FROM conversation_summaries WHERE parent_conversation_id = ?1",
    )?;
    rows.query_map([id], |row| row.get(0))?.collect()
}
