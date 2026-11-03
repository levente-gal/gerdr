//! The manual test sheet: a feature's acceptance criteria (from its
//! brief), one row each, to try by hand and mark passed or failed, a
//! failure with a note saying what went wrong. The results save as they
//! are given (`Feature::manual`, see `feature::manual`). Mark passed marks
//! Manual test done once every criterion passed on the code as it is now;
//! Send failures sends the failures to the Build agent.
//!
//! The model and its keys are here, the painting in `chrome::manual_test`.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, MANUAL_CLOSE, MANUAL_PASS, MANUAL_SEND, ManualHit, ManualMark, ManualRow,
        ManualTestView,
    },
    feature::{
        self, Feature, Stage, Status, artifacts,
        manual::{self, Criterion, Verdict},
        store,
    },
    features::plural,
    text_field::{Edit, TextField},
    tooltip::Tooltip,
};

/// What the note editor says when Enter is pressed with no note.
const NO_NOTE: &str = "Not failed: say what went wrong first";

/// The manual test sheet's state, while it is open.
#[derive(Debug, Default)]
pub struct ManualSheet {
    pub project: String,
    pub slug: String,
    /// The row with the keyboard.
    pub cursor: usize,
    /// The first row shown.
    pub top: usize,
    /// The row whose note is being written, and the note.
    pub editing: Option<(usize, TextField)>,
    /// Why the last thing asked was refused.
    pub notice: Option<String>,
    pub hover: Option<ManualHit>,
    /// The keys list shows instead of the rows (F1).
    pub keys: bool,
}

/// What the sheet asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Mark criterion `i` passed, or clear it when it is.
    Toggle(usize),
    /// Mark criterion `i` failed, with this note.
    Fail(usize, String),
    /// Clear criterion `i`'s result.
    Clear(usize),
    /// Mark Manual test done.
    MarkPassed,
    /// Send the failures to the Build agent.
    SendFailures,
}

/// The keys of the sheet and what they do, for F1.
fn keys_list() -> Vec<(String, String)> {
    [
        ("Up / Down", "move between the criteria"),
        ("Ctrl+J / Ctrl+K", "move between the criteria"),
        ("Home / End", "go to the first or last criterion"),
        ("Space / Enter", "mark the criterion passed, or not"),
        ("f", "mark it failed, saying what went wrong"),
        ("Del", "clear its result"),
        ("Ctrl+Enter", "mark Manual test passed"),
        ("Ctrl+Shift+Enter", "send the failures to the Build agent"),
        ("F1", "hide these keys"),
        ("Esc", "close the sheet"),
    ]
    .into_iter()
    .map(|(k, w)| (k.to_owned(), w.to_owned()))
    .collect()
}

impl ManualSheet {
    /// The key hints of the footer, as the sheet is now.
    fn hints(&self) -> Vec<(String, String)> {
        let hints: &[(&str, &str)] = if self.keys {
            &[("Esc", "back")]
        } else if let Some((_, field)) = &self.editing {
            let esc = if field.trim().is_empty() {
                "cancel"
            } else {
                "save"
            };
            &[("Enter", "save"), ("Esc", esc)]
        } else {
            &[
                ("Space", "pass"),
                ("f", "fail\u{2026}"),
                ("Del", "clear"),
                ("Ctrl+Enter", "mark passed"),
                ("Ctrl+Shift+Enter", "send failures"),
                ("F1", "keys"),
                ("Esc", "close"),
            ]
        };
        hints
            .iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect()
    }

    /// Start writing row `i`'s note, from `note`.
    fn edit(&mut self, i: usize, note: &str) {
        let mut field = TextField::default();
        field.set(note);
        self.cursor = i;
        self.editing = Some((i, field));
        self.notice = None;
    }

    /// Stop writing a note, keeping it: what saves the note, if one was
    /// written. A note written is kept, never thrown away.
    fn take_note(&mut self) -> Option<Outcome> {
        let (i, field) = self.editing.take()?;
        self.notice = None;
        let note = field.trim().to_owned();
        (!note.is_empty()).then_some(Outcome::Fail(i, note))
    }

