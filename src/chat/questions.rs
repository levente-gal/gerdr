//! The answers the user picks on a question card (Claude's AskUserQuestion,
//! `live::Question`), kept per pane chat until Submit sends them all: one
//! question shows at a time, each option a row, and a last row "Other"
//! takes the user's own words.

use std::collections::BTreeSet;

use super::live::{Answers, PendingAsk, Question};
use crate::text_field::{Line, TextField};

/// A question card's picks, for the ask `ask`; another ask starts afresh.
#[derive(Clone, Debug, Default)]
pub struct Card {
    pub ask: String,
    /// The question shown.
    pub at: usize,
    /// The row the keys are on: an option, or the options' count for
    /// Other.
    pub row: usize,
    /// Per question, its rows picked (Other is the options' count).
    pub picked: Vec<BTreeSet<usize>>,
    /// Per question, the user's own answer.
    pub other: Vec<TextField>,
}

/// What a question card shows of its `Card`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardView {
    pub at: usize,
    pub row: usize,
    /// The rows picked of the question shown.
    pub picked: BTreeSet<usize>,
    /// Its Other field.
    pub other: Line,
    /// Per question: it has an answer.
    pub answered: Vec<bool>,
}

impl CardView {
    /// Every question has its answer: Submit can go.
    pub fn ready(&self) -> bool {
        self.answered.iter().all(|a| *a)
    }
}

impl Card {
    /// Ready for `ask`: started afresh when it is another ask.
    pub fn sync(&mut self, ask: &PendingAsk) {
        if self.ask != ask.ask || self.picked.len() != ask.questions.len() {
            let n = ask.questions.len();
            *self = Card {
                ask: ask.ask.clone(),
                picked: vec![BTreeSet::new(); n],
                other: vec![TextField::default(); n],
                ..Card::default()
            };
        }
    }

    /// What `ask`'s card shows; a fresh one for an ask not synced yet.
    pub fn view(&self, ask: &PendingAsk) -> CardView {
        let mut card = self.clone();
        card.sync(ask);
        let qs = &ask.questions;
        CardView {
            at: card.at,
            row: card.row,
            picked: card.picked.get(card.at).cloned().unwrap_or_default(),
            other: card
                .other
                .get(card.at)
                .map(TextField::line)
                .unwrap_or_default(),
            answered: (0..qs.len())
                .map(|i| card.answer(qs, i).is_some())
                .collect(),
        }
    }

    /// The keys are on the Other row: what they type is its text.
    pub fn on_other(&self, qs: &[Question]) -> bool {
        qs.get(self.at).is_some_and(|q| self.row == q.options.len())
    }

    /// The Other field of the question shown.
    pub fn field(&mut self) -> Option<&mut TextField> {
        self.other.get_mut(self.at)
    }

    /// Pick row `row` of the question shown (toggle it, of several picks)
    /// and put the keys on it. A pick of one of a single-pick question's
    /// options goes on to the next question, but from the last.
    pub fn pick(&mut self, qs: &[Question], row: usize) {
        let Some(q) = qs.get(self.at) else {
            return;
        };
        if row > q.options.len() {
            return;
        }
        self.row = row;
        let picked = &mut self.picked[self.at];
        match q.multi {
            true if !picked.remove(&row) => _ = picked.insert(row),
            true => {}
            false => *picked = BTreeSet::from([row]),
        }
        if !q.multi && row < q.options.len() && self.at + 1 < qs.len() {
            self.go(qs, self.at + 1);
        }
    }

    /// The Other field's text changed: it is picked while it holds words.
    pub fn typed(&mut self, qs: &[Question]) {
        let Some(q) = qs.get(self.at) else {
            return;
        };
        let other = q.options.len();
        let words = !self.other[self.at].trim().is_empty();
        let picked = &mut self.picked[self.at];
        match (words, q.multi) {
            (true, true) => _ = picked.insert(other),
            (true, false) => *picked = BTreeSet::from([other]),
            (false, _) => _ = picked.remove(&other),
        }
    }

    /// Move the keys `by` rows, Other the last.
    pub fn step(&mut self, qs: &[Question], by: isize) {
        if let Some(q) = qs.get(self.at) {
            self.row = self.row.saturating_add_signed(by).min(q.options.len());
        }
    }

    /// Show question `to`, the keys on its first row picked.
    pub fn go(&mut self, qs: &[Question], to: usize) {
        if to < qs.len() {
            self.at = to;
            self.row = self.picked[to].first().copied().unwrap_or(0);
        }
    }

