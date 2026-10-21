//! gerdr's own UI: the workspace sidebar on the left (each workspace with
//! what its tabs show put together, and a card per coding agent herdr sees
//! in it), the tab strip above the apps
//! (each tab with what runs in it, as herdr reports;
//! the light/dark switch at its right end), a bar over each terminal pane
//! (what runs in it, and buttons to restart and stop it), a placeholder
//! over each pane whose window is still to come, and the key help
//! panel shown while the prefix is armed, the switcher, the project menu
//! under the sidebar header, the context menus, the tooltip and the
//! toasts over the apps; the welcome screen covering it all, and the quit
//! dialog over that. All are drawn on the CPU into memory buffers at
//! physical resolution (crisp at fractional scales) and only redrawn when
//! they change. The popups fade in and out (see `crate::anim`): only a
//! part whose opacity moves is handed to the renderer anew.
//!
//! The look follows Zed's default themes, One Dark and One Light.

mod canvas;
mod consts;
mod drop;
#[cfg(test)]
mod golden;
mod help;
mod menu;
mod painter;
mod pane_bar;
mod placeholder;
mod project_menu;
mod quit;
mod repos;
mod shapes;
mod sidebar;
mod switcher;
mod tabs;
mod theme;
mod toast;
mod tooltip;
mod welcome;
mod worktrees;

use std::time::Instant;

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

use crate::anim::{FAST, Fade, SLOW, phase};

// One surface per module; the rest of gerdr reaches all of it as
// `chrome::X`. The canvas and the shapes stay inside.
use canvas::*;
pub use consts::*;
pub use drop::*;
pub use help::*;
pub use menu::*;
pub use painter::*;
pub use pane_bar::*;
pub use placeholder::*;
pub use project_menu::*;
pub use quit::*;
pub use repos::*;
use shapes::*;
pub use sidebar::*;
pub use switcher::*;
pub use tabs::*;
pub use theme::*;
pub use toast::*;
pub use tooltip::*;
pub use welcome::*;
pub use worktrees::*;

pub(crate) type Rect = Rectangle<i32, Logical>;

/// How far a popup rises as it fades in, in logical pixels.
const SLIDE: i32 = 4;
/// The environment variable that outlines every hit rect in red.
pub const DEBUG_HITS: &str = "GERDR_DEBUG_HITS";

/// What a placeholder's drawing depends on: it, its button under the
/// pointer, the theme and the scale.
type PlaceholderKey = (PlaceholderView, Option<PlaceholderButton>, Theme, f64);

pub struct ChromeView {
    /// When the view was taken, for what moves with time.
    pub now: Instant,
    /// Where the pointer is (window coordinates): the toasts' buttons and
    /// the placeholders' Retry and Close light up under it.
    pub pointer: Point<f64, Logical>,
    /// The help panel is pinned open, rather than up for the prefix: it
    /// then carries a title and an Esc hint.
    pub help_pinned: bool,
    /// The sidebar: its workspaces, their agents and what moves in it.
    pub sidebar: SidebarView,
    /// Where the blocked agents' pulse stands, 0 to 1 (see `anim::pulse`).
    pub pulse: f32,
    /// The tab strip of the active workspace.
    pub tab_strip: TabStripView,
    /// The focused pane's client rect in the active tab, whatever it shows.
    pub focused_pane: Option<Rect>,
    /// Where a pane held by its bar would land if dropped now: the part of
    /// the pane, or the tab, under the pointer.
    pub pane_drop: Option<Rect>,
    /// The panes of the active tab whose window is still to come.
    pub placeholders: Vec<PlaceholderView>,
    /// Set while a context menu is open.
    pub context_menu: Option<MenuView>,
    /// Set while a tooltip shows.
    pub tooltip: Option<TooltipView>,
    /// Set while the switcher is open.
    pub switcher: Option<SwitcherView>,
    /// Set while waiting for the key after the prefix, or while the help
    /// is pinned: the bindings to list.
    pub prefix: Option<std::sync::Arc<Help>>,
    /// Set while the welcome screen covers everything.
    pub welcome: Option<WelcomeView>,
    /// Set while the quit dialog is open, over everything.
    pub quit: Option<QuitView>,
    /// Set while the project menu is open.
    pub project_menu: Option<ProjectMenuView>,
    /// Set while the worktrees popup is open.
    pub worktrees: Option<WorktreesView>,
    /// Set while the repositories panel is open.
    pub repos: Option<ReposView>,
    /// The toasts showing, oldest first, such as "Switched to gerdr".
    pub toasts: Vec<ToastView>,
    /// The bars over the active tab's terminal panes.
    pub pane_bars: Vec<PaneBarView>,
    pub theme: Theme,
    pub density: Density,
    pub sidebar_width: i32,
    pub size: Size<i32, Logical>,
    pub scale: f64,
}

