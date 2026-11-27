//! The lists a chat offers of an agent's models or efforts where its CLI
//! takes them on a typed line (Claude Code's and Cursor's `/model <id>`,
//! Claude Code's `/effort <level>`): made by gerdr, but shown and keyed as
//! the menus the other agents draw on their screens (`model_menu`), so
//! each agent's picker looks alike.

use super::menu::Menu;
use crate::{integrations::common::screen::asked, stage_models::Offer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Model,
    Effort,
}

/// One such list while it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub what: What,
    /// What it offers, in order: what its line takes, and the name shown.
    pub all: Vec<Offer>,
    /// The id of the one the agent is on, when known.
    pub now: Option<String>,
    /// What is typed into its search.
    pub query: String,
    /// The arrow's row, among those its search shows.
    pub at: usize,
    /// What it says under its title while it offers nothing: why.
    pub note: Option<String>,
    /// What it says under its title while nothing is typed: what a pick
    /// does beyond this chat.
    pub hint: Option<String>,
}

/// What a key did to a list.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Stay,
    Close,
    /// Type this line to the agent; the name of what it picks.
    Pick {
        line: String,
        name: String,
    },
}

impl List {
    /// A list of `all`, its arrow on `now`.
    pub fn new(what: What, all: Vec<Offer>, now: Option<String>) -> Self {
        let mut list = Self {
            what,
            all,
            now,
            query: String::new(),
            at: 0,
            note: None,
            hint: None,
        };
        list.at_now();
        list
    }

    /// Its offers brought up to date (a read of the models), the arrow
    /// kept on the one it was on.
    pub fn refresh(&mut self, all: Vec<Offer>) {
        let on = self.shown().get(self.at).map(|o| o.id.clone());
        self.all = all;
        match on.and_then(|id| self.shown().iter().position(|o| o.id == id)) {
            Some(at) => self.at = at,
            None => self.at_now(),
        }
    }

    fn at_now(&mut self) {
        let now = self.now.clone();
        self.at = (self.shown().iter())
            .position(|o| Some(&o.id) == now.as_ref())
            .unwrap_or(0);
    }

    /// The offers its search shows: those whose id or name holds it.
    fn shown(&self) -> Vec<&Offer> {
        let q = self.query.to_lowercase();
        (self.all.iter())
            .filter(|o| o.id.to_lowercase().contains(&q) || o.name.to_lowercase().contains(&q))
            .collect()
    }

    /// It as the chat and the phone show a menu.
    pub fn menu(&self) -> Menu {
        let title = match self.what {
            What::Model => "Select model",
            What::Effort => "Set effort",
        };
        let mut ask = asked(title, &self.query, None);
        if let Some(hint) = self.hint.as_ref().filter(|_| self.query.is_empty()) {
            ask.push((hint.clone(), false));
        }
        let shown = self.shown();
        if shown.is_empty() {
            let why = match &self.note {
                Some(note) if self.all.is_empty() => note.clone(),
                _ => "None match".into(),
            };
            ask.push((why, false));
        }
        let choices = (shown.iter().enumerate())
            .map(|(i, o)| {
                let current = Some(&o.id) == self.now.as_ref();
                let mut c = crate::integrations::common::screen::choice(
                    o.name.clone(),
                    String::new(),
                    i == self.at,
                    current,
                );
                c.n = u8::try_from(i + 1).unwrap_or(u8::MAX);
                c
            })
            .collect();
        Menu {
            ask,
            choices,
            arrows: true,
            search: true,
            ..Menu::default()
        }
    }

    /// Key `key` pressed, as `PICKER_KEYS` name them (herdr's names).
    pub fn press(&mut self, key: &str) -> Step {
        let n = self.shown().len();
        match key {
            "Down" if n > 0 => self.at = (self.at + 1) % n,
            "Up" if n > 0 => self.at = (self.at + n - 1) % n,
            "Enter" => return self.pick(self.at),
            "backspace" => {
                self.query.pop();
                self.at_now();
            }
            "esc" if !self.query.is_empty() => {
                self.query.clear();
                self.at_now();
            }
            "esc" => return Step::Close,
            _ => {}
        }
        Step::Stay
    }

    /// Letters typed into its search.
    pub fn typed(&mut self, text: &str) {
        self.query.push_str(text);
        self.at = 0;
    }

    /// Pick row `at` (from 0) of those shown.
    pub fn pick(&self, at: usize) -> Step {
        let Some(o) = self.shown().get(at).copied() else {
            return Step::Stay;
        };
        let line = match self.what {
            What::Model => super::model::line(&o.id),
            What::Effort => effort_line(&o.id),
        };
        Step::Pick {
            line,
            name: o.name.clone(),
        }
    }
}

/// What gerdr types to set an agent's effort to `level`.
pub fn effort_line(level: &str) -> String {
    format!("/effort {level}")
}

/// `levels` as offers: each its own id and name.
pub fn levels(levels: &[&str]) -> Vec<Offer> {
    (levels.iter())
        .map(|l| Offer {
            id: (*l).to_owned(),
            name: (*l).to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(l: &List) -> Vec<String> {
        l.menu().choices.into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn a_list_searches_moves_and_picks_its_line() {
        let all = levels(&["low", "medium", "high"]);
        let mut l = List::new(What::Effort, all, Some("medium".into()));
        let m = l.menu();
        assert!(m.arrows && m.search && m.choices[1].picked);
        assert_eq!(labels(&l), ["low", "medium (current)", "high"]);
        l.press("Down");
        assert_eq!(
            l.press("Enter"),
            Step::Pick {
                line: "/effort high".into(),
                name: "high".into()
            }
        );
        l.press("Down");
        assert_eq!(l.at, 0, "the arrow wraps round");
        l.typed("h");
        assert_eq!(labels(&l), ["high"]);
        l.typed("z");
        assert_eq!(l.menu().ask.last().unwrap().0, "None match");
        assert_eq!(l.press("Enter"), Step::Stay);
        assert_eq!(l.press("esc"), Step::Stay, "Esc clears the search first");
        assert_eq!(labels(&l).len(), 3);
        assert_eq!(l.press("esc"), Step::Close);
    }

    #[test]
    fn a_model_list_types_model_and_keeps_its_arrow_on_a_refresh() {
        let o = |id: &str, name: &str| Offer {
            id: id.into(),
            name: name.into(),
        };
        let mut l = List::new(What::Model, Vec::new(), Some("gpt-5.2".into()));
        l.note = Some("Reading Cursor's models…".into());
        assert_eq!(l.menu().ask[1].0, "Reading Cursor's models…");
        l.refresh(vec![o("auto", "Auto"), o("gpt-5.2", "GPT-5.2")]);
        assert_eq!(l.at, 1, "on the one it is on once read");
        l.press("Up");
        l.refresh(vec![
            o("new", "New"),
            o("auto", "Auto"),
            o("gpt-5.2", "GPT-5.2"),
        ]);
        assert_eq!(l.at, 1, "still on Auto");
        assert_eq!(
            l.pick(2),
            Step::Pick {
                line: "/model gpt-5.2".into(),
                name: "GPT-5.2".into()
            }
        );
    }
}
