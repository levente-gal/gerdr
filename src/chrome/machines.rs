//! The dialog connecting a machine (see `crate::machines`): its SSH target,
//! the name it goes by, how far connecting got, and why it failed with what
//! to do about it.

use super::{
    ButtonStyle, DIALOG_WIDTH, FONT_SIZE, FieldSpot, LABEL_FONT_SIZE, POPUP_RADIUS, Painter,
    Palette, QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE,
    Rect, Rgb, SHADOW, band, dialog_buttons, field_text, phys,
};
use crate::text_field::Line;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// How wide the labels left of the fields are.
const LABEL_W: i32 = 80;
/// The room between the last row and the buttons, past the line gap.
const ROWS_END: i32 = 6;

/// What has the keyboard in the dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MachineFocus {
    #[default]
    Target,
    Name,
}

impl MachineFocus {
    pub fn other(self) -> Self {
        match self {
            Self::Target => Self::Name,
            Self::Name => Self::Target,
        }
    }
}

/// What the dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineView {
    /// What it is for, or how far connecting got.
    pub body: String,
    pub target: Line,
    pub name: Line,
    /// What it is called when no name is given.
    pub name_hint: String,
    pub focus: MachineFocus,
    /// What is wrong, as a sentence.
    pub error: Option<String>,
    /// What to do about it.
    pub how: Option<String>,
    /// Its buttons' labels; the last is the default.
    pub buttons: Vec<String>,
    /// Connecting: the fields wait.
    pub busy: bool,
    pub hover: Option<MachineHit>,
    /// The keys besides the buttons, said left of them.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What the pointer is over in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MachineHit {
    Target,
    Name,
    Button(usize),
}

/// Where the dialog's parts go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// The labels' room, then the fields'.
    pub labels: [Rect; 2],
    pub target: Rect,
    pub name: Rect,
    pub error: Option<Rect>,
    pub how: Option<Rect>,
    pub buttons: Vec<Rect>,
}

/// Where the parts of `view` go, centred on an output of `size`.
pub fn machine_layout(view: &MachineView, size: Size<i32, Logical>) -> MachineLayout {
    let lines = i32::from(view.error.is_some()) + i32::from(view.how.is_some());
    let height = 2 * QUIT_PADDING
        + QUIT_TITLE
        + QUIT_LINE_GAP
        + QUIT_BODY
        + QUIT_LINE_GAP
        + 2 * (QUIT_BUTTON + QUIT_LINE_GAP)
        + lines * (QUIT_BODY + QUIT_LINE_GAP)
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
    let rows = [next(QUIT_BUTTON), next(QUIT_BUTTON)];
    let error = view.error.is_some().then(|| next(QUIT_BODY));
    let how = view.how.is_some().then(|| next(QUIT_BODY));
    let split = |row: Rect| {
        let label = Rectangle::new(row.loc, (LABEL_W, row.size.h).into());
        let field = Rectangle::new(
            (row.loc.x + LABEL_W, row.loc.y).into(),
            ((row.size.w - LABEL_W).max(0), row.size.h).into(),
        );
        (label, field)
    };
    let (target_label, target) = split(rows[0]);
    let (name_label, name) = split(rows[1]);
    let labels: Vec<&str> = view.buttons.iter().map(String::as_str).collect();
    MachineLayout {
        panel,
        title,
        body,
        labels: [target_label, name_label],
        target,
        name,
        error,
        how,
        buttons: dialog_buttons(panel, &labels),
    }
}

impl MachineLayout {
    /// What `pos` is over.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<MachineHit> {
        let on = |r: &Rect| r.to_f64().contains(pos);
        if let Some(button) = self.buttons.iter().position(on) {
            return Some(MachineHit::Button(button));
        }
        // A click on a label goes to its field, as on a form.
        if on(&self.labels[0].merge(self.target)) {
            return Some(MachineHit::Target);
        }
        if on(&self.labels[1].merge(self.name)) {
            return Some(MachineHit::Name);
        }
        None
    }
}

