//! The screenshot viewer: a Manual test criterion's screenshots one at a
//! time over everything, opened from its row on Manual test's page (a click
//! on one, or Alt+I for its first). Left / Right go through them (also its
//! Previous and Next buttons); Del (its Delete button) asks to delete the
//! one showing; Esc, Alt+I, its Close button or a press off its buttons
//! close it.

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
    /// The feature they are of, by project and slug, and the criterion's id.
    feature: (String, String),
    id: String,
    /// What they are of: the criterion's id and title.
    title: String,
    hover: Option<ShotViewerHit>,
}

impl Gerdr {
    /// Open the viewer on screenshot `n` of criterion `i` of the feature on
    /// the tab.
    pub(crate) fn open_shot_viewer(&mut self, i: usize, n: usize) {
        let Some((feature, row)) = (self.features.panel_feature()).and_then(|f| {
            let row = self.criterion_rows(f).into_iter().nth(i)?;
            Some(((f.project.clone(), f.slug.clone()), row))
        }) else {
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
            feature,
            id: row.id,
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
            Some(ShotViewerHit::Delete) => self.ask_delete_shot(),
            Some(ShotViewerHit::Close) | None => self.close_shot_viewer(),
        }
    }

    /// A key while it is open: Left / Right (`h` / `l`) step through the
    /// screenshots, going round; Del asks to delete the one showing; Esc
    /// and Alt+I close it.
    pub(crate) fn shot_viewer_key(&mut self, mods: &ModifiersState, sym: Keysym) {
        let bare = !(mods.ctrl || mods.alt || mods.shift || mods.logo);
        match sym {
            Keysym::Left | Keysym::h if bare => self.shot_step(-1),
            Keysym::Right | Keysym::l if bare => self.shot_step(1),
            Keysym::Delete if bare => self.ask_delete_shot(),
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

    /// Ask to delete the screenshot showing: its file goes, and it goes
    /// from its criterion's failure.
    fn ask_delete_shot(&mut self) {
        let Some(viewer) = self.overlays.shot_viewer.as_ref() else {
            return;
        };
        let Some(path) = viewer.shots.get(viewer.at) else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { return };
        let (project, slug) = viewer.feature.clone();
        let id = viewer.id.clone();
        self.ask(
            "Delete this screenshot?".into(),
            format!("{name} goes from {id}'s failure and from disk. This can't be undone."),
            "Delete",
            crate::confirm::Then::DeleteShot {
                project,
                slug,
                id,
                name,
            },
        );
    }

    /// Delete screenshot `name` of criterion `id` of feature `slug` of
    /// `project`: off its failure, then its file; the viewer goes on with
    /// the others, or closes when none is left.
    pub(crate) fn delete_shot(&mut self, project: &str, slug: &str, id: &str, name: &str) {
        if self.feature_writer(project, slug).is_some() {
            return self.take_feature(project, slug, None);
        }
        let dropped = (self.feature_mut(project, slug))
            .is_some_and(|f| crate::feature::manual::drop_shot(&mut f.manual, id, name));
        if !dropped {
            return self.show_warning(format!("{id} no longer has {name}"));
        }
        self.save_feature(project, slug);
        let dir = crate::feature::store::dir(None, project, slug);
        let path = dir.join(crate::feature::manual::SHOTS_DIR).join(name);
        match std::fs::remove_file(&path) {
            // A file already gone is as good as deleted.
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("deleting screenshot {}: {err}", path.display());
                self.show_error(format!(
                    "{name} is off {id}, but gerdr couldn't delete its file: {err}"
                ));
            }
            _ => self.show_success(format!("Screenshot {name} deleted")),
        }
        if let Some(viewer) = self.overlays.shot_viewer.as_mut() {
            viewer.shots.retain(|p| p != &path);
            viewer.at = viewer.at.min(viewer.shots.len().saturating_sub(1));
            if viewer.shots.is_empty() {
                self.close_shot_viewer();
            }
        }
    }
}
