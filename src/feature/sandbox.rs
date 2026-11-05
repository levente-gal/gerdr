//! A feature's sandbox, for trying it by hand: the project running from the
//! feature's worktrees, on a private copy of its containers and their data,
//! while main keeps running.
//!
//! The copy runs under a compose project of its own, from a file generated
//! out of `docker compose config`. Each port main publishes gets a free port
//! on the loopback addresses instead. The app doesn't learn those: it runs in
//! a network namespace of its own, where main's port numbers lead to the
//! copies, so its settings stay as they are.
//!
//! This file holds what can be decided without running anything. Above all
//! `generate`, whose result must never share a volume with main: two database
//! servers on one data folder ruin it, so it checks that itself
//! (`check_isolated`) and refuses rather than hands out such a file.

use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    net::TcpListener,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Repo;

/// The label on everything a sandbox starts, its value naming the feature
/// (`feature_label`): how leftovers are found again.
pub const LABEL: &str = "gerdr.feature";

const LABEL_PROJECT: &str = "com.docker.compose.project";
const LABEL_SERVICE: &str = "com.docker.compose.service";
const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
const LABEL_CONFIG_FILES: &str = "com.docker.compose.project.config_files";
const LABEL_ENV_FILE: &str = "com.docker.compose.project.environment_file";

/// Images of message brokers. Their data doesn't copy: what waits in a queue
/// would run a second time, on the sandbox's workers.
const BROKERS: &[&str] = &["redis", "valkey", "keydb", "rabbitmq", "nats"];

/// How many times `free_port` tries to find a port free on both addresses.
const PORT_TRIES: usize = 32;
/// The names of this machine a `.env` value can connect to.
const LOOPBACK: &[&str] = &["127.0.0.1", "localhost", "[::1]"];

/// A running container, as `docker inspect` tells of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub compose: Option<ComposeLabels>,
    /// The ports it publishes on this machine.
    pub ports: Vec<u16>,
}

/// What compose wrote on a container it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeLabels {
    pub project: String,
    pub service: String,
    pub working_dir: PathBuf,
    pub config_files: Vec<PathBuf>,
    pub env_file: Option<PathBuf>,
}

impl ComposeLabels {
    fn is_under(&self, roots: &[PathBuf]) -> bool {
        roots.iter().any(|root| self.working_dir.starts_with(root))
    }
}

/// Read the output of `docker inspect <containers>`.
pub fn containers_of(inspect: &str) -> Result<Vec<Container>, String> {
    let list: Vec<Value> = serde_json::from_str(inspect).map_err(|e| e.to_string())?;
    Ok(list.iter().map(container_of).collect())
}

