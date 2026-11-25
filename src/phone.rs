//! The phone view: a small web page of the open features, the stage each
//! is at, what waits for the user, their history and stage files, and the
//! last lines of their agents' panes. It only reads: the features' files on
//! disk, and herdr.
//!
//! It runs on its own (`gerdr --phone [port]`), apart from the compositor,
//! so nothing it does can hold gerdr up; a user service keeps it running
//! (see `docs/phone.md`). It listens on localhost only: `tailscale serve`
//! brings the tailnet's requests to it over HTTPS. A request naming any
//! other host is refused, so a web page can't reach it by rebinding a
//! name of its own to localhost.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use resvg::{tiny_skia, usvg};
use serde_json::{Value, json};

use crate::{
    chat::mode,
    feature::{Feature, Stage, Status, layout, setup, store},
    feature_agents::{self, Gate},
    feature_workspace::feature_on,
    herdr::{self, AgentInfo, AgentStatus, Herdr, WorkspaceInfo},
    inbox_sources, md_preview, phone_control, phone_devices,
    projects::{self, Project, WorkspaceRecord},
    stats_panel::{chat_and_screen, claude_chat},
    web_push,
};

/// The option that runs it in place of gerdr.
pub const COMMAND: &str = "--phone";
/// The port it listens on unless told another.
const PORT: u16 = 8390;
/// The most a request's head may take.
const HEAD_LIMIT: u64 = 16 * 1024;
/// How many lines of an agent's pane are sent, unless more are asked.
const PANE_LINES: usize = 400;
/// The most lines herdr reads with their colours; past that, all of the
/// pane's scrollback is read, plain.
const PANE_STYLED_MAX: usize = 1000;
/// How many lines of a setup log are sent: its end, where it failed.
const LOG_LINES: usize = 400;
/// How many history events a feature's page gets, latest first.
const HISTORY: usize = 300;
/// The most a request's body may take.
const BODY_LIMIT: u64 = 64 * 1024;
/// How long gerdr is given to answer.
const GERDR_TIME: Duration = Duration::from_secs(25);

const INDEX: &str = include_str!("../assets/phone/index.html");
const APP_JS: &str = include_str!("../assets/phone/app.js");
const SW_JS: &str = include_str!("../assets/phone/sw.js");
const APP_CSS: &str = include_str!("../assets/phone/app.css");
const ANSI_UP: &str = include_str!("../assets/phone/ansi_up.js");
const ICON: &str = include_str!("../dist/gerdr.svg");
const MANIFEST: &str = include_str!("../assets/phone/manifest.webmanifest");

/// Serve the phone view on `port` (else `PORT`) until killed.
pub fn run(port: Option<&str>) {
    use tracing_subscriber::{EnvFilter, fmt};
    fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let port = match port.map(str::parse::<u16>) {
        None => PORT,
        Some(Ok(port)) => port,
        Some(Err(_)) => {
            eprintln!("gerdr --phone: the port must be a number, like {PORT}");
            std::process::exit(2);
        }
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("Couldn't listen on 127.0.0.1:{port}: {err}");
            std::process::exit(1);
        }
    };
    let icons = Icons::new();
    phone_devices::started(port);
    tracing::info!("Phone view on http://127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let icons = icons.clone();
                std::thread::spawn(move || serve(stream, &icons));
            }
            Err(err) => tracing::warn!("Couldn't take a connection: {err}"),
        }
    }
}

/// The app's icon drawn for the home screens that want a picture.
#[derive(Clone, Default)]
struct Icons {
    touch: Vec<u8>,
    small: Vec<u8>,
    big: Vec<u8>,
}

impl Icons {
    fn new() -> Self {
        let draw = |size| {
            png(size).unwrap_or_else(|| {
                tracing::warn!("Couldn't draw the {size} px icon");
                Vec::new()
            })
        };
        Self {
            touch: draw(180),
            small: draw(192),
            big: draw(512),
        }
    }
}

fn png(size: u32) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_str(ICON, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().ok()
}

/// What a request is answered with.
#[derive(Debug, PartialEq)]
struct Reply {
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
}

impl Reply {
    fn ok(kind: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            kind,
            body: body.into(),
        }
    }

    fn json(value: &Value) -> Self {
        Self::ok("application/json", value.to_string())
    }

    fn error(status: u16, why: &str) -> Self {
        Self {
            status,
            kind: "application/json",
            body: json!({ "error": why }).to_string().into_bytes(),
        }
    }
}

fn serve(stream: TcpStream, icons: &Icons) {
    if let Err(err) = stream.set_read_timeout(Some(Duration::from_secs(10))) {
        tracing::warn!("Couldn't set a timeout on a connection: {err}");
    }
    let reply = match read_request(&stream) {
        Some(request) => {
            if !host_ok(&request.host) {
                tracing::warn!("Refused a request for host {:?}", request.host);
                Reply::error(421, "This host isn't served")
            } else if let Some(refused) = refuse(&request) {
                refused
            } else if request.method == "GET" {
                answer(&request.target, icons, &request.from)
            } else if request.method == "POST" && request.from_page {
                act(&request.target, &request.body, &request.host, &request.from)
            } else if request.method == "POST" {
                // A form of another site can't set the header: only our page does.
                Reply::error(403, "Not from the phone view")
            } else {
                Reply::error(405, "Only GET and POST")
            }
        }
        None => Reply::error(400, "Bad request"),
    };
    let reason = match reply.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        503 => "Service Unavailable",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; script-src 'self'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data:\r\n\
         Connection: close\r\n\r\n",
        reply.status,
        reply.kind,
        reply.body.len()
    );
    let mut stream = stream;
    if let Err(err) = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&reply.body))
    {
        tracing::debug!("Couldn't send a reply: {err}");
    }
}

/// A request, as far as the phone view reads it.
#[derive(Debug, Default, PartialEq)]
struct Request {
    method: String,
    target: String,
    host: String,
    /// It says it comes from the phone view's page (`X-Gerdr: phone`).
    from_page: bool,
    /// The tailnet address it comes from, as `tailscale serve` passes it:
    /// empty for one from this machine itself.
    from: String,
    /// Who its sender is signed in to the tailnet as.
    login: String,
    /// What its browser says it is.
    agent: String,
    body: Vec<u8>,
}

