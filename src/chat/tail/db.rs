//! A database chat (OpenCode's, Cursor's) read by what changed since the
//! last read, not whole each time.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
};

use serde_json::Value;

use crate::{ai_work::locate::rows_of, integrations::DbChat};

/// What of an OpenCode chat was read.
#[derive(Default)]
pub struct OpenCodeDb {
    /// The messages, by seq (v2) or time created (v1), then id.
    rows: BTreeMap<(i64, String), Value>,
    /// v1 parts, by message id, time created and id.
    parts: BTreeMap<(String, i64, String), Value>,
    /// The newest `time_updated` read, by table. Rows from it on are read
    /// again: OpenCode writes a row over as it streams, and a write in the
    /// same millisecond keeps the time.
    marks: HashMap<&'static str, i64>,
    /// Whether the chat is in the v2 tables; None: not looked.
    v2: Option<bool>,
    /// The child sessions its `task` calls started (subagents), by id,
    /// each kept as a chat of its own; and whether its calls ended, so it
    /// grows no more and isn't read again.
    children: HashMap<String, (OpenCodeDb, bool)>,
}

impl DbChat for OpenCodeDb {
    fn read(&mut self, id: &str, conn: &rusqlite::Connection) -> Result<(), String> {
        self.opencode(conn, id)
    }

    /// The rows: v1 messages carry their parts, and a `task` part its
    /// child session's rows, once read, as `subagent`.
    fn rows(&self) -> Vec<Cow<'_, Value>> {
        (self.rows.iter())
            .map(|((_, id), row)| {
                let from = (id.clone(), i64::MIN, String::new());
                let parts: Vec<Value> = (self.parts.range(from..))
                    .take_while(|((message, ..), _)| message == id)
                    .map(|(_, part)| {
                        let mut part = part.clone();
                        self.attach(&mut part["data"]);
                        part
                    })
                    .collect();
                let content = row["data"]["content"].as_array().into_iter().flatten();
                let tasks = content.clone().any(|part| self.child(part).is_some());
                if parts.is_empty() && !tasks {
                    return Cow::Borrowed(row);
                }
                let mut row = row.clone();
                if !parts.is_empty() {
                    row["parts"] = Value::Array(parts);
                }
                let content = row["data"]["content"].as_array_mut().into_iter().flatten();
                content.for_each(|part| self.attach(part));
                Cow::Owned(row)
            })
            .collect()
    }
}