/// Sidebar inputs: the view (with what moves in it, quantized), theme,
/// size, scale.
type SidebarKey = (SidebarView, Theme, Size<i32, Logical>, f64);

pub struct Chrome {
    painter: Painter,
    /// When the chrome was made: what spins is timed from it.
    epoch: Instant,
    /// Outline every hit rect in red (`DEBUG_HITS` in the environment).
    debug_hits: bool,
    sidebar: Cached<SidebarKey>,
    /// The band breathing over a blocked card's header (one size fits all
    /// the cards).
    pulse: Cached<(Size<i32, Logical>, Theme, f64)>,
    tabs: Cached<(TabStripView, Theme, i32, f64)>,
    help: Overlay<(HelpLayout, Theme, f64)>,
    menu: Overlay<(MenuView, Theme, f64)>,
    tooltip: Overlay<(TooltipView, i32, Theme, f64)>,
    switcher: Overlay<(SwitcherView, i32, Theme, f64)>,
    welcome: Overlay<(WelcomeView, Size<i32, Logical>, Theme, f64)>,
    quit: Overlay<(QuitView, Size<i32, Logical>, Theme, f64)>,
    project_menu: Overlay<(ProjectMenuView, Size<i32, Logical>, Theme, f64)>,
    worktrees: Overlay<(WorktreesView, Size<i32, Logical>, Theme, f64)>,
    repos: Overlay<(ReposView, Size<i32, Logical>, Theme, f64)>,
    /// One per toast, by id, kept while it fades out.
    toasts: Vec<(u64, ToastOverlay)>,
    /// One per placeholder, and the spinner they share.
    placeholders: Vec<Cached<PlaceholderKey>>,
    spinner: Cached<(u32, Theme, f64)>,
    /// One per bar over a terminal pane.
    pane_bars: Vec<Cached<(PaneBarView, Theme, f64)>>,
    /// The strip along the focused pane's top edge.
    focus_strip: Cached<(Size<i32, Logical>, Theme, f64)>,
    /// The highlight where a held pane would land: its size, theme, scale.
    pane_drop: Cached<(Size<i32, Logical>, Theme, f64)>,
    /// The red outlines of every hit rect, while `debug_hits`.
    hits: Cached<(Vec<Rect>, Size<i32, Logical>, f64)>,
}

impl Chrome {
    /// The byte of `field`'s text nearest output x `x`, as it is drawn on
    /// an output of `scale`: where a click there puts the caret.
    pub fn caret_at(&self, field: &FieldSpot, x: f64, scale: f64) -> usize {
        self.painter.caret_at(
            &field.line,
            field.text,
            (x * scale) as f32,
            field.size,
            scale,
        )
    }

    pub fn new() -> Self {
        Self {
            painter: Painter::new(),
            epoch: Instant::now(),
            debug_hits: std::env::var_os(DEBUG_HITS).is_some_and(|v| v == "1"),
            sidebar: Cached::default(),
            pulse: Cached::default(),
            tabs: Cached::default(),
            help: Overlay::default(),
            menu: Overlay::default(),
            tooltip: Overlay::default(),
            switcher: Overlay::default(),
            welcome: Overlay::default(),
            quit: Overlay::default(),
            project_menu: Overlay::default(),
            worktrees: Overlay::default(),
            repos: Overlay::default(),
            toasts: Vec::new(),
            placeholders: Vec::new(),
            spinner: Cached::default(),
            pane_bars: Vec::new(),
            focus_strip: Cached::default(),
            pane_drop: Cached::default(),
            hits: Cached::default(),
        }
    }

    /// How wide a toast's button labelled `label` is (see
    /// `Painter::toast_button_width`).
    pub fn toast_button_width(&self, label: &str) -> i32 {
        self.painter.toast_button_width(label)
    }

