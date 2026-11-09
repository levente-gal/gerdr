//! The plan review's sheet: a feature's suggestions, one row each, to accept,
//! reject or say something of, with all the one under the cursor says below
//! them. The decisions save as they are made (`Feature::plan_review`, see
//! `feature::plan_review`). The finishing button starts the final plan, or
//! with nothing accepted uses the plan as it is; Ask reviewer sends the notes
//! to the reviewer, whose rewrite is read in while the sheet stays open: the
//! cursor stays on its suggestion, by id.
//!
//! The model and its keys are here, the painting in `chrome::plan_sheet`.

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{
        self, PLAN_ASK, PLAN_CLOSE, PLAN_FINISH, PlanCitation, PlanDetail, PlanField, PlanHit,
        PlanMark, PlanRow, PlanSheetView,
    },
    feature::{
        Stage, Status,
        plan_review::{self, Decision, Kind, PlanReview, Source, State, Suggestion},
    },
    feature_plan_review::deciding,
    features::plural,
    menu::MenuAction,
    scroll::Motion,
    text_field::{Edit, TextField},
    toast::Level,
    tooltip::Tooltip,
};

/// What the note editor says when Enter is pressed on a question with no
/// answer written.
const NO_ANSWER: &str = "Answer the question first: write it as a note";
/// What the add field says when Enter is pressed with nothing written.
const NO_TEXT: &str = "Say what to change in the plan first";
const WRITE_FINAL: &str = "Write final plan";
/// The reason `r` rejects with: the sheet asks for none.
const SHEET_REASON: &str = "Rejected on the review sheet";
const USE_AS_IS: &str = "Use the plan as it is";

/// What the sheet's text field is writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Writing {
    /// Suggestion `id`'s note; `accept`: accept it once the note is saved.
    Note { id: String, accept: bool },
    /// A suggestion of the user's own.
    Add,
}

/// The sheet's state, while it is open.
#[derive(Debug, Default)]
pub struct PlanSheet {
    pub project: String,
    pub slug: String,
    /// The suggestion with the keyboard, by id: rewrites move it around.
    pub at: Option<String>,
    /// Where it was, for when it is gone.
    pub cursor: usize,
    /// The first row shown.
    pub top: usize,
    /// The row last scrolled into view: the wheel may have scrolled it out,
    /// and only a key or the cursor moving brings it back.
    pub revealed: Option<usize>,
    /// The first line shown of what the suggestion says.
    pub detail_top: usize,
    /// The first key line shown, with F1.
    pub keys_top: usize,
    pub editing: Option<(Writing, TextField)>,
    /// The suggestion of the user's own that a second Del removes.
    pub armed: Option<String>,
    /// Why the last thing asked was refused.
    pub notice: Option<String>,
    pub hover: Option<PlanHit>,
    /// The keys list shows instead of the rest (F1).
    pub keys: bool,
}

/// What the sheet asks gerdr to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    /// Decide suggestion `.0`.
    Decide(String, Decision),
    /// Save suggestion `id`'s note, and accept it if `accept`.
    Note {
        id: String,
        note: String,
        accept: bool,
    },
    /// Add this suggestion of the user's own.
    Add(String),
    /// Keep this as the draft of a suggestion of the user's own.
    KeepDraft(String),
    /// Open citation `.1` of suggestion `.0` in Zed.
    Citation(String, usize),
    /// Start the final plan, or use the plan as it is.
    Finish,
    /// Send the notes to the reviewer.
    Ask,
}

/// What a key is pressed on.
struct Shown<'a> {
    rows: &'a [PlanRow],
    /// The draft of a suggestion of the user's own.
    draft: &'a str,
    /// How many lines PgUp and PgDn go.
    page: usize,
}

/// The keys of the sheet and what they do, for F1.
fn keys_list() -> Vec<(String, String)> {
    [
        ("Up / Down", "move between the suggestions"),
        ("Ctrl+J / Ctrl+K", "move between the suggestions"),
        ("Home / End", "go to the first or last suggestion"),
        ("PgUp / PgDn", "scroll what the suggestion says"),
        (
            "Space",
            "accept it, or clear that; a question wants its answer",
        ),
        ("r", "reject it, or clear that"),
        ("n", "write a note on it"),
        ("a", "add a suggestion of your own"),
        ("Del", "clear its decision, or remove one of your own"),
        ("Ctrl+O", "open its first citation in Zed"),
        (
            "Ctrl+Enter",
            "write the final plan, or use the plan as it is",
        ),
        ("Ctrl+Shift+Enter", "send your notes to the reviewer"),
        ("F1", "hide these keys"),
        ("Esc", "go back, then close the sheet"),
    ]
    .into_iter()
    .map(|(k, w)| (k.to_owned(), w.to_owned()))
    .collect()
}

impl PlanSheet {
    /// The row with the keyboard: by its id while that is there, else where
    /// it was.
    fn row_at(&self, rows: &[PlanRow]) -> usize {
        (self.at.as_ref())
            .and_then(|id| rows.iter().position(|r| r.id == *id))
            .unwrap_or_else(|| self.cursor.min(rows.len().saturating_sub(1)))
    }

    /// Follow what the rows are now: the cursor to its suggestion, or, with
    /// that gone, the one in its place.
    fn settle(&mut self, rows: &[PlanRow]) {
        let i = self.row_at(rows);
        self.cursor = i;
        let id = rows.get(i).map(|r| r.id.clone());
        if id != self.at {
            self.at = id;
            self.detail_top = 0;
        }
    }

    /// Put the cursor on row `i`.
    fn go(&mut self, i: usize, rows: &[PlanRow]) {
        self.cursor = i;
        self.at = rows.get(i).map(|r| r.id.clone());
        self.detail_top = 0;
    }

    /// Start writing: `text` is what is there so far.
    fn write(&mut self, writing: Writing, text: &str) {
        let mut field = TextField::default();
        field.set(text);
        self.editing = Some((writing, field));
        self.notice = None;
        self.armed = None;
    }

    /// Stop writing, keeping what was written, without acting on it: a note
    /// is saved, not accepted; a suggestion stays a draft. Nothing written
    /// on a note leaves the note it had.
    fn take_text(&mut self) -> Option<Outcome> {
        let (writing, field) = self.editing.take()?;
        self.notice = None;
        let text = field.trim().to_owned();
        match writing {
            Writing::Note { id, .. } => (!text.is_empty()).then_some(Outcome::Note {
                id,
                note: text,
                accept: false,
            }),
            Writing::Add => Some(Outcome::KeepDraft(text)),
        }
    }

