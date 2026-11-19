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
    feature::{
        Feature, Stage, Status,
        review_import::{Finding, FindingState},
        store,
    },
    feature_agents::{self, Gate},
    herdr::{self, AgentStatus, Herdr},
    inbox_sources, md_preview, phone_control,
    projects::{self, Project},
    web_push,
};

/// The option that runs it in place of gerdr.
pub const COMMAND: &str = "--phone";
/// The port it listens on unless told another.
const PORT: u16 = 8390;
/// The most a request's head may take.
const HEAD_LIMIT: u64 = 16 * 1024;
/// How many lines of an agent's pane are sent.
const PANE_LINES: usize = 400;
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
            } else if request.method == "GET" {
                answer(&request.target, icons)
            } else if request.method == "POST" && request.from_page {
                act(&request.target, &request.body, &request.host)
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
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; script-src 'self' https://cdn.jsdelivr.net; \
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

fn answer(target: &str, icons: &Icons) -> Reply {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = parse_query(query);
    let arg = |name: &str| query.get(name).map(String::as_str).unwrap_or_default();
    match path {
        "/" => Reply::ok("text/html; charset=utf-8", INDEX),
        "/app.js" => Reply::ok("text/javascript; charset=utf-8", APP_JS),
        "/app.css" => Reply::ok("text/css; charset=utf-8", APP_CSS),
        "/icon.svg" => Reply::ok("image/svg+xml", ICON),
        "/icon-180.png" => Reply::ok("image/png", icons.touch.clone()),
        "/icon-192.png" => Reply::ok("image/png", icons.small.clone()),
        "/icon-512.png" => Reply::ok("image/png", icons.big.clone()),
        "/manifest.webmanifest" => Reply::ok("application/manifest+json", MANIFEST),
        "/api/features" => Reply::json(&all_features()),
        "/api/feature" => feature_page(arg("p"), arg("s")),
        "/api/doc" => doc(arg("p"), arg("s"), arg("f")),
        "/api/pane" => pane(arg("p"), arg("s"), arg("pane")),
        "/sw.js" => Reply::ok("text/javascript; charset=utf-8", SW_JS),
        "/api/push" => match web_push::public_key() {
            Ok(key) => Reply::json(&json!({ "key": key })),
            Err(why) => Reply::error(500, &why),
        },
        _ => Reply::error(404, "Nothing here"),
    }
}

/// A phone asks for notifications, or no more: `{subscription, on}`, the
/// subscription as its browser's `toJSON` gives it. Notes go out from the
/// phone view's address on `host`.
fn push(body: &[u8], host: &str) -> Reply {
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

/// A press from the page: `/api/press`, `/api/decide` and `/api/send-back`
/// go on to gerdr, `/api/agent` to the agent's pane.
fn act(target: &str, body: &[u8], host: &str) -> Reply {
    if target == "/api/push" {
        return push(body, host);
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
        "/api/press" => "press",
        "/api/decide" => "decide",
        "/api/send-back" => "send-back",
        "/api/agent" => {
            let (pane, line, key) = (text("pane"), text("text"), text("key"));
            return agent_input(&project, &slug, &pane, &line, &key);
        }
        _ => return Reply::error(404, "Nothing here"),
    };
    let mut request = json!({ "op": op, "project": project, "slug": slug });
    for (name, value) in args {
        request[name] = value;
    }
    match gerdr(&request) {
        Ok(answer) => Reply::json(&answer),
        Err(why) => Reply::error(502, &why),
    }
}

/// Ask the gerdr running here: `request` on its phone socket (see
/// `phone_control`), the one changed last first.
// shortcut: with several gerdrs running, the newest that answers acts; any
// of them knows every project's features, so that holds until one doesn't.
fn gerdr(request: &Value) -> Result<Value, String> {
    let dir = phone_control::dir().ok_or("No runtime folder to find gerdr in")?;
    let mut sockets: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("gerdr isn't running ({e})"))?
        .flatten()
        .filter(|e| phone_control::is_socket_name(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    sockets.sort_by_key(|s| std::cmp::Reverse(s.0));
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
            .set_read_timeout(Some(GERDR_TIME))
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

/// The keys the phone may press in an agent's pane: herdr's names.
const AGENT_KEYS: [&str; 4] = ["Enter", "esc", "Up", "Down"];

/// Type `line` to an agent of feature `slug` (Enter after it), or press
/// `key`, or type a menu's number.
fn agent_input(project: &str, slug: &str, pane: &str, line: &str, key: &str) -> Reply {
    let (project, f) = match find(project, slug) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    if !agent_panes(&f).any(|(_, p, _)| p == pane) {
        return Reply::error(404, "Not one of this feature's agents");
    }
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    let sent = if !line.trim().is_empty() {
        // A line break would go as Enter: one line it is.
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        herdr.send_agent_line(pane, &line)
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

/// The projects gerdr keeps, as saved.
fn projects() -> Vec<Project> {
    projects::Store::load().0.projects
}

/// How herdr says each agent of a project's session is doing, by pane; an
/// error when its session can't be asked (gerdr's not running it, say).
fn agent_states(session: &str) -> Result<HashMap<String, AgentStatus>, String> {
    let mut herdr = Herdr::on_socket(herdr::socket_path(session));
    let agents = herdr.agent_list()?;
    Ok(agents
        .into_iter()
        .map(|a| (a.pane_id, a.agent_status))
        .collect())
}

/// Every open feature of every project, the ones waiting for the user
/// first.
fn all_features() -> Value {
    let mut features = Vec::new();
    let mut problems = Vec::new();
    for project in projects() {
        let open: Vec<Feature> = match store::load_all(None, &project.id) {
            Ok(all) => all
                .into_iter()
                .filter(|f| f.status == Status::Open)
                .collect(),
            Err(err) => {
                tracing::warn!("Couldn't read {}'s features: {err}", project.name);
                problems.push(format!("Couldn't read {}'s features: {err}", project.name));
                continue;
            }
        };
        if open.is_empty() {
            continue;
        }
        let states = agent_states(&project.session);
        if let Err(err) = &states {
            // Normal for a project gerdr isn't running: said on its cards.
            tracing::debug!("No agent states for {}: {err}", project.name);
        }
        features.extend(open.iter().map(|f| summary(&project, f, &states)));
    }
    features.sort_by_key(|f| {
        let waits = f["waits"]["since"].as_u64();
        let updated = f["updated"].as_u64().unwrap_or(0);
        (waits.is_none(), waits.unwrap_or(0), u64::MAX - updated)
    });
    add_cards(&mut features);
    json!({ "features": features, "problems": problems, "now": crate::clock::now_secs() })
}

/// Put on each of `features` its sidebar card as gerdr draws it (marks,
/// line, tone); none when gerdr can't be asked, and the stages alone show.
fn add_cards(features: &mut [Value]) {
    let asked: Vec<Value> = (features.iter())
        .map(|f| json!([f["project"], f["slug"]]))
        .collect();
    let mut cards = match gerdr(&json!({ "op": "cards", "features": asked })) {
        Ok(answer) => answer,
        Err(why) => {
            // The page says gerdr isn't there by the actions it can't offer.
            tracing::debug!("No cards from gerdr: {why}");
            return;
        }
    };
    for f in features {
        let key = format!(
            "{}/{}",
            f["project"].as_str().unwrap_or(""),
            f["slug"].as_str().unwrap_or("")
        );
        f["card"] = cards["cards"][&key].take();
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
    stage_runs.chain(own)
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

fn find(project: &str, slug: &str) -> Result<(Project, Feature), Reply> {
    let project = projects()
        .into_iter()
        .find(|p| p.id == project)
        .ok_or_else(|| Reply::error(404, "No such project"))?;
    let feature = store::read_at(&store::dir(None, &project.id, slug))
        .map_err(|err| Reply::error(404, &format!("Couldn't read the feature: {err}")))?;
    Ok((project, feature))
}

/// A feature's page: what the list shows, its history and its files.
fn feature_page(project: &str, slug: &str) -> Reply {
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
    page["docs"] = docs(&store::dir(None, &project.id, slug)).into();
    page["findings"] = findings(&f).into();
    page["offer"] = gerdr(&json!({ "op": "offer", "project": project.id, "slug": slug }))
        .unwrap_or_else(|why| json!({ "error": why }));
    page["now"] = crate::clock::now_secs().into();
    Reply::json(&page)
}

/// Feature `f`'s AI review findings, those to decide first.
fn findings(f: &Feature) -> Vec<Value> {
    let Some(review) = &f.review else {
        return Vec::new();
    };
    let mut all: Vec<&Finding> = review.findings.iter().collect();
    all.sort_by_key(|x| (x.state != FindingState::Open, x.severity));
    all.into_iter()
        .map(|x| {
            let at = x
                .line
                .map_or_else(|| x.file.clone(), |l| format!("{}:{l}", x.file));
            json!({
                "id": x.id,
                "severity": x.severity,
                "state": x.state,
                "title": x.title,
                "body": x.body,
                "fix": x.fix,
                "at": if x.repo.is_empty() { at } else { format!("{} {at}", x.repo) },
                "reason": x.reason,
            })
        })
        .collect()
}

/// The markdown files in a feature's folder, by name.
fn docs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| doc_name_ok(n))
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

/// Markdown file `name` of a feature, rendered.
fn doc(project: &str, slug: &str, name: &str) -> Reply {
    if !doc_name_ok(name) {
        return Reply::error(404, "No such file");
    }
    let (project, _) = match find(project, slug) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    match std::fs::read_to_string(store::dir(None, &project.id, slug).join(name)) {
        Ok(text) => Reply::json(&json!({ "html": md_preview::render(&text) })),
        Err(err) => Reply::error(404, &format!("Couldn't read {name}: {err}")),
    }
}

/// The last lines of an agent's pane, with their colours. Only a pane one
/// of the feature's agents runs in: not the user's own shells.
fn pane(project: &str, slug: &str, pane: &str) -> Reply {
    let (project, f) = match find(project, slug) {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    let Some((label, _, _)) = agent_panes(&f).find(|(_, p, _)| *p == pane) else {
        return Reply::error(404, "Not one of this feature's agents");
    };
    let mut herdr = Herdr::on_socket(herdr::socket_path(&project.session));
    let text = herdr.pane_styled(pane, PANE_LINES).and_then(|text| {
        if text.trim().is_empty() {
            herdr.pane_read(pane, "visible", PANE_LINES)
        } else {
            Ok(text)
        }
    });
    match text {
        Ok(text) => Reply::json(&json!({ "label": label, "text": text })),
        Err(err) => Reply::error(502, &format!("Couldn't read the pane: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(act("/api/press", b"nope", "localhost").status, 400);
        assert_eq!(act("/api/nope", b"{}", "localhost").status, 404);
        assert_eq!(
            act("/api/agent", br#"{"p":"no-such","s":"x"}"#, "localhost").status,
            404
        );
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
    }

    #[test]
    fn unknown_paths_are_not_found() {
        assert_eq!(answer("/nope", &Icons::default()).status, 404);
        assert_eq!(answer("/", &Icons::default()).status, 200);
    }

    #[test]
    fn the_icon_draws() {
        assert!(png(32).is_some_and(|png| png.starts_with(b"\x89PNG")));
    }
}
