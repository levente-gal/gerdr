//! Keeping T3 Code up to date without losing its cards (see `t3_agents`).
//!
//! gerdr reads T3 Code's database, which T3 Code changes as it likes; a
//! version gerdr can't read shows its threads with basic status only (see
//! `t3_agents::Source`). So gerdr starts T3 Code's server at a version of
//! its choosing (`Updates::pin`, see `Gerdr::start_t3`), and moves it on
//! only to versions it read:
//!
//! - every `CHECK_EVERY` (one gerdr at a time) it asks npm for the newest
//!   version on the channel of the one running (`latest`, `nightly` or
//!   `preview`);
//! - a version making the same database migration as the one gerdr reads
//!   now fits as it is: npm says which migration each version makes;
//! - else gerdr runs it for a moment on an empty home of its own, where it
//!   makes its database, and reads that (`try_version`): nothing of the
//!   user's is touched, no thread of theirs resumed;
//! - a version that fits is downloaded (by T3 Code's own npm launcher),
//!   pinned, and the server restarted at it once none of its threads works
//!   or waits for the user. One that doesn't is said once, and its schema
//!   kept beside gerdr's files (`t3-schema-<version>.sql`) to compare with
//!   `src/t3_schema.sql`.
//!
//! "Update T3 Code" in the launcher does the same at once, asking before
//! stopping threads at work; a version that doesn't fit can still be taken.
//! What was decided is in `t3.json` beside gerdr's other files, shared by
//! every gerdr.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{herdr::AgentStatus, t3_agents::Source, toast::Level};

/// How often T3 Code's newest version is looked for.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How long after gerdr starts it first looks whether a check is due.
const FIRST_LOOK: Duration = Duration::from_secs(60);
/// How often whether a check is due is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(600);
/// T3 Code's npm package.
const REGISTRY: &str = "https://registry.npmjs.org/t3";
/// How long a download may take.
const DOWNLOAD_TIME: Duration = Duration::from_secs(600);
/// How long a version tried may take to make its database.
const TRY_TIME: Duration = Duration::from_secs(90);
/// How long a migration that npm didn't name stays the same before a
/// version tried counts as done making its database.
const SETTLE: Duration = Duration::from_secs(3);
/// How long a process asked to stop has before it is killed.
const STOP_TIME: Duration = Duration::from_secs(5);
/// How long a server asked to stop for a restart has before it is killed.
const RESTART_STOP_TIME: Duration = Duration::from_secs(10);
/// How many versions' verdicts are kept.
const VERDICTS: usize = 30;

/// Whether gerdr reads what a version of T3 Code keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// It does; the migration its database is at, if known.
    Fits { migration: Option<i64> },
    /// It doesn't, and why.
    Breaks { why: String },
}

/// What gerdr decided about T3 Code's versions, in `t3.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    /// The version gerdr starts the server at; none: the newest.
    pub pin: Option<String>,
    /// Whether newer versions wait to be asked for, instead of being
    /// taken as soon as they fit.
    pub manual: bool,
    /// When the newest version was last looked for, in seconds since 1970.
    pub checked_at: u64,
    /// The versions looked at.
    pub verdicts: BTreeMap<String, Verdict>,
    /// The versions the user was told don't fit.
    pub told: BTreeSet<String>,
}

impl Updates {
    fn path() -> PathBuf {
        crate::projects::data_dir().join("t3.json")
    }

    /// What `t3.json` says; nothing decided if it can't be read.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        let written = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|json| crate::projects::write_atomic(&path, &json));
        if let Err(err) = written {
            tracing::warn!("{}: {err}", path.display());
        }
    }

    /// Whether the newest version is to be looked for at `now`.
    pub fn due(&self, now: u64) -> bool {
        !self.manual && now.saturating_sub(self.checked_at) >= CHECK_EVERY.as_secs()
    }

    /// Note `verdict` on `version`, forgetting the oldest beyond
    /// `VERDICTS`.
    fn judge(&mut self, version: &str, verdict: Verdict) {
        self.verdicts.insert(version.to_owned(), verdict);
        while self.verdicts.len() > VERDICTS {
            let Some(oldest) = self.verdicts.keys().min_by(|a, b| compare(a, b)).cloned() else {
                break;
            };
            self.verdicts.remove(&oldest);
        }
    }
}

