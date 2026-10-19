//! A buffer of premultiplied ARGB pixels drawn on the CPU.

use super::{APP_ICON_SIZE, Palette, Rect, Rgb, SHADOW, coverage, rounded_rect};
use crate::app_icons::AppIcon;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Rectangle, Size, Transform};

/// ARGB8888 pixels with premultiplied alpha, stored little-endian as B, G, R, A.
pub(super) struct Canvas {
    pub(super) w: i32,
    pub(super) h: i32,
    pub(super) px: Vec<u8>,
}

impl Canvas {
    pub(super) fn new(size: Size<i32, Logical>) -> Self {
        Self {
            w: size.w,
            h: size.h,
            px: vec![0; (size.w * size.h * 4) as usize],
        }
    }

    pub(super) fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, [r, g, b]: Rgb) {
        let (x0, x1) = (x.max(0), (x + w).min(self.w));
        if x0 >= x1 {
            return;
        }
        for py in y.max(0)..(y + h).min(self.h) {
            let row = ((py * self.w + x0) * 4) as usize..((py * self.w + x1) * 4) as usize;
            for pixel in self.px[row].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&[b, g, r, 0xff]);
            }
        }
    }

    /// A rectangle's outline, `b` pixels thick, drawn inside its edges
    /// (physical pixels).
    pub(super) fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, b: i32, color: Rgb) {
        self.fill(x, y, w, b, color);
        self.fill(x, y + h - b, w, b, color);
        self.fill(x, y, b, h, color);
        self.fill(x + w - b, y, b, h, color);
    }

    /// Lay `color` over the pixel at `coverage` opacity.
    pub(super) fn blend(&mut self, x: i32, y: i32, [r, g, b]: Rgb, coverage: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let a = coverage.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        if a >= 1.0 {
            self.px[i..i + 4].copy_from_slice(&[b, g, r, 0xff]);
            return;
        }
        for (dst, src) in self.px[i..i + 4].iter_mut().zip([b, g, r, 0xff]) {
            *dst = (*dst as f32 * (1.0 - a) + src as f32 * a).round() as u8;
        }
    }

    /// Blend `color` over every pixel at the coverage `shape` gives for its centre.
    pub(super) fn paint(&mut self, color: Rgb, shape: impl Fn(f32, f32) -> f32) {
        self.paint_in(0, 0, self.w, self.h, color, shape);
    }

    /// `paint`, limited to a rectangle (physical pixels).
    pub(super) fn paint_in(
        &mut self,
        x0: i32,
        y0: i32,
        w: i32,
        h: i32,
        color: Rgb,
        shape: impl Fn(f32, f32) -> f32,
    ) {
        for y in y0.max(0)..(y0 + h).min(self.h) {
            for x in x0.max(0)..(x0 + w).min(self.w) {
                let a = shape(x as f32 + 0.5, y as f32 + 0.5);
                if a > 0.0 {
                    self.blend(x, y, color, a);
                }
            }
        }
    }

    /// `icon` `APP_ICON_SIZE` big, centred on `rect` (logical pixels); a
    /// symbolic one in `tint`.
    pub(super) fn picture(&mut self, icon: &AppIcon, rect: Rect, scale: f64, tint: Rgb) {
        self.picture_sized(icon, rect, scale, tint, APP_ICON_SIZE);
    }

    /// `icon` `size` big (logical pixels), centred on `rect`; a symbolic
    /// one in `tint`.
    pub(super) fn picture_sized(
        &mut self,
        icon: &AppIcon,
        rect: Rect,
        scale: f64,
        tint: Rgb,
        size: i32,
    ) {
        let size = (size as f64 * scale).round() as usize;
        let (w, h, rgba) = icon.scaled(size);
        let centre = |start: i32, len: i32| (start as f64 + len as f64 / 2.0) * scale;
        let x0 = (centre(rect.loc.x, rect.size.w) - w as f64 / 2.0).round() as i32;
        let y0 = (centre(rect.loc.y, rect.size.h) - h as f64 / 2.0).round() as i32;
        for (i, &[r, g, b, a]) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (x0 + (i % w) as i32, y0 + (i / w) as i32);
            if a == 0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
                continue;
            }
            if icon.symbolic() {
                self.blend(x, y, tint, a as f32 / 255.0);
                continue;
            }
            // Premultiplied: the icon's colour, then what shows through.
            let j = ((y * self.w + x) * 4) as usize;
            let keep = 1.0 - a as f32 / 255.0;
            for (dst, src) in self.px[j..j + 4].iter_mut().zip([b, g, r, a]) {
                *dst = (src as f32 + *dst as f32 * keep).round().min(255.0) as u8;
            }
        }
    }

    /// A filled, anti-aliased rounded rectangle (physical pixels).
    pub(super) fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgb) {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let at = |px: i32, py: i32| {
            coverage(rounded_rect(
                px as f32 + 0.5,
                py as f32 + 0.5,
                x,
                y,
                w,
                h,
                r,
            ))
        };
        let (x0, x1) = (
            (x.floor() as i32).max(0),
            ((x + w).ceil() as i32).min(self.w),
        );
        for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(self.h) {
            // The shape is convex: blend the edges in from both ends of the
            // row up to the first fully covered pixel, and fill between.
            let mut left = x0;
            while left < x1 {
                let a = at(left, py);
                if a >= 1.0 {
                    break;
                }
                if a > 0.0 {
                    self.blend(left, py, color, a);
                }
                left += 1;
            }
            let mut right = x1;
            while right > left {
                let a = at(right - 1, py);
                if a >= 1.0 {
                    break;
                }
                if a > 0.0 {
                    self.blend(right - 1, py, color, a);
                }
                right -= 1;
            }
            self.fill(left, py, right - left, 1, color);
        }
    }

    /// A popup panel of logical `size` with a soft drop shadow, drawn `SHADOW`
    /// in from the top left.
    pub(super) fn popup(&mut self, size: Size<i32, Logical>, radius: f32, scale: f64, p: &Palette) {
        let px = |v: i32| (v as f64 * scale).round() as i32;
        let s = scale as f32;
        let (ox, oy) = (px(SHADOW) as f32, px(SHADOW) as f32);
        let (w, h) = (px(size.w) as f32, px(size.h) as f32);
        let radius = radius * s;

        // Soft shadow, dropped slightly below the panel.
        let blur = (SHADOW as f32 - 2.0) * s;
        let shadow_y = oy + 3.0 * s;
        self.paint([0, 0, 0], |x, y| {
            let d = rounded_rect(x, y, ox, shadow_y, w, h, radius);
            let t = 1.0 - (d / blur).clamp(0.0, 1.0);
            p.shadow * t * t
        });
        self.rounded(ox, oy, w, h, radius, p.border);
        let b = px(1).max(1) as f32;
        self.rounded(
            ox + b,
            oy + b,
            w - 2.0 * b,
            h - 2.0 * b,
            radius - b,
            p.elevated,
        );
    }

    /// `opaque`: every pixel is fully opaque, so the compositor need not draw behind it.
    pub(super) fn into_buffer(self, opaque: bool) -> MemoryRenderBuffer {
        let opaque = opaque.then(|| vec![Rectangle::from_size((self.w, self.h).into())]);
        MemoryRenderBuffer::from_slice(
            &self.px,
            Fourcc::Argb8888,
            (self.w, self.h),
            1,
            Transform::Normal,
            opaque,
        )
    }
}
