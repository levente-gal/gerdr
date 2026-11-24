//! An agent's pane painted as a chat (see `crate::chat`), in the pane's
//! place: a head line, the items (folded tool cards, edits with their
//! change, the agent's markdown), the waiting panel while it waits on the
//! user (its menu as gerdr's own, else its screen rows, and the keys to
//! answer), and the reply box.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    utils::{Logical, Point, Rectangle},
};

use super::doc_layout::{self, DocPiece, width_in};
use super::features::{Font, SMALL_LINE};
use super::glyphs::Face;
use super::repos::{DIFF_BAR, DIFF_INSET, DIFF_TINT};
use super::scrollback::{STYLED_ROW, StyledRow};
use super::{
    BUTTON_WIDTH, ButtonStyle, CHEVRON_WIDTH, Canvas, DOT_SLOT, Dot, FIELD_GAP, FONT_SIZE,
    FieldSpot, HINTS_HEIGHT, ListBar, PANE_BAR_HEIGHT, Painter, Palette, QUIT_BUTTON, ROW_BUTTON,
    ROW_INSET, ROW_RADIUS, Rect, Rgb, SMALL_FONT_SIZE, Thumb, band, button_width, centred_dot,
    check, chevron_down, chevron_side, field_text, gutter_track, hint_at, hint_spots, icon_sdf,
    mix, phys, physical, rect_px, soft_rows, status_dot, wrap_field_height,
};
use crate::{
    ChatRows,
    chat::{
        ChatFocus, DiffSide, Item, Reading, ToolOut,
        live::{AskReply, PendingAsk, Running},
        menu::{self, Menu},
        rows::{self, Row, Summary},
    },
    herdr::AgentStatus,
    md_blocks::DocBlock,
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
    /// The agent's folder: paths in it show relative to it.
    pub cwd: Option<PathBuf>,
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
    /// The footer's key hints, from the chat's key tables (`chat::footer`).
    pub hints: Vec<(String, String)>,
    /// Its transcript holds messages before those read: whether they are
    /// being read (true) or can be asked for (false).
    pub earlier: Option<bool>,
    pub hover: Option<ChatHit>,
    /// The tools its agent's hooks said run that the transcript doesn't
    /// hold yet (`rows::unread`): rows at the list's end.
    pub running: Vec<Running>,
    /// What its agent asks to run, waiting on the chat's answer: a card in
    /// the waiting panel's place.
    pub ask: Option<PendingAsk>,
}

impl PartialEq for ChatView {
    fn eq(&self, other: &Self) -> bool {
        let rows = |w: &Option<Arc<Vec<_>>>, v: &Option<Arc<Vec<_>>>| match (w, v) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        self.rect == other.rect
            && self.label == other.label
            && self.cwd == other.cwd
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
            && self.hints == other.hints
            && self.earlier == other.earlier
            && self.hover == other.hover
            && self.running == other.running
            && self.ask == other.ask
    }
}

/// A key the waiting panel sends the agent, by its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKey {
    Esc,
    Up,
    Down,
    /// To the questions' tab before or after.
    Left,
    Right,
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
    /// "Earlier messages", above the first one read.
    Earlier,
    /// A key button of the waiting panel.
    Key(WaitKey),
    /// The reply box.
    Composer,
    Send,
    /// "Show screen", where the chat couldn't be found or read, and on the
    /// ask's card.
    ShowScreen,
    /// Allow or Deny on the ask's card.
    Answer(AskReply),
    /// The ask's head row: shows or folds what it would run.
    AskFold,
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
        let on = |rect: Rect| rect.to_f64().contains(pos);
        if on(self.send) {
            return Some(ChatHit::Send);
        }
        if on(self.composer) {
            return Some(ChatHit::Composer);
        }
        let parts = parts(view);
        if let Some((key, _)) = parts.keys.iter().find(|(_, rect)| on(*rect)) {
            return Some(ChatHit::Key(*key));
        }
        if let Some((hit, _)) = parts.answers.iter().find(|(_, rect)| on(*rect)) {
            return Some(*hit);
        }
        if let Some((at, laid)) = &parts.card
            && (laid.spots.iter()).any(|(r, _)| on(Rectangle::new(r.loc + *at, r.size)))
        {
            return Some(ChatHit::AskFold);
        }
        if let Some((at, laid)) = &parts.menu {
            let pos = pos - at.loc.to_f64();
            return (laid.spots.iter())
                .find(|(rect, _)| rect.to_f64().contains(pos))
                .and_then(|(_, spot)| match spot {
                    Spot::Key(key) => Some(ChatHit::Key(*key)),
                    _ => None,
                });
        }
        if !on(self.list) {
            return None;
        }
        if let Some(message) = message_layout(view, self.list) {
            return message
                .screen
                .filter(|b| on(*b))
                .map(|_| ChatHit::ShowScreen);
        }
        let (items, content) = stacked(view, parts.column.1);
        let top = top_of(view, content, self.list.size.h);
        if thumb_of(&parts, content, top).is_some_and(|thumb| thumb.takes(pos)) {
            return Some(ChatHit::Scrollbar);
        }
        let y = (pos.y - self.list.loc.y as f64).floor() as i32 + top;
        let x = (pos.x - parts.column.0 as f64).floor() as i32;
        if view.earlier.is_some() && (PART_GAP..PART_GAP + CARD_ROW).contains(&y) {
            return (0..parts.column.1).contains(&x).then_some(ChatHit::Earlier);
        }
        let i = items.partition_point(|(t, laid)| t + laid.h <= y);
        // The running rows at the end aren't the transcript's: nothing to
        // pick or fold there.
        if i >= items.len() - view.running.len() {
            return None;
        }
        let (t, laid) = items.get(i).filter(|(t, _)| *t <= y)?;
        let at = Point::from((x, y - t));
        let spot = laid.spots.iter().find(|(rect, _)| rect.contains(at));
        Some(match spot.map(|(_, spot)| *spot) {
            Some(Spot::Fold) => ChatHit::Fold(i),
            Some(Spot::Path) => ChatHit::Path(i),
            Some(Spot::ShowAll) => ChatHit::ShowAll(i),
            Some(Spot::Key(key)) => ChatHit::Key(key),
            None => ChatHit::Item(i),
        })
    }
}

/// Where `view`'s parts go.
pub fn chat_layout(view: &ChatView) -> ChatLayout {
    let parts = parts(view);
    let content = match message(view) {
        Some(_) => 0,
        None => stacked(view, parts.column.1).1,
    };
    ChatLayout {
        content,
        list: parts.list,
        waiting: parts.waiting,
        composer: parts.composer,
        send: parts.send,
    }
}

/// The reply box as a field, for the pointer and the caret (checklist O6).
pub fn chat_field_spot(view: &ChatView, scale: f64) -> Option<FieldSpot> {
    let area = parts(view).composer;
    (area.size.w > 0 && area.size.h > 0).then(|| FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: view.draft.clone(),
    })
}

/// The list's scrollbar, while not all of it shows.
#[cfg(test)]
fn chat_thumb(view: &ChatView) -> Option<Thumb> {
    if message(view).is_some() {
        return None;
    }
    let parts = parts(view);
    let (_, content) = stacked(view, parts.column.1);
    thumb_of(&parts, content, top_of(view, content, parts.list.size.h))
}

/// Where item `i` of `view` is down its list's content: its top and height,
/// to scroll it into view.
pub fn chat_item_span(view: &ChatView, i: usize) -> Option<(i32, i32)> {
    let (items, _) = stacked(view, parts(view).column.1);
    items.get(i).map(|(y, laid)| (*y, laid.h))
}

/// The footer's key hints (K3), as the chat's key tables say them.
pub fn chat_hints(view: &ChatView) -> Vec<(&str, &str)> {
    super::hint_strs(&view.hints)
}

/// The hint of `chat_hints` under `pos`, to press as its key.
pub fn chat_hint_at(view: &ChatView, pos: Point<f64, Logical>) -> Option<usize> {
    let hints = chat_hints(view);
    hint_at(&hints, &hint_spots(&hints, parts(view).hints), pos)
}

/// Where the dots of what works in `view` blink (output coordinates): its
/// head's while its agent works, and its running tools' in view.
pub fn chat_blinkers(view: &ChatView) -> Vec<Rect> {
    let (dot, _) = standing(view);
    if !dot.blinks() {
        return Vec::new();
    }
    let parts = parts(view);
    let mut dots = vec![centred_dot(head_slot(parts.head))];
    if message(view).is_some() {
        return dots;
    }
    let list = parts.list;
    let (items, content) = stacked(view, parts.column.1);
    let top = top_of(view, content, list.size.h);
    for (y, laid) in &items[in_view(&items, top, list.size.h)] {
        let at = Point::from((parts.column.0, list.loc.y + y - top));
        let running = laid.bits.iter().filter(|(_, bit)| *bit == Bit::Running);
        dots.extend(
            running
                .map(|(rect, _)| centred_dot(Rectangle::new(rect.loc + at, rect.size)))
                .filter(|dot| list.contains_rect(*dot)),
        );
    }
    dots
}

/// Room in from the chat's sides, and between its parts.
const PAD: i32 = 12;
const PART_GAP: i32 = 8;
/// The head line.
const HEAD: i32 = PANE_BAR_HEIGHT;
/// The widest the items' column gets: longer lines read badly.
const CHAT_MAX_W: i32 = 800;
/// Between two rows of one speaker, and where the speaker changes.
const STEP_GAP: i32 = 6;
const TURN_GAP: i32 = 18;
/// How much of the column the user's bubble takes at most, in percent.
const BUBBLE_SHARE: i32 = 75;
/// How many lines of an edit's change show while it is folded.
const EDIT_PEEK: usize = 8;
/// How many lines of the prompt a subagent was given show while folded.
const PROMPT_PEEK: usize = 3;
/// A card's head row: its fold mark, name, gist and state.
const CARD_ROW: i32 = ROW_BUTTON;
const CARD_GAP: i32 = 8;
/// Room around the user's words on their ground.
const BUBBLE_PAD: i32 = 8;
/// How far a subagent's items are in from its card.
const NEST: i32 = 16;
/// How many lines of a tool's input or output, or of an edit's change,
/// show before "Show all".
pub const CHAT_CUT: usize = 200;
/// The most screen rows the waiting panel shows.
const WAIT_ROWS: usize = 14;
/// Room either side of a key button's label.
const KEY_PAD: i32 = 12;
/// The waiting panel's keys under the agent's screen rows.
const WAIT_KEYS: [WaitKey; 7] = [
    WaitKey::Esc,
    WaitKey::Digit(1),
    WaitKey::Digit(2),
    WaitKey::Digit(3),
    WaitKey::Up,
    WaitKey::Down,
    WaitKey::Enter,
];
/// The most lines of what an ask would run its card shows unfolded.
const ASK_LINES: usize = 12;
/// The ask card's buttons, as they say them (the footer hints take their
/// words: checklist K3).
pub const ASK_BUTTONS: [(ChatHit, &str); 3] = [
    (ChatHit::Answer(AskReply::Allow), "Allow"),
    (ChatHit::Answer(AskReply::Deny), "Deny"),
    (ChatHit::ShowScreen, "Show screen"),
];

/// What `unfolded` holds for ask `ask` while its card shows what it would
/// run.
pub fn ask_key(ask: &str) -> String {
    format!("{ask}\u{0}ask")
}

/// Its keys under a menu: its choices are buttons of their own. The tab
/// keys go when it asks one question.
const MENU_KEYS: [WaitKey; 6] = [
    WaitKey::Esc,
    WaitKey::Left,
    WaitKey::Right,
    WaitKey::Up,
    WaitKey::Down,
    WaitKey::Enter,
];
/// How wide a menu choice's number is.
const CHOICE_NUM: i32 = 28;
/// How big the check of an answered question's tab is.
const TAB_CHECK: i32 = 14;

