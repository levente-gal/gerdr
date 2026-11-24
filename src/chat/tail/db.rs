//! A database chat (OpenCode's, Cursor's) read by what changed since the
//! last read, not whole each time.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
};

use serde_json::Value;

use crate::ai_work::{AgentKind, locate::rows_of};

/// What of a database chat was read.
#[derive(Default)]
pub(super) struct Kept {
    /// The rows `parse::snapshot` takes, in its order: Cursor messages by
    /// rowid, OpenCode messages by seq (v2) or time created (v1); then id.
    rows: BTreeMap<(i64, String), Value>,
    /// OpenCode v1 parts, by message id, time created and id.
    parts: BTreeMap<(String, i64, String), Value>,
    /// Cursor: every blob's id by rowid, a message or not, as last read.
    /// Blobs are named by their content: a rowid with the same id is the
    /// same blob.
    blobs: HashMap<i64, String>,
    /// OpenCode: the newest `time_updated` read, by table. Rows from it on
    /// are read again: OpenCode writes a row over as it streams, and a
    /// write in the same millisecond keeps the time.
    marks: HashMap<&'static str, i64>,
    /// OpenCode: whether the chat is in the v2 tables; None: not looked.
    v2: Option<bool>,
}

impl Kept {
    /// Read what changed in database `conn` since the last read: rows new
    /// or written over, and rows gone.
    pub fn read(
        &mut self,
        agent: AgentKind,
        id: &str,
        conn: &rusqlite::Connection,
    ) -> Result<(), String> {
        match agent {
            AgentKind::Cursor => self.cursor(conn).map_err(|err| err.to_string()),
            _ => self.opencode(conn, id),
        }
    }

