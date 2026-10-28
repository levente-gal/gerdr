//! App icons, as desktop entries name them: a path, or a name looked up in
//! the icon theme (the desktop's, then the themes it inherits, then
//! "hicolor"), then in the `pixmaps` folders. PNG and SVG icons are read;
//! each file once, as it is kept for the next look up.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use resvg::{tiny_skia, usvg};

use crate::home_dir;

/// How big SVG icons are drawn, and the smallest PNG size preferred.
const SIZE: u32 = 64;

/// How deep a theme's folders are searched.
const MAX_DEPTH: usize = 4;

/// An icon's pixels: RGBA, premultiplied. Equal only to its copies, so
/// comparing them costs nothing.
#[derive(Clone)]
pub struct AppIcon(Arc<Picture>);

struct Picture {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    /// A one-colour icon, meant to be tinted like text.
    symbolic: bool,
}

impl PartialEq for AppIcon {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AppIcon {}

impl fmt::Debug for AppIcon {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AppIcon({}x{})", self.0.width, self.0.height)
    }
}

impl AppIcon {
    pub fn symbolic(&self) -> bool {
        self.0.symbolic
    }

    /// The icon scaled to fit `size` × `size` pixels, keeping its shape:
    /// its width, height and RGBA pixels, premultiplied.
    pub fn scaled(&self, size: usize) -> (usize, usize, Vec<u8>) {
        let p = &self.0;
        let fit = size as f32 / p.width.max(p.height) as f32;
        let w = ((p.width as f32 * fit).round() as usize).max(1);
        let h = ((p.height as f32 * fit).round() as usize).max(1);
        let (xs, ys) = (weights(p.width, w), weights(p.height, h));
        let mut out = vec![0; w * h * 4];
        for (y, row) in ys.iter().enumerate() {
            for (x, col) in xs.iter().enumerate() {
                let mut sum = [0.0f32; 4];
                for &(sy, wy) in row {
                    for &(sx, wx) in col {
                        let i = (sy * p.width + sx) * 4;
                        for (s, v) in sum.iter_mut().zip(&p.rgba[i..i + 4]) {
                            *s += *v as f32 * wx * wy;
                        }
                    }
                }
                let i = (y * w + x) * 4;
                for (o, s) in out[i..i + 4].iter_mut().zip(sum) {
                    *o = s.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        (w, h, out)
    }
}

/// For each of `to` pixels along a side, the pixels of the `from` long side
/// it covers, with how much of it each makes up.
fn weights(from: usize, to: usize) -> Vec<Vec<(usize, f32)>> {
    let step = from as f32 / to as f32;
    (0..to)
        .map(|i| {
            let (start, end) = (i as f32 * step, (i + 1) as f32 * step);
            let last = (end.ceil() as usize).min(from);
            (start.floor() as usize..last)
                .map(|j| {
                    let overlap = end.min(j as f32 + 1.0) - start.max(j as f32);
                    (j, overlap / step)
                })
                .filter(|(_, w)| *w > 0.0)
                .collect()
        })
        .collect()
}

/// The icon files by name, as the theme picks them.
#[derive(Default)]
pub struct IconIndex {
    files: HashMap<String, PathBuf>,
}

impl IconIndex {
    /// The icons in the `icons` and `pixmaps` folders of `data_dirs`.
    pub fn new(data_dirs: &[PathBuf]) -> Self {
        let mut bases: Vec<PathBuf> = home_dir().map(|h| h.join(".icons")).into_iter().collect();
        bases.extend(data_dirs.iter().map(|dir| dir.join("icons")));
        let mut files = HashMap::new();
        for theme in themes(&bases) {
            // The best file of each name in this theme.
            let mut best: HashMap<String, (u32, PathBuf)> = HashMap::new();
            for base in &bases {
                let dir = base.join(&theme);
                let mut found = Vec::new();
                icon_files(&dir, 0, &mut found);
                for path in found {
                    let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let rank = rank(path.strip_prefix(&dir).unwrap_or(&path));
                    if best.get(name).is_none_or(|(r, _)| rank > *r) {
                        best.insert(name.to_owned(), (rank, path.clone()));
                    }
                }
            }
            for (name, (_, path)) in best {
                files.entry(name).or_insert(path);
            }
        }
        for dir in data_dirs.iter().map(|dir| dir.join("pixmaps")) {
            let mut found = Vec::new();
            icon_files(&dir, MAX_DEPTH, &mut found);
            found.sort();
            for path in found {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    files.entry(name.to_owned()).or_insert(path);
                }
            }
        }
        Self { files }
    }

    /// The icon a desktop entry's `Icon` value names, if it can be read.
    pub fn find(&self, icon: &str) -> Option<AppIcon> {
        let icon = icon.trim();
        if icon.is_empty() {
            return None;
        }
        let path = if icon.starts_with('/') {
            PathBuf::from(icon)
        } else {
            let name = icon
                .strip_suffix(".png")
                .or_else(|| icon.strip_suffix(".svg"))
                .unwrap_or(icon);
            self.files.get(name)?.clone()
        };
        load(&path)
    }
}

/// The icon themes to look in, the first first: the desktop's, those it
/// inherits, then "hicolor".
fn themes(bases: &[PathBuf]) -> Vec<String> {
    let mut themes = Vec::new();
    let mut next: Vec<String> = current_theme().into_iter().collect();
    while let Some(theme) = next.pop() {
        if theme == "hicolor" || themes.contains(&theme) || themes.len() > 16 {
            continue;
        }
        let inherits = bases
            .iter()
            .find_map(|base| std::fs::read_to_string(base.join(&theme).join("index.theme")).ok())
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.trim().strip_prefix("Inherits=").map(str::to_owned))
            });
        themes.push(theme);
        // Popped from the end: the first inherited theme comes next.
        next.extend(
            inherits
                .iter()
                .flat_map(|list| list.split(',').rev())
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty()),
        );
    }
    themes.push("hicolor".into());
    themes
}

