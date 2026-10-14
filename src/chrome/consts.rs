//! Sizes and geometry of the chrome, in logical pixels unless said otherwise.

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
/// The tab strip: a title row, then a line for what runs in the tab.
pub const TAB_STRIP_HEIGHT: i32 = TAB_TITLE_HEIGHT + TAB_LINE + TAB_BOTTOM;

/// How high the tab strip is at `density`: one line of tabs around the
/// "+" pill, with room above and below it when comfortable (36) and a
/// pixel each way when compact (30).
pub fn tab_strip_height(density: Density) -> i32 {
    match density {
        Density::Comfortable => NEW_TAB_SIZE + 2 * NEW_TAB_INSET,
        Density::Compact => NEW_TAB_SIZE + 2,
    }
}
pub(super) const TAB_TITLE_HEIGHT: i32 = 28;
pub(super) const TAB_LINE: i32 = 15;
pub(super) const TAB_BOTTOM: i32 = 5;
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
/// A workspace row: just its name, or its name in a shorter title band,
/// then the same detail lines as a tab.
pub(super) const ROW_HEIGHT: i32 = 32;
pub(super) const ROW_TITLE_HEIGHT: i32 = 28;
pub(super) const ROW_BOTTOM: i32 = 6;
pub(super) const ROWS_TOP: i32 = BAR_HEIGHT + 6;
/// Horizontal inset of the rounded sidebar row highlight.
pub(super) const ROW_INSET: i32 = 6;
pub(super) const ROW_RADIUS: f32 = 6.0;
pub(super) const CLOSE_WIDTH: i32 = 28;
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
pub(super) const GAP: i32 = 1;
/// Agent cards, under their workspace's row: indent past the row's icon,
/// space around them, inner padding, the header line and each detail line.
pub(super) const CARD_INDENT: i32 = 14;
pub(super) const CARD_GAP: i32 = 4;
pub(super) const CARD_PAD: i32 = 7;
pub(super) const CARD_HEADER: i32 = 20;
pub(super) const CARD_LINE: i32 = 17;
pub(super) const CARD_RADIUS: f32 = 6.0;
/// Room for the status mark left of the agent's name.
pub(super) const STATUS_WIDTH: i32 = 10;
pub(super) const STATUS_GAP: i32 = 7;
/// The status pill at the right of a card's header.
pub(super) const PILL_HEIGHT: i32 = 16;
pub(super) const PILL_PADDING: i32 = 6;
/// Gap between a detail's label and its value.
pub(super) const FIELD_GAP: i32 = 8;
pub(super) const FONT_SIZE: f32 = 15.0;
/// Tab labels and workspace names.
pub(super) const LABEL_FONT_SIZE: f32 = 17.0;
pub(super) const SMALL_FONT_SIZE: f32 = 13.0;
/// Agent card details, and their ids and status pill.
pub(super) const DETAIL_FONT_SIZE: f32 = 12.0;
pub(super) const TINY_FONT_SIZE: f32 = 12.0;
pub(super) const KEY_FONT_SIZE: f32 = 13.0;
pub(super) const ICON_SIZE: f32 = 20.0;
pub(super) const PADDING: f32 = 10.0;
pub(super) const HELP_MARGIN: i32 = 16;
pub(super) const HELP_PADDING: i32 = 14;
pub(super) const HELP_RADIUS: f32 = 8.0;
/// Room around popups (help panel, menu) for their drop shadow.
pub(super) const SHADOW: i32 = 12;
pub(super) const HELP_TITLE_HEIGHT: i32 = 40;
pub(super) const HELP_ROW_HEIGHT: i32 = 30;
pub(super) const HELP_KEY_GAP: i32 = 16;
pub(super) const HELP_COLUMN_GAP: i32 = 32;
/// Keycaps: text padding, gap around the "/" between alternatives, corner radius.
pub(super) const CAP_PADDING: i32 = 6;
pub(super) const CAP_GAP: i32 = 5;
pub(super) const CAP_RADIUS: f32 = 4.0;
/// The menu under the terminal button's chevron.
pub(super) const MENU_WIDTH: i32 = 180;
pub(super) const MENU_PADDING: i32 = 4;
pub(super) const MENU_ITEM_HEIGHT: i32 = 30;
pub(super) const MENU_RADIUS: f32 = 6.0;

/// The switcher, at the top of the app area.
pub(super) const SWITCHER_WIDTH: i32 = 640;
pub(super) const SWITCHER_TOP: i32 = 24;
pub(super) const SWITCHER_INPUT: i32 = 44;
pub(super) const SWITCHER_PADDING: i32 = 6;
pub(super) const SWITCHER_ROW: i32 = 34;
/// The most rows it shows; it scrolls for more.
pub(super) const SWITCHER_ROWS: i32 = 12;
/// The caret after the query.
pub(super) const CARET_WIDTH: i32 = 2;
pub(super) const CARET_HEIGHT: i32 = 18;