    /// Question `i`'s answer as Claude takes it: the label picked, several
    /// joined by ", ", the user's own words in Other's place; none while
    /// nothing is picked (or only an empty Other).
    pub fn answer(&self, qs: &[Question], i: usize) -> Option<String> {
        let q = qs.get(i)?;
        let picked = self.picked.get(i)?;
        let mut said: Vec<&str> = (picked.iter())
            .filter_map(|row| q.options.get(*row))
            .map(|o| o.label.as_str())
            .collect();
        let own = self.other[i].trim();
        if picked.contains(&q.options.len()) && !own.is_empty() {
            said.push(own);
        }
        (!said.is_empty()).then(|| said.join(", "))
    }

    /// The answers to all of `qs`, by each one's text; none until each has
    /// one.
    pub fn answers(&self, qs: &[Question]) -> Option<Answers> {
        (0..qs.len())
            .map(|i| Some((qs[i].question.clone(), self.answer(qs, i)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::live::QuestionOption;

    fn question(text: &str, labels: &[&str], multi: bool) -> Question {
        Question {
            question: text.into(),
            header: String::new(),
            options: (labels.iter())
                .map(|l| QuestionOption {
                    label: (*l).into(),
                    description: String::new(),
                })
                .collect(),
            multi,
        }
    }

    fn ask(questions: Vec<Question>) -> PendingAsk {
        PendingAsk {
            ask: "a1".into(),
            tool: "AskUserQuestion".into(),
            gist: String::new(),
            input: String::new(),
            questions,
            plan: false,
            since: std::time::Instant::now(),
        }
    }

    #[test]
    fn picks_answer_each_question_as_claude_takes_them() {
        let a = ask(vec![
            question("Size?", &["Small", "Big"], false),
            question("Fruits?", &["Apple", "Pear", "Fig"], true),
        ]);
        let qs = &a.questions;
        let mut card = Card::default();
        card.sync(&a);
        assert!(!card.view(&a).ready());
        assert_eq!(card.answers(qs), None);
        // One pick of a single-pick question goes on to the next.
        card.pick(qs, 1);
        assert_eq!((card.at, card.row), (1, 0));
        assert_eq!(card.view(&a).answered, [true, false]);
        // Several picks toggle and stay; they join in the options' order.
        card.pick(qs, 2);
        card.pick(qs, 0);
        card.pick(qs, 1);
        card.pick(qs, 1);
        assert_eq!(card.at, 1);
        assert_eq!(card.answer(qs, 1).as_deref(), Some("Apple, Fig"));
        // Typing in Other picks it, its words after the labels.
        card.field().unwrap().set("  kiwi ");
        card.typed(qs);
        assert_eq!(card.answer(qs, 1).as_deref(), Some("Apple, Fig, kiwi"));
        let view = card.view(&a);
        assert!(view.ready());
        assert_eq!(view.other.text, "  kiwi ");
        let answers = card.answers(qs).unwrap();
        assert_eq!(answers["Size?"], "Big");
        assert_eq!(answers["Fruits?"], "Apple, Fig, kiwi");
        // Emptied, Other is no answer.
        card.field().unwrap().clear();
        card.typed(qs);
        assert_eq!(card.answer(qs, 1).as_deref(), Some("Apple, Fig"));
        // Back on the first: its own words replace the label picked, and
        // picking Other doesn't move on.
        card.go(qs, 0);
        assert_eq!(card.row, 1);
        card.pick(qs, 2);
        assert_eq!(card.at, 0);
        assert_eq!(card.answer(qs, 0), None, "Other picked with no words");
        card.field().unwrap().set("Medium");
        card.typed(qs);
        assert_eq!(card.answer(qs, 0).as_deref(), Some("Medium"));
        // Never the word "Other".
        assert!(
            !card
                .answers(qs)
                .unwrap()
                .values()
                .any(|v| v.contains("Other"))
        );
    }

    #[test]
    fn the_last_question_doesnt_move_on_and_a_new_ask_starts_afresh() {
        let a = ask(vec![question("Size?", &["Small", "Big"], false)]);
        let qs = &a.questions;
        let mut card = Card::default();
        card.sync(&a);
        card.pick(qs, 0);
        assert_eq!((card.at, card.row), (0, 0));
        assert!(card.view(&a).ready());
        // The keys stop at Other, and at the first row.
        card.step(qs, 5);
        assert!(card.on_other(qs));
        card.step(qs, -9);
        assert_eq!(card.row, 0);
        let b = PendingAsk {
            ask: "a2".into(),
            ..a.clone()
        };
        assert!(!card.view(&b).ready());
        card.sync(&b);
        assert_eq!(card.answers(qs), None);
    }
}
