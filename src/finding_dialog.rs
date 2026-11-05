//! The dialog asking for a finding's text, from AI review's page or its
//! diff: why it is dismissed, its fix and the user's comment on it, or a
//! comment of the user's own. Esc closes it and keeps what was typed for the
//! next opening on the same thing; nothing here loses anything, since a
//! dismissed finding can be reopened.

use std::collections::HashMap;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Size},
};

use crate::{
    Gerdr,
    chrome::{self, FINDING_DIALOG_ACTION, FindingDialogHit, FindingDialogView, FindingFieldView},
    feature::{
        Stage, Status,
        review_import::{self, Decision, Finding, FindingState},
    },
    text_field::{Edit, TextField},
};

/// What the dialog asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// Why a finding is dismissed.
    Dismiss,
    /// A finding's fix and the user's comment on it.
    Edit,
    /// What an item of the user's own says.
    EditOwn,
    /// A new item of the user's own.
    Add,
}

/// What was typed and left: by project, slug, what was asked and the finding.
pub type Drafts = HashMap<(String, String, Ask, String), Vec<String>>;

/// The dialog's state, while it is open.
#[derive(Debug)]
pub struct FindingDialog {
    pub project: String,
    pub slug: String,
    pub ask: Ask,
    /// The finding it is on; empty for a new item.
    pub id: String,
    /// The finding's title; for a new item, what it is for.
    pub about: String,
    pub fields: Vec<TextField>,
    /// What the fields held when opened: what is typed beyond is a draft.
    pub from: Vec<String>,
    pub focus: usize,
    pub error: Option<String>,
    pub hover: Option<FindingDialogHit>,
    /// Saving it marks the finding to fix too.
    pub marks_fix: bool,
}

/// What the dialog asks gerdr to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingDialogOutcome {
    Cancel,
    Save,
}

impl FindingDialog {
    /// Why it can't be saved, as a sentence.
    fn problem(&self) -> Option<&'static str> {
        let empty = self.fields.first().is_none_or(|f| f.trim().is_empty());
        match self.ask {
            Ask::Dismiss if empty => Some("Say why you dismiss it."),
            Ask::Add | Ask::EditOwn if empty => Some("Say what to fix."),
            _ => None,
        }
    }

    /// Save, or say why it can't be.
    fn save(&mut self) -> Option<FindingDialogOutcome> {
        self.error = self.problem().map(str::to_owned);
        self.error.is_none().then_some(FindingDialogOutcome::Save)
    }

    fn step(&mut self, back: bool) {
        let n = self.fields.len().max(1);
        self.focus = (self.focus + if back { n - 1 } else { 1 }) % n;
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        size: Size<i32, Logical>,
    ) -> Option<FindingDialogOutcome> {
        let others = mods.alt || mods.logo;
        let enter = matches!(sym, Keysym::Return | Keysym::KP_Enter);
        match sym {
            // Closed, what was typed kept for the next opening.
            Keysym::Escape => return Some(FindingDialogOutcome::Cancel),
            _ if enter && mods.shift && !mods.ctrl && !others => {
                if let Some(field) = self.fields.get_mut(self.focus) {
                    field.newline();
                    self.error = None;
                }
            }
            _ if enter && !mods.shift && !others => return self.save(),
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::Up | Keysym::KP_Up | Keysym::Down | Keysym::KP_Down
                if !(mods.ctrl || others) =>
            {
                let up = matches!(sym, Keysym::Up | Keysym::KP_Up);
                if let Some(field) = self.fields.get_mut(self.focus)
                    && let Some(at) = chrome::finding_field_step(&field.line(), up, size)
                {
                    field.click(at, mods.shift, 1);
                }
            }
            _ => {
                if let Some(field) = self.fields.get_mut(self.focus)
                    && field.key(mods, sym, typed) == Edit::Changed
                {
                    self.error = None;
                }
            }
        }
        None
    }

    fn view(&self) -> FindingDialogView {
        let (title, labels, action): (String, &[(&str, &str, i32)], &str) = match self.ask {
            Ask::Dismiss => (
                format!("Dismiss {}", self.id),
                &[("Why?", "Why this doesn't need fixing", 4)],
                "Dismiss",
            ),
            Ask::Edit => (
                format!("Edit {}", self.id),
                &[
                    ("Fix", "What should change", 6),
                    (
                        "Your comment",
                        "Anything else the Build agent should know",
                        3,
                    ),
                ],
                if self.marks_fix {
                    "Save and fix"
                } else {
                    "Save"
                },
            ),
            Ask::EditOwn => (
                format!("Edit {}", self.id),
                &[("Comment", "What should change", 6)],
                if self.marks_fix {
                    "Save and fix"
                } else {
                    "Save"
                },
            ),
            Ask::Add => (
                "Add a comment".to_owned(),
                &[("Comment", "What should change", 6)],
                "Add",
            ),
        };
        let fields = (self.fields.iter().zip(labels))
            .map(|(field, (label, placeholder, rows))| FindingFieldView {
                label,
                line: field.line(),
                placeholder,
                rows: *rows,
            })
            .collect();
        let mut keys = Vec::new();
        if self.fields.len() > 1 {
            keys.push(("Tab", "next"));
        }
        keys.push(("Shift+Enter", "new line"));
        keys.push(("Esc", "close"));
        FindingDialogView {
            title,
            about: self.about.clone(),
            fields,
            focus: self.focus,
            action,
            error: self.error.clone(),
            hover: self.hover,
            keys,
        }
    }

    fn draft_key(&self) -> (String, String, Ask, String) {
        (
            self.project.clone(),
            self.slug.clone(),
            self.ask,
            self.id.clone(),
        )
    }

    fn texts(&self) -> Vec<String> {
        self.fields.iter().map(|f| f.line().text).collect()
    }
}

