//! Whether the keyring (the session's Secret Service, gnome-keyring or
//! KWallet's) is open, for the apps whose logins it locks: a Chromium's
//! cookies and saved passwords, a VS Code's sign-ins, `gh`'s token.
//!
//! Such an app that starts while the keyring is locked, or while its
//! daemon starts again after a crash, gets no key: it can't read its
//! cookies and drops them, and its logins are gone. Several asking it to
//! unlock at once, or one quitting while it answers, can crash
//! gnome-keyring itself. So gerdr holds them (see `Gerdr::launch`) until
//! the keyring is open, and asks for it once, itself.
//!
//! A thread keeps one connection to the session bus: it reads whether the
//! default collection is locked when the service comes or goes and every
//! little while, and asks to unlock it when gerdr wants (see `ask`). The
//! event loop hears of each change (see `Gerdr::watch_keyring`).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use dbus::{Message, blocking::Connection, message::MatchRule};
use smithay::reexports::calloop::channel;

use crate::{Gerdr, layout, menu::MenuAction, toast::Level};

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const DEFAULT: &str = "/org/freedesktop/secrets/aliases/default";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";

/// How long a call to the keyring may take: its daemon may be started
/// for it.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the keyring is read again: while locked or asked, and else.
const POLL_LOCKED: Duration = Duration::from_secs(2);
const POLL_OPEN: Duration = Duration::from_secs(15);
/// How long the user is given to answer the unlock prompt.
const PROMPT_WAIT: Duration = Duration::from_secs(600);

/// What gerdr knows of the keyring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not read yet.
    Unknown,
    /// No keyring answers, or it has no default collection: apps go
    /// without, so there is nothing to wait for.
    Absent,
    Open,
    Locked,
    /// gerdr asked to unlock it, and the prompt is up.
    Asking,
    /// The user dismissed the prompt: it stays locked until asked again.
    Dismissed,
}

impl State {
    /// Whether an app that keeps its logins in the keyring can start.
    pub fn ready(self) -> bool {
        matches!(self, State::Open | State::Absent)
    }

    fn from_u8(n: u8) -> Self {
        match n {
            1 => State::Absent,
            2 => State::Open,
            3 => State::Locked,
            4 => State::Asking,
            5 => State::Dismissed,
            _ => State::Unknown,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            State::Unknown => 0,
            State::Absent => 1,
            State::Open => 2,
            State::Locked => 3,
            State::Asking => 4,
            State::Dismissed => 5,
        }
    }
}

static STATE: AtomicU8 = AtomicU8::new(0);
/// Whether the keyring is read (see `watch`): without, nothing waits for it.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// gerdr wants the keyring unlocked: the thread asks once.
static ASK: AtomicBool = AtomicBool::new(false);

/// The keyring as last read.
pub fn state() -> State {
    if !WATCHING.load(Ordering::Acquire) {
        return State::Absent;
    }
    State::from_u8(STATE.load(Ordering::Acquire))
}

/// Ask the user to unlock the keyring, once: nothing when it is open,
/// or the prompt is up.
pub fn ask() {
    ASK.store(true, Ordering::Release);
}

/// Read the keyring on a thread of its own, which sends `tx` each change.
pub fn watch(tx: channel::Sender<State>) {
    WATCHING.store(true, Ordering::Release);
    let spawned = std::thread::Builder::new()
        .name("keyring".into())
        .spawn(move || run(&tx));
    if let Err(err) = spawned {
        tracing::warn!("keyring: {err}");
        publish(State::Absent, None);
    }
}

/// Set the state, and tell the event loop when it changed.
fn publish(state: State, tx: Option<&channel::Sender<State>>) {
    let old = STATE.swap(state.to_u8(), Ordering::AcqRel);
    if old != state.to_u8() {
        tracing::info!("keyring: {state:?}");
        if let Some(tx) = tx {
            let _ = tx.send(state);
        }
    }
}

