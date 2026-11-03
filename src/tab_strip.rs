//! Input to the tab strip: presses on the tabs, the "+" button, the
//! overflow chip, the inbox and problems buttons and the theme switch; the pointer moving over them
//! (hover and tooltips); and their right-click menus (see `chrome::tabs`
//! for what the strip shows).

use smithay::utils::{Logical, Point, Rectangle, Size};

use std::time::Instant;

use crate::{
    Action, Gerdr, Reorderable, Tab,
    chrome::{self, TabHit, TabLayout, TabStripView, TabView, ThemeMode},
    input::{BTN_LEFT, BTN_MIDDLE},
    layout::Dir,
    menu::{MenuAction, MenuItem, MenuSpec},
    nav_history::Visit,
    rename::RenameTarget,
    tooltip::Tooltip,
};

/// The mode the theme switch goes to from `mode`: the system's choice,
/// then dark, then light, round again.
pub fn next_mode(mode: ThemeMode) -> ThemeMode {
    match mode {
        ThemeMode::System => ThemeMode::Dark,
        ThemeMode::Dark => ThemeMode::Light,
        ThemeMode::Light => ThemeMode::System,
    }
}

/// What the inbox button's tooltip says while it counts `count` items:
/// items of every kind, not only the agents the sidebar's "Needs you"
/// list shows, so it says so.
pub fn inbox_tooltip(count: usize) -> String {
    match count {
        0 => "Inbox".to_owned(),
        1 => "Inbox · 1 item: agents, repos, pull requests, tickets".to_owned(),
        n => format!("Inbox · {n} items: agents, repos, pull requests, tickets"),
    }
}

/// What the problems button's tooltip says while it counts `count`.
pub fn problems_tooltip(count: usize) -> String {
    match count {
        1 => "1 problem · click for what and where to fix it".to_owned(),
        n => format!("{n} problems · click for what and where to fix them"),
    }
}

/// How many characters of a problem's text a line of its menu holds.
const PROBLEM_LINE: usize = 30;
/// How many lines of its menu a problem takes at most: enough for a
/// problem's text and the fix it names (a longer one ends in "…", and the
/// log holds all of it).
const PROBLEM_LINES: usize = 8;

/// `text` in lines of at most `width` characters, broken between words
/// (a longer word is cut), at most `most` of them: the last one ends in
/// "…" when the text goes on.
pub fn wrap_lines(text: &str, width: usize, most: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = line.chars().count();
            let room = if used == 0 {
                width
            } else {
                width.saturating_sub(used + 1)
            };
            if word.len() <= room {
                if used > 0 {
                    line.push(' ');
                }
                line.extend(word.iter());
                break;
            }
            if used == 0 {
                // Longer than a whole line: as much of it as fits.
                let rest = word.split_off(width.max(1));
                lines.push(word.into_iter().collect());
                word = rest;
            } else {
                lines.push(std::mem::take(&mut line));
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > most {
        lines.truncate(most);
        if let Some(last) = lines.last_mut() {
            let kept: String = last.chars().take(width.saturating_sub(1)).collect();
            *last = format!("{kept}\u{2026}");
        }
    }
    lines
}

/// The problems button's menu: each of `problems`, in lines that can't be
/// picked, then where to fix them, the settings file and the log (named
/// by the file names `settings` and `log`, at the right).
pub fn problems_menu_items(problems: &[&str], settings: &str, log: &str) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = problems
        .iter()
        .flat_map(|text| wrap_lines(text, PROBLEM_LINE, PROBLEM_LINES))
        .map(|line| MenuItem::new(&line, MenuAction::OpenLog).enabled(false))
        .collect();
    items.push(MenuItem::new("Open settings", MenuAction::OpenSettings).hint(settings));
    items.push(MenuItem::new("Open log", MenuAction::OpenLog).hint(log));
    items
}

/// What the theme switch's tooltip says of `mode`, the look it gives
/// (`dark`) and where a click goes.
pub fn mode_tooltip(mode: ThemeMode, dark: bool) -> String {
    let now = match mode {
        ThemeMode::System if dark => "System theme (dark now)",
        ThemeMode::System => "System theme (light now)",
        ThemeMode::Dark => "Dark theme",
        ThemeMode::Light => "Light theme",
    };
    let next = match next_mode(mode) {
        ThemeMode::System => "system",
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    };
    format!("{now} · click for {next}")
}

