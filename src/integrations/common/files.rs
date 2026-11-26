//! Reading agents' files: their first lines, how they are now, and
//! subagents' transcripts as they grow.

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead as _, Read as _, Seek as _, SeekFrom},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use crate::chat::parse::Reader;

/// A file as it is: its inode, change time and length.
pub type Stamp = (u64, i64, i64, u64);

pub fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.ino(), meta.mtime(), meta.mtime_nsec(), meta.len()))
}

/// How database `path` and its WAL are now; an error when the database
/// can't be looked at.
pub fn with_wal(path: &Path) -> std::io::Result<[Option<Stamp>; 2]> {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    Ok([Some(stamp(path)?), stamp(Path::new(&wal)).ok()])
}

/// The first line of the file at `path`, up to `most` bytes of it; when
/// `whole`, none until it ends.
pub fn first_line(path: &Path, most: u64, whole: bool) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(file.take(most))
        .read_line(&mut line)
        .ok()?;
    (!whole || line.ends_with('\n')).then_some(line)
}

/// `len` bytes of `file` from `from` on.
pub fn read_at(file: &mut File, from: u64, len: u64) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Where the whole lines of `bytes` end: a last line still being written
/// waits until it is whole.
pub fn whole_end(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1)
}

/// Subagents' own transcripts, apart from their chat's, read as they grow.
#[derive(Default)]
pub struct SubFiles {
    /// How far each is read, by file.
    read: HashMap<PathBuf, u64>,
    /// What the agent noted of each file, kept when the chat is read
    /// again: Claude Code's, the call that started it.
    pub noted: HashMap<PathBuf, String>,
    /// A database of subagents and its WAL (`with_wal`) when last looked
    /// in, with how many were still to find then (the Antigravity CLI's
    /// summaries).
    pub looked: Option<([Option<Stamp>; 2], usize)>,
}

impl SubFiles {
    /// The chat is read again from the start: its subagents too.
    pub fn reset(&mut self) {
        self.read.clear();
        self.looked = None;
    }

    /// The whole lines subagent `agent`'s file `path` grew by, fed to
    /// `reader`; `call` started it. A file is read from its last `chunk`
    /// bytes.
    pub fn read(
        &mut self,
        reader: &mut Reader,
        path: &Path,
        agent: &str,
        call: &str,
        chunk: u64,
    ) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let offset = match self.read.get(path) {
            Some(&at) if at <= len => at,
            _ => len.saturating_sub(chunk),
        };
        if offset == len {
            self.read.insert(path.to_owned(), len);
            return Ok(());
        }
        let bytes = read_at(&mut file, offset, len - offset)?;
        let end = whole_end(&bytes);
        reader.feed_subagent(agent, call, &bytes[..end]);
        self.read.insert(path.to_owned(), offset + end as u64);
        Ok(())
    }
}
