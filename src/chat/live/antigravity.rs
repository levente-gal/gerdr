//! The Antigravity CLI's hooks, for live word (see `super`): `gerdr
//! agy-hook` reads a hook's input and tells gerdr; `merged` adds it to
//! `~/.gemini/config/hooks.json` under a name of its own.
//!
//! No asks: Antigravity has no hook for the permissions it asks for, only
//! `PreToolUse` on every call, and holding every call for the chat would
//! ask where Antigravity doesn't.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde_json::{Value, json};

use super::{LiveEvent, What};
use crate::stats_panel::tool_gist;

/// The subcommand Antigravity's hooks run.
pub const HOOK_COMMAND: &str = "agy-hook";
/// The agent, as herdr names it.
const AGENT: &str = "agy";
/// gerdr's hook's name in `hooks.json`, beside herdr's (`"herdr"`).
const NAME: &str = "gerdr";
/// The most of a hook's input read.
const MAX_INPUT: u64 = 4 << 20;
/// The most of a transcript's end read for a call's step.
const MAX_TAIL: u64 = 1 << 20;
/// A tool's arguments that say its gist, first found first (as the chat's
/// `parse::antigravity` says it).
const GIST_KEYS: [&str; 8] = [
    "CommandLine",
    "AbsolutePath",
    "Path",
    "TargetFile",
    "DirectoryPath",
    "SearchPath",
    "Query",
    "Url",
];

/// `gerdr agy-hook`: read the hook's input from stdin and tell gerdr.
/// Prints `{}` (Antigravity wants an object, and gerdr changes nothing),
/// and never fails the hook.
pub fn run_hook() {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    println!("{{}}");
    let Some((socket, pane)) = super::from_env() else {
        return;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&input) else {
        return;
    };
    let Some(what) = what_of(&payload, read_tail) else {
        return;
    };
    let event = LiveEvent {
        pane,
        agent: AGENT.into(),
        at_ms: super::now_ms(),
        what,
    };
    super::send(&socket, &event);
}

/// What hook input `payload` says; `None` for what gerdr doesn't hear.
/// `tail` reads the end of the transcript at a path, to name a call as
/// the chat does.
fn what_of(payload: &Value, tail: impl Fn(&Path) -> Option<String>) -> Option<What> {
    let text = |key: &str| payload[key].as_str().filter(|t| !t.is_empty());
    let call = &payload["toolCall"];
    let id = || {
        let lines = tail(Path::new(text("transcriptPath")?))?;
        let name = call["name"].as_str()?;
        step_call(&lines, payload["stepIdx"].as_u64()?, name, &call["args"])
    };
    // The chat's id when the transcript says it, else one of the hook's own:
    // the call then shows as running beside the chat's row till it ends.
    let id = || {
        id().unwrap_or_else(|| match call["id"].as_str() {
            Some(own) => format!("agy-{own}"),
            None => format!("agy-step{}", payload["stepIdx"]),
        })
    };
    Some(match text("hookEventName")? {
        "PreInvocation" => What::Session {
            kind: "path".into(),
            value: text("transcriptPath")?.to_owned(),
        },
        "PreToolUse" => What::ToolStart {
            call: id(),
            name: call["name"].as_str().unwrap_or("tool").to_owned(),
            gist: (GIST_KEYS.iter())
                .find_map(|k| call["args"].get(*k).and_then(tool_gist))
                .unwrap_or_default(),
        },
        "PostToolUse" => What::ToolEnd {
            call: id(),
            failed: text("error").is_some(),
        },
        "Stop" => What::TurnEnd,
        _ => return None,
    })
}

/// The chat's id (`step<n>.<k>`, see `parse::antigravity`) of the call
/// named `name` with `args` that runs as step `step`, by transcript
/// `lines`: call `k` of the last model turn at or before `step`. Its calls
/// run as the steps after it, in order; one of the same name and
/// arguments is taken if the step doesn't say.
fn step_call(lines: &str, step: u64, name: &str, args: &Value) -> Option<String> {
    let turn = (lines.lines().rev())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| {
            line["type"] == "PLANNER_RESPONSE"
                && line["step_index"].as_u64().is_some_and(|at| at <= step)
        })?;
    let at = turn["step_index"].as_u64()?;
    let calls = turn["tool_calls"].as_array()?;
    let named = |k: &usize| calls.get(*k).is_some_and(|c| c["name"] == name);
    let by_step = (step.checked_sub(at + 1)).and_then(|k| usize::try_from(k).ok());
    let k = (by_step.filter(named))
        .or_else(|| (0..calls.len()).find(|k| named(k) && calls[*k]["args"] == *args))
        .or_else(|| (0..calls.len()).find(named))?;
    Some(format!("step{at}.{k}"))
}

/// The last `MAX_TAIL` bytes of the whole transcript of the one at `path`
/// (herdr's hook names the cut one; the chat reads the whole one beside
/// it), from a line's start.
fn read_tail(path: &Path) -> Option<String> {
    let full = path.with_file_name("transcript_full.jsonl");
    let mut file = std::fs::File::open(&full)
        .or_else(|_| std::fs::File::open(path))
        .ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(MAX_TAIL)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = match len > MAX_TAIL {
        true => text.split_once('\n').map_or("", |(_, rest)| rest),
        false => &text,
    };
    Some(text.to_owned())
}