impl Gerdr {
    /// Open the dialog asking `ask` of `id` with fields holding `from`,
    /// or what was last left typed there.
    #[allow(clippy::too_many_arguments)]
    fn open_finding_dialog(
        &mut self,
        project: &str,
        slug: &str,
        ask: Ask,
        id: &str,
        about: String,
        from: Vec<String>,
        marks_fix: bool,
    ) {
        let key = (project.to_owned(), slug.to_owned(), ask, id.to_owned());
        let shown = (self.features.finding_drafts.get(&key))
            .filter(|draft| draft.len() == from.len())
            .unwrap_or(&from);
        let fields = shown
            .iter()
            .map(|text| TextField::multiline(text))
            .collect();
        self.context_menu = None;
        self.stop_key_repeat();
        self.features.finding_dialog = Some(FindingDialog {
            project: project.to_owned(),
            slug: slug.to_owned(),
            ask,
            id: id.to_owned(),
            about,
            fields,
            from,
            focus: 0,
            error: None,
            hover: None,
            marks_fix,
        });
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Finding `id` of `slug`, while its AI review is current; else says why
    /// not.
    fn finding_to_ask(&mut self, project: &str, slug: &str, id: &str) -> Option<Finding> {
        let feature = self.feature(project, slug)?;
        let title = feature.title.clone();
        if feature.status != Status::Open || feature.current() != Some(Stage::AiReview) {
            self.show_warning(format!("\u{201c}{title}\u{201d} isn't at AI review"));
            return None;
        }
        let found = feature.review.as_ref().and_then(|r| r.finding(id)).cloned();
        if found.is_none() {
            self.show_warning(format!("There's no finding {id}"));
        }
        found
    }

    /// Ask why finding `id` is dismissed.
    pub fn ask_dismiss_finding(&mut self, project: &str, slug: &str, id: &str) {
        if let Some(f) = self.finding_to_ask(project, slug, id) {
            let from = vec![String::new()];
            self.open_finding_dialog(project, slug, Ask::Dismiss, id, f.title, from, false);
        }
    }

    /// Ask for finding `id`'s fix and comment; for an item of the user's
    /// own, for what it says.
    pub fn ask_edit_finding(&mut self, project: &str, slug: &str, id: &str) {
        let Some(f) = self.finding_to_ask(project, slug, id) else {
            return;
        };
        if let Err(why) = review_import::editable(&f) {
            return self.show_warning(why);
        }
        let marks_fix = f.state == FindingState::Open;
        if f.yours {
            let text = match f.body.is_empty() {
                true => f.title.clone(),
                false => format!("{}\n{}", f.title, f.body),
            };
            let about = "It goes to the Build agent with the fixes".to_owned();
            self.open_finding_dialog(
                project,
                slug,
                Ask::EditOwn,
                id,
                about,
                vec![text],
                marks_fix,
            );
        } else {
            let from = vec![f.fix_text().to_owned(), f.note.clone()];
            self.open_finding_dialog(project, slug, Ask::Edit, id, f.title, from, marks_fix);
        }
    }

    /// Ask for a comment of the user's own, to go with the fixes.
    pub fn ask_add_comment(&mut self, project: &str, slug: &str) {
        let stage = self
            .feature(project, slug)
            .and_then(crate::feature::Feature::current);
        let about = match stage {
            Some(Stage::CodeReview) => "It goes to the Build agent when you plan fixes",
            _ => "It goes to the Build agent with the fixes, and the next review checks it",
        };
        let from = vec![String::new()];
        self.open_finding_dialog(project, slug, Ask::Add, "", about.to_owned(), from, false);
    }

    /// A key while the dialog has the keyboard.
    pub fn finding_dialog_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<FindingDialogOutcome> {
        let size = self.output_size();
        self.features
            .finding_dialog
            .as_mut()?
            .key(mods, sym, typed, size)
    }

    /// Do what the dialog asked: close it keeping what was typed, or save
    /// it and close.
    pub fn apply_finding_dialog(&mut self, outcome: FindingDialogOutcome) {
        let Some(dialog) = self.features.finding_dialog.take() else {
            return;
        };
        let (key, texts) = (dialog.draft_key(), dialog.texts());
        self.refresh_pointer_focus();
        if outcome == FindingDialogOutcome::Cancel {
            if texts == dialog.from {
                self.features.finding_drafts.remove(&key);
            } else {
                self.features.finding_drafts.insert(key, texts);
            }
            return;
        }
        self.features.finding_drafts.remove(&key);
        let (project, slug, id) = (&dialog.project, &dialog.slug, &dialog.id);
        let first = texts.first().map_or("", String::as_str);
        match dialog.ask {
            Ask::Dismiss => {
                let why = Decision::Dismiss(first.to_owned());
                self.decide_finding(project, slug, id, why);
                self.feature_findings_changed(project, slug);
            }
            Ask::Edit => {
                let note = texts.get(1).map_or("", String::as_str);
                self.edit_finding(project, slug, id, first, note);
            }
            Ask::EditOwn => self.edit_own_finding(project, slug, id, first),
            Ask::Add => self.add_finding_comment(project, slug, first),
        }
    }

    /// A left click while the dialog is open.
    pub fn finding_dialog_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(view) = self.finding_dialog_view() else {
            return;
        };
        let layout = chrome::finding_dialog_layout(&view, size);
        let shift = self.seat.get_keyboard().unwrap().modifier_state().shift;
        let Some(dialog) = self.features.finding_dialog.as_mut() else {
            return;
        };
        let outcome = match layout.hit(pos) {
            Some(FindingDialogHit::Button(FINDING_DIALOG_ACTION)) => dialog.save(),
            Some(FindingDialogHit::Button(_)) => Some(FindingDialogOutcome::Cancel),
            Some(FindingDialogHit::Field(i)) => {
                dialog.focus = i;
                if let Some(at) = chrome::finding_field_at(&view, i, size, pos)
                    && let Some(field) = dialog.fields.get_mut(i)
                {
                    field.click(at, shift, 1);
                }
                None
            }
            // Off it, it stays: a click away never throws what was typed
            // away.
            None => None,
        };
        if let Some(outcome) = outcome {
            self.apply_finding_dialog(outcome);
        }
    }

