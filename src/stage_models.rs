//! The models a feature stage's agent can start on, as the picker opened
//! from its card offers them (see `Gerdr::open_model_picker`). Each agent
//! says what it offers (`Integration::offers`). One whose CLI lists its
//! own (`Integration::models_read`) takes a couple of seconds to, so the
//! list is read off the event loop each time the picker opens, while the
//! list last read shows. A model none offers can be typed.

use smithay::reexports::calloop::channel;

use crate::{Gerdr, integrations};

/// A model to offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// What `--model` takes.
    pub id: String,
    /// What it is called.
    pub name: String,
}

/// Where a read of the models a CLI lists stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    #[default]
    Idle,
    Busy,
    /// The last read failed.
    Failed,
}

/// What a read gave, and whose models it read.
type Read = (&'static str, Result<Vec<Offer>, String>);

/// The models a CLI listed, as last read.
#[derive(Default)]
pub struct ListedModels {
    pub offers: Vec<Offer>,
    pub reading: Reading,
    /// Where a read sends what it gave, and whose models it read; `None`
    /// until set up.
    tx: Option<channel::Sender<Read>>,
}

impl ListedModels {
    /// Read the models the CLI `base` runs lists, off the event loop, unless
    /// a read is under way.
    pub(crate) fn read(&mut self, base: &str) {
        let Some(tx) = self.tx.clone() else {
            return;
        };
        if self.reading == Reading::Busy {
            return;
        }
        self.reading = Reading::Busy;
        let agent = integrations::by_command(base);
        let base = base.to_owned();
        std::thread::spawn(move || {
            let read = agent.list_models(&base);
            // The loop is gone only as gerdr quits.
            let _ = tx.send((agent.label(), read));
        });
    }
}

impl Gerdr {
    /// Get ready for reads of the models a CLI lists.
    pub fn watch_listed_models(&mut self) {
        let (tx, rx) = channel::channel();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg((agent, read)) = event {
                state.listed_models_read(agent, read);
            }
        });
        match inserted {
            Ok(_) => self.features.listed_models.tx = Some(tx),
            Err(err) => tracing::warn!("reading the models agents list: {err}"),
        }
    }

    /// Read the models the CLI `base` runs lists, off the event loop,
    /// unless a read is under way.
    pub(crate) fn read_listed_models(&mut self, base: &str) {
        self.features.listed_models.read(base);
    }

    fn listed_models_read(&mut self, agent: &str, read: Result<Vec<Offer>, String>) {
        let waiting = self.model_picker_open();
        let models = &mut self.features.listed_models;
        match read {
            Ok(offers) => {
                models.offers = offers;
                models.reading = Reading::Idle;
            }
            Err(why) => {
                tracing::warn!("reading {agent}'s models: {why}");
                models.reading = Reading::Failed;
                // A list read before still serves, and with the picker gone
                // no one waits for this: the log has it either way, and the
                // next opening reads again.
                if waiting && models.offers.is_empty() {
                    self.show_error(format!("Couldn't read {agent}'s models: {why}"));
                }
            }
        }
        self.refresh_model_picker();
        self.refresh_chat_lists();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_models_read_once_set_up_and_one_at_a_time() {
        let mut models = ListedModels::default();
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
