//! gerdr's own text fields (the switcher's query, the welcome screen's and
//! the project menu's fields): a line edited with the keys text fields
//! take everywhere, and what they show.
//!
//! A `TextField` keeps its text, a caret and a selection, and what undo
//! goes back to. Its `key` takes the usual keys: arrows (with Ctrl by
//! words, with Shift selecting), Home and End, Backspace and Delete (with
//! Ctrl by words), Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+Z and Ctrl+Shift+Z or
//! Ctrl+Y, and Ctrl+W and Ctrl+U as in shells. Any key it has nothing to do
//! with (Home with the caret at the start) is left to the overlay, which
//! lets it act on its list. So an overlay passes its field the keys first,
//! and keeps those the field doesn't take.
//!
//! A paste reaches a field the way a typed character does. Ctrl+V (see
//! `is_paste`) over an overlay reads the clipboard off the event loop, and
//! its text comes back as the key `PASTE` through the overlay's own key
//! handling (see `Gerdr::paste_text`), to the field that has focus. A copy
//! goes the other way: the copied text waits in `take_copied` for the key
//! handling to give it to the clipboard. So any field edited with `key`
//! pastes and copies with nothing more, and reacts to a paste as to typing:
//! a list refilters, a folder field suggests folders.

use std::{
    cell::{Cell, RefCell},
    ops::{Deref, Range},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

/// The key a paste comes as: one keyboards don't send.
pub const PASTE: Keysym = Keysym::VoidSymbol;

/// The most edits undo goes back.
const UNDO_LIMIT: usize = 100;

thread_local! {
    /// The text `PASTE` types, while `pasting` hands it over.
    static PASTED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The text a field copied, for the clipboard.
    static COPIED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Whether a field took a key since `took_key` was last asked.
    static TOOK: Cell<bool> = const { Cell::new(false) };
}

/// Whether the key pastes: Ctrl+V (with Shift too, as in terminals) or
/// Shift+Insert.
pub fn is_paste(mods: &ModifiersState, sym: Keysym) -> bool {
    match sym {
        Keysym::v | Keysym::V => mods.ctrl,
        Keysym::Insert | Keysym::KP_Insert => mods.shift,
        _ => false,
    }
}

/// Run `handle`, an overlay's key handling, on the key `PASTE`, which types
/// `text` into the first field it reaches: as one line, its line breaks and
/// tabs as spaces, as the fields are one line.
pub fn pasting<R>(text: &str, handle: impl FnOnce(Keysym) -> R) -> R {
    let line: String = text
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    PASTED.set(Some(line));
    let out = handle(PASTE);
    PASTED.take();
    out
}

/// The text a field copied or cut since this was last asked, for the
/// clipboard.
pub fn take_copied() -> Option<String> {
    COPIED.take()
}

/// Whether a field took a key since this was last asked.
pub fn took_key() -> bool {
    TOOK.replace(false)
}

/// What a key did to a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// It changed the text.
    Changed,
    /// It moved the caret or the selection, or copied, leaving the text.
    Moved,
    /// Nothing: the key is the overlay's.
    Ignored,
}

impl Edit {
    pub fn changed(self) -> bool {
        self == Edit::Changed
    }
}

/// What a text field shows: its text, where the caret is in it, and where
/// the selection reaches from the caret to (the caret itself for none), as
/// byte offsets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
}

impl Line {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The selected bytes; empty for none.
    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }
}

impl Deref for Line {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<&str> for Line {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl From<&str> for Line {
    /// `text` with the caret at its end.
    fn from(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            caret: text.len(),
            anchor: text.len(),
        }
    }
}

/// A line of text being edited.
#[derive(Clone, Debug, Default)]
pub struct TextField {
    line: Line,
    /// What undo goes back to, the latest last.
    undo: Vec<Line>,
    /// What redo goes forward to, the latest undone last.
    redo: Vec<Line>,
    /// The last edit typed characters: the next one undoes with it.
    typing: bool,
}

impl Deref for TextField {
    type Target = str;

    fn deref(&self) -> &str {
        &self.line.text
    }
}

impl PartialEq<&str> for TextField {
    fn eq(&self, other: &&str) -> bool {
        self.line.text == *other
    }
}

impl From<&str> for TextField {
    /// `text` with the caret at its end.
    fn from(text: &str) -> Self {
        Self {
            line: text.into(),
            ..Default::default()
        }
    }
}

impl From<String> for TextField {
    fn from(text: String) -> Self {
        text.as_str().into()
    }
}

impl TextField {
    /// What it shows.
    pub fn line(&self) -> Line {
        self.line.clone()
    }

