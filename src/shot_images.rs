//! Pictures on disk as gerdr's pages show them: a manual test's
//! screenshots and a doc's images. Each one's size, read from its file's
//! header, and its picture at the size it is drawn, both kept a while so a
//! page drawn again reads no file. PNG, JPEG, WebP, GIF (its first frame)
//! and SVG are read, picked by the file's extension.

use std::{
    cell::Cell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

use resvg::{tiny_skia, usvg};

use crate::app_icons::AppIcon;

/// How many pictures at a size drawn are kept: a page's thumbnails and a
/// few opened ones.
const KEEP_SCALED: usize = 24;
/// How many screenshots are kept whole, to draw again at another size: one
/// is some 15 MB.
const KEEP_WHOLE: usize = 2;

/// Pictures kept by what they are of, the last drawn first.
type Kept<K> = LazyLock<Mutex<Vec<(K, AppIcon)>>>;

/// Each picture's width and height, `None` for one that can't be read.
type Sizes = HashMap<PathBuf, Option<(u32, u32)>>;

static SIZES: LazyLock<Mutex<Sizes>> = LazyLock::new(Default::default);
/// By file and the longer side's pixels.
static SCALED: Kept<Key> = LazyLock::new(Default::default);
static WHOLE: Kept<PathBuf> = LazyLock::new(Default::default);

/// The width and height of the picture at `path`; `None` when it can't be read.
pub fn size(path: &Path) -> Option<(u32, u32)> {
    format(path)?;
    if let Some(size) = SIZES.lock().ok()?.get(path) {
        return *size;
    }
    let size = read_size(path);
    if size.is_none() {
        tracing::warn!("can't read the picture {}", path.display());
    }
    SIZES.lock().ok()?.insert(path.to_owned(), size);
    size
}

/// The picture formats read, by their files' extensions.
pub const FORMATS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// The format of the picture at `path`, by its extension in lower case.
fn format(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    FORMATS.contains(&ext.as_str()).then_some(ext)
}

/// A picture's width and height, from its file's header; an SVG's from
/// its document, rounded to whole pixels.
fn read_size(path: &Path) -> Option<(u32, u32)> {
    let open = || std::fs::File::open(path).ok().map(std::io::BufReader::new);
    match format(path)?.as_str() {
        "png" => png_size(path),
        "jpg" | "jpeg" => {
            let mut jpeg = zune_jpeg::JpegDecoder::new(open()?);
            jpeg.decode_headers().ok()?;
            let info = jpeg.info()?;
            Some((u32::from(info.width), u32::from(info.height)))
        }
        "gif" => {
            let gif = gif::DecodeOptions::new().read_info(open()?).ok()?;
            Some((u32::from(gif.width()), u32::from(gif.height())))
        }
        "webp" => Some(image_webp::WebPDecoder::new(open()?).ok()?.dimensions()),
        _ => {
            let size = svg(path)?.size();
            let px = |side: f32| side.round().max(1.0) as u32;
            Some((px(size.width()), px(size.height())))
        }
    }
}

/// A PNG's width and height, from its header: 8 bytes of signature, then
/// the IHDR chunk's length and name, then the two as big-endian words.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    let word = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    Some((word(16), word(20)))
}

fn svg(path: &Path) -> Option<usvg::Tree> {
    let data = std::fs::read(path).ok()?;
    let mut options = usvg::Options::default();
    // Only an SVG with text waits for the system's fonts to be read.
    if data.windows(5).any(|w| w == b"<text") {
        options.fontdb = SVG_FONTS.clone();
        options.font_resolver.select_fallback = Box::new(fallback);
    }
    usvg::Tree::from_data(&data, &options).ok()
}

