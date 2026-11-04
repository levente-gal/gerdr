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
//! The extension also has a page of its own, `preview.html`, that shows a
//! markdown file rendered (see `md_preview`): gerdr sends it the file's
//! HTML on load and after each change, and it sends back the links clicked
//! in it. In the file's tab (see `doc_tab`), gerdr tells it the line of the review's caret and the comments' lines
//! to mark, and it sends back the line of a block clicked in it.
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

use crate::{Gerdr, desktop, md_preview};

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
/// Which block of a preview holds a line of its file.
const BLOCKS_JS: &str = include_str!("../assets/web-extension/blocks.js");

/// The extension's other files, by name: the preview page (see
/// `md_preview`) and its styles.
const FILES: [(&str, &str); 8] = [
    ("open.html", OPEN_HTML),
    ("open.js", OPEN_JS),
    ("blocks.js", BLOCKS_JS),
    (
        "preview.html",
        include_str!("../assets/web-extension/preview.html"),
    ),
    (
        "preview.js",
        include_str!("../assets/web-extension/preview.js"),
    ),
    (
        "preview.css",
        include_str!("../assets/web-extension/preview.css"),
    ),
    (
        "markdown-light.css",
        include_str!("../assets/web-extension/markdown-light.css"),
    ),
    (
        "markdown-dark.css",
        include_str!("../assets/web-extension/markdown-dark.css"),
    ),
];

/// A message bigger than this ends the connection.
pub(crate) const MESSAGE_MAX: usize = 1 << 20;

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
    /// T3 Code's app in the pane's tab didn't take its project filter
    /// (see `Gerdr::t3_unscoped`): what went wrong, as a short word, and
    /// what the page showed.
    Unscoped {
        pane: String,
        reason: String,
        detail: String,
    },
    /// T3 Code's app in the pane's tab took its project filter.
    Scoped { pane: String },
    /// A link clicked in the pane's preview (see `md_preview`).
    Open { pane: String, url: String },
    /// The pane's preview page loaded, and shows nothing yet.
    Ready { pane: String },
    /// A block clicked in the pane's preview, from this line of its file.
    Line { pane: String, line: usize },
    /// The pane's preview scrolled to this line of its file, to the
    /// fraction, at its top (see `DiffView::top_line`).
    Scroll { pane: String, line: f64 },
    /// T3 Code's app in the pane's tab started no new thread (see
    /// `Gerdr::t3_no_thread`): what went wrong, as a short word, and what
    /// the page showed.
    NoThread {
        pane: String,
        reason: String,
        detail: String,
    },
}

