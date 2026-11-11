//! Files kept by their content, compressed: `blobs/<aa>/<sha256>.zst`. The
//! same content is kept once.

use std::{io, path::Path};

use super::{
    BlobRef,
    zst::{self, BLOB_LEVEL},
};

/// Keep `bytes` in feature folder `dir`, unless they are there already.
pub fn put(dir: &Path, bytes: &[u8]) -> io::Result<BlobRef> {
    let blob = BlobRef::of(bytes);
    let path = blob.path(dir);
    if path.is_file() {
        return Ok(blob);
    }
    super::write::atomic(&path, &zst::compress(bytes, BLOB_LEVEL)?)?;
    Ok(blob)
}

/// The content of `blob` in feature folder `dir`.
#[cfg(test)]
pub fn get(dir: &Path, blob: &BlobRef) -> io::Result<Vec<u8>> {
    let raw = zst::read(&blob.path(dir))?;
    if raw.len() as u64 != blob.len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("blob {} is cut short", blob.sha),
        ));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_blob_is_kept_once_and_read_back() {
        let dir = super::super::write::tests::scratch("blobs");
        let blob = put(&dir, b"plan v1").unwrap();
        let path = blob.path(&dir);
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(put(&dir, b"plan v1").unwrap(), blob);
        let again = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(first, again, "written twice");
        assert_eq!(get(&dir, &blob).unwrap(), b"plan v1");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let folder = std::fs::metadata(path.parent().unwrap()).unwrap();
        assert_eq!(folder.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn a_missing_blob_is_an_error() {
        let dir = super::super::write::tests::scratch("blobs-missing");
        assert!(get(&dir, &BlobRef::of(b"never kept")).is_err());
    }
}
