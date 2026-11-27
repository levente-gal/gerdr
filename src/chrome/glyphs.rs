//! Text shaped and drawn by cosmic-text, for `painter`: a line of text in
//! one of gerdr's four faces, measured and drawn into a `Canvas`. Shaping
//! (HarfRust) joins ligatures and the scripts that need it, and a glyph the
//! face lacks (`❯`, `✓`, an emoji, CJK) comes from another system font, in
//! colour when it has colour.
//!
//! The faces are the ones fontconfig picks for `sans-serif` and
//! `monospace`, found by their files in a database of the system's fonts,
//! which is read once and shared. Each thread has a `FontSystem` of its own
//! (`Painter` isn't `Sync`, and layouts measure off the painter), with the
//! lines it shaped kept by text, face and size.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Color, Family, FontSystem, LayoutGlyph, Metrics, Shaping,
    SwashCache, Weight, Wrap, fontdb,
};

use super::{Canvas, Rgb};

/// A face text is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Face {
    Ui,
    /// The UI font in bold, for names that head a group.
    Bold,
    /// For diffs, logs and terminal text.
    Mono,
    /// Terminal text drawn bold.
    MonoBold,
    /// The UI font slanted, for a file's emphasis: its italic face, or
    /// its upright one sheared where it has none.
    Italic,
}

/// The weight bold text is drawn at, from a font with a weight axis.
const BOLD_WEIGHT: u16 = 650;
/// The weight terminal bold is drawn at.
const MONO_BOLD_WEIGHT: u16 = 700;

impl Face {
    fn weight(self) -> Weight {
        match self {
            Face::Ui | Face::Mono | Face::Italic => Weight::NORMAL,
            Face::Bold => Weight(BOLD_WEIGHT),
            Face::MonoBold => Weight(MONO_BOLD_WEIGHT),
        }
    }

    fn mono(self) -> bool {
        matches!(self, Face::Mono | Face::MonoBold)
    }

    fn style(self) -> fontdb::Style {
        match self {
            Face::Italic => fontdb::Style::Italic,
            _ => fontdb::Style::Normal,
        }
    }
}

/// The system's fonts, and the families fontconfig picks.
struct Fonts {
    db: fontdb::Database,
    /// The UI font's family; `None` when fontconfig named no font.
    ui: Option<String>,
    /// The monospace font's family; `None` when there is none (the UI font
    /// draws monospace text then).
    mono: Option<String>,
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        drop_undrawable(&mut db);
        let ui = family_of(&mut db, "sans-serif");
        if ui.is_none() {
            tracing::warn!("chrome: no usable font found, labels will be blank");
        }
        let mono = family_of(&mut db, "monospace");
        if mono.is_none() {
            tracing::warn!("chrome: no monospace font found, diffs use the UI font");
        }
        if let Some(ui) = &ui {
            db.set_sans_serif_family(ui.clone());
        }
        if let Some(mono) = &mono {
            db.set_monospace_family(mono.clone());
        }
        Fonts { db, ui, mono }
    })
}

/// Take out of `db` the faces swash can't draw: colour fonts in COLR
/// version 1 only (Fedora's Noto Color Emoji), whose glyphs come out blank.
/// A fallback then goes on to a font that draws them (Noto Emoji, in one
/// colour).
// shortcut: COLRv1 emoji show in one colour; draw COLRv1 when swash can.
fn drop_undrawable(db: &mut fontdb::Database) {
    let ids: Vec<fontdb::ID> = db.faces().map(|face| face.id).collect();
    let blank: Vec<fontdb::ID> = ids
        .into_iter()
        .filter(|&id| db.with_face_data(id, colr_v1_only).unwrap_or(false))
        .collect();
    for id in blank {
        db.remove_face(id);
    }
}

