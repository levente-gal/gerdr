//! The screenshots of a manual test as the feature tab's page shows them:
//! each one's size, read from its PNG's header, and its picture at the size
//! it is drawn, both kept a while so a page drawn again reads no file.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use resvg::tiny_skia;

use crate::app_icons::AppIcon;

/// How many pictures at a size drawn are kept: a page's thumbnails and a
/// few opened ones.
const KEEP_SCALED: usize = 24;
/// How many screenshots are kept whole, to draw again at another size: one
/// is some 15 MB.
const KEEP_WHOLE: usize = 2;

/// Pictures kept by what they are of, the last drawn first.
type Kept<K> = LazyLock<Mutex<Vec<(K, AppIcon)>>>;

/// Each PNG's width and height, `None` for one that can't be read.
type Sizes = HashMap<PathBuf, Option<(u32, u32)>>;

static SIZES: LazyLock<Mutex<Sizes>> = LazyLock::new(Default::default);
/// By file and the longer side's pixels.
static SCALED: Kept<(PathBuf, usize)> = LazyLock::new(Default::default);
static WHOLE: Kept<PathBuf> = LazyLock::new(Default::default);

/// The width and height of the PNG at `path`; `None` when it can't be read.
pub fn size(path: &Path) -> Option<(u32, u32)> {
    if let Some(size) = SIZES.lock().ok()?.get(path) {
        return *size;
    }
    let size = read_size(path);
    if size.is_none() {
        tracing::warn!("can't read the screenshot {}", path.display());
    }
    SIZES.lock().ok()?.insert(path.to_owned(), size);
    size
}

/// A PNG's width and height, from its header: 8 bytes of signature, then
/// the IHDR chunk's length and name, then the two as big-endian words.
fn read_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    let word = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    Some((word(16), word(20)))
}

/// The screenshot at `path` scaled so its longer side is `px` pixels;
/// `None` when it can't be read.
// shortcut: read and decoded on the paint thread, some tens of ms the first
// time one shows; move to a worker if pages with many screenshots stutter.
pub fn picture(path: &Path, px: usize) -> Option<AppIcon> {
    let key = (path.to_owned(), px);
    if let Some(found) = take(&SCALED, &key) {
        return Some(found);
    }
    let whole = match take(&WHOLE, &key.0) {
        Some(whole) => whole,
        None => {
            let data = std::fs::read(path).ok()?;
            let pixmap = tiny_skia::Pixmap::decode_png(&data).ok()?;
            let whole = AppIcon::from_rgba(
                pixmap.width() as usize,
                pixmap.height() as usize,
                pixmap.take(),
            );
            keep(&WHOLE, key.0.clone(), whole.clone(), KEEP_WHOLE);
            whole
        }
    };
    let (w, h, rgba) = whole.scaled(px);
    let scaled = AppIcon::from_rgba(w, h, rgba);
    keep(&SCALED, key, scaled.clone(), KEEP_SCALED);
    Some(scaled)
}

/// What `cache` keeps under `key`, moved to the front.
fn take<K: PartialEq>(cache: &Mutex<Vec<(K, AppIcon)>>, key: &K) -> Option<AppIcon> {
    let mut cache = cache.lock().ok()?;
    let at = cache.iter().position(|(k, _)| k == key)?;
    let found = cache.remove(at);
    let icon = found.1.clone();
    cache.insert(0, found);
    Some(icon)
}

/// Keep `icon` under `key` at the front of `cache`, which keeps `most`.
fn keep<K>(cache: &Mutex<Vec<(K, AppIcon)>>, key: K, icon: AppIcon, most: usize) {
    if let Ok(mut cache) = cache.lock() {
        cache.insert(0, (key, icon));
        cache.truncate(most);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pngs_size_comes_from_its_header_and_its_picture_scales_to_fit() {
        let dir = std::env::temp_dir().join(format!("gerdr-shot-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AC-1-1.png");
        let pixmap = tiny_skia::Pixmap::new(40, 20).unwrap();
        pixmap.save_png(&path).unwrap();
        assert_eq!(size(&path), Some((40, 20)));
        let picture = picture(&path, 10).unwrap();
        assert_eq!(picture.size(), (10, 5));
        let gone = dir.join("gone.png");
        assert_eq!(size(&gone), None);
        assert!(super::picture(&gone, 10).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
