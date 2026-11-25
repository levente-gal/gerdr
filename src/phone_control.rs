//! The phone view's way into a running gerdr (see `phone`): a socket of
//! this gerdr's, `$XDG_RUNTIME_DIR/gerdr/phone-<pid>.sock`, taking one
//! request a connection, a line of JSON, answered with a line of JSON once
//! the loop did it (see `features::phone_page`).
//!
//! The runtime folder is the user's own, so only their programs reach it.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};
use smithay::reexports::calloop::{
    channel,
    timer::{TimeoutAction, Timer},
};

use crate::{
    Gerdr,
    features::PhoneAct,
    host_idle,
    web_push::{Note, Pusher},
};

/// The most a request may take.
const REQUEST_MAX: u64 = 64 * 1024;
/// How long a request waits for the loop to answer it.
const ANSWER_TIME: Duration = Duration::from_secs(20);
/// How long a new feature's request waits: its worktrees are made and
/// origin is asked for its bases first.
pub const CREATE_TIME: Duration = Duration::from_secs(120);
/// A reminder goes to the phones only if nothing with its tag went for
/// this long.
const REMIND_PHONE: Duration = Duration::from_secs(15 * 60);

/// How often held notes look whether the user has gone (see `push_phone`).
const HOLD_LOOK: Duration = Duration::from_secs(5);

/// The folder of the sockets: `$XDG_RUNTIME_DIR/gerdr`.
pub fn dir() -> Option<PathBuf> {
    Some(crate::herdr::runtime_dir()?.join("gerdr"))
}

/// Whether `name` is a phone socket's file name.
pub fn is_socket_name(name: &str) -> bool {
    name.starts_with("phone-") && name.ends_with(".sock")
}

/// A request, as the phone view sends it.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// What the phone may do to a feature, and the page of its `stage`
    /// (by its place in `Stage::ALL`), else the one the desktop shows.
    Offer {
        project: String,
        slug: String,
        #[serde(default)]
        stage: Option<usize>,
    },
    /// How features stand, as their sidebar cards show it: `[project, slug]`
    /// pairs.
    Cards { features: Vec<(String, String)> },
    /// Press button `id` of a stage's page (see `features::PhoneAct`),
    /// with the text it asks for, if it asks.
    Press {
        project: String,
        slug: String,
        #[serde(default)]
        stage: Option<usize>,
        id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        confirm: bool,
    },
    /// Send the stage's file back to its agent.
    SendBack {
        project: String,
        slug: String,
        text: String,
        #[serde(default)]
        confirm: bool,
    },
    /// What a new feature of each project may start on: Discover's agents.
    NewOffer { projects: Vec<String> },
    /// The phone shows agent pane `pane` of `project`: the permission its
    /// agent asks for, if it waits on one (see `chat::live`). Its asks wait
    /// on the phone's card while it shows the pane.
    Ask { project: String, pane: String },
    /// Send `text` to agent pane `pane` as the chat would: a side question
    /// (`/btw …`) is followed for its answer, which the `ask` op then
    /// lists.
    Side {
        project: String,
        pane: String,
        text: String,
    },
    /// Answer ask `ask` of agent pane `pane`: allow or deny it, and for
    /// one with questions, allow it with what the user answered.
    Answer {
        project: String,
        pane: String,
        ask: String,
        reply: crate::chat::live::AskReply,
        #[serde(default)]
        answers: Option<crate::chat::live::Answers>,
    },
    /// Start a feature, and its Discover agent: answered once both are.
    Create {
        project: String,
        title: String,
        /// The repositories it changes, by name: a root of several only.
        #[serde(default)]
        repos: Vec<String>,
        /// What the user wants of it, for Discover's agent.
        #[serde(default)]
        idea: String,
        /// The agent to run Discover on; none: the one it gets anyway.
        #[serde(default)]
        agent: Option<String>,
    },
}

/// A request and where its answer goes.
pub struct Call {
    request: Request,
    answer: mpsc::Sender<Value>,
}

