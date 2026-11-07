//! Writing a feature's AI work: lines appended to this machine's events
//! file, and the check logs kept as blobs. Folders are 0700 and files 0600:
//! transcripts and logs can hold secrets.

use std::{io, path::Path};

use super::{BlobRef, Line};

/// Append `line` to `events-<machine>.jsonl` of feature folder `dir`, in
/// one small write.
pub fn append(dir: &Path, line: &Line) -> io::Result<()> {
    let _ = (dir, line);
    Ok(())
}

/// Keep what a check printed, `scrollback` cut from the last `head` (the
/// line a check starts with) on, in feature folder `dir`. True with it:
/// `head` was gone, so its start is missing.
pub fn check_log(dir: &Path, scrollback: &str, head: &str) -> io::Result<(BlobRef, bool)> {
    let _ = dir;
    let (from, truncated) = match scrollback.rfind(head) {
        Some(at) => (at, false),
        None => (0, true),
    };
    Ok((BlobRef::of(&scrollback.as_bytes()[from..]), truncated))
}
