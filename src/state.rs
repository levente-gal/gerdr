//! The groups of `Gerdr`'s fields: each a part of gerdr's state that its
//! own code reads and changes.

use super::*;

/// The apps and agents the launcher offers, and where it was used.
pub struct Launcher {
    /// How often and recently things were switched to, for ordering lists.
    pub frecency: Frecency,
    /// The folders zoxide knows, the highest ranked first.
    pub recent_dirs: Vec<PathBuf>,
    /// Where a read of zoxide's folders sends them; `None` until set up.
    pub zoxide: Option<channel::Sender<Vec<PathBuf>>>,
    /// Where an import of an app's profile tells how it went; `None` until
    /// set up (see `profile_import`).
    pub profile_imports: Option<channel::Sender<profile_import::Imported>>,
    /// The apps named in the config file, to launch into a split.
    pub apps: Vec<launch::App>,
    /// The apps installed on the system, offered in the run prompt.
    pub installed: Vec<launch::App>,
    pub cli_agents: Vec<cli_agents::CliAgent>,
    /// Where a read of the installed apps sends them; `None` until set up.
    pub installed_tx: Option<channel::Sender<Vec<launch::App>>>,
}

/// What settings.toml says, as last read (see `config::apply_settings`).
pub struct Settings {
    /// The look chosen: the system's, dark or light.
    pub theme_mode: ThemeMode,
    /// Whether the system prefers a dark look (see `system_theme`).
    pub system_dark: bool,
    /// How much room the chrome takes, from the config file.
    pub density: Density,
    /// Whether the sidebar shows its "Needs you" list, from the config file.
    pub needs_you: bool,
    /// Whether agents that need the user say so with a desktop
    /// notification, from the config file.
    pub notify: bool,
    /// Whether new Alacrittys reload their config file (`[terminal]`).
    pub live_config_reload: bool,
    /// When the config file was last read as written (see
    /// `Gerdr::watch_settings`).
    pub settings_modified: Option<std::time::SystemTime>,
    /// How wide the sidebar is, kept in `state.json` for the next start.
    /// Whether the footer shows, from the config file.
    pub show_footer: bool,
    /// Whether T3 Code's server has a log, as last looked at.
    pub t3_log: bool,
    /// The agent CLI commands the config file names (see `cli_agents`),
    /// and the agent CLIs installed, as of the last look.
    pub agent_config: std::collections::BTreeMap<String, String>,
}

/// The keyboard: the keymap, the prefix, and keys held or repeating.
pub struct KeyState {
    /// Keys whose press was a gerdr shortcut; their release is swallowed too.
    pub suppressed_keys: Vec<Keycode>,
    /// The prefix was pressed; the next key is a gerdr command.
    pub prefix_armed: bool,
    /// The prefix and the keys after it, from the config file.
    pub keymap: keys::Keymap,
    /// The key held over an overlay, and the timer repeating it.
    pub key_repeat: Option<(Keycode, RegistrationToken)>,
}

/// The pointer: where it is, what it presses and what it drags.
pub struct Mouse {
    /// The divider being dragged, while the left button holds it.
    pub drag: Option<Drag>,
    /// Where the held pane (see `reorder`) would land if dropped now.
    pub pane_drop: Option<pane_drag::PaneDrop>,
    /// The workspace row or tab held, while the left button holds it.
    pub reorder: Option<Reorder>,
    /// The cursor shape gerdr last set over its chrome.
    pub chrome_cursor: CursorIcon,
    /// Where the pointer is, in window coordinates.
    pub pos: Point<f64, Logical>,
    /// The last press anywhere (when, where, which button), to tell a
    /// double click.
    pub last_press: Option<(Instant, Point<f64, Logical>, u32)>,
    /// Whether a scrolled list moved on its own or under the fingers at the
    /// last frame (see `tick_scrolls`).
    pub scrolling: bool,
    /// How many left presses in a row the last was, a moment apart and in
    /// about one place (see `count_press`).
    pub press_count: u32,
    /// While a press in a text field is held past its text: when it last
    /// went on for it, and the part of a step carried to the next frame.
    pub field_edge: Option<(std::time::Instant, f64)>,
    /// A key, a paste or a press may have moved a text field's caret since
    /// the last frame: `keep_field_scroll` keeps where its view starts.
    pub field_scroll_due: bool,
    /// A press in an overlay's text field is held: moving selects.
    pub field_drag: bool,
    /// The left button went down on the repositories panel's diff and is
    /// held: moving drags the diff cursor's range on.
    pub repos_rows_drag: bool,
    /// The left button went down on the comment being written in the
    /// diff and is held: moving selects its text.
    pub repos_editor_drag: bool,
    /// While a press on the diff (either of those) is held past its rows:
    /// when they last scrolled for it, and the part of a row carried to
    /// the next frame.
    pub repos_drag_edge: Option<(std::time::Instant, f64)>,
    /// A scrollbar's thumb held with the left button: moving drags it.
    pub thumb_drag: Option<scrollbars::ThumbDrag>,
}