fn container_of(item: &Value) -> Container {
    let labels = &item["Config"]["Labels"];
    let label = |key: &str| labels[key].as_str().filter(|value| !value.is_empty());
    let compose = (|| {
        Some(ComposeLabels {
            project: label(LABEL_PROJECT)?.to_owned(),
            service: label(LABEL_SERVICE)?.to_owned(),
            working_dir: PathBuf::from(label(LABEL_WORKING_DIR)?),
            config_files: label(LABEL_CONFIG_FILES)?
                .split(',')
                .map(PathBuf::from)
                .collect(),
            env_file: label(LABEL_ENV_FILE)
                // Compose lists several with commas; the last one wins, and
                // one file is what a project nearly always has.
                .and_then(|files| files.split(',').next_back())
                .map(PathBuf::from),
        })
    })();
    let mut ports: Vec<u16> = item["NetworkSettings"]["Ports"]
        .as_object()
        .into_iter()
        .flat_map(|ports| ports.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|bound| bound["HostPort"].as_str()?.parse().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    Container {
        name: item["Name"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_owned(),
        image: item["Config"]["Image"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        compose,
        ports,
    }
}

/// A compose project running in main.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    pub project: String,
    pub working_dir: PathBuf,
    pub config_files: Vec<PathBuf>,
    pub env_file: Option<PathBuf>,
    /// Its running containers, by service.
    pub services: BTreeMap<String, Container>,
}

/// The compose projects among `containers` whose folder is under one of
/// `roots`: the ones a project of those folders runs. Others are another
/// project's business.
pub fn stacks_under(containers: &[Container], roots: &[PathBuf]) -> Vec<Stack> {
    let mut stacks = BTreeMap::<&str, Stack>::new();
    for container in containers {
        let Some(compose) = &container.compose else {
            continue;
        };
        if !compose.is_under(roots) {
            continue;
        }
        stacks
            .entry(&compose.project)
            .or_insert_with(|| Stack {
                project: compose.project.clone(),
                working_dir: compose.working_dir.clone(),
                config_files: compose.config_files.clone(),
                env_file: compose.env_file.clone(),
                services: BTreeMap::new(),
            })
            .services
            .insert(compose.service.clone(), container.clone());
    }
    stacks.into_values().collect()
}

/// The containers among `containers` compose didn't start. Nothing tells which
/// project they belong to or how to start them again, so they aren't copied.
pub fn loose(containers: &[Container]) -> Vec<&Container> {
    containers.iter().filter(|c| c.compose.is_none()).collect()
}

/// The variables a `.env` file sets to something, quotes off their values.
pub fn env_pairs(env: &str) -> Vec<(String, String)> {
    env.lines()
        .filter_map(|line| {
            let line = line.trim_start().trim_start_matches("export ");
            let (key, value) = line.split_once('=')?;
            let (key, value) = (key.trim(), value.trim());
            (!key.starts_with('#') && !value.is_empty())
                .then(|| (key.to_owned(), value.trim_matches(['"', '\'']).to_owned()))
        })
        .collect()
}

/// The ports `value` names on this machine, as in a URL. `0.0.0.0` isn't one
/// of the hosts: it says where a server listens, not what it connects to.
fn local_ports(value: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for host in LOOPBACK {
        for (at, _) in value.match_indices(host) {
            let before = value[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '-')) {
                continue;
            }
            let Some(rest) = value[at + host.len()..].strip_prefix(':') else {
                continue;
            };
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            ports.extend(rest[..end].parse::<u16>());
        }
    }
    ports
}

/// What the variables `pairs` of a `.env` file point at that a sandbox won't
/// have: a port on this machine published by a container outside `roots`,
/// which isn't copied, while the sandbox has its own localhost. In words,
/// variable names and never their values, which can hold passwords.
///
/// A port alone counts only beside a local `<NAME>_HOST`: a `PORT=9001` is
/// mostly the app's own, which the sandbox runs itself.
pub fn out_of_reach(
    pairs: &[(String, String)],
    containers: &[Container],
    roots: &[PathBuf],
) -> Vec<String> {
    let copied = |c: &Container| c.compose.as_ref().is_some_and(|l| l.is_under(roots));
    let serving = |port: u16| {
        let mut with_port = containers.iter().filter(|c| c.ports.contains(&port));
        let outside = with_port.clone().find(|c| !copied(c))?;
        // On another address, one of the project's own has it too: copied.
        (!with_port.any(copied)).then_some(outside)
    };
    let is_local = |host: &str| host == "::1" || LOOPBACK.contains(&host);
    let host_of = |prefix: &str| {
        let key = format!("{prefix}_HOST");
        pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    };
    let mut found = Vec::new();
    for (key, value) in pairs {
        let mut ports = local_ports(value);
        if key
            .strip_suffix("_PORT")
            .and_then(host_of)
            .is_some_and(|h| is_local(h))
        {
            ports.extend(value.parse::<u16>());
        }
        for port in ports {
            if let Some(container) = serving(port) {
                found.push(format!("{key} ({}, port {port})", container.name));
            }
        }
    }
    found
}

/// Where `path` is in the feature: the same place in the worktree of the
/// repository holding it, or `path` itself when the feature doesn't hold
/// that repository.
pub fn in_worktree(path: &Path, repos: &[Repo]) -> PathBuf {
    repos
        .iter()
        .filter_map(|repo| Some((repo, path.strip_prefix(&repo.main).ok()?)))
        // Repositories can nest: the deepest one holds it.
        .max_by_key(|(repo, _)| repo.main.components().count())
        .map_or_else(|| path.to_owned(), |(repo, rest)| repo.worktree.join(rest))
}

/// The compose project name of feature `slug`'s copy of `stack`. A slug is
/// unique only within its project, and a feature can hold several stacks.
pub fn compose_name(project: &str, slug: &str, stack: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (project, slug, stack).hash(&mut hasher);
    let hash = hasher.finish() & 0xff_ffff;
    let clean: String = format!("{slug}-{stack}")
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_' | '-') => c,
            _ => '-',
        })
        .collect();
    format!("gerdr-{clean}-{hash:06x}")
}

/// What `LABEL` says for feature `slug` of `project`.
pub fn feature_label(project: &str, slug: &str) -> String {
    format!("{project}/{slug}")
}

/// A port free on both loopback addresses and not among `taken`. The copies
/// publish on both, since a client can try `::1` first for `localhost`.
pub fn free_port(taken: &BTreeSet<u16>) -> Result<u16, String> {
    for _ in 0..PORT_TRIES {
        let v4 = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = v4.local_addr().map_err(|e| e.to_string())?.port();
        if !taken.contains(&port) && TcpListener::bind(("::1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("no free port found".to_owned())
}

/// What `generate` needs to know beyond the compose file.
pub struct Input<'a> {
    /// The new compose project's name (`compose_name`).
    pub name: &'a str,
    /// What `LABEL` says (`feature_label`).
    pub label: &'a str,
    /// The services running in main. The copy starts those and what they
    /// depend on; with none, all that `config` lists.
    pub running: &'a BTreeSet<String>,
    /// The sandbox's own folder, where copied bind mounts go.
    pub data_dir: &'a Path,
    /// Ports to stay off besides main's: other sandboxes', and this sandbox's
    /// other stacks', which nothing has bound yet.
    pub taken: &'a BTreeSet<u16>,
    /// Whether a writable bind mount of this folder holds data to copy,
    /// rather than something to keep sharing, like the docker socket.
    pub copies_bind: &'a dyn Fn(&Path) -> bool,
    /// A free port, none given out twice (see `free_port`).
    pub next_port: &'a mut dyn FnMut(&BTreeSet<u16>) -> Result<u16, String>,
}

/// A generated compose file and what must happen around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub compose: Value,
    /// Main's published ports and where the copy publishes them instead.
    pub forwards: Vec<Forward>,
    pub copies: Vec<VolumeCopy>,
    /// Bind mounts to copy before the copy starts.
    pub binds: Vec<BindCopy>,
    /// Volumes left empty, with the service they belong to.
    pub fresh: Vec<FreshVolume>,
}

/// One of main's published ports, in the copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub service: String,
    /// The port the app knows, which main publishes.
    pub main_port: u16,
    /// The port the copy publishes on the loopback addresses.
    pub host_port: u16,
    pub udp: bool,
}

