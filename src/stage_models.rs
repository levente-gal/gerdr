//! The models a feature stage's agent can start on, as the picker opened
//! from its card offers them (see `Gerdr::open_model_picker`).
//!
//! Claude Code has no command listing its models, so it offers the aliases
//! that always name the latest of each. Cursor lists its own, for this
//! account; that takes it a couple of seconds, so it runs off the event loop
//! each time the picker opens, while the list last read shows. A model
//! neither offers can be typed.

use std::{process::Command, time::Duration};

use smithay::reexports::calloop::channel;

use crate::{Gerdr, feature::agents::program, t3_update::output_within};

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
