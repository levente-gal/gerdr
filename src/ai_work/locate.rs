//! Where each agent CLI keeps a chat: Claude's `projects/*/<id>.jsonl`,
//! Cursor's `chats/*/<id>/store.db`, Codex's `sessions/*/*/*/rollout-*.jsonl`,
//! OpenCode's database and T3 Code's. On another machine they are looked
//! up by a script and pulled into `.staging` first.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{AgentKind, Chat};
use crate::integrations::common::{
    find::{files_under, found},
    pick::{Begun, nearest},
};

/// A Codex chat found by time began this long before its run at most.
const CODEX_BEFORE: u64 = 60;
/// A Codex chat found by time began this long after its run at most.
const CODEX_AFTER: u64 = 600;

/// When a chat found by folder and time began: from `from` to `to`, the
/// one begun nearest `near` picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: u64,
    pub to: u64,
    pub near: u64,
}

impl Span {
    /// A run's begun at `started`: from a minute before it to ten after,
    /// nearest it.
    pub fn of(started: u64) -> Self {
        Self {
            from: started.saturating_sub(CODEX_BEFORE),
            to: started + CODEX_AFTER,
            near: started,
        }
    }
}

/// A chat found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub agent: AgentKind,
    /// Its id: the one asked for, or the one found by folder and time.
    pub id: String,
    /// Its transcript, or its database.
    pub main: PathBuf,
    /// The files that go with it, by their path in `extra/`.
    pub extra: Vec<(String, PathBuf)>,
    /// What `main` is, and how it is kept.
    pub form: Form,
    /// Pieces read out of a database, by their path in `extra/`: T3 Code's
    /// thread and runs, OpenCode's session.
    pub rows: Vec<(String, Vec<u8>)>,
}

/// What a found chat's `main` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    /// Lines that only grow (Claude, Codex): kept as frames appended.
    Lines,
    /// A Cursor `store.db`: kept as a snapshot, taken again when it moved.
    Store,
    /// The chat as lines read out of `main`, a database (OpenCode).
    Made(Vec<u8>),
    /// No transcript of its own: only `rows` (a T3 thread whose agent's
    /// chat wasn't found).
    Nothing,
}

/// Where the agent CLIs keep their chats: the home's, or a test's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Homes {
    /// Claude Code's config folders: `$CLAUDE_CONFIG_DIR`, then `~/.claude`.
    pub claude: Vec<PathBuf>,
    /// `~/.cursor`.
    pub cursor: PathBuf,
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex: PathBuf,
    /// OpenCode's database.
    pub opencode: PathBuf,
    /// T3 Code's home, when it ran here.
    pub t3: Option<PathBuf>,
    /// Where pi keeps its sessions: `~/.pi/agent/sessions`.
    pub pi: PathBuf,
    /// The Antigravity CLI's home: `~/.gemini/antigravity-cli`.
    pub antigravity: PathBuf,
}

impl Homes {
    /// This machine's. None without a home.
    pub fn here() -> Option<Self> {
        let home = crate::home_dir()?;
        let env = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
        };
        let mut homes = Self::under(&home);
        if let Some(config) = env("CLAUDE_CONFIG_DIR")
            && !homes.claude.contains(&config)
        {
            homes.claude.insert(0, config);
        }
        if let Some(codex) = env("CODEX_HOME") {
            homes.codex = codex;
        }
        if let Some(data) = env("XDG_DATA_HOME") {
            homes.opencode = data.join("opencode").join("opencode.db");
        }
        if let Some(sessions) = env("PI_CODING_AGENT_SESSION_DIR") {
            homes.pi = sessions;
        } else if let Some(agent) = env("PI_CODING_AGENT_DIR") {
            homes.pi = agent.join("sessions");
        }
        homes.t3 = crate::t3_agents::home();
        Some(homes)
    }

    /// All of them under home folder `home`, as each CLI keeps them by
    /// default; no T3 Code.
    pub fn under(home: &Path) -> Self {
        Self {
            claude: vec![home.join(".claude")],
            cursor: home.join(".cursor"),
            codex: home.join(".codex"),
            opencode: home.join(".local/share/opencode/opencode.db"),
            t3: None,
            pi: home.join(".pi/agent/sessions"),
            antigravity: home.join(".gemini/antigravity-cli"),
        }
    }
}