fn run(tx: &channel::Sender<State>) {
    let conn = match Connection::new_session() {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!("keyring: no session bus: {err}");
            return publish(State::Absent, Some(tx));
        }
    };
    // Read again at once when the keyring's daemon comes or goes: one
    // started again after a crash starts locked.
    let changed = Arc::new(AtomicBool::new(false));
    let owner = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
        .with_sender("org.freedesktop.DBus");
    let flag = changed.clone();
    let watched = conn.add_match(owner, move |(): (), _, msg: &Message| {
        if msg.get1::<String>().as_deref() == Some(SERVICE) {
            flag.store(true, Ordering::Release);
        }
        true
    });
    if let Err(err) = watched {
        tracing::warn!("keyring: not told when it restarts: {err}");
    }
    let mut last = None::<Instant>;
    // The prompt up: when it went up, and whether it answered.
    let mut prompt: Option<(Instant, Arc<AtomicBool>)> = None;
    loop {
        if let Err(err) = conn.process(Duration::from_millis(250)) {
            tracing::warn!("keyring: {err}");
            std::thread::sleep(Duration::from_secs(1));
        }
        let now = Instant::now();
        let current = state();
        if let Some((since, done)) = &prompt
            && (done.load(Ordering::Acquire) || now - *since > PROMPT_WAIT)
        {
            prompt = None;
            // What was asked meanwhile is answered.
            ASK.store(false, Ordering::Release);
            // Still locked once the prompt went: dismissed (or left).
            let found = match read(&conn) {
                State::Locked => State::Dismissed,
                found => found,
            };
            publish(found, Some(tx));
            last = Some(now);
            continue;
        }
        if prompt.is_some() {
            continue;
        }
        let poll = if current.ready() {
            POLL_OPEN
        } else {
            POLL_LOCKED
        };
        let due = changed.swap(false, Ordering::AcqRel) || last.is_none_or(|t| now - t > poll);
        let asked = ASK.swap(false, Ordering::AcqRel);
        if !due && !asked {
            continue;
        }
        last = Some(now);
        let mut found = read(&conn);
        if found == State::Locked {
            if asked {
                match unlock(&conn) {
                    Ok(Some(done)) => {
                        prompt = Some((now, done));
                        found = State::Asking;
                    }
                    Ok(None) => found = read(&conn),
                    // Said like a dismissed prompt: the held apps say they
                    // wait, with an Unlock to try again.
                    Err(err) => {
                        tracing::warn!("keyring: couldn't ask to unlock it: {err}");
                        found = State::Dismissed;
                    }
                }
            } else if current == State::Dismissed {
                // Not asked again unasked.
                found = State::Dismissed;
            }
        }
        publish(found, Some(tx));
    }
}

/// Whether the default collection is locked, read now.
fn read(conn: &Connection) -> State {
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    let collection = conn.with_proxy(SERVICE, DEFAULT, CALL_TIMEOUT);
    match collection.get::<bool>(COLLECTION_IFACE, "Locked") {
        Ok(true) => State::Locked,
        Ok(false) => State::Open,
        Err(err) => {
            tracing::debug!("keyring: {err}");
            State::Absent
        }
    }
}

/// Ask the keyring to unlock the default collection: when that takes
/// the user's answer, the prompt is up and the flag says when it went.
/// `None` when it opened at once.
fn unlock(conn: &Connection) -> Result<Option<Arc<AtomicBool>>, dbus::Error> {
    let service = conn.with_proxy(SERVICE, SERVICE_PATH, CALL_TIMEOUT);
    let default = dbus::Path::from(DEFAULT);
    let (_, prompt): (Vec<dbus::Path>, dbus::Path) =
        service.method_call(SERVICE_IFACE, "Unlock", (vec![default],))?;
    if &*prompt == "/" {
        return Ok(None);
    }
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let completed = MatchRule::new_signal(PROMPT_IFACE, "Completed").with_path(prompt.clone());
    // Once: the match goes with it.
    conn.add_match(completed, move |(): (), _, _: &Message| {
        flag.store(true, Ordering::Release);
        false
    })?;
    // This connection stays while the prompt is up: the keyring would
    // take it down with its asker.
    conn.with_proxy(SERVICE, prompt, CALL_TIMEOUT)
        .method_call::<(), _, _, _>(PROMPT_IFACE, "Prompt", ("",))?;
    Ok(Some(done))
}

