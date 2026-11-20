//! The phones that reach the phone view (see `phone`), and whether its
//! server runs: for the footer's phone chip and the Phone connections
//! dialog, which lists each phone with Block, Notifications off and Forget,
//! and the server with Stop, Start and Restart.
//!
//! The phone view writes down each device that asks it something in
//! `devices.json`, next to `push.json`, by its tailnet address: `tailscale
//! serve` passes it as `X-Forwarded-For`. A blocked one gets 403. gerdr
//! blocks, unblocks and forgets them in the same file; both lock it for
//! each change. The page polls every 2 to 5 s while it shows, so a phone
//! that asked in the last `CONNECTED` seconds counts as connected.
//!
//! The server says it runs in `phone-server.json` in gerdr's runtime
//! folder; gerdr looks at both every 2 s (see `tasks::look_at_kept`).

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::PathBuf,
    process::Command,
};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel;

use crate::{Gerdr, chrome::Dot, clock::now_secs, menu::MenuAction, tasks::Task, web_push};

/// How long after its last request a phone counts as connected.
pub const CONNECTED: u64 = 15;
/// A device's last request is written down at most this often.
const WRITE_EVERY: u64 = 5;
/// The systemd user unit `docs/phone.md` sets up.
const UNIT: &str = "gerdr-phone";

/// A device that reached the phone view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Its tailnet address.
    pub ip: String,
    /// Its name on the tailnet ("pixel-8"), else what its browser runs on
    /// ("iPhone").
    pub name: String,
    /// Who it is signed in to the tailnet as.
    #[serde(default)]
    pub login: String,
    /// When it first and last asked, in seconds since the epoch.
    pub first: u64,
    pub last: u64,
    #[serde(default)]
    pub blocked: bool,
}

/// The phone view's server, as it said it started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub pid: u32,
    pub port: u16,
    pub since: u64,
}

/// What gerdr last saw of the phone view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    /// Its server, while it runs.
    pub server: Option<Server>,
    /// The server runs as the systemd unit.
    pub in_unit: bool,
    /// The unit is set up, so the server can start from gerdr.
    pub unit: bool,
    pub devices: Vec<Device>,
    /// Why the devices couldn't be read.
    pub error: Option<String>,
    /// The addresses of the devices that get notifications.
    pub notified: Vec<String>,
}

/// What the dialog's buttons do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhoneDo {
    Stop,
    Start,
    Restart,
    Block(String),
    Unblock(String),
    NotesOff(String),
    Forget(String),
}

fn path() -> PathBuf {
    crate::projects::data_dir()
        .join("phone")
        .join("devices.json")
}

fn server_path() -> Option<PathBuf> {
    Some(crate::phone_control::dir()?.join("phone-server.json"))
}

/// The devices written down; none before the first.
pub fn list() -> Result<Vec<Device>, String> {
    let path = path();
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let mut text = String::new();
    file.lock_shared()
        .and_then(|()| file.read_to_string(&mut text))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    parse(&text).map_err(|err| format!("{}: {err}", path.display()))
}

fn parse(text: &str) -> Result<Vec<Device>, serde_json::Error> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text)
}

/// Change the devices written down by `f`, the file locked meanwhile.
pub fn change(f: impl FnOnce(&mut Vec<Device>)) -> Result<(), String> {
    let path = path();
    let at = |err: &dyn std::fmt::Display| format!("{}: {err}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| at(&e))?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| at(&e))?;
    file.lock().map_err(|e| at(&e))?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| at(&e))?;
    let mut list = parse(&text).map_err(|e| at(&e))?;
    f(&mut list);
    let text = serde_json::to_string_pretty(&list).map_err(|e| at(&e))?;
    file.set_len(0)
        .and_then(|()| file.rewind())
        .and_then(|()| file.write_all(text.as_bytes()))
        .map_err(|e| at(&e))
}

/// The phone view had a request from `ip`, whose browser says `agent`,
/// signed in as `login`: write it down. Whether it is blocked; an error
/// when the list can't be read, so a blocked phone isn't let in by it.
pub fn seen(ip: &str, agent: &str, login: &str) -> Result<bool, String> {
    let now = now_secs();
    let known = list()?.into_iter().find(|d| d.ip == ip);
    match &known {
        Some(d) if d.blocked => return Ok(true),
        Some(d) if now < d.last + WRITE_EVERY => return Ok(false),
        _ => {}
    }
    let name = known
        .is_none()
        .then(|| tailnet_name(ip).unwrap_or_else(|| browser(agent).to_owned()));
    let mut blocked = false;
    change(|list| match list.iter_mut().find(|d| d.ip == ip) {
        Some(d) => {
            d.last = now;
            blocked = d.blocked;
            if !login.is_empty() {
                login.clone_into(&mut d.login);
            }
        }
        None => list.push(Device {
            ip: ip.to_owned(),
            name: name.unwrap_or_else(|| browser(agent).to_owned()),
            login: login.to_owned(),
            first: now,
            last: now,
            blocked: false,
        }),
    })?;
    Ok(blocked)
}

