//! What a chat cost, from its transcript: messages, tool calls and tokens,
//! by model. Claude says a message's usage on each of its lines, so they
//! count once by `message.id`; Codex's turn ends with its count; Cursor
//! says no tokens.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::AgentKind;
use crate::feature::Stage;

/// What one message, or many, cost.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub messages: u64,
    pub tool_calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_create: u64,
    pub cache_read: u64,
}

impl Usage {
    /// Add `other` to it.
    pub fn add(&mut self, other: &Usage) {
        self.messages += other.messages;
        self.tool_calls += other.tool_calls;
        self.input += other.input;
        self.output += other.output;
        self.cache_create += other.cache_create;
        self.cache_read += other.cache_read;
    }
}

/// One message of a transcript.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// When, in milliseconds since the epoch; 0 when the transcript doesn't
    /// say (Cursor's).
    pub at_ms: u64,
    pub model: String,
    /// It was a subagent's.
    pub sidechain: bool,
    pub usage: Usage,
}

/// The messages of transcript `raw` of `agent`. Lines that aren't what
/// it writes are passed over. Cursor's `raw` is its `store.db`.
pub fn read(agent: AgentKind, raw: &[u8]) -> Vec<Message> {
    match agent {
        AgentKind::Claude => claude(raw),
        AgentKind::Codex => codex(raw),
        AgentKind::Cursor => cursor(raw),
        AgentKind::OpenCode | AgentKind::T3 | AgentKind::Unknown => Vec::new(),
    }
}

/// The JSON lines of `raw`, each with its number.
fn json_lines(raw: &[u8]) -> impl Iterator<Item = (usize, Value)> + '_ {
    (raw.split(|b| *b == b'\n').enumerate())
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|(i, line)| Some((i, serde_json::from_slice(line).ok()?)))
}

fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Claude Code's transcript: a line per content block of an assistant
/// message, each repeating the message's usage.
fn claude(raw: &[u8]) -> Vec<Message> {
    struct Seen {
        message: Message,
        tools: HashSet<String>,
        unnamed_tools: u64,
    }
    let mut order = Vec::new();
    let mut seen: HashMap<String, Seen> = HashMap::new();
    for (i, line) in json_lines(raw) {
        if line["type"] != "assistant" {
            continue;
        }
        let msg = &line["message"];
        let model = msg["model"].as_str().unwrap_or_default();
        if model == "<synthetic>" {
            continue;
        }
        let key = (msg["id"].as_str().or(line["requestId"].as_str()))
            .map_or_else(|| format!("line {i}"), str::to_owned);
        let entry = seen.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Seen {
                message: Message {
                    at_ms: line["timestamp"].as_str().and_then(iso_ms).unwrap_or(0),
                    model: model.to_owned(),
                    sidechain: line["isSidechain"].as_bool().unwrap_or(false),
                    usage: Usage {
                        messages: 1,
                        ..Usage::default()
                    },
                },
                tools: HashSet::new(),
                unnamed_tools: 0,
            }
        });
        // The same on every line of a message, but for output that may grow
        // as it streams: the most said counts.
        let usage = &msg["usage"];
        let u = &mut entry.message.usage;
        u.input = u.input.max(num(&usage["input_tokens"]));
        u.output = u.output.max(num(&usage["output_tokens"]));
        u.cache_create = (u.cache_create).max(num(&usage["cache_creation_input_tokens"]));
        u.cache_read = u.cache_read.max(num(&usage["cache_read_input_tokens"]));
        let blocks = msg["content"].as_array().into_iter().flatten();
        for block in blocks.filter(|b| b["type"] == "tool_use") {
            match block["id"].as_str() {
                Some(id) => {
                    entry.tools.insert(id.to_owned());
                }
                None => entry.unnamed_tools += 1,
            }
        }
    }
    (order.into_iter())
        .filter_map(|key| seen.remove(&key))
        .map(|mut s| {
            s.message.usage.tool_calls = s.tools.len() as u64 + s.unnamed_tools;
            s.message
        })
        .collect()
}

/// Running totals of a Codex chat, as its `token_count` events say them.
#[derive(Clone, Copy, Default)]
struct Totals {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
}

impl Totals {
    fn of(info: &Value) -> Self {
        let t = &info["total_token_usage"];
        Self {
            input: num(&t["input_tokens"]),
            cached: num(&t["cached_input_tokens"]),
            cache_write: num(&t["cache_write_input_tokens"]),
            output: num(&t["output_tokens"]),
        }
    }
}

/// A Codex turn being read: when it began, its last totals, its tool
/// calls.
#[derive(Default)]
struct Turn {
    at_ms: u64,
    last: Option<Totals>,
    tools: u64,
}

