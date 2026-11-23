//! An agent's pane painted as a chat (see `crate::chat`), in the pane's
//! place: a head line, the items (folded tool cards, edits with their
//! change, the agent's markdown), the waiting panel with the agent's
//! screen rows and keys while it waits on the user, and the reply box.

// Wave 2 takes this out once every part is used.
#![allow(dead_code)]

use std::{collections::HashSet, sync::Arc};

use smithay::utils::{Logical, Point};

use super::{FieldSpot, Rect};
use crate::{
    chat::{ChatFocus, Item, Reading},
    herdr::AgentStatus,
    text_field::Line,
};

/// What a pane's chat is painted from, each frame. Cheap to compare: the
/// items and screen rows by their `Arc`.
#[derive(Clone, Debug)]
pub struct ChatView {
    /// The pane's client rect, in output coordinates.
    pub rect: Rect,
    /// What the head line names: the agent, and the chat found.
    pub label: String,
    pub status: AgentStatus,
    /// Its agent is paused (Prefix Alt+A): nothing can be sent.
    pub paused: bool,
    pub reading: Reading,
    pub items: Arc<Vec<Item>>,
    pub unfolded: HashSet<String>,
    pub picked: Option<usize>,
    /// How far down the list is scrolled, in logical pixels.
    pub top: f64,
    pub focus: ChatFocus,
    /// This pane has the keys.
    pub focused: bool,
    pub draft: Line,
    pub waiting: Option<Arc<Vec<(String, crate::scrollback::Styled)>>>,
    pub hover: Option<ChatHit>,
}

impl PartialEq for ChatView {
    fn eq(&self, other: &Self) -> bool {
        let rows = |w: &Option<Arc<Vec<_>>>, v: &Option<Arc<Vec<_>>>| match (w, v) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        self.rect == other.rect
            && self.label == other.label
            && self.status == other.status
            && self.paused == other.paused
            && self.reading == other.reading
            && Arc::ptr_eq(&self.items, &other.items)
            && self.unfolded == other.unfolded
            && self.picked == other.picked
            && self.top == other.top
            && self.focus == other.focus
            && self.focused == other.focused
            && self.draft == other.draft
            && rows(&self.waiting, &other.waiting)
            && self.hover == other.hover
    }
}

/// A key the waiting panel sends the agent, by its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKey {
    Esc,
    Up,
    Down,
    Enter,
    /// A menu choice, 1 to 9.
    Digit(u8),
}

/// What is under the pointer in a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatHit {
    /// An item's card, by index: picks it.
    Item(usize),
    /// An item's fold mark or head row: folds or unfolds it.
    Fold(usize),
    /// An edit's path: opens the file.
    Path(usize),
    /// "Show all" under a tool output cut short.
    ShowAll(usize),
    /// A key button of the waiting panel.
    Key(WaitKey),
    /// The reply box.
    Composer,
    Send,
    /// "Show screen", where the chat couldn't be found or read.
    ShowScreen,
    /// The list's scrollbar.
    Scrollbar,
}

/// Where a chat's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatLayout {
    /// How high all items are, laid out for the list's width.
    pub content: i32,
    /// The list, under the head line.
    pub list: Rect,
    pub waiting: Option<Rect>,
    pub composer: Rect,
    pub send: Rect,
}

impl ChatLayout {
    /// What is at `pos`, if anything.
    pub fn hit(&self, view: &ChatView, pos: Point<f64, Logical>) -> Option<ChatHit> {
        let _ = (view, pos);
        None
    }
}

/// Where `view`'s parts go.
pub fn chat_layout(view: &ChatView) -> ChatLayout {
    let r = view.rect;
    ChatLayout {
        content: 0,
        list: r,
        waiting: None,
        composer: r,
        send: r,
    }
}

/// The reply box as a field, for the pointer and the caret (checklist O6).
pub fn chat_field_spot(view: &ChatView, scale: f64) -> Option<FieldSpot> {
    let _ = (view, scale);
    None
}
