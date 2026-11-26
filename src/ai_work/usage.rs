//! What a chat cost, from its transcript: messages, tool calls and tokens,
//! by model. Claude says a message's usage on each of its lines, so they
//! count once by `message.id`; Codex's turn ends with its count; Cursor
//! says no tokens.

use std::collections::BTreeMap;

use serde_json::Value;

use super::AgentKind;
#[cfg(test)]
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
    crate::integrations::of(agent).usage(raw)
}

/// The JSON lines of `raw`, each with its number.
pub(crate) fn json_lines(raw: &[u8]) -> impl Iterator<Item = (usize, Value)> + '_ {
    (raw.split(|b| *b == b'\n').enumerate())
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|(i, line)| Some((i, serde_json::from_slice(line).ok()?)))
}

/// The bytes `hex` spells.
pub(crate) fn from_hex(hex: &str) -> Option<Vec<u8>> {
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
#[cfg(test)]
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
    for m in messages {
        let Some(i) = run_of(m, starts) else {
            continue;
        };
        (out.entry((i, m.model.clone(), m.sidechain)).or_default()).add(&m.usage);
    }
    out
}

/// When the last message timed went to each run of `starts`, as
/// `split_at` shares them out, in ms since the epoch: what a run with no
/// end said was heard of it last.
pub fn last_at(messages: &[Message], starts: &[u64]) -> BTreeMap<usize, u64> {
    let mut out: BTreeMap<usize, u64> = BTreeMap::new();
    for m in messages.iter().filter(|m| m.at_ms > 0) {
        if let Some(i) = run_of(m, starts) {
            let last = out.entry(i).or_default();
            *last = (*last).max(m.at_ms);
        }
    }
    out
}

/// Which of the runs begun at `starts` message `m` goes to: the latest
/// begun by then, else the first.
fn run_of(m: &Message, starts: &[u64]) -> Option<usize> {
    let first = (0..starts.len()).min_by_key(|i| (starts[*i], *i));
    let latest = (0..starts.len())
        .filter(|i| starts[*i] <= m.at_ms)
        .max_by_key(|i| (starts[*i], *i));
    latest.or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

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
