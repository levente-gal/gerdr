//! What a Cursor CLI chat says of itself, read out of its `store.db`: how
//! full its context is, what it cost, its messages; and the model its
//! config says every chat runs on.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::ai_work::{
    context::Context,
    usage::{Message, Usage, from_hex},
};

/// The context of the Cursor chat whose `store.db` is `db`, as its latest
/// root blob says it: the one its `meta` names, a protobuf message whose
/// field 5 holds the tokens used (1) and the window (2), what Cursor's own
/// `/context` shows. Cursor doesn't document it: a root that doesn't read
/// so (another layout, or encrypted) says nothing.
pub(crate) fn cursor_context(db: &Path) -> Option<Context> {
    let conn = crate::cursor_agents::open_read_only(db).ok()?;
    let one = |sql: &str, key: &str| conn.query_row(sql, [key], |r| r.get::<_, Vec<u8>>(0)).ok();
    let hex = one("SELECT CAST(value AS BLOB) FROM meta WHERE key = ?1", "0")?;
    let meta = from_hex(std::str::from_utf8(&hex).ok()?)?;
    let meta: Value = serde_json::from_slice(&meta).ok()?;
    let root = one(
        "SELECT data FROM blobs WHERE id = ?1",
        meta["latestRootBlobId"].as_str()?,
    )?;
    cursor_root_context(&root)
}

/// The context a Cursor root blob `root` holds in its field 5.
fn cursor_root_context(root: &[u8]) -> Option<Context> {
    let usage = (proto_fields(root)?.into_iter()).find_map(|(n, field)| match (n, field) {
        (5, Proto::Bytes(b)) => proto_fields(b),
        _ => None,
    })?;
    let int = |want: u64| {
        usage.iter().find_map(|(n, field)| match field {
            Proto::Int(v) if *n == want => Some(*v),
            _ => None,
        })
    };
    let used = int(1).filter(|used| *used > 0)?;
    Some(Context {
        used,
        window: int(2).filter(|w| *w > 0),
    })
}

/// A protobuf field's value: a varint, the bytes of a length-delimited one,
/// or a fixed-width one, left unread.
enum Proto<'a> {
    Int(u64),
    Bytes(&'a [u8]),
    Fixed,
}

/// The fields of protobuf message `b`, by number; None where `b` isn't one.
fn proto_fields(mut b: &[u8]) -> Option<Vec<(u64, Proto<'_>)>> {
    fn varint(b: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&x, rest) = b.split_first()?;
            *b = rest;
            v |= u64::from(x & 0x7f) << shift;
            if x < 0x80 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let key = varint(&mut b)?;
        let value = match key & 7 {
            0 => Proto::Int(varint(&mut b)?),
            2 => {
                let len = usize::try_from(varint(&mut b)?).ok()?;
                let (bytes, rest) = b.split_at_checked(len)?;
                b = rest;
                Proto::Bytes(bytes)
            }
            wire @ (1 | 5) => {
                b = b.get(if wire == 1 { 8 } else { 4 }..)?;
                Proto::Fixed
            }
            _ => return None,
        };
        out.push((key >> 3, value));
    }
    Some(out)
}

/// Cursor CLI's chat database: its messages are JSON blobs with a role.
/// It says no tokens and no times. The kept copy is read in memory, so
/// the chat never lands in a temp file other users can read.
pub(crate) fn cursor(raw: &[u8]) -> Vec<Message> {
    cursor_conn(raw)
        .and_then(|conn| cursor_rows(&conn))
        .unwrap_or_else(|err| {
            tracing::warn!("ai work: reading a Cursor chat: {err}");
            Vec::new()
        })
}

/// A read-only in-memory database of `raw`, a whole SQLite file.
fn cursor_conn(raw: &[u8]) -> rusqlite::Result<rusqlite::Connection> {
    let mut bytes = raw.to_vec();
    // An in-memory database can't keep a WAL: bytes 18 and 19 say
    // "rollback journal" so SQLite reads the pages as they are.
    if bytes.len() >= 20 && bytes.starts_with(b"SQLite format 3\0") {
        bytes[18] = 1;
        bytes[19] = 1;
    }
    let mut conn = rusqlite::Connection::open_in_memory()?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, bytes.as_slice(), bytes.len(), true)?;
    Ok(conn)
}

