//! What starting an agent CLI shares: its flags, as most take them.

use std::path::PathBuf;

/// Command `line` allowed into `dirs` too, one `--add-dir` each.
pub fn add_dirs(mut line: String, dirs: &[PathBuf]) -> String {
    for dir in dirs {
        line.push_str(" --add-dir ");
        line.push_str(&crate::desktop::shell_quote(&dir.to_string_lossy()));
    }
    line
}
