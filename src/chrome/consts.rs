//! Sizes and geometry of the chrome, in logical pixels unless said otherwise.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::WorkspaceKind;

/// How much room the chrome takes: comfortable (the default) or compact,
/// from the config file's `[ui]` table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

/// Height of the sidebar header, in logical pixels.
pub const BAR_HEIGHT: i32 = 34;
/// How high the tab strip is at `density`: one line of tabs around the
/// "+" pill, with room above and below it when comfortable (36) and a
/// pixel each way when compact (30).
pub fn tab_strip_height(density: Density) -> i32 {
    match density {
        Density::Comfortable => NEW_TAB_SIZE + 2 * NEW_TAB_INSET,
        Density::Compact => NEW_TAB_SIZE + 2,
    }
}
pub(super) fn tab_line() -> i32 {
    line(detail_font_size(), 3)
}
/// Room for the branch icon left of the branch name.
pub(super) const BRANCH_ICON_WIDTH: i32 = 10;
pub(super) const BRANCH_ICON_GAP: i32 = 5;
/// Room for each Git mark after a workspace's branch (uncommitted changes,
/// ahead, behind), and the gap before it.
pub(super) const GIT_MARK_WIDTH: i32 = 8;
pub(super) const GIT_MARK_GAP: i32 = 4;
/// Width of the workspace sidebar, in logical pixels: at start, and the
/// range it can be dragged to.
pub const SIDEBAR_WIDTH: i32 = 200;
pub const SIDEBAR_MIN_WIDTH: i32 = 120;
pub const SIDEBAR_MAX_WIDTH: i32 = 480;
/// Where the sidebar's rows start, under its header.
pub(super) const ROWS_TOP: i32 = BAR_HEIGHT + 6;
/// Horizontal inset of the rounded sidebar row highlight.
pub(super) const ROW_INSET: i32 = 6;
pub(super) const ROW_RADIUS: f32 = 6.0;
pub(super) const TAB_MAX_WIDTH: i32 = 360;
pub(super) const BUTTON_WIDTH: i32 = 36;
/// Side of the rounded square behind the tab strip's "+", which reaches on
/// over its menu chevron, and the room left of it.
pub(super) const NEW_TAB_SIZE: i32 = 28;
pub(super) const NEW_TAB_INSET: i32 = (BUTTON_WIDTH - NEW_TAB_SIZE) / 2;
/// The sidebar header's buttons: "add project", and the split "add terminal"
/// button with its menu chevron.
pub(super) const HEADER_BUTTON: i32 = 28;
pub(super) const CHEVRON_WIDTH: i32 = 16;
/// Space between the chevron and the sidebar's right edge.
pub(super) const HEADER_MARGIN: i32 = 4;
/// Room for the workspace kind icon left of each sidebar row's name.
pub(super) const ROW_ICON_WIDTH: i32 = 14;
pub(super) const ROW_ICON_GAP: i32 = 8;
/// An app's own icon in a switcher row: bigger than the drawn ones, reaching
/// a little into the room around them.
pub(super) const APP_ICON_SIZE: i32 = 20;
/// How big an app icon a sidebar row shows for its workspace.
pub(super) const ROW_PICTURE_SIZE: i32 = 16;
pub(super) const GAP: i32 = 1;
/// A line under an agent's row in the sidebar: a subagent's, a GitHub
/// section's.
pub(super) fn card_line_px() -> i32 {
    line(detail_font_size(), 5)
}
/// Room for the status mark left of the agent's name.
pub(super) const STATUS_WIDTH: i32 = 10;
/// How big an agent's badge (its icon) is on a row of the sidebar or the
/// switcher.
pub(super) const AGENT_ICON: i32 = 12;
pub(super) const STATUS_GAP: i32 = 7;
/// The status pill at the right of a card's header.
pub(super) fn pill_height() -> i32 {
    line(tiny_font_size(), 4)
}
pub(super) const PILL_PADDING: i32 = 6;
/// Gap between a detail's label and its value.
pub(super) const FIELD_GAP: i32 = 8;

