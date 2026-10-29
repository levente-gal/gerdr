//! Following and steering the web pages gerdr shows, through an extension
//! of its own in the browser's profile (`assets/web-extension`). A web
//! page opens at the extension's `open.html` (see `boot_url`), which takes
//! the tab as the pane's and sends it on to the page. The extension tells
//! gerdr where each pane's tab is whenever that changes, and gerdr can send
//! a pane's tab to another page (see `Gerdr::navigate_web_pane`).
//!
//! They talk over native messaging: the browser runs `gerdr --web-host`
//! (see `run_host`), which passes the messages on as they are, to and from
//! gerdr's socket: each its size in four bytes, then its JSON. DevTools
//! would do too, but sites like Google's refuse to sign in a browser
//! serving it.
//!
//! Chromium keeps a command-line extension's worker registered from the
//! run before, changed or not, while the extension's version stays the
//! same; and with the version changed, while the worker's file's name
//! does. Both follow the worker's content (see `install`).
//!
//! A pane without a tab of the extension's (its browser was running
//! before, or can't run the host, as from a Flatpak) is followed by
//! `web_pages` still.

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel;

use crate::{Gerdr, desktop};

/// The extension's id, which the key in its manifest fixes.
pub const EXTENSION_ID: &str = "elaigohdbahomgbfbgmgbofdmgoejbed";

/// The folder of a browser profile the extension is written to.
const EXTENSION_DIR: &str = "gerdr-extension";

/// The name the extension knows the host by.
const HOST_NAME: &str = "dev.gerdr.web";

/// The argument gerdr runs as the host with, followed by its socket.
pub const HOST_COMMAND: &str = "--web-host";

const MANIFEST: &str = include_str!("../assets/web-extension/manifest.json");
const WORKER: &str = include_str!("../assets/web-extension/worker.js");
const OPEN_HTML: &str = include_str!("../assets/web-extension/open.html");
const OPEN_JS: &str = include_str!("../assets/web-extension/open.js");

/// A message bigger than this ends the connection.
const MESSAGE_MAX: usize = 1 << 20;

/// How often the host tries gerdr's socket again while it isn't there.
const HOST_RETRY: Duration = Duration::from_secs(1);

/// What the extension tells gerdr about a pane, by its token (see
/// `pane_token`).
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
enum FromBrowser {
    /// The pane's tab shows `url` now, and can go `back` and `forward` if
    /// it says so (nothing said where the page can't be asked).
    Page {
        pane: String,
        url: String,
        #[serde(default)]
        back: Option<bool>,
        #[serde(default)]
        forward: Option<bool>,
    },
    /// The pane's tab closed.
    Closed { pane: String },
}

/// What gerdr asks the extension to do with a pane's tab.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ToBrowser<'a> {
    Navigate { pane: String, url: &'a str },
    Back { pane: String },
    Forward { pane: String },
    Reload { pane: String },
}

/// A step a web pane's tab takes through its history, or again where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebStep {
    Back,
    Forward,
    Reload,
}

/// Whether a web pane's tab can go back and forward, as its page last said:
/// `None` for one that wasn't asked or couldn't say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebHistory {
    pub back: Option<bool>,
    pub forward: Option<bool>,
}

/// What happens on a connection to gerdr's socket, by the connection.
enum Event {
    Connected(u64, UnixStream),
    Message(u64, FromBrowser),
    Gone(u64),
}

/// The connections of the browsers running the extension, and which
/// panes' tabs each has.
#[derive(Default)]
pub struct WebControl {
    /// gerdr's socket, once it listens on it.
    socket: Option<PathBuf>,
    connections: HashMap<u64, UnixStream>,
    /// The connection of each pane whose tab the extension took, by pane.
    panes: HashMap<u32, u64>,
    /// Where each of those panes' tabs can go, by pane.
    history: HashMap<u32, WebHistory>,
}

impl WebControl {
    /// Whether the extension follows `pane`'s tab.
    pub fn follows(&self, pane: u32) -> bool {
        self.panes.contains_key(&pane)
    }

    /// Where `pane`'s tab can go, as the extension last said.
    pub fn history(&self, pane: u32) -> WebHistory {
        self.history.get(&pane).copied().unwrap_or_default()
    }

    /// gerdr's socket, once it listens on it.
    pub fn socket(&self) -> Option<&Path> {
        self.socket.as_deref()
    }
}

/// How the extension knows `pane`: with this gerdr's pid, so another's
/// tabs aren't taken for this one's panes.
fn pane_token(pane: u32) -> String {
    format!("{}.{pane}", std::process::id())
}

/// The pane `token` names, if it is this gerdr's.
fn token_pane(token: &str) -> Option<u32> {
    let (pid, pane) = token.split_once('.')?;
    (pid.parse::<u32>().ok()? == std::process::id())
        .then(|| pane.parse().ok())
        .flatten()
}

