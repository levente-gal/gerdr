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

mod badge;
mod canvas;
mod checkout;
mod confirm;
mod consts;
mod drop;
mod features;
mod footer;
#[cfg(test)]
mod golden;
mod help;
mod identity;
mod hints;
mod inbox;
mod layers;
mod manual_test;
mod menu;
mod painter;
mod pane_bar;
mod placeholder;
mod project_menu;
mod quit;
mod repos;
mod restart;
mod scrollback;
mod search;
mod shapes;
mod sidebar;
mod switcher;
mod tabs;
mod theme;
mod thumb;
mod toast;
mod tooltip;
mod welcome;
mod worktrees;

use std::time::Instant;

use smithay::{
    backend::renderer::{element::memory::MemoryRenderBuffer, gles::GlesRenderer},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    anim::{FAST, Fade, SLOW, phase},
    overlay::Overlay as Kind,
};

// One surface per module; the rest of gerdr reaches all of it as
// `chrome::X`. The canvas and the shapes stay inside.
pub use badge::*;
use canvas::*;
pub use checkout::*;
pub use confirm::*;
pub use consts::*;
pub use drop::*;
pub use features::*;
pub use footer::*;
pub use help::*;
pub use identity::*;
pub use hints::*;
pub use inbox::*;
pub use layers::ChromeElement;
use layers::*;
pub use manual_test::*;
pub use menu::*;
pub use painter::*;
pub use pane_bar::*;
pub use placeholder::*;
pub use project_menu::*;
pub use quit::*;
pub use repos::*;
pub use restart::*;
pub use scrollback::*;
pub use search::*;
use shapes::*;
pub use sidebar::*;
pub use switcher::*;
pub use tabs::*;
pub use theme::*;
pub use thumb::*;
pub use toast::*;
pub use tooltip::*;
pub use welcome::*;
pub use worktrees::*;

pub(crate) type Rect = Rectangle<i32, Logical>;

/// How far a popup rises as it fades in, in logical pixels.
const SLIDE: i32 = 4;
/// The environment variable that outlines every hit rect in red.
pub const DEBUG_HITS: &str = "GERDR_DEBUG_HITS";

/// What the tasks panel's drawing depends on: it, the output, the
/// footer's height, the theme and the scale.
type TasksKey = (TasksView, Size<i32, Logical>, i32, Theme, f64);

/// What a placeholder's drawing depends on: it, its button under the
/// pointer, the theme and the scale.
type PlaceholderKey = (PlaceholderView, Option<PlaceholderButton>, Theme, f64);

pub struct ChromeView {
    /// When the view was taken, for what moves with time.
    pub now: Instant,
    /// Where the pointer is (window coordinates): the toasts' buttons and
    /// "×"s and the placeholders' Retry and Close light up under it.
    pub pointer: Point<f64, Logical>,
    /// The help panel is pinned open, rather than up for the prefix: it
    /// then carries a title and an Esc hint.
    pub help_pinned: bool,
    /// What the pointer is on in the help panel.
    pub help_hover: Option<HelpHit>,
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
    /// Set while the restart dialog is open, over all but that.
    pub restart: Option<RestartView>,
    /// Set while the checkout dialog is open, over all but the quit dialog.
    pub checkout: Option<CheckoutView>,
    /// Set while the confirm dialog is open, over all but the quit dialog.
    pub confirm: Option<ConfirmView>,
    /// Set while the new feature dialog is open.
    pub new_feature: Option<NewFeatureView>,
    /// Set while the name and email dialog is open.
    pub identity: Option<IdentityView>,
    /// Set while the manual test sheet is open.
    pub manual_test: Option<ManualTestView>,
    /// Set while the feature panel is open.
    pub features: Option<FeaturePanelView>,
    /// Set while the project menu is open.
    pub project_menu: Option<ProjectMenuView>,
    /// Set while the worktrees popup is open.
    pub worktrees: Option<WorktreesView>,
    /// Set while the repositories panel is open.
    pub repos: Option<ReposView>,
    /// Set while the inbox panel is open.
    pub inbox: Option<InboxView>,
    /// Set while the scrollback viewer is open over a terminal pane.
    pub scrollback: Option<ScrollbackView>,
    /// The toasts showing, oldest first, such as "Switched to gerdr".
    pub toasts: Vec<ToastView>,
    /// The bars over the active tab's terminal panes.
    pub pane_bars: Vec<PaneBarView>,
    /// The footer along the bottom, unless the config file hides it.
    pub footer: Option<FooterView>,
    /// Set while the tasks panel is open over the footer.
    pub tasks: Option<TasksView>,
    /// Where the button unzooming the zoomed pane floats, if one is.
    pub unzoom: Option<Rect>,
    pub theme: Theme,
    pub density: Density,
    pub sidebar_width: i32,
    pub size: Size<i32, Logical>,
    pub scale: f64,
}

