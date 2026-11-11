//! The checkout dialog over everything: whether a worktree's untracked
//! files come along when its branch is checked out in the main worktree
//! (see `crate::checkout`).

use super::{
    ButtonStyle, DIALOG_WIDTH, FONT_SIZE, LABEL_FONT_SIZE, POPUP_RADIUS, Painter, Palette,
    QUIT_BODY, QUIT_BUTTON, QUIT_BUTTON_GAP, QUIT_LINE_GAP, QUIT_PADDING, QUIT_TITLE, Rect, SHADOW,
    band, dialog_buttons, phys,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size};

/// The buttons' labels: Cancel, Leave behind, Bring along.
const BUTTONS: [&str; 3] = ["Cancel", "Leave behind", "Bring along"];
/// The keys the dialog takes besides its buttons, said by them.
const KEYS: [(&str, &str); 2] = [("B", "bring"), ("L", "leave")];
/// A file's row, the gap between rows, and the room between the last and
/// the buttons.
const ROW: i32 = 24;
const ROW_GAP: i32 = 2;
const ROWS_END: i32 = 16;
/// How far a row's text sits in from the row's left edge.
const ROW_INSET: i32 = 6;
/// The gap between a path and what is said after it.
const TAG_GAP: i32 = 12;
/// The room kept above and below the dialog on a short output.
const MARGIN: i32 = 40;
/// The most rows it shows; past them the last says how many more.
const MAX_ROWS: usize = 12;

/// What the checkout dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutView {
    /// The branch checked out.
    pub branch: String,
    /// The main worktree's folder's name, and the worktree's.
    pub main: String,
    pub worktree: String,
    /// The untracked files and folders, in git's order.
    pub rows: Vec<CheckoutRow>,
    /// Some can come along: the main worktree has nothing at their path.
    pub can_bring: bool,
    /// The selected button: 0 Cancel, 1 Leave behind, 2 Bring along.
    pub selected: usize,
    /// The button under the pointer.
    pub hover: Option<usize>,
}

/// An untracked file or folder of the checkout dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutRow {
    /// From the worktree's top folder; a folder ends in `/`.
    pub path: String,
    /// The untracked files in a folder.
    pub files: usize,
    /// It is a symbolic link.
    pub link: bool,
    /// The main worktree has something at its path already.
    pub clash: bool,
}

/// Where the parts of the checkout dialog go, in output coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutLayout {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    /// One per row that fits, from the first; when not all do, the last
    /// says how many more there are.
    pub rows: Vec<Rect>,
    /// Cancel, Leave behind, Bring along.
    pub buttons: Vec<Rect>,
}

/// Where the checkout dialog's parts go, with `rows` files, centred on an
/// output of `size`: as many rows as fit.
pub fn checkout_layout(size: Size<i32, Logical>, rows: usize) -> CheckoutLayout {
    let fixed = 2 * QUIT_PADDING + QUIT_TITLE + 2 * QUIT_LINE_GAP + QUIT_BODY + ROWS_END - ROW_GAP
        + QUIT_BUTTON;
    let fit = ((size.h - 2 * MARGIN - fixed) / (ROW + ROW_GAP)).max(1) as usize;
    let shown = rows.min(fit).min(MAX_ROWS);
    let height = fixed + shown as i32 * (ROW + ROW_GAP);
    let width = DIALOG_WIDTH.min(size.w);
    let panel = Rectangle::new(
        (
            ((size.w - width) / 2).max(0),
            ((size.h - height) / 2).max(0),
        )
            .into(),
        (width, height).into(),
    );
    let (x, y) = (panel.loc.x + QUIT_PADDING, panel.loc.y + QUIT_PADDING);
    let inner = width - 2 * QUIT_PADDING;
    let title = Rectangle::new((x, y).into(), (inner, QUIT_TITLE).into());
    let body_y = y + QUIT_TITLE + QUIT_LINE_GAP;
    let body = Rectangle::new((x, body_y).into(), (inner, QUIT_BODY).into());
    let rows_y = body_y + QUIT_BODY + QUIT_LINE_GAP;
    let rows = (0..shown as i32)
        .map(|i| {
            let y = rows_y + i * (ROW + ROW_GAP);
            Rectangle::new((x, y).into(), (inner, ROW).into())
        })
        .collect();
    let buttons = dialog_buttons(panel, &BUTTONS);
    CheckoutLayout {
        panel,
        title,
        body,
        rows,
        buttons,
    }
}