    /// Hide the tooltip at once, without its fade: a press did.
    pub fn drop_tooltip(&mut self) {
        self.tooltip = Overlay::default();
    }

    /// How far the spinner has turned at `now`, as a step of
    /// `SPINNER_STEPS`.
    fn spinner_step(&self, now: Instant) -> u32 {
        (phase(self.epoch, SPINNER_PERIOD, now) * SPINNER_STEPS as f32) as u32 % SPINNER_STEPS
    }

    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        view: ChromeView,
    ) -> Vec<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let now = view.now;
        let scale = view.scale;
        let theme = view.theme;
        let sidebar_w = view.sidebar_width;
        let strip_h = tab_strip_height(view.density);
        let output: Size<i32, Logical> = (view.size.w.max(1), view.size.h.max(1)).into();
        let sidebar_size: Size<i32, Logical> = (sidebar_w, output.h).into();
        let strip_size: Size<i32, Logical> = ((view.size.w - sidebar_w).max(1), strip_h).into();
        // The app area, under the tab strip and right of the sidebar.
        let area = Rectangle::new(
            (sidebar_w, strip_h).into(),
            (strip_size.w, (view.size.h - strip_h).max(1)).into(),
        );
        let pointer = view.pointer;
        let painter = &self.painter;
        // A popup's buffer reaches past its panel by the shadow on every side.
        let shadowed = |panel: Rect| {
            (
                panel.loc - Point::from((SHADOW, SHADOW)),
                Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW)),
            )
        };
        let spinner_step = self.spinner_step(now);
        // Overlays that faded out for good are let go of.
        self.toasts.retain(|(_, toast)| toast.lingers());

        // Front to back. Every surface is one block here; the popups fade
        // in and out through `Overlay`.
        let mut parts: Vec<Part> = Vec::new();

        // The hit rects, outlined for checking them against the drawing.
        if self.debug_hits {
            let rects = hit_rects(&view, output, area);
            let buffer = self.hits.get((rects, output, scale), |(rects, output, _)| {
                hit_outlines(rects, *output, scale)
            });
            parts.push((buffer, Point::from((0, 0)), output, 1.0));
        }

        // The tooltip, by what it is about, over all else: it is offered
        // only for what nothing covers, or by the popup it is on.
        let tooltip = view.tooltip.map(|tip| {
            let w = painter.tooltip_width(&tip, output.w - 2 * HELP_MARGIN, scale);
            let size = (w, tip.height()).into();
            let (loc, size) = shadowed(tooltip_layout(tip.anchor, size, output));
            ((tip, w, theme, scale), loc, size)
        });
        parts.extend(self.tooltip.part(tooltip, now, true, |(tip, w, ..)| {
            painter.tooltip(tip, *w, scale, theme.palette())
        }));

        // The quit dialog covers everything.
        let quit = view.quit.map(|quit| {
            let (loc, size) = shadowed(quit_layout(output).panel);
            ((quit, output, theme, scale), loc, size)
        });
        parts.extend(self.quit.part(quit, now, true, |(quit, output, ..)| {
            painter.quit_dialog(quit, *output, scale, theme.palette())
        }));

        // The welcome screen covers all the rest; while it is fully there,
        // nothing under it is drawn.
        let welcome = view
            .welcome
            .map(|welcome| ((welcome, output, theme, scale), Point::from((0, 0)), output));
        let welcome_part =
            self.welcome
                .part_over(welcome, now, SLOW, false, |(welcome, output, ..)| {
                    painter.welcome(welcome, *output, scale, theme.palette())
                });
        let covered = welcome_part.as_ref().is_some_and(|part| part.3 >= 1.0);
        parts.extend(welcome_part);
        if covered {
            return render_parts(renderer, parts, scale);
        }

        // The context menu, under its anchor: over the popups it may open
        // on.
        let menu = view.context_menu.map(|menu| {
            let (panel, _) = menu_layout(menu.anchor, &menu.rows, output);
            let (loc, size) = shadowed(panel);
            ((menu, theme, scale), loc, size)
        });
        parts.extend(self.menu.part(menu, now, true, |(menu, ..)| {
            painter.menu(menu, scale, theme.palette())
        }));

        // The worktrees popup, over the sidebar and the apps.
        let worktrees = view.worktrees.map(|popup| {
            let (loc, size) = shadowed(worktrees_layout(&popup, output).panel);
            ((popup, output, theme, scale), loc, size)
        });
        parts.extend(
            self.worktrees
                .part(worktrees, now, true, |(popup, output, ..)| {
                    painter.worktrees(popup, *output, scale, theme.palette())
                }),
        );

        // The repositories panel, over nearly everything.
        let repos = view.repos.map(|panel| {
            let (loc, size) = shadowed(repos_layout(&panel, output).panel);
            ((panel, output, theme, scale), loc, size)
        });
        parts.extend(self.repos.part(repos, now, true, |(panel, output, ..)| {
            painter.repos(panel, *output, scale, theme.palette())
        }));

        // The project menu reaches over the sidebar and the apps.
        let project_menu = view.project_menu.map(|menu| {
            let (loc, size) = shadowed(project_menu_layout(&menu, output).panel);
            ((menu, output, theme, scale), loc, size)
        });
        parts.extend(
            self.project_menu
                .part(project_menu, now, true, |(menu, output, ..)| {
                    painter.project_menu(menu, *output, scale, theme.palette())
                }),
        );

        // The switcher at the top of the app area.
        let switcher = view.switcher.map(|switcher| {
            let panel = switcher_geometry(&switcher, area).panel;
            let (loc, size) = shadowed(panel);
            ((switcher, panel.size.w, theme, scale), loc, size)
        });
        parts.extend(
            self.switcher
                .part(switcher, now, true, |(switcher, w, ..)| {
                    painter.switcher(switcher, *w, scale, theme.palette())
                }),
        );

        // The help panel, centred at the bottom of the app area.
        let pinned = view.help_pinned;
        let help = view.prefix.as_deref().map(|help| {
            let layout = painter.help_layout(help, area.size.w - 2 * HELP_MARGIN, pinned, scale);
            let panel = Rectangle::new(
                (
                    sidebar_w + (area.size.w - layout.size.w) / 2,
                    (strip_h + area.size.h - layout.size.h - HELP_MARGIN).max(strip_h),
                )
                    .into(),
                layout.size,
            );
            let (loc, size) = shadowed(panel);
            ((layout, theme, scale), loc, size)
        });
        parts.extend(self.help.part(help, now, true, |(layout, ..)| {
            let help = view.prefix.as_deref().expect("the help the layout is of");
            painter.help_panel(help, layout, scale, theme.palette())
        }));

        // The toasts, stacked at the bottom right of the app area, each
        // fading on its own. One leaving fades where the stack has it, which
        // holds its place until it is gone.
        let layouts = toast_layout(&view.toasts, area);
        for toast in view.toasts.iter().filter(|toast| !toast.leaving) {
            if !self.toasts.iter().any(|(id, _)| *id == toast.id) {
                self.toasts.push((toast.id, Overlay::default()));
            }
        }
        for (id, overlay) in self.toasts.iter_mut() {
            let i = view.toasts.iter().position(|toast| toast.id == *id);
            if let Some(i) = i.filter(|&i| view.toasts[i].leaving) {
                overlay.place = Some(shadowed(layouts[i].panel));
            }
            let shown = i.filter(|&i| !view.toasts[i].leaving).map(|i| {
                let layout = &layouts[i];
                let hovered = layout
                    .button
                    .is_some_and(|button| button.to_f64().contains(pointer));
                let (loc, size) = shadowed(layout.panel);
                (
                    (
                        view.toasts[i].clone(),
                        layout.panel.size.w,
                        hovered,
                        theme,
                        scale,
                    ),
                    loc,
                    size,
                )
            });
            parts.extend(overlay.part(shown, now, true, |(toast, w, hovered, ..)| {
                painter.toast(toast, *w, *hovered, scale, theme.palette())
            }));
        }

        // The placeholders over the panes still waiting for their window,
        // and the spinner they share, turning with time.
        self.placeholders
            .resize_with(view.placeholders.len(), Cached::default);
        let spinner = self
            .spinner
            .get((spinner_step, theme, scale), |(step, ..)| {
                painter.spinner(*step, scale, theme.palette())
            });
        for (cached, placeholder) in self.placeholders.iter_mut().zip(view.placeholders) {
            let rect = placeholder.rect;
            if rect.size.w <= 0 || rect.size.h <= 0 {
                continue;
            }
            let layout = placeholder_layout(rect, placeholder.timed_out);
            if let Some(spot) = layout.spinner {
                parts.push((spinner, spot.loc, spot.size, 1.0));
            }
            let hovered = layout.button_at(pointer);
            let buffer = cached.get(
                (placeholder, hovered, theme, scale),
                |(placeholder, hovered, ..)| {
                    painter.placeholder(placeholder, *hovered, scale, theme.palette())
                },
            );
            parts.push((buffer, rect.loc, rect.size, 1.0));
        }

        // The strip marking the focused pane, then the pane bars, both on
        // the apps and under everything else.
        if let Some(pane) = view.focused_pane {
            let strip = focus_strip_rect(pane);
            if strip.size.w > 0 && strip.size.h > 0 {
                let buffer = self
                    .focus_strip
                    .get((strip.size, theme, scale), |(size, ..)| {
                        focus_strip(*size, scale, theme.palette())
                    });
                parts.push((buffer, strip.loc, strip.size, 1.0));
            }
        }
        self.pane_bars
            .resize_with(view.pane_bars.len(), Cached::default);
        for (cached, bar) in self.pane_bars.iter_mut().zip(view.pane_bars) {
            let (loc, size, alpha) = bar.placement();
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let buffer = cached.get((bar.still(), theme, scale), |(bar, ..)| {
                painter.pane_bar(bar, scale, theme.palette())
            });
            parts.push((buffer, loc, size, alpha));
        }
        // Where a pane held by its bar would land: over the apps and the
        // tab strip, after the bars so it goes under them.
        if let Some(rect) = view.pane_drop.filter(|r| r.size.w > 0 && r.size.h > 0) {
            let buffer = self.pane_drop.get((rect.size, theme, scale), |(size, ..)| {
                pane_drop_highlight(*size, scale, theme.palette())
            });
            parts.push((buffer, rect.loc, rect.size, 1.0));
        }

        // The blocked agents' pulse: one band per row, composed over the
        // sidebar with the pulse as its opacity, so the sidebar itself is
        // not redrawn for it.
        let pulse_alpha = pulse_alpha(view.pulse);
        let mut bands = Vec::new();
        if pulse_alpha > 0.0 {
            let layout = sidebar_layout_at(
                &view.sidebar.workspaces,
                &view.sidebar.agents,
                sidebar_w,
                view.density,
            )
            .scrolled(view.sidebar.state.scroll, output.h);
            let blocked = |i: usize, k: usize| {
                view.sidebar
                    .agents
                    .get(i)
                    .and_then(|cards| cards.get(k))
                    .is_some_and(|card| card.status == crate::herdr::AgentStatus::Blocked)
            };
            // The rows in the workspaces, and those in the "Needs you" list.
            let rows = layout
                .cards
                .iter()
                .enumerate()
                .flat_map(|(i, rows)| rows.iter().map(move |(k, rect)| (i, *k, *rect)));
            for (_, _, rect) in rows
                .chain(layout.inbox.iter().copied())
                .filter(|(i, k, _)| blocked(*i, *k))
            {
                let band = blocked_band(rect, view.density);
                // Not on a row scrolled under the header.
                if band.size.w > 0 && band.size.h > 0 && band.loc.y >= BAR_HEIGHT {
                    bands.push(band);
                }
            }
        }

        // The sidebar and the tab strip.
        let sidebar = self.sidebar.get(
            (view.sidebar, theme, sidebar_size, scale),
            |(sidebar, ..)| painter.sidebar_view(sidebar, sidebar_size, scale, theme.palette()),
        );
        let strip = self.tabs.get(
            (view.tab_strip, theme, strip_size.w, scale),
            |(strip, ..)| painter.tab_strip_view(strip, theme, strip_size, scale),
        );
        parts.push((sidebar, Point::from((0, 0)), sidebar_size, 1.0));
        // Every row is as wide as the next, so one band buffer fits all.
        if let Some(size) = bands.first().map(|band| band.size) {
            let buffer = self.pulse.get((size, theme, scale), |(size, ..)| {
                painter.pulse_band(*size, scale, theme.palette())
            });
            for band in bands.iter().filter(|band| band.size == size) {
                parts.push((buffer, band.loc, size, pulse_alpha));
            }
        }
        parts.push((strip, Point::from((sidebar_w, 0)), strip_size, 1.0));
        render_parts(renderer, parts, scale)
    }
}