/// The extension's page that opens `url` for `pane`.
pub fn boot_url(pane: u32, url: &str) -> String {
    format!(
        "chrome-extension://{EXTENSION_ID}/open.html?pane={}&url={}",
        pane_token(pane),
        percent_encoded(url)
    )
}

/// `text` with every byte but the unreserved ones percent-encoded.
fn percent_encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A name that changes with `bytes` (FNV-1a), stable between builds.
fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The extension's version for its worker's `hash`: four numbers of up to
/// 16 bits.
fn version(hash: u64) -> String {
    let part = |shift: u32| (hash >> shift) & 0xffff;
    format!("{}.{}.{}.{}", part(48), part(32), part(16), part(0))
}

/// Write `contents` to `path` unless it holds them already.
fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    std::fs::write(path, contents)
}

/// Write the extension into browser profile `profile`, with the host it
/// runs: gerdr, passing messages to `socket`. The extension's folder.
pub fn install(profile: &Path, socket: &Path) -> std::io::Result<PathBuf> {
    let dir = profile.join(EXTENSION_DIR);
    std::fs::create_dir_all(&dir)?;
    let hash = content_hash(WORKER.as_bytes());
    let worker = format!("worker-{hash:016x}.js");
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("worker-") && name != worker {
            std::fs::remove_file(entry.path())?;
        }
    }
    let manifest = MANIFEST
        .replace("\"worker.js\"", &format!("\"{worker}\""))
        .replace(
            "\"version\": \"1\"",
            &format!("\"version\": \"{}\"", version(hash)),
        );
    write_if_changed(&dir.join("manifest.json"), manifest.as_bytes())?;
    write_if_changed(&dir.join(&worker), WORKER.as_bytes())?;
    write_if_changed(&dir.join("open.html"), OPEN_HTML.as_bytes())?;
    write_if_changed(&dir.join("open.js"), OPEN_JS.as_bytes())?;

    // A binary replaced while it runs reads as deleted.
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    let host = dir.join("host");
    let script = format!(
        "#!/bin/sh\nexec {} {HOST_COMMAND} {}\n",
        desktop::shell_quote(exe),
        desktop::shell_quote(&socket.to_string_lossy())
    );
    write_if_changed(&host, script.as_bytes())?;
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755))?;

    let hosts = profile.join("NativeMessagingHosts");
    std::fs::create_dir_all(&hosts)?;
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "gerdr",
        "path": host,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?;
    write_if_changed(&hosts.join(format!("{HOST_NAME}.json")), &manifest)?;
    Ok(dir)
}

/// Run as the native messaging host: pass each message the browser writes
/// to gerdr's `socket`, and back, until either end closes. While the
/// socket isn't there, it is tried again. Message by message, not by
/// `io::copy`: what it splices to the browser's pipe never reaches it.
pub fn run_host(socket: &Path) {
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(_) => std::thread::sleep(HOST_RETRY),
        }
    };
    let Ok(mut to_gerdr) = stream.try_clone() else {
        return;
    };
    std::thread::spawn(move || {
        pass_messages(&mut std::io::stdin().lock(), &mut to_gerdr);
        std::process::exit(0);
    });
    // Unbuffered: the browser waits for each message whole.
    use std::os::fd::AsFd;
    let Ok(stdout) = std::io::stdout().as_fd().try_clone_to_owned() else {
        return;
    };
    pass_messages(&mut &stream, &mut std::fs::File::from(stdout));
}

/// Pass the messages read from `from` on to `to`, until either ends.
fn pass_messages(from: &mut impl Read, to: &mut impl Write) {
    while let Some(message) = read_message(from) {
        if to.write_all(&framed(&message)).is_err() {
            return;
        }
    }
}

/// The next message on `stream`: its size in four bytes (in the machine's
/// order, as the browser writes it), then its JSON. `None` once the stream
/// ends or goes wrong.
fn read_message(stream: &mut impl Read) -> Option<Vec<u8>> {
    let mut size = [0; 4];
    stream.read_exact(&mut size).ok()?;
    let size = u32::from_ne_bytes(size) as usize;
    if size > MESSAGE_MAX {
        return None;
    }
    let mut message = vec![0; size];
    stream.read_exact(&mut message).ok()?;
    Some(message)
}

/// `message` as written on the socket (see `read_message`).
fn framed(message: &[u8]) -> Vec<u8> {
    let mut framed = (message.len() as u32).to_ne_bytes().to_vec();
    framed.extend_from_slice(message);
    framed
}

