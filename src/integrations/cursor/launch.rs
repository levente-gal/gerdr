//! Starting the Cursor CLI: its other name, its launcher's words, the chat
//! id it makes, its models.

use std::{path::Path, process::Command, time::Duration};

use crate::{
    ai_work::locate::Homes, feature::agents::program, stage_models::Offer, t3_update::output_within,
};

/// What makes it start a chat it made first with `create-chat`, its id
/// kept in `id_file` (shell-quoted); `base` is its command.
pub fn new_chat(base: &str, id_file: &str) -> String {
    format!(
        " --resume \"$({} create-chat | tee {id_file})\"",
        program(base)
    )
}

/// Where its script is in `argv`, what its process shows. Its launcher ends
/// in `exec -a "$0" node --use-system-ca …/index.js "$@"`, so node's flags
/// and the script come before what the CLI was told, and the CLI refuses
/// them (`unknown option '--use-system-ca'`).
pub fn launcher_script(argv: &[&str]) -> Option<usize> {
    let name = argv.first()?.rsplit('/').next()?;
    let at = argv.iter().skip(1).position(|w| !w.starts_with('-'))? + 1;
    let ours = name == "cursor-agent" || (name == "agent" && argv[at].contains("/cursor-agent/"));
    (ours && argv[at].ends_with("/index.js")).then_some(at)
}

/// Whether `command`, whose program is `name`, runs it by its other name,
/// `agent`: told by where that name leads (`…/cursor-agent/versions/…/cursor-agent`),
/// or by its script in a line saved before gerdr dropped it.
pub fn runs_as(name: &str, command: &str) -> bool {
    name == "agent" && (command.contains("/cursor-agent/") || leads_to_cursor(command))
}

/// Whether the first word of `command` is a link into it.
fn leads_to_cursor(command: &str) -> bool {
    (crate::projects::split_command(command).first())
        .and_then(|program| std::fs::canonicalize(program).ok())
        .is_some_and(|path| path.to_string_lossy().contains("/cursor-agent/"))
}

/// The model of chat `id`, from its store under `homes`' chats (one folder
/// per working directory). None for a chat that never answered.
pub fn run_model(homes: &Homes, id: &str) -> Option<String> {
    model_in(&homes.cursor.join("chats"), id)
}

fn model_in(chats: &Path, id: &str) -> Option<String> {
    let db = (std::fs::read_dir(chats).ok()?.flatten())
        .map(|dir| dir.path().join(id).join("store.db"))
        .find(|db| db.is_file())?;
    let read = || -> rusqlite::Result<String> {
        crate::cursor_agents::open_read_only(&db)?.query_row(
            "SELECT value FROM meta WHERE key = '0'",
            [],
            |row| row.get(0),
        )
    };
    // Asked again at every read of the feature while it runs, and its agent's
    // name shows meanwhile: not worth a toast, nor a line each time.
    let meta = read()
        .inspect_err(|err| tracing::debug!("reading {}: {err}", db.display()))
        .ok()?;
    let meta: serde_json::Value =
        serde_json::from_slice(&crate::ai_work::usage::from_hex(&meta)?).ok()?;
    let model = meta["lastUsedModel"].as_str()?;
    (!model.is_empty()).then(|| model.to_owned())
}

/// How long `cursor-agent --list-models` may take: it asks over the network.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// The models its CLI `base` lists, for this account. Takes it a couple of
/// seconds.
pub fn list_models(base: &str) -> Result<Vec<Offer>, String> {
    let mut cmd = Command::new(program(base));
    cmd.arg("--list-models");
    output_within(cmd, READ_LIMIT)
        .and_then(|out| models_from(out.status.success(), &out.stdout, &out.stderr))
}

/// The models in what `cursor-agent --list-models` printed: its rows are
/// `id - Name`, between a heading and a tip.
fn parse_models(text: &str) -> Vec<Offer> {
    (text.lines())
        .filter_map(|line| {
            let (id, name) = line.trim().split_once(" - ")?;
            (!id.is_empty() && !id.contains(char::is_whitespace)).then(|| Offer {
                id: id.to_owned(),
                name: name.trim().to_owned(),
            })
        })
        .collect()
}

/// What a read of its models gave: the models, or why there are none.
fn models_from(status_ok: bool, stdout: &[u8], stderr: &[u8]) -> Result<Vec<Offer>, String> {
    let offers = parse_models(&String::from_utf8_lossy(stdout));
    if status_ok && !offers.is_empty() {
        return Ok(offers);
    }
    let stderr = String::from_utf8_lossy(stderr);
    let why = (stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty());
    Err(why.unwrap_or("it listed none").to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::ai_work::write::tests::scratch;

    /// A chat `id` under `chats` whose first meta row is `meta`, as hex;
    /// none without one.
    pub(crate) fn chat(chats: &Path, id: &str, meta: Option<&str>) {
        let chat = chats.join("hash-of-cwd").join(id);
        std::fs::create_dir_all(&chat).unwrap();
        let db = rusqlite::Connection::open(chat.join("store.db")).unwrap();
        db.execute("CREATE TABLE meta (key TEXT, value TEXT)", [])
            .unwrap();
        if let Some(meta) = meta {
            let hex: String = meta.bytes().map(|b| format!("{b:02x}")).collect();
            db.execute("INSERT INTO meta VALUES ('0', ?1)", [hex])
                .unwrap();
        }
    }

    fn chats(name: &str, id: &str, meta: Option<&str>) -> PathBuf {
        let dir = scratch(&format!("cursor-model-{name}"));
        chat(&dir, id, meta);
        dir
    }

    #[test]
    fn a_cursor_chat_names_its_last_used_model() {
        let dir = chats("cursor", "k1", Some("{\"lastUsedModel\":\"grok-4.7\"}"));
        assert_eq!(model_in(&dir, "k1").as_deref(), Some("grok-4.7"));
        assert_eq!(model_in(&dir, "other"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cursor_chat_without_a_model_names_none() {
        let empty = chats("empty", "k1", None);
        assert_eq!(model_in(&empty, "k1"), None);
        let unused = chats("unused", "k1", Some("{\"lastUsedModel\":\"\"}"));
        assert_eq!(model_in(&unused, "k1"), None);
        let broken = chats("broken", "k1", Some("not json"));
        assert_eq!(model_in(&broken, "k1"), None);
        for dir in [empty, unused, broken] {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn cursors_list_gives_its_rows_only() {
        let text = "\nAvailable models\n\nauto - Auto (default)\ngpt-5.2 - GPT-5.2\n\n\
             Tip: use --model <id> - or /model <id> in interactive mode\n";
        assert_eq!(
            parse_models(text),
            vec![
                Offer {
                    id: "auto".into(),
                    name: "Auto (default)".into()
                },
                Offer {
                    id: "gpt-5.2".into(),
                    name: "GPT-5.2".into()
                },
            ]
        );
    }

    #[test]
    fn a_read_that_gives_no_models_says_why() {
        assert_eq!(
            models_from(false, b"", b"\n  Not logged in\nmore\n"),
            Err("Not logged in".to_owned())
        );
        assert_eq!(
            models_from(true, b"Available models\n", b""),
            Err("it listed none".to_owned())
        );
        assert!(models_from(true, b"a - A\n", b"noise").is_ok());
        // A failed run isn't trusted for what it did print.
        assert!(models_from(false, b"a - A\n", b"").is_err());
    }
}
