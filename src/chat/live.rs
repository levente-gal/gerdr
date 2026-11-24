//! Live word from the agents in herdr panes (see
//! `/home/lgal/reports/gerdr-agent-live-plan.md`): each agent keeps running
//! as its TUI in its pane, and its own hooks (Claude, Cursor's CLI) or a
//! plugin in its TUI (OpenCode) tell gerdr what happens as it happens: the
//! chat it holds, the tool running, a turn's end, a permission it asks for.
//! The transcript stays what the chat shows (`tail`); this adds what it
//! can't say yet, and answers asks from the chat.
//!
//! The way in is a stream socket per herdr session (`socket_for`), one JSON
//! line a `LiveEvent`. The connection that sent an `Ask` waits on it for one
//! `Answer` line.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::herdr::AgentSession;

pub mod claude;
pub mod opencode;
mod serve;

/// How long an ask waits for an answer from the chat before the agent's TUI
/// asks itself.
pub const ASK_WAIT: Duration = Duration::from_secs(60);
/// How long a hook waits on gerdr for anything but an ask's answer.
const SEND_WAIT: Duration = Duration::from_millis(500);
/// The most of one line gerdr reads from a connection.
const MAX_LINE: usize = 1 << 20;
/// Who reports the sessions heard here, as herdr's integrations name
/// themselves (`herdr:opencode`).
pub const SOURCE: &str = "gerdr:live";

/// One thing an agent in pane `pane` says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveEvent {
    /// herdr's pane id (`HERDR_PANE_ID`).
    pub pane: String,
    /// The agent, as herdr names it (`claude`, `cursor`, `opencode`).
    pub agent: String,
    /// When, in ms since the epoch; gerdr's own clock if it is ahead.
    #[serde(default)]
    pub at_ms: i64,
    pub what: What,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum What {
    /// The chat the pane's agent holds: `kind` `id` or `path`, as herdr's
    /// `agent_session`.
    Session {
        kind: String,
        value: String,
    },
    /// A tool started: its call id, its name, its one line.
    ToolStart {
        call: String,
        name: String,
        gist: String,
    },
    ToolEnd {
        call: String,
        failed: bool,
    },
    /// The agent's turn ended: nothing runs.
    TurnEnd,
    /// The agent asks to run `tool`; the connection waits for an `Answer`.
    Ask {
        ask: String,
        tool: String,
        gist: String,
        input: String,
    },
    /// An ask was answered elsewhere (in the TUI) or withdrawn.
    AskGone {
        ask: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskReply {
    Allow,
    Deny,
    /// No answer from the chat: the agent's TUI asks as it always does.
    Pass,
}

/// gerdr's answer to an ask, one line on its connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ask: String,
    pub reply: AskReply,
}

/// A tool an agent runs now, as its hooks said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub call: String,
    pub name: String,
    pub gist: String,
    pub since_ms: i64,
}

/// An ask waiting for its answer from the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAsk {
    pub ask: String,
    pub tool: String,
    pub gist: String,
    pub input: String,
    pub since: Instant,
}

/// What a pane's agent said live, for its chat (`PaneChat::live`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneLive {
    pub running: Vec<Running>,
    pub ask: Option<PendingAsk>,
}

impl PaneLive {
    /// Take in `what`, but sessions and asks (`Gerdr::live_event` keeps
    /// those). Whether anything changed.
    pub fn hear(&mut self, what: &What, at_ms: i64) -> bool {
        match what {
            What::ToolStart { call, name, gist } => {
                self.running.retain(|r| r.call != *call);
                self.running.push(Running {
                    call: call.clone(),
                    name: name.clone(),
                    gist: gist.clone(),
                    since_ms: at_ms,
                });
                true
            }
            What::ToolEnd { call, .. } => {
                let had = self.running.len();
                self.running.retain(|r| r.call != *call);
                had != self.running.len()
            }
            What::TurnEnd => !std::mem::take(&mut self.running).is_empty(),
            What::Session { .. } | What::Ask { .. } | What::AskGone { .. } => false,
        }
    }
}

/// An ask waiting on the connection it came by.
pub struct HeldAsk {
    pub pane: String,
    pub conn: u64,
    pub stream: UnixStream,
    /// The running call it asks for, as its tool and line match: once that
    /// call ends, the TUI's own prompt answered it (`settled_by`).
    pub call: Option<String>,
}

/// Whether `what`, heard from the pane of ask `held`, says the ask was
/// answered in the agent's own prompt: Claude shows its prompt beside the
/// hook's wait, and doesn't end the hook when the user answers there.
pub fn settled_by(held: &HeldAsk, what: &What) -> bool {
    match what {
        What::ToolEnd { call, .. } => held.call.as_ref() == Some(call),
        What::TurnEnd => true,
        _ => false,
    }
}

