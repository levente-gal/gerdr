//! How full an agent's context is, from the end of its transcript: what its
//! last turn sent the model, and the model's window. Claude Code says it in
//! each answer's usage, Codex in its `token_count` events, Cursor in its
//! chat database's root blob; T3 Code isn't read.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use super::{AgentKind, locate, observe, thread};
use crate::feature::{Feature, Stage};

/// What an agent's last turn sent, and how much its model takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub used: u64,
    /// None where neither the transcript nor the model's name says it.
    pub window: Option<u64>,
}

impl Context {
    /// Whether it is close to full: the agent soon compacts or gets lost.
    pub fn nearly_full(&self) -> bool {
        self.window.is_some_and(|w| self.used * 5 >= w * 4)
    }
}

/// How much of a transcript's end is read: a turn's usage is near its end.
const TAIL: u64 = 256 * 1024;

/// The windows of Claude's models before the 4.6 family; all since take 1M.
const SMALL_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

/// The window of Claude model `model`: 200k for Haiku 4.5 and the models
/// before Opus and Sonnet 4.6, 1M for every one since (and any not known
/// yet, as new ones are likely to keep it).
fn claude_window(model: &str) -> u64 {
    let old = [
        "-4-5", "-4-1", "-4-0", "-4-2025", "-3-", "claude-3", "claude-2",
    ];
    match old.iter().any(|o| model.contains(o)) {
        true => SMALL_WINDOW,
        false => LARGE_WINDOW,
    }
}

/// The context of `agent`'s chat whose transcript ends with `tail`. A line
/// cut where the tail starts isn't JSON: it is passed over.
pub fn context(agent: AgentKind, tail: &[u8]) -> Option<Context> {
    let mut lines = (tail.split(|b| *b == b'\n').rev())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok());
    let num = |v: &Value| v.as_u64().unwrap_or(0);
    match agent {
        AgentKind::Claude => lines.find_map(|v| {
            let msg = &v["message"];
            let model = msg["model"].as_str()?;
            let main = v["type"] == "assistant" && v["isSidechain"] != true;
            if !main || model.is_empty() || model == "<synthetic>" {
                return None;
            }
            let u = &msg["usage"];
            let used = num(&u["input_tokens"])
                + num(&u["cache_creation_input_tokens"])
                + num(&u["cache_read_input_tokens"]);
            // More than the window the name gives: it runs on the large one.
            let window = Some(claude_window(model)).map(|w| match used > w {
                true => LARGE_WINDOW.max(w),
                false => w,
            });
            (used > 0).then_some(Context { used, window })
        }),
        AgentKind::Codex => lines.find_map(|v| {
            let p = &v["payload"];
            let counted = v["type"] == "event_msg" && p["type"] == "token_count";
            let info = &p["info"];
            if !counted || !info.is_object() {
                return None;
            }
            // Its input tokens count the cached ones too.
            let used = num(&info["last_token_usage"]["input_tokens"]);
            let window = info["model_context_window"].as_u64();
            (used > 0).then_some(Context { used, window })
        }),
        AgentKind::Cursor
        | AgentKind::OpenCode
        | AgentKind::T3
        | AgentKind::Pi
        | AgentKind::Antigravity
        | AgentKind::Unknown => None,
    }
}