// Text: every size the chrome sets text in, all here and all grown from
// `text_size()`, so changing it makes all the chrome's text bigger or smaller
// and keeps their steps. (`[ui] zoom` grows everything at run time.)
/// The chrome's text size when nothing sets another.
pub const DEFAULT_TEXT: f32 = 17.0;
/// The smallest text size `set_text_size` takes.
pub const TEXT_MIN: f32 = 12.0;
/// The largest text size `set_text_size` takes.
pub const TEXT_MAX: f32 = 24.0;
/// The current text size, as `f32` bits: one for the whole process.
static TEXT_PX: AtomicU32 = AtomicU32::new(DEFAULT_TEXT.to_bits());
/// The chrome's current text size, which every size below steps from.
pub fn text_size() -> f32 {
    f32::from_bits(TEXT_PX.load(Ordering::Relaxed))
}
/// Sets the chrome's text size, held to `TEXT_MIN..=TEXT_MAX`. Cached
/// drawings and layouts see the change and are made again.
pub fn set_text_size(px: f32) {
    let px = if px.is_nan() {
        DEFAULT_TEXT
    } else {
        px.clamp(TEXT_MIN, TEXT_MAX)
    };
    TEXT_PX.store(px.to_bits(), Ordering::Relaxed);
}
/// `memo`, emptied first when the text size changed since it was filled:
/// for memos of measured text or layout, which hold for one size only.
pub(super) fn sized<T: Default>(memo: &mut (u32, T)) -> &mut T {
    let now = text_size().to_bits();
    if memo.0 != now {
        *memo = (now, T::default());
    }
    &mut memo.1
}
/// The chrome's own text: rows, buttons, fields, dialogs.
pub(super) fn font_size() -> f32 {
    text_size()
}
/// The welcome screen's greeting.
pub(super) fn greeting_font_size() -> f32 {
    text_size() + 13.0
}
/// A feature tab's title, and its page's headings.
pub(super) fn title_font_size() -> f32 {
    text_size() + 5.0
}
/// Tab labels and workspace names, and a feature page's body.
pub(super) fn label_font_size() -> f32 {
    text_size() + 2.0
}
/// Secondary text: details under a row, hints, tooltips.
pub(super) fn small_font_size() -> f32 {
    text_size() - 2.0
}
/// Agent card details, and their ids and status pill.
pub(super) fn detail_font_size() -> f32 {
    text_size() - 3.0
}
/// The smallest text (counts on badges and pills): the same size as the
/// details, one size under `small_font_size()`.
pub(super) fn tiny_font_size() -> f32 {
    detail_font_size()
}
/// Keys on keycaps.
pub(super) fn key_font_size() -> f32 {
    small_font_size()
}
/// Monospace text read as output: the scrollback, an inbox item's blocks.
pub(super) fn mono_font_size() -> f32 {
    small_font_size()
}
/// Monospace code: diffs, commits and files in the repositories panel.
pub(super) fn code_font_size() -> f32 {
    text_size() - 1.0
}
/// How high something holding a line of `size` text is: the text and
/// `room` more around it. Heights made with it grow with the text.
pub(super) const fn line(size: f32, room: i32) -> i32 {
    size as i32 + room
}
pub(super) const PADDING: f32 = 10.0;
pub(super) const HELP_MARGIN: i32 = 16;
pub(super) const HELP_PADDING: i32 = 14;
/// The help panel's corners, as every popup's (`POPUP_RADIUS`).
pub(super) const HELP_RADIUS: f32 = POPUP_RADIUS;
/// Room around popups (help panel, menu) for their drop shadow.
pub(super) const SHADOW: i32 = 12;
pub(super) fn help_title_height() -> i32 {
    line(font_size(), 25)
}
pub(super) fn help_row_height() -> i32 {
    line(font_size(), 15)
}
pub(super) const HELP_KEY_GAP: i32 = 16;
pub(super) const HELP_COLUMN_GAP: i32 = 32;
/// Keycaps: text padding, gap around the "/" between alternatives, corner radius.
pub(super) const CAP_PADDING: i32 = 6;
pub(super) const CAP_GAP: i32 = 5;
pub(super) const CAP_RADIUS: f32 = 4.0;
/// The menu under the terminal button's chevron.
pub(super) const MENU_WIDTH: i32 = 180;
pub(super) const MENU_PADDING: i32 = 4;
pub(super) fn menu_item_height() -> i32 {
    line(font_size(), 15)
}
/// What row highlights were rounded from (`MENU_RADIUS - 2.0` is
/// `ROW_HIGHLIGHT_RADIUS`); a popup's own corners are `POPUP_RADIUS`.
pub(super) const MENU_RADIUS: f32 = 6.0;

