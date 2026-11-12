//! A panel's keys as one table: what each chord does now, whether the footer
//! hints it, and its row on the F1 sheet. The handler, the footer, a click on
//! a hint (`press_hint`) and F1 all read the same rows, so they can't drift
//! (checklist K3).
//!
//! A chord is written as the footer shows it ("Ctrl+Shift+E", "Alt+1") and
//! read with `keys::hint_key`, which gives the key's own symbol, as a press
//! has it (K8): rows match on that and the modifiers, never on `typed`.

use smithay::input::keyboard::{Keysym, ModifiersState};

/// One key of a panel, for context `C`, doing `A`.
pub struct Binding<C, A> {
    /// The chord, as the footer and F1 show it.
    pub chord: &'static str,
    /// What it does now; none while it isn't offered, so that the key then
    /// passes to the next row (or the next handler) and isn't hinted.
    pub does: fn(&C) -> Option<A>,
    /// Whether the footer hints it while offered: some keys are only on F1
    /// and in menus (checklist K3, K7).
    pub footer: bool,
    /// Its words on F1; empty to leave it off, for a chord another row of the
    /// same words already lists there. A row with no F1 words and no footer
    /// hint is an alias: a press the panel took before its keys were a table
    /// (Shift+Esc for Esc, say), kept so that it still works, never shown.
    pub f1: &'static str,
}

/// Whether the key `mods` and `sym` (its own symbol, as a press gives it) is
/// `chord`.
pub fn is(chord: &str, mods: &ModifiersState, sym: Keysym) -> bool {
    let Some((want, key)) = crate::keys::hint_key(chord) else {
        return false;
    };
    // ISO_Left_Tab is Shift+Tab, as some layouts give it without Shift.
    let shift = mods.shift || sym == Keysym::ISO_Left_Tab;
    want.ctrl == mods.ctrl
        && want.alt == mods.alt
        && want.shift == shift
        && want.logo == mods.logo
        && same_key(sym, key)
}

/// Whether `pressed` is `key`: keypad and shifted forms of a key are the key.
fn same_key(pressed: Keysym, key: Keysym) -> bool {
    let plain = |sym: Keysym| match sym {
        Keysym::KP_Enter => Keysym::Return,
        Keysym::KP_Delete => Keysym::Delete,
        Keysym::ISO_Left_Tab => Keysym::Tab,
        Keysym::KP_Up => Keysym::Up,
        Keysym::KP_Down => Keysym::Down,
        Keysym::KP_Left => Keysym::Left,
        Keysym::KP_Right => Keysym::Right,
        Keysym::KP_Home => Keysym::Home,
        Keysym::KP_End => Keysym::End,
        Keysym::KP_Page_Up => Keysym::Page_Up,
        Keysym::KP_Page_Down => Keysym::Page_Down,
        sym => match sym.key_char() {
            Some(c) if c.is_ascii_uppercase() => {
                Keysym::from_char(c.to_ascii_lowercase())
            }
            _ => sym,
        },
    };
    plain(pressed) == plain(key)
}

/// What the first row of `table` that is the key pressed and offered now
/// does.
pub fn press<C, A>(
    table: &[Binding<C, A>],
    ctx: &C,
    mods: &ModifiersState,
    sym: Keysym,
) -> Option<A> {
    (table.iter())
        .filter(|b| is(b.chord, mods, sym))
        .find_map(|b| (b.does)(ctx))
}

/// The footer's hints of `table` now: each offered row the footer shows,
/// once per chord, with `label` saying what its action does (the button's
/// words, checklist K3).
pub fn hints<C, A>(
    table: &[Binding<C, A>],
    ctx: &C,
    label: impl Fn(&A) -> String,
) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for b in table.iter().filter(|b| b.footer) {
        if out.iter().any(|(chord, _)| *chord == b.chord) {
            continue;
        }
        if let Some(action) = (b.does)(ctx) {
            out.push((b.chord, label(&action)));
        }
    }
    out
}

/// The F1 rows of `table`, in its order: chord and words.
pub fn f1_rows<C, A>(table: &[Binding<C, A>]) -> Vec<(&'static str, &'static str)> {
    (table.iter())
        .filter(|b| !b.f1.is_empty())
        .map(|b| (b.chord, b.f1))
        .collect()
}

/// `rows` (chord, words) with each run of rows saying the same words as
/// one row, its chords joined as "Up / Down": how F1 lists a pair, and a
/// footer offers a choice ("Esc / Q").
pub fn joined<W: AsRef<str>>(
    rows: impl IntoIterator<Item = (&'static str, W)>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (chord, words) in rows {
        match out.last_mut() {
            Some((keys, said)) if said == words.as_ref() => {
                keys.push_str(" / ");
                keys.push_str(chord);
            }
            _ => out.push((chord.to_owned(), words.as_ref().to_owned())),
        }
    }
    out
}

/// For tests of a panel: over every context in `ctxs`, each footer hint
/// presses a row that is offered, every chord parses, and every chord in
/// the table but an alias's has an F1 row. Panics naming what is wrong.
#[cfg(test)]
pub fn check<C, A>(table: &[Binding<C, A>], ctxs: &[C]) {
    for b in table {
        assert!(crate::keys::hint_key(b.chord).is_some(), "{} doesn't parse", b.chord);
        let alias = !b.footer && b.f1.is_empty();
        assert!(
            alias || table.iter().any(|o| o.chord == b.chord && !o.f1.is_empty()),
            "{} is on no F1 row",
            b.chord
        );
    }
    for ctx in ctxs {
        for (chord, _) in hints(table, ctx, |_| String::new()) {
            let (mods, sym) = crate::keys::hint_key(chord).expect(chord);
            assert!(press(table, ctx, &mods, sym).is_some(), "{chord} is hinted but does nothing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ctx {
        open: bool,
    }

    #[derive(Debug, PartialEq)]
    enum Act {
        Close,
        Edit,
    }

    const TABLE: &[Binding<Ctx, Act>] = &[
        Binding {
            chord: "Esc",
            does: |_| Some(Act::Close),
            footer: true,
            f1: "close",
        },
        Binding {
            chord: "Shift+E",
            does: |c| c.open.then_some(Act::Edit),
            footer: true,
            f1: "edit",
        },
    ];

    #[test]
    fn a_row_presses_only_while_offered() {
        let (mods, sym) = crate::keys::hint_key("Shift+E").unwrap();
        assert_eq!(press(TABLE, &Ctx { open: true }, &mods, sym), Some(Act::Edit));
        assert_eq!(press(TABLE, &Ctx { open: false }, &mods, sym), None);
        // A press gives Shift+E as its own symbol `e` (K8), or `E` on some
        // layouts: both are the key.
        assert_eq!(
            press(TABLE, &Ctx { open: true }, &mods, Keysym::E),
            Some(Act::Edit)
        );
        let label = |a: &Act| format!("{a:?}");
        assert_eq!(hints(TABLE, &Ctx { open: false }, label), [("Esc", "Close".to_owned())]);
        check(TABLE, &[Ctx { open: true }, Ctx { open: false }]);
    }
}