/// `chat` among `homes`, if it is there; ids in `claimed` belong to other
/// runs. An error: a database that couldn't be read.
pub fn find(
    homes: &Homes,
    chat: &Chat,
    claimed: &HashSet<String>,
) -> Result<Option<Found>, String> {
    find_in(homes, chat, claimed, Span::of(chat.started))
}

/// `chat` as `find` finds it, one without an id begun in `span`.
pub fn find_in(
    homes: &Homes,
    chat: &Chat,
    claimed: &HashSet<String>,
    span: Span,
) -> Result<Option<Found>, String> {
    let id = chat.id.as_deref().filter(|id| plain_id(id));
    if chat.id.is_some() && id.is_none() {
        return Err(format!("{:?} isn't a chat id", chat.id));
    }
    crate::integrations::of(chat.agent).find(homes, chat, id, claimed, span)
}

/// Whether `id` can go into a path and a script as it is.
pub fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && (id.chars()).all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One file a script found there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub path: String,
    pub len: u64,
    /// In seconds since the epoch.
    pub mtime: u64,
}

/// What a script found of a chat on another machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub agent: AgentKind,
    pub id: String,
    /// Its transcript, or its `store.db`.
    pub main: Remote,
    /// A Cursor `store.db`'s `-wal`.
    pub wal: Option<Remote>,
    /// The files that go with it, by their path in `extra/`.
    pub extra: Vec<(String, Remote)>,
}

/// The shell lines that print a file as `script`'s lines go: `say <tag>
/// <rel> <path>` prints its tag, "size mtime", rel and path, apart by tabs.
const SAY: &str = r#"st() { stat -c '%s %Y' "$1" 2>/dev/null || stat -f '%z %m' "$1"; }
say() { printf '%s\t%s\t%s\t%s\n' "$1" "$(st "$3")" "$2" "$3"; }
"#;

/// The script that lists `chat`'s files on another machine, for `here`'s
/// search there. Its lines are read by `read_listing`. None for what isn't
/// kept from there: OpenCode and T3 Code chats, and ids that aren't plain.
pub fn script(chat: &Chat) -> Option<String> {
    let id = chat.id.as_deref();
    if id.is_some_and(|id| !plain_id(id)) {
        return None;
    }
    let body = crate::integrations::of(chat.agent).remote_script(chat, id)?;
    Some(format!("{SAY}{body}exit 0\n"))
}

