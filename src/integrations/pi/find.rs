//! Where pi keeps a chat: `sessions/--<folder>--/<time>_<id>.jsonl`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{FIRST_LINE, Form, Found, Span, found, plain_id, subdirs},
        usage::iso_ms,
    },
    integrations::common::{
        files::first_line,
        pick::{Begun, nearest},
    },
};

/// pi's sessions under `sessions`, with when each began by its name
/// (`<yyyy-mm-ddThh-mm-ss-mmmZ>_<id>.jsonl`), in a folder per working
/// folder.
fn sessions_in(sessions: &Path) -> Vec<(PathBuf, u64)> {
    (subdirs(sessions).iter())
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let (time, _) = name.strip_suffix(".jsonl")?.split_once('_')?;
            // `2026-11-24T19-27-30-349Z` as ISO: its clock's dashes as colons.
            let (date, clock) = time.split_once('T')?;
            let mut parts = clock.trim_end_matches('Z').split('-');
            let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
            let at = iso_ms(&format!("{date}T{h}:{m}:{s}Z"))? / 1000;
            Some((path, at))
        })
        .collect()
}

/// pi chat `id`: `sessions/--<folder>--/<time>_<id>.jsonl`.
pub(crate) fn by_id(sessions: &Path, id: &str) -> Option<Found> {
    let end = format!("_{id}.jsonl");
    let (main, _) = (sessions_in(sessions).into_iter())
        .find(|(path, _)| path.to_str().is_some_and(|p| p.ends_with(&end)))?;
    Some(found(AgentKind::Pi, id, main, Form::Lines))
}

/// The pi chat of `chat`'s run, by folder and time: its first line says
/// where it runs (`{"type":"session","id","timestamp","cwd"}`).
pub(crate) fn by_time(
    sessions: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let metas: Vec<(PathBuf, Begun)> = (sessions_in(sessions).into_iter())
        .filter(|(_, at)| (span.from..=span.to).contains(at))
        .filter_map(|(path, _)| {
            let head: Value =
                serde_json::from_str(first_line(&path, FIRST_LINE, false)?.trim()).ok()?;
            if head["type"] != "session" {
                return None;
            }
            let meta = Begun {
                id: head["id"].as_str()?.to_owned(),
                cwd: PathBuf::from(head["cwd"].as_str()?),
                started: iso_ms(head["timestamp"].as_str()?)? / 1000,
            };
            Some((path, meta))
        })
        .collect();
    let at = nearest(
        chat,
        span,
        claimed,
        (metas.iter().enumerate()).map(|(i, (_, m))| (i, m)),
    )?;
    let (main, meta) = &metas[at];
    plain_id(&meta.id).then(|| found(AgentKind::Pi, &meta.id, main.clone(), Form::Lines))
}
