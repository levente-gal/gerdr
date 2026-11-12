//! The git identity dialog: git refused to commit for want of a name and
//! email (`Failure::Identity`), so gerdr asks for them, keeps them in the
//! user's git config (every repository) or in the failed ones only, and
//! runs the batch that failed again.
//!
//! Its fields come filled when they can: from the repositories' own
//! config, the project's other repositories, then the GitHub account `gh`
//! is signed in to. Read and written off the event loop's thread.

use std::path::{Path, PathBuf};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, IDENTITY_SAVE, IdentityFocus, IdentityHit, IdentityView},
    git_ops::{self, Failure, Identity},
    repos::{RepoEvent, RunState},
    text_field::{Edit, TextField},
};

/// The dialog's state, while it is open.
#[derive(Debug)]
pub struct IdentityDialog {
    /// Which opening it is: an older one's answers are stale.
    pub serial: u64,
    /// The batch run again once saved, while it is the panel's.
    pub batch: Option<u64>,
    /// The repositories that failed: where "only these" keeps them.
    pub roots: Vec<PathBuf>,
    pub name: TextField,
    pub email: TextField,
    pub focus: IdentityFocus,
    /// Kept for every repository (the user's git config); else in `roots`.
    pub global: bool,
    pub error: Option<String>,
    pub hover: Option<IdentityHit>,
    /// What runs again once saved: "Commit and push".
    pub retry: String,
    /// Being written: keys and clicks wait for it.
    pub saving: bool,
}

/// What the dialog asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityOutcome {
    Cancel,
    /// Write these, for every repository or only the failed ones.
    Save {
        who: Identity,
        global: bool,
    },
}

impl IdentityDialog {
    fn new(serial: u64, batch: Option<u64>, roots: Vec<PathBuf>, retry: String) -> Self {
        Self {
            serial,
            batch,
            roots,
            name: TextField::default(),
            email: TextField::default(),
            focus: IdentityFocus::Name,
            global: true,
            error: None,
            hover: None,
            retry,
            saving: false,
        }
    }

    /// What it holds, trimmed.
    fn who(&self) -> Identity {
        Identity {
            name: self.name.trim().to_owned(),
            email: self.email.trim().to_owned(),
        }
    }

    /// Fill the fields still empty with `guess`; the keyboard goes to the
    /// first left empty.
    fn guessed(&mut self, guess: Identity) {
        if self.name.trim().is_empty() && !guess.name.is_empty() {
            self.name.set(&guess.name);
        }
        if self.email.trim().is_empty() && !guess.email.is_empty() {
            self.email.set(&guess.email);
        }
        let untouched = self.focus == IdentityFocus::Name && self.error.is_none();
        if untouched && !self.name.trim().is_empty() && self.email.trim().is_empty() {
            self.focus = IdentityFocus::Email;
        }
    }

    /// What saving would do, or why it can't, as a sentence.
    fn save(&mut self) -> Option<IdentityOutcome> {
        if self.saving {
            return None;
        }
        let who = self.who();
        if let Some(problem) = who.problem() {
            self.error = Some(problem.into());
            self.focus = if who.name.is_empty() || problem.contains("name") {
                IdentityFocus::Name
            } else {
                IdentityFocus::Email
            };
            return None;
        }
        Some(IdentityOutcome::Save {
            who,
            global: self.global,
        })
    }

    fn field_mut(&mut self) -> Option<&mut TextField> {
        match self.focus {
            IdentityFocus::Name => Some(&mut self.name),
            IdentityFocus::Email => Some(&mut self.email),
            IdentityFocus::Scope => None,
        }
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<IdentityOutcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let scope = self.focus == IdentityFocus::Scope;
        match sym {
            // Closed, what was typed kept for the next opening.
            Keysym::Escape => return Some(IdentityOutcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return self.save(),
            Keysym::Tab if !mods.shift => self.focus = self.focus.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.focus = self.focus.step(true),
            Keysym::Down | Keysym::KP_Down => self.focus = self.focus.step(false),
            Keysym::Up | Keysym::KP_Up => self.focus = self.focus.step(true),
            Keysym::j | Keysym::J if ctrl => self.focus = self.focus.step(false),
            Keysym::k | Keysym::K if ctrl => self.focus = self.focus.step(true),
            Keysym::Left | Keysym::KP_Left if scope && plain => self.global = true,
            Keysym::Right | Keysym::KP_Right if scope && plain => self.global = false,
            Keysym::space if scope && plain => self.global = !self.global,
            _ => {
                if let Some(field) = self.field_mut()
                    && field.key(mods, sym, typed) == Edit::Changed
                {
                    self.error = None;
                }
            }
        }
        None
    }

    fn view(&self) -> IdentityView {
        let only = match self.roots.as_slice() {
            [root] => format!("Only {}", repo_name(root)),
            roots => format!("Only these {}", roots.len()),
        };
        let body = if self.saving {
            "saving\u{2026}".to_owned()
        } else {
            format!(
                "They go on every commit. Once saved, {} runs again.",
                self.retry
            )
        };
        let mut keys = vec![("Tab", "next")];
        if self.focus == IdentityFocus::Scope {
            keys.push(("Space", "switch"));
        }
        keys.push(("Esc", "close"));
        IdentityView {
            body,
            name: self.name.line(),
            email: self.email.line(),
            focus: self.focus,
            global: self.global,
            only,
            error: self.error.clone(),
            hover: self.hover,
            keys,
        }
    }
}

/// A repository's name, as the panel shows it: its folder's.
fn repo_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The repositories of the batch that failed for want of a name and email.
fn identity_roots(batch: &crate::repos::Batch) -> Vec<PathBuf> {
    batch
        .runs
        .iter()
        .filter(|r| {
            matches!(
                r.state,
                RunState::Failed {
                    failure: Failure::Identity,
                    ..
                }
            )
        })
        .map(|r| r.root.clone())
        .collect()
}

impl Gerdr {
    /// The panel's batch failed somewhere for want of a name and email.
    pub(crate) fn identity_failed(&self) -> bool {
        self.repos
            .batch
            .as_ref()
            .is_some_and(|b| !identity_roots(b).is_empty())
    }

