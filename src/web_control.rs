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
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel;

use crate::{
    Gerdr, desktop,
    feature::manual::{Area, Picked},
};

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
/// The extension's other files, by name: the page a tab opens on.
const FILES: [(&str, &str); 2] = [("open.html", OPEN_HTML), ("open.js", OPEN_JS)];

/// Files the extension had once, and its folder no longer keeps: the
/// markdown preview's page, gerdr painting it now (see `doc_page`).
const GONE: [&str; 7] = [
    "blocks.js",
    "preview.html",
    "preview.js",
    "preview.css",
    "markdown-light.css",
    "markdown-dark.css",
    "markdown-css-license",
];

/// A message to the browser bigger than this ends the connection: the
/// browser takes no more from its host.
pub(crate) const MESSAGE_MAX: usize = 1 << 20;

/// A message from the browser bigger than this ends the connection. The
/// browser sends its host up to 4 GB; a page's screenshot is the biggest
/// gerdr asks for.
const FROM_BROWSER_MAX: usize = 64 << 20;

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
    /// What the pane's tab shows, as gerdr asked for it (see
    /// `Gerdr::capture_web_pane`): a PNG as a `data:` URL, or why there is
    /// none.
    Shot {
        pane: String,
        #[serde(default)]
        data: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
    /// The area of the pane's page the user selected, as gerdr asked (see
    /// `Gerdr::select_web_pane`): its page's address, the area, the page's
    /// main elements in it, and a PNG of it as a `data:` URL, or why there
    /// is none.
    Selected {
        pane: String,
        url: String,
        #[serde(default)]
        rect: Area,
        #[serde(default)]
        elements: Vec<Picked>,
        #[serde(default)]
        data: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
    /// Selecting on the pane's page stopped with nothing selected:
    /// `cancelled`, `navigated`, or `error: …` saying why.
    Unselected { pane: String, reason: String },
    /// T3 Code's app in the pane's tab started no new thread (see
    /// `Gerdr::t3_no_thread`): what went wrong, as a short word, and what
    /// the page showed.
    NoThread {
        pane: String,
        reason: String,
        detail: String,
    },
}

impl FromBrowser {
    /// The token or boot page code of the pane it is about.
    fn pane_mut(&mut self) -> &mut String {
        match self {
            Self::Page { pane, .. }
            | Self::Closed { pane }
            | Self::Unscoped { pane, .. }
            | Self::Scoped { pane }
            | Self::Shot { pane, .. }
            | Self::Selected { pane, .. }
            | Self::Unselected { pane, .. }
            | Self::NoThread { pane, .. } => pane,
        }
    }
}

/// A model for T3 Code's draft, as its app's `ModelSelection` has it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftModel<'a> {
    instance_id: &'a str,
    model: &'a str,
}

/// What gerdr asks the extension to do with a pane's tab.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ToBrowser<'a> {
    /// Know the tab its boot page named by `code` by `pane` from now on.
    Pane {
        code: &'a str,
        pane: String,
    },
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
        /// Past the browser's cache, as Ctrl+Shift+R does.
        #[serde(rename = "bypassCache", skip_serializing_if = "std::ops::Not::not")]
        bypass_cache: bool,
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
        /// The model its draft is set to.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<DraftModel<'a>>,
    },
    /// Send back what the pane's tab shows, as a PNG.
    Capture {
        pane: String,
    },
    /// Let the user select an area of the pane's page, and send it back.
    Select {
        pane: String,
    },
    /// Stop selecting on the pane's page.
    Unselect {
        pane: String,
    },
}

/// A step a web pane's tab takes through its history, or again where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebStep {
    Back,
    Forward,
    Reload,
    /// Again where it is, past the browser's cache.
    HardReload,
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

    /// The pane `said` names, from connection `id`: by its token, or by its
    /// boot page's code (see `claim`). The connection claiming a pane by its
    /// code holds it, and is told its token to use from then on.
    fn pane_for(&mut self, id: u64, said: &str) -> Option<u32> {
        if let Some(pane) = token_pane(said) {
            return Some(pane);
        }
        let mut codes = CODES.lock().unwrap_or_else(PoisonError::into_inner);
        let claimed = claim(&mut codes, id, said)?;
        drop(codes);
        let pane = claimed
            .inspect_err(|why| tracing::warn!("web extension: {why}"))
            .ok()?;
        if self.panes.insert(pane, id) != Some(id) {
            let told = ToBrowser::Pane {
                code: said,
                pane: pane_token(pane),
            };
            if let (Ok(message), Some(stream)) =
                (serde_json::to_vec(&told), self.connections.get_mut(&id))
            {
                let _ = stream.write_all(&framed(&message));
            }
        }
        Some(pane)
    }
}