/// The desktop's icon theme, from GNOME's settings or KDE's.
fn current_theme() -> Option<String> {
    let gsettings = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|theme| theme.trim().trim_matches('\'').to_owned());
    let kde = || {
        let text = std::fs::read_to_string(home_dir()?.join(".config/kdeglobals")).ok()?;
        let mut in_icons = false;
        text.lines().map(str::trim).find_map(|line| {
            if line.starts_with('[') {
                in_icons = line == "[Icons]";
                None
            } else if in_icons {
                line.strip_prefix("Theme=").map(str::to_owned)
            } else {
                None
            }
        })
    };
    gsettings.or_else(kde).filter(|theme| !theme.is_empty())
}

/// The PNG and SVG files in `dir` and the folders below it.
fn icon_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                icon_files(&path, depth + 1, files);
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext == "png" || ext == "svg")
        {
            files.push(path);
        }
    }
}

/// How well an icon file (by its path in its theme) suits, the higher the
/// better: the smallest PNG at least `SIZE` big, then an SVG, then the
/// biggest smaller PNG. A theme folder names the size: "48x48", "48",
/// "48x48@2" or "scalable".
fn rank(path: &Path) -> u32 {
    if path.extension().is_some_and(|ext| ext == "svg") {
        return 5000;
    }
    let size = path
        .components()
        .filter_map(|c| {
            let c = c.as_os_str().to_str()?;
            let (size, scale) = c.split_once('@').unwrap_or((c, "1"));
            let size: u32 = size.split_once('x').map_or(size, |(w, _)| w).parse().ok()?;
            Some(size * scale.parse::<u32>().ok()?.max(1))
        })
        .next()
        .unwrap_or(0);
    if size >= SIZE {
        10000 - size.min(4096)
    } else {
        size
    }
}

/// The files read so far, by path; `None` for those that couldn't be.
static LOADED: LazyLock<Mutex<HashMap<PathBuf, Option<AppIcon>>>> = LazyLock::new(Default::default);

/// The icon in the PNG or SVG file at `path`, read once.
fn load(path: &Path) -> Option<AppIcon> {
    if let Some(icon) = LOADED.lock().ok()?.get(path) {
        return icon.clone();
    }
    let icon = read(path);
    if icon.is_none() {
        tracing::debug!("can't read icon {}", path.display());
    }
    LOADED.lock().ok()?.insert(path.to_owned(), icon.clone());
    icon
}

fn read(path: &Path) -> Option<AppIcon> {
    let data = std::fs::read(path).ok()?;
    let symbolic = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with("-symbolic"));
    if path.extension().is_some_and(|ext| ext == "svg") {
        return AppIcon::from_svg(&data, symbolic);
    }
    let pixmap = tiny_skia::Pixmap::decode_png(&data).ok()?;
    Some(AppIcon::from_pixmap(pixmap, symbolic))
}