/// Whether the face at `index` of font file `data` has colour only as COLR
/// version 1: no COLR version 0, no bitmaps (`CBDT`, `sbix`).
fn colr_v1_only(data: &[u8], index: u32) -> bool {
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let u32_at =
        |at: usize| (data.get(at..at + 4)).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    // A collection points at each face's table directory.
    let dir = match data.get(..4) {
        Some(b"ttcf") => match u32_at(12 + 4 * index as usize) {
            Some(at) => at as usize,
            None => return false,
        },
        _ => 0,
    };
    let Some(tables) = u16_at(dir + 4) else {
        return false;
    };
    let mut colr = None;
    for i in 0..tables as usize {
        let record = dir + 12 + 16 * i;
        match data.get(record..record + 4) {
            Some(b"CBDT" | b"sbix") => return false,
            Some(b"COLR") => colr = u32_at(record + 8),
            Some(_) => {}
            None => return false,
        }
    }
    colr.and_then(|at| u16_at(at as usize))
        .is_some_and(|version| version >= 1)
}

/// The family of the file fontconfig picks for `pattern`, loaded into `db`
/// when it is outside the folders `db` read.
fn family_of(db: &mut fontdb::Database, pattern: &str) -> Option<String> {
    let path = std::path::PathBuf::from(super::painter::find_family(pattern)?);
    let of_file = |db: &fontdb::Database| {
        db.faces().find_map(|face| {
            let file = match &face.source {
                fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p,
                fontdb::Source::Binary(_) => return None,
            };
            (*file == path).then(|| face.families.first().map(|(name, _)| name.clone()))?
        })
    };
    if let Some(family) = of_file(db) {
        return Some(family);
    }
    if let Err(err) = db.load_font_file(&path) {
        tracing::warn!("chrome: couldn't read the font {}: {err}", path.display());
        return None;
    }
    of_file(db)
}

/// A line of text shaped: its glyphs from `x = 0` on the baseline, and how
/// wide it is (physical pixels).
pub(super) struct Shaped {
    pub(super) glyphs: Vec<LayoutGlyph>,
    pub(super) w: f32,
}

/// Shaped lines by face and size in pixels, then by text: a line is looked
/// up by its `&str`, with no `String` made for it.
type Lines = HashMap<(Face, u32), HashMap<String, Rc<Shaped>>>;

/// How many shaped lines a thread keeps; past it, they are all let go.
// shortcut: the cache empties at once when full; keep the lines used last if a
// frame after the clear shows up slow.
const SHAPED_MAX: usize = 20_000;

struct Shaper {
    fs: FontSystem,
    swash: SwashCache,
    lines: Lines,
    /// The cap height of each face at a size: what text is centred on.
    caps: HashMap<(Face, u32), f32>,
    /// Each face's em per pixel of line height (`em_per_px`).
    ems: HashMap<Face, f32>,
}

thread_local! {
    static SHAPER: RefCell<Option<Shaper>> = const { RefCell::new(None) };
}

/// `f` with this thread's shaper, made the first time.
fn with<T>(f: impl FnOnce(&mut Shaper) -> T) -> T {
    SHAPER.with_borrow_mut(|shaper| {
        let shaper = shaper.get_or_insert_with(|| {
            let locale = sys_locale().unwrap_or_else(|| "en-US".into());
            Shaper {
                fs: FontSystem::new_with_locale_and_db(locale, fonts().db.clone()),
                swash: SwashCache::new(),
                lines: HashMap::new(),
                caps: HashMap::new(),
                ems: HashMap::new(),
            }
        });
        f(shaper)
    })
}

/// The locale, from the environment as glibc reads it.
fn sys_locale() -> Option<String> {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| v.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
}

/// Whether there is a UI font to draw with.
pub(super) fn available() -> bool {
    fonts().ui.is_some()
}

/// The face of `face`'s own family nearest its weight.
fn face_id(face: Face) -> Option<fontdb::ID> {
    let fonts = fonts();
    let family = match face.mono() {
        true => fonts.mono.as_deref().or(fonts.ui.as_deref()),
        false => fonts.ui.as_deref(),
    }?;
    fonts.db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: face.weight(),
        style: face.style(),
        ..fontdb::Query::default()
    })
}