/// How the extension knows `pane`: with this gerdr's pid, so another's
/// tabs aren't taken for this one's panes, and a secret of this run, so a
/// program on the socket can't name a pane by guessing its number.
fn pane_token(pane: u32) -> String {
    format!("{}.{}.{pane}", std::process::id(), secret())
}

/// The pane `token` names, if it is this gerdr's.
fn token_pane(token: &str) -> Option<u32> {
    let (pid, rest) = token.split_once('.')?;
    let (secret_there, pane) = rest.split_once('.')?;
    (pid.parse::<u32>().ok()? == std::process::id() && secret_there == secret())
        .then(|| pane.parse().ok())
        .flatten()
}

/// This run's secret in `pane_token`.
fn secret() -> &'static str {
    static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SECRET.get_or_init(random_hex)
}

/// 128 random bits or so, in hex: from the kernel's random uuid.
fn random_hex() -> String {
    let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid").unwrap_or_default();
    let hex = uuid.trim().replace('-', "");
    if !hex.is_empty() {
        return hex;
    }
    // Keyed by the system's randomness too, should `/proc` be unreadable.
    use std::hash::{BuildHasher, Hasher};
    let half = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    format!("{:016x}{:016x}", half(), half())
}

/// How long a boot page's code waits for its tab to claim it.
const CODE_WAIT: Duration = Duration::from_secs(60);

/// The code a pane's boot page carries in place of its token, which shows
/// in the browser's command line to any program of the user's: when it
/// was made, and the connection that claimed it (see `pane_for`).
struct Code {
    code: String,
    made: Instant,
    claimer: Option<u64>,
}

/// Each pane's last boot page code, by pane.
static CODES: Mutex<std::collections::BTreeMap<u32, Code>> =
    Mutex::new(std::collections::BTreeMap::new());

/// The extension's page that opens `url` for `pane`.
pub fn boot_url(pane: u32, url: &str) -> String {
    format!(
        "chrome-extension://{EXTENSION_ID}/open.html?pane={}&url={}",
        boot_code(pane),
        percent_encoded(url)
    )
}

/// The code of `pane`'s boot page: its last while unclaimed and fresh, as
/// a command is made again before it runs, else a new one.
fn boot_code(pane: u32) -> String {
    let mut codes = CODES.lock().unwrap_or_else(PoisonError::into_inner);
    let fresh = |c: &Code| c.claimer.is_none() && c.made.elapsed() < CODE_WAIT;
    if !codes.get(&pane).is_some_and(fresh) {
        let code = Code {
            code: random_hex(),
            made: Instant::now(),
            claimer: None,
        };
        codes.insert(pane, code);
    }
    codes[&pane].code.clone()
}

/// What a boot page's code `said` by connection `id` gives: the pane, for
/// the first connection to bring it within `CODE_WAIT`, and again for it
/// alone; `Err` for another, or one too late.
fn claim(
    codes: &mut std::collections::BTreeMap<u32, Code>,
    id: u64,
    said: &str,
) -> Option<Result<u32, String>> {
    let (&pane, code) = codes.iter_mut().find(|(_, c)| c.code == said)?;
    Some(match code.claimer {
        Some(by) if by == id => Ok(pane),
        Some(_) => Err(format!(
            "pane {pane}'s code came again, from another connection"
        )),
        None if code.made.elapsed() >= CODE_WAIT => {
            Err(format!("pane {pane}'s code came too late"))
        }
        None => {
            code.claimer = Some(id);
            Ok(pane)
        }
    })
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

/// What makes the extension in `profile` a version of its own, with a
/// worker of its own name. A profile copied from another (see
/// `seed_browser_profile`) says the worker is registered, without the
/// worker's script, which isn't copied: the browser then fails to start it
/// (`DidStartWorkerFail … 5`) until its next start, and a boot page it
/// refused meanwhile stays an error page. A new version is registered
/// afresh.
fn profile_hash(profile: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    content_hash(profile.as_os_str().as_bytes())
}

/// The extension's version for its worker's `hash`: four numbers of up to
/// 16 bits.
fn version(hash: u64) -> String {
    let part = |shift: u32| (hash >> shift) & 0xffff;
    format!("{}.{}.{}.{}", part(48), part(32), part(16), part(0))
}

/// Write `contents` to `path` unless it holds them already: through a
/// temporary file, so a browser starting meanwhile never reads half of it.
fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    crate::projects::write_atomic(path, contents)
}

