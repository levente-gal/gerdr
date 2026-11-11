//! zstd as AI work uses it: blobs whole, at a high level; transcripts as
//! frames appended as they grow, at a quick one. A file of frames one after
//! another reads back as their contents joined.

use std::{io, io::Write as _, path::Path};

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
    // The stream decoder goes on through every frame it meets.
    zstd::stream::decode_all(packed)
}

/// The content of the file at `path`: one frame or several.
#[cfg(test)]
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    decompress(&std::fs::read(path)?)
}

/// Add `raw` to the file at `path` as a new frame, made 0600 when it isn't
/// there. The frame goes in one write, after the others. The file's length
/// after it.
pub fn append_frame(path: &Path, raw: &[u8], level: i32) -> io::Result<u64> {
    let frame = compress(raw, level)?;
    let mut file = super::write::open_append(path)?;
    file.write_all(&frame)?;
    Ok(file.metadata()?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_appended_read_back_joined() {
        let dir = super::super::write::tests::scratch("zst-frames");
        let path = dir.join("t.jsonl.zst");
        append_frame(&path, b"{\"a\":1}\n", FRAME_LEVEL).unwrap();
        append_frame(&path, b"{\"b\":2}\n", FRAME_LEVEL).unwrap();
        append_frame(&path, b"", FRAME_LEVEL).unwrap();
        let len = append_frame(&path, b"{\"c\":3}", FRAME_LEVEL).unwrap();
        assert_eq!(len, std::fs::metadata(&path).unwrap().len());
        assert_eq!(read(&path).unwrap(), b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}");
    }

    #[test]
    fn one_frame_round_trips() {
        let raw = b"the same thing over and over, the same thing over and over";
        let packed = compress(raw, BLOB_LEVEL).unwrap();
        assert_eq!(decompress(&packed).unwrap(), raw);
    }
}