    /// A key while the text field is open: it takes them all.
    fn field_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) -> Option<Outcome> {
        if sym == Keysym::Escape {
            return self.take_text();
        }
        let (writing, field) = self.editing.as_mut()?;
        if !matches!(sym, Keysym::Return | Keysym::KP_Enter) {
            if field.key(mods, sym, typed) == Edit::Changed {
                self.notice = None;
            }
            return None;
        }
        let text = field.trim().to_owned();
        let refused = match writing {
            Writing::Note { accept: true, .. } if text.is_empty() => Some(NO_ANSWER),
            Writing::Add if text.is_empty() => Some(NO_TEXT),
            _ => None,
        };
        if let Some(why) = refused {
            self.notice = Some(why.into());
            return None;
        }
        let (writing, _) = self.editing.take()?;
        self.notice = None;
        Some(match writing {
            Writing::Note { id, accept } => Outcome::Note {
                id,
                note: text,
                accept,
            },
            Writing::Add => Outcome::Add(text),
        })
    }

    /// Space, or a click on the mark: accept row `i`, or clear it when it
    /// is. A question not answered gets its answer first. One of the user's
    /// own has no undecided: clearing it would remove it, which is Del's.
    fn toggle(&mut self, i: usize, rows: &[PlanRow]) -> Option<Outcome> {
        let row = rows.get(i)?;
        let id = row.id.clone();
        if row.mark == PlanMark::Accepted {
            if row.yours {
                self.notice = Some("Yours stays accepted until you reject it or remove it".into());
                return None;
            }
            return Some(Outcome::Decide(id, Decision::Clear));
        }
        if row.kind == Kind::Question && row.note.trim().is_empty() {
            self.write(Writing::Note { id, accept: true }, "");
            return None;
        }
        Some(Outcome::Decide(id, Decision::Accept))
    }

    /// `r`: reject row `i`, or clear it when it is.
    fn reject(&mut self, i: usize, rows: &[PlanRow]) -> Option<Outcome> {
        let row = rows.get(i)?;
        let id = row.id.clone();
        if row.mark != PlanMark::Rejected {
            return Some(Outcome::Decide(
                id,
                Decision::Reject(SHEET_REASON.to_owned()),
            ));
        }
        if row.yours {
            self.notice = Some("Yours stays rejected until you accept it or remove it".into());
            return None;
        }
        Some(Outcome::Decide(id, Decision::Clear))
    }

    /// Handle a key: `sym` is the key's own symbol, `typed` what it types.
    fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        shown: &Shown,
    ) -> Option<Outcome> {
        if self.editing.is_some() {
            return self.field_key(mods, sym, typed);
        }
        if sym.is_modifier_key() {
            return None;
        }
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        if self.keys {
            match sym {
                Keysym::Escape | Keysym::F1 => self.keys = false,
                Keysym::Down | Keysym::KP_Down if plain => self.keys_top += 1,
                Keysym::Up | Keysym::KP_Up if plain => {
                    self.keys_top = self.keys_top.saturating_sub(1);
                }
                Keysym::Page_Down | Keysym::KP_Page_Down if plain => {
                    self.keys_top += shown.page;
                }
                Keysym::Page_Up | Keysym::KP_Page_Up if plain => {
                    self.keys_top = self.keys_top.saturating_sub(shown.page);
                }
                _ => {}
            }
            return None;
        }
        let rows = shown.rows;
        self.settle(rows);
        let armed = self.armed.take();
        self.notice = None;
        let last = rows.len().saturating_sub(1);
        let i = self.cursor;
        let any = !rows.is_empty();
        match sym {
            Keysym::Escape if armed.is_some() => {}
            Keysym::Escape => return Some(Outcome::Close),
            Keysym::F1 => {
                self.keys = true;
                self.keys_top = 0;
            }
            Keysym::Down | Keysym::KP_Down if plain => self.go((i + 1).min(last), rows),
            Keysym::Up | Keysym::KP_Up if plain => self.go(i.saturating_sub(1), rows),
            Keysym::j | Keysym::J if ctrl => self.go((i + 1).min(last), rows),
            Keysym::k | Keysym::K if ctrl => self.go(i.saturating_sub(1), rows),
            Keysym::Home | Keysym::KP_Home if plain => self.go(0, rows),
            Keysym::End | Keysym::KP_End if plain => self.go(last, rows),
            Keysym::Page_Down | Keysym::KP_Page_Down if plain => {
                self.detail_top += shown.page;
            }
            Keysym::Page_Up | Keysym::KP_Page_Up if plain => {
                self.detail_top = self.detail_top.saturating_sub(shown.page);
            }
            Keysym::Return | Keysym::KP_Enter if ctrl && mods.shift => return Some(Outcome::Ask),
            Keysym::Return | Keysym::KP_Enter if ctrl => return Some(Outcome::Finish),
            Keysym::o | Keysym::O if ctrl && !mods.shift && any => {
                return Some(Outcome::Citation(rows[i].id.clone(), 0));
            }
            Keysym::space if plain && any => return self.toggle(i, rows),
            Keysym::r if plain && !mods.shift && any => return self.reject(i, rows),
            Keysym::n if plain && !mods.shift && any => {
                let row = &rows[i];
                let writing = Writing::Note {
                    id: row.id.clone(),
                    accept: false,
                };
                self.write(writing, &row.note);
            }
            Keysym::a if plain && !mods.shift => self.write(Writing::Add, shown.draft),
            Keysym::Delete | Keysym::KP_Delete if plain && any => {
                let row = &rows[i];
                if row.yours && armed.as_ref() != Some(&row.id) {
                    self.armed = Some(row.id.clone());
                    return None;
                }
                return Some(Outcome::Decide(row.id.clone(), Decision::Clear));
            }
            _ => {}
        }
        None
    }

    /// The key hints of the footer, as the sheet is now. What the footer
    /// can't show waits in F1; `finish` says what Ctrl+Enter does.
    fn hints(&self, rows: &[PlanRow], finish: &str) -> Vec<(String, String)> {
        let finish = if finish == WRITE_FINAL {
            "final plan"
        } else {
            "use the plan"
        };
        let hints: Vec<(&str, &str)> = if self.keys {
            vec![("Esc", "back")]
        } else if let Some((writing, field)) = &self.editing {
            let empty = field.trim().is_empty();
            match writing {
                Writing::Note { accept: true, .. } => vec![
                    ("Enter", "answer and accept"),
                    ("Esc", if empty { "cancel" } else { "keep the answer" }),
                ],
                Writing::Note { .. } => vec![
                    ("Enter", "save"),
                    ("Esc", if empty { "cancel" } else { "save" }),
                ],
                Writing::Add => vec![
                    ("Enter", "add"),
                    ("Esc", if empty { "cancel" } else { "keep the draft" }),
                ],
            }
        } else if self.armed.is_some() {
            vec![("Del", "remove"), ("Esc", "keep it")]
        } else if rows.is_empty() {
            vec![
                ("a", "add\u{2026}"),
                ("Ctrl+Enter", finish),
                ("F1", "keys"),
                ("Esc", "close"),
            ]
        } else {
            let at = rows.get(self.row_at(rows));
            let del = if at.is_some_and(|r| r.yours) {
                "remove"
            } else {
                "clear"
            };
            vec![
                ("Space", "accept"),
                ("r", "reject"),
                ("n", "note\u{2026}"),
                ("a", "add\u{2026}"),
                ("Del", del),
                ("Ctrl+Enter", finish),
                ("Ctrl+Shift+Enter", "ask"),
                ("F1", "keys"),
                ("Esc", "close"),
            ]
        };
        hints
            .iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect()
    }
}