/// Listen on `path`, each connection read on a thread of its own, its
/// request passed to the loop.
fn listen(path: &std::path::Path) -> std::io::Result<channel::Channel<Call>> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    let (tx, rx) = channel::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::warn!("phone socket: {err}");
                    continue;
                }
            };
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, &tx));
        }
    });
    Ok(rx)
}

fn serve(stream: UnixStream, tx: &channel::Sender<Call>) {
    let mut line = String::new();
    let read = BufReader::new((&stream).take(REQUEST_MAX)).read_line(&mut line);
    let answer = match read.map_err(|e| e.to_string()).and_then(|_| {
        serde_json::from_str::<Request>(&line).map_err(|e| format!("bad request: {e}"))
    }) {
        Ok(request) => {
            let time = match request {
                Request::Create { .. } => CREATE_TIME,
                _ => ANSWER_TIME,
            };
            let (answer, wait) = mpsc::channel();
            if tx.send(Call { request, answer }).is_err() {
                return;
            }
            wait.recv_timeout(time)
                .unwrap_or_else(|_| json!({ "error": "gerdr didn't answer in time" }))
        }
        Err(why) => json!({ "error": why }),
    };
    let mut stream = stream;
    if let Err(err) = writeln!(stream, "{answer}") {
        tracing::debug!("phone socket: the answer didn't go: {err}");
    }
}

/// Remove the phone sockets in `dir`, but `mine`, no gerdr listens on:
/// left by gerdrs that ended.
fn remove_dead_sockets(dir: &std::path::Path, mine: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if path == mine || !name.is_some_and(|n| is_socket_name(&n)) {
            continue;
        }
        if let Err(err) = UnixStream::connect(&path)
            && err.kind() == std::io::ErrorKind::ConnectionRefused
            && let Err(err) = std::fs::remove_file(&path)
        {
            tracing::warn!("phone socket {}: {err}", path.display());
        }
    }
}

impl Gerdr {
    /// Listen for the phone view, on a socket of this gerdr's.
    pub fn watch_phone(&mut self) {
        let Some(dir) = dir() else {
            return tracing::warn!("phone socket: no runtime folder");
        };
        let path = dir.join(format!("phone-{}.sock", std::process::id()));
        let inserted = std::fs::create_dir_all(&dir)
            .and_then(|()| listen(&path))
            .map_err(|e| e.to_string())
            .and_then(|rx| {
                self.loop_handle
                    .insert_source(rx, |event, _, state| {
                        if let channel::Event::Msg(call) = event {
                            state.answer_phone(call);
                        }
                    })
                    .map_err(|e| e.to_string())
            });
        match inserted {
            Ok(_) => remove_dead_sockets(&dir, &path),
            Err(err) => tracing::warn!("phone socket {}: {err}", path.display()),
        }
    }

    /// Whether the user is away from the machine: the host saw no input for
    /// `host_idle::AWAY`; on a host that can't tell, gerdr saw none.
    pub(crate) fn away(&mut self) -> bool {
        match self.notices.idle.as_mut() {
            Some(idle) => idle.away(),
            None => self
                .notices
                .last_input
                .is_none_or(|at| at.elapsed() >= host_idle::AWAY),
        }
    }

    /// Send `note` to the phones that asked, while the user is away. A
    /// `reminder` goes only if nothing with its tag went for
    /// `REMIND_PHONE`. Before the user counts as away, a note is held: if
    /// they went before it came (no input since), it goes once they count.
    pub(crate) fn push_phone(&mut self, note: Note, reminder: bool) {
        if self.away() {
            return self.send_phone(note, reminder);
        }
        if reminder {
            return;
        }
        let watching = !self.notices.held.is_empty();
        let held = &mut self.notices.held;
        held.retain(|(n, _)| n.tag != note.tag);
        held.push((note, Instant::now()));
        if watching {
            return;
        }
        let look = |_, _: &mut (), state: &mut Gerdr| {
            let away = state.away();
            for note in take_due(&mut state.notices.held, Instant::now(), away) {
                state.send_phone(note, false);
            }
            if state.notices.held.is_empty() {
                TimeoutAction::Drop
            } else {
                TimeoutAction::ToDuration(HOLD_LOOK)
            }
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(HOLD_LOOK), look)
        {
            tracing::warn!("holding a phone note: {err}");
            self.notices.held.clear();
        }
    }