/// A named volume of main's to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeCopy {
    pub from: String,
    pub to: String,
    /// The services mounting it: to be paused while it copies.
    pub services: Vec<String>,
}

/// A bind mount folder to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindCopy {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// A named volume that starts empty instead of as a copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshVolume {
    pub volume: String,
    pub service: String,
}

/// The sandbox's copy of `config`, the output of `docker compose config
/// --format json`, which spells out main's names for its volumes and networks.
pub fn generate(config: &Value, input: Input) -> Result<Generated, String> {
    let services = config["services"]
        .as_object()
        .ok_or("the compose file lists no services")?;
    let kept = kept_services(services, input.running);
    let mut out = config.clone();
    out["name"] = Value::String(input.name.to_owned());

    let mut main_ports = BTreeSet::new();
    for service in services.values() {
        for port in service["ports"].as_array().into_iter().flatten() {
            if let Some(published) = published(port)? {
                main_ports.insert(published);
            }
        }
    }

    let mut forwards = Vec::new();
    let mut binds = Vec::new();
    let mut used_volumes = BTreeMap::<String, Vec<String>>::new();
    let mut used_networks = BTreeSet::new();
    let mut taken = main_ports;
    taken.extend(input.taken);
    let mut new_services = Map::new();
    for (name, service) in services {
        if !kept.contains(name) {
            continue;
        }
        let mut service = service.clone();
        refuse_shared(name, &service)?;
        let fields = service
            .as_object_mut()
            .ok_or("the compose file has a service that isn't a mapping")?;
        fields.remove("container_name");
        rewrite_ports(name, fields, &mut taken, &mut forwards, input.next_port)?;
        let label = Value::String(input.label.to_owned());
        match fields.get_mut("labels").and_then(Value::as_object_mut) {
            Some(labels) => {
                labels.insert(LABEL.to_owned(), label);
            }
            None => {
                fields.insert(
                    "labels".to_owned(),
                    Value::Object(Map::from_iter([(LABEL.to_owned(), label)])),
                );
            }
        }
        rewrite_mounts(name, fields, &input, &mut used_volumes, &mut binds);
        let own_mode = fields.contains_key("network_mode");
        match fields.get_mut("networks").and_then(Value::as_object_mut) {
            Some(networks) => {
                for (network, settings) in networks.iter_mut() {
                    used_networks.insert(network.clone());
                    // A fixed address only fits a network with main's fixed
                    // subnet, which the copy can't have too.
                    if let Some(settings) = settings.as_object_mut() {
                        settings.remove("ipv4_address");
                        settings.remove("ipv6_address");
                    }
                }
            }
            None if !own_mode => {
                used_networks.insert("default".to_owned());
            }
            None => {}
        }
        new_services.insert(name.clone(), service);
    }
    out["services"] = Value::Object(new_services);

    let (volumes, copies, fresh) = rewrite_volumes(config, input.name, services, &used_volumes)?;
    set_or_remove(&mut out, "volumes", volumes);
    let networks = rewrite_networks(config, input.name, &used_networks)?;
    set_or_remove(&mut out, "networks", networks);

    check_isolated(config, &out)?;
    Ok(Generated {
        compose: out,
        forwards,
        copies,
        binds,
        fresh,
    })
}

fn set_or_remove(out: &mut Value, key: &str, value: Map<String, Value>) {
    if let Some(fields) = out.as_object_mut() {
        if value.is_empty() {
            fields.remove(key);
        } else {
            fields.insert(key.to_owned(), Value::Object(value));
        }
    }
}