/// Seconds since 1970.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A version of T3 Code as npm has it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    t3code: Option<ReleaseMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct ReleaseMeta {
    #[serde(rename = "databaseMigration", default)]
    database_migration: Option<i64>,
}

impl Release {
    /// The migration its database ends at, as it says.
    pub fn migration(&self) -> Option<i64> {
        self.t3code.as_ref()?.database_migration
    }
}

/// The npm tag of the channel `version` is on.
pub fn channel(version: &str) -> &'static str {
    match version.split_once('-') {
        Some((_, pre)) if pre.starts_with("nightly") => "nightly",
        Some((_, pre)) if pre.starts_with("preview") => "preview",
        _ => "latest",
    }
}

/// How version `a` compares with `b`: numbers part by part, a release
/// after its pre-releases.
pub fn compare(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (Vec<u64>, Option<&str>) {
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        let core = core.split('.').map(|n| n.parse().unwrap_or(0)).collect();
        (core, pre)
    }
    fn part(a: &str, b: &str) -> Ordering {
        match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        }
    }
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    core_a.cmp(&core_b).then_with(|| match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            let (mut a, mut b) = (a.split('.'), b.split('.'));
            loop {
                match (a.next(), b.next()) {
                    (None, None) => break Ordering::Equal,
                    (None, Some(_)) => break Ordering::Less,
                    (Some(_), None) => break Ordering::Greater,
                    (Some(x), Some(y)) => match part(x, y) {
                        Ordering::Equal => {}
                        other => break other,
                    },
                }
            }
        }
    })
}

/// What npm says of T3 Code at `tag` (a channel or a version).
fn release(tag: &str) -> Result<Release, String> {
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "--max-time", "20"])
        .arg(format!("{REGISTRY}/{tag}"));
    let out = output_within(curl, Duration::from_secs(30))?;
    if !out.status.success() {
        return Err(last_line(&out.stderr, "npm didn't answer"));
    }
    serde_json::from_slice(&out.stdout).map_err(|err| format!("npm's answer: {err}"))
}

/// Version `version`'s `t3`, under T3 Code's home `home`.
pub fn runtime(home: &Path, version: &str) -> PathBuf {
    home.join("runtime/versions").join(version).join("t3")
}

/// Whether version `version` is downloaded whole, as T3 Code's npm launcher
/// marks it.
pub fn installed(home: &Path, version: &str) -> bool {
    let dir = home.join("runtime/versions").join(version);
    std::fs::read_to_string(dir.join(".install-complete")).is_ok_and(|done| done.trim() == version)
        && dir.join("t3").is_file()
}

/// Download version `version` into T3 Code's home `home`, unless it is
/// there: its npm launcher does, asked only for its version.
fn install(home: &Path, version: &str) -> Result<PathBuf, String> {
    if !installed(home, version) {
        if !crate::desktop::installed("npx") {
            return Err("npx isn't installed".to_owned());
        }
        let mut npx = Command::new("npx");
        npx.args(["--yes", &format!("t3@{version}"), "--version"])
            .env("T3CODE_HOME", home);
        let out = output_within(npx, DOWNLOAD_TIME)?;
        if !installed(home, version) {
            return Err(format!(
                "downloading {version}: {}",
                last_line(&out.stderr, "it didn't download")
            ));
        }
    }
    Ok(runtime(home, version))
}

/// The last line `text` says, else `otherwise`.
fn last_line(text: &[u8], otherwise: &str) -> String {
    let text = String::from_utf8_lossy(text);
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or(otherwise)
        .to_owned()
}