/// The messages of Cursor chat database `raw`, as its blobs hold them
/// (a role and a content each), in the order they were written. Blobs
/// that are no message are left out.
pub fn cursor_messages(raw: &[u8]) -> Result<Vec<Value>, String> {
    (cursor_conn(raw).and_then(|conn| cursor_messages_in(&conn))).map_err(|err| err.to_string())
}

/// The messages of the Cursor chat database open on `conn`, as
/// `cursor_messages` gives them. A live `store.db` opened as a file is read
/// with its WAL, which a copy of its bytes leaves out.
pub fn cursor_messages_in(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT data FROM blobs ORDER BY rowid")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let Ok(data) = row.get::<_, Vec<u8>>(0) else {
            continue;
        };
        if let Ok(blob) = serde_json::from_slice::<Value>(&data)
            && blob["role"].is_string()
        {
            out.push(blob);
        }
    }
    Ok(out)
}

fn cursor_rows(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Message>> {
    let meta = conn.query_row("SELECT value FROM meta WHERE key = '0'", [], |r| {
        r.get::<_, String>(0)
    });
    // A chat that never answered names no model.
    let model = (meta.ok())
        .and_then(|hex| from_hex(&hex))
        .and_then(|meta| serde_json::from_slice::<Value>(&meta).ok())
        .and_then(|meta| meta["lastUsedModel"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut stmt = conn.prepare("SELECT data FROM blobs")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        // Some blobs are encrypted, or aren't messages at all.
        let Ok(data) = row.get::<_, Vec<u8>>(0) else {
            continue;
        };
        let Ok(blob) = serde_json::from_slice::<Value>(&data) else {
            continue;
        };
        if blob["role"] != "assistant" {
            continue;
        }
        let parts = blob["content"].as_array().into_iter().flatten();
        let tools = parts.filter(|p| p["type"] == "tool-call").count() as u64;
        out.push(Message {
            at_ms: 0,
            model: model.clone(),
            sidechain: false,
            usage: Usage {
                messages: 1,
                tool_calls: tools,
                ..Usage::default()
            },
        });
    }
    Ok(out)
}

/// The file the Cursor CLI keeps its model in: its `/model` makes the model
/// picked every chat's default, so it is what a chat runs on next.
fn cursor_config() -> Option<PathBuf> {
    let dir = match std::env::var_os("CURSOR_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => crate::ai_work::locate::Homes::here()?.cursor,
    };
    Some(dir.join("cli-config.json"))
}

// shortcut: Cursor's model is its config's, which every chat shares; a chat
// started on `--model` runs on that one meanwhile. Read its footer once
// probed, if it says.
/// The model Cursor's chats run on, by its name. A config it can't read
/// says nothing, read again with the chat's next items: the button then
/// says "Model" and its picker still works.
pub fn cursor_now() -> Option<String> {
    cursor_model_in(&std::fs::read(cursor_config()?).ok()?)
}

/// The model Cursor's config `bytes` names.
fn cursor_model_in(bytes: &[u8]) -> Option<String> {
    let config: Value = serde_json::from_slice(bytes).ok()?;
    let model = &config["model"];
    let name = (model["displayName"].as_str()).or(model["modelId"].as_str())?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_work::{AgentKind, usage::read};

    /// Protobuf field `n` as bytes: its key, its length, then `body`.
    fn field(n: u8, body: &[u8]) -> Vec<u8> {
        [&[n << 3 | 2, body.len() as u8][..], body].concat()
    }

    /// A root blob as a Cursor chat's: message ids, then the context (112766
    /// of 256000, with a part's breakdown), then a folder.
    fn cursor_root() -> Vec<u8> {
        let part = field(1, b"tools");
        let usage = [
            &[0x08, 0xfe, 0xf0, 0x06, 0x10, 0x80, 0xd0, 0x0f][..],
            &field(3, &part),
        ]
        .concat();
        [
            field(1, &[7; 32]),
            field(1, &[8; 32]),
            field(5, &usage),
            field(9, b"file:///x"),
        ]
        .concat()
    }

    #[test]
    fn a_cursor_root_blob_says_its_context() {
        let used = Context {
            used: 112_766,
            window: Some(256_000),
        };
        assert_eq!(cursor_root_context(&cursor_root()), Some(used));
        // Cut short, encrypted, or with no field 5: nothing.
        let root = cursor_root();
        assert_eq!(cursor_root_context(&root[..root.len() - 3]), None);
        assert_eq!(cursor_root_context(&[0xff, 0xff, 0xff]), None);
        assert_eq!(cursor_root_context(&field(1, &[7; 32])), None);
    }

    #[test]
    fn a_cursor_chat_database_reads_its_latest_root() {
        let dir = std::env::temp_dir().join(format!("gerdr-context-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("store.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let meta = r#"{"latestRootBlobId":"r2","lastUsedModel":"default"}"#;
        let hex: String = meta.bytes().map(|b| format!("{b:02x}")).collect();
        conn.execute("INSERT INTO meta VALUES ('0', ?1)", [hex])
            .unwrap();
        let old = field(5, &[0x08, 0x05, 0x10, 0x0a]);
        for (id, data) in [("r1", old), ("r2", cursor_root())] {
            conn.execute(
                "INSERT INTO blobs VALUES (?1, ?2)",
                rusqlite::params![id, data],
            )
            .unwrap();
        }
        let got = cursor_context(&db).map(|c| c.used);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Some(112_766));
    }

    #[test]
    fn cursor_chats_count_their_answers() {
        cursor_chat_counts("rollback", "DELETE");
    }

    /// A chat copied in WAL mode reads in memory too.
    #[test]
    fn cursor_chats_in_wal_mode_read_in_memory() {
        cursor_chat_counts("wal", "WAL");
    }

    fn cursor_chat_counts(name: &str, journal: &str) {
        let dir =
            std::env::temp_dir().join(format!("gerdr-usage-cursor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("store.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let _: String = conn
            .query_row(&format!("PRAGMA journal_mode = {journal}"), [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let meta = r#"{"lastUsedModel":"grok-4.7"}"#;
        let hex: String = meta.bytes().map(|b| format!("{b:02x}")).collect();
        conn.execute("INSERT INTO meta VALUES ('0', ?1)", [hex])
            .unwrap();
        let blobs = [
            r#"{"role":"user","content":"go"}"#,
            r#"{"role":"assistant","content":[{"type":"tool-call"},{"type":"tool-call"}]}"#,
            r#"{"role":"assistant","content":[{"type":"text"}]}"#,
            "\u{1}encrypted",
        ];
        for (i, b) in blobs.iter().enumerate() {
            conn.execute(
                "INSERT INTO blobs VALUES (?1, ?2)",
                rusqlite::params![i.to_string(), b.as_bytes()],
            )
            .unwrap();
        }
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(conn);
        let raw = std::fs::read(&db).unwrap();
        assert_eq!(raw[18], if journal == "WAL" { 2 } else { 1 });
        let got = read(AgentKind::Cursor, &raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].model, "grok-4.7");
        assert_eq!(got.iter().map(|m| m.usage.tool_calls).sum::<u64>(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live chat's messages still in its WAL are read from the file,
    /// and missed in a copy of its bytes.
    #[test]
    fn a_live_cursor_store_is_read_through_its_wal() {
        let dir = std::env::temp_dir().join(format!("gerdr-usage-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("store.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);",
            )
            .unwrap();
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        for (i, blob) in [r#"{"role":"user","content":"go"}"#, "\u{1}x"]
            .iter()
            .enumerate()
        {
            writer
                .execute(
                    "INSERT INTO blobs VALUES (?1, ?2)",
                    rusqlite::params![i.to_string(), blob.as_bytes()],
                )
                .unwrap();
        }
        // The writer stays open, as Cursor's does: the rows are in the WAL.
        let reader = crate::cursor_agents::open_read_only(&db).unwrap();
        let got = cursor_messages_in(&reader).unwrap();
        assert_eq!(got, [serde_json::json!({"role": "user", "content": "go"})]);
        let copied = cursor_messages(&std::fs::read(&db).unwrap()).unwrap();
        assert!(copied.is_empty(), "{copied:?}");
        drop((reader, writer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursors_config_names_its_model() {
        let config = br#"{"model":{"modelId":"default","displayName":"Auto"},"selectedModel":{"modelId":"default"}}"#;
        assert_eq!(cursor_model_in(config).as_deref(), Some("Auto"));
        assert_eq!(
            cursor_model_in(br#"{"model":{"modelId":"gpt-5.2"}}"#).as_deref(),
            Some("gpt-5.2")
        );
        assert_eq!(cursor_model_in(b"{}"), None);
    }
}