/// The theme items of a menu: one per mode, the one in use marked.
pub fn theme_items(mode: ThemeMode) -> Vec<MenuItem> {
    [
        (ThemeMode::System, "System theme"),
        (ThemeMode::Dark, "Dark theme"),
        (ThemeMode::Light, "Light theme"),
    ]
    .into_iter()
    .map(|(m, label)| {
        let item = MenuItem::new(label, MenuAction::Theme(m));
        if m == mode {
            item.hint("current")
        } else {
            item
        }
    })
    .collect()
}

/// The key hints a menu shows: each is the keys after the prefix, or
/// `None` when the action has no key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyHints {
    pub new_tab: Option<String>,
    pub close_tab: Option<String>,
    pub split_right: Option<String>,
    pub split_down: Option<String>,
    pub run_tab: Option<String>,
    pub zoom_pane: Option<String>,
}

/// The menu of tab `t` in workspace `w`. Splitting works on the tab
/// shown, where `pane` is its focused pane; other tabs show the entries
/// disabled. A tab whose pane `zoomed` fills it can be unzoomed.
pub fn tab_menu_items(
    w: usize,
    t: usize,
    pane: Option<u32>,
    zoomed: Option<u32>,
    hints: &KeyHints,
) -> Vec<MenuItem> {
    let hinted = |item: MenuItem, hint: &Option<String>| match hint {
        Some(hint) => item.hint(hint),
        None => item,
    };
    let unzoom = zoomed.map(|id| {
        hinted(
            MenuItem::new("Unzoom pane", MenuAction::ZoomPane(id)),
            &hints.zoom_pane,
        )
    });
    let mut items = vec![
        hinted(
            MenuItem::new("New tab", MenuAction::NewTab(w)),
            &hints.new_tab,
        ),
        MenuItem::new("Rename…", MenuAction::RenameTab(w, t)),
        hinted(
            MenuItem::new(
                "Split right",
                MenuAction::SplitPane(pane.unwrap_or(0), Dir::Right),
            )
            .enabled(pane.is_some()),
            &hints.split_right,
        ),
        hinted(
            MenuItem::new(
                "Split down",
                MenuAction::SplitPane(pane.unwrap_or(0), Dir::Down),
            )
            .enabled(pane.is_some()),
            &hints.split_down,
        ),
    ];
    items.extend(unzoom);
    items.push(hinted(
        MenuItem::new("Close tab", MenuAction::CloseTab(w, t)),
        &hints.close_tab,
    ));
    items
}

/// The menu under the "+" of workspace `w`: a new terminal tab, the run
/// prompt for an app in a new tab (by its key until a menu action opens
/// it), and the theme modes.
pub fn plus_menu_items(w: usize, mode: ThemeMode, hints: &KeyHints) -> Vec<MenuItem> {
    let mut items = vec![match &hints.new_tab {
        Some(hint) => MenuItem::new("New tab", MenuAction::NewTab(w)).hint(hint),
        None => MenuItem::new("New tab", MenuAction::NewTab(w)),
    }];
    let run = MenuItem::new(
        "Run an app in a new tab…",
        MenuAction::RunApp(crate::launch::Place::Tab),
    );
    items.push(match &hints.run_tab {
        Some(hint) => run.hint(hint),
        None => run,
    });
    items.extend(theme_items(mode));
    items
}

/// What the overflow chip's tooltip says of the `blocked` workspaces not
/// shown, by name, among `others` of them, and what a click does.
pub fn overflow_tooltip(others: usize, blocked: &[&str]) -> String {
    let names = blocked.join(", ");
    let what = match (others, blocked.len()) {
        (1, _) => format!("The other workspace needs input: {names}"),
        (n, 1) => format!("1 of {n} other workspaces needs input: {names}"),
        (n, b) => format!("{b} of {n} other workspaces need input: {names}"),
    };
    let click = if blocked.len() > 1 {
        "click for the next, right-click for the list"
    } else {
        "click to go there, right-click for the list"
    };
    format!("{what} · {click}")
}

/// What the "more tabs" chip's tooltip says of the tabs it holds, by
/// index and title.
pub fn more_tooltip(hidden: &[(usize, &str)]) -> String {
    let names: Vec<String> = hidden
        .iter()
        .map(|(i, title)| format!("{} {title}", i + 1))
        .collect();
    let n = hidden.len();
    let tabs = if n == 1 { "tab" } else { "tabs" };
    format!(
        "{n} more {tabs}: {} · click for the next, right-click for the list",
        names.join(", ")
    )
}

/// The menu of the "more tabs" chip: a row showing each of the `hidden`
/// tabs (by index and title) of workspace `w`.
pub fn hidden_tab_items(w: usize, hidden: &[(usize, &str)]) -> Vec<MenuItem> {
    hidden
        .iter()
        .map(|(t, title)| {
            MenuItem::new(&format!("{} {title}", t + 1), MenuAction::SwitchTab(w, *t))
        })
        .collect()
}