impl AppIcon {
    /// The icon an SVG document draws, `SIZE` big; a `symbolic` one is
    /// tinted like text where it is drawn.
    pub fn from_svg(data: &[u8], symbolic: bool) -> Option<Self> {
        let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
        let size = tree.size();
        let fit = SIZE as f32 / size.width().max(size.height());
        let w = (size.width() * fit).round().max(1.0) as u32;
        let h = (size.height() * fit).round().max(1.0) as u32;
        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(fit, fit),
            &mut pixmap.as_mut(),
        );
        Some(Self::from_pixmap(pixmap, symbolic))
    }

    fn from_pixmap(pixmap: tiny_skia::Pixmap, symbolic: bool) -> Self {
        AppIcon(Arc::new(Picture {
            width: pixmap.width() as usize,
            height: pixmap.height() as usize,
            rgba: pixmap.take(),
            symbolic,
        }))
    }
}

/// gerdr's own icon for the host window, straight RGBA as winit takes it.
/// Only X11 reads it; Wayland desktops find `dist/gerdr.svg` through the
/// window's app_id and `gerdr.desktop`.
pub fn window_icon() -> Option<smithay::reexports::winit::window::Icon> {
    const PX: u32 = 128;
    let tree = usvg::Tree::from_data(
        include_bytes!("../dist/gerdr.svg"),
        &usvg::Options::default(),
    )
    .ok()?;
    let size = tree.size();
    let fit = PX as f32 / size.width().max(size.height());
    let mut pixmap = tiny_skia::Pixmap::new(PX, PX)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(fit, fit),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    smithay::reexports::winit::window::Icon::from_rgba(rgba, PX, PX).ok()
}

/// GitHub's mark (Octicons' `mark-github`, MIT licensed), symbolic: the
/// icon of GitHub workspaces and of the CI card.
pub fn github_icon() -> Option<AppIcon> {
    static ICON: OnceLock<Option<AppIcon>> = OnceLock::new();
    ICON.get_or_init(|| AppIcon::from_svg(include_bytes!("../assets/github-mark.svg"), true))
        .clone()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{github_icon, rank, weights, window_icon};

    #[test]
    fn ranks_sizes() {
        let r = |p: &str| rank(Path::new(p));
        assert!(r("64x64/apps/a.png") > r("128x128/apps/a.png"));
        assert!(r("128x128/apps/a.png") > r("scalable/apps/a.svg"));
        assert!(r("scalable/apps/a.svg") > r("48x48/apps/a.png"));
        assert!(r("48x48/apps/a.png") > r("32x32/apps/a.png"));
        assert_eq!(r("32x32@2/apps/a.png"), r("64x64/apps/a.png"));
        assert_eq!(r("apps/48/a.png"), r("48x48/apps/a.png"));
    }

    #[test]
    fn weights_cover_each_pixel_once() {
        for (from, to) in [(64, 20), (48, 20), (16, 40), (3, 3)] {
            let all = weights(from, to);
            for w in &all {
                assert!((w.iter().map(|(_, w)| w).sum::<f32>() - 1.0).abs() < 1e-4);
            }
            let mut per_source = vec![0.0; from];
            for (j, w) in all.iter().flatten() {
                per_source[*j] += w;
            }
            let expected = to as f32 / from as f32;
            assert!(per_source.iter().all(|s| (s - expected).abs() < 1e-3));
        }
    }

    #[test]
    fn gerdr_draws_its_own_window_icon() {
        assert!(window_icon().is_some());
    }

    #[test]
    fn the_github_mark_is_a_symbolic_ring_around_a_cut_out_cat() {
        let icon = github_icon().expect("the mark reads");
        assert!(icon.symbolic());
        // Its own copies, read once.
        assert_eq!(github_icon(), Some(icon.clone()));
        let (w, h, rgba) = icon.scaled(16);
        assert_eq!((w, h), (16, 16));
        let alpha = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3];
        // The ring is drawn, the cat's face is cut out, the corners empty.
        assert!(alpha(1, 8) > 200 && alpha(14, 8) > 200 && alpha(8, 1) > 200);
        assert!(alpha(8, 8) < 50);
        assert!(alpha(0, 0) < 50 && alpha(15, 15) < 50);
    }
}