/// Write the extension into browser profile `profile`, with the host it
/// runs: gerdr, passing messages to `socket`. The extension's folder.
pub fn install(profile: &Path, socket: &Path) -> std::io::Result<PathBuf> {
    let dir = profile.join(EXTENSION_DIR);
    std::fs::create_dir_all(&dir)?;
    let hash = content_hash(WORKER.as_bytes()) ^ profile_hash(profile);
    let worker = format!("worker-{hash:016x}.js");
    let version_hash = extension_hash() ^ profile_hash(profile);
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
    for name in GONE {
        match std::fs::remove_file(dir.join(name)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
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
        pass_messages(
            &mut std::io::stdin().lock(),
            &mut to_gerdr,
            FROM_BROWSER_MAX,
        );
        std::process::exit(0);
    });
    // Unbuffered: the browser waits for each message whole.
    use std::os::fd::AsFd;
    let Ok(stdout) = std::io::stdout().as_fd().try_clone_to_owned() else {
        return;
    };
    pass_messages(&mut &stream, &mut std::fs::File::from(stdout), MESSAGE_MAX);
}

/// Pass the messages read from `from` on to `to`, until either ends or
/// one is bigger than `max`.
fn pass_messages(from: &mut impl Read, to: &mut impl Write, max: usize) {
    while let Some(message) = read_message(from, max) {
        if to.write_all(&framed(&message)).is_err() {
            return;
        }
    }
}