/// What a key's button says.
fn key_label(key: WaitKey) -> String {
    match key {
        WaitKey::Esc => "Esc".into(),
        WaitKey::Up => "↑".into(),
        WaitKey::Down => "↓".into(),
        WaitKey::Left => "←".into(),
        WaitKey::Right => "→".into(),
        WaitKey::Enter => "Enter".into(),
        WaitKey::Digit(d) => d.to_string(),
    }
}

/// All of a chat's parts, in output coordinates.
struct Parts {
    head: Rect,
    list: Rect,
    /// Where the items go across the list: from x, so wide.
    column: (i32, i32),
    /// Where the list's scrollbar goes: at the pane's right edge.
    bar_x: i32,
    waiting: Option<Rect>,
    /// The agent's menu in the waiting panel, laid out from where it goes.
    menu: Option<(Rect, Rc<Laid>)>,
    /// The screen rows in the waiting panel when it shows no menu, and
    /// its key buttons.
    rows: Rect,
    keys: Vec<(WaitKey, Rect)>,
    /// The ask's card in the waiting panel's place: where its body is laid
    /// from, and its body; and its buttons.
    card: Option<(Point<i32, Logical>, Laid)>,
    answers: Vec<(ChatHit, Rect)>,
    composer: Rect,
    send: Rect,
    hints: Rect,
}

/// How wide a key button labelled `label` is.
fn key_width(label: &str) -> i32 {
    (width_in(label, Face::Ui, FONT_SIZE).ceil() as i32 + 2 * KEY_PAD).max(BUTTON_WIDTH)
}

/// The screen rows the waiting panel shows of `rows`: up to the last
/// one with something on it, the last `WAIT_ROWS` of those.
fn screen_rows<T>(rows: &[(String, T)]) -> &[(String, T)] {
    let end = (rows.iter())
        .rposition(|(text, _)| !text.trim().is_empty())
        .map_or(0, |i| i + 1);
    &rows[end.saturating_sub(WAIT_ROWS)..end]
}

/// From the bottom up: the hints, the reply box with Send at its right,
/// the waiting panel while the agent waits (as many screen rows as leave
/// the list three card rows), and the list from under the head line.
fn parts(view: &ChatView) -> Parts {
    let r = view.rect;
    let full = (r.size.w - 2 * PAD).max(0);
    let w = full.min(CHAT_MAX_W);
    let x = r.loc.x + PAD + (full - w) / 2;
    let bottom = r.loc.y + r.size.h;
    let head = Rectangle::new(r.loc, (r.size.w, HEAD.min(r.size.h)).into());
    let hints = Rectangle::new(
        (r.loc.x, bottom - HINTS_HEIGHT).into(),
        (r.size.w, HINTS_HEIGHT).into(),
    );
    let send_w = button_width("Send");
    let field_w = (w - send_w - FIELD_GAP).max(0);
    let field_h = wrap_field_height(&view.draft, field_w);
    let composer = Rectangle::new(
        (x, hints.loc.y - PART_GAP - field_h).into(),
        (field_w, field_h).into(),
    );
    let send = Rectangle::new(
        (
            x + field_w + FIELD_GAP,
            composer.loc.y + field_h - QUIT_BUTTON,
        )
            .into(),
        (send_w, QUIT_BUTTON).into(),
    );
    let list_top = head.loc.y + head.size.h;
    let mut end = composer.loc.y - PART_GAP;
    let (mut waiting, mut menu, mut rows, mut keys) = (None, None, Rect::default(), Vec::new());
    let (mut card, mut answers) = (None, Vec::new());
    let frame = 3 * PART_GAP + ROW_BUTTON;
    let inner = (w - 2 * PART_GAP).max(0);
    if let Some(ask) = &view.ask {
        // As many lines of what it would run as leave the list three card
        // rows.
        let mut laid = lay_ask(view, ask, inner, 0);
        if view.unfolded.contains(&ask_key(&ask.ask)) {
            let room = (end - list_top - 3 * CARD_ROW - frame - laid.h - PART_GAP).max(0);
            let lines = (room / SMALL_LINE) as usize;
            laid = lay_ask(view, ask, inner, lines.min(ASK_LINES));
        }
        let h = frame + laid.h;
        let panel = Rectangle::new((x, end - h).into(), (w, h).into());
        let at = Point::from((x + PART_GAP, panel.loc.y + PART_GAP));
        let (mut bx, by) = (at.x, at.y + laid.h + PART_GAP);
        for (hit, label) in ASK_BUTTONS {
            let bw = button_width(label);
            if bx + bw > at.x + inner {
                break;
            }
            answers.push((hit, rect(bx, by, bw, ROW_BUTTON)));
            bx += bw + PART_GAP / 2;
        }
        card = Some((at, laid));
        waiting = Some(panel);
        end = panel.loc.y - PART_GAP;
    } else if let Some(screen) = &view.waiting {
        let room = (end - list_top - 3 * CARD_ROW - frame - PART_GAP).max(0);
        let laid = menu_laid(screen, inner, room);
        let (body, shown_keys) = match &laid {
            Some((laid, tabs)) => {
                let keys = MENU_KEYS.iter().copied();
                let keys: Vec<WaitKey> = match tabs {
                    true => keys.collect(),
                    false => keys
                        .filter(|k| !matches!(k, WaitKey::Left | WaitKey::Right))
                        .collect(),
                };
                (laid.h, keys)
            }
            None => {
                let n = (screen_rows(screen).len() as i32).min(room / STYLED_ROW);
                (n * STYLED_ROW, WAIT_KEYS.to_vec())
            }
        };
        let h = frame + body;
        let panel = Rectangle::new((x, end - h).into(), (w, h).into());
        let at = Rectangle::new(
            (x + PART_GAP, panel.loc.y + PART_GAP).into(),
            (inner, body).into(),
        );
        match laid {
            Some((laid, _)) => menu = Some((at, laid)),
            None => rows = at,
        }
        let (mut kx, ky) = (x + PART_GAP, at.loc.y + body + PART_GAP);
        for key in shown_keys {
            let kw = key_width(&key_label(key));
            if kx + kw > x + PART_GAP + inner {
                break;
            }
            keys.push((
                key,
                Rectangle::new((kx, ky).into(), (kw, ROW_BUTTON).into()),
            ));
            kx += kw + PART_GAP / 2;
        }
        waiting = Some(panel);
        end = panel.loc.y - PART_GAP;
    }
    let list = Rectangle::new(
        (r.loc.x, list_top).into(),
        (r.size.w, (end - list_top).max(0)).into(),
    );
    Parts {
        head,
        list,
        column: (x, w),
        bar_x: r.loc.x + PAD + full + 2,
        waiting,
        menu,
        rows,
        keys,
        card,
        answers,
        composer,
        send,
        hints,
    }
}

/// The menu at the bottom of `screen` laid out `w` wide, if it has one
/// and it fits `room` high (without what its choices say more, if need
/// be), and whether it has tabs; else the panel shows the screen rows.
/// Laid out once while they stay.
fn menu_laid(screen: &Arc<ChatRows>, w: i32, room: i32) -> Option<(Rc<Laid>, bool)> {
    type Kept = (Arc<ChatRows>, i32, i32, Option<(Rc<Laid>, bool)>);
    thread_local! {
        static MENU: RefCell<Option<Kept>> = const { RefCell::new(None) };
    }
    let kept = MENU.with_borrow(|memo| {
        (memo.as_ref())
            .filter(|(rows, mw, mr, _)| Arc::ptr_eq(rows, screen) && *mw == w && *mr == room)
            .map(|(.., laid)| laid.clone())
    });
    if let Some(laid) = kept {
        return laid;
    }
    let laid = menu::menu(screen).and_then(|m| {
        let fits = |laid: &Laid| laid.h <= room;
        let laid = Some(lay_menu(&m, w, true))
            .filter(fits)
            .or_else(|| Some(lay_menu(&m, w, false)).filter(fits))?;
        Some((Rc::new(laid), !m.tabs.is_empty()))
    });
    MENU.set(Some((screen.clone(), w, room, laid.clone())));
    laid
}

/// A menu laid out from `(0, 0)` for `w`: its questions' tabs, what it
/// asks, and its choices, each a press on its key, with what they say
/// more when `more`.
fn lay_menu(m: &Menu, w: i32, more: bool) -> Laid {
    let mut l = Laid::default();
    let size = Font::Body.size();
    let mut y = 0;
    if !m.tabs.is_empty() {
        let mut x = 0;
        for tab in &m.tabs {
            let face = if tab.current { Face::Bold } else { Face::Ui };
            let mark_w = if tab.done {
                TAB_CHECK + PART_GAP / 2
            } else {
                0
            };
            let text_w = width_in(&tab.label, face, size).ceil() as i32;
            let tw = mark_w + text_w + 2 * BUBBLE_PAD;
            if x + tw > w {
                break;
            }
            if tab.current {
                l.put(rect(x, 0, tw, CARD_ROW), Bit::Picked);
            }
            if tab.done {
                let y = (CARD_ROW - TAB_CHECK) / 2;
                l.put(rect(x + BUBBLE_PAD, y, TAB_CHECK, TAB_CHECK), Bit::Check);
            }
            let mark = if tab.current { Mark::Text } else { Mark::Muted };
            let bit = Bit::Label {
                text: tab.label.clone(),
                face,
                size,
                mark,
                centred: false,
            };
            l.put(rect(x + BUBBLE_PAD + mark_w, 0, text_w, CARD_ROW), bit);
            x += tw + PART_GAP / 2;
        }
        y = l.h + PART_GAP;
    }
    for (text, bold) in &m.ask {
        let block = match bold {
            true => DocBlock::Heading {
                level: 2,
                text: text.as_str().into(),
            },
            false => DocBlock::Para(text.as_str().into()),
        };
        let (pieces, h) = doc_layout::stack_of(&[block], w);
        l.doc(pieces, (0, y));
        y += h + PART_GAP / 2;
    }
    y += PART_GAP / 2;
    let tw = (w - CHOICE_NUM).max(1);
    for choice in &m.choices {
        let top = y;
        let num = format!("{}.", choice.n);
        let num = Bit::Label {
            text: num,
            face: Face::Ui,
            size,
            mark: Mark::Muted,
            centred: false,
        };
        let face = if choice.picked { Face::Bold } else { Face::Ui };
        let label = Bit::Label {
            text: choice.label.clone(),
            face,
            size,
            mark: Mark::Text,
            centred: false,
        };
        let mut bits = vec![
            (rect(ROW_INSET, y, CHOICE_NUM, CARD_ROW), num),
            (
                rect(ROW_INSET + CHOICE_NUM, y, tw - ROW_INSET, CARD_ROW),
                label,
            ),
        ];
        y += CARD_ROW;
        if more && !choice.detail.is_empty() {
            let note = DocBlock::Note(choice.detail.clone());
            let (pieces, h) = doc_layout::stack_of(&[note], tw - 2 * ROW_INSET);
            let at = Point::from((ROW_INSET + CHOICE_NUM, y));
            bits.extend(
                (pieces.into_iter())
                    .map(|(r, piece)| (Rectangle::new(r.loc + at, r.size), Bit::Doc(piece))),
            );
            y += h + PART_GAP / 2;
        }
        let ground = rect(0, top, w, y - top);
        if choice.picked {
            l.put(ground, Bit::Picked);
        }
        for (r, bit) in bits {
            l.put(r, bit);
        }
        if (1..=9).contains(&choice.n) {
            l.spots.push((ground, Spot::Key(WaitKey::Digit(choice.n))));
        }
        y += STEP_GAP / 2;
    }
    l.h = l.h.max(y - STEP_GAP / 2);
    l
}

/// Where the head's status dot goes, at its left.
fn head_slot(head: Rect) -> Rect {
    Rectangle::new(
        (head.loc.x + PAD, head.loc.y).into(),
        (DOT_SLOT, head.size.h).into(),
    )
}

