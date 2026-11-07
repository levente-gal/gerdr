//! The index: `index.sqlite` under `ai-work`, drawn from the events files
//! for the stats panel. It is never the only copy of anything: gone, or of
//! another `user_version`, it is drawn again.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// The version of the tables (see `schema.sql`); `user_version` is
/// `SCHEMA * 1000 + FORMAT`.
pub const SCHEMA: u32 = 1;

/// The tables, as made in a new index.
pub const TABLES: &str = include_str!("schema.sql");

/// The index's file under `root`.
pub fn path(root: &Path) -> PathBuf {
    root.join("index.sqlite")
}

/// The index under `root`, to write to; made, or made again, as needed.
pub fn open(root: &Path) -> Result<Connection, String> {
    Err(format!("{} isn't kept yet", path(root).display()))
}

/// The index under `root`, only to read.
pub fn open_read(root: &Path) -> Result<Connection, String> {
    Err(format!("{} isn't kept yet", path(root).display()))
}

/// Take what was added to the events files of feature folders `dirs` since
/// last time into `conn`: the number of lines.
pub fn ingest(conn: &mut Connection, dirs: &[PathBuf]) -> Result<usize, String> {
    let _ = (conn, dirs);
    Ok(0)
}
