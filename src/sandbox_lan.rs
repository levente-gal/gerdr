//! A feature's sandbox opened to the local network, for a phone: its app's
//! ports open on this machine's network address under the same numbers, each
//! connection a command of its own that enters the sandbox's namespace and
//! connects to that port there (`machine_sandbox::relay_script`).
//!
//! The same numbers matter: an app on the phone gets the addresses the
//! sandbox's commands print (Expo's QR code says `exp://<address>:8081`), and
//! the namespace has this machine's address, so what the sandbox says of
//! itself is right on the phone too. `{lan_ip}` in `workflow.sandbox_env`
//! becomes that address as a sandbox starts, for an app's API URL.
//!
//! It is off until asked for, as anything on the network can then reach the
//! sandbox's app and its copy of the data. It ends with the sandbox, when the
//! sandbox starts again (a new namespace), and with gerdr. Only a sandbox on
//! this machine: one elsewhere has its own network.

use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
    process::Command,
};

use crate::{
    Gerdr,
    feature::Status,
    feature_sandbox,
    port_holder,
    machine_sandbox::{Missing, Relay, relay_script, start_relay_on},
};

/// What is said when this machine has no network to open a sandbox on.
const NO_ADDRESS: &str = "This machine has no network address to open the sandbox on";

/// What `{lan_ip}` in a sandbox's variables becomes.
pub const LAN_IP: &str = "{lan_ip}";

/// The address this machine has on its network: the one its default route
/// leaves from. Connecting a UDP socket sends nothing; it only picks the route.
/// `None` with no network.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // A documentation address: never reached, only routed to.
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// `value`, a sandbox variable's, with `{lan_ip}` as `ip`, or `localhost`
/// with no network.
pub fn with_lan_ip(value: &str, ip: Option<Ipv4Addr>) -> String {
    let ip = ip.map_or_else(|| "localhost".to_owned(), |ip| ip.to_string());
    value.replace(LAN_IP, &ip)
}

/// What a sandbox has open on the network.
#[derive(Debug)]
pub struct LanOpen {
    title: String,
    /// The sandbox it reaches: a new one closes it.
    holder: i32,
    ip: Ipv4Addr,
    relays: BTreeMap<u16, Relay>,
    /// Ports it listens on that something here holds already, each with what
    /// was said of it.
    taken: BTreeMap<u16, String>,
    missing: Missing,
    said_missing: bool,
}

/// What a feature's page says of its sandbox on the network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanSeen {
    pub ip: String,
    pub open: Vec<u16>,
    pub taken: Vec<u16>,
}

impl LanOpen {
    fn seen(&self) -> LanSeen {
        LanSeen {
            ip: self.ip.to_string(),
            open: self.relays.keys().copied().collect(),
            taken: self.taken.keys().copied().collect(),
        }
    }

    /// The sandbox listens on `ports`: open what isn't, close what it no
    /// longer listens on. The ports found taken just now, with what to say of
    /// each.
    fn follow(&mut self, ports: &[u16]) -> Vec<(u16, String)> {
        self.relays.retain(|port, _| ports.contains(port));
        self.taken.retain(|port, text| {
            let kept = ports.contains(port);
            if !kept {
                crate::problems::forget(feature_sandbox::PROBLEMS, text);
            }
            kept
        });
        let mut newly = Vec::new();
        for &port in ports {
            if self.relays.contains_key(&port) {
                continue;
            }
            match relay_into(self.ip, self.holder, port, self.missing.clone()) {
                Ok(relay) => {
                    self.relays.insert(port, relay);
                    if let Some(text) = self.taken.remove(&port) {
                        crate::problems::forget(feature_sandbox::PROBLEMS, &text);
                    }
                }
                Err(err) => {
                    if !self.taken.contains_key(&port) {
                        tracing::warn!("port {port} on the network for the sandbox: {err}");
                        let text = taken_text(&self.title, port, port_holder::of(port).as_ref());
                        self.taken.insert(port, text.clone());
                        newly.push((port, text));
                    }
                }
            }
        }
        newly
    }
}

impl Drop for LanOpen {
    fn drop(&mut self) {
        for text in self.taken.values() {
            crate::problems::forget(feature_sandbox::PROBLEMS, text);
        }
    }
}

/// What's said of `port` being held here, by `holder` when it can be told: it
/// goes on the badge too, as what holds it is for the user to stop.
fn taken_text(title: &str, port: u16, holder: Option<&port_holder::Holder>) -> String {
    match holder {
        Some(holder) => format!(
            "Port {port} is taken here by {}: stop it so a phone can reach the sandbox of \u{201c}{title}\u{201d}",
            holder.named()
        ),
        None => format!(
            "Port {port} is taken here, so a phone can't reach the sandbox of \u{201c}{title}\u{201d} at it: stop what holds it"
        ),
    }
}