pub struct Chrome {
    painter: Painter,
    /// When the chrome was made: what spins is timed from it.
    epoch: Instant,
    /// Outline every hit rect in red (`DEBUG_HITS` in the environment).
    debug_hits: bool,
    sidebar: SidebarLayers,
    tabs: Cached<(TabStripView, Theme, i32, f64)>,
    footer: Cached<(FooterView, Theme, i32, f64)>,
    tasks: Overlay<TasksKey>,
    help: Overlay<(HelpLayout, Option<HelpHit>, Theme, f64)>,
    menu: Overlay<(MenuView, Theme, f64)>,
    tooltip: Overlay<(TooltipView, i32, Theme, f64)>,
    switcher: SwitcherLayers,
    welcome: Overlay<(WelcomeView, Size<i32, Logical>, Theme, f64)>,
    quit: Overlay<(QuitView, Size<i32, Logical>, Theme, f64)>,
    restart: Overlay<(RestartView, Size<i32, Logical>, Theme, f64)>,
    checkout: Overlay<(CheckoutView, Size<i32, Logical>, Theme, f64)>,
    confirm: Overlay<(ConfirmView, Size<i32, Logical>, Theme, f64)>,
    new_feature: Overlay<(NewFeatureView, Size<i32, Logical>, Theme, f64)>,
    identity: Overlay<(IdentityView, Size<i32, Logical>, Theme, f64)>,
    manual_test: Overlay<(ManualTestView, Size<i32, Logical>, Theme, f64)>,
    features: Overlay<(FeaturePanelView, Size<i32, Logical>, Theme, f64)>,
    project_menu: ProjectMenuLayers,
    worktrees: WorktreesLayers,
    repos: ReposLayers,
    inbox: InboxLayers,
    scrollback: ScrollbackLayers,
    /// One per toast, by id, kept while it fades out.
    toasts: Vec<(u64, ToastOverlay)>,
    /// One per placeholder, and the spinner they share.
    placeholders: Vec<Cached<PlaceholderKey>>,
    spinner: Cached<(u32, Theme, f64)>,
    /// The blinking dot of what runs, composed over every surface showing
    /// one (see `badge`).
    blink_dot: Cached<(Theme, f64)>,
    /// One per bar over a terminal pane.
    pane_bars: Vec<Cached<(PaneBarView, Theme, f64)>>,
    pane_grips: Vec<Cached<GripKey>>,
    /// The button over a zoomed pane: lit or not, theme, scale.
    unzoom: Cached<(bool, Theme, f64)>,
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
            sidebar: SidebarLayers::default(),
            tabs: Cached::default(),
            footer: Cached::default(),
            tasks: Overlay::default(),
            help: Overlay::default(),
            menu: Overlay::default(),
            tooltip: Overlay::default(),
            switcher: SwitcherLayers::default(),
            welcome: Overlay::default(),
            quit: Overlay::default(),
            restart: Overlay::default(),
            checkout: Overlay::default(),
            confirm: Overlay::default(),
            new_feature: Overlay::default(),
            identity: Overlay::default(),
            manual_test: Overlay::default(),
            features: Overlay::default(),
            project_menu: ProjectMenuLayers::default(),
            worktrees: WorktreesLayers::default(),
            repos: ReposLayers::default(),
            inbox: InboxLayers::default(),
            scrollback: ScrollbackLayers::default(),
            toasts: Vec::new(),
            placeholders: Vec::new(),
            spinner: Cached::default(),
            blink_dot: Cached::default(),
            pane_bars: Vec::new(),
            pane_grips: Vec::new(),
            unzoom: Cached::default(),
            focus_strip: Cached::default(),
            pane_drop: Cached::default(),
            hits: Cached::default(),
        }
    }

    /// What `pos` is over in the help panel listing `help` in the app
    /// `area`, `pinned` open or up for the prefix; `None` off it.
    pub fn help_hit(
        &self,
        help: &Help,
        pinned: bool,
        area: Rect,
        scale: f64,
        pos: Point<f64, Logical>,
    ) -> Option<HelpHit> {
        let max = (area.size.w - 2 * HELP_MARGIN, area.size.h - 2 * HELP_MARGIN);
        let layout = self
            .painter
            .help_layout_within(help, max.into(), pinned, scale);
        let panel = help_rect(&layout, area);
        if !panel.to_f64().contains(pos) {
            return None;
        }
        let spots = self.painter.help_spots(help, &layout, panel.loc, scale);
        let hit = spots
            .into_iter()
            .find(|(_, rect)| rect.to_f64().contains(pos));
        Some(hit.map_or(HelpHit::Panel, |(hit, _)| hit))
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
        mut view: ChromeView,
    ) -> Vec<ChromeElement> {
        let now = view.now;
        let scale = view.scale;
        let theme = view.theme;
        let sidebar_w = view.sidebar_width;
        let strip_h = tab_strip_height(view.density);
        let output: Size<i32, Logical> = (view.size.w.max(1), view.size.h.max(1)).into();
        let strip_size: Size<i32, Logical> = ((view.size.w - sidebar_w).max(1), strip_h).into();
        let footer_h = if view.footer.is_some() {
            status_bar_height(view.density)
        } else {
            0
        };
        // Over the footer: the sidebar's height, and the app area's bottom.
        let above: Size<i32, Logical> = (output.w, (output.h - footer_h).max(1)).into();
        // The app area, under the tab strip and right of the sidebar.
        let area = Rectangle::new(
            (sidebar_w, strip_h).into(),
            (strip_size.w, (view.size.h - strip_h - footer_h).max(1)).into(),
        );
        let pointer = view.pointer;
        let painter = &self.painter;
        let spinner_step = self.spinner_step(now);
        let blink = crate::anim::blink(self.epoch, now);
        let dot = self.blink_dot.get((theme, scale), |_| {
            blink_dot(Dot::Working, scale, theme.palette())
        });
        // Overlays that faded out for good are let go of.
        self.toasts.retain(|(_, toast)| toast.lingers());

        // Front to back. Every surface is one block here; the popups fade
        // in and out through `Overlay`.
        let mut parts: Vec<Part> = Vec::new();

        // The hit rects, outlined for checking them against the drawing.
        if self.debug_hits {
            let rects = hit_rects(&view, output, area, footer_h);
            let buffer = self.hits.get((rects, output, scale), |(rects, output, _)| {
                hit_outlines(rects, *output, scale)
            });
            parts.push(Part::new(buffer, Point::from((0, 0)), output, 1.0));
        }

        // The overlays, top first in their one order (see
        // `crate::overlay`): what takes a click is what shows on top.
        for kind in Kind::ORDER {
            match kind {
                // The tooltip, by what it is about, over all else: it is
                // offered only for what nothing covers, or by the popup it
                // is on.
                Kind::Tooltip => {
                    let tooltip = view.tooltip.take().map(|tip| {
                        let w = painter.tooltip_width(&tip, output.w - 2 * HELP_MARGIN, scale);
                        let size = (w, tip.height()).into();
                        let (loc, size) = shadowed(tooltip_layout(tip.anchor, size, output));
                        ((tip, w, theme, scale), loc, size)
                    });
                    parts.extend(self.tooltip.part(tooltip, now, true, |(tip, w, ..)| {
                        painter.tooltip(tip, *w, scale, theme.palette())
                    }));
                }
                // The quit dialog covers everything.
                Kind::Quit => {
                    let quit = view.quit.take().map(|quit| {
                        let layout = quit_layout(output, quit.rows.len(), quit.switch_to.is_some());
                        let (loc, size) = shadowed(layout.panel);
                        ((quit, output, theme, scale), loc, size)
                    });
                    let part = self.quit.part(quit, now, true, |(quit, output, ..)| {
                        painter.quit_dialog(quit, *output, scale, theme.palette())
                    });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                // The other dialogs cover all but the quit dialog.
                Kind::Restart => {
                    let restart = view.restart.take().map(|restart| {
                        let (loc, size) =
                            shadowed(restart_layout(output, restart.rows.len()).panel);
                        ((restart, output, theme, scale), loc, size)
                    });
                    let part = self
                        .restart
                        .part(restart, now, true, |(restart, output, ..)| {
                            painter.restart_dialog(restart, *output, scale, theme.palette())
                        });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                Kind::Checkout => {
                    let checkout = view.checkout.take().map(|checkout| {
                        let (loc, size) =
                            shadowed(checkout_layout(output, checkout.rows.len()).panel);
                        ((checkout, output, theme, scale), loc, size)
                    });
                    let part = self
                        .checkout
                        .part(checkout, now, true, |(checkout, output, ..)| {
                            painter.checkout_dialog(checkout, *output, scale, theme.palette())
                        });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                Kind::NewFeature => {
                    let dialog = view.new_feature.take().map(|v| {
                        let layout = new_feature_layout(output, v.repos.len(), v.error.is_some());
                        let (loc, size) = shadowed(layout.panel);
                        ((v, output, theme, scale), loc, size)
                    });
                    let part = self.new_feature.part(dialog, now, true, |(v, output, ..)| {
                        painter.new_feature_dialog(v, *output, scale, theme.palette())
                    });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                Kind::GitIdentity => {
                    let dialog = view.identity.take().map(|v| {
                        let (loc, size) =
                            shadowed(identity_layout(output, v.error.is_some()).panel);
                        ((v, output, theme, scale), loc, size)
                    });
                    let part = self.identity.part(dialog, now, true, |(v, output, ..)| {
                        painter.identity_dialog(v, *output, scale, theme.palette())
                    });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                Kind::ManualTest => {
                    let sheet = view.manual_test.take().map(|v| {
                        let (loc, size) = shadowed(manual_test_layout(&v, output).panel);
                        ((v, output, theme, scale), loc, size)
                    });
                    let part = self.manual_test.part(sheet, now, true, |(v, output, ..)| {
                        painter.manual_test_sheet(v, *output, scale, theme.palette())
                    });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                Kind::Confirm => {
                    let confirm = view.confirm.take().map(|confirm| {
                        let lines = confirm_body_lines(&confirm.body);
                        let (loc, size) = shadowed(confirm_lines_layout(output, "", lines).panel);
                        ((confirm, output, theme, scale), loc, size)
                    });
                    let part = self
                        .confirm
                        .part(confirm, now, true, |(confirm, output, ..)| {
                            painter.confirm_dialog(confirm, *output, scale, theme.palette())
                        });
                    parts.extend(scrimmed(part, output, theme.palette()));
                }
                // The context menu, under its anchor: over the popups it
                // may open on.
                Kind::ContextMenu => {
                    let menu = view.context_menu.take().map(|menu| {
                        let (panel, _) = menu_layout(menu.anchor, &menu.rows, output);
                        let (loc, size) = shadowed(panel);
                        ((menu, theme, scale), loc, size)
                    });
                    parts.extend(self.menu.part(menu, now, true, |(menu, ..)| {
                        painter.menu(menu, scale, theme.palette())
                    }));
                }
                // The toasts, stacked at the bottom right of the app area,
                // each fading on its own, over every popup but the menu and
                // the dialogs (the manual test sheet is under them). One leaving fades where the stack has it,
                // which holds its place until it is gone.
                Kind::Toasts => {
                    let layouts = toast_layout(&view.toasts, toast_area(area, view.unzoom));
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
                            let lit = layout.spot_at(pointer);
                            let (loc, size) = shadowed(layout.panel);
                            (
                                (
                                    view.toasts[i].clone(),
                                    layout.panel.size.w,
                                    lit,
                                    theme,
                                    scale,
                                ),
                                loc,
                                size,
                            )
                        });
                        parts.extend(overlay.part(shown, now, true, |(toast, w, lit, ..)| {
                            painter.toast_lit(toast, *w, *lit, scale, theme.palette())
                        }));
                    }
                }
                // The welcome screen covers all the rest; while it is fully
                // there, nothing under it is drawn.
                Kind::Welcome => {
                    let welcome = view.welcome.take().map(|welcome| {
                        ((welcome, output, theme, scale), Point::from((0, 0)), output)
                    });
                    let welcome_part = self.welcome.part_over(
                        welcome,
                        now,
                        SLOW,
                        false,
                        |(welcome, output, ..)| {
                            painter.welcome(welcome, *output, scale, theme.palette())
                        },
                    );
                    let covered = welcome_part.as_ref().is_some_and(|part| part.alpha >= 1.0);
                    parts.extend(welcome_part);
                    if covered {
                        return render_parts(renderer, parts, scale);
                    }
                }
                // The help panel, centred at the bottom of the app area,
                // over the panels it may be pinned over.
                Kind::Help => {
                    let pinned = view.help_pinned;
                    let help = view.prefix.as_deref().map(|help| {
                        let max = (area.size.w - 2 * HELP_MARGIN, area.size.h - 2 * HELP_MARGIN);
                        let layout = painter.help_layout_within(help, max.into(), pinned, scale);
                        let (loc, size) = shadowed(help_rect(&layout, area));
                        ((layout, view.help_hover, theme, scale), loc, size)
                    });
                    parts.extend(self.help.part(help, now, true, |(layout, hover, ..)| {
                        let help = view.prefix.as_deref().expect("the help the layout is of");
                        painter.help_panel(help, layout, *hover, scale, theme.palette())
                    }));
                }
                // The background tasks, over the footer.
                Kind::Tasks => {
                    let tasks = view.tasks.take().map(|tasks| {
                        let (loc, size) = shadowed(tasks_layout(&tasks, output, footer_h).panel);
                        ((tasks, output, footer_h, theme, scale), loc, size)
                    });
                    parts.extend(self.tasks.part(
                        tasks,
                        now,
                        true,
                        |(tasks, output, footer, ..)| {
                            painter.tasks_panel(tasks, *output, *footer, scale, theme.palette())
                        },
                    ));
                }
                // The feature panel, centred over the apps.
                Kind::Features => {
                    let panel = view.features.take().map(|v| {
                        let (loc, size) = shadowed(feature_panel_layout(&v, output).panel);
                        ((v, output, theme, scale), loc, size)
                    });
                    parts.extend(self.features.part(panel, now, true, |(v, output, ..)| {
                        painter.feature_panel(v, *output, scale, theme.palette())
                    }));
                }
                // The project menu reaches over the sidebar and the apps.
                Kind::ProjectMenu => parts.extend(self.project_menu.parts(
                    view.project_menu.take(),
                    output,
                    theme,
                    scale,
                    now,
                    painter,
                )),
                // The worktrees popup, over the sidebar and the apps.
                Kind::Worktrees => parts.extend(self.worktrees.parts(
                    view.worktrees.take(),
                    output,
                    theme,
                    scale,
                    now,
                    painter,
                )),
                // The repositories panel, over nearly everything.
                Kind::Repos => parts.extend(self.repos.parts(
                    view.repos.take(),
                    output,
                    theme,
                    scale,
                    now,
                    painter,
                )),
                // The inbox panel, as high.
                Kind::Inbox => parts.extend(self.inbox.parts(
                    view.inbox.take(),
                    output,
                    theme,
                    scale,
                    now,
                    painter,
                    Some((dot, blink)),
                )),
                // The scrollback viewer, over its pane.
                Kind::Scrollback => parts.extend(self.scrollback.parts(
                    view.scrollback.take(),
                    theme,
                    scale,
                    now,
                    painter,
                )),
                // The switcher at the top of the app area.
                Kind::Switcher => parts.extend(self.switcher.parts(
                    view.switcher.take(),
                    area,
                    theme,
                    scale,
                    now,
                    painter,
                    Some((dot, blink)),
                )),
            }
        }

        // The button unzooming the zoomed pane, over it and under the
        // toasts.
        if let Some(button) = view.unzoom {
            let hovered = button.to_f64().contains(pointer);
            let buffer = self.unzoom.get((hovered, theme, scale), |(hovered, ..)| {
                painter.unzoom_button(*hovered, scale, theme.palette())
            });
            let (loc, size) = shadowed(button);
            parts.push(Part::new(buffer, loc, size, 1.0));
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
                parts.push(Part::new(spinner, spot.loc, spot.size, 1.0));
            }
            let hovered = layout.button_at(pointer);
            let buffer = cached.get(
                (placeholder, hovered, theme, scale),
                |(placeholder, hovered, ..)| {
                    painter.placeholder(placeholder, *hovered, scale, theme.palette())
                },
            );
            parts.push(Part::new(buffer, rect.loc, rect.size, 1.0));
        }

        // The pane bars, on the apps and under everything else.
        self.pane_bars
            .resize_with(view.pane_bars.len(), Cached::default);
        self.pane_grips
            .resize_with(view.pane_bars.len(), Cached::default);
        let bars = self.pane_bars.iter_mut().zip(self.pane_grips.iter_mut());
        for ((cached, grip_cache), bar) in bars.zip(view.pane_bars) {
            let (loc, size, alpha) = bar.placement();
            if size.w > 0 && size.h > 0 {
                push_dots(&mut parts, dot, bar.blinker(), alpha * blink);
                let buffer = cached.get((bar.still(), theme, scale), |(bar, ..)| {
                    painter.pane_bar(bar, scale, theme.palette())
                });
                parts.push(Part::new(buffer, loc, size, alpha));
            }
            // A floating bar's grip stays, under its pill while it shows.
            if let Some(grip) = bar.grip() {
                let buffer = grip_cache.get(
                    (grip.size, bar.focused, theme, scale),
                    |(size, focused, ..)| {
                        painter.pane_handle(*size, *focused, scale, theme.palette())
                    },
                );
                parts.push(Part::new(buffer, grip.loc, grip.size, pane_bar::PILL_REST));
            }
        }
        // The strip marking the focused pane, on the apps and under the
        // pane bars, whose pills hang over it. The app shows through it,
        // as through a grip.
        if let Some(pane) = view.focused_pane {
            let strip = focus_strip_rect(pane);
            if strip.size.w > 0 && strip.size.h > 0 {
                let buffer = self
                    .focus_strip
                    .get((strip.size, theme, scale), |(size, ..)| {
                        focus_strip(*size, scale, theme.palette())
                    });
                parts.push(Part::new(buffer, strip.loc, strip.size, pane_bar::PILL_REST));
            }
        }
        // Where a pane held by its bar would land: over the apps and the
        // tab strip, after the bars so it goes under them.
        if let Some(rect) = view.pane_drop.filter(|r| r.size.w > 0 && r.size.h > 0) {
            let buffer = self.pane_drop.get((rect.size, theme, scale), |(size, ..)| {
                pane_drop_highlight(*size, scale, theme.palette())
            });
            parts.push(Part::new(buffer, rect.loc, rect.size, 1.0));
        }

        // The sidebar and the tab strip.
        // The tab strip's blinking dots, moved onto the output.
        let strip_dots: Vec<Rect> = tab_blinkers(&view.tab_strip, strip_size)
            .into_iter()
            .map(|dot| Rectangle::new(dot.loc + Point::from((sidebar_w, 0)), dot.size))
            .collect();
        let strip = self.tabs.get(
            (view.tab_strip, theme, strip_size.w, scale),
            |(strip, ..)| painter.tab_strip_view(strip, theme, strip_size, scale),
        );
        if let Some(footer) = view.footer {
            let size: Size<i32, Logical> = (output.w, footer_h).into();
            let top = output.h - footer_h;
            let dots = footer_blinkers(&footer, size)
                .into_iter()
                .map(|dot| Rectangle::new(dot.loc + Point::from((0, top)), dot.size));
            push_dots(&mut parts, dot, dots, blink);
            let buffer = self
                .footer
                .get((footer, theme, output.w, scale), |(footer, ..)| {
                    painter.footer(footer, size, scale, theme.palette())
                });
            parts.push(Part::new(buffer, Point::from((0, top)), size, 1.0));
        }
        parts.extend(self.sidebar.parts(
            view.sidebar,
            sidebar_w,
            view.pulse,
            view.density,
            above,
            theme,
            scale,
            painter,
            Some((dot, blink)),
        ));
        push_dots(&mut parts, dot, strip_dots, blink);
        parts.push(Part::new(
            strip,
            Point::from((sidebar_w, 0)),
            strip_size,
            1.0,
        ));
        render_parts(renderer, parts, scale)
    }
}

/// Where a popup's buffer goes for its `panel`: it reaches past the panel
/// by the shadow on every side.
fn shadowed(panel: Rect) -> (Point<i32, Logical>, Size<i32, Logical>) {
    (
        panel.loc - Point::from((SHADOW, SHADOW)),
        Size::from((panel.size.w + 2 * SHADOW, panel.size.h + 2 * SHADOW)),
    )
}

/// A modal dialog's `part`, if it shows, and the scrim dimming the
/// `output` under it, fading in and out with it.
fn scrimmed(part: Option<Part>, output: Size<i32, Logical>, p: &Palette) -> Vec<Part> {
    let Some(part) = part else {
        return Vec::new();
    };
    let dim = scrim(Rectangle::from_size(output), part.alpha, p);
    vec![part, dim]
}

/// The blinking `dot` at each of `rects` (output coordinates), `alpha`
/// opaque: in front of the surface they are on, so pushed before it.
fn push_dots(
    parts: &mut Vec<Part>,
    dot: &MemoryRenderBuffer,
    rects: impl IntoIterator<Item = Rect>,
    alpha: f32,
) {
    for rect in rects {
        if rect.size.w > 0 && rect.size.h > 0 {
            parts.push(Part::new(dot, rect.loc, rect.size, alpha));
        }
    }
}

/// What a floating bar's grip is drawn from: its size, whether its pane
/// is focused, the theme and the scale.
type GripKey = (Size<i32, Logical>, bool, Theme, f64);

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
    /// The parts over it last worked out (see `parts`).
    more: Vec<Part>,
}