/// A request's head, and its body as long as `Content-Length` says.
fn read_request(stream: impl Read) -> Option<Request> {
    let mut reader = BufReader::new(stream.take(HEAD_LIMIT + BODY_LIMIT));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let mut request = Request {
        method: parts.next()?.to_owned(),
        target: parts.next()?.to_owned(),
        ..Request::default()
    };
    let mut length = 0;
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            value.clone_into(&mut request.host);
        } else if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().ok()?;
        } else if name.eq_ignore_ascii_case("x-gerdr") {
            request.from_page = value == "phone";
        } else if name.eq_ignore_ascii_case("x-forwarded-for") {
            // The last is the one `tailscale serve` added: what came before
            // is the sender's own say.
            let last = value.rsplit(',').next().unwrap_or_default();
            last.trim().clone_into(&mut request.from);
        } else if name.eq_ignore_ascii_case("tailscale-user-login") {
            value.clone_into(&mut request.login);
        } else if name.eq_ignore_ascii_case("user-agent") {
            value.clone_into(&mut request.agent);
        }
    }
    if length > BODY_LIMIT {
        return None;
    }
    request.body = vec![0; usize::try_from(length).ok()?];
    reader.read_exact(&mut request.body).ok()?;
    Some(request)
}

/// Whether a request for `host` is one to answer: this machine by itself,
/// or its name on the tailnet.
fn host_ok(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    let name = name.to_ascii_lowercase();
    name == "localhost" || name == "127.0.0.1" || name.ends_with(".ts.net")
}

/// A phone's request, written down (see `phone_devices`): refused while
/// it is blocked. One from this machine itself isn't a phone's.
fn refuse(request: &Request) -> Option<Reply> {
    if request.from.is_empty() {
        return None;
    }
    match phone_devices::seen(&request.from, &request.agent, &request.login) {
        Ok(false) => None,
        Ok(true) => Some(Reply::error(403, "This phone is blocked in gerdr")),
        Err(err) => {
            tracing::warn!("Couldn't read the phones: {err}");
            Some(Reply::error(503, "gerdr can't read its list of phones"))
        }
    }
}

fn answer(target: &str, icons: &Icons, from: &str) -> Reply {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = parse_query(query);
    let arg = |name: &str| query.get(name).map(String::as_str).unwrap_or_default();
    match path {
        "/" => Reply::ok("text/html; charset=utf-8", INDEX),
        "/app.js" => Reply::ok("text/javascript; charset=utf-8", APP_JS),
        "/app.css" => Reply::ok("text/css; charset=utf-8", APP_CSS),
        "/ansi_up.js" => Reply::ok("text/javascript; charset=utf-8", ANSI_UP),
        "/icon.svg" => Reply::ok("image/svg+xml", ICON),
        "/icon-180.png" => Reply::ok("image/png", icons.touch.clone()),
        "/icon-192.png" => Reply::ok("image/png", icons.small.clone()),
        "/icon-512.png" => Reply::ok("image/png", icons.big.clone()),
        "/manifest.webmanifest" => Reply::ok("application/manifest+json", MANIFEST),
        "/api/features" => Reply::json(&all_features()),
        "/api/new" => Reply::json(&new_offer()),
        "/api/feature" => feature_page(arg("p"), arg("s"), arg("stage").parse().ok()),
        "/api/doc" => doc(arg("p"), arg("s"), arg("f")),
        "/api/pane" => pane(arg("p"), arg("s"), arg("pane"), arg("lines")),
        "/api/commands" => commands(arg("p"), arg("s"), arg("pane")),
        "/sw.js" => Reply::ok("text/javascript; charset=utf-8", SW_JS),
        "/api/push" => match web_push::public_key() {
            Ok(key) => Reply::json(&json!({ "key": key, "on": web_push::claim(arg("e"), from) })),
            Err(why) => Reply::error(500, &why),
        },
        _ => Reply::error(404, "Nothing here"),
    }
}

/// A phone asks for notifications, or no more: `{subscription, on}`, the
/// subscription as its browser's `toJSON` gives it. Notes go out from the
/// phone view's address on `host`, for the phone at `from`.
fn push(body: &[u8], host: &str, from: &str) -> Reply {
    #[derive(serde::Deserialize)]
    struct Keys {
        p256dh: String,
        auth: String,
    }
    #[derive(serde::Deserialize)]
    struct Sub {
        endpoint: String,
        keys: Keys,
    }
    #[derive(serde::Deserialize)]
    struct Ask {
        subscription: Sub,
        on: bool,
    }
    let Ok(ask) = serde_json::from_slice::<Ask>(body) else {
        return Reply::error(400, "Bad request");
    };
    let sub = web_push::Subscription {
        endpoint: ask.subscription.endpoint,
        p256dh: ask.subscription.keys.p256dh,
        auth: ask.subscription.keys.auth,
        origin: format!("https://{host}"),
        device: from.to_owned(),
    };
    match web_push::subscribe(sub, ask.on) {
        Ok(()) => Reply::json(&json!({ "on": ask.on })),
        Err(why) => Reply::error(400, &why),
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

/// `text` with its `%XX` escapes and `+` decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A press from the page: `/api/new`, `/api/press` and `/api/send-back`
/// go on to gerdr, `/api/agent` to the agent's pane.
fn act(target: &str, body: &[u8], host: &str, from: &str) -> Reply {
    if target == "/api/push" {
        return push(body, host, from);
    }
    if target == "/api/push-test" {
        let note = web_push::Note {
            title: "gerdr".into(),
            body: "Notifications work.".into(),
            tag: "test".into(),
            url: "/".into(),
        };
        return match web_push::send_all(&note) {
            Ok(0) => Reply::error(400, "No phone asked for notifications"),
            Ok(n) => Reply::json(
                &json!({ "said": [{ "text": format!("Sent to {n}"), "level": "success" }] }),
            ),
            Err(why) => Reply::error(502, &format!("Couldn't send: {why}")),
        };
    }
    let Ok(Value::Object(mut args)) = serde_json::from_slice::<Value>(body) else {
        return Reply::error(400, "Bad request");
    };
    let mut text = |name: &str| match args.remove(name) {
        Some(Value::String(text)) => text,
        _ => String::new(),
    };
    let (project, slug) = (text("p"), text("s"));
    let op = match target {
        "/api/new" => "create",
        "/api/press" => "press",
        "/api/send-back" => "send-back",
        "/api/answer" => "answer",
        "/api/agent" => {
            let (pane, line, key) = (text("pane"), text("text"), text("key"));
            let mode = text("mode");
            if !mode.is_empty() {
                return agent_mode(&project, &slug, &pane, &mode, &text("from"));
            }
            return agent_input(&project, &slug, &pane, &line, &key);
        }
        _ => return Reply::error(404, "Nothing here"),
    };
    let mut request = json!({ "op": op, "project": project, "slug": slug });
    for (name, value) in args {
        request[name] = value;
    }
    let time = match op {
        "create" => phone_control::CREATE_TIME + Duration::from_secs(5),
        _ => GERDR_TIME,
    };
    match gerdr_within(Some(&project), &request, time) {
        Ok(answer) => Reply::json(&answer),
        Err(why) => Reply::error(502, &why),
    }
}

/// The permission the agent in `pane` of `project` asks for on gerdr's card,
/// if it waits on one; gerdr holds its asks for the phone while it reads
/// the pane. And how long its turn has run, in seconds, while it works.
/// None without a gerdr to ask.
fn pane_ask(project: &str, pane: &str) -> (Value, Value) {
    let request = json!({ "op": "ask", "project": project, "pane": pane });
    match gerdr(Some(project), &request) {
        Ok(answer) => (answer["ask"].clone(), answer["working"].clone()),
        Err(why) => {
            tracing::debug!("phone: the ask of pane {pane}: {why}");
            (Value::Null, Value::Null)
        }
    }
}

/// Ask the gerdr running here: `request` on its phone socket (see
/// `phone_control`). For `project`, the gerdr showing it first: another
/// gerdr's copy of its features is as of when that one read them. Then the
/// one changed last first.
fn gerdr(project: Option<&str>, request: &Value) -> Result<Value, String> {
    gerdr_within(project, request, GERDR_TIME)
}

/// `gerdr`, given `time` to answer.
fn gerdr_within(project: Option<&str>, request: &Value, time: Duration) -> Result<Value, String> {
    let dir = phone_control::dir().ok_or("No runtime folder to find gerdr in")?;
    let mut sockets: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("gerdr isn't running ({e})"))?
        .flatten()
        .filter(|e| phone_control::is_socket_name(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    sockets.sort_by_key(|s| std::cmp::Reverse(s.0));
    let lock = project.and_then(|id| Some(crate::project_lock::dir()?.join(format!("{id}.lock"))));
    if let Some(lock) = lock {
        // Stable: newest first still, among those that don't show it.
        sockets.sort_by_key(|(_, path)| !socket_holds(path, &lock));
    }
    for (_, path) in sockets {
        let mut stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(err) => {
                // Left by a gerdr that ended: nothing listens on it any more.
                if err.kind() == std::io::ErrorKind::ConnectionRefused
                    && let Err(err) = std::fs::remove_file(&path)
                {
                    tracing::warn!("Couldn't remove {}: {err}", path.display());
                }
                continue;
            }
        };
        stream
            .set_read_timeout(Some(time))
            .map_err(|e| e.to_string())?;
        writeln!(stream, "{request}").map_err(|e| format!("Couldn't ask gerdr: {e}"))?;
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|e| format!("gerdr didn't answer: {e}"))?;
        return serde_json::from_str(&line).map_err(|e| format!("gerdr's answer: {e}"));
    }
    Err("gerdr isn't running".into())
}

/// Whether the gerdr listening on phone socket `socket` (`phone-<pid>.sock`)
/// holds file `lock` open: shows its project.
fn socket_holds(socket: &Path, lock: &Path) -> bool {
    let name = socket.file_name().map(|n| n.to_string_lossy().into_owned());
    let pid = name
        .as_deref()
        .and_then(|n| n.strip_prefix("phone-")?.strip_suffix(".sock"));
    let Some(pid) = pid.filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) else {
        return false;
    };
    let mut fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten();
    fds.any(|fd| std::fs::read_link(fd.path()).is_ok_and(|to| to == lock))
}