/// Whether the monospace font draws bold heavier than regular: a bold face
/// of its own, or one face with a weight axis that reaches bold.
pub(super) fn has_mono_bold() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        if fonts().mono.is_none() {
            return false;
        }
        let (regular, bold) = (face_id(Face::Mono), face_id(Face::MonoBold));
        if regular != bold {
            return true;
        }
        let Some(id) = regular else { return false };
        let wght = u32::from_be_bytes(*b"wght");
        with(|s| {
            s.fs.get_font(id, Face::Mono.weight()).is_some_and(|font| {
                (font.as_swash().variations().find_by_tag(wght))
                    .is_some_and(|axis| axis.max_value() >= f32::from(MONO_BOLD_WEIGHT))
            })
        })
    })
}

fn attrs(face: Face) -> Attrs<'static> {
    let fonts = fonts();
    let family = match face.mono() {
        true => fonts.mono.as_deref().or(fonts.ui.as_deref()),
        false => fonts.ui.as_deref(),
    };
    Attrs::new()
        .family(family.map_or(Family::SansSerif, Family::Name))
        .weight(face.weight())
        .style(face.style())
        // As gerdr drew text before: unhinted, at the pen's fraction.
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
}

impl Shaper {
    fn shaped(&mut self, text: &str, face: Face, px: f32) -> Rc<Shaped> {
        let size = (face, px.to_bits());
        if let Some(found) = self.lines.get(&size).and_then(|by| by.get(text)) {
            return found.clone();
        }
        let shaped = Rc::new(self.shape(text, face, px));
        if self.lines.values().map(HashMap::len).sum::<usize>() >= SHAPED_MAX {
            self.lines.clear();
        }
        let by = self.lines.entry(size).or_default();
        by.insert(text.to_owned(), shaped.clone());
        shaped
    }

    fn shape(&mut self, text: &str, face: Face, px: f32) -> Shaped {
        if text.is_empty() || px <= 0.0 {
            return Shaped {
                glyphs: Vec::new(),
                w: 0.0,
            };
        }
        let em = px * self.em_per_px(face);
        let mut buffer = Buffer::new_empty(Metrics::new(em, em));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs(face), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        // One line, but a `\n` in it starts another: they go on after it,
        // their clusters counted in the whole text.
        let starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let (mut glyphs, mut w) = (Vec::new(), 0.0);
        for run in buffer.layout_runs() {
            let from = starts.get(run.line_i).copied().unwrap_or(0);
            glyphs.extend(run.glyphs.iter().map(|g| LayoutGlyph {
                x: g.x + w,
                start: g.start + from,
                end: g.end + from,
                ..g.clone()
            }));
            w += run.line_w;
        }
        Shaped { glyphs, w }
    }

    /// The em size text of size 1 is drawn at in `face`. A size in gerdr is
    /// the font's line height, ascent to descent (as `ab_glyph` had it), not
    /// its em: Noto Sans at 14 has an em of 10.3.
    fn em_per_px(&mut self, face: Face) -> f32 {
        if let Some(ratio) = self.ems.get(&face) {
            return *ratio;
        }
        let ratio = face_id(face)
            .and_then(|id| self.fs.get_font(id, face.weight()))
            .map(|font| {
                let m = font.as_swash().metrics(&[]);
                let height = m.ascent + m.descent;
                match height > 0.0 {
                    true => f32::from(m.units_per_em) / height,
                    false => 1.0,
                }
            })
            .unwrap_or(1.0);
        self.ems.insert(face, ratio);
        ratio
    }

    fn cap_height(&mut self, face: Face, px: f32) -> f32 {
        if let Some(cap) = self.caps.get(&(face, px.to_bits())) {
            return *cap;
        }
        let h = self.shape("H", face, px);
        let cap = h
            .glyphs
            .first()
            .and_then(|g| {
                let key = g.physical((0.0, 0.0), 1.0).cache_key;
                let image = self.swash.get_image(&mut self.fs, key).as_ref()?;
                Some(image.placement.top as f32)
            })
            .unwrap_or(px * 0.5);
        self.caps.insert((face, px.to_bits()), cap);
        cap
    }
}