/// A toast's overlay: the toast, its width, what of it is lit (its
/// button or its "×"), the theme and the scale.
type ToastOverlay = Overlay<(ToastView, i32, Option<ToastSpot>, Theme, f64)>;

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
            more: Vec::new(),
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
    ) -> Option<Part> {
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
    ) -> Option<Part> {
        self.fade_part(view, now, dur, slide, draw)
            .map(|(part, ..)| part)
    }

    /// `part`, and `more` parts over it (a list's rows, say: worked out
    /// only while the view is there), front to back, going up and fading
    /// with it. Those last worked out fade out with it.
    fn parts(
        &mut self,
        view: Placed<K>,
        now: Instant,
        slide: bool,
        draw: impl FnOnce(&K) -> MemoryRenderBuffer,
        more: impl FnOnce(&K) -> Vec<Part>,
    ) -> Vec<Part> {
        if let Some((key, ..)) = &view {
            self.more = more(key);
        }
        let Some((part, rise, alpha)) = self.fade_part(view, now, FAST, slide, draw) else {
            if !self.present {
                self.more.clear();
            }
            return Vec::new();
        };
        let mut parts: Vec<Part> = self
            .more
            .iter()
            .map(|p| p.clone().with(rise, alpha))
            .collect();
        parts.push(part);
        parts
    }

    /// `part_over`, and how far the part is moved down and its opacity.
    fn fade_part(
        &mut self,
        view: Placed<K>,
        now: Instant,
        dur: std::time::Duration,
        slide: bool,
        draw: impl FnOnce(&K) -> MemoryRenderBuffer,
    ) -> Option<(Part, i32, f32)> {
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
        Some((
            Part::new(buffer, (loc.x, loc.y + rise).into(), size, alpha),
            rise,
            alpha,
        ))
    }
}