/// The switcher, at the top of the app area.
pub(super) const SWITCHER_WIDTH: i32 = 640;
pub(super) const SWITCHER_TOP: i32 = 24;
pub(super) fn switcher_input() -> i32 {
    line(font_size(), 29)
}
pub(super) const SWITCHER_PADDING: i32 = 6;
/// A section heading's band in the switcher (its rows are taller, see
/// chrome/switcher.rs).
pub(super) fn switcher_section() -> i32 {
    line(small_font_size(), 21)
}
/// The most rows it shows; it scrolls for more.
pub(super) const SWITCHER_ROWS: i32 = 12;
/// The caret after the query.
pub(super) const CARET_WIDTH: i32 = 2;
pub(super) fn caret_height() -> i32 {
    line(font_size(), 3)
}

/// The welcome screen: a column at most this wide, centred on the output.
pub(super) const WELCOME_WIDTH: i32 = 560;
/// From the top: greeting, subtitle, a gap, the "Recent projects" label,
/// the rows, a gap, the button and the note under it.
pub(super) fn greeting_height() -> i32 {
    line(greeting_font_size(), 12)
}
pub(super) fn subtitle_height() -> i32 {
    line(font_size(), 9)
}
pub(super) const WELCOME_GAP: i32 = 24;
pub(super) fn welcome_label_px() -> i32 {
    line(small_font_size(), 11)
}
/// The most rows it shows; it scrolls for more.
pub(super) const WELCOME_ROWS: i32 = 8;
pub(super) fn welcome_button() -> i32 {
    line(font_size(), 21)
}
pub(super) const WELCOME_BUTTON_WIDTH: i32 = 200;
pub(super) const NOTE_GAP: i32 = 6;
pub(super) fn note_height() -> i32 {
    line(small_font_size(), 7)
}
/// The key hints, a margin above the output's bottom edge.
pub(super) fn hints_height() -> i32 {
    line(small_font_size(), 11)
}
/// Where the middle of the welcome screen sits, as a share of the output's height.
pub(super) const WELCOME_MIDDLE: f64 = 0.42;
/// Space between the hints' entries, and between each key and its action.
pub(super) const HINT_GAP: i32 = 18;
pub(super) const HINT_KEY_GAP: i32 = 6;
/// Width of the focus ring around the selected button, and its gap to it.
pub(super) const FOCUS_RING: f32 = 1.5;
pub(super) const FOCUS_GAP: f32 = 2.0;

/// The quit dialog, centred on the output: title, question, a checkbox
/// row per thing to stop and "Don't ask again", and the buttons at the
/// bottom right. Every modal dialog is laid out with these.
pub(super) const QUIT_PADDING: i32 = 20;
pub(super) fn quit_title() -> i32 {
    line(label_font_size(), 7)
}
pub(super) fn quit_body() -> i32 {
    line(font_size(), 7)
}
pub(super) const QUIT_LINE_GAP: i32 = 10;
/// The checkbox row: the box and its label, all of it clickable.
pub(super) fn checkbox_row() -> i32 {
    line(font_size(), 9)
}
pub(super) const CHECKBOX_SIZE: i32 = 16;
pub(super) const CHECKBOX_GAP: i32 = 8;
pub(super) fn quit_button() -> i32 {
    line(font_size(), 17)
}
pub(super) const QUIT_BUTTON_GAP: i32 = 8;
pub(super) const BUTTON_RADIUS: f32 = 6.0;

/// Shared look tokens: every popup's corner (help, menus, tooltips,
/// toasts, dialogs), every list row's highlight corner, and every modal
/// dialog's width.
pub(super) const POPUP_RADIUS: f32 = 8.0;
pub(super) const ROW_HIGHLIGHT_RADIUS: f32 = 4.0;
pub(super) const DIALOG_WIDTH: i32 = 560;

/// The sidebar header's project name: the gap between it and its chevron,
/// and the room after the chevron inside the highlight.
pub(super) const HEADER_NAME_GAP: i32 = 2;
pub(super) const HEADER_NAME_END: i32 = 4;