    /// Replace its text with `text`, the caret at the end; undo brings the
    /// old back.
    pub fn set(&mut self, text: &str) {
        if self.line.text != text {
            self.remember();
            self.line = text.into();
        }
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    /// Select all of it, as typing replaces it.
    pub fn select_all(&mut self) {
        self.line.anchor = 0;
        self.line.caret = self.line.text.len();
        self.typing = false;
    }

    /// A click on byte `at` of it, the `count`th in a row there: the first
    /// puts the caret there (the selection reaching it when `extend`), the
    /// second selects the word there, the third all of it.
    pub fn click(&mut self, at: usize, extend: bool, count: u32) {
        let at = at.min(self.line.text.len());
        match count {
            0 | 1 => {
                if !extend {
                    self.line.anchor = at;
                }
                self.line.caret = at;
            }
            2 => {
                let word = word_around(&self.line.text, at);
                self.line.anchor = word.start;
                self.line.caret = word.end;
            }
            _ => self.select_all(),
        }
        self.typing = false;
    }

    /// The pointer dragged from a click to byte `at`: the selection reaches
    /// it.
    pub fn drag_to(&mut self, at: usize) {
        self.line.caret = at.min(self.line.text.len());
        self.typing = false;
    }

    /// Edit it with a key: `sym` is the key's own symbol, `typed` the one
    /// with Shift and the layout applied.
    pub fn key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Edit {
        let edit = self.edit(mods, sym, typed);
        if edit != Edit::Ignored {
            TOOK.set(true);
        }
        edit
    }

    fn edit(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Edit {
        if sym == PASTE {
            return match PASTED.take() {
                Some(text) => self.insert(&text, false),
                None => Edit::Ignored,
            };
        }
        if mods.alt || mods.logo {
            return Edit::Ignored;
        }
        let (ctrl, shift) = (mods.ctrl, mods.shift);
        let Line { text, caret, .. } = &self.line;
        let (caret, end) = (*caret, text.len());
        let selection = self.line.selection();
        match sym {
            Keysym::Left | Keysym::KP_Left => {
                let to = if ctrl {
                    word_start(text, caret, class)
                } else if !shift && !selection.is_empty() {
                    selection.start
                } else {
                    prev_char(text, caret)
                };
                self.go(to, shift)
            }
            Keysym::Right | Keysym::KP_Right => {
                let to = if ctrl {
                    word_end(text, caret)
                } else if !shift && !selection.is_empty() {
                    selection.end
                } else {
                    next_char(text, caret)
                };
                self.go(to, shift)
            }
            Keysym::Home | Keysym::KP_Home => self.go(0, shift),
            Keysym::End | Keysym::KP_End => self.go(end, shift),
            Keysym::BackSpace if ctrl => self.delete(word_start(text, caret, class)..caret),
            Keysym::w | Keysym::W if ctrl => {
                self.delete(word_start(text, caret, |c| u8::from(!c.is_whitespace()))..caret)
            }
            Keysym::BackSpace => self.delete(prev_char(text, caret)..caret),
            Keysym::Delete | Keysym::KP_Delete if shift && !ctrl => self.cut(),
            Keysym::Delete | Keysym::KP_Delete if ctrl => self.delete(caret..word_end(text, caret)),
            Keysym::Delete | Keysym::KP_Delete => self.delete(caret..next_char(text, caret)),
            Keysym::u | Keysym::U if ctrl => self.delete(0..caret),
            Keysym::a | Keysym::A if ctrl => {
                if selection == (0..end) {
                    return Edit::Ignored;
                }
                self.select_all();
                Edit::Moved
            }
            Keysym::c | Keysym::C | Keysym::Insert | Keysym::KP_Insert if ctrl => {
                if selection.is_empty() {
                    return Edit::Ignored;
                }
                COPIED.set(Some(text[selection].to_owned()));
                Edit::Moved
            }
            Keysym::x | Keysym::X if ctrl => self.cut(),
            Keysym::z | Keysym::Z if ctrl && shift => self.redo(),
            Keysym::z | Keysym::Z if ctrl => self.undo(),
            Keysym::y | Keysym::Y if ctrl => self.redo(),
            _ => match typed.key_char().filter(|c| !c.is_control() && !ctrl) {
                Some(c) => self.insert(c.encode_utf8(&mut [0; 4]), true),
                None => Edit::Ignored,
            },
        }
    }

    /// Keep what it is now for undo.
    fn remember(&mut self) {
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.line.clone());
        self.redo.clear();
        self.typing = false;
    }

    /// Move the caret to `to`, the selection reaching back to where it
    /// was (and further) when `extend`, else none.
    fn go(&mut self, to: usize, extend: bool) -> Edit {
        let anchor = if extend { self.line.anchor } else { to };
        if (to, anchor) == (self.line.caret, self.line.anchor) {
            return Edit::Ignored;
        }
        self.line.caret = to;
        self.line.anchor = anchor;
        self.typing = false;
        Edit::Moved
    }

    /// Delete the selection, or `range` without one.
    fn delete(&mut self, range: Range<usize>) -> Edit {
        let selection = self.line.selection();
        let range = if selection.is_empty() {
            range
        } else {
            selection
        };
        if range.is_empty() {
            return Edit::Ignored;
        }
        self.remember();
        self.line.text.replace_range(range.clone(), "");
        self.line.caret = range.start;
        self.line.anchor = range.start;
        Edit::Changed
    }

    /// Put `text` in place of the selection; `typed` characters undo
    /// together, up to a space.
    fn insert(&mut self, text: &str, typed: bool) -> Edit {
        let selection = self.line.selection();
        if text.is_empty() && selection.is_empty() {
            return Edit::Ignored;
        }
        if !(typed && self.typing && selection.is_empty() && text != " ") {
            self.remember();
        }
        self.line.text.replace_range(selection.clone(), text);
        self.line.caret = selection.start + text.len();
        self.line.anchor = self.line.caret;
        self.typing = typed;
        Edit::Changed
    }

    /// Copy the selection and delete it.
    fn cut(&mut self) -> Edit {
        let selection = self.line.selection();
        if selection.is_empty() {
            return Edit::Ignored;
        }
        COPIED.set(Some(self.line.text[selection.clone()].to_owned()));
        self.delete(selection)
    }

    fn undo(&mut self) -> Edit {
        let Some(line) = self.undo.pop() else {
            return Edit::Ignored;
        };
        self.redo.push(std::mem::replace(&mut self.line, line));
        self.typing = false;
        Edit::Changed
    }

    fn redo(&mut self) -> Edit {
        let Some(line) = self.redo.pop() else {
            return Edit::Ignored;
        };
        self.undo.push(std::mem::replace(&mut self.line, line));
        self.typing = false;
        Edit::Changed
    }
}

/// Where the character before `at` starts.
fn prev_char(text: &str, at: usize) -> usize {
    text[..at]
        .chars()
        .next_back()
        .map_or(at, |c| at - c.len_utf8())
}

/// Where the character after `at` ends.
fn next_char(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

/// A character's kind, for what a word is: space (0), letters and digits
/// (1), or other marks (2).
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Where the word before `at` starts: past the spaces before it, back over
/// the characters of the same kind (by `class`, 0 for space).
fn word_start(text: &str, at: usize, class: impl Fn(char) -> u8) -> usize {
    let (mut word, mut start) = (None, at);
    for (i, c) in text[..at].char_indices().rev() {
        match (word, class(c)) {
            (None, 0) => {}
            (None, kind) => word = Some(kind),
            (Some(kind), other) if kind != other => break,
            _ => {}
        }
        start = i;
    }
    start
}

/// Where the word after `at` ends: past the spaces after it, on over the
/// characters of the same kind.
fn word_end(text: &str, at: usize) -> usize {
    let (mut word, mut end) = (None, at);
    for (i, c) in text[at..].char_indices() {
        match (word, class(c)) {
            (None, 0) => {}
            (None, kind) => word = Some(kind),
            (Some(kind), other) if kind != other => break,
            _ => {}
        }
        end = at + i + c.len_utf8();
    }
    end
}

/// The word (or run of spaces, or of marks) at byte `at`: the characters
/// of the same kind as the one after it, else the one before.
fn word_around(text: &str, at: usize) -> Range<usize> {
    let Some(kind) = text[at..]
        .chars()
        .next()
        .or_else(|| text[..at].chars().next_back())
        .map(class)
    else {
        return at..at;
    };
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| class(*c) != kind)
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> ModifiersState {
        ModifiersState::default()
    }

    fn with(ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            shift,
            ..Default::default()
        }
    }