/// The font to draw `c` in when the fonts in `used` lack it. usvg's own
/// pick is the first font on the system that has it (Comfortaa for an
/// arrow here); this walks cosmic-text's lists, the ones gerdr's own text
/// falls back through, keeping the style and nearest weight.
// shortcut: usvg draws the whole text in the fallback font when that font
// has all of it, so a serif line with an arrow turns Noto Sans; per-glyph
// fallback needs usvg's shaping changed.
fn fallback(
    c: char,
    used: &[usvg::fontdb::ID],
    fonts: &mut std::sync::Arc<usvg::fontdb::Database>,
) -> Option<usvg::fontdb::ID> {
    use cosmic_text::Fallback;
    use unicode_script::UnicodeScript;
    let base = fonts.face(*used.first()?)?;
    let lists = cosmic_text::PlatformFallback;
    let locale = std::env::var("LANG").unwrap_or_default();
    let mut names = lists
        .script_fallback(c.script(), &locale)
        .iter()
        .chain(lists.common_fallback());
    let picked = names.find_map(|name| {
        fonts
            .faces()
            .filter(|face| {
                !used.contains(&face.id)
                    && face.families.iter().any(|(family, _)| family == name)
                    && has_char(fonts, face.id, c)
            })
            .min_by_key(|face| {
                (
                    face.style != base.style,
                    face.weight.0.abs_diff(base.weight.0),
                )
            })
            .map(|face| face.id)
    });
    picked.or_else(|| (usvg::FontResolver::default_fallback_selector())(c, used, fonts))
}

/// Whether the font `id` has a glyph for `c`.
fn has_char(fonts: &usvg::fontdb::Database, id: usvg::fontdb::ID, c: char) -> bool {
    use cosmic_text::skrifa::{FontRef, MetadataProvider};
    fonts
        .with_face_data(id, |data, index| {
            FontRef::from_index(data, index).ok()?.charmap().map(c)
        })
        .flatten()
        .is_some()
}

/// Read the fonts for an SVG's text on a thread of its own, some 100 ms (most
/// of it fc-match), so the first page with one doesn't stall a frame.
pub fn warm() {
    let warm = std::thread::Builder::new().name("svg fonts".into());
    if let Err(err) = warm.spawn(|| LazyLock::force(&SVG_FONTS)) {
        tracing::warn!("reading SVG fonts ahead: {err}");
    }
}

/// The system's fonts for an SVG's text, read once (`warm`).
static SVG_FONTS: LazyLock<std::sync::Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    use usvg::fontdb::Database;
    let mut fonts = Database::new();
    fonts.load_system_fonts();
    // fontdb's own names for serif, sans-serif… (Times New Roman, Arial…)
    // may not be installed; fontconfig says what stands for them here.
    let generics = [
        (
            "serif",
            Database::set_serif_family as fn(&mut Database, String),
        ),
        ("sans-serif", Database::set_sans_serif_family),
        ("monospace", Database::set_monospace_family),
        ("cursive", Database::set_cursive_family),
        ("fantasy", Database::set_fantasy_family),
    ];
    for (generic, set) in generics {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", generic])
            .output();
        match out {
            Err(e) => {
                tracing::warn!("can't run fc-match for SVG fonts: {e}");
                break;
            }
            Ok(out) => {
                if let Ok(name) = String::from_utf8(out.stdout)
                    && out.status.success()
                    && !name.is_empty()
                {
                    set(&mut fonts, name);
                }
            }
        }
    }
    std::sync::Arc::new(fonts)
});

