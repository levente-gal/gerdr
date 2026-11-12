//! The criteria strip: a feature's acceptance criteria over its sandbox's app,
//! to try one by one and mark passed or failed there, without going back to
//! the feature tab between tries.
//!
//! It shows on a web pane of a feature's workspace at a page of the sandbox
//! (the project's `sandbox_url`, else one on this machine, as the sandbox's
//! app opens), while the feature is at Manual test and its brief has
//! criteria. The criterion it shows starts at the first not tried; Pass marks
//! it and moves on to the next not tried, Fail… opens a note on the strip,
//! written as on the criterion's card on the page (the same note: the page and
//! the strip show one result). Screenshot adds what the page shows to the
//! note, a PNG in the feature's data folder (`manual::SHOTS_DIR`) that the
//! failures sent to Build name. Page shows the criterion on Manual test's
//! page.
//!
//! The strip folds to a chip in the pane's corner, and stays folded or not
//! (`state.json`). It takes what lands on it; the keys only once given them,
//! by the prefix and Shift+C, a click on its note field or Fail…, and back to
//! the app by Esc, the same chord, or a press on the app. With the keys:
//! Space or Enter passes, F fails, Del clears, Ctrl+PgUp / Ctrl+PgDn (or Up /
//! Down) step through the criteria, Alt+C takes a screenshot, Ctrl+O shows the
//! page, Alt+Down folds it. Writing a note, Enter saves it; Esc never does:
//! it leaves the criterion as it was, what is written kept as a draft for its
//! next note (Ctrl+Shift+Del discards it), Shift+Enter breaks
//! the line, as on the card, and Alt+C adds a screenshot.
//!
//! The user's own comments are rows of it too, after the criteria: Done and
//! Not done (Space, F) as on the page, Edit… opens the comment dialog. Add
//! comment… (C) adds one from any row. Deleting one is on the page.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point},
};

use crate::{
    Gerdr, chrome,
    chrome::{CriteriaStripView, StripHit, StripTag},
    feature::{
        Stage,
        manual::{self, Criterion, Verdict},
        store,
    },
    features::{PanelOutcome, criterion_buttons, criterion_details, marks_criteria},
    key_table::{self, Binding},
    manual_test::{Outcome, Row},
    text_field::TextField,
    tooltip::Tooltip,
};

/// The buttons on the strip for row `i`, as `strip_buttons` lists them:
/// those of its card, but deleting a comment, which the page does, and then
/// adding one.
fn strip_buttons(i: usize, row: &Row) -> Vec<(chrome::PageButton, PanelOutcome)> {
    let mut out: Vec<_> = (criterion_buttons(i, row).into_iter())
        .filter(|(_, o)| !matches!(o, PanelOutcome::Manual(Outcome::DeleteComment(_))))
        .collect();
    let add = chrome::PageButton {
        label: "Add comment\u{2026}".to_owned(),
        style: chrome::ButtonStyle::Quiet,
    };
    out.push((add, PanelOutcome::AddComment));
    out
}

/// How long a screenshot is waited for before the page is said not to send
/// it.
const SHOT_WAIT: Duration = Duration::from_secs(10);

/// A feature, by its project and slug.
type Key = (String, String);

/// The criteria strip's state, across the features whose sandboxes show.
#[derive(Default)]
pub struct CriteriaStrip {
    /// The criterion each feature's strip shows, once stepped to.
    at: HashMap<Key, usize>,
    /// The note being written on each feature's strip.
    drafts: HashMap<Key, Draft>,
    /// Notes Esc put away unsaved, by feature: writing the same criterion's
    /// note again goes on with one. Esc never saves, nor throws one away.
    parked: HashMap<Key, Draft>,
    /// Its steps show.
    open: bool,
    /// The pane whose strip has the keys.
    keys: Option<u32>,
    /// What of it is under the pointer.
    pub hover: Option<StripHit>,
    /// The screenshot asked for, until it comes.
    shot: Option<ShotWait>,
}

/// A note being written on the strip.
struct Draft {
    /// The criterion it is about.
    criterion: Criterion,
    field: TextField,
    /// The screenshots taken for it, to go with it once saved.
    shots: Vec<String>,
    /// Whether the strip had the keys before: it keeps them after.
    had_keys: bool,
}

/// A screenshot asked of a pane's page.
struct ShotWait {
    pane: u32,
    feature: Key,
    criterion: Criterion,
    since: Instant,
}

/// Where the strip shows: over `pane`, for feature `slug` of `project`.
struct Target {
    pane: u32,
    rect: chrome::Rect,
    project: String,
    slug: String,
}

impl Target {
    fn key(&self) -> Key {
        (self.project.clone(), self.slug.clone())
    }
}

/// `url`'s scheme, host and port, lowercase: `http://localhost:3000`.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}").to_lowercase())
}