/// `port` on `ip` here, passing each connection into the namespace `holder`
/// keeps.
fn relay_into(ip: Ipv4Addr, holder: i32, port: u16, missing: Missing) -> io::Result<Relay> {
    let listener = TcpListener::bind(SocketAddr::from((ip, port)))?;
    listener.set_nonblocking(true)?;
    let command = move || {
        let mut command = Command::new("nsenter");
        command
            .args(["--user", "--net", "--preserve-credentials", "-t"])
            .arg(holder.to_string())
            .args(["--", "bash", "-c"])
            .arg(relay_script(port));
        command
    };
    start_relay_on(vec![listener], port, command, missing)
}

impl Gerdr {
    /// The sandbox of feature `slug` is open on the network: how it stands.
    pub(crate) fn sandbox_lan(&self, project: &str, slug: &str) -> Option<LanSeen> {
        let key = (project.to_owned(), slug.to_owned());
        self.features.build.sandbox_lan.get(&key).map(LanOpen::seen)
    }

    /// Open feature `slug`'s sandbox to the network, asking first, as anything
    /// there can then reach it; close it again at once if it is open.
    pub(crate) fn ask_sandbox_lan(&mut self, project: &str, slug: &str) {
        let key = (project.to_owned(), slug.to_owned());
        if self.features.build.sandbox_lan.remove(&key).is_some() {
            return self.show_success("The sandbox is closed to the network".into());
        }
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let Some(run) = feature.sandbox.as_ref().filter(|run| run.alive()) else {
            return self.show_warning("The sandbox isn't running".into());
        };
        if run.on_another_machine() || feature.writer.is_some() {
            return self.show_warning(
                "Only a sandbox on this machine opens to the network from here".into(),
            );
        }
        let Some(ip) = lan_ip() else {
            return self.show_error(NO_ADDRESS.into());
        };
        let ports: Vec<String> = run.listening_wide().iter().map(u16::to_string).collect();
        let what = match ports.is_empty() {
            true => "what it listens on".to_owned(),
            false => format!("ports {}", ports.join(", ")),
        };
        let mut body = format!(
            "Phones and anything else on your network can then reach its app at {ip}, {what}, and its copy of the data. It closes when the sandbox stops, or on Ctrl+Shift+N."
        );
        if let Some(local) = local_only(run) {
            body.push_str(&format!(" {local}."));
        }
        self.ask(
            "Open the sandbox to your network?".into(),
            body,
            "Open",
            crate::confirm::Then::OpenSandboxLan(project.to_owned(), slug.to_owned()),
        );
    }

    /// Open feature `slug`'s sandbox to the network, as asked.
    pub(crate) fn open_sandbox_lan(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return;
        };
        let title = feature.title.clone();
        let Some(holder) = (feature.sandbox.as_ref())
            .filter(|run| run.alive() && !run.on_another_machine())
            .and_then(|run| run.holder.as_ref().map(|h| h.pid))
        else {
            return self.show_warning("The sandbox isn't running".into());
        };
        let Some(ip) = lan_ip() else {
            return self.show_error(NO_ADDRESS.into());
        };
        let key = (project.to_owned(), slug.to_owned());
        let open = LanOpen {
            title,
            holder,
            ip,
            relays: BTreeMap::new(),
            taken: BTreeMap::new(),
            missing: Missing::default(),
            said_missing: false,
        };
        self.features.build.sandbox_lan.insert(key, open);
        self.follow_sandbox_lan(project, slug);
        if let Some(seen) = self.sandbox_lan(project, slug)
            && !seen.open.is_empty()
        {
            let ports: Vec<String> = seen.open.iter().map(u16::to_string).collect();
            self.show_success(format!(
                "The sandbox is open to your network at {}: {}",
                seen.ip,
                ports.join(", ")
            ));
        } else if let Some(local) = (self.feature(project, slug))
            .and_then(|f| f.sandbox.as_ref())
            .and_then(local_only)
        {
            self.show_warning(local);
        }
    }

    /// Keep what is open on the network in line with what each sandbox
    /// listens on, and close it for a sandbox gone or started again.
    pub(crate) fn sandbox_lan_tick(&mut self) {
        if self.features.build.sandbox_lan.is_empty() {
            return;
        }
        let keys: Vec<(String, String)> = self.features.build.sandbox_lan.keys().cloned().collect();
        for (project, slug) in keys {
            self.follow_sandbox_lan(&project, &slug);
        }
    }

    fn follow_sandbox_lan(&mut self, project: &str, slug: &str) {
        let key = (project.to_owned(), slug.to_owned());
        let run = (self.feature(project, slug))
            .filter(|f| f.status == Status::Open)
            .and_then(|f| f.sandbox.clone())
            .filter(|run| run.alive() && !run.on_another_machine());
        let holder = run
            .as_ref()
            .and_then(|run| run.holder.as_ref().map(|h| h.pid));
        let same = (self.features.build.sandbox_lan.get(&key))
            .is_some_and(|open| Some(open.holder) == holder);
        let (Some(run), true) = (run, same) else {
            // Its namespace went: what reached it goes, said once.
            if self.features.build.sandbox_lan.remove(&key).is_some() {
                self.show_notice("The sandbox closed to the network as it stopped".into());
            }
            return;
        };
        let ports = run.listening_wide();
        let Some(open) = self.features.build.sandbox_lan.get_mut(&key) else {
            return;
        };
        let newly = open.follow(&ports);
        let missing =
            open.missing.0.load(std::sync::atomic::Ordering::Relaxed) && !open.said_missing;
        open.said_missing |= missing;
        for (_, text) in newly {
            crate::problems::report(feature_sandbox::PROBLEMS, text.clone());
            self.show_warning(text);
        }
        if missing {
            let text =
                "A phone can't reach the sandbox: this machine needs bash and nsenter for that"
                    .to_owned();
            tracing::warn!("{text}");
            self.show_error(text);
        }
    }
}

