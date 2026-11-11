//! The git identity dialog (see `crate::git_identity`): the name and
//! email git wants on commits, and whether every repository gets them or
//! only those that failed.

use super::{
    BUTTON_RADIUS, ButtonStyle, Canvas, DIALOG_WIDTH, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE,
    POPUP_RADIUS, Painter, Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP,
    QUIT_PADDING, QUIT_TITLE, Rect, Rgb, SHADOW, band, dialog_buttons, field_text, focus_ring,
    phys,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The dialog's buttons.
pub const IDENTITY_BUTTONS: [&str; 2] = ["Cancel", "Save and retry"];
/// The Save button, after Cancel: the dialog's default.
pub const IDENTITY_SAVE: usize = 1;
/// How wide the labels left of the fields are.
const LABEL_W: i32 = 80;
/// The room between the scope toggle's outline and its segments.
const TRACK_PAD: i32 = 2;
/// The room between the last row and the buttons, past the line gap.
const ROWS_END: i32 = 6;

/// What has the keyboard in the dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IdentityFocus {
    #[default]
    Name,
    Email,
    /// Where they are kept: every repository, or only these.
    Scope,
}

impl IdentityFocus {
    /// The next one down (`back`: up), round.
    pub fn step(self, back: bool) -> Self {
        use IdentityFocus::*;
        match (self, back) {
            (Name, false) | (Scope, true) => Email,
            (Email, false) | (Name, true) => Scope,
            (Scope, false) | (Email, true) => Name,
        }
    }
}

/// What the git identity dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityView {
    /// What it is for: "git needs them to commit in gerdr".
    pub body: String,
    pub name: Line,
    pub email: Line,
    pub focus: IdentityFocus,
    /// Kept in the user's git config, for every repository; else in the
    /// failed ones only.
    pub global: bool,
    /// The label of the segment keeping them in those only: "Only gerdr",
    /// "Only these 3".
    pub only: String,
    /// What is wrong with the form, as a sentence.
    pub error: Option<String>,
    pub hover: Option<IdentityHit>,
    /// The keys besides the buttons, said left of them.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What the pointer is over in the git identity dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdentityHit {
    Name,
    Email,
    /// A scope segment: every repository (`true`), or only those failed.
    Scope(bool),
    /// 0 Cancel, 1 Save and retry.
    Button(usize),
}

/// Where the git identity dialog's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// The labels' room, then the fields': name, email, scope.
    pub labels: [Rect; 3],
    pub name: Rect,
    pub email: Rect,
    /// The scope toggle's outline, and its segments: every repository,
    /// only those.
    pub track: Rect,
    pub scope: [Rect; 2],
    pub error: Option<Rect>,
    /// Cancel, Save and retry.
    pub buttons: [Rect; 2],
}

/// Where the git identity dialog's parts go, with an error line if
/// `error`, centred on an output of `size`.
pub fn identity_layout(size: Size<i32, Logical>, error: bool) -> IdentityLayout {
    let error_h = if error { QUIT_BODY + QUIT_LINE_GAP } else { 0 };
    let height = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_LINE_GAP
        + QUIT_BODY
        + QUIT_LINE_GAP
        + 3 * (QUIT_BUTTON + QUIT_LINE_GAP)
        + error_h
        + ROWS_END
        + QUIT_BUTTON;
    let width = DIALOG_WIDTH.min(size.w);
    let panel = Rectangle::new(
        (
            ((size.w - width) / 2).max(0),
            ((size.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, mut y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = width - 2 * QUIT_PADDING;
    let mut next = |h: i32| {
        let rect = Rectangle::new((x, y).into(), (inner, h).into());
        y += h + QUIT_LINE_GAP;
        rect
    };
    let title = next(QUIT_TITLE);
    let body = next(QUIT_BODY);
    let rows = [next(QUIT_BUTTON), next(QUIT_BUTTON), next(QUIT_BUTTON)];
    let error = error.then(|| next(QUIT_BODY));
    let split = |row: Rect| {
        let label = Rectangle::new(row.loc, (LABEL_W, row.size.h).into());
        let field = Rectangle::new(
            (row.loc.x + LABEL_W, row.loc.y).into(),
            ((row.size.w - LABEL_W).max(0), row.size.h).into(),
        );
        (label, field)
    };
    let (name_label, name) = split(rows[0]);
    let (email_label, email) = split(rows[1]);
    let (scope_label, track) = split(rows[2]);
    let half = (track.size.w - 2 * TRACK_PAD) / 2;
    let segment = |i: i32| {
        Rectangle::new(
            (track.loc.x + TRACK_PAD + i * half, track.loc.y + TRACK_PAD).into(),
            (half, track.size.h - 2 * TRACK_PAD).into(),
        )
    };
    let buttons = dialog_buttons(panel, &IDENTITY_BUTTONS);
    IdentityLayout {
        panel,
        title,
        body,
        labels: [name_label, email_label, scope_label],
        name,
        email,
        track,
        scope: [segment(0), segment(1)],
        error,
        buttons: [buttons[0], buttons[1]],
    }
}

impl IdentityLayout {
    /// What `pos` is over.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<IdentityHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some(button) = self.buttons.iter().position(on) {
            return Some(IdentityHit::Button(button));
        }
        if let Some(i) = self.scope.iter().position(on) {
            return Some(IdentityHit::Scope(i == 0));
        }
        // A click on a label goes to its field, as on a form.
        let row = |label: Rect, field: Rect| on(&label.merge(field));
        if row(self.labels[0], self.name) {
            return Some(IdentityHit::Name);
        }
        if row(self.labels[1], self.email) {
            return Some(IdentityHit::Email);
        }
        None
    }

    /// What the pointer can press.
    pub fn controls(&self) -> Vec<Rect> {
        self.scope.iter().chain(&self.buttons).copied().collect()
    }
}

/// The dialog's field with the keyboard, as the caret and the pointer
/// care; none while the scope has it.
pub fn identity_field(
    view: &IdentityView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    let layout = identity_layout(size, view.error.is_some());
    let (area, line) = match view.focus {
        IdentityFocus::Name => (layout.name, &view.name),
        IdentityFocus::Email => (layout.email, &view.email),
        IdentityFocus::Scope => return None,
    };
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: line.clone(),
    })
}