/// Whether `url` is a page of a sandbox's app: of `set`, the page the
/// project's sandbox opens, or without one, of this machine's `localhost`,
/// where the sandbox's pages and ports open (`machine_sandbox` for one
/// elsewhere). T3 Code's `*.localhost` pages aren't.
fn sandbox_page(url: &str, set: Option<&str>) -> bool {
    match set.and_then(origin) {
        Some(set) => origin(url).is_some_and(|at| at == set),
        None => crate::feature_sandbox::is_local_page(url),
    }
}

/// The first of `rows` after `i` not tried, going round; none when all were.
fn next_untried(rows: &[Row], i: usize) -> Option<usize> {
    let n = rows.len();
    (1..=n)
        .map(|k| (i + k) % n)
        .find(|&j| rows[j].found.is_none())
}

/// Where the strip of `rows` starts: at the first not tried, else the first.
fn first_untried(rows: &[Row]) -> usize {
    rows.iter().position(|r| r.found.is_none()).unwrap_or(0)
}

/// "1 screenshot", "2 screenshots".
fn shots_text(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "1 screenshot".into(),
        n => format!("{n} screenshots"),
    }
}

/// What a key does on the strip with the keys: `STRIP_KEYS`' on a
/// criterion, `NOTE_KEYS`' while its note is written.
#[derive(Debug, PartialEq)]
enum StripKey {
    /// What the criterion's card does: pass, fail, clear.
    Do(Outcome),
    AddComment,
    /// Give the keys back to the app.
    Close,
    /// Show the criterion on Manual test's page.
    OnPage,
    /// Fold the strip to a chip.
    Fold,
    /// Add a screenshot of the page to the note.
    Shot,
    /// Save the note.
    Save,
    Newline,
    /// Put the note away unsaved, as a draft.
    Park,
    /// Throw the draft away.
    Discard,
}

/// The criterion the strip shows, as its keys read it.
struct StripCtx {
    i: usize,
    /// It is a comment of the user's own.
    comment: bool,
    /// It was tried.
    found: bool,
}

/// The kinds of row the strip shows, to look a key up on: a criterion
/// not tried, one tried, and a comment.
const STRIP_STATES: [StripCtx; 3] = [
    StripCtx {
        i: 0,
        comment: false,
        found: false,
    },
    StripCtx {
        i: 0,
        comment: false,
        found: true,
    },
    StripCtx {
        i: 0,
        comment: true,
        found: false,
    },
];

/// The strip's keys on a criterion; its tooltips say them. Up / Down and
/// Ctrl+PgUp / Ctrl+PgDn step through the criteria after it. Aliases
/// (no footer, no F1) keep the presses that worked before the table.
const STRIP_KEYS: &[Binding<StripCtx, StripKey>] = &[
    Binding {
        chord: "Space",
        does: |c| Some(StripKey::Do(Outcome::Toggle(c.i))),
        footer: true,
        f1: "pass it and go to the next, or take the pass back; a comment: done",
    },
    Binding {
        chord: "Enter",
        does: |c| Some(StripKey::Do(Outcome::Toggle(c.i))),
        footer: false,
        f1: "the same as Space",
    },
    Binding {
        chord: "F",
        does: |c| {
            Some(StripKey::Do(match c.comment {
                true => Outcome::Fail(c.i, String::new()),
                false => Outcome::Write(c.i),
            }))
        },
        footer: true,
        f1: "fail it, saying what went wrong; a comment: not done",
    },
    Binding {
        chord: "Del",
        does: |c| (c.found && !c.comment).then_some(StripKey::Do(Outcome::Clear(c.i))),
        footer: true,
        f1: "take its result back",
    },
    Binding {
        chord: "C",
        does: |_| Some(StripKey::AddComment),
        footer: true,
        f1: "add a comment of your own",
    },
    Binding {
        chord: "Esc",
        does: |_| Some(StripKey::Close),
        footer: true,
        f1: "give the keys back to the app",
    },
    Binding {
        chord: "Shift+Esc",
        does: |_| Some(StripKey::Close),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+Esc",
        does: |_| Some(StripKey::Close),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Alt+Esc",
        does: |_| Some(StripKey::Close),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Ctrl+O",
        does: |_| Some(StripKey::OnPage),
        footer: true,
        f1: "show it on Manual test's page",
    },
    Binding {
        chord: "Ctrl+Super+O",
        does: |_| Some(StripKey::OnPage),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Alt+Down",
        does: |_| Some(StripKey::Fold),
        footer: true,
        f1: "fold the strip to a chip",
    },
    Binding {
        chord: "Alt+Shift+Down",
        does: |_| Some(StripKey::Fold),
        footer: false,
        f1: "",
    },
    Binding {
        chord: "Alt+C",
        does: |_| Some(StripKey::Shot),
        footer: true,
        f1: "add a screenshot of the page to the note",
    },
    Binding {
        chord: "Alt+Super+C",
        does: |_| Some(StripKey::Shot),
        footer: false,
        f1: "",
    },
];