/// What `cmd` printed, run in a process group of its own, which is stopped
/// after `limit`.
pub(crate) fn output_within(mut cmd: Command, limit: Duration) -> Result<Output, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|err| format!("{}: {err}", cmd.get_program().to_string_lossy()))?;
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                stop(&mut child);
                return Err(format!("no answer in {}s", limit.as_secs()));
            }
            Err(err) => return Err(err.to_string()),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Stop `child`'s process group: asked first, killed after `STOP_TIME`.
fn stop(child: &mut Child) {
    let Ok(group) = i32::try_from(child.id()) else {
        return;
    };
    // Safety: signals to a process group gerdr started.
    unsafe { libc::kill(-group, libc::SIGTERM) };
    let start = Instant::now();
    while start.elapsed() < STOP_TIME {
        if matches!(child.try_wait(), Ok(Some(_))) {
            // Its helpers too, should one stay behind.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe { libc::kill(-group, libc::SIGKILL) };
    let _ = child.wait();
}

/// Whether gerdr reads what T3 Code's `t3` at `bin`, version `version`,
/// keeps: it runs a moment on an empty home of its own, making its
/// database up to migration `target` (when npm said), which is then read.
/// The migration it is at, else why not; a database that can't be read
/// leaves its schema beside gerdr's files.
pub fn try_version(bin: &Path, version: &str, target: Option<i64>) -> Result<Option<i64>, String> {
    let dir = std::env::temp_dir().join(format!("gerdr-t3-{}-{version}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let tried = try_in(&dir, bin, version, target);
    let _ = std::fs::remove_dir_all(&dir);
    tried
}

fn try_in(
    dir: &Path,
    bin: &Path,
    version: &str,
    target: Option<i64>,
) -> Result<Option<i64>, String> {
    let home = dir.join("home");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map_err(|err| format!("no free port: {err}"))?
        .port();
    let mut child = Command::new(bin)
        .arg("serve")
        .arg("--base-dir")
        .arg(&home)
        .args(["--port", &port.to_string(), "--no-browser"])
        .args(["--log-level", "error"])
        .current_dir(dir)
        .env_remove("T3CODE_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|err| format!("{}: {err}", bin.display()))?;
    let db = home.join(crate::t3_agents::DB);
    let start = Instant::now();
    let mut seen: Option<(i64, Instant)> = None;
    let made = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Err(format!("it stopped ({status}) before making its database"));
        }
        let at = db
            .exists()
            .then(|| crate::t3_agents::open_read_only(&db).ok())
            .flatten()
            .and_then(|conn| crate::t3_agents::migration(&conn));
        if let Some(at) = at {
            if target.is_some_and(|target| at >= target) {
                break Ok(());
            }
            match seen {
                Some((last, since)) if last == at => {
                    if target.is_none() && since.elapsed() >= SETTLE {
                        break Ok(());
                    }
                }
                _ => seen = Some((at, Instant::now())),
            }
        }
        if start.elapsed() >= TRY_TIME {
            break Err(format!("no database after {}s", TRY_TIME.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    stop(&mut child);
    made?;
    let conn = crate::t3_agents::open_read_only(&db).map_err(|err| err.to_string())?;
    let migration = crate::t3_agents::migration(&conn);
    match crate::t3_agents::read_rows(&conn) {
        Ok(_) => Ok(migration),
        Err(err) => {
            keep_schema(&conn, version);
            Err(err.to_string())
        }
    }
}

/// Keep the schema of the database at `conn`, made by version `version`,
/// beside gerdr's files.
fn keep_schema(conn: &rusqlite::Connection, version: &str) {
    let path = crate::projects::data_dir().join(format!("t3-schema-{version}.sql"));
    let schema: rusqlite::Result<Vec<String>> = conn
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY tbl_name, type DESC")
        .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect());
    let written = schema.map_err(std::io::Error::other).and_then(|lines| {
        crate::projects::write_atomic(&path, (lines.join(";\n") + ";\n").as_bytes())
    });
    match written {
        Ok(()) => tracing::info!("t3 code {version}'s schema: {}", path.display()),
        Err(err) => tracing::warn!("{}: {err}", path.display()),
    }
}

/// The lock one gerdr at a time looks for versions under, held until
/// dropped; none while another holds it.
fn lock() -> Option<File> {
    let dir = crate::herdr::runtime_dir()
        .map(|dir| dir.join("gerdr"))
        .unwrap_or_else(crate::projects::data_dir);
    std::fs::create_dir_all(&dir).ok()?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("t3-update.lock"))
        .ok()?;
    use std::os::fd::AsRawFd;
    // Safety: a flock on a file held open; let go of when it is dropped.
    let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    free.then_some(file)
}

/// What gerdr reads of T3 Code now.
#[derive(Clone, Debug)]
pub struct Reading {
    /// The server's version, else the one pinned.
    pub version: Option<String>,
    /// The migration its database is at.
    pub migration: Option<i64>,
    /// Whether it reads its database in full.
    pub reads: bool,
}

/// What looking for a newer T3 Code found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// None newer than the one gerdr reads; the newest.
    Current(String),
    /// A newer one gerdr reads, downloaded.
    Fits(String),
    /// A newer one gerdr can't read, and why.
    Breaks(String, String),
    /// Another gerdr is looking.
    Busy,
}

/// Look for a newer T3 Code than `now`'s, under T3 Code's home `home`,
/// trying it if npm can't say it fits; noted in `t3.json`.
pub fn look(home: &Path, now_reading: &Reading) -> Result<Found, String> {
    let Some(_lock) = lock() else {
        return Ok(Found::Busy);
    };
    let mut updates = Updates::load();
    let current = now_reading.version.clone().or_else(|| updates.pin.clone());
    let newest = release(channel(current.as_deref().unwrap_or_default()))?;
    updates.checked_at = now();
    let version = newest.version.clone();
    if current
        .as_deref()
        .is_some_and(|current| compare(&version, current) != Ordering::Greater)
    {
        updates.save();
        return Ok(Found::Current(version));
    }
    let verdict = match updates.verdicts.get(&version) {
        Some(verdict) => verdict.clone(),
        None => {
            let same = now_reading.reads
                && newest.migration().is_some()
                && newest.migration() == now_reading.migration;
            let verdict = if same {
                Verdict::Fits {
                    migration: newest.migration(),
                }
            } else {
                let bin = install(home, &version)?;
                match try_version(&bin, &version, newest.migration()) {
                    Ok(migration) => Verdict::Fits { migration },
                    Err(why) => Verdict::Breaks { why },
                }
            };
            tracing::info!("t3 code {version}: {verdict:?}");
            updates.judge(&version, verdict.clone());
            verdict
        }
    };
    let found = match verdict {
        Verdict::Fits { .. } => match install(home, &version) {
            Ok(_) => Found::Fits(version),
            Err(err) => {
                updates.save();
                return Err(err);
            }
        },
        Verdict::Breaks { why } => Found::Breaks(version, why),
    };
    updates.save();
    Ok(found)
}

/// Download version `version` under the lock, as asked for.
fn fetch(home: &Path, version: &str) -> Result<Found, String> {
    let Some(_lock) = lock() else {
        return Ok(Found::Busy);
    };
    install(home, version).map(|_| Found::Fits(version.to_owned()))
}

/// Whether process `pid` is a T3 Code server: its program is a `t3`.
fn is_t3(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
        let program = cmdline.split(|b| *b == 0).next().unwrap_or_default();
        Path::new(std::str::from_utf8(program).unwrap_or_default())
            .file_name()
            .is_some_and(|name| name == "t3")
    })
}

impl crate::Gerdr {
    /// Look whether a check for a newer T3 Code is due every `LOOK_EVERY`,
    /// from `FIRST_LOOK` on.
    pub fn watch_t3_updates(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        self.t3.updates = Updates::load();
        let tick = |_, _: &mut (), state: &mut crate::Gerdr| {
            state.t3.updates = Updates::load();
            if state.t3.updates.due(now()) {
                state.check_t3(false);
            }
            TimeoutAction::ToDuration(LOOK_EVERY)
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(FIRST_LOOK), tick)
        {
            tracing::warn!("t3 code's updates: {err}");
        }
    }

    /// What gerdr reads of T3 Code now.
    fn t3_reading(&self) -> Reading {
        Reading {
            version: self
                .t3
                .server
                .as_ref()
                .and_then(|s| s.version.clone())
                .or_else(|| self.t3.updates.pin.clone()),
            migration: self.t3.migration,
            reads: self.t3.source == Source::Database,
        }
    }

    /// Look for a newer T3 Code off the event loop, and take it if it fits
    /// (see `look`); `asked`: by the user, who hears what came of it.
    pub fn check_t3(&mut self, asked: bool) {
        let Some(home) = self.t3.home.clone() else {
            return;
        };
        let reading = self.t3_reading();
        self.off_loop_t3(asked, move || look(&home, &reading));
    }

    /// Run `work` off the event loop, then take in what it found.
    fn off_loop_t3(
        &mut self,
        asked: bool,
        work: impl FnOnce() -> Result<Found, String> + Send + 'static,
    ) {
        use smithay::reexports::calloop::channel;
        if self.t3.checking {
            if asked {
                let text = "Already looking for a newer T3 Code".to_owned();
                self.toast(text, Level::Info);
            }
            return;
        }
        let (tx, rx) = channel::channel::<Result<Found, String>>();
        let inserted = self.loop_handle.insert_source(rx, move |event, _, state| {
            if let channel::Event::Msg(found) = event {
                state.t3.checking = false;
                state.t3_found(found, asked);
            }
        });
        if let Err(err) = inserted {
            return tracing::warn!("t3 code's updates: {err}");
        }
        self.t3.checking = true;
        if asked {
            self.toast("Looking for a newer T3 Code…".to_owned(), Level::Info);
        }
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
    }

    /// Take in what looking for a newer T3 Code found.
    fn t3_found(&mut self, found: Result<Found, String>, asked: bool) {
        self.t3.updates = Updates::load();
        match found {
            Err(err) if asked => {
                let text = format!("Couldn't update T3 Code: {err}");
                self.toast(text, Level::Error);
            }
            Err(err) => tracing::info!("t3 code's updates: {err}"),
            Ok(Found::Busy) if asked => {
                let text = "Another gerdr is updating T3 Code".to_owned();
                self.toast(text, Level::Info);
            }
            Ok(Found::Busy) => {}
            Ok(Found::Current(newest)) => {
                let running = self.t3_reading().version;
                if running.is_some_and(|v| v != newest) && asked {
                    // Pinned back on purpose: the newest is there to take.
                    let text = format!("T3 Code {newest} is out");
                    let action = (
                        "Take it".to_owned(),
                        crate::menu::MenuAction::UpdateT3(Some(newest)),
                    );
                    self.toast_with(text, Level::Info, Some(action));
                } else if asked {
                    self.toast("T3 Code is up to date".to_owned(), Level::Success);
                }
            }
            Ok(Found::Fits(version)) => self.switch_t3(version, asked),
            Ok(Found::Breaks(version, why)) => {
                tracing::warn!("t3 code {version} doesn't fit gerdr: {why}");
                if !asked && self.t3.updates.told.contains(&version) {
                    return;
                }
                self.t3.updates.told.insert(version.clone());
                self.t3.updates.save();
                // The version is in the log: nightlies' names are long.
                let text = "Newer T3 Code isn't readable yet".to_owned();
                let action = (
                    "Update anyway".to_owned(),
                    crate::menu::MenuAction::UpdateT3(Some(version)),
                );
                self.toast_with(text, Level::Warning, Some(action));
            }
        }
    }

    /// "Update T3 Code": to the newest version that fits, or to `version`
    /// whether it fits or not.
    pub fn update_t3(&mut self, version: Option<String>) {
        let Some(home) = self.t3.home.clone() else {
            return;
        };
        match version {
            None => self.check_t3(true),
            Some(version) => self.off_loop_t3(true, move || fetch(&home, &version)),
        }
    }

    /// Turn taking newer versions of T3 Code without asking on or off.
    pub fn toggle_t3_updates(&mut self) {
        let mut updates = Updates::load();
        updates.manual = !updates.manual;
        updates.save();
        let text = if updates.manual {
            "T3 Code updates when you ask"
        } else {
            "T3 Code updates itself"
        };
        self.t3.updates = updates;
        self.toast(text.to_owned(), Level::Info);
    }

    /// How many of T3 Code's threads work or wait for the user.
    fn t3_busy(&self) -> usize {
        self.t3
            .rows
            .iter()
            .filter(|r| matches!(r.status(), AgentStatus::Working | AgentStatus::Blocked))
            .count()
    }

    /// Run T3 Code at downloaded version `version` from now on: restart the
    /// server at it, once its threads are idle (or, `asked`, after asking).
    fn switch_t3(&mut self, version: String, asked: bool) {
        self.t3.updates.pin = Some(version.clone());
        self.t3.updates.save();
        let server = self.t3.server.clone().filter(|_| self.t3.alive);
        let Some(server) = server else {
            let text = "Newer T3 Code is ready to start".to_owned();
            return self.toast(text, Level::Success);
        };
        if server.version.as_deref() == Some(version.as_str()) {
            if asked {
                self.toast(format!("T3 Code runs {version}"), Level::Success);
            }
            return;
        }
        self.t3.restart_at = Some(version.clone());
        match self.t3_busy() {
            0 => self.restart_t3(),
            busy if asked => self.ask_restart_t3(version, busy),
            _ => {
                let text = "T3 Code restarts once its threads are done".to_owned();
                self.toast(text, Level::Info);
            }
        }
    }

    /// Restart the server at the version waiting, if any, once no thread
    /// works or waits.
    pub fn t3_maybe_restart(&mut self) {
        if self.t3.restart_at.is_some() && self.t3.alive && self.t3_busy() == 0 {
            self.restart_t3();
        }
    }

    /// Stop T3 Code's server, then start it again at the version pinned,
    /// in the workspace showing its app (else the shown one).
    pub fn restart_t3(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let Some(version) = self.t3.restart_at.take() else {
            return;
        };
        let Some(server) = self.t3.server.clone().filter(|_| self.t3.alive) else {
            return;
        };
        if !is_t3(server.pid) {
            tracing::warn!("t3 code's server: pid {} isn't t3's", server.pid);
            return;
        }
        let w = self.t3_pane().map_or(self.active_workspace, |(w, ..)| w);
        let port = server.port();
        self.toast(format!("Restarting T3 Code at {version}"), Level::Info);
        let Ok(pid) = i32::try_from(server.pid) else {
            return;
        };
        // Safety: a signal to the server's process, checked to be t3's.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let asked_at = Instant::now();
        let wait = move |_, _: &mut (), state: &mut crate::Gerdr| {
            if server.alive() {
                if asked_at.elapsed() < RESTART_STOP_TIME {
                    return TimeoutAction::ToDuration(Duration::from_millis(250));
                }
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            state.start_t3_at(w, port);
            TimeoutAction::Drop
        };
        let timer = Timer::from_duration(Duration::from_millis(250));
        if let Err(err) = self.loop_handle.insert_source(timer, wait) {
            tracing::warn!("t3 code's server: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_then_release() {
        assert_eq!(compare("0.1.7", "0.1.6"), Ordering::Greater);
        assert_eq!(compare("0.1.10", "0.1.9"), Ordering::Greater);
        assert_eq!(compare("0.1.6", "0.1.6"), Ordering::Equal);
        assert_eq!(
            compare("0.1.7-nightly.20261102.4066", "0.1.6"),
            Ordering::Greater
        );
        assert_eq!(
            compare("0.1.7", "0.1.7-nightly.20261102.4066"),
            Ordering::Greater
        );
        assert_eq!(
            compare("0.1.7-nightly.20261102.4066", "0.1.7-nightly.20261102.4061"),
            Ordering::Greater
        );
        assert_eq!(
            compare("0.1.7-nightly.20261102.999", "0.1.7-nightly.20261101.4048"),
            Ordering::Greater
        );
    }

    #[test]
    fn a_version_stays_on_its_channel() {
        assert_eq!(channel("0.1.6"), "latest");
        assert_eq!(channel("0.1.7-nightly.20261102.4066"), "nightly");
        assert_eq!(channel("0.1.6-preview.20261030.3948"), "preview");
        assert_eq!(channel(""), "latest");
    }

    #[test]
    fn npm_says_which_migration_a_version_makes() {
        let json = r#"{"name":"t3","version":"0.1.6","t3code":{"databaseMigration":76}}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.version, "0.1.6");
        assert_eq!(release.migration(), Some(76));
        let bare: Release = serde_json::from_str(r#"{"version":"0.0.2"}"#).unwrap();
        assert_eq!(bare.migration(), None);
    }

    #[test]
    fn checks_come_every_six_hours_unless_manual() {
        let mut updates = Updates {
            checked_at: 1_000,
            ..Updates::default()
        };
        assert!(!updates.due(1_000 + 3600));
        assert!(updates.due(1_000 + 6 * 3600));
        updates.manual = true;
        assert!(!updates.due(1_000 + 6 * 3600));
    }

    #[test]
    fn only_the_newest_verdicts_are_kept() {
        let mut updates = Updates::default();
        for n in 0..VERDICTS + 3 {
            updates.judge(&format!("0.1.{n}"), Verdict::Fits { migration: None });
        }
        assert_eq!(updates.verdicts.len(), VERDICTS);
        assert!(!updates.verdicts.contains_key("0.1.0"));
        assert!(
            updates
                .verdicts
                .contains_key(&format!("0.1.{}", VERDICTS + 2))
        );
    }

    #[test]
    fn a_verdict_reads_back_from_its_file() {
        let mut updates = Updates {
            pin: Some("0.1.6".to_owned()),
            ..Updates::default()
        };
        updates.judge(
            "0.1.7",
            Verdict::Breaks {
                why: "no such column".to_owned(),
            },
        );
        let json = serde_json::to_string(&updates).unwrap();
        assert_eq!(serde_json::from_str::<Updates>(&json).unwrap(), updates);
        assert_eq!(
            serde_json::from_str::<Updates>("{}").unwrap(),
            Updates::default()
        );
    }

    /// T3 Code's newest version on `channel`, downloaded into a scratch
    /// home and tried: whether gerdr reads it. Needs the network and npx;
    /// run by hand with `cargo test -- --ignored newest`.
    fn try_newest(channel: &str) {
        let home =
            std::env::temp_dir().join(format!("gerdr-t3-newest-{channel}-{}", std::process::id()));
        std::fs::create_dir_all(home.join("userdata")).unwrap();
        let newest = release(channel).unwrap();
        let bin = install(&home, &newest.version).unwrap();
        let tried = try_version(&bin, &newest.version, newest.migration());
        let _ = std::fs::remove_dir_all(&home);
        eprintln!(
            "{} (migration {:?}): {tried:?}",
            newest.version,
            newest.migration()
        );
        assert_eq!(tried, Ok(newest.migration()));
    }

    #[test]
    #[ignore = "downloads T3 Code"]
    fn newest_latest_fits() {
        try_newest("latest");
    }

    #[test]
    #[ignore = "downloads T3 Code"]
    fn newest_nightly_fits() {
        try_newest("nightly");
    }
}