/// What the pointer is over in the chrome, and what that shows.
pub struct Hover {
    /// What of the footer the pointer is on.
    pub footer: Option<chrome::FooterHit>,
    /// The key help stays open until closed again.
    pub help_pinned: bool,
    /// What the pointer is on in the help panel, while it shows.
    pub help: Option<HelpHit>,
    /// What the pointer is over in the tab strip, and how far it has lit
    /// up (or faded since the pointer left).
    pub tab: anim::Hover<chrome::TabHit>,
    /// What the pointer is over in the sidebar, with its fades.
    pub sidebar: anim::Hover<chrome::SidebarHit>,
    /// The tooltip offered for what is under the pointer, and the one shown.
    pub tooltip: TooltipState,
    /// The workspace whose header shows the strip of agents to start,
    /// while the pointer is on its AI button or the strip.
    pub strip: Option<usize>,
    /// The record of the workspace `agent_strip` opened on: a strip whose
    /// workspace moved or closed since isn't shown on another's header.
    pub strip_record: Option<String>,
    /// The workspace whose strip of agents opens, and when, while the
    /// pointer rests on its AI button; whether a timer waits for it.
    pub strip_due: Option<(usize, std::time::Instant)>,
    pub strip_timed: bool,
    /// The pointer is over the sidebar header's project name.
    pub header: bool,
    /// The pane bar button under the pointer, by pane.
    pub pane_bar: Option<(u32, BarButton)>,
}

/// The popups and panels over the apps; none open is all `None`.
pub struct Overlays {
    /// The tasks panel, while it is open over the footer.
    pub tasks_panel: Option<tasks::TasksPanel>,
    /// The machines panel, while it is open (see `machines_panel`).
    pub machines_panel: Option<machines_panel::MachinesPanel>,
    /// A workspace or tab being renamed in place.
    pub rename: Option<Rename>,
    /// The scrollback viewer, while it is open over a terminal pane.
    pub scrollback: Option<scrollback::Scrollback>,
    /// The herdr pane whose output is being read for the viewer, off the
    /// event loop (see `scrollback_panel`).
    pub scrollback_reading: Option<String>,
    /// The folder picker, while it is open, and what it is open for.
    pub picker: Option<(Child, PickFor)>,
    /// The context menu open, if one is.
    pub context_menu: Option<ContextMenu>,
    /// The switcher, while it is open.
    pub switcher: Option<switcher::Switcher>,
    /// The welcome screen, while it is shown.
    pub welcome: Option<welcome::Welcome>,
    /// The project menu, while it is open.
    pub project_menu: Option<project_menu::ProjectMenu>,
    /// The quit dialog, while it is open.
    pub quit: Option<quit::QuitDialog>,
    /// The restart dialog, while it is open.
    pub restart: Option<restart::RestartDialog>,
    /// The checkout dialog, while it is open.
    pub checkout: Option<checkout::CheckoutDialog>,
    /// The confirm dialog, while it is open.
    pub confirm: Option<confirm::ConfirmDialog>,
    /// The stats panel on the AI work, while it is open (see `stats_panel`).
    pub(crate) ai_stats: Option<stats_panel::StatsPanel>,
    /// The stats panel as it was when it opened a file in the scrollback
    /// viewer: closing the viewer shows it again.
    pub(crate) ai_stats_behind: Option<stats_panel::StatsPanel>,
}

/// GitHub CI of the repos: what was read, and the read under way.
pub struct CiWatch {
    /// The CI of the shown project's repositories, by root (see
    /// `ci_watch`).
    pub repos: HashMap<PathBuf, ci::RepoCi>,
    /// The repositories the last round of reading the CI was asked about.
    pub asked: Vec<PathBuf>,
    /// Where each row of the CI stood, for telling what finished.
    pub states: HashMap<(String, ci::Subject), ci::CiState>,
    /// A round of reading the CI is still running, and another is wanted
    /// once it is done.
    pub pending: bool,
    pub again: bool,
    /// Hands a round's CI to the event loop (see `watch_ci`).
    pub sender: Option<channel::Sender<ci_watch::CiRead>>,
}