/// A drawn buffer, where it goes on the output, the logical size it covers
/// and its opacity (0 to 1).
type Part<'a> = (
    &'a MemoryRenderBuffer,
    Point<i32, Logical>,
    Size<i32, Logical>,
    f32,
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
        .filter_map(|(buffer, loc, size, alpha)| {
            // The buffer holds physical pixels at buffer scale 1: sample all of it
            // and squeeze it into the logical size.
            let src = Rectangle::from_size(physical(size, scale).to_f64());
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc.to_f64().to_physical(scale),
                buffer,
                Some(alpha),
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

/// A popup's cached buffer, faded in as its view appears and out as it
/// goes: the last buffer is kept and shown with falling opacity where it
/// was.
struct Overlay<K> {
    cached: Cached<K>,
    /// Where the part last went, to fade it out there.
    place: Option<(Point<i32, Logical>, Size<i32, Logical>)>,
    /// The fade running, if one is.
    fade: Option<Fade>,
    /// Whether the view was there the last time it was asked for.
    present: bool,
}

/// A toast's overlay: the toast, its width, whether its button is lit,
/// the theme and the scale.
type ToastOverlay = Overlay<(ToastView, i32, bool, Theme, f64)>;

/// A popup's inputs while it is there: its cache key, where it goes and
/// the logical size it covers.
type Placed<K> = Option<(K, Point<i32, Logical>, Size<i32, Logical>)>;

impl<K> Default for Overlay<K> {
    fn default() -> Self {
        Self {
            cached: Cached::default(),
            place: None,
            fade: None,
            present: false,
        }
    }
}

impl<K: PartialEq> Overlay<K> {
    /// Whether there is still something to show: the view, or its fade out.
    fn lingers(&self) -> bool {
        self.present || self.fade.is_some()
    }

    /// The part to draw this frame, if any: `view` is the key, place and
    /// size while the view is there. A view that appears rises over `FAST`
    /// (and up by `SLIDE` when `slide`); one that goes fades at its last
    /// place. A fade turning around goes on from where it stands.
    fn part(
        &mut self,
        view: Placed<K>,
        now: Instant,
        slide: bool,
        draw: impl FnOnce(&K) -> MemoryRenderBuffer,
    ) -> Option<Part<'_>> {
        self.part_over(view, now, FAST, slide, draw)
    }

    /// `part`, fading over `dur`.
    fn part_over(
        &mut self,
        view: Placed<K>,
        now: Instant,
        dur: std::time::Duration,
        slide: bool,
        draw: impl FnOnce(&K) -> MemoryRenderBuffer,
    ) -> Option<Part<'_>> {
        let present = view.is_some();
        if present != self.present {
            let from = match self.fade {
                Some(fade) => fade.value(now),
                None if self.present => 1.0,
                None => 0.0,
            };
            self.fade = Some(Fade::from_value(now, dur, present, from));
            self.present = present;
        }
        let alpha = match self.fade {
            Some(fade) if fade.done(now) => {
                self.fade = None;
                if present { 1.0 } else { 0.0 }
            }
            Some(fade) => fade.value(now),
            None if present => 1.0,
            None => 0.0,
        };
        let (loc, size) = match &view {
            Some((_, loc, size)) => {
                self.place = Some((*loc, *size));
                (*loc, *size)
            }
            None => self.place?,
        };
        if alpha <= 0.0 {
            return None;
        }
        let buffer = match view {
            Some((key, ..)) => self.cached.get(key, draw),
            None => self.cached.buffer.as_ref()?,
        };
        let rise = if slide {
            ((1.0 - alpha) * SLIDE as f32).round() as i32
        } else {
            0
        };
        Some((buffer, (loc.x, loc.y + rise).into(), size, alpha))
    }
}