/// The last `TAIL` bytes of the file at `path`.
fn tail_of(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let from = file.metadata().ok()?.len().saturating_sub(TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(tail)
}

/// How full the context of each of `feature`'s open agents is, by stage,
/// with the run each read was of; `dir` is its data dir, which keeps the
/// chat ids AI work found. Reads files, so not on the UI thread. Runs on
/// another machine and chats that can't be read say nothing.
pub fn read_contexts(feature: &Feature, dir: &Path) -> BTreeMap<Stage, (u32, Context)> {
    let Some(homes) = locate::Homes::here() else {
        return BTreeMap::new();
    };
    (feature.runs.iter())
        .filter(|(_, run)| run.live() && run.machine.is_none())
        .filter_map(|(stage, run)| {
            let chat = observe::chat(feature, *stage, run, run.session());
            if !matches!(
                chat.agent,
                AgentKind::Claude | AgentKind::Codex | AgentKind::Cursor
            ) {
                return None;
            }
            let claimed = thread::claimed(dir, &chat);
            let chat = thread::with_known_id(dir, &chat);
            let found = locate::find(&homes, &chat, &claimed).ok()??;
            let context = match found.agent {
                AgentKind::Cursor => cursor_context(&found.main)?,
                agent => context(agent, &tail_of(&found.main)?)?,
            };
            Some((*stage, (run.n, context)))
        })
        .collect()
}

/// The context of the Cursor chat whose `store.db` is `db`, as its latest
/// root blob says it: the one its `meta` names, a protobuf message whose
/// field 5 holds the tokens used (1) and the window (2), what Cursor's own
/// `/context` shows. Cursor doesn't document it: a root that doesn't read
/// so (another layout, or encrypted) says nothing.
fn cursor_context(db: &Path) -> Option<Context> {
    let conn = crate::cursor_agents::open_read_only(db).ok()?;
    let one = |sql: &str, key: &str| conn.query_row(sql, [key], |r| r.get::<_, Vec<u8>>(0)).ok();
    let hex = one("SELECT CAST(value AS BLOB) FROM meta WHERE key = ?1", "0")?;
    let meta = super::usage::from_hex(std::str::from_utf8(&hex).ok()?)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(model: &str, sidechain: bool, input: u64, created: u64, read: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"isSidechain\":{sidechain},\"message\":{{\"model\":\"{model}\",\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":{created},\"cache_read_input_tokens\":{read},\"output_tokens\":9}}}}}}\n"
        )
    }

    #[test]
    fn a_claude_chat_counts_what_its_last_main_answer_sent() {
        // A line cut where the tail began, then the chat as it ends.
        let tail = [
            "\"usage\":{\"input_tokens\":3}}\n".to_owned(),
            claude("claude-opus-5-5", false, 10, 0, 1_000),
            claude("claude-opus-5-5", false, 2, 500, 83_500),
            claude("claude-haiku-5-5", true, 7, 0, 7),
            claude("<synthetic>", false, 0, 0, 0),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n".to_owned(),
        ]
        .concat();
        let got = context(AgentKind::Claude, tail.as_bytes());
        assert_eq!(
            got,
            Some(Context {
                used: 84_002,
                window: Some(LARGE_WINDOW)
            })
        );
        assert!(!got.unwrap().nearly_full());
        assert_eq!(context(AgentKind::Claude, b""), None);
    }

    #[test]
    fn claude_windows_go_by_model() {
        assert_eq!(claude_window("claude-opus-5-5"), LARGE_WINDOW);
        assert_eq!(claude_window("claude-sonnet-4-6"), LARGE_WINDOW);
        assert_eq!(claude_window("claude-haiku-4-5"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-haiku-4-5-20251001"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-sonnet-4-20250514"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-3-7-sonnet"), SMALL_WINDOW);
        assert_eq!(claude_window("claude-next-9"), LARGE_WINDOW);
        // Over the small window: it runs on the large one.
        let tail = claude("claude-haiku-4-5", false, 0, 0, 250_000);
        let got = context(AgentKind::Claude, tail.as_bytes()).unwrap();
        assert_eq!(got.window, Some(LARGE_WINDOW));
        let tail = claude("claude-haiku-4-5", false, 0, 0, 170_000);
        assert!(
            context(AgentKind::Claude, tail.as_bytes())
                .unwrap()
                .nearly_full()
        );
    }

    #[test]
    fn a_codex_chat_counts_its_last_turn_in_its_window() {
        let event = |input: u64| {
            format!(
                "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":999999}},\"last_token_usage\":{{\"input_tokens\":{input},\"cached_input_tokens\":27392}},\"model_context_window\":258400}}}}}}\n"
            )
        };
        let none =
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":null}}\n";
        let tail = [event(1_000), event(28_351), none.to_owned()].concat();
        assert_eq!(
            context(AgentKind::Codex, tail.as_bytes()),
            Some(Context {
                used: 28_351,
                window: Some(258_400)
            })
        );
        assert_eq!(context(AgentKind::Cursor, tail.as_bytes()), None);
        assert_eq!(context(AgentKind::Codex, b"not json\n"), None);
    }

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
}
