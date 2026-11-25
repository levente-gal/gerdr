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
use super::features::{Font, small_line};
use super::glyphs::Face;
use super::repos::{DIFF_BAR, DIFF_INSET, DIFF_TINT};
use super::scrollback::{StyledRow, styled_row};
use super::{
    BUTTON_RADIUS, BUTTON_WIDTH, ButtonStyle, CHECKBOX_GAP, CHECKBOX_SIZE, CHEVRON_WIDTH, Canvas,
    Check, DOT_SLOT, Dot, FIELD_GAP, FieldSpot, ListBar, Painter, Palette, ROW_INSET, ROW_RADIUS,
    Rect, Rgb, Thumb, arrow, band, button_width, centred_dot, check, chevron_down, chevron_side,
    field_text, font_size, gutter_track, hint_at, hint_spots, hints_height, icon_sdf, mix,
    pane_bar_height, phys, physical, quit_button, rect_px, row_button, small_font_size, soft_rows,
    status_dot, suggestion_row, wrap_field_height,
};
use crate::{
    ChatRows,
    chat::{
        ChatFocus, DiffSide, Item, Reading, ToolOut, btw,
        commands::Slash,
        live::{AskReply, PendingAsk, Question, Running},
        menu::{self, Menu},
        questions::CardView,
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
    /// What its agent suggests saying next: the empty reply box shows it
    /// in place of its placeholder.
    pub suggestion: Option<String>,
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
    /// When its agent's turn began (Unix ms), while it works: what the
    /// clocks of the head and the working row count from.
    pub since: Option<u64>,
    /// The `/` menu over the reply box, while it shows.
    pub slash: Option<SlashView>,
    /// The modes its agent offers (`chat::mode::offered`), under the reply
    /// box; none for an agent without.
    pub modes: Vec<String>,
    /// The one its screen last said it is in.
    pub mode: Option<String>,
    /// The user's picks on the card of the ask's questions, while it asks
    /// some.
    pub card: Option<CardView>,
}

/// The `/` menu's rows in sight, as painted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashView {
    /// Each command's `/name` with what it takes, and what it does.
    pub rows: Vec<(String, String)>,
    /// Where the first row is among the commands matching.
    pub first: usize,
    /// The row picked, among `rows`.
    pub picked: usize,
}

impl SlashView {
    pub fn of(slash: &Slash) -> Self {
        let (first, shown) = slash.in_sight();
        let rows = (shown.iter().map(|i| &slash.all[*i]))
            .map(|c| {
                let name = match c.hint.is_empty() {
                    true => format!("/{}", c.name),
                    false => format!("/{} {}", c.name, c.hint),
                };
                (name, c.about.clone())
            })
            .collect();
        Self {
            rows,
            first,
            picked: slash.picked - first,
        }
    }
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
            && self.suggestion == other.suggestion
            && self.hints == other.hints
            && self.earlier == other.earlier
            && self.hover == other.hover
            && self.running == other.running
            && self.ask == other.ask
            && self.since == other.since
            && self.slash == other.slash
            && self.modes == other.modes
            && self.mode == other.mode
            && self.card == other.card
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
    /// A command of the `/` menu, by its place among those matching.
    Command(usize),
    /// The button at the list's foot while it is scrolled up: goes to the
    /// last item and follows new ones.
    Bottom,
    /// The bar at the list's top while it is scrolled up past the user's
    /// words: row `n`, the last of them. Goes to it.
    Pinned(usize),
    /// One of the agent's modes under the reply box, by its place in
    /// `ChatView::modes`: switches the agent to it.
    Mode(usize),
    /// A part of the card of the ask's questions.
    Card(CardHit),
}

