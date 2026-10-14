//! gerdr's own UI: the workspace sidebar on the left (each workspace with
//! what its tabs show put together, and a card per coding agent herdr sees
//! in it), the tab strip above the apps
//! (each tab with what runs in it, as herdr reports;
//! the light/dark switch at its right end), a bar over each terminal pane
//! (what runs in it, and buttons to restart and stop it), and the key help
//! panel shown while the prefix is armed, the switcher, the project menu
//! under the sidebar header and a notice over the apps; the welcome
//! screen covering it all, and the quit dialog over that. All are drawn on the CPU into memory buffers at physical
//! resolution (crisp at fractional scales) and only redrawn when they change.
//!
//! The look follows Zed's default themes, One Dark and One Light.

mod canvas;
mod consts;
#[cfg(test)]
mod golden;
mod help;
mod menu;
mod notice;
mod painter;
mod pane_bar;
mod project_menu;
mod quit;
mod shapes;
mod sidebar;
mod switcher;
mod tabs;
mod theme;
mod welcome;

use smithay::{
    backend::renderer::{
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
        gles::GlesRenderer,
    },
    utils::{Logical, Point, Rectangle, Size},
};

// One surface per module; the rest of gerdr reaches all of it as
// `chrome::X`. The canvas and the shapes stay inside.
use canvas::*;
pub use consts::*;
pub use help::*;
pub use menu::*;
pub use painter::*;
pub use pane_bar::*;
pub use project_menu::*;
pub use quit::*;
use shapes::*;
pub use sidebar::*;
pub use switcher::*;
pub use tabs::*;
pub use theme::*;
pub use welcome::*;

pub(crate) type Rect = Rectangle<i32, Logical>;

pub struct ChromeView {
    pub workspaces: Vec<WorkspaceView>,
    /// Per workspace, the agents running in it.
    pub agents: Vec<Vec<AgentView>>,
    pub active_workspace: Option<usize>,
    pub tabs: Vec<TabView>,
    pub active_tab: usize,
    /// Set while the terminal menu is open: the item under the pointer.
    pub menu: Option<Option<usize>>,
    /// Set while the switcher is open.
    pub switcher: Option<SwitcherView>,
    /// Set while waiting for the key after the prefix: the bindings to list.
    pub prefix: Option<std::sync::Arc<Help>>,
    /// Set while the welcome screen covers everything.
    pub welcome: Option<WelcomeView>,
    /// Set while the quit dialog is open, over everything.
    pub quit: Option<QuitView>,
    /// The project shown, by name, for the sidebar header.
    pub project: Option<String>,
    /// The pointer is over the sidebar header's project name.
    pub header_hover: bool,
    /// Set while the project menu is open.
    pub project_menu: Option<ProjectMenuView>,
    /// A short note at the top of the app area, such as "Switched to gerdr".
    pub notice: Option<String>,
    /// The bars over the active tab's terminal panes.
    pub pane_bars: Vec<PaneBarView>,
    pub theme: Theme,
    pub sidebar_width: i32,
    pub size: Size<i32, Logical>,
    pub scale: f64,
}

/// Sidebar inputs: workspaces, their agents, the active one, whether the
/// terminal menu is open, the project's name and whether it is lit (under
/// the pointer or its menu open), theme, size, scale.
type SidebarKey = (
    Vec<WorkspaceView>,
    Vec<Vec<AgentView>>,
    Option<usize>,
    bool,
    Option<String>,
    bool,
    Theme,
    Size<i32, Logical>,
    f64,
);

pub struct Chrome {
    painter: Painter,
    sidebar: Cached<SidebarKey>,
    tabs: Cached<(Vec<TabView>, usize, bool, Theme, i32, f64)>,
    help: Cached<(HelpLayout, Theme, f64)>,
    menu: Cached<(Option<usize>, Theme, f64)>,
    switcher: Cached<(SwitcherView, i32, Theme, f64)>,
    welcome: Cached<(WelcomeView, Size<i32, Logical>, Theme, f64)>,
    quit: Cached<(QuitView, Size<i32, Logical>, Theme, f64)>,
    project_menu: Cached<(ProjectMenuView, Size<i32, Logical>, Theme, f64)>,
    /// The notice's text and width.
    notice: Cached<(String, i32, Theme, f64)>,
    /// One per bar over a terminal pane.
    pane_bars: Vec<Cached<(PaneBarView, Theme, f64)>>,
}

