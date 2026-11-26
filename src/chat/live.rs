//! Live word from the agents in herdr panes (see
//! `/home/lgal/reports/gerdr-agent-live-plan.md`): each agent keeps running
//! as its TUI in its pane, and its own hooks (Claude, Cursor's CLI,
//! Antigravity) or a plugin in its TUI (OpenCode, pi) tell gerdr what
//! happens as it happens: the chat it holds, the tool running, a turn's
//! end, a permission it asks for.
//! The transcript stays what the chat shows (`tail`); this adds what it
//! can't say yet, and answers asks from the chat.
//!
//! The way in is a stream socket per herdr session (`socket_for`), one JSON
//! line a `LiveEvent`. The connection that sent an `Ask` waits on it for one
//! `Answer` line.

use std::{
    collections::{BTreeMap, HashMap},
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Item;
use crate::herdr::AgentSession;

pub(crate) mod serve;

/// How long an ask waits for an answer from the chat before the agent's TUI
/// asks itself.
pub const ASK_WAIT: Duration = Duration::from_secs(60);
/// How long a question (Claude's AskUserQuestion) or a plan waits for its
/// answer from a card: its TUI shows its own menu beside the wait, so it may
/// wait long, and a plan takes reading.
pub const QUESTION_WAIT: Duration = Duration::from_secs(600);
/// How long after a phone last read an agent pane (it reads a shown one
/// every 2 s) its asks still wait on the phone's card.
pub const PHONE_SIGHT: Duration = Duration::from_secs(5);
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
    /// `questions` when it asks the user questions (Claude's
    /// AskUserQuestion): answered with `Answer::answers`. `plan` when it
    /// asks to go ahead with its plan: `input` is the plan's markdown.
    Ask {
        ask: String,
        tool: String,
        gist: String,
        input: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        questions: Vec<Question>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        plan: bool,
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

/// The answers to an ask's questions, by each one's text
/// (`Question::question`): an option's label, several joined by ", ", or
/// the user's own words.
pub type Answers = BTreeMap<String, String>;

/// One question an agent asks the user, as Claude's AskUserQuestion has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    /// Its short name, for its tab.
    #[serde(default)]
    pub header: String,
    pub options: Vec<QuestionOption>,
    /// Several options may be picked.
    #[serde(default)]
    pub multi: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// How long an ask with `questions`, or for a `plan`, waits for its answer
/// from a card.
pub fn ask_wait(plan: bool, questions: &[Question]) -> Duration {
    if questions.is_empty() && !plan {
        ASK_WAIT
    } else {
        QUESTION_WAIT
    }
}

/// gerdr's answer to an ask, one line on its connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ask: String,
    pub reply: AskReply,
    /// For an ask with questions, allowed: what the user answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Answers>,
    /// Denied: what the agent is told why, in place of the plain refusal
    /// (a plan sent back with the user's comments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
    /// What it asks the user, when it is questions, not a permission.
    pub questions: Vec<Question>,
    /// It asks to go ahead with its plan: `input` is the plan's markdown.
    pub plan: bool,
    pub since: Instant,
}

/// What a pane's agent said live, for its chat (`PaneChat::live`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneLive {
    pub running: Vec<Running>,
    pub ask: Option<PendingAsk>,
    /// How long the calls heard start and end took, in ms, by call: for
    /// the tools a chat gives no time of (Cursor's).
    pub took: HashMap<String, u64>,
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
                let Some(at) = self.running.iter().position(|r| r.call == *call) else {
                    return false;
                };
                let since = self.running.remove(at).since_ms;
                self.took
                    .insert(call.clone(), (at_ms - since).max(0) as u64);
                true
            }
            What::TurnEnd => !std::mem::take(&mut self.running).is_empty(),
            What::Session { .. } | What::Ask { .. } | What::AskGone { .. } => false,
        }
    }

    /// `items` with the times heard on the tools whose chat gives none.
    pub fn timed(&self, mut items: Arc<Vec<Item>>) -> Arc<Vec<Item>> {
        fn untimed(items: &[Item], took: &HashMap<String, u64>) -> bool {
            items.iter().any(|item| match item {
                Item::Tool { id, output, .. } | Item::Subagent { id, output, .. }
                    if output.as_ref().is_some_and(|out| out.ms.is_none())
                        && took.contains_key(id) =>
                {
                    true
                }
                Item::Subagent { items, .. } => untimed(items, took),
                _ => false,
            })
        }
        fn fill(items: &mut Arc<Vec<Item>>, took: &HashMap<String, u64>) {
            if !untimed(items, took) {
                return;
            }
            for item in Arc::make_mut(items) {
                let (Item::Tool { id, output, .. } | Item::Subagent { id, output, .. }) = item
                else {
                    continue;
                };
                if let Some(out) = output.as_mut().filter(|out| out.ms.is_none()) {
                    out.ms = took.get(id.as_str()).copied();
                }
                if let Item::Subagent { items, .. } = item {
                    fill(items, took);
                }
            }
        }
        fill(&mut items, &self.took);
        items
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
    /// What it asks, for the phone view (the pane's chat may never have
    /// shown here).
    pub asked: PendingAsk,
}