impl Painter {
    /// The git identity dialog for an output of `size`, inside a margin
    /// of `SHADOW` holding its drop shadow: what it is for, the name and
    /// email fields, the scope, Cancel and Save and retry.
    pub(super) fn identity_dialog(
        &self,
        view: &IdentityView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = identity_layout(size, view.error.is_some());
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let line = |c: &mut Canvas, text: &str, rect: Rect, size: f32, color: Rgb| {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(c, text, size, x, end, band(rect, scale), scale, color)
        };
        line(
            &mut c,
            "Git needs your name and email",
            layout.title,
            LABEL_FONT_SIZE,
            p.text,
        );
        line(&mut c, &view.body, layout.body, FONT_SIZE, p.muted);
        for (label, rect) in ["Name", "Email", "Save for"].into_iter().zip(layout.labels) {
            line(&mut c, label, rect, FONT_SIZE, p.muted);
        }
        let focus = view.focus;
        self.field(
            &mut c,
            &view.name,
            "Your Name",
            shift(layout.name),
            focus == IdentityFocus::Name,
            scale,
            p,
        );
        self.field(
            &mut c,
            &view.email,
            "you@example.com",
            shift(layout.email),
            focus == IdentityFocus::Email,
            scale,
            p,
        );
        let segments = [
            ("Every repository", shift(layout.scope[0])),
            (view.only.as_str(), shift(layout.scope[1])),
        ];
        let hover = match view.hover {
            Some(IdentityHit::Scope(all)) => Some(if all { 0 } else { 1 }),
            _ => None,
        };
        let on = if view.global { 0 } else { 1 };
        // The ring first: the toggle paints over its inside.
        if focus == IdentityFocus::Scope {
            focus_ring(
                &mut c,
                shift(layout.track),
                BUTTON_RADIUS,
                p.elevated,
                scale,
                p.accent,
            );
        }
        self.segmented(&mut c, shift(layout.track), &segments, on, hover, scale, p);
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }

        // Left of the buttons, the keys besides them; then Cancel and
        // Save and retry, the default.
        let first = shift(layout.buttons[0]);
        let left = shift(layout.title).loc.x;
        let row = Rectangle::new((left, first.loc.y).into(), (0, first.size.h).into());
        self.dialog_keys(
            &mut c,
            &view.keys,
            None,
            row,
            first.loc.x - QUIT_BUTTON_GAP,
            scale,
            p,
        );
        for (i, (rect, label)) in layout.buttons.iter().zip(IDENTITY_BUTTONS).enumerate() {
            let style = if i == IDENTITY_SAVE {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(IdentityHit::Button(i));
            self.button(
                &mut c,
                label,
                shift(*rect),
                (hovered, false),
                style,
                scale,
                p,
            );
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_steps_round_both_ways() {
        use IdentityFocus::*;
        assert_eq!(Name.step(false), Email);
        assert_eq!(Email.step(false), Scope);
        assert_eq!(Scope.step(false), Name);
        assert_eq!(Name.step(true), Scope);
        assert_eq!(Scope.step(true), Email);
        assert_eq!(Email.step(true), Name);
    }

    #[test]
    fn a_click_on_a_label_or_field_or_segment_hits_it() {
        let layout = identity_layout((1200, 800).into(), false);
        let mid = |r: Rect| Point::from((r.loc.x as f64 + 4.0, r.loc.y as f64 + 4.0));
        assert_eq!(layout.hit(mid(layout.labels[0])), Some(IdentityHit::Name));
        assert_eq!(layout.hit(mid(layout.email)), Some(IdentityHit::Email));
        assert_eq!(
            layout.hit(mid(layout.scope[0])),
            Some(IdentityHit::Scope(true))
        );
        assert_eq!(
            layout.hit(mid(layout.scope[1])),
            Some(IdentityHit::Scope(false))
        );
        assert_eq!(
            layout.hit(mid(layout.buttons[1])),
            Some(IdentityHit::Button(1))
        );
        assert_eq!(layout.hit(mid(layout.title)), None);
    }

    #[test]
    fn it_fits_a_small_output() {
        let size = Size::from((860, 500));
        let layout = identity_layout(size, true);
        assert!(layout.panel.loc.x >= 0 && layout.panel.loc.y >= 0);
        assert!(layout.panel.loc.x + layout.panel.size.w <= size.w);
        assert!(layout.panel.loc.y + layout.panel.size.h <= size.h);
        let last = layout.error.unwrap();
        assert!(last.loc.y + last.size.h <= layout.buttons[0].loc.y);
    }
}
