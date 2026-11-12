//! Checking a worktree's branch out in the main worktree when the worktree
//! has untracked files: a dialog lists them, name first and their folder
//! after, and asks whether they come along to the main worktree or stay
//! where they are. Those the main worktree has something at already stay
//! either way, as a link to the main worktree's own `venv` does.
//!
//! Left/Right and Tab move between the buttons, Enter presses the selected
//! one (Bring along, by default), B and L press Bring along and Leave
//! behind, Esc cancels.

use std::path::{Path, PathBuf};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, CheckoutRow, CheckoutView},
    git::{self, Untracked},
    key_table::{self, Binding},
};

/// Leave behind, after Cancel.
const LEAVE: usize = 1;
/// Bring along, the last.
const BRING: usize = 2;

/// The keys the dialog takes besides its buttons, which press them, by
/// whether Bring along can be pressed: the handler and the keys shown by
/// the buttons read them.
const KEYS: &[Binding<bool, usize>] = &[
    Binding {
        chord: "B",
        does: |can_bring| can_bring.then_some(BRING),
        footer: true,
        f1: "Bring the untracked files along",
    },
    Binding {
        chord: "L",
        does: |_| Some(LEAVE),
        footer: true,
        f1: "Leave them behind",
    },
];

/// The keys shown beside the buttons, Bring along pressable (`can_bring`)
/// or not.
pub(crate) fn keys(can_bring: bool) -> Vec<(String, String)> {
    let label = |button: &usize| match *button {
        BRING => "bring".to_owned(),
        _ => "leave".to_owned(),
    };
    (key_table::hints(KEYS, &can_bring, label).into_iter())
        .map(|(k, w)| (k.to_owned(), w))
        .collect()
}

/// The checkout dialog's state, while it is open.
#[derive(Debug)]
pub struct CheckoutDialog {
    /// The worktree whose branch is checked out.
    pub worktree: PathBuf,
    pub branch: String,
    /// The main worktree, by its name, and its folder's.
    pub main_name: String,
    pub main_folder: String,
    /// The untracked files and folders of the worktree.
    pub files: Vec<Untracked>,
    /// Those the main worktree has something at already, by path.
    pub clashes: Vec<String>,
    /// The selected button: 0 Cancel, 1 Leave behind, 2 Bring along.
    pub selected: usize,
    /// The button under the pointer.
    pub hover: Option<usize>,
}

/// What the checkout dialog asks gerdr to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Check out, moving the untracked files to the main worktree.
    Bring,
    /// Check out, leaving them in the worktree.
    Leave,
    Cancel,
}

impl CheckoutDialog {
    /// The files that can come along: those nothing is in the way of.
    fn movable(&self) -> impl Iterator<Item = &Untracked> {
        self.files
            .iter()
            .filter(|f| !self.clashes.contains(&f.path))
    }

    /// Whether any file can come along.
    fn can_bring(&self) -> bool {
        self.movable().next().is_some()
    }

    /// What pressing button `button` asks for; nothing for Bring along
    /// while it can't.
    fn press(&self, button: usize) -> Option<Outcome> {
        match button {
            BRING if self.can_bring() => Some(Outcome::Bring),
            BRING => None,
            LEAVE => Some(Outcome::Leave),
            _ => Some(Outcome::Cancel),
        }
    }

    /// Handle a key: `sym` is the key's own symbol.
    fn key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        if let Some(button) = key_table::press(KEYS, &self.can_bring(), mods, sym) {
            return self.press(button);
        }
        match sym {
            Keysym::Escape => return Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => return self.press(self.selected),
            Keysym::Left | Keysym::KP_Left | Keysym::ISO_Left_Tab => self.step(-1),
            Keysym::Tab if mods.shift => self.step(-1),
            Keysym::Right | Keysym::KP_Right | Keysym::Tab => self.step(1),
            _ => {}
        }
        None
    }

    /// Move the selection `by` buttons, wrapping around those that can be
    /// pressed.
    fn step(&mut self, by: isize) {
        let buttons = if self.can_bring() { 3 } else { 2 };
        self.selected = (self.selected as isize + by).rem_euclid(buttons) as usize;
    }
}