    /// The pointer moved while the dialog is open.
    pub fn finding_dialog_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(view) = self.finding_dialog_view() else {
            return;
        };
        let hit = chrome::finding_dialog_layout(&view, size).hit(pos);
        if let Some(dialog) = self.features.finding_dialog.as_mut() {
            dialog.hover = hit;
        }
    }

    /// What the pointer can press in the dialog.
    pub fn finding_dialog_controls(&self) -> Vec<chrome::Rect> {
        self.finding_dialog_view().map_or(Vec::new(), |view| {
            chrome::finding_dialog_layout(&view, self.output_size()).controls()
        })
    }

    /// The dialog's focused field, for pastes.
    pub(crate) fn finding_dialog_field_mut(&mut self) -> Option<&mut TextField> {
        let dialog = self.features.finding_dialog.as_mut()?;
        dialog.fields.get_mut(dialog.focus)
    }

    pub fn finding_dialog_view(&self) -> Option<FindingDialogView> {
        self.features
            .finding_dialog
            .as_ref()
            .map(FindingDialog::view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(ask: Ask, from: &[&str]) -> FindingDialog {
        FindingDialog {
            project: "p".into(),
            slug: "s".into(),
            ask,
            id: "r1-1".into(),
            about: "Off by one".into(),
            fields: from.iter().map(|t| TextField::multiline(t)).collect(),
            from: from.iter().map(|t| (*t).to_owned()).collect(),
            focus: 0,
            error: None,
            hover: None,
            marks_fix: true,
        }
    }

    fn press(
        d: &mut FindingDialog,
        mods: &ModifiersState,
        sym: Keysym,
    ) -> Option<FindingDialogOutcome> {
        d.key(mods, sym, sym, (1200, 800).into())
    }

    fn none() -> ModifiersState {
        ModifiersState::default()
    }

    #[test]
    fn an_empty_reason_or_comment_keeps_it_open_and_says_why() {
        for (ask, why) in [
            (Ask::Dismiss, "Say why you dismiss it."),
            (Ask::Add, "Say what to fix."),
            (Ask::EditOwn, "Say what to fix."),
        ] {
            let mut d = dialog(ask, &[" "]);
            assert_eq!(press(&mut d, &none(), Keysym::Return), None);
            assert_eq!(d.error.as_deref(), Some(why));
            // Typing clears it.
            assert_eq!(press(&mut d, &none(), Keysym::a), None);
            assert_eq!(d.error, None);
            assert_eq!(
                press(&mut d, &none(), Keysym::Return),
                Some(FindingDialogOutcome::Save)
            );
        }
    }

    #[test]
    fn an_edit_may_leave_its_fields_empty() {
        let mut d = dialog(Ask::Edit, &["", ""]);
        assert_eq!(
            press(&mut d, &none(), Keysym::Return),
            Some(FindingDialogOutcome::Save)
        );
    }

    #[test]
    fn enter_saves_shift_enter_breaks_the_line_and_tab_goes_round() {
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };
        let mut d = dialog(Ask::Edit, &["fix", "note"]);
        assert_eq!(press(&mut d, &shift, Keysym::Return), None);
        assert_eq!(d.texts()[0], "fix\n");
        assert_eq!(press(&mut d, &none(), Keysym::Tab), None);
        assert_eq!(d.focus, 1);
        assert_eq!(press(&mut d, &shift, Keysym::ISO_Left_Tab), None);
        assert_eq!(d.focus, 0);
        assert_eq!(press(&mut d, &shift, Keysym::Tab), None);
        assert_eq!(d.focus, 1);
        assert_eq!(
            press(&mut d, &ctrl, Keysym::Return),
            Some(FindingDialogOutcome::Save)
        );
        assert_eq!(
            press(&mut d, &none(), Keysym::Escape),
            Some(FindingDialogOutcome::Cancel)
        );
    }

    #[test]
    fn its_button_says_what_saving_does() {
        let mut d = dialog(Ask::Edit, &["a", "b"]);
        assert_eq!(d.view().action, "Save and fix");
        d.marks_fix = false;
        assert_eq!(d.view().action, "Save");
        assert_eq!(dialog(Ask::Dismiss, &[""]).view().action, "Dismiss");
        assert_eq!(dialog(Ask::Add, &[""]).view().action, "Add");
        assert_eq!(dialog(Ask::Edit, &["a", "b"]).view().fields.len(), 2);
    }
}