/// The strip's keys while a note is written on it; the rest go to its
/// field.
const NOTE_KEYS: &[Binding<(), StripKey>] = &[
    Binding {
        chord: "Enter",
        does: |_| Some(StripKey::Save),
        footer: true,
        f1: "save the note",
    },
    Binding {
        chord: "Shift+Enter",
        does: |_| Some(StripKey::Newline),
        footer: true,
        f1: "start a new line",
    },
    Binding {
        chord: "Esc",
        does: |_| Some(StripKey::Park),
        footer: true,
        f1: "leave it unsaved, kept as a draft for its next note",
    },
    Binding {
        chord: crate::text_field::DISCARD_KEY,
        does: |_| Some(StripKey::Discard),
        footer: true,
        f1: "discard it",
    },
    Binding {
        chord: "Alt+C",
        does: |_| Some(StripKey::Shot),
        footer: true,
        f1: "add a screenshot of the page to the note",
    },
    Binding {
        chord: "Alt+Super+C",
        does: |_| Some(StripKey::Shot),
        footer: false,
        f1: "",
    },
];

/// The chord of `table`'s first key doing what `want` picks, as one of
/// `ctxs` has it; empty for none. Tooltips say the keys by it.
fn chord_of<C>(
    table: &[Binding<C, StripKey>],
    ctxs: &[C],
    want: impl Fn(&StripKey) -> bool,
) -> &'static str {
    (ctxs.iter())
        .find_map(|ctx| {
            (table.iter())
                .find(|b| (b.does)(ctx).is_some_and(|k| want(&k)))
                .map(|b| b.chord)
        })
        .unwrap_or("")
}

/// The chord of the strip's key doing `key` on some criterion.
fn strip_chord(key: &StripKey) -> &'static str {
    chord_of(STRIP_KEYS, &STRIP_STATES, |k| k == key)
}

/// What a button of a criterion's strip says it does, and its key.
fn button_tip(outcome: &Outcome, label: &str) -> (&'static str, &'static str) {
    let text = match outcome {
        Outcome::Toggle(_) if label == "Done" => "Mark the comment done",
        Outcome::Fail(..) => "Mark the comment not done: it goes to Build",
        Outcome::EditComment(_) => "Edit the comment",
        Outcome::Toggle(_) if label == "Pass" => "Mark it passed, and go to the next",
        Outcome::Toggle(_) => "Take the pass back",
        Outcome::Write(_) if label.starts_with("Edit") => "Edit what went wrong",
        Outcome::Write(_) => "Mark it failed and say what went wrong",
        Outcome::Clear(_) => "Take the failure back",
        _ => return ("", ""),
    };
    let same = |k: &StripKey| matches!(k, StripKey::Do(o) if std::mem::discriminant(o) == std::mem::discriminant(outcome));
    (text, chord_of(STRIP_KEYS, &STRIP_STATES, same))
}

impl Gerdr {
    /// Where the strip shows now, if it does: on the shown tab's web pane at
    /// a sandbox's page (the focused one first), in a feature's workspace at
    /// Manual test with criteria.
    fn criteria_target(&self) -> Option<Target> {
        let tab = self.tab()?;
        if tab.is_feature() {
            return None;
        }
        let project = self.project.clone()?;
        let feature = self.workspace_feature(self.active_workspace)?;
        if !marks_criteria(feature, Stage::ManualTest)
            || self.features.criteria(&project, &feature.slug).is_empty()
        {
            return None;
        }
        let set = (self.projects.project(&project)).and_then(|p| p.workflow.sandbox_url.clone());
        let sandbox = |id: u32| {
            (tab.panes.iter())
                .find(|p| p.id == id)
                .and_then(|p| p.app_command.as_deref())
                .is_some_and(|url| crate::is_web_link(url) && sandbox_page(url, set.as_deref()))
        };
        let rects = tab.shown_rects(self.content_rect());
        let focused = tab.focused_id();
        let (pane, rect) = (rects.iter())
            .find(|(id, _)| Some(*id) == focused && sandbox(*id))
            .or_else(|| rects.iter().find(|(id, _)| sandbox(*id)))
            .copied()?;
        Some(Target {
            pane,
            rect,
            project,
            slug: feature.slug.clone(),
        })
    }

    /// The rows of the strip's feature, and the one it shows.
    fn strip_rows(&self, target: &Target) -> Option<(Vec<Row>, usize)> {
        let feature = self.feature(&target.project, &target.slug)?;
        let rows = self.criterion_rows(feature);
        if rows.is_empty() {
            return None;
        }
        let at = (self.criteria_strip.at.get(&target.key()).copied())
            .unwrap_or_else(|| first_untried(&rows))
            .min(rows.len() - 1);
        Some((rows, at))
    }

