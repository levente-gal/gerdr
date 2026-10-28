//! Which projects a gerdr has open, so another gerdr leaves them be unless
//! started with `--attach`.
//!
//! Each gerdr holds a shared `flock` on `<dir>/<project id>.lock` for the
//! project it shows. The kernel lets go of it when gerdr ends, even by a
//! crash, so no lock outlives its gerdr. A project is open elsewhere when an
//! exclusive lock can't be had; gerdr's own shared lock counts too, so
//! callers leave out the project they show themselves.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The folder of the locks: `$XDG_RUNTIME_DIR/gerdr/projects`; none
/// without a runtime folder, and then no project counts as open elsewhere.
pub fn dir() -> Option<PathBuf> {
    Some(crate::herdr::runtime_dir()?.join("gerdr/projects"))
}

/// The lock of the project shown, held until dropped.
#[derive(Debug)]
pub struct ProjectLock {
    id: String,
    _file: File,
}

impl ProjectLock {
    /// Hold project `id`'s lock in `dir`, shared: `--attach` gerdrs hold it
    /// alongside.
    pub fn take(dir: &Path, id: &str) -> io::Result<Self> {
        let file = open(dir, id)?;
        flock(&file, libc::LOCK_SH | libc::LOCK_NB)?;
        Ok(Self {
            id: id.to_owned(),
            _file: file,
        })
    }

    /// The project it is the lock of.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Whether a gerdr holds project `id`'s lock in `dir`, this one included.
pub fn is_open_elsewhere(dir: &Path, id: &str) -> bool {
    let Ok(file) = open(dir, id) else {
        return false;
    };
    // Closing the file lets go of the lock just taken.
    flock(&file, libc::LOCK_EX | libc::LOCK_NB).is_err()
}

fn open(dir: &Path, id: &str) -> io::Result<File> {
    std::fs::create_dir_all(dir)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("{id}.lock")))
}

fn flock(file: &File, op: libc::c_int) -> io::Result<()> {
    // Safety: the fd is open for as long as `file` lives.
    match unsafe { libc::flock(file.as_raw_fd(), op) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gerdr-project-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_held_lock_makes_the_project_open_elsewhere_until_dropped() {
        let dir = temp_dir("held");
        assert!(!is_open_elsewhere(&dir, "aft"));
        let lock = ProjectLock::take(&dir, "aft").unwrap();
        assert!(is_open_elsewhere(&dir, "aft"));
        assert!(!is_open_elsewhere(&dir, "other"));
        drop(lock);
        assert!(!is_open_elsewhere(&dir, "aft"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attached_gerdrs_hold_the_lock_together() {
        let dir = temp_dir("shared");
        let first = ProjectLock::take(&dir, "aft").unwrap();
        let second = ProjectLock::take(&dir, "aft").unwrap();
        drop(first);
        assert!(is_open_elsewhere(&dir, "aft"));
        drop(second);
        assert!(!is_open_elsewhere(&dir, "aft"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