/// The head's dot and word for where the agent stands.
fn standing(view: &ChatView) -> (Dot, &'static str) {
    if view.paused {
        return (Dot::Unknown, "paused");
    }
    // herdr sees no menu on its screen while its agent asks the chat.
    if view.ask.is_some() {
        return (Dot::of_status(AgentStatus::Blocked), "waits for you");
    }
    let word = match view.status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "waits for you",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "",
    };
    (Dot::of_status(view.status), word)
}

/// The agent's name in the head's label ("claude · api" names claude).
fn agent_name(label: &str) -> &str {
    match label.split(" · ").next().map(str::trim) {
        Some(name) if !name.is_empty() => name,
        _ => "agent",
    }
}

/// What the list says in its items' place, and whether "Show screen" goes
/// under it; none while it shows items.
fn message(view: &ChatView) -> Option<(&str, bool)> {
    match &view.reading {
        Reading::Missing(why) | Reading::Failed(why) => Some((why, true)),
        _ if !view.running.is_empty() => None,
        Reading::Finding if view.items.is_empty() => Some(("Reading the chat…", false)),
        Reading::Read if view.items.is_empty() => Some(("Nothing said yet", false)),
        _ => None,
    }
}

/// The list's message laid out: its lines centred in the `list`, wrapped
/// to three at most, and "Show screen" under them.
struct Message {
    lines: Vec<(String, Rect)>,
    screen: Option<Rect>,
}

/// How many lines a message takes at most.
const MESSAGE_LINES: usize = 3;

fn message_layout(view: &ChatView, list: Rect) -> Option<Message> {
    let (text, screen) = message(view)?;
    let room = (list.size.w - 2 * PAD).max(1);
    let rows = soft_rows(text, room as f32);
    let n = rows.len().min(MESSAGE_LINES);
    let button = if screen {
        PART_GAP * 2 + QUIT_BUTTON
    } else {
        0
    };
    let mut y = list.loc.y + (list.size.h - n as i32 * SMALL_LINE - button) / 2;
    let mut lines = Vec::new();
    for (i, row) in rows.iter().take(n).enumerate() {
        // The last line shown takes the rest, cut short as it is painted.
        let end = if i + 1 == n { text.len() } else { row.end };
        let rect = Rectangle::new((list.loc.x + PAD, y).into(), (room, SMALL_LINE).into());
        lines.push((text[row.start..end].trim().to_owned(), rect));
        y += SMALL_LINE;
    }
    let screen = screen.then(|| {
        let w = button_width("Show screen");
        let x = list.loc.x + (list.size.w - w) / 2;
        Rectangle::new((x, y + PART_GAP * 2).into(), (w, QUIT_BUTTON).into())
    });
    Some(Message { lines, screen })
}

/// The items laid out for a column `w` wide: each one's top in the list
/// and its layout, and how high they all are with the room above and
/// below them.
fn stacked(view: &ChatView, w: i32) -> (Vec<(i32, Rc<Laid>)>, i32) {
    let rows = rows::rows(&view.items, &view.unfolded);
    let mut y = PART_GAP + earlier_h(view);
    let mut laid_rows = Vec::with_capacity(rows.len() + view.running.len());
    let mut was = None;
    let said = (rows.iter()).map(|row| {
        let yours = matches!(row, Row::Item(i) if matches!(view.items[*i], Item::User { .. }));
        (yours, laid_row(view, row, w))
    });
    let running = (view.running.iter()).map(|r| (false, running_row(view, r, w)));
    for (yours, laid) in said.chain(running) {
        if let Some(was) = was {
            y += if was == yours { STEP_GAP } else { TURN_GAP };
        }
        laid_rows.push((y, laid.clone()));
        y += laid.h;
        was = Some(yours);
    }
    let content = if laid_rows.is_empty() {
        0
    } else {
        y + PART_GAP
    };
    (laid_rows, content)
}

/// How high the "Earlier messages" row above the items is, with its gap:
/// none when the transcript was read from its start.
fn earlier_h(view: &ChatView) -> i32 {
    match view.earlier {
        Some(_) => CARD_ROW + TURN_GAP,
        None => 0,
    }
}

/// What the "Earlier messages" row says, and its colour.
fn earlier_words(reading: bool) -> (&'static str, Mark) {
    match reading {
        true => ("Reading earlier messages…", Mark::Muted),
        false => ("Earlier messages", Mark::Link),
    }
}

/// Row `row` of `view` laid out for a column `w` wide: a subagent's own
/// rows in from its card.
fn laid_row(view: &ChatView, row: &Row, w: i32) -> Rc<Laid> {
    let (cwd, unfolded) = (view.cwd.as_deref(), &view.unfolded);
    let nest = NEST * row.depth() as i32;
    let items = row.items(&view.items);
    let Some(first) = items.first() else {
        return Rc::default();
    };
    match row.item(&view.items) {
        Some(item) => {
            let step = matches!(innermost(row), Row::Step(_));
            laid(item, w, nest + if step { NEST } else { 0 }, unfolded, cwd)
        }
        None => {
            let open = matches!(innermost(row), Row::Steps { open: true, .. });
            let mut s = rows::summary(items);
            s.running = (s.running.take()).map(|gist| short(&gist, cwd));
            laid_steps(first.id(), s, open, w, nest)
        }
    }
}

/// A tool its agent's hooks said runs, laid out as the transcript's
/// running tools are.
fn running_row(view: &ChatView, r: &Running, w: i32) -> Rc<Laid> {
    let item = Item::Tool {
        id: r.call.clone(),
        name: r.name.clone(),
        gist: r.gist.clone(),
        input: String::new(),
        output: None,
    };
    laid(&item, w, 0, &view.unfolded, view.cwd.as_deref())
}

/// The ask's card laid out from `(0, 0)` for `w` wide: the tool and its
/// gist on a row that folds, under it unfolded the first `lines` of what it
/// would run, then what the agent waits for.
fn lay_ask(view: &ChatView, ask: &PendingAsk, w: i32, lines: usize) -> Laid {
    let mut l = Laid::default();
    let open = view.unfolded.contains(&ask_key(&ask.ask));
    l.head(Head {
        x: 0,
        w,
        open,
        name: &ask.tool,
        bold: true,
        gist: &short(&ask.gist, view.cwd.as_deref()),
        tail: Vec::new(),
        running: false,
        path: false,
        mono: true,
    });
    if open {
        let (ix, iw) = (CHEVRON_WIDTH, (w - CHEVRON_WIDTH).max(1));
        let all: Vec<&str> = ask.input.lines().collect();
        let shown = all.len().min(lines);
        let mut y = l.below();
        for line in &all[..shown] {
            let bit = Bit::Label {
                text: line.replace('\t', "    "),
                face: Face::Mono,
                size: SMALL_FONT_SIZE,
                mark: Mark::Text,
                centred: false,
            };
            l.put(rect(ix, y, iw, SMALL_LINE), bit);
            y += SMALL_LINE;
        }
        if all.len() > shown {
            let text = format!("{} more lines", all.len() - shown);
            l.put(
                rect(ix, l.below(), iw, SMALL_LINE),
                label(&text, Face::Ui, Mark::Muted),
            );
        }
    }
    let words = format!(
        "{} waits for your answer · Show screen to answer in its own prompt",
        agent_name(&view.label)
    );
    l.put(
        rect(0, l.below(), w, SMALL_LINE),
        label(&words, Face::Ui, Mark::Muted),
    );
    l
}

/// The row `row` is down its subagent cards.
fn innermost(row: &Row) -> &Row {
    match row {
        Row::In { row, .. } => innermost(row),
        row => row,
    }
}

/// How far down a list `h` high of `content` is scrolled: `view.top`, no
/// further than its end. A list stuck to its bottom can ask for any more.
fn top_of(view: &ChatView, content: i32, h: i32) -> i32 {
    view.top.max(0.0).min((content - h).max(0) as f64).round() as i32
}

/// Which of `items` (as `stacked` gives them) show in a list `h` high
/// scrolled `top` down: only those are painted.
fn in_view(items: &[(i32, Rc<Laid>)], top: i32, h: i32) -> Range<usize> {
    let from = items.partition_point(|(y, laid)| y + laid.h <= top);
    let to = items.partition_point(|(y, _)| *y < top + h);
    from..to.max(from)
}

/// The list's scrollbar in the room right of its items, while they don't
/// all show.
fn thumb_of(parts: &Parts, content: i32, top: i32) -> Option<Thumb> {
    let track = gutter_track(parts.bar_x, parts.list);
    let bar = ListBar {
        down: top as f64,
        total: content as f64,
    };
    Thumb::of(track, parts.list.size.h as f64, bar)
}

/// What a press on part of an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Fold,
    Path,
    ShowAll,
    /// A menu's choice: presses its key.
    Key(WaitKey),
}

/// A colour of a card's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Text,
    Muted,
    Added,
    Removed,
    Failed,
    Link,
}

impl Mark {
    fn color(self, p: &Palette) -> Rgb {
        match self {
            Mark::Text => p.text,
            Mark::Muted => p.muted,
            Mark::Added => p.success,
            Mark::Removed | Mark::Failed => p.error,
            Mark::Link => p.accent,
        }
    }
}

/// The ground under the user's words: a tint of the chat's, as the
/// element colour is the chat's own in the light theme.
fn bubble(p: &Palette) -> Rgb {
    mix(p.terminal_background, p.text, BUBBLE_TINT)
}

/// How much of the words' colour tints the user's bubble.
const BUBBLE_TINT: f32 = 0.08;

/// A thing an item paints, in its place.
#[derive(Clone, Debug, PartialEq)]
enum Bit {
    /// The ground under the user's words.
    Bubble,
    /// A piece of markdown or code, as a file's page paints it.
    Doc(DocPiece),
    /// A card's fold mark: open or not.
    Fold(bool),
    /// One line, cut short with "…" where its place ends (L5).
    Label {
        text: String,
        face: Face,
        size: f32,
        mark: Mark,
        centred: bool,
    },
    /// A line of an edit's change, tinted as the diff view tints it.
    Diff(DiffSide, String),
    /// Where a running tool's dot blinks: composed, not painted.
    Running,
    /// The ground of the choice a menu's arrow is on, or of its current
    /// tab.
    Picked,
    /// The check of a question answered.
    Check,
}

/// An item laid out from `(0, 0)` for its width: what it paints, where a
/// press on it does something of its own, and how high it is.
#[derive(Debug, Default, PartialEq)]
struct Laid {
    bits: Vec<(Rect, Bit)>,
    spots: Vec<(Rect, Spot)>,
    h: i32,
}

/// A card's head row (see `Laid::head`).
struct Head<'a> {
    x: i32,
    w: i32,
    open: bool,
    name: &'a str,
    bold: bool,
    gist: &'a str,
    tail: Vec<(String, Mark)>,
    running: bool,
    /// The gist is a file's path: a press on it opens the file.
    path: bool,
    /// The gist is a command or a path: in the small monospace face.
    mono: bool,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rectangle::new((x, y).into(), (w.max(0), h.max(0)).into())
}

fn label(text: &str, face: Face, mark: Mark) -> Bit {
    Bit::Label {
        text: text.to_owned(),
        face,
        size: FONT_SIZE,
        mark,
        centred: false,
    }
}

/// How a tool's result says how it went: ok or failed, and how long it
/// took.
fn outcome(output: &Option<ToolOut>) -> Vec<(String, Mark)> {
    let Some(out) = output else {
        return Vec::new();
    };
    let took = out.ms.and_then(took);
    match (out.failed, took) {
        (true, Some(took)) => vec![(format!("failed · {took}"), Mark::Failed)],
        (true, None) => vec![("failed".to_owned(), Mark::Failed)],
        (false, Some(took)) => vec![(took, Mark::Muted)],
        (false, None) => Vec::new(),
    }
}