    /// What the strip shows, if it does.
    pub(crate) fn criteria_strip_view(&self) -> Option<CriteriaStripView> {
        let target = self.criteria_target()?;
        let (rows, i) = self.strip_rows(&target)?;
        let row = &rows[i];
        let strip = &self.criteria_strip;
        let draft = (strip.drafts.get(&target.key())).filter(|d| d.criterion.id == row.id);
        let saved = row.shots.len();
        let waiting = (strip.shot.as_ref()).is_some_and(|w| w.feature == target.key());
        let shots = match waiting {
            true => "taking a screenshot\u{2026}".to_owned(),
            false => shots_text(draft.map_or(saved, |d| saved + d.shots.len())),
        };
        let state = match (row.made, row.sent) {
            (true, _) => "fix made: try again",
            (false, true) => crate::feature::review_import::sent_word(false, row.planning),
            (false, false) => "",
        };
        let keys = self.criteria_strip_has_keys();
        Some(CriteriaStripView {
            pane: target.rect,
            folded: self.projects.criteria_folded && draft.is_none(),
            open: strip.open || keys,
            place: format!("{} \u{b7} {}/{}", row.id, i + 1, rows.len()),
            title: row.title.clone(),
            tag: match row.found {
                None => StripTag::NotRun,
                Some(Verdict::Pass) => StripTag::Passed,
                Some(Verdict::Fail) => StripTag::Failed,
            },
            state: state.to_owned(),
            steps: criterion_details(row)
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect(),
            note: row.note.clone(),
            field: draft.map(|d| d.field.line()),
            shots,
            buttons: (strip_buttons(i, row).into_iter())
                .map(|(button, _)| (button.label, button.style))
                .collect(),
            steps_through: rows.len() > 1,
            hover: strip.hover,
            keys,
        })
    }

    /// The strip's layout, if it shows.
    fn strip_layout(&self) -> Option<chrome::StripLayout> {
        Some(chrome::criteria_strip_layout(&self.criteria_strip_view()?))
    }

    /// What of the strip is at `pos`, if it is there.
    pub(crate) fn criteria_strip_hit(&self, pos: Point<f64, Logical>) -> Option<StripHit> {
        self.strip_layout()?.hit(pos)
    }

    /// Whether the strip has the keys: given them, over the pane it was
    /// given them on.
    pub(crate) fn criteria_strip_has_keys(&self) -> bool {
        let Some(pane) = self.criteria_strip.keys else {
            return false;
        };
        self.criteria_target().is_some_and(|t| t.pane == pane)
    }

    /// The keys go back to the app from the strip, if it had them. A note
    /// being written stays, to go on with.
    pub(crate) fn drop_criteria_keys(&mut self) {
        self.criteria_strip.keys = None;
    }

    /// Give the strip the keys, unfolded, or give them back to the app.
    pub(crate) fn toggle_criteria_keys(&mut self) {
        if self.criteria_strip_has_keys() {
            return self.drop_criteria_keys();
        }
        let Some(target) = self.criteria_target() else {
            return self.show_warning(
                "No sandbox app with criteria to try shows here: open it from Manual test".into(),
            );
        };
        self.set_criteria_folded(false);
        self.criteria_strip.keys = Some(target.pane);
        self.stop_key_repeat();
    }

    /// Fold the strip to its chip, or unfold it; kept in `state.json`.
    fn set_criteria_folded(&mut self, folded: bool) {
        if self.projects.criteria_folded != folded {
            self.projects.criteria_folded = folded;
            self.projects.save();
        }
        if folded {
            self.drop_criteria_keys();
        }
    }

    /// The note being written on the strip shown, while one is.
    pub(crate) fn criteria_field_mut(&mut self) -> Option<&mut TextField> {
        let key = self.criteria_target()?.key();
        Some(&mut self.criteria_strip.drafts.get_mut(&key)?.field)
    }

