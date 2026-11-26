//! Claude Code's hooks, for live word (see `chat::live`): `gerdr
//! claude-hook` reads a hook's input and tells gerdr; `merged` adds it to
//! `~/.claude/settings.json`.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    chat::live::{self, Answer, AskReply, Changed, QUESTION_WAIT, Question, QuestionOption, What},
    integrations::common::hook,
    stats_panel::tool_gist,
};

/// The tool Claude's plan mode asks to go ahead with its plan by: its input
/// has the plan's markdown (`plan`) and file (`planFilePath`).
pub(super) const PLAN_TOOL: &str = "ExitPlanMode";
/// The subcommand Claude's hooks run.
pub const HOOK_COMMAND: &str = "claude-hook";
/// The agent, as herdr names it.
const AGENT: &str = "claude";
/// The hook events gerdr listens to, each with how long its hook may take,
/// in seconds: an ask waits for the chat's answer (`ask_wait` and a little).
const EVENTS: [(&str, u64); 6] = [
    ("SessionStart", 5),
    ("PreToolUse", 5),
    ("PostToolUse", 5),
    ("PostToolUseFailure", 5),
    ("Stop", 5),
    ("PermissionRequest", QUESTION_WAIT.as_secs() + 30),
];
/// The most of a tool's input an ask shows, in chars.
const MAX_ASK_INPUT: usize = 4000;

/// `gerdr claude-hook`: read the hook's input from stdin and tell gerdr;
/// for `PermissionRequest`, print the chat's answer, if any. Always exits
/// at once without gerdr, and never fails the hook.
pub fn run_hook() {
    let Some(payload) = hook::pane_input() else {
        return;
    };
    let now = live::now_ms();
    let ask = format!("claude-{}-{now}", std::process::id());
    let Some(what) = what_of(&payload, &ask) else {
        return;
    };
    let answer = hook::tell(AGENT, now, what);
    if let Some(out) = answer.and_then(|answer| decision(answer, &payload["tool_input"])) {
        println!("{out}");
    }
}

/// Add gerdr's hooks to Claude Code under home folder `home`, if it is
/// installed.
pub fn install(home: &Path) -> Vec<Result<Option<Changed>, String>> {
    if !home.join(".claude").is_dir() {
        return Vec::new();
    }
    vec![live::hook_command(HOOK_COMMAND).and_then(|command| {
        live::install(&home.join(".claude/settings.json"), |text| {
            merged(text, &command)
        })
    })]
}

/// What hook input `payload` says, an ask named `ask`; `None` for what
/// gerdr doesn't hear.
fn what_of(payload: &Value, ask: &str) -> Option<What> {
    let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
    let input = &payload["tool_input"];
    Some(match payload.get("hook_event_name")?.as_str()? {
        "SessionStart" => What::Session {
            kind: "path".into(),
            value: text("transcript_path")?,
        },
        "PreToolUse" => What::ToolStart {
            call: text("tool_use_id")?,
            name: text("tool_name")?,
            gist: tool_gist(input).unwrap_or_default(),
        },
        "PostToolUse" => What::ToolEnd {
            call: text("tool_use_id")?,
            failed: payload["tool_response"]["is_error"].as_bool() == Some(true),
        },
        "PostToolUseFailure" => What::ToolEnd {
            call: text("tool_use_id")?,
            failed: true,
        },
        "Stop" => What::TurnEnd,
        "PermissionRequest" => {
            let tool = text("tool_name")?;
            // A plan reads as its markdown; other input as its JSON.
            let mut shown = match input["plan"].as_str().filter(|_| tool == PLAN_TOOL) {
                Some(plan) => plan.to_owned(),
                None => serde_json::to_string_pretty(input).unwrap_or_default(),
            };
            if let Some((cut, _)) = shown.char_indices().nth(MAX_ASK_INPUT) {
                shown.truncate(cut);
                shown.push('…');
            }
            What::Ask {
                ask: ask.to_owned(),
                plan: tool == PLAN_TOOL,
                tool,
                gist: tool_gist(input).unwrap_or_default(),
                input: shown,
                questions: questions_of(input),
            }
        }
        _ => return None,
    })
}

/// The questions AskUserQuestion's `input` asks, none for other tools'.
fn questions_of(input: &Value) -> Vec<Question> {
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
    (input["questions"].as_array().into_iter().flatten())
        .filter_map(|q| {
            let options = (q["options"].as_array()?.iter())
                .map(|o| QuestionOption {
                    label: text(o, "label"),
                    description: text(o, "description"),
                })
                .collect();
            Some(Question {
                question: q["question"].as_str()?.to_owned(),
                header: text(q, "header"),
                options,
                multi: q["multiSelect"].as_bool() == Some(true),
            })
        })
        .collect()
}