/// Codex's rollout: each turn's `token_count` events hold the chat's
/// running totals, so a turn costs its last total less the turn before's.
/// A turn is one message, at its start.
fn codex(raw: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    let mut model = String::new();
    let mut before = Totals::default();
    let mut turn: Option<Turn> = None;
    let close = |turn: Option<Turn>, before: &mut Totals, model: &str, out: &mut Vec<Message>| {
        let Some(Turn {
            at_ms,
            last: Some(last),
            tools,
        }) = turn
        else {
            return;
        };
        let less = |a: u64, b: u64| a.saturating_sub(b);
        let cached = less(last.cached, before.cached);
        out.push(Message {
            at_ms,
            model: model.to_owned(),
            sidechain: false,
            usage: Usage {
                messages: 1,
                tool_calls: tools,
                // Codex counts cached input within its input.
                input: less(less(last.input, before.input), cached),
                output: less(last.output, before.output),
                cache_create: less(last.cache_write, before.cache_write),
                cache_read: cached,
            },
        });
        *before = last;
    };
    for (_, line) in json_lines(raw) {
        let at_ms = line["timestamp"].as_str().and_then(iso_ms).unwrap_or(0);
        let begun = || Turn {
            at_ms,
            ..Turn::default()
        };
        let payload = &line["payload"];
        let kind = payload["type"].as_str().unwrap_or_default();
        match (line["type"].as_str().unwrap_or_default(), kind) {
            ("turn_context", _) => {
                if let Some(m) = payload["model"].as_str() {
                    model = m.to_owned();
                }
            }
            ("event_msg", "task_started") => {
                close(turn.take(), &mut before, &model, &mut out);
                turn = Some(begun());
            }
            ("event_msg", "token_count") if payload["info"].is_object() => {
                turn.get_or_insert_with(begun).last = Some(Totals::of(&payload["info"]));
            }
            ("event_msg", "task_complete") => close(turn.take(), &mut before, &model, &mut out),
            ("response_item", k) if k.ends_with("_call") => {
                turn.get_or_insert_with(begun).tools += 1;
            }
            _ => {}
        }
    }
    close(turn, &mut before, &model, &mut out);
    out
}

/// Cursor CLI's chat database: its messages are JSON blobs with a role.
/// It says no tokens and no times. The kept copy is read in memory, so
/// the chat never lands in a temp file other users can read.
fn cursor(raw: &[u8]) -> Vec<Message> {
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

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    (hex.as_bytes().chunks(2))
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// `2026-11-03T10:28:06.709Z` (or with `+02:00`) in milliseconds since the
/// epoch.
pub fn iso_ms(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut ymd = date.split('-');
    let year: i64 = ymd.next()?.parse().ok()?;
    let month: i64 = ymd.next()?.parse().ok()?;
    let day: i64 = ymd.next()?.parse().ok()?;
    let (clock, offset) = match time.find(['Z', 'z', '+', '-']) {
        Some(at) => time.split_at(at),
        None => (time, ""),
    };
    let mut hms = clock.split(':');
    let hour: i64 = hms.next()?.parse().ok()?;
    let minute: i64 = hms.next()?.parse().ok()?;
    let secs: f64 = hms.next().unwrap_or("0").parse().ok()?;
    let offset_mins = match offset.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let (h, m) = offset[1..].split_once(':').unwrap_or((&offset[1..], "0"));
            let mins = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
            if *sign == b'+' { mins } else { -mins }
        }
        _ => 0,
    };
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let whole = days * 86_400 + hour * 3600 + minute * 60 - offset_mins * 60;
    let ms = whole * 1000 + (secs * 1000.0).round() as i64;
    u64::try_from(ms).ok()
}

/// `messages` by run and model: each goes to the latest of `runs` (stage,
/// n, when it began in ms) that began before it.
pub fn split(
    messages: &[Message],
    runs: &[(Stage, u32, u64)],
) -> BTreeMap<(Stage, u32, String), Usage> {
    let starts: Vec<u64> = runs.iter().map(|r| r.2).collect();
    let mut out: BTreeMap<(Stage, u32, String), Usage> = BTreeMap::new();
    for ((i, model, _), usage) in split_at(messages, &starts) {
        let (stage, n, _) = runs[i];
        out.entry((stage, n, model)).or_default().add(&usage);
    }
    out
}