    fn press(field: &mut TextField, mods: &ModifiersState, sym: Keysym) -> Edit {
        field.key(mods, sym, sym)
    }

    fn type_in(field: &mut TextField, text: &str) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            assert_eq!(field.key(&none(), sym, sym), Edit::Changed);
        }
    }

    /// The text with the caret as `|` and the selection in `[` `]`.
    fn shown(field: &TextField) -> String {
        let Line {
            text,
            caret,
            anchor,
        } = field.line();
        let mut marks = vec![(caret, '|')];
        if caret != anchor {
            let sel = field.line.selection();
            marks = vec![(sel.start, '['), (sel.end, ']')];
        }
        let mut out = text;
        for (at, mark) in marks.into_iter().rev() {
            out.insert(at, mark);
        }
        out
    }

    #[test]
    fn types_deletes_and_leaves_other_keys_alone() {
        let mut f = TextField::default();
        type_in(&mut f, "foo bar");
        assert_eq!(press(&mut f, &none(), Keysym::BackSpace), Edit::Changed);
        assert_eq!(shown(&f), "foo ba|");
        assert_eq!(press(&mut f, &with(true, false), Keysym::w), Edit::Changed);
        assert_eq!(shown(&f), "foo |");
        assert_eq!(press(&mut f, &none(), Keysym::Up), Edit::Ignored);
        assert_eq!(press(&mut f, &with(true, false), Keysym::n), Edit::Ignored);
        // Nothing after the caret to delete: Delete is the overlay's.
        assert_eq!(press(&mut f, &none(), Keysym::Delete), Edit::Ignored);
        assert_eq!(press(&mut f, &with(true, false), Keysym::u), Edit::Changed);
        assert_eq!(shown(&f), "|");
        assert_eq!(press(&mut f, &none(), Keysym::BackSpace), Edit::Ignored);
    }

    #[test]
    fn the_caret_moves_by_characters_and_words() {
        let mut f = TextField::from("cd ~/Sites/blog");
        let ctrl = with(true, false);
        press(&mut f, &none(), Keysym::Left);
        assert_eq!(shown(&f), "cd ~/Sites/blo|g");
        press(&mut f, &ctrl, Keysym::Left);
        assert_eq!(shown(&f), "cd ~/Sites/|blog");
        press(&mut f, &ctrl, Keysym::Left);
        assert_eq!(shown(&f), "cd ~/Sites|/blog");
        press(&mut f, &ctrl, Keysym::Left);
        assert_eq!(shown(&f), "cd ~/|Sites/blog");
        press(&mut f, &ctrl, Keysym::Right);
        assert_eq!(shown(&f), "cd ~/Sites|/blog");
        press(&mut f, &none(), Keysym::Home);
        assert_eq!(shown(&f), "|cd ~/Sites/blog");
        // At the start already: Home is the overlay's.
        assert_eq!(press(&mut f, &none(), Keysym::Home), Edit::Ignored);
        assert_eq!(press(&mut f, &none(), Keysym::Left), Edit::Ignored);
        press(&mut f, &ctrl, Keysym::Delete);
        assert_eq!(shown(&f), "| ~/Sites/blog");
        press(&mut f, &none(), Keysym::Delete);
        press(&mut f, &none(), Keysym::End);
        press(&mut f, &ctrl, Keysym::BackSpace);
        assert_eq!(shown(&f), "~/Sites/|");
    }

    #[test]
    fn shift_selects_and_typing_replaces_the_selection() {
        let mut f = TextField::from("héllo world");
        let (shift, ctrl_shift) = (with(false, true), with(true, true));
        press(&mut f, &ctrl_shift, Keysym::Left);
        assert_eq!(shown(&f), "héllo [world]");
        press(&mut f, &shift, Keysym::Left);
        assert_eq!(shown(&f), "héllo[ world]");
        // Left lets go of the selection at its start.
        press(&mut f, &none(), Keysym::Left);
        assert_eq!(shown(&f), "héllo| world");
        press(&mut f, &shift, Keysym::Home);
        assert_eq!(shown(&f), "[héllo] world");
        type_in(&mut f, "x");
        assert_eq!(shown(&f), "x| world");
        press(&mut f, &with(true, false), Keysym::a);
        assert_eq!(shown(&f), "[x world]");
        assert_eq!(press(&mut f, &none(), Keysym::BackSpace), Edit::Changed);
        assert_eq!(shown(&f), "|");
    }

    #[test]
    fn copies_cuts_and_pastes_over_the_selection() {
        let mut f = TextField::from("docker compose up");
        let ctrl = with(true, false);
        take_copied();
        // Nothing selected: Ctrl+C is the overlay's (the switcher closes).
        assert_eq!(press(&mut f, &ctrl, Keysym::c), Edit::Ignored);
        assert_eq!(take_copied(), None);
        press(&mut f, &with(true, true), Keysym::Left);
        assert_eq!(press(&mut f, &ctrl, Keysym::c), Edit::Moved);
        assert_eq!(take_copied().as_deref(), Some("up"));
        assert_eq!(press(&mut f, &ctrl, Keysym::x), Edit::Changed);
        assert_eq!(take_copied().as_deref(), Some("up"));
        assert_eq!(shown(&f), "docker compose |");
        press(&mut f, &with(true, true), Keysym::Left);
        pasting("down\n", |sym| press(&mut f, &none(), sym));
        assert_eq!(shown(&f), "docker down|");
    }

    #[test]
    fn undo_and_redo_go_back_and_forth_a_word_at_a_time() {
        let mut f = TextField::default();
        let ctrl = with(true, false);
        type_in(&mut f, "make test");
        press(&mut f, &ctrl, Keysym::BackSpace);
        assert_eq!(shown(&f), "make |");
        press(&mut f, &ctrl, Keysym::z);
        assert_eq!(shown(&f), "make test|");
        press(&mut f, &ctrl, Keysym::z);
        assert_eq!(shown(&f), "make|");
        press(&mut f, &with(true, true), Keysym::z);
        assert_eq!(shown(&f), "make test|");
        press(&mut f, &ctrl, Keysym::z);
        press(&mut f, &ctrl, Keysym::z);
        assert_eq!(shown(&f), "|");
        assert_eq!(press(&mut f, &ctrl, Keysym::z), Edit::Ignored);
        press(&mut f, &ctrl, Keysym::y);
        assert_eq!(shown(&f), "make|");
        f.set("other");
        press(&mut f, &ctrl, Keysym::z);
        assert_eq!(shown(&f), "make|");
    }

    #[test]
    fn clicks_place_the_caret_and_select_words_or_all() {
        let mut f = TextField::from("docker compose up");
        f.click(3, false, 1);
        assert_eq!(shown(&f), "doc|ker compose up");
        f.drag_to(10);
        assert_eq!(shown(&f), "doc[ker com]pose up");
        f.click(16, true, 1);
        assert_eq!(shown(&f), "doc[ker compose u]p");
        f.click(9, false, 2);
        assert_eq!(shown(&f), "docker [compose] up");
        // At the end, the word before.
        f.click(17, false, 2);
        assert_eq!(shown(&f), "docker compose [up]");
        f.click(9, false, 3);
        assert_eq!(shown(&f), "[docker compose up]");
        f.click(99, false, 1);
        assert_eq!(shown(&f), "docker compose up|");
    }

    #[test]
    fn a_paste_types_one_line_into_the_first_field_it_reaches() {
        let (mut first, mut second) = (TextField::from("echo "), TextField::default());
        let typed = pasting(" a\tb\nc\n", |sym| {
            let first = first.key(&none(), sym, sym);
            let second = second.key(&none(), sym, sym);
            (first, second)
        });
        assert_eq!(typed, (Edit::Changed, Edit::Ignored));
        assert_eq!((&*first, &*second), ("echo a b c", ""));
        // Nothing is left over for a key after it.
        let mut later = TextField::default();
        assert_eq!(later.key(&none(), PASTE, PASTE), Edit::Ignored);
    }

    #[test]
    fn a_field_tells_it_took_a_key() {
        let mut f = TextField::from("a");
        took_key();
        press(&mut f, &none(), Keysym::Up);
        assert!(!took_key());
        press(&mut f, &none(), Keysym::Left);
        assert!(took_key());
        assert!(!took_key());
    }

    #[test]
    fn pastes_with_ctrl_v_or_shift_insert() {
        let (ctrl, shift) = (with(true, false), with(false, true));
        assert!(is_paste(&ctrl, Keysym::v));
        assert!(is_paste(&ctrl, Keysym::V));
        assert!(is_paste(&shift, Keysym::Insert));
        assert!(!is_paste(&none(), Keysym::v));
        assert!(!is_paste(&ctrl, Keysym::Insert));
    }
}