/// The keys the phone may press in an agent's pane: herdr's names.
const AGENT_KEYS: [&str; 4] = ["Enter", "esc", "Up", "Down"];

/// Type `line` to an agent of feature `slug` (with no `slug`, of a
/// workspace: see `agent_of`), Enter after it, or press `key`, or type a
/// menu's number; `close` closes a feature agent's pane, as closing its
/// tab on the desktop does: a research agent that answered nothing.
fn agent_input(project: &str, slug: &str, pane: &str, line: &str, key: &str) -> Reply {
    let project = match agent_of(project, slug, pane) {
        Ok((project, _)) => project,
        Err(reply) => return reply,
    };
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    let sent = if !line.trim().is_empty() {
        // A line break would go as Enter: one line it is.
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        herdr.send_agent_line(pane, &line)
    } else if key == "close" && !slug.is_empty() {
        herdr.pane_close(pane)
    } else if AGENT_KEYS.contains(&key) {
        herdr.send_keys(pane, &[key])
    } else if key.len() == 1 && key.bytes().all(|b| b.is_ascii_digit()) {
        herdr
            .call(
                "pane.send_input",
                json!({ "pane_id": pane, "text": key, "keys": [] }),
            )
            .map(drop)
    } else {
        return Reply::error(400, "Nothing to send");
    };
    match sent {
        Ok(()) => Reply::json(&json!({ "said": [] })),
        Err(err) => Reply::error(502, &format!("Couldn't reach the agent: {err}")),
    }
}

/// Switch the agent in `pane` to mode `want`, as the desktop chat does
/// (`chat::mode::switch`); `from` is the mode the phone showed. Answers
/// once its screen shows it, or why not.
fn agent_mode(project: &str, slug: &str, pane: &str, want: &str, from: &str) -> Reply {
    let project = match agent_of(project, slug, pane) {
        Ok((project, _)) => project,
        Err(reply) => return reply,
    };
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    let agent = match herdr.agent_list() {
        Ok(agents) => agents.into_iter().find(|a| a.pane_id == pane),
        Err(err) => return Reply::error(502, &format!("Couldn't reach the agent: {err}")),
    };
    let Some(kind) = agent.as_ref().and_then(crate::chat::source::kind) else {
        return Reply::error(404, "No agent runs there any more");
    };
    // The key would answer its menu.
    let waits = agent.as_ref().is_some_and(|a| a.agent_status == AgentStatus::Blocked);
    let went = match waits {
        true => Err("the agent waits on an answer first".to_owned()),
        false => {
            let mut agent = mode::InPane { herdr: &mut herdr, pane, kind };
            let n = mode::modes(kind).len();
            let was = Some(from).filter(|f| !f.is_empty());
            mode::switch(&mut agent, n, was, want, mode::READ_EVERY)
        }
    };
    match went {
        Ok(()) => Reply::json(&json!({ "said": [] })),
        Err(why) => Reply::error(409, &format!("Couldn't switch the agent to {want}: {why}")),
    }
}

