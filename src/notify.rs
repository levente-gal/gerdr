//! Desktop notifications, for agents that want attention while their pane is
//! out of sight. They go to the host's notification daemon over the session
//! bus, on a connection the event loop polls like any other source: there is
//! no thread or process waiting for the user to answer.

use std::{
    collections::HashMap,
    os::fd::{BorrowedFd, OwnedFd},
    time::Duration,
};

use dbus::{
    Message,
    arg::PropMap,
    channel::{BusType, Channel},
    message::MessageType,
};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// gerdr's connection to the notification daemon. Each notification belongs
/// to a key (a herdr pane id), and a newer one for the same key replaces it.
pub struct Notifier {
    channel: Channel,
    /// Keys of the notifications sent whose id hasn't come back yet, by
    /// message serial.
    sent: HashMap<u32, String>,
    /// Notifications on screen, by the daemon's id.
    shown: HashMap<u32, String>,
}

impl Notifier {
    /// Connect to the session bus and listen for clicks. Blocks until the bus
    /// has answered, which it does at once.
    pub fn connect() -> Result<Self, dbus::Error> {
        let mut channel = Channel::get_private(BusType::Session)?;
        channel.set_watch_enabled(true);
        for member in ["ActionInvoked", "NotificationClosed"] {
            let rule = format!("type='signal',interface='{DEST}',member='{member}'");
            let add = Message::new_method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "AddMatch",
            )
            .map_err(|err| dbus::Error::new_failed(&err))?
            .append1(rule);
            channel
                .send(add)
                .map_err(|()| dbus::Error::new_failed("AddMatch not sent"))?;
        }
        channel.flush();
        Ok(Self {
            channel,
            sent: HashMap::new(),
            shown: HashMap::new(),
        })
    }

    /// The fd to poll for reading; ready when there is something to `dispatch`.
    pub fn fd(&self) -> std::io::Result<OwnedFd> {
        // Safety: the channel keeps its fd open for as long as gerdr runs.
        unsafe { BorrowedFd::borrow_raw(self.channel.watch().fd) }.try_clone_to_owned()
    }

    /// Show a notification for `key`, in place of the last one for it if
    /// that is still on screen; clicking it shows up in `dispatch`.
    pub fn send(&mut self, key: &str, summary: &str, body: &str) {
        let replaces = self
            .shown
            .iter()
            .find(|(_, k)| *k == key)
            .map_or(0, |(id, _)| *id);
        let Ok(msg) = Message::new_method_call(DEST, PATH, DEST, "Notify") else {
            return;
        };
        let msg = msg
            .append3("gerdr", replaces, "utilities-terminal")
            .append3(summary, body, vec!["default", "Show"])
            .append2(PropMap::new(), -1i32);
        match self.channel.send(msg) {
            Ok(serial) => {
                tracing::debug!("notification for {key}: {summary}");
                self.sent.insert(serial, key.to_string());
            }
            Err(()) => tracing::warn!("notification not sent"),
        }
        // Mostly a no-op: send writes at once unless the socket is full.
        self.channel.flush();
    }

    /// Take in what the daemon sent, and return the keys of the
    /// notifications the user clicked since. `Err` once the bus is gone.
    pub fn dispatch(&mut self) -> Result<Vec<String>, ()> {
        self.channel.read_write(Some(Duration::ZERO))?;
        let mut clicked = Vec::new();
        while let Some(mut msg) = self.channel.pop_message() {
            match msg.msg_type() {
                MessageType::MethodReturn => {
                    let key = msg
                        .get_reply_serial()
                        .and_then(|serial| self.sent.remove(&serial));
                    if let (Some(key), Ok(id)) = (key, msg.read1::<u32>()) {
                        self.shown.insert(id, key);
                    }
                }
                MessageType::Error => {
                    if let Some(key) = msg
                        .get_reply_serial()
                        .and_then(|serial| self.sent.remove(&serial))
                    {
                        let name = msg.as_result().err().and_then(|e| e.name().map(str::to_owned));
                        tracing::warn!(
                            "notification for {key} refused: {}",
                            name.as_deref().unwrap_or("unknown error")
                        );
                    }
                }
                MessageType::Signal => match msg.member().as_deref() {
                    Some("ActionInvoked") => {
                        if let Ok((id, action)) = msg.read2::<u32, &str>()
                            && action == "default"
                            && let Some(key) = self.shown.get(&id)
                        {
                            clicked.push(key.clone());
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok(id) = msg.read1::<u32>() {
                            self.shown.remove(&id);
                        }
                    }
                    _ => {}
                },
                MessageType::MethodCall => {}
            }
        }
        Ok(clicked)
    }
}