/// What to tell when `run`'s app listens on loopback only, which stays off
/// the network: `None` when something of it listens wider, or nothing does.
fn local_only(run: &crate::feature::sandbox_run::SandboxRun) -> Option<String> {
    let (all, wide) = (run.listening(), run.listening_wide());
    if !wide.is_empty() || all.is_empty() {
        return None;
    }
    let ports: Vec<String> = all.iter().map(u16::to_string).collect();
    Some(format!(
        "Its app listens on this machine only (ports {}): start it on 0.0.0.0 for a phone to reach it",
        ports.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_ip_in_a_variable_is_the_address_or_localhost() {
        let ip = Some(Ipv4Addr::new(192, 168, 1, 5));
        assert_eq!(
            with_lan_ip("http://{lan_ip}:9001", ip),
            "http://192.168.1.5:9001"
        );
        assert_eq!(
            with_lan_ip("http://{lan_ip}:9001", None),
            "http://localhost:9001"
        );
        assert_eq!(with_lan_ip("plain", ip), "plain");
    }

    /// A server in a namespace of its own, as a sandbox's app: reached here
    /// through the relay, on the same port.
    #[test]
    fn a_connection_here_reaches_the_app_in_the_namespace() {
        use std::io::{Read, Write};
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let serve = format!(
            "ip link set lo up 2>/dev/null; exec python3 -c 'import socket\ns=socket.socket()\ns.bind((\"127.0.0.1\",{port}))\ns.listen()\nc,_=s.accept()\nc.recv(9)\nc.sendall(b\"inside\")'"
        );
        let Ok(mut app) = Command::new("unshare")
            .args(["--user", "--map-root-user", "--net", "sh", "-c", &serve])
            .spawn()
        else {
            return eprintln!("no unshare here: not tried");
        };
        std::thread::sleep(std::time::Duration::from_millis(500));
        let relay = relay_into(
            Ipv4Addr::LOCALHOST,
            app.id() as i32,
            port,
            Missing::default(),
        );
        let relay = relay.expect("the port is free here: the app holds it only inside");
        let mut stream = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.write_all(b"hi").unwrap();
        let mut got = Vec::new();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let _ = stream.read_to_end(&mut got);
        drop(relay);
        let _ = app.kill();
        let _ = app.wait();
        assert_eq!(got, b"inside");
    }

    #[test]
    fn a_port_held_here_is_taken_and_said_once() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let mut open = LanOpen {
            title: "T".into(),
            holder: std::process::id() as i32,
            ip: Ipv4Addr::LOCALHOST,
            relays: BTreeMap::new(),
            taken: BTreeMap::new(),
            missing: Missing::default(),
            said_missing: false,
        };
        let newly = open.follow(&[port]);
        assert_eq!(newly.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [port]);
        // This test holds it, so it is named.
        assert!(newly[0].1.contains(&format!("(pid {}", std::process::id())), "{}", newly[0].1);
        assert!(open.follow(&[port]).is_empty(), "said once");
        assert_eq!(open.seen().taken, [port]);
        // No longer listened on in the sandbox: no longer said.
        assert!(open.follow(&[]).is_empty());
        assert!(open.seen().taken.is_empty());
        assert!(open.seen().open.is_empty());
        drop(held);
    }
}