/// The dialog's field with the keyboard, as the caret and the pointer care;
/// none while it connects.
pub fn machine_field(
    view: &MachineView,
    size: Size<i32, Logical>,
    scale: f64,
) -> Option<FieldSpot> {
    if view.busy {
        return None;
    }
    let layout = machine_layout(view, size);
    let (area, line) = match view.focus {
        MachineFocus::Target => (layout.target, &view.target),
        MachineFocus::Name => (layout.name, &view.name),
    };
    Some(FieldSpot {
        area,
        text: field_text(area, scale),
        size: FONT_SIZE,
        line: line.clone(),
    })
}

impl Painter {
    /// The dialog for an output of `size`, inside a margin of `SHADOW`
    /// holding its drop shadow.
    pub(super) fn machine_dialog(
        &self,
        view: &MachineView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = machine_layout(view, size);
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };
        let line = |c: &mut super::Canvas, text: &str, rect: Rect, size: f32, color: Rgb| {
            let rect = shift(rect);
            let (x, end) = phys(rect, scale);
            self.text(c, text, size, x, end, band(rect, scale), scale, color)
        };
        line(
            &mut c,
            "Connect a machine",
            layout.title,
            LABEL_FONT_SIZE,
            p.text,
        );
        line(&mut c, &view.body, layout.body, FONT_SIZE, p.muted);
        for (label, rect) in ["Address", "Name"].into_iter().zip(layout.labels) {
            line(&mut c, label, rect, FONT_SIZE, p.muted);
        }
        let focused = |focus| !view.busy && view.focus == focus;
        self.field(
            &mut c,
            &view.target,
            "user@host",
            shift(layout.target),
            focused(MachineFocus::Target),
            scale,
            p,
        );
        let hint = if view.name_hint.is_empty() {
            "What to call it"
        } else {
            view.name_hint.as_str()
        };
        self.field(
            &mut c,
            &view.name,
            hint,
            shift(layout.name),
            focused(MachineFocus::Name),
            scale,
            p,
        );
        if let (Some(rect), Some(error)) = (layout.error, &view.error) {
            line(&mut c, error, rect, FONT_SIZE, p.error);
        }
        if let (Some(rect), Some(how)) = (layout.how, &view.how) {
            line(&mut c, how, rect, FONT_SIZE, p.muted);
        }

        // Left of the buttons, the keys besides them; then the buttons, the
        // last the default.
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
        let last = view.buttons.len().saturating_sub(1);
        for (i, (rect, label)) in layout.buttons.iter().zip(&view.buttons).enumerate() {
            let style = if i == last && !view.busy {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Plain
            };
            let hovered = view.hover == Some(MachineHit::Button(i));
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

    fn view(error: bool) -> MachineView {
        MachineView {
            body: "body".into(),
            target: Line::default(),
            name: Line::default(),
            name_hint: String::new(),
            focus: MachineFocus::Target,
            error: error.then(|| "Why.".to_owned()),
            how: error.then(|| "How.".to_owned()),
            buttons: if error {
                vec!["Cancel".into(), "Copy command".into(), "Try again".into()]
            } else {
                vec!["Cancel".into(), "Connect".into()]
            },
            busy: false,
            hover: None,
            keys: Vec::new(),
        }
    }

    #[test]
    fn a_click_on_a_label_or_field_or_button_hits_it() {
        let v = view(false);
        let layout = machine_layout(&v, (1200, 800).into());
        let mid = |r: Rect| Point::from((r.loc.x as f64 + 4.0, r.loc.y as f64 + 4.0));
        assert_eq!(layout.hit(mid(layout.labels[0])), Some(MachineHit::Target));
        assert_eq!(layout.hit(mid(layout.name)), Some(MachineHit::Name));
        assert_eq!(
            layout.hit(mid(layout.buttons[1])),
            Some(MachineHit::Button(1))
        );
        assert_eq!(layout.hit(mid(layout.title)), None);
    }

    #[test]
    fn it_fits_a_small_output_and_its_lines_clear_the_buttons() {
        let size = Size::from((860, 500));
        let layout = machine_layout(&view(true), size);
        assert!(layout.panel.loc.x >= 0 && layout.panel.loc.y >= 0);
        assert!(layout.panel.loc.x + layout.panel.size.w <= size.w);
        assert!(layout.panel.loc.y + layout.panel.size.h <= size.h);
        let last = layout.how.unwrap();
        assert!(last.loc.y + last.size.h <= layout.buttons[0].loc.y);
    }
}
