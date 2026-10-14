//! Input to the workspace sidebar: presses on its header, the workspace
//! rows and the agent cards, the pointer moving over it (hover, tooltips),
//! its context menus, and what moves in it (see `chrome::sidebar` for what
//! it shows).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use smithay::utils::{Logical, Point};

use crate::{
    Gerdr, Reorderable,
    anim::{FAST, Fade, quantize},
    chrome::{self, CardState, Rect, RowState, SidebarHit, SidebarLayout, SidebarState},
    herdr::AgentStatus,
    input::{BTN_LEFT, BTN_MIDDLE},
    menu::{MenuAction, MenuItem, MenuSpec},
    rename::RenameTarget,
    tooltip::Tooltip,
};

// Nothing reads the sidebar's state yet: `chrome_view` takes it up (see
// the sidebar handoff). Until then these have no caller outside the tests.
/// How long a blocked card's pulse takes to go up and back down.
#[allow(dead_code)]
const PULSE: Duration = Duration::from_millis(1500);

/// What the pointer is on in the sidebar and since when, and what it just
/// left, each with its fade: the hover pills fade in and out.
#[derive(Debug, Default)]
struct Hover {
    over: Option<(SidebarHit, Fade)>,
    leaving: Option<(SidebarHit, Fade)>,
}

impl Hover {
    /// The pointer is on `hit` (nothing with a pill when `None`) at `now`.
    fn track(&mut self, hit: Option<SidebarHit>, now: Instant) {
        if self.over.map(|(h, _)| h) == hit {
            return;
        }
        if let Some((h, fade)) = self.over.take() {
            self.leaving = Some((h, reverse(fade, now)));
        }
        if let Some(hit) = hit {
            // Back on what was being left: carry on from where its fade is.
            let fade = match self.leaving {
                Some((left, fade)) if left == hit => reverse(fade, now),
                _ => Fade::new(now, FAST, true),
            };
            self.over = Some((hit, fade));
        }
    }

    /// How lit `hit`'s pill is at `now`, from 0 to 1.
    #[allow(dead_code)]
    fn alpha(&self, hit: SidebarHit, now: Instant) -> f32 {
        [self.over, self.leaving]
            .into_iter()
            .flatten()
            .find(|(h, _)| *h == hit)
            .map_or(0.0, |(_, fade)| fade.value(now))
    }
}

/// `fade` turned around at `now`, going the other way from where it stands.
fn reverse(fade: Fade, now: Instant) -> Fade {
    let value = fade.value(now);
    let rising = !fade.rising;
    // How far along the new fade already is, by its value.
    let progress = if rising { value } else { 1.0 - value };
    Fade::new(now - fade.dur.mul_f32(progress), fade.dur, rising)
}

thread_local! {
    // The hover state lives here until `Gerdr` has a field for it (see the
    // sidebar handoff); gerdr runs on one thread.
    static HOVER: RefCell<Hover> = RefCell::new(Hover::default());
}

/// Where a blocked card's pulse stands at `now`, from 0 to 1 and back
/// every `PULSE`, as a looping fade from `epoch`.
#[allow(dead_code)]
fn pulse(epoch: Instant, now: Instant) -> f32 {
    let half = PULSE / 2;
    let n = (now.saturating_duration_since(epoch).as_nanos() / half.as_nanos().max(1)) as u32;
    Fade::new(epoch + half * n, half, n.is_multiple_of(2)).value(now)
}

impl Gerdr {
    /// Where the sidebar's parts sit, at the configured density.
    pub(crate) fn sidebar_layout(&self) -> SidebarLayout {
        let agents = self.agent_views();
        let workspaces: Vec<_> = self
            .workspace_views()
            .into_iter()
            .map(|(ws, _)| ws)
            .collect();
        chrome::sidebar_layout_at(&workspaces, &agents, self.sidebar_width, self.density)
    }

    /// Whether something covers the sidebar, taking the pointer from it.
    fn sidebar_covered(&self) -> bool {
        self.quit.is_some()
            || self.welcome.is_some()
            || self.project_menu.is_some()
            || self.switcher.is_some()
            || self.context_menu.is_some()
    }