/// How long `ms` took, said once it is a second or more: quicker is noise.
fn took(ms: u64) -> Option<String> {
    match ms {
        0..1000 => None,
        1000..60_000 => Some(format!("{:.1} s", ms as f64 / 1000.0)),
        _ => Some(format!("{} min {} s", ms / 60_000, ms % 60_000 / 1000)),
    }
}

/// `gist` shorter for a card's row, in the agent's folder `cwd`.
fn short(gist: &str, cwd: Option<&Path>) -> String {
    rows::short_gist(gist, cwd, crate::home_dir().as_deref())
}

/// `text`'s paragraphs, split at its blank lines.
fn paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.split("\n\n")
        .map(str::trim_end)
        .filter(|p| !p.trim().is_empty())
}

impl Laid {
    fn put(&mut self, rect: Rect, bit: Bit) {
        self.h = self.h.max(rect.loc.y + rect.size.h);
        self.bits.push((rect, bit));
    }

    /// `pieces` laid out from `(0, 0)` put with that at `at`.
    fn doc(&mut self, pieces: Vec<(Rect, DocPiece)>, at: (i32, i32)) {
        for (rect, piece) in pieces {
            self.put(
                Rectangle::new(rect.loc + Point::from(at), rect.size),
                Bit::Doc(piece),
            );
        }
    }

    /// Where the next part of an unfolded card goes.
    fn below(&self) -> i32 {
        self.h + PART_GAP / 2
    }

    /// A card's head row at the top: the fold mark, its name, the gist
    /// cut short before the tail (the state, at the row's right end), a
    /// press on it folding or unfolding the card.
    fn head(&mut self, head: Head) {
        let Head { x, w, open, .. } = head;
        self.put(rect(x, 0, CHEVRON_WIDTH, CARD_ROW), Bit::Fold(open));
        let face = if head.bold { Face::Bold } else { Face::Ui };
        let mark = if head.bold { Mark::Text } else { Mark::Muted };
        let wide = |text: &str, face| width_in(text, face, FONT_SIZE).ceil() as i32;
        let widths: Vec<i32> = head.tail.iter().map(|(t, _)| wide(t, Face::Ui)).collect();
        let dot = if head.running { DOT_SLOT } else { 0 };
        let tail_w = widths.iter().map(|w| w + CARD_GAP).sum::<i32>() + dot;
        let end = x + w;
        let tail_x = (end - tail_w).max(x + CHEVRON_WIDTH);
        let name_x = x + CHEVRON_WIDTH;
        let name_w = wide(head.name, face).min(tail_x - name_x);
        self.put(
            rect(name_x, 0, name_w, CARD_ROW),
            label(head.name, face, mark),
        );
        let gist_x = name_x + name_w + CARD_GAP;
        let gist_w = tail_x - CARD_GAP - gist_x;
        if !head.gist.is_empty() && gist_w > 0 {
            let gist = head.gist.lines().next().unwrap_or_default();
            let mark = if head.path { Mark::Text } else { Mark::Muted };
            let (gface, gsize) = match head.mono {
                true => (Face::Mono, SMALL_FONT_SIZE),
                false => (Face::Ui, FONT_SIZE),
            };
            let bit = Bit::Label {
                text: gist.to_owned(),
                face: gface,
                size: gsize,
                mark,
                centred: false,
            };
            self.put(rect(gist_x, 0, gist_w, CARD_ROW), bit);
            if head.path {
                let shown = (width_in(gist, gface, gsize).ceil() as i32).min(gist_w);
                self.spots
                    .push((rect(gist_x, 0, shown, CARD_ROW), Spot::Path));
            }
        }
        let mut tx = tail_x;
        if head.running {
            self.put(rect(tx, 0, DOT_SLOT, CARD_ROW), Bit::Running);
            tx += DOT_SLOT;
        }
        for ((text, mark), w) in head.tail.into_iter().zip(widths) {
            let room = w.min(end - tx);
            self.put(
                rect(tx + CARD_GAP, 0, room, CARD_ROW),
                label(&text, Face::Ui, mark),
            );
            tx += w + CARD_GAP;
        }
        self.spots.push((rect(x, 0, w, CARD_ROW), Spot::Fold));
    }

    /// "Show all (N lines)" under what was cut to `CHAT_CUT` of `n` lines.
    fn more(&mut self, x: i32, w: i32, n: usize) {
        let row = rect(x, self.below(), w, SMALL_LINE);
        let text = format!("Show all ({n} lines)");
        let shown = width_in(&text, Face::Ui, FONT_SIZE).ceil() as i32;
        self.put(row, label(&text, Face::Ui, Mark::Link));
        self.spots
            .push((rect(x, row.loc.y, shown.min(w), SMALL_LINE), Spot::ShowAll));
    }

    /// `text` as a code block from `x`, `w` wide, below the rest: its
    /// first `CHAT_CUT` lines, and "Show all" when it has more.
    fn code(&mut self, text: &str, x: i32, w: i32, cut: usize) {
        let lines: Vec<String> = (text.lines().take(cut))
            .map(|line| line.replace('\t', "    "))
            .collect();
        let (pieces, _) = doc_layout::stack_of(&[DocBlock::Code(lines)], w);
        let top = self.below();
        self.doc(pieces, (x, top));
        let n = text.lines().count();
        if n > cut {
            self.more(x, w, n);
        }
    }
}

/// What `unfolded` holds for item `id` once its output shows whole ("Show
/// all"), beside its id for it being unfolded.
pub fn whole_key(id: &str) -> String {
    format!("{id}\u{0}whole")
}

/// `item` laid out for a column `w` wide, in `nest` from its left, as
/// `unfolded` has its cards; its paths shown from the agent's `cwd`.
fn lay(item: &Item, w: i32, nest: i32, unfolded: &HashSet<String>, cwd: Option<&Path>) -> Laid {
    #[cfg(test)]
    LAYS.with(|n| n.set(n.get() + 1));
    let mut l = Laid::default();
    let (x, cw) = (nest, (w - nest).max(1));
    let open = unfolded.contains(item.id());
    let cut = match unfolded.contains(&whole_key(item.id())) {
        true => usize::MAX,
        false => CHAT_CUT,
    };
    // A card's insides: in from its fold mark.
    let (ix, iw) = (x + CHEVRON_WIDTH, (cw - CHEVRON_WIDTH).max(1));
    match item {
        // The user's words in a bubble at the right, the agent's from the
        // left: who speaks shows without naming them.
        Item::User { text, .. } => {
            let inner = (cw * BUBBLE_SHARE / 100 - 2 * BUBBLE_PAD).max(1);
            let blocks: Vec<DocBlock> =
                paragraphs(text).map(|p| DocBlock::Para(p.into())).collect();
            let (mut pieces, mut h) = doc_layout::stack_of(&blocks, inner);
            // A subagent's prompt is long and said already by its card's
            // title: its first lines, unfolded on asking.
            let mut more = 0;
            if nest > 0 && !open {
                let mut tops: Vec<i32> = (pieces.iter())
                    .filter(|(_, piece)| matches!(piece, DocPiece::Text { .. }))
                    .map(|(r, _)| r.loc.y)
                    .collect();
                tops.dedup();
                if let Some(&cut) = tops.get(PROMPT_PEEK) {
                    more = tops.len() - PROMPT_PEEK;
                    pieces.retain(|(r, _)| r.loc.y < cut);
                    h = (pieces.iter())
                        .map(|(r, _)| r.loc.y + r.size.h)
                        .max()
                        .unwrap_or(0);
                }
            }
            let wide = (pieces.iter())
                .filter_map(|(_, piece)| match piece {
                    DocPiece::Text { text, .. } => {
                        Some(width_in(text, Face::Ui, Font::Body.size()))
                    }
                    _ => None,
                })
                .fold(0.0, f32::max)
                .ceil() as i32;
            let bw = (wide + 2 * BUBBLE_PAD).min(cw);
            let bx = x + cw - bw;
            l.put(rect(bx, 0, bw, h + 2 * BUBBLE_PAD), Bit::Bubble);
            l.doc(pieces, (bx + BUBBLE_PAD, BUBBLE_PAD));
            if more > 0 || (nest > 0 && open) {
                l.spots
                    .push((rect(bx, 0, bw, h + 2 * BUBBLE_PAD), Spot::Fold));
            }
            if more > 0 {
                let text = format!("{more} more lines");
                let wide = width_in(&text, Face::Ui, FONT_SIZE).ceil() as i32;
                let row = rect(x + cw - wide, l.below(), wide, SMALL_LINE);
                l.put(row, label(&text, Face::Ui, Mark::Link));
                l.spots.push((row, Spot::Fold));
            }
        }
        Item::Assistant { blocks, .. } => {
            let (pieces, _) = doc_layout::stack_of(blocks, cw);
            l.doc(pieces, (x, 0));
        }
        Item::Thinking { text, .. } => {
            // Codex and OpenCode open theirs with a bold title: no stars.
            let first = (text
                .lines()
                .map(|line| line.trim().trim_matches('*').trim()))
            .find(|line| !line.is_empty());
            l.head(Head {
                x,
                w: cw,
                open,
                name: "Thinking",
                bold: false,
                gist: if open { "" } else { first.unwrap_or_default() },
                tail: Vec::new(),
                running: false,
                path: false,
                mono: false,
            });
            if open {
                let blocks: Vec<DocBlock> =
                    paragraphs(text).map(|p| DocBlock::Note(p.into())).collect();
                let (pieces, _) = doc_layout::stack_of(&blocks, iw);
                let top = l.below();
                l.doc(pieces, (ix, top));
            }
        }
        Item::Tool {
            name,
            gist,
            input,
            output,
            ..
        } => {
            l.head(Head {
                x,
                w: cw,
                open,
                name,
                bold: true,
                gist: &short(gist, cwd),
                tail: outcome(output),
                running: output.is_none(),
                path: false,
                mono: true,
            });
            if open {
                // An input its row already says whole isn't said again.
                if !input.trim().is_empty() && input.trim() != gist.trim() {
                    l.code(input, ix, iw, cut);
                }
                if let Some(out) = output.as_ref().filter(|out| !out.text.trim().is_empty()) {
                    l.code(&out.text, ix, iw, cut);
                }
            }
        }
        Item::Edit {
            path, diff, output, ..
        } => {
            let count = |side| diff.iter().filter(|line| line.side == side).count();
            let (added, removed) = (count(DiffSide::Added), count(DiffSide::Removed));
            let mut tail = Vec::new();
            if added > 0 {
                tail.push((format!("+{added}"), Mark::Added));
            }
            if removed > 0 {
                tail.push((format!("−{removed}"), Mark::Removed));
            }
            tail.extend(outcome(output));
            l.head(Head {
                x,
                w: cw,
                open,
                name: "Edit",
                bold: true,
                gist: &short(path, cwd),
                tail,
                running: output.is_none() && diff.is_empty(),
                path: true,
                mono: true,
            });
            // Its first lines show folded, as the change is what it says.
            let shown = if open { cut } else { EDIT_PEEK };
            let mut y = l.below();
            for line in diff.iter().take(shown) {
                let text = line.text.replace('\t', "    ");
                l.put(rect(ix, y, iw, SMALL_LINE), Bit::Diff(line.side, text));
                y += SMALL_LINE;
            }
            if open && diff.len() > cut {
                l.more(ix, iw, diff.len());
            } else if !open && diff.len() > EDIT_PEEK {
                let text = format!("{} more lines", diff.len() - EDIT_PEEK);
                let row = rect(ix, l.below(), iw, SMALL_LINE);
                let wide = width_in(&text, Face::Ui, FONT_SIZE).ceil() as i32;
                l.put(row, label(&text, Face::Ui, Mark::Link));
                l.spots
                    .push((rect(ix, row.loc.y, wide.min(iw), SMALL_LINE), Spot::Fold));
            }
        }
        Item::Subagent {
            title,
            items,
            output,
            ..
        } => {
            let steps = match items.len() {
                1 => "1 step".to_owned(),
                n => format!("{n} steps"),
            };
            let mut tail = vec![(steps, Mark::Muted)];
            tail.extend(outcome(output));
            l.head(Head {
                x,
                w: cw,
                open,
                name: "Subagent",
                bold: true,
                gist: title,
                tail,
                running: output.is_none(),
                path: false,
                mono: false,
            });
            // Open, its own rows follow it in the list (`rows::rows`).
        }
        Item::Note { text, .. } => {
            let line = text.lines().next().unwrap_or_default();
            let bit = Bit::Label {
                text: line.to_owned(),
                face: Face::Ui,
                size: SMALL_FONT_SIZE,
                mark: Mark::Muted,
                centred: true,
            };
            l.put(rect(0, 0, w, SMALL_LINE), bit);
        }
    }
    l
}