/// gerdr's side: what it heard, and the asks waiting on their connections.
#[derive(Default)]
pub struct Hub {
    /// The socket gerdr listens on, for the herdr session it follows.
    pub socket: Option<PathBuf>,
    /// The sessions heard, by pane: laid over herdr's `agent_session` when
    /// herdr names none (see `Gerdr::chats_sync`).
    pub sessions: HashMap<String, AgentSession>,
    /// The asks waiting, by ask id.
    pub asks: HashMap<String, HeldAsk>,
    /// The id the next connection gets.
    pub next_conn: u64,
    /// The files gerdr changed to be told (`install`), this run: the hooks
    /// are added once a run, the first time a chat shows.
    pub installed: bool,
}

/// The socket of the gerdr following the herdr session at `herdr_socket`:
/// `$XDG_RUNTIME_DIR/gerdr-live-<16 hex of sha256(path)>.sock`. OpenCode's
/// plugin (`assets/opencode/gerdr-live/tui.js`) names it the same way.
pub fn socket_for(herdr_socket: &Path) -> Option<PathBuf> {
    let hash = Sha256::digest(herdr_socket.as_os_str().as_encoded_bytes());
    let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    Some(crate::herdr::runtime_dir()?.join(format!("gerdr-live-{hex}.sock")))
}

/// For a hook program: gerdr's socket for the herdr session it runs under,
/// and its pane; `None` outside herdr.
pub fn from_env() -> Option<(PathBuf, String)> {
    let herdr = std::env::var_os("HERDR_SOCKET_PATH")?;
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty())?;
    Some((socket_for(Path::new(&herdr))?, pane))
}

/// Tell gerdr at `socket` `event`. For an ask, its answer, waited for up
/// to `ASK_WAIT` and a little: `None` when no gerdr listens or it said
/// nothing. Never fails: a hook must not get in its agent's way.
pub fn send(socket: &Path, event: &LiveEvent) -> Option<AskReply> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let asks = matches!(event.what, What::Ask { .. });
    let _ = stream.set_write_timeout(Some(SEND_WAIT));
    let mut line = serde_json::to_vec(event).ok()?;
    line.push(b'\n');
    stream.write_all(&line).ok()?;
    if !asks {
        return None;
    }
    let _ = stream.set_read_timeout(Some(ASK_WAIT + Duration::from_secs(5)));
    let mut answer = String::new();
    BufReader::new(stream.take(MAX_LINE as u64))
        .read_line(&mut answer)
        .ok()?;
    Some(serde_json::from_str::<Answer>(&answer).ok()?.reply)
}

/// Listen at `path`, made anew, without blocking.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// The whole lines in `buf`, taken out of it, and whether the connection
/// sent too long a line (then it is dropped).
pub fn take_lines(buf: &mut Vec<u8>) -> (Vec<LiveEvent>, bool) {
    let mut events = Vec::new();
    while let Some(end) = buf.iter().position(|b| *b == b'\n') {
        let line: Vec<u8> = buf.drain(..=end).collect();
        match serde_json::from_slice::<LiveEvent>(&line) {
            Ok(event) => events.push(event),
            Err(err) if line.iter().any(|b| !b.is_ascii_whitespace()) => {
                tracing::debug!("agent live: a line gerdr can't read: {err}");
            }
            Err(_) => {}
        }
    }
    (events, buf.len() > MAX_LINE)
}

/// Write `answer` on `stream`, one line; a hook gone meanwhile is fine.
pub fn answer(stream: &mut UnixStream, answer: &Answer) {
    if let Ok(mut line) = serde_json::to_vec(answer) {
        line.push(b'\n');
        let _ = stream.set_write_timeout(Some(SEND_WAIT));
        if let Err(err) = stream.write_all(&line) {
            tracing::debug!("agent live: answering {}: {err}", answer.ask);
        }
    }
}

/// A user's file gerdr changed, with what it held before (`None`: it
/// wasn't there), for Undo (checklist D1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub path: PathBuf,
    pub before: Option<String>,
}

