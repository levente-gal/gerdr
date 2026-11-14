//! The screenshot viewer: a Manual test criterion's screenshots one at a
//! time over everything, opened from its row on Manual test's page (a click
//! on one, or Alt+I for its first). Left / Right go through them (also its
//! Previous and Next buttons); Esc, Alt+I, its Close button or a press
//! off its buttons close it.

use std::path::PathBuf;

use smithay::{
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point},
};

use crate::{
    Gerdr,
    chrome::{self, ShotViewerHit},
};

/// The viewer, while it is open.
pub struct ShotViewer {
    /// The criterion's screenshots.
    shots: Vec<PathBuf>,
    /// The one showing.
    at: usize,
    /// What they are of: the criterion's id and title.
    title: String,
    hover: Option<ShotViewerHit>,
}

impl Gerdr {
    /// Open the viewer on screenshot `n` of criterion `i` of the feature on
    /// the tab.
    pub(crate) fn open_shot_viewer(&mut self, i: usize, n: usize) {
        let Some(row) =
            (self.features.panel_feature()).and_then(|f| self.criterion_rows(f).into_iter().nth(i))
        else {
            return;
        };
        if row.shots.is_empty() {
            return self.show_warning(format!("{} has no screenshots", row.id));
        }
        self.select_criterion(i);
        self.overlays.shot_viewer = Some(ShotViewer {
            at: n.min(row.shots.len() - 1),
            shots: row.shots,
            title: format!("{} \u{b7} {}", row.id, row.title),
            hover: None,
        });
    }

    pub(crate) fn close_shot_viewer(&mut self) {
        self.overlays.shot_viewer = None;
    }

    /// What the viewer shows, while it is open.
    pub(crate) fn shot_viewer_view(&self) -> Option<chrome::ShotViewerView> {
        let viewer = self.overlays.shot_viewer.as_ref()?;
        let path = viewer.shots.get(viewer.at)?.clone();
        Some(chrome::ShotViewerView {
            size: crate::shot_images::size(&path),
            path,
            title: viewer.title.clone(),
            at: viewer.at,
            of: viewer.shots.len(),
            hover: viewer.hover,
        })
    }

    fn shot_viewer_layout(&self) -> Option<chrome::ShotViewerLayout> {
        let view = self.shot_viewer_view()?;
        Some(chrome::shot_viewer_layout(&view, self.output_size()))
    }

    /// Its buttons: the pointer is a hand over them.
    pub(crate) fn shot_viewer_controls(&self) -> Vec<chrome::Rect> {
        (self.shot_viewer_layout().into_iter())
            .flat_map(|l| l.buttons.into_iter().map(|(_, r)| r))
            .collect()
    }

    /// The pointer moved to `pos`: the button under it lights up.
    pub(crate) fn shot_viewer_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = (self.shot_viewer_layout()).and_then(|l| chrome::shot_viewer_hit(&l, pos));
        if let Some(viewer) = self.overlays.shot_viewer.as_mut() {
            viewer.hover = hit;
        }
    }

    /// A press at `pos`: what its button says, else it closes.
    pub(crate) fn shot_viewer_click(&mut self, pos: Point<f64, Logical>) {
        match (self.shot_viewer_layout()).and_then(|l| chrome::shot_viewer_hit(&l, pos)) {
            Some(ShotViewerHit::Prev) => self.shot_step(-1),
            Some(ShotViewerHit::Next) => self.shot_step(1),
            Some(ShotViewerHit::Close) | None => self.close_shot_viewer(),
        }
    }

    /// A key while it is open: Left / Right (`h` / `l`) step through the
    /// screenshots, going round; Esc and Alt+I close it.
    pub(crate) fn shot_viewer_key(&mut self, mods: &ModifiersState, sym: Keysym) {
        let bare = !(mods.ctrl || mods.alt || mods.shift || mods.logo);
        match sym {
            Keysym::Left | Keysym::h if bare => self.shot_step(-1),
            Keysym::Right | Keysym::l if bare => self.shot_step(1),
            Keysym::Escape => self.close_shot_viewer(),
            Keysym::i | Keysym::I if mods.alt => self.close_shot_viewer(),
            _ => {}
        }
    }

    /// Show the screenshot `by` from the one showing, going round.
    fn shot_step(&mut self, by: isize) {
        if let Some(viewer) = self.overlays.shot_viewer.as_mut() {
            let n = viewer.shots.len() as isize;
            viewer.at = (viewer.at as isize + by).rem_euclid(n.max(1)) as usize;
        }
    }
}