/// What a new feature can be started on: each project with a workspace
/// folder, its repositories as the new feature dialog finds them, and the
/// agents gerdr may run its Discover on.
fn new_offer() -> Value {
    let all = projects();
    let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
    let (mut agents, error) = match gerdr(None, &json!({ "op": "new-offer", "projects": ids })) {
        Ok(answer) => (answer, None),
        Err(why) => (Value::Null, Some(why)),
    };
    let projects: Vec<Value> = (all.iter())
        .map(|p| {
            let mut entry = json!({ "id": p.id, "name": p.name });
            let (dirs, ignore, bases) = crate::features::root_search(p);
            let found = match dirs.first() {
                None => Err("It has no workspace folder".to_owned()),
                Some(from) => layout::find_project_root(from, &dirs, &[], &ignore, &bases),
            };
            match found {
                Ok(root) => {
                    entry["multi"] = root.multi.into();
                    let names: Vec<&str> = root.repos.iter().map(|r| r.name.as_str()).collect();
                    entry["repos"] = names.into();
                }
                Err(why) => entry["error"] = why.into(),
            }
            entry["agents"] = agents["agents"][&p.id].take();
            entry
        })
        .collect();
    json!({ "projects": projects, "error": error })
}

/// The projects gerdr keeps, as saved.
fn projects() -> Vec<Project> {
    projects::Store::load().0.projects
}

/// How herdr says each agent of a project's session is doing, by pane; an
/// error when its session can't be asked (gerdr's not running it, say).
fn agent_states(session: &str) -> Result<HashMap<String, AgentStatus>, String> {
    let mut herdr = Herdr::on_socket(herdr::socket_path(session));
    Ok(states_of(&herdr.agent_list()?))
}

fn states_of(agents: &[AgentInfo]) -> HashMap<String, AgentStatus> {
    (agents.iter())
        .map(|a| (a.pane_id.clone(), a.agent_status))
        .collect()
}

/// Every agent of a project's session, and its workspaces; an error when
/// its session can't be asked.
fn session_agents(session: &str) -> Result<(Vec<AgentInfo>, Vec<WorkspaceInfo>), String> {
    let mut herdr = Herdr::on_socket(herdr::socket_path(session));
    Ok((herdr.agent_list()?, herdr.workspace_list()?))
}

/// The agents of `agents` that aren't `features`' (a project's): in no
/// feature's workspace, nor one of a feature's own panes. Each with the
/// name of its workspace, if herdr has it. A workspace's folder is the
/// project's record of it (`records`), else the one gerdr gave herdr: a
/// restored herdr server keeps no metadata.
fn workspace_agents<'a>(
    agents: &'a [AgentInfo],
    workspaces: &'a [WorkspaceInfo],
    records: &[WorkspaceRecord],
    features: &[Feature],
) -> Vec<(&'a AgentInfo, Option<&'a str>)> {
    let feature_pane =
        |pane: &str| (features.iter()).any(|f| agent_panes(f).any(|(_, p, _)| p == pane));
    (agents.iter())
        .filter(|a| !feature_pane(&a.pane_id))
        .filter_map(|a| {
            let ws = workspaces.iter().find(|w| w.workspace_id == a.workspace_id);
            let record = |w: &WorkspaceInfo| {
                let r = records.iter().find(|r| r.mirrors(&w.workspace_id));
                r.map(|r| r.dir.clone())
            };
            let dir = ws.and_then(|w| record(w).or_else(|| w.dir()));
            match dir.is_some_and(|d| feature_on(features, &d).is_some()) {
                true => None,
                false => Some((a, ws.map(|w| w.label.as_str()))),
            }
        })
        .collect()
}

/// What the phone calls a workspace's agent: its workspace, then its name.
fn workspace_agent_label(agent: &AgentInfo, workspace: Option<&str>) -> String {
    match workspace {
        Some(w) => format!("{w} · {}", agent.label()),
        None => agent.label().to_owned(),
    }
}

/// The agent in `pane` of `project` the phone may read and type to, and
/// what to call it: one of feature `slug`'s agents; with no `slug`, any
/// agent herdr runs in the project's session. Never a plain shell.
fn agent_of(project: &str, slug: &str, pane: &str) -> Result<(Project, String), Reply> {
    if !slug.is_empty() {
        let (project, f) = find(project, slug)?;
        let found = agent_panes(&f).find(|(_, p, _)| *p == pane);
        let (label, _, _) =
            found.ok_or_else(|| Reply::error(404, "Not one of this feature's agents"))?;
        return Ok((project, label));
    }
    let project = project_of(project)?;
    let (agents, workspaces) = session_agents(&project.session)
        .map_err(|err| Reply::error(502, &format!("Couldn't ask herdr: {err}")))?;
    let agent = (agents.iter().find(|a| a.pane_id == pane))
        .ok_or_else(|| Reply::error(404, "No such agent"))?;
    let ws = workspaces
        .iter()
        .find(|w| w.workspace_id == agent.workspace_id);
    let label = workspace_agent_label(agent, ws.map(|w| w.label.as_str()));
    Ok((project, label))
}

/// Every open feature of every project, the ones waiting for the user
/// first; and each project's agents in workspaces of no feature.
fn all_features() -> Value {
    let mut features = Vec::new();
    let mut others = Vec::new();
    let mut problems = Vec::new();
    for project in projects() {
        let all = match store::load_all(None, &project.id) {
            Ok(all) => all,
            Err(err) => {
                tracing::warn!("Couldn't read {}'s features: {err}", project.name);
                problems.push(format!("Couldn't read {}'s features: {err}", project.name));
                continue;
            }
        };
        let session = session_agents(&project.session);
        if let Err(err) = &session {
            // Normal for a project gerdr isn't running: said on its cards.
            tracing::debug!("No agents of {}: {err}", project.name);
        }
        let states = session
            .as_ref()
            .map(|(a, _)| states_of(a))
            .map_err(Clone::clone);
        let open = all.iter().filter(|f| f.status == Status::Open);
        features.extend(open.map(|f| summary(&project, f, &states)));
        let Ok((agents, workspaces)) = &session else {
            continue;
        };
        let found = workspace_agents(agents, workspaces, &project.workspaces, &all);
        let theirs: Vec<Value> = (found.into_iter())
            .map(|(a, ws)| {
                json!({
                    "pane": a.pane_id,
                    "label": workspace_agent_label(a, ws),
                    "title": a.shown_title(),
                    "status": status_word(a.agent_status),
                })
            })
            .collect();
        if !theirs.is_empty() {
            let entry =
                json!({ "project": project.id, "projectName": project.name, "agents": theirs });
            others.push(entry);
        }
    }
    features.sort_by_key(|f| {
        let waits = f["waits"]["since"].as_u64();
        let updated = f["updated"].as_u64().unwrap_or(0);
        (waits.is_none(), waits.unwrap_or(0), u64::MAX - updated)
    });
    add_cards(&mut features);
    json!({
        "features": features,
        "agents": others,
        "problems": problems,
        "now": crate::clock::now_secs(),
    })
}