/// How wide `text` is in `face` at `px` pixels, in pixels.
pub(super) fn width(text: &str, face: Face, px: f32) -> f32 {
    if !available() {
        return 0.0;
    }
    with(|s| s.shaped(text, face, px).w)
}

/// `text` shaped in `face` at `px` pixels.
pub(super) fn shaped(text: &str, face: Face, px: f32) -> Rc<Shaped> {
    with(|s| s.shaped(text, face, px))
}

/// The cap height of `face` at `px` pixels: how far above the baseline an
/// `H` reaches.
pub(super) fn cap_height(face: Face, px: f32) -> f32 {
    with(|s| s.cap_height(face, px))
}

/// Whether `face`'s own font has `ch` (a fallback font isn't asked).
pub(super) fn has_glyph(face: Face, ch: char) -> bool {
    if !available() {
        return false;
    }
    let Some(id) = face_id(face) else {
        return false;
    };
    with(|s| {
        (s.fs.get_font(id, face.weight()))
            .is_some_and(|font| font.as_swash().charmap().map(ch) != 0)
    })
}

/// Draw `text` in `face` at `px` pixels from `x` on the line `baseline`
/// (physical pixels), in `color`; with `clip`, only the glyphs wholly
/// between its edges, stopping at the first past its right. Returns where
/// the text ends.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    c: &mut Canvas,
    text: &str,
    face: Face,
    px: f32,
    x: f32,
    baseline: f32,
    clip: Option<(f32, f32)>,
    [r, g, b]: Rgb,
) -> f32 {
    if !available() {
        return x;
    }
    with(|s| {
        let shaped = s.shaped(text, face, px);
        let base = Color::rgb(r, g, b);
        for glyph in &shaped.glyphs {
            let at = x + glyph.x;
            if let Some((min_x, max_x)) = clip {
                if at + glyph.w > max_x {
                    break;
                }
                if at < min_x {
                    continue;
                }
            }
            let physical = glyph.physical((x, baseline), 1.0);
            s.swash
                .with_pixels(&mut s.fs, physical.cache_key, base, |gx, gy, color| {
                    let (r, g, b, a) = color.as_rgba_tuple();
                    c.blend(
                        physical.x + gx,
                        physical.y + gy,
                        [r, g, b],
                        f32::from(a) / 255.0,
                    );
                });
        }
        x + shaped.w
    })
}

#[cfg(test)]
mod tests {
    /// A font file of one face with `tables` (tag, data).
    fn font(tables: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0; 6]);
        let mut at = 12 + 16 * tables.len();
        let mut body: Vec<u8> = Vec::new();
        for (tag, data) in tables {
            out.extend(tag.iter());
            out.extend([0; 4]);
            out.extend((at as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            body.extend(data.iter());
            at += data.len();
        }
        out.extend(body);
        out
    }

    #[test]
    fn only_colour_fonts_swash_cant_draw_are_dropped() {
        let v1 = font(&[(b"COLR", &[0, 1, 0, 0])]);
        let v0 = font(&[(b"COLR", &[0, 0, 0, 0])]);
        let v1_bitmaps = font(&[(b"COLR", &[0, 1]), (b"CBDT", &[0])]);
        let plain = font(&[(b"glyf", &[0])]);
        assert!(super::colr_v1_only(&v1, 0));
        assert!(!super::colr_v1_only(&v0, 0));
        assert!(!super::colr_v1_only(&v1_bitmaps, 0));
        assert!(!super::colr_v1_only(&plain, 0));
        assert!(!super::colr_v1_only(&[], 0));
        assert!(!super::colr_v1_only(b"ttcf", 3));
    }
}
