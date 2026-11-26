//! Finding a Codex chat: its rollout by id, else by folder and time from
//! each rollout's first line (`session_meta`), here or on another machine.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    ai_work::{
        AgentKind, Chat,
        locate::{FIRST_LINE, Form, Found, Span, civil, found, iso_secs, subdirs},
    },
    chat::source::transcript,
    integrations::common::{
        files::first_line,
        pick::{Begun, nearest},
    },
};

/// The rollouts in Codex date folder `day`.
fn rollouts(day: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(day) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = (entries.flatten())
        .map(|e| e.path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("rollout-") && name.ends_with(".jsonl") && path.is_file()
        })
        .collect();
    files.sort();
    files
}

/// Codex chat `id`: `sessions/<y>/<m>/<d>/rollout-<time>-<id>.jsonl`.
pub(crate) fn codex_by_id(codex: &Path, id: &str) -> Option<Found> {
    let end = format!("-{id}.jsonl");
    let years = subdirs(&codex.join("sessions"));
    let days = (years.iter().rev())
        .flat_map(|y| subdirs(y).into_iter().rev())
        .flat_map(|m| subdirs(&m).into_iter().rev());
    let main = days.flat_map(|day| rollouts(&day)).find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(&end))
    })?;
    Some(found(AgentKind::Codex, id, main, Form::Lines))
}

/// The Codex date folders a run that began at `started` may have put its
/// chat in: the day before to the day after, whatever the time zone.
pub fn codex_days(started: u64) -> Vec<String> {
    days_of_span(started, started)
}

/// The Codex date folders of chats begun from `from` to `to`, a day more
/// on either side.
fn days_of_span(from: u64, to: u64) -> Vec<String> {
    ((from / 86_400) as i64 - 1..=(to / 86_400) as i64 + 1)
        .map(|day| {
            let (y, m, d) = civil(day);
            format!("{y:04}/{m:02}/{d:02}")
        })
        .collect()
}

pub(crate) fn codex_meta(first: &str) -> Option<Begun> {
    let line: Value = serde_json::from_str(first.trim()).ok()?;
    if line["type"] != "session_meta" {
        return None;
    }
    let payload = &line["payload"];
    let id = payload["id"].as_str().or(payload["session_id"].as_str())?;
    let at = payload["timestamp"]
        .as_str()
        .or(line["timestamp"].as_str())?;
    Some(Begun {
        id: id.to_owned(),
        cwd: PathBuf::from(payload["cwd"].as_str()?),
        started: iso_secs(at)?,
    })
}

/// The Codex chat of `chat`'s run, by folder and time.
pub(crate) fn codex_by_time(
    codex: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Option<Found> {
    let sessions = codex.join("sessions");
    let metas: Vec<(PathBuf, Begun)> = (days_of_span(span.from, span.to).iter())
        .flat_map(|day| rollouts(&sessions.join(day)))
        .filter_map(|path| {
            let meta = codex_meta(&first_line(&path, FIRST_LINE, false)?)?;
            Some((path, meta))
        })
        .collect();
    let at = nearest(
        chat,
        span,
        claimed,
        metas.iter().enumerate().map(|(i, (_, m))| (i, m)),
    )?;
    let (main, meta) = &metas[at];
    Some(found(AgentKind::Codex, &meta.id, main.clone(), Form::Lines))
}

/// The body of `script` for Codex chat `id`, else `chat`'s by time.
pub(crate) fn codex_script(chat: &Chat, id: Option<&str>) -> String {
    match id {
        Some(id) => format!(
            r#"for f in "${{CODEX_HOME:-$HOME/.codex}}"/sessions/*/*/*/rollout-*-'{id}'.jsonl; do
  [ -f "$f" ] && say M '' "$f" && exit 0
done
"#
        ),
        None => {
            let days = codex_days(chat.started).join(" ");
            format!(
                r#"for day in {days}; do
  for f in "${{CODEX_HOME:-$HOME/.codex}}"/sessions/$day/rollout-*.jsonl; do
    [ -f "$f" ] || continue
    say C '' "$f"
    printf 'L\t'; head -n 1 "$f"; echo
  done
done
"#
            )
        }
    }
}

/// `path` as it really is, if it is a Codex rollout: a regular
/// `rollout-*.jsonl` file under `sessions/` of Codex's home `codex`.
pub(crate) fn codex_rollout(path: &str, codex: &Path) -> Option<PathBuf> {
    transcript(path, &[codex.join("sessions")], |name| {
        name.starts_with("rollout-")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::{
        locate::{
            Homes, find,
            tests::{chat, put},
        },
        write::tests::scratch,
    };

    fn rollout(codex: &Path, day: &str, id: &str, cwd: &Path, at: &str) -> PathBuf {
        let path = codex.join(format!(
            "sessions/{day}/rollout-2026-08-26T13-58-45-{id}.jsonl"
        ));
        let meta = serde_json::json!({
            "timestamp": at,
            "type": "session_meta",
            "payload": {"id": id, "timestamp": at, "cwd": cwd},
        });
        put(&path, &format!("{meta}\n{{\"type\":\"turn\"}}\n"));
        path
    }

    #[test]
    fn a_codex_chat_is_found_by_id() {
        let home = scratch("locate-codex-id");
        let homes = Homes::under(&home);
        let path = rollout(
            &homes.codex,
            "2026/08/26",
            "019a-1",
            Path::new("/w"),
            "2026-08-26T10:58:45Z",
        );
        rollout(
            &homes.codex,
            "2026/08/26",
            "019a-2",
            Path::new("/w"),
            "2026-08-26T10:58:45Z",
        );
        let c = chat(AgentKind::Codex, Some("019a-1"), 0, &[]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.id.as_str(), f.main), ("019a-1", path));
    }

    #[test]
    fn a_codex_chat_is_found_by_folder_and_time() {
        let home = scratch("locate-codex-time");
        let homes = Homes::under(&home);
        let work = Path::new("/work/feat");
        let started = iso_secs("2026-08-26T10:58:00Z").unwrap();
        // Another folder, too early, too late, then two that fit: the
        // nearest wins unless another run has it.
        rollout(
            &homes.codex,
            "2026/08/26",
            "other",
            Path::new("/else"),
            "2026-08-26T10:58:10Z",
        );
        rollout(
            &homes.codex,
            "2026/08/26",
            "early",
            work,
            "2026-08-26T10:56:00Z",
        );
        rollout(
            &homes.codex,
            "2026/08/26",
            "late",
            work,
            "2026-08-26T11:09:00Z",
        );
        let near = rollout(
            &homes.codex,
            "2026/08/26",
            "near",
            &work.join("api"),
            "2026-08-26T10:58:30Z",
        );
        let far = rollout(
            &homes.codex,
            "2026/08/27",
            "far",
            work,
            "2026-08-26T11:05:00Z",
        );
        let c = chat(AgentKind::Codex, None, started, &[work]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!((f.id.as_str(), &f.main), ("near", &near));
        let claimed = HashSet::from(["near".to_owned()]);
        let f = find(&homes, &c, &claimed).unwrap().unwrap();
        assert_eq!((f.id.as_str(), &f.main), ("far", &far));
    }
}
