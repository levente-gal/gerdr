//! Input to the tab strip: presses on the tabs, the "+" button, the
//! overflow chip and the theme switch; the pointer moving over them
//! (hover and tooltips); and their right-click menus (see `chrome::tabs`
//! for what the strip shows).

use smithay::utils::{Logical, Point, Rectangle, Size};

use std::time::Instant;

use crate::{
    Gerdr, Reorderable,
    anim::{self, Fade},
    chrome::{self, TabHit, TabLayout, TabStripView, TabView, ThemeMode},
    input::{BTN_LEFT, BTN_MIDDLE},
    layout::Dir,
    menu::{MenuAction, MenuItem, MenuSpec},
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
}

/// The menu of tab `t` in workspace `w`. Splitting works on the tab
/// shown, where `pane` is its focused pane; other tabs show the entries
/// disabled.
pub fn tab_menu_items(w: usize, t: usize, pane: Option<u32>, hints: &KeyHints) -> Vec<MenuItem> {
    let hinted = |item: MenuItem, hint: &Option<String>| match hint {
        Some(hint) => item.hint(hint),
        None => item,
    };
    vec![
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
        hinted(
            MenuItem::new("Close tab", MenuAction::CloseTab(w, t)),
            &hints.close_tab,
        ),
    ]
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
/// shown, by name, among `others` of them.
pub fn overflow_tooltip(others: usize, blocked: &[&str]) -> String {
    let names = blocked.join(", ");
    match (others, blocked.len()) {
        (1, _) => format!("The other workspace needs input: {names}"),
        (n, 1) => format!("1 of {n} other workspaces needs input: {names}"),
        (n, b) => format!("{b} of {n} other workspaces need input: {names}"),
    }
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
    /// covers the strip, and the other workspaces with a blocked agent.
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
        };
        if let Some((hit, fade)) = self.tab_hover {
            view.hover = Some(hit);
            view.hover_alpha = anim::quantize(fade.value(Instant::now()));
        }
        view
    }

    /// Whether something covers the strip, so nothing in it lights up.
    fn tab_strip_covered(&self) -> bool {
        self.quit.is_some()
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
        self.tab_hover = match (self.tab_hover, hit) {
            (Some((old, fade)), Some(new)) if old == new => Some((
                old,
                if fade.rising {
                    fade
                } else {
                    Fade::from_value(now, anim::FAST, true, fade.value(now))
                },
            )),
            (_, Some(new)) => Some((new, Fade::new(now, anim::FAST, true))),
            (Some((old, fade)), None) if fade.rising => Some((
                old,
                Fade::from_value(now, anim::FAST, false, fade.value(now)),
            )),
            (Some((old, fade)), None) if !fade.done(now) => Some((old, fade)),
            _ => None,
        };
        view.hover = self.tab_hover.map(|(hit, _)| hit);
        view.hover_alpha = self
            .tab_hover
            .map_or(0, |(_, fade)| anim::quantize(fade.value(now)));
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
    fn tab_strip_layout(&self, view: &TabStripView) -> TabLayout {
        let strip = self.tab_strip_rect();
        chrome::tab_layout(view.tabs.len(), strip.size, view.prefix, view.overflow())
    }

    /// The keys the strip's menus and tooltips name, from the help.
    fn tab_key_hints(&self) -> KeyHints {
        let help = self.keymap.help();
        let key = |label: &str, nth: usize| -> Option<String> {
            let (keys, _) = help.rows.iter().find(|(_, l)| l == label)?;
            let key = keys.split(" / ").nth(nth)?;
            Some(format!("{} {key}", help.prefix))
        };
        KeyHints {
            new_tab: key("new tab", 0),
            close_tab: key("close tab", 0),
            split_right: key("split pane right / down", 0).or_else(|| key("split pane right", 0)),
            split_down: key("split pane right / down", 1).or_else(|| key("split pane down", 0)),
            run_tab: key("run an app in a new tab", 0),
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

    /// A press at `(x, y)` in the tab strip (strip coordinates). Left
    /// click selects a tab (a second one right after renames it: see
    /// `input`); "×" or middle click closes it. A tab held with the left
    /// button can be dragged to another place. "+" opens a terminal in a
    /// new tab, the overflow chip switches to a workspace that needs
    /// input, and the switch at the right end cycles the theme mode.
    pub(crate) fn tab_strip_press(&mut self, x: i32, y: i32, button: u32) {
        let view = self.tab_strip_view();
        let hit = self.tab_strip_layout(&view).hit(x, y);
        match (button, hit) {
            (BTN_LEFT, Some(TabHit::Toggle)) => self.set_theme_mode(next_mode(self.theme_mode)),
            (BTN_LEFT, Some(TabHit::Overflow)) => {
                if let Some((w, _)) = self.blocked_elsewhere().into_iter().next() {
                    self.switch_workspace(w);
                }
            }
            (BTN_LEFT, Some(TabHit::Close(idx)))
            | (BTN_MIDDLE, Some(TabHit::Tab(idx)))
            | (BTN_MIDDLE, Some(TabHit::Close(idx))) => self.close_tab(idx),
            (BTN_LEFT, Some(TabHit::Tab(idx))) => {
                self.switch_tab(idx);
                self.hold(Reorderable::Tab);
            }
            (BTN_LEFT, Some(TabHit::Plus)) => self.new_tab(),
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
            TabHit::Tab(i) => Tooltip::new(&view.tabs.get(i)?.tooltip(), anchor(layout.tabs[i])),
            TabHit::Close(i) => keyed(
                Tooltip::new("Close tab", anchor(layout.closes[i])),
                &hints.close_tab,
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
        })
    }

    /// The menu a right press at `pos` (window coordinates) in the tab
    /// strip opens, if any: a tab's, or the "+" button's and the theme
    /// switch's.
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
            TabHit::Tab(t) | TabHit::Close(t) => {
                let pane = (t == view.active)
                    .then(|| self.tab()?.focused_id())
                    .flatten();
                Some(MenuSpec {
                    anchor: anchor(layout.tabs[t]),
                    items: tab_menu_items(w, t, pane, &hints),
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
            TabHit::Overflow | TabHit::Prefix => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyHints, mode_tooltip, next_mode, overflow_tooltip, plus_menu_items, tab_menu_items,
        theme_items,
    };
    use crate::{chrome::ThemeMode, layout::Dir, menu::MenuAction};

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
        };
        let items = tab_menu_items(1, 2, Some(7), &hints);
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
        let other = tab_menu_items(1, 0, None, &KeyHints::default());
        assert!(!other[2].enabled && !other[3].enabled);
        assert!(other[0].hint.is_none());
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
            "The other workspace needs input: api"
        );
        assert_eq!(
            overflow_tooltip(3, &["api"]),
            "1 of 3 other workspaces needs input: api"
        );
        assert_eq!(
            overflow_tooltip(3, &["api", "web"]),
            "2 of 3 other workspaces need input: api, web"
        );
    }
}