impl OpenCodeDb {
    /// The child session OpenCode tool part `part` started, when it is a
    /// `task` call and that session was read.
    fn child<'a>(&'a self, part: &Value) -> Option<&'a OpenCodeDb> {
        let (child, _) = self.children.get(started(part)?)?;
        child.v2.is_some().then_some(child)
    }

    /// Tool part `part` given its child session's rows, when it has one.
    fn attach(&self, part: &mut Value) {
        if let Some(child) = self.child(part) {
            let rows = child.rows().into_iter().map(Cow::into_owned).collect();
            part["subagent"] = Value::Array(rows);
        }
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
        self.subagents(conn, id);
        Ok(())
    }

    /// Read what changed in the child sessions the `task` calls started:
    /// those still running, and each once after its calls ended.
    fn subagents(&mut self, conn: &rusqlite::Connection, own: &str) {
        // A session a later call resumes has all its calls to end.
        let mut tasks: HashMap<String, bool> = HashMap::new();
        let parts = (self.parts.values().map(|part| &part["data"])).chain(
            (self.rows.values())
                .flat_map(|row| row["data"]["content"].as_array().into_iter().flatten()),
        );
        for part in parts {
            // A session naming itself would read itself without end.
            if let Some(child) = started(part).filter(|child| *child != own) {
                let ended = matches!(
                    part["state"]["status"].as_str(),
                    Some("completed" | "error")
                );
                *tasks.entry(child.to_owned()).or_insert(true) &= ended;
            }
        }
        self.children.retain(|child, _| tasks.contains_key(child));
        for (id, ended) in tasks {
            let (child, settled) = self.children.entry(id.clone()).or_default();
            // A later call resuming it reads it again.
            if *settled && ended {
                continue;
            }
            match child.opencode(conn, &id) {
                Ok(()) => *settled = ended,
                // One subagent unread leaves the chat as it is: its call shows.
                Err(err) => tracing::warn!("agent chat: can't read OpenCode subagent {id}: {err}"),
            }
        }
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

/// The child session OpenCode tool part `part` (v1's or v2's) started:
/// a `task` call's, as its metadata names it from when it runs.
fn started(part: &Value) -> Option<&str> {
    let name = part["tool"].as_str().or(part["name"].as_str());
    if name != Some("task") {
        return None;
    }
    part["state"]["metadata"]["sessionId"].as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::AgentKind;

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

    fn texts(kept: &OpenCodeDb) -> Vec<String> {
        (kept.rows().iter())
            .flat_map(|row| row["parts"].as_array().cloned().unwrap_or_default())
            .map(|part| part["data"]["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn an_opencode_chat_reads_what_was_written_since() {
        let conn = opencode_db();
        let mut kept = OpenCodeDb::default();
        kept.read("ses_a", &conn).unwrap();
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
        kept.read("ses_a", &conn).unwrap();
        assert_eq!(texts(&kept), ["word", "s"]);
        assert_eq!(kept.rows().len(), 1);
        assert_eq!(kept.marks["part"], 900);
        let mut other = OpenCodeDb::default();
        assert!(other.read("ses_c", &conn).is_err());
    }

    /// The items of what `kept` read, as the chat shows them.
    fn items(kept: &OpenCodeDb) -> Vec<crate::chat::Item> {
        let rows = kept.rows();
        crate::chat::parse::snapshot(AgentKind::OpenCode, rows.iter().map(|row| &**row))
    }

    /// The texts of the answers in subagent card `item`.
    fn said(item: &crate::chat::Item) -> (String, Vec<String>, bool) {
        let crate::chat::Item::Subagent {
            id, items, output, ..
        } = item
        else {
            panic!("{item:?}")
        };
        let said = (items.iter())
            .filter_map(|item| match item {
                crate::chat::Item::Assistant { blocks, .. } => Some(format!("{blocks:?}")),
                _ => None,
            })
            .collect();
        (id.clone(), said, output.is_some())
    }

    #[test]
    fn an_opencode_task_shows_its_child_session_as_it_grows() {
        let conn = opencode_db();
        let task = |status: &str| {
            format!(
                "{{\"type\":\"tool\",\"tool\":\"task\",\"callID\":\"call_t\",\"state\":{{\"status\":\"{status}\",\
                 \"input\":{{\"description\":\"Look around\",\"prompt\":\"go\"}},\
                 \"metadata\":{{\"sessionId\":\"ses_c\"}},\"output\":\"done\"}}}}"
            )
        };
        conn.execute(
            "INSERT INTO part VALUES ('p9', 'm2', 'ses_a', 850, 850, ?1)",
            [task("running")],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES ('ses_c');
             INSERT INTO message VALUES ('c1', 'ses_c', 860, 860, '{\"role\":\"assistant\"}');
             INSERT INTO part VALUES ('cp1', 'c1', 'ses_c', 861, 861, '{\"type\":\"text\",\"text\":\"Reading\"}');",
        )
        .unwrap();
        let mut kept = OpenCodeDb::default();
        kept.read("ses_a", &conn).unwrap();
        let card = |kept: &OpenCodeDb| said(items(kept).last().unwrap());
        let (id, words, ended) = card(&kept);
        assert_eq!(id, "agent:call_t");
        assert_eq!(words.len(), 1);
        assert!(!ended);
        // The child grows, the parent not: read all the same.
        conn.execute_batch(
            "INSERT INTO message VALUES ('c2', 'ses_c', 870, 870, '{\"role\":\"assistant\"}');
             INSERT INTO part VALUES ('cp2', 'c2', 'ses_c', 871, 871, '{\"type\":\"text\",\"text\":\"Found\"}');",
        )
        .unwrap();
        kept.read("ses_a", &conn).unwrap();
        assert_eq!(card(&kept).1.len(), 2);
        // Its call ended: read once more, then no more.
        conn.execute(
            "UPDATE part SET data = ?1, time_updated = 880 WHERE id = 'p9'",
            [task("completed")],
        )
        .unwrap();
        kept.read("ses_a", &conn).unwrap();
        assert!(card(&kept).2);
        assert!(kept.children["ses_c"].1);
        conn.execute_batch(
            "INSERT INTO message VALUES ('c3', 'ses_c', 890, 890, '{\"role\":\"assistant\"}');",
        )
        .unwrap();
        kept.read("ses_a", &conn).unwrap();
        assert_eq!(kept.children["ses_c"].0.rows.len(), 2);
        // The call gone, its child goes too.
        conn.execute_batch("DELETE FROM part WHERE id = 'p9';")
            .unwrap();
        kept.read("ses_a", &conn).unwrap();
        assert!(kept.children.is_empty());
    }

    #[test]
    fn a_v2_opencode_task_shows_its_child_session() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY);
             CREATE TABLE session_v2 (id TEXT PRIMARY KEY);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session_v2 VALUES ('ses_a'); INSERT INTO session_v2 VALUES ('ses_c');
             INSERT INTO session_message VALUES ('s1', 'ses_a', 'assistant', 1, 10, 10,
               '{\"content\":[{\"type\":\"text\",\"text\":\"Asking\"},{\"type\":\"tool\",\"id\":\"call_t\",\"name\":\"task\",\"state\":{\"status\":\"running\",\"input\":{\"description\":\"Look\"},\"metadata\":{\"sessionId\":\"ses_c\"}}}]}');
             INSERT INTO session_message VALUES ('c1', 'ses_c', 'assistant', 1, 11, 11,
               '{\"content\":[{\"type\":\"text\",\"text\":\"Inside\"}]}');",
        )
        .unwrap();
        let mut kept = OpenCodeDb::default();
        kept.read("ses_a", &conn).unwrap();
        let items = items(&kept);
        assert_eq!(items.len(), 2, "{items:?}");
        let (id, words, ended) = said(&items[1]);
        assert_eq!(
            (id.as_str(), words.len(), ended),
            ("agent:call_t", 1, false)
        );
    }
}
