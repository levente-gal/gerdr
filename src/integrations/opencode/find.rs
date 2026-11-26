//! Where OpenCode keeps a chat: a session in its database, `opencode.db`,
//! by its id or by folder and time.

use std::{collections::HashSet, path::Path};

use serde_json::Value;

use crate::ai_work::{
    AgentKind, Chat,
    locate::{Form, Found, Span, columns, found, plain_id, pretty, rows_json},
};

/// The OpenCode chat of `chat`: by its id, or by folder and time. Its
/// schema isn't known for sure, so it is asked first; a database not as
/// expected finds nothing.
pub(crate) fn find(
    db: &Path,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Result<Option<Found>, String> {
    if !db.is_file() {
        return Ok(None);
    }
    let read = || -> rusqlite::Result<Option<Found>> {
        let conn = crate::cursor_agents::open_read_only(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let has = |table: &str, want: &[&str]| -> rusqlite::Result<bool> {
            let cols = columns(&conn, table)?;
            Ok(want.iter().all(|w| cols.iter().any(|c| c == w)))
        };
        let need = ["id", "directory", "time_created"];
        // OpenCode 2 keeps both: older chats in the first tables, newer
        // ones only in the second.
        let mut schemas = Vec::new();
        if has("session", &need)? && has("message", &["session_id", "data"])? {
            schemas.push(("session", false));
        }
        if has("session_v2", &need)? && has("session_message", &["session_id", "seq", "data"])? {
            schemas.push(("session_v2", true));
        }
        let mut hit = None;
        for (session, v2) in schemas {
            if let Some(row) = session_in(&conn, session, chat, claimed, span)? {
                hit = Some((row, v2));
                break;
            }
        }
        let Some((session_row, v2)) = hit else {
            return Ok(None);
        };
        let id = session_row["id"].as_str().unwrap_or_default().to_owned();
        if !plain_id(&id) {
            return Ok(None);
        }
        let mut lines = Vec::new();
        let messages = match v2 {
            false => rows_json(
                &conn,
                "SELECT * FROM message WHERE session_id = ?1 ORDER BY time_created, id",
                &id,
            )?,
            true => rows_json(
                &conn,
                "SELECT * FROM session_message WHERE session_id = ?1 ORDER BY seq",
                &id,
            )?,
        };
        let parts = match !v2 && has("part", &["message_id", "session_id", "data"])? {
            true => rows_json(
                &conn,
                "SELECT * FROM part WHERE session_id = ?1 ORDER BY time_created, id",
                &id,
            )?,
            false => Vec::new(),
        };
        for mut message in messages {
            let own: Vec<Value> = (parts.iter())
                .filter(|p| p["message_id"] == message["id"])
                .cloned()
                .collect();
            if !own.is_empty() {
                message["parts"] = Value::Array(own);
            }
            lines.extend(serde_json::to_vec(&message).unwrap_or_default());
            lines.push(b'\n');
        }
        let mut chat = found(
            AgentKind::OpenCode,
            &id,
            db.to_path_buf(),
            Form::Made(lines),
        );
        chat.rows = vec![("opencode/session.json".to_owned(), pretty(&session_row))];
        Ok(Some(chat))
    };
    read().map_err(|err| format!("reading {}: {err}", db.display()))
}

/// The session row of `chat` in OpenCode's table `table`.
fn session_in(
    conn: &rusqlite::Connection,
    table: &str,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> rusqlite::Result<Option<Value>> {
    if let Some(id) = &chat.id {
        let rows = rows_json(conn, &format!("SELECT * FROM {table} WHERE id = ?1"), id)?;
        return Ok(rows.into_iter().next());
    }
    // OpenCode's times are in milliseconds.
    let (from, to) = (span.from * 1000, span.to * 1000);
    let rows = rows_json(
        conn,
        &format!(
            "SELECT * FROM {table} WHERE time_created BETWEEN {from} AND {to} AND ?1 = ?1 \
             ORDER BY time_created"
        ),
        "",
    )?;
    let near = span.near * 1000;
    Ok((rows.into_iter())
        .filter(|row| row["parent_id"].is_null())
        .filter(|row| !row["id"].as_str().is_some_and(|id| claimed.contains(id)))
        .filter(|row| {
            let dir = Path::new(row["directory"].as_str().unwrap_or(""));
            !dir.as_os_str().is_empty() && chat.folders.iter().any(|f| dir.starts_with(f))
        })
        .min_by_key(|row| row["time_created"].as_u64().unwrap_or(0).abs_diff(near)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai_work::locate::{Homes, find},
        feature::Stage,
    };

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-ai-work-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn chat(agent: AgentKind, id: Option<&str>, started: u64, folders: &[&Path]) -> Chat {
        Chat {
            agent,
            id: id.map(str::to_owned),
            stage: Stage::Build,
            n: 1,
            started,
            folders: folders.iter().map(|f| f.to_path_buf()).collect(),
            machine: None,
        }
    }

    #[test]
    fn an_opencode_chat_is_read_out_of_its_database() {
        let home = scratch("locate-opencode");
        let homes = Homes::under(&home);
        std::fs::create_dir_all(homes.opencode.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&homes.opencode).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_a', NULL, '/w/x', 100500);
             INSERT INTO session VALUES ('ses_sub', 'ses_a', '/w/x', 100600);
             INSERT INTO message VALUES ('m1', 'ses_a', 100700, '{\"role\":\"user\"}');
             INSERT INTO part VALUES ('p1', 'm1', 'ses_a', 100701, '{\"text\":\"hi\"}');",
        )
        .unwrap();
        drop(conn);
        let c = chat(AgentKind::OpenCode, None, 100, &[Path::new("/w")]);
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        assert_eq!(f.id, "ses_a");
        let Form::Made(lines) = &f.form else {
            panic!("{:?}", f.form)
        };
        let line: Value =
            serde_json::from_slice(lines.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(line["data"]["role"], "user");
        assert_eq!(line["parts"][0]["data"]["text"], "hi");
        assert_eq!(f.rows[0].0, "opencode/session.json");
    }

    #[test]
    fn a_newer_opencode_chat_is_found_beside_older_ones() {
        let home = scratch("locate-opencode2");
        let homes = Homes::under(&home);
        std::fs::create_dir_all(homes.opencode.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&homes.opencode).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, time_created INTEGER);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_old', NULL, '/w/x', 100500);
             INSERT INTO session_v2 VALUES ('ses_old', NULL, '/w/x', 100500);
             INSERT INTO session_v2 VALUES ('ses_new', NULL, '/w/y', 200500);
             INSERT INTO session_message VALUES ('m1', 'ses_new', 'user', 1, '{\"text\":\"hi\"}');",
        )
        .unwrap();
        drop(conn);
        let mut c = chat(AgentKind::OpenCode, None, 200, &[Path::new("/w/y")]);
        c.id = Some("ses_new".into());
        let f = find(&homes, &c, &HashSet::new()).unwrap().unwrap();
        let Form::Made(lines) = &f.form else {
            panic!("{:?}", f.form)
        };
        let line: Value =
            serde_json::from_slice(lines.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(
            (line["type"].as_str(), line["data"]["text"].as_str()),
            (Some("user"), Some("hi"))
        );
        // By folder and time too.
        c.id = None;
        assert_eq!(
            find(&homes, &c, &HashSet::new()).unwrap().unwrap().id,
            "ses_new"
        );
    }
}