/// Git status of the repos: what was read, and the read under way.
pub struct GitWatch {
    /// The Git status of the workspace folders in a repository, by folder.
    pub status: HashMap<PathBuf, git::GitStatus>,
    /// A round of reading the Git status is still running.
    pub pending: bool,
    /// Another round is wanted once it is done.
    pub again: bool,
    /// Hands a round's Git status to the event loop (see `watch_git`).
    pub read: Option<channel::Sender<HashMap<PathBuf, git::GitStatus>>>,
    /// The address of each workspace folder's repository page, once read
    /// (see `git::web_url`).
    pub repo_pages: HashMap<PathBuf, Option<String>>,
}

/// Back and forward through visited tabs (see `nav_history`).
pub struct Nav {
    /// The tabs shown, to go back and forward through.
    pub history: nav_history::NavHistory<nav_history::Visit>,
    /// A step back or forward to a tab of another project, waiting for it
    /// to show (see `nav`).
    pub pending: Option<nav::PendingVisit>,
    /// The history as it was last put in the store (see `Gerdr::keep_nav`).
    pub kept: Option<nav_history::NavHistory<nav_history::Visit>>,
}

/// Apps kept across gerdr restarts (see `keep`).
pub struct Keep {
    /// The apps projects keep running, as last looked at (see `tasks`).
    pub kept: Vec<tasks::Kept>,
    /// The programs whose apps keep running when gerdr quits (see `keep`).
    pub apps: Vec<String>,
    /// The shown project's server of apps kept running, while gerdr shows
    /// its windows.
    pub keeper: Option<keep::Keeper>,
    /// Whether gerdr said that keeping apps running needs wprs.
    pub warned: bool,
}

/// Child processes and process groups gerdr started and must end.
pub struct Procs {
    pub children: Vec<Child>,
    /// Process groups we started, all terminated when gerdr exits.
    pub groups: Vec<i32>,
    /// Process groups of projects switched away from, until they are gone;
    /// their late windows are closed instead of placed.
    pub retired: Vec<i32>,
    /// The process groups of the Zed launches of the shown project.
    pub zed: Vec<i32>,
    /// Those of the project switched away from, while they may still take
    /// the next `zed -n`, and until when they are waited for.
    pub old_zed: Vec<i32>,
    pub old_zed_until: Option<Instant>,
}

/// The project's start, stop and attach (see `projects`).
pub struct Life {
    /// The project to stop once another is shown instead, and what of it
    /// (see `ask_switch`).
    pub stop_left: Option<(String, projects::Stops)>,
    /// The lock that tells other gerdrs the project shown is open here;
    /// kept while it is shown again after a reload.
    pub project_lock: Option<project_lock::ProjectLock>,
    /// Says to other machines that this gerdr shows the project: `project_lock`
    /// is under a runtime folder they can't name (see `feature::lease`).
    pub lease_lock: Option<project_lock::ProjectLock>,
    /// Started with `--attach`: projects another gerdr has open may be
    /// shown here too.
    pub attach: bool,
    /// Whether each project's session runs, by id, as last checked.
    pub running: HashMap<String, bool>,
    /// Where a check of which projects' sessions run sends its answer (by
    /// project id); `None` until set up.
    pub running_status: Option<channel::Sender<Vec<(String, bool)>>>,
    /// Where stopping a project's session in the background sends whether
    /// it stopped (by project id); `None` until set up.
    pub stop_status: Option<channel::Sender<(String, bool)>>,
    /// Where a project's start or stop commands send how they went; `None`
    /// until set up.
    pub hook_status: Option<channel::Sender<hooks::Done>>,
    /// The projects whose start commands run, by id.
    pub starting: HashSet<String>,
    /// The project to show once its start commands are done.
    pub pending_enter: Option<PendingEnter>,
    /// The projects whose sessions are being stopped, by id: not shown
    /// until they have.
    pub stopping: HashSet<String>,
    /// The projects to forget once their sessions have stopped, by id (see
    /// `Gerdr::forget_project`).
    pub forget_after_stop: HashSet<String>,
    /// Projects to give a new id once stopped: by id, the new one (see
    /// `project_rename`).
    pub id_after_stop: HashMap<String, String>,
    /// The project just made and being shown, by id: the switch's notice
    /// says it was made.
    pub just_created: Option<String>,
    /// A project to show although another gerdr has it open, by id (see
    /// `Gerdr::attach_project`).
    pub attach_once: Option<String>,
    /// The project shown before the one shown now, by id, for
    /// `Action::LastProject`.
    pub previous_project: Option<String>,
}