/// Whether `rel` stays inside the folder it is a path in.
fn inside(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// What `script(chat)` printed, `out`, as the chat's files there; ids in
/// `claimed` belong to other runs. None: nothing was found.
pub fn read_listing(chat: &Chat, out: &str, claimed: &HashSet<String>) -> Option<Listing> {
    let mut main = None;
    let mut wal = None;
    let mut extra = Vec::new();
    let mut candidates: Vec<(Remote, Begun)> = Vec::new();
    let mut last: Option<Remote> = None;
    for line in out.lines() {
        if let Some(first) = line.strip_prefix("L\t") {
            if let (Some(remote), Some(meta)) = (
                last.take(),
                crate::integrations::codex::find::codex_meta(first),
            ) {
                candidates.push((remote, meta));
            }
            continue;
        }
        let mut fields = line.splitn(4, '\t');
        let (Some(tag), Some(stat), Some(rel), Some(path)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let mut stat = stat.split_whitespace().map(|n| n.parse().unwrap_or(0));
        let remote = Remote {
            path: path.to_owned(),
            len: stat.next().unwrap_or(0),
            mtime: stat.next().unwrap_or(0),
        };
        match tag {
            "M" => main = main.or(Some(remote)),
            "W" => wal = Some(remote),
            "X" if inside(rel) => extra.push((rel.to_owned(), remote)),
            "C" => last = Some(remote),
            _ => {}
        }
    }
    let (main, id) = match (main, &chat.id) {
        (Some(main), Some(id)) => (main, id.clone()),
        (None, None) if crate::integrations::of(chat.agent).remote_guesses() => {
            let at = nearest(
                chat,
                Span::of(chat.started),
                claimed,
                candidates.iter().enumerate().map(|(i, (_, m))| (i, m)),
            )?;
            let (remote, meta) = candidates.swap_remove(at);
            (remote, meta.id)
        }
        _ => return None,
    };
    Some(Listing {
        agent: chat.agent,
        id,
        main,
        wal,
        extra,
    })
}

/// Where a pulled chat's files go in its staging folder: the transcript or
/// `store.db` (its `-wal` beside it), and `extra/<rel>`.
pub fn staged_main(agent: AgentKind, staging: &Path) -> PathBuf {
    match crate::integrations::of(agent).staged_store() {
        true => staging.join("store.db"),
        false => staging.join("transcript.jsonl"),
    }
}

/// The chat `id` of `agent` pulled into `staging`.
pub fn staged(agent: AgentKind, id: &str, staging: &Path) -> Option<Found> {
    let main = staged_main(agent, staging);
    if !main.is_file() {
        return None;
    }
    let form = match crate::integrations::of(agent).staged_store() {
        true => Form::Store,
        false => Form::Lines,
    };
    let mut chat = found(agent, id, main, form);
    chat.extra = files_under(&staging.join("extra"), &[""])
        .into_iter()
        .map(|(rel, path)| (rel.trim_start_matches('/').to_owned(), path))
        .collect();
    Some(chat)
}

/// Days since the epoch as a year, month and day (Howard Hinnant's
/// `civil_from_days`).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A year, month and day as days since the epoch (`days_from_civil`).
fn days_of(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An ISO 8601 time like `2026-08-26T10:58:45.531Z` (or with `+02:00`) in
/// seconds since the epoch.
pub(crate) fn iso_secs(text: &str) -> Option<u64> {
    let num = |s: Option<&str>| s?.parse::<i64>().ok();
    let (date, time) = text.split_once('T')?;
    let mut ymd = date.splitn(3, '-');
    let (y, m, d) = (num(ymd.next())?, num(ymd.next())?, num(ymd.next())?);
    let clock_end = time.find(['Z', '+', '-']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(clock_end);
    let mut hms = clock.splitn(3, ':');
    let (h, mi) = (num(hms.next())?, num(hms.next())?);
    let s = hms
        .next()
        .map_or(Some(0), |s| s.split('.').next()?.parse::<i64>().ok())?;
    let offset = match zone.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let mut parts = zone[1..].splitn(2, ':');
            let (zh, zm) = (num(parts.next())?, num(parts.next()).unwrap_or(0));
            let secs = zh * 3600 + zm * 60;
            if *sign == b'+' { secs } else { -secs }
        }
        _ => 0,
    };
    let month = u32::try_from(m).ok()?;
    let day = u32::try_from(d).ok()?;
    let secs = days_of(y, month, day) * 86_400 + h * 3600 + mi * 60 + s - offset;
    u64::try_from(secs).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        ai_work::write::tests::scratch, feature::Stage, integrations::codex::find::codex_days,
    };

    pub(crate) fn chat(
        agent: AgentKind,
        id: Option<&str>,
        started: u64,
        folders: &[&Path],
    ) -> Chat {
        Chat {
            agent,
            id: id.map(str::to_owned),
            stage: Stage::Build,
            n: 1,
            started,
            folders: folders.iter().map(|f| f.to_path_buf()).collect(),
            machine: None,
        }
    }

    pub(crate) fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn pi_and_antigravity_chats_are_found_by_id_and_by_folder_and_time() {
        use crate::integrations::antigravity::find::transcript as antigravity_transcript;
        let home = std::env::temp_dir().join(format!("gerdr-locate-piagy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let homes = Homes::under(&home);
        let started = 1_787_741_925; // 2026-08-26T10:58:45Z
        let (app, other) = (Path::new("/w/app"), Path::new("/w/other"));
        let session = |id: &str, cwd: &str| {
            format!(
                r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-08-26T10:58:50.000Z","cwd":"{cwd}"}}"#
            )
        };
        put(
            &homes
                .pi
                .join("--w-app--/2026-08-26T10-58-50-000Z_p-1.jsonl"),
            &session("p-1", "/w/app"),
        );
        put(
            &homes
                .pi
                .join("--w-other--/2026-08-26T10-58-50-000Z_p-2.jsonl"),
            &session("p-2", "/w/other"),
        );
        let none = HashSet::new();
        let pi = |id: Option<&str>, folder: &Path, claimed: &HashSet<String>| {
            find(
                &homes,
                &chat(AgentKind::Pi, id, started, &[folder]),
                claimed,
            )
            .unwrap()
            .map(|f| f.id)
        };
        assert_eq!(pi(None, app, &none).as_deref(), Some("p-1"));
        assert_eq!(pi(None, other, &none).as_deref(), Some("p-2"));
        assert_eq!(
            pi(Some("p-2"), app, &none).as_deref(),
            Some("p-2"),
            "by id, any folder"
        );
        let held = HashSet::from(["p-1".to_owned()]);
        assert_eq!(pi(None, app, &held), None, "another pane's");
        // Antigravity: its history names each conversation's folder.
        let ms = (started + 5) * 1000;
        let history = format!(
            "{{\"timestamp\":{ms},\"workspace\":\"/w/other\",\"conversationId\":\"a-2\"}}\n\
             {{\"timestamp\":{ms},\"workspace\":\"/w/app\",\"conversationId\":\"a-1\"}}\n\
             {{\"timestamp\":{},\"workspace\":\"/w/app\",\"conversationId\":\"a-1\"}}\n",
            ms + 90_000
        );
        put(&homes.antigravity.join("history.jsonl"), &history);
        for id in ["a-1", "a-2"] {
            put(&antigravity_transcript(&homes.antigravity, id), "{}\n");
        }
        let agy = |id: Option<&str>, folder: &Path| {
            find(
                &homes,
                &chat(AgentKind::Antigravity, id, started, &[folder]),
                &none,
            )
            .unwrap()
            .map(|f| (f.id, f.main))
        };
        assert_eq!(
            agy(None, app),
            Some((
                "a-1".into(),
                antigravity_transcript(&homes.antigravity, "a-1")
            ))
        );
        assert_eq!(agy(None, other).map(|(id, _)| id).as_deref(), Some("a-2"));
        assert_eq!(agy(Some("a-3"), app), None, "no such conversation");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn times_read_as_seconds() {
        assert_eq!(iso_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_secs("2026-08-26T10:58:45.531Z"), Some(1_787_741_925));
        assert_eq!(iso_secs("2026-08-26T12:58:45+02:00"), Some(1_787_741_925));
        assert_eq!(civil(days_of(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(
            codex_days(1_787_741_925),
            vec!["2026/08/25", "2026/08/26", "2026/08/27"]
        );
    }

    #[test]
    fn listings_read_back_from_a_script() {
        let c = chat(AgentKind::Cursor, Some("c-1"), 0, &[]);
        assert!(script(&c).unwrap().contains("id='c-1'"));
        let out = "M\t100 5\t\t/h/.cursor/chats/a/c-1/store.db\n\
                   W\t7 6\t\t/h/.cursor/chats/a/c-1/store.db-wal\n\
                   X\t2 5\tmeta.json\t/h/.cursor/chats/a/c-1/meta.json\n\
                   X\t2 5\t../evil\t/etc/passwd\n";
        let listing = read_listing(&c, out, &HashSet::new()).unwrap();
        assert_eq!(listing.main.len, 100);
        assert_eq!(listing.wal.unwrap().mtime, 6);
        assert_eq!(listing.extra.len(), 1);
        assert_eq!(read_listing(&c, "", &HashSet::new()), None);

        let work = Path::new("/w");
        let started = iso_secs("2026-08-26T10:58:00Z").unwrap();
        let codex = chat(AgentKind::Codex, None, started, &[work]);
        assert!(script(&codex).unwrap().contains("2026/08/26"));
        let first = r#"{"type":"session_meta","payload":{"id":"x-1","cwd":"/w","timestamp":"2026-08-26T10:58:30Z"}}"#;
        let out =
            format!("C\t9 1\t\t/h/.codex/sessions/2026/08/26/rollout-a-x-1.jsonl\nL\t{first}\n");
        let listing = read_listing(&codex, &out, &HashSet::new()).unwrap();
        assert_eq!(listing.id, "x-1");
        assert!(script(&chat(AgentKind::OpenCode, None, 0, &[])).is_none());
    }

    #[test]
    fn the_scripts_run_in_sh() {
        let home = scratch("locate-script");
        let folder = home.join(".claude/projects/-w");
        put(&folder.join("s-1.jsonl"), "{}\n");
        put(&folder.join("s-1/subagents/a.jsonl"), "{}\n");
        let c = chat(AgentKind::Claude, Some("s-1"), 0, &[]);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script(&c).unwrap())
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        let listing = read_listing(&c, &out, &HashSet::new()).unwrap();
        assert_eq!(
            listing.main.path,
            folder.join("s-1.jsonl").to_str().unwrap()
        );
        assert_eq!(listing.main.len, 3);
        assert_eq!(listing.extra[0].0, "subagents/a.jsonl");
    }
}