    /// Open the dialog on the panel's batch's repositories that failed for
    /// want of a name and email, its fields filled off the thread.
    pub fn open_identity(&mut self) {
        if self.repos.identity.is_some() {
            return;
        }
        let Some(batch) = &self.repos.batch else {
            return;
        };
        let roots = identity_roots(batch);
        if roots.is_empty() {
            return self.show_warning("Nothing failed for want of a name and email".into());
        }
        let (id, retry) = (batch.id, batch.kind.label().to_owned());
        let others: Vec<PathBuf> = (self.repos.items.iter())
            .map(|i| i.root.clone())
            .filter(|r| !roots.contains(r))
            .collect();
        self.repos.next_id += 1;
        let serial = self.repos.next_id;
        let mut dialog = IdentityDialog::new(serial, Some(id), roots.clone(), retry);
        if let Some((draft, global)) = self.repos.identity_draft.take() {
            dialog.guessed(draft);
            dialog.global = global;
        }
        self.overlays.context_menu = None;
        self.stop_key_repeat();
        self.repos.identity = Some(dialog);
        if let Some(tx) = self.repos_sender() {
            std::thread::spawn(move || {
                let guess = git_ops::guess_identity(&roots, &others);
                let _ = tx.send(RepoEvent::IdentityGuess { serial, guess });
            });
        }
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// The fields' guess came: filled where still empty.
    pub(crate) fn identity_guess(&mut self, serial: u64, guess: Identity) {
        if let Some(dialog) = self.repos.identity.as_mut().filter(|d| d.serial == serial) {
            dialog.guessed(guess);
        }
    }

    /// A key while the dialog has the keyboard.
    pub fn identity_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<IdentityOutcome> {
        self.repos.identity.as_mut()?.key(mods, sym, typed)
    }

    /// Do what the dialog asked: close it keeping what was typed, or write
    /// the name and email off the thread.
    pub fn apply_identity(&mut self, outcome: IdentityOutcome) {
        match outcome {
            IdentityOutcome::Cancel => {
                if let Some(dialog) = self.repos.identity.take() {
                    let who = dialog.who();
                    if !(who.name.is_empty() && who.email.is_empty()) || !dialog.global {
                        self.repos.identity_draft = Some((who, dialog.global));
                    }
                }
                self.refresh_pointer_focus();
            }
            IdentityOutcome::Save { who, global } => {
                let tx = self.repos_sender();
                let Some(dialog) = self.repos.identity.as_mut() else {
                    return;
                };
                let Some(tx) = tx else {
                    dialog.error = Some("gerdr couldn't start writing them; try again.".into());
                    return;
                };
                dialog.saving = true;
                dialog.error = None;
                let (serial, roots) = (dialog.serial, dialog.roots.clone());
                std::thread::spawn(move || {
                    let result = git_ops::set_identity(&roots, global, &who);
                    let _ = tx.send(RepoEvent::IdentitySaved { serial, result });
                });
            }
        }
    }

    /// The name and email were written, or not: closed and the batch run
    /// again, or why shown in it.
    pub(crate) fn identity_saved(&mut self, serial: u64, result: Result<(), String>) {
        let Some(dialog) = self.repos.identity.as_mut().filter(|d| d.serial == serial) else {
            return;
        };
        dialog.saving = false;
        if let Err(why) = result {
            let why = why.trim_end_matches('.');
            dialog.error = Some(format!("git couldn't save them: {why}."));
            return;
        }
        let Some(dialog) = self.repos.identity.take() else {
            return;
        };
        self.repos.identity_draft = None;
        self.refresh_pointer_focus();
        let whom = if dialog.global {
            "every repository".to_owned()
        } else {
            match dialog.roots.as_slice() {
                [root] => repo_name(root),
                roots => format!("{} repositories", roots.len()),
            }
        };
        self.show_success(format!("Name and email saved for {whom}"));
        let current = self.repos.batch.as_ref().map(|b| b.id);
        if dialog.batch.is_some() && dialog.batch == current {
            self.retry_failed();
        }
    }

    /// A left click while the dialog is open.
    pub fn identity_click(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        let Some(dialog) = self.repos.identity.as_mut() else {
            return;
        };
        if dialog.saving {
            return;
        }
        let layout = chrome::identity_layout(size, dialog.error.is_some());
        let outcome = match layout.hit(pos) {
            Some(IdentityHit::Button(IDENTITY_SAVE)) => dialog.save(),
            Some(IdentityHit::Button(_)) => Some(IdentityOutcome::Cancel),
            Some(IdentityHit::Name) => {
                dialog.focus = IdentityFocus::Name;
                None
            }
            Some(IdentityHit::Email) => {
                dialog.focus = IdentityFocus::Email;
                None
            }
            Some(IdentityHit::Scope(global)) => {
                dialog.focus = IdentityFocus::Scope;
                dialog.global = global;
                None
            }
            // Off it, it stays: a click away never throws what was typed
            // away.
            None => None,
        };
        if let Some(outcome) = outcome {
            self.apply_identity(outcome);
        }
    }

    /// The pointer moved while the dialog is open.
    pub fn identity_hover(&mut self, pos: Point<f64, Logical>) {
        let size = self.output_size();
        if let Some(dialog) = self.repos.identity.as_mut() {
            let layout = chrome::identity_layout(size, dialog.error.is_some());
            dialog.hover = layout.hit(pos);
        }
    }

    /// What the pointer can press in the dialog.
    pub fn identity_controls(&self) -> Vec<chrome::Rect> {
        let Some(dialog) = &self.repos.identity else {
            return Vec::new();
        };
        chrome::identity_layout(self.output_size(), dialog.error.is_some()).controls()
    }

    /// The dialog's focused field, for the pointer and pastes.
    pub(crate) fn identity_field_mut(&mut self) -> Option<&mut TextField> {
        self.repos.identity.as_mut()?.field_mut()
    }

    pub fn identity_view(&self) -> Option<IdentityView> {
        self.repos.identity.as_ref().map(IdentityDialog::view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> IdentityDialog {
        IdentityDialog::new(
            1,
            Some(7),
            vec!["/src/app".into()],
            "Commit and push".into(),
        )
    }

    fn press(d: &mut IdentityDialog, sym: Keysym) -> Option<IdentityOutcome> {
        d.key(&ModifiersState::default(), sym, sym)
    }

    #[test]
    fn a_guess_fills_only_empty_fields_and_moves_on_to_the_one_left() {
        let mut d = dialog();
        d.name.set("Me");
        d.guessed(Identity {
            name: "Octo Cat".into(),
            email: "".into(),
        });
        assert_eq!(&*d.name, "Me");
        assert_eq!(d.focus, IdentityFocus::Email);
        d.guessed(Identity {
            name: "Octo Cat".into(),
            email: "o@c.at".into(),
        });
        assert_eq!(&*d.email, "o@c.at");
    }

    #[test]
    fn enter_saves_only_what_git_takes() {
        let mut d = dialog();
        assert_eq!(press(&mut d, Keysym::Return), None);
        assert!(d.error.is_some());
        d.name.set("Octo Cat");
        d.email.set("octo cat");
        assert_eq!(press(&mut d, Keysym::Return), None);
        assert_eq!(d.focus, IdentityFocus::Email);
        d.email.set(" o@c.at ");
        assert_eq!(
            press(&mut d, Keysym::Return),
            Some(IdentityOutcome::Save {
                who: Identity {
                    name: "Octo Cat".into(),
                    email: "o@c.at".into(),
                },
                global: true,
            })
        );
    }

    #[test]
    fn the_scope_switches_with_space_and_arrows_only_when_focused() {
        let mut d = dialog();
        press(&mut d, Keysym::space);
        assert!(d.global);
        assert_eq!(&*d.name, " ");
        d.focus = IdentityFocus::Scope;
        press(&mut d, Keysym::space);
        assert!(!d.global);
        press(&mut d, Keysym::Left);
        assert!(d.global);
        press(&mut d, Keysym::Right);
        assert!(!d.global);
    }

    #[test]
    fn esc_closes_and_saving_waits() {
        let mut d = dialog();
        assert_eq!(press(&mut d, Keysym::Escape), Some(IdentityOutcome::Cancel));
        d.name.set("Octo Cat");
        d.email.set("o@c.at");
        d.saving = true;
        assert_eq!(press(&mut d, Keysym::Return), None);
    }

    #[test]
    fn the_scope_names_the_failed_repositories() {
        let mut d = dialog();
        assert_eq!(d.view().only, "Only app");
        d.roots.push("/src/api".into());
        assert_eq!(d.view().only, "Only these 2");
    }
}