/// The raster picture at `path` whole, its pixels premultiplied.
// shortcut: a JPEG's EXIF rotation isn't applied, so a phone photo may lie
// on its side; read the Orientation tag if docs show photos.
fn decode(path: &Path) -> Option<AppIcon> {
    let data = std::fs::read(path).ok()?;
    let (w, h, mut rgba) = match format(path)?.as_str() {
        "png" => {
            let pixmap = tiny_skia::Pixmap::decode_png(&data).ok()?;
            let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
            return Some(AppIcon::from_rgba(w, h, pixmap.take()));
        }
        "jpg" | "jpeg" => {
            let options = zune_jpeg::zune_core::options::DecoderOptions::default()
                .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
            let cursor = zune_jpeg::zune_core::bytestream::ZCursor::new(&data);
            let mut jpeg = zune_jpeg::JpegDecoder::new_with_options(cursor, options);
            let rgba = jpeg.decode().ok()?;
            let info = jpeg.info()?;
            (usize::from(info.width), usize::from(info.height), rgba)
        }
        "gif" => {
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::RGBA);
            let mut gif = options.read_info(data.as_slice()).ok()?;
            let (w, h) = (usize::from(gif.width()), usize::from(gif.height()));
            let frame = gif.read_next_frame().ok()??;
            // The first frame may cover only part of the screen; the rest stays clear.
            let mut rgba = vec![0u8; w * h * 4];
            let (left, top, fw) = (
                frame.left as usize,
                frame.top as usize,
                frame.width as usize,
            );
            for (y, row) in frame.buffer.chunks_exact(fw * 4).enumerate() {
                let (y, cut) = (top + y, fw.min(w.saturating_sub(left)));
                if y < h && cut > 0 {
                    let at = (y * w + left) * 4;
                    rgba[at..at + cut * 4].copy_from_slice(&row[..cut * 4]);
                }
            }
            (w, h, rgba)
        }
        "webp" => {
            let mut webp = image_webp::WebPDecoder::new(std::io::Cursor::new(&data)).ok()?;
            let (w, h) = webp.dimensions();
            let mut pixels = vec![0u8; webp.output_buffer_size()?];
            webp.read_image(&mut pixels).ok()?;
            let rgba = match webp.has_alpha() {
                true => pixels,
                false => (pixels.as_chunks::<3>().0.iter())
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect(),
            };
            (w as usize, h as usize, rgba)
        }
        _ => return None,
    };
    if rgba.len() != w * h * 4 {
        return None;
    }
    for [r, g, b, a] in rgba.as_chunks_mut::<4>().0 {
        for c in [r, g, b] {
            *c = (u16::from(*c) * u16::from(*a) / 255) as u8;
        }
    }
    Some(AppIcon::from_rgba(w, h, rgba))
}

/// The SVG at `path` drawn so its longer side is `px` pixels.
fn draw_svg(path: &Path, px: usize) -> Option<AppIcon> {
    let tree = svg(path)?;
    let size = tree.size();
    let fit = px as f32 / size.width().max(size.height());
    let w = (size.width() * fit).round().max(1.0) as u32;
    let h = (size.height() * fit).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(fit, fit),
        &mut pixmap.as_mut(),
    );
    Some(AppIcon::from_rgba(w as usize, h as usize, pixmap.take()))
}

/// The picture at `path` scaled so its longer side is `px` pixels, once
/// the worker has decoded it; meanwhile the same picture kept at another
/// size, or `None` (as for one that can't be read). A buffer drawn while
/// one was missing is drawn again when it comes (`drawing`).
pub fn picture(path: &Path, px: usize) -> Option<AppIcon> {
    let key = (path.to_owned(), px);
    if let Some(found) = take(&SCALED, &key) {
        return Some(found);
    }
    // Tests draw at once, as their pixels are compared.
    if cfg!(test) {
        return scaled(path, px);
    }
    later(key)
}

/// `key` sent to the worker, and its picture kept at another size till
/// then; `None` when there is none, or it couldn't be read a moment ago.
fn later(key: Key) -> Option<AppIcon> {
    if failed(&key) {
        return None;
    }
    let path = key.0.clone();
    WAITED.set(true);
    ask(key);
    SCALED
        .lock()
        .ok()?
        .iter()
        .find(|((p, _), _)| *p == path)
        .map(|(_, icon)| icon.clone())
}