/// Every rect the chrome hit-tests, in window coordinates, for `view`.
fn hit_rects(view: &ChromeView, output: Size<i32, Logical>, area: Rect) -> Vec<Rect> {
    let mut rects = Vec::new();
    let sidebar = sidebar_layout_at(
        &view.sidebar.workspaces,
        &view.sidebar.agents,
        view.sidebar_width,
        view.density,
    )
    .scrolled(view.sidebar.state.scroll, output.h);
    rects.extend([sidebar.header, sidebar.plus, sidebar.repos]);
    for row in &sidebar.rows {
        rects.extend([row.rect, row.close]);
        rects.extend(
            [row.chip, row.fold, row.ci, row.worktrees]
                .into_iter()
                .flatten(),
        );
    }
    rects.extend(sidebar.cards.iter().flatten().map(|(_, rect)| *rect));
    rects.extend(sidebar.inbox.iter().map(|(_, _, rect)| *rect));
    let strip_w = output.w - view.sidebar_width;
    let shift = |rect: Rect| {
        Rectangle::new(
            (rect.loc.x + view.sidebar_width, rect.loc.y).into(),
            rect.size,
        )
    };
    let strip = &view.tab_strip;
    let strip_size = (strip_w, tab_strip_height(view.density)).into();
    let layout = tab_layout(
        strip.tabs.len(),
        strip_size,
        strip.prefix,
        strip.overflow(),
        strip.watch.is_some(),
    );
    rects.extend(layout.tabs.iter().chain(&layout.closes).map(|r| shift(*r)));
    if !layout.tabs.is_empty() {
        rects.push(shift(layout.plus));
    }
    rects.extend(layout.overflow.into_iter().chain(layout.prefix).map(shift));
    rects.push(shift(layout.toggle));
    for bar in view.pane_bars.iter().filter(|bar| bar.visible) {
        rects.push(bar.rect);
        rects.extend(pane_bar_buttons(bar.rect).map(|(_, rect)| rect));
    }
    if let Some(menu) = &view.context_menu {
        rects.extend(menu_layout(menu.anchor, &menu.rows, output).1);
    }
    let toasts = toast_layout(&view.toasts, area);
    for (layout, _) in toasts.iter().zip(&view.toasts).filter(|(_, t)| !t.leaving) {
        rects.push(layout.panel);
        rects.extend(layout.button);
    }
    for placeholder in &view.placeholders {
        let layout = placeholder_layout(placeholder.rect, placeholder.timed_out);
        rects.extend(layout.retry.into_iter().chain(layout.close));
    }
    if let Some(switcher) = &view.switcher {
        let layout = switcher_geometry(switcher, area);
        rects.extend(layout.rows.into_iter().chain(layout.chips));
    }
    if let Some(popup) = &view.worktrees {
        for (rect, buttons) in worktrees_layout(popup, output).entries {
            rects.push(rect);
            rects.extend(buttons.into_iter().map(|(_, rect)| rect));
        }
    }
    if let Some(panel) = &view.repos {
        let layout = repos_layout(panel, output);
        for (line, check, expander) in layout.lines {
            rects.extend([line, check]);
            rects.extend(expander);
        }
        rects.extend(layout.buttons.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.detail_tabs.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.chips.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.more);
        rects.push(layout.splitter);
        rects.push(layout.field);
        rects.push(layout.branch);
        if let Some((menu, rows)) = layout.branch_menu {
            rects.push(menu);
            rects.extend(rows);
        }
    }
    if view.quit.is_some() {
        let quit = quit_layout(output);
        rects.push(quit.checkbox);
        rects.extend(quit.buttons);
    }
    rects
}

/// `rects` outlined in red, a pixel thick, on a clear buffer of `output`.
fn hit_outlines(rects: &[Rect], output: Size<i32, Logical>, scale: f64) -> MemoryRenderBuffer {
    let mut c = Canvas::new(physical(output, scale));
    let px = |v: i32| (v as f64 * scale).round() as i32;
    for rect in rects {
        c.outline(
            px(rect.loc.x),
            px(rect.loc.y),
            px(rect.size.w),
            px(rect.size.h),
            px(1).max(1),
            [0xe0, 0x30, 0x30],
        );
    }
    c.into_buffer(false)
}

#[cfg(test)]
pub(super) fn inside(outer: Rect, inner: Rect) -> bool {
    inner.loc.x >= outer.loc.x
        && inner.loc.y >= outer.loc.y
        && inner.loc.x + inner.size.w <= outer.loc.x + outer.size.w
        && inner.loc.y + inner.size.h <= outer.loc.y + outer.size.h
}