/// Listen on `socket`, each connection read on a thread of its own.
fn listen(socket: &Path) -> std::io::Result<channel::Channel<Event>> {
    // One left by a gerdr of the same pid, gone.
    match std::fs::remove_file(socket) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let listener = UnixListener::bind(socket)?;
    let (tx, rx) = channel::channel();
    std::thread::spawn(move || {
        for (id, stream) in (0..).zip(listener.incoming()) {
            let Ok(mut stream) = stream else {
                continue;
            };
            let Ok(writer) = stream.try_clone() else {
                continue;
            };
            if tx.send(Event::Connected(id, writer)).is_err() {
                return;
            }
            let tx = tx.clone();
            std::thread::spawn(move || {
                while let Some(message) = read_message(&mut stream) {
                    match serde_json::from_slice(&message) {
                        Ok(message) => {
                            if tx.send(Event::Message(id, message)).is_err() {
                                return;
                            }
                        }
                        Err(err) => tracing::debug!("web extension: {err}"),
                    }
                }
                let _ = tx.send(Event::Gone(id));
            });
        }
    });
    Ok(rx)
}

impl Gerdr {
    /// Listen for the extension's hosts, on a socket of this gerdr's in
    /// the runtime folder.
    pub fn watch_web_control(&mut self) {
        let socket = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("gerdr-web-{}.sock", std::process::id()));
        let inserted = listen(&socket).map_err(|e| e.to_string()).and_then(|rx| {
            self.loop_handle
                .insert_source(rx, |event, _, state| {
                    if let channel::Event::Msg(event) = event {
                        state.take_web_event(event);
                    }
                })
                .map_err(|e| e.to_string())
        });
        match inserted {
            Ok(_) => self.web_control.socket = Some(socket),
            Err(err) => tracing::warn!("web pages' socket {}: {err}", socket.display()),
        }
    }

    /// Stop listening for the extension's hosts: the socket goes.
    pub fn close_web_control(&mut self) {
        if let Some(socket) = self.web_control.socket.take() {
            let _ = std::fs::remove_file(socket);
        }
    }

    fn take_web_event(&mut self, event: Event) {
        let control = &mut self.web_control;
        match event {
            Event::Connected(id, stream) => {
                control.connections.insert(id, stream);
            }
            Event::Gone(id) => {
                control.connections.remove(&id);
                control.panes.retain(|_, connection| *connection != id);
                let panes = &control.panes;
                control.history.retain(|pane, _| panes.contains_key(pane));
            }
            Event::Message(_, FromBrowser::Closed { pane }) => {
                if let Some(pane) = token_pane(&pane) {
                    control.panes.remove(&pane);
                    control.history.remove(&pane);
                }
            }
            Event::Message(
                id,
                FromBrowser::Page {
                    pane,
                    url,
                    back,
                    forward,
                },
            ) => {
                let Some(pane) = token_pane(&pane) else {
                    return;
                };
                control.panes.insert(pane, id);
                control.history.insert(pane, WebHistory { back, forward });
                if crate::is_web_link(&url) {
                    self.set_web_page(pane, url);
                }
                self.place_booting();
            }
        }
    }

    /// Take `url` as where `pane` is now: for the tab layouts to save (see
    /// `record_tabs`) and a restart to open, while its window is on its
    /// way too.
    pub(crate) fn set_web_page(&mut self, pane: u32, url: String) {
        for tab in self.workspaces.iter_mut().flat_map(|ws| &mut ws.tabs) {
            let command = tab
                .panes
                .iter_mut()
                .find(|p| p.id == pane)
                .map(|p| &mut p.app_command)
                .or_else(|| {
                    tab.expected
                        .iter_mut()
                        .find(|e| e.pane == pane && e.pid.is_some())
                        .map(|e| &mut e.app_command)
                });
            if let Some(command) = command {
                if command.as_deref() != Some(url.as_str()) {
                    tracing::debug!("web pane {pane} now at {url}");
                    *command = Some(url);
                }
                return;
            }
        }
    }

    /// Send `pane`'s tab to `url`, if the extension follows it; whether it
    /// was asked to.
    pub fn navigate_web_pane(&mut self, pane: u32, url: &str) -> bool {
        let message = ToBrowser::Navigate {
            pane: pane_token(pane),
            url,
        };
        let sent = self.send_to_browser(pane, &message);
        if sent {
            self.set_web_page(pane, url.to_owned());
        }
        sent
    }

    /// Take `pane`'s tab a `step`, if the extension follows it; whether it
    /// was asked to. Where it lands comes back as any other page does.
    pub fn web_pane_step(&mut self, pane: u32, step: WebStep) -> bool {
        let pane_id = pane_token(pane);
        let message = match step {
            WebStep::Back => ToBrowser::Back { pane: pane_id },
            WebStep::Forward => ToBrowser::Forward { pane: pane_id },
            WebStep::Reload => ToBrowser::Reload { pane: pane_id },
        };
        self.send_to_browser(pane, &message)
    }

    /// Send `message` to the browser holding `pane`'s tab; whether it went.
    fn send_to_browser(&mut self, pane: u32, message: &ToBrowser) -> bool {
        let control = &mut self.web_control;
        let Some(id) = control.panes.get(&pane).copied() else {
            return false;
        };
        let Ok(message) = serde_json::to_vec(message) else {
            return false;
        };
        control
            .connections
            .get_mut(&id)
            .is_some_and(|stream| stream.write_all(&framed(&message)).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_is_known_by_this_gerdrs_pid() {
        assert_eq!(token_pane(&pane_token(7)), Some(7));
        assert_eq!(token_pane("1.7"), None);
        assert_eq!(token_pane("7"), None);
    }

    #[test]
    fn the_boot_page_carries_the_page_encoded() {
        let url = boot_url(3, "https://github.com/cli/cli/pulls?q=is:open#top");
        let pid = std::process::id();
        assert_eq!(
            url,
            format!(
                "chrome-extension://{EXTENSION_ID}/open.html?pane={pid}.3\
                 &url=https%3A%2F%2Fgithub.com%2Fcli%2Fcli%2Fpulls%3Fq%3Dis%3Aopen%23top"
            )
        );
    }

    #[test]
    fn the_version_holds_the_hash_in_four_parts() {
        assert_eq!(version(0x0001_0002_0003_ffff), "1.2.3.65535");
    }

    #[test]
    fn messages_are_framed_by_their_size() {
        let mut bytes: &[u8] = &framed(b"{}");
        assert_eq!(read_message(&mut bytes).as_deref(), Some(&b"{}"[..]));
        assert_eq!(read_message(&mut bytes), None);
    }

    #[test]
    fn the_host_passes_messages_whole() {
        let input = [framed(b"{\"a\":1}"), framed(b"{}")].concat();
        let mut output = Vec::new();
        pass_messages(&mut &input[..], &mut output);
        assert_eq!(output, input);
    }

    #[test]
    fn the_extensions_messages_parse() {
        let page: FromBrowser =
            serde_json::from_str(r#"{"type":"page","pane":"1.2","url":"https://a.b","title":"t"}"#)
                .unwrap();
        assert_eq!(
            page,
            FromBrowser::Page {
                pane: "1.2".into(),
                url: "https://a.b".into(),
                back: None,
                forward: None,
            }
        );
        let page: FromBrowser = serde_json::from_str(
            r#"{"type":"page","pane":"1.2","url":"https://a.b","back":true,"forward":null}"#,
        )
        .unwrap();
        assert_eq!(
            page,
            FromBrowser::Page {
                pane: "1.2".into(),
                url: "https://a.b".into(),
                back: Some(true),
                forward: None,
            }
        );
        let message = serde_json::to_string(&ToBrowser::Navigate {
            pane: "1.2".into(),
            url: "https://c.d",
        })
        .unwrap();
        assert_eq!(
            message,
            r#"{"type":"navigate","pane":"1.2","url":"https://c.d"}"#
        );
        let steps = [
            ToBrowser::Back { pane: "1.2".into() },
            ToBrowser::Forward { pane: "1.2".into() },
            ToBrowser::Reload { pane: "1.2".into() },
        ]
        .map(|message| serde_json::to_string(&message).unwrap());
        assert_eq!(
            steps,
            [
                r#"{"type":"back","pane":"1.2"}"#,
                r#"{"type":"forward","pane":"1.2"}"#,
                r#"{"type":"reload","pane":"1.2"}"#,
            ]
        );
    }

    #[test]
    fn the_extension_is_written_with_its_worker_named_by_content() {
        let profile = std::env::temp_dir().join(format!("gerdr-web-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let dir = install(&profile, Path::new("/run/gerdr.sock")).unwrap();
        std::fs::write(dir.join("worker-old.js"), "").unwrap();
        install(&profile, Path::new("/run/gerdr.sock")).unwrap();
        let worker = format!("worker-{:016x}.js", content_hash(WORKER.as_bytes()));
        let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        assert!(manifest.contains(&format!("\"service_worker\": \"{worker}\"")));
        let hash = content_hash(WORKER.as_bytes());
        assert!(manifest.contains(&format!("\"version\": \"{}\"", version(hash))));
        assert!(dir.join(&worker).exists());
        assert!(!dir.join("worker-old.js").exists());
        let host = std::fs::read_to_string(dir.join("host")).unwrap();
        assert!(host.ends_with(" --web-host /run/gerdr.sock\n"));
        let hosts = profile.join("NativeMessagingHosts/dev.gerdr.web.json");
        let hosts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(hosts).unwrap()).unwrap();
        assert_eq!(hosts["path"], dir.join("host").to_string_lossy().as_ref());
        std::fs::remove_dir_all(&profile).unwrap();
    }
}