    fn send_phone(&mut self, note: Note, reminder: bool) {
        let now = Instant::now();
        let pushed = &mut self.notices.pushed;
        pushed.retain(|_, at| now.duration_since(*at) < REMIND_PHONE);
        if reminder && pushed.contains_key(&note.tag) {
            return;
        }
        pushed.insert(note.tag.clone(), now);
        self.notices
            .pusher
            .get_or_insert_with(Pusher::start)
            .send(note);
    }

    fn answer_phone(&mut self, call: Call) {
        let Call { request, answer } = call;
        let reply = match request {
            Request::Create {
                project,
                title,
                repos,
                idea,
                agent,
            } => return self.phone_create(project, title, repos, idea, agent, answer),
            Request::NewOffer { projects } => self.phone_new_offer(&projects),
            Request::Offer {
                project,
                slug,
                stage,
            } => self.phone_offer(&project, &slug, stage),
            Request::Cards { features } => self.phone_cards(&features),
            Request::Ask { pane, .. } => {
                let asked = self.phone_ask(&pane).map(|a| {
                    // A plan reads as its markdown, rendered here.
                    let plan =
                        crate::chrome::is_plan(&a).then(|| crate::md_preview::render(&a.input));
                    json!({
                        "ask": a.ask, "tool": a.tool, "gist": a.gist, "input": a.input,
                        "questions": a.questions, "plan": plan,
                    })
                });
                let sides = self.phone_sides(&pane);
                json!({ "ask": asked, "working": self.turn_took(&pane), "sides": sides })
            }
            Request::Side { pane, text, .. } => match self.send_line(&pane, &text) {
                Ok(()) => json!({ "ok": true }),
                Err(why) => json!({ "error": format!("Couldn't send it: {why}") }),
            },
            Request::Answer {
                pane,
                ask,
                reply,
                answers,
                ..
            } => match self.phone_answer(&pane, &ask, reply, answers) {
                true => json!({ "ok": true }),
                false => json!({ "error": "It was answered already" }),
            },
            Request::Press {
                project,
                slug,
                stage,
                id,
                text,
                confirm,
            } => {
                let act = PhoneAct::Press { stage, id, text };
                self.phone_act(&project, &slug, act, confirm)
            }
            Request::SendBack {
                project,
                slug,
                text,
                confirm,
            } => {
                // Empty, it may still have line comments to send.
                self.phone_act(&project, &slug, PhoneAct::SendBack(text), confirm)
            }
        };
        // The phone may have given up on it.
        let _ = answer.send(reply);
    }
}