impl Gerdr {
    /// Ask how worktree `wt`'s untracked `files` go when its branch is
    /// checked out in the main worktree at `main`, called `main_name`.
    pub fn ask_checkout(
        &mut self,
        wt: git::Worktree,
        main: &Path,
        main_name: String,
        files: Vec<Untracked>,
    ) {
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let mut dialog = CheckoutDialog {
            branch: wt.name(),
            worktree: wt.path,
            main_name,
            main_folder: crate::paths::folder_name(main),
            files,
            clashes: git::clashes(main, &paths),
            selected: BRING,
            hover: None,
        };
        if !dialog.can_bring() {
            dialog.selected = LEAVE;
        }
        self.context_menu = None;
        self.checkout = Some(dialog);
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// A key while the checkout dialog has the keyboard.
    pub fn checkout_key(&mut self, mods: &ModifiersState, sym: Keysym) -> Option<Outcome> {
        self.checkout.as_mut()?.key(mods, sym)
    }

    /// Do what the checkout dialog asked.
    pub fn apply_checkout(&mut self, outcome: Outcome) {
        let Some(dialog) = self.checkout.take() else {
            return;
        };
        // Gone, what is under the pointer gets it back.
        self.refresh_pointer_focus();
        let files = match outcome {
            Outcome::Cancel => return,
            Outcome::Leave => Vec::new(),
            Outcome::Bring => dialog.movable().cloned().collect(),
        };
        self.checkout_worktree_at(&dialog.worktree, dialog.main_name, files);
    }

    /// Where the checkout dialog's parts go, as it asks now.
    fn checkout_layout(&self) -> Option<chrome::CheckoutLayout> {
        let rows = self.checkout.as_ref()?.files.len();
        Some(chrome::checkout_layout(self.output_size(), rows))
    }

    /// A left click while the checkout dialog is open; `false` if it isn't.
    pub fn checkout_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(layout) = self.checkout_layout() else {
            return false;
        };
        let Some(dialog) = self.checkout.as_mut() else {
            return false;
        };
        let outcome = match checkout_hit(&layout, pos) {
            Some(button) => dialog.press(button),
            None if layout.panel.to_f64().contains(pos) => None,
            None => Some(Outcome::Cancel),
        };
        if let Some(outcome) = outcome {
            self.apply_checkout(outcome);
        }
        true
    }

    /// The pointer moved while the checkout dialog is open.
    pub fn checkout_hover(&mut self, pos: Point<f64, Logical>) {
        let Some(layout) = self.checkout_layout() else {
            return;
        };
        if let Some(dialog) = self.checkout.as_mut() {
            dialog.hover = checkout_hit(&layout, pos);
        }
    }

    /// The controls of the checkout dialog, while it is open: what the
    /// pointer can press.
    pub fn checkout_controls(&self) -> Vec<chrome::Rect> {
        self.checkout_layout()
            .map_or_else(Vec::new, |layout| layout.buttons)
    }

    pub fn checkout_view(&self) -> Option<CheckoutView> {
        let dialog = self.checkout.as_ref()?;
        let rows = dialog
            .files
            .iter()
            .map(|f| CheckoutRow {
                path: f.path.clone(),
                files: f.files,
                link: f.link,
                clash: dialog.clashes.contains(&f.path),
            })
            .collect();
        Some(CheckoutView {
            branch: dialog.branch.clone(),
            main: dialog.main_folder.clone(),
            worktree: crate::paths::folder_name(&dialog.worktree),
            rows,
            can_bring: dialog.can_bring(),
            selected: dialog.selected,
            hover: dialog.hover,
        })
    }
}

/// The last part of folder `dir`.
/// The button `pos` (output coordinates) is over in the checkout dialog.
fn checkout_hit(layout: &chrome::CheckoutLayout, pos: Point<f64, Logical>) -> Option<usize> {
    layout.buttons.iter().position(|b| b.to_f64().contains(pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(clashes: &[&str]) -> CheckoutDialog {
        let file = |path: &str| Untracked {
            path: path.into(),
            files: 1,
            link: false,
        };
        let clashes: Vec<String> = clashes.iter().map(|c| c.to_string()).collect();
        CheckoutDialog {
            worktree: "/r/wt".into(),
            branch: "topic".into(),
            main_name: "main".into(),
            main_folder: "r".into(),
            files: vec![file("dev/a.py"), file("venv")],
            selected: if clashes.is_empty() { BRING } else { LEAVE },
            clashes,
            hover: None,
        }
    }

    fn press(dialog: &mut CheckoutDialog, sym: Keysym) -> Option<Outcome> {
        dialog.key(&ModifiersState::default(), sym)
    }

    #[test]
    fn the_key_table_agrees_with_itself() {
        key_table::check(KEYS, &[true, false]);
        assert_eq!(keys(false), [("L".to_owned(), "leave".to_owned())]);
    }

    #[test]
    fn keys_pick_a_button() {
        let mut d = dialog(&[]);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Bring));
        assert_eq!(press(&mut d, Keysym::Left), None);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Leave));
        assert_eq!(press(&mut d, Keysym::Tab), None);
        assert_eq!(d.selected, BRING);
        assert_eq!(press(&mut d, Keysym::Tab), None);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Cancel));
        assert_eq!(press(&mut d, Keysym::l), Some(Outcome::Leave));
        assert_eq!(press(&mut d, Keysym::b), Some(Outcome::Bring));
        assert_eq!(press(&mut d, Keysym::Escape), Some(Outcome::Cancel));
    }

    #[test]
    fn a_clash_stays_and_the_rest_come() {
        let d = dialog(&["venv"]);
        assert!(d.can_bring());
        let movable: Vec<&str> = d.movable().map(|f| f.path.as_str()).collect();
        assert_eq!(movable, ["dev/a.py"]);
    }

    #[test]
    fn all_in_the_way_keeps_them_behind() {
        let mut d = dialog(&["dev/a.py", "venv"]);
        assert_eq!(press(&mut d, Keysym::b), None);
        assert_eq!(press(&mut d, Keysym::Return), Some(Outcome::Leave));
        // The selection skips Bring along.
        press(&mut d, Keysym::Right);
        press(&mut d, Keysym::Right);
        assert_eq!(d.selected, LEAVE);
        assert_eq!(d.press(BRING), None);
    }
}