/// The next message on `stream`: its size in four bytes (in the machine's
/// order, as the browser writes it), then its JSON. `None` once the stream
/// ends or goes wrong, or the message is bigger than `max`.
fn read_message(stream: &mut impl Read, max: usize) -> Option<Vec<u8>> {
    let mut size = [0; 4];
    stream.read_exact(&mut size).ok()?;
    let size = u32::from_ne_bytes(size) as usize;
    if size > max {
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
                while let Some(message) = read_message(&mut stream, FROM_BROWSER_MAX) {
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
            Ok(_) => self.web.control.socket = Some(socket),
            Err(err) => tracing::warn!("web pages' socket {}: {err}", socket.display()),
        }
    }

    fn take_web_event(&mut self, event: Event) {
        let control = &mut self.web.control;
        // From here on a message names its pane by its token.
        let event = match event {
            Event::Message(id, mut message) => {
                let said = message.pane_mut();
                let Some(pane) = control.pane_for(id, said) else {
                    return;
                };
                *said = pane_token(pane);
                Event::Message(id, message)
            }
            event => event,
        };
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
                    self.page_closed(pane);
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
            Event::Message(_, FromBrowser::Shot { pane, data, error }) => {
                if let Some(pane) = token_pane(&pane) {
                    let shot = match (data, error) {
                        (Some(data), _) => png_of(&data),
                        (None, error) => Err(error.unwrap_or_else(|| "no picture came".into())),
                    };
                    self.web_pane_shot(pane, shot);
                }
            }
            Event::Message(
                _,
                FromBrowser::Selected {
                    pane,
                    url,
                    rect,
                    elements,
                    data,
                    error,
                },
            ) => {
                if let Some(pane) = token_pane(&pane) {
                    let shot = match (data, error) {
                        (Some(data), _) => png_of(&data),
                        (None, error) => Err(error.unwrap_or_else(|| "no picture came".into())),
                    };
                    self.page_selected(pane, url, rect, elements, shot);
                }
            }
            Event::Message(_, FromBrowser::Unselected { pane, reason }) => {
                if let Some(pane) = token_pane(&pane) {
                    self.page_unselected(pane, &reason);
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
                // Another connection's pane stays its own until it is gone.
                if control.panes.get(&pane).is_some_and(|held| *held != id) {
                    tracing::warn!("web extension: pane {pane} is another connection's");
                    return;
                }
                control.panes.insert(pane, id);
                control.history.insert(pane, WebHistory { back, forward });
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
        model: Option<&str>,
    ) -> bool {
        let model = model.and_then(crate::integrations::t3::launch::selection);
        let message = ToBrowser::NewThread {
            pane: pane_token(pane),
            origin,
            dirs: strings(dirs),
            prompt,
            model: model.map(|(instance_id, model)| DraftModel { instance_id, model }),
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
            WebStep::Reload => ToBrowser::Reload {
                pane: pane_id,
                bypass_cache: false,
            },
            WebStep::HardReload => ToBrowser::Reload {
                pane: pane_id,
                bypass_cache: true,
            },
        };
        self.send_to_browser(pane, &message)
    }

    /// Ask for what `pane`'s tab shows, if the extension follows it; whether
    /// it was asked. It comes back to `Gerdr::web_pane_shot`.
    pub(crate) fn capture_web_pane(&mut self, pane: u32) -> bool {
        let message = ToBrowser::Capture {
            pane: pane_token(pane),
        };
        self.send_to_browser(pane, &message)
    }

    /// Let the user select an area of `pane`'s page, if the extension
    /// follows its tab; whether it was asked. It comes back to
    /// `Gerdr::page_selected`, or `Gerdr::page_unselected`.
    pub(crate) fn select_web_pane(&mut self, pane: u32) -> bool {
        let message = ToBrowser::Select {
            pane: pane_token(pane),
        };
        self.send_to_browser(pane, &message)
    }

    /// Stop selecting on `pane`'s page, if the extension follows its tab;
    /// whether it was asked.
    pub(crate) fn unselect_web_pane(&mut self, pane: u32) -> bool {
        let message = ToBrowser::Unselect {
            pane: pane_token(pane),
        };
        self.send_to_browser(pane, &message)
    }

    /// Send `message` to the browser holding `pane`'s tab; whether it went.
    fn send_to_browser(&mut self, pane: u32, message: &ToBrowser) -> bool {
        let Ok(message) = serde_json::to_vec(message) else {
            return false;
        };
        self.web.send_bytes(pane, &message)
    }
}

impl crate::state::Web {
    /// Stop listening for the extension's hosts: the socket goes.
    pub fn close_web_control(&mut self) {
        if let Some(socket) = self.control.socket.take() {
            let _ = std::fs::remove_file(socket);
        }
    }
    /// Send `message`, JSON, to the browser holding `pane`'s tab; whether
    /// it went.
    fn send_bytes(&mut self, pane: u32, message: &[u8]) -> bool {
        let control = &mut self.control;
        let Some(id) = control.panes.get(&pane).copied() else {
            return false;
        };
        control
            .connections
            .get_mut(&id)
            .is_some_and(|stream| stream.write_all(&framed(message)).is_ok())
    }
}

/// The PNG in `url`, a `data:` URL of one in base64.
fn png_of(url: &str) -> Result<Vec<u8>, String> {
    let data = (url.strip_prefix("data:image/png;base64,"))
        .ok_or_else(|| "the picture isn't a PNG".to_owned())?;
    base64_decoded(data).ok_or_else(|| "the picture came broken".to_owned())
}

/// The bytes `text` holds in base64 (padded or not); `None` when it holds
/// anything else.
fn base64_decoded(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut held) = (0u32, 0);
    for &c in text {
        bits = bits << 6 | u32::from(value(c)?);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    Some(out)
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
    fn web_sends_framed_to_the_pane_s_browser_only() {
        let mut web = crate::state::Web::default();
        assert!(!web.send_bytes(3, b"{}"));
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        web.control.connections.insert(1, ours);
        web.control.panes.insert(3, 1);
        assert!(web.send_bytes(3, b"{}"));
        assert!(!web.send_bytes(4, b"{}"));
        let mut got = vec![0; framed(b"{}").len()];
        theirs.read_exact(&mut got).unwrap();
        assert_eq!(got, framed(b"{}"));
    }

    #[test]
    fn closing_web_control_takes_its_socket_away() {
        let mut web = crate::state::Web::default();
        web.close_web_control();
        let socket = std::env::temp_dir().join(format!("gerdr-web-close-{}", std::process::id()));
        std::fs::write(&socket, "").unwrap();
        web.control.socket = Some(socket.clone());
        web.close_web_control();
        assert!(!socket.exists() && web.control.socket.is_none());
    }

    #[test]
    fn a_pane_is_known_by_this_gerdrs_pid() {
        assert_eq!(token_pane(&pane_token(7)), Some(7));
        assert_eq!(token_pane("1.7"), None);
        assert_eq!(token_pane("7"), None);
        let pid = std::process::id();
        assert_eq!(token_pane(&format!("{pid}.7")), None);
        assert_eq!(token_pane(&format!("{pid}.0123.7")), None);
        assert!(secret().len() >= 32);
    }

    #[test]
    fn a_boot_page_code_names_its_pane_once() {
        let mut codes = std::collections::BTreeMap::new();
        let code = |made: Instant| Code {
            code: "c0de".into(),
            made,
            claimer: None,
        };
        codes.insert(5, code(Instant::now()));
        assert_eq!(claim(&mut codes, 1, "c0de"), Some(Ok(5)));
        // Its own connection again, before it heard the token.
        assert_eq!(claim(&mut codes, 1, "c0de"), Some(Ok(5)));
        assert!(matches!(claim(&mut codes, 2, "c0de"), Some(Err(_))));
        assert_eq!(claim(&mut codes, 2, "other"), None);
        let Some(long_ago) = Instant::now().checked_sub(CODE_WAIT) else {
            return;
        };
        codes.insert(5, code(long_ago));
        assert!(matches!(claim(&mut codes, 1, "c0de"), Some(Err(_))));
    }

    #[test]
    fn the_boot_page_carries_the_page_encoded() {
        let url = boot_url(3, "https://github.com/cli/cli/pulls?q=is:open#top");
        let pane = boot_code(3);
        assert!(
            !url.contains(&pane_token(3)),
            "its token shows in the browser's command line"
        );
        assert_eq!(
            url,
            format!(
                "chrome-extension://{EXTENSION_ID}/open.html?pane={pane}\
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
        assert_eq!(
            read_message(&mut bytes, MESSAGE_MAX).as_deref(),
            Some(&b"{}"[..])
        );
        assert_eq!(read_message(&mut bytes, MESSAGE_MAX), None);
        // One too big ends it.
        let mut bytes: &[u8] = &framed(b"{\"a\":1}");
        assert_eq!(read_message(&mut bytes, 4), None);
    }

    #[test]
    fn a_screenshot_comes_as_a_png_in_base64() {
        assert_eq!(base64_decoded("TWFu").as_deref(), Some(&b"Man"[..]));
        assert_eq!(base64_decoded("TWE=").as_deref(), Some(&b"Ma"[..]));
        assert_eq!(base64_decoded("TQ==").as_deref(), Some(&b"M"[..]));
        assert_eq!(base64_decoded("T!=="), None);
        assert_eq!(png_of("data:image/png;base64,TWFu"), Ok(b"Man".to_vec()));
        assert!(png_of("data:image/jpeg;base64,TWFu").is_err());
        let shot: FromBrowser = serde_json::from_str(
            r#"{"type":"shot","pane":"1.2","data":"data:image/png;base64,TQ=="}"#,
        )
        .unwrap();
        assert!(matches!(
            shot,
            FromBrowser::Shot {
                data: Some(_),
                error: None,
                ..
            }
        ));
        let failed: FromBrowser =
            serde_json::from_str(r#"{"type":"shot","pane":"1.2","error":"no"}"#).unwrap();
        assert!(matches!(
            failed,
            FromBrowser::Shot {
                data: None,
                error: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_selection_comes_with_its_area_and_elements() {
        let selected: FromBrowser = serde_json::from_str(
            r#"{"type":"selected","pane":"1.2","url":"http://localhost:4200/orders",
                "rect":{"x":10,"y":20,"w":300,"h":120},
                "elements":[{"tag":"table","selector":"main > table","text":"Order",
                    "rect":{"x":12,"y":22,"w":290,"h":100},"component":["OrderTable"],
                    "source":"src/OrderTable.tsx:42","styles":{"gap":"8px"}}],
                "data":"data:image/png;base64,TQ=="}"#,
        )
        .unwrap();
        let FromBrowser::Selected {
            url,
            rect,
            elements,
            data: Some(_),
            error: None,
            ..
        } = selected
        else {
            panic!("not a selection: {selected:?}");
        };
        assert_eq!(url, "http://localhost:4200/orders");
        assert_eq!(rect.w, 300);
        assert_eq!(elements[0].component, ["OrderTable"]);
        assert_eq!(elements[0].styles["gap"], "8px");
        // A page without React, and a shot that failed.
        let bare: FromBrowser = serde_json::from_str(
            r#"{"type":"selected","pane":"1.2","url":"http://x","elements":[{"tag":"div","selector":"div"}],"error":"no"}"#,
        )
        .unwrap();
        assert!(matches!(
            bare,
            FromBrowser::Selected {
                data: None,
                error: Some(_),
                ..
            }
        ));
        let stopped: FromBrowser =
            serde_json::from_str(r#"{"type":"unselected","pane":"1.2","reason":"cancelled"}"#)
                .unwrap();
        assert_eq!(
            stopped,
            FromBrowser::Unselected {
                pane: "1.2".into(),
                reason: "cancelled".into()
            }
        );
    }

    #[test]
    fn the_host_passes_messages_whole() {
        let input = [framed(b"{\"a\":1}"), framed(b"{}")].concat();
        let mut output = Vec::new();
        pass_messages(&mut &input[..], &mut output, MESSAGE_MAX);
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
            model: None,
        })
        .unwrap();
        assert_eq!(
            new_thread,
            r#"{"type":"newthread","pane":"1.2","origin":"http://app-1.localhost:3773","dirs":["/w/app"]}"#
        );
        // Left out, they arrive undefined, which `executeScript` refuses as
        // an argument: the worker passes null.
        assert!(WORKER.contains("asked.prompt ?? null, asked.model ?? null"));
        let drafted = serde_json::to_string(&ToBrowser::NewThread {
            pane: "1.2".into(),
            origin: "o",
            dirs: vec![],
            prompt: Some("p"),
            model: Some(DraftModel {
                instance_id: "cursor",
                model: "grok-4.7",
            }),
        })
        .unwrap();
        assert!(
            drafted
                .ends_with(r#""prompt":"p","model":{"instanceId":"cursor","model":"grok-4.7"}}"#)
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
            ToBrowser::Reload {
                pane: "1.2".into(),
                bypass_cache: false,
            },
            ToBrowser::Reload {
                pane: "1.2".into(),
                bypass_cache: true,
            },
        ]
        .map(|message| serde_json::to_string(&message).unwrap());
        assert_eq!(
            steps,
            [
                r#"{"type":"back","pane":"1.2"}"#,
                r#"{"type":"forward","pane":"1.2"}"#,
                r#"{"type":"reload","pane":"1.2"}"#,
                r#"{"type":"reload","pane":"1.2","bypassCache":true}"#,
            ]
        );
    }

    #[test]
    fn the_extension_is_written_with_its_worker_named_by_content() {
        let profile = std::env::temp_dir().join(format!("gerdr-web-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let dir = install(&profile, Path::new("/run/gerdr.sock")).unwrap();
        std::fs::write(dir.join("worker-old.js"), "").unwrap();
        std::fs::write(dir.join("preview.html"), "").unwrap();
        install(&profile, Path::new("/run/gerdr.sock")).unwrap();
        let place = profile_hash(&profile);
        let worker = format!("worker-{:016x}.js", content_hash(WORKER.as_bytes()) ^ place);
        let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        assert!(manifest.contains(&format!("\"service_worker\": \"{worker}\"")));
        let hash = extension_hash() ^ place;
        assert!(manifest.contains(&format!("\"version\": \"{}\"", version(hash))));
        // A copy of the profile elsewhere is another version, with
        // another worker: the copy's stale registration isn't used.
        assert_ne!(place, profile_hash(&profile.join("copy")));
        assert!(dir.join(&worker).exists());
        assert!(dir.join("open.html").exists());
        assert!(!dir.join("worker-old.js").exists());
        assert!(!dir.join("preview.html").exists());
        let host = std::fs::read_to_string(dir.join("host")).unwrap();
        assert!(host.ends_with(" --web-host /run/gerdr.sock\n"));
        let hosts = profile.join("NativeMessagingHosts/dev.gerdr.web.json");
        let hosts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(hosts).unwrap()).unwrap();
        assert_eq!(hosts["path"], dir.join("host").to_string_lossy().as_ref());
        std::fs::remove_dir_all(&profile).unwrap();
    }
}