/// The held notes to send now, taken out of `held`: all of them once the
/// user is `away`. Older ones go first, away or not: the user touched the
/// machine after them. Away starts `AWAY` after the last input and is seen
/// up to `HOLD_LOOK` late, so a note older than both came before it.
fn take_due(held: &mut Vec<(Note, Instant)>, now: Instant, away: bool) -> Vec<Note> {
    held.retain(|(_, at)| now.duration_since(*at) <= host_idle::AWAY + HOLD_LOOK);
    if !away {
        return Vec::new();
    }
    held.drain(..).map(|(note, _)| note).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_notes_go_once_away_unless_older_than_the_last_input() {
        let note = |tag: &str| Note {
            title: String::new(),
            body: String::new(),
            tag: tag.into(),
            url: "/".into(),
        };
        let now = Instant::now() + host_idle::AWAY * 2;
        let mut held = vec![
            (note("stale"), now - host_idle::AWAY - HOLD_LOOK * 2),
            (note("fresh"), now - host_idle::AWAY + HOLD_LOOK),
        ];
        assert!(take_due(&mut held, now, false).is_empty());
        assert_eq!(held.len(), 1, "the stale note is dropped");
        let sent = take_due(&mut held, now, true);
        assert_eq!(
            sent.iter().map(|n| n.tag.as_str()).collect::<Vec<_>>(),
            ["fresh"]
        );
        assert!(held.is_empty());
    }

    #[test]
    fn requests_read_as_the_phone_sends_them() {
        let press: Request = serde_json::from_str(
            r#"{"op":"press","project":"p","slug":"s","id":"RerunCheck(1)","stage":4}"#,
        )
        .expect("press");
        assert_eq!(
            press,
            Request::Press {
                project: "p".into(),
                slug: "s".into(),
                stage: Some(4),
                id: "RerunCheck(1)".into(),
                text: None,
                confirm: false
            }
        );
        let back: Request = serde_json::from_str(
            r#"{"op":"send-back","project":"p","slug":"s","text":"x","confirm":true}"#,
        )
        .expect("send back");
        assert!(matches!(back, Request::SendBack { confirm: true, .. }));
        let cards: Request =
            serde_json::from_str(r#"{"op":"cards","features":[["p","s"]]}"#).expect("cards");
        let features = vec![("p".to_owned(), "s".to_owned())];
        assert_eq!(cards, Request::Cards { features });
        // The phone sends the feature's slug too: not needed for an ask.
        let ask: Request =
            serde_json::from_str(r#"{"op":"ask","project":"p","slug":"s","pane":"w1:p2"}"#)
                .expect("ask");
        assert!(matches!(ask, Request::Ask { pane, .. } if pane == "w1:p2"));
        let side: Request =
            serde_json::from_str(r#"{"op":"side","project":"p","pane":"w1:p2","text":"/btw why"}"#)
                .expect("side");
        assert!(matches!(side, Request::Side { text, .. } if text == "/btw why"));
        let answer: Request = serde_json::from_str(
            r#"{"op":"answer","project":"p","slug":"s","pane":"w1:p2","ask":"q1","reply":"deny"}"#,
        )
        .expect("answer");
        assert!(matches!(
            answer,
            Request::Answer {
                reply: crate::chat::live::AskReply::Deny,
                ..
            }
        ));
        let create: Request = serde_json::from_str(
            r#"{"op":"create","project":"p","slug":"","title":"Bulk edit","repos":["api"]}"#,
        )
        .expect("create");
        assert_eq!(
            create,
            Request::Create {
                project: "p".into(),
                title: "Bulk edit".into(),
                repos: vec!["api".into()],
                idea: String::new(),
                agent: None,
            }
        );
        let offer: Request =
            serde_json::from_str(r#"{"op":"new-offer","projects":["p"]}"#).expect("offer");
        assert_eq!(offer, Request::NewOffer { projects: vec!["p".into()] });
    }

    #[test]
    fn sockets_no_gerdr_listens_on_go() {
        let dir = std::env::temp_dir().join(format!("gerdr-phone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (dead, live, mine) = (
            dir.join("phone-1.sock"),
            dir.join("phone-2.sock"),
            dir.join("phone-3.sock"),
        );
        drop(UnixListener::bind(&dead).expect("dead"));
        let _live = UnixListener::bind(&live).expect("live");
        drop(UnixListener::bind(&mine).expect("mine"));
        std::fs::write(dir.join("other.sock"), "").expect("other");
        remove_dead_sockets(&dir, &mine);
        assert!(!dead.exists());
        assert!(live.exists() && mine.exists() && dir.join("other.sock").exists());
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    #[test]
    fn a_request_is_answered_on_its_connection() {
        let (a, b) = UnixStream::pair().expect("pair");
        let (tx, rx) = channel::channel::<Call>();
        let server = std::thread::spawn(move || serve(b, &tx));
        writeln!(&a, r#"{{"op":"offer","project":"p","slug":"s"}}"#).expect("write");
        let call = rx.recv().expect("call");
        assert_eq!(
            call.request,
            Request::Offer {
                project: "p".into(),
                slug: "s".into(),
                stage: None
            }
        );
        call.answer.send(json!({ "buttons": [] })).expect("answer");
        server.join().expect("server");
        let mut line = String::new();
        BufReader::new(&a).read_line(&mut line).expect("read");
        assert_eq!(line.trim(), r#"{"buttons":[]}"#);
    }
}
