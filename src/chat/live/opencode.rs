//! OpenCode's TUI plugin, for live word (see `super`): the plugin
//! (`assets/opencode/gerdr-live/tui.js`) runs in each TUI, in its pane, and
//! tells gerdr; `files` puts it in OpenCode's config.
//!
//! Wave 2 C fills this in; the signatures are the contract.

use std::path::{Path, PathBuf};

/// What to write under `config` (`~/.config/opencode`) for the plugin to
/// load, each path with a merge of what it holds now: the plugin's file,
/// and `cli.json` naming it.
#[allow(clippy::type_complexity)]
pub fn files(config: &Path) -> Vec<(PathBuf, Box<dyn FnOnce(Option<&str>) -> Result<String, String>>)> {
    let _ = config;
    Vec::new()
}