/// Every rect the chrome hit-tests, in window coordinates, for `view`.
fn hit_rects(
    view: &ChromeView,
    output: Size<i32, Logical>,
    area: Rect,
    footer_h: i32,
) -> Vec<Rect> {
    let mut rects = Vec::new();
    if let Some(footer) = &view.footer {
        let top = output.h - footer_h;
        let layout = footer_layout(footer, (output.w, footer_h).into());
        rects.extend(
            layout
                .chips
                .iter()
                .chain([&layout.count])
                .map(|r| Rectangle::new(r.loc + Point::from((0, top)), r.size)),
        );
    }
    if let Some(tasks) = &view.tasks {
        let layout = tasks_layout(tasks, output, footer_h);
        rects.push(layout.panel);
        rects.extend(layout.rows.iter().copied());
        rects.extend(layout.controls());
    }

    let sidebar = sidebar_layout_at(
        &view.sidebar.workspaces,
        &view.sidebar.agents,
        view.sidebar_width,
        view.density,
    )
    .with_features(
        view.sidebar.features.len(),
        view.sidebar_width,
        view.density,
    )
    .scrolled_over(
        view.sidebar.state.scroll,
        view.sidebar.state.over,
        output.h - footer_h,
    );
    rects.extend([sidebar.header, sidebar.plus, sidebar.repos]);
    for row in &sidebar.rows {
        rects.extend([row.rect, row.close]);
        rects.extend(
            [row.chip, row.errors, row.fold, row.ci, row.worktrees]
                .into_iter()
                .flatten(),
        );
    }
    rects.extend(sidebar.cards.iter().flatten().map(|(_, rect)| *rect));
    rects.extend(sidebar.inbox_title);
    rects.extend(sidebar.inbox.iter().map(|(_, _, rect)| *rect));
    rects.extend(sidebar.features_title);
    rects.extend(sidebar.features.iter().copied());
    let strip_w = output.w - view.sidebar_width;
    let shift = |rect: Rect| {
        Rectangle::new(
            (rect.loc.x + view.sidebar_width, rect.loc.y).into(),
            rect.size,
        )
    };
    let strip = &view.tab_strip;
    let strip_size = (strip_w, tab_strip_height(view.density)).into();
    let layout = strip.layout(strip_size);
    rects.extend(
        layout
            .tabs
            .iter()
            .chain(&layout.closes)
            .filter(|r| r.size.w > 0)
            .map(|r| shift(*r)),
    );
    if layout.plus.size.w > 0 {
        rects.push(shift(layout.plus));
    }
    rects.extend(
        layout
            .more
            .into_iter()
            .chain(layout.overflow)
            .chain(layout.prefix)
            .chain(layout.watch)
            .chain(layout.inbox)
            .chain(layout.problems)
            .map(shift),
    );
    rects.push(shift(layout.toggle));
    for bar in view.pane_bars.iter().filter(|bar| bar.visible) {
        rects.push(bar.rect);
        rects.extend(
            pane_bar_buttons(bar.rect, bar.buttons, bar.zoom)
                .into_iter()
                .map(|(_, rect)| rect),
        );
    }
    rects.extend(view.unzoom);
    if let Some(menu) = &view.context_menu {
        rects.extend(menu_layout(menu.anchor, &menu.rows, output).1);
    }
    let toasts = toast_layout(&view.toasts, toast_area(area, view.unzoom));
    for (layout, _) in toasts.iter().zip(&view.toasts).filter(|(_, t)| !t.leaving) {
        rects.push(layout.panel);
        rects.extend(layout.button);
    }
    for placeholder in &view.placeholders {
        let layout = placeholder_layout(placeholder.rect, placeholder.timed_out);
        rects.extend(layout.retry.into_iter().chain(layout.close));
    }
    // A scrolled list's rows, as far as the list shows them.
    let within = |list: Rect| move |rect: Rect| rect.intersection(list);
    if let Some(switcher) = &view.switcher {
        let layout = switcher_geometry(switcher, area);
        rects.extend(layout.rows.into_iter().filter_map(within(layout.list)));
        rects.extend(layout.chips);
    }
    if let Some(popup) = &view.worktrees {
        let layout = worktrees_layout(popup, output);
        for (rect, buttons) in layout.entries {
            rects.extend(within(layout.list)(rect));
            rects.extend(
                buttons
                    .into_iter()
                    .filter_map(|(_, rect)| within(layout.list)(rect)),
            );
        }
    }
    if let Some(panel) = &view.repos {
        let layout = repos_layout(panel, output);
        for (line, check, expander) in layout.lines {
            rects.extend(
                [line, check]
                    .into_iter()
                    .filter_map(within(layout.tree_list)),
            );
            rects.extend(expander.and_then(within(layout.tree_list)));
        }
        rects.extend(layout.buttons.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.detail_tabs.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.tree_tools.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.chips.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.more);
        rects.push(layout.splitter);
        rects.push(layout.field);
        rects.push(layout.branch);
        if let Some((menu, rows)) = layout.branch_menu {
            rects.push(menu);
            match layout.branch_list {
                Some(list) => rects.extend(rows.into_iter().filter_map(within(list))),
                None => rects.extend(rows),
            }
        }
    }
    if let Some(panel) = &view.inbox {
        let layout = inbox_layout(panel, output);
        for (rect, buttons) in layout.rows {
            rects.extend(within(layout.window)(rect));
            rects.extend(buttons.into_iter().filter_map(within(layout.window)));
        }
        rects.extend(layout.scope.into_iter().map(|(_, rect)| rect));
        rects.extend(layout.detail_buttons);
        rects.push(layout.query);
        rects.extend(layout.hints.into_iter().map(|(_, rect)| rect));
    }
    if let Some(panel) = &view.scrollback {
        let layout = scrollback_layout(panel.panel, &panel.hints);
        rects.push(layout.list);
        rects.extend(layout.hints.into_iter().map(|(_, rect)| rect));
    }
    if let Some(restart) = &view.restart {
        let layout = restart_layout(output, restart.rows.len());
        rects.extend(layout.rows);
        rects.extend(layout.buttons);
    }
    if let Some(checkout) = &view.checkout {
        rects.extend(checkout_layout(output, checkout.rows.len()).buttons);
    }
    if let Some(confirm) = &view.confirm {
        let lines = confirm_body_lines(&confirm.body);
        rects.extend(confirm_lines_layout(output, "", lines).buttons);
    }
    if let Some(quit) = &view.quit {
        let quit = quit_layout(output, quit.rows.len(), quit.switch_to.is_some());
        rects.extend(quit.rows);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_dims_all_under_it_as_far_as_it_has_faded_in() {
        let p = Theme::Dark.palette();
        let output: Size<i32, Logical> = (1920, 1080).into();
        let dialog = Part::new(
            &MemoryRenderBuffer::default(),
            (700, 400).into(),
            (520, 280).into(),
            0.5,
        );
        let parts = scrimmed(Some(dialog), output, p);
        // The dialog in front, then the scrim over the whole output.
        assert_eq!(parts.len(), 2);
        assert!(!parts[0].stretched && parts[1].stretched);
        assert_eq!(parts[1].size, output);
        assert_eq!(parts[1].alpha, p.scrim * 0.5);
        assert!(scrimmed(None, output, p).is_empty());
    }
}