/// The project menu: a panel at most this wide, just below the sidebar
/// header, reaching over the apps.
pub(super) const PROJECT_MENU_WIDTH: i32 = 440;
pub(super) const PROJECT_MENU_TOP: i32 = BAR_HEIGHT + 2;
/// The query field (the title in form mode), the space around the list,
/// and the footer with the key hints.
pub(super) fn project_menu_input() -> i32 {
    line(font_size(), 29)
}
pub(super) const PROJECT_MENU_PADDING: i32 = 6;
pub(super) fn project_menu_footer() -> i32 {
    line(small_font_size(), 23)
}
/// A project, "New project" or folder row, and a section label.
pub(super) fn project_menu_row() -> i32 {
    line(font_size(), 25)
}
pub(super) fn project_menu_section() -> i32 {
    line(small_font_size(), 9)
}
/// The fewest and most entries the list shows; it scrolls for more.
pub(super) const PROJECT_MENU_MIN_ROWS: i32 = 3;
pub(super) const PROJECT_MENU_MAX_ROWS: i32 = 12;
/// Room for the running dot or the icon left of each row's label, and the
/// gap after it.
pub(super) const MENU_MARK_WIDTH: i32 = 14;
pub(super) const MENU_MARK_GAP: i32 = 8;
/// The square buttons on a project row under the pointer: their side, the
/// gap between them and the room after the last.
pub(super) fn row_button() -> i32 {
    line(font_size(), 11)
}
pub(super) const ROW_BUTTON_GAP: i32 = 2;
pub(super) const ROW_BUTTON_INSET: i32 = 6;
/// The field a project row's name turns into while renaming: its height,
/// and how far it reaches left of the name.
pub(super) fn rename_field_px() -> i32 {
    line(font_size(), 13)
}
pub(super) const RENAME_INSET: i32 = 6;

/// The project menu's form: its inner padding, the column of labels, a row
/// and the field in it, and the gap between parts side by side.
pub(super) const FORM_PADDING: i32 = 14;
pub(super) const FORM_LABEL_WIDTH: i32 = 64;
pub(super) fn form_row() -> i32 {
    line(font_size(), 25)
}
pub(super) fn form_field() -> i32 {
    line(font_size(), 17)
}
pub(super) const FORM_GAP: i32 = 8;
/// The Browse button right of the folder field.
pub(super) const BROWSE_WIDTH: i32 = 84;
/// A folder offered under the folder field.
pub(super) fn suggestion_row() -> i32 {
    line(font_size(), 13)
}
/// The "Opens" choices: what kind of workspace the new project's folder
/// opens as (`WorkspaceKind::Project` is the Zed + terminal workspace, not
/// a project), its label and its width; their height, and the side of the
/// round mark before the label.
pub(super) const KIND_CHOICES: [(WorkspaceKind, &str, i32); 2] = [
    (WorkspaceKind::Project, "Zed + terminal", 150),
    (WorkspaceKind::Terminal, "Terminal only", 130),
];
pub(super) fn kind_height() -> i32 {
    line(font_size(), 13)
}
pub(super) const RADIO_SIZE: i32 = 16;
/// The line saying why the project can't be created.
pub(super) fn error_height() -> i32 {
    line(small_font_size(), 9)
}
/// Cancel and Create & switch: labels and widths.
pub(super) const FORM_BUTTONS: [(&str, i32); 2] = [("Cancel", 84), ("Create & switch", 136)];
/// Cancel, Run now and Save in the project settings form: labels and widths.
pub(super) const SETTINGS_BUTTONS: [(&str, i32); 3] =
    [("Cancel", 84), ("Run now", 96), ("Save", 84)];

/// How high a pane's bar is: the band along its pane's top, over its
/// window, which starts under it. The bar's buttons are this wide, this
/// far apart and in from its ends.
pub fn pane_bar_height() -> i32 {
    line(detail_font_size(), 16)
}
pub(super) const BAR_BUTTON: i32 = 24;
pub(super) const BAR_BUTTON_GAP: i32 = 2;
pub(super) const BAR_INSET: i32 = 4;
pub(super) const BAR_NOTE_GAP: i32 = 8;

/// How wide the output must be for a file under review to show its
/// preview beside it (see `doc_tab`), in logical pixels.
pub const DOCK_MIN_WIDTH: i32 = 1400;
/// The review's share of the output's width beside its preview, as it
/// starts and as far as its split is dragged.
pub const DOCK_SHARE: f64 = 0.5;
pub const DOCK_SHARE_MIN: f64 = 0.3;
pub const DOCK_SHARE_MAX: f64 = 0.7;
/// The room between the review and its preview, and around the preview.
pub const DOCK_GAP: i32 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_memo_empties_when_the_text_size_changed() {
        // Drawn at another size (or never): emptied, then kept.
        let mut memo = ((text_size() + 1.0).to_bits(), vec![1]);
        assert!(sized(&mut memo).is_empty());
        sized(&mut memo).push(2);
        assert_eq!(sized(&mut memo), &[2]);
    }
}