/// Web panes steered through the browser extension, and page previews.
pub struct Web {
    /// A read of where the web pages are is under way (see `watch_web_pages`).
    pub pending: bool,
    /// The browsers following and steering the web pages (see
    /// `web_control`).
    pub control: web_control::WebControl,
    /// The web tabs showing a markdown file's preview (see `md_preview`).
    pub previews: md_preview::Previews,
}

/// Cursor agent chats read from Cursor's database.
#[derive(Default)]
pub struct CursorWatch {
    /// What gerdr knows of its Cursor's chats (see `cursor_agents`).
    pub reads: cursor_agents::Chats,
    /// Those doing something or with news, shown as cards.
    pub chats: Vec<cursor_agents::CursorChat>,
    /// Those idle that the database has, some shown as history (see
    /// `cursor_agents::History`).
    pub idle: Vec<cursor_agents::CursorChat>,
    /// Asks for Cursor's database to be read now, keeping the chats given.
    pub poke: Option<std::sync::mpsc::Sender<Vec<String>>>,
    /// Where Cursor's hooks reach this gerdr.
    pub socket: Option<PathBuf>,
    /// The Cursor chat last shown from its card, whose card is filled
    /// while Cursor has the keyboard.
    pub opened: Option<String>,
}

/// Desktop notifications, focus asks, and the clipboard.
pub struct Notices {
    /// gerdr's connection to the host's notification daemon; `None` without
    /// a session bus.
    pub notifier: Option<notify::Notifier>,
    /// The host's xdg-activation for gerdr's window, made at the first
    /// notification click; `None` until then or on a host without it.
    pub activation: Option<activate::Activation>,
    /// gerdr's own window has the host's keyboard focus.
    pub host_focused: bool,
    /// Brings the clipboard's text read for a paste back (see `clipboard`).
    pub paste: Option<channel::Sender<(input::Overlay, String)>>,
    /// The text copied from one of gerdr's own text fields, while gerdr
    /// owns the clipboard with it (see `clipboard`).
    pub copied: Option<String>,
}

/// The herdr session of the open project: all of it goes when the project is left.
#[derive(Default)]
pub struct Session {
    /// gerdr's herdr session, holding the terminals; `None` without herdr.
    pub herdr: Option<herdr::Herdr>,
    /// The coding agents herdr sees in the session, shown in the sidebar.
    pub agents: Vec<herdr::AgentInfo>,
    /// What herdr reports about the tabs of gerdr's workspaces, by herdr tab
    /// id, shown in the tab strip.
    pub tabs: HashMap<String, herdr::TabDetail>,
    /// Since when each blocked agent was last notified of (or seen to be
    /// blocked): a reminder goes after `BLOCKED_REMINDER`.
    pub blocked_since: HashMap<agents::AgentKey, Instant>,
    /// What each agent was doing when last looked at, to tell news by.
    pub agent_statuses: HashMap<agents::AgentKey, herdr::AgentStatus>,
    /// The event loop sources following the project's herdr session, removed
    /// when switching to another project.
    pub sources: Vec<RegistrationToken>,
    /// Asks the thread polling the project's herdr session for a round now.
    pub poke: Option<std::sync::mpsc::Sender<()>>,
    /// Since when each herdr pane's shell has been idle, by pane id: its
    /// command is forgotten only after a while, so panes dying on shutdown
    /// don't count.
    pub idle_since: HashMap<String, Instant>,
    /// Since when the tab layouts differ from what was saved of them.
    pub tabs_changed: Option<Instant>,
    /// The process leading each herdr pane's foreground when its command was
    /// last noted, by pane id: a command is noted once per process, as
    /// some rewrite their command line once running.
    pub pane_leaders: HashMap<String, i32>,
    /// The last command run in each herdr pane, by pane id, to run again
    /// from its bar.
    pub last_commands: HashMap<String, pane_control::Remembered>,
    /// The panes being stopped or restarted, by herdr pane id.
    pub pane_jobs: HashMap<String, pane_control::Job>,
    /// Since when each workspace herdr said closed has been, by herdr id:
    /// forgotten only after a while, so a server going down doesn't count.
    pub closed_workspaces: HashMap<String, Instant>,
    /// Since when each record without a herdr workspace has had no
    /// workspace open, by id (see `keep_records`).
    pub gone_since: HashMap<String, Instant>,
    /// The records of the workspaces closed on purpose, to forget once
    /// they are gone (see `keep_records`).
    pub closing_records: HashSet<String>,
    /// When the shown project's server was restarted and brought its
    /// workspaces back, if it was.
    pub restored_at: Option<Instant>,
    /// The project's workspaces, by herdr id, that the restarted server
    /// hasn't brought back yet: what ran in them runs again once it does.
    pub replay_pending: HashSet<String>,
}