/// The picture at `path` read, scaled so its longer side is `px` pixels and
/// kept; `None` when it can't be read.
fn scaled(path: &Path, px: usize) -> Option<AppIcon> {
    let key = (path.to_owned(), px);
    if format(path).as_deref() == Some("svg") {
        let drawn = draw_svg(path, px)?;
        keep(&SCALED, key, drawn.clone(), KEEP_SCALED);
        return Some(drawn);
    }
    let whole = match take(&WHOLE, &key.0) {
        Some(whole) => whole,
        None => {
            let whole = decode(path)?;
            keep(&WHOLE, key.0.clone(), whole.clone(), KEEP_WHOLE);
            whole
        }
    };
    let (w, h, rgba) = whole.scaled(px);
    let scaled = AppIcon::from_rgba(w, h, rgba);
    keep(&SCALED, key, scaled.clone(), KEEP_SCALED);
    Some(scaled)
}

/// A picture by its file and the longer side's pixels.
type Key = (PathBuf, usize);

/// How long a picture that couldn't be read is left before it is tried
/// again: its file may be written by then.
const RETRY: Duration = Duration::from_secs(2);

/// How many pictures the worker has done, read or not.
static DONE: AtomicU64 = AtomicU64::new(0);
/// Pictures sent to the worker and not done yet.
static ASKED: Mutex<Vec<Key>> = Mutex::new(Vec::new());
/// Pictures the worker couldn't read, and when.
static FAILED: Mutex<Vec<(Key, Instant)>> = Mutex::new(Vec::new());
/// Where pictures go to be decoded; `None` when its thread couldn't start.
static WORKER: LazyLock<Option<Sender<Key>>> = LazyLock::new(|| {
    let (tx, rx) = std::sync::mpsc::channel::<Key>();
    let worker = std::thread::Builder::new().name("pictures".into());
    let started = worker.spawn(move || {
        for key in rx {
            // A decoder's panic costs this picture, not the worker.
            let read = || scaled(&key.0, key.1);
            let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read))
                .ok()
                .flatten();
            if drawn.is_none()
                && let Ok(mut failed) = FAILED.lock()
            {
                failed.retain(|(_, at)| at.elapsed() < RETRY);
                failed.push((key.clone(), Instant::now()));
            }
            if let Ok(mut asked) = ASKED.lock() {
                asked.retain(|k| *k != key);
            }
            DONE.fetch_add(1, Ordering::Release);
        }
    });
    match started {
        Ok(_) => Some(tx),
        Err(err) => {
            tracing::warn!("pictures are decoded on the paint thread: {err}");
            None
        }
    }
});

thread_local! {
    /// Whether a picture asked for on this thread wasn't decoded yet.
    static WAITED: Cell<bool> = const { Cell::new(false) };
}

/// `draw` run, and, when it asked for a picture not decoded yet, how many
/// the worker had done before it: what it drew is to be drawn again once
/// `done` has moved past that.
pub fn drawing<T>(draw: impl FnOnce() -> T) -> (T, Option<u64>) {
    let before = done();
    let outer = WAITED.replace(false);
    let drawn = draw();
    let waited = WAITED.get();
    WAITED.set(outer || waited);
    (drawn, waited.then_some(before))
}

/// How many pictures the worker has done (see `drawing`).
pub fn done() -> u64 {
    DONE.load(Ordering::Acquire)
}

/// Send `key` to the worker, unless it is there already.
fn ask(key: Key) {
    let Some(worker) = WORKER.as_ref() else {
        scaled(&key.0, key.1);
        DONE.fetch_add(1, Ordering::Release);
        return;
    };
    let Ok(mut asked) = ASKED.lock() else {
        return;
    };
    if asked.contains(&key) {
        return;
    }
    asked.push(key.clone());
    if let Err(err) = worker.send(key) {
        let key = err.0;
        asked.retain(|k| *k != key);
        drop(asked);
        tracing::warn!(
            "the picture worker stopped; {} is read here",
            key.0.display()
        );
        scaled(&key.0, key.1);
        DONE.fetch_add(1, Ordering::Release);
    }
}

