//! The models a feature stage's agent can start on, as the picker opened
//! from its card offers them (see `Gerdr::open_model_picker`).
//!
//! Claude Code has no command listing its models, so it offers the aliases
//! that always name the latest of each. Cursor lists its own, for this
//! account; that takes it a couple of seconds, so it runs off the event loop
//! each time the picker opens, while the list last read shows. T3 Code
//! offers the models its threads ran lately, then those its model list
//! names, each as `instance/model`: its draft is set to that (see `inT3` in
//! the extension's worker.js). A model none offers can be typed.

use std::{path::Path, process::Command, time::Duration};

use smithay::reexports::calloop::channel;

use crate::{Gerdr, feature::agents::program, t3_agents::ThreadRow, t3_update::output_within};

/// A model to offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// What `--model` takes.
    pub id: String,
    /// What it is called.
    pub name: String,
}

/// What Claude Code's `--model` takes for the latest of each model.
pub fn claude_offers() -> Vec<Offer> {
    [
        ("fable", "Fable"),
        ("opus", "Opus"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ]
    .map(|(id, name)| Offer {
        id: id.to_owned(),
        name: name.to_owned(),
    })
    .to_vec()
}

/// The file in T3 Code's home naming each driver's current models.
const T3_MANIFEST: &str = "userdata/model-manifest.json";

/// T3 Code's provider instance and model in `id` (`claudeAgent/claude-opus-5-5`),
/// as its draft takes them; none for an id not so.
pub fn t3_selection(id: &str) -> Option<(&str, &str)> {
    let (instance, model) = id.split_once('/')?;
    let (instance, model) = (instance.trim(), model.trim());
    (!instance.is_empty() && !model.is_empty()).then_some((instance, model))
}

/// The model of T3 Code's `id`, as its threads name it: `claude-opus-5-5`.
pub fn t3_model_name(id: &str) -> &str {
    t3_selection(id).map_or(id, |(_, model)| model)
}

/// What T3 Code's drafts can be set to: the models `rows` (its threads, the
/// newest first) ran, then those of each driver in its model list at `home`.
// shortcut: a thread's provider is taken as its instance, which holds for
// T3 Code's own instances; a custom one's models are typed until read apart.
pub fn t3_offers(home: Option<&Path>, rows: &[ThreadRow]) -> Vec<Offer> {
    let used = (rows.iter()).filter_map(|r| Some((r.provider.clone()?, r.model.clone()?)));
    let listed = home.map_or_else(Vec::new, |home| {
        match std::fs::read(home.join(T3_MANIFEST)) {
            Ok(bytes) => t3_manifest_models(&bytes),
            Err(err) => {
                tracing::warn!("reading T3 Code's model list: {err}");
                Vec::new()
            }
        }
    });
    let mut offers: Vec<Offer> = Vec::new();
    for (instance, model) in used.chain(listed) {
        let id = format!("{instance}/{model}");
        if model != "default" && !offers.iter().any(|o| o.id == id) {
            offers.push(Offer { id, name: model });
        }
    }
    offers
}

/// The (driver, model) pairs in T3 Code's model list `bytes`.
fn t3_manifest_models(bytes: &[u8]) -> Vec<(String, String)> {
    let json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!("reading T3 Code's model list: {err}");
            return Vec::new();
        }
    };
    let Some(drivers) = json["manifest"]["currentModels"].as_object() else {
        tracing::warn!("T3 Code's model list has no manifest.currentModels");
        return Vec::new();
    };
    (drivers.iter())
        .flat_map(|(driver, models)| {
            (models.as_array().into_iter().flatten())
                .filter_map(|m| Some((driver.clone(), m.as_str()?.to_owned())))
        })
        .collect()
}

/// How long `cursor-agent --list-models` may take: it asks over the network.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// The models in what `cursor-agent --list-models` printed: its rows are
/// `id - Name`, between a heading and a tip.
fn parse_cursor_models(text: &str) -> Vec<Offer> {
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

/// What a read of Cursor's models gave: the models, or why there are none.
fn cursor_models_from(status_ok: bool, stdout: &[u8], stderr: &[u8]) -> Result<Vec<Offer>, String> {
    let offers = parse_cursor_models(&String::from_utf8_lossy(stdout));
    if status_ok && !offers.is_empty() {
        return Ok(offers);
    }
    let stderr = String::from_utf8_lossy(stderr);
    let why = (stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty());
    Err(why.unwrap_or("it listed none").to_owned())
}

/// Where a read of Cursor's models stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    #[default]
    Idle,
    Busy,
    /// The last read failed.
    Failed,
}

/// Cursor's models, as last read.
#[derive(Default)]
pub struct CursorModels {
    pub offers: Vec<Offer>,
    pub reading: Reading,
    /// Where a read sends what it gave; `None` until set up.
    tx: Option<channel::Sender<Result<Vec<Offer>, String>>>,
}