/// Put on each of `features` its sidebar card as gerdr draws it (marks,
/// line, tone); none when gerdr can't be asked, and the stages alone show.
/// Each project's are asked of the gerdr showing it, if one does.
fn add_cards(features: &mut [Value]) {
    let mut by_project: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for f in features.iter() {
        let project = f["project"].as_str().unwrap_or("").to_owned();
        (by_project.entry(project).or_default()).push(json!([f["project"], f["slug"]]));
    }
    let mut cards = serde_json::Map::new();
    for (project, asked) in by_project {
        match gerdr(Some(&project), &json!({ "op": "cards", "features": asked })) {
            Ok(mut answer) => {
                if let Value::Object(got) = answer["cards"].take() {
                    cards.extend(got);
                }
            }
            Err(why) => {
                // The page says gerdr isn't there by the actions it can't offer.
                tracing::debug!("No cards of {project} from gerdr: {why}");
            }
        }
    }
    for f in features {
        let key = format!(
            "{}/{}",
            f["project"].as_str().unwrap_or(""),
            f["slug"].as_str().unwrap_or("")
        );
        f["card"] = cards.remove(&key).unwrap_or(Value::Null);
    }
}

/// What the list shows of feature `f`.
/// `states`: how herdr says the project's agents do, or why it couldn't.
fn summary(
    project: &Project,
    f: &Feature,
    states: &Result<HashMap<String, AgentStatus>, String>,
) -> Value {
    let current = f.current();
    let stages: Vec<Value> = Stage::ALL
        .into_iter()
        .map(|s| json!({ "label": s.label(), "done": f.is_done(s), "now": Some(s) == current }))
        .collect();
    // shortcut: no manual test counts here (they need the criteria read),
    // so a Manual test gate isn't shown; read them if that one is missed.
    let waits = feature_agents::gate_of(f, |_| None).map(|(stage, since, kind)| {
        let gate = Gate {
            project: project.id.clone(),
            slug: f.slug.clone(),
            title: f.title.clone(),
            stage,
            since,
            kind,
        };
        let (title, next, _) = inbox_sources::gate_words(&gate);
        json!({ "title": title, "next": next, "since": since })
    });
    let last = f.history.last();
    json!({
        "project": project.id,
        "projectName": project.name,
        "slug": f.slug,
        "title": f.title,
        "stage": current.map_or("Shipped", Stage::label),
        "stages": stages,
        "waits": waits,
        "agents": states.as_ref().map(|s| agents(f, s)).unwrap_or_default(),
        "agentsError": states.as_ref().err().map(|_| "Couldn't ask herdr about its agents"),
        "updated": last.map_or(f.created, |e| e.at),
        "last": last.map(|e| e.what.clone()),
    })
}

/// Feature `f`'s agents with a pane on this machine: their names, panes and
/// when they started.
fn agent_panes(f: &Feature) -> impl Iterator<Item = (String, &str, u64)> {
    let stage_runs = f
        .runs
        .iter()
        .filter(|(_, run)| run.machine.is_none())
        .filter_map(|(stage, run)| {
            Some((
                format!("{} · {}", stage.label(), run.agent),
                run.pane.as_deref()?,
                run.started,
            ))
        });
    let own = f
        .own_agents
        .iter()
        .filter_map(|run| Some((run.agent.clone(), run.pane.as_deref()?, run.started)));
    let research = f.research.iter().filter_map(|r| {
        let label = format!("Research R{} · {}", r.id, r.agent);
        Some((label, r.pane.as_deref()?, r.started.unwrap_or_default()))
    });
    stage_runs.chain(own).chain(research)
}

/// Feature `f`'s agents still in a pane here, with how herdr says they do.
fn agents(f: &Feature, states: &HashMap<String, AgentStatus>) -> Vec<Value> {
    agent_panes(f)
        .filter_map(|(label, pane, started)| {
            let status = states.get(pane)?;
            Some(json!({
                "label": label,
                "pane": pane,
                "status": status_word(*status),
                "started": started,
            }))
        })
        .collect()
}

fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "waiting for you",
        AgentStatus::Idle | AgentStatus::Done => "idle",
        AgentStatus::Unknown => "unknown",
    }
}

fn project_of(id: &str) -> Result<Project, Reply> {
    (projects().into_iter().find(|p| p.id == id))
        .ok_or_else(|| Reply::error(404, "No such project"))
}

fn find(project: &str, slug: &str) -> Result<(Project, Feature), Reply> {
    let project = project_of(project)?;
    let feature = store::read_at(&store::dir(None, &project.id, slug))
        .map_err(|err| Reply::error(404, &format!("Couldn't read the feature: {err}")))?;
    Ok((project, feature))
}

/// A feature's page: what the list shows, its history, and the page of
/// its `stage` (by its place in `Stage::ALL`, else the one gerdr shows) as
/// gerdr's feature tab says it, with that stage's files.
fn feature_page(project: &str, slug: &str, stage: Option<usize>) -> Reply {
    let (project, f) = match find(project, slug) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    let states = agent_states(&project.session);
    let mut page = summary(&project, &f, &states);
    add_cards(std::slice::from_mut(&mut page));
    let history: Vec<Value> = f
        .history
        .iter()
        .rev()
        .take(HISTORY)
        .map(|e| json!({ "at": e.at, "what": e.what }))
        .collect();
    page["history"] = history.into();
    let request = json!({ "op": "offer", "project": project.id, "slug": slug, "stage": stage });
    let mut offer =
        gerdr(Some(&project.id), &request).unwrap_or_else(|why| json!({ "error": why }));
    // Without gerdr, the stage alone, and its files.
    let mut shown = offer["page"].take();
    if !shown.is_object() {
        let at = f.current().unwrap_or(Stage::Ship);
        let stage = stage.and_then(|i| Stage::ALL.get(i).copied()).unwrap_or(at);
        shown = json!({
            "stage": Stage::ALL.iter().position(|s| *s == stage),
            "label": stage.label(),
            "own": stage == at,
            "sections": [],
        });
    }
    add_files(
        &mut shown,
        &store::dir(None, &project.id, slug),
        fix_plan_waits(&f),
    );
    page["page"] = shown;
    page["offer"] = offer;
    page["now"] = crate::clock::now_secs().into();
    Reply::json(&page)
}

/// A stage's files, the one to read first first. Those a fix round or a
/// merge writes go with Build's and Ship's.
fn stage_files(stage: Stage) -> &'static [&'static str] {
    match stage {
        Stage::Discover => &["brief.md", "decisions.md", "idea.md"],
        Stage::Plan => &["plan.md"],
        Stage::PlanReview => &["plan-final.md"],
        Stage::Build => &["build.md", "fix-plan.md"],
        Stage::Verify => &["evidence.md", "fix-plan.md"],
        Stage::AiReview => &["fix-plan.md"],
        Stage::ManualTest => &["evidence.md", "fix-plan.md"],
        Stage::CodeReview => &["fix-plan.md"],
        Stage::Ship => &["summary.md"],
    }
}