/// Whether the worker couldn't read `key` a moment ago.
fn failed(key: &Key) -> bool {
    FAILED.lock().is_ok_and(|failed| {
        failed
            .iter()
            .any(|(k, at)| k == key && at.elapsed() < RETRY)
    })
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

    /// Waits until `done()` passes `from`, for at most 10 s.
    fn wait_past(from: u64) {
        let start = Instant::now();
        while done() == from && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_picture_not_decoded_yet_comes_from_the_worker_and_says_it_did() {
        let dir = std::env::temp_dir().join(format!("gerdr-later-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("later.png");
        tiny_skia::Pixmap::new(40, 20)
            .unwrap()
            .save_png(&path)
            .unwrap();
        let key = (path.clone(), 10);

        let (first, waits) = drawing(|| later(key.clone()));
        assert!(first.is_none(), "nothing kept at another size yet");
        let before = waits.expect("the draw is told it waited");
        wait_past(before);
        let got = take(&SCALED, &key).expect("the worker kept it");
        assert_eq!(got.size(), (10, 5));

        // At another size, the one kept stands in while it is decoded.
        let (stand_in, waits) = drawing(|| later((path.clone(), 30)));
        assert_eq!(stand_in.map(|icon| icon.size()), Some((10, 5)));
        assert!(waits.is_some());

        // One that can't be read isn't asked for again at once.
        let gone = (dir.join("gone.png"), 10);
        let (_, waits) = drawing(|| later(gone.clone()));
        wait_past(waits.unwrap());
        let (none, waits) = drawing(|| later(gone));
        assert!(none.is_none() && waits.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

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

    #[test]
    fn jpeg_gif_webp_and_svg_pictures_are_read_too() {
        let dir = std::env::temp_dir().join(format!("gerdr-pictures-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#3080c0"/></svg>"##;
        let files: [(&str, &[u8]); 4] = [
            ("a.JPG", include_bytes!("../assets/test-pictures/a.jpg")),
            ("a.gif", include_bytes!("../assets/test-pictures/a.gif")),
            ("a.webp", include_bytes!("../assets/test-pictures/a.webp")),
            ("a.svg", svg),
        ];
        for (name, data) in files {
            let path = dir.join(name);
            std::fs::write(&path, data).unwrap();
            assert_eq!(size(&path), Some((40, 20)), "{name}");
            let picture = picture(&path, 10).unwrap();
            assert_eq!(picture.size(), (10, 5), "{name}");
        }
        // The WebP is #3080c0 at half alpha, kept premultiplied as all pictures are.
        let (_, _, rgba) = decode(&dir.join("a.webp")).unwrap().scaled(40);
        let near = |a: u8, b: u8| a.abs_diff(b) <= 2;
        assert!(
            near(rgba[0], 24) && near(rgba[1], 64) && near(rgba[2], 96) && near(rgba[3], 128),
            "{:?}",
            &rgba[..4]
        );
        // An SVG's text is drawn, in the system's sans-serif when the named font is missing.
        let text = dir.join("text.svg");
        std::fs::write(
            &text,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text x="2" y="16" font-size="16" font-family="No Such Font, sans-serif">Hi</text></svg>"#,
        )
        .unwrap();
        let (_, _, rgba) = picture(&text, 40).unwrap().scaled(40);
        assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        // A character the font lacks is drawn from cosmic-text's fallback
        // lists, not from whichever system font comes first.
        let mut fonts = SVG_FONTS.clone();
        let query = usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Serif],
            ..Default::default()
        };
        let serif = fonts.query(&query).unwrap();
        let arrow = fallback('→', &[serif], &mut fonts).unwrap();
        let family = fonts.face(arrow).unwrap().families[0].0.clone();
        use cosmic_text::Fallback;
        assert!(
            cosmic_text::PlatformFallback
                .common_fallback()
                .contains(&family.as_str()),
            "{family}"
        );
        let other = dir.join("a.bmp");
        std::fs::write(&other, b"BM").unwrap();
        assert_eq!(size(&other), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