/// An item's last layout: laying one out measures its every word, and the
/// list is laid out whenever its view changes (a scroll, a hover).
struct Memo {
    item: Item,
    w: i32,
    nest: i32,
    cwd: Option<PathBuf>,
    /// Which of it is unfolded (see `opened`).
    open: Vec<bool>,
    laid: Rc<Laid>,
}

/// How many items `MEMO` holds before it starts over.
const MEMOS: usize = 4096;

thread_local! {
    static MEMO: RefCell<HashMap<String, Memo>> = RefCell::new(HashMap::new());
}

#[cfg(test)]
thread_local! {
    /// How many items were laid out, memo or not: for the tests.
    static LAYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether `item` is unfolded, and its output shown whole: what its layout
/// depends on of `unfolded` (an open subagent's own rows are rows of the
/// list).
fn opened(item: &Item, unfolded: &HashSet<String>, out: &mut Vec<bool>) {
    let open = unfolded.contains(item.id());
    out.push(open);
    out.push(unfolded.contains(&whole_key(item.id())));
}

/// `item` laid out for a column `w` wide, as `unfolded` has it: once per
/// its id, width, folds and content, whatever the theme (layout doesn't
/// depend on it).
fn laid(
    item: &Item,
    w: i32,
    nest: i32,
    unfolded: &HashSet<String>,
    cwd: Option<&Path>,
) -> Rc<Laid> {
    let mut open = Vec::new();
    opened(item, unfolded, &mut open);
    let kept = MEMO.with_borrow(|memo| {
        let m = memo.get(item.id())?;
        let same = m.w == w && m.nest == nest && m.open == open && m.cwd.as_deref() == cwd;
        (same && m.item == *item).then(|| m.laid.clone())
    });
    if let Some(laid) = kept {
        return laid;
    }
    let laid = Rc::new(lay(item, w, nest, unfolded, cwd));
    MEMO.with_borrow_mut(|memo| {
        if memo.len() >= MEMOS {
            memo.clear();
        }
        let item = item.clone();
        let entry = Memo {
            item,
            w,
            nest,
            cwd: cwd.map(Path::to_path_buf),
            open,
            laid: laid.clone(),
        };
        memo.insert(entry.item.id().to_owned(), entry);
    });
    laid
}

/// A steps row's last layout: what it said, open or not, and how wide.
type StepsMemo = (Summary, bool, i32, Rc<Laid>);

thread_local! {
    /// Each steps row's last layout, by its first item's id.
    static STEPS: RefCell<HashMap<String, StepsMemo>> = RefCell::new(HashMap::new());
}

/// The steps row from item `id` on saying `s`, its fold mark `open`, for
/// a column `w` wide: laid out once while they stay.
fn laid_steps(id: &str, s: Summary, open: bool, w: i32, nest: i32) -> Rc<Laid> {
    let kept = STEPS.with_borrow(|memo| {
        let (was, o, ww, laid) = memo.get(id)?;
        (*was == s && *o == open && *ww == w).then(|| laid.clone())
    });
    if let Some(laid) = kept {
        return laid;
    }
    let laid = lay_steps(&s, open, w, nest);
    STEPS.with_borrow_mut(|memo| {
        if memo.len() >= MEMOS {
            memo.clear();
        }
        memo.insert(id.to_owned(), (s, open, w, laid.clone()));
    });
    laid
}

fn lay_steps(s: &Summary, open: bool, w: i32, nest: i32) -> Rc<Laid> {
    #[cfg(test)]
    LAYS.with(|n| n.set(n.get() + 1));
    let mut l = Laid::default();
    let mut tail = Vec::new();
    if s.failed > 0 {
        tail.push((format!("{} failed", s.failed), Mark::Failed));
    }
    tail.extend(took(s.ms).map(|took| (took, Mark::Muted)));
    l.head(Head {
        x: nest,
        w: (w - nest).max(1),
        open,
        name: &s.text,
        bold: false,
        gist: s.running.as_deref().unwrap_or_default(),
        tail,
        running: s.running.is_some(),
        path: false,
        mono: true,
    });
    Rc::new(l)
}

impl Painter {
    /// A pane's chat, the size of its pane. The blinking dots (see
    /// `chat_blinkers`) are composed over it.
    pub(super) fn agent_chat(
        &self,
        view: &ChatView,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let r = view.rect;
        let mut c = Canvas::new(physical(r.size, scale));
        // The ground a terminal pane has, as the chat stands in for one.
        c.fill(0, 0, c.w, c.h, p.terminal_background);
        let here = |rect: Rect| Rectangle::new(rect.loc - r.loc, rect.size);
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let rule = |c: &mut Canvas, y: i32| {
            c.fill(0, px(y - r.loc.y), c.w, px(1).max(1), p.border_variant)
        };
        let parts = parts(view);

        // The head: the dot and the label, and the word for its status.
        let head = here(parts.head);
        let (dot, word) = standing(view);
        status_dot(&mut c, centred_dot(head_slot(head)), dot, scale, p);
        let (hx, hend) = phys(head, scale);
        let pad = px(PAD) as f32;
        let word_x = hend - pad - self.text_width(word, SMALL_FONT_SIZE, scale);
        let b = band(head, scale);
        self.text(
            &mut c,
            word,
            SMALL_FONT_SIZE,
            word_x,
            hend,
            b,
            scale,
            p.muted,
        );
        let label_x = hx + px(PAD + DOT_SLOT + CARD_GAP / 2) as f32;
        let label_end = word_x - px(CARD_GAP) as f32;
        self.bold_text(
            &mut c,
            &view.label,
            FONT_SIZE,
            label_x,
            label_end,
            b,
            scale,
            p.text,
        );
        rule(&mut c, parts.head.loc.y + parts.head.size.h - 1);

        // The list, or what it says in its place.
        let list = here(parts.list);
        if let Some(message) = message_layout(view, parts.list) {
            for (text, rect) in &message.lines {
                self.centered(&mut c, text, FONT_SIZE, here(*rect), scale, p.muted);
            }
            if let Some(button) = message.screen {
                let lit = view.hover == Some(ChatHit::ShowScreen);
                let style = ButtonStyle::Plain;
                self.button(
                    &mut c,
                    "Show screen",
                    here(button),
                    (lit, false),
                    style,
                    scale,
                    p,
                );
            }
        } else {
            let (items, content) = stacked(view, parts.column.1);
            let top = top_of(view, content, list.size.h);
            let (lx, ly, lw, lh) = rect_px(list, scale);
            c.clipped(lx as i32, ly as i32, lw as i32, lh as i32, |c| {
                if let Some(reading) = view.earlier {
                    let row = rect(
                        parts.column.0 - r.loc.x,
                        list.loc.y + PART_GAP - top,
                        parts.column.1,
                        CARD_ROW,
                    );
                    let hover = if view.hover == Some(ChatHit::Earlier) {
                        1.0
                    } else {
                        0.0
                    };
                    self.row_highlight(c, row, (false, hover), scale, p);
                    let (words, mark) = earlier_words(reading);
                    self.centered(c, words, FONT_SIZE, row, scale, mark.color(p));
                }
                for i in in_view(&items, top, list.size.h) {
                    let (y, laid) = &items[i];
                    let at = Point::from((parts.column.0 - r.loc.x, list.loc.y + y - top));
                    self.chat_item(c, view, (i, laid), at, parts.column.1, scale, p);
                }
            });
            if let Some(thumb) = thumb_of(&parts, content, top) {
                let thumb = Thumb {
                    track: here(thumb.track),
                    top: thumb.top - r.loc.y as f64,
                    ..thumb
                };
                self.thumb_in(&mut c, &thumb, scale, p);
            }
        }
        rule(&mut c, parts.list.loc.y + parts.list.size.h);

        // The waiting panel: the agent's screen rows and the keys to answer.
        if let Some(panel) = parts.waiting {
            let (x, y, w, h) = rect_px(here(panel), scale);
            let radius = ROW_RADIUS * scale as f32;
            // Framed: its ground is the chat's.
            let line = (scale as f32).round().max(1.0);
            c.rounded(x, y, w, h, radius, p.border_variant);
            let (x, y, w, h) = (x + line, y + line, w - 2.0 * line, h - 2.0 * line);
            c.rounded(x, y, w, h, radius, p.terminal_background);
            if let Some((at, laid)) = &parts.card {
                let at = *at - r.loc;
                if view.hover == Some(ChatHit::AskFold) {
                    for (spot, _) in &laid.spots {
                        let row = Rectangle::new(spot.loc + at, spot.size);
                        self.row_highlight(&mut c, row, (false, 1.0), scale, p);
                    }
                }
                for (rect, bit) in &laid.bits {
                    let rect = Rectangle::new(rect.loc + at, rect.size);
                    self.chat_bit(&mut c, rect, bit, scale, p);
                }
                for (hit, rect) in &parts.answers {
                    let label = (ASK_BUTTONS.iter())
                        .find(|(h, _)| h == hit)
                        .map_or("", |(_, l)| l);
                    let style = match hit {
                        ChatHit::Answer(AskReply::Allow) => ButtonStyle::Primary,
                        _ => ButtonStyle::Plain,
                    };
                    let lit = view.hover == Some(*hit);
                    self.button(&mut c, label, here(*rect), (lit, false), style, scale, p);
                }
            } else if let Some((at, laid)) = &parts.menu {
                let at = here(*at).loc;
                for (rect, spot) in &laid.spots {
                    // The bubble's tint: the hover ground is the panel's own
                    // in the light theme.
                    if matches!(spot, Spot::Key(key) if view.hover == Some(ChatHit::Key(*key))) {
                        let rect = Rectangle::new(rect.loc + at, rect.size);
                        let (x, y, w, h) = rect_px(rect, scale);
                        c.rounded(x, y, w, h, ROW_RADIUS * scale as f32, bubble(p));
                    }
                }
                for (rect, bit) in &laid.bits {
                    let rect = Rectangle::new(rect.loc + at, rect.size);
                    self.chat_bit(&mut c, rect, bit, scale, p);
                }
            } else {
                let screen = view
                    .waiting
                    .as_deref()
                    .map_or(&[][..], |rows| screen_rows(rows));
                let n = (parts.rows.size.h / STYLED_ROW) as usize;
                let rows: Vec<StyledRow> = (screen[screen.len().saturating_sub(n)..].iter())
                    .map(|(text, styles)| (text.as_str(), styles.as_slice()))
                    .collect();
                self.paint_styled_rows(&mut c, &rows, here(parts.rows), scale, p);
            }
            for (key, rect) in &parts.keys {
                let lit = view.hover == Some(ChatHit::Key(*key));
                self.button(
                    &mut c,
                    &key_label(*key),
                    here(*rect),
                    (lit, false),
                    ButtonStyle::Plain,
                    scale,
                    p,
                );
            }
        }

        // The reply box and Send, dimmed while there is nothing to send or
        // the agent is paused.
        let focused = view.focused && view.focus == ChatFocus::Composer;
        let hint = format!("Reply to {}…", agent_name(&view.label));
        let composer = here(parts.composer);
        self.wrap_field(&mut c, &view.draft, &hint, composer, focused, scale, p);
        let style = match view.paused || view.draft.text.trim().is_empty() {
            true => ButtonStyle::Off,
            false => ButtonStyle::Primary,
        };
        let lit = view.hover == Some(ChatHit::Send);
        self.button(
            &mut c,
            "Send",
            here(parts.send),
            (lit, false),
            style,
            scale,
            p,
        );

        let hints = chat_hints(view);
        let spots = hint_spots(&hints, here(parts.hints));
        self.hints(&mut c, &hints, &spots, None, scale, p);
        c.into_buffer(true)
    }

    /// Row `i`, laid out as `laid`, with its `(0, 0)` at `at`: on the
    /// selected ground when picked, the element's when hovered and a press
    /// does something on it (words don't light up).
    #[allow(clippy::too_many_arguments)]
    fn chat_item(
        &self,
        c: &mut Canvas,
        view: &ChatView,
        (i, laid): (usize, &Laid),
        at: Point<i32, Logical>,
        w: i32,
        scale: f64,
        p: &Palette,
    ) {
        let hovered = !laid.spots.is_empty()
            && matches!(
                view.hover,
                Some(ChatHit::Item(j) | ChatHit::Fold(j) | ChatHit::Path(j) | ChatHit::ShowAll(j))
                    if j == i
            );
        let ground = rect(
            at.x - ROW_INSET,
            at.y - STEP_GAP / 2,
            w + 2 * ROW_INSET,
            laid.h + STEP_GAP,
        );
        let hover = if hovered { 1.0 } else { 0.0 };
        self.row_highlight(c, ground, (view.picked == Some(i), hover), scale, p);
        for (rect, bit) in &laid.bits {
            let rect = Rectangle::new(rect.loc + at, rect.size);
            self.chat_bit(c, rect, bit, scale, p);
        }
    }

    fn chat_bit(&self, c: &mut Canvas, rect: Rect, bit: &Bit, scale: f64, p: &Palette) {
        let s = scale as f32;
        let ((x, end), b) = (phys(rect, scale), band(rect, scale));
        match bit {
            Bit::Bubble => {
                let (x, y, w, h) = rect_px(rect, scale);
                c.rounded(x, y, w, h, ROW_RADIUS * s, bubble(p));
            }
            Bit::Doc(piece) => self.doc_piece(c, rect, piece, scale, p),
            Bit::Fold(open) => icon_sdf(c, rect, scale, p.muted, |x, y| match open {
                true => chevron_down(x, y, s),
                false => chevron_side(false, x, y, s),
            }),
            Bit::Label {
                text,
                face,
                size,
                mark,
                centred,
            } => {
                let color = mark.color(p);
                match (face, centred) {
                    (_, true) => self.centered(c, text, *size, rect, scale, color),
                    (Face::Bold, _) => {
                        self.bold_text(c, text, *size, x, end, b, scale, color);
                    }
                    (Face::Mono | Face::MonoBold, _) => {
                        self.mono_text(c, text, *size, x, end, b, scale, color);
                    }
                    (Face::Ui, _) => {
                        self.text(c, text, *size, x, end, b, scale, color);
                    }
                }
            }
            Bit::Diff(side, text) => {
                let (x, y, w, h) = rect_px(rect, scale);
                let tone = match side {
                    DiffSide::Added => Some(p.success),
                    DiffSide::Removed => Some(p.error),
                    DiffSide::Context => None,
                };
                if let Some(tone) = tone {
                    let (x, y, w, h) = (x as i32, y as i32, w as i32, h as i32);
                    c.fill(x, y, w, h, mix(p.terminal_background, tone, DIFF_TINT));
                    c.fill(x, y, (DIFF_BAR as f32 * s).round() as i32, h, tone);
                }
                let tx = x + (DIFF_INSET as f32 * s).round();
                let size = Font::Small.size();
                self.mono_text(c, text, size, tx, x + w, b, scale, p.text);
            }
            Bit::Running => {}
            Bit::Picked => self.row_highlight(c, rect, (true, 0.0), scale, p),
            Bit::Check => icon_sdf(c, rect, scale, p.success, |x, y| check(x, y, s)),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::chat::{DiffLine, ToolOut};
    use crate::chrome::{Theme, inside};

    /// A chat of each kind of item, in a pane at `(200, 60)`.
    pub(in crate::chrome) fn items() -> Arc<Vec<Item>> {
        let tool = |id: &str, name: &str, gist: &str, out: Option<&str>| Item::Tool {
            id: id.into(),
            name: name.into(),
            gist: gist.into(),
            input: gist.into(),
            output: out.map(|text| ToolOut {
                text: text.into(),
                failed: false,
                ms: Some(4100),
            }),
        };
        let diff = |side, text: &str| DiffLine {
            side,
            text: text.into(),
        };
        Arc::new(vec![
            Item::User {
                id: "u1".into(),
                at: None,
                text: "Add a retry to the upload client".into(),
            },
            Item::Assistant {
                id: "a1".into(),
                at: None,
                blocks: Arc::new(crate::md_blocks::blocks(
                    "I'll read the client first, then wrap `send` in a **retry**.",
                )),
            },
            tool("t1", "Read", "src/upload.rs", Some("fn upload() {}\n")),
            Item::Thinking {
                id: "th".into(),
                text: "The client posts once; a retry with backoff fits here.".into(),
            },
            Item::Edit {
                id: "e1".into(),
                path: "src/upload.rs".into(),
                diff: vec![
                    diff(
                        DiffSide::Context,
                        "fn upload(client: &Client) -> Result<()> {",
                    ),
                    diff(
                        DiffSide::Removed,
                        "    let resp = client.post(url).send()?;",
                    ),
                    diff(
                        DiffSide::Added,
                        "    let resp = retry(3, || client.post(url).send())?;",
                    ),
                    diff(DiffSide::Context, "}"),
                ],
                output: Some(ToolOut {
                    text: String::new(),
                    failed: false,
                    ms: None,
                }),
            },
            tool(
                "t2",
                "Bash",
                "cargo test upload",
                Some("running 4 tests\ntest upload::retries ... ok\ntest result: ok. 4 passed"),
            ),
            tool(
                "t3",
                "Bash",
                "cargo clippy --all-targets -- -D warnings",
                None,
            ),
            Item::Note {
                id: "n1".into(),
                text: "Compacted the chat".into(),
            },
            Item::Assistant {
                id: "a2".into(),
                at: None,
                blocks: Arc::new(crate::md_blocks::blocks(
                    "Done. Retries 3 times with **backoff**:\n\n- 100 ms\n- 200 ms\n- 400 ms",
                )),
            },
        ])
    }

    pub(in crate::chrome) fn view() -> ChatView {
        ChatView {
            rect: Rectangle::new((200, 60).into(), (640, 760).into()),
            label: "claude · api".into(),
            cwd: None,
            status: AgentStatus::Working,
            paused: false,
            reading: Reading::Read,
            items: items(),
            unfolded: ["t2", "e1"].into_iter().map(String::from).collect(),
            picked: None,
            top: 0.0,
            focus: ChatFocus::List,
            focused: true,
            draft: Line::default(),
            waiting: None,
            hints: crate::chat::footer(ChatFocus::List, true),
            earlier: None,
            hover: None,
            running: Vec::new(),
            ask: None,
        }
    }

    /// A pane's screen rows of an approval menu, as herdr gives them.
    pub(in crate::chrome) fn screen() -> Arc<Vec<(String, crate::scrollback::Styled)>> {
        let ansi = "\x1b[1mBash command\x1b[0m\n\n  rm -rf target\n\n\x1b[33mDo you want to proceed?\x1b[0m\n\x1b[36m❯ 1. Yes\x1b[0m\n  2. Yes, and don't ask again\n  3. No\n\n\n";
        Arc::new(crate::scrollback::parse_ansi(ansi))
    }

    /// Claude Code asking the second of its questions, as herdr reads the
    /// screen: the current tab in reverse.
    pub(in crate::chrome) fn question() -> Arc<Vec<(String, crate::scrollback::Styled)>> {
        let ansi = "←  ☒ Size  \x1b[7m☐ Where\x1b[27m  ✔ Submit  →\n\n\x1b[1mWhere should the size be changed from? The switcher has rows for it already, keys are quicker.\x1b[0m\n\n❯ 1. Switcher\n     Rows 'Bigger UI', 'Smaller UI' and 'Reset UI size' in the switcher, in steps of 10%.\n  2. Keys\n     Prefix keys only.\n  3. Type something.\n\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel\n";
        Arc::new(crate::scrollback::parse_ansi(ansi))
    }

    fn middle(r: Rect) -> Point<f64, Logical> {
        (
            r.loc.x as f64 + r.size.w as f64 / 2.0,
            r.loc.y as f64 + r.size.h as f64 / 2.0,
        )
            .into()
    }

    /// Where item `i` of `view` is, in output coordinates.
    fn item_rect(view: &ChatView, i: usize) -> Rect {
        let parts = parts(view);
        let (items, content) = stacked(view, parts.column.1);
        let top = top_of(view, content, parts.list.size.h);
        let (y, laid) = &items[i];
        rect(
            parts.column.0,
            parts.list.loc.y + y - top,
            parts.column.1,
            laid.h,
        )
    }

    #[test]
    fn the_parts_stack_in_the_pane_and_the_reply_box_grows() {
        let v = view();
        let p = parts(&v);
        for part in [p.head, p.list, p.composer, p.send, p.hints] {
            assert!(inside(v.rect, part), "{part:?}");
        }
        assert_eq!(p.list.loc.y, p.head.loc.y + p.head.size.h);
        assert!(p.list.loc.y + p.list.size.h < p.composer.loc.y);
        assert!(p.composer.loc.y + p.composer.size.h < p.hints.loc.y);
        assert!(p.composer.loc.x + p.composer.size.w < p.send.loc.x);
        assert!(p.waiting.is_none() && p.keys.is_empty());
        // Lines written make it taller, the list shorter.
        let mut long = v.clone();
        long.draft = Line {
            text: "one\ntwo\nthree".into(),
            ..Line::default()
        };
        let q = parts(&long);
        assert!(q.composer.size.h > p.composer.size.h);
        assert!(q.list.size.h < p.list.size.h);
        // Send stays at the box's bottom.
        assert_eq!(
            q.send.loc.y + q.send.size.h,
            q.composer.loc.y + q.composer.size.h
        );
    }

    #[test]
    fn the_waiting_panel_holds_the_screen_rows_and_the_keys() {
        let mut v = view();
        // No menu on it: a prompt asking for a word.
        let ansi = "\x1b[1mTrust this folder?\x1b[0m\n\n  /home/u/api\n\n  Type yes or no\n\n\n";
        v.waiting = Some(Arc::new(crate::scrollback::parse_ansi(ansi)));
        let p = parts(&v);
        let panel = p.waiting.expect("a waiting panel");
        assert!(p.menu.is_none());
        assert!(panel.loc.y > p.list.loc.y + p.list.size.h);
        assert!(panel.loc.y + panel.size.h < p.composer.loc.y);
        // The rows up to the last with words on them.
        let shown = screen_rows(v.waiting.as_deref().unwrap());
        assert_eq!(shown.last().unwrap().0.trim(), "Type yes or no");
        assert_eq!(p.rows.size.h, shown.len() as i32 * STYLED_ROW);
        assert_eq!(p.keys.len(), WAIT_KEYS.len());
        for (_, key) in &p.keys {
            assert!(inside(panel, *key));
        }
        let layout = chat_layout(&v);
        let (esc, enter) = (p.keys[0].1, p.keys[6].1);
        assert_eq!(
            layout.hit(&v, middle(esc)),
            Some(ChatHit::Key(WaitKey::Esc))
        );
        assert_eq!(
            layout.hit(&v, middle(enter)),
            Some(ChatHit::Key(WaitKey::Enter))
        );
        assert_eq!(
            layout.hit(&v, middle(p.keys[2].1)),
            Some(ChatHit::Key(WaitKey::Digit(2)))
        );
        // A short pane keeps its list three card rows: the rows give way.
        v.rect.size.h = 260;
        let p = parts(&v);
        assert!(p.list.size.h >= 3 * CARD_ROW, "{:?}", p.list);
        assert!(p.rows.size.h < shown.len() as i32 * STYLED_ROW);
        // Too narrow for all keys: those that fit.
        v.rect.size.w = 260;
        let p = parts(&v);
        assert!(!p.keys.is_empty() && p.keys.len() < WAIT_KEYS.len());
        assert!(
            p.keys
                .iter()
                .all(|(_, key)| inside(p.waiting.unwrap(), *key))
        );
    }

    #[test]
    fn a_menu_on_the_screen_shows_as_choices_to_press() {
        let mut v = view();
        v.waiting = Some(screen());
        let p = parts(&v);
        let panel = p.waiting.expect("a waiting panel");
        let (at, laid) = p.menu.clone().expect("the menu, not the rows");
        assert!(inside(panel, at));
        // Its choices are its buttons: no digit keys, no tab keys.
        let keys: Vec<WaitKey> = p.keys.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [WaitKey::Esc, WaitKey::Up, WaitKey::Down, WaitKey::Enter]
        );
        // A press on a choice presses its number.
        let choices: Vec<Rect> = (laid.spots.iter())
            .map(|(r, _)| Rectangle::new(r.loc + at.loc, r.size))
            .collect();
        assert_eq!(choices.len(), 3);
        let layout = chat_layout(&v);
        for (n, choice) in (1..).zip(&choices) {
            assert_eq!(
                layout.hit(&v, middle(*choice)),
                Some(ChatHit::Key(WaitKey::Digit(n)))
            );
        }
        // The one under the arrow is lit, its words bold.
        assert!(
            laid.bits.iter().any(|(r, bit)| *bit == Bit::Picked
                && Rectangle::new(r.loc + at.loc, r.size) == choices[0])
        );
        let said = |text: &str| {
            (laid.bits.iter())
                .any(|(_, bit)| matches!(bit, Bit::Label { text: t, .. } if t == text))
        };
        assert!(said("Yes") && said("No") && said("2."));
        // A pane too short for it shows the screen's rows.
        v.rect.size.h = 260;
        assert!(parts(&v).menu.is_none());
    }

    #[test]
    fn a_menu_with_questions_shows_their_tabs_and_the_tab_keys() {
        let mut v = view();
        v.waiting = Some(question());
        let p = parts(&v);
        let (_, laid) = p.menu.expect("a menu");
        let keys: Vec<WaitKey> = p.keys.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, MENU_KEYS);
        let labels: Vec<&str> = (laid.bits.iter())
            .filter_map(|(_, bit)| match bit {
                Bit::Label { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Size",
                "Where",
                "Submit",
                "1.",
                "Switcher",
                "2.",
                "Keys",
                "3.",
                "Type something."
            ]
        );
    }

    #[test]
    fn presses_find_the_part_they_are_on() {
        let v = view();
        let layout = chat_layout(&v);
        assert_eq!(layout.hit(&v, middle(layout.send)), Some(ChatHit::Send));
        assert_eq!(
            layout.hit(&v, middle(layout.composer)),
            Some(ChatHit::Composer)
        );
        assert_eq!(layout.hit(&v, middle(parts(&v).head)), None);
        // The user's words: the item. A steps row: its fold.
        assert_eq!(
            layout.hit(&v, middle(item_rect(&v, 0))),
            Some(ChatHit::Item(0))
        );
        let steps = item_rect(&v, 2);
        let row =
            |r: Rect, x: i32| Point::from(((r.loc.x + x) as f64, (r.loc.y + CARD_ROW / 2) as f64));
        assert_eq!(layout.hit(&v, row(steps, 4)), Some(ChatHit::Fold(2)));
        // An edit's path, right of its name, opens the file.
        let edit = item_rect(&v, 3);
        let name = width_in("Edit", Face::Bold, FONT_SIZE).ceil() as i32;
        let on_path = CHEVRON_WIDTH + name + CARD_GAP + 4;
        assert_eq!(layout.hit(&v, row(edit, on_path)), Some(ChatHit::Path(3)));
        // Its unfolded change is the item, not its fold.
        let low = Point::from((middle(edit).x, (edit.loc.y + edit.size.h - 4) as f64));
        assert_eq!(layout.hit(&v, low), Some(ChatHit::Item(3)));
        // Between two items: nothing.
        let gap = Point::from((middle(edit).x, (edit.loc.y + edit.size.h + 2) as f64));
        assert_eq!(layout.hit(&v, gap), None);
    }

    #[test]
    fn a_long_output_is_cut_with_a_show_all() {
        let mut v = view();
        let text: String = (0..250).map(|i| format!("line {i}\n")).collect();
        let long = Item::Tool {
            id: "long".into(),
            name: "Bash".into(),
            gist: "seq 250".into(),
            input: String::new(),
            output: Some(ToolOut {
                text,
                failed: false,
                ms: None,
            }),
        };
        v.items = Arc::new(vec![long]);
        v.unfolded = ["long".to_owned(), rows::steps_key("long")].into();
        let laid = laid(&v.items[0], 500, 0, &v.unfolded, None);
        let code = laid
            .bits
            .iter()
            .filter(|(_, b)| matches!(b, Bit::Doc(DocPiece::CodeLine(t)) if !t.is_empty()));
        assert_eq!(code.count(), CHAT_CUT);
        let (spot, _) = laid
            .spots
            .iter()
            .find(|(_, s)| *s == Spot::ShowAll)
            .expect("Show all");
        assert!(
            laid.bits.iter().any(
                |(_, b)| matches!(b, Bit::Label { text, .. } if text == "Show all (250 lines)")
            )
        );
        assert_eq!(spot.loc.y + spot.size.h, laid.h);
        // Pressed where the list shows it: in its open steps, row 1.
        let mut parts_v = v.clone();
        parts_v.top = f64::MAX;
        let layout = chat_layout(&parts_v);
        let list = layout.list;
        let p = parts(&parts_v);
        let (shown, _) = stacked(&parts_v, p.column.1);
        let (spot, _) = (shown[1].1.spots.iter())
            .find(|(_, s)| *s == Spot::ShowAll)
            .expect("Show all in the list");
        let at = Point::from((
            (p.column.0 + spot.loc.x + 4) as f64,
            (list.loc.y + list.size.h - PART_GAP - 4) as f64,
        ));
        assert_eq!(layout.hit(&parts_v, at), Some(ChatHit::ShowAll(1)));
    }

    #[test]
    fn a_cards_gist_stops_before_its_state() {
        let gist = "a very long command ".repeat(20);
        let item = Item::Tool {
            id: "g".into(),
            name: "Bash".into(),
            gist,
            input: String::new(),
            output: Some(ToolOut {
                text: String::new(),
                failed: true,
                ms: Some(1500),
            }),
        };
        let laid = laid(&item, 400, 0, &HashSet::new(), None);
        let labels: Vec<(Rect, &str, Mark)> = (laid.bits.iter())
            .filter_map(|(r, b)| match b {
                Bit::Label { text, mark, .. } => Some((*r, text.as_str(), *mark)),
                _ => None,
            })
            .collect();
        let (gist, state) = (labels[1], labels[2]);
        assert_eq!(state.1, "failed · 1.5 s");
        assert_eq!(state.2, Mark::Failed);
        assert!(
            gist.0.loc.x + gist.0.size.w < state.0.loc.x,
            "{gist:?} {state:?}"
        );
        assert!(state.0.loc.x + state.0.size.w <= 400);
        // Folded: one row.
        assert_eq!(laid.h, CARD_ROW);
    }

    #[test]
    fn the_users_words_sit_right_and_a_folded_edit_shows_its_first_lines() {
        let v = view();
        let p = parts(&v);
        let (shown, _) = stacked(&v, p.column.1);
        let bubble = (shown[0].1.bits.iter())
            .find_map(|(r, b)| (*b == Bit::Bubble).then_some(*r))
            .expect("a bubble");
        assert_eq!(bubble.loc.x + bubble.size.w, p.column.1);
        assert!(bubble.loc.x > 0);
        let diff: Vec<DiffLine> = (0..20)
            .map(|i| DiffLine {
                side: DiffSide::Added,
                text: format!("line {i}"),
            })
            .collect();
        let edit = Item::Edit {
            id: "big".into(),
            path: "/w/src/a.rs".into(),
            diff,
            output: None,
        };
        let cwd = Some(Path::new("/w"));
        let folded = laid(&edit, 500, 0, &HashSet::new(), cwd);
        let lines = |l: &Laid| {
            (l.bits.iter())
                .filter(|(_, b)| matches!(b, Bit::Diff(..)))
                .count()
        };
        assert_eq!(lines(&folded), EDIT_PEEK);
        let more = (folded.bits.iter())
            .any(|(_, b)| matches!(b, Bit::Label { text, .. } if text == "12 more lines"));
        assert!(more);
        assert!(
            folded
                .spots
                .iter()
                .filter(|(_, s)| *s == Spot::Fold)
                .count()
                == 2
        );
        // Its path from the agent's folder.
        let path = (folded.bits.iter())
            .any(|(_, b)| matches!(b, Bit::Label { text, .. } if text == "src/a.rs"));
        assert!(path);
        let open = laid(&edit, 500, 0, &HashSet::from(["big".to_owned()]), cwd);
        assert_eq!(lines(&open), 20);
    }

    #[test]
    fn a_subagents_prompt_shows_its_first_lines_till_unfolded() {
        let text = (0..10)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let user = Item::User {
            id: "p".into(),
            at: None,
            text,
        };
        let lines = |l: &Laid| {
            (l.bits.iter())
                .filter(|(_, b)| matches!(b, Bit::Doc(DocPiece::Text { .. })))
                .count()
        };
        let none = HashSet::new();
        // The chat's own words show whole.
        assert_eq!(lines(&laid(&user, 500, 0, &none, None)), 10);
        let folded = laid(&user, 500, NEST, &none, None);
        assert_eq!(lines(&folded), PROMPT_PEEK);
        let more = (folded.bits.iter())
            .any(|(_, b)| matches!(b, Bit::Label { text, .. } if text == "7 more lines"));
        assert!(more);
        let open = laid(&user, 500, NEST, &HashSet::from(["p".to_owned()]), None);
        assert_eq!(lines(&open), 10);
        assert!(open.spots.iter().any(|(_, s)| *s == Spot::Fold));
    }

    #[test]
    fn the_column_stops_at_its_widest_and_stands_in_the_middle() {
        let mut v = view();
        v.rect.size.w = 2000;
        let p = parts(&v);
        assert_eq!(p.column.1, CHAT_MAX_W);
        let left = p.column.0 - v.rect.loc.x;
        let right = v.rect.loc.x + v.rect.size.w - (p.column.0 + p.column.1);
        assert!((left - right).abs() <= 1, "{left} {right}");
        assert!(p.bar_x > p.column.0 + p.column.1);
    }

    #[test]
    fn earlier_messages_sit_above_the_first_one_read() {
        let mut v = view();
        let below = item_rect(&v, 0).loc.y;
        v.earlier = Some(false);
        let layout = chat_layout(&v);
        let first = item_rect(&v, 0);
        assert_eq!(first.loc.y - below, CARD_ROW + TURN_GAP);
        let p = parts(&v);
        let row = Point::from((
            (p.column.0 + p.column.1 / 2) as f64,
            (layout.list.loc.y + PART_GAP + CARD_ROW / 2) as f64,
        ));
        assert_eq!(layout.hit(&v, row), Some(ChatHit::Earlier));
        assert_eq!(layout.hit(&v, middle(first)), Some(ChatHit::Item(0)));
        assert_eq!(earlier_words(true).0, "Reading earlier messages…");
    }

    #[test]
    fn a_step_in_an_open_subagent_folds_where_it_shows() {
        let mut v = view();
        let own = vec![
            Item::Assistant {
                id: "s1".into(),
                at: None,
                blocks: Arc::new(crate::md_blocks::blocks("Looking.")),
            },
            Item::Tool {
                id: "st1".into(),
                name: "Grep".into(),
                gist: "fn parse".into(),
                input: String::new(),
                output: None,
            },
        ];
        v.items = Arc::new(vec![Item::Subagent {
            id: "agent:t".into(),
            title: "Find it".into(),
            items: Arc::new(own),
            output: None,
        }]);
        v.unfolded = ["agent:t".to_owned()].into();
        let layout = chat_layout(&v);
        // The card, its words, its steps: three rows, the steps in from it.
        let steps = item_rect(&v, 2);
        let at = |x: i32| {
            Point::from((
                (steps.loc.x + x) as f64,
                (steps.loc.y + CARD_ROW / 2) as f64,
            ))
        };
        assert_eq!(layout.hit(&v, at(NEST + 4)), Some(ChatHit::Fold(2)));
        let (shown, _) = stacked(&v, parts(&v).column.1);
        assert_eq!(shown.len(), 3);
        let fold = (shown[2].1.bits.iter())
            .find_map(|(r, b)| matches!(b, Bit::Fold(_)).then_some(r.loc.x))
            .unwrap();
        assert_eq!(fold, NEST);
    }

    #[test]
    fn items_are_laid_out_once_until_they_change() {
        let v = view();
        let w = parts(&v).column.1;
        let count = || LAYS.with(|n| n.get());
        let _ = stacked(&v, w);
        let before = count();
        let (first, h) = stacked(&v, w);
        assert_eq!(count(), before, "nothing laid out again");
        // A new read of the same items, as the tail sends: still none.
        let mut again = v.clone();
        again.items = Arc::new(v.items.as_ref().clone());
        let (second, same) = stacked(&again, w);
        assert_eq!(count(), before);
        assert_eq!(h, same);
        assert!(Rc::ptr_eq(&first[0].1, &second[0].1));
        // Opening steps lays out their row and its two steps.
        again.unfolded.insert(rows::steps_key("t2"));
        let (third, taller) = stacked(&again, w);
        assert_eq!(count(), before + 3);
        assert!(taller > h);
        assert!(Rc::ptr_eq(&first[0].1, &third[0].1));
        assert!(!Rc::ptr_eq(&first[4].1, &third[4].1));
        // Another width lays them all out.
        let _ = stacked(&again, w - 40);
        assert_eq!(count(), before + 3 + third.len());
    }

    #[test]
    fn only_the_items_in_view_are_painted() {
        let mut v = view();
        let many: Vec<Item> = (0..400)
            .map(|i| Item::User {
                id: format!("u{i}"),
                at: None,
                text: format!("message {i}"),
            })
            .collect();
        v.items = Arc::new(many);
        let p = parts(&v);
        let (items, content) = stacked(&v, p.column.1);
        let shown = in_view(&items, 0, p.list.size.h);
        assert_eq!(shown.start, 0);
        assert!(shown.len() < 20, "{shown:?}");
        // Scrolled to the end, the last ones; past it, the same.
        let top = content - p.list.size.h;
        let end = in_view(&items, top, p.list.size.h);
        assert_eq!(end.end, items.len());
        v.top = f64::MAX;
        assert_eq!(top_of(&v, content, p.list.size.h), top);
        let (y, laid) = &items[end.start];
        assert!(y + laid.h > top);
        let (y, laid) = &items[end.start - 1];
        assert!(y + laid.h <= top);
        // The bar shows, at the bottom.
        let thumb = chat_thumb(&v).expect("a scrollbar");
        assert_eq!(
            thumb.top + thumb.h,
            (thumb.track.loc.y + thumb.track.size.h) as f64
        );
        assert_eq!(
            chat_layout(&v).hit(&v, middle(thumb.track)),
            Some(ChatHit::Scrollbar)
        );
    }

    #[test]
    fn an_empty_or_missing_chat_says_so() {
        let mut v = view();
        v.items = Arc::new(Vec::new());
        v.reading = Reading::Finding;
        assert_eq!(message(&v), Some(("Reading the chat…", false)));
        v.reading = Reading::Read;
        assert_eq!(message(&v), Some(("Nothing said yet", false)));
        let layout = chat_layout(&v);
        assert_eq!(layout.content, 0);
        assert!(chat_thumb(&v).is_none());
        let why = "Couldn't find this agent's chat. Its screen still works.";
        v.reading = Reading::Missing(why.into());
        let m = message_layout(&v, parts(&v).list).unwrap();
        assert_eq!(m.lines.len(), 1);
        assert_eq!(m.lines[0].0, why);
        let button = m.screen.expect("Show screen");
        assert_eq!(
            chat_layout(&v).hit(&v, middle(button)),
            Some(ChatHit::ShowScreen)
        );
        // A narrow pane wraps it.
        v.rect.size.w = 240;
        let m = message_layout(&v, parts(&v).list).unwrap();
        assert!(m.lines.len() > 1);
        assert_eq!(
            m.lines
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            why
        );
        // Items read before stay shown while it reads again.
        let mut v = view();
        v.reading = Reading::Finding;
        assert!(message(&v).is_none());
    }

    #[test]
    fn the_footer_says_the_keys_of_where_the_keys_are() {
        let mut v = view();
        let keys = |v: &ChatView| {
            chat_hints(v)
                .into_iter()
                .map(|(k, _)| k.to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&v), ["J / K", "Space", "Y", "I"]);
        v.focus = ChatFocus::Composer;
        v.hints = crate::chat::footer(ChatFocus::Composer, true);
        assert_eq!(chat_hints(&v)[2], ("Esc", "back"));
        assert!(!keys(&v).iter().any(|k| k == crate::text_field::DISCARD_KEY));
        v.draft.text = "half a reply".into();
        v.hints = crate::chat::footer(ChatFocus::Composer, false);
        assert_eq!(chat_hints(&v)[2], ("Esc", "keep draft"));
        assert!(keys(&v).iter().any(|k| k == crate::text_field::DISCARD_KEY));
        let spots = hint_spots(&chat_hints(&v), parts(&v).hints);
        let (i, spot) = spots[0];
        assert_eq!(chat_hint_at(&v, middle(spot)), Some(i));
        let field = chat_field_spot(&v, 1.0).unwrap();
        assert_eq!(field.area, chat_layout(&v).composer);
        assert_eq!(field.line.text, "half a reply");
    }

    #[test]
    fn the_head_and_running_tools_blink_only_while_the_agent_works() {
        let mut v = view();
        let dots = chat_blinkers(&v);
        // The head's, and that of the steps the clippy card runs in.
        assert_eq!(dots.len(), 2, "{dots:?}");
        assert!(inside(parts(&v).head, dots[0]));
        assert!(inside(item_rect(&v, 4), dots[1]));
        v.status = AgentStatus::Blocked;
        assert!(chat_blinkers(&v).is_empty());
        v.status = AgentStatus::Working;
        v.paused = true;
        assert!(chat_blinkers(&v).is_empty());
        assert_eq!(standing(&v).1, "paused");
    }

    fn running(call: &str) -> Running {
        Running {
            call: call.into(),
            name: "Bash".into(),
            gist: "cargo test".into(),
            since_ms: 0,
        }
    }

    #[test]
    fn a_running_tool_the_transcript_lacks_shows_at_the_end() {
        let mut v = view();
        let w = parts(&v).column.1;
        let before = stacked(&v, w).0.len();
        v.running = vec![running("c9")];
        v.top = 1e9;
        let (rows, _) = stacked(&v, w);
        assert_eq!(rows.len(), before + 1);
        let last = item_rect(&v, before);
        assert!(rows[before].1.bits.iter().any(|(_, b)| *b == Bit::Running));
        // Its dot blinks while the agent works; a press there picks nothing.
        assert!(chat_blinkers(&v).iter().any(|dot| inside(last, *dot)));
        assert_eq!(chat_layout(&v).hit(&v, middle(last)), None);
        // An empty chat shows it, not "Nothing said yet".
        v.items = Arc::new(Vec::new());
        assert!(message(&v).is_none());
        assert_eq!(stacked(&v, w).0.len(), 1);
    }

    fn ask() -> PendingAsk {
        let input: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        PendingAsk {
            ask: "q1".into(),
            tool: "Bash".into(),
            gist: "rm -rf target".into(),
            input: input.join("\n"),
            since: std::time::Instant::now(),
        }
    }

    fn labels(laid: &Laid) -> Vec<String> {
        (laid.bits.iter())
            .filter_map(|(_, bit)| match bit {
                Bit::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_asks_card_takes_the_waiting_panels_place() {
        let mut v = view();
        v.waiting = Some(screen());
        v.ask = Some(ask());
        let p = parts(&v);
        let panel = p.waiting.expect("the card's panel");
        assert!(p.keys.is_empty() && p.rows.size.h == 0);
        assert!(panel.loc.y > p.list.loc.y + p.list.size.h);
        assert!(panel.loc.y + panel.size.h < p.composer.loc.y);
        let layout = chat_layout(&v);
        let hits: Vec<_> = (p.answers.iter())
            .map(|(_, b)| layout.hit(&v, middle(*b)))
            .collect();
        assert_eq!(
            hits,
            [
                Some(ChatHit::Answer(AskReply::Allow)),
                Some(ChatHit::Answer(AskReply::Deny)),
                Some(ChatHit::ShowScreen),
            ]
        );
        assert!(p.answers.iter().all(|(_, b)| inside(panel, *b)));
        let (at, laid) = p.card.as_ref().unwrap();
        let head = Point::from(((at.x + 4) as f64, (at.y + CARD_ROW / 2) as f64));
        assert_eq!(layout.hit(&v, head), Some(ChatHit::AskFold));
        // Folded: the tool and its gist, not what it would run.
        let said = labels(laid);
        assert!(said.contains(&"Bash".to_owned()) && said.contains(&"rm -rf target".to_owned()));
        assert!(!said.contains(&"line 0".to_owned()));
        assert!(
            said.iter()
                .any(|t| t.starts_with("claude waits for your answer"))
        );
        // The head says the agent waits, and nothing blinks.
        assert_eq!(standing(&v).1, "waits for you");
        assert!(chat_blinkers(&v).is_empty());
        // Unfolded: its first lines, and how many more.
        v.unfolded.insert(ask_key("q1"));
        let q = parts(&v);
        let said = labels(&q.card.as_ref().unwrap().1);
        assert!(said.contains(&"line 11".to_owned()) && !said.contains(&"line 12".to_owned()));
        assert!(said.contains(&"18 more lines".to_owned()));
        assert!(q.waiting.unwrap().size.h > panel.size.h);
        // A shorter pane keeps its list three card rows: the lines give way.
        v.rect.size.h = 460;
        let q = parts(&v);
        assert!(q.list.size.h >= 3 * CARD_ROW, "{:?}", q.list);
        let said = labels(&q.card.as_ref().unwrap().1);
        assert!(!said.contains(&"line 11".to_owned()));
    }

    /// The marks and words in their colours read on the chat's ground and
    /// on a picked item's, in both themes (L3).
    #[test]
    fn the_cards_words_read_in_both_themes() {
        for theme in [Theme::Dark, Theme::Light] {
            let p = theme.palette();
            for ground in [p.terminal_background, p.selected] {
                for mark in [
                    Mark::Text,
                    Mark::Muted,
                    Mark::Added,
                    Mark::Removed,
                    Mark::Link,
                ] {
                    let ratio = super::super::contrast(mark.color(p), ground);
                    assert!(ratio >= 3.0, "{theme:?} {mark:?} {ratio}");
                }
            }
            let ratio = super::super::contrast(p.text, bubble(p));
            assert!(ratio >= 4.5, "{theme:?} the user's words: {ratio}");
        }
    }
}