/// What gerdr asks the extension to do with a pane's tab.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ToBrowser<'a> {
    Navigate {
        pane: String,
        url: &'a str,
        /// By the page's own router when the tab shows that app already,
        /// without loading it again (see `goInApp` in worker.js).
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        soft: bool,
    },
    Back {
        pane: String,
    },
    Forward {
        pane: String,
    },
    Reload {
        pane: String,
    },
    /// Keep T3 Code's app at `origin` in the pane's tab to the projects
    /// under `dirs`: the tab at another site is let be.
    Scope {
        pane: String,
        origin: &'a str,
        dirs: Vec<String>,
    },
    /// Start a new thread in T3 Code's app at `origin` in the pane's tab,
    /// in a project in `dirs`, once the tab shows it.
    NewThread {
        pane: String,
        origin: &'a str,
        dirs: Vec<String>,
        /// What its draft says, for the user to send; it is then in the
        /// project at one of `dirs` exactly, none around them.
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<&'a str>,
    },
    /// Show a markdown file in the pane's preview page (see
    /// `md_preview`): rendered as `html`, or `error` saying why it isn't.
    Preview {
        pane: String,
        title: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        html: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        dark: bool,
        /// The lines with review comments, and what each says.
        marks: &'a [(usize, String)],
    },
    /// Mark the block of the pane's preview holding this line of its
    /// file, where its review's caret is; none, no block.
    Caret {
        pane: String,
        line: Option<usize>,
    },
    /// Scroll the pane's preview to this line of its file, to the
    /// fraction, at its top: where its review beside it stands.
    Scroll {
        pane: String,
        line: f64,
    },
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
pub(crate) fn percent_encoded(text: &str) -> String {
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

/// What the extension's version follows: its worker and every other file,
/// so a changed page loads too.
fn extension_hash() -> u64 {
    let all: Vec<&str> = std::iter::once(WORKER)
        .chain(FILES.iter().map(|(_, contents)| *contents))
        .collect();
    content_hash(all.join("\0").as_bytes())
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
    let version_hash = extension_hash();
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
            &format!("\"version\": \"{}\"", version(version_hash)),
        );
    write_if_changed(&dir.join("manifest.json"), manifest.as_bytes())?;
    write_if_changed(&dir.join(&worker), WORKER.as_bytes())?;
    for (name, contents) in FILES {
        write_if_changed(&dir.join(name), contents.as_bytes())?;
    }

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
                    self.preview_closed(pane);
                }
            }
            Event::Message(_, FromBrowser::Open { pane, url }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.preview_link(pane, &url);
                }
            }
            Event::Message(_, FromBrowser::Ready { pane }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.preview_ready(pane);
                }
            }
            Event::Message(_, FromBrowser::Line { pane, line }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.preview_line(pane, line);
                }
            }
            Event::Message(_, FromBrowser::Scroll { pane, line }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.preview_scrolled(pane, line);
                }
            }
            Event::Message(
                _,
                FromBrowser::Unscoped {
                    pane,
                    reason,
                    detail,
                },
            ) => {
                if let Some(pane) = token_pane(&pane) {
                    self.t3_unscoped(pane, &reason, &detail);
                }
            }
            Event::Message(
                _,
                FromBrowser::NoThread {
                    pane,
                    reason,
                    detail,
                },
            ) => {
                if let Some(pane) = token_pane(&pane) {
                    self.t3_no_thread(pane, &reason, &detail);
                }
            }
            Event::Message(_, FromBrowser::Scoped { pane }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.t3_scoped(pane);
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
                self.preview_page(pane, &url);
                if crate::is_web_link(&url) {
                    self.t3_page(pane, &url);
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
        self.navigate(pane, url, false)
    }

    /// Send `pane`'s tab to `url` of the single-page app it shows, by the
    /// app's own router, without loading the page again: a load where the
    /// tab is at another site, the app can't be steered, or the extension
    /// is older. Whether the extension follows the tab.
    pub fn go_in_web_app(&mut self, pane: u32, url: &str) -> bool {
        self.navigate(pane, url, true)
    }

    fn navigate(&mut self, pane: u32, url: &str, soft: bool) -> bool {
        let message = ToBrowser::Navigate {
            pane: pane_token(pane),
            url,
            soft,
        };
        let sent = self.send_to_browser(pane, &message);
        if sent {
            self.set_web_page(pane, url.to_owned());
        }
        sent
    }

    /// Keep T3 Code's app at `origin` in `pane`'s tab to the projects
    /// under `dirs`, if the extension follows it; whether it was asked to.
    pub fn scope_web_pane(&mut self, pane: u32, origin: &str, dirs: &[PathBuf]) -> bool {
        let message = ToBrowser::Scope {
            pane: pane_token(pane),
            origin,
            dirs: strings(dirs),
        };
        self.send_to_browser(pane, &message)
    }

    /// Start a new thread in T3 Code's app at `origin` in `pane`'s tab, in
    /// a project under `dirs` (now, or once the tab shows the app), if the
    /// extension follows it; whether it was asked to.
    pub fn new_thread_in_web_pane(
        &mut self,
        pane: u32,
        origin: &str,
        dirs: &[PathBuf],
        prompt: Option<&str>,
    ) -> bool {
        let message = ToBrowser::NewThread {
            pane: pane_token(pane),
            origin,
            dirs: strings(dirs),
            prompt,
        };
        self.send_to_browser(pane, &message)
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

    /// Show `shown` in `pane`'s preview page, if the extension follows
    /// its tab; whether it was sent. One too big for a message says so
    /// instead.
    pub(crate) fn preview_to_browser(&mut self, pane: u32, shown: &md_preview::Shown) -> bool {
        let body = shown.body.as_deref().map_err(String::as_str);
        let message = preview_message(pane, shown, body).or_else(|| {
            let name = shown.title.rsplit(" \u{b7} ").next().unwrap_or("The file");
            let why = format!("{name} is too big to preview");
            preview_message(pane, shown, Err(&why))
        });
        message.is_some_and(|message| self.send_bytes(pane, &message))
    }

    /// Mark the block holding `line` in `pane`'s preview page, if the
    /// extension follows its tab; whether it was sent.
    pub(crate) fn caret_to_browser(&mut self, pane: u32, line: Option<usize>) -> bool {
        let message = ToBrowser::Caret {
            pane: pane_token(pane),
            line,
        };
        self.send_to_browser(pane, &message)
    }

    /// Scroll `pane`'s preview page to `line` of its file at its top, if
    /// the extension follows its tab; whether it was sent.
    pub(crate) fn scroll_to_browser(&mut self, pane: u32, line: f64) -> bool {
        let message = ToBrowser::Scroll {
            pane: pane_token(pane),
            line,
        };
        self.send_to_browser(pane, &message)
    }

    /// Send `message` to the browser holding `pane`'s tab; whether it went.
    fn send_to_browser(&mut self, pane: u32, message: &ToBrowser) -> bool {
        let Ok(message) = serde_json::to_vec(message) else {
            return false;
        };
        self.send_bytes(pane, &message)
    }

    /// Send `message`, JSON, to the browser holding `pane`'s tab; whether
    /// it went.
    fn send_bytes(&mut self, pane: u32, message: &[u8]) -> bool {
        let control = &mut self.web_control;
        let Some(id) = control.panes.get(&pane).copied() else {
            return false;
        };
        control
            .connections
            .get_mut(&id)
            .is_some_and(|stream| stream.write_all(&framed(message)).is_ok())
    }
}

/// The message showing `shown` in `pane`'s preview page, its body `body`
/// (the rendered file, or why it isn't); `None` when it is bigger than a
/// message may be.
pub(crate) fn preview_message(
    pane: u32,
    shown: &md_preview::Shown,
    body: Result<&str, &str>,
) -> Option<Vec<u8>> {
    let message = ToBrowser::Preview {
        pane: pane_token(pane),
        title: &shown.title,
        html: body.ok(),
        error: body.err(),
        dark: shown.dark,
        marks: &shown.marks,
    };
    let message = serde_json::to_vec(&message).ok()?;
    (message.len() <= MESSAGE_MAX).then_some(message)
}

/// `dirs` as the extension takes them.
fn strings(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect()
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
        let unscoped: FromBrowser = serde_json::from_str(
            r#"{"type":"unscoped","pane":"1.2","reason":"no-groups","detail":"d"}"#,
        )
        .unwrap();
        assert_eq!(
            unscoped,
            FromBrowser::Unscoped {
                pane: "1.2".into(),
                reason: "no-groups".into(),
                detail: "d".into(),
            }
        );
        let scoped: FromBrowser =
            serde_json::from_str(r#"{"type":"scoped","pane":"1.2"}"#).unwrap();
        assert_eq!(scoped, FromBrowser::Scoped { pane: "1.2".into() });
        let no_thread: FromBrowser = serde_json::from_str(
            r#"{"type":"nothread","pane":"1.2","reason":"no-match","detail":"d"}"#,
        )
        .unwrap();
        assert_eq!(
            no_thread,
            FromBrowser::NoThread {
                pane: "1.2".into(),
                reason: "no-match".into(),
                detail: "d".into(),
            }
        );
        let new_thread = serde_json::to_string(&ToBrowser::NewThread {
            pane: "1.2".into(),
            origin: "http://app-1.localhost:3773",
            dirs: vec!["/w/app".into()],
            prompt: None,
        })
        .unwrap();
        assert_eq!(
            new_thread,
            r#"{"type":"newthread","pane":"1.2","origin":"http://app-1.localhost:3773","dirs":["/w/app"]}"#
        );
        let scope = serde_json::to_string(&ToBrowser::Scope {
            pane: "1.2".into(),
            origin: "http://app-1.localhost:3773",
            dirs: vec!["/w/app".into()],
        })
        .unwrap();
        assert_eq!(
            scope,
            r#"{"type":"scope","pane":"1.2","origin":"http://app-1.localhost:3773","dirs":["/w/app"]}"#
        );
        let message = serde_json::to_string(&ToBrowser::Navigate {
            pane: "1.2".into(),
            url: "https://c.d",
            soft: false,
        })
        .unwrap();
        assert_eq!(
            message,
            r#"{"type":"navigate","pane":"1.2","url":"https://c.d"}"#
        );
        let soft = serde_json::to_string(&ToBrowser::Navigate {
            pane: "1.2".into(),
            url: "https://c.d/x",
            soft: true,
        })
        .unwrap();
        assert_eq!(
            soft,
            r#"{"type":"navigate","pane":"1.2","url":"https://c.d/x","soft":true}"#
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
        let hash = extension_hash();
        assert!(manifest.contains(&format!("\"version\": \"{}\"", version(hash))));
        assert!(dir.join(&worker).exists());
        assert!(dir.join("preview.html").exists());
        assert!(dir.join("markdown-dark.css").exists());
        assert!(!dir.join("worker-old.js").exists());
        let host = std::fs::read_to_string(dir.join("host")).unwrap();
        assert!(host.ends_with(" --web-host /run/gerdr.sock\n"));
        let hosts = profile.join("NativeMessagingHosts/dev.gerdr.web.json");
        let hosts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(hosts).unwrap()).unwrap();
        assert_eq!(hosts["path"], dir.join("host").to_string_lossy().as_ref());
        std::fs::remove_dir_all(&profile).unwrap();
    }

    #[test]
    fn a_preview_marks_the_block_of_its_reviews_caret() {
        let caret = ToBrowser::Caret {
            pane: pane_token(4),
            line: Some(12),
        };
        let caret = serde_json::to_value(&caret).unwrap();
        assert_eq!(caret["type"], "caret");
        assert_eq!(caret["line"], 12);
        let none = ToBrowser::Caret {
            pane: pane_token(4),
            line: None,
        };
        assert!(serde_json::to_value(&none).unwrap()["line"].is_null());
        // A click on a block puts the caret on its first line.
        let token = pane_token(4);
        let clicked = format!(r#"{{"type":"line","pane":"{token}","line":7}}"#);
        let FromBrowser::Line { pane, line } = serde_json::from_str(&clicked).unwrap() else {
            panic!("not a line");
        };
        assert_eq!((token_pane(&pane), line), (Some(4), 7));
    }

    #[test]
    fn a_preview_scrolls_with_its_review() {
        let to = ToBrowser::Scroll {
            pane: pane_token(4),
            line: 12.5,
        };
        let to = serde_json::to_value(&to).unwrap();
        assert_eq!(
            (to["type"].as_str(), to["line"].as_f64()),
            (Some("scroll"), Some(12.5))
        );
        let token = pane_token(4);
        let from = format!(r#"{{"type":"scroll","pane":"{token}","line":3.75}}"#);
        let FromBrowser::Scroll { pane, line } = serde_json::from_str(&from).unwrap() else {
            panic!("not a scroll");
        };
        assert_eq!((token_pane(&pane), line), (Some(4), 3.75));
    }

    /// The page's lookup of the block holding a line, run by node if it
    /// is here.
    #[test]
    fn a_line_is_in_its_innermost_block() {
        let Ok(node) = which_node() else {
            eprintln!("no node: skipped");
            return;
        };
        let check = r#"
const { sourceLines, pickBlock, lineY, yLine } = require(process.argv[1]);
const assert = require("assert");
assert.deepStrictEqual(sourceLines("3:1-5:20"), [3, 5]);
assert.strictEqual(sourceLines("x"), null);
// A list (lines 3-6) with items 3 and 4-6, a paragraph at 8.
const blocks = [[1, 1, "h1"], [3, 6, "ul"], [3, 3, "li1"], [4, 6, "li2"], [8, 9, "p"]];
assert.strictEqual(pickBlock(blocks, 1), "h1");
assert.strictEqual(pickBlock(blocks, 5), "li2");
assert.strictEqual(pickBlock(blocks, 3), "li1");
// A blank line between blocks: the one before it.
assert.strictEqual(pickBlock(blocks, 7), "li2");
assert.strictEqual(pickBlock(blocks, 2), "h1");
assert.strictEqual(pickBlock([[2, 2, "p"]], 1), null);
// A heading (line 1) at 0-40, a list (3-6) at 60-160 with items 3 and
// 4-6, a paragraph (8-9) at 180-220.
const spans = [[1, 1, 0, 40], [3, 6, 60, 100], [3, 3, 60, 25], [4, 6, 85, 75], [8, 9, 180, 40]];
assert.strictEqual(lineY(spans, 1), 0);
assert.strictEqual(lineY(spans, 1.5), 20);
assert.strictEqual(lineY(spans, 5), 85 + 25);
// Between blocks, between the end of one and the start of the next.
assert.strictEqual(lineY(spans, 2), 40);
assert.strictEqual(lineY(spans, 2.5), 50);
assert.strictEqual(lineY(spans, 7.5), 170);
assert.strictEqual(lineY(spans, 50), 220);
// Turned round, the place gives its line back.
for (const line of [1, 1.5, 2.5, 3, 5, 7.5, 8.25]) {
  assert.ok(Math.abs(yLine(spans, lineY(spans, line)) - line) < 1e-9, String(line));
}
assert.strictEqual(yLine(spans, 1000), 10);
// Above the first block, the line before it, from the page's top.
const low = [[1, 1, 32, 40], [3, 3, 100, 20]];
assert.strictEqual(lineY(low, 0), 0);
assert.strictEqual(lineY(low, 0.5), 16);
assert.strictEqual(lineY(low, -2), 0);
assert.strictEqual(yLine(low, 0), 0);
assert.strictEqual(yLine(low, 16), 0.5);
assert.strictEqual(lineY([], 3), 0);
assert.strictEqual(yLine([], 3), 1);
"#;
        let blocks = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/web-extension/blocks.js"
        );
        let out = std::process::Command::new(node)
            .args(["-e", check, blocks])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn which_node() -> Result<std::path::PathBuf, ()> {
        let home = std::env::var_os("HOME").ok_or(())?;
        let mise =
            std::path::Path::new(&home).join(".local/share/mise/installs/node/latest/bin/node");
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|dir| dir.join("node"))
            .chain([mise])
            .find(|node| node.is_file())
            .ok_or(())
    }
}