/// The rows of the sheet: `review`'s suggestions.
fn rows(review: &PlanReview) -> Vec<PlanRow> {
    (review.suggestions.iter())
        .map(|s| PlanRow {
            id: s.id.clone(),
            mark: match s.state {
                State::Open => PlanMark::Open,
                State::Accepted => PlanMark::Accepted,
                State::Rejected => PlanMark::Rejected,
            },
            severity: s.severity,
            kind: s.kind,
            title: s.title.clone(),
            plan: s.plan.clone(),
            yours: s.source == Source::You,
            dropped: s.dropped,
            note: s.note.clone(),
        })
        .collect()
}

/// All `s` says, as the sheet shows it.
fn detail(s: &Suggestion) -> PlanDetail {
    PlanDetail {
        title: s.title.clone(),
        plan: s.plan.clone(),
        dropped: s.dropped,
        question: s.kind == Kind::Question,
        why: s.why.clone(),
        change: s.change.clone(),
        evidence: (s.evidence.iter())
            .map(|e| {
                let place = format!("{}:{}", e.file, e.line);
                PlanCitation {
                    place: if e.repo.is_empty() {
                        place
                    } else {
                        format!("{}: {place}", e.repo)
                    },
                    quote: e.quote.clone(),
                    not_found: e.found == Some(false),
                }
            })
            .collect(),
        note: s.note.clone(),
    }
}