    /// Handle a key, the sheet showing `rows`: `sym` is the key's own
    /// symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        rows: &[ManualRow],
    ) -> Option<Outcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let enter = matches!(sym, Keysym::Return | Keysym::KP_Enter);
        if let Some((i, field)) = &mut self.editing {
            let i = *i;
            let note = field.trim().to_owned();
            if enter {
                if note.is_empty() {
                    self.notice = Some(NO_NOTE.into());
                    return None;
                }
                self.editing = None;
                self.notice = None;
                return Some(Outcome::Fail(i, note));
            }
            if sym == Keysym::Escape {
                return self.take_note();
            }
            if field.key(mods, sym, typed) == Edit::Changed {
                self.notice = None;
            }
            return None;
        }
        if self.keys {
            if matches!(sym, Keysym::Escape | Keysym::F1) {
                self.keys = false;
            }
            return None;
        }
        let last = rows.len().saturating_sub(1);
        let down = |c: usize| (c + 1).min(last);
        match sym {
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::F1 => self.keys = true,
            Keysym::Down | Keysym::KP_Down if plain => self.cursor = down(self.cursor),
            Keysym::Up | Keysym::KP_Up if plain => self.cursor = self.cursor.saturating_sub(1),
            Keysym::j | Keysym::J if ctrl => self.cursor = down(self.cursor),
            Keysym::k | Keysym::K if ctrl => self.cursor = self.cursor.saturating_sub(1),
            Keysym::Return | Keysym::KP_Enter if ctrl && mods.shift => {
                return Some(Outcome::SendFailures);
            }
            Keysym::Return | Keysym::KP_Enter if ctrl => return Some(Outcome::MarkPassed),
            Keysym::Home | Keysym::KP_Home if plain => self.cursor = 0,
            Keysym::End | Keysym::KP_End if plain => self.cursor = last,
            Keysym::space | Keysym::Return | Keysym::KP_Enter if plain && !rows.is_empty() => {
                return Some(Outcome::Toggle(self.cursor.min(last)));
            }
            Keysym::f if plain && !mods.shift && !rows.is_empty() => {
                let i = self.cursor.min(last);
                let note = rows.get(i).map_or("", |r| r.note.as_str()).to_owned();
                self.edit(i, &note);
            }
            Keysym::Delete | Keysym::KP_Delete if plain && !rows.is_empty() => {
                return Some(Outcome::Clear(self.cursor.min(last)));
            }
            _ => {}
        }
        None
    }
}

/// The rows of the sheet: `criteria` as `test` has them on `patch`.
fn rows(
    test: Option<&manual::ManualTest>,
    criteria: &[Criterion],
    patch: Option<&str>,
) -> Vec<ManualRow> {
    criteria
        .iter()
        .map(|c| {
            let check = manual::check(test, c, patch);
            let mark = match check.map(|k| k.state) {
                None => ManualMark::Untested,
                Some(Verdict::Pass) => ManualMark::Passed,
                Some(Verdict::Fail) => ManualMark::Failed,
            };
            ManualRow {
                id: c.id.clone(),
                text: c.text.clone(),
                mark,
                note: check
                    .filter(|k| k.state == Verdict::Fail)
                    .map_or_else(String::new, |k| k.note.clone()),
                sent: check.is_some_and(|k| k.state == Verdict::Fail && k.sent),
            }
        })
        .collect()
}

/// How far the manual test of `criteria` went, in words: "2 of 3
/// criteria passed · 1 failed", "1 failure sent to Build".
pub(crate) fn counts_text(n: manual::Counts) -> String {
    let total = n.total();
    if n.failed > 0 && n.sent == n.failed {
        return format!("{} sent to Build", plural(n.sent, "failure"));
    }
    let what = if total == 1 { "criterion" } else { "criteria" };
    let passed = format!("{} of {total} {what} passed", n.passed);
    if n.failed > 0 {
        format!("{passed} \u{b7} {} failed", n.failed)
    } else {
        passed
    }
}