/// What the hook prints for the chat's `answer` to an ask for a tool run
/// with `input`: nothing for `Pass`, so Claude's TUI asks as it always
/// does. A question's answers go in as the tool's input, its questions
/// kept; any other allow gives the input back as it was, as Claude runs
/// `ExitPlanMode` only with one (a bare allow leaves its menu up).
fn decision(answer: Answer, input: &Value) -> Option<String> {
    let message = (answer.message).unwrap_or_else(|| "Denied from gerdr's chat".into());
    let decision = match (answer.reply, answer.answers) {
        (AskReply::Allow, Some(answers)) => {
            let mut input = input.clone();
            input["answers"] = json!(answers);
            json!({ "behavior": "allow", "updatedInput": input })
        }
        (AskReply::Allow, None) => json!({ "behavior": "allow", "updatedInput": input }),
        (AskReply::Deny, _) => {
            json!({ "behavior": "deny", "message": message })
        }
        (AskReply::Pass, _) => return None,
    };
    let out = json!({
        "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision }
    });
    Some(out.to_string())
}

/// `existing`, the text of `~/.claude/settings.json`, with `command` run
/// for gerdr's events, in place of any older gerdr's, all else kept. Only
/// `"hooks"` is written anew; text that isn't a settings object is an
/// error.
pub fn merged(existing: Option<&str>, command: &str) -> Result<String, String> {
    let text = existing.filter(|t| !t.trim().is_empty());
    let root: Value = match text {
        Some(text) => serde_json::from_str(text).map_err(|err| err.to_string())?,
        None => json!({}),
    };
    let old = (root.as_object().ok_or("not a JSON object")?)
        .get("hooks")
        .cloned();
    let mut hooks = old.clone().unwrap_or_else(|| json!({}));
    let events = hooks.as_object_mut().ok_or("\"hooks\" isn't an object")?;
    for (event, timeout) in EVENTS {
        let groups = (events.entry(event))
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("\"{event}\" isn't a list"))?;
        // A group left empty by taking gerdr's hook out goes too.
        groups.retain_mut(|group| {
            let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let had = list.len();
            list.retain(|hook| {
                !(hook.get("command").and_then(Value::as_str))
                    .is_some_and(|c| crate::cursor_agents::is_gerdr_command(c, HOOK_COMMAND))
            });
            list.len() == had || !list.is_empty()
        });
        let hook = json!({ "type": "command", "command": command, "timeout": timeout });
        let mut group = json!({ "hooks": [hook] });
        if !matches!(event, "SessionStart" | "Stop") {
            group["matcher"] = "*".into();
        }
        groups.push(group);
    }
    match text {
        Some(text) if old.as_ref() == Some(&hooks) => Ok(text.to_owned()),
        Some(text) => live::with_key(text, "hooks", &hooks),
        None => serde_json::to_string_pretty(&json!({ "hooks": hooks }))
            .map(|text| text + "\n")
            .map_err(|err| err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "SessionStart" => include_str!("fixtures/SessionStart.json"),
            "PreToolUse" => include_str!("fixtures/PreToolUse.json"),
            "PostToolUse" => include_str!("fixtures/PostToolUse.json"),
            "PermissionRequest" => include_str!("fixtures/PermissionRequest.json"),
            "PermissionRequestQuestion" => {
                include_str!("fixtures/PermissionRequestQuestion.json")
            }
            "Stop" => include_str!("fixtures/Stop.json"),
            "UserPromptSubmit" => include_str!("fixtures/UserPromptSubmit.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn hooks_say_what_claude_does() {
        let what = |name| what_of(&fixture(name), "q1");
        let call = "toolu_01Vv5bFcWUmhcvCarPCiFz2z";
        let transcript = fixture("SessionStart")["transcript_path"].clone();
        assert_eq!(
            what("SessionStart"),
            Some(What::Session {
                kind: "path".into(),
                value: transcript.as_str().unwrap().into(),
            })
        );
        assert_eq!(
            what("PreToolUse"),
            Some(What::ToolStart {
                call: call.into(),
                name: "Bash".into(),
                gist: "echo done > out.txt".into(),
            })
        );
        let end = |failed| {
            Some(What::ToolEnd {
                call: call.into(),
                failed,
            })
        };
        assert_eq!(what("PostToolUse"), end(false));
        let mut failure = fixture("PostToolUse");
        failure["hook_event_name"] = "PostToolUseFailure".into();
        assert_eq!(what_of(&failure, "q1"), end(true));
        assert_eq!(what("Stop"), Some(What::TurnEnd));
        assert_eq!(what("UserPromptSubmit"), None);
        let Some(What::Ask {
            ask,
            tool,
            gist,
            input,
            questions,
            ..
        }) = what("PermissionRequest")
        else {
            panic!("not an ask");
        };
        assert_eq!((ask.as_str(), tool.as_str()), ("q1", "Bash"));
        assert_eq!(gist, "echo done > out.txt");
        assert!(
            input.starts_with("{\n  \"command\": \"echo done > out.txt\""),
            "{input}"
        );
        assert!(questions.is_empty());
        // Plan mode's plan reads as its markdown.
        let mut plan = fixture("PermissionRequest");
        plan["tool_name"] = "ExitPlanMode".into();
        plan["tool_input"] = serde_json::json!({ "plan": "# Plan\n\n- one" });
        let Some(What::Ask { input, .. }) = what_of(&plan, "q2") else {
            panic!("not an ask");
        };
        assert_eq!(input, "# Plan\n\n- one");
    }

    #[test]
    fn a_question_asks_its_questions_and_its_answers_are_claudes_input() {
        let payload = fixture("PermissionRequestQuestion");
        let Some(What::Ask {
            tool, questions, ..
        }) = what_of(&payload, "q1")
        else {
            panic!("not an ask");
        };
        assert_eq!(tool, "AskUserQuestion");
        let fruits = Question {
            question: "Pick fruits".into(),
            header: "Fruits".into(),
            options: ["Apple", "Banana", "Grape"]
                .map(|f| QuestionOption {
                    label: f.into(),
                    description: format!("{f} fruit"),
                })
                .into(),
            multi: true,
        };
        assert_eq!(questions, vec![fruits]);
        let answers = Answer {
            ask: "q1".into(),
            reply: AskReply::Allow,
            answers: Some([("Pick fruits".into(), "Apple, Grape".into())].into()),
            message: None,
        };
        let input = &payload["tool_input"];
        let out: Value = serde_json::from_str(&decision(answers, input).unwrap()).unwrap();
        let decided = &out["hookSpecificOutput"]["decision"];
        assert_eq!(decided["behavior"], "allow");
        assert_eq!(decided["updatedInput"]["questions"], input["questions"]);
        assert_eq!(
            decided["updatedInput"]["answers"],
            json!({"Pick fruits": "Apple, Grape"})
        );
    }

    #[test]
    fn the_chats_answer_is_claudes_decision() {
        let input = json!({"command": "ls"});
        let reply = |reply| Answer {
            ask: "q1".into(),
            reply,
            answers: None,
            message: None,
        };
        let out: Value =
            serde_json::from_str(&decision(reply(AskReply::Allow), &input).unwrap()).unwrap();
        let allow = json!({"hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": {"behavior": "allow", "updatedInput": {"command": "ls"}}
        }});
        assert_eq!(out, allow);
        let out = decision(reply(AskReply::Deny), &input).unwrap();
        assert!(out.contains(r#""behavior":"deny""#), "{out}");
        assert!(out.contains("Denied from gerdr's chat"), "{out}");
        assert_eq!(decision(reply(AskReply::Pass), &input), None);
        // A plan sent back: the comments are why.
        let back = Answer {
            message: Some("- step 2: use the cache".into()),
            ..reply(AskReply::Deny)
        };
        let out: Value = serde_json::from_str(&decision(back, &input).unwrap()).unwrap();
        let decided = &out["hookSpecificOutput"]["decision"];
        assert_eq!(decided["message"], "- step 2: use the cache");
    }

    #[test]
    fn gerdrs_hooks_go_in_beside_the_others() {
        let command = "/opt/gerdr claude-hook";
        let theirs = r#"{
  "env": {"B": "1", "A": "2"},
  "model": "opus",
  "hooks": {
    "SessionStart": [
      {"matcher": "^(startup|resume)$", "hooks": [{"type": "command", "command": "bash herdr.sh session", "timeout": 10}]}
    ],
    "Stop": [
      {"hooks": [{"type": "command", "command": "'/old/gerdr' claude-hook", "timeout": 5}]},
      {"hooks": [{"type": "command", "command": "notify-send done"}, {"type": "command", "command": "/old/gerdr claude-hook"}]}
    ]
  },
  "theme": "light"
}
"#;
        let out = merged(Some(theirs), command).unwrap();
        // The user's keys keep their order and form.
        let head =
            "{\n  \"env\": {\"B\": \"1\", \"A\": \"2\"},\n  \"model\": \"opus\",\n  \"hooks\": {";
        assert!(out.starts_with(head), "{out}");
        assert!(out.ends_with("},\n  \"theme\": \"light\"\n}\n"), "{out}");
        let root: Value = serde_json::from_str(&out).unwrap();
        let hooks = &root["hooks"];
        let start = &hooks["SessionStart"];
        assert_eq!(start[0]["hooks"][0]["command"], "bash herdr.sh session");
        assert_eq!(start[1]["hooks"][0]["command"], command);
        assert_eq!(start[1].get("matcher"), None);
        // Old gerdr hooks are gone, their group too when it held no other.
        let stop = json!([
            {"hooks": [{"type": "command", "command": "notify-send done"}]},
            {"hooks": [{"type": "command", "command": command, "timeout": 5}]}
        ]);
        assert_eq!(hooks["Stop"], stop);
        // An ask's hook outlives the wait for its answer, a question's too.
        let wait = hooks["PermissionRequest"][0]["hooks"][0]["timeout"].as_u64();
        assert!(Duration::from_secs(wait.unwrap()) > QUESTION_WAIT + Duration::from_secs(5));
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "*");
        assert_eq!(merged(Some(&out), command).unwrap(), out);
        let fresh = merged(None, command).unwrap();
        assert_eq!(merged(Some(&fresh), command).unwrap(), fresh);
        assert!(merged(Some("{oops"), command).is_err());
        assert!(merged(Some("[]"), command).is_err());
        assert!(merged(Some(r#"{"hooks": []}"#), command).is_err());
    }
}