/// What a browser saying `agent` runs on.
fn browser(agent: &str) -> &'static str {
    ["iPhone", "iPad", "Android", "Mac", "Windows", "Linux"]
        .into_iter()
        .find(|w| agent.contains(w))
        .unwrap_or("A browser")
}

/// The name the tailnet gives `ip`'s machine: the first part of its DNS
/// name.
fn tailnet_name(ip: &str) -> Option<String> {
    ip.parse::<std::net::IpAddr>().ok()?;
    let out = Command::new("tailscale")
        .args(["whois", "--json", ip])
        .output()
        .inspect_err(|err| tracing::warn!("Couldn't ask tailscale who {ip} is: {err}"))
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            "tailscale whois {ip}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let who: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let name = who["Node"]["Name"].as_str()?.split('.').next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// The phone view's server started on `port`: say so to gerdr.
pub fn started(port: u16) {
    let Some(path) = server_path() else {
        return tracing::warn!("No runtime folder to tell gerdr the phone view runs in");
    };
    let server = Server {
        pid: std::process::id(),
        port,
        since: now_secs(),
    };
    let written = std::fs::create_dir_all(path.parent().unwrap_or(&path))
        .map_err(|e| e.to_string())
        .and_then(|()| serde_json::to_vec(&server).map_err(|e| e.to_string()))
        .and_then(|text| crate::projects::write_atomic(&path, &text).map_err(|e| e.to_string()));
    if let Err(err) = written {
        tracing::warn!("Couldn't write {}: {err}", path.display());
    }
}

/// The phone view's server, if it runs, and whether as the unit.
fn server() -> Option<(Server, bool)> {
    let text = std::fs::read(server_path()?).ok()?;
    let server: Server = serde_json::from_slice(&text).ok()?;
    let proc = PathBuf::from(format!("/proc/{}", server.pid));
    // A pid used again by another program isn't the server.
    let cmdline = std::fs::read(proc.join("cmdline")).ok()?;
    if !cmdline
        .split(|b| *b == 0)
        .any(|arg| arg == crate::phone::COMMAND.as_bytes())
    {
        return None;
    }
    let cgroup = std::fs::read_to_string(proc.join("cgroup")).unwrap_or_default();
    let in_unit = cgroup.contains(&format!("/{UNIT}.service"));
    Some((server, in_unit))
}

fn unit_path() -> PathBuf {
    crate::herdr::config_home()
        .join("systemd/user")
        .join(format!("{UNIT}.service"))
}

/// Look at the phone view again.
pub fn look() -> Seen {
    let (server, in_unit) = server().map_or((None, false), |(s, u)| (Some(s), u));
    let (devices, error) = match list() {
        Ok(devices) => (devices, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    Seen {
        server,
        in_unit,
        unit: unit_path().exists(),
        devices,
        error,
        notified: web_push::subscriptions()
            .into_iter()
            .map(|s| s.device)
            .collect(),
    }
}

/// How long ago `at` is, shortly: "now", "5m ago".
fn ago(at: u64, now: u64) -> String {
    match now.saturating_sub(at) {
        0..CONNECTED => "now".to_owned(),
        secs => format!("{} ago", crate::tasks::uptime(secs)),
    }
}

impl Seen {
    fn connected(&self, now: u64) -> impl Iterator<Item = &Device> {
        let runs = self.server.is_some();
        self.devices
            .iter()
            .filter(move |d| runs && !d.blocked && now.saturating_sub(d.last) < CONNECTED)
    }

    /// The footer's chip, while the server runs.
    pub fn task(&self, now: u64) -> Option<Task> {
        let server = self.server.as_ref()?;
        let names: Vec<&str> = self.connected(now).map(|d| d.name.as_str()).collect();
        let (chip, said) = match names.as_slice() {
            [] => ("Phone view".to_owned(), "no phone connected".to_owned()),
            [one] => (format!("Phone: {one}"), format!("{one} connected")),
            many => (
                format!("{} phones", many.len()),
                format!("{} connected", many.join(", ")),
            ),
        };
        Some(Task {
            title: "Phone view".to_owned(),
            chip,
            detail: format!("{said} · port {}", server.port),
            dot: if names.is_empty() {
                Dot::Unknown
            } else {
                Dot::Done
            },
            actions: vec![("Connections", MenuAction::OpenPhones)],
        })
    }

    /// The Phone connections dialog's rows: the server, then each phone,
    /// the last to ask first.
    pub fn rows(&self, now: u64) -> Vec<Task> {
        let phone = |a: PhoneDo| MenuAction::Phone(a);
        let mut rows = vec![match &self.server {
            Some(server) => {
                let mut actions = vec![("Stop", phone(PhoneDo::Stop))];
                if self.in_unit {
                    actions.insert(0, ("Restart", phone(PhoneDo::Restart)));
                }
                Task {
                    title: "Phone view".to_owned(),
                    chip: String::new(),
                    detail: format!(
                        "on port {} · up {}",
                        server.port,
                        crate::tasks::uptime(now.saturating_sub(server.since))
                    ),
                    dot: Dot::Done,
                    actions,
                }
            }
            None => Task {
                title: "Phone view".to_owned(),
                chip: String::new(),
                detail: if self.unit {
                    "stopped".to_owned()
                } else {
                    format!("stopped · no {UNIT} service to start, see docs/phone.md")
                },
                dot: Dot::Idle,
                actions: if self.unit {
                    vec![("Start", phone(PhoneDo::Start))]
                } else {
                    Vec::new()
                },
            },
        }];
        if let Some(error) = &self.error {
            rows.push(Task {
                title: "Couldn't read the phones".to_owned(),
                chip: String::new(),
                detail: error.clone(),
                dot: Dot::Failed,
                actions: Vec::new(),
            });
        }
        let mut devices: Vec<&Device> = self.devices.iter().collect();
        devices.sort_by_key(|d| std::cmp::Reverse(d.last));
        for d in devices {
            let notified = self.notified.contains(&d.ip);
            let connected = self.connected(now).any(|c| c.ip == d.ip);
            // What matters first: the address and login are cut on a narrow row.
            let mut detail = vec![if d.blocked {
                "blocked".to_owned()
            } else if connected {
                "connected".to_owned()
            } else {
                format!("seen {}", ago(d.last, now))
            }];
            if notified {
                detail.push("notifications on".to_owned());
            }
            detail.push(d.ip.clone());
            if !d.login.is_empty() {
                detail.push(d.login.clone());
            }
            let mut actions = vec![if d.blocked {
                ("Unblock", phone(PhoneDo::Unblock(d.ip.clone())))
            } else {
                ("Block", phone(PhoneDo::Block(d.ip.clone())))
            }];
            if notified {
                actions.push(("Notifications off", phone(PhoneDo::NotesOff(d.ip.clone()))));
            }
            actions.push(("Forget", phone(PhoneDo::Forget(d.ip.clone()))));
            rows.push(Task {
                title: d.name.clone(),
                chip: String::new(),
                detail: detail.join(" · "),
                dot: if d.blocked {
                    Dot::Failed
                } else if connected {
                    Dot::Done
                } else {
                    Dot::Idle
                },
                actions,
            });
        }
        rows
    }
}

/// Do `what` to the devices' list and the notifications; what to say.
fn act_on_devices(what: &PhoneDo) -> Result<String, String> {
    let name = |ip: &str| {
        list()
            .ok()
            .and_then(|l| l.into_iter().find(|d| d.ip == ip))
            .map_or_else(|| ip.to_owned(), |d| d.name)
    };
    let set = |ip: &str, blocked: bool| {
        change(|list| {
            for d in list.iter_mut().filter(|d| d.ip == ip) {
                d.blocked = blocked;
            }
        })
    };
    Ok(match what {
        PhoneDo::Block(ip) => {
            set(ip, true)?;
            // A blocked phone gets nothing more, notifications included.
            web_push::drop_device(ip)?;
            format!("Blocked {}", name(ip))
        }
        PhoneDo::Unblock(ip) => {
            set(ip, false)?;
            format!("Unblocked {}", name(ip))
        }
        PhoneDo::NotesOff(ip) => {
            web_push::drop_device(ip)?;
            format!("{} gets no more notifications", name(ip))
        }
        PhoneDo::Forget(ip) => {
            let name = name(ip);
            web_push::drop_device(ip)?;
            change(|list| list.retain(|d| d.ip != *ip))?;
            format!("Forgot {name}")
        }
        PhoneDo::Stop | PhoneDo::Start | PhoneDo::Restart => String::new(),
    })
}

impl Gerdr {
    /// Open the Phone connections dialog.
    pub fn open_phones(&mut self) {
        self.open_tasks_of(0, true);
    }

    /// Press a button of the Phone connections dialog.
    pub fn phone_do(&mut self, what: PhoneDo) {
        let seen = &self.keep.phone;
        let verb = match what {
            PhoneDo::Stop if seen.in_unit => "stop",
            PhoneDo::Stop => {
                let Some(server) = &seen.server else {
                    return self.show_warning("The phone view isn't running".to_owned());
                };
                // Started by hand, not as the unit: it ends with its signal.
                // SAFETY: kill takes no pointers; the pid is the server's,
                // checked against its command line just now.
                if unsafe { libc::kill(server.pid as i32, libc::SIGTERM) } == 0 {
                    self.show_success("Stopped the phone view".to_owned());
                } else {
                    let err = std::io::Error::last_os_error();
                    self.show_error(format!("Couldn't stop the phone view: {err}"));
                }
                return self.look_at_kept();
            }
            PhoneDo::Start => "start",
            PhoneDo::Restart => "restart",
            _ => {
                match act_on_devices(&what) {
                    Ok(said) => self.show_success(said),
                    Err(err) => self.show_error(format!("Couldn't change the phones: {err}")),
                }
                return self.look_at_kept();
            }
        };
        self.systemctl(verb);
    }

    /// `systemctl --user <verb> gerdr-phone`, off the loop's thread; what
    /// it did shows when it is done.
    fn systemctl(&mut self, verb: &'static str) {
        let (tx, rx) = channel::channel::<Result<(), String>>();
        let done = move |event, _: &mut (), state: &mut Gerdr| {
            let channel::Event::Msg(result) = event else {
                return;
            };
            let done = match verb {
                "stop" => "Stopped",
                "start" => "Started",
                _ => "Restarted",
            };
            match result {
                Ok(()) => state.show_success(format!("{done} the phone view")),
                Err(err) => state.show_error(format!("Couldn't {verb} the phone view: {err}")),
            }
            state.look_at_kept();
        };
        if let Err(err) = self.loop_handle.insert_source(rx, done) {
            return self.show_error(format!("Couldn't {verb} the phone view: {err}"));
        }
        std::thread::spawn(move || {
            let result = Command::new("systemctl")
                .args(["--user", verb, UNIT])
                .output()
                .map_err(|err| format!("systemctl: {err}"))
                .and_then(|out| {
                    out.status
                        .success()
                        .then_some(())
                        .ok_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_owned())
                });
            // The loop ends only with gerdr.
            let _ = tx.send(result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(ip: &str, last: u64, blocked: bool) -> Device {
        Device {
            ip: ip.into(),
            name: format!("phone {ip}"),
            last,
            blocked,
            ..Device::default()
        }
    }

    fn seen(devices: Vec<Device>, runs: bool) -> Seen {
        Seen {
            server: runs.then_some(Server {
                pid: 1,
                port: 8390,
                since: 0,
            }),
            devices,
            notified: vec!["100.1.1.2".into()],
            ..Seen::default()
        }
    }

    #[test]
    fn browsers_are_named_by_what_they_run_on() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)";
        assert_eq!(browser(iphone), "iPhone");
        assert_eq!(
            browser("Mozilla/5.0 (Linux; Android 14; Pixel 8)"),
            "Android"
        );
        assert_eq!(browser("curl/8"), "A browser");
    }

    #[test]
    fn a_phone_that_asked_lately_is_connected_while_the_server_runs() {
        let now = 1000;
        let devices = vec![
            device("100.1.1.1", now - 3, false),
            device("100.1.1.2", now - 600, false),
            device("100.1.1.3", now - 1, true),
        ];
        let task = seen(devices.clone(), true).task(now).expect("a chip");
        assert_eq!(task.chip, "Phone: phone 100.1.1.1");
        assert_eq!(task.dot, Dot::Done);
        assert!(seen(devices.clone(), false).task(now).is_none());
        let alone = seen(vec![device("100.1.1.2", now - 600, false)], true);
        assert_eq!(alone.task(now).expect("a chip").dot, Dot::Unknown);

        let rows = seen(devices, true).rows(now);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Phone view",
                "phone 100.1.1.3",
                "phone 100.1.1.1",
                "phone 100.1.1.2"
            ]
        );
        let labels = |row: &Task| row.actions.iter().map(|(l, _)| *l).collect::<Vec<_>>();
        assert_eq!(labels(&rows[1]), ["Unblock", "Forget"]);
        assert_eq!(labels(&rows[2]), ["Block", "Forget"]);
        assert_eq!(labels(&rows[3]), ["Block", "Notifications off", "Forget"]);
        assert!(
            rows[3].detail.contains("seen 10m ago"),
            "{}",
            rows[3].detail
        );
    }

    #[test]
    fn a_stopped_server_starts_only_through_its_unit() {
        let stopped = Seen::default().rows(0);
        assert!(stopped[0].actions.is_empty());
        let unit = Seen {
            unit: true,
            ..Seen::default()
        };
        assert_eq!(unit.rows(0)[0].actions[0].0, "Start");
    }

    #[test]
    fn an_empty_list_file_is_no_devices() {
        assert_eq!(parse(" \n").expect("empty"), Vec::new());
        assert!(parse("{").is_err());
    }
}
