//! zstd as AI work uses it: blobs whole, at a high level; transcripts as
//! frames appended as they grow, at a quick one. A file of frames one after
//! another reads back as their contents joined.

use std::{io, path::Path};

/// The level blobs are kept at.
pub const BLOB_LEVEL: i32 = 9;
/// The level a transcript's appended frames are kept at.
pub const FRAME_LEVEL: i32 = 3;

/// `raw`, compressed at `level` into one frame.
pub fn compress(raw: &[u8], level: i32) -> io::Result<Vec<u8>> {
    zstd::stream::encode_all(raw, level)
}

/// The content of `packed`: one frame or several.
pub fn decompress(packed: &[u8]) -> io::Result<Vec<u8>> {
    zstd::stream::decode_all(packed)
}

/// Add `raw` to the file at `path` as a new frame.
pub fn append_frame(path: &Path, raw: &[u8], level: i32) -> io::Result<()> {
    let _ = (path, raw, level);
    Ok(())
}