    /// What is under the pointer in the sidebar, if it is free and there.
    fn sidebar_hit(&self, layout: &SidebarLayout, pos: Point<f64, Logical>) -> Option<SidebarHit> {
        if self.sidebar_covered() || pos.x >= self.sidebar_width as f64 {
            return None;
        }
        layout.hit(pos)
    }

    /// What moves in the sidebar at `now`: the hover pills, the blocked
    /// cards' pulse, the "+" menu, the name being renamed.
    #[allow(dead_code)]
    pub(crate) fn sidebar_state(&self, now: Instant) -> SidebarState {
        let layout = self.sidebar_layout();
        let hit = self.sidebar_hit(&layout, self.pointer);
        let (rows, cards, plus) = HOVER.with(|hover| {
            let mut hover = hover.borrow_mut();
            hover.track(hit.map(SidebarHit::pill), now);
            let rows = (0..layout.rows.len())
                .map(|i| RowState {
                    hover: quantize(hover.alpha(SidebarHit::Row(i), now)),
                    close: hit == Some(SidebarHit::Close(i)),
                })
                .collect();
            let cards = self
                .agent_views()
                .iter()
                .enumerate()
                .map(|(i, cards)| {
                    cards
                        .iter()
                        .enumerate()
                        .map(|(k, card)| CardState {
                            hover: quantize(hover.alpha(SidebarHit::Card(i, k), now)),
                            pulse: if card.status == AgentStatus::Blocked {
                                quantize(pulse(self.start_time, now))
                            } else {
                                0
                            },
                        })
                        .collect()
                })
                .collect();
            let plus = quantize(hover.alpha(SidebarHit::Plus, now));
            (rows, cards, plus)
        });
        let plus_open = self
            .context_menu
            .as_ref()
            .is_some_and(|menu| menu.spec.anchor == layout.plus);
        let rename = self.rename.as_ref().and_then(|r| match r.target {
            RenameTarget::Workspace(w) => Some((w, r.field.line())),
            RenameTarget::Tab(..) => None,
        });
        SidebarState {
            density: self.density,
            rows,
            cards,
            plus,
            plus_open,
            rename,
        }
    }

    /// A press at `(x, y)` in the sidebar. Left click selects a workspace,
    /// "×" or middle click closes it, a double click on its name renames
    /// it; a left click on an agent card shows its pane. A row held with
    /// the left button can be dragged to another place. The "+" opens the
    /// menu of what to add; the project's name left of it opens the
    /// project menu (the welcome screen while no project is shown). A
    /// press anywhere but the name being renamed commits the rename.
    pub(crate) fn sidebar_press(&mut self, x: i32, y: i32, button: u32) {
        let pos = Point::from((x as f64 + 0.5, y as f64 + 0.5));
        let layout = self.sidebar_layout();
        let renaming = match self.rename.as_ref().map(|r| r.target) {
            Some(RenameTarget::Workspace(w)) => layout.rows.get(w).map(|row| row.rename_field()),
            _ => None,
        };
        if let Some(field) = renaming {
            if field.to_f64().contains(pos) {
                return;
            }
            self.commit_rename();
        }
        let Some(hit) = layout.hit(pos) else {
            return;
        };
        match (hit, button) {
            (SidebarHit::Plus, BTN_LEFT) => self.open_menu(self.sidebar_plus_menu(&layout)),
            (SidebarHit::Header, BTN_LEFT) => {
                if self.project.is_some() {
                    self.toggle_project_menu();
                } else {
                    self.open_welcome(true);
                }
            }
            (SidebarHit::Card(i, k), BTN_LEFT) => {
                if let Some(card) = self.agent_views().get(i).and_then(|c| c.get(k)) {
                    let pane_id = card.pane_id.clone();
                    self.focus_agent(&pane_id);
                }
            }
            (SidebarHit::Close(i), BTN_LEFT) => self.close_workspace(i),
            (SidebarHit::Name(i), BTN_LEFT) if self.is_double(pos) => {
                self.start_rename(RenameTarget::Workspace(i));
            }
            (SidebarHit::Row(i) | SidebarHit::Name(i) | SidebarHit::Chip(i), BTN_LEFT) => {
                self.switch_workspace(i);
                self.hold(Reorderable::Workspace);
            }
            (hit, BTN_MIDDLE) => {
                if let Some(i) = hit.row() {
                    self.close_workspace(i);
                }
            }
            _ => {}
        }
    }