/// What is left before every criterion passed: "AC-2 failed, AC-3 not
/// tried".
fn left_text(rows: &[ManualRow]) -> String {
    rows.iter()
        .filter_map(|r| match r.mark {
            ManualMark::Passed => None,
            ManualMark::Failed => Some(format!("{} failed", r.id)),
            ManualMark::Untested => Some(format!("{} not tried", r.id)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The acceptance criteria in the brief of feature `slug` of `project`,
/// read from disk: none when it has no brief, or none in it.
pub(crate) fn read_criteria(project: &str, slug: &str) -> Vec<Criterion> {
    let data = store::dir(None, project, slug);
    let Some(path) = artifacts::path(&data, Stage::Discover) else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => manual::parse_criteria(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            tracing::warn!("reading {}: {err}", path.display());
            Vec::new()
        }
    }
}

impl Gerdr {
    /// Read the criteria of feature `slug` of `project` from its brief
    /// again: the panel, the inbox and the sheet show them.
    pub(crate) fn load_criteria(&mut self, project: &str, slug: &str) {
        let criteria = read_criteria(project, slug);
        let key = (project.to_owned(), slug.to_owned());
        if criteria.is_empty() {
            self.features.criteria.remove(&key);
        } else {
            self.features.criteria.insert(key, criteria);
        }
    }

    /// The criteria of feature `slug` of `project`, as last read.
    pub(crate) fn criteria(&self, project: &str, slug: &str) -> &[Criterion] {
        self.features
            .criteria
            .get(&(project.to_owned(), slug.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// The patch-id of feature `slug`'s code as last read, if it was.
    fn manual_patch(&self, project: &str, slug: &str) -> Option<String> {
        self.feature_reading(project, slug).map(|r| r.patch.clone())
    }

    /// How far feature `f`'s manual test went on its code as last read
    /// (unknown: as recorded); `None` when its brief has no criteria.
    pub(crate) fn manual_counts(&self, f: &Feature) -> Option<manual::Counts> {
        let criteria = self.criteria(&f.project, &f.slug);
        if criteria.is_empty() {
            return None;
        }
        let patch = self.manual_patch(&f.project, &f.slug);
        Some(manual::counts(
            f.manual.as_ref(),
            criteria,
            patch.as_deref(),
        ))
    }

    /// Open the manual test sheet on feature `slug` of `project`.
    pub fn open_manual_test(&mut self, project: &str, slug: &str) {
        self.load_criteria(project, slug);
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        if self.criteria(project, slug).is_empty() {
            return self.show_warning(format!(
                "The brief of \u{201c}{title}\u{201d} has no acceptance criteria to try"
            ));
        }
        // The first criterion not tried yet, else the first.
        let patch = self.manual_patch(project, slug);
        let cursor = rows(
            feature.manual.as_ref(),
            self.criteria(project, slug),
            patch.as_deref(),
        )
        .iter()
        .position(|r| r.mark == ManualMark::Untested)
        .unwrap_or(0);
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.features.manual = Some(ManualSheet {
            project: project.to_owned(),
            slug: slug.to_owned(),
            cursor,
            ..ManualSheet::default()
        });
        self.reveal_manual_row();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
        self.probe_feature(project, slug);
    }

    /// Close the manual test sheet.
    pub fn close_manual_test(&mut self) {
        self.features.manual = None;
        self.refresh_pointer_focus();
    }

    /// The rows of the sheet open.
    fn manual_rows(&self) -> Vec<ManualRow> {
        let Some(sheet) = &self.features.manual else {
            return Vec::new();
        };
        let Some(feature) = self.feature(&sheet.project, &sheet.slug) else {
            return Vec::new();
        };
        let patch = self.manual_patch(&sheet.project, &sheet.slug);
        rows(
            feature.manual.as_ref(),
            self.criteria(&sheet.project, &sheet.slug),
            patch.as_deref(),
        )
    }

    /// Scroll the sheet's rows to the cursor.
    fn reveal_manual_row(&mut self) {
        let Some(view) = self.manual_test_view() else {
            return;
        };
        let top = chrome::manual_reveal(&view, self.output_size());
        if let Some(sheet) = self.features.manual.as_mut() {
            sheet.top = top;
        }
    }

    /// A key while the manual test sheet has the keyboard.
    pub fn manual_test_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let rows = self.manual_rows();
        let out = self.features.manual.as_mut()?.key(mods, sym, typed, &rows);
        self.reveal_manual_row();
        out
    }

    /// Refuse what the sheet asked, saying why on it and in a toast.
    fn manual_refuse(&mut self, why: String) {
        if let Some(sheet) = self.features.manual.as_mut() {
            sheet.notice = Some(why.clone());
        }
        self.show_warning(why);
    }

    /// Do what the manual test sheet asked.
    pub fn apply_manual_test(&mut self, outcome: Outcome) {
        let Some((project, slug)) = self
            .features
            .manual
            .as_ref()
            .map(|s| (s.project.clone(), s.slug.clone()))
        else {
            return;
        };
        let Some(feature) = self.feature(&project, &slug) else {
            self.close_manual_test();
            return self.show_warning("That feature is gone".into());
        };
        let title = feature.title.clone();
        let criteria = self.criteria(&project, &slug).to_vec();
        let reading = self.feature_reading(&project, &slug).cloned();
        let patch = reading.as_ref().map(|r| r.patch.clone());
        let reading_text =
            format!("Still reading the code of \u{201c}{title}\u{201d}; try again in a moment");
        match outcome {
            Outcome::Close => self.close_manual_test(),
            Outcome::Toggle(i) | Outcome::Fail(i, _) => {
                let Some(c) = criteria.get(i) else {
                    return;
                };
                let Some(patch) = patch else {
                    // The note goes back in its field, not thrown away.
                    if let (Outcome::Fail(_, note), Some(sheet)) =
                        (&outcome, self.features.manual.as_mut())
                    {
                        sheet.edit(i, note);
                    }
                    return self.manual_refuse(reading_text);
                };
                let Some(f) = self.feature_mut(&project, &slug) else {
                    return;
                };
                let passed = manual::check(f.manual.as_ref(), c, Some(&patch))
                    .is_some_and(|k| k.state == Verdict::Pass);
                match outcome {
                    Outcome::Fail(_, note) => {
                        manual::set(
                            &mut f.manual,
                            c,
                            &patch,
                            Verdict::Fail,
                            &note,
                            feature::now(),
                        );
                    }
                    _ if passed => _ = manual::clear(&mut f.manual, &c.id),
                    _ => manual::set(&mut f.manual, c, &patch, Verdict::Pass, "", feature::now()),
                }
                self.manual_saved(&project, &slug);
            }
            Outcome::Clear(i) => {
                let Some(c) = criteria.get(i) else {
                    return;
                };
                let Some(f) = self.feature_mut(&project, &slug) else {
                    return;
                };
                if manual::clear(&mut f.manual, &c.id) {
                    self.manual_saved(&project, &slug);
                }
            }
            Outcome::MarkPassed => {
                let Some(feature) = self.feature(&project, &slug) else {
                    return;
                };
                if feature.status != Status::Open || feature.current() != Some(Stage::ManualTest) {
                    let why = match feature.current() {
                        Some(stage) if feature.status == Status::Open => {
                            format!("Mark {} done first", stage.label())
                        }
                        _ => format!("\u{201c}{title}\u{201d} isn't open"),
                    };
                    return self.manual_refuse(why);
                }
                let Some(reading) = reading else {
                    return self.manual_refuse(reading_text);
                };
                if !reading.dirty.is_empty() {
                    return self.manual_refuse(format!(
                        "Commit or drop the uncommitted changes in {} first",
                        reading.dirty.join(", ")
                    ));
                }
                let rows = rows(feature.manual.as_ref(), &criteria, Some(&reading.patch));
                let left = left_text(&rows);
                if !left.is_empty() {
                    return self.manual_refuse(format!("Not passed yet: {left}"));
                }
                self.close_manual_test();
                self.read_and_tick(&project, &slug, Stage::ManualTest);
            }
            Outcome::SendFailures => {
                let Some(patch) = patch else {
                    return self.manual_refuse(reading_text);
                };
                let Some(feature) = self.feature(&project, &slug) else {
                    return;
                };
                let (n, text) = manual::failures_markdown(
                    feature.manual.as_ref(),
                    &criteria,
                    Some(&patch),
                    &title,
                );
                if n == 0 {
                    return self.manual_refuse("No failures to send: mark one failed first".into());
                }
                if !self.send_to_build(&project, &slug, "manual test failures", text) {
                    return;
                }
                if let Some(f) = self.feature_mut(&project, &slug) {
                    manual::mark_sent(&mut f.manual, &criteria, Some(&patch));
                }
                tracing::info!("sent {n} manual test failures of {project}/{slug} to Build");
                self.manual_saved(&project, &slug);
            }
        }
    }

    /// The sheet's results changed: saved at once.
    fn manual_saved(&mut self, project: &str, slug: &str) {
        if let Some(sheet) = self.features.manual.as_mut() {
            sheet.notice = None;
        }
        self.save_feature(project, slug);
    }

    /// A left click while the manual test sheet is open.
    pub fn manual_test_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.manual_test_view() else {
            return;
        };
        let layout = chrome::manual_test_layout(&view, self.output_size());
        let hit = layout.hit(pos);
        if let Some(ManualHit::Hint(i)) = hit {
            if let Some((keys, _)) = view.hints.get(i) {
                let keys = keys.clone();
                self.press_hint(crate::input::Overlay::ManualTest, &keys, pos);
            }
            return;
        }
        let rows = self.manual_rows();
        let Some(sheet) = self.features.manual.as_mut() else {
            return;
        };
        let (kept, outcome) = match hit {
            // A button keeps the note being written, then acts.
            Some(ManualHit::Button(b)) => {
                let outcome = match b {
                    MANUAL_CLOSE => Outcome::Close,
                    MANUAL_SEND => Outcome::SendFailures,
                    MANUAL_PASS => Outcome::MarkPassed,
                    _ => return,
                };
                (sheet.take_note(), Some(outcome))
            }
            Some(ManualHit::Mark(i)) if sheet.editing.is_none() => {
                sheet.cursor = i;
                (None, Some(Outcome::Toggle(i)))
            }
            // Its own row's note goes on being written.
            Some(ManualHit::Fail(i)) if sheet.editing.as_ref().is_some_and(|(at, _)| *at == i) => {
                (None, None)
            }
            Some(ManualHit::Fail(i)) => {
                // The note being written for another row is kept.
                let kept = sheet.take_note();
                let note = rows.get(i).map_or("", |r| r.note.as_str()).to_owned();
                sheet.edit(i, &note);
                (kept, None)
            }
            Some(ManualHit::Row(i)) if sheet.editing.is_none() => {
                sheet.cursor = i;
                (None, None)
            }
            // Off it, it stays: a click away never throws a note away.
            _ => (None, None),
        };
        if let Some(kept) = kept {
            self.apply_manual_test(kept);
            // Refused (the code still being read): the button waits.
            if self
                .features
                .manual
                .as_ref()
                .is_some_and(|s| s.notice.is_some())
            {
                return;
            }
        }
        if let Some(outcome) = outcome {
            self.apply_manual_test(outcome);
        }
    }

    /// The pointer moved while the manual test sheet is open: what is
    /// under it lights up, and a criterion cut short shows whole.
    pub fn manual_test_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.manual_test_view()?;
        let size = self.output_size();
        let hit = chrome::manual_test_layout(&view, size).hit(pos);
        if let Some(sheet) = self.features.manual.as_mut() {
            sheet.hover = hit;
        }
        let layout = chrome::manual_test_layout(&view, size);
        match hit? {
            ManualHit::Button(MANUAL_PASS) => Some(
                Tooltip::new("Mark Manual test passed", layout.buttons[MANUAL_PASS])
                    .key("Ctrl+Enter"),
            ),
            ManualHit::Button(MANUAL_SEND) => Some(
                Tooltip::new(
                    "Send the failures to the Build agent",
                    layout.buttons[MANUAL_SEND],
                )
                .key("Ctrl+Shift+Enter"),
            ),
            ManualHit::Mark(i) => {
                let spot = layout.rows.iter().find(|s| s.i == i)?;
                Some(Tooltip::new("Mark passed, or not", spot.mark).key("Space"))
            }
            ManualHit::Fail(i) => {
                let spot = layout.rows.iter().find(|s| s.i == i)?;
                Some(Tooltip::new("Mark failed, saying what went wrong", spot.fail).key("f"))
            }
            ManualHit::Row(_) => {
                let (text, rect) = chrome::manual_test_tip(&view, size, pos)?;
                Some(Tooltip::new(&text, rect))
            }
            _ => None,
        }
    }

    /// Scroll the sheet's rows `by` rows.
    pub fn manual_test_scroll(&mut self, by: isize) {
        let Some(view) = self.manual_test_view() else {
            return;
        };
        let most = chrome::manual_most_top(&view, self.output_size());
        if let Some(sheet) = self.features.manual.as_mut() {
            sheet.top = sheet.top.saturating_add_signed(by).min(most);
        }
    }

    /// The sheet's rows shown, for the wheel's pitch.
    pub fn manual_test_rows(&self) -> Vec<chrome::Rect> {
        self.manual_test_view().map_or_else(Vec::new, |view| {
            chrome::manual_test_layout(&view, self.output_size()).row_rects()
        })
    }

    /// What the pointer can press in the sheet.
    pub fn manual_test_controls(&self) -> Vec<chrome::Rect> {
        self.manual_test_view().map_or_else(Vec::new, |view| {
            chrome::manual_test_layout(&view, self.output_size()).controls()
        })
    }

    pub fn manual_test_view(&self) -> Option<ManualTestView> {
        let sheet = self.features.manual.as_ref()?;
        let feature = self.feature(&sheet.project, &sheet.slug)?;
        let rows = self.manual_rows();
        let reading = self.feature_reading(&sheet.project, &sheet.slug);
        let patch = reading.map(|r| r.patch.as_str());
        let n = manual::counts(
            feature.manual.as_ref(),
            self.criteria(&sheet.project, &sheet.slug),
            patch,
        );
        let mut sub = counts_text(n);
        match reading {
            None => sub.push_str(" \u{b7} reading the code"),
            Some(r) if !r.dirty.is_empty() => {
                sub.push_str(&format!(
                    " \u{b7} uncommitted changes in {}",
                    r.dirty.join(", ")
                ));
            }
            Some(_) => {}
        }
        Some(ManualTestView {
            title: format!("Manual test of \u{201c}{}\u{201d}", feature.title),
            sub,
            cursor: sheet.cursor.min(rows.len().saturating_sub(1)),
            top: sheet.top,
            editing: sheet.editing.as_ref().map(|(i, f)| (*i, f.line())),
            notice: sheet.notice.clone(),
            hover: sheet.hover,
            keys: sheet.keys.then(keys_list),
            hints: sheet.hints(),
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(sym: Keysym) -> (ModifiersState, Keysym) {
        (ModifiersState::default(), sym)
    }

    fn ctrl(sym: Keysym) -> (ModifiersState, Keysym) {
        (
            ModifiersState {
                ctrl: true,
                ..ModifiersState::default()
            },
            sym,
        )
    }

    fn row(id: &str, mark: ManualMark, note: &str) -> ManualRow {
        ManualRow {
            id: id.into(),
            text: "Given a, when b, then c".into(),
            mark,
            note: note.into(),
            sent: false,
        }
    }

    fn press(
        sheet: &mut ManualSheet,
        rows: &[ManualRow],
        k: (ModifiersState, Keysym),
    ) -> Option<Outcome> {
        sheet.key(&k.0, k.1, k.1, rows)
    }

    #[test]
    fn keys_move_mark_and_close() {
        let rows = vec![
            row("AC-1", ManualMark::Untested, ""),
            row("AC-2", ManualMark::Failed, "it broke"),
        ];
        let mut s = ManualSheet::default();
        assert_eq!(
            press(&mut s, &rows, key(Keysym::space)),
            Some(Outcome::Toggle(0))
        );
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Toggle(0))
        );
        press(&mut s, &rows, key(Keysym::End));
        assert_eq!(s.cursor, 1);
        press(&mut s, &rows, key(Keysym::Home));
        press(&mut s, &rows, ctrl(Keysym::j));
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(s.cursor, 1);
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Delete)),
            Some(Outcome::Clear(1))
        );
        assert_eq!(
            press(&mut s, &rows, ctrl(Keysym::Return)),
            Some(Outcome::MarkPassed)
        );
        let mut both = ctrl(Keysym::Return);
        both.0.shift = true;
        assert_eq!(press(&mut s, &rows, both), Some(Outcome::SendFailures));
        press(&mut s, &rows, ctrl(Keysym::k));
        assert_eq!(s.cursor, 0);
        // F1 shows the keys; Esc goes back, then closes.
        press(&mut s, &rows, key(Keysym::F1));
        assert!(s.keys);
        assert_eq!(press(&mut s, &rows, key(Keysym::space)), None);
        assert_eq!(press(&mut s, &rows, key(Keysym::Escape)), None);
        assert!(!s.keys);
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Escape)),
            Some(Outcome::Close)
        );
    }

    #[test]
    fn a_failure_needs_a_note_and_esc_keeps_one_written() {
        let rows = vec![
            row("AC-1", ManualMark::Untested, ""),
            row("AC-2", ManualMark::Failed, "it broke"),
        ];
        let mut s = ManualSheet::default();
        press(&mut s, &rows, key(Keysym::f));
        assert!(s.editing.is_some());
        // Enter with no note keeps the editor open, saying why.
        assert_eq!(press(&mut s, &rows, key(Keysym::Return)), None);
        assert_eq!(s.notice.as_deref(), Some(NO_NOTE));
        assert!(s.editing.is_some());
        // Esc with no note leaves it unmarked.
        assert_eq!(press(&mut s, &rows, key(Keysym::Escape)), None);
        assert!(s.editing.is_none() && s.notice.is_none());
        // A failure's note comes back to edit; Enter saves it.
        s.cursor = 1;
        press(&mut s, &rows, key(Keysym::f));
        assert_eq!(s.hints()[1].1, "save");
        press(&mut s, &rows, key(Keysym::exclam));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Fail(1, "it broke!".into()))
        );
        // Esc with a note saves it too.
        press(&mut s, &rows, key(Keysym::f));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Escape)),
            Some(Outcome::Fail(1, "it broke".into()))
        );
        // Typed in the editor, f and Space are text.
        s.edit(0, "");
        assert_eq!(press(&mut s, &rows, key(Keysym::space)), None);
        assert_eq!(s.hints()[1].1, "cancel");
    }

    #[test]
    fn taking_a_note_keeps_what_was_written() {
        let mut s = ManualSheet::default();
        assert_eq!(s.take_note(), None);
        s.edit(2, "  ");
        assert_eq!(s.take_note(), None);
        assert!(s.editing.is_none());
        s.edit(2, " menu still on ");
        s.notice = Some(NO_NOTE.into());
        assert_eq!(
            s.take_note(),
            Some(Outcome::Fail(2, "menu still on".into()))
        );
        assert!(s.editing.is_none() && s.notice.is_none());
    }

    #[test]
    fn counts_read_as_words() {
        let n = |passed, failed, sent, untested| manual::Counts {
            passed,
            failed,
            sent,
            untested,
        };
        assert_eq!(
            counts_text(n(2, 1, 0, 0)),
            "2 of 3 criteria passed \u{b7} 1 failed"
        );
        assert_eq!(counts_text(n(0, 0, 0, 1)), "0 of 1 criterion passed");
        assert_eq!(counts_text(n(1, 1, 1, 1)), "1 failure sent to Build");
        assert_eq!(
            left_text(&[
                row("AC-1", ManualMark::Passed, ""),
                row("AC-2", ManualMark::Failed, "x"),
                row("AC-3", ManualMark::Untested, ""),
            ]),
            "AC-2 failed, AC-3 not tried"
        );
    }
}
