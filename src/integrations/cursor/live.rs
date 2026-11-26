//! The Cursor CLI's live hooks: gerdr adds itself to `~/.cursor/hooks.json`,
//! which Cursor's IDE and its CLI both run. Under the CLI, a hook tells the
//! chat of its pane; under the IDE, it hands the event to the gerdr the IDE
//! runs in (`cursor_agents`).

use std::{os::unix::net::UnixDatagram, path::Path};

use serde_json::Value;

use crate::{
    chat::live,
    cursor_agents::{HookEvent, hook_socket, is_cli_agent, is_gerdr_command, live_path, now_ms},
    integrations::common::hook,
};

/// The subcommand Cursor's hooks run.
pub const HOOK_COMMAND: &str = "cursor-hook";

/// Cursor's agent, as herdr names it: a chat's live word is told as its.
const LIVE_AGENT: &str = "cursor";

/// The hook events gerdr listens to.
const EVENTS: [&str; 8] = [
    "sessionStart",
    "beforeSubmitPrompt",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "afterAgentResponse",
    "stop",
    "sessionEnd",
];

/// How long a hook may take, in seconds: it only passes a datagram on.
const HOOK_TIMEOUT: u64 = 2;

/// `gerdr cursor-hook`: read a hook's input and pass what it says to the
/// gerdr Cursor runs in (by its `WAYLAND_DISPLAY`; another Cursor's go
/// nowhere). It answers nothing, so it changes nothing of what the agent
/// does, and always succeeds.
pub fn run_hook() {
    let Some(payload) = hook::input() else {
        return;
    };
    let Some(event) = HookEvent::from_payload(&payload, now_ms()) else {
        return;
    };
    // Cursor's agent CLI runs the same hooks, but herdr shows it already,
    // as the agent of its pane: its word goes to that pane's chat.
    let under_cli = crate::launch::lineage(std::process::id())
        .into_iter()
        .filter_map(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok())
        .any(|cmdline| is_cli_agent(&crate::launch::cmdline_args(&cmdline)));
    if under_cli {
        if let Some(what) = live_what(&payload) {
            hook::tell(LIVE_AGENT, event.at_ms, what);
        }
        return;
    }
    let display = std::env::var_os("WAYLAND_DISPLAY");
    let (Ok(message), Some(path)) = (
        serde_json::to_vec(&event),
        display.as_deref().and_then(hook_socket),
    ) else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    let _ = socket.set_nonblocking(true);
    let _ = socket.send_to(&message, path);
}

/// What hook input `payload` of Cursor's agent CLI says, for the chat of
/// its pane; `None` for what the chat doesn't hear.
fn live_what(payload: &Value) -> Option<live::What> {
    let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
    let call = || text("tool_use_id");
    Some(match payload.get("hook_event_name")?.as_str()? {
        "sessionStart" => live::What::Session {
            kind: "id".into(),
            value: text("conversation_id").or_else(|| text("session_id"))?,
        },
        "preToolUse" => live::What::ToolStart {
            call: call()?,
            name: text("tool_name")?,
            gist: crate::chat::parse::gist(&payload["tool_input"]),
        },
        "postToolUse" => live::What::ToolEnd {
            call: call()?,
            failed: false,
        },
        "postToolUseFailure" => live::What::ToolEnd {
            call: call()?,
            failed: true,
        },
        "stop" | "sessionEnd" => live::What::TurnEnd,
        _ => return None,
    })
}

/// `existing`, the text of a hooks.json, with `command` run for each of
/// gerdr's events in place of any older gerdr's hook (one running
/// `HOOK_COMMAND` from a program called gerdr), keeping all else as it is.
/// No file starts empty. Text that isn't a hooks.json is an error.
pub fn merge_hooks(existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut root: Value = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => serde_json::from_str(text).map_err(|err| err.to_string())?,
        None => serde_json::json!({ "version": 1, "hooks": {} }),
    };
    let object = root.as_object_mut().ok_or("not a JSON object")?;
    object.entry("version").or_insert(Value::from(1));
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or("\"hooks\" isn't an object")?;
    for event in EVENTS {
        let list = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("\"{event}\" isn't a list"))?;
        list.retain(|hook| {
            !hook
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|c| is_gerdr_command(c, HOOK_COMMAND))
        });
        list.push(serde_json::json!({ "command": command, "timeout": HOOK_TIMEOUT }));
    }
    serde_json::to_string_pretty(&root)
        .map(|text| text + "\n")
        .map_err(|err| err.to_string())
}