/// Write `merge(what path holds)` to `path`, unless it holds that already:
/// what changed, if anything.
pub fn install(
    path: &Path,
    merge: impl FnOnce(Option<&str>) -> Result<String, String>,
) -> Result<Option<Changed>, String> {
    let before = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let merged = merge(before.as_deref()).map_err(|err| format!("{}: {err}", path.display()))?;
    if before.as_deref() == Some(merged.as_str()) {
        return Ok(None);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    crate::projects::write_atomic(path, merged.as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Some(Changed {
        path: path.to_owned(),
        before,
    }))
}

/// `text`, a JSON object, with its top-level `key` set to `value`. Only
/// that value's text changes: serde_json here sorts keys, and the user's
/// files keep their order and spacing.
pub(super) fn with_key(text: &str, key: &str, value: &serde_json::Value) -> Result<String, String> {
    let root: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if !root.is_object() {
        return Err("not a JSON object".into());
    }
    let pretty = |indent: &str| {
        let text = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
        Ok::<_, String>(text.replace('\n', &format!("\n{indent}")))
    };
    let b = text.as_bytes();
    // The object's members: where each value starts and ends, by key.
    let mut i = skip_ws(b, 0) + 1;
    let (mut found, mut last_end, mut indent) = (None, None, None::<String>);
    loop {
        i = skip_ws(b, i);
        if b.get(i) != Some(&b'"') {
            break;
        }
        let key_end = value_end(b, i);
        let name: String = serde_json::from_slice(&b[i..key_end]).map_err(|err| err.to_string())?;
        let line = text[..i].rfind('\n').map_or(0, |n| n + 1);
        let own = &text[line..i];
        let own = own.chars().all(char::is_whitespace).then(|| own.to_owned());
        let start = skip_ws(b, skip_ws(b, key_end) + 1);
        let end = value_end(b, start);
        if name == key {
            found = Some((start, end, own.clone().unwrap_or_default()));
        }
        indent = indent.or(own);
        last_end = Some(end);
        i = skip_ws(b, end);
        if b.get(i) == Some(&b',') {
            i += 1;
        }
    }
    let quoted = serde_json::to_string(key).map_err(|err| err.to_string())?;
    match (found, last_end) {
        (Some((start, end, own)), _) => Ok(format!(
            "{}{}{}",
            &text[..start],
            pretty(&own)?,
            &text[end..]
        )),
        (None, Some(end)) => {
            let indent = indent.unwrap_or_else(|| "  ".into());
            let member = format!(",\n{indent}{quoted}: {}", pretty(&indent)?);
            Ok(format!("{}{member}{}", &text[..end], &text[end..]))
        }
        (None, None) => serde_json::to_string_pretty(&serde_json::json!({ key: value }))
            .map(|text| text + "\n")
            .map_err(|err| err.to_string()),
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Where the JSON value starting at `i` of valid JSON `b` ends.
fn value_end(b: &[u8], mut i: usize) -> usize {
    let (mut depth, mut in_string) = (0usize, false);
    while let Some(&c) = b.get(i) {
        i += 1;
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' if depth == 0 => return i,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth <= 1 => return if depth == 0 { i - 1 } else { i },
            b'}' | b']' => depth -= 1,
            b',' if depth == 0 => return i - 1,
            c if depth == 0 && c.is_ascii_whitespace() => return i - 1,
            _ => {}
        }
    }
    i
}

/// Put back what `changed` held before gerdr changed it.
pub fn undo(changed: &Changed) -> Result<(), String> {
    let path = &changed.path;
    match &changed.before {
        Some(text) => crate::projects::write_atomic(path, text.as_bytes()),
        None => std::fs::remove_file(path),
    }
    .map_err(|err| format!("{}: {err}", path.display()))
}

/// The command a hook runs this gerdr with: `<exe> <subcommand>`.
pub fn hook_command(subcommand: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    Ok(format!("{} {subcommand}", crate::desktop::shell_quote(exe)))
}

/// Now, in ms since the epoch.
pub fn now_ms() -> i64 {
    crate::cursor_agents::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A folder of its own under the system's temporary one, removed when
    /// dropped; short, for socket paths.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gl-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn event(what: What) -> LiveEvent {
        LiveEvent {
            pane: "w1:p2".into(),
            agent: "claude".into(),
            at_ms: 5,
            what,
        }
    }

    #[test]
    fn events_go_as_one_line_each_and_come_back() {
        let a = event(What::ToolStart {
            call: "c1".into(),
            name: "Bash".into(),
            gist: "ls".into(),
        });
        let b = event(What::TurnEnd);
        let mut buf = Vec::new();
        for e in [&a, &b] {
            buf.extend(serde_json::to_vec(e).unwrap());
            buf.push(b'\n');
        }
        buf.extend(b"{\"pane\":");
        let (events, too_long) = take_lines(&mut buf);
        assert_eq!(events, vec![a, b]);
        assert!(!too_long);
        assert_eq!(buf, b"{\"pane\":");
    }

    #[test]
    fn the_wire_names_are_plain() {
        let line = serde_json::to_string(&event(What::AskGone { ask: "x".into() })).unwrap();
        assert!(
            line.contains(r#""what":{"type":"ask_gone","ask":"x"}"#),
            "{line}"
        );
        let answer: Answer = serde_json::from_str(r#"{"ask":"x","reply":"allow"}"#).unwrap();
        assert_eq!(answer.reply, AskReply::Allow);
    }

    #[test]
    fn running_tools_come_and_go() {
        let mut live = PaneLive::default();
        let start = |call: &str| What::ToolStart {
            call: call.into(),
            name: "Bash".into(),
            gist: "ls".into(),
        };
        assert!(live.hear(&start("a"), 1));
        assert!(live.hear(&start("b"), 2));
        let end = What::ToolEnd {
            call: "a".into(),
            failed: false,
        };
        assert!(live.hear(&end, 3));
        assert!(!live.hear(&end, 3));
        assert_eq!(live.running.len(), 1);
        assert!(live.hear(&What::TurnEnd, 4));
        assert!(live.running.is_empty());
    }

    #[test]
    fn an_ask_waits_for_its_answer_on_the_connection() {
        let dir = TempDir::new();
        let path = dir.path().join("live.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let event: LiveEvent = serde_json::from_str(&line).unwrap();
            let What::Ask { ask, .. } = event.what else {
                panic!("not an ask: {line}");
            };
            answer(
                &mut stream,
                &Answer {
                    ask,
                    reply: AskReply::Deny,
                },
            );
        });
        let ask = event(What::Ask {
            ask: "q1".into(),
            tool: "Bash".into(),
            gist: "rm x".into(),
            input: "{}".into(),
        });
        assert_eq!(send(&path, &ask), Some(AskReply::Deny));
        server.join().unwrap();
    }

    #[test]
    fn no_gerdr_no_wait() {
        let dir = TempDir::new();
        let ask = event(What::TurnEnd);
        assert_eq!(send(&dir.path().join("none.sock"), &ask), None);
    }

    #[test]
    fn a_key_set_leaves_the_rest_of_the_text_as_it_was() {
        let text = "{\n  \"z\": {\"b\": 1, \"a\": [1, \"}\"]},\n  \"hooks\": {\"x\": 1},\n  \"m\": \"\\\"\"\n}\n";
        let value = serde_json::json!({ "y": [2] });
        assert_eq!(
            with_key(text, "hooks", &value).unwrap(),
            "{\n  \"z\": {\"b\": 1, \"a\": [1, \"}\"]},\n  \"hooks\": {\n    \"y\": [\n      2\n    ]\n  },\n  \"m\": \"\\\"\"\n}\n"
        );
        let added = with_key("{\n  \"z\": true\n}", "p", &serde_json::json!(["a"])).unwrap();
        assert_eq!(added, "{\n  \"z\": true,\n  \"p\": [\n    \"a\"\n  ]\n}");
        assert_eq!(
            with_key(" {} ", "p", &serde_json::json!(1)).unwrap(),
            "{\n  \"p\": 1\n}\n"
        );
        assert!(with_key("[]", "p", &serde_json::json!(1)).is_err());
        assert!(with_key("{oops", "p", &serde_json::json!(1)).is_err());
    }

    #[test]
    fn the_socket_is_named_as_the_opencode_plugin_names_it() {
        // node: createHash("sha256").update("/tmp/herdr-test.sock").digest("hex").slice(0, 16)
        let Some(path) = socket_for(Path::new("/tmp/herdr-test.sock")) else {
            return; // No XDG_RUNTIME_DIR here.
        };
        assert_eq!(
            path.file_name().unwrap(),
            "gerdr-live-4f2d902ac55aa4a1.sock"
        );
    }

    #[test]
    fn an_ask_is_settled_by_its_calls_end_or_the_turns() {
        let held = HeldAsk {
            pane: "w1:p2".into(),
            conn: 0,
            stream: UnixStream::pair().unwrap().0,
            call: Some("c1".into()),
        };
        let end = |call: &str| What::ToolEnd {
            call: call.into(),
            failed: false,
        };
        assert!(settled_by(&held, &end("c1")));
        // Another call, run beside it, ends: still waiting.
        assert!(!settled_by(&held, &end("c2")));
        assert!(settled_by(&held, &What::TurnEnd));
        let start = What::ToolStart {
            call: "c3".into(),
            name: "Read".into(),
            gist: "x".into(),
        };
        assert!(!settled_by(&held, &start));
    }

    #[test]
    fn install_changes_once_and_undo_puts_it_back() {
        let dir = TempDir::new();
        let path = dir.path().join("a/settings.json");
        let changed = install(&path, |_| Ok("{}".into())).unwrap().unwrap();
        assert_eq!(changed.before, None);
        assert_eq!(install(&path, |t| Ok(t.unwrap().into())).unwrap(), None);
        undo(&changed).unwrap();
        assert!(!path.exists());
    }
}