/// The stage file `name` goes with, if any: research's with Discover's,
/// a merge's conflicts with Ship's.
fn file_stage(name: &str) -> Option<Stage> {
    if name.starts_with("research") {
        return Some(Stage::Discover);
    }
    if name.starts_with("conflicts-") {
        return Some(Stage::Ship);
    }
    Stage::ALL
        .into_iter()
        .find(|s| stage_files(*s).contains(&name))
}

/// The files shown folded under Latest files, not as a page's doc: what
/// a check or a fix round left, the fix plan first.
const FOLDED: [&str; 2] = [crate::feature::fix_round::FILE, "evidence.md"];

/// Whether `f`'s fix plan waits for the user: written, not yet made.
fn fix_plan_waits(f: &Feature) -> bool {
    f.fix_round.as_ref().is_some_and(|r| {
        !r.fixes_stopped() && r.phase == crate::feature::fix_round::FixPhase::Ready
    })
}

/// Put the files of stage page `page` on it: the one to read in its doc
/// section, rendered, where gerdr's page has one; else the stage's first,
/// after all else, unless the page already says what it holds (Build's
/// milestones, Ship's recap). The others go under Latest files: the fix
/// plan and evidence there rendered, folded, the plan open while it
/// `waits`; the rest named, to open: the stage's others and those of no
/// stage.
fn add_files(page: &mut Value, dir: &Path, waits: bool) {
    let stage = (page["stage"].as_u64()).and_then(|i| Stage::ALL.get(i as usize).copied());
    let names = docs(dir);
    let mine_too = |n: &str| stage.is_some_and(|s| stage_files(s).contains(&n));
    let mut mine: Vec<&String> = names
        .iter()
        .filter(|n| {
            stage.is_some_and(|s| file_stage(n) == Some(s))
                || file_stage(n).is_none()
                || mine_too(n)
        })
        .collect();
    let order = |n: &str| (stage.map(stage_files).unwrap_or_default().iter()).position(|x| *x == n);
    let first = |n: &str| (FOLDED.iter().position(|x| *x == n)).unwrap_or(usize::MAX);
    mine.sort_by_key(|n| (first(n), order(n).unwrap_or(usize::MAX)));
    let Some(sections) = page["sections"].as_array_mut() else {
        return;
    };
    let kind = |s: &Value, k: &str| s["kind"].as_str() == Some(k);
    let folded = |s: &Value| FOLDED.contains(&s["name"].as_str().unwrap_or_default());
    // Code review's fix plan goes with the files, as on the other pages.
    sections.retain(|s| !(kind(s, "doc") && folded(s)));
    // Verify's and AI review's files are what a check or a fix round left:
    // under Latest files, not shown as the page's.
    let said = matches!(stage, Some(Stage::Verify | Stage::AiReview))
        || sections
            .iter()
            .any(|s| kind(s, "milestones") || kind(s, "recap"));
    if !said && !sections.iter().any(|s| kind(s, "doc")) {
        let first = (mine.iter()).find(|n| order(n).is_some() && !FOLDED.contains(&n.as_str()));
        sections.extend(first.map(|n| json!({ "kind": "doc", "name": n })));
    }
    let read_into = |s: &mut Value, name: &str| {
        let text = (doc_name_ok(name))
            .then(|| std::fs::read_to_string(dir.join(name)).ok())
            .flatten();
        match text {
            Some(text) => s["html"] = md_preview::render(&text).into(),
            None => s["error"] = format!("Couldn't read {name}").into(),
        }
    };
    let mut read = Vec::new();
    for s in sections.iter_mut().filter(|s| kind(s, "doc")) {
        let name = s["name"].as_str().unwrap_or_default().to_owned();
        read_into(s, &name);
        read.push(name);
    }
    mine.retain(|n| !read.contains(n));
    let mut files: Vec<Value> = (mine.into_iter())
        .map(|n| {
            let mut file = json!({ "name": n });
            if FOLDED.contains(&n.as_str()) {
                read_into(&mut file, n);
                file["open"] = (waits && n == crate::feature::fix_round::FILE).into();
            }
            file
        })
        .collect();
    // What setup printed, where a failed one is set up again.
    if matches!(stage, Some(Stage::Build | Stage::Verify)) && dir.join(setup::LOG).is_file() {
        files.push(json!({ "name": setup::LOG }));
    }
    page["files"] = files.into();
}

/// The markdown files in a feature's folder, by name: not the copies kept
/// of a stage's file as written and as approved.
fn docs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| doc_name_ok(n))
        .filter(|n| !n.ends_with("-written.md") && !n.ends_with("-approved.md"))
        .collect();
    names.sort();
    names
}

/// Whether `name` is a markdown file name, with no way out of the folder.
fn doc_name_ok(name: &str) -> bool {
    name.ends_with(".md")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The last lines of a log, as a block of code to render.
fn log_block(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let last = lines[lines.len().saturating_sub(LOG_LINES)..].join("\n");
    // A fence longer than any run of backticks in it.
    let mut fence = "```".to_owned();
    while last.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}\n{last}\n{fence}\n")
}

/// Markdown file `name` of a feature, rendered; its setup log, as code.
fn doc(project: &str, slug: &str, name: &str) -> Reply {
    if !doc_name_ok(name) && name != setup::LOG {
        return Reply::error(404, "No such file");
    }
    let (project, _) = match find(project, slug) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    match std::fs::read_to_string(store::dir(None, &project.id, slug).join(name)) {
        Ok(text) if name == setup::LOG => {
            Reply::json(&json!({ "html": md_preview::render(&log_block(&text)) }))
        }
        Ok(text) => Reply::json(&json!({ "html": md_preview::render(&text) })),
        Err(err) => Reply::error(404, &format!("Couldn't read {name}: {err}")),
    }
}

/// The last lines of an agent's pane, with their colours: `lines` of them
/// (`PANE_LINES` when not a number), or `all` its scrollback, plain. Only
/// a pane an agent runs in (see `agent_of`): not the user's own shells.
/// `more`: its scrollback holds more than was sent. A Claude Code pane on
/// the alternate screen keeps no scrollback: read deeper, it gives the
/// end of its chat's transcript, then its screen.
/// The slash commands the agent in `pane` takes (as `agent_of` finds it), for the
/// reply box's `/` menu: `[{name, about, hint}]`, none when herdr finds no
/// agent there.
fn commands(project: &str, slug: &str, pane: &str) -> Reply {
    let (project, _) = match agent_of(project, slug, pane) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    let agents = match herdr.agent_list() {
        Ok(agents) => agents,
        Err(err) => return Reply::error(502, &format!("Couldn't ask herdr: {err}")),
    };
    let listed = (agents.iter().find(|a| a.pane_id == pane))
        .and_then(|a| {
            let kind = crate::chat::source::kind(a)?;
            Some(crate::chat::commands::here(kind, a.cwd.as_deref()?))
        })
        .unwrap_or_default();
    let listed: Vec<Value> = (listed.iter())
        .map(|c| json!({ "name": c.name, "about": c.about, "hint": c.hint }))
        .collect();
    Reply::json(&Value::Array(listed))
}