/// What is said after a row's path, dimmed: how many files a folder
/// holds, and that it is a link.
fn tag(row: &CheckoutRow) -> String {
    let mut tags = Vec::new();
    if row.path.ends_with('/') {
        let files = if row.files == 1 { "file" } else { "files" };
        tags.push(format!("{} {files}", row.files));
    }
    if row.link {
        tags.push("link".to_owned());
    }
    tags.join(" \u{b7} ")
}

impl Painter {
    /// The checkout dialog for an output of `size`, inside a margin of
    /// `SHADOW` holding its drop shadow: the question, a row per untracked
    /// file, name first and its folder after, and Cancel, Leave behind and
    /// Bring along.
    pub(super) fn checkout_dialog(
        &self,
        view: &CheckoutView,
        size: Size<i32, Logical>,
        scale: f64,
        p: &Palette,
    ) -> MemoryRenderBuffer {
        let layout = checkout_layout(size, view.rows.len());
        let panel = layout.panel;
        let mut c = self.popup(panel.size, POPUP_RADIUS, scale, p);
        let s = scale as f32;
        // Rects relative to the buffer, which starts `SHADOW` before the panel.
        let shift = |r: Rect| {
            Rectangle::new(
                r.loc + Point::from((SHADOW - panel.loc.x, SHADOW - panel.loc.y)),
                r.size,
            )
        };

        let title = shift(layout.title);
        let (title_x, title_end) = phys(title, scale);
        self.text(
            &mut c,
            &format!("Check out {} in {}?", view.branch, view.main),
            LABEL_FONT_SIZE,
            title_x,
            title_end,
            band(title, scale),
            scale,
            p.text,
        );
        let body = shift(layout.body);
        let (body_x, body_end) = phys(body, scale);
        let some_stay = view.rows.iter().any(|r| r.clash);
        let body_text = match (view.can_bring, some_stay) {
            (true, false) => format!(
                "Bring its untracked files along to {}, or leave them in {}",
                view.main, view.worktree
            ),
            (true, true) => format!(
                "Bring its untracked files along to {}, but those marked, or leave them",
                view.main
            ),
            (false, _) => format!(
                "{} has all its untracked files already: they stay in {}",
                view.main, view.worktree
            ),
        };
        self.text(
            &mut c,
            &body_text,
            FONT_SIZE,
            body_x,
            body_end,
            band(body, scale),
            scale,
            p.muted,
        );

        // A row per file: its name, its folder dimmed, then what more there
        // is to say; the last says how many more when not all fit.
        let more = view.rows.len().saturating_sub(layout.rows.len());
        for (i, (rect, row)) in layout.rows.iter().zip(&view.rows).enumerate() {
            let rect = shift(*rect);
            let (x, end) = phys(rect, scale);
            let inset = (ROW_INSET as f32 * s).round();
            let (x, end) = (x + inset, end - inset);
            let row_band = band(rect, scale);
            if more > 0 && i + 1 == layout.rows.len() {
                let left = more + 1;
                self.text(
                    &mut c,
                    &format!("+{left} more"),
                    FONT_SIZE,
                    x,
                    end,
                    row_band,
                    scale,
                    p.placeholder,
                );
                break;
            }
            let after = self.name_first(
                &mut c,
                &row.path,
                FONT_SIZE,
                (x, end),
                row_band,
                scale,
                (p.text, p.placeholder),
            );
            let gap = (TAG_GAP as f32 * s).round();
            let mut pen = after;
            let tag = tag(row);
            if !tag.is_empty() {
                pen = self.text(
                    &mut c,
                    &tag,
                    FONT_SIZE,
                    pen + gap,
                    end,
                    row_band,
                    scale,
                    p.placeholder,
                );
            }
            if row.clash {
                self.text(
                    &mut c,
                    &format!("stays: in {} already", view.main),
                    FONT_SIZE,
                    pen + gap,
                    end,
                    row_band,
                    scale,
                    p.warning,
                );
            }
        }

        // The keys besides the buttons, at their left; then the buttons:
        // Bring along, the default, in the accent, plain and dimmed while
        // it can't be pressed (its label still readable).
        let first = shift(layout.buttons[0]);
        let keys_row = Rectangle::new(
            (shift(layout.body).loc.x, first.loc.y).into(),
            (0, first.size.h).into(),
        );
        let keys: &[(&str, &str)] = if view.can_bring { &KEYS } else { &KEYS[1..] };
        let keys_end = first.loc.x - QUIT_BUTTON_GAP;
        self.dialog_keys(&mut c, keys, None, keys_row, keys_end, scale, p);
        let last = BUTTONS.len() - 1;
        for (i, (rect, label)) in layout.buttons.iter().zip(BUTTONS).enumerate() {
            let style = match i {
                _ if i < last => ButtonStyle::Plain,
                _ if view.can_bring => ButtonStyle::Primary,
                _ => ButtonStyle::Off,
            };
            let state = (view.hover == Some(i), view.selected == i);
            self.button(&mut c, label, shift(*rect), state, style, scale, p);
        }
        c.into_buffer(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckoutRow, CheckoutView, checkout_layout, tag};
    use crate::chrome::{Painter, Theme, inside};

    #[test]
    fn rows_fit_between_the_body_and_the_buttons() {
        let l = checkout_layout((1920, 1080).into(), 3);
        assert_eq!((l.rows.len(), l.buttons.len()), (3, 3));
        assert!(l.rows.iter().chain(&l.buttons).all(|r| inside(l.panel, *r)));
        let last = l.rows[2];
        assert!(l.body.loc.y + l.body.size.h <= l.rows[0].loc.y);
        assert!(
            l.buttons
                .iter()
                .all(|b| b.loc.y >= last.loc.y + last.size.h)
        );
        // Many are capped, and fewer still on a short output.
        assert_eq!(checkout_layout((1920, 1080).into(), 40).rows.len(), 12);
        let short = checkout_layout((800, 300).into(), 40);
        assert!(short.rows.len() < 12 && !short.rows.is_empty());
        assert!(short.rows.iter().all(|r| inside(short.panel, *r)));
    }

    #[test]
    fn folders_and_links_say_so() {
        let row = |path: &str, files, link| CheckoutRow {
            path: path.into(),
            files,
            link,
            clash: false,
        };
        assert_eq!(tag(&row("dev/a.py", 1, false)), "");
        assert_eq!(tag(&row("dev/new/", 3, false)), "3 files");
        assert_eq!(tag(&row("venv", 1, true)), "link");
    }

    #[test]
    fn it_paints() {
        let painter = Painter::new();
        let row = |path: &str, clash| CheckoutRow {
            path: path.into(),
            files: 2,
            link: false,
            clash,
        };
        for (rows, can_bring) in [
            (
                vec![row("dev/measure.py", false), row("dev/new/", false)],
                true,
            ),
            (vec![row("venv", true)], false),
            (
                (0..40).map(|i| row(&format!("f{i}.py"), false)).collect(),
                true,
            ),
        ] {
            let view = CheckoutView {
                branch: "feature/optimization-performance".into(),
                main: "services-api".into(),
                worktree: "services-api-perf".into(),
                rows,
                can_bring,
                selected: 2,
                hover: Some(1),
            };
            for theme in [Theme::Dark, Theme::Light] {
                for scale in [1.0, 2.0] {
                    for size in [(1280, 720), (60, 40)] {
                        painter.checkout_dialog(&view, size.into(), scale, theme.palette());
                    }
                }
            }
        }
    }
}