/// `existing`, the text of Antigravity's `hooks.json`, with gerdr's hook
/// (running `command`) under its own name, in place of any older gerdr's;
/// all else kept as it is. Text that isn't a JSON object is an error.
pub fn merged(existing: Option<&str>, command: &str) -> Result<String, String> {
    let handler = json!({ "type": "command", "command": command, "timeout": 5 });
    let group = json!([{ "matcher": "*", "hooks": [handler.clone()] }]);
    let ours = json!({
        "PreInvocation": [handler.clone()],
        "PreToolUse": group.clone(),
        "PostToolUse": group,
        "Stop": [handler],
    });
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else {
        return serde_json::to_string_pretty(&json!({ NAME: ours }))
            .map(|text| text + "\n")
            .map_err(|err| err.to_string());
    };
    let root: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let object = root.as_object().ok_or("not a JSON object")?;
    if object.get(NAME) == Some(&ours) {
        return Ok(text.to_owned());
    }
    super::with_key(text, NAME, &ours)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic transcript: the user's step, a model turn with two
    /// calls, their results.
    const TRANSCRIPT: &str = r#"{"step_index":0,"type":"USER_INPUT","source":"USER_EXPLICIT","content":"hi"}
{"step_index":1,"type":"PLANNER_RESPONSE","source":"MODEL","tool_calls":[{"name":"view_file","args":{"AbsolutePath":"/w/a.rs"}},{"name":"run_command","args":{"CommandLine":"cargo test","Cwd":"/w"}}]}
{"step_index":2,"type":"GENERIC","source":"MODEL","content":"..."}
"#;

    fn payload(event: &str) -> Value {
        json!({
            "conversationId": "c0ffee00-0000-4000-8000-000000000001",
            "workspacePaths": ["/w"],
            "transcriptPath": "/h/.gemini/antigravity-cli/brain/c0ffee00-0000-4000-8000-000000000001/.system_generated/logs/transcript.jsonl",
            "modelName": "auto",
            "hookEventName": event,
            "cwd": "/w",
            "toolCall": {"id": "toolu_01", "name": "run_command", "args": {"CommandLine": "cargo test", "Cwd": "/w"}},
            "stepIdx": 3,
        })
    }

    fn what(payload: &Value) -> Option<What> {
        what_of(payload, |_| Some(TRANSCRIPT.to_owned()))
    }

    #[test]
    fn hooks_say_what_antigravity_does() {
        let session = what(&payload("PreInvocation"));
        let path = payload("x")["transcriptPath"].as_str().unwrap().to_owned();
        let want = What::Session {
            kind: "path".into(),
            value: path,
        };
        assert_eq!(session, Some(want));
        let start = What::ToolStart {
            call: "step1.1".into(),
            name: "run_command".into(),
            gist: "cargo test".into(),
        };
        assert_eq!(what(&payload("PreToolUse")), Some(start));
        let end = |failed| {
            Some(What::ToolEnd {
                call: "step1.1".into(),
                failed,
            })
        };
        assert_eq!(what(&payload("PostToolUse")), end(false));
        let mut failed = payload("PostToolUse");
        failed["error"] = "exit status 1".into();
        assert_eq!(what(&failed), end(true));
        assert_eq!(what(&payload("Stop")), Some(What::TurnEnd));
        assert_eq!(what(&payload("SessionStart")), None);
        // No transcript to say the chat's id: the hook's own.
        let own = what_of(&payload("PreToolUse"), |_| None);
        assert!(matches!(own, Some(What::ToolStart { call, .. }) if call == "agy-toolu_01"));
    }

    #[test]
    fn a_call_is_named_by_its_step_else_its_name_and_arguments() {
        let args = json!({"CommandLine": "cargo test", "Cwd": "/w"});
        let call = |step, name: &str, args: &Value| step_call(TRANSCRIPT, step, name, args);
        assert_eq!(call(2, "view_file", &json!({})), Some("step1.0".into()));
        assert_eq!(call(3, "run_command", &args), Some("step1.1".into()));
        // The step is the model turn's own: the call by name and arguments.
        assert_eq!(call(1, "run_command", &args), Some("step1.1".into()));
        assert_eq!(call(1, "view_file", &json!({})), Some("step1.0".into()));
        assert_eq!(call(3, "grep_search", &args), None);
        assert_eq!(call(0, "run_command", &args), None, "no turn before");
    }

    #[test]
    fn gerdrs_hook_goes_in_beside_the_others() {
        let command = "/opt/gerdr agy-hook";
        let theirs = r#"{
  "herdr": {
    "PreInvocation": [
      {
        "command": "bash 'herdr-agent-state.sh' session",
        "timeout": 10,
        "type": "command"
      }
    ]
  },
  "gerdr": {"Stop": [{"command": "/old/gerdr agy-hook"}]},
  "mine": {"enabled": false}
}
"#;
        let out = merged(Some(theirs), command).unwrap();
        assert!(out.starts_with(&theirs[..theirs.find("  \"gerdr\"").unwrap()]));
        assert!(
            out.ends_with(",\n  \"mine\": {\"enabled\": false}\n}\n"),
            "{out}"
        );
        let root: Value = serde_json::from_str(&out).unwrap();
        let ours = &root["gerdr"];
        assert_eq!(
            ours["Stop"],
            json!([{"type": "command", "command": command, "timeout": 5}])
        );
        assert_eq!(ours["PreToolUse"][0]["matcher"], "*");
        assert_eq!(ours["PostToolUse"][0]["hooks"][0]["command"], command);
        assert_eq!(merged(Some(&out), command).unwrap(), out);
        let fresh = merged(None, command).unwrap();
        assert_eq!(merged(Some(&fresh), command).unwrap(), fresh);
        assert!(merged(Some("{oops"), command).is_err());
        assert!(merged(Some("[]"), command).is_err());
    }
}