/// The running call an ask for `tool` running `gist` is for: the oldest of
/// those that match, but those other asks wait for (`claimed`). Claude
/// asks for calls made side by side one after the other, in order.
pub fn asked_call(
    running: &[Running],
    tool: &str,
    gist: &str,
    claimed: &[String],
) -> Option<String> {
    (running.iter())
        .find(|r| r.name == tool && r.gist == gist && !claimed.contains(&r.call))
        .map(|r| r.call.clone())
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
    /// When a phone last showed each agent pane: its asks wait on the
    /// phone's card too, for `PHONE_SIGHT` after.
    pub phone_seen: HashMap<String, Instant>,
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
/// to `ask_wait` and a little: `None` when no gerdr listens or it said
/// nothing. Never fails: a hook must not get in its agent's way.
pub fn send(socket: &Path, event: &LiveEvent) -> Option<Answer> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let wait = match &event.what {
        What::Ask {
            questions, plan, ..
        } => Some(ask_wait(*plan, questions)),
        _ => None,
    };
    let _ = stream.set_write_timeout(Some(SEND_WAIT));
    let mut line = serde_json::to_vec(event).ok()?;
    line.push(b'\n');
    stream.write_all(&line).ok()?;
    let wait = wait?;
    let _ = stream.set_read_timeout(Some(wait + Duration::from_secs(5)));
    let mut answer = String::new();
    BufReader::new(stream.take(MAX_LINE as u64))
        .read_line(&mut answer)
        .ok()?;
    serde_json::from_str::<Answer>(&answer).ok()
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

/// Now, in ms since the epoch.
pub fn now_ms() -> i64 {
    crate::cursor_agents::now_ms()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A folder of its own under the system's temporary one, removed when
    /// dropped; short, for socket paths.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gl-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub(crate) fn path(&self) -> &Path {
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
    fn a_plan_waits_as_long_as_a_question() {
        assert_eq!(ask_wait(false, &[]), ASK_WAIT);
        assert_eq!(ask_wait(true, &[]), QUESTION_WAIT);
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
        assert_eq!(live.took["a"], 2);
        assert!(live.hear(&What::TurnEnd, 4));
        assert!(live.running.is_empty());
    }

    #[test]
    fn tools_untimed_get_the_time_heard() {
        let tool = |id: &str, ms| Item::Tool {
            id: id.into(),
            name: "Shell".into(),
            gist: "ls".into(),
            input: String::new(),
            output: Some(super::super::ToolOut {
                text: String::new(),
                failed: false,
                ms,
            }),
        };
        let mut live = PaneLive::default();
        let items = Arc::new(vec![tool("a", None), tool("b", Some(7))]);
        assert!(
            Arc::ptr_eq(&live.timed(items.clone()), &items),
            "nothing heard"
        );
        live.took = HashMap::from([("a".into(), 1500), ("b".into(), 9)]);
        let sub = Item::Subagent {
            id: "s".into(),
            title: "look".into(),
            items: Arc::new(vec![tool("a", None)]),
            output: None,
        };
        let items = Arc::new(vec![tool("a", None), tool("b", Some(7)), sub]);
        let timed = live.timed(items);
        assert_eq!(timed[0], tool("a", Some(1500)));
        assert_eq!(timed[1], tool("b", Some(7)), "the chat's own time stays");
        let Item::Subagent { items, .. } = &timed[2] else {
            panic!()
        };
        assert_eq!(items[0], tool("a", Some(1500)));
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
                    answers: None,
                    message: None,
                },
            );
        });
        let ask = event(What::Ask {
            ask: "q1".into(),
            tool: "Bash".into(),
            gist: "rm x".into(),
            input: "{}".into(),
            questions: Vec::new(),
            plan: false,
        });
        assert_eq!(send(&path, &ask).map(|a| a.reply), Some(AskReply::Deny));
        server.join().unwrap();
    }

    #[test]
    fn no_gerdr_no_wait() {
        let dir = TempDir::new();
        let ask = event(What::TurnEnd);
        assert_eq!(send(&dir.path().join("none.sock"), &ask), None);
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
    fn an_ask_is_for_the_oldest_matching_call_not_asked_for_yet() {
        let run = |call: &str, gist: &str| Running {
            call: call.into(),
            name: "Bash".into(),
            gist: gist.into(),
            since_ms: 0,
        };
        let running = [run("a", "ls"), run("b", "rm x"), run("c", "rm x")];
        assert_eq!(asked_call(&running, "Bash", "rm x", &[]), Some("b".into()));
        assert_eq!(
            asked_call(&running, "Bash", "rm x", &["b".into()]),
            Some("c".into())
        );
        assert_eq!(
            asked_call(&running, "Bash", "rm x", &["b".into(), "c".into()]),
            None
        );
        assert_eq!(asked_call(&running, "Edit", "ls", &[]), None);
    }

    #[test]
    fn an_ask_is_settled_by_its_calls_end_or_the_turns() {
        let held = HeldAsk {
            pane: "w1:p2".into(),
            conn: 0,
            stream: UnixStream::pair().unwrap().0,
            call: Some("c1".into()),
            asked: PendingAsk {
                ask: "q".into(),
                tool: "Bash".into(),
                gist: "rm x".into(),
                input: String::new(),
                questions: Vec::new(),
                plan: false,
                since: Instant::now(),
            },
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
}
