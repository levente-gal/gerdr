//! The chat in gerdr: flipping a pane between its chat and its screen,
//! the chat's keys, pointer and scrolling, and what it sends to the agent.
//!
//! A pane shows its chat while it was asked to (`PaneChat::shown`) and herdr
//! still finds an agent in it: one that quit leaves its shell, and the
//! screen shows again. The chats of the shown tab are followed by the tail
//! thread; hidden ones (Screen, or a tab out of sight) are not, and are
//! read again once shown (`chats_sync`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use smithay::{
    desktop::Window,
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::channel,
    utils::{Logical, Point},
};

use super::{
    ChatFocus, Event, Item, PaneChat, Reading,
    btw::{self, Follow, Side, SideState},
    commands::{self, Command, Slash},
    live::{Answers, AskReply, PendingAsk, Question},
    menu::{self, ChoiceKind, Menu},
    questions::Card,
    mode,
    model,
    rows::{rows, unread},
    tail::Tail,
};
use crate::{
    ChatRows, Gerdr, Pane, Workspace,
    ai_work::AgentKind,
    chrome::{self, CardHit, ChatHit, ChatLayout, ChatView, FieldSpot, WaitKey},
    herdr::{AgentInfo, AgentSession, AgentStatus, Herdr, ProcessInfo},
    key_table::{self, Binding},
    md_blocks::{DocBlock, ItemMark},
    prepare::{self, Step},
    scroll::{Bounds, Motion, NOTCH_PX, Scroll},
    text_field::{self, TextField},
};

/// How many of a pane's last screen rows are read while its agent waits:
/// enough for a menu with its question (`chat::menu`); the waiting panel
/// shows fewer of them when it finds none.
const WAITING_ROWS: usize = 60;
/// How often those rows are read again while its agent waits, at most.
const WAITING_EVERY: Duration = Duration::from_secs(1);
/// How many of a pane's last rows are read for a side question's panel:
/// Cursor draws its answer into the chat, as long as it is.
const SIDE_ROWS: usize = 200;
/// How often they are read while a side question is followed.
const SIDE_EVERY: Duration = Duration::from_millis(300);
/// How many reads in a row may fail before it gives up.
const SIDE_MISSES: u32 = 10;
/// How often a chat's screen is read for its agent's mode otherwise: the
/// agent changes it by itself too (Claude leaving plan mode).
const MODE_EVERY: Duration = Duration::from_secs(3);
/// How many times, `WAITING_EVERY` apart, an idle Claude Code's screen is
/// read for its suggestion before gerdr gives up until its next turn: it
/// comes a few seconds after the turn, or never when it is turned off.
pub(super) const SUGGEST_READS: u8 = 20;

impl PaneChat {
    /// A chat asked for the first time: shown, following its bottom, with an
    /// empty reply of lines.
    pub(crate) fn opened() -> Box<Self> {
        Box::new(Self {
            shown: true,
            stick: true,
            draft: TextField::multiline(""),
            ..Default::default()
        })
    }
}

/// Show a pane's chat the first time, then flip it between chat and screen;
/// all else it holds is kept, so the next flip is instant.
fn flip(chat: &mut Option<Box<PaneChat>>) {
    match chat {
        Some(chat) => chat.shown = !chat.shown,
        None => *chat = Some(PaneChat::opened()),
    }
}

/// A key of the chat's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKey {
    Next,
    Previous,
    Fold,
    Top,
    Bottom,
    /// A page up (-1) or down (1).
    Page(i8),
    Copy,
    Compose,
    /// Fill the reply with what the agent suggests saying next.
    Suggested,
}

/// A key of the chat's reply box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerKey {
    Send,
    Newline,
    /// Back to the list, the reply kept as a draft.
    Back,
    Discard,
    /// Fill the empty reply with what the agent suggests saying next.
    Suggested,
}

/// A key of the `/` menu over the reply box, while it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashKey {
    Next,
    Previous,
    /// Fill the reply with the command picked, to add to it.
    Fill,
    /// Send the command picked.
    Run,
    Close,
}

/// A key of the ask's card, while its agent waits on the chat's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AskKey {
    Answer(AskReply),
    /// Show or fold what it would run.
    Input,
}

/// The ask card's keys: they come before the list's and the reply box's
/// (`docs/ux-conventions.md`: Ctrl+Enter / Ctrl+Shift+Enter approve and
/// send back).
const ASK_KEYS: &[Binding<(), AskKey>] = &[
    Binding {
        chord: "Ctrl+Enter",
        does: |_| Some(AskKey::Answer(AskReply::Allow)),
        footer: true,
        f1: "allow what the agent asks to run",
    },
    Binding {
        chord: "Ctrl+Shift+Enter",
        does: |_| Some(AskKey::Answer(AskReply::Deny)),
        footer: true,
        f1: "deny what the agent asks to run",
    },
    Binding {
        chord: "Alt+Shift+I",
        does: |_| Some(AskKey::Input),
        footer: true,
        f1: "show or fold what the agent would run",
    },
];

/// The footer's hints of the ask card's keys, its input shown (`open`) or
/// not: Allow and Deny in their buttons' words (checklist K3).
fn ask_footer(open: bool, what: &str) -> Vec<(String, String)> {
    let said = |hit: ChatHit| {
        (chrome::ASK_BUTTONS.iter())
            .find(|(h, _)| *h == hit)
            .map_or("", |(_, label)| label)
            .to_lowercase()
    };
    (key_table::hints(ASK_KEYS, &(), |k| match k {
        AskKey::Answer(reply) => said(ChatHit::Answer(*reply)),
        AskKey::Input if open => format!("fold {what}"),
        AskKey::Input => format!("show {what}"),
    })
    .into_iter())
    .map(|(chord, said)| (chord.to_owned(), said))
    .collect()
}

/// Open `chat`'s last plan card in the list and pick it, folding its plan
/// ask's peek: the list has the room the waiting panel lacks. The card's
/// row, to scroll to; `None` and nothing changed without one.
fn show_plan(chat: &mut PaneChat) -> Option<usize> {
    let shown = rows(&chat.items, &chat.unfolded);
    let i = (shown.iter()).rposition(|row| matches!(row.item(&chat.items), Some(Item::Plan { .. })))?;
    if let Some(id) = shown[i].fold_key(&chat.items) {
        chat.unfolded.insert(id);
    }
    if let Some(ask) = &chat.live.ask {
        chat.unfolded.insert(chrome::ask_key(&ask.ask));
    }
    chat.picked = Some(i);
    chat.focus = ChatFocus::List;
    Some(i)
}

/// A key of the questions' card, while it has the keys (the list's place).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardKey {
    /// Next, or Submit on the last question.
    Go,
    /// Answer in terminal.
    Pass,
    /// From Other's words back to the options.
    Back,
    /// Pick or tick the row the keys are on.
    Pick,
    Down,
    Up,
    /// The question before or after.
    Before,
    After,
}

/// Where the questions' card stands, for its keys.
struct CardCtx {
    /// Several questions, Several picks, the last question shown.
    many: bool,
    multi: bool,
    last: bool,
    /// The keys are on Other: what they type is its words.
    other: bool,
    /// Its words aren't empty: Space is one of them.
    typing: bool,
}

/// The questions' card's keys: they come before the list's (Up, Down,
/// Space, Enter are the card's while it shows), never the reply box's.
const CARD_KEYS: &[Binding<CardCtx, CardKey>] = &[
    Binding {
        chord: "Enter",
        does: |_| Some(CardKey::Go),
        footer: true,
        f1: "go to the next question; on the last, submit the answers",
    },
    Binding {
        chord: "Space",
        does: |c| (!c.typing).then_some(CardKey::Pick),
        footer: true,
        f1: "pick the option the keys are on (tick it, where several go)",
    },
    Binding {
        chord: "Up",
        does: |_| Some(CardKey::Up),
        footer: true,
        f1: "go to the option above",
    },
    Binding {
        chord: "Down",
        does: |_| Some(CardKey::Down),
        footer: true,
        f1: "go to the option below",
    },
    Binding {
        chord: "Left",
        does: |c| (c.many && !c.other).then_some(CardKey::Before),
        footer: true,
        f1: "show the question before",
    },
    Binding {
        chord: "Right",
        does: |c| (c.many && !c.other).then_some(CardKey::After),
        footer: true,
        f1: "show the next question",
    },
    Binding {
        chord: "Esc",
        does: |c| {
            Some(if c.other {
                CardKey::Back
            } else {
                CardKey::Pass
            })
        },
        footer: true,
        f1: "give the questions back to the agent's own menu (from Other: back to the options)",
    },
];

/// The digits picking the card's options, hinted as one (they type into
/// Other while the keys are on it).
const CARD_DIGITS: &str = "1–9";

fn card_ctx(card: &Card, qs: &[Question]) -> CardCtx {
    let other = card.on_other(qs);
    CardCtx {
        many: qs.len() > 1,
        multi: qs.get(card.at).is_some_and(|q| q.multi),
        last: card.at + 1 >= qs.len(),
        other,
        typing: other && card.other.get(card.at).is_some_and(|f| !f.is_empty()),
    }
}

/// The footer's hints of the questions' card's keys, Next / Submit and
/// Answer in terminal in their buttons' words (checklist K3).
fn card_footer(card: &Card, ask: &PendingAsk) -> Vec<(String, String)> {
    let mut card = card.clone();
    card.sync(ask);
    let ctx = card_ctx(&card, &ask.questions);
    let [go, pass] = chrome::question_buttons(ctx.last).map(|(_, label)| label.to_lowercase());
    let pick = if ctx.multi { "tick" } else { "pick" };
    let mut hints = key_table::hints(CARD_KEYS, &ctx, |k| match k {
        CardKey::Go => go.clone(),
        CardKey::Pass => pass.clone(),
        CardKey::Back => "back".to_owned(),
        CardKey::Pick => pick.to_owned(),
        CardKey::Up | CardKey::Down => "choose".to_owned(),
        CardKey::Before | CardKey::After => "question".to_owned(),
    });
    if !ctx.other {
        hints.insert(1, (CARD_DIGITS, pick.to_owned()));
    }
    key_table::joined(hints)
}

/// A key for `chat`'s questions' card, while it has the keys: `Some` with
/// what it asks of gerdr when the card took it, `None` when it is the
/// list's.
fn card_key(
    chat: &mut PaneChat,
    mods: &ModifiersState,
    sym: Keysym,
    typed: Keysym,
) -> Option<Option<ChatDo>> {
    let ask = chat.live.ask.clone()?;
    let qs = &ask.questions;
    let card = &mut chat.card;
    card.sync(&ask);
    let ctx = card_ctx(card, qs);
    match key_table::press(CARD_KEYS, &ctx, mods, sym) {
        Some(CardKey::Go) => return Some(card_go(card, qs)),
        Some(CardKey::Pass) => return Some(Some(ChatDo::Answer(AskReply::Pass))),
        Some(CardKey::Pick) => card.pick(qs, card.row),
        Some(CardKey::Back | CardKey::Up) => card.step(qs, -1),
        Some(CardKey::Down) => card.step(qs, 1),
        Some(CardKey::Before) => card.go(qs, card.at.saturating_sub(1)),
        Some(CardKey::After) => card.go(qs, card.at + 1),
        // On Other, the keys write its words.
        None if ctx.other => {
            let edit = card.field()?.key(mods, sym, typed);
            if edit.changed() {
                card.typed(qs);
            }
            return (edit != text_field::Edit::Ignored).then_some(None);
        }
        None => {
            let digit = (typed.key_char())
                .filter(|_| !(mods.ctrl || mods.alt || mods.logo))
                .and_then(|c| c.to_digit(10))
                .filter(|d| (1..=9).contains(d))?;
            card.pick(qs, digit as usize - 1);
        }
    }
    Some(None)
}

/// Next, or Submit on the last question: the answers to send once each
/// question has one, else the first without one shown.
fn card_go(card: &mut Card, qs: &[Question]) -> Option<ChatDo> {
    if card.at + 1 < qs.len() {
        card.go(qs, card.at + 1);
        return None;
    }
    if let Some(answers) = card.answers(qs) {
        return Some(ChatDo::Answers(answers));
    }
    let open = (0..qs.len()).find(|i| card.answer(qs, *i).is_none());
    card.go(qs, open.unwrap_or(card.at));
    None
}

/// A press on `hit` of `chat`'s questions' card: what it asks of gerdr.
fn card_press(chat: &mut PaneChat, hit: CardHit) -> Option<ChatDo> {
    let ask = chat.live.ask.clone()?;
    let qs = &ask.questions;
    chat.focus = ChatFocus::List;
    chat.card.sync(&ask);
    match hit {
        CardHit::Tab(i) => chat.card.go(qs, i),
        CardHit::Row(i) => chat.card.pick(qs, i),
        CardHit::Other => chat.card.row = qs.get(chat.card.at)?.options.len(),
        CardHit::Go => return card_go(&mut chat.card, qs),
    }
    None
}

/// What a press on choice `n` of the menu `m` read off the screen sends:
/// its digit, after Up while the menu's arrow is on the user's own answer
/// (a digit typed there is its text); none for that row itself, whose words
/// go in the reply box.
fn choice_keys(m: Option<&Menu>, n: u8) -> Option<Vec<WaitKey>> {
    let choices = m.map_or(&[][..], |m| &m.choices);
    let free = |c: &&menu::Choice| c.kind == ChoiceKind::Free;
    if choices.iter().filter(free).any(|c| c.n == n) {
        return None;
    }
    let up = choices.iter().filter(free).any(|c| c.picked);
    Some(
        up.then_some(WaitKey::Up)
            .into_iter()
            .chain([WaitKey::Digit(n)])
            .collect(),
    )
}

/// The list's keys (`docs/ux-conventions.md`, the agent chat). As the
/// reply box's, they are offered by whether Left fills in a suggestion
/// (`fillable`).
const LIST_KEYS: &[Binding<bool, ListKey>] = &[
    Binding {
        chord: "J",
        does: |_| Some(ListKey::Next),
        footer: true,
        f1: "go to the next item",
    },
    Binding {
        chord: "Ctrl+J",
        does: |_| Some(ListKey::Next),
        footer: false,
        f1: "go to the next item",
    },
    key_table::alias("Down", |_| Some(ListKey::Next)),
    Binding {
        chord: "K",
        does: |_| Some(ListKey::Previous),
        footer: true,
        f1: "go to the previous item",
    },
    Binding {
        chord: "Ctrl+K",
        does: |_| Some(ListKey::Previous),
        footer: false,
        f1: "go to the previous item",
    },
    key_table::alias("Up", |_| Some(ListKey::Previous)),
    Binding {
        chord: "Space",
        does: |_| Some(ListKey::Fold),
        footer: true,
        f1: "fold or unfold the item",
    },
    Binding {
        chord: "Enter",
        does: |_| Some(ListKey::Fold),
        footer: false,
        f1: "fold or unfold the item",
    },
    Binding {
        chord: "G",
        does: |_| Some(ListKey::Top),
        footer: false,
        f1: "go to the first item",
    },
    key_table::alias("Home", |_| Some(ListKey::Top)),
    Binding {
        chord: "Shift+G",
        does: |_| Some(ListKey::Bottom),
        footer: false,
        f1: "go to the last item, following new ones",
    },
    key_table::alias("End", |_| Some(ListKey::Bottom)),
    Binding {
        chord: "PgUp",
        does: |_| Some(ListKey::Page(-1)),
        footer: false,
        f1: "scroll a page up",
    },
    Binding {
        chord: "PgDn",
        does: |_| Some(ListKey::Page(1)),
        footer: false,
        f1: "scroll a page down",
    },
    Binding {
        chord: "Y",
        does: |_| Some(ListKey::Copy),
        footer: true,
        f1: "copy the item",
    },
    Binding {
        chord: "Ctrl+C",
        does: |_| Some(ListKey::Copy),
        footer: false,
        f1: "copy the item",
    },
    Binding {
        chord: "I",
        does: |_| Some(ListKey::Compose),
        footer: true,
        f1: "write a reply (or type it at once)",
    },
    Binding {
        chord: "Left",
        does: |&fill| fill.then_some(ListKey::Suggested),
        footer: true,
        f1: "fill the reply with what the agent suggests saying next",
    },
];

