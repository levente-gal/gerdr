//! Whether the system prefers a dark or a light look, for the System
//! theme mode. Asked of the settings portal over the session bus
//! (`org.freedesktop.portal.Settings`, key `color-scheme` in the
//! `org.freedesktop.appearance` namespace: 1 for dark, 2 for light, 0 for
//! no preference), and followed through its `SettingChanged` signal on a
//! thread of its own. Without the portal, or with no preference there,
//! GNOME's setting is read once with `gsettings`; such a desktop isn't
//! followed for changes. With no preference anywhere, the look stays as
//! it was: dark at start.
//!
//! The first read is started at once on a thread (`read`), as the portal
//! may take up to `TIMEOUT` and `gsettings` is a process: gerdr waits for
//! it only when the System mode needs it before the first terminals (see
//! `watch`).

use std::{
    process::{Command, Stdio},
    time::Duration,
};

use dbus::{
    arg::{RefArg, Variant},
    blocking::Connection,
    message::MatchRule,
};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
/// How long the portal may take to answer: it is asked before the first
/// terminals start, so not for long.
const TIMEOUT: Duration = Duration::from_millis(500);
/// How long the watching thread waits for a signal at a time.
const WAIT: Duration = Duration::from_secs(3600);

/// What the portal's `color-scheme` value asks for: dark (1), light (2),
/// or neither.
fn prefers_dark(scheme: u64) -> Option<bool> {
    match scheme {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// The number in `arg`, through the variants around it (`Read` wraps the
/// value in two).
fn number(arg: &dyn RefArg) -> Option<u64> {
    arg.as_u64()
        .or_else(|| arg.as_i64().and_then(|n| u64::try_from(n).ok()))
}

/// The portal's `color-scheme` over `conn`: by `ReadOne` (portal version 2
/// on), else `Read`.
fn portal_scheme(conn: &Connection) -> Option<u64> {
    let proxy = conn.with_proxy(PORTAL, PORTAL_PATH, TIMEOUT);
    type Value = (Variant<Box<dyn RefArg>>,);
    let read_one: Result<Value, _> = proxy.method_call(SETTINGS, "ReadOne", (NAMESPACE, KEY));
    let (value,) = match read_one {
        Ok(value) => value,
        Err(_) => proxy
            .method_call::<Value, _, _, _>(SETTINGS, "Read", (NAMESPACE, KEY))
            .ok()?,
    };
    number(&value.0)
}

/// GNOME's `color-scheme`, for a desktop without the portal.
fn gsettings_scheme() -> Option<u64> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", KEY])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_gsettings(&String::from_utf8_lossy(&out.stdout))
}

/// The portal's value for what `gsettings` printed.
fn parse_gsettings(text: &str) -> Option<u64> {
    match text.trim().trim_matches('\'') {
        "prefer-dark" => Some(1),
        "prefer-light" => Some(2),
        "default" => Some(0),
        _ => None,
    }
}

/// Whether the system prefers dark over `conn` (the portal), else as
/// GNOME has it; `None` with no preference either way.
fn preference(conn: Option<&Connection>) -> Option<bool> {
    conn.and_then(portal_scheme)
        .and_then(prefers_dark)
        .or_else(|| gsettings_scheme().and_then(prefers_dark))
}

/// Whether the system prefers dark now; `None` with no preference.
fn current() -> Option<bool> {
    preference(Connection::new_session().ok().as_ref())
}

/// The system's preference, being read on a thread of its own.
pub struct Reading(std::thread::JoinHandle<Option<bool>>);

/// Start reading whether the system prefers dark.
pub fn read() -> Reading {
    Reading(std::thread::spawn(current))
}

impl Reading {
    /// Wait for the reading: whether the system prefers dark, `None` with
    /// no preference (or a thread that failed).
    pub fn wait(self) -> Option<bool> {
        self.0.join().ok().flatten()
    }
}

/// Follow the system's preference on a thread of its own: `send` gets
/// `first` once it is read, if it was not waited for, then the preference
/// each time the portal says it changed. Nothing more without the portal.
pub fn watch(first: Option<Reading>, send: impl Fn(bool) + Send + 'static) {
    std::thread::spawn(move || {
        if let Some(dark) = first.and_then(Reading::wait) {
            send(dark);
        }
        let conn = match Connection::new_session() {
            Ok(conn) => conn,
            Err(err) => return tracing::debug!("system theme: {err}"),
        };
        let rule = MatchRule::new_signal(SETTINGS, "SettingChanged");
        let changed = move |(namespace, key, value): (String, String, Variant<Box<dyn RefArg>>),
                            _: &Connection,
                            _: &dbus::Message| {
            if namespace == NAMESPACE
                && key == KEY
                && let Some(dark) = number(&value.0).and_then(prefers_dark)
            {
                send(dark);
            }
            true
        };
        if let Err(err) = conn.add_match(rule, changed) {
            return tracing::debug!("system theme: {err}");
        }
        loop {
            if let Err(err) = conn.process(WAIT) {
                return tracing::warn!("system theme: {err}; no longer followed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{number, parse_gsettings, prefers_dark};
    use dbus::arg::{RefArg, Variant};

    #[test]
    fn the_scheme_names_a_look_or_none() {
        assert_eq!(prefers_dark(1), Some(true));
        assert_eq!(prefers_dark(2), Some(false));
        assert_eq!(prefers_dark(0), None);
        assert_eq!(prefers_dark(7), None);
        assert_eq!(parse_gsettings("'prefer-dark'\n"), Some(1));
        assert_eq!(parse_gsettings("'prefer-light'"), Some(2));
        assert_eq!(parse_gsettings("'default'"), Some(0));
        assert_eq!(parse_gsettings(""), None);
    }

    #[test]
    fn the_value_is_read_through_its_variants() {
        let one: Box<dyn RefArg> = Box::new(1u32);
        assert_eq!(number(&*one), Some(1));
        let wrapped: Box<dyn RefArg> =
            Box::new(Variant(Box::new(Variant(2u32)) as Box<dyn RefArg>));
        assert_eq!(number(&*wrapped), Some(2));
        let text: Box<dyn RefArg> = Box::new("dark".to_owned());
        assert_eq!(number(&*text), None);
    }
}
