//! A thread of its own that finds the chats of the panes shown as chats,
//! reads their transcripts as they grow and sends their items back.

use std::{
    collections::HashMap,
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    os::unix::fs::MetadataExt as _,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use smithay::reexports::calloop::channel;

use super::{Event, Item, Source, parse::Reader, source};
use crate::{ai_work::locate::Homes, herdr::AgentInfo};

/// The most of a transcript read the first time: its end.
// shortcut: earlier turns past this aren't read; paging back comes in Inc 3.
pub const CHAT_TAIL: u64 = 2 * 1024 * 1024;
/// How often the followed chats are looked at.
const POLL: Duration = Duration::from_millis(500);
/// How often a chat not found yet is looked for again: that lists the
/// agent's folders of chats.
const FIND_AGAIN: Duration = Duration::from_secs(5);
/// The bytes kept from before the read offset: when they change, the file
/// was written anew and is read again from the start.
const PREV: usize = 4096;

enum Cmd {
    Follow(Box<AgentInfo>),
    Stop(String),
}

/// The thread: what it is asked to follow.
pub struct Tail {
    cmds: mpsc::Sender<Cmd>,
}

impl Tail {
    /// Start the thread, sending what it reads to `out`.
    pub fn spawn(out: channel::Sender<Event>) -> Self {
        let (cmds, asked) = mpsc::channel();
        std::thread::spawn(move || {
            let homes = Homes::here().unwrap_or_else(|| {
                tracing::warn!("agent chats: no home folder, so no chat is found");
                Homes {
                    claude: Vec::new(),
                    ..Homes::under(std::path::Path::new("/"))
                }
            });
            run(&asked, &mut Chats::new(homes), &out);
        });
        Self { cmds }
    }

    /// Follow the chat of the agent in pane `agent.pane_id`: find it, read
    /// it, and send its items each time it grows. Asked again for the same
    /// pane (herdr reports another session), it finds the chat again.
    pub fn follow(&self, agent: AgentInfo) {
        self.send(Cmd::Follow(Box::new(agent)));
    }

    /// Stop following the chat of pane `pane_id` (hidden or closed).
    pub fn stop(&self, pane_id: &str) {
        self.send(Cmd::Stop(pane_id.to_owned()));
    }

    fn send(&self, cmd: Cmd) {
        if self.cmds.send(cmd).is_err() {
            tracing::warn!("agent chats: the thread that reads them has ended");
        }
    }
}

/// The thread's loop: until the `Tail` is dropped or gerdr stops listening.
fn run(asked: &mpsc::Receiver<Cmd>, chats: &mut Chats, out: &channel::Sender<Event>) {
    let mut next = Instant::now() + POLL;
    loop {
        // Nothing followed: nothing to poll, wait for a command.
        let cmd = match chats.panes.is_empty() {
            true => asked
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            false => asked.recv_timeout(next.saturating_duration_since(Instant::now())),
        };
        let mut events = Vec::new();
        match cmd {
            Ok(Cmd::Follow(agent)) => {
                if chats.panes.is_empty() {
                    next = Instant::now() + POLL;
                }
                chats.follow(*agent, &mut events);
            }
            Ok(Cmd::Stop(pane_id)) => chats.stop(&pane_id),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                chats.poll(&mut events);
                next = Instant::now() + POLL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        for event in events {
            if out.send(event).is_err() {
                return;
            }
        }
    }
}

/// The panes followed, and their chats.
struct Chats {
    homes: Homes,
    panes: HashMap<String, Followed>,
}

struct Followed {
    agent: AgentInfo,
    /// When its chat was last looked for.
    looked: Instant,
    /// Its chat, or why it has none (told the user once).
    chat: Result<Chat, String>,
}

impl Chats {
    fn new(homes: Homes) -> Self {
        Self {
            homes,
            panes: HashMap::new(),
        }
    }

    fn follow(&mut self, agent: AgentInfo, out: &mut Vec<Event>) {
        let found = source::find(&agent, &self.homes);
        if let Some(had) = self.panes.get_mut(&agent.pane_id) {
            let same = match (&had.chat, &found) {
                (Ok(chat), Ok(source)) => chat.source == *source,
                (Err(why), Err(now)) => why == now,
                _ => false,
            };
            if same {
                had.agent = agent;
                return;
            }
        }
        let pane_id = agent.pane_id.clone();
        let mut followed = Followed {
            chat: Err(String::new()),
            agent,
            looked: Instant::now(),
        };
        followed.found(found, out);
        self.panes.insert(pane_id, followed);
    }

    fn stop(&mut self, pane_id: &str) {
        self.panes.remove(pane_id);
    }

    /// Read what each chat grew by; look again for the chats not found.
    fn poll(&mut self, out: &mut Vec<Event>) {
        for followed in self.panes.values_mut() {
            match &mut followed.chat {
                Ok(chat) => chat.poll(&followed.agent.pane_id, out),
                Err(_) if followed.looked.elapsed() < FIND_AGAIN => {}
                Err(why) => {
                    followed.looked = Instant::now();
                    let found = source::find(&followed.agent, &self.homes);
                    if found.as_ref().err() != Some(&*why) {
                        followed.found(found, out);
                    }
                }
            }
        }
    }
}

impl Followed {
    /// Take chat `found`: read it, or say why there is none.
    fn found(&mut self, found: Result<Source, String>, out: &mut Vec<Event>) {
        let pane_id = self.agent.pane_id.clone();
        match found {
            Ok(source) => {
                let mut chat = Chat::new(source);
                chat.poll(&pane_id, out);
                self.chat = Ok(chat);
            }
            Err(why) => {
                let session = &self.agent.agent_session;
                tracing::warn!("agent chat of pane {pane_id}: {why} (session {session:?})");
                out.push(Event::Missing {
                    pane_id,
                    why: why.clone(),
                });
                self.chat = Err(why);
            }
        }
    }
}

/// A transcript being read, line by line as it grows.
struct Chat {
    source: Source,
    reader: Reader,
    /// The file read (device and inode): another one is read from the start.
    file: Option<(u64, u64)>,
    /// How far it is read.
    offset: u64,
    /// Whether `offset` is at a line's start: not when the first read began
    /// mid-line.
    whole: bool,
    /// The bytes just before `offset`, at most `PREV`.
    prev: Vec<u8>,
    /// The items last sent; None: send them next, changed or not.
    sent: Option<Arc<Vec<Item>>>,
    /// A read failed, and the user was told.
    failed: bool,
    /// How often it was read again from the start.
    resets: u32,
}

impl Chat {
    fn new(source: Source) -> Self {
        Self {
            reader: Reader::new(source.agent),
            source,
            file: None,
            offset: 0,
            whole: true,
            prev: Vec::new(),
            sent: None,
            failed: false,
            resets: 0,
        }
    }

    /// Read what it grew by, and send its items if they changed.
    fn poll(&mut self, pane_id: &str, out: &mut Vec<Event>) {
        if let Err(err) = self.read() {
            if !self.failed {
                let path = self.source.path.display();
                tracing::warn!("agent chat of pane {pane_id}: can't read {path}: {err}");
                out.push(Event::Failed {
                    pane_id: pane_id.to_owned(),
                    error: format!("Couldn't read this agent's chat: {err}"),
                });
                self.failed = true;
                self.sent = None;
            }
            return;
        }
        self.failed = false;
        if self.sent.as_deref().map(Vec::as_slice) != Some(self.reader.items()) {
            let items = Arc::new(self.reader.items().to_vec());
            self.sent = Some(items.clone());
            out.push(Event::Items {
                pane_id: pane_id.to_owned(),
                source: self.source.clone(),
                items,
            });
        }
    }

    /// Feed the reader the whole lines written since the last read. Another
    /// file, a shorter one or one whose bytes before the offset changed is
    /// read again from the start, with a fresh reader.
    fn read(&mut self) -> std::io::Result<()> {
        let mut file = File::open(&self.source.path)?;
        let meta = file.metadata()?;
        let (id, len) = ((meta.dev(), meta.ino()), meta.len());
        if self.file != Some(id) || len < self.offset {
            self.restart(id, len);
        }
        if len == self.offset {
            return Ok(());
        }
        let back = self.prev.len() as u64;
        file.seek(SeekFrom::Start(self.offset - back))?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(len - self.offset + back)
            .read_to_end(&mut bytes)?;
        if !bytes.starts_with(&self.prev) {
            self.restart(id, len);
            file.seek(SeekFrom::Start(self.offset))?;
            bytes.clear();
            (&mut file)
                .take(len - self.offset)
                .read_to_end(&mut bytes)?;
        }
        let back = self.prev.len();
        let new = &bytes[back..];
        // Begun mid-line: from the first whole one.
        let start = match self.whole {
            true => 0,
            false => match new.iter().position(|&b| b == b'\n') {
                Some(at) => {
                    self.whole = true;
                    at + 1
                }
                None => new.len(),
            },
        };
        // A last line still being written waits until it is whole.
        let end =
            (new[start..].iter().rposition(|&b| b == b'\n')).map_or(start, |at| start + at + 1);
        if end > start {
            self.reader.feed(&new[start..end]);
        }
        self.offset += end as u64;
        let before = &bytes[..back + end];
        self.prev = before[before.len().saturating_sub(PREV)..].to_vec();
        Ok(())
    }

    /// Read file `id`, `len` bytes long, again from the start: its last
    /// `CHAT_TAIL` bytes.
    fn restart(&mut self, id: (u64, u64), len: u64) {
        self.reader = Reader::new(self.source.agent);
        self.file = Some(id);
        self.offset = len.saturating_sub(CHAT_TAIL);
        self.whole = self.offset == 0;
        self.prev.clear();
        self.sent = None;
        self.resets += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write as _, path::PathBuf};

    use super::*;
    use crate::chat::source::tests::agent;

    fn line(role: &str, text: &str) -> String {
        let id = format!("{role}-{text}");
        let message =
            serde_json::json!({ "role": role, "content": [{ "type": "text", "text": text }] });
        let line = serde_json::json!({ "type": role, "uuid": id, "message": message });
        format!("{line}\n")
    }

    struct Home {
        dir: PathBuf,
        chats: Chats,
    }

    impl Home {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("gerdr-chat-tail-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(".claude/projects/-work")).expect("dir");
            let chats = Chats::new(Homes::under(&dir));
            Self { dir, chats }
        }

        fn file(&self, id: &str) -> PathBuf {
            self.dir.join(format!(".claude/projects/-work/{id}.jsonl"))
        }

        fn append(&self, id: &str, text: &str) {
            let mut file = (std::fs::OpenOptions::new().create(true).append(true))
                .open(self.file(id))
                .expect("open");
            file.write_all(text.as_bytes()).expect("write");
        }

        fn follow(&mut self, id: &str) -> Vec<Event> {
            let mut out = Vec::new();
            self.chats.follow(agent("claude", "id", id), &mut out);
            out
        }

        fn poll(&mut self) -> Vec<Event> {
            let mut out = Vec::new();
            self.chats.poll(&mut out);
            out
        }

        fn chat(&self) -> &Chat {
            self.chats.panes["p1"].chat.as_ref().expect("a chat")
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn items_of(events: &[Event]) -> Vec<&str> {
        (events.iter())
            .filter_map(|event| match event {
                Event::Items { source, .. } => Some(source.id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_chat_is_read_then_read_as_it_grows() {
        let mut home = Home::new("grow");
        let first = line("user", "hello");
        home.append("s1", &first);
        let events = home.follow("s1");
        assert_eq!(items_of(&events), ["s1"]);
        assert_eq!(home.chat().offset, first.len() as u64);
        assert!(home.poll().is_empty(), "nothing new");
        // A half-written line waits until it is whole.
        let second = line("assistant", "hi there");
        let (head, rest) = second.split_at(10);
        home.append("s1", head);
        home.poll();
        assert_eq!(home.chat().offset, first.len() as u64);
        home.append("s1", rest);
        home.poll();
        assert_eq!(home.chat().offset, (first.len() + second.len()) as u64);
        assert_eq!(home.chat().resets, 1, "read once from the start");
        // Asked again for the same chat: nothing new.
        assert!(home.follow("s1").is_empty());
    }

    #[test]
    fn a_shorter_or_rewritten_file_is_read_again() {
        let mut home = Home::new("reset");
        home.append("s1", &(line("user", "one") + &line("user", "two")));
        home.follow("s1");
        // Shorter.
        std::fs::write(home.file("s1"), line("user", "three")).expect("write");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(home.chat().resets, 2);
        assert_eq!(home.chat().offset, line("user", "three").len() as u64);
        // As long and more, but other bytes before the offset.
        let other = line("user", "fours") + &line("user", "five");
        std::fs::write(home.file("s1"), &other).expect("write");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(
            (home.chat().resets, home.chat().offset),
            (3, other.len() as u64)
        );
        // Another file in its place.
        let moved = home.dir.join("new.jsonl");
        std::fs::write(&moved, &other).expect("write");
        std::fs::rename(&moved, home.file("s1")).expect("rename");
        assert_eq!(items_of(&home.poll()), ["s1"]);
        assert_eq!(home.chat().resets, 4);
    }

    #[test]
    fn a_long_chat_is_read_from_its_end() {
        let mut home = Home::new("long");
        let filler = line("user", &"x".repeat(1000));
        let lines = (CHAT_TAIL as usize / filler.len()) + 10;
        home.append("s1", &filler.repeat(lines));
        home.follow("s1");
        let len = std::fs::metadata(home.file("s1")).unwrap().len();
        assert_eq!(home.chat().offset, len);
        assert!(home.chat().whole);
        home.append("s1", &line("user", "more"));
        home.poll();
        assert_eq!(home.chat().resets, 1);
    }

    #[test]
    fn a_new_session_switches_chats_and_stop_forgets() {
        let mut home = Home::new("switch");
        home.append("s1", &line("user", "one"));
        home.append("s2", &line("user", "two"));
        home.follow("s1");
        assert_eq!(items_of(&home.follow("s2")), ["s2"]);
        assert_eq!(home.chat().source.path, home.file("s2"));
        home.chats.stop("p1");
        assert!(home.chats.panes.is_empty());
        assert!(home.poll().is_empty());
    }

    #[test]
    fn a_chat_not_written_yet_is_found_once_it_is() {
        let mut home = Home::new("later");
        let events = home.follow("s1");
        assert!(matches!(&events[..], [Event::Missing { why, .. }] if why == source::NOT_YET));
        let ago = |home: &mut Home| {
            for followed in home.chats.panes.values_mut() {
                followed.looked -= FIND_AGAIN;
            }
        };
        ago(&mut home);
        assert!(home.poll().is_empty(), "told once");
        home.append("s1", &line("user", "one"));
        // Looked for again only once `FIND_AGAIN` is over.
        assert!(home.poll().is_empty());
        ago(&mut home);
        assert_eq!(items_of(&home.poll()), ["s1"]);
    }

    #[test]
    fn a_failed_read_is_told_once_and_tried_again() {
        let mut home = Home::new("failed");
        home.append("s1", &line("user", "one"));
        home.follow("s1");
        std::fs::remove_file(home.file("s1")).expect("remove");
        assert!(matches!(&home.poll()[..], [Event::Failed { .. }]));
        assert!(home.poll().is_empty(), "told once");
        home.append("s1", &line("user", "two"));
        assert_eq!(items_of(&home.poll()), ["s1"]);
    }

    #[test]
    fn the_thread_sends_and_ends_with_its_tail() {
        let home = Home::new("thread");
        home.append("s1", &line("user", "one"));
        let (out, events) = channel::channel();
        let (cmds, asked) = mpsc::channel();
        let homes = Homes::under(&home.dir);
        let thread = std::thread::spawn(move || run(&asked, &mut Chats::new(homes), &out));
        let tail = Tail { cmds };
        tail.follow(agent("claude", "id", "s1"));
        drop(tail);
        thread.join().expect("thread ends");
        let mut got = Vec::new();
        let mut events_loop =
            smithay::reexports::calloop::EventLoop::<Vec<String>>::try_new().unwrap();
        events_loop
            .handle()
            .insert_source(events, |event, _, got: &mut Vec<String>| {
                if let channel::Event::Msg(Event::Items { source, .. }) = event {
                    got.push(source.id);
                }
            })
            .unwrap();
        events_loop.dispatch(Duration::ZERO, &mut got).unwrap();
        assert_eq!(got, ["s1"]);
    }
}
