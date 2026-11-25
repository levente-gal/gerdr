//! Images pasted into an agent's chat, on the desktop or the phone: a
//! terminal agent can't take an image's bytes, so gerdr saves the image to a
//! file and the reply carries its path, which every agent reads.
//! shortcut: images are never pruned; add pruning by age if the folder grows.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// The image mime types gerdr pastes into a chat, the most wanted first.
pub const MIMES: [&str; 2] = ["image/png", "image/jpeg"];

/// The folder of pasted images.
pub fn dir() -> PathBuf {
    crate::projects::data_dir().join("chat-images")
}

/// Save image `bytes` (a PNG or a JPEG) to a new file in the pasted
/// images' folder; where it went.
pub fn save(bytes: &[u8]) -> Result<PathBuf, String> {
    save_in(&dir(), bytes)
}

/// The file extension of image `bytes`, by their first bytes; `None` for
/// anything but a PNG or a JPEG.
fn extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else {
        None
    }
}

fn save_in(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let ext = extension(bytes).ok_or("it isn't a PNG or a JPEG")?;
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    // Named by the time; two in the same millisecond take the next one.
    let mut at = crate::ai_work::now_ms();
    loop {
        let path = dir.join(format!("{at}.{ext}"));
        match std::fs::File::create_new(&path) {
            Ok(mut file) => {
                return file
                    .write_all(bytes)
                    .map(|()| path.clone())
                    .map_err(|err| format!("{}: {err}", path.display()));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => at += 1,
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_and_a_jpeg_are_saved_by_kind_and_anything_else_is_refused() {
        let dir = std::env::temp_dir().join(format!("gerdr-chat-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let png = b"\x89PNG\r\n\x1a\nrest";
        let a = save_in(&dir, png).unwrap();
        let b = save_in(&dir, png).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&a).unwrap(), png);
        let jpg = save_in(&dir, &[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        assert_eq!(jpg.extension().unwrap(), "jpg");
        assert!(save_in(&dir, b"GIF89a").is_err());
        assert!(save_in(&dir, b"").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