/// The welcome screen: a column at most this wide, centred on the output.
pub(super) const WELCOME_WIDTH: i32 = 560;
pub(super) const GREETING_FONT_SIZE: f32 = 28.0;
/// From the top: greeting, subtitle, a gap, the "Recent projects" label,
/// the rows, a gap, the button and the note under it.
pub(super) const GREETING_HEIGHT: i32 = 40;
pub(super) const SUBTITLE_HEIGHT: i32 = 24;
pub(super) const WELCOME_GAP: i32 = 24;
pub(super) const WELCOME_LABEL: i32 = 24;
/// A project row: its name, detail and age on one line.
pub(super) const WELCOME_ROW: i32 = 44;
/// The most rows it shows; it scrolls for more.
pub(super) const WELCOME_ROWS: i32 = 8;
pub(super) const WELCOME_BUTTON: i32 = 36;
pub(super) const WELCOME_BUTTON_WIDTH: i32 = 200;
pub(super) const NOTE_GAP: i32 = 6;
pub(super) const NOTE_HEIGHT: i32 = 20;
/// The key hints, a margin above the output's bottom edge.
pub(super) const HINTS_HEIGHT: i32 = 24;
/// Where the middle of the welcome screen sits, as a share of the output's height.
pub(super) const WELCOME_MIDDLE: f64 = 0.42;
/// Space between the hints' entries, and between each key and its action.
pub(super) const HINT_GAP: i32 = 18;
pub(super) const HINT_KEY_GAP: i32 = 6;
/// Width of the focus ring around the selected button, and its gap to it.
pub(super) const FOCUS_RING: f32 = 1.5;
pub(super) const FOCUS_GAP: f32 = 2.0;

/// The quit dialog, centred on the output: title, question, the "Don't ask
/// again" checkbox, and the buttons at the bottom right.
pub(super) const QUIT_WIDTH: i32 = 420;
pub(super) const QUIT_HEIGHT: i32 = 190;
pub(super) const QUIT_PADDING: i32 = 20;
pub(super) const QUIT_TITLE: i32 = 24;
pub(super) const QUIT_BODY: i32 = 22;
pub(super) const QUIT_LINE_GAP: i32 = 10;
/// The checkbox row: the box and its label, all of it clickable.
pub(super) const CHECKBOX_ROW: i32 = 24;
pub(super) const CHECKBOX_WIDTH: i32 = 150;
pub(super) const CHECKBOX_SIZE: i32 = 16;
pub(super) const CHECKBOX_GAP: i32 = 8;
pub(super) const QUIT_BUTTON: i32 = 32;
pub(super) const QUIT_BUTTON_GAP: i32 = 8;
/// Cancel, Stop project, Keep running: labels and widths.
pub(super) const QUIT_BUTTONS: [(&str, i32); 3] =
    [("Cancel", 84), ("Stop project", 116), ("Keep running", 124)];
pub(super) const BUTTON_RADIUS: f32 = 6.0;

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
pub(super) const PROJECT_MENU_INPUT: i32 = 44;
pub(super) const PROJECT_MENU_PADDING: i32 = 6;
pub(super) const PROJECT_MENU_FOOTER: i32 = 36;
/// A project, "New project" or folder row, and a section label.
pub(super) const PROJECT_MENU_ROW: i32 = 40;
pub(super) const PROJECT_MENU_SECTION: i32 = 22;
/// The fewest and most entries the list shows; it scrolls for more.
pub(super) const PROJECT_MENU_MIN_ROWS: i32 = 3;
pub(super) const PROJECT_MENU_MAX_ROWS: i32 = 12;
/// Room for the running dot or the icon left of each row's label, and the
/// gap after it.
pub(super) const MENU_MARK_WIDTH: i32 = 14;
pub(super) const MENU_MARK_GAP: i32 = 8;
/// The square buttons on a project row under the pointer: their side, the
/// gap between them and the room after the last.
pub(super) const ROW_BUTTON: i32 = 26;
pub(super) const ROW_BUTTON_GAP: i32 = 2;
pub(super) const ROW_BUTTON_INSET: i32 = 6;
/// The field a project row's name turns into while renaming: its height,
/// and how far it reaches left of the name.
pub(super) const RENAME_FIELD: i32 = 28;
pub(super) const RENAME_INSET: i32 = 6;

/// The project menu's form: its inner padding, the column of labels, a row
/// and the field in it, and the gap between parts side by side.
pub(super) const FORM_PADDING: i32 = 14;
pub(super) const FORM_LABEL_WIDTH: i32 = 64;
pub(super) const FORM_ROW: i32 = 40;
pub(super) const FORM_FIELD: i32 = 32;
pub(super) const FORM_GAP: i32 = 8;
/// The Browse button right of the folder field.
pub(super) const BROWSE_WIDTH: i32 = 84;
/// A folder offered under the folder field.
pub(super) const SUGGESTION_ROW: i32 = 28;
/// The "Opens" choices: what each opens, its label and its width; their
/// height, and the side of the round mark before the label.
pub(super) const KIND_CHOICES: [(WorkspaceKind, &str, i32); 2] = [
    (WorkspaceKind::Project, "Zed + terminal", 150),
    (WorkspaceKind::Terminal, "Terminal only", 130),
];
pub(super) const KIND_HEIGHT: i32 = 28;
pub(super) const RADIO_SIZE: i32 = 16;
/// The line saying why the project can't be created.
pub(super) const ERROR_HEIGHT: i32 = 22;
/// Cancel and Create & switch: labels and widths.
pub(super) const FORM_BUTTONS: [(&str, i32); 2] = [("Cancel", 84), ("Create & switch", 136)];
/// Cancel, Run now and Save in the project settings form: labels and widths.
pub(super) const SETTINGS_BUTTONS: [(&str, i32); 3] =
    [("Cancel", 84), ("Run now", 96), ("Save", 84)];

/// The notice: a pill this high, its text this far in from its ends, this
/// far below the tab strip.
pub(super) const NOTICE_HEIGHT: i32 = 30;
pub(super) const NOTICE_PADDING: i32 = 16;
pub(super) const NOTICE_TOP: i32 = 8;

/// The bar over each terminal pane: this high, with its restart and stop
/// buttons this wide at its right end, this far apart and in from the end.
pub const PANE_BAR_HEIGHT: i32 = 24;
pub(super) const BAR_BUTTON: i32 = 24;
pub(super) const BAR_BUTTON_GAP: i32 = 2;
pub(super) const BAR_INSET: i32 = 4;
pub(super) const BAR_NOTE_GAP: i32 = 8;