fn pane(project: &str, slug: &str, pane: &str, lines: &str) -> Reply {
    let (project, label) = match agent_of(project, slug, pane) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    // Its title, for the page's head, and its modes.
    let agent = (herdr.agent_list().ok())
        .and_then(|agents| agents.into_iter().find(|a| a.pane_id == pane));
    let title = agent.as_ref().and_then(|a| a.shown_title());
    let kind = (agent.as_ref().and_then(crate::chat::source::kind))
        .filter(|kind| !mode::modes(*kind).is_empty());
    // The modes it offers and the one `screen` (its last rows) shows.
    let modes = |screen: &str| match kind {
        Some(kind) => {
            let now = mode::shown_in(kind, screen);
            (json!(mode::offered(kind, now.as_deref())), json!(now))
        }
        None => (json!([]), Value::Null),
    };
    let all = lines == "all";
    let want = lines
        .parse()
        .unwrap_or(PANE_LINES)
        .clamp(1, PANE_STYLED_MAX);
    let rows = herdr.pane_scrollback_rows(pane).ok();
    let chat = (rows == Some(0))
        .then(|| claude_chat(&mut herdr, pane))
        .flatten();
    if let Some(path) = &chat
        && (all || want > PANE_LINES)
    {
        return match herdr.pane_read(pane, "visible", PANE_LINES) {
            Ok(screen) => {
                let text = chat_and_screen(path, &screen);
                let (ask, working) = pane_ask(&project.id, pane);
                let (modes, mode) = modes(&screen);
                let reply = json!({ "label": label, "title": title, "text": text, "more": false, "ask": ask, "working": working, "modes": modes, "mode": mode });
                Reply::json(&reply)
            }
            Err(err) => Reply::error(502, &format!("Couldn't read the pane: {err}")),
        };
    }
    let text = match all {
        true => herdr.pane_scrollback(pane).map(|(text, _)| text),
        false => herdr.pane_styled(pane, want),
    };
    // An app on the alternate screen has no scrollback: its screen.
    let text = text.and_then(|text| match text.trim().is_empty() {
        true => herdr.pane_read(pane, "visible", PANE_LINES),
        false => Ok(text),
    });
    match text {
        Ok(text) => {
            // The rows above its screen, wrapped: at worst one more read, of all.
            let more = chat.is_some()
                || (!all && rows.is_some_and(|rows| rows > text.lines().count() as u64));
            let (ask, working) = pane_ask(&project.id, pane);
            let (modes, mode) = modes(&text);
            let reply = json!({ "label": label, "title": title, "text": text, "more": more, "ask": ask, "working": working, "modes": modes, "mode": mode });
            Reply::json(&reply)
        }
        Err(err) => Reply::error(502, &format!("Couldn't read the pane: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_holds_the_lock_its_gerdr_has_open() {
        let dir = std::env::temp_dir().join(format!("gerdr-socket-holds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let lock = dir.join("p.lock");
        let _open = std::fs::File::create(&lock).expect("lock");
        let mine = dir.join(format!("phone-{}.sock", std::process::id()));
        assert!(socket_holds(&mine, &lock));
        assert!(!socket_holds(&mine, &dir.join("q.lock")));
        assert!(!socket_holds(&dir.join("phone-.sock"), &lock));
        assert!(!socket_holds(&dir.join("phone-../1.sock"), &lock));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_this_machine_and_its_tailnet_name_are_served() {
        assert!(host_ok("127.0.0.1:8390"));
        assert!(host_ok("localhost"));
        assert!(host_ok("desk.tail1234.ts.net"));
        assert!(host_ok("Desk.Tail1234.TS.NET:443"));
        assert!(!host_ok("evil.example"));
        assert!(!host_ok("ts.net.evil.example"));
        assert!(!host_ok(""));
    }

    #[test]
    fn the_sender_is_the_address_tailscale_added_last() {
        let head = "GET / HTTP/1.1\r\nHost: x.ts.net\r\nX-Forwarded-For: 1.2.3.4, 100.64.0.7\r\n\
                    Tailscale-User-Login: me@example.com\r\nUser-Agent: Pixel\r\n\r\n";
        let request = read_request(head.as_bytes()).expect("request");
        assert_eq!(request.from, "100.64.0.7");
        assert_eq!(request.login, "me@example.com");
        assert_eq!(request.agent, "Pixel");
        let local = read_request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".as_bytes());
        assert_eq!(local.expect("request").from, "");
    }

    #[test]
    fn a_request_gives_method_target_host_and_body() {
        let head = "GET /api/feature?p=a&s=b HTTP/1.1\r\nhost: x.ts.net\r\nAccept: */*\r\n\r\n";
        let request = read_request(head.as_bytes()).expect("request");
        assert_eq!(
            (
                request.method.as_str(),
                request.target.as_str(),
                request.host.as_str()
            ),
            ("GET", "/api/feature?p=a&s=b", "x.ts.net")
        );
        assert!(!request.from_page && request.body.is_empty());
        assert_eq!(read_request("GET / HTTP/1.1\r\nHost: a".as_bytes()), None);
        let post = "POST /api/press HTTP/1.1\r\nHost: a\r\nX-Gerdr: phone\r\nContent-Length: 7\r\n\r\n{\"p\":1}";
        let request = read_request(post.as_bytes()).expect("post");
        assert!(request.from_page);
        assert_eq!(request.body, b"{\"p\":1}");
        let big = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            BODY_LIMIT + 1
        );
        assert_eq!(read_request(big.as_bytes()), None);
    }

    #[test]
    fn a_press_needs_a_body_and_a_known_route() {
        assert_eq!(act("/api/press", b"nope", "localhost", "").status, 400);
        assert_eq!(act("/api/nope", b"{}", "localhost", "").status, 404);
        assert_eq!(
            act("/api/agent", br#"{"p":"no-such","s":"x"}"#, "localhost", "").status,
            404
        );
        // A workspace's agent: no feature named.
        assert_eq!(
            act("/api/agent", br#"{"p":"no-such","s":""}"#, "localhost", "").status,
            404
        );
    }

    #[test]
    fn workspace_agents_are_those_of_no_feature() {
        let agent = |pane: &str, ws: &str| -> AgentInfo {
            serde_json::from_value(json!({
                "pane_id": pane, "terminal_id": "t", "workspace_id": ws, "tab_id": "x",
                "agent": "claude", "agent_status": "working",
            }))
            .expect("agent")
        };
        let workspace = |id: &str, label: &str, dir: &str| -> WorkspaceInfo {
            serde_json::from_value(json!({
                "workspace_id": id, "label": label, "tokens": { "gerdr_kind": "terminal", "gerdr_dir_0": dir },
            }))
            .expect("workspace")
        };
        let mut f = crate::feature::tests::feature("f", 1);
        f.dir = "/work/.features/f".into();
        let run = crate::feature::agents::StageRun {
            pane: Some("w3:p1".into()),
            ..Default::default()
        };
        f.runs.insert(Stage::Build, run);
        let workspaces = [
            workspace("w1", "f", "/work/.features/f"),
            workspace("w2", "api", "/work/api"),
            // Restored: no metadata, only the project's record.
            serde_json::from_value(json!({ "workspace_id": "w5", "label": "f" })).expect("ws"),
        ];
        let records: Vec<WorkspaceRecord> = serde_json::from_value(json!([
            { "herdr_id": "w5", "label": "f", "kind": "terminal", "dir": "/work/.features/f" },
        ]))
        .expect("records");
        // Its feature's workspaces, its own agent elsewhere, and the user's.
        let agents = [
            agent("w1:p1", "w1"),
            agent("w5:p1", "w5"),
            agent("w3:p1", "w3"),
            agent("w2:p1", "w2"),
            agent("w4:p1", "w4"),
        ];
        let kept: Vec<String> = workspace_agents(&agents, &workspaces, &records, &[f])
            .into_iter()
            .map(|(a, ws)| format!("{} {}", a.pane_id, workspace_agent_label(a, ws)))
            .collect();
        assert_eq!(kept, ["w2:p1 api · claude", "w4:p1 claude"]);
    }

    #[test]
    fn an_agents_title_is_its_own_else_its_terminals() {
        let agent = |extra: Value| -> AgentInfo {
            let mut a =
                json!({ "pane_id": "p", "terminal_id": "t", "workspace_id": "w", "tab_id": "x" });
            a.as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            serde_json::from_value(a).expect("agent")
        };
        let both = agent(json!({ "title": "Fix login", "terminal_title": "✳ Claude Code" }));
        assert_eq!(both.shown_title().as_deref(), Some("Fix login"));
        let term = agent(json!({ "title": "", "terminal_title": "✳ Bulk edit" }));
        assert_eq!(term.shown_title().as_deref(), Some("Bulk edit"));
        assert_eq!(agent(json!({})).shown_title(), None);
    }

    #[test]
    fn query_values_are_decoded() {
        let q = parse_query("p=ao%2Dapp&s=log+in&bad=%zz&end=%4");
        assert_eq!(q["p"], "ao-app");
        assert_eq!(q["s"], "log in");
        assert_eq!(q["bad"], "%zz");
        assert_eq!(q["end"], "%4");
    }

    #[test]
    fn doc_names_stay_in_the_feature_folder() {
        assert!(doc_name_ok("plan.md"));
        assert!(doc_name_ok("plan-written.md"));
        assert!(!doc_name_ok("../x.md"));
        assert!(!doc_name_ok("a/b.md"));
        assert!(!doc_name_ok(".hidden.md"));
        assert!(!doc_name_ok("feature.json"));
        assert_eq!(doc("p", "s", "../../x.md").status, 404);
        assert_eq!(commands("no-such", "x", "p1").status, 404);
    }

    #[test]
    fn a_stage_page_reads_its_first_file_and_names_the_rest() {
        let dir = std::env::temp_dir().join(format!("gerdr-phone-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        for name in [
            "brief.md",
            "brief-written.md",
            "research-R1.md",
            "decisions.md",
            "plan.md",
            "notes.md",
        ] {
            std::fs::write(dir.join(name), "# Hi").expect("file");
        }
        let mut page = json!({ "stage": 0, "sections": [] });
        add_files(&mut page, &dir, false);
        assert_eq!(page["sections"][0]["name"], "brief.md");
        assert!(
            page["sections"][0]["html"]
                .as_str()
                .is_some_and(|h| h.contains("Hi"))
        );
        assert_eq!(
            page["files"],
            json!([{ "name": "decisions.md" }, { "name": "notes.md" }, { "name": "research-R1.md" }])
        );
        // Build's milestones say what its log holds: the log is only named.
        let mut page = json!({ "stage": 3, "sections": [{ "kind": "milestones" }] });
        add_files(&mut page, &dir, false);
        assert_eq!(page["sections"].as_array().map(Vec::len), Some(1));
        // Manual test's evidence folds under the fix plan, open while it waits.
        std::fs::write(dir.join("evidence.md"), "# Ev").expect("file");
        std::fs::write(dir.join("fix-plan.md"), "# Fix").expect("file");
        let mut page = json!({ "stage": 7, "sections": [] });
        add_files(&mut page, &dir, true);
        assert_eq!(page["sections"], json!([]));
        let files = page["files"].as_array().expect("files");
        let names: Vec<&str> = files.iter().filter_map(|f| f["name"].as_str()).collect();
        assert_eq!(names[..2], ["fix-plan.md", "evidence.md"]);
        assert_eq!(
            (files[0]["open"].clone(), files[1]["open"].clone()),
            (json!(true), json!(false))
        );
        assert!(files[1]["html"].as_str().is_some_and(|h| h.contains("Ev")));
        // Code review's fix plan leaves the page for the files, folded once made.
        let doc = json!({ "kind": "doc", "name": "fix-plan.md" });
        let mut page = json!({ "stage": 6, "sections": [doc] });
        add_files(&mut page, &dir, false);
        assert_eq!(page["sections"], json!([]));
        assert_eq!(page["files"][0]["name"], "fix-plan.md");
        assert_eq!(page["files"][0]["open"], false);
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    #[test]
    fn a_log_is_its_last_lines_fenced_past_its_own_backticks() {
        let text: String = (0..LOG_LINES + 5).map(|i| format!("line {i}\n")).collect();
        let block = log_block(&format!("{text}```"));
        assert!(block.starts_with("````\n"), "{}", &block[..10]);
        assert!(!block.contains("line 4\n"));
        assert!(block.contains(&format!("line {}", LOG_LINES + 4)));
    }

    #[test]
    fn unknown_paths_are_not_found() {
        assert_eq!(answer("/nope", &Icons::default(), "").status, 404);
        assert_eq!(answer("/", &Icons::default(), "").status, 200);
    }

    #[test]
    fn the_icon_draws() {
        assert!(png(32).is_some_and(|png| png.starts_with(b"\x89PNG")));
    }
}