/// `messages` by the run they go to (its index in `starts`, when each run
/// began in ms), by model and by whether a subagent sent them. One from
/// before every run, or with no time, goes to the first run.
pub fn split_at(messages: &[Message], starts: &[u64]) -> BTreeMap<(usize, String, bool), Usage> {
    let mut out: BTreeMap<(usize, String, bool), Usage> = BTreeMap::new();
    let first = (0..starts.len()).min_by_key(|i| (starts[*i], *i));
    for m in messages {
        let latest = (0..starts.len())
            .filter(|i| starts[*i] <= m.at_ms)
            .max_by_key(|i| (starts[*i], *i));
        let Some(i) = latest.or(first) else {
            continue;
        };
        (out.entry((i, m.model.clone(), m.sidechain)).or_default()).add(&m.usage);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_line(id: &str, at: &str, tool_id: Option<&str>) -> String {
        let content = match tool_id {
            Some(t) => format!(r#"[{{"type":"tool_use","id":"{t}","name":"Read"}}]"#),
            None => r#"[{"type":"text","text":"hi"}]"#.to_owned(),
        };
        format!(
            r#"{{"type":"assistant","requestId":"req_{id}","timestamp":"{at}","isSidechain":false,"message":{{"id":"msg_{id}","model":"claude-opus-5-5","usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":50}},"content":{content}}}}}"#
        )
    }

    #[test]
    fn claude_usage_counts_once_per_message() {
        let raw = [
            r#"{"type":"user","message":{"content":"go"}}"#.to_owned(),
            claude_line("a", "2026-11-03T10:28:06.709Z", None),
            claude_line("a", "2026-11-03T10:28:06.718Z", Some("t1")),
            claude_line("a", "2026-11-03T10:28:06.720Z", Some("t2")),
            claude_line("b", "2026-11-03T10:28:10.000Z", Some("t3")),
            r#"{"type":"assistant","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":9}}}"#.to_owned(),
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-11-03T10:28:11Z","message":{"model":"claude-haiku","usage":{"output_tokens":7}}}"#.to_owned(),
            "not json".to_owned(),
        ]
        .join("\n");
        let got = read(AgentKind::Claude, raw.as_bytes());
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].at_ms, 1_793_701_686_709);
        assert_eq!(
            got[0].usage,
            Usage {
                messages: 1,
                tool_calls: 2,
                input: 2,
                output: 50,
                cache_create: 100,
                cache_read: 1000,
            }
        );
        assert_eq!(got[1].usage.tool_calls, 1);
        assert!(got[2].sidechain);
        assert_eq!(got[2].model, "claude-haiku");
        assert_eq!(got[2].usage.output, 7);
        let total = got.iter().fold(Usage::default(), |mut t, m| {
            t.add(&m.usage);
            t
        });
        assert_eq!(total.input, 4);
        assert_eq!(total.output, 107);
    }

    #[test]
    fn codex_turns_cost_their_last_total_less_the_last_turns() {
        let tc = |at: &str, input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output}}}}}}}}}"#
            )
        };
        let raw = [
            r#"{"timestamp":"2026-08-26T11:00:10.570Z","type":"session_meta","payload":{"id":"s"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:10.571Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:10.965Z","type":"turn_context","payload":{"model":"gpt-5.6-luna"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:00:15.478Z","type":"response_item","payload":{"type":"custom_tool_call"}}"#.to_owned(),
            tc("2026-08-26T11:00:15.576Z", 100, 40, 10),
            r#"{"timestamp":"2026-08-26T11:00:16Z","type":"response_item","payload":{"type":"function_call"}}"#.to_owned(),
            tc("2026-08-26T11:00:22.306Z", 300, 200, 30),
            r#"{"timestamp":"2026-08-26T11:00:29Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_owned(),
            r#"{"timestamp":"2026-08-26T11:02:34Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_owned(),
            tc("2026-08-26T11:02:51Z", 500, 300, 45),
            r#"{"timestamp":"2026-08-26T11:03:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#.to_owned(),
        ]
        .join("\n");
        let got = read(AgentKind::Codex, raw.as_bytes());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].model, "gpt-5.6-luna");
        assert_eq!(got[0].at_ms, iso_ms("2026-08-26T11:00:10.571Z").unwrap());
        assert_eq!(
            got[0].usage,
            Usage {
                messages: 1,
                tool_calls: 2,
                input: 100,
                output: 30,
                cache_create: 0,
                cache_read: 200,
            }
        );
        assert_eq!(
            got[1].usage,
            Usage {
                messages: 1,
                tool_calls: 0,
                input: 100,
                output: 15,
                cache_create: 0,
                cache_read: 100,
            }
        );
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

    #[test]
    fn iso_times_read_with_their_offset() {
        assert_eq!(iso_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(iso_ms("1970-01-01T02:00:01.5+02:00"), Some(1500));
        assert_eq!(iso_ms("2026-11-03T10:28:06Z"), Some(1_793_701_686_000));
        assert_eq!(iso_ms("garbage"), None);
    }

    #[test]
    fn messages_go_to_the_latest_run_begun_before_them() {
        let m = |at_ms: u64, model: &str, output: u64| Message {
            at_ms,
            model: model.into(),
            sidechain: false,
            usage: Usage {
                messages: 1,
                output,
                ..Usage::default()
            },
        };
        let messages = [
            m(500, "a", 1),
            m(1000, "a", 2),
            m(1500, "a", 4),
            m(2000, "b", 8),
            m(3500, "a", 16),
        ];
        let runs = [
            (Stage::Build, 1, 1000),
            (Stage::Build, 2, 3000),
            (Stage::Plan, 1, 2000),
        ];
        let got = split(&messages, &runs);
        let out = |s, n, model: &str| got.get(&(s, n, model.to_owned())).map(|u| u.output);
        // Before every run: the first; at its start: that run.
        assert_eq!(out(Stage::Build, 1, "a"), Some(7));
        assert_eq!(out(Stage::Plan, 1, "b"), Some(8));
        assert_eq!(out(Stage::Build, 2, "a"), Some(16));
        assert_eq!(got.len(), 3);
        assert!(split(&messages, &[]).is_empty());
    }
}