/// The reply box's keys; the rest edit it as any text field. Left fills
/// in a suggestion only while the reply is empty (then it moves nothing).
const COMPOSER_KEYS: &[Binding<bool, ComposerKey>] = &[
    Binding {
        chord: "Enter",
        does: |_| Some(ComposerKey::Send),
        footer: true,
        f1: "send the reply to the agent",
    },
    Binding {
        chord: "Shift+Enter",
        does: |_| Some(ComposerKey::Newline),
        footer: true,
        f1: "start a new line",
    },
    Binding {
        chord: "Esc",
        does: |_| Some(ComposerKey::Back),
        footer: true,
        f1: "go back to the list, the reply kept as a draft",
    },
    Binding {
        chord: text_field::DISCARD_KEY,
        does: |_| Some(ComposerKey::Discard),
        footer: true,
        f1: "discard the draft",
    },
    Binding {
        chord: "Left",
        does: |&fill| fill.then_some(ComposerKey::Suggested),
        footer: true,
        f1: "",
    },
];

/// The mode key, from the list and the reply box alike: the agents' own.
const MODE_KEYS: &[Binding<(), ()>] = &[Binding {
    chord: "Shift+Tab",
    does: |_| Some(()),
    footer: true,
    f1: "switch the agent to its next mode (plan, build, …)",
}];

/// The model key, from the list and the reply box alike: the key that
/// picks a stage agent's model on the feature tab.
pub(crate) const MODEL_KEYS: &[Binding<(), ()>] = &[Binding {
    chord: "Alt+Shift+A",
    does: |_| Some(()),
    footer: true,
    f1: "pick the model the agent runs on",
}];

/// The `/` menu's keys: they come before the reply box's while it shows.
const SLASH_KEYS: &[Binding<(), SlashKey>] = &[
    Binding {
        chord: "Enter",
        does: |_| Some(SlashKey::Run),
        footer: true,
        f1: "send the command picked to the agent",
    },
    Binding {
        chord: "Tab",
        does: |_| Some(SlashKey::Fill),
        footer: true,
        f1: "fill in the command picked, to add to it",
    },
    Binding {
        chord: "Down",
        does: |_| Some(SlashKey::Next),
        footer: true,
        f1: "pick the next command",
    },
    key_table::alias("Ctrl+J", |_| Some(SlashKey::Next)),
    Binding {
        chord: "Up",
        does: |_| Some(SlashKey::Previous),
        footer: true,
        f1: "pick the previous command",
    },
    key_table::alias("Ctrl+K", |_| Some(SlashKey::Previous)),
    Binding {
        chord: "Esc",
        does: |_| Some(SlashKey::Close),
        footer: true,
        f1: "close the commands, keeping the reply",
    },
];

/// The footer's key hints while the `/` menu shows (checklist K3).
pub(crate) fn slash_footer() -> Vec<(String, String)> {
    key_table::joined(key_table::hints(SLASH_KEYS, &(), |k| {
        match k {
            SlashKey::Run => "run",
            SlashKey::Fill => "fill in",
            SlashKey::Next | SlashKey::Previous => "pick",
            SlashKey::Close => "close",
        }
        .to_owned()
    }))
}

/// Open, filter or close `chat`'s `/` menu for its reply as it now is;
/// `load` lists its agent's commands, read each time the menu opens.
fn slash_sync(chat: &mut PaneChat, load: impl FnOnce() -> Vec<Command>) {
    let typed = (chat.focus == ChatFocus::Composer)
        .then(|| commands::query(&chat.draft).map(str::to_owned))
        .flatten();
    match typed {
        Some(typed) => chat
            .slash
            .get_or_insert_with(|| Slash::new(load()))
            .sync(&typed),
        None => chat.slash = None,
    }
}

/// The footer's key hints of a chat whose keys are at `focus` (checklist
/// K3: from the tables the keys are read with), with the mode key's if its
/// agent has `modes` and the model key's if it `models`. Esc in the list does nothing, so it has none; Esc in
/// the reply box says what becomes of the draft.
pub(crate) fn footer(
    focus: ChatFocus,
    draft_empty: bool,
    suggested: bool,
    modes: bool,
    models: bool,
) -> Vec<(String, String)> {
    let fill = draft_empty && suggested;
    let mode = (key_table::hints(MODE_KEYS, &(), |_| "mode".to_owned()).into_iter())
        .filter(|_| modes)
        .chain(
            (key_table::hints(MODEL_KEYS, &(), |_| "model".to_owned()).into_iter())
                .filter(|_| models),
        )
        .map(|(chord, said)| (chord.to_owned(), said));
    let keys = match focus {
        ChatFocus::List => key_table::joined(key_table::hints(LIST_KEYS, &fill, |k| {
            match k {
                ListKey::Next | ListKey::Previous => "move",
                ListKey::Fold => "fold",
                ListKey::Copy => "copy",
                ListKey::Compose => "reply",
                ListKey::Suggested => "fill in",
                ListKey::Top | ListKey::Bottom | ListKey::Page(_) => "scroll",
            }
            .to_owned()
        })),
        ChatFocus::Composer => {
            let hints = key_table::hints(COMPOSER_KEYS, &fill, |k| {
                match k {
                    ComposerKey::Send => "send",
                    ComposerKey::Newline => "new line",
                    ComposerKey::Back if draft_empty => "back",
                    ComposerKey::Back => "keep draft",
                    ComposerKey::Discard => "discard",
                    ComposerKey::Suggested => "fill in",
                }
                .to_owned()
            });
            let discard = |chord: &str| chord == text_field::DISCARD_KEY;
            (hints.into_iter())
                .filter(|(chord, _)| !(draft_empty && discard(chord)))
                .map(|(chord, said)| (chord.to_owned(), said))
                .collect()
        }
    };
    keys.into_iter().chain(mode).collect()
}

/// The footer's key hints of `chat` (K3): its ask card's, or its questions'
/// card's while that has the keys, then those of where its keys are, but
/// for a key the card took.
fn chat_footer(chat: &PaneChat, modes: bool, models: bool) -> Vec<(String, String)> {
    let ask = chat.live.ask.as_ref();
    let questions = ask.filter(|a| !a.questions.is_empty());
    let card = questions.filter(|_| chat.focus == ChatFocus::List);
    let mut hints = match (ask, card) {
        (_, Some(ask)) => card_footer(&chat.card, ask),
        (Some(ask), _) if questions.is_none() => {
            let what = match chrome::is_plan(ask) {
                true => "plan",
                false => "input",
            };
            ask_footer(chrome::ask_open(ask, &chat.unfolded), what)
        }
        _ => Vec::new(),
    };
    let rest = match chat.slash.as_ref().is_some_and(|s| s.open()) {
        true => slash_footer(),
        false => footer(
            chat.focus,
            chat.draft.line().text.is_empty(),
            chat.suggestion.is_some(),
            modes,
            models,
        ),
    };
    let taken = |chord: &str| card.is_some() && CARD_KEYS.iter().any(|b| b.chord == chord);
    hints.extend(rest.into_iter().filter(|(chord, _)| !taken(chord)));
    hints
}

/// What a key of the chat asks of gerdr, once the chat took it.
#[derive(Clone, Debug, PartialEq)]
enum ChatDo {
    /// Send the reply to the agent.
    Send,
    /// Answer the ask on the chat's card.
    Answer(AskReply),
    /// Answer the questions on the chat's card.
    Answers(Answers),
    /// Give the clipboard this text.
    Copy(String),
    /// Scroll to the top, to the bottom (following new items again), or a
    /// page up or down.
    Top,
    Bottom,
    Page(i8),
    /// Step the agent to its next mode.
    NextMode,
    /// Open the picker of the agent's model.
    PickModel,
}

/// A key of the reply box, while it has the keys.
fn composer_key(
    chat: &mut PaneChat,
    mods: &ModifiersState,
    sym: Keysym,
    typed: Keysym,
) -> Option<ChatDo> {
    match key_table::press(COMPOSER_KEYS, &fillable(chat), mods, sym) {
        Some(ComposerKey::Send) => return Some(ChatDo::Send),
        Some(ComposerKey::Newline) => _ = chat.draft.newline(),
        // Never saved nor thrown away (checklist K2).
        Some(ComposerKey::Back) => chat.focus = ChatFocus::List,
        Some(ComposerKey::Discard) => chat.draft.clear(),
        Some(ComposerKey::Suggested) => fill_suggested(chat),
        None => _ = chat.draft.key(mods, sym, typed),
    }
    None
}

/// Whether Left fills `chat`'s reply with its agent's suggestion: one
/// waits, and the reply is empty.
fn fillable(chat: &PaneChat) -> bool {
    chat.suggestion.is_some() && chat.draft.is_empty()
}

/// Fill `chat`'s reply with its agent's suggestion, the caret at its end,
/// to send or change.
fn fill_suggested(chat: &mut PaneChat) {
    if let Some(text) = chat.suggestion.clone() {
        chat.draft.set(&text);
        chat.focus = ChatFocus::Composer;
    }
}

/// Whether the key typed is a character, to write into the reply box.
fn types(mods: &ModifiersState, typed: Keysym) -> bool {
    !(mods.ctrl || mods.alt || mods.logo) && typed.key_char().is_some_and(|c| !c.is_control())
}

/// A key for `chat` while it has the keys: `sym` the key's own symbol,
/// `typed` what Shift and the layout made of it (checklist K8).
fn chat_key(
    chat: &mut PaneChat,
    mods: &ModifiersState,
    sym: Keysym,
    typed: Keysym,
) -> Option<ChatDo> {
    let questions = (chat.live.ask.as_ref()).is_some_and(|a| !a.questions.is_empty());
    if questions
        && chat.focus == ChatFocus::List
        && let Some(done) = card_key(chat, mods, sym, typed)
    {
        return done;
    }
    if let Some(ask) = chat.live.ask.as_ref().filter(|_| !questions) {
        match key_table::press(ASK_KEYS, &(), mods, sym) {
            Some(AskKey::Answer(reply)) => return Some(ChatDo::Answer(reply)),
            Some(AskKey::Input) => {
                let key = chrome::ask_key(&ask.ask);
                if !chat.unfolded.remove(&key) {
                    chat.unfolded.insert(key);
                }
                return None;
            }
            None => {}
        }
    }
    if key_table::press(MODE_KEYS, &(), mods, sym).is_some() {
        return Some(ChatDo::NextMode);
    }
    if key_table::press(MODEL_KEYS, &(), mods, sym).is_some() {
        return Some(ChatDo::PickModel);
    }
    if chat.focus == ChatFocus::Composer
        && let Some(slash) = chat.slash.as_mut().filter(|slash| slash.open())
    {
        let name = slash.picked().map(|c| c.name.clone()).unwrap_or_default();
        match key_table::press(SLASH_KEYS, &(), mods, sym) {
            Some(SlashKey::Next) => slash.step(1),
            Some(SlashKey::Previous) => slash.step(-1),
            Some(SlashKey::Close) => slash.hidden = true,
            Some(SlashKey::Fill) => chat.draft.set(&format!("/{name} ")),
            Some(SlashKey::Run) => {
                chat.draft.set(&format!("/{name}"));
                return Some(ChatDo::Send);
            }
            None => return composer_key(chat, mods, sym, typed),
        }
        return None;
    }
    if chat.focus == ChatFocus::Composer {
        return composer_key(chat, mods, sym, typed);
    }
    // The keys move over the rows as they show: a fold of steps is one.
    let shown = rows(&chat.items, &chat.unfolded);
    let n = shown.len();
    let last = n.checked_sub(1);
    match key_table::press(LIST_KEYS, &fillable(chat), mods, sym) {
        Some(ListKey::Next) => chat.picked = chat.picked.map_or(last, |i| Some((i + 1).min(n - 1))),
        Some(ListKey::Previous) => {
            chat.picked = chat.picked.map_or(last, |i| Some(i.saturating_sub(1)))
        }
        Some(ListKey::Fold) => {
            let row = chat.picked.and_then(|i| shown.get(i));
            if let Some(id) = row.and_then(|row| row.fold_key(&chat.items))
                && !chat.unfolded.remove(&id)
            {
                chat.unfolded.insert(id);
            }
        }
        Some(ListKey::Top) => {
            chat.picked = (n > 0).then_some(0);
            return Some(ChatDo::Top);
        }
        Some(ListKey::Bottom) => {
            chat.picked = last;
            return Some(ChatDo::Bottom);
        }
        Some(ListKey::Page(by)) => return Some(ChatDo::Page(by)),
        Some(ListKey::Copy) => {
            let row = chat.picked.or(last).and_then(|i| shown.get(i))?;
            let texts: Vec<String> = row.items(&chat.items).iter().map(item_text).collect();
            return Some(ChatDo::Copy(texts.join("\n\n")));
        }
        Some(ListKey::Compose) => chat.focus = ChatFocus::Composer,
        Some(ListKey::Suggested) => fill_suggested(chat),
        // Any character goes to the reply box, typed into it; a paste too.
        None if sym == text_field::PASTE || types(mods, typed) => {
            chat.focus = ChatFocus::Composer;
            chat.draft.key(mods, sym, typed);
        }
        None => {}
    }
    None
}

