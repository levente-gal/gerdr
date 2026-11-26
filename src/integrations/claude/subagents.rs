//! Claude Code's subagents that write files of their own beside the chat's
//! (`<chat>/subagents/agent-<id>.jsonl`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    chat::{Source, parse::Reader},
    integrations::common::files::SubFiles,
};

/// What Claude Code's subagents wrote since the last read, each in its
/// own file beside the chat's (`<chat>/subagents/agent-<id>.jsonl`), the
/// call that started it named in its `.meta.json`. Only those whose
/// call was read: their work has a place to show.
pub(crate) fn read(
    chat: &Source,
    reader: &mut Reader,
    files: &mut SubFiles,
    chunk: u64,
) -> std::io::Result<()> {
    let dir = chat.path.with_extension("").join("subagents");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(agent) = (path.file_name().and_then(|n| n.to_str()))
            .and_then(|n| n.strip_prefix("agent-")?.strip_suffix(".jsonl"))
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(call) = started_by(&mut files.noted, &path) else {
            continue;
        };
        if !reader.knows_call(&call) {
            continue;
        }
        if let Err(err) = files.read(reader, &path, &agent, &call, chunk) {
            // One subagent unread leaves the chat as it is.
            tracing::warn!("agent chat: can't read {}: {err}", path.display());
        }
    }
    Ok(())
}

/// The call that started the subagent of file `path`, as its meta file
/// says (kept in `noted`); asked again until it says.
fn started_by(noted: &mut HashMap<PathBuf, String>, path: &Path) -> Option<String> {
    if let Some(call) = noted.get(path) {
        return Some(call.clone());
    }
    let meta = std::fs::read(path.with_extension("meta.json")).ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&meta).ok()?;
    let call = meta["toolUseId"].as_str()?.to_owned();
    noted.insert(path.to_owned(), call.clone());
    Some(call)
}