    /// The rows, as `parse::snapshot` takes them: OpenCode v1 messages
    /// carry their parts.
    pub fn rows(&self) -> Vec<Cow<'_, Value>> {
        (self.rows.iter())
            .map(|((_, id), row)| {
                let from = (id.clone(), i64::MIN, String::new());
                let parts: Vec<Value> = (self.parts.range(from..))
                    .take_while(|((message, ..), _)| message == id)
                    .map(|(_, part)| part.clone())
                    .collect();
                if parts.is_empty() {
                    return Cow::Borrowed(row);
                }
                let mut row = row.clone();
                row["parts"] = Value::Array(parts);
                Cow::Owned(row)
            })
            .collect()
    }

    fn cursor(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let mut ids = conn.prepare("SELECT rowid, id FROM blobs")?;
        let alive = (ids.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?)
            .collect::<rusqlite::Result<HashMap<i64, String>>>()?;
        self.rows
            .retain(|(rowid, id), _| alive.get(rowid) == Some(id));
        let new: HashSet<i64> = (alive.iter())
            .filter(|(rowid, id)| self.blobs.get(rowid) != Some(id))
            .map(|(rowid, _)| *rowid)
            .collect();
        // New blobs come last: one scan from the first of them.
        if let Some(from) = new.iter().min() {
            let mut data = conn.prepare("SELECT rowid, id, data FROM blobs WHERE rowid >= ?1")?;
            let mut rows = data.query([from])?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                // Some blobs are encrypted, or aren't messages at all.
                let (true, Ok(id), Ok(bytes)) = (
                    new.contains(&rowid),
                    row.get::<_, String>(1),
                    row.get::<_, Vec<u8>>(2),
                ) else {
                    continue;
                };
                if let Ok(blob) = serde_json::from_slice::<Value>(&bytes)
                    && blob["role"].is_string()
                {
                    self.rows.insert((rowid, id), blob);
                }
            }
        }
        self.blobs = alive;
        Ok(())
    }

    fn opencode(&mut self, conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
        let v2 = match self.v2 {
            Some(v2) => v2,
            // Like `locate`: an older chat may be in both; the first wins.
            None => {
                let has = |table: &str| {
                    let sql = format!("SELECT 1 FROM {table} WHERE id = ?1");
                    conn.query_row(&sql, [id], |_| Ok(())).is_ok()
                };
                let v2 = match (has("session"), has("session_v2")) {
                    (true, _) => false,
                    (false, true) => true,
                    (false, false) => return Err("its database holds this chat no more".into()),
                };
                *self.v2.insert(v2)
            }
        };
        let tables: &[&'static str] = match v2 {
            true => &["session_message"],
            false => &["message", "part"],
        };
        for &table in tables {
            self.opencode_table(conn, table, id)
                .map_err(|err| format!("reading {table}: {err}"))?;
        }
        Ok(())
    }

    /// Read OpenCode table `table`'s rows of chat `id` written since the
    /// last read, and drop the ones gone.
    fn opencode_table(
        &mut self,
        conn: &rusqlite::Connection,
        table: &'static str,
        id: &str,
    ) -> rusqlite::Result<()> {
        let mut ids = conn.prepare(&format!("SELECT id FROM {table} WHERE session_id = ?1"))?;
        let mut alive =
            (ids.query_map([id], |r| r.get(0))?).collect::<rusqlite::Result<HashSet<String>>>()?;
        let mark = self.marks.get(table).copied().unwrap_or(i64::MIN);
        let order = match table {
            "session_message" => "seq",
            _ => "time_created",
        };
        let sql = format!(
            "SELECT * FROM {table} WHERE session_id = ?1 AND time_updated >= ?2 ORDER BY {order}"
        );
        let new = rows_of(conn, &sql, rusqlite::params![id, mark])?;
        for row in &new {
            // Read again: its keys may have changed with it.
            alive.remove(row["id"].as_str().unwrap_or_default());
        }
        let newest = (new.iter())
            .filter_map(|row| row["time_updated"].as_i64())
            .max();
        if let Some(newest) = newest {
            self.marks.insert(table, newest.max(mark));
        }
        let key = |row: &Value| {
            let at = row[order].as_i64().unwrap_or(0);
            (at, row["id"].as_str().unwrap_or_default().to_owned())
        };
        if table == "part" {
            self.parts.retain(|(.., id), _| alive.contains(id));
            for row in new {
                let (at, id) = key(&row);
                let message = row["message_id"].as_str().unwrap_or_default().to_owned();
                self.parts.insert((message, at, id), row);
            }
        } else {
            self.rows.retain(|(_, id), _| alive.contains(id));
            for row in new {
                self.rows.insert(key(&row), row);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opencode_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES ('ses_a');
             INSERT INTO message VALUES ('m1', 'ses_a', 700, 700, '{\"role\":\"user\"}');
             INSERT INTO message VALUES ('m2', 'ses_a', 800, 800, '{\"role\":\"assistant\"}');
             INSERT INTO message VALUES ('x1', 'ses_b', 750, 750, '{\"role\":\"user\"}');
             INSERT INTO part VALUES ('p1', 'm1', 'ses_a', 701, 701, '{\"text\":\"hi\"}');
             INSERT INTO part VALUES ('p2', 'm2', 'ses_a', 801, 801, '{\"text\":\"wo\"}');",
        )
        .unwrap();
        conn
    }

    fn texts(kept: &Kept) -> Vec<String> {
        (kept.rows().iter())
            .flat_map(|row| row["parts"].as_array().cloned().unwrap_or_default())
            .map(|part| part["data"]["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn an_opencode_chat_reads_what_was_written_since() {
        let conn = opencode_db();
        let mut kept = Kept::default();
        kept.read(AgentKind::OpenCode, "ses_a", &conn).unwrap();
        let rows = kept.rows();
        assert_eq!(rows.len(), 2, "its own messages only");
        assert_eq!(rows[1]["data"]["role"], "assistant");
        assert_eq!(texts(&kept), ["hi", "wo"]);
        // Written over in the same millisecond, a new part, a message gone.
        conn.execute_batch(
            "UPDATE part SET data = '{\"text\":\"word\"}' WHERE id = 'p2';
             INSERT INTO part VALUES ('p3', 'm2', 'ses_a', 802, 900, '{\"text\":\"s\"}');
             DELETE FROM message WHERE id = 'm1'; DELETE FROM part WHERE id = 'p1';",
        )
        .unwrap();
        kept.read(AgentKind::OpenCode, "ses_a", &conn).unwrap();
        assert_eq!(texts(&kept), ["word", "s"]);
        assert_eq!(kept.rows().len(), 1);
        assert_eq!(kept.marks["part"], 900);
        let mut other = Kept::default();
        assert!(other.read(AgentKind::OpenCode, "ses_c", &conn).is_err());
    }

    #[test]
    fn a_cursor_chat_reads_only_blobs_it_has_not() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             INSERT INTO blobs VALUES ('a', CAST('{\"role\":\"user\",\"n\":1}' AS BLOB));
             INSERT INTO blobs VALUES ('root', CAST('{\"tree\":1}' AS BLOB));
             INSERT INTO blobs VALUES ('b', CAST('{\"role\":\"assistant\",\"n\":2}' AS BLOB));",
        )
        .unwrap();
        let mut kept = Kept::default();
        let ns = |kept: &Kept| -> Vec<i64> {
            (kept.rows().iter())
                .map(|r| r["n"].as_i64().unwrap())
                .collect()
        };
        kept.read(AgentKind::Cursor, "c", &conn).unwrap();
        assert_eq!(ns(&kept), [1, 2]);
        // The last blob gone and another one in its rowid.
        conn.execute_batch(
            "DELETE FROM blobs WHERE id = 'b';
             INSERT INTO blobs VALUES ('c', CAST('{\"role\":\"assistant\",\"n\":3}' AS BLOB));",
        )
        .unwrap();
        kept.read(AgentKind::Cursor, "c", &conn).unwrap();
        assert_eq!(ns(&kept), [1, 3]);
    }
}