/// How many tabs the back or forward button's menu lists.
const VISITED_ITEMS: usize = 15;

/// A tab in the back or forward button's menu: its workspace, its
/// number and its title (after its project's name, for another project's).
pub fn visited_label(workspace: &str, t: usize, title: &str) -> String {
    format!("{workspace} › {} {title}", t + 1)
}

/// The back (`back`) or forward button's tooltip, while it can go (`can`)
/// or not.
pub fn visited_tooltip(back: bool, can: bool) -> String {
    match (back, can) {
        (true, true) => "Back to the tab shown before · right click for more".into(),
        (false, true) => "Forward to the tab gone back from · right click for more".into(),
        (true, false) => "Back · nothing to go back to".into(),
        (false, false) => "Forward · nothing to go forward to".into(),
    }
}

/// Which of `among` (in order) comes after `from`, round to the first:
/// the next blocked workspace, or the next tab that doesn't fit.
pub fn next_after(among: &[usize], from: usize) -> Option<usize> {
    among
        .iter()
        .copied()
        .find(|&i| i > from)
        .or(among.first().copied())
}

/// The key going to tab `t`, as the help lists it: the prefix and the
/// tab's number, for the first nine.
pub fn tab_key(help: &crate::chrome::Help, t: usize) -> Option<String> {
    if t >= 9 {
        return None;
    }
    if let Some(key) = help.key_of(Action::GotoTab(t), 0) {
        return Some(key);
    }
    // The help folds "1" to "9" into one range on one line, standing for
    // them all.
    let folded = help.rows.iter().any(|(keys, _)| keys.contains('–'));
    folded.then(|| format!("{} {}", help.prefix, t + 1))
}

impl Gerdr {
    /// Where the strip sits in the window.
    pub(crate) fn tab_strip_rect(&self) -> Rectangle<i32, Logical> {
        let size: Size<i32, Logical> = (
            (self.output_size().w - self.sidebar_width).max(1),
            self.tab_strip_height(),
        )
            .into();
        Rectangle::new((self.sidebar_width, 0).into(), size)
    }

