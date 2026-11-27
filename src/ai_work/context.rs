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

/// `n` tokens, short: "84k", "1M"; a whole figure needs no ".0".
pub fn short(n: u64) -> String {
    crate::stats_panel::tokens(n).replace(".0", "")
}

impl Context {
    /// Whether it is close to full: the agent soon compacts or gets lost.
    pub fn nearly_full(&self) -> bool {
        self.window.is_some_and(|w| self.used * 5 >= w * 4)
    }

    /// In words: "context 84k / 1M", with ", nearly full" when it is.
    pub fn said(&self) -> String {
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

/// The context of `agent`'s chat whose transcript ends with `tail`. A line
/// cut where the tail starts isn't JSON: it is passed over.
#[cfg(test)]
pub fn context(agent: AgentKind, tail: &[u8]) -> Option<Context> {
    crate::integrations::of(agent).context_in(tail)
}

pub(crate) fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
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
