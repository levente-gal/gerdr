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

    /// In words: "context 84k / 1M", with ", nearly full" when it is.
    pub fn said(&self) -> String {
        // "84k", "1M": a whole figure needs no ".0".
        let short = |n| crate::stats_panel::tokens(n).replace(".0", "");
        let mut said = match self.window {
            Some(window) => format!("context {} / {}", short(self.used), short(window)),
            None => format!("context {}", short(self.used)),
        };
        if self.nearly_full() {
            said.push_str(", nearly full");
        }
        said
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
#[cfg(test)]
pub fn context(agent: AgentKind, tail: &[u8]) -> Option<Context> {
    crate::integrations::of(agent).context_in(tail)
}

/// The JSON lines of `tail`, last first; a line cut where the tail starts
/// isn't JSON: it is passed over.
pub(crate) fn lines_back(tail: &[u8]) -> impl Iterator<Item = Value> + '_ {
    (tail.split(|b| *b == b'\n').rev())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
}

pub(crate) fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Claude Code's, as `context` reads it.
pub(crate) fn claude_context(tail: &[u8]) -> Option<Context> {
    lines_back(tail).find_map(|v| {
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
    })
}

/// pi's, as `context` reads it: each assistant message says what its
/// request sent; pi names no window.
pub(crate) fn pi_context(tail: &[u8]) -> Option<Context> {
    lines_back(tail).find_map(|v| {
        let m = &v["message"];
        if v["type"] != "message" || m["role"] != "assistant" {
            return None;
        }
        let u = &m["usage"];
        let used = num(&u["input"]) + num(&u["cacheRead"]) + num(&u["cacheWrite"]);
        (used > 0).then_some(Context { used, window: None })
    })
}

/// The last `TAIL` bytes of the file at `path`.
pub(crate) fn tail_of(path: &Path) -> Option<Vec<u8>> {
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
            if !crate::integrations::of(chat.agent).caps().context {
                return None;
            }
            let claimed = thread::claimed(dir, &chat);
            let chat = thread::with_known_id(dir, &chat);
            let found = locate::find(&homes, &chat, &claimed).ok()??;
            Some((*stage, (run.n, read(found.agent, &found.main)?)))
        })
        .collect()
}

/// The context of `agent`'s chat kept at `path`: its transcript, or a
/// Cursor chat's `store.db`. Reads the file.
pub fn read(agent: AgentKind, path: &Path) -> Option<Context> {
    crate::integrations::of(agent).context(path)
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
        // pi's: its last assistant message's, cached input counted in.
        let pi = |input: u64| {
            format!(
                r#"{{"type":"message","message":{{"role":"assistant","usage":{{"input":{input},"output":9,"cacheRead":100,"cacheWrite":10}}}}}}"#
            )
        };
        let tail = format!("{}\n{}\n", pi(1), pi(5));
        assert_eq!(
            context(AgentKind::Pi, tail.as_bytes()),
            Some(Context {
                used: 115,
                window: None
            })
        );
    }
}