/// How `review` stands, in words: "3 to decide · 2 accepted · 1 rejected".
fn counts_text(review: &PlanReview) -> String {
    let mut parts = Vec::new();
    for (n, what) in [
        (review.open(), "to decide"),
        (review.count(State::Accepted), "accepted"),
        (review.count(State::Rejected), "rejected"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    if parts.is_empty() {
        parts.push("no suggestions".to_owned());
    }
    if !review.draft.is_empty() {
        parts.push("draft kept".to_owned());
    }
    parts.join(" \u{b7} ")
}

/// The finishing button's label.
fn finish_label(review: &PlanReview) -> &'static str {
    if review.any_accepted() {
        WRITE_FINAL
    } else {
        USE_AS_IS
    }
}

/// The sheet's button to press next: the finishing one once none is open.
fn primary_button(review: &PlanReview) -> Option<usize> {
    (review.open() == 0).then_some(PLAN_FINISH)
}

/// Why the finishing action has to wait, if it does.
fn finish_refused(review: &PlanReview) -> Option<String> {
    let open = review.open();
    (open > 0).then(|| format!("{} still to decide", plural(open, "suggestion")))
}

/// Why there is nothing to ask the reviewer, if there isn't.
fn ask_refused(review: &PlanReview) -> Option<String> {
    plan_review::notes(review)
        .is_empty()
        .then(|| "Nothing to ask: write a note on a suggestion first".to_owned())
}

impl Gerdr {
    /// Open the sheet on feature `slug` of `project`.
    // Show in plan opens the Suggestions tab now; Wave 2 removes the sheet.
    #[allow(dead_code)]
    pub(crate) fn open_plan_sheet(&mut self, project: &str, slug: &str) {
        let Some(feature) = self.feature(project, slug) else {
            return self.show_warning("That feature is gone".into());
        };
        let Some(review) = deciding(feature) else {
            return self.show_warning("The suggestions aren't read yet".into());
        };
        // The first still to decide, else the first.
        let open = (review.suggestions.iter()).position(|s| s.state == State::Open);
        let cursor = open.unwrap_or(0);
        let at = review.suggestions.get(cursor).map(|s| s.id.clone());
        self.stop_key_repeat();
        self.close_menu();
        self.tooltip.hide();
        self.chrome.drop_tooltip();
        self.plan_sheet = Some(PlanSheet {
            project: project.to_owned(),
            slug: slug.to_owned(),
            at,
            cursor,
            ..PlanSheet::default()
        });
        self.plan_sheet_fit();
        // It takes the pointer from the client under it.
        self.refresh_pointer_focus();
    }

    /// Close the sheet.
    pub(crate) fn close_plan_sheet(&mut self) {
        self.plan_sheet = None;
        self.refresh_pointer_focus();
    }

    /// The review of the sheet open, as it is now.
    fn plan_sheet_review(&self) -> Option<&PlanReview> {
        let sheet = self.plan_sheet.as_ref()?;
        self.feature(&sheet.project, &sheet.slug)?
            .plan_review
            .as_ref()
    }

    /// Follow what happened to the sheet's feature: close once it is gone,
    /// or no longer has suggestions to decide (the final plan started, the
    /// stage done), where its buttons would only be refused. A text being
    /// written keeps it open till that is done, if it has a feature to be
    /// kept in. A rewrite read in moves its rows: the cursor stays on its
    /// suggestion.
    pub(crate) fn plan_sheet_follows(&mut self) {
        let Some(sheet) = &self.plan_sheet else {
            return;
        };
        let (project, slug) = (sheet.project.clone(), sheet.slug.clone());
        let writing = sheet.editing.is_some();
        // Nowhere for a text to be kept, and nothing to show it over.
        let feature = self.feature(&project, &slug);
        let Some(feature) = feature.filter(|f| f.plan_review.is_some()) else {
            tracing::info!("closing the plan review's sheet of {project}/{slug}: it is gone");
            return self.close_plan_sheet();
        };
        let open = feature.status == Status::Open
            && feature.current() == Some(Stage::PlanReview)
            && deciding(feature).is_some();
        if !open && !writing {
            tracing::info!(
                "closing the plan review's sheet of {project}/{slug}: no suggestions to decide"
            );
            return self.close_plan_sheet();
        }
        self.plan_sheet_fit();
    }

    /// Bring the sheet's cursor and scrolling in line with what it shows.
    fn plan_sheet_fit(&mut self) {
        let rows = self.plan_sheet_review().map(rows).unwrap_or_default();
        if let Some(sheet) = self.plan_sheet.as_mut() {
            sheet.settle(&rows);
        }
        let Some(view) = self.plan_sheet_view() else {
            return;
        };
        let output = self.output_size();
        let reveal = chrome::plan_sheet_reveal(&view, output);
        let most = chrome::plan_sheet_most_top(&view, output);
        let layout = chrome::plan_sheet_layout(&view, output);
        if let Some(sheet) = self.plan_sheet.as_mut() {
            if sheet.revealed == Some(sheet.cursor) {
                sheet.top = sheet.top.min(most);
            } else {
                sheet.top = reveal;
                sheet.revealed = Some(sheet.cursor);
            }
            if sheet.keys {
                sheet.keys_top = sheet.keys_top.min(layout.keys_most);
            } else {
                sheet.detail_top = sheet.detail_top.min(layout.detail_most);
            }
        }
    }

    /// A key while the sheet has the keyboard.
    pub(crate) fn plan_sheet_key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
    ) -> Option<Outcome> {
        let review = self.plan_sheet_review();
        let rows = review.map(rows).unwrap_or_default();
        let draft = review.map(|r| r.draft.clone()).unwrap_or_default();
        let view = self.plan_sheet_view();
        let layout = view.map(|v| chrome::plan_sheet_layout(&v, self.output_size()));
        let sheet = self.plan_sheet.as_ref()?;
        let page = layout.map_or(1, |l| {
            if sheet.keys {
                l.keys.len().saturating_sub(1).max(1)
            } else {
                l.page
            }
        });
        let shown = Shown {
            rows: &rows,
            draft: &draft,
            page,
        };
        let sheet = self.plan_sheet.as_mut()?;
        // A key is for the row with the cursor, which should show.
        if !sym.is_modifier_key() {
            sheet.revealed = None;
        }
        let out = sheet.key(mods, sym, typed, &shown);
        // A text done may be what kept it open (see `plan_sheet_follows`).
        if out.is_none() {
            self.plan_sheet_follows();
        }
        out
    }

    /// Refuse what the sheet asked, saying why on it.
    fn plan_sheet_refuse(&mut self, why: String) {
        if let Some(sheet) = self.plan_sheet.as_mut() {
            sheet.notice = Some(why);
        }
    }

    /// Keep `text` as the draft of a suggestion of the user's own.
    fn keep_suggestion_draft(&mut self, project: &str, slug: &str, text: String) {
        let Some(review) = self
            .feature_mut(project, slug)
            .and_then(|f| f.plan_review.as_mut())
        else {
            return;
        };
        if review.draft != text {
            review.draft = text;
            self.save_feature(project, slug);
        }
    }

    /// Do what the sheet asked.
    pub(crate) fn apply_plan_sheet(&mut self, outcome: Outcome) {
        let Some(sheet) = self.plan_sheet.as_ref() else {
            return;
        };
        let (project, slug) = (sheet.project.clone(), sheet.slug.clone());
        if self.feature(&project, &slug).is_none() {
            self.close_plan_sheet();
            return self.show_warning("That feature is gone".into());
        }
        match outcome {
            Outcome::Close => {
                // A text being written is kept (see `closing` in `input`).
                let kept = (self.plan_sheet.as_mut()).and_then(PlanSheet::take_text);
                if let Some(kept) = kept {
                    self.apply_plan_sheet(kept);
                }
                self.close_plan_sheet();
            }
            Outcome::Decide(id, decision) => self.plan_sheet_decide(&project, &slug, &id, decision),
            Outcome::Note { id, note, accept } => {
                self.plan_sheet_note(&project, &slug, &id, &note, accept);
            }
            Outcome::Add(text) => {
                if let Some(review) = self
                    .feature_mut(&project, &slug)
                    .and_then(|f| f.plan_review.as_mut())
                {
                    review.draft.clear();
                }
                self.add_suggestion(&project, &slug, &text);
                // On what was just written.
                let last = self
                    .plan_sheet_review()
                    .and_then(|r| r.suggestions.last().map(|s| s.id.clone()));
                if let Some(sheet) = self.plan_sheet.as_mut() {
                    sheet.at = last;
                    sheet.detail_top = 0;
                }
            }
            Outcome::KeepDraft(text) => self.keep_suggestion_draft(&project, &slug, text),
            Outcome::Citation(id, i) => self.plan_sheet_citation(&project, &slug, &id, i),
            Outcome::Finish => {
                if let Some(why) = self.plan_sheet_review().and_then(finish_refused) {
                    return self.plan_sheet_refuse(why);
                }
                self.close_plan_sheet();
                self.write_final_plan(&project, &slug);
            }
            Outcome::Ask => {
                if let Some(why) = self.plan_sheet_review().and_then(ask_refused) {
                    return self.plan_sheet_refuse(why);
                }
                self.ask_reviewer(&project, &slug);
            }
        }
        self.plan_sheet_follows();
    }

    /// Decide suggestion `id`, saying why when it can't be. Removing one of
    /// the user's own can be undone.
    fn plan_sheet_decide(&mut self, project: &str, slug: &str, id: &str, decision: Decision) {
        let removed = (decision == Decision::Clear)
            .then(|| self.plan_sheet_review())
            .flatten()
            .and_then(|r| {
                let at = r.suggestions.iter().position(|s| s.id == id)?;
                let s = &r.suggestions[at];
                (s.source == Source::You).then(|| (at, s.clone()))
            });
        let done = match removed {
            Some(_) => self.delete_own_suggestion(project, slug, id),
            None => self.decide_suggestion(project, slug, id, decision),
        };
        if let Err(why) = done {
            return self.plan_sheet_refuse(why);
        }
        if let Some((at, suggestion)) = removed {
            let undo = MenuAction::RestoreSuggestion {
                project: project.to_owned(),
                slug: slug.to_owned(),
                at,
                suggestion: Box::new(suggestion),
            };
            self.toast_with(
                "Removed your suggestion".into(),
                Level::Success,
                Some(("Undo".to_owned(), undo)),
            );
        }
    }

    /// Save suggestion `id`'s note, and accept it if `accept`. A note whose
    /// suggestion is gone stays, as the draft of one of the user's own.
    fn plan_sheet_note(&mut self, project: &str, slug: &str, id: &str, note: &str, accept: bool) {
        let Some(s) = self.plan_sheet_review().and_then(|r| r.suggestion(id)) else {
            let draft = self.plan_sheet_review().map(|r| r.draft.clone());
            let text = match draft.as_deref() {
                None | Some("") => note.to_owned(),
                Some(draft) => format!("{draft} {note}"),
            };
            self.keep_suggestion_draft(project, slug, text);
            let why = "That suggestion is gone: what you wrote is kept as a draft";
            return self.plan_sheet_refuse(why.into());
        };
        let answered = s.kind == Kind::Question && s.state == State::Accepted;
        if note.is_empty() && answered {
            let why = "An accepted question keeps its answer: clear the decision first";
            return self.plan_sheet_refuse(why.into());
        }
        if s.note != note {
            self.note_suggestion(project, slug, id, note);
        }
        if accept && let Err(why) = self.decide_suggestion(project, slug, id, Decision::Accept) {
            self.plan_sheet_refuse(why);
        }
    }

    /// Open citation `i` of suggestion `id` in Zed, or say why not. The
    /// sheet makes way: Zed is what is worked in next.
    fn plan_sheet_citation(&mut self, project: &str, slug: &str, id: &str, i: usize) {
        match self.suggestion_citation(project, slug, id, i) {
            Err(why) => self.plan_sheet_refuse(why),
            Ok(place) => {
                self.close_plan_sheet();
                self.open_citation(project, slug, place);
            }
        }
    }

    /// A left click while the sheet is open.
    pub(crate) fn plan_sheet_click(&mut self, pos: Point<f64, Logical>) {
        let Some(view) = self.plan_sheet_view() else {
            return;
        };
        let hit = chrome::plan_sheet_layout(&view, self.output_size()).hit(pos);
        if let Some(PlanHit::Hint(i)) = hit {
            if let Some((keys, _)) = view.hints.get(i) {
                let keys = keys.clone();
                self.press_hint(crate::input::Overlay::PlanSheet, &keys, pos);
            }
            return;
        }
        let rows = view.rows;
        let Some(sheet) = self.plan_sheet.as_mut() else {
            return;
        };
        let writing = sheet.editing.is_some();
        if !matches!(hit, None | Some(PlanHit::Field | PlanHit::Panel)) {
            sheet.notice = None;
            sheet.armed = None;
        }
        let (kept, outcome) = match hit {
            // A button keeps the text being written, then acts.
            Some(PlanHit::Button(b)) => {
                let outcome = match b {
                    PLAN_CLOSE => Outcome::Close,
                    PLAN_ASK => Outcome::Ask,
                    PLAN_FINISH => Outcome::Finish,
                    _ => return,
                };
                (sheet.take_text(), Some(outcome))
            }
            Some(PlanHit::Citation(i)) => {
                let Some(id) = sheet.at.clone() else {
                    return;
                };
                (sheet.take_text(), Some(Outcome::Citation(id, i)))
            }
            Some(PlanHit::Mark(i)) if !writing => {
                sheet.go(i, &rows);
                (None, sheet.toggle(i, &rows))
            }
            Some(PlanHit::Row(i)) if !writing => {
                sheet.go(i, &rows);
                (None, None)
            }
            // Off it, it stays: a click away never throws a text away.
            _ => (None, None),
        };
        if let Some(kept) = kept {
            self.apply_plan_sheet(kept);
            // Refused: the button waits.
            if (self.plan_sheet.as_ref()).is_some_and(|s| s.notice.is_some()) {
                return;
            }
        }
        if let Some(outcome) = outcome {
            self.apply_plan_sheet(outcome);
        }
        self.plan_sheet_follows();
    }

    /// The pointer moved while the sheet is open: what is under it lights
    /// up, and a title cut short shows whole.
    pub(crate) fn plan_sheet_hover(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let view = self.plan_sheet_view()?;
        let size = self.output_size();
        let layout = chrome::plan_sheet_layout(&view, size);
        let hit = layout.hit(pos);
        if let Some(sheet) = self.plan_sheet.as_mut() {
            sheet.hover = hit;
        }
        match hit? {
            PlanHit::Button(PLAN_FINISH) => {
                let what = if view.finish == WRITE_FINAL {
                    "Write the final plan from what you accepted"
                } else {
                    "Use the plan as it is: nothing was accepted"
                };
                Some(Tooltip::new(what, layout.buttons[PLAN_FINISH]).key("Ctrl+Enter"))
            }
            PlanHit::Button(PLAN_ASK) => Some(
                Tooltip::new("Send your notes to the reviewer", layout.buttons[PLAN_ASK])
                    .key("Ctrl+Shift+Enter"),
            ),
            PlanHit::Mark(i) => {
                let spot = layout.rows.iter().find(|s| s.i == i)?;
                Some(Tooltip::new("Accept, or clear that", spot.mark).key("Space"))
            }
            PlanHit::Citation(i) => {
                let (rect, _) =
                    (layout.lines.iter()).find(|(_, l)| l.piece == chrome::PlanPiece::Place(i))?;
                let tip = Tooltip::new("Open in Zed, at this line", *rect);
                Some(if i == 0 { tip.key("Ctrl+O") } else { tip })
            }
            PlanHit::Row(_) => {
                let (text, rect) = chrome::plan_sheet_tip(&view, size, pos)?;
                Some(Tooltip::new(&text, rect))
            }
            _ => None,
        }
    }

    /// The wheel or the touchpad moved over the sheet: the keys with F1;
    /// else the text under the pointer, or the rows.
    pub(crate) fn plan_sheet_scroll(&mut self, motion: Motion) {
        let Some(view) = self.plan_sheet_view() else {
            return;
        };
        let size = self.output_size();
        let layout = chrome::plan_sheet_layout(&view, size);
        let over_text = layout.detail.to_f64().contains(self.pointer);
        let pitch = if view.keys.is_some() {
            layout.key_pitch()
        } else if over_text {
            layout.line_pitch()
        } else {
            match layout.row_rects().as_slice() {
                [first, second, ..] => (second.loc.y - first.loc.y) as f64,
                [only] => only.size.h as f64,
                [] => 1.0,
            }
        };
        let by = self.motion_rows(motion, pitch);
        if by == 0 {
            return;
        }
        let most = chrome::plan_sheet_most_top(&view, size);
        let Some(sheet) = self.plan_sheet.as_mut() else {
            return;
        };
        if view.keys.is_some() {
            sheet.keys_top = sheet
                .keys_top
                .saturating_add_signed(by)
                .min(layout.keys_most);
        } else if over_text {
            sheet.detail_top = sheet
                .detail_top
                .saturating_add_signed(by)
                .min(layout.detail_most);
        } else {
            sheet.top = sheet.top.saturating_add_signed(by).min(most);
        }
    }

    /// What the pointer can press in the sheet.
    pub(crate) fn plan_sheet_controls(&self) -> Vec<chrome::Rect> {
        self.plan_sheet_view().map_or_else(Vec::new, |view| {
            chrome::plan_sheet_layout(&view, self.output_size()).controls()
        })
    }

    pub(crate) fn plan_sheet_view(&self) -> Option<PlanSheetView> {
        let sheet = self.plan_sheet.as_ref()?;
        let feature = self.feature(&sheet.project, &sheet.slug)?;
        let review = feature.plan_review.as_ref()?;
        Some(sheet_view(sheet, &feature.title, review))
    }
}