    /// The pointer moved to `pos` over the strip: what is under it lights up,
    /// with its tooltip.
    pub(crate) fn criteria_strip_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let layout = self.strip_layout()?;
        let hit = layout.hit(pos);
        self.criteria_strip.hover = hit;
        let hit = hit?;
        let rect = layout.rect_of(hit)?;
        let (text, key): (String, &str) = match hit {
            StripHit::Prev => ("Previous criterion".into(), "Ctrl+PgUp"),
            StripHit::Next => ("Next criterion".into(), "Ctrl+PgDn"),
            StripHit::Title => ("Show its steps, or hide them".into(), ""),
            StripHit::Page => (
                "Show it on Manual test's page".into(),
                strip_chord(&StripKey::OnPage),
            ),
            StripHit::Fold => (
                "Fold the criteria to a chip".into(),
                strip_chord(&StripKey::Fold),
            ),
            StripHit::Chip => ("Unfold the criteria".into(), ""),
            StripHit::Shot => (
                "Add a screenshot of the page to the note".into(),
                strip_chord(&StripKey::Shot),
            ),
            StripHit::Done => (
                "Save the note".into(),
                chord_of(NOTE_KEYS, &[()], |k| *k == StripKey::Save),
            ),
            StripHit::Button(b) => {
                let target = self.criteria_target()?;
                let (rows, i) = self.strip_rows(&target)?;
                let (button, outcome) = strip_buttons(i, &rows[i]).into_iter().nth(b)?;
                let PanelOutcome::Manual(outcome) = outcome else {
                    let mut tip =
                        Tooltip::new("Add a comment of your own: what should change", rect);
                    tip.key = Some(strip_chord(&StripKey::AddComment).to_owned());
                    return Some(tip);
                };
                let (text, key) = button_tip(&outcome, &button.label);
                (text.into(), key)
            }
            StripHit::Field | StripHit::Panel => return None,
        };
        if text.is_empty() {
            return None;
        }
        let mut tip = Tooltip::new(&text, rect);
        tip.key = (!key.is_empty()).then(|| key.to_owned());
        Some(tip)
    }

    /// A left click at `pos` on the strip.
    pub(crate) fn criteria_strip_click(&mut self, pos: Point<f64, Logical>) {
        let Some(hit) = self.criteria_strip_hit(pos) else {
            return;
        };
        self.criteria_strip_press(hit);
    }

    /// What `hit` on the strip does when pressed.
    fn criteria_strip_press(&mut self, hit: StripHit) {
        let Some(target) = self.criteria_target() else {
            return;
        };
        match hit {
            StripHit::Prev => self.step_criterion(&target, -1),
            StripHit::Next => self.step_criterion(&target, 1),
            StripHit::Title => self.criteria_strip.open = !self.criteria_strip.open,
            StripHit::Button(b) => {
                let Some((rows, i)) = self.strip_rows(&target) else {
                    return;
                };
                let outcome = match strip_buttons(i, &rows[i]).into_iter().nth(b) {
                    Some((_, PanelOutcome::Manual(outcome))) => outcome,
                    Some((_, PanelOutcome::AddComment)) => {
                        if self.keep_strip_note(&target) {
                            self.ask_add_comment(&target.project, &target.slug);
                        }
                        return;
                    }
                    _ => return,
                };
                // A note being written is saved before what changes the
                // criterion's result; Fail… goes on with it.
                if !matches!(outcome, Outcome::Write(_)) && !self.keep_strip_note(&target) {
                    return;
                }
                self.strip_outcome(&target, outcome);
            }
            StripHit::Page => {
                if self.keep_strip_note(&target) {
                    self.criterion_on_page(&target);
                }
            }
            StripHit::Fold => {
                if self.keep_strip_note(&target) {
                    self.set_criteria_folded(true);
                }
            }
            StripHit::Chip => self.set_criteria_folded(false),
            // The caret goes where the click is (see `press_field`).
            StripHit::Field => self.criteria_strip.keys = Some(target.pane),
            StripHit::Shot => self.criterion_shot(&target),
            StripHit::Done => _ = self.keep_strip_note(&target),
            StripHit::Panel => {}
        }
    }

    /// Show criterion `by` from the one shown, going round; a note being
    /// written is saved first.
    fn step_criterion(&mut self, target: &Target, by: isize) {
        if !self.keep_strip_note(target) {
            return;
        }
        let Some((rows, i)) = self.strip_rows(target) else {
            return;
        };
        let n = rows.len() as isize;
        let to = (i as isize + by).rem_euclid(n) as usize;
        self.criteria_strip.at.insert(target.key(), to);
    }

    /// Show the criterion shown on Manual test's page.
    fn criterion_on_page(&mut self, target: &Target) {
        let Some((_, i)) = self.strip_rows(target) else {
            return;
        };
        self.drop_criteria_keys();
        self.open_feature_at(&target.project, &target.slug, Stage::ManualTest);
        if (self.features.panel.as_ref())
            .is_some_and(|p| p.project == target.project && p.slug == target.slug)
        {
            self.select_criterion(i);
        }
    }

    /// Whether the strip's feature can be changed here: one another machine
    /// writes is taken over first, which a press then waits for.
    fn strip_writes(&mut self, target: &Target) -> bool {
        if self.feature_writer(&target.project, &target.slug).is_some() {
            self.take_feature(&target.project, &target.slug, None);
            return false;
        }
        if self
            .feature_reading(&target.project, &target.slug)
            .is_none()
        {
            let title = (self.feature(&target.project, &target.slug)).map(|f| f.title.clone());
            self.probe_feature(&target.project, &target.slug);
            self.show_warning(format!(
                "Still reading the code of \u{201c}{}\u{201d}; try again in a moment",
                title.unwrap_or_default()
            ));
            return false;
        }
        true
    }

    /// Do what a criterion's button on the strip asked.
    fn strip_outcome(&mut self, target: &Target, outcome: Outcome) {
        let key = target.key();
        match outcome {
            Outcome::Write(i) => {
                let Some(c) = self
                    .features
                    .criteria(&target.project, &target.slug)
                    .get(i)
                    .cloned()
                else {
                    return;
                };
                if !self.strip_writes(target) {
                    return;
                }
                self.write_strip_note(target, c);
            }
            Outcome::Toggle(i) => {
                if !self.strip_writes(target) {
                    return;
                }
                self.apply_manual_test_on(&target.project, &target.slug, Outcome::Toggle(i));
                // Passed, the next not tried is up.
                if let Some((rows, _)) = self.strip_rows(target)
                    && rows.get(i).is_some_and(|r| r.found == Some(Verdict::Pass))
                    && let Some(next) = next_untried(&rows, i)
                {
                    self.criteria_strip.at.insert(key, next);
                }
            }
            Outcome::Clear(i) => {
                if self.strip_writes(target) {
                    self.apply_manual_test_on(&target.project, &target.slug, Outcome::Clear(i));
                }
            }
            // A comment's Not done; editing it opens its dialog.
            outcome @ (Outcome::Fail(..) | Outcome::EditComment(_)) => {
                if self.strip_writes(target) {
                    self.apply_manual_test_on(&target.project, &target.slug, outcome);
                }
            }
            Outcome::MarkPassed
            | Outcome::SendFailures
            | Outcome::MakeFixes
            | Outcome::DeleteComment(_) => {}
        }
    }

    /// Start writing the note of criterion `c` on the strip, from what went
    /// wrong as saved; one being written goes on.
    fn write_strip_note(&mut self, target: &Target, c: Criterion) {
        let key = target.key();
        if (self.criteria_strip.drafts.get(&key)).is_some_and(|d| d.criterion == c) {
            self.criteria_strip.keys = Some(target.pane);
            return;
        }
        // One on another criterion is saved first, never dropped.
        if !self.keep_strip_note(target) {
            return;
        }
        if (self.criteria_strip.parked.get(&key)).is_some_and(|d| d.criterion == c)
            && let Some(mut draft) = self.criteria_strip.parked.remove(&key)
        {
            draft.had_keys = self.criteria_strip_has_keys();
            self.criteria_strip.drafts.insert(key, draft);
            self.criteria_strip.keys = Some(target.pane);
            self.stop_key_repeat();
            return;
        }
        let note = (self.feature(&target.project, &target.slug))
            .and_then(|f| manual::check(f.manual.as_ref(), &c))
            .filter(|k| k.state == Verdict::Fail)
            .map(|k| k.note.clone())
            .unwrap_or_default();
        let field = TextField::multiline(&note);
        let had_keys = self.criteria_strip_has_keys();
        self.criteria_strip.drafts.insert(
            key,
            Draft {
                criterion: c,
                field,
                shots: Vec::new(),
                had_keys,
            },
        );
        self.criteria_strip.keys = Some(target.pane);
        self.stop_key_repeat();
    }

    /// Esc on the strip's note: put away unsaved, for the next note on the
    /// same criterion; with nothing written or taken, it just closes.
    fn park_strip_note(&mut self, target: &Target) {
        let key = target.key();
        let Some(draft) = self.criteria_strip.drafts.remove(&key) else {
            return;
        };
        if !draft.had_keys {
            self.drop_criteria_keys();
        }
        if !draft.field.trim().is_empty() || !draft.shots.is_empty() {
            self.criteria_strip.parked.insert(key, draft);
        }
    }

    /// Stop writing the strip's note, saving it as the criterion's failure if
    /// one was written; false when it can't be yet, and goes on being
    /// written. With nothing written and no screenshot, the criterion stays
    /// as it was.
    fn keep_strip_note(&mut self, target: &Target) -> bool {
        let key = target.key();
        let Some(draft) = self.criteria_strip.drafts.get(&key) else {
            return true;
        };
        let note = draft.field.trim().to_owned();
        if note.is_empty() && !draft.shots.is_empty() {
            self.show_warning("Say what went wrong first: the screenshot goes with it".into());
            return false;
        }
        if !note.is_empty() && !self.strip_writes(target) {
            return false;
        }
        let Some(draft) = self.criteria_strip.drafts.remove(&key) else {
            return true;
        };
        if !draft.had_keys {
            self.drop_criteria_keys();
        }
        if note.is_empty() {
            return true;
        }
        let i = (self.features.criteria(&target.project, &target.slug).iter())
            .position(|c| *c == draft.criterion);
        let Some(i) = i else {
            // The brief changed under it: the note isn't lost.
            self.criteria_strip.drafts.insert(key, draft);
            self.show_warning("That criterion changed in the brief; copy the note over".into());
            return false;
        };
        self.apply_manual_test_on(&target.project, &target.slug, Outcome::Fail(i, note));
        if !draft.shots.is_empty() {
            let added = (self.feature_mut(&target.project, &target.slug))
                .is_some_and(|f| manual::add_shots(&mut f.manual, &draft.criterion, &draft.shots));
            if added {
                self.save_feature(&target.project, &target.slug);
            } else {
                tracing::warn!(
                    "screenshots of {} in {}/{} kept on no failure",
                    draft.criterion.id,
                    target.project,
                    target.slug
                );
            }
        }
        true
    }

    /// Ask the strip's page for a screenshot, for the note of the criterion
    /// shown: one is started if none is written.
    fn criterion_shot(&mut self, target: &Target) {
        if self.criteria_strip.shot.is_some() {
            return self.show_warning("Still taking the last screenshot".into());
        }
        let Some((_, i)) = self.strip_rows(target) else {
            return;
        };
        let Some(c) = self
            .features
            .criteria(&target.project, &target.slug)
            .get(i)
            .cloned()
        else {
            return;
        };
        if !self.strip_writes(target) {
            return;
        }
        if !self.capture_web_pane(target.pane) {
            return self.show_warning(
                "This page can't be captured: its browser runs without gerdr's extension".into(),
            );
        }
        self.write_strip_note(target, c.clone());
        let since = Instant::now();
        self.criteria_strip.shot = Some(ShotWait {
            pane: target.pane,
            feature: target.key(),
            criterion: c,
            since,
        });
        let wait = move |_, _: &mut (), gerdr: &mut Gerdr| {
            if (gerdr.criteria_strip.shot.as_ref()).is_some_and(|w| w.since == since) {
                gerdr.criteria_strip.shot = None;
                tracing::warn!("no screenshot came in {SHOT_WAIT:?}");
                gerdr.show_warning("The page sent no screenshot; try again".into());
            }
            TimeoutAction::Drop
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(Timer::from_duration(SHOT_WAIT), wait)
        {
            tracing::warn!("screenshot wait: {err}");
        }
    }

    /// What `pane`'s page showed came back, as a PNG or why there is none.
    /// One nobody waits for any more (it came after `SHOT_WAIT`, said then)
    /// is dropped.
    pub(crate) fn web_pane_shot(&mut self, pane: u32, shot: Result<Vec<u8>, String>) {
        let Some(wait) = (self.criteria_strip.shot.take()).filter(|w| w.pane == pane) else {
            return;
        };
        let png = match shot {
            Ok(png) => png,
            Err(why) => {
                tracing::warn!("screenshot of pane {pane}: {why}");
                return self.show_error(format!("The screenshot failed: {why}"));
            }
        };
        let (project, slug) = &wait.feature;
        let dir = store::dir(None, project, slug).join(manual::SHOTS_DIR);
        let taken: Vec<String> = match std::fs::read_dir(&dir) {
            Ok(entries) => (entries.flatten())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        let name = manual::shot_name(&wait.criterion.id, &taken);
        let written =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(&name), &png));
        if let Err(err) = written {
            tracing::warn!("saving screenshot {}: {err}", dir.join(&name).display());
            return self.show_error(format!("gerdr couldn't save the screenshot: {err}"));
        }
        tracing::info!("screenshot {name} of {project}/{slug} saved");
        let id = wait.criterion.id.clone();
        match self.criteria_strip.drafts.get_mut(&wait.feature) {
            Some(draft) if draft.criterion == wait.criterion => draft.shots.push(name),
            // Its note was saved meanwhile: it goes with the failure.
            _ => {
                let added = (self.feature_mut(project, slug))
                    .is_some_and(|f| manual::add_shots(&mut f.manual, &wait.criterion, &[name]));
                if !added {
                    return self.show_warning(format!(
                        "The screenshot of {id} is saved, but {id} isn't failed to keep it"
                    ));
                }
                let (project, slug) = (project.clone(), slug.clone());
                self.save_feature(&project, &slug);
            }
        }
        self.show_success(format!("Screenshot added to {id}"));
    }

    /// A key while the strip has the keys.
    pub(crate) fn criteria_strip_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) {
        let Some(target) = self.criteria_target() else {
            return self.drop_criteria_keys();
        };
        if self.criteria_strip.drafts.contains_key(&target.key()) {
            match key_table::press(NOTE_KEYS, &(), mods, sym) {
                Some(StripKey::Save) => _ = self.keep_strip_note(&target),
                Some(StripKey::Newline) => {
                    if let Some(field) = self.criteria_field_mut() {
                        field.newline();
                    }
                }
                Some(StripKey::Park) => self.park_strip_note(&target),
                Some(StripKey::Discard) => {
                    self.criteria_strip.parked.remove(&target.key());
                    let draft = self.criteria_strip.drafts.remove(&target.key());
                    if draft.is_some_and(|d| !d.had_keys) {
                        self.drop_criteria_keys();
                    }
                }
                Some(StripKey::Shot) => self.criterion_shot(&target),
                _ => {
                    if let Some(field) = self.criteria_field_mut() {
                        field.key(mods, sym, typed);
                    }
                }
            }
            return;
        }
        let Some((rows, i)) = self.strip_rows(&target) else {
            return;
        };
        let ctx = StripCtx {
            i,
            comment: rows[i].comment,
            found: rows[i].found.is_some(),
        };
        match key_table::press(STRIP_KEYS, &ctx, mods, sym) {
            Some(StripKey::Do(outcome)) => self.strip_outcome(&target, outcome),
            Some(StripKey::AddComment) => self.ask_add_comment(&target.project, &target.slug),
            Some(StripKey::Close) => self.drop_criteria_keys(),
            Some(StripKey::OnPage) => self.criterion_on_page(&target),
            Some(StripKey::Fold) => self.set_criteria_folded(true),
            Some(StripKey::Shot) => self.criterion_shot(&target),
            Some(_) => {}
            // Stepping through the criteria, as a list's arrows: its
            // buttons' tooltips name Ctrl+PgUp / Ctrl+PgDn, which
            // `hint_key` doesn't read.
            None => {
                let bare = !(mods.ctrl || mods.alt || mods.shift || mods.logo);
                match sym {
                    Keysym::Page_Up if mods.ctrl => self.step_criterion(&target, -1),
                    Keysym::Page_Down if mods.ctrl => self.step_criterion(&target, 1),
                    Keysym::Up | Keysym::KP_Up if bare => self.step_criterion(&target, -1),
                    Keysym::Down | Keysym::KP_Down if bare => self.step_criterion(&target, 1),
                    _ => {}
                }
            }
        }
    }

    /// The sandbox's app of feature `slug` opened: the strip over it reads
    /// the feature's criteria and code.
    pub(crate) fn sandbox_shown(&mut self, project: &str, slug: &str) {
        self.probe_feature(project, slug);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_page_is_one_of_this_machine_or_the_set_one() {
        assert!(sandbox_page("http://localhost:3000/rows", None));
        assert!(sandbox_page("http://127.0.0.1:8080", None));
        assert!(sandbox_page("http://[::1]:8080/a", None));
        // T3 Code's pages, not the sandbox's.
        assert!(!sandbox_page("http://app.localhost:5173/", None));
        assert!(!sandbox_page("https://github.com/x/y", None));
        assert!(!sandbox_page("http://localhostile.com", None));
        let set = Some("http://app.test:8000/home");
        assert!(sandbox_page("http://APP.test:8000/rows?x=1", set));
        assert!(!sandbox_page("http://localhost:3000", set));
        assert!(!sandbox_page("not a page", None));
    }

    fn row(found: Option<Verdict>) -> Row {
        Row {
            id: "AC-1".into(),
            title: "T".into(),
            text: "Given a, when b, then c".into(),
            found,
            note: String::new(),
            sent: false,
            made: false,
            earlier: false,
            retest: None,
            shots: Vec::new(),
            comment: false,
            open: false,
            seen: false,
            planning: false,
        }
    }

    #[test]
    fn a_pass_moves_on_to_the_next_not_tried_going_round() {
        let pass = Some(Verdict::Pass);
        let rows = vec![row(None), row(pass), row(pass), row(None)];
        assert_eq!(first_untried(&rows), 0);
        assert_eq!(next_untried(&rows, 0), Some(3));
        assert_eq!(next_untried(&rows, 3), Some(0));
        let done = vec![row(pass), row(pass)];
        assert_eq!(next_untried(&done, 0), None);
        assert_eq!(first_untried(&done), 0);
    }

    #[test]
    fn buttons_say_their_keys() {
        assert_eq!(button_tip(&Outcome::Toggle(0), "Pass").1, "Space");
        assert_eq!(button_tip(&Outcome::Write(0), "Fail\u{2026}").1, "F");
        assert_eq!(button_tip(&Outcome::Clear(0), "Fail").1, "Del");
        assert_eq!(shots_text(2), "2 screenshots");
    }

    #[test]
    fn the_strips_keys_are_one_table_its_tooltips_read() {
        key_table::check(STRIP_KEYS, &STRIP_STATES);
        key_table::check(NOTE_KEYS, &[()]);
        let press = |ctx: &StripCtx, chord: &str| {
            let (mods, sym) = crate::keys::hint_key(chord).unwrap();
            key_table::press(STRIP_KEYS, ctx, &mods, sym)
        };
        let [untried, tried, comment] = &STRIP_STATES;
        assert_eq!(
            press(tried, "Enter"),
            Some(StripKey::Do(Outcome::Toggle(0)))
        );
        assert_eq!(press(untried, "F"), Some(StripKey::Do(Outcome::Write(0))));
        assert_eq!(
            press(comment, "F"),
            Some(StripKey::Do(Outcome::Fail(0, String::new())))
        );
        // Del clears a criterion tried, not a comment or one not tried.
        assert_eq!(press(tried, "Del"), Some(StripKey::Do(Outcome::Clear(0))));
        assert_eq!(press(untried, "Del"), None);
        assert_eq!(press(comment, "Del"), None);
        // A press gives `f` and `c` as their own symbols, no Shift (K8).
        let bare = ModifiersState::default();
        assert!(key_table::press(STRIP_KEYS, untried, &bare, Keysym::c).is_some());
        // Presses that worked before the table still do.
        assert_eq!(press(untried, "Shift+Esc"), Some(StripKey::Close));
        assert_eq!(press(untried, "Alt+Shift+Down"), Some(StripKey::Fold));
        assert_eq!(press(untried, "Ctrl+Super+O"), Some(StripKey::OnPage));
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        let kp = key_table::press(STRIP_KEYS, untried, &alt, Keysym::KP_Down);
        assert_eq!(kp, Some(StripKey::Fold));
        assert_eq!(strip_chord(&StripKey::AddComment), "C");
        assert_eq!(strip_chord(&StripKey::Fold), "Alt+Down");
        assert_eq!(
            chord_of(NOTE_KEYS, &[()], |k| *k == StripKey::Save),
            "Enter"
        );
    }
}