    /// Everything the strip is drawn from: the tabs of the active workspace
    /// (a tab named by hand keeps its name), the theme mode, the prefix
    /// badge, the rename in progress, what the pointer is over while nothing
    /// covers the strip, the other workspaces with a blocked agent, and the
    /// inbox's count.
    pub(crate) fn tab_strip_view(&self) -> TabStripView {
        let views = self.workspace_views();
        let tabs: Vec<TabView> = self
            .workspace()
            .map(|ws| self.tab_views(ws))
            .unwrap_or_default();
        let others = views
            .iter()
            .enumerate()
            .filter(|(w, _)| self.workspace().is_none() || *w != self.active_workspace);
        let elsewhere = others.fold((0, 0), |(n, blocked), (_, (ws, _))| {
            let is_blocked = ws.status == Some(crate::herdr::AgentStatus::Blocked);
            (n + 1, blocked + usize::from(is_blocked))
        });
        let workspaces: Vec<_> = views.into_iter().map(|(ws, _)| ws).collect();
        let inbox = self.inbox_count(&workspaces, &self.agent_views());
        let renaming = self.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Tab(w, t) if w == self.active_workspace => Some((t, r.field.line())),
            _ => None,
        });
        let mut view = TabStripView {
            tabs,
            active: self.workspace().map_or(0, |ws| ws.active_tab),
            prefix: self.prefix_armed || self.help_pinned,
            mode: self.theme_mode,
            hover: None,
            hover_alpha: 0,
            renaming,
            elsewhere,
            watch: self.focused_pull_request().map(|(_, _, watched)| watched),
            inbox: Some(inbox),
            problems: Some(crate::problems::list().len()).filter(|n| *n > 0),
            plus: self.workspace().is_some(),
            nav: self.can_go(),
        };
        view.hover = self.tab_hover.over();
        view.hover_alpha = self.tab_hover.alpha_over(Instant::now());
        view
    }

    /// Whether something covers the strip, so nothing in it lights up.
    fn tab_strip_covered(&self) -> bool {
        self.quit.is_some()
            || self.restart.is_some()
            || self.checkout.is_some()
            || self.confirm.is_some()
            || self.welcome.is_some()
            || self.project_menu.is_some()
            || self.switcher.is_some()
            || self.context_menu.is_some()
            || self.drag.is_some()
            || self.reorder.is_some_and(|r| r.moving)
    }

    /// Move the strip's hover on, once a frame: what the pointer is over
    /// fades in, what it left fades out and is forgotten once gone. The
    /// strip's view as it then is, for the frame to draw.
    pub(crate) fn tab_hover_tick(&mut self, now: Instant) -> TabStripView {
        let mut view = self.tab_strip_view();
        let hit = if self.tab_strip_covered() {
            None
        } else {
            self.tab_strip_hit_at(&view, self.pointer)
        };
        self.tab_hover.track(hit, now);
        view.hover = self.tab_hover.over();
        view.hover_alpha = self.tab_hover.alpha_over(now);
        view
    }

    /// The tab a drag at window x `x` would put the held tab at: the
    /// strip's layout as it shows now.
    pub(crate) fn tab_slot_at(&self, x: f64) -> usize {
        let view = self.tab_strip_view();
        chrome::tab_slot(&self.tab_strip_layout(&view), x - self.sidebar_width as f64)
    }

    /// What `pos` (window coordinates) is over in the strip drawn from
    /// `view`, if it is over the strip at all.
    pub(crate) fn tab_strip_hit_at(
        &self,
        view: &TabStripView,
        pos: Point<f64, Logical>,
    ) -> Option<TabHit> {
        let strip = self.tab_strip_rect();
        if !strip.to_f64().contains(pos) {
            return None;
        }
        let at: Point<i32, Logical> =
            (pos.x as i32 - strip.loc.x, pos.y as i32 - strip.loc.y).into();
        chrome::tab_strip_hit(view, strip.size, at)
    }

    /// The strip's layout, for presses: from what it shows now.
    pub(crate) fn tab_strip_layout(&self, view: &TabStripView) -> TabLayout {
        view.layout(self.tab_strip_rect().size)
    }

    /// The keys the strip's menus and tooltips name, from the help.
    fn tab_key_hints(&self) -> KeyHints {
        let help = self.keymap.help();
        let key = |action: Action| help.key_of(action, 0);
        KeyHints {
            new_tab: key(Action::NewTab),
            close_tab: key(Action::CloseTab),
            split_right: key(Action::Split(Dir::Right)),
            split_down: key(Action::Split(Dir::Down)),
            run_tab: key(Action::RunTab),
            zoom_pane: key(Action::ZoomPane),
        }
    }

    /// The back (`back`) or forward button's menu, at `anchor` (window
    /// coordinates): the tabs going that way would show, the next first,
    /// each by its workspace and title.
    fn visited_menu(&self, anchor: Rectangle<i32, Logical>, back: bool) -> Option<MenuSpec> {
        let can = |v: &Visit| self.can_visit(v);
        let visits = if back {
            self.nav.backs(can)
        } else {
            self.nav.forwards(can)
        };
        let items: Vec<MenuItem> = visits
            .into_iter()
            .take(VISITED_ITEMS)
            .filter_map(|visit| {
                let label = self.visit_label(&visit)?;
                Some(MenuItem::new(&label, MenuAction::GoToVisited(visit, back)))
            })
            .collect();
        (!items.is_empty()).then_some(MenuSpec { anchor, items })
    }

    /// The problems button's menu, at `anchor` (window coordinates): the
    /// problems kept now, and where to fix them.
    fn problems_menu(&self, anchor: Rectangle<i32, Logical>) -> MenuSpec {
        let problems = crate::problems::list();
        let texts: Vec<&str> = problems.iter().map(|p| p.text.as_str()).collect();
        let name = |path: std::path::PathBuf| {
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        };
        let (settings, log) = (name(crate::config::config_file()), name(crate::log_path()));
        MenuSpec {
            anchor,
            items: problems_menu_items(&texts, &settings, &log),
        }
    }

    /// The names of the workspaces not shown whose agent is blocked.
    fn blocked_elsewhere(&self) -> Vec<(usize, String)> {
        self.workspace_views()
            .into_iter()
            .enumerate()
            .filter(|(w, (ws, _))| {
                *w != self.active_workspace && ws.status == Some(crate::herdr::AgentStatus::Blocked)
            })
            .map(|(w, (ws, _))| (w, ws.name))
            .collect()
    }

    /// The blocked agents of the workspaces not shown, as menu items
    /// showing each.
    fn blocked_agent_items(&self) -> Vec<MenuItem> {
        let names = self.blocked_elsewhere();
        let agents = self.agent_views();
        names
            .iter()
            .flat_map(|(w, name)| {
                agents
                    .get(*w)
                    .into_iter()
                    .flatten()
                    .filter(|agent| agent.status == crate::herdr::AgentStatus::Blocked)
                    .map(move |agent| {
                        let who = agent.name.as_deref().unwrap_or(&agent.kind);
                        MenuItem::new(
                            &format!("{name}: {who}"),
                            MenuAction::FocusAgent(agent.key.clone()),
                        )
                    })
            })
            .collect()
    }

    /// A press at `(x, y)` in the tab strip (strip coordinates). Left
    /// click selects a tab (a second one right after renames it: see
    /// `input`); "×" or middle click closes it. A tab held with the left
    /// button can be dragged to another place. "+" opens a terminal in a
    /// new tab, the "more tabs" chip shows the next tab that doesn't fit,
    /// the overflow chip switches to the next workspace that needs input,
    /// the inbox button opens the inbox, the problems button lists them,
    /// and the switch at the right end cycles the theme mode.
    pub(crate) fn tab_strip_press(&mut self, x: i32, y: i32, button: u32) {
        let view = self.tab_strip_view();
        let size = self.tab_strip_rect().size;
        let hit = chrome::tab_strip_hit(&view, size, (x, y).into());
        match (button, hit) {
            (BTN_LEFT, Some(TabHit::Back)) => self.go_back(),
            (BTN_LEFT, Some(TabHit::Forward)) => self.go_forward(),
            (BTN_LEFT, Some(TabHit::Toggle)) => self.set_theme_mode(next_mode(self.theme_mode)),
            (BTN_LEFT, Some(TabHit::Overflow)) => {
                let blocked: Vec<usize> = self
                    .blocked_elsewhere()
                    .into_iter()
                    .map(|(w, _)| w)
                    .collect();
                if let Some(w) = next_after(&blocked, self.active_workspace) {
                    self.switch_workspace(w);
                }
            }
            (BTN_LEFT, Some(TabHit::More)) => {
                let hidden = self.tab_strip_layout(&view).hidden();
                if let Some(t) = next_after(&hidden, view.active) {
                    self.switch_tab(t);
                }
            }
            (BTN_LEFT, Some(TabHit::Close(idx)))
            | (BTN_MIDDLE, Some(TabHit::Tab(idx) | TabHit::Unzoom(idx)))
            | (BTN_MIDDLE, Some(TabHit::Close(idx))) => self.close_tab(idx),
            (BTN_LEFT, Some(TabHit::Unzoom(idx))) => {
                self.switch_tab(idx);
                if let Some(id) = self.tab().and_then(Tab::zoomed) {
                    self.zoom_pane(id);
                }
            }
            (BTN_LEFT, Some(TabHit::Tab(idx))) => {
                self.switch_tab(idx);
                self.hold(Reorderable::Tab);
            }
            (BTN_LEFT, Some(TabHit::Plus)) => self.new_tab(),
            (BTN_LEFT, Some(TabHit::Watch)) => self.toggle_watch(),
            (BTN_LEFT, Some(TabHit::Inbox)) => self.open_inbox(None),
            (BTN_LEFT, Some(TabHit::Problems)) => {
                let strip = self.tab_strip_rect();
                if let Some(mut anchor) = self.tab_strip_layout(&view).problems {
                    anchor.loc += strip.loc;
                    self.open_menu(self.problems_menu(anchor));
                }
            }
            _ => {}
        }
    }

    /// The pointer moved to `pos` (window coordinates) over the tab strip,
    /// showing `view`, with nothing open over it: the tooltip for what is
    /// under it, if it has one. (The hover itself is read off the pointer
    /// when the strip is drawn.)
    pub(crate) fn tab_strip_motion(
        &mut self,
        view: &TabStripView,
        pos: Point<f64, Logical>,
    ) -> Option<Tooltip> {
        let hit = self.tab_strip_hit_at(view, pos)?;
        let layout = self.tab_strip_layout(view);
        let strip = self.tab_strip_rect();
        let anchor = |mut rect: Rectangle<i32, Logical>| {
            rect.loc += strip.loc;
            rect
        };
        let hints = self.tab_key_hints();
        let keyed = |tip: Tooltip, key: &Option<String>| match key {
            Some(key) => tip.key(key),
            None => tip,
        };
        Some(match hit {
            TabHit::Back => keyed(
                Tooltip::new(&visited_tooltip(true, view.nav.0), anchor(layout.back)),
                &self.keymap.help().key_of(Action::GoBack, 0),
            ),
            TabHit::Forward => keyed(
                Tooltip::new(&visited_tooltip(false, view.nav.1), anchor(layout.forward)),
                &self.keymap.help().key_of(Action::GoForward, 0),
            ),
            TabHit::Tab(i) => keyed(
                Tooltip::new(&view.tabs.get(i)?.tooltip(), anchor(layout.tabs[i])),
                &tab_key(&self.keymap.help(), i),
            ),
            TabHit::More => {
                let hidden: Vec<(usize, &str)> = layout
                    .hidden()
                    .into_iter()
                    .filter_map(|t| Some((t, view.tabs.get(t)?.title.as_str())))
                    .collect();
                Tooltip::new(&more_tooltip(&hidden), anchor(layout.more?))
            }
            TabHit::Close(i) => keyed(
                Tooltip::new("Close tab", anchor(layout.closes[i])),
                &hints.close_tab,
            ),
            TabHit::Unzoom(i) => keyed(
                Tooltip::new(
                    "Unzoom pane",
                    anchor(chrome::tab_zoom_rect(view, &layout, i, strip.size.h)?),
                ),
                &hints.zoom_pane,
            ),
            TabHit::Plus => keyed(
                Tooltip::new("New tab · right click to run an app", anchor(layout.plus)),
                &hints.new_tab,
            ),
            TabHit::Overflow => {
                let blocked = self.blocked_elsewhere();
                let names: Vec<&str> = blocked.iter().map(|(_, name)| name.as_str()).collect();
                Tooltip::new(
                    &overflow_tooltip(view.elsewhere.0, &names),
                    anchor(layout.overflow?),
                )
            }
            TabHit::Prefix => Tooltip::new(
                "Waiting for the key after the prefix · Esc cancels",
                anchor(layout.prefix?),
            ),
            TabHit::Toggle => Tooltip::new(
                &mode_tooltip(view.mode, self.theme() == chrome::Theme::Dark),
                anchor(layout.toggle),
            ),
            TabHit::Watch => {
                let (_, n, watched) = self.focused_pull_request()?;
                let text = if watched {
                    format!("Stop watching the CI of #{n}")
                } else {
                    format!("Watch the CI of #{n}")
                };
                Tooltip::new(&text, anchor(layout.watch?))
            }
            TabHit::Problems => {
                Tooltip::new(&problems_tooltip(view.problems?), anchor(layout.problems?))
            }
            TabHit::Inbox => {
                let tip = Tooltip::new(&inbox_tooltip(view.inbox?), anchor(layout.inbox?));
                let key = self.keymap.help().key_of(Action::Inbox, 0);
                keyed(tip, &key)
            }
        })
    }

    /// The menu a right press at `pos` (window coordinates) in the tab
    /// strip opens, if any: a tab's, the "+" button's, the theme switch's,
    /// the "more tabs" chip's list of the tabs it holds, or the problems
    /// button's (as a left press opens it).
    pub(crate) fn tab_strip_menu(&self, pos: Point<f64, Logical>) -> Option<MenuSpec> {
        let view = self.tab_strip_view();
        let hit = self.tab_strip_hit_at(&view, pos)?;
        let layout = self.tab_strip_layout(&view);
        let strip = self.tab_strip_rect();
        let anchor = |mut rect: Rectangle<i32, Logical>| {
            rect.loc += strip.loc;
            rect
        };
        let hints = self.tab_key_hints();
        let w = self.active_workspace;
        match hit {
            TabHit::Back => self.visited_menu(anchor(layout.back), true),
            TabHit::Forward => self.visited_menu(anchor(layout.forward), false),
            TabHit::Tab(t) | TabHit::Close(t) | TabHit::Unzoom(t) => {
                let pane = (t == view.active)
                    .then(|| self.tab()?.focused_id())
                    .flatten();
                let tab = self.workspace()?.tabs.get(t)?;
                let mut items = tab_menu_items(w, t, pane, tab.zoomed(), &hints);
                // A feature's panel tab has no panes, no name of its own,
                // and doesn't close: only a new tab is left to ask for.
                if tab.is_feature() {
                    items.truncate(1);
                }
                // A file's tab has its page only, and closes.
                if tab.review().is_some() {
                    let close = items.pop();
                    items.truncate(1);
                    items.extend(close);
                }
                Some(MenuSpec {
                    anchor: anchor(layout.tabs[t]),
                    items,
                })
            }
            TabHit::Plus => Some(MenuSpec {
                anchor: anchor(layout.plus),
                items: plus_menu_items(w, view.mode, &hints),
            }),
            TabHit::Toggle => Some(MenuSpec {
                anchor: anchor(layout.toggle),
                items: theme_items(view.mode),
            }),
            TabHit::Overflow => Some(MenuSpec {
                anchor: anchor(layout.overflow?),
                items: self.blocked_agent_items(),
            })
            .filter(|spec| !spec.items.is_empty()),
            TabHit::More => {
                let hidden: Vec<(usize, &str)> = layout
                    .hidden()
                    .into_iter()
                    .filter_map(|t| Some((t, view.tabs.get(t)?.title.as_str())))
                    .collect();
                Some(MenuSpec {
                    anchor: anchor(layout.more?),
                    items: hidden_tab_items(w, &hidden),
                })
                .filter(|spec| !spec.items.is_empty())
            }
            TabHit::Problems => Some(self.problems_menu(anchor(layout.problems?))),
            TabHit::Prefix | TabHit::Watch | TabHit::Inbox => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyHints, hidden_tab_items, inbox_tooltip, mode_tooltip, more_tooltip, next_after,
        next_mode, overflow_tooltip, plus_menu_items, problems_menu_items, problems_tooltip,
        tab_key, tab_menu_items, theme_items, wrap_lines,
    };
    use crate::{chrome::ThemeMode, layout::Dir, menu::MenuAction};

    #[test]
    fn the_inbox_tooltip_says_how_many_need_the_user() {
        assert_eq!(inbox_tooltip(0), "Inbox");
        assert_eq!(
            inbox_tooltip(1),
            "Inbox · 1 item: agents, repos, pull requests, tickets"
        );
        assert_eq!(
            inbox_tooltip(3),
            "Inbox · 3 items: agents, repos, pull requests, tickets"
        );
    }

    #[test]
    fn the_switch_cycles_system_dark_light() {
        assert_eq!(next_mode(ThemeMode::System), ThemeMode::Dark);
        assert_eq!(next_mode(ThemeMode::Dark), ThemeMode::Light);
        assert_eq!(next_mode(ThemeMode::Light), ThemeMode::System);
        assert_eq!(
            mode_tooltip(ThemeMode::System, true),
            "System theme (dark now) · click for dark"
        );
        assert_eq!(
            mode_tooltip(ThemeMode::Light, false),
            "Light theme · click for system"
        );
    }

    #[test]
    fn a_tab_menu_offers_new_rename_split_close() {
        let hints = KeyHints {
            new_tab: Some("Ctrl+B c".into()),
            close_tab: Some("Ctrl+B x".into()),
            split_right: Some("Ctrl+B v".into()),
            split_down: Some("Ctrl+B -".into()),
            run_tab: Some("Ctrl+B Shift+R".into()),
            zoom_pane: Some("Ctrl+B z".into()),
        };
        let items = tab_menu_items(1, 2, Some(7), None, &hints);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "New tab",
                "Rename…",
                "Split right",
                "Split down",
                "Close tab"
            ]
        );
        assert_eq!(items[0].action, MenuAction::NewTab(1));
        assert_eq!(items[0].hint.as_deref(), Some("Ctrl+B c"));
        assert_eq!(items[1].action, MenuAction::RenameTab(1, 2));
        assert_eq!(items[2].action, MenuAction::SplitPane(7, Dir::Right));
        assert_eq!(items[3].action, MenuAction::SplitPane(7, Dir::Down));
        assert_eq!(items[3].hint.as_deref(), Some("Ctrl+B -"));
        assert_eq!(items[4].action, MenuAction::CloseTab(1, 2));
        assert!(items.iter().all(|i| i.enabled));
        // A tab not shown can't be split from here.
        let other = tab_menu_items(1, 0, None, None, &KeyHints::default());
        assert!(!other[2].enabled && !other[3].enabled);
        assert!(other[0].hint.is_none());
        // A tab with a zoomed pane can be unzoomed, before it is closed.
        let zoomed = tab_menu_items(1, 2, Some(7), Some(5), &hints);
        assert_eq!(zoomed[4].label, "Unzoom pane");
        assert_eq!(zoomed[4].action, MenuAction::ZoomPane(5));
        assert_eq!(zoomed[4].hint.as_deref(), Some("Ctrl+B z"));
        assert_eq!(zoomed[5].action, MenuAction::CloseTab(1, 2));
    }

    #[test]
    fn the_plus_menu_offers_tabs_and_theme_modes() {
        let items = plus_menu_items(0, ThemeMode::Dark, &KeyHints::default());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "New tab",
                "Run an app in a new tab…",
                "System theme",
                "Dark theme",
                "Light theme"
            ]
        );
        assert!(items[1].enabled);
        assert_eq!(
            items[1].action,
            MenuAction::RunApp(crate::launch::Place::Tab)
        );
        assert_eq!(items[3].hint.as_deref(), Some("current"));
        assert_eq!(items[4].action, MenuAction::Theme(ThemeMode::Light));
        let themes = theme_items(ThemeMode::System);
        assert_eq!(themes[0].hint.as_deref(), Some("current"));
        assert!(themes[1].hint.is_none());
    }

    #[test]
    fn the_overflow_tooltip_names_who_needs_input() {
        assert_eq!(
            overflow_tooltip(1, &["api"]),
            "The other workspace needs input: api · click to go there, right-click for the list"
        );
        assert_eq!(
            overflow_tooltip(3, &["api"]),
            "1 of 3 other workspaces needs input: api · click to go there, right-click for the list"
        );
        assert_eq!(
            overflow_tooltip(3, &["api", "web"]),
            "2 of 3 other workspaces need input: api, web · click for the next, right-click for the list"
        );
    }

    #[test]
    fn chips_cycle_through_what_they_hold() {
        assert_eq!(next_after(&[1, 4, 6], 0), Some(1));
        assert_eq!(next_after(&[1, 4, 6], 4), Some(6));
        // Round to the first after the last.
        assert_eq!(next_after(&[1, 4, 6], 6), Some(1));
        assert_eq!(next_after(&[], 2), None);
        assert_eq!(
            more_tooltip(&[(9, "vim"), (10, "logs")]),
            "2 more tabs: 10 vim, 11 logs · click for the next, right-click for the list"
        );
        assert_eq!(
            more_tooltip(&[(3, "zsh")]),
            "1 more tab: 4 zsh · click for the next, right-click for the list"
        );
    }

    #[test]
    fn problem_texts_wrap_between_words_and_end_in_an_ellipsis() {
        assert_eq!(
            wrap_lines("Settings: line 4 has an unknown key", 16, 3),
            ["Settings: line 4", "has an unknown", "key"]
        );
        assert_eq!(
            wrap_lines("one two three four", 9, 2),
            ["one two", "three\u{2026}"]
        );
        // A word longer than a line is cut.
        assert_eq!(
            wrap_lines("~/.config/gerdr/x", 8, 3),
            ["~/.confi", "g/gerdr/", "x"]
        );
        assert!(wrap_lines("  ", 8, 3).is_empty());
    }

    #[test]
    fn a_problem_shows_the_fix_it_names_in_full() {
        let text = "The tickets of AdvancedFreightTrading/optics aren't in their board's order: \
                    the GitHub token lacks the read:project scope. Run gh auth refresh -s \
                    read:project.";
        let items = problems_menu_items(&[text], "config.toml", "gerdr.log");
        let shown: Vec<&str> = items
            .iter()
            .filter(|i| !i.enabled)
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(shown.join(" "), text);
    }

    #[test]
    fn the_problems_menu_lists_them_then_where_to_fix_them() {
        let items = problems_menu_items(&["herdr isn't installed"], "config.toml", "gerdr.log");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            ["herdr isn't installed", "Open settings", "Open log"]
        );
        assert!(!items[0].enabled);
        assert_eq!(items[1].action, MenuAction::OpenSettings);
        assert_eq!(items[1].hint.as_deref(), Some("config.toml"));
        assert_eq!(items[2].action, MenuAction::OpenLog);
        assert!(items[1].enabled && items[2].enabled);
        assert_eq!(
            problems_tooltip(1),
            "1 problem · click for what and where to fix it"
        );
        assert!(problems_tooltip(3).starts_with("3 problems"));
    }

    #[test]
    fn the_more_chip_lists_the_tabs_it_holds() {
        let items = hidden_tab_items(2, &[(9, "vim"), (10, "logs")]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["10 vim", "11 logs"]);
        assert_eq!(items[1].action, MenuAction::SwitchTab(2, 10));
        assert!(hidden_tab_items(0, &[]).is_empty());
    }

    #[test]
    fn the_first_nine_tabs_have_keys() {
        let help = crate::keys::Keymap::default().help();
        assert_eq!(tab_key(&help, 0).as_deref(), Some("Ctrl+B 1"));
        assert_eq!(tab_key(&help, 8).as_deref(), Some("Ctrl+B 9"));
        assert_eq!(tab_key(&help, 9), None);
        // Keys of their own, not folded into a range: each tab's.
        let press = |t| Some(crate::chrome::HelpPress::Action(crate::Action::GotoTab(t)));
        let own = crate::chrome::Help {
            prefix: "Ctrl+A".into(),
            rows: vec![
                ("F1".into(), "Go to tab 1".into()),
                ("F2".into(), "Go to tab 2".into()),
            ],
            presses: vec![vec![press(0)], vec![press(1)]],
        };
        assert_eq!(tab_key(&own, 1).as_deref(), Some("Ctrl+A F2"));
        assert_eq!(tab_key(&own, 2), None);
    }
}