/// Add this gerdr to Cursor's hooks, in `~/.cursor/hooks.json`, unless it
/// is there already.
pub fn install_hooks(home: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let command = format!(
        "{} {HOOK_COMMAND}",
        crate::desktop::shell_quote(live_path(&exe.to_string_lossy()))
    );
    let path = home.join(".cursor/hooks.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let merged = merge_hooks(existing.as_deref(), &command)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(());
    }
    crate::projects::write_atomic(&path, merged.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gerdrs_hooks_join_the_users() {
        let command = "/opt/gerdr/gerdr cursor-hook";
        let fresh: Value = serde_json::from_str(&merge_hooks(None, command).unwrap()).unwrap();
        assert_eq!(fresh["version"], 1);
        assert_eq!(fresh["hooks"]["stop"][0]["command"], command);
        let theirs = r#"{"version":1,"hooks":{"stop":[{"command":"notify-send done"},
            {"command":"'/old/gerdr' cursor-hook"},{"command":"'/new/gerdr (deleted)' cursor-hook"}],
            "afterFileEdit":[{"command":"fmt"}]},"x":2}"#;
        let merged = merge_hooks(Some(theirs), command).unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        let stop = value["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["command"], "notify-send done");
        assert_eq!(stop[1]["command"], command);
        assert_eq!(value["hooks"]["afterFileEdit"][0]["command"], "fmt");
        assert_eq!(value["x"], 2);
        // Merging again changes nothing.
        assert_eq!(merge_hooks(Some(&merged), command).unwrap(), merged);
        assert!(merge_hooks(Some("{oops"), command).is_err());
        assert!(merge_hooks(Some("[]"), command).is_err());
    }

    fn cli_fixture(name: &str) -> Value {
        let text = match name {
            "sessionStart" => include_str!("fixtures/sessionStart.json"),
            "preToolUse" => include_str!("fixtures/preToolUse.json"),
            "postToolUse" => include_str!("fixtures/postToolUse.json"),
            "sessionEnd" => include_str!("fixtures/sessionEnd.json"),
            "afterAgentThought" => include_str!("fixtures/afterAgentThought.json"),
            "beforeShellExecution" => {
                include_str!("fixtures/beforeShellExecution.json")
            }
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_clis_hooks_say_what_its_chat_does() {
        let chat = "9db0b999-6676-492c-9967-b1429261dfe8";
        let call = "76a33794-2f13-4600-8bcc-c9434423bb48";
        let what = |name| live_what(&cli_fixture(name));
        assert_eq!(
            what("sessionStart"),
            Some(live::What::Session {
                kind: "id".into(),
                value: chat.into(),
            })
        );
        assert_eq!(
            what("preToolUse"),
            Some(live::What::ToolStart {
                call: call.into(),
                name: "Shell".into(),
                gist: "echo done > out.txt".into(),
            })
        );
        let end = |failed| {
            Some(live::What::ToolEnd {
                call: call.into(),
                failed,
            })
        };
        assert_eq!(what("postToolUse"), end(false));
        let mut failure = cli_fixture("postToolUse");
        failure["hook_event_name"] = "postToolUseFailure".into();
        assert_eq!(live_what(&failure), end(true));
        assert_eq!(what("sessionEnd"), Some(live::What::TurnEnd));
        let mut stop = cli_fixture("sessionEnd");
        stop["hook_event_name"] = "stop".into();
        assert_eq!(live_what(&stop), Some(live::What::TurnEnd));
        assert_eq!(what("afterAgentThought"), None);
        assert_eq!(what("beforeShellExecution"), None);
    }

    #[test]
    fn a_session_from_the_cli_finds_its_chat_for_sure() {
        let dir = std::env::temp_dir().join(format!("gerdr-cursorlive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(live::What::Session { kind, value }) = live_what(&cli_fixture("sessionStart"))
        else {
            panic!("no session");
        };
        let store = dir.join(format!(".cursor/chats/f00/{value}/store.db"));
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, "").unwrap();
        // As `Gerdr::live_event` keeps it, laid over herdr's naming none.
        let mut agent = crate::chat::source::tests::agent(LIVE_AGENT, "id", "x");
        agent.agent_session = Some(crate::herdr::AgentSession {
            source: live::SOURCE.into(),
            agent: LIVE_AGENT.into(),
            kind,
            value: value.clone(),
        });
        let homes = crate::ai_work::locate::Homes::under(&dir);
        let source = crate::chat::source::find(&agent, &homes).expect("found");
        assert_eq!(
            (source.id, source.path, source.guessed),
            (value, store, false)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