/// `running` and what those need, or all `services` when nothing runs.
fn kept_services(services: &Map<String, Value>, running: &BTreeSet<String>) -> BTreeSet<String> {
    let mut kept: BTreeSet<String> = running
        .iter()
        .filter(|name| services.contains_key(*name))
        .cloned()
        .collect();
    if kept.is_empty() {
        return services.keys().cloned().collect();
    }
    let mut todo: Vec<String> = kept.iter().cloned().collect();
    while let Some(name) = todo.pop() {
        let service = &services[&name];
        let mut needs: Vec<String> = service["depends_on"]
            .as_object()
            .map(|deps| deps.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(other) = service["network_mode"]
            .as_str()
            .and_then(|mode| mode.strip_prefix("service:"))
        {
            needs.push(other.to_owned());
        }
        for need in needs {
            if services.contains_key(&need) && kept.insert(need.clone()) {
                todo.push(need);
            }
        }
    }
    kept
}

/// Refuse a service that would keep sharing the host's ports or another
/// container of main's with main, whatever is renamed around it.
fn refuse_shared(name: &str, service: &Value) -> Result<(), String> {
    let mode = service["network_mode"].as_str().unwrap_or_default();
    if mode == "host" {
        return Err(format!(
            "Service {name} uses the host's network, so it can't run beside main"
        ));
    }
    for key in ["network_mode", "pid", "ipc"] {
        if service[key]
            .as_str()
            .is_some_and(|value| value.starts_with("container:"))
        {
            return Err(format!(
                "Service {name} joins another container by name ({key}), which main is using"
            ));
        }
    }
    let joins = service["volumes_from"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|from| from.starts_with("container:"));
    if joins {
        return Err(format!(
            "Service {name} mounts the volumes of a container by name, which main is using"
        ));
    }
    Ok(())
}

/// The fixed host port `port` publishes, if it has one.
fn published(port: &Value) -> Result<Option<u16>, String> {
    let text = match &port["published"] {
        Value::Null => return Ok(None),
        Value::String(text) if text.is_empty() => return Ok(None),
        Value::String(text) => text.clone(),
        value => value.to_string(),
    };
    text.parse()
        .map(Some)
        .map_err(|_| format!("Published port {text} isn't a single port, which isn't supported"))
}

/// Publish each of a service's fixed ports on a free port of the loopback
/// addresses. One with no fixed port goes: nothing can know its number.
fn rewrite_ports(
    service: &str,
    fields: &mut Map<String, Value>,
    taken: &mut BTreeSet<u16>,
    forwards: &mut Vec<Forward>,
    next_port: &mut dyn FnMut(&BTreeSet<u16>) -> Result<u16, String>,
) -> Result<(), String> {
    let Some(ports) = fields.get("ports").and_then(Value::as_array).cloned() else {
        return Ok(());
    };
    let mut rewritten = Vec::new();
    for port in &ports {
        let Some(main_port) = published(port)? else {
            continue;
        };
        let udp = port["protocol"].as_str() == Some("udp");
        if forwards
            .iter()
            .any(|f: &Forward| f.main_port == main_port && f.udp == udp)
        {
            return Err(format!(
                "Port {main_port} is published twice, which a sandbox can't tell apart"
            ));
        }
        let host_port = next_port(taken)?;
        taken.insert(host_port);
        forwards.push(Forward {
            service: service.to_owned(),
            main_port,
            host_port,
            udp,
        });
        for address in ["127.0.0.1", "::1"] {
            let mut copy = port.clone();
            copy["host_ip"] = Value::String(address.to_owned());
            copy["published"] = Value::String(host_port.to_string());
            rewritten.push(copy);
        }
    }
    fields.insert("ports".to_owned(), Value::Array(rewritten));
    Ok(())
}

/// Note the named volumes a service mounts, and point its writable data
/// bind mounts at a copy.
fn rewrite_mounts(
    service: &str,
    fields: &mut Map<String, Value>,
    input: &Input,
    used: &mut BTreeMap<String, Vec<String>>,
    binds: &mut Vec<BindCopy>,
) {
    let Some(mounts) = fields.get_mut("volumes").and_then(Value::as_array_mut) else {
        return;
    };
    for (index, mount) in mounts.iter_mut().enumerate() {
        let source = mount["source"].as_str().map(str::to_owned);
        match (mount["type"].as_str(), source) {
            (Some("volume"), Some(source)) => {
                used.entry(source).or_default().push(service.to_owned());
            }
            (Some("bind"), Some(source)) => {
                let read_only = mount["read_only"].as_bool().unwrap_or(false);
                if read_only || !(input.copies_bind)(Path::new(&source)) {
                    continue;
                }
                let to = input.data_dir.join(format!("binds/{service}-{index}"));
                mount["source"] = Value::String(to.display().to_string());
                binds.push(BindCopy {
                    from: PathBuf::from(source),
                    to,
                });
            }
            _ => {}
        }
    }
}

type VolumeParts = (Map<String, Value>, Vec<VolumeCopy>, Vec<FreshVolume>);

/// The volumes in use, under new names, with what to copy.
fn rewrite_volumes(
    config: &Value,
    name: &str,
    services: &Map<String, Value>,
    used: &BTreeMap<String, Vec<String>>,
) -> Result<VolumeParts, String> {
    let mut out = Map::new();
    let mut copies = Vec::new();
    let mut fresh = Vec::new();
    for (key, mounting) in used {
        let main = &config["volumes"][key];
        if main["external"].as_bool().unwrap_or(false) {
            return Err(format!(
                "Volume {key} is external, so a sandbox can't tell it's safe to copy"
            ));
        }
        let from = main["name"].as_str().unwrap_or(key).to_owned();
        let to = format!("{name}_{key}");
        let mut volume = if main.is_object() {
            main.clone()
        } else {
            Value::Object(Map::new())
        };
        volume["name"] = Value::String(to.clone());
        out.insert(key.clone(), volume);
        let broker = mounting
            .iter()
            .find(|service| is_broker(services[*service]["image"].as_str().unwrap_or_default()));
        match broker {
            Some(service) => fresh.push(FreshVolume {
                volume: to,
                service: service.clone(),
            }),
            None => copies.push(VolumeCopy {
                from,
                to,
                services: mounting.clone(),
            }),
        }
    }
    Ok((out, copies, fresh))
}

/// The networks in use, under new names. A subnet main fixed would overlap
/// main's, so the copy lets docker choose.
fn rewrite_networks(
    config: &Value,
    name: &str,
    used: &BTreeSet<String>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for key in used {
        let main = &config["networks"][key];
        if main["external"].as_bool().unwrap_or(false) {
            return Err(format!(
                "Network {key} is external, so a sandbox would stay connected to main"
            ));
        }
        let mut network = if main.is_object() {
            main.clone()
        } else {
            Value::Object(Map::new())
        };
        network["name"] = Value::String(format!("{name}_{key}"));
        if let Some(ipam) = network.get_mut("ipam").and_then(Value::as_object_mut) {
            ipam.remove("config");
        }
        out.insert(key.clone(), network);
    }
    Ok(out)
}

/// Whether `image` is a message broker's.
fn is_broker(image: &str) -> bool {
    let without_tag = image.split(['@', ':']).next().unwrap_or(image);
    let base = without_tag.rsplit('/').next().unwrap_or(without_tag);
    BROKERS.contains(&base)
}

/// Refuse a generated file that still reaches anything of main's by name. The
/// last check before a file is handed out: a mistake above must end here,
/// not in two database servers on one data folder.
fn check_isolated(main: &Value, copy: &Value) -> Result<(), String> {
    let names = |config: &Value, key: &str| -> BTreeSet<String> {
        config[key]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| value["name"].as_str().unwrap_or(key).to_owned())
            .collect()
    };
    for key in ["volumes", "networks"] {
        let main_names = names(main, key);
        if let Some(shared) = names(copy, key).intersection(&main_names).next() {
            return Err(format!("The sandbox would share {shared} with main"));
        }
    }
    let services = copy["services"].as_object();
    for (name, service) in services.into_iter().flatten() {
        if service.get("container_name").is_some() {
            return Err(format!("Service {name} would keep main's container name"));
        }
        let mounts = service["volumes"].as_array().into_iter().flatten();
        for mount in mounts.filter(|mount| mount["type"] == "volume") {
            let Some(source) = mount["source"].as_str() else {
                continue;
            };
            if copy["volumes"].get(source).is_none() {
                return Err(format!(
                    "Service {name} would mount volume {source} of main"
                ));
            }
        }
    }
    let main_ports = |config: &Value| -> BTreeSet<String> {
        config["services"]
            .as_object()
            .into_iter()
            .flat_map(|services| services.values())
            .flat_map(|service| service["ports"].as_array().into_iter().flatten())
            .filter_map(|port| match &port["published"] {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .collect()
    };
    if let Some(shared) = main_ports(copy).intersection(&main_ports(main)).next() {
        return Err(format!("The sandbox would publish main's port {shared}"));
    }
    Ok(())
}

/// The `-T`, or with `udp` the `-U`, value that makes pasta lead main's ports
/// to the copies'.
pub fn pasta_spec(forwards: &[Forward], udp: bool) -> String {
    let list: Vec<String> = forwards
        .iter()
        .filter(|forward| forward.udp == udp)
        .map(|forward| format!("{}:{}", forward.main_port, forward.host_port))
        .collect();
    if list.is_empty() {
        "none".to_owned()
    } else {
        list.join(",")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// What `docker compose config --format json` says of a stack like the
    /// one this was made for.
    fn config() -> Value {
        json!({
            "name": "docker",
            "services": {
                "db": {
                    "image": "postgres:15",
                    "container_name": "main-db",
                    "networks": {"pg-network": {"ipv4_address": "10.5.0.2"}},
                    "ports": [{"mode": "ingress", "target": 5432, "published": "5433", "protocol": "tcp"}],
                    "volumes": [{"type": "volume", "source": "pgdata", "target": "/var/lib/postgresql/data"}],
                },
                "redis": {
                    "image": "redis:8.0.3",
                    "networks": {"default": null},
                    "ports": [{"mode": "ingress", "target": 6379, "published": "6379", "protocol": "tcp"}],
                    "volumes": [{"type": "volume", "source": "redisdata", "target": "/data"}],
                },
                "keycloak": {
                    "image": "keycloak/keycloak:20",
                    "networks": {"default": null},
                    "ports": [{"mode": "ingress", "target": 8080, "published": "8080", "protocol": "tcp"}],
                    "volumes": [
                        {"type": "bind", "source": "/repo/keycloak/import", "target": "/import", "read_only": true},
                        {"type": "bind", "source": "/repo/data", "target": "/data"},
                        {"type": "bind", "source": "/var/run/docker.sock", "target": "/docker.sock"},
                    ],
                },
            },
            "networks": {
                "default": {"name": "docker_default", "ipam": {}},
                "pg-network": {"name": "docker_pg-network", "ipam": {"config": [{"subnet": "10.5.0.0/16"}]}},
                "unused": {"name": "docker_unused", "external": true},
            },
            "volumes": {
                "pgdata": {"name": "docker_pgdata"},
                "redisdata": {"name": "docker_redisdata"},
            },
        })
    }

    fn generated(config: &Value, running: &[&str]) -> Result<Generated, String> {
        let running: BTreeSet<String> = running.iter().map(|s| (*s).to_owned()).collect();
        let mut next = 41000;
        let mut next_port = |_: &BTreeSet<u16>| {
            next += 1;
            Ok(next)
        };
        generate(
            config,
            Input {
                name: "gerdr-x",
                label: "ao/x",
                running: &running,
                data_dir: Path::new("/sandbox"),
                taken: &BTreeSet::new(),
                copies_bind: &|path| path.starts_with("/repo"),
                next_port: &mut next_port,
            },
        )
    }

    #[test]
    fn nothing_of_mains_stays_named() {
        let out = generated(&config(), &[]).unwrap();
        let text = out.compose.to_string();
        for main_name in [
            "docker_pgdata",
            "docker_redisdata",
            "docker_default",
            "docker_pg-network",
            "main-db",
        ] {
            assert!(!text.contains(main_name), "{main_name} still in {text}");
        }
        assert_eq!(out.compose["name"], "gerdr-x");
        assert_eq!(out.compose["volumes"]["pgdata"]["name"], "gerdr-x_pgdata");
        assert_eq!(
            out.compose["networks"]["default"]["name"],
            "gerdr-x_default"
        );
    }

    #[test]
    fn a_share_of_mains_volume_is_refused() {
        let main = config();
        let mut copy = generated(&main, &[]).unwrap().compose;
        copy["volumes"]["pgdata"]["name"] = json!("docker_pgdata");
        let err = check_isolated(&main, &copy).unwrap_err();
        assert!(err.contains("docker_pgdata"), "{err}");
    }

    #[test]
    fn a_mount_of_an_unlisted_volume_is_refused() {
        let main = config();
        let mut copy = generated(&main, &[]).unwrap().compose;
        copy["services"]["db"]["volumes"][0]["source"] = json!("docker_pgdata");
        assert!(check_isolated(&main, &copy).is_err());
    }

    #[test]
    fn a_kept_container_name_or_main_port_is_refused() {
        let main = config();
        let mut copy = generated(&main, &[]).unwrap().compose;
        copy["services"]["db"]["container_name"] = json!("main-db");
        assert!(check_isolated(&main, &copy).is_err());
        let mut copy = generated(&main, &[]).unwrap().compose;
        copy["services"]["db"]["ports"][0]["published"] = json!("5433");
        assert!(check_isolated(&main, &copy).is_err());
    }

    #[test]
    fn ports_move_to_both_loopback_addresses() {
        let out = generated(&config(), &["db"]).unwrap();
        assert_eq!(
            out.forwards,
            [Forward {
                service: "db".to_owned(),
                main_port: 5433,
                host_port: 41001,
                udp: false,
            }]
        );
        let ports = out.compose["services"]["db"]["ports"].as_array().unwrap();
        let addresses: Vec<&str> = ports
            .iter()
            .map(|p| p["host_ip"].as_str().unwrap())
            .collect();
        assert_eq!(addresses, ["127.0.0.1", "::1"]);
        assert!(
            ports
                .iter()
                .all(|p| p["published"] == "41001" && p["target"] == 5432)
        );
    }

    #[test]
    fn a_port_with_no_fixed_number_goes() {
        let mut main = config();
        main["services"]["db"]["ports"] = json!([{"target": 5432, "protocol": "tcp"}]);
        let out = generated(&main, &["db"]).unwrap();
        assert!(out.forwards.is_empty());
        assert_eq!(out.compose["services"]["db"]["ports"], json!([]));
    }

    #[test]
    fn a_port_range_is_refused() {
        let mut main = config();
        main["services"]["db"]["ports"][0]["published"] = json!("8000-8010");
        assert!(generated(&main, &[]).unwrap_err().contains("8000-8010"));
    }

    #[test]
    fn a_picked_port_never_is_one_of_mains() {
        let running = BTreeSet::new();
        let mut next_port = |taken: &BTreeSet<u16>| {
            // Main's own ports must already be in what is taken.
            assert!(taken.contains(&5433) && taken.contains(&6379) && taken.contains(&8080));
            Ok(41000 + taken.len() as u16)
        };
        let out = generate(
            &config(),
            Input {
                name: "gerdr-x",
                label: "ao/x",
                running: &running,
                data_dir: Path::new("/sandbox"),
                taken: &BTreeSet::new(),
                copies_bind: &|_| false,
                next_port: &mut next_port,
            },
        )
        .unwrap();
        let ports: BTreeSet<u16> = out.forwards.iter().map(|f| f.host_port).collect();
        assert_eq!(ports.len(), out.forwards.len());
    }

    #[test]
    fn a_picked_port_never_is_one_held_elsewhere() {
        let running = BTreeSet::new();
        let elsewhere = BTreeSet::from([41001, 41002]);
        let mut next_port =
            |taken: &BTreeSet<u16>| Ok((41001..).find(|port| !taken.contains(port)).unwrap());
        let out = generate(
            &config(),
            Input {
                name: "gerdr-x",
                label: "ao/x",
                running: &running,
                data_dir: Path::new("/sandbox"),
                taken: &elsewhere,
                copies_bind: &|_| false,
                next_port: &mut next_port,
            },
        )
        .unwrap();
        assert!(!out.forwards.is_empty());
        assert!(
            out.forwards
                .iter()
                .all(|f| !elsewhere.contains(&f.host_port))
        );
    }

    #[test]
    fn a_broker_starts_empty_and_a_database_copies() {
        let out = generated(&config(), &[]).unwrap();
        assert_eq!(
            out.copies,
            [VolumeCopy {
                from: "docker_pgdata".to_owned(),
                to: "gerdr-x_pgdata".to_owned(),
                services: vec!["db".to_owned()],
            }]
        );
        assert_eq!(
            out.fresh,
            [FreshVolume {
                volume: "gerdr-x_redisdata".to_owned(),
                service: "redis".to_owned(),
            }]
        );
    }

    #[test]
    fn brokers_are_told_by_image_name() {
        assert!(is_broker("redis:8.0.3"));
        assert!(is_broker("docker.io/library/rabbitmq:3-management"));
        assert!(is_broker("redis@sha256:abc"));
        assert!(!is_broker("redis-commander:latest"));
        assert!(!is_broker("postgres:15"));
    }

    #[test]
    fn only_writable_data_binds_copy() {
        let out = generated(&config(), &[]).unwrap();
        assert_eq!(
            out.binds,
            [BindCopy {
                from: PathBuf::from("/repo/data"),
                to: PathBuf::from("/sandbox/binds/keycloak-1"),
            }]
        );
        let mounts = &out.compose["services"]["keycloak"]["volumes"];
        assert_eq!(mounts[0]["source"], "/repo/keycloak/import");
        assert_eq!(mounts[1]["source"], "/sandbox/binds/keycloak-1");
        assert_eq!(mounts[2]["source"], "/var/run/docker.sock");
    }

    #[test]
    fn only_running_services_and_their_needs_start() {
        let mut main = config();
        main["services"]["db"]["depends_on"] = json!({"redis": {"condition": "service_started"}});
        let out = generated(&main, &["db"]).unwrap();
        let services = out.compose["services"].as_object().unwrap();
        let names: Vec<&String> = services.keys().collect();
        assert_eq!(names, ["db", "redis"]);
        assert!(out.compose["networks"].get("unused").is_none());
    }

    #[test]
    fn with_nothing_running_all_start() {
        let out = generated(&config(), &[]).unwrap();
        assert_eq!(out.compose["services"].as_object().unwrap().len(), 3);
    }

    #[test]
    fn a_fixed_subnet_and_address_go() {
        let out = generated(&config(), &[]).unwrap();
        assert_eq!(
            out.compose["networks"]["pg-network"]["ipam"].get("config"),
            None
        );
        let network = &out.compose["services"]["db"]["networks"]["pg-network"];
        assert_eq!(network.get("ipv4_address"), None);
    }

    #[test]
    fn the_label_goes_on_every_service() {
        let mut main = config();
        main["services"]["db"]["labels"] = json!({"mine": "kept"});
        let out = generated(&main, &[]).unwrap();
        assert_eq!(out.compose["services"]["db"]["labels"]["mine"], "kept");
        for service in out.compose["services"].as_object().unwrap().values() {
            assert_eq!(service["labels"][LABEL], "ao/x");
        }
    }

    #[test]
    fn what_cant_be_isolated_is_refused() {
        for (key, value, word) in [
            ("network_mode", json!("host"), "host"),
            ("network_mode", json!("container:main-db"), "container"),
            ("pid", json!("container:main-db"), "container"),
            ("volumes_from", json!(["container:main-db"]), "container"),
        ] {
            let mut main = config();
            main["services"]["db"][key] = value;
            let err = generated(&main, &["db"]).unwrap_err();
            assert!(err.contains(word), "{key}: {err}");
        }
        let mut main = config();
        main["volumes"]["pgdata"]["external"] = json!(true);
        assert!(generated(&main, &[]).unwrap_err().contains("external"));
        let mut main = config();
        main["networks"]["pg-network"]["external"] = json!(true);
        assert!(generated(&main, &[]).unwrap_err().contains("external"));
    }

    #[test]
    fn an_unused_external_network_does_not_stop_it() {
        assert!(generated(&config(), &[]).is_ok());
    }

    #[test]
    fn pasta_gets_each_protocol_apart() {
        let forwards = [
            Forward {
                service: "db".to_owned(),
                main_port: 5433,
                host_port: 41001,
                udp: false,
            },
            Forward {
                service: "r".to_owned(),
                main_port: 6379,
                host_port: 41002,
                udp: false,
            },
        ];
        assert_eq!(pasta_spec(&forwards, false), "5433:41001,6379:41002");
        assert_eq!(pasta_spec(&forwards, true), "none");
    }

    fn inspected(name: &str, service: &str, dir: &str) -> Value {
        json!({
            "Name": format!("/{name}"),
            "Config": {
                "Image": "postgres:15",
                "Labels": {
                    "com.docker.compose.project": "docker",
                    "com.docker.compose.service": service,
                    "com.docker.compose.project.working_dir": dir,
                    "com.docker.compose.project.config_files": format!("{dir}/a.yml,{dir}/b.yml"),
                    "com.docker.compose.project.environment_file": "/repo/.env",
                },
            },
        })
    }

    #[test]
    fn stacks_come_from_labels_under_the_project() {
        let inspect = json!([
            inspected("docker-db-1", "db", "/ao/aft-backend/docker"),
            inspected("docker-redis-1", "redis", "/ao/aft-backend/docker"),
            inspected("other-db-1", "db", "/elsewhere"),
            {"Name": "/plain", "Config": {"Image": "nginx", "Labels": {}}},
        ])
        .to_string();
        let containers = containers_of(&inspect).unwrap();
        let stacks = stacks_under(&containers, &[PathBuf::from("/ao")]);
        assert_eq!(stacks.len(), 1);
        let stack = &stacks[0];
        assert_eq!(stack.project, "docker");
        assert_eq!(stack.env_file.as_deref(), Some(Path::new("/repo/.env")));
        assert_eq!(stack.config_files.len(), 2);
        let names: Vec<&String> = stack.services.keys().collect();
        assert_eq!(names, ["db", "redis"]);
        let loose: Vec<&str> = loose(&containers).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(loose, ["plain"]);
    }

    fn pairs(env: &str) -> Vec<(String, String)> {
        env_pairs(env)
    }

    fn publishing(name: &str, dir: Option<&str>, ports: &[u16]) -> Container {
        Container {
            name: name.to_owned(),
            image: "postgres:15".to_owned(),
            compose: dir.map(|dir| ComposeLabels {
                project: "p".to_owned(),
                service: "db".to_owned(),
                working_dir: PathBuf::from(dir),
                config_files: Vec::new(),
                env_file: None,
            }),
            ports: ports.to_vec(),
        }
    }

    #[test]
    fn published_ports_are_read_once_each() {
        let inspect = json!([{
            "Name": "/db",
            "Config": {"Image": "postgres:15", "Labels": {}},
            "NetworkSettings": {"Ports": {
                "5432/tcp": [
                    {"HostIp": "0.0.0.0", "HostPort": "5433"},
                    {"HostIp": "::", "HostPort": "5433"},
                ],
                "443/tcp": null,
            }},
        }])
        .to_string();
        assert_eq!(containers_of(&inspect).unwrap()[0].ports, [5433]);
    }

    #[test]
    fn a_url_to_a_port_of_another_project_is_out_of_reach() {
        let env = "\
# DATABASE_URL=postgres://old@127.0.0.1:5433/off
export DATABASE_URL=\"postgres://me:hunter2@127.0.0.1:5433/app\"
CACHE=redis://localhost:6379/0
OTHER=http://mylocalhost:5433
BIND=0.0.0.0:5433
";
        let other = publishing("docker-db-1", Some("/other/docker"), &[5433, 6379]);
        let found = out_of_reach(&pairs(env), &[other], &[PathBuf::from("/app")]);
        assert_eq!(
            found,
            [
                "DATABASE_URL (docker-db-1, port 5433)",
                "CACHE (docker-db-1, port 6379)"
            ]
        );
        assert!(!found.join(" ").contains("hunter2"));
    }

    #[test]
    fn a_container_compose_did_not_start_is_out_of_reach_too() {
        let plain = publishing("plain", None, &[5433]);
        let found = out_of_reach(
            &pairs("DB=postgres://127.0.0.1:5433/app"),
            &[plain],
            &[PathBuf::from("/app")],
        );
        assert_eq!(found, ["DB (plain, port 5433)"]);
    }

    #[test]
    fn what_the_project_runs_itself_is_copied_so_in_reach() {
        let env = "DB=postgres://127.0.0.1:5433/app";
        let roots = [PathBuf::from("/app")];
        let own = publishing("own", Some("/app/docker"), &[5433]);
        assert!(out_of_reach(&pairs(env), std::slice::from_ref(&own), &roots).is_empty());
        let other = publishing("other", Some("/other"), &[5433]);
        assert!(out_of_reach(&pairs(env), &[other, own], &roots).is_empty());
    }

    #[test]
    fn a_port_alone_counts_beside_a_local_host_only() {
        let other = publishing("db", Some("/other"), &[5433]);
        let roots = [PathBuf::from("/app")];
        let found = |env: &str| out_of_reach(&pairs(env), std::slice::from_ref(&other), &roots);
        assert_eq!(
            found("DB_HOST=localhost\nDB_PORT=5433"),
            ["DB_PORT (db, port 5433)"]
        );
        assert!(found("DB_HOST=db.example.com\nDB_PORT=5433").is_empty());
        assert!(found("APP_PORT=5433").is_empty());
        assert!(found("PORT=5433").is_empty());
    }

    fn repo(name: &str, main: &str, worktree: &str) -> Repo {
        Repo {
            name: name.to_owned(),
            main: PathBuf::from(main),
            base: "main".to_owned(),
            base_commit: "abc".to_owned(),
            worktree: PathBuf::from(worktree),
            touched: true,
            branch: None,
            project: None,
        }
    }

    #[test]
    fn a_path_maps_to_the_worktree_holding_it() {
        let repos = [
            repo("api", "/ao/api", "/ao/.features/x/api"),
            repo("nested", "/ao/api/nested", "/ao/.features/x/nested"),
        ];
        assert_eq!(
            in_worktree(Path::new("/ao/api/docker/a.yml"), &repos),
            Path::new("/ao/.features/x/api/docker/a.yml")
        );
        assert_eq!(
            in_worktree(Path::new("/ao/api/nested/a.yml"), &repos),
            Path::new("/ao/.features/x/nested/a.yml")
        );
        assert_eq!(
            in_worktree(Path::new("/ao/web/a.yml"), &repos),
            Path::new("/ao/web/a.yml")
        );
        // A folder that only starts the same isn't inside.
        assert_eq!(
            in_worktree(Path::new("/ao/api2/a.yml"), &repos),
            Path::new("/ao/api2/a.yml")
        );
    }

    #[test]
    fn compose_names_are_valid_and_apart() {
        let name = compose_name("ao", "Fix Login!", "docker");
        assert!(name.starts_with("gerdr-fix-login--docker-"), "{name}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        );
        assert_ne!(name, compose_name("other", "Fix Login!", "docker"));
        assert_ne!(name, compose_name("ao", "Fix Login!", "docker2"));
    }

    #[test]
    fn a_free_port_is_not_a_taken_one() {
        let port = free_port(&BTreeSet::new()).unwrap();
        assert_ne!(port, 0);
        assert_ne!(free_port(&BTreeSet::from([port])).unwrap(), port);
    }
}