/// What the sheet `sheet` shows of `review`, of the feature called `title`.
fn sheet_view(sheet: &PlanSheet, title: &str, review: &PlanReview) -> PlanSheetView {
    let rows = rows(review);
    let cursor = sheet.row_at(&rows);
    let at = rows.get(cursor).map(|r| r.id.as_str());
    let finish = finish_label(review);
    let not_found = review.not_found();
    PlanSheetView {
        title: format!("Plan review of \u{201c}{title}\u{201d}"),
        sub: counts_text(review),
        missing: (not_found > 0).then(|| format!("{} not found", plural(not_found, "citation"))),
        cursor,
        top: sheet.top,
        detail: at.and_then(|id| review.suggestion(id)).map(detail),
        detail_top: sheet.detail_top,
        editing: sheet.editing.as_ref().map(|(writing, field)| {
            let (label, placeholder) = match writing {
                Writing::Note { accept: true, .. } => {
                    ("Your answer to this question", "What the answer is")
                }
                Writing::Note { .. } => ("Your note on this suggestion", "What you say of it"),
                Writing::Add => ("Your own suggestion", "What to change in the plan"),
            };
            PlanField {
                label,
                placeholder,
                line: field.line(),
            }
        }),
        armed: (sheet.armed.as_ref()).and_then(|id| rows.iter().position(|r| r.id == *id)),
        notice: sheet.notice.clone(),
        hover: sheet.hover,
        finish,
        primary: primary_button(review),
        keys: sheet.keys.then(keys_list),
        keys_top: sheet.keys_top,
        hints: sheet.hints(&rows, finish),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{plan_review::Evidence, review_import::Severity};

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

    fn row(id: &str, mark: PlanMark) -> PlanRow {
        PlanRow {
            id: id.into(),
            mark,
            severity: Severity::High,
            kind: Kind::Gap,
            title: "No cap on the retries".into(),
            plan: "Milestone 3".into(),
            yours: false,
            dropped: false,
            note: String::new(),
        }
    }

    fn question(id: &str, note: &str) -> PlanRow {
        PlanRow {
            kind: Kind::Question,
            note: note.into(),
            ..row(id, PlanMark::Open)
        }
    }

    fn yours(id: &str, mark: PlanMark) -> PlanRow {
        PlanRow {
            yours: true,
            ..row(id, mark)
        }
    }

    fn press(
        sheet: &mut PlanSheet,
        rows: &[PlanRow],
        k: (ModifiersState, Keysym),
    ) -> Option<Outcome> {
        let shown = Shown {
            rows,
            draft: "",
            page: 5,
        };
        sheet.key(&k.0, k.1, k.1, &shown)
    }

    fn sheet_on(rows: &[PlanRow], i: usize) -> PlanSheet {
        let mut s = PlanSheet::default();
        s.go(i, rows);
        s
    }

    fn decide(id: &str, decision: Decision) -> Option<Outcome> {
        Some(Outcome::Decide(id.into(), decision))
    }

    #[test]
    fn keys_move_decide_and_close() {
        let rows = vec![
            row("a", PlanMark::Open),
            row("b", PlanMark::Accepted),
            row("c", PlanMark::Rejected),
        ];
        let mut s = sheet_on(&rows, 0);
        assert_eq!(press(&mut s, &rows, key(Keysym::Down)), None);
        assert_eq!(s.at.as_deref(), Some("b"));
        press(&mut s, &rows, ctrl(Keysym::j));
        assert_eq!(s.at.as_deref(), Some("c"));
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(s.cursor, 2, "it stops at the end");
        press(&mut s, &rows, key(Keysym::Up));
        press(&mut s, &rows, ctrl(Keysym::k));
        assert_eq!(s.at.as_deref(), Some("a"));
        press(&mut s, &rows, key(Keysym::End));
        assert_eq!(s.at.as_deref(), Some("c"));
        press(&mut s, &rows, key(Keysym::Home));
        assert_eq!(s.at.as_deref(), Some("a"));
        // Space accepts, or clears what is accepted; r rejects, or clears
        // what is rejected.
        assert_eq!(
            press(&mut s, &rows, key(Keysym::space)),
            decide("a", Decision::Accept)
        );
        assert_eq!(
            press(&mut s, &rows, key(Keysym::r)),
            decide("a", Decision::Reject(SHEET_REASON.into()))
        );
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::space)),
            decide("b", Decision::Clear)
        );
        assert_eq!(
            press(&mut s, &rows, key(Keysym::r)),
            decide("b", Decision::Reject(SHEET_REASON.into()))
        );
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::r)),
            decide("c", Decision::Clear)
        );
        // Del clears at once.
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Delete)),
            decide("c", Decision::Clear)
        );
        assert_eq!(
            press(&mut s, &rows, ctrl(Keysym::Return)),
            Some(Outcome::Finish)
        );
        let mut both = ctrl(Keysym::Return);
        both.0.shift = true;
        assert_eq!(press(&mut s, &rows, both), Some(Outcome::Ask));
        assert_eq!(
            press(&mut s, &rows, ctrl(Keysym::o)),
            Some(Outcome::Citation("c".into(), 0))
        );
        // Plain Enter acts on nothing: decisions aren't made by accident.
        assert_eq!(press(&mut s, &rows, key(Keysym::Return)), None);
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
    fn pgup_and_pgdn_scroll_the_text_and_a_move_starts_it_over() {
        let rows = vec![row("a", PlanMark::Open), row("b", PlanMark::Open)];
        let mut s = sheet_on(&rows, 0);
        press(&mut s, &rows, key(Keysym::Page_Down));
        press(&mut s, &rows, key(Keysym::Page_Down));
        assert_eq!((s.cursor, s.detail_top), (0, 10));
        press(&mut s, &rows, key(Keysym::Page_Up));
        assert_eq!(s.detail_top, 5);
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(s.detail_top, 0);
    }

    #[test]
    fn the_cursor_follows_its_suggestion_through_a_rewrite() {
        let rows = vec![row("a", PlanMark::Open), row("b", PlanMark::Open)];
        let mut s = sheet_on(&rows, 1);
        s.detail_top = 4;
        // Moved along by what came in before it: the same one, still read.
        let moved = vec![
            row("new", PlanMark::Open),
            row("a", PlanMark::Open),
            row("b", PlanMark::Open),
        ];
        s.settle(&moved);
        assert_eq!((s.cursor, s.at.as_deref(), s.detail_top), (2, Some("b"), 4));
        // Gone: what is in its place, and at its start.
        let gone = vec![row("new", PlanMark::Open), row("a", PlanMark::Open)];
        s.settle(&gone);
        assert_eq!((s.cursor, s.at.as_deref(), s.detail_top), (1, Some("a"), 0));
        s.settle(&[]);
        assert_eq!((s.cursor, s.at.as_deref()), (0, None));
    }

    #[test]
    fn an_unanswered_question_gets_its_answer_then_is_accepted() {
        let rows = vec![question("q", ""), question("done", "Yes, both")];
        let mut s = sheet_on(&rows, 0);
        assert_eq!(press(&mut s, &rows, key(Keysym::space)), None);
        assert!(matches!(
            &s.editing,
            Some((Writing::Note { id, accept: true }, _)) if id == "q"
        ));
        // Enter with no answer stays, saying why.
        assert_eq!(press(&mut s, &rows, key(Keysym::Return)), None);
        assert_eq!(s.notice.as_deref(), Some(NO_ANSWER));
        assert!(s.editing.is_some());
        // The keys are text now.
        assert_eq!(press(&mut s, &rows, key(Keysym::y)), None);
        assert_eq!(s.notice, None);
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Note {
                id: "q".into(),
                note: "y".into(),
                accept: true
            })
        );
        assert!(s.editing.is_none());
        // Esc keeps the answer, and leaves the question not accepted.
        press(&mut s, &rows, key(Keysym::space));
        press(&mut s, &rows, key(Keysym::y));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Escape)),
            Some(Outcome::Note {
                id: "q".into(),
                note: "y".into(),
                accept: false
            })
        );
        // Answered already: accepted at once.
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::space)),
            decide("done", Decision::Accept)
        );
    }

    #[test]
    fn notes_are_written_and_kept() {
        let rows = vec![PlanRow {
            note: "old".into(),
            ..row("a", PlanMark::Open)
        }];
        let mut s = sheet_on(&rows, 0);
        press(&mut s, &rows, key(Keysym::n));
        assert_eq!(s.editing.as_ref().map(|(_, f)| f.trim()), Some("old"));
        assert_eq!(s.hints(&rows, WRITE_FINAL)[1].1, "save");
        // In the field, what moves or decides is text.
        press(&mut s, &rows, key(Keysym::r));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Note {
                id: "a".into(),
                note: "oldr".into(),
                accept: false
            })
        );
        // Enter on an emptied note empties it; Esc leaves it as it was.
        s.write(
            Writing::Note {
                id: "a".into(),
                accept: false,
            },
            "",
        );
        assert_eq!(s.hints(&rows, WRITE_FINAL)[1].1, "cancel");
        assert_eq!(press(&mut s, &rows, key(Keysym::Escape)), None);
        s.write(
            Writing::Note {
                id: "a".into(),
                accept: false,
            },
            "",
        );
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Note {
                id: "a".into(),
                note: String::new(),
                accept: false
            })
        );
    }

    #[test]
    fn adding_keeps_a_draft_on_esc_and_brings_it_back() {
        let rows = vec![row("a", PlanMark::Open)];
        let mut s = sheet_on(&rows, 0);
        press(&mut s, &rows, key(Keysym::a));
        // Enter with nothing written stays, saying why.
        assert_eq!(press(&mut s, &rows, key(Keysym::Return)), None);
        assert_eq!(s.notice.as_deref(), Some(NO_TEXT));
        press(&mut s, &rows, key(Keysym::x));
        assert_eq!(s.hints(&rows, WRITE_FINAL)[1].1, "keep the draft");
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Escape)),
            Some(Outcome::KeepDraft("x".into()))
        );
        assert!(s.editing.is_none());
        // It comes back to go on with.
        let shown = Shown {
            rows: &rows,
            draft: "x",
            page: 5,
        };
        let (mods, a) = key(Keysym::a);
        assert_eq!(s.key(&mods, a, a, &shown), None);
        assert_eq!(s.editing.as_ref().map(|(_, f)| f.trim()), Some("x"));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Return)),
            Some(Outcome::Add("x".into()))
        );
        // A button or a closing keeps it too.
        s.write(Writing::Add, "half");
        assert_eq!(s.take_text(), Some(Outcome::KeepDraft("half".into())));
        assert_eq!(s.take_text(), None);
    }

    #[test]
    fn del_on_your_own_arms_first_and_esc_or_a_key_disarms() {
        let rows = vec![yours("u1", PlanMark::Accepted), row("a", PlanMark::Open)];
        let mut s = sheet_on(&rows, 0);
        assert_eq!(s.hints(&rows, WRITE_FINAL)[4].1, "remove");
        assert_eq!(press(&mut s, &rows, key(Keysym::Delete)), None);
        assert_eq!(s.armed.as_deref(), Some("u1"));
        assert_eq!(
            s.hints(&rows, WRITE_FINAL),
            [
                ("Del".to_owned(), "remove".to_owned()),
                ("Esc".to_owned(), "keep it".to_owned())
            ]
        );
        // Esc steps back from it, without closing.
        assert_eq!(press(&mut s, &rows, key(Keysym::Escape)), None);
        assert_eq!(s.armed, None);
        // Another key disarms; a modifier on the way to one doesn't.
        press(&mut s, &rows, key(Keysym::Delete));
        press(&mut s, &rows, key(Keysym::Shift_L));
        assert!(s.armed.is_some());
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(s.armed, None);
        press(&mut s, &rows, key(Keysym::Up));
        press(&mut s, &rows, key(Keysym::Delete));
        assert_eq!(
            press(&mut s, &rows, key(Keysym::Delete)),
            decide("u1", Decision::Clear)
        );
        assert_eq!(s.armed, None);
    }

    /// Clearing one of the user's own removes it (`plan_review::delete_own`),
    /// so that must only ever be Del's, armed.
    #[test]
    fn neither_space_nor_r_removes_your_own() {
        let accepted = vec![yours("u1", PlanMark::Accepted)];
        let mut s = sheet_on(&accepted, 0);
        assert_eq!(press(&mut s, &accepted, key(Keysym::space)), None);
        assert!(s.notice.is_some(), "it says why");
        assert_eq!(
            press(&mut s, &accepted, key(Keysym::r)),
            decide("u1", Decision::Reject(SHEET_REASON.into()))
        );
        let rejected = vec![yours("u1", PlanMark::Rejected)];
        let mut s = sheet_on(&rejected, 0);
        assert_eq!(press(&mut s, &rejected, key(Keysym::r)), None);
        assert!(s.notice.is_some(), "it says why");
        assert_eq!(
            press(&mut s, &rejected, key(Keysym::space)),
            decide("u1", Decision::Accept)
        );
    }

    #[test]
    fn with_nothing_to_decide_the_row_keys_do_nothing() {
        let mut s = PlanSheet::default();
        for k in [Keysym::space, Keysym::r, Keysym::n, Keysym::Delete] {
            assert_eq!(press(&mut s, &[], key(k)), None, "{k:?}");
            assert!(s.editing.is_none());
        }
        assert_eq!(press(&mut s, &[], ctrl(Keysym::o)), None);
        assert_eq!(
            press(&mut s, &[], ctrl(Keysym::Return)),
            Some(Outcome::Finish)
        );
        press(&mut s, &[], key(Keysym::a));
        assert!(s.editing.is_some());
    }

    #[test]
    fn a_refusal_goes_with_the_next_key() {
        let rows = vec![row("a", PlanMark::Open)];
        let mut s = sheet_on(&rows, 0);
        s.notice = Some("1 suggestion still to decide".into());
        press(&mut s, &rows, key(Keysym::Control_L));
        assert!(s.notice.is_some(), "a modifier isn't a key yet");
        press(&mut s, &rows, key(Keysym::Down));
        assert_eq!(s.notice, None);
    }

    /// Every hint and F1 line does something, and each thing the handler
    /// does is on the F1 list.
    #[test]
    fn hints_f1_and_handler_agree() {
        let rows = vec![
            row("a", PlanMark::Open),
            row("b", PlanMark::Open),
            yours("u1", PlanMark::Accepted),
        ];
        let acts = |keys: &str| {
            let (mods, sym) =
                crate::keys::hint_key(keys).unwrap_or_else(|| panic!("{keys} is a key"));
            let mut s = sheet_on(&rows, 1);
            s.detail_top = 5;
            let shown = Shown {
                rows: &rows,
                draft: "",
                page: 5,
            };
            let out = s.key(&mods, sym, sym, &shown);
            let moved = s.cursor != 1
                || s.detail_top != 5
                || s.keys
                || s.editing.is_some()
                || s.armed.is_some();
            assert!(out.is_some() || moved, "{keys} does nothing");
        };
        let s = sheet_on(&rows, 1);
        for (keys, _) in s.hints(&rows, WRITE_FINAL) {
            acts(&keys);
        }
        let listed = keys_list();
        for (keys, _) in &listed {
            for one in keys.split(" / ") {
                // As F1 writes them; the chords' names are xkb's.
                acts(match one {
                    "PgUp" => "Page_Up",
                    "PgDn" => "Page_Down",
                    other => other,
                });
            }
        }
        // What the footer shows is listed too.
        let on_list = |k: &str| {
            listed
                .iter()
                .any(|(keys, _)| keys.split(" / ").any(|one| one == k))
        };
        for (k, _) in s.hints(&rows, WRITE_FINAL) {
            assert!(on_list(&k), "{k} isn't on F1");
        }
        assert!(on_list("Ctrl+Shift+Enter") && on_list("PgDn") && on_list("Ctrl+O"));
    }

    #[test]
    fn the_footer_shows_all_its_hints() {
        let rows = vec![yours("u1", PlanMark::Accepted), row("a", PlanMark::Open)];
        for finish in [WRITE_FINAL, USE_AS_IS] {
            let hints = sheet_on(&rows, 0).hints(&rows, finish);
            let strs: Vec<(&str, &str)> = hints
                .iter()
                .map(|(k, w)| (k.as_str(), w.as_str()))
                .collect();
            let wide = crate::chrome::footer_fit(&strs);
            assert_eq!(wide, hints.len(), "{hints:?}");
        }
    }

    fn suggestion(id: &str, state: State) -> Suggestion {
        Suggestion {
            id: id.into(),
            title: format!("About {id}"),
            state,
            ..Suggestion::default()
        }
    }

    fn review(suggestions: Vec<Suggestion>) -> PlanReview {
        PlanReview {
            stamp: Some((1, 1)),
            suggestions,
            ..PlanReview::default()
        }
    }

    #[test]
    fn finishing_waits_for_every_decision_and_asking_for_a_note() {
        let mut r = review(vec![
            suggestion("a", State::Open),
            suggestion("b", State::Open),
            suggestion("c", State::Rejected),
        ]);
        assert_eq!(
            finish_refused(&r).as_deref(),
            Some("2 suggestions still to decide")
        );
        plan_review::decide(&mut r, "a", Decision::Accept).unwrap();
        assert_eq!(
            finish_refused(&r).as_deref(),
            Some("1 suggestion still to decide")
        );
        plan_review::decide(&mut r, "b", Decision::Reject("No".into())).unwrap();
        assert_eq!(finish_refused(&r), None);
        assert_eq!(finish_refused(&review(Vec::new())), None);
        // Only a note on the reviewer's own can be asked about.
        assert!(ask_refused(&r).is_some());
        plan_review::set_note(&mut r, "a", "Why five?").unwrap();
        assert_eq!(ask_refused(&r), None);
    }

    #[test]
    fn counts_read_as_words() {
        let mut r = review(vec![
            suggestion("a", State::Open),
            suggestion("b", State::Open),
            suggestion("c", State::Open),
            suggestion("d", State::Accepted),
            suggestion("e", State::Accepted),
            suggestion("f", State::Rejected),
        ]);
        assert_eq!(
            counts_text(&r),
            "3 to decide \u{b7} 2 accepted \u{b7} 1 rejected"
        );
        r.suggestions.truncate(1);
        assert_eq!(counts_text(&r), "1 to decide");
        r.draft = "half".into();
        assert_eq!(counts_text(&r), "1 to decide \u{b7} draft kept");
        assert_eq!(counts_text(&review(Vec::new())), "no suggestions");
    }

    #[test]
    fn the_finishing_button_says_what_it_does_and_is_primary_once_none_is_open() {
        let open = review(vec![suggestion("a", State::Open)]);
        assert_eq!(primary_button(&open), None);
        assert_eq!(finish_label(&open), USE_AS_IS);
        let some = review(vec![
            suggestion("a", State::Open),
            suggestion("b", State::Accepted),
        ]);
        assert_eq!(primary_button(&some), None);
        assert_eq!(finish_label(&some), WRITE_FINAL);
        let done = review(vec![suggestion("b", State::Accepted)]);
        assert_eq!(primary_button(&done), Some(PLAN_FINISH));
        assert_eq!(finish_label(&done), WRITE_FINAL);
        let rejected = review(vec![suggestion("b", State::Rejected)]);
        assert_eq!(primary_button(&rejected), Some(PLAN_FINISH));
        assert_eq!(finish_label(&rejected), USE_AS_IS);
        let none = review(Vec::new());
        assert_eq!(primary_button(&none), Some(PLAN_FINISH));
        assert_eq!(finish_label(&none), USE_AS_IS);
    }

    #[test]
    fn the_view_shows_the_cursor_on_its_suggestion_with_the_citations() {
        let mut cited = suggestion("b", State::Open);
        cited.evidence = vec![
            Evidence {
                repo: "api".into(),
                file: "src/a.rs".into(),
                line: 12,
                quote: "fn a()".into(),
                found: Some(false),
            },
            Evidence {
                file: "src/b.rs".into(),
                line: 3,
                quote: "fn b()".into(),
                found: Some(true),
                ..Evidence::default()
            },
        ];
        let r = review(vec![suggestion("a", State::Accepted), cited]);
        let mut s = PlanSheet {
            at: Some("b".into()),
            ..PlanSheet::default()
        };
        let v = sheet_view(&s, "Bulk edit", &r);
        assert_eq!(v.title, "Plan review of \u{201c}Bulk edit\u{201d}");
        assert_eq!(v.cursor, 1);
        assert_eq!(v.missing.as_deref(), Some("1 citation not found"));
        let d = v.detail.expect("the suggestion's text");
        assert_eq!(d.title, "About b");
        assert_eq!(d.evidence[0].place, "api: src/a.rs:12");
        assert!(d.evidence[0].not_found && !d.evidence[1].not_found);
        assert_eq!(d.evidence[1].place, "src/b.rs:3");
        s.keys = true;
        assert!(sheet_view(&s, "t", &r).keys.is_some());
        // Nothing to decide: no rows, nothing under the cursor.
        let v = sheet_view(&PlanSheet::default(), "t", &review(Vec::new()));
        assert!(v.rows.is_empty() && v.detail.is_none());
        assert_eq!(v.primary, Some(PLAN_FINISH));
        assert_eq!(v.finish, USE_AS_IS);
        assert_eq!(v.missing, None);
    }

    #[test]
    fn a_removed_suggestion_of_your_own_goes_back_where_it_was() {
        let mut r = review(vec![
            suggestion("a", State::Open),
            suggestion("u1", State::Accepted),
            suggestion("b", State::Open),
        ]);
        r.suggestions[1].source = Source::You;
        let taken = r.suggestions[1].clone();
        plan_review::delete_own(&mut r, "u1").unwrap();
        assert_eq!(r.suggestions.len(), 2);
        assert!(plan_review::restore(&mut r, 1, taken.clone()));
        assert_eq!(r.suggestions[1].id, "u1");
        assert!(!plan_review::restore(&mut r, 1, taken), "not twice");
    }
}