    /// The pointer moved to `pos` with nothing open over the sidebar: the
    /// header's project name and the row, card or button under it light
    /// up. The tooltip for what is under the pointer, if it has one.
    pub(crate) fn sidebar_motion(&mut self, pos: Point<f64, Logical>) -> Option<Tooltip> {
        let layout = self.sidebar_layout();
        let hit = self.sidebar_hit(&layout, pos);
        self.header_hover = self.project.is_some() && hit == Some(SidebarHit::Header);
        HOVER.with(|hover| {
            hover
                .borrow_mut()
                .track(hit.map(SidebarHit::pill), Instant::now())
        });
        let workspaces = self.workspace_views();
        let agents = self.agent_views();
        match hit? {
            SidebarHit::Plus => {
                let tip = Tooltip::new("New terminal, folder or project", layout.plus);
                let keys: Vec<_> = ["terminal workspace in home", "open project"]
                    .into_iter()
                    .filter_map(|label| self.key_hint(label))
                    .collect();
                Some(if keys.is_empty() {
                    tip
                } else {
                    tip.key(&keys.join(" / "))
                })
            }
            SidebarHit::Close(i) => {
                let tip = Tooltip::new("Close workspace", layout.rows[i].close);
                Some(match self.key_hint("close workspace") {
                    Some(key) => tip.key(&key),
                    None => tip,
                })
            }
            SidebarHit::Chip(i) => {
                let text = agents
                    .get(i)
                    .map(|cards| {
                        cards
                            .iter()
                            .map(|c| format!("{} {}", c.label(), chrome::status_label(c.status)))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .filter(|t| !t.is_empty())?;
                Some(Tooltip::new(&text, layout.rows[i].chip?))
            }
            SidebarHit::Name(i) => {
                let (ws, row) = (workspaces.get(i)?, layout.rows.get(i)?);
                let fits = chrome::name_fits(&ws.0.name, row.name.size.w);
                (!fits).then(|| Tooltip::new(&ws.0.name, row.name))
            }
            SidebarHit::Card(i, k) => {
                let card = agents.get(i)?.get(k)?;
                let rect = layout.cards[i].iter().find(|(c, _)| *c == k)?.1;
                Some(Tooltip::new(&card.summary(), rect))
            }
            SidebarHit::Header | SidebarHit::Row(_) => None,
        }
    }

    /// The menu a right press at `pos` in the sidebar opens, if any: for
    /// the "+", what to add; for a workspace row, what to do with the
    /// workspace; for an agent card, with the agent.
    pub(crate) fn sidebar_menu(&self, pos: Point<f64, Logical>) -> Option<MenuSpec> {
        let layout = self.sidebar_layout();
        let at = Rect::new((pos.x as i32, pos.y as i32).into(), (0, 0).into());
        match layout.hit(pos)? {
            SidebarHit::Plus => Some(self.sidebar_plus_menu(&layout)),
            SidebarHit::Card(i, k) => {
                let card = self.agent_views().get(i)?.get(k)?.clone();
                let cwd = self
                    .agents
                    .iter()
                    .find(|a| a.pane_id == card.pane_id)
                    .and_then(|a| a.cwd.clone());
                let mut items = vec![MenuItem::new("Focus", MenuAction::FocusAgent(card.pane_id))];
                items.push(match cwd {
                    Some(cwd) => MenuItem::new("Copy path", MenuAction::CopyPath(cwd)),
                    None => MenuItem::new("Copy path", MenuAction::NewTerminal).enabled(false),
                });
                Some(MenuSpec { anchor: at, items })
            }
            hit => {
                let i = hit.row()?;
                let dir = self.workspaces.get(i)?.dir.clone();
                let hinted = |item: MenuItem, label: &str| match self.key_hint(label) {
                    Some(key) => item.hint(&key),
                    None => item,
                };
                Some(MenuSpec {
                    anchor: at,
                    items: vec![
                        hinted(MenuItem::new("New tab", MenuAction::NewTab(i)), "new tab"),
                        MenuItem::new("Rename", MenuAction::RenameWorkspace(i)),
                        MenuItem::new("Copy path", MenuAction::CopyPath(dir)),
                        hinted(
                            MenuItem::new("Close workspace", MenuAction::CloseWorkspace(i)),
                            "close workspace",
                        ),
                    ],
                })
            }
        }
    }

    /// The menu under the header's "+": what to add, with the keys that
    /// do the same.
    pub(crate) fn sidebar_plus_menu(&self, layout: &SidebarLayout) -> MenuSpec {
        let hinted = |item: MenuItem, label: &str| match self.key_hint(label) {
            Some(key) => item.hint(&key),
            None => item,
        };
        MenuSpec {
            anchor: layout.plus,
            items: vec![
                hinted(
                    MenuItem::new("New terminal", MenuAction::NewTerminal),
                    "terminal workspace in home",
                ),
                MenuItem::new("Open folder…", MenuAction::OpenFolder),
                hinted(
                    MenuItem::new("New project…", MenuAction::NewProject),
                    "open project",
                ),
            ],
        }
    }

    /// The key for the action the help panel calls `label`, with the
    /// prefix before it, like "Ctrl+B Shift+T".
    fn key_hint(&self, label: &str) -> Option<String> {
        let help = self.keymap.help();
        help.rows
            .iter()
            .find(|(_, l)| l == label)
            .map(|(keys, _)| format!("{} {keys}", help.prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::{FAST, Fade, Hover, PULSE, pulse, reverse};
    use crate::chrome::SidebarHit;
    use std::time::{Duration, Instant};

    #[test]
    fn a_reversed_fade_keeps_its_value() {
        let start = Instant::now();
        let up = Fade::new(start, FAST, true);
        let at = start + FAST / 4;
        let down = reverse(up, at);
        assert!(!down.rising);
        assert!((down.value(at) - up.value(at)).abs() < 0.01);
        assert_eq!(down.value(at + FAST), 0.0);
    }

    #[test]
    fn hover_fades_in_out_and_back() {
        let start = Instant::now();
        let mut hover = Hover::default();
        hover.track(Some(SidebarHit::Row(0)), start);
        assert_eq!(hover.alpha(SidebarHit::Row(0), start), 0.0);
        assert_eq!(hover.alpha(SidebarHit::Row(0), start + FAST), 1.0);
        assert_eq!(hover.alpha(SidebarHit::Row(1), start + FAST), 0.0);
        // Leaving: the old pill fades out while the new fades in.
        let later = start + 2 * FAST;
        hover.track(Some(SidebarHit::Row(1)), later);
        assert_eq!(hover.alpha(SidebarHit::Row(0), later), 1.0);
        assert_eq!(hover.alpha(SidebarHit::Row(0), later + FAST), 0.0);
        assert_eq!(hover.alpha(SidebarHit::Row(1), later + FAST), 1.0);
        // Back onto the one half faded out: it comes back from there.
        let mid = later + 2 * FAST;
        hover.track(Some(SidebarHit::Row(0)), mid);
        let half = mid + FAST / 2;
        hover.track(None, half);
        let v = hover.alpha(SidebarHit::Row(0), half);
        assert!(v > 0.4 && v < 0.6, "{v}");
        hover.track(Some(SidebarHit::Row(0)), half);
        assert!((hover.alpha(SidebarHit::Row(0), half) - v).abs() < 0.01);
        assert_eq!(hover.alpha(SidebarHit::Row(0), half + FAST), 1.0);
    }

    #[test]
    fn the_pulse_loops() {
        let epoch = Instant::now();
        assert_eq!(pulse(epoch, epoch), 0.0);
        assert_eq!(pulse(epoch, epoch + PULSE / 2), 1.0);
        assert!(pulse(epoch, epoch + PULSE) < 0.01);
        let quarter = pulse(epoch, epoch + PULSE / 4);
        assert!(quarter > 0.45 && quarter < 0.55, "{quarter}");
        let three = pulse(epoch, epoch + PULSE * 3 / 4);
        assert!(three > 0.45 && three < 0.55, "{three}");
        assert!(pulse(epoch, epoch + PULSE * 10 + Duration::from_millis(750)) > 0.99);
    }
}