impl CursorModels {
    /// Read the models of the Cursor CLI `base` runs, off the event loop,
    /// unless a read is under way.
    pub(crate) fn read(&mut self, base: &str) {
        let Some(tx) = self.tx.clone() else {
            return;
        };
        if self.reading == Reading::Busy {
            return;
        }
        self.reading = Reading::Busy;
        let mut cmd = Command::new(program(base));
        cmd.arg("--list-models");
        std::thread::spawn(move || {
            let read = output_within(cmd, READ_LIMIT)
                .and_then(|out| cursor_models_from(out.status.success(), &out.stdout, &out.stderr));
            // The loop is gone only as gerdr quits.
            let _ = tx.send(read);
        });
    }
}

impl Gerdr {
    /// Get ready for reads of Cursor's models.
    pub fn watch_cursor_models(&mut self) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(read) = event {
                state.cursor_models_read(read);
            }
        });
        match inserted {
            Ok(_) => self.features.cursor_models.tx = Some(tx),
            Err(err) => tracing::warn!("reading Cursor's models: {err}"),
        }
    }

    /// Read the models of the Cursor CLI `base` runs, off the event loop,
    /// unless a read is under way.
    pub(crate) fn read_cursor_models(&mut self, base: &str) {
        self.features.cursor_models.read(base);
    }

    fn cursor_models_read(&mut self, read: Result<Vec<Offer>, String>) {
        let waiting = self.model_picker_open();
        let models = &mut self.features.cursor_models;
        match read {
            Ok(offers) => {
                models.offers = offers;
                models.reading = Reading::Idle;
            }
            Err(why) => {
                tracing::warn!("reading Cursor's models: {why}");
                models.reading = Reading::Failed;
                // A list read before still serves, and with the picker gone
                // no one waits for this: the log has it either way, and the
                // next opening reads again.
                if waiting && models.offers.is_empty() {
                    self.show_error(format!("Couldn't read Cursor's models: {why}"));
                }
            }
        }
        self.refresh_model_picker();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t3_offers_go_by_threads_then_its_list() {
        let dir = std::env::temp_dir().join(format!("gerdr-t3-offers-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("userdata")).unwrap();
        let manifest = r#"{"manifest":{"currentModels":{"claudeAgent":["claude-opus-5-5","claude-haiku-5-5"],"codex":["gpt-6-astra"]}}}"#;
        std::fs::write(dir.join(T3_MANIFEST), manifest).unwrap();
        let row = |provider: &str, model: &str| ThreadRow {
            provider: Some(provider.into()),
            model: Some(model.into()),
            ..Default::default()
        };
        let rows = [
            row("cursor", "grok-4.7"),
            row("cursor", "default"),
            row("claudeAgent", "claude-haiku-5-5"),
        ];
        let ids: Vec<String> = t3_offers(Some(&dir), &rows)
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(
            ids,
            [
                "cursor/grok-4.7",
                "claudeAgent/claude-haiku-5-5",
                "claudeAgent/claude-opus-5-5",
                "codex/gpt-6-astra"
            ]
        );
        // No list read: the threads' models only.
        assert_eq!(t3_offers(None, &rows[..1])[0].name, "grok-4.7");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_t3_model_is_instance_then_model() {
        assert_eq!(
            t3_selection("cursor/grok-4.7"),
            Some(("cursor", "grok-4.7"))
        );
        assert_eq!(
            t3_selection("openrouter/anthropic/claude"),
            Some(("openrouter", "anthropic/claude"))
        );
        assert_eq!(t3_selection("grok-4.7"), None);
        assert_eq!(t3_selection("/grok"), None);
        assert_eq!(t3_model_name("cursor/grok-4.7"), "grok-4.7");
    }

    #[test]
    fn cursors_list_gives_its_rows_only() {
        let text = "\nAvailable models\n\nauto - Auto (default)\ngpt-5.2 - GPT-5.2\n\n\
             Tip: use --model <id> - or /model <id> in interactive mode\n";
        assert_eq!(
            parse_cursor_models(text),
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
            cursor_models_from(false, b"", b"\n  Not logged in\nmore\n"),
            Err("Not logged in".to_owned())
        );
        assert_eq!(
            cursor_models_from(true, b"Available models\n", b""),
            Err("it listed none".to_owned())
        );
        assert!(cursor_models_from(true, b"a - A\n", b"noise").is_ok());
        // A failed run isn't trusted for what it did print.
        assert!(cursor_models_from(false, b"a - A\n", b"").is_err());
    }

    #[test]
    fn cursor_models_read_once_set_up_and_one_at_a_time() {
        let mut models = CursorModels::default();
        models.read("cursor-agent");
        assert_eq!(models.reading, Reading::Idle, "not set up: no read");
        let (tx, rx) = channel::channel();
        models.tx = Some(tx);
        models.reading = Reading::Busy;
        models.read("cursor-agent");
        drop(models);
        assert!(rx.recv().is_err(), "a read under way: no second one");
    }
}