impl Chrome {
    /// The byte of `field`'s text nearest output x `x`, as it is drawn on
    /// an output of `scale`: where a click there puts the caret.
    pub fn caret_at(&self, field: &FieldSpot, x: f64, scale: f64) -> usize {
        self.painter
            .caret_at(&field.line, field.text, (x * scale) as f32, scale)
    }

    pub fn new() -> Self {
        Self {
            painter: Painter::new(),
            sidebar: Cached::default(),
            tabs: Cached::default(),
            help: Cached::default(),
            menu: Cached::default(),
            switcher: Cached::default(),
            welcome: Cached::default(),
            quit: Cached::default(),
            project_menu: Cached::default(),
            notice: Cached::default(),
            pane_bars: Vec::new(),
        }
    }

    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        view: ChromeView,
    ) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let scale = view.scale;
        let theme = view.theme;
        let sidebar_w = view.sidebar_width;
        let output: Size<i32, Logical> = (view.size.w.max(1), view.size.h.max(1)).into();
        let painter = &self.painter;

        // Front to back: the quit dialog covers everything, the welcome
        // screen all the rest.
        let mut parts = Vec::new();
        if let Some(quit) = view.quit {
            let panel = quit_layout(output).panel;
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self
                .quit
                .get((quit, output, theme, scale), |(quit, output, ..)| {
                    painter.quit_dialog(quit, *output, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
        if let Some(welcome) = view.welcome {
            let buffer = self
                .welcome
                .get((welcome, output, theme, scale), |(welcome, output, ..)| {
                    painter.welcome(welcome, *output, scale, theme.palette())
                });
            parts.push((buffer, Point::from((0, 0)), output));
            return render_parts(renderer, parts, scale);
        }

        let sidebar_size: Size<i32, Logical> = (sidebar_w, view.size.h.max(1)).into();
        let strip_size: Size<i32, Logical> =
            ((view.size.w - sidebar_w).max(1), TAB_STRIP_HEIGHT).into();
        let menu_open = view.menu.is_some();
        let header_lit = view.header_hover || view.project_menu.is_some();
        let sidebar_key = (
            view.workspaces,
            view.agents,
            view.active_workspace,
            menu_open,
            view.project,
            header_lit,
            theme,
            sidebar_size,
            scale,
        );
        let sidebar = self.sidebar.get(
            sidebar_key,
            |(workspaces, agents, active, menu_open, project, lit, ..)| {
                painter.sidebar(
                    workspaces,
                    agents,
                    *active,
                    *menu_open,
                    (project.as_deref(), *lit),
                    sidebar_size,
                    scale,
                    theme.palette(),
                )
            },
        );
        let strip = self.tabs.get(
            (
                view.tabs,
                view.active_tab,
                view.prefix.is_some(),
                theme,
                strip_size.w,
                scale,
            ),
            |(tabs, active, prefix, ..)| {
                painter.tab_strip(tabs, *active, *prefix, theme, strip_size, scale)
            },
        );

        // Then the project menu, the switcher, the terminal menu and the
        // help panel cover the apps; the project menu the sidebar too.
        if let Some(menu) = view.project_menu {
            let panel = project_menu_layout(&menu, output).panel;
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self
                .project_menu
                .get((menu, output, theme, scale), |(menu, output, ..)| {
                    painter.project_menu(menu, *output, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
        if let Some(switcher) = view.switcher {
            let area = Rectangle::new(
                (sidebar_w, TAB_STRIP_HEIGHT).into(),
                (strip_size.w, (view.size.h - TAB_STRIP_HEIGHT).max(1)).into(),
            );
            let (panel, _, _) = switcher_layout(switcher.rows.len(), area);
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let buffer = self.switcher.get(
                (switcher, panel.size.w, theme, scale),
                |(switcher, w, ..)| painter.switcher(switcher, *w, scale, theme.palette()),
            );
            parts.push((buffer, loc, size));
        }
        if let Some(hover) = view.menu {
            let (panel, _) = terminal_menu(sidebar_w);
            let loc = panel.loc - Point::from((SHADOW, SHADOW));
            let size = (panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW).into();
            let menu = self.menu.get((hover, theme, scale), |(hover, _, _)| {
                painter.menu(*hover, scale, theme.palette())
            });
            parts.push((menu, loc, size));
        }
        if let Some(help) = view.prefix.as_deref() {
            // Centred at the bottom of the app area.
            let (area_w, area_h) = (view.size.w - sidebar_w, view.size.h - TAB_STRIP_HEIGHT);
            let layout = painter.help_layout(help, area_w - 2 * HELP_MARGIN, scale);
            // The buffer reaches past the panel by the shadow on every side.
            let loc = Point::from((
                sidebar_w + (area_w - layout.size.w) / 2 - SHADOW,
                (TAB_STRIP_HEIGHT + area_h - layout.size.h - HELP_MARGIN).max(TAB_STRIP_HEIGHT)
                    - SHADOW,
            ));
            let size = (layout.size.w + 2 * SHADOW, layout.size.h + 2 * SHADOW).into();
            let panel = self.help.get((layout, theme, scale), |(layout, _, _)| {
                painter.help_panel(help, layout, scale, theme.palette())
            });
            parts.push((panel, loc, size));
        }
        if let Some(notice) = view.notice {
            // Centred at the top of the app area, just below the tab strip.
            let w = painter.notice_width(&notice, strip_size.w - 2 * HELP_MARGIN, scale);
            let loc = Point::from((
                sidebar_w + (strip_size.w - w) / 2 - SHADOW,
                TAB_STRIP_HEIGHT + NOTICE_TOP - SHADOW,
            ));
            let size = (w + 2 * SHADOW, NOTICE_HEIGHT + 2 * SHADOW).into();
            let buffer = self
                .notice
                .get((notice, w, theme, scale), |(notice, w, ..)| {
                    painter.notice(notice, *w, scale, theme.palette())
                });
            parts.push((buffer, loc, size));
        }
        // The pane bars sit on the apps, under everything else.
        self.pane_bars
            .resize_with(view.pane_bars.len(), Cached::default);
        for (cached, bar) in self.pane_bars.iter_mut().zip(view.pane_bars) {
            let (loc, size) = (bar.rect.loc, bar.rect.size);
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let buffer = cached.get((bar, theme, scale), |(bar, ..)| {
                painter.pane_bar(bar, scale, theme.palette())
            });
            parts.push((buffer, loc, size));
        }
        parts.push((sidebar, Point::from((0, 0)), sidebar_size));
        parts.push((strip, Point::from((sidebar_w, 0)), strip_size));
        render_parts(renderer, parts, scale)
    }
}

/// A drawn buffer, where it goes on the output and the logical size it covers.
type Part<'a> = (
    &'a MemoryRenderBuffer,
    Point<i32, Logical>,
    Size<i32, Logical>,
);

/// Render elements for buffers of physical pixels, each covering a logical
/// size at a logical place on the output.
fn render_parts(
    renderer: &mut GlesRenderer,
    parts: Vec<Part<'_>>,
    scale: f64,
) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
    parts
        .into_iter()
        .filter_map(|(buffer, loc, size)| {
            // The buffer holds physical pixels at buffer scale 1: sample all of it
            // and squeeze it into the logical size.
            let src = Rectangle::from_size(physical(size, scale).to_f64());
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc.to_f64().to_physical(scale),
                buffer,
                None,
                Some(src),
                Some(size),
                Kind::Unspecified,
            )
            .ok()
        })
        .collect()
}

/// A buffer plus the inputs it was drawn from.
struct Cached<K> {
    key: Option<K>,
    buffer: Option<MemoryRenderBuffer>,
}

impl<K> Default for Cached<K> {
    fn default() -> Self {
        Self {
            key: None,
            buffer: None,
        }
    }
}

impl<K: PartialEq> Cached<K> {
    fn get(&mut self, key: K, draw: impl FnOnce(&K) -> MemoryRenderBuffer) -> &MemoryRenderBuffer {
        if self.key.as_ref() != Some(&key) || self.buffer.is_none() {
            self.buffer = Some(draw(&key));
            self.key = Some(key);
        }
        self.buffer.as_ref().unwrap()
    }
}

#[cfg(test)]
pub(super) fn inside(outer: Rect, inner: Rect) -> bool {
    inner.loc.x >= outer.loc.x
        && inner.loc.y >= outer.loc.y
        && inner.loc.x + inner.size.w <= outer.loc.x + outer.size.w
        && inner.loc.y + inner.size.h <= outer.loc.y + outer.size.h
}