/// What copying `item` gives: what the user wrote, the agent's words, a
/// tool's input and output, an edit's change.
fn item_text(item: &Item) -> String {
    match item {
        Item::User { text, .. }
        | Item::Thinking { text, .. }
        | Item::Note { text, .. }
        | Item::Plan { text, .. } => text.clone(),
        Item::Assistant { blocks, .. } => blocks_text(blocks),
        Item::Tool {
            name,
            input,
            output,
            ..
        } => {
            let mut text = format!("{name}\n{input}");
            if let Some(out) = output {
                text.push_str("\n\n");
                text.push_str(&out.text);
            }
            text
        }
        Item::Edit { path, diff, .. } => {
            let mut text = path.clone();
            for line in diff {
                let mark = match line.side {
                    super::DiffSide::Context => ' ',
                    super::DiffSide::Added => '+',
                    super::DiffSide::Removed => '-',
                };
                text.push('\n');
                text.push(mark);
                text.push_str(&line.text);
            }
            text
        }
        Item::Subagent { title, items, .. } => std::iter::once(title.clone())
            .chain(items.iter().map(item_text))
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

/// The agent's words as markdown again, near enough: what the transcript
/// gave is parsed into blocks once, off the UI thread.
fn blocks_text(blocks: &[DocBlock]) -> String {
    let block = |b: &DocBlock| match b {
        DocBlock::Heading { level, text } => {
            format!("{} {}", "#".repeat(usize::from(*level)), text.text)
        }
        DocBlock::Para(text) => text.text.clone(),
        DocBlock::Item { depth, mark, text } => {
            let mark = match mark {
                ItemMark::Bullet => "- ".to_owned(),
                ItemMark::Number(n) => format!("{n}. "),
                ItemMark::Task(done) => format!("- [{}] ", if *done { 'x' } else { ' ' }),
                ItemMark::Under => "  ".to_owned(),
            };
            format!("{}{mark}{}", "  ".repeat(usize::from(*depth)), text.text)
        }
        DocBlock::Code(lines) => format!("```\n{}\n```", lines.join("\n")),
        DocBlock::Quote(text) => format!("> {}", text.text),
        DocBlock::Table(rows) => (rows.iter().map(|r| r.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        DocBlock::Rule => "---".to_owned(),
        DocBlock::Note(text) => text.clone(),
    };
    blocks.iter().map(block).collect::<Vec<_>>().join("\n\n")
}

/// The reply as one line: a line break typed into the pane would send it
/// there and then.
/// shortcut: a reply of lines goes as one; type it with bracketed paste per
/// agent when someone needs the line breaks.
fn one_line(draft: &str) -> String {
    draft.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a line typed into a pane, where herdr says `info` runs, goes to
/// its agent: with its shell in front, it would run as a command.
fn agent_in_front(info: &Result<ProcessInfo, String>) -> Result<(), String> {
    match prepare::step(info) {
        Step::Busy => Ok(()),
        Step::Drop => Err(match info {
            Err(err) => format!("herdr didn't say what runs in the pane: {err}"),
            Ok(_) => "its pane is gone".to_owned(),
        }),
        Step::Wait | Step::Send => Err("its pane's shell is in front, not the agent".to_owned()),
    }
}

/// herdr's name of `key`, for `pane.send_keys` (as the phone presses them).
fn wait_key_name(key: WaitKey) -> String {
    match key {
        WaitKey::Esc => "esc".into(),
        WaitKey::Up => "Up".into(),
        WaitKey::Down => "Down".into(),
        WaitKey::Left => "Left".into(),
        WaitKey::Right => "Right".into(),
        WaitKey::Enter => "Enter".into(),
        WaitKey::Digit(d) => d.to_string(),
    }
}

/// Whether `over`, the overlay on top at a chat's middle, hides it: the
/// small ones that come and go over everything don't.
fn covers_chat(over: Option<crate::overlay::Overlay>) -> bool {
    use crate::overlay::Overlay;
    over.is_some_and(|o| !matches!(o, Overlay::Tooltip | Overlay::Toasts | Overlay::Help))
}

/// Which panes' chats to follow afresh and which to stop following: `wanted`
/// are those to follow now (shown, in sight), with their agents as herdr has
/// them. One already followed is followed again when herdr says its agent
/// holds another session (`/clear`, `/resume`, a new chat).
fn follow_plan<'a>(
    followed: &HashMap<String, Option<AgentSession>>,
    wanted: &'a [AgentInfo],
) -> (Vec<&'a AgentInfo>, Vec<String>) {
    let follow = (wanted.iter())
        .filter(|a| followed.get(&a.pane_id) != Some(&a.agent_session))
        .collect();
    let stop = (followed.keys())
        .filter(|id| !wanted.iter().any(|a| &a.pane_id == *id))
        .cloned()
        .collect();
    (follow, stop)
}

/// Where a chat stands: its first item, that item's row's top in its list,
/// and how far the list is scrolled.
struct Place {
    first: String,
    y: i32,
    top: f64,
}

/// Take `event` from the tail into its pane's chat, among `chats` (by herdr
/// pane id). Whether a chat took it: one closed meanwhile doesn't.
fn take_event<'a>(
    chats: impl IntoIterator<Item = (&'a str, &'a mut PaneChat)>,
    event: Event,
) -> bool {
    let pane_id = match &event {
        Event::Items { pane_id, .. }
        | Event::Missing { pane_id, .. }
        | Event::Failed { pane_id, .. } => pane_id.clone(),
    };
    let Some((_, chat)) = chats.into_iter().find(|(id, _)| *id == pane_id) else {
        return false;
    };
    match event {
        Event::Items {
            source,
            items,
            earlier,
            model,
            context,
            ..
        } => {
            chat.context = context;
            // Another chat in the pane (another agent, a new session): its
            // own model, not the last one's.
            if model.is_some() || chat.source.as_ref() != Some(&source) {
                chat.model = model;
            }
            let n = rows(&items, &chat.unfolded).len();
            chat.picked = chat.picked.filter(|i| *i < n);
            chat.items = chat.live.timed(items);
            chat.earlier = earlier;
            chat.reading_earlier = false;
            chat.source = Some(source);
            chat.reading = Reading::Read;
        }
        Event::Missing { why, .. } => {
            chat.reading = Reading::Missing(why);
            chat.model = None;
            chat.context = None;
        }
        Event::Failed { error, .. } => {
            chat.reading = Reading::Failed(error);
            chat.model = None;
            chat.context = None;
        }
    }
    true
}

/// The rows of a pane's screen worth showing: the last `n`, without the
/// empty ones at the bottom.
fn last_rows(mut rows: ChatRows, n: usize) -> ChatRows {
    while rows.last().is_some_and(|(text, _)| text.trim().is_empty()) {
        rows.pop();
    }
    let cut = rows.len().saturating_sub(n);
    rows.split_off(cut)
}

/// Pane `pane_id`'s last `n` screen rows with their colours, read from
/// herdr off the UI thread.
fn screen_rows(herdr: &mut Herdr, pane_id: &str, n: usize) -> Result<ChatRows, String> {
    let text = herdr.pane_styled(pane_id, n)?;
    // An app on the alternate screen has no scrollback: its screen, plainly.
    let text = match text.trim().is_empty() {
        true => herdr.pane_screen_styled(pane_id, n)?,
        false => text,
    };
    Ok(last_rows(crate::scrollback::parse_ansi(&text), n))
}

/// Follow side question `follow` asked of pane `pane_id`'s agent on its
/// screen (`btw::Follow`), on the thread it runs on, until it is answered
/// or fails; each change goes to `tx` as side `n` of the pane.
fn follow_side(
    herdr: &mut Herdr,
    pane_id: &str,
    n: usize,
    mut follow: Follow,
    tx: &channel::Sender<(String, usize, Side)>,
) {
    let mut misses = 0;
    loop {
        std::thread::sleep(SIDE_EVERY);
        let was = follow.side.clone();
        let step = match screen_rows(herdr, pane_id, SIDE_ROWS) {
            Ok(rows) => {
                misses = 0;
                let rows: Vec<&str> = rows.iter().map(|(text, _)| text.as_str()).collect();
                follow.took(btw::panel(&rows), Instant::now())
            }
            Err(err) => {
                misses += 1;
                if misses < SIDE_MISSES {
                    continue;
                }
                follow.side.state = SideState::Failed(format!("couldn't read its pane: {err}"));
                btw::Step::Done
            }
        };
        let keys: &[&str] = match &step {
            btw::Step::Press(keys) => keys,
            btw::Step::Close(key) => std::slice::from_ref(key),
            btw::Step::Wait | btw::Step::Done => &[],
        };
        // Read again on the next round: a key herdr refused shows the
        // same panel, and its answer is kept as read.
        if !keys.is_empty()
            && let Err(err) = herdr.send_keys(pane_id, keys)
        {
            tracing::warn!("pressing {keys:?} for a side question in pane {pane_id}: {err}");
        }
        // The loop is gone only as gerdr quits.
        if follow.side != was && tx.send((pane_id.to_owned(), n, follow.side.clone())).is_err() {
            return;
        }
        if matches!(step, btw::Step::Close(_) | btw::Step::Done) {
            return;
        }
    }
}

/// Whether `agent`'s screen may show what it suggests saying next: a
/// Claude Code whose turn ended.
pub(crate) fn suggests(agent: &AgentInfo) -> bool {
    matches!(agent.agent_status, AgentStatus::Idle | AgentStatus::Done)
        && super::source::kind(agent) == Some(crate::ai_work::AgentKind::Claude)
}

/// What Claude Code suggests saying next, as its screen shows it: the
/// last `❯` line, its reply box, holding dim text only (what the user
/// types there is plain).
pub(crate) fn suggestion(rows: &[(String, crate::scrollback::Styled)]) -> Option<String> {
    let (text, styles) = rows.iter().rev().find(|(text, _)| text.starts_with('❯'))?;
    let chars: Vec<char> = text.chars().collect();
    let start = (chars.iter()).take_while(|c| **c == '❯' || c.is_whitespace()).count();
    let dim = |i: usize| (styles.iter()).any(|(at, style)| at.contains(&i) && style.dim);
    let words: String = chars[start..].iter().collect::<String>().trim_end().to_owned();
    let ghost = (start..chars.len()).all(|i| chars[i].is_whitespace() || dim(i));
    (ghost && !words.is_empty()).then_some(words)
}

/// How a chat's list scrolls: in pixels, from its top to as far as its
/// items are higher than it.
fn chat_bounds(layout: &ChatLayout) -> Bounds {
    let view = f64::from(layout.list.size.h);
    Bounds {
        max: (f64::from(layout.content) - view).max(0.0),
        notch: NOTCH_PX,
        unit: 1.0,
        next: 1.0,
        prev: 1.0,
        view,
    }
}

/// The chat of herdr pane `pane_id`, among `workspaces`' panes and those
/// whose window is still to come.
pub(crate) fn chat_mut<'a>(
    workspaces: &'a mut [Workspace],
    pane_id: &str,
) -> Option<&'a mut PaneChat> {
    chats_mut(workspaces).find_map(|(id, chat)| (id == pane_id).then_some(chat))
}

/// Every chat of `workspaces`, by herdr pane id.
fn chats_mut(workspaces: &mut [Workspace]) -> impl Iterator<Item = (&str, &mut PaneChat)> {
    workspaces
        .iter_mut()
        .flat_map(|ws| &mut ws.tabs)
        .flat_map(|tab| {
            let panes = (tab.panes.iter_mut())
                .filter_map(|p| Some((p.herdr.as_ref()?.pane_id.as_str(), p.chat.as_deref_mut()?)));
            let expected = (tab.expected.iter_mut())
                .filter_map(|e| Some((e.herdr.as_ref()?.pane_id.as_str(), e.chat.as_deref_mut()?)));
            panes.chain(expected)
        })
}

impl Gerdr {
    /// herdr's agent in pane `pane_id`, if it found one there.
    pub(crate) fn chat_agent(&self, pane_id: &str) -> Option<&AgentInfo> {
        self.session.agents.iter().find(|a| a.pane_id == pane_id)
    }

    /// Whether `pane` shows its chat in its place: it was asked to, and herdr
    /// still finds its agent. One that quit leaves its shell: the screen.
    pub(crate) fn chat_shown(&self, pane: &Pane) -> bool {
        pane.chat.as_ref().is_some_and(|c| c.shown)
            && (pane.herdr.as_ref()).is_some_and(|h| self.chat_agent(&h.pane_id).is_some())
    }

    /// Whether `window`'s pane shows its chat in its place.
    pub(crate) fn window_shows_chat(&self, window: &Window) -> bool {
        (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .flat_map(|tab| &tab.panes)
            .find(|p| &p.window == window)
            .is_some_and(|p| self.chat_shown(p))
    }

    /// Show the focused agent pane as a chat, or its screen again.
    pub(crate) fn toggle_chat(&mut self) {
        match self.tab().and_then(|tab| tab.focused_id()) {
            Some(id) => self.toggle_pane_chat(id),
            None => self.show_notice("No agent runs in this pane".into()),
        }
    }

    /// Show pane `id` of the active tab as its agent's chat, or its screen
    /// again (the prefix key, its bar's button, its bar's menu). Hiding the
    /// chat never stops the agent. A pane herdr finds no agent in has no
    /// chat: nothing changes.
    pub(crate) fn toggle_pane_chat(&mut self, id: u32) {
        let pane_id = (self.tab())
            .and_then(|tab| tab.herdr_pane(id))
            .map(|p| p.pane_id.clone());
        if !pane_id.is_some_and(|p| self.chat_agent(&p).is_some()) {
            return self.show_notice("No agent runs in this pane".into());
        }
        let (w, t) = match self.workspace() {
            Some(ws) => (self.active_workspace, ws.active_tab),
            None => return,
        };
        let Some(pane) = (self.workspaces[w].tabs[t].panes.iter_mut()).find(|p| p.id == id) else {
            return;
        };
        flip(&mut pane.chat);
        // Its window off screen or back, the chat followed or not.
        self.arrange();
        // The keys go to the chat, or to the window again.
        self.focus_pane(|_| Some(id));
    }

    /// The chats shown and followed brought in line with what shows and with
    /// herdr's agents, after a round of them: a chat whose agent came or
    /// went shows or hides.
    pub(crate) fn chats_refresh(&mut self) {
        let shown: HashSet<String> = (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .flat_map(|tab| &tab.panes)
            .filter(|p| self.chat_shown(p))
            .filter_map(|p| Some(p.herdr.as_ref()?.pane_id.clone()))
            .collect();
        if shown == self.chats.shown {
            return self.chats_sync();
        }
        // `arrange` syncs them.
        self.arrange();
        // The keys go to the chat that came, or to the window again.
        if let Some(window) = self.tab().and_then(|tab| tab.focused.clone()) {
            self.focus(Some(window));
        }
    }

    /// Follow the chats shown in sight, and only those (checklist C6: a
    /// closed pane's, a hidden one's, one in a tab out of sight stop), and
    /// read the screens of those whose agent waits on the user.
    pub(crate) fn chats_sync(&mut self) {
        let mut shown = HashSet::new();
        let mut wanted = Vec::new();
        for (w, ws) in self.workspaces.iter().enumerate() {
            for (t, tab) in ws.tabs.iter().enumerate() {
                for pane in tab.panes.iter().filter(|p| self.chat_shown(p)) {
                    let Some(agent) =
                        (pane.herdr.as_ref()).and_then(|h| self.chat_agent(&h.pane_id))
                    else {
                        continue;
                    };
                    shown.insert(agent.pane_id.clone());
                    if self.is_visible(w, t) {
                        wanted.push(self.with_live_session(agent));
                    }
                }
            }
        }
        if !shown.is_empty() {
            self.install_live_hooks();
        }
        self.chats.shown = shown;
        let (follow, stop) = follow_plan(&self.chats.followed, &wanted);
        let follow: Vec<AgentInfo> = follow.into_iter().cloned().collect();
        if !(follow.is_empty() && stop.is_empty())
            && self.start_chat_tail()
            && let Some(tail) = self.chats.tail.as_ref()
        {
            for pane_id in stop {
                tail.stop(&pane_id);
                self.chats.followed.remove(&pane_id);
            }
            for agent in follow {
                (self.chats.followed).insert(agent.pane_id.clone(), agent.agent_session.clone());
                tail.follow(agent);
            }
        }
        // When each agent's turn began, as this round of herdr's sees it.
        let now = crate::ai_work::now_ms();
        let agents = &self.session.agents;
        let turns = &mut self.chats.turns;
        turns.retain(|id, _| agents.iter().any(|a| a.pane_id == *id));
        (self.chats.sides).retain(|id, _| agents.iter().any(|a| a.pane_id == *id));
        for agent in agents {
            let turn = turns.entry(agent.pane_id.clone()).or_default();
            turn.note(agent.agent_status, now);
        }
        // Side questions asked before `/clear` or `/new` go with that session.
        let sessions: Vec<(String, Option<AgentSession>)> = (self.session.agents.iter())
            .map(|a| (a.pane_id.clone(), self.with_live_session(a).agent_session))
            .collect();
        let mut left = Vec::new();
        for (pane_id, sides) in &mut self.chats.sides {
            let now = (sessions.iter()).find(|(id, _)| id == pane_id);
            if btw::drop_left(sides, now.and_then(|(_, s)| s.as_ref())) {
                left.push(pane_id.clone());
            }
        }
        for pane_id in left {
            self.chat_put_sides(&pane_id);
        }
        // What is kept of chats gone with their panes.
        let kept: HashSet<String> = chats_mut(&mut self.workspaces)
            .map(|(id, _)| id.to_owned())
            .collect();
        self.chats.scrolls.retain(|id, _| kept.contains(id));
        self.chats.read_at.retain(|id, _| kept.contains(id));
        let agents = &self.session.agents;
        (self.chats.modes).retain(|id, _| agents.iter().any(|a| a.pane_id == *id));
        self.chats_read_rows(&wanted);
        self.live_show_held();
        self.live_release_hidden();
    }

    /// Start the thread following the chats, the first time one shows;
    /// whether it runs.
    fn start_chat_tail(&mut self) -> bool {
        if self.chats.tail.is_none() {
            let (tx, rx) = channel::channel::<Event>();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg(event) = event {
                    state.chat_event(event);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent chats: {err}");
                self.show_error("Couldn't start reading the agents' chats".into());
                return false;
            }
            let socket = (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf());
            self.chats.tail = Some(Tail::spawn(tx, socket));
        }
        true
    }

    /// What the tail read of a pane's chat, or why it couldn't.
    fn chat_event(&mut self, event: Event) {
        // The chat says it too, in its place (checklist E7).
        if let Event::Failed { pane_id, error } = &event {
            tracing::warn!("the chat of pane {pane_id}: {error}");
        }
        let items_of = match &event {
            Event::Items { pane_id, .. } => Some(pane_id.clone()),
            _ => None,
        };
        let place = items_of.as_deref().and_then(|p| self.chat_place_of(p));
        // A pane closed meanwhile: nothing waits for it.
        take_event(chats_mut(&mut self.workspaces), event);
        if let Some(pane_id) = &items_of {
            self.chat_put_sides(pane_id);
        }
        if let (Some(pane_id), Some(place)) = (items_of, place) {
            self.chat_keep_place(&pane_id, place);
        }
    }

    /// Where pane `pane_id`'s chat stands, to stay there as earlier
    /// messages come in above.
    fn chat_place_of(&self, pane_id: &str) -> Option<Place> {
        let (_, _, view) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)?;
        let first = view.items.first()?.id().to_owned();
        let (y, _) = chrome::chat_item_span(&view, 0)?;
        Some(Place {
            first,
            y,
            top: view.top,
        })
    }

    /// Earlier messages came in above pane `pane_id`'s chat, which stood
    /// at `place`: the same messages stay in view, and the same one picked.
    fn chat_keep_place(&mut self, pane_id: &str, place: Place) {
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)
        else {
            return;
        };
        let Some(i) = (view.items.iter()).position(|item| item.id() == place.first) else {
            return;
        };
        let shown = rows(&view.items, &view.unfolded);
        let Some(r) = shown.iter().position(|row| row.holds(i)) else {
            return;
        };
        let Some((y, _)) = chrome::chat_item_span(&view, r).filter(|(y, _)| *y != place.y) else {
            return;
        };
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        chat.picked = chat
            .picked
            .map(|p| (p + r).min(shown.len().saturating_sub(1)));
        if !chat.stick {
            self.chat_scroll_to(pane_id, Some(place.top + f64::from(y - place.y)));
        }
    }

    /// Ask for the messages before those read of pane `pane_id`'s chat,
    /// once until they come.
    fn chat_earlier(&mut self, pane_id: &str) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        if !chat.earlier || chat.reading_earlier {
            return;
        }
        if let Some(tail) = self.chats.tail.as_ref() {
            chat.reading_earlier = true;
            tail.earlier(pane_id);
        }
    }

    /// Read the screens of the chats in `wanted` whose agent waits on the
    /// user, once a `WAITING_EVERY` at most; the others show none. Those of
    /// idle Claude Codes are read too, until their suggestion shows or
    /// `SUGGEST_READS` found none; it goes as the agent works again.
    fn chats_read_rows(&mut self, wanted: &[AgentInfo]) {
        let blocked: HashSet<&str> = (wanted.iter())
            .filter(|a| a.agent_status == AgentStatus::Blocked)
            .map(|a| a.pane_id.as_str())
            .collect();
        let idle: HashSet<&str> = (wanted.iter())
            .filter(|a| suggests(a))
            .map(|a| a.pane_id.as_str())
            .collect();
        let mut suggesting = Vec::new();
        for (pane_id, chat) in chats_mut(&mut self.workspaces) {
            if !blocked.contains(pane_id) {
                chat.waiting = None;
            }
            if !idle.contains(pane_id) {
                (chat.suggestion, chat.suggest_reads) = (None, 0);
            } else if chat.suggestion.is_none() && chat.suggest_reads < SUGGEST_READS {
                suggesting.push(pane_id.to_owned());
            }
        }
        self.chats
            .waiting
            .retain(|id, _| blocked.contains(id.as_str()));
        // The others with modes, for the one they are in.
        let moded = (wanted.iter())
            .filter(|a| !blocked.contains(a.pane_id.as_str()))
            .filter(|a| super::source::kind(a).is_some_and(|k| !mode::modes(k).is_empty()))
            .map(|a| (a.pane_id.as_str(), MODE_EVERY));
        let due: Vec<String> = (blocked.iter().map(|id| (*id, WAITING_EVERY)))
            .chain(suggesting.iter().map(|id| (id.as_str(), WAITING_EVERY)))
            .chain(moded)
            .filter(|(id, every)| {
                (self.chats.read_at.get(*id)).is_none_or(|at| at.elapsed() >= *every)
            })
            .map(|(id, _)| id.to_owned())
            .collect();
        for pane_id in due {
            self.read_rows(pane_id, Duration::ZERO);
        }
    }

    /// Read pane `pane_id`'s screen rows on a thread of its own, `after`
    /// from now; not while they are being read already.
    fn read_rows(&mut self, pane_id: String, after: Duration) {
        if self.chats.reading.contains(&pane_id) {
            return;
        }
        let Some(socket) = (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf()) else {
            return;
        };
        let Some(tx) = self.rows_sender() else {
            return;
        };
        self.chats.reading.insert(pane_id.clone());
        self.chats.read_at.insert(pane_id.clone(), Instant::now());
        let id = pane_id.clone();
        let spawned = std::thread::Builder::new()
            .name("chat rows".into())
            .spawn(move || {
                std::thread::sleep(after);
                let rows = screen_rows(&mut Herdr::on_socket(socket), &id, WAITING_ROWS);
                // The loop is gone only as gerdr quits.
                let _ = tx.send((id, rows));
            });
        if let Err(err) = spawned {
            tracing::warn!("reading the screen of pane {pane_id}: {err}");
            self.chats.reading.remove(&pane_id);
        }
    }

    /// Where the reads of screen rows come back; set up the first time.
    fn rows_sender(&mut self) -> Option<channel::Sender<(String, Result<ChatRows, String>)>> {
        if self.chats.rows_tx.is_none() {
            let (tx, rx) = channel::channel();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((pane_id, rows)) = event {
                    state.take_rows(pane_id, rows);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent chats' screens: {err}");
                return None;
            }
            self.chats.rows_tx = Some(tx);
        }
        self.chats.rows_tx.clone()
    }

    /// Pane `pane_id`'s screen rows as read, for its waiting panel, for
    /// the suggestion of its idle agent, and for its agent's mode while it
    /// doesn't wait (a menu then covers it).
    fn take_rows(&mut self, pane_id: String, rows: Result<ChatRows, String>) {
        self.chats.reading.remove(&pane_id);
        let Some(agent) = self.chat_agent(&pane_id) else {
            return;
        };
        let (status, kind) = (Some(agent.agent_status), super::source::kind(agent));
        if status != Some(AgentStatus::Blocked) {
            match (kind, &rows) {
                (Some(kind), Ok(rows)) => self.chat_take_mode(&pane_id, kind, rows),
                // Tried again on herdr's next round.
                (_, Err(err)) => tracing::debug!("the mode of pane {pane_id}: {err}"),
                (None, Ok(_)) => {}
            }
        }
        let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
            return;
        };
        if let Some(AgentStatus::Idle | AgentStatus::Done) = status {
            // A failed read counts too, so a pane herdr can't read isn't
            // asked again every second for ever.
            chat.suggest_reads = chat.suggest_reads.saturating_add(1);
            match rows {
                Ok(rows) => chat.suggestion = suggestion(&rows),
                Err(err) => tracing::warn!("the suggestion in pane {pane_id}: {err}"),
            }
            return;
        }
        // Too late: its agent waits no more.
        if status != Some(AgentStatus::Blocked) {
            return;
        }
        match rows {
            Ok(rows) if chat.waiting.as_ref() != Some(&rows) => {
                self.chats.waiting.insert(pane_id, Arc::new(rows.clone()));
                chat.waiting = Some(rows);
            }
            Ok(_) => {}
            // Read again on herdr's next round; a toast a second would
            // drown the rest. The panel keeps the rows it had.
            Err(err) => tracing::warn!("the screen of pane {pane_id}: {err}"),
        }
    }

    /// The focused pane of the active tab while it shows its chat in sight.
    fn focused_chat(&self) -> Option<&Pane> {
        let tab = self.tab()?;
        if tab.review().is_some_and(|review| review.page.is_none()) {
            return None;
        }
        let id = tab.focused_id()?;
        (tab.panes.iter()).find(|p| p.id == id && self.chat_shown(p))
    }

    /// Whether the keys go to the focused pane's chat: it shows one, and
    /// nothing over it takes them. The prefix works as everywhere.
    pub(crate) fn chat_has_keys(&self) -> bool {
        self.overlays.rename.is_none()
            && self.grabbing_overlay().is_none()
            && self.key_overlay().is_none()
            && !self.is_open(crate::overlay::Overlay::Features)
            && self.focused_chat().is_some()
    }

    /// The herdr pane of the focused chat, and the chat.
    fn focused_chat_mut(&mut self) -> Option<(String, &mut PaneChat)> {
        let pane_id = self.focused_chat()?.herdr.as_ref()?.pane_id.clone();
        let chat = chat_mut(&mut self.workspaces, &pane_id)?;
        Some((pane_id, chat))
    }

    /// The field of the focused chat with the keys: its reply box, or its
    /// questions' card's Other.
    pub(crate) fn chat_field_mut(&mut self) -> Option<&mut TextField> {
        let (_, chat) = self.focused_chat_mut()?;
        if chat.focus == ChatFocus::Composer {
            return Some(&mut chat.draft);
        }
        let ask = chat.live.ask.as_ref()?;
        (chat.card.ask == ask.ask && chat.card.on_other(&ask.questions))
            .then(|| chat.card.field())
            .flatten()
    }

    /// Where the focused chat's field with the keys is (`chat_field_mut`).
    pub(crate) fn chat_field_spot(&self) -> Option<FieldSpot> {
        let (_, _, view) = self.chat_places().into_iter().find(|(_, _, v)| v.focused)?;
        chrome::chat_field_spot(&view, self.scale())
    }

    /// A key for the focused chat (see `chat_key`).
    pub(crate) fn chat_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) {
        let Some((pane_id, chat)) = self.focused_chat_mut() else {
            return;
        };
        let picked = chat.picked;
        match chat_key(chat, mods, sym, typed) {
            None => self.chat_show_picked(&pane_id, picked),
            Some(ChatDo::Send) => self.chat_send(&pane_id),
            Some(ChatDo::Answer(reply)) => self.chat_answer(&pane_id, reply, None),
            Some(ChatDo::Answers(answers)) => {
                self.chat_answer(&pane_id, AskReply::Allow, Some(answers))
            }
            Some(ChatDo::Copy(text)) => {
                let lines = text.lines().count().max(1);
                let s = if lines == 1 { "" } else { "s" };
                self.copy_text(text);
                self.show_success(format!("Copied {lines} line{s}"));
            }
            Some(ChatDo::NextMode) => self.chat_next_mode(&pane_id),
            Some(ChatDo::PickModel) => self.chat_open_model_picker(&pane_id),
            Some(ChatDo::Top) => self.chat_scroll_to(&pane_id, Some(0.0)),
            Some(ChatDo::Bottom) => self.chat_scroll_to(&pane_id, None),
            Some(ChatDo::Page(by)) => {
                let page = (self.chat_places().into_iter())
                    .find(|(_, p, _)| *p == pane_id)
                    .map(|(_, _, view)| chrome::chat_layout(&view));
                if let Some(layout) = page {
                    let b = chat_bounds(&layout);
                    let scroll = (self.chats.scrolls.entry(pane_id.clone())).or_default();
                    let to = scroll.pos() + f64::from(by) * b.view;
                    scroll.glide_to(to, Instant::now(), b);
                    if let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) {
                        chat.stick = false;
                    }
                }
            }
        }
        self.chat_slash_sync(&pane_id);
    }

    /// Open, filter or close pane `pane_id`'s `/` menu for its reply.
    fn chat_slash_sync(&mut self, pane_id: &str) {
        let agent = self.session.agents.iter().find(|a| a.pane_id == pane_id);
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        slash_sync(chat, || {
            let kind = agent.and_then(super::source::kind);
            let cwd = agent.and_then(|a| a.cwd.as_deref());
            match (kind, cwd) {
                (Some(kind), Some(cwd)) => commands::here(kind, cwd),
                _ => Vec::new(),
            }
        });
    }

    /// Stand pane `pane_id`'s chat at `top` pixels down, or at its bottom
    /// following new items (`None`).
    fn chat_scroll_to(&mut self, pane_id: &str, top: Option<f64>) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        chat.stick = top.is_none();
        if let Some(top) = top {
            chat.top = top;
            (self.chats.scrolls.entry(pane_id.to_owned()).or_default()).jump(top);
        }
    }

    /// Send the focused chat's reply to its agent, as if typed in its pane.
    /// A reply refused keeps its draft.
    fn chat_send(&mut self, pane_id: &str) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        let line = one_line(&chat.draft);
        if line.is_empty() {
            return;
        }
        match self.send_line(pane_id, &line) {
            Ok(()) => {
                if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
                    chat.draft.clear();
                    chat.stick = true;
                    // Spent: none is read again until the turn it starts.
                    (chat.suggestion, chat.suggest_reads) = (None, SUGGEST_READS);
                }
            }
            Err(why) => {
                tracing::warn!("a reply to the agent in pane {pane_id}: {why}");
                self.show_error(format!("Couldn't send the reply: {why}"));
            }
        }
    }

    /// Send `line` to pane `pane_id`'s agent, from its chat or the phone: a
    /// side question (`/btw …`) to an agent that takes them is followed for
    /// its answer. Nothing goes while its side panel shows: that would take
    /// the keys (Cursor's closes on any).
    pub(crate) fn send_line(&mut self, pane_id: &str, line: &str) -> Result<(), String> {
        let sides = self.chats.sides.get(pane_id).map(Vec::as_slice).unwrap_or_default();
        if (sides.iter()).any(|side| side.state == SideState::Answering) {
            return Err("its agent is still answering a side question".into());
        }
        let offered = (self.session.agents.iter())
            .find(|a| a.pane_id == pane_id)
            .and_then(super::source::kind)
            .is_some_and(super::commands::asks_aside);
        let question = btw::question(line).filter(|_| offered);
        self.send_to_agent(pane_id, line)?;
        if let Some(question) = question {
            self.ask_side(pane_id, question);
        }
        Ok(())
    }

    /// Follow side question `question`, just sent to pane `pane_id`'s agent,
    /// on a thread of its own; its rows show in the chat meanwhile.
    fn ask_side(&mut self, pane_id: &str, question: &str) {
        let mut side = Side::new(question, crate::ai_work::now_ms());
        side.session =
            (self.chat_agent(pane_id)).and_then(|a| self.with_live_session(a).agent_session);
        let list = self.chats.sides.entry(pane_id.to_owned()).or_default();
        list.push(side.clone());
        let n = list.len() - 1;
        let socket = (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf());
        let tx = self.sides_sender();
        let started = match (socket, tx) {
            (Some(socket), Some(tx)) => {
                let id = pane_id.to_owned();
                let follow = Follow::new(side, Instant::now());
                std::thread::Builder::new()
                    .name("side question".into())
                    .spawn(move || follow_side(&mut Herdr::on_socket(socket), &id, n, follow, &tx))
                    .map(drop)
                    .map_err(|err| err.to_string())
            }
            _ => Err("herdr isn't connected".into()),
        };
        if let Err(why) = started {
            tracing::warn!("following a side question in pane {pane_id}: {why}");
            let side = &mut self.chats.sides.get_mut(pane_id).expect("just pushed")[n];
            side.state = SideState::Failed(format!("gerdr couldn't read it: {why}"));
        }
        self.chat_put_sides(pane_id);
    }

    /// Where followed side questions come back; set up the first time.
    fn sides_sender(&mut self) -> Option<channel::Sender<(String, usize, Side)>> {
        if self.chats.sides_tx.is_none() {
            let (tx, rx) = channel::channel();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((pane_id, n, side)) = event {
                    state.take_side(pane_id, n, side);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent side questions: {err}");
                return None;
            }
            self.chats.sides_tx = Some(tx);
        }
        self.chats.sides_tx.clone()
    }

    /// Side question `n` of pane `pane_id` as followed now.
    fn take_side(&mut self, pane_id: String, n: usize, side: Side) {
        // Its chat says why too (checklist E7).
        if let SideState::Failed(why) = &side.state {
            tracing::warn!("the side question in pane {pane_id}: {why}");
        }
        // Too late: its pane's agent is gone.
        let Some(slot) = self.chats.sides.get_mut(&pane_id).and_then(|l| l.get_mut(n)) else {
            return;
        };
        *slot = side;
        self.chat_put_sides(&pane_id);
    }

    /// Pane `pane_id`'s chat with its side questions' rows as they stand.
    fn chat_put_sides(&mut self, pane_id: &str) {
        let sides = self.chats.sides.get(pane_id).map(Vec::as_slice).unwrap_or_default();
        if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
            chat.items = btw::with_sides(&chat.items, sides);
        }
    }

    /// The side questions asked of pane `pane_id`'s agent this run, for
    /// the phone's card: each answer as HTML.
    pub(crate) fn phone_sides(&self, pane_id: &str) -> serde_json::Value {
        let sides = self.chats.sides.get(pane_id).map(Vec::as_slice).unwrap_or_default();
        (sides.iter())
            .map(|side| {
                let html = crate::md_preview::render(&btw::answer_md(&side.lines));
                let mut entry = serde_json::to_value(&side.state).unwrap_or_default();
                entry["question"] = side.question.clone().into();
                entry["html"] = html.into();
                entry
            })
            .collect()
    }

    /// Type `line` into herdr pane `pane_id`, then Enter, if its agent is in
    /// front: never into its shell, where it would run as a command.
    fn send_to_agent(&mut self, pane_id: &str, line: &str) -> Result<(), String> {
        if self.pane_paused(pane_id) {
            return Err("the agent is paused; resume it first".into());
        }
        let herdr = (self.session.herdr.as_mut()).ok_or("herdr isn't connected")?;
        agent_in_front(&herdr.process_info(pane_id))?;
        herdr.send_agent_line(pane_id, line)
    }

    /// The mode pane `pane_id`'s agent of `kind` is in, as its screen
    /// `rows` say; one that doesn't say keeps the last it said.
    fn chat_take_mode(&mut self, pane_id: &str, kind: AgentKind, rows: &ChatRows) {
        let texts: Vec<&str> = rows.iter().map(|(text, _)| text.as_str()).collect();
        if let Some(now) = mode::shown(kind, &texts) {
            self.chats.modes.insert(pane_id.to_owned(), now);
        }
    }

    /// Switch pane `pane_id`'s agent to mode `want`, picked in its chat:
    /// on a thread, a press of its mode key at a time till its screen shows
    /// it (`mode::switch`). One switch per pane at a time.
    fn chat_pick_mode(&mut self, pane_id: &str, want: &str) {
        let Some(kind) = self.chat_agent(pane_id).and_then(super::source::kind) else {
            return;
        };
        if self.chats.switching.contains(pane_id)
            || self.chats.modes.get(pane_id).is_some_and(|now| now == want)
        {
            return;
        }
        if let Err(why) = self.mode_key_goes(pane_id) {
            return self.chat_mode_failed(pane_id, want, &why);
        }
        let (Some(socket), Some(tx)) = (
            (self.session.herdr.as_ref()).map(|h| h.socket().to_path_buf()),
            self.mode_sender(),
        ) else {
            return self.chat_mode_failed(pane_id, want, "herdr isn't connected");
        };
        let was = self.chats.modes.get(pane_id).cloned();
        let (pane, wanted) = (pane_id.to_owned(), want.to_owned());
        let spawned = std::thread::Builder::new()
            .name("chat mode".into())
            .spawn(move || {
                let mut herdr = Herdr::on_socket(socket);
                let mut agent = mode::InPane {
                    herdr: &mut herdr,
                    pane: &pane,
                    kind,
                };
                let n = mode::modes(kind).len();
                let went = mode::switch(&mut agent, n, was.as_deref(), &wanted, mode::READ_EVERY);
                // The loop is gone only as gerdr quits.
                let _ = tx.send((pane, wanted, went));
            });
        match spawned {
            Ok(_) => _ = self.chats.switching.insert(pane_id.to_owned()),
            Err(err) => self.chat_mode_failed(pane_id, want, &err.to_string()),
        }
    }

    /// Where the mode switches come back; set up the first time.
    fn mode_sender(&mut self) -> Option<channel::Sender<crate::ModeSwitched>> {
        if self.chats.mode_tx.is_none() {
            let (tx, rx) = channel::channel();
            let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
                if let channel::Event::Msg((pane_id, want, went)) = event {
                    state.mode_switched(pane_id, want, went);
                }
            });
            if let Err(err) = inserted {
                tracing::warn!("agent chats' mode switches: {err}");
                return None;
            }
            self.chats.mode_tx = Some(tx);
        }
        self.chats.mode_tx.clone()
    }

    /// Pane `pane_id`'s agent was switched to `want`, or `went` says why not.
    fn mode_switched(&mut self, pane_id: String, want: String, went: Result<(), String>) {
        self.chats.switching.remove(&pane_id);
        match went {
            Ok(()) => _ = self.chats.modes.insert(pane_id, want),
            Err(why) => {
                self.chat_mode_failed(&pane_id, &want, &why);
                // What it is in now.
                self.read_rows(pane_id, Duration::ZERO);
            }
        }
    }

    /// Step pane `pane_id`'s agent to its next mode, as its own key does;
    /// nothing for an agent without modes, where the key means another
    /// thing (pi's thinking level), or while one is being switched.
    fn chat_next_mode(&mut self, pane_id: &str) {
        let kind = self.chat_agent(pane_id).and_then(super::source::kind);
        if kind.is_none_or(|kind| mode::modes(kind).is_empty())
            || self.chats.switching.contains(pane_id)
        {
            return;
        }
        let pressed = self.mode_key_goes(pane_id).and_then(|()| {
            let herdr = (self.session.herdr.as_mut()).ok_or("herdr isn't connected")?;
            herdr.send_keys(pane_id, &[mode::NEXT_KEY])
        });
        match pressed {
            Ok(()) => self.read_rows(pane_id.to_owned(), mode::READ_EVERY),
            Err(why) => {
                tracing::warn!("the next mode of the agent in pane {pane_id}: {why}");
                self.show_error(format!("Couldn't switch the agent's mode: {why}"));
            }
        }
    }

    /// Whether the mode key would reach pane `pane_id`'s agent: it is in
    /// front, not paused, and waits on nothing (the key would answer its
    /// menu).
    fn mode_key_goes(&mut self, pane_id: &str) -> Result<(), String> {
        if self.pane_paused(pane_id) {
            return Err("the agent is paused; resume it first".into());
        }
        if (self.chat_agent(pane_id)).is_some_and(|a| a.agent_status == AgentStatus::Blocked) {
            return Err("the agent waits on an answer first".into());
        }
        let herdr = (self.session.herdr.as_mut()).ok_or("herdr isn't connected")?;
        agent_in_front(&herdr.process_info(pane_id))
    }

    /// Open the picker of pane `pane_id`'s agent's model, on the one its
    /// chat says it is on; nothing for an agent gerdr can't switch.
    pub(crate) fn chat_open_model_picker(&mut self, pane_id: &str) {
        let Some(slot) = self.chat_model_slot(pane_id) else {
            return;
        };
        let of = crate::switcher::ModelFor::Chat {
            pane: pane_id.to_owned(),
        };
        self.open_model_picker(of, slot);
    }

    /// The agent of pane `pane_id`'s chat, for the picker of its model;
    /// none for one gerdr can't switch.
    pub(crate) fn chat_model_slot(
        &mut self,
        pane_id: &str,
    ) -> Option<crate::feature_agents::ModelSlot> {
        let kind = (self.chat_agent(pane_id).and_then(super::source::kind))
            .filter(|kind| model::picks(*kind))?;
        let now = chat_mut(&mut self.workspaces, pane_id).and_then(|chat| chat.model.clone());
        let offers = model::offers(kind, &self.features.cursor_models.offers);
        Some(crate::feature_agents::ModelSlot {
            agent: model::name(kind).to_owned(),
            base: model::base(kind).to_owned(),
            picked: now.and_then(|now| model::picked(&offers, &now)),
        })
    }

    /// Switch pane `pane_id`'s agent to model `id`, picked in its chat:
    /// its `/model` line typed into its pane. Its button says `id`'s name
    /// till its chat says what it is on.
    pub(crate) fn chat_pick_model(&mut self, pane_id: &str, id: &str) {
        let Some(kind) = self.chat_agent(pane_id).and_then(super::source::kind) else {
            return;
        };
        let sent = (self.mode_key_goes(pane_id))
            .and_then(|()| self.send_to_agent(pane_id, &model::line(id)));
        if let Err(why) = sent {
            tracing::warn!("switching the agent in pane {pane_id} to model {id}: {why}");
            return self.show_error(format!("Couldn't switch the agent to {id}: {why}"));
        }
        let offers = model::offers(kind, &self.features.cursor_models.offers);
        let name = match id {
            "default" => "Default",
            _ => (offers.iter().find(|o| o.id == id)).map_or(id, |o| o.name.as_str()),
        };
        if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
            chat.model = Some(name.to_owned());
        }
    }

    /// Switching pane `pane_id`'s agent to mode `want` failed for `why`.
    fn chat_mode_failed(&mut self, pane_id: &str, want: &str, why: &str) {
        tracing::warn!("switching the agent in pane {pane_id} to {want}: {why}");
        self.show_error(format!("Couldn't switch the agent to {want}: {why}"));
    }

    /// Answer what pane `pane_id`'s agent asks, from its chat's card, with
    /// `answers` to its questions. One that waits no more (its hook gone,
    /// its time up) just loses its card.
    fn chat_answer(&mut self, pane_id: &str, reply: AskReply, answers: Option<Answers>) {
        let Some(ask) = chat_mut(&mut self.workspaces, pane_id)
            .and_then(|chat| chat.live.ask.as_ref())
            .map(|ask| ask.ask.clone())
        else {
            return;
        };
        match answers {
            Some(answers) => self.live_answers(&ask, answers),
            None => self.live_answer(&ask, reply),
        }
        if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
            chat.live.ask = None;
        }
    }

    /// Press `key` in pane `pane_id`, from its waiting panel's button: a
    /// digit typed as a menu's choice, without Enter (see `choice_keys`).
    fn chat_wait_key(&mut self, pane_id: &str, key: WaitKey) {
        let keys = match key {
            WaitKey::Digit(n) => {
                let m = (self.chats.waiting.get(pane_id)).and_then(|rows| menu::menu(rows));
                choice_keys(m.as_ref(), n)
            }
            key => Some(vec![key]),
        };
        // The user's own answer: written in the reply box, sent by Send.
        let Some(keys) = keys else {
            if let Some(chat) = chat_mut(&mut self.workspaces, pane_id) {
                chat.focus = ChatFocus::Composer;
            }
            return;
        };
        if self.pane_paused(pane_id) {
            return self.show_error("Couldn't press the key: the agent is paused".into());
        }
        let Some(herdr) = self.session.herdr.as_mut() else {
            return self.show_error("Couldn't press the key: herdr isn't connected".into());
        };
        for key in keys {
            let name = wait_key_name(key);
            // A digit goes as a key press: as text, Claude types it into its
            // menu's own-answer row (probed with 2.1.332).
            let sent = herdr.send_keys(pane_id, &[&name]);
            if let Err(err) = sent {
                tracing::warn!("pressing {name} in pane {pane_id}: {err}");
                return self
                    .show_error(format!("Couldn't press {name} in the agent's pane: {err}"));
            }
        }
        // Its screen read again soon, as the menu moves.
        self.chats.read_at.remove(pane_id);
    }

    /// Open the file edit or plan `i` of pane `pane_id`'s chat wrote, in the
    /// workspace's Zed.
    fn chat_open_path(&mut self, pane_id: &str, i: usize) {
        let Some(chat) = chat_mut(&mut self.workspaces, pane_id) else {
            return;
        };
        let shown = rows(&chat.items, &chat.unfolded);
        let path = match shown.get(i).and_then(|row| row.item(&chat.items)) {
            Some(Item::Edit { path, .. } | Item::Plan { path: Some(path), .. }) => path,
            _ => return,
        };
        let mut file = PathBuf::from(path);
        if file.is_relative()
            && let Some(cwd) = self.chat_agent(pane_id).and_then(|a| a.cwd.clone())
        {
            file = cwd.join(file);
        }
        self.open_in_zed(&file, crate::launch::Place::Tab, "the file");
    }

    /// Whether herdr pane `pane_id`'s chat can be seen: painted in the active
    /// tab, and no panel or dialog over it (the feature panel, the
    /// repositories, the inbox…). An ask waits on its card only then.
    pub(crate) fn chat_in_sight(&self, pane_id: &str) -> bool {
        (self.chat_places().iter())
            .find(|(_, pane, _)| pane == pane_id)
            .is_some_and(|(_, _, view)| {
                let r = view.rect.to_f64();
                let middle = Point::from((r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0));
                !covers_chat(self.overlay_at(middle))
            })
    }

    /// The active tab's panes shown as chats: each pane's id, its herdr
    /// pane's, and what its chat is painted from.
    /// How long agent pane `pane_id`'s turn has run, in seconds, while it
    /// works as its chat would say: not paused, not asking. Its chat's
    /// words date the turn when it shows one. For the phone's working row.
    pub(crate) fn turn_took(&self, pane_id: &str) -> Option<u64> {
        let agent = (self.session.agents.iter()).find(|a| a.pane_id == pane_id)?;
        let asks = (self.chats.live.asks.values()).any(|held| held.pane == pane_id);
        if agent.agent_status != AgentStatus::Working || asks || self.pane_paused(pane_id) {
            return None;
        }
        let items = (self.workspaces.iter().flat_map(|ws| &ws.tabs))
            .flat_map(|tab| &tab.panes)
            .filter(|p| p.herdr.as_ref().is_some_and(|h| h.pane_id == pane_id))
            .find_map(|p| p.chat.as_deref())
            .map_or(&[][..], |chat| &chat.items[..]);
        let since = self.chats.turns.get(pane_id)?.since(items)?;
        Some(crate::ai_work::now_ms().saturating_sub(since) / 1000)
    }

    fn chat_places(&self) -> Vec<(u32, String, ChatView)> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        if tab.review().is_some_and(|review| review.page.is_none()) {
            return Vec::new();
        }
        let focused = tab.focused_id().filter(|_| self.chat_has_keys());
        let pointer = Some(self.mouse.pos).filter(|pos| self.overlay_at(*pos).is_none());
        (tab.shown_rects(self.content_rect()).into_iter())
            .filter_map(|(id, rect)| {
                let pane = (tab.panes.iter()).find(|p| p.id == id && self.chat_shown(p))?;
                let pane_id = &pane.herdr.as_ref()?.pane_id;
                let agent = self.chat_agent(pane_id)?;
                let chat = pane.chat.as_ref()?;
                // The agent, its title, then its folder (cut first when
                // narrow); herdr named no chat when it was found by folder
                // and time: it may be the wrong one.
                let mut label = agent.label().to_owned();
                if let Some(title) = agent.shown_title() {
                    label = format!("{label} · {title}");
                }
                if let Some(name) = (agent.cwd.as_deref())
                    .and_then(|cwd| cwd.file_name())
                    .and_then(|name| name.to_str())
                {
                    label = format!("{label} · {name}");
                }
                if chat.source.as_ref().is_some_and(|source| source.guessed) {
                    label.push_str(" · found by folder and time");
                }
                // Its model's button: what its chat says it is on.
                let model = (super::source::kind(agent))
                    .filter(|kind| model::picks(*kind))
                    .map(|_| chat.model.clone().unwrap_or_else(|| "Model".to_owned()));
                let modes = (super::source::kind(agent))
                    .filter(|kind| !mode::modes(*kind).is_empty())
                    .map_or_else(Vec::new, |kind| {
                        mode::offered(kind, self.chats.modes.get(pane_id).map(String::as_str))
                    });
                let mut view = ChatView {
                    rect: tab.window_rect(id, rect),
                    label,
                    cwd: agent.cwd.clone(),
                    status: agent.agent_status,
                    paused: self.pane_paused(pane_id),
                    reading: chat.reading.clone(),
                    items: chat.items.clone(),
                    unfolded: chat.unfolded.clone(),
                    picked: chat.picked,
                    top: chat.top,
                    focus: chat.focus,
                    focused: focused == Some(id),
                    draft: chat.draft.line(),
                    waiting: self.chats.waiting.get(pane_id).cloned(),
                    suggestion: chat.suggestion.clone(),
                    hints: chat_footer(chat, !modes.is_empty(), model.is_some()),
                    slash: (chat.slash.as_ref())
                        .filter(|slash| slash.open())
                        .map(chrome::SlashView::of),
                    earlier: match (chat.earlier, chat.reading_earlier) {
                        (false, _) => None,
                        (true, reading) => Some(reading),
                    },
                    hover: None,
                    running: unread(&chat.items, &chat.live.running),
                    ask: chat.live.ask.clone(),
                    card: (chat.live.ask.as_ref())
                        .filter(|ask| !ask.questions.is_empty())
                        .map(|ask| chat.card.view(ask)),
                    since: (self.chats.turns.get(pane_id)).and_then(|t| t.since(&chat.items)),
                    modes,
                    mode: self.chats.modes.get(pane_id).cloned(),
                    model,
                    context: chat.context.map(|c| c.said()),
                };
                view.hover = pointer
                    .filter(|pos| view.rect.to_f64().contains(*pos))
                    .and_then(|pos| chrome::chat_layout(&view).hit(&view, pos));
                Some((id, pane_id.clone(), view))
            })
            .collect()
    }

    /// What the chats of the active tab are painted from.
    pub(crate) fn chat_views(&self) -> Vec<ChatView> {
        (self.chat_places().into_iter())
            .map(|(_, _, view)| view)
            .collect()
    }

    /// The chat at `pos`, if one is painted there.
    fn chat_place_at(&self, pos: Point<f64, Logical>) -> Option<(u32, String, ChatView)> {
        (self.chat_places().into_iter()).find(|(_, _, view)| view.rect.to_f64().contains(pos))
    }

    /// The tooltip of a chat's part under `pos`: its button to the end's.
    pub(crate) fn chat_tip(&self, pos: Point<f64, Logical>) -> Option<crate::tooltip::Tooltip> {
        let (_, _, view) = self.chat_place_at(pos)?;
        if let Some((i, rect)) = chrome::chat_mode_at(&view, pos) {
            let tip = format!("Switch the agent to {}, or to its next mode with", view.modes[i]);
            return Some(crate::tooltip::Tooltip::new(&tip, rect).key(MODE_KEYS[0].chord));
        }
        if let Some(bar) = chrome::chat_pinned(&view).filter(|bar| bar.to_f64().contains(pos)) {
            return Some(crate::tooltip::Tooltip::new("Go to your message", bar));
        }
        if let Some(rect) = chrome::chat_model_at(&view, pos) {
            let tip = "Pick the model the agent runs on";
            return Some(crate::tooltip::Tooltip::new(tip, rect).key(MODEL_KEYS[0].chord));
        }
        let button = chrome::chat_bottom_button(&view)?;
        (button.to_f64().contains(pos))
            .then(|| crate::tooltip::Tooltip::new("Go to the end", button).key("Shift+G"))
    }

    /// `button` went down at `pos` over a chat: its pane takes the focus, and
    /// a left press (only: checklist O2) does what is under it. Whether a
    /// chat was there. A press in the reply box gives it the keys; the caret
    /// is then placed by `field` (the text fields' own press).
    pub(crate) fn chat_press(
        &mut self,
        pos: Point<f64, Logical>,
        button: u32,
        field: impl FnOnce(&mut Self),
    ) -> bool {
        let Some((id, pane_id, view)) = self.chat_place_at(pos) else {
            return false;
        };
        self.focus_pane(|_| Some(id));
        if button != crate::input::BTN_LEFT {
            return true;
        }
        // A key hint does what its key does (checklist K3).
        if let Some(i) = chrome::chat_hint_at(&view, pos) {
            let keys = view.hints[i].0.clone();
            self.press_hint(crate::input::Overlay::Chat, &keys, pos);
            return true;
        }
        let Some(hit) = chrome::chat_layout(&view).hit(&view, pos) else {
            return true;
        };
        let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
            return true;
        };
        match hit {
            ChatHit::Item(i) => {
                chat.picked = Some(i);
                chat.focus = ChatFocus::List;
            }
            ChatHit::Fold(i) => {
                chat.picked = Some(i);
                let row = rows(&chat.items, &chat.unfolded).get(i).cloned();
                if let Some(id) = row.and_then(|row| row.fold_key(&chat.items))
                    && !chat.unfolded.remove(&id)
                {
                    chat.unfolded.insert(id);
                }
            }
            ChatHit::ShowAll(i) => {
                chat.picked = Some(i);
                let shown = rows(&chat.items, &chat.unfolded);
                if let Some(id) = shown.get(i).and_then(|row| row.fold_key(&chat.items)) {
                    chat.unfolded.insert(chrome::whole_key(&id));
                    chat.unfolded.insert(id);
                }
            }
            ChatHit::Path(i) => self.chat_open_path(&pane_id, i),
            ChatHit::Earlier => self.chat_earlier(&pane_id),
            ChatHit::Key(key) => self.chat_wait_key(&pane_id, key),
            ChatHit::Composer => {
                chat.focus = ChatFocus::Composer;
                field(self);
            }
            ChatHit::Command(i) => {
                let name = (chat.slash.as_ref())
                    .and_then(|slash| slash.shown.get(i).map(|c| slash.all[*c].name.clone()));
                if let Some(name) = name {
                    chat.draft.set(&format!("/{name} "));
                    chat.focus = ChatFocus::Composer;
                    chat.slash = None;
                }
            }
            ChatHit::Send => self.chat_send(&pane_id),
            ChatHit::Bottom => self.chat_scroll_to(&pane_id, None),
            ChatHit::Pinned(i) => {
                chat.picked = Some(i);
                chat.focus = ChatFocus::List;
                let to = chrome::chat_item_span(&view, i).map_or(0, |(y, _)| y);
                self.chat_scroll_to(&pane_id, Some(f64::from(to)));
            }
            ChatHit::Mode(i) => {
                if let Some(want) = view.modes.get(i) {
                    self.chat_pick_mode(&pane_id, want);
                }
            }
            ChatHit::Model => self.chat_open_model_picker(&pane_id),
            ChatHit::Answer(reply) => self.chat_answer(&pane_id, reply, None),
            ChatHit::Card(hit) => {
                let answers = card_press(chat, hit);
                if hit == CardHit::Other {
                    field(self);
                } else if let Some(ChatDo::Answers(answers)) = answers {
                    self.chat_answer(&pane_id, AskReply::Allow, Some(answers));
                }
            }
            ChatHit::AskFold => {
                if let Some(ask) = &chat.live.ask {
                    let key = chrome::ask_key(&ask.ask);
                    if !chat.unfolded.remove(&key) {
                        chat.unfolded.insert(key);
                    }
                }
            }
            ChatHit::AskPlan => {
                if let Some(i) = show_plan(chat) {
                    let to = chrome::chat_item_span(&view, i).map_or(0, |(y, _)| y);
                    self.chat_scroll_to(&pane_id, Some(f64::from(to)));
                }
            }
            ChatHit::ShowScreen => self.toggle_pane_chat(id),
            ChatHit::Scrollbar => {
                self.chats.bar_drag = Some(pane_id);
                self.chat_drag(pos);
            }
        }
        true
    }

    /// The pointer moved to `pos` holding a chat's scrollbar: its list
    /// stands where the pointer is along it.
    pub(crate) fn chat_drag(&mut self, pos: Point<f64, Logical>) {
        let Some(pane_id) = self.chats.bar_drag.clone() else {
            return;
        };
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| *p == pane_id)
        else {
            self.chats.bar_drag = None;
            return;
        };
        let layout = chrome::chat_layout(&view);
        let b = chat_bounds(&layout);
        let list = layout.list;
        let along = (pos.y - f64::from(list.loc.y)) / f64::from(list.size.h.max(1));
        let top = along.clamp(0.0, 1.0) * b.max;
        self.chat_scroll_to(&pane_id, (top < b.max).then_some(top));
    }

    /// The list of pane `pane_id`'s chat scrolled for its picked item to
    /// show, when the keys moved it from `was`.
    fn chat_show_picked(&mut self, pane_id: &str, was: Option<usize>) {
        let Some((_, _, view)) = (self.chat_places().into_iter()).find(|(_, p, _)| p == pane_id)
        else {
            return;
        };
        let Some(i) = view.picked.filter(|_| view.picked != was) else {
            return;
        };
        let Some((y, h)) = chrome::chat_item_span(&view, i) else {
            return;
        };
        let (y, h, shown) = (
            f64::from(y),
            f64::from(h),
            f64::from(chrome::chat_layout(&view).list.size.h),
        );
        let to = match () {
            _ if y < view.top => y,
            _ if y + h > view.top + shown => (y + h - shown).min(y),
            _ => return,
        };
        self.chat_scroll_to(pane_id, Some(to));
    }

    /// The wheel or fingers moved over a chat at `pos`: its list moves, and
    /// stops following new items once moved up. Whether a chat was there.
    pub(crate) fn chat_scroll(&mut self, pos: Point<f64, Logical>, motion: Option<Motion>) -> bool {
        let Some((_, pane_id, view)) = self.chat_place_at(pos) else {
            return false;
        };
        let Some(motion) = motion else {
            return true;
        };
        let b = chat_bounds(&chrome::chat_layout(&view));
        let scroll =
            (self.chats.scrolls.entry(pane_id.clone())).or_insert_with(|| Scroll::at(view.top));
        scroll.apply(motion, Instant::now(), b);
        let up = match motion {
            Motion::Wheel(n) | Motion::Notches(n) => n < 0.0,
            Motion::Finger { px, .. } => px < 0.0,
            Motion::Lift { .. } => false,
        };
        if up && let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) {
            chat.stick = false;
        }
        true
    }

    /// Move each chat shown on to `now`: one following new items stands at
    /// its bottom; one scrolled back to its bottom follows them again.
    pub(crate) fn chat_scroll_tick(&mut self, now: Instant) {
        let mut at_top = Vec::new();
        for (_, pane_id, view) in self.chat_places() {
            let b = chat_bounds(&chrome::chat_layout(&view));
            let scroll =
                (self.chats.scrolls.entry(pane_id.clone())).or_insert_with(|| Scroll::at(view.top));
            let Some(chat) = chat_mut(&mut self.workspaces, &pane_id) else {
                continue;
            };
            if chat.stick && !scroll.moving(now) {
                scroll.jump(b.max);
            }
            let moving = scroll.tick(now, b);
            if !moving && b.max > 0.0 && scroll.pos() >= b.max - 0.5 {
                chat.stick = true;
            }
            chat.top = scroll.pos().clamp(0.0, b.max);
            // Scrolled up to the first message read: read the ones before.
            if !chat.stick && chat.top < 1.0 && chat.earlier && !chat.reading_earlier {
                at_top.push(pane_id);
            }
        }
        for pane_id in at_top {
            self.chat_earlier(&pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_over_a_chat_hides_it_but_toasts_and_help_dont() {
        use crate::overlay::Overlay;
        assert!(!covers_chat(None));
        assert!(covers_chat(Some(Overlay::Features)));
        assert!(covers_chat(Some(Overlay::Repos)));
        assert!(covers_chat(Some(Overlay::Confirm)));
        assert!(!covers_chat(Some(Overlay::Toasts)));
        assert!(!covers_chat(Some(Overlay::Help)));
        assert!(!covers_chat(Some(Overlay::Tooltip)));
    }
    use crate::chat::{DiffLine, DiffSide, Source, ToolOut};
    use crate::herdr::Process;

    fn agent(pane: &str, session: Option<&str>) -> AgentInfo {
        let session = session.map(|id| {
            format!(r#","agent_session":{{"source":"herdr:claude","agent":"claude","kind":"id","value":"{id}"}}"#)
        });
        serde_json::from_str(&format!(
            r#"{{"pane_id":"{pane}","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t1","agent":"claude","display_agent":null,"name":null,"cwd":null,"foreground_cwd":null,"title":null,"terminal_title":null,"terminal_title_stripped":null{}}}"#,
            session.unwrap_or_default()
        ))
        .unwrap()
    }

    /// A key as input sends it: `sym` the key's own, `typed` what Shift made
    /// of it (checklist K8).
    fn press(chat: &mut PaneChat, keys: &str, typed: char) -> Option<ChatDo> {
        let (mods, sym) = crate::keys::hint_key(keys).unwrap();
        chat_key(chat, &mods, sym, Keysym::from_char(typed))
    }

    fn user(id: &str, text: &str) -> Item {
        Item::User {
            id: id.into(),
            at: None,
            text: text.into(),
        }
    }

    fn with_items(items: Vec<Item>) -> PaneChat {
        let mut chat = *PaneChat::opened();
        chat.items = Arc::new(items);
        chat
    }

    #[test]
    fn the_first_toggle_shows_the_chat_and_the_next_flip_keeps_it() {
        let mut chat = None;
        flip(&mut chat);
        let shown = chat.as_mut().unwrap();
        assert!(shown.shown && shown.stick);
        assert_eq!(shown.focus, ChatFocus::List);
        shown.draft.set("half a reply");
        shown.unfolded.insert("t1".into());
        shown.top = 120.0;
        flip(&mut chat);
        let hidden = chat.as_ref().unwrap();
        assert!(!hidden.shown);
        assert_eq!(hidden.draft, "half a reply");
        assert!(hidden.unfolded.contains("t1"));
        assert_eq!(hidden.top, 120.0);
        flip(&mut chat);
        assert!(chat.unwrap().shown);
    }

    #[test]
    fn shown_chats_are_followed_hidden_and_closed_ones_stopped() {
        let followed = HashMap::new();
        let wanted = [agent("w1:p1", Some("a"))];
        let (follow, stop) = follow_plan(&followed, &wanted);
        assert_eq!(follow.len(), 1);
        assert!(stop.is_empty());
        let followed = HashMap::from([("w1:p1".to_owned(), wanted[0].agent_session.clone())]);
        assert_eq!(follow_plan(&followed, &wanted), (vec![], vec![]));
        // Another session in the pane: found and read afresh.
        let again = [agent("w1:p1", Some("b"))];
        assert_eq!(follow_plan(&followed, &again).0.len(), 1);
        // Closed, hidden or out of sight: no longer wanted.
        assert_eq!(
            follow_plan(&followed, &[]),
            (vec![], vec!["w1:p1".to_owned()])
        );
    }

    #[test]
    fn what_the_tail_reads_goes_to_its_own_pane() {
        let (mut one, mut two) = (*PaneChat::opened(), *PaneChat::opened());
        one.picked = Some(4);
        let source = Source {
            agent: crate::ai_work::AgentKind::Claude,
            id: "abc".into(),
            path: "/x.jsonl".into(),
            guessed: false,
        };
        let event = Event::Items {
            pane_id: "w1:p2".into(),
            source: source.clone(),
            items: Arc::new(vec![user("u1", "hi")]),
            earlier: true,
            model: Some("Opus 5.5".into()),
            context: Some(crate::ai_work::context::Context {
                used: 84_000,
                window: Some(1_000_000),
            }),
        };
        assert!(take_event(
            [("w1:p1", &mut one), ("w1:p2", &mut two)],
            event
        ));
        assert_eq!(two.reading, Reading::Read);
        assert_eq!(two.items.len(), 1);
        assert!(two.earlier && !two.reading_earlier);
        assert_eq!(two.model.as_deref(), Some("Opus 5.5"));
        assert_eq!(two.source, Some(source));
        assert_eq!(one.reading, Reading::Finding);
        // Another agent in the pane, its chat not written yet: the last
        // one's model goes.
        one.model = Some("Haiku 5.5".into());
        let missing = Event::Missing {
            pane_id: "w1:p1".into(),
            why: "no chat".into(),
        };
        assert!(take_event([("w1:p1", &mut one)], missing));
        assert_eq!(one.reading, Reading::Missing("no chat".into()));
        assert_eq!(one.model, None);
        assert_eq!(two.context.map(|c| c.said()).as_deref(), Some("context 84k / 1M"));
        let gone = Event::Failed {
            pane_id: "w1:p9".into(),
            error: "x".into(),
        };
        assert!(!take_event([("w1:p1", &mut one)], gone));
    }

    #[test]
    fn list_keys_move_fold_copy_and_type_into_the_reply() {
        let mut chat = with_items(vec![user("u1", "one"), user("u2", "two")]);
        assert_eq!(press(&mut chat, "K", 'k'), None);
        assert_eq!(chat.picked, Some(1));
        press(&mut chat, "Ctrl+K", 'k');
        assert_eq!(chat.picked, Some(0));
        press(&mut chat, "J", 'j');
        assert_eq!(chat.picked, Some(1));
        press(&mut chat, "Space", ' ');
        assert!(chat.unfolded.contains("u2"));
        press(&mut chat, "Enter", '\r');
        assert!(chat.unfolded.is_empty());
        assert_eq!(press(&mut chat, "G", 'g'), Some(ChatDo::Top));
        assert_eq!(chat.picked, Some(0));
        // Shift+G comes as g, Shift held, G typed.
        assert_eq!(press(&mut chat, "Shift+G", 'G'), Some(ChatDo::Bottom));
        assert_eq!(press(&mut chat, "Y", 'y'), Some(ChatDo::Copy("two".into())));
        assert_eq!(chat.focus, ChatFocus::List);
        // A character goes to the reply, typed into it; Shift+J too.
        press(&mut chat, "Shift+J", 'J');
        assert_eq!(chat.focus, ChatFocus::Composer);
        assert_eq!(chat.draft, "J");
    }

    #[test]
    fn the_reply_sends_on_enter_and_esc_keeps_its_draft() {
        let mut chat = with_items(Vec::new());
        press(&mut chat, "I", 'i');
        assert_eq!(chat.focus, ChatFocus::Composer);
        // In the reply, list keys are letters.
        press(&mut chat, "J", 'j');
        press(&mut chat, "K", 'k');
        assert_eq!(chat.draft, "jk");
        press(&mut chat, "Shift+Enter", '\r');
        assert_eq!(chat.draft, "jk\n");
        press(&mut chat, "Esc", '\u{1b}');
        assert_eq!(chat.focus, ChatFocus::List);
        assert_eq!(chat.draft, "jk\n");
        press(&mut chat, "I", 'i');
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
        press(&mut chat, "Ctrl+Shift+Del", '\u{7f}');
        assert_eq!(chat.draft, "");
        assert_eq!(one_line("fix it\n  and test\n"), "fix it and test");
    }

    /// Type `text` into `chat`, the `/` menu kept up after each key as
    /// gerdr does, from `all`.
    fn type_in(chat: &mut PaneChat, text: &str, all: &[&str]) {
        for c in text.chars() {
            let sym = Keysym::from_char(c);
            chat_key(chat, &ModifiersState::default(), sym, sym);
            sync(chat, all);
        }
    }

    fn sync(chat: &mut PaneChat, all: &[&str]) {
        let cmd = |name: &&str| Command {
            name: (*name).into(),
            about: String::new(),
            hint: String::new(),
        };
        slash_sync(chat, || all.iter().map(cmd).collect());
    }

    #[test]
    fn a_slash_offers_the_agents_commands_to_pick() {
        let all = ["clear", "compact", "config"];
        let mut chat = with_items(Vec::new());
        type_in(&mut chat, "/co", &all);
        assert_eq!(chat.focus, ChatFocus::Composer);
        let slash = chat.slash.as_ref().unwrap();
        assert!(slash.open());
        assert_eq!(slash.shown.len(), 2);
        assert_eq!(footer_of(&chat)[0].1, "run");
        // Down picks the next; Tab fills it in and closes the menu.
        press(&mut chat, "Down", '\0');
        let picked = chat.slash.as_ref().unwrap().picked().unwrap().name.clone();
        press(&mut chat, "Tab", '\t');
        sync(&mut chat, &all);
        assert_eq!(chat.draft, &*format!("/{picked} "));
        assert!(chat.slash.is_none());

        // Esc closes the menu and keeps the reply, then goes to the list.
        press(&mut chat, "Ctrl+Shift+Del", '\u{7f}');
        sync(&mut chat, &all);
        type_in(&mut chat, "/cl", &all);
        press(&mut chat, "Esc", '\u{1b}');
        sync(&mut chat, &all);
        assert!(!chat.slash.as_ref().unwrap().open());
        assert_eq!((&*chat.draft, chat.focus), ("/cl", ChatFocus::Composer));
        // Typing on opens it again; Enter sends the command picked.
        type_in(&mut chat, "e", &all);
        assert!(chat.slash.as_ref().unwrap().open());
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
        assert_eq!(chat.draft, "/clear");
        press(&mut chat, "Esc", '\u{1b}');
        press(&mut chat, "Esc", '\u{1b}');
        assert_eq!(chat.focus, ChatFocus::List);
        sync(&mut chat, &all);
        assert!(chat.slash.is_none());

        // No command matches: Enter sends what was typed.
        press(&mut chat, "I", 'i');
        press(&mut chat, "Ctrl+Shift+Del", '\u{7f}');
        type_in(&mut chat, "/xyz", &all);
        assert!(!chat.slash.as_ref().unwrap().open());
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
        assert_eq!(chat.draft, "/xyz");
    }

    fn footer_of(chat: &PaneChat) -> Vec<(String, String)> {
        match chat.slash.as_ref().is_some_and(|s| s.open()) {
            true => slash_footer(),
            false => footer(
                chat.focus,
                chat.draft.is_empty(),
                chat.suggestion.is_some(),
                false,
                false,
            ),
        }
    }

    #[test]
    fn a_reply_never_goes_to_a_shell() {
        let process = |pid: i32, name: &str| Process {
            pid,
            name: name.into(),
            argv: vec![name.into()],
            cmdline: name.into(),
            cwd: None,
        };
        let at_prompt = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(10),
            foreground_processes: vec![process(10, "zsh")],
        };
        assert!(agent_in_front(&Ok(at_prompt)).is_err());
        let agent = ProcessInfo {
            shell_pid: Some(10),
            foreground_process_group_id: Some(20),
            foreground_processes: vec![process(20, "claude")],
        };
        assert_eq!(agent_in_front(&Ok(agent)), Ok(()));
        assert!(agent_in_front(&Err("no server".into())).is_err());
    }

    #[test]
    fn a_copied_item_says_what_it_holds() {
        let tool = Item::Tool {
            id: "t".into(),
            name: "Bash".into(),
            gist: "ls".into(),
            input: "ls -la".into(),
            output: Some(ToolOut {
                text: "a\nb".into(),
                failed: false,
                ms: None,
            }),
        };
        assert_eq!(item_text(&tool), "Bash\nls -la\n\na\nb");
        let edit = Item::Edit {
            id: "e".into(),
            path: "src/a.rs".into(),
            diff: vec![
                DiffLine {
                    side: DiffSide::Removed,
                    text: "old".into(),
                },
                DiffLine {
                    side: DiffSide::Added,
                    text: "new".into(),
                },
            ],
            output: None,
        };
        assert_eq!(item_text(&edit), "src/a.rs\n-old\n+new");
        let said = Item::Assistant {
            id: "a".into(),
            at: None,
            blocks: Arc::new(crate::md_blocks::blocks("# Done\n\nIt **works**.\n\n- one")),
        };
        assert_eq!(item_text(&said), "# Done\n\nIt works.\n\n- one");
    }

    #[test]
    fn the_waiting_panel_shows_the_last_rows_but_empty_ones() {
        let rows = |texts: &[&str]| -> ChatRows {
            texts
                .iter()
                .map(|t| ((*t).to_owned(), Vec::new()))
                .collect()
        };
        let got = last_rows(rows(&["a", "b", "c", "", " "]), 2);
        assert_eq!(got, rows(&["b", "c"]));
        assert_eq!(wait_key_name(WaitKey::Esc), "esc");
        assert_eq!(wait_key_name(WaitKey::Digit(2)), "2");
    }

    #[test]
    fn shift_tab_steps_the_mode_from_the_list_and_the_reply_box() {
        let mut chat = with_items(vec![user("u1", "one")]);
        assert_eq!(press(&mut chat, "Shift+Tab", '\t'), Some(ChatDo::NextMode));
        chat.focus = ChatFocus::Composer;
        chat.draft.set("/co");
        assert_eq!(press(&mut chat, "Shift+Tab", '\t'), Some(ChatDo::NextMode));
        assert_eq!(chat.draft, "/co");
        // Hinted only where the agent has modes.
        let has = |modes| {
            footer(ChatFocus::List, true, false, modes, false)
                .iter()
                .any(|(k, _)| k == "Shift+Tab")
        };
        assert!(has(true) && !has(false));
        // It means nothing else in the chat (K1).
        let others: Vec<&str> = (LIST_KEYS.iter().map(|b| b.chord))
            .chain(COMPOSER_KEYS.iter().map(|b| b.chord))
            .chain(SLASH_KEYS.iter().map(|b| b.chord))
            .chain(ASK_KEYS.iter().map(|b| b.chord))
            .collect();
        assert!(!others.contains(&"Shift+Tab"));
    }

    #[test]
    fn alt_shift_a_picks_the_model_from_the_list_and_the_reply_box() {
        let mut chat = with_items(vec![user("u1", "one")]);
        assert_eq!(
            press(&mut chat, "Alt+Shift+A", 'A'),
            Some(ChatDo::PickModel)
        );
        chat.focus = ChatFocus::Composer;
        chat.draft.set("hi");
        assert_eq!(
            press(&mut chat, "Alt+Shift+A", 'A'),
            Some(ChatDo::PickModel)
        );
        assert_eq!(chat.draft, "hi");
        // Hinted only where gerdr can switch the agent's model.
        let has = |models| {
            (footer(ChatFocus::List, true, false, false, models).iter())
                .any(|(k, said)| k == "Alt+Shift+A" && said == "model")
        };
        assert!(has(true) && !has(false));
        let others: Vec<&str> = (LIST_KEYS.iter().map(|b| b.chord))
            .chain(COMPOSER_KEYS.iter().map(|b| b.chord))
            .chain(SLASH_KEYS.iter().map(|b| b.chord))
            .chain(ASK_KEYS.iter().map(|b| b.chord))
            .chain(MODE_KEYS.iter().map(|b| b.chord))
            .collect();
        assert!(!others.contains(&MODEL_KEYS[0].chord));
        assert!(crate::keys::hint_key(MODEL_KEYS[0].chord).is_some());
    }

    #[test]
    fn a_suggestion_is_the_dim_text_of_the_reply_line_only() {
        let read = |ansi: &str| suggestion(&crate::scrollback::parse_ansi(ansi));
        // As Claude Code 2.1.332 drew it under herdr: the chat's own `❯`
        // lines above, then its reply box.
        let rule = "\x1b[0m\x1b[38;2;138;138;138m────\x1b[0m\r\n";
        let screen = format!(
            "❯ Create hello.txt\r\n● Done.\r\n{rule}❯\u{a0}\x1b[0m\x1b[2mcommit this\x1b[0m\r\n{rule}  \x1b[0m\x1b[38;2;150;108;30m⏵⏵ auto mode on\x1b[0m\n"
        );
        assert_eq!(read(&screen).as_deref(), Some("commit this"));
        // Typed by the user: plain. Nothing in the box: none.
        assert_eq!(read("❯\u{a0}com\r\n"), None);
        assert_eq!(read("❯\u{a0}\r\n"), None);
        assert_eq!(read("❯ \x1b[2mcom\x1b[0mmit\r\n"), None);
    }

    #[test]
    fn left_fills_the_empty_reply_with_the_suggestion() {
        let mut chat = with_items(vec![user("u1", "one")]);
        // None waits: Left does nothing and isn't hinted.
        assert_eq!(press(&mut chat, "Left", '\0'), None);
        assert_eq!((&*chat.draft, chat.focus), ("", ChatFocus::List));
        assert!(!footer_of(&chat).iter().any(|(k, _)| k == "Left"));
        chat.suggestion = Some("commit this".into());
        assert!(footer_of(&chat).contains(&("Left".into(), "fill in".into())));
        // From the list it fills the reply and goes there, to send or change.
        press(&mut chat, "Left", '\0');
        assert_eq!((&*chat.draft, chat.focus), ("commit this", ChatFocus::Composer));
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
        // In a reply written, Left moves the caret as ever.
        chat.draft.set("ab");
        press(&mut chat, "Left", '\0');
        chat.draft.key(&ModifiersState::default(), Keysym::x, Keysym::x);
        assert_eq!(chat.draft, "axb");
        assert!(!footer_of(&chat).iter().any(|(k, _)| k == "Left"));
        // The empty reply box takes it too.
        chat.draft.clear();
        press(&mut chat, "Left", '\0');
        assert_eq!(chat.draft, "commit this");
    }

    #[test]
    fn the_chat_keys_agree_with_their_hints() {
        for (chord, _) in key_table::f1_rows(LIST_KEYS)
            .into_iter()
            .chain(key_table::f1_rows(COMPOSER_KEYS))
        {
            assert!(crate::keys::hint_key(chord).is_some(), "{chord}");
        }
    }

    #[test]
    fn show_whole_plan_opens_its_card_in_the_list() {
        let mut chat = with_items(vec![user("u1", "one")]);
        assert_eq!(show_plan(&mut chat), None);
        assert!(chat.unfolded.is_empty());
        let plan = crate::chat::parse::plan_item("p".into(), None, "# Plan\n\nDo it.");
        let mut chat = with_items(vec![user("u1", "one"), plan, user("u2", "two")]);
        chat.live.ask = Some(crate::chat::live::PendingAsk {
            ask: "a1".into(),
            tool: crate::chat::live::PLAN_TOOL.into(),
            gist: String::new(),
            input: "# Plan\n\nDo it.".into(),
            questions: Vec::new(),
            since: Instant::now(),
        });
        chat.focus = ChatFocus::Composer;
        assert_eq!(show_plan(&mut chat), Some(1));
        assert!(chat.unfolded.contains("p"));
        // The panel's peek folds: the card shows the plan.
        assert!(!chrome::ask_open(chat.live.ask.as_ref().unwrap(), &chat.unfolded));
        assert_eq!((chat.picked, chat.focus), (Some(1), ChatFocus::List));
    }

    #[test]
    fn an_asks_keys_answer_it_and_show_what_it_would_run() {
        let mut chat = with_items(vec![user("u1", "one")]);
        // No card: no answer.
        assert_eq!(press(&mut chat, "Ctrl+Enter", '\r'), None);
        chat.live.ask = Some(crate::chat::live::PendingAsk {
            ask: "q1".into(),
            tool: "Bash".into(),
            gist: "ls".into(),
            input: "{}".into(),
            questions: Vec::new(),
            since: Instant::now(),
        });
        let allow = Some(ChatDo::Answer(AskReply::Allow));
        assert_eq!(press(&mut chat, "Ctrl+Enter", '\r'), allow);
        let deny = Some(ChatDo::Answer(AskReply::Deny));
        assert_eq!(press(&mut chat, "Ctrl+Shift+Enter", '\r'), deny);
        // From the reply box too, nothing typed into it.
        chat.focus = ChatFocus::Composer;
        assert_eq!(press(&mut chat, "Ctrl+Enter", '\r'), allow);
        press(&mut chat, "Alt+Shift+I", 'I');
        assert!(chat.unfolded.contains(&chrome::ask_key("q1")));
        press(&mut chat, "Alt+Shift+I", 'I');
        assert!(chat.unfolded.is_empty());
        assert_eq!(chat.draft, "");
        // The footer says them in the buttons' words.
        let hints: Vec<(String, String)> = [
            ("Ctrl+Enter", "allow"),
            ("Ctrl+Shift+Enter", "deny"),
            ("Alt+Shift+I", "show input"),
        ]
        .map(|(k, s)| (k.to_owned(), s.to_owned()))
        .into();
        assert_eq!(ask_footer(false, "input"), hints);
        assert_eq!(ask_footer(true, "input")[2].1, "fold input");
        assert_eq!(ask_footer(true, "plan")[2].1, "fold plan");
        // Their chords parse and mean nothing else in the chat (K1).
        key_table::check(ASK_KEYS, &[()]);
        let others: Vec<&str> = (LIST_KEYS.iter().map(|b| b.chord))
            .chain(COMPOSER_KEYS.iter().map(|b| b.chord))
            .collect();
        assert!(ASK_KEYS.iter().all(|b| !others.contains(&b.chord)));
    }

    /// Two questions: a size, one pick; fruits, several.
    fn questions_ask() -> PendingAsk {
        use crate::chat::live::QuestionOption;
        let q = |text: &str, labels: &[&str], multi| Question {
            question: text.into(),
            header: String::new(),
            options: (labels.iter())
                .map(|l| QuestionOption {
                    label: (*l).into(),
                    description: String::new(),
                })
                .collect(),
            multi,
        };
        PendingAsk {
            ask: "q2".into(),
            tool: "AskUserQuestion".into(),
            gist: String::new(),
            input: String::new(),
            questions: vec![
                q("Size?", &["Small", "Big"], false),
                q("Fruits?", &["Apple", "Pear", "Fig"], true),
            ],
            since: Instant::now(),
        }
    }

    #[test]
    fn a_questions_card_takes_the_keys_till_its_answers_go() {
        let mut chat = with_items(vec![user("u1", "one")]);
        chat.live.ask = Some(questions_ask());
        // Not a permission: Ctrl+Enter allows nothing.
        assert_eq!(press(&mut chat, "Ctrl+Enter", '\r'), None);
        // A digit picks; one pick goes on to the next question.
        assert_eq!(press(&mut chat, "2", '2'), None);
        assert_eq!(chat.card.at, 1);
        // Space ticks the row the keys are on; Down moves; a digit ticks.
        press(&mut chat, "Space", ' ');
        press(&mut chat, "Down", '\0');
        press(&mut chat, "Space", ' ');
        press(&mut chat, "3", '3');
        press(&mut chat, "2", '2');
        assert_eq!(
            chat.card.answer(&questions_ask().questions, 1).as_deref(),
            Some("Apple, Fig")
        );
        // On Other the keys write, digits and Space too; Esc steps back.
        press(&mut chat, "4", '4');
        for (key, c) in [("K", 'k'), ("Space", ' '), ("2", '2')] {
            assert_eq!(press(&mut chat, key, c), None);
        }
        assert_eq!(chat.card.other[1], "k 2");
        assert_eq!(chat.draft, "", "nothing went to the reply box");
        press(&mut chat, "Esc", '\x1b');
        assert_eq!(chat.card.row, 2);
        // Left shows the question before; Enter the next again.
        press(&mut chat, "Left", '\0');
        assert_eq!((chat.card.at, chat.card.row), (0, 1));
        assert_eq!(press(&mut chat, "Enter", '\r'), None);
        assert_eq!(chat.card.at, 1);
        // On the last, Enter submits them all.
        let Some(ChatDo::Answers(answers)) = press(&mut chat, "Enter", '\r') else {
            panic!("the answers");
        };
        assert_eq!(answers["Size?"], "Big");
        assert_eq!(answers["Fruits?"], "Apple, Fig, k 2");
        // Esc off Other gives the questions back to the agent's menu.
        let pass = Some(ChatDo::Answer(AskReply::Pass));
        assert_eq!(press(&mut chat, "Esc", '\x1b'), pass);
        // The reply box keeps its keys: Enter sends, a digit is typed.
        chat.focus = ChatFocus::Composer;
        press(&mut chat, "1", '1');
        assert_eq!(chat.draft, "1");
        assert_eq!(press(&mut chat, "Enter", '\r'), Some(ChatDo::Send));
    }

    #[test]
    fn submit_waits_for_every_answer() {
        let mut chat = with_items(Vec::new());
        chat.live.ask = Some(questions_ask());
        press(&mut chat, "Right", '\0');
        press(&mut chat, "1", '1');
        // The size has none: Enter shows it, sending nothing.
        assert_eq!(press(&mut chat, "Enter", '\r'), None);
        assert_eq!(chat.card.at, 0);
        assert!(!chat.card.view(chat.live.ask.as_ref().unwrap()).ready());
        press(&mut chat, "1", '1');
        assert!(matches!(
            press(&mut chat, "Enter", '\r'),
            Some(ChatDo::Answers(_))
        ));
    }

    #[test]
    fn the_cards_footer_says_its_keys_in_its_buttons_words() {
        let ask = questions_ask();
        let mut card = Card::default();
        let hints: Vec<(String, String)> = [
            ("Enter", "next"),
            ("1–9 / Space", "pick"),
            ("Up / Down", "choose"),
            ("Left / Right", "question"),
            ("Esc", "answer in terminal"),
        ]
        .map(|(k, s)| (k.to_owned(), s.to_owned()))
        .into();
        assert_eq!(card_footer(&card, &ask), hints);
        card.sync(&ask);
        card.go(&ask.questions, 1);
        let last = card_footer(&card, &ask);
        assert_eq!(last[0], ("Enter".to_owned(), "submit".to_owned()));
        assert_eq!(last[1].1, "tick");
        // On Other: no digits, no question keys, Esc steps back.
        card.row = 3;
        let other = card_footer(&card, &ask);
        assert!(
            other
                .iter()
                .all(|(k, _)| !k.contains('1') && !k.contains("Left"))
        );
        assert_eq!(other.last().unwrap().1, "back");
        let ctxs = [false, true].map(|b| CardCtx {
            many: b,
            multi: b,
            last: b,
            other: b,
            typing: b,
        });
        key_table::check(CARD_KEYS, &ctxs);
        // While the card has the keys, the list's hints lose its keys.
        let mut chat = with_items(Vec::new());
        chat.live.ask = Some(ask);
        let hints = chat_footer(&chat, false, false);
        assert!(hints.iter().all(|(k, s)| k != "Space" || s != "fold"));
        assert!(hints.iter().any(|(k, _)| k == "Y"));
    }

    #[test]
    fn a_press_on_the_screens_menu_steps_off_the_users_own_answer_first() {
        let choice = |n: u8, picked, kind| menu::Choice {
            n,
            label: String::new(),
            detail: String::new(),
            picked,
            checked: Some(false),
            kind,
        };
        let m = |free_picked: bool| Menu {
            choices: vec![
                choice(1, !free_picked, ChoiceKind::Pick),
                choice(2, false, ChoiceKind::Pick),
                choice(3, free_picked, ChoiceKind::Free),
                choice(4, false, ChoiceKind::Chat),
            ],
            multi: true,
            ..Menu::default()
        };
        assert_eq!(
            choice_keys(Some(&m(false)), 2),
            Some(vec![WaitKey::Digit(2)])
        );
        // The arrow on the user's own answer: a digit would be its text.
        assert_eq!(
            choice_keys(Some(&m(true)), 2),
            Some(vec![WaitKey::Up, WaitKey::Digit(2)])
        );
        // That row itself: its words go in the reply box.
        assert_eq!(choice_keys(Some(&m(false)), 3), None);
        assert_eq!(choice_keys(None, 1), Some(vec![WaitKey::Digit(1)]));
    }
}