/// What is under the pointer on the card of an ask's questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardHit {
    /// A question's tab: shows it.
    Tab(usize),
    /// An option's row, Other's last: picks it.
    Row(usize),
    /// Other's field: its words.
    Other,
    /// Next, or Submit on the last question.
    Go,
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
        let parts = parts(view);
        // The menu lies over the list and the waiting panel.
        if let (Some(slash), Some(i)) = (&view.slash, parts.slash.iter().position(|r| on(*r))) {
            return Some(ChatHit::Command(slash.first + parts.slash_skip + i));
        }
        if on(self.send) {
            return Some(ChatHit::Send);
        }
        if on(self.composer) {
            return Some(ChatHit::Composer);
        }
        if let Some((i, _)) = parts.modes.iter().find(|(_, rect)| on(*rect)) {
            return Some(ChatHit::Mode(*i));
        }
        if let Some((key, _)) = parts.keys.iter().find(|(_, rect)| on(*rect)) {
            return Some(ChatHit::Key(*key));
        }
        if let Some((hit, _)) = parts.answers.iter().find(|(_, rect)| on(*rect)) {
            return Some(*hit);
        }
        if let Some((at, laid)) = &parts.card
            && let Some((_, spot)) =
                (laid.spots.iter()).find(|(r, _)| on(Rectangle::new(r.loc + *at, r.size)))
        {
            return Some(match spot {
                Spot::Card(hit) => ChatHit::Card(*hit),
                _ => ChatHit::AskFold,
            });
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
        if bottom_button(view, &parts, content).is_some_and(on) {
            return Some(ChatHit::Bottom);
        }
        if let Some((i, ..)) = pinned(view, &parts, &items, content).filter(|(_, bar, _)| on(*bar)) {
            return Some(ChatHit::Pinned(i));
        }
        let top = top_of(view, content, self.list.size.h);
        if thumb_of(&parts, content, top).is_some_and(|thumb| thumb.takes(pos)) {
            return Some(ChatHit::Scrollbar);
        }
        let y = (pos.y - self.list.loc.y as f64).floor() as i32 + top;
        let x = (pos.x - parts.column.0 as f64).floor() as i32;
        if view.earlier.is_some() && (PART_GAP..PART_GAP + card_row()).contains(&y) {
            return (0..parts.column.1).contains(&x).then_some(ChatHit::Earlier);
        }
        let i = items.partition_point(|(t, laid)| t + laid.h <= y);
        // The running rows and the working row at the end aren't the
        // transcript's: nothing to pick or fold there.
        if i >= items.len() - view.running.len() - usize::from(works(view)) {
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
            Some(Spot::Card(hit)) => ChatHit::Card(hit),
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

/// The field with the keys, for the pointer and the caret (checklist O6):
/// the reply box, or the Other field of the questions' card while the
/// card's keys are on it.
pub fn chat_field_spot(view: &ChatView, scale: f64) -> Option<FieldSpot> {
    let parts = parts(view);
    let (area, line) = match &view.card {
        _ if view.focus == ChatFocus::Composer => (parts.composer, view.draft.clone()),
        Some(card) if on_other(view, card) => {
            let (at, laid) = parts.card.as_ref()?;
            let (r, _) = (laid.spots.iter()).find(|(_, s)| *s == Spot::Card(CardHit::Other))?;
            (Rectangle::new(r.loc + *at, r.size), card.other.clone())
        }
        _ => return None,
    };
    (area.size.w > 0 && area.size.h > 0).then(|| FieldSpot {
        area,
        text: field_text(area, scale),
        size: font_size(),
        line,
    })
}

/// The questions' card has the keys, on its Other row: what they type is
/// its words.
fn on_other(view: &ChatView, card: &CardView) -> bool {
    view.focus == ChatFocus::List
        && (view.ask.as_ref())
            .and_then(|ask| ask.questions.get(card.at))
            .is_some_and(|q| card.row == q.options.len())
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
    if !works(view) {
        return Vec::new();
    }
    let parts = parts(view);
    let running = in_sight(view, &parts, &Bit::Running)
        .into_iter()
        .map(centred_dot);
    std::iter::once(centred_dot(head_slot(parts.head)))
        .chain(running.filter(|dot| parts.list.contains_rect(*dot)))
        .collect()
}

/// Where the spinner of the working row turns (output coordinates), while
/// its agent works and the row is in view.
pub fn chat_spinners(view: &ChatView) -> Vec<Rect> {
    if !works(view) {
        return Vec::new();
    }
    let parts = parts(view);
    let side = super::features::RUN_SPIN;
    (in_sight(view, &parts, &Bit::Spinner).into_iter())
        .map(|slot| {
            let at = slot.loc + Point::from(((slot.size.w - side) / 2, (slot.size.h - side) / 2));
            Rectangle::new(at, (side, side).into())
        })
        .filter(|spin| parts.list.contains_rect(*spin))
        .collect()
}

/// The clocks telling how long its agent has worked this turn at `now`
/// (Unix ms), with where they go (output coordinates) and their font size:
/// the head's, and the working row's while it is in view.
pub fn chat_clocks(view: &ChatView, now: u64) -> Vec<(String, Rect, f32)> {
    let Some(since) = view.since.filter(|_| works(view)) else {
        return Vec::new();
    };
    let text = crate::clock::took(std::time::Duration::from_millis(now.saturating_sub(since)));
    let parts = parts(view);
    let fit = |slot: Rect, size: f32| {
        let w = (width_in(&text, Face::Ui, size).ceil() as i32).min(slot.size.w);
        (
            text.clone(),
            Rectangle::new(slot.loc, (w, slot.size.h).into()),
            size,
        )
    };
    let head = head_clock(view, parts.head).map(|slot| fit(slot, small_font_size()));
    let rows = (in_sight(view, &parts, &Bit::Clock).into_iter())
        .filter(|slot| parts.list.contains_rect(*slot))
        .map(|slot| fit(slot, font_size()));
    head.into_iter().chain(rows).collect()
}

/// Where the line sweeping under the head goes while its agent works
/// (output coordinates): along the head's bottom edge.
pub fn chat_sweep(view: &ChatView) -> Option<Rect> {
    let head = parts(view).head;
    let h = super::features::SWEEP_H;
    works(view).then(|| rect(head.loc.x, head.loc.y + head.size.h - h, head.size.w, h))
}

/// Where the rows in sight of `view`'s list hold `bit` (output
/// coordinates); none while the list shows a message instead.
fn in_sight(view: &ChatView, parts: &Parts, bit: &Bit) -> Vec<Rect> {
    if message(view).is_some() {
        return Vec::new();
    }
    let list = parts.list;
    let (items, content) = stacked(view, parts.column.1);
    let top = top_of(view, content, list.size.h);
    let mut found = Vec::new();
    for (y, laid) in &items[in_view(&items, top, list.size.h)] {
        let at = Point::from((parts.column.0, list.loc.y + y - top));
        found.extend(
            (laid.bits.iter())
                .filter(|(_, b)| b == bit)
                .map(|(rect, _)| Rectangle::new(rect.loc + at, rect.size)),
        );
    }
    found
}

/// Whether its agent works: neither paused nor asking (what the head's dot
/// blinking says).
fn works(view: &ChatView) -> bool {
    standing(view).0.blinks()
}

/// The longest a clock reads (`clock::took`), every digit its widest: what
/// its place is laid out for, so the time it tells never changes the chat.
const CLOCK_WIDEST: &str = "88m 88s";
/// Between "working" and its clock: a space.
const CLOCK_GAP: i32 = 4;

/// Where the head's clock goes while its agent works, at the head's right
/// end after "working".
fn head_clock(view: &ChatView, head: Rect) -> Option<Rect> {
    view.since.filter(|_| works(view))?;
    let w = width_in(CLOCK_WIDEST, Face::Ui, small_font_size()).ceil() as i32;
    Some(rect(
        head.loc.x + head.size.w - PAD - w,
        head.loc.y,
        w,
        head.size.h,
    ))
}

/// Room in from the chat's sides, and between its parts.
const PAD: i32 = 12;
const PART_GAP: i32 = 8;
/// Between the `/` menu's edge and its rows.
const SLASH_BORDER: i32 = 2;
/// The head line.
fn head_px() -> i32 {
    pane_bar_height()
}
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
/// How many lines of a plan show while its card is folded.
const PLAN_PEEK: usize = 12;
/// A card's head row: its fold mark, name, gist and state.
fn card_row() -> i32 {
    row_button()
}
const CARD_GAP: i32 = 8;
/// How far an option's description is pulled up into its label's row.
const NOTE_PULL: i32 = 6;
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
/// How far a mode's segment sits in from its track's edge.
const MODE_INSET: i32 = 2;
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

/// The questions' card's buttons, as they say them: Next, or Submit on the
/// last question, then the one giving the questions back to the agent's own
/// menu (the footer hints take their words: checklist K3).
pub fn question_buttons(last: bool) -> [(ChatHit, &'static str); 2] {
    let go = if last { "Submit" } else { "Next" };
    [
        (ChatHit::Card(CardHit::Go), go),
        (ChatHit::Answer(AskReply::Pass), "Answer in terminal"),
    ]
}

/// The buttons under `view`'s ask: the questions' card's, else Allow,
/// Deny and Show screen.
fn ask_buttons(view: &ChatView) -> Vec<(ChatHit, &'static str)> {
    match (&view.card, &view.ask) {
        (Some(card), Some(ask)) => question_buttons(card.at + 1 >= ask.questions.len()).to_vec(),
        _ => ASK_BUTTONS.to_vec(),
    }
}

/// What the questions' card's Other field says while it is empty.
const OTHER_HINT: &str = "Other…";

/// What `unfolded` holds for ask `ask` while its card shows what it would
/// run.
pub fn ask_key(ask: &str) -> String {
    format!("{ask}\u{0}ask")
}

/// Whether `ask` is plan mode asking to go ahead with its plan: its input
/// is the plan's markdown.
pub fn is_plan(ask: &PendingAsk) -> bool {
    ask.tool == "ExitPlanMode"
}

/// Whether `ask`'s card shows what it would run, as `unfolded` has it: a
/// plan shows until folded, as it is what the answer is about; other input
/// once unfolded.
pub fn ask_open(ask: &PendingAsk, unfolded: &HashSet<String>) -> bool {
    unfolded.contains(&ask_key(&ask.ask)) != is_plan(ask)
}

/// Plan `md` as blocks, parsed once while it stays the same (the ask's
/// card is laid out on every paint).
fn plan_blocks(md: &str) -> Arc<Vec<DocBlock>> {
    thread_local! {
        static KEPT: RefCell<Option<(String, Arc<Vec<DocBlock>>)>> = const { RefCell::new(None) };
    }
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        match kept.as_ref().filter(|(said, _)| said == md) {
            Some((_, blocks)) => blocks.clone(),
            None => {
                let blocks = Arc::new(crate::md_blocks::blocks(md));
                *kept = Some((md.to_owned(), blocks.clone()));
                blocks
            }
        }
    })
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
    /// The `/` menu's rows, hanging over the reply box, and how many of
    /// its first rows don't fit above them (the picked one always shows).
    slash: Vec<Rect>,
    slash_skip: usize,
    /// The agent's modes under the reply box: their track, and each one
    /// that fits, by its place in `ChatView::modes`.
    mode_track: Option<Rect>,
    modes: Vec<(usize, Rect)>,
}

/// How wide a key button labelled `label` is.
fn key_width(label: &str) -> i32 {
    (width_in(label, Face::Ui, font_size()).ceil() as i32 + 2 * KEY_PAD).max(BUTTON_WIDTH)
}

/// The screen rows the waiting panel shows of `rows`: up to the last
/// one with something on it, the last `WAIT_ROWS` of those.
fn screen_rows<T>(rows: &[(String, T)]) -> &[(String, T)] {
    let end = (rows.iter())
        .rposition(|(text, _)| !text.trim().is_empty())
        .map_or(0, |i| i + 1);
    &rows[end.saturating_sub(WAIT_ROWS)..end]
}

/// From the bottom up: the hints, the agent's modes, the reply box with
/// Send at its right, the waiting panel while the agent waits (as many screen rows as leave
/// the list three card rows), and the list from under the head line.
fn parts(view: &ChatView) -> Parts {
    let r = view.rect;
    let full = (r.size.w - 2 * PAD).max(0);
    let w = full.min(CHAT_MAX_W);
    let x = r.loc.x + PAD + (full - w) / 2;
    let bottom = r.loc.y + r.size.h;
    let head = Rectangle::new(r.loc, (r.size.w, head_px().min(r.size.h)).into());
    let hints = Rectangle::new(
        (r.loc.x, bottom - hints_height()).into(),
        (r.size.w, hints_height()).into(),
    );
    let send_w = button_width("Send");
    let field_w = (w - send_w - FIELD_GAP).max(0);
    let field_h = wrap_field_height(&view.draft, field_w);
    // As many of the modes as fit across, on a track as high as a button.
    let (mut mode_track, mut modes, mut modes_h) = (None, Vec::new(), 0);
    if !view.modes.is_empty() {
        let y = hints.loc.y - PART_GAP / 2 - row_button();
        let mut sx = x + MODE_INSET;
        for (i, mode) in view.modes.iter().enumerate() {
            let sw = width_in(mode, Face::Ui, small_font_size()).ceil() as i32 + 2 * KEY_PAD;
            if sx + sw + MODE_INSET > x + w {
                break;
            }
            let h = row_button() - 2 * MODE_INSET;
            modes.push((i, rect(sx, y + MODE_INSET, sw, h)));
            sx += sw;
        }
        mode_track = Some(rect(x, y, sx + MODE_INSET - x, row_button()));
        modes_h = row_button() + PART_GAP / 2;
    }
    let composer = Rectangle::new(
        (x, hints.loc.y - PART_GAP - modes_h - field_h).into(),
        (field_w, field_h).into(),
    );
    let send = Rectangle::new(
        (
            x + field_w + FIELD_GAP,
            composer.loc.y + field_h - quit_button(),
        )
            .into(),
        (send_w, quit_button()).into(),
    );
    let list_top = head.loc.y + head.size.h;
    let mut end = composer.loc.y - PART_GAP;
    let (mut waiting, mut menu, mut rows, mut keys) = (None, None, Rect::default(), Vec::new());
    let (mut card, mut answers) = (None, Vec::new());
    let frame = 3 * PART_GAP + row_button();
    let inner = (w - 2 * PART_GAP).max(0);
    if let Some(ask) = &view.ask {
        // As many lines of what it would run as leave the list three card
        // rows; the options' descriptions go when they don't fit.
        let mut laid = match &view.card {
            Some(card) => lay_question(view, &ask.questions, card, inner, true),
            None => lay_ask(view, ask, inner, 0),
        };
        let room = (end - list_top - 3 * card_row() - frame).max(0);
        if let Some(card) = view.card.as_ref().filter(|_| laid.h > room) {
            laid = lay_question(view, &ask.questions, card, inner, false);
        } else if view.card.is_none() && ask_open(ask, &view.unfolded) {
            let room = (end - list_top - 3 * card_row() - frame - laid.h - PART_GAP).max(0);
            let lines = (room / small_line()) as usize;
            laid = lay_ask(view, ask, inner, lines.min(ASK_LINES));
        }
        let h = frame + laid.h;
        let panel = Rectangle::new((x, end - h).into(), (w, h).into());
        let at = Point::from((x + PART_GAP, panel.loc.y + PART_GAP));
        let (mut bx, by) = (at.x, at.y + laid.h + PART_GAP);
        for (hit, label) in ask_buttons(view) {
            let bw = button_width(label);

            if bx + bw > at.x + inner {
                break;
            }
            answers.push((hit, rect(bx, by, bw, row_button())));
            bx += bw + PART_GAP / 2;
        }
        card = Some((at, laid));
        waiting = Some(panel);
        end = panel.loc.y - PART_GAP;
    } else if let Some(screen) = &view.waiting {
        let room = (end - list_top - 3 * card_row() - frame - PART_GAP).max(0);
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
                let n = (screen_rows(screen).len() as i32).min(room / styled_row());
                (n * styled_row(), WAIT_KEYS.to_vec())
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
                Rectangle::new((kx, ky).into(), (kw, row_button()).into()),
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
    // As many rows as fit between the head and the reply box.
    let n = view.slash.as_ref().map_or(0, |s| s.rows.len()) as i32;
    let n = n.min((composer.loc.y - PART_GAP / 2 - list_top - 2 * SLASH_BORDER) / suggestion_row());
    let n = n.max(0);
    let slash_skip = view
        .slash
        .as_ref()
        .map_or(0, |s| (s.picked + 1).saturating_sub(n as usize));
    let slash = (0..n)
        .map(|i| {
            let y = composer.loc.y - PART_GAP / 2 - SLASH_BORDER - (n - i) * suggestion_row();
            rect(x, y, w, suggestion_row())
        })
        .collect();
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
        slash,
        slash_skip,
        mode_track,
        modes,
    }
}

/// The menu at the bottom of `screen` laid out `w` wide, if it has one
/// and it fits `room` high (without what its choices say more, if need
/// be), and whether it has tabs; else the panel shows the screen rows.
/// Laid out once while they stay.
fn menu_laid(screen: &Arc<ChatRows>, w: i32, room: i32) -> Option<(Rc<Laid>, bool)> {
    type Kept = (Arc<ChatRows>, i32, i32, Option<(Rc<Laid>, bool)>);
    thread_local! {
        static MENU: RefCell<(u32, Option<Kept>)> = const { RefCell::new((0, None)) };
    }
    let kept = MENU.with_borrow_mut(|memo| {
        (super::sized(memo).as_ref())
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
    MENU.with_borrow_mut(|memo| {
        *super::sized(memo) = Some((screen.clone(), w, room, laid.clone()))
    });
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
        let tabs = m.tabs.iter().map(|t| (t.label.as_str(), t.done, t.current));
        lay_tabs(&mut l, tabs, w);
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
        // A check box's choice: the box, then its label.
        let (box_w, tw) = match choice.checked {
            Some(_) => (
                CHECKBOX_SIZE + CHECKBOX_GAP,
                tw - CHECKBOX_SIZE - CHECKBOX_GAP,
            ),
            None => (0, tw),
        };
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
        let label_x = ROW_INSET + CHOICE_NUM + box_w;
        let mut bits = vec![
            (rect(ROW_INSET, y, CHOICE_NUM, card_row()), num),
            (rect(label_x, y, tw - ROW_INSET, card_row()), label),
        ];
        if let Some(on) = choice.checked {
            let at = rect(ROW_INSET + CHOICE_NUM, y, CHECKBOX_SIZE, card_row());
            bits.push((at, Bit::Checkbox(on)));
        }
        y += card_row();
        if more && !choice.detail.is_empty() {
            let note = DocBlock::Note(choice.detail.clone());
            let (pieces, h) = doc_layout::stack_of(&[note], tw - 2 * ROW_INSET);
            let at = Point::from((label_x, y));
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

/// Tab chips `tabs` (label, answered, current) in a row at the top of `l`,
/// `w` wide, as many as fit: the current one on its own ground and bold,
/// an answered one checked. Where each chip went.
fn lay_tabs<'a>(
    l: &mut Laid,
    tabs: impl Iterator<Item = (&'a str, bool, bool)>,
    w: i32,
) -> Vec<Rect> {
    let size = Font::Body.size();
    let mut chips = Vec::new();
    let mut x = 0;
    for (text, done, current) in tabs {
        let face = if current { Face::Bold } else { Face::Ui };
        let mark_w = if done { TAB_CHECK + PART_GAP / 2 } else { 0 };
        let text_w = width_in(text, face, size).ceil() as i32;
        let tw = mark_w + text_w + 2 * BUBBLE_PAD;
        if x + tw > w {
            break;
        }
        let chip = rect(x, 0, tw, card_row());
        if current {
            l.put(chip, Bit::Picked);
        }
        if done {
            let y = (card_row() - TAB_CHECK) / 2;
            l.put(rect(x + BUBBLE_PAD, y, TAB_CHECK, TAB_CHECK), Bit::Check);
        }
        let mark = if current { Mark::Text } else { Mark::Muted };
        let bit = Bit::Label {
            text: text.to_owned(),
            face,
            size,
            mark,
            centred: false,
        };
        l.put(rect(x + BUBBLE_PAD + mark_w, 0, text_w, card_row()), bit);
        chips.push(chip);
        x += tw + PART_GAP / 2;
    }
    chips
}

/// The most characters of a question a tab without a header shows.
const TAB_CHARS: usize = 24;

/// What question `q`'s tab says: its header, else its first words.
fn tab_name(q: &Question) -> String {
    match q.header.trim() {
        "" if q.question.chars().count() > TAB_CHARS => {
            let cut: String = q.question.chars().take(TAB_CHARS - 1).collect();
            format!("{}…", cut.trim_end())
        }
        "" => q.question.clone(),
        header => header.to_owned(),
    }
}

/// The card of the questions `qs` laid out from `(0, 0)` for `w` wide, as
/// `card` has them, one at a time: their tabs (with more than one), the
/// question shown, a row per option (a radio, or a check box where several
/// may be picked, the label and, when `more`, its description), and last
/// Other, a field for the user's own words. The row with the keys is lit
/// while the card has them.
fn lay_question(view: &ChatView, qs: &[Question], card: &CardView, w: i32, more: bool) -> Laid {
    let mut l = Laid::default();
    let Some(q) = qs.get(card.at) else {
        return l;
    };
    let mut y = 0;
    if qs.len() > 1 {
        let names: Vec<String> = qs.iter().map(tab_name).collect();
        let tabs = (names.iter().enumerate())
            .map(|(i, name)| (name.as_str(), card.answered[i], i == card.at));
        for (i, chip) in lay_tabs(&mut l, tabs, w).into_iter().enumerate() {
            l.spots.push((chip, Spot::Card(CardHit::Tab(i))));
        }
        y = l.h + PART_GAP;
    }
    let block = DocBlock::Heading {
        level: 2,
        text: q.question.as_str().into(),
    };
    let (pieces, h) = doc_layout::stack_of(&[block], w);
    l.doc(pieces, (0, y));
    y += h + PART_GAP;
    let keys = view.focused && view.focus == ChatFocus::List;
    let mark = |on: bool| match q.multi {
        true => Bit::Checkbox(on),
        false => Bit::Radio(on),
    };
    let tx = ROW_INSET + CHECKBOX_SIZE + CHECKBOX_GAP;
    let tw = (w - tx - ROW_INSET).max(1);
    let other = q.options.len();
    for i in 0..=other {
        let option = q.options.get(i);
        let top = y;
        let row_h = match i == other {
            true => wrap_field_height(&card.other, tw).max(card_row()),
            false => card_row(),
        };
        // By the first line of Other's words, as they wrap.
        let mark_h = row_h.min(wrap_field_height(&Line::default(), tw).max(card_row()));
        let mut bits = vec![(
            rect(ROW_INSET, y, CHECKBOX_SIZE, mark_h),
            mark(card.picked.contains(&i)),
        )];
        match option {
            Some(option) => {
                // Bold at the page's size: its description under it is a
                // page note.
                let text = Bit::Label {
                    text: option.label.clone(),
                    face: Face::Bold,
                    size: Font::Body.size(),
                    mark: Mark::Text,
                    centred: false,
                };
                bits.push((rect(tx, y, tw, row_h), text));
            }
            None => {
                let field = rect(tx, y, tw, row_h);
                let focused = keys && card.row == other;
                bits.push((field, Bit::Field(card.other.clone(), focused)));
                l.spots.push((field, Spot::Card(CardHit::Other)));
            }
        }
        let described = option.filter(|o| more && !o.description.is_empty());
        // A description sits up against its label, in the row's slack below it.
        y += row_h - if described.is_some() { NOTE_PULL } else { 0 };
        if let Some(option) = described {
            let note = DocBlock::Note(option.description.clone());
            let (pieces, h) = doc_layout::stack_of(&[note], tw);
            let at = Point::from((tx, y));
            bits.extend(
                (pieces.into_iter())
                    .map(|(r, piece)| (Rectangle::new(r.loc + at, r.size), Bit::Doc(piece))),
            );
            y += h + PART_GAP / 2;
        }
        let ground = rect(0, top, w, y - top);
        if keys && card.row == i {
            l.put(ground, Bit::Picked);
        }
        for (r, bit) in bits {
            l.put(r, bit);
        }
        // Other's field takes its own presses (put first, found first).
        l.spots.push((ground, Spot::Card(CardHit::Row(i))));
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
        AgentStatus::Working => WORKING,
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
        _ if !view.running.is_empty() || works(view) => None,
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
        PART_GAP * 2 + quit_button()
    } else {
        0
    };
    let mut y = list.loc.y + (list.size.h - n as i32 * small_line() - button) / 2;
    let mut lines = Vec::new();
    for (i, row) in rows.iter().take(n).enumerate() {
        // The last line shown takes the rest, cut short as it is painted.
        let end = if i + 1 == n { text.len() } else { row.end };
        let rect = Rectangle::new((list.loc.x + PAD, y).into(), (room, small_line()).into());
        lines.push((text[row.start..end].trim().to_owned(), rect));
        y += small_line();
    }
    let screen = screen.then(|| {
        let w = button_width("Show screen");
        let x = list.loc.x + (list.size.w - w) / 2;
        Rectangle::new((x, y + PART_GAP * 2).into(), (w, quit_button()).into())
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
    let working = works(view).then(|| (false, working_row()));
    for (yours, laid) in said.chain(running).chain(working) {
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
        Some(_) => card_row() + TURN_GAP,
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

/// The row at the list's end while its agent works: a spinner in the fold
/// marks' column, "working", and its clock's place (both composed).
fn working_row() -> Rc<Laid> {
    let mut l = Laid::default();
    l.put(rect(0, 0, CHEVRON_WIDTH, card_row()), Bit::Spinner);
    let word_w = width_in(WORKING, Face::Ui, font_size()).ceil() as i32;
    let x = CHEVRON_WIDTH;
    l.put(
        rect(x, 0, word_w, card_row()),
        label(WORKING, Face::Ui, Mark::Muted),
    );
    let clock_w = width_in(CLOCK_WIDEST, Face::Ui, font_size()).ceil() as i32;
    l.put(
        rect(x + word_w + CLOCK_GAP, 0, clock_w, card_row()),
        Bit::Clock,
    );
    Rc::new(l)
}

/// What the head and the working row say while the agent works.
const WORKING: &str = "working";

/// The ask's card laid out from `(0, 0)` for `w` wide: the tool and its
/// gist on a row that folds, under it unfolded the first `lines` of what it
/// would run, then what the agent waits for.
fn lay_ask(view: &ChatView, ask: &PendingAsk, w: i32, lines: usize) -> Laid {
    let mut l = Laid::default();
    let open = ask_open(ask, &view.unfolded);
    l.head(Head {
        x: 0,
        w,
        open,
        name: if is_plan(ask) { "Plan" } else { &ask.tool },
        bold: true,
        gist: &short(&ask.gist, view.cwd.as_deref()),
        tail: Vec::new(),
        running: false,
        path: false,
        mono: true,
    });
    if open && is_plan(ask) {
        // The plan as markdown, as much as `lines` small lines' room
        // holds: the whole of it is on its card in the chat.
        let (ix, iw) = (CHEVRON_WIDTH, (w - CHEVRON_WIDTH).max(1));
        let room = lines as i32 * small_line();
        let (mut pieces, mut h) = doc_layout::stack_of(&plan_blocks(&ask.input), iw);
        let mut fit: Vec<i32> = (pieces.iter())
            .filter(|(r, piece)| {
                matches!(piece, DocPiece::Text { .. } | DocPiece::CodeLine(_))
                    && r.loc.y + r.size.h <= room
            })
            .map(|(r, _)| r.loc.y)
            .collect();
        fit.dedup();
        let more = peek(&mut pieces, &mut h, fit.len());
        let top = l.below();
        l.doc(pieces, (ix, top));
        if more > 0 {
            let text = match more {
                1 => "1 more line in the chat".to_owned(),
                n => format!("{n} more lines in the chat"),
            };
            l.put(
                rect(ix, l.below(), iw, small_line()),
                label(&text, Face::Ui, Mark::Muted),
            );
        }
    } else if open {
        let (ix, iw) = (CHEVRON_WIDTH, (w - CHEVRON_WIDTH).max(1));
        let all: Vec<&str> = ask.input.lines().collect();
        let shown = all.len().min(lines);
        let mut y = l.below();
        for line in &all[..shown] {
            let bit = Bit::Label {
                text: line.replace('\t', "    "),
                face: Face::Mono,
                size: small_font_size(),
                mark: Mark::Text,
                centred: false,
            };
            l.put(rect(ix, y, iw, small_line()), bit);
            y += small_line();
        }
        if all.len() > shown {
            let text = format!("{} more lines", all.len() - shown);
            l.put(
                rect(ix, l.below(), iw, small_line()),
                label(&text, Face::Ui, Mark::Muted),
            );
        }
    }
    let words = format!(
        "{} waits for your answer · Show screen to answer in its own prompt",
        agent_name(&view.label)
    );
    l.put(
        rect(0, l.below(), w, small_line()),
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

/// The button that goes to the list's end, centred at its foot, while
/// more than a card row of it is below what shows.
fn bottom_button(view: &ChatView, parts: &Parts, content: i32) -> Option<Rect> {
    let list = parts.list;
    let below = content - list.size.h - top_of(view, content, list.size.h);
    let size = BUTTON_WIDTH;
    (below > card_row() && list.size.h > size + 2 * PART_GAP).then(|| {
        let x = parts.column.0 + (parts.column.1 - size) / 2;
        rect(x, list.loc.y + list.size.h - size - PART_GAP, size, size)
    })
}

/// The bar at the list's top while it is scrolled up from its end past
/// the user's words: the last of their rows above what shows, where the bar
/// goes, and its layout. A subagent's prompt isn't the user's.
fn pinned(
    view: &ChatView,
    parts: &Parts,
    items: &[(i32, Rc<Laid>)],
    content: i32,
) -> Option<(usize, Rect, Laid)> {
    let list = parts.list;
    let top = top_of(view, content, list.size.h);
    if content - list.size.h - top <= card_row() {
        return None;
    }
    let rows = rows::rows(&view.items, &view.unfolded);
    let mut said = (rows.iter().enumerate().take(items.len())).filter_map(|(i, row)| match row {
        Row::Item(j) => match &view.items[*j] {
            Item::User { text, .. } => Some((i, text)),
            _ => None,
        },
        _ => None,
    });
    // The last above what shows, and the one after it.
    let mut last = None;
    let next = said.find(|&(i, text)| {
        let above = items[i].0 < top;
        if above {
            last = Some((i, text));
        }
        !above
    });
    let (i, text) = last?;
    let laid = lay_pinned(text, parts.column.1);
    let bar = rect(parts.column.0, list.loc.y + PART_GAP / 2, parts.column.1, laid.h);
    // Over half the list, it would hide what it is there to help read; and
    // it would hide the user's next words starting under it.
    let under = next.is_some_and(|(n, _)| items[n].0 < top + bar.size.h + PART_GAP);
    (bar.size.h + PART_GAP <= list.size.h / 2 && !under).then_some((i, bar, laid))
}

/// Where `view`'s bar of the user's words at the list's top is, while it
/// shows.
pub fn chat_pinned(view: &ChatView) -> Option<Rect> {
    if message(view).is_some() {
        return None;
    }
    let parts = parts(view);
    let (items, content) = stacked(view, parts.column.1);
    pinned(view, &parts, &items, content).map(|(_, bar, _)| bar)
}

/// Where `view`'s button to the list's end is, while it shows.
pub fn chat_bottom_button(view: &ChatView) -> Option<Rect> {
    if message(view).is_some() {
        return None;
    }
    let parts = parts(view);
    let (_, content) = stacked(view, parts.column.1);
    bottom_button(view, &parts, content)
}

/// The agent's mode under `pos`, by its place in `ChatView::modes`, and
/// where its segment is.
pub fn chat_mode_at(view: &ChatView, pos: Point<f64, Logical>) -> Option<(usize, Rect)> {
    (parts(view).modes.into_iter()).find(|(_, rect)| rect.to_f64().contains(pos))
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
    /// A part of the questions' card.
    Card(CardHit),
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

/// A frame on the chat's ground at `(x, y, w, h)` px: the waiting panel's,
/// a side question's.
fn framed(c: &mut Canvas, (x, y, w, h): (f32, f32, f32, f32), scale: f64, p: &Palette) {
    let radius = ROW_RADIUS * scale as f32;
    let line = (scale as f32).round().max(1.0);
    c.rounded(x, y, w, h, radius, p.border_variant);
    let (x, y, w, h) = (x + line, y + line, w - 2.0 * line, h - 2.0 * line);
    c.rounded(x, y, w, h, radius, p.terminal_background);
}

/// How much of the words' colour tints the user's bubble.
const BUBBLE_TINT: f32 = 0.08;

/// A thing an item paints, in its place.
#[derive(Clone, Debug, PartialEq)]
enum Bit {
    /// The ground under the user's words.
    Bubble,
    /// A framed card's ground: a side question's, a plan's.
    Card,
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
    /// Where the working row's spinner turns and its clock ticks: composed.
    Spinner,
    Clock,
    /// The ground of the choice a menu's arrow is on, or of its current
    /// tab.
    Picked,
    /// The check of a question answered.
    Check,
    /// A radio button, on or not: one pick of several.
    Radio(bool),
    /// A check box, ticked or not.
    Checkbox(bool),
    /// A field for the user's own words, wrapping them as the reply box
    /// does; with the keys or not.
    Field(Line, bool),
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
        size: font_size(),
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

/// The tops of `pieces`' lines of words and code, top first.
fn line_tops(pieces: &[(Rect, DocPiece)]) -> Vec<i32> {
    let mut tops: Vec<i32> = (pieces.iter())
        .filter(|(_, piece)| matches!(piece, DocPiece::Text { .. } | DocPiece::CodeLine(_)))
        .map(|(r, _)| r.loc.y)
        .collect();
    tops.dedup();
    tops
}

/// `pieces` cut to their first `n` lines, `h` their height then: how many
/// lines were cut. A quote's bar or a code block's ground ends at the cut.
fn peek(pieces: &mut Vec<(Rect, DocPiece)>, h: &mut i32, n: usize) -> usize {
    let tops = line_tops(pieces);
    let Some(&cut) = tops.get(n) else {
        return 0;
    };
    pieces.retain(|(r, _)| r.loc.y < cut);
    for (r, _) in pieces.iter_mut() {
        r.size.h = r.size.h.min(cut - r.loc.y);
    }
    *h = (pieces.iter())
        .map(|(r, _)| r.loc.y + r.size.h)
        .max()
        .unwrap_or(0);
    tops.len() - n
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
        self.put(rect(x, 0, CHEVRON_WIDTH, card_row()), Bit::Fold(open));
        let face = if head.bold { Face::Bold } else { Face::Ui };
        let mark = if head.bold { Mark::Text } else { Mark::Muted };
        let wide = |text: &str, face| width_in(text, face, font_size()).ceil() as i32;
        let widths: Vec<i32> = head.tail.iter().map(|(t, _)| wide(t, Face::Ui)).collect();
        let dot = if head.running { DOT_SLOT } else { 0 };
        let tail_w = widths.iter().map(|w| w + CARD_GAP).sum::<i32>() + dot;
        let end = x + w;
        let tail_x = (end - tail_w).max(x + CHEVRON_WIDTH);
        let name_x = x + CHEVRON_WIDTH;
        let name_w = wide(head.name, face).min(tail_x - name_x);
        self.put(
            rect(name_x, 0, name_w, card_row()),
            label(head.name, face, mark),
        );
        let gist_x = name_x + name_w + CARD_GAP;
        let gist_w = tail_x - CARD_GAP - gist_x;
        if !head.gist.is_empty() && gist_w > 0 {
            let gist = head.gist.lines().next().unwrap_or_default();
            let mark = if head.path { Mark::Text } else { Mark::Muted };
            let (gface, gsize) = match head.mono {
                true => (Face::Mono, small_font_size()),
                false => (Face::Ui, font_size()),
            };
            let bit = Bit::Label {
                text: gist.to_owned(),
                face: gface,
                size: gsize,
                mark,
                centred: false,
            };
            self.put(rect(gist_x, 0, gist_w, card_row()), bit);
            if head.path {
                let shown = (width_in(gist, gface, gsize).ceil() as i32).min(gist_w);
                self.spots
                    .push((rect(gist_x, 0, shown, card_row()), Spot::Path));
            }
        }
        let mut tx = tail_x;
        if head.running {
            self.put(rect(tx, 0, DOT_SLOT, card_row()), Bit::Running);
            tx += DOT_SLOT;
        }
        for ((text, mark), w) in head.tail.into_iter().zip(widths) {
            let room = w.min(end - tx);
            self.put(
                rect(tx + CARD_GAP, 0, room, card_row()),
                label(&text, Face::Ui, mark),
            );
            tx += w + CARD_GAP;
        }
        self.spots.push((rect(x, 0, w, card_row()), Spot::Fold));
    }

    /// "Show all (N lines)" under what was cut to `CHAT_CUT` of `n` lines.
    fn more(&mut self, x: i32, w: i32, n: usize) {
        let row = rect(x, self.below(), w, small_line());
        let text = format!("Show all ({n} lines)");
        let shown = width_in(&text, Face::Ui, font_size()).ceil() as i32;
        self.put(row, label(&text, Face::Ui, Mark::Link));
        self.spots.push((
            rect(x, row.loc.y, shown.min(w), small_line()),
            Spot::ShowAll,
        ));
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

/// The user's words `text` on the bar at the list's top, `w` wide: their
/// first `PROMPT_PEEK` lines, an ellipsis ending them when there is more.
fn lay_pinned(text: &str, w: i32) -> Laid {
    let blocks: Vec<DocBlock> = paragraphs(text).map(|p| DocBlock::Para(p.into())).collect();
    let (mut pieces, mut h) = doc_layout::stack_of(&blocks, (w - 2 * BUBBLE_PAD).max(1));
    let more = peek(&mut pieces, &mut h, PROMPT_PEEK);
    if more > 0
        && let Some((_, DocPiece::Text { text, .. })) = (pieces.iter_mut())
            .rev()
            .find(|(_, piece)| matches!(piece, DocPiece::Text { .. }))
    {
        text.push('…');
    }
    let mut l = Laid::default();
    l.put(rect(0, 0, w, h + 2 * BUBBLE_PAD), Bit::Bubble);
    l.doc(pieces, (BUBBLE_PAD, BUBBLE_PAD));
    l
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
            let more = match nest > 0 && !open {
                true => peek(&mut pieces, &mut h, PROMPT_PEEK),
                false => 0,
            };
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
                let wide = width_in(&text, Face::Ui, font_size()).ceil() as i32;
                let row = rect(x + cw - wide, l.below(), wide, small_line());
                l.put(row, label(&text, Face::Ui, Mark::Link));
                l.spots.push((row, Spot::Fold));
            }
        }
        // A side question: its question and answer framed apart, as the
        // agent's chat won't keep them.
        Item::Assistant { id, blocks, .. } if btw::is_side(id) => {
            let (pieces, h) = doc_layout::stack_of(blocks, (cw - 2 * BUBBLE_PAD).max(1));
            l.put(rect(x, 0, cw, h + 2 * BUBBLE_PAD), Bit::Card);
            l.doc(pieces, (x + BUBBLE_PAD, BUBBLE_PAD));
        }
        Item::Assistant { blocks, .. } => {
            let (pieces, _) = doc_layout::stack_of(blocks, cw);
            l.doc(pieces, (x, 0));
        }
        Item::Plan {
            path,
            blocks,
            output,
            ..
        } => {
            let gist = path.as_deref().map(|p| short(p, cwd)).unwrap_or_default();
            l.head(Head {
                x,
                w: cw,
                open,
                name: "Plan",
                bold: true,
                gist: &gist,
                tail: outcome(output),
                running: output.is_none(),
                path: path.is_some(),
                mono: true,
            });
            // The plan on its card, as the agent's words read: its first
            // lines, whole once unfolded.
            let (mut pieces, mut h) = doc_layout::stack_of(blocks, (iw - 2 * BUBBLE_PAD).max(1));
            let lines = line_tops(&pieces).len();
            let more = match open {
                true => 0,
                false => peek(&mut pieces, &mut h, PLAN_PEEK),
            };
            let top = l.below();
            l.put(rect(ix, top, iw, h + 2 * BUBBLE_PAD), Bit::Card);
            l.doc(pieces, (ix + BUBBLE_PAD, top + BUBBLE_PAD));
            let link = match (more, open && lines > PLAN_PEEK) {
                (0, false) => None,
                (0, true) => Some("Show less".to_owned()),
                (1, _) => Some("Show whole plan (1 more line)".to_owned()),
                (n, _) => Some(format!("Show whole plan ({n} more lines)")),
            };
            if let Some(text) = link {
                let row = rect(ix, l.below(), iw, small_line());
                let wide = width_in(&text, Face::Ui, font_size()).ceil() as i32;
                l.put(row, label(&text, Face::Ui, Mark::Link));
                l.spots
                    .push((rect(ix, row.loc.y, wide.min(iw), small_line()), Spot::Fold));
            }
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
                l.put(rect(ix, y, iw, small_line()), Bit::Diff(line.side, text));
                y += small_line();
            }
            if open && diff.len() > cut {
                l.more(ix, iw, diff.len());
            } else if !open && diff.len() > EDIT_PEEK {
                let text = format!("{} more lines", diff.len() - EDIT_PEEK);
                let row = rect(ix, l.below(), iw, small_line());
                let wide = width_in(&text, Face::Ui, font_size()).ceil() as i32;
                l.put(row, label(&text, Face::Ui, Mark::Link));
                l.spots
                    .push((rect(ix, row.loc.y, wide.min(iw), small_line()), Spot::Fold));
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
                size: small_font_size(),
                mark: Mark::Muted,
                centred: true,
            };
            l.put(rect(0, 0, w, small_line()), bit);
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
    static MEMO: RefCell<(u32, HashMap<String, Memo>)> = RefCell::new((0, HashMap::new()));
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
    let kept = MEMO.with_borrow_mut(|memo| {
        let m = super::sized(memo).get(item.id())?;
        let same = m.w == w && m.nest == nest && m.open == open && m.cwd.as_deref() == cwd;
        (same && m.item == *item).then(|| m.laid.clone())
    });
    if let Some(laid) = kept {
        return laid;
    }
    let laid = Rc::new(lay(item, w, nest, unfolded, cwd));
    MEMO.with_borrow_mut(|memo| {
        let memo = super::sized(memo);
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
    static STEPS: RefCell<(u32, HashMap<String, StepsMemo>)> = RefCell::new((0, HashMap::new()));
}

/// The steps row from item `id` on saying `s`, its fold mark `open`, for
/// a column `w` wide: laid out once while they stay.
fn laid_steps(id: &str, s: Summary, open: bool, w: i32, nest: i32) -> Rc<Laid> {
    let kept = STEPS.with_borrow_mut(|memo| {
        let (was, o, ww, laid) = super::sized(memo).get(id)?;
        (*was == s && *o == open && *ww == w).then(|| laid.clone())
    });
    if let Some(laid) = kept {
        return laid;
    }
    let laid = lay_steps(&s, open, w, nest);
    STEPS.with_borrow_mut(|memo| {
        let memo = super::sized(memo);
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
        // While it works, its clock is composed after the word.
        let word_end = match head_clock(view, parts.head) {
            Some(clock) => px(clock.loc.x - r.loc.x - CLOCK_GAP) as f32,
            None => hend - pad,
        };
        let word_x = word_end - self.text_width(word, small_font_size(), scale);
        let b = band(head, scale);
        self.text(
            &mut c,
            word,
            small_font_size(),
            word_x,
            word_end,
            b,
            scale,
            p.muted,
        );
        let label_x = hx + px(PAD + DOT_SLOT + CARD_GAP / 2) as f32;
        let label_end = word_x - px(CARD_GAP) as f32;
        self.bold_text(
            &mut c,
            &view.label,
            font_size(),
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
                self.centered(&mut c, text, font_size(), here(*rect), scale, p.muted);
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
                        card_row(),
                    );
                    let hover = if view.hover == Some(ChatHit::Earlier) {
                        1.0
                    } else {
                        0.0
                    };
                    self.row_highlight(c, row, (false, hover), scale, p);
                    let (words, mark) = earlier_words(reading);
                    self.centered(c, words, font_size(), row, scale, mark.color(p));
                }
                for i in in_view(&items, top, list.size.h) {
                    let (y, laid) = &items[i];
                    let at = Point::from((parts.column.0 - r.loc.x, list.loc.y + y - top));
                    self.chat_item(c, view, (i, laid), at, parts.column.1, scale, p);
                }
            });
            if let Some((_, bar, laid)) = pinned(view, &parts, &items, content) {
                // A ground across the list, so no item shows beside it.
                let at = parts.list.loc;
                let strip = rect(at.x, at.y, parts.list.size.w, bar.size.h + PART_GAP);
                let (x, y, w, h) = rect_px(here(strip), scale);
                c.fill(x as i32, y as i32, w as i32, h as i32, p.terminal_background);
                rule(&mut c, strip.loc.y + strip.size.h - 1);
                let lit = matches!(view.hover, Some(ChatHit::Pinned(_)));
                for (rect, bit) in &laid.bits {
                    let rect = here(Rectangle::new(rect.loc + bar.loc, rect.size));
                    match bit {
                        Bit::Bubble if lit => {
                            let (x, y, w, h) = rect_px(rect, scale);
                            c.rounded(x, y, w, h, ROW_RADIUS * scale as f32, p.selected);
                        }
                        bit => self.chat_bit(&mut c, rect, bit, scale, p),
                    }
                }
            }
            if let Some(thumb) = thumb_of(&parts, content, top) {
                let thumb = Thumb {
                    track: here(thumb.track),
                    top: thumb.top - r.loc.y as f64,
                    ..thumb
                };
                self.thumb_in(&mut c, &thumb, scale, p);
            }
            if let Some(button) = bottom_button(view, &parts, content) {
                let lit = view.hover == Some(ChatHit::Bottom);
                let (x, y, w, h) = rect_px(here(button), scale);
                let b = (scale as f32).round().max(1.0);
                c.rounded(x, y, w, h, w / 2.0, p.border);
                let fill = if lit { p.selected } else { p.element };
                c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, w / 2.0 - b, fill);
                let s = 1.3 * scale as f32;
                icon_sdf(&mut c, here(button), scale, p.text, |x, y| {
                    arrow(false, x, y, s)
                });
            }
        }
        rule(&mut c, parts.list.loc.y + parts.list.size.h);

        // The waiting panel: the agent's screen rows and the keys to answer.
        if let Some(panel) = parts.waiting {
            framed(&mut c, rect_px(here(panel), scale), scale, p);
            if let Some((at, laid)) = &parts.card {
                let at = *at - r.loc;
                for (spot, what) in &laid.spots {
                    let row = Rectangle::new(spot.loc + at, spot.size);
                    match what {
                        Spot::Card(CardHit::Other) => {}
                        // The menu rows' hover: the bubble's tint.
                        Spot::Card(hit) if view.hover == Some(ChatHit::Card(*hit)) => {
                            let (x, y, w, h) = rect_px(row, scale);
                            c.rounded(x, y, w, h, ROW_RADIUS * scale as f32, bubble(p));
                        }
                        Spot::Fold if view.hover == Some(ChatHit::AskFold) => {
                            self.row_highlight(&mut c, row, (false, 1.0), scale, p);
                        }
                        _ => {}
                    }
                }
                for (rect, bit) in &laid.bits {
                    let rect = Rectangle::new(rect.loc + at, rect.size);
                    self.chat_bit(&mut c, rect, bit, scale, p);
                }
                let buttons = ask_buttons(view);
                let ready = view.card.as_ref().is_none_or(CardView::ready);
                for (hit, rect) in &parts.answers {
                    let label = (buttons.iter())
                        .find(|(h, _)| h == hit)
                        .map_or("", |(_, l)| l);
                    let style = match hit {
                        ChatHit::Answer(AskReply::Allow) => ButtonStyle::Primary,
                        // Submit waits for every answer.
                        ChatHit::Card(CardHit::Go) if label == "Submit" && !ready => {
                            ButtonStyle::Off
                        }
                        ChatHit::Card(CardHit::Go) => ButtonStyle::Primary,
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
                let n = (parts.rows.size.h / styled_row()) as usize;
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
        let hint = (view.suggestion.clone())
            .unwrap_or_else(|| format!("Reply to {}…", agent_name(&view.label)));
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

        if let Some(track) = parts.mode_track {
            let segments: Vec<(&str, Rect)> = (parts.modes.iter())
                .map(|(i, rect)| (view.modes[*i].as_str(), here(*rect)))
                .collect();
            let on = (view.mode.as_ref())
                .and_then(|now| parts.modes.iter().position(|(i, _)| view.modes[*i] == *now))
                .unwrap_or(usize::MAX);
            let hover = match view.hover {
                Some(ChatHit::Mode(m)) => parts.modes.iter().position(|(i, _)| *i == m),
                _ => None,
            };
            self.segmented(&mut c, here(track), &segments, on, hover, scale, p);
        }

        if let Some(slash) = &view.slash {
            let rows = (parts.slash.as_slice(), parts.slash_skip);
            self.chat_slash(&mut c, view, slash, rows, &here, scale, p);
        }

        let hints = chat_hints(view);
        let spots = hint_spots(&hints, here(parts.hints));
        self.hints(&mut c, &hints, &spots, None, scale, p);
        c.into_buffer(true)
    }

    /// The `/` menu in a box over the reply box, as the folder suggestions
    /// of the new project form: the picked row on the selected ground, the
    /// hovered one half way to it.
    #[allow(clippy::too_many_arguments)]
    fn chat_slash(
        &self,
        c: &mut Canvas,
        view: &ChatView,
        slash: &SlashView,
        (rows, skip): (&[Rect], usize),
        here: &dyn Fn(Rect) -> Rect,
        scale: f64,
        p: &Palette,
    ) {
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return;
        };
        let s = scale as f32;
        let edge = SLASH_BORDER;
        let frame = here(rect(
            first.loc.x - edge,
            first.loc.y - edge,
            first.size.w + 2 * edge,
            last.loc.y + last.size.h - first.loc.y + 2 * edge,
        ));
        let (x, y, w, h) = rect_px(frame, scale);
        let (b, r) = ((scale.round() as f32).max(1.0), BUTTON_RADIUS * s);
        c.rounded(x, y, w, h, r, p.border);
        c.rounded(x + b, y + b, w - 2.0 * b, h - 2.0 * b, r - b, p.element);
        let shown = slash.rows.iter().enumerate().skip(skip);
        for (row, (i, (name, about))) in rows.iter().zip(shown) {
            let row = here(*row);
            let (x, y, w, h) = rect_px(row, scale);
            if i == slash.picked {
                c.rounded(x, y, w, h, r - b, p.selected);
            } else if view.hover == Some(ChatHit::Command(slash.first + i)) {
                c.rounded(x, y, w, h, r - b, mix(p.element, p.selected, 0.5));
            }
            let (start, end) = phys(row, scale);
            let pad = (f64::from(PART_GAP) * scale) as f32;
            let band = band(row, scale);
            let size = small_font_size();
            let after = self.text(c, name, size, start + pad, end - pad, band, scale, p.text);
            if !about.is_empty() {
                let x = after + 2.0 * pad;
                self.text(c, about, size, x, end - pad, band, scale, p.placeholder);
            }
        }
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
            Bit::Card => framed(c, rect_px(rect, scale), scale, p),
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
            Bit::Running | Bit::Spinner | Bit::Clock => {}
            Bit::Picked => self.row_highlight(c, rect, (true, 0.0), scale, p),
            Bit::Check => icon_sdf(c, rect, scale, p.success, |x, y| check(x, y, s)),
            Bit::Radio(on) => _ = self.radio(c, rect, *on, scale, p),
            Bit::Checkbox(on) => {
                let state = if *on { Check::On } else { Check::Off };
                // Never lit itself: its row is (as the quit dialog's).
                self.checkbox(c, rect, state, false, scale, p);
            }
            Bit::Field(line, focused) => {
                self.wrap_field(c, line, OTHER_HINT, rect, *focused, scale, p);
            }
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
            suggestion: None,
            hints: crate::chat::footer(ChatFocus::List, true, false, false),
            earlier: None,
            hover: None,
            running: Vec::new(),
            ask: None,
            since: Some(1_000),
            slash: None,
            modes: Vec::new(),
            mode: None,
            card: None,
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

    /// Claude asking three questions on its card: a size (one pick, with
    /// descriptions), where (one pick), and fruits (several).
    pub(in crate::chrome) fn question_ask() -> PendingAsk {
        use crate::chat::live::QuestionOption;
        let q = |header: &str, text: &str, options: &[(&str, &str)], multi| Question {
            question: text.into(),
            header: header.into(),
            options: (options.iter())
                .map(|(label, description)| QuestionOption {
                    label: (*label).into(),
                    description: (*description).into(),
                })
                .collect(),
            multi,
        };
        PendingAsk {
            ask: "q3".into(),
            tool: "AskUserQuestion".into(),
            gist: String::new(),
            input: String::new(),
            questions: vec![
                q(
                    "Size",
                    "How should the UI grow? The switcher has rows for it already, keys are quicker.",
                    &[
                        (
                            "Switcher rows + keys (Recommended)",
                            "Rows 'Bigger UI', 'Smaller UI' and 'Reset UI size', in steps of 10%, plus Prefix keys.",
                        ),
                        ("Switcher rows only", "Only the three rows, no new keys."),
                        ("Keys only", ""),
                    ],
                    false,
                ),
                q(
                    "",
                    "Where should it be saved?",
                    &[("config.toml", ""), ("state.json", "")],
                    false,
                ),
                q(
                    "Fruits",
                    "Which fruits do you want? Pick all that apply.",
                    &[
                        ("Apple", "Crisp and sweet"),
                        ("Banana", "Soft"),
                        ("Grape", "In bunches"),
                    ],
                    true,
                ),
            ],
            since: std::time::Instant::now(),
        }
    }

    /// Claude Code asking a question of several picks, as herdr read the
    /// user's screen: an edge at the end of every row, its arrow on the
    /// user's own answer.
    pub(in crate::chrome) fn ticks() -> Arc<Vec<(String, crate::scrollback::Styled)>> {
        let rule = "─".repeat(50);
        let rows = [
            "←  ☒ Languages  ☐ Fruits  ☐ Places  ✔ Submit  →".to_owned(),
            String::new(),
            "\x1b[1mWhich languages do you write code in? Pick all that apply.\x1b[0m".to_owned(),
            String::new(),
            "  1. [ ] Rust".to_owned(),
            "     Systems code".to_owned(),
            "  2. [✔] Python".to_owned(),
            "     Scripts and tooling".to_owned(),
            "❯ 3. [ ] Type something".to_owned(),
            "     Next".to_owned(),
            rule,
            "  4. Chat about this".to_owned(),
            String::new(),
        ];
        let ansi: String = rows.iter().map(|row| format!("{row:<70}│\n")).collect();
        Arc::new(crate::scrollback::parse_ansi(&ansi))
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
        assert_eq!(p.rows.size.h, shown.len() as i32 * styled_row());
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
        assert!(p.list.size.h >= 3 * card_row(), "{:?}", p.list);
        assert!(p.rows.size.h < shown.len() as i32 * styled_row());
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
    fn the_slash_menu_hangs_over_the_reply_box_and_its_rows_take_presses() {
        let rows = (0..3).map(|i| (format!("/c{i}"), String::new())).collect();
        let v = ChatView {
            focus: ChatFocus::Composer,
            slash: Some(SlashView {
                rows,
                first: 5,
                picked: 0,
            }),
            ..view()
        };
        let p = parts(&v);
        assert_eq!(p.slash.len(), 3);
        let last = p.slash[2];
        assert!(last.loc.y + last.size.h <= p.composer.loc.y);
        let layout = chat_layout(&v);
        assert_eq!(
            layout.hit(&v, middle(p.slash[0])),
            Some(ChatHit::Command(5))
        );
        assert_eq!(layout.hit(&v, middle(last)), Some(ChatHit::Command(7)));
        assert_eq!(layout.hit(&v, middle(p.composer)), Some(ChatHit::Composer));
        // A pane too short for its rows shows those that fit.
        let short = ChatView {
            rect: Rect::new(v.rect.loc, (v.rect.size.w, 150).into()),
            ..v
        };
        let p = parts(&short);
        assert!(p.slash.len() < 3);
        assert_eq!(p.slash_skip, 0, "the picked first row shows");
        let short = ChatView {
            slash: Some(SlashView {
                picked: 2,
                ..short.slash.clone().unwrap()
            }),
            ..short
        };
        let p = parts(&short);
        assert_eq!(p.slash_skip + p.slash.len(), 3, "the picked last row shows");
        let last = *p.slash.last().unwrap();
        assert_eq!(
            chat_layout(&short).hit(&short, middle(last)),
            Some(ChatHit::Command(7))
        );
        assert!(
            p.slash
                .iter()
                .all(|r| r.loc.y >= p.head.loc.y + p.head.size.h)
        );
    }

    #[test]
    fn the_modes_sit_under_the_reply_box_as_many_as_fit() {
        let modes: Vec<String> = ["Agent", "Plan", "Debug", "Ask"].map(String::from).into();
        let v = ChatView {
            modes: modes.clone(),
            ..view()
        };
        let (layout, p) = (chat_layout(&v), parts(&v));
        assert_eq!(p.modes.len(), 4);
        let track = p.mode_track.unwrap();
        assert!(track.loc.y >= layout.composer.loc.y + layout.composer.size.h);
        assert!(track.loc.y + track.size.h <= p.hints.loc.y);
        for (i, rect) in &p.modes {
            assert_eq!(layout.hit(&v, middle(*rect)), Some(ChatHit::Mode(*i)));
            assert_eq!(chat_mode_at(&v, middle(*rect)).map(|(m, _)| m), Some(*i));
        }
        // No modes, no track; the reply box sits lower.
        let plain = parts(&view());
        assert!(plain.mode_track.is_none());
        assert!(plain.composer.loc.y > layout.composer.loc.y);
        // A narrow pane shows those that fit.
        let narrow = ChatView {
            rect: Rectangle::new((0, 0).into(), (200, 500).into()),
            modes,
            ..view()
        };
        let shown = parts(&narrow).modes.len();
        assert!((1..4).contains(&shown), "{shown}");
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
        let row = |r: Rect, x: i32| {
            Point::from(((r.loc.x + x) as f64, (r.loc.y + card_row() / 2) as f64))
        };
        assert_eq!(layout.hit(&v, row(steps, 4)), Some(ChatHit::Fold(2)));
        // An edit's path, right of its name, opens the file.
        let edit = item_rect(&v, 3);
        let name = width_in("Edit", Face::Bold, font_size()).ceil() as i32;
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
        // No working row under it.
        parts_v.status = AgentStatus::Idle;
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
        assert_eq!(laid.h, card_row());
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
    fn a_plan_shows_on_its_card_cut_until_unfolded() {
        let text: String = (0..20).map(|i| format!("- step {i}\n")).collect();
        let plan = Item::Plan {
            id: "plan".into(),
            path: Some("/h/.claude/plans/x.md".into()),
            blocks: Arc::new(crate::md_blocks::blocks(&text)),
            text,
            output: None,
        };
        let lines = |l: &Laid| {
            (l.bits.iter())
                .filter(|(_, b)| matches!(b, Bit::Doc(DocPiece::Text { .. })))
                .count()
        };
        let link = |l: &Laid| {
            (l.bits.iter()).find_map(|(_, b)| match b {
                Bit::Label {
                    text,
                    mark: Mark::Link,
                    ..
                } => Some(text.clone()),
                _ => None,
            })
        };
        let card = |l: &Laid| {
            (l.bits.iter())
                .find_map(|(r, b)| (*b == Bit::Card).then_some(*r))
                .expect("a card")
        };
        let folded = laid(&plan, 500, 0, &HashSet::new(), None);
        assert_eq!(lines(&folded), PLAN_PEEK);
        assert_eq!(
            link(&folded).as_deref(),
            Some("Show whole plan (8 more lines)")
        );
        // The head and the link fold it.
        assert_eq!(
            (folded.spots.iter())
                .filter(|(_, s)| *s == Spot::Fold)
                .count(),
            2
        );
        // Every line shown is on the card.
        let ground = card(&folded);
        assert!(
            (folded.bits.iter())
                .filter(|(_, b)| matches!(b, Bit::Doc(_)))
                .all(|(r, _)| r.loc.y + r.size.h <= ground.loc.y + ground.size.h)
        );
        let open = laid(&plan, 500, 0, &HashSet::from(["plan".to_owned()]), None);
        assert_eq!(lines(&open), 20);
        assert_eq!(link(&open).as_deref(), Some("Show less"));
        assert!(card(&open).size.h > ground.size.h);
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
        assert_eq!(first.loc.y - below, card_row() + TURN_GAP);
        let p = parts(&v);
        let row = Point::from((
            (p.column.0 + p.column.1 / 2) as f64,
            (layout.list.loc.y + PART_GAP + card_row() / 2) as f64,
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
        v.status = AgentStatus::Idle;
        let layout = chat_layout(&v);
        // The card, its words, its steps: three rows, the steps in from it.
        let steps = item_rect(&v, 2);
        let at = |x: i32| {
            Point::from((
                (steps.loc.x + x) as f64,
                (steps.loc.y + card_row() / 2) as f64,
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
        // Idle: no working row, which is laid out every time.
        let v = ChatView {
            status: AgentStatus::Idle,
            ..view()
        };
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
        // At the end, no button to go there; scrolled up, one at the foot.
        assert_eq!(bottom_button(&v, &p, content), None);
        v.top = 0.0;
        let button = bottom_button(&v, &p, content).expect("a button to the end");
        assert_eq!(
            button.loc.y + button.size.h + PART_GAP,
            p.list.loc.y + p.list.size.h
        );
        assert_eq!(
            chat_layout(&v).hit(&v, middle(button)),
            Some(ChatHit::Bottom)
        );
        assert_eq!(chat_bottom_button(&v), Some(button), "its tooltip's place");
    }

    #[test]
    fn the_last_words_scrolled_past_stay_on_top() {
        let mut v = view();
        let answer = |i: usize| Item::Assistant {
            id: format!("a{i}"),
            at: None,
            blocks: Arc::new(vec![DocBlock::Para("an answer".into())]),
        };
        let said = |id: &str, text: String| Item::User {
            id: id.into(),
            at: None,
            text,
        };
        let long = (0..12).map(|i| format!("line {i}")).collect::<Vec<_>>();
        let mut all = vec![said("u0", long.join("\n\n"))];
        all.extend((0..80).map(answer));
        all.push(said("u1", "the last".into()));
        all.extend((100..140).map(answer));
        v.items = Arc::new(all);
        let p = parts(&v);
        let (items, content) = stacked(&v, p.column.1);
        // At the top and at the end, none.
        assert!(pinned(&v, &p, &items, content).is_none());
        v.top = f64::MAX;
        assert!(pinned(&v, &p, &items, content).is_none());
        // Among the answers to the first words: their first three lines.
        v.top = f64::from(items[40].0);
        let (i, bar, laid) = pinned(&v, &p, &items, content).expect("a bar");
        assert_eq!(i, 0);
        assert_eq!(bar.loc.y, p.list.loc.y + PART_GAP / 2);
        let texts: Vec<&str> = (laid.bits.iter())
            .filter_map(|(_, bit)| match bit {
                Bit::Doc(DocPiece::Text { text, .. }) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["line 0", "line 1", "line 2…"]);
        assert_eq!(
            chat_layout(&v).hit(&v, middle(bar)),
            Some(ChatHit::Pinned(0))
        );
        assert_eq!(chat_pinned(&v), Some(bar), "its tooltip's place");
        // Past the last words, those.
        v.top = f64::from(items[82].0);
        assert_eq!(pinned(&v, &p, &items, content).map(|(i, ..)| i), Some(81));
        // Gone to the last words: they show, uncovered.
        v.top = f64::from(items[81].0);
        assert!(pinned(&v, &p, &items, content).is_none());
    }

    #[test]
    fn an_empty_or_missing_chat_says_so() {
        let mut v = view();
        v.items = Arc::new(Vec::new());
        v.reading = Reading::Finding;
        // While it works, its working row shows instead.
        assert_eq!(message(&v), None);
        v.status = AgentStatus::Idle;
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
        v.hints = crate::chat::footer(ChatFocus::Composer, true, false, false);
        assert_eq!(chat_hints(&v)[2], ("Esc", "back"));
        assert!(!keys(&v).iter().any(|k| k == crate::text_field::DISCARD_KEY));
        v.draft.text = "half a reply".into();
        v.hints = crate::chat::footer(ChatFocus::Composer, false, false, false);
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
        // Above the working row, the list's last.
        let ran = before - 1;
        let last = item_rect(&v, ran);
        assert!(rows[ran].1.bits.iter().any(|(_, b)| *b == Bit::Running));
        // Its dot blinks while the agent works; a press there picks nothing.
        assert!(chat_blinkers(&v).iter().any(|dot| inside(last, *dot)));
        assert_eq!(chat_layout(&v).hit(&v, middle(last)), None);
        // An empty chat shows it, not "Nothing said yet".
        v.items = Arc::new(Vec::new());
        assert!(message(&v).is_none());
        assert_eq!(stacked(&v, w).0.len(), 2);
    }

    #[test]
    fn the_working_row_spinner_sweep_and_clocks_show_only_while_it_works() {
        let mut v = view();
        v.top = 1e9;
        let w = parts(&v).column.1;
        let (rows, _) = stacked(&v, w);
        let last = &rows.last().expect("rows").1;
        assert!(last.bits.iter().any(|(_, b)| *b == Bit::Spinner));
        assert!(last.bits.iter().any(|(_, b)| *b == Bit::Clock));
        let row = item_rect(&v, rows.len() - 1);
        let spins = chat_spinners(&v);
        assert_eq!(spins.len(), 1, "{spins:?}");
        assert!(inside(row, spins[0]));
        // 72 s since the turn began, in the head and on the row.
        let clocks = chat_clocks(&v, 1_000 + 72_000);
        assert_eq!(clocks.len(), 2, "{clocks:?}");
        assert!(clocks.iter().all(|(text, ..)| text == "1m 12s"));
        assert!(inside(parts(&v).head, clocks[0].1));
        assert!(inside(row, clocks[1].1));
        let sweep = chat_sweep(&v).expect("a sweep");
        assert!(inside(parts(&v).head, sweep));
        // A press on the row picks nothing.
        assert_eq!(chat_layout(&v).hit(&v, middle(row)), None);
        // In a short pane scrolled up, the row is out of view: only the
        // head's clock.
        let up = ChatView {
            rect: Rectangle::new(v.rect.loc, (v.rect.size.w, 360).into()),
            top: 0.0,
            ..v.clone()
        };
        assert!(chat_spinners(&up).is_empty());
        assert_eq!(chat_clocks(&up, 73_000).len(), 1);
        // None of it while it waits, is paused, asks or is idle.
        let still = |v: &ChatView| {
            let (rows, _) = stacked(v, w);
            let row = rows
                .last()
                .is_some_and(|(_, l)| l.bits.iter().any(|(_, b)| *b == Bit::Spinner));
            !row && chat_spinners(v).is_empty()
                && chat_clocks(v, 73_000).is_empty()
                && chat_sweep(v).is_none()
        };
        for status in [AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done] {
            assert!(still(&ChatView { status, ..view() }), "{status:?}");
        }
        assert!(still(&ChatView {
            paused: true,
            ..view()
        }));
        assert!(still(&ChatView {
            ask: Some(ask()),
            ..view()
        }));
        // No turn start known: the row and sweep, no clocks.
        v.since = None;
        assert!(chat_clocks(&v, 73_000).is_empty());
        assert_eq!(chat_spinners(&v).len(), 1);
    }

    fn ask() -> PendingAsk {
        let input: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        PendingAsk {
            ask: "q1".into(),
            tool: "Bash".into(),
            gist: "rm -rf target".into(),
            input: input.join("\n"),
            questions: Vec::new(),
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

    /// Plan mode asking to go ahead with a plan of 30 steps.
    pub(in crate::chrome) fn plan_ask() -> PendingAsk {
        let steps: String = (0..30).map(|i| format!("{}. step {i}\n", i + 1)).collect();
        PendingAsk {
            ask: "p1".into(),
            tool: "ExitPlanMode".into(),
            gist: String::new(),
            input: format!("# Retry the upload\n\n{steps}"),
            questions: Vec::new(),
            since: std::time::Instant::now(),
        }
    }

    #[test]
    fn a_plans_ask_shows_it_as_markdown_until_folded() {
        let mut v = view();
        v.ask = Some(plan_ask());
        let text = |laid: &Laid| -> Vec<String> {
            (laid.bits.iter())
                .filter_map(|(_, b)| match b {
                    Bit::Doc(DocPiece::Text { text, .. }) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let p = parts(&v);
        let (_, laid) = p.card.as_ref().unwrap();
        let shown = text(laid);
        assert_eq!(shown.first().map(String::as_str), Some("Retry the upload"));
        assert!(shown.len() > 3 && shown.len() < 31, "{shown:?}");
        let more = format!("{} more lines in the chat", 31 - shown.len());
        assert!(labels(laid).contains(&more), "{:?}", labels(laid));
        // Folded, as other input shows unfolded.
        v.unfolded.insert(ask_key("p1"));
        let q = parts(&v);
        assert!(text(&q.card.as_ref().unwrap().1).is_empty());
        assert!(q.waiting.unwrap().size.h < p.waiting.unwrap().size.h);
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
        let head = Point::from(((at.x + 4) as f64, (at.y + card_row() / 2) as f64));
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
        assert!(q.list.size.h >= 3 * card_row(), "{:?}", q.list);
        let said = labels(&q.card.as_ref().unwrap().1);
        assert!(!said.contains(&"line 11".to_owned()));
    }

    #[test]
    fn a_questions_card_shows_one_question_at_a_time_with_its_rows() {
        let mut v = view();
        v.waiting = Some(screen());
        v.ask = Some(question_ask());
        let blank = CardView {
            answered: vec![false; 3],
            ..CardView::default()
        };
        v.card = Some(blank.clone());
        let p = parts(&v);
        let panel = p.waiting.expect("the card's panel");
        assert!(
            p.menu.is_none() && p.keys.is_empty(),
            "it beats the screen's menu"
        );
        let (at, laid) = p.card.as_ref().unwrap();
        let said = labels(laid);
        // A tab per question, a header or its first words; the first
        // question's options, none of the others'.
        assert_eq!(said[..3], ["Size", "Where should it be save…", "Fruits"]);
        assert!(said.contains(&"Keys only".to_owned()) && !said.contains(&"Apple".to_owned()));
        let radios = laid
            .bits
            .iter()
            .filter(|(_, b)| matches!(b, Bit::Radio(_)))
            .count();
        assert_eq!(radios, 4, "three options and Other");
        assert!(laid.bits.iter().any(|(_, b)| matches!(b, Bit::Field(..))));
        let layout = chat_layout(&v);
        let spot = |hit: CardHit| {
            let (r, _) = laid
                .spots
                .iter()
                .find(|(_, s)| *s == Spot::Card(hit))
                .unwrap();
            // Near its left: Other's row has its field at the right.
            let r = Rectangle::new(r.loc + *at, r.size);
            Point::from(((r.loc.x + 4) as f64, (r.loc.y + r.size.h / 2) as f64))
        };

        for hit in [
            CardHit::Tab(2),
            CardHit::Row(1),
            CardHit::Row(3),
            CardHit::Other,
        ] {
            assert_eq!(layout.hit(&v, spot(hit)), Some(ChatHit::Card(hit)));
        }
        // Next, and the way back to the agent's own menu.
        let hits: Vec<_> = (p.answers.iter())
            .map(|(_, b)| layout.hit(&v, middle(*b)))
            .collect();
        let pass = ChatHit::Answer(AskReply::Pass);
        assert_eq!(hits, [Some(ChatHit::Card(CardHit::Go)), Some(pass)]);
        assert!(p.answers.iter().all(|(_, b)| inside(panel, *b)));
        assert_eq!(ask_buttons(&v)[0].1, "Next");
        // The last: check boxes, Submit, the tabs of those answered checked.
        v.card = Some(CardView {
            at: 2,
            row: 3,
            other: Line {
                text: "Kiwi".into(),
                ..Line::default()
            },
            answered: vec![true, true, false],
            ..blank
        });
        assert_eq!(ask_buttons(&v)[0].1, "Submit");
        let p = parts(&v);
        let laid = &p.card.as_ref().unwrap().1;
        let boxes = laid
            .bits
            .iter()
            .filter(|(_, b)| matches!(b, Bit::Checkbox(_)))
            .count();
        assert_eq!(boxes, 4);
        assert_eq!(
            laid.bits.iter().filter(|(_, b)| *b == Bit::Check).count(),
            2
        );
        // The keys on Other: its field takes the pointer and the caret (O6).
        let field = chat_field_spot(&v, 1.0).expect("Other's field");
        assert_eq!(field.line.text, "Kiwi");
        assert!(inside(p.waiting.unwrap(), field.area));
        v.card.as_mut().unwrap().row = 0;
        assert!(chat_field_spot(&v, 1.0).is_none());
        // A short pane keeps its list: the descriptions go.
        v.card.as_mut().unwrap().at = 0;
        let tall = parts(&v).waiting.unwrap().size.h;
        v.rect.size.h = 420;
        let q = parts(&v);
        assert!(q.waiting.unwrap().size.h < tall, "{tall} {:?}", q.waiting);
        assert!(
            !labels(&q.card.unwrap().1)
                .iter()
                .any(|t| t.starts_with("Rows"))
        );
    }

    #[test]
    fn the_screens_menu_shows_its_check_boxes_and_no_edges() {
        let mut v = view();
        v.waiting = Some(ticks());
        let (_, laid) = parts(&v).menu.expect("a menu");
        let ticked: Vec<bool> = (laid.bits.iter())
            .filter_map(|(_, b)| match b {
                Bit::Checkbox(on) => Some(*on),
                _ => None,
            })
            .collect();
        assert_eq!(ticked, [false, true, false]);
        let said = labels(&laid);
        assert!(said.contains(&"Rust".to_owned()) && said.contains(&"Submit".to_owned()));
        let docs = (laid.bits.iter()).filter_map(|(_, b)| match b {
            Bit::Doc(DocPiece::Text { text, .. }) => Some(text.clone()),
            _ => None,
        });
        assert!(
            said.into_iter()
                .chain(docs)
                .all(|t| !t.contains('│') && !t.contains("[ ]")),
            "no edges, no [ ] in words"
        );
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
