//! Getting a herdr pane's shell ready: activate the repository's Python venv,
//! bring back `mise` version overrides, and (when the pane comes back after
//! its server restarted) run again what ran in it. Each pane gets one line,
//! typed in once its shell waits at the prompt, so it runs after the shell's
//! rc files (and `mise activate`) and the venv's `bin` comes first in PATH.

#![allow(dead_code)] // contract: implemented by the pane preparation work

use std::path::{Path, PathBuf};

use crate::projects::VenvSetting;

/// What to do in one pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prep {
    pub pane_id: String,
    /// The pane's folder.
    pub pane_cwd: PathBuf,
    /// The workspace's folder: `find_venv` looks no higher.
    pub ws_dir: PathBuf,
    pub venv: VenvSetting,
    /// The environment captured from what ran in the pane
    /// (`PaneRecord::env`): its `VIRTUAL_ENV` wins over `venv`.
    pub captured_env: Vec<(String, String)>,
    /// Where to `cd` first, if the command ran elsewhere than `pane_cwd`.
    pub cd: Option<PathBuf>,
    /// What to run, after a restart.
    pub command: Option<String>,
}

/// The venv a shell in `pane_cwd` uses: the first `.venv` or `venv` folder
/// with a `bin/activate`, looking from `pane_cwd` up to `ws_dir`.
pub fn find_venv(pane_cwd: &Path, ws_dir: &Path) -> Option<PathBuf> {
    let _ = (pane_cwd, ws_dir);
    todo!("pane preparation")
}

/// The line to type into the pane, or `None` if there's nothing to do. It
/// starts with a space, so shells ignoring such lines keep it out of their
/// history.
pub fn prepare_line(prep: &Prep) -> Option<String> {
    let _ = prep;
    todo!("pane preparation")
}

/// Prepare `preps` on a thread of their own, talking to the server on
/// `socket`: each pane gets its line once its shell is idle at its prompt.
/// A pane already running something is left alone.
pub fn spawn_prepare(socket: PathBuf, preps: Vec<Prep>) {
    let _ = (socket, preps);
    todo!("pane preparation")
}
