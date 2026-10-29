//! What the chrome hands the renderer: parts, each a drawn buffer placed
//! on the output and maybe cut to a rectangle. A scrolled list is drawn as
//! a buffer per row over its panel (see `RowCache`), so scrolling it only
//! moves the rows' buffers on the GPU: nothing is drawn again but the rows
//! coming into view.

use std::cell::Cell;

use smithay::{
    backend::renderer::{
        element::{
            Element, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            utils::CropRenderElement,
        },
        gles::GlesRenderer,
    },
    utils::{Logical, Physical, Point, Rectangle, Size},
};

use super::{Rect, physical};

/// What the chrome renders: a part's buffer, cut to its clip.
pub type ChromeElement = CropRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>;

/// A drawn buffer and where it goes.
#[derive(Clone, Debug)]
pub(super) struct Part {
    pub buffer: MemoryRenderBuffer,
    /// Its top left on the output, in logical pixels; a fraction places
    /// it to the nearest physical pixel.
    pub loc: Point<f64, Logical>,
    /// The logical size it covers; it holds physical pixels at the scale
    /// (see `physical`).
    pub size: Size<i32, Logical>,
    /// Its opacity, 0 to 1.
    pub alpha: f32,
    /// What of it shows, on the output: all of it when `None`.
    pub clip: Option<Rect>,
}

impl Part {
    pub(super) fn new(
        buffer: &MemoryRenderBuffer,
        loc: Point<i32, Logical>,
        size: Size<i32, Logical>,
        alpha: f32,
    ) -> Self {
        Self {
            buffer: buffer.clone(),
            loc: loc.to_f64(),
            size,
            alpha,
            clip: None,
        }
    }

    /// The part at `loc` instead, to the fraction of a pixel.
    pub(super) fn at(mut self, loc: Point<f64, Logical>) -> Self {
        self.loc = loc;
        self
    }

    /// The part showing only within `clip` (on the output).
    pub(super) fn clipped(mut self, clip: Rect) -> Self {
        self.clip = Some(clip);
        self
    }

    /// The part moved down by `dy` and faded by `alpha`, as the popup it
    /// is on is.
    pub(super) fn with(mut self, dy: i32, alpha: f32) -> Self {
        self.loc.y += dy as f64;
        self.alpha *= alpha;
        if let Some(clip) = self.clip.as_mut() {
            clip.loc.y += dy;
        }
        self
    }
}

/// Render elements for `parts`, at `scale`.
pub(super) fn render_parts(
    renderer: &mut GlesRenderer,
    parts: Vec<Part>,
    scale: f64,
) -> Vec<ChromeElement> {
    parts
        .into_iter()
        .filter_map(|part| {
            // The buffer holds physical pixels at buffer scale 1: sample all
            // of it and squeeze it into the logical size, starting on a
            // whole physical pixel.
            let src = Rectangle::from_size(physical(part.size, scale).to_f64());
            let loc: Point<i32, Physical> = part.loc.to_physical(scale).to_i32_round();
            let element = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc.to_f64(),
                &part.buffer,
                Some(part.alpha),
                Some(src),
                Some(part.size),
                Kind::Unspecified,
            )
            .ok()?;
            let crop = match part.clip {
                Some(clip) => clip.to_physical_precise_round(scale),
                None => element.geometry(scale.into()),
            };
            CropRenderElement::from_element(element, scale, crop)
        })
        .collect()
}

/// Bytes of row buffers a cache keeps beyond those shown.
const ROW_CACHE_BYTES: usize = 48 << 20;

/// Buffers drawn for a list's rows, by what each is drawn from: a row
/// scrolled back into view, or one still in view, is not drawn again. All
/// those asked for since the last `sweep` are kept, and others while they
/// fit in `ROW_CACHE_BYTES`, the latest asked for first.
pub(super) struct RowCache<K> {
    /// The newest last.
    rows: Vec<Row<K>>,
    /// Counts sweeps: a row asked for since the last has it as its `seen`.
    frame: u64,
    /// How many rows were drawn, for tests.
    drawn: Cell<usize>,
}

struct Row<K> {
    key: K,
    buffer: MemoryRenderBuffer,
    bytes: usize,
    seen: u64,
}

impl<K> Default for RowCache<K> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            frame: 0,
            drawn: Cell::new(0),
        }
    }
}

impl<K: PartialEq> RowCache<K> {
    /// The buffer for the row drawn from `key`, drawn with `draw` (to a
    /// buffer of `size`, logical, at `scale`) unless it is kept.
    pub(super) fn get(
        &mut self,
        key: K,
        size: Size<i32, Logical>,
        scale: f64,
        draw: impl FnOnce(&K) -> MemoryRenderBuffer,
    ) -> MemoryRenderBuffer {
        let frame = self.frame;
        let row = match self.rows.iter().position(|r| r.key == key) {
            Some(i) => self.rows.remove(i),
            None => {
                self.drawn.set(self.drawn.get() + 1);
                let px = physical(size, scale);
                Row {
                    buffer: draw(&key),
                    key,
                    bytes: (px.w.max(0) * px.h.max(0) * 4) as usize,
                    seen: frame,
                }
            }
        };
        let buffer = row.buffer.clone();
        self.rows.push(Row { seen: frame, ..row });
        buffer
    }

    /// The frame is done: let go of rows not asked for in it, past the
    /// budget.
    pub(super) fn sweep(&mut self) {
        let frame = self.frame;
        let mut kept = 0;
        for row in self.rows.iter_mut().rev() {
            if row.seen != frame {
                kept += row.bytes;
                if kept > ROW_CACHE_BYTES {
                    row.seen = u64::MAX;
                }
            }
        }
        self.rows.retain(|r| r.seen != u64::MAX);
        self.frame += 1;
    }

    /// Let go of every row (the panel closed for good, say).
    pub(super) fn clear(&mut self) {
        self.rows.clear();
    }

    /// How many rows were drawn so far.
    #[cfg(test)]
    pub(super) fn drawn(&self) -> usize {
        self.drawn.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_kept_is_not_drawn_again() {
        let mut cache = RowCache::default();
        let size = Size::from((10, 10));
        for frame in 0..3 {
            for row in 0..4 {
                cache.get(row + frame, size, 1.0, |_| MemoryRenderBuffer::default());
            }
            cache.sweep();
        }
        // Rows 0 to 3, then 4 and then 5 came into view.
        assert_eq!(cache.drawn(), 6);
    }
}