impl Gerdr {
    /// Read the keyring off the event loop (see `watch`), and start the
    /// apps held for it once it is open.
    pub fn watch_keyring(&mut self) {
        let (tx, rx) = channel::channel::<State>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(keyring) = event {
                state.keyring_changed(keyring);
            }
        });
        match inserted {
            Ok(_) => watch(tx),
            Err(err) => {
                tracing::warn!("keyring: {err}");
                publish(State::Absent, None);
            }
        }
    }

    /// Start the apps held for the keyring once it is open; ask for it
    /// while they wait, and say so when the user said no.
    fn keyring_changed(&mut self, keyring: State) {
        if keyring.ready() {
            return self.start_held();
        }
        let held = self.held_labels();
        if held.is_empty() {
            return;
        }
        match keyring {
            State::Locked => ask(),
            State::Dismissed => self.toast_with(
                format!(
                    "The keyring stayed locked: {} {} for it, so its logins aren't lost",
                    held.join(", "),
                    if held.len() == 1 { "waits" } else { "wait" }
                ),
                Level::Warning,
                Some(("Unlock".into(), MenuAction::UnlockKeyring)),
            ),
            _ => {}
        }
    }

    /// An app was held for the keyring: ask for it, unless it is being
    /// read yet (asked then, see `keyring_changed`) or was refused.
    pub(crate) fn wait_for_keyring(&mut self) {
        match state() {
            State::Locked => ask(),
            State::Dismissed => self.keyring_changed(State::Dismissed),
            _ => {}
        }
    }

    /// What the apps held for the keyring are, once each ("chromium-browser").
    fn held_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| &tab.expected)
            .filter(|e| e.held)
            .map(|e| e.label())
            .collect();
        labels.sort();
        labels.dedup();
        labels
    }

    /// Start the apps held for the keyring (see `Gerdr::launch_into`); the
    /// queued ones in their turn (see `launch_next_apps`). One that doesn't
    /// start leaves its leaf.
    fn start_held(&mut self) {
        let mut held = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for e in tab
                    .expected
                    .iter()
                    .filter(|e| e.held && e.app_id.is_empty())
                {
                    if let Some(command) = &e.app_command {
                        held.push((w, t, e.pane, command.clone()));
                    }
                }
            }
        }
        let mut dropped = false;
        for (w, t, pane, command) in held {
            let pid = self.start_app(w, t, &command, None);
            let kept = self.is_kept(pid);
            let tab = &mut self.workspaces[w].tabs[t];
            let Some(i) = tab.expected.iter().position(|e| e.pane == pane) else {
                continue;
            };
            match pid {
                Some(pid) => {
                    let e = &mut tab.expected[i];
                    e.pid = Some(pid);
                    e.held = false;
                    e.kept = kept;
                    e.started = Instant::now();
                }
                None => {
                    let gone = tab.expected.remove(i);
                    layout::remove(&mut tab.layout, gone.pane);
                    dropped = true;
                }
            }
        }
        dropped |= self.launch_next_apps();
        if dropped {
            self.prune();
        }
        self.arrange();
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    /// Run in a private session bus with a keyring of its own, never the
    /// user's: `dbus-run-session -- sh -c 'export HOME=$(mktemp -d);
    /// echo -n pw | gnome-keyring-daemon --daemonize --unlock
    /// --components=secrets; cargo test -- --ignored keyring'`.
    #[test]
    #[ignore]
    fn reads_whether_the_keyring_is_locked() {
        let conn = dbus::blocking::Connection::new_session().unwrap();
        assert_eq!(super::read(&conn), State::Open);
        let service = conn.with_proxy(super::SERVICE, super::SERVICE_PATH, super::CALL_TIMEOUT);
        let _: (Vec<dbus::Path>,) = service
            .method_call(
                super::SERVICE_IFACE,
                "Lock",
                (vec![dbus::Path::from(super::DEFAULT)],),
            )
            .unwrap();
        assert_eq!(super::read(&conn), State::Locked);
    }

    #[test]
    fn apps_wait_only_for_a_keyring_that_is_there_and_shut() {
        for state in [State::Open, State::Absent] {
            assert!(state.ready(), "{state:?}");
        }
        for state in [
            State::Unknown,
            State::Locked,
            State::Asking,
            State::Dismissed,
        ] {
            assert!(!state.ready(), "{state:?}");
        }
        for n in 0..6 {
            assert_eq!(State::from_u8(n).to_u8(), n);
        }
    }
}
