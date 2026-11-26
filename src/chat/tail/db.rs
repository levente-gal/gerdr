//! A database chat (OpenCode's, Cursor's) read by what changed since the
//! last read, not whole each time.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
};

use serde_json::Value;

use crate::integrations::DbChat;

/// What of a Cursor chat was read.
#[derive(Default)]
pub struct CursorDb {
    /// The messages, by rowid and id.
    rows: BTreeMap<(i64, String), Value>,
    /// Every blob's id by rowid, a message or not, as last read. Blobs are
    /// named by their content: a rowid with the same id is the same blob.
    blobs: HashMap<i64, String>,
}

impl DbChat for CursorDb {
    fn read(&mut self, _id: &str, conn: &rusqlite::Connection) -> Result<(), String> {
        self.cursor(conn).map_err(|err| err.to_string())
    }

    fn rows(&self) -> Vec<Cow<'_, Value>> {
        self.rows.values().map(Cow::Borrowed).collect()
    }
}

impl CursorDb {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut kept = CursorDb::default();
        let ns = |kept: &CursorDb| -> Vec<i64> {
            (kept.rows().iter())
                .map(|r| r["n"].as_i64().unwrap())
                .collect()
        };
        kept.read("c", &conn).unwrap();
        assert_eq!(ns(&kept), [1, 2]);
        // The last blob gone and another one in its rowid.
        conn.execute_batch(
            "DELETE FROM blobs WHERE id = 'b';
             INSERT INTO blobs VALUES ('c', CAST('{\"role\":\"assistant\",\"n\":3}' AS BLOB));",
        )
        .unwrap();
        kept.read("c", &conn).unwrap();
        assert_eq!(ns(&kept), [1, 3]);
    }
}
