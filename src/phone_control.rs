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
use smithay::reexports::calloop::channel;

use crate::{
    Gerdr,
    feature::review_import::Decision,
    features::PhoneAct,
    host_idle,
    web_push::{Note, Pusher},
};

/// The most a request may take.
const REQUEST_MAX: u64 = 64 * 1024;
/// How long a request waits for the loop to answer it.
const ANSWER_TIME: Duration = Duration::from_secs(20);
/// A reminder goes to the phones only if nothing with its tag went for
/// this long.
const REMIND_PHONE: Duration = Duration::from_secs(15 * 60);

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
    /// Press a button of its page.
    Press {
        project: String,
        slug: String,
        label: String,
        #[serde(default)]
        confirm: bool,
    },
    /// Decide a finding: `fix`, `dismiss` (with a reason) or `reopen`.
    Decide {
        project: String,
        slug: String,
        id: String,
        decision: String,
        #[serde(default)]
        reason: String,
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
            let (answer, wait) = mpsc::channel();
            if tx.send(Call { request, answer }).is_err() {
                return;
            }
            wait.recv_timeout(ANSWER_TIME)
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

/// A decision in the phone's words.
fn decision(word: &str, reason: String) -> Result<Decision, String> {
    match word {
        "fix" => Ok(Decision::Fix),
        "reopen" => Ok(Decision::Reopen),
        "dismiss" if reason.trim().is_empty() => Err("A dismissal needs a reason".into()),
        "dismiss" => Ok(Decision::Dismiss(reason.trim().to_owned())),
        _ => Err(format!("No such decision: {word}")),
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
                            let answer = state.answer_phone(call.request);
                            // The phone may have given up on it.
                            let _ = call.answer.send(answer);
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
    /// `REMIND_PHONE`.
    pub(crate) fn push_phone(&mut self, note: Note, reminder: bool) {
        if !self.away() {
            return;
        }
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

    fn answer_phone(&mut self, request: Request) -> Value {
        match request {
            Request::Offer {
                project,
                slug,
                stage,
            } => self.phone_offer(&project, &slug, stage),
            Request::Cards { features } => self.phone_cards(&features),
            Request::Press {
                project,
                slug,
                label,
                confirm,
            } => self.phone_act(&project, &slug, PhoneAct::Press(label), confirm),
            Request::Decide {
                project,
                slug,
                id,
                decision: word,
                reason,
                confirm,
            } => match decision(&word, reason) {
                Ok(d) => self.phone_act(&project, &slug, PhoneAct::Decide(id, d), confirm),
                Err(why) => json!({ "error": why }),
            },
            Request::SendBack {
                project,
                slug,
                text,
                confirm,
            } => {
                if text.trim().is_empty() {
                    return json!({ "error": "Say what to change" });
                }
                self.phone_act(&project, &slug, PhoneAct::SendBack(text), confirm)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_the_phone_sends_them() {
        let press: Request =
            serde_json::from_str(r#"{"op":"press","project":"p","slug":"s","label":"Approve"}"#)
                .expect("press");
        assert_eq!(
            press,
            Request::Press {
                project: "p".into(),
                slug: "s".into(),
                label: "Approve".into(),
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
    fn a_dismissal_needs_a_reason() {
        assert_eq!(decision("fix", String::new()), Ok(Decision::Fix));
        assert!(decision("dismiss", "  ".into()).is_err());
        assert_eq!(
            decision("dismiss", " not real ".into()),
            Ok(Decision::Dismiss("not real".into()))
        );
        assert!(decision("maybe", String::new()).is_err());
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
