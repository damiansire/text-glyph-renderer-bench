//! atlas.rs — CPU glyph rasterizer + GPU texture atlas manager.
//!
//! Ports the shelf-packing + LRU-eviction design of
//! `poc-1c-webgpu-atlas/src/atlas.js` from JS/WebGPU to Rust/wgpu:
//!   - The atlas is one `R8Unorm` GPU texture (single channel: coverage mask,
//!     4x less VRAM than RGBA8).
//!   - Glyphs are packed left-to-right on fixed-height shelves; when a shelf is
//!     full a new one starts below it.
//!   - When the atlas is full, the least-recently-used glyph is evicted (via the
//!     `lru` crate) and its rectangle returns to a free-list, so later glyphs
//!     reuse that space instead of forcing a whole-atlas rebuild.
//!
//! The packing/eviction bookkeeping (`AtlasPacker`) is deliberately GPU-free so
//! it can be unit-tested without a `wgpu::Device`. `GlyphAtlas` wraps it with the
//! actual GPU texture and does the CPU rasterize + `queue.write_texture` upload
//! on a cache miss.

use lru::LruCache;
use std::collections::HashMap;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// Atlas texture size in pixels (square). Matches the Web/WebGPU PoC.
pub(crate) const ATLAS_SIZE: u32 = 2048;
/// Padding (px) around each glyph's rasterized bitmap in the atlas, so bilinear
/// sampling at a glyph's edge never bleeds into its neighbor.
const CELL_PADDING: u32 = 2;
/// Vertical supersampling factor for the scanline rasterizer's anti-aliasing.
/// Horizontal coverage is computed exactly (fractional pixel overlap), so only
/// the Y axis needs supersampling.
const SUPERSAMPLE_Y: u32 = 4;

// ── Cache key ────────────────────────────────────────────────────────────────

/// Cache key for a rasterized glyph: glyph id + font size.
///
/// Never uses a raw `f32` as a hash/eq key (see repo `CLAUDE.md`): the size is
/// stored as its bit pattern via `f32::to_bits()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GlyphKey {
    pub glyph_id: u16,
    font_size_bits: u32,
}

impl GlyphKey {
    pub(crate) fn new(glyph_id: u16, font_size_px: f32) -> Self {
        Self {
            glyph_id,
            font_size_bits: font_size_px.to_bits(),
        }
    }
}

// ── Public slot / stats types ───────────────────────────────────────────────

/// Where a cached glyph lives in the atlas, plus the metrics needed to place its
/// screen-space quad (all in pixels).
#[derive(Debug, Clone, Copy)]
pub(crate) struct AtlasSlot {
    /// Top-left of the glyph's coverage bitmap inside the atlas texture (atlas
    /// pixels, i.e. NOT yet divided by `ATLAS_SIZE`).
    pub atlas_x: u32,
    pub atlas_y: u32,
    /// Glyph bitmap size in pixels (0x0 for whitespace / no-ink glyphs).
    pub glyph_w: f32,
    pub glyph_h: f32,
    /// Offset from the pen position to the bitmap's top-left corner, in pixels
    /// (`bearing_x` rightward, `bearing_y` downward from the baseline).
    pub bearing_x: f32,
    pub bearing_y: f32,
    /// Horizontal advance in font units scaled to pixels (informational: the
    /// real render/bench loop uses HarfBuzz's shaped `x_advance` instead, which
    /// accounts for kerning/GSUB — this is only the raw `hmtx` advance).
    #[allow(
        dead_code,
        reason = "informational metric, not read by the render loop on purpose (see doc comment); kept for API completeness / future consumers"
    )]
    pub advance: f32,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AtlasStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl AtlasStats {
    /// Hit rate in `[0, 1]`. An atlas that has never been queried reports `1.0`
    /// (vacuously "perfect"), matching `atlas.js`'s convention.
    pub(crate) fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            1.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

// ── Packer internals ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug)]
struct Shelf {
    next_x: u32,
    y: u32,
    height: u32,
}

/// Pure CPU-side shelf packer + LRU eviction bookkeeping. Holds no GPU handles,
/// so its allocation/eviction logic is unit-testable without a `wgpu::Device`.
#[derive(Debug)]
pub(crate) struct AtlasPacker {
    atlas_size: u32,
    shelves: Vec<Shelf>,
    next_shelf_y: u32,
    free_list: Vec<Rect>,
    rects: HashMap<GlyphKey, Rect>,
    cache: LruCache<GlyphKey, AtlasSlot>,
    stats: AtlasStats,
}

impl AtlasPacker {
    pub(crate) fn new(atlas_size: u32) -> Self {
        Self {
            atlas_size,
            shelves: Vec::new(),
            next_shelf_y: 0,
            free_list: Vec::new(),
            rects: HashMap::new(),
            cache: LruCache::unbounded(),
            stats: AtlasStats::default(),
        }
    }

    /// Looks up a cached slot, recording a hit/miss. Returns `None` on a miss —
    /// the caller must rasterize, `allocate`, upload, then `insert`.
    pub(crate) fn get(&mut self, key: GlyphKey) -> Option<AtlasSlot> {
        match self.cache.get(&key) {
            Some(slot) => {
                self.stats.hits += 1;
                Some(*slot)
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// Reserves a `padded_w x padded_h` rectangle for `key`, evicting LRU glyphs
    /// (reclaiming their rects to the free-list) until one fits, or `None` if
    /// even a fully-evicted atlas cannot fit it (glyph larger than the atlas).
    fn allocate(&mut self, padded_w: u32, padded_h: u32) -> Option<Rect> {
        loop {
            if let Some(rect) = self.take_from_free_list(padded_w, padded_h) {
                return Some(rect);
            }
            if let Some(shelf) = self
                .shelves
                .iter_mut()
                .find(|s| s.height >= padded_h && s.next_x + padded_w <= self.atlas_size)
            {
                let rect = Rect {
                    x: shelf.next_x,
                    y: shelf.y,
                    w: padded_w,
                    h: shelf.height,
                };
                shelf.next_x += padded_w;
                return Some(rect);
            }
            if self.next_shelf_y + padded_h <= self.atlas_size {
                let y = self.next_shelf_y;
                self.shelves.push(Shelf {
                    next_x: padded_w,
                    y,
                    height: padded_h,
                });
                self.next_shelf_y += padded_h;
                return Some(Rect {
                    x: 0,
                    y,
                    w: padded_w,
                    h: padded_h,
                });
            }
            if !self.evict_one() {
                return None; // atlas fully evicted and it still doesn't fit.
            }
        }
    }

    /// Best-fit search of the free-list. On a hit, splits off unused width past
    /// `REMAINDER_MIN` so it stays reusable instead of being lost as a sliver.
    fn take_from_free_list(&mut self, w: u32, h: u32) -> Option<Rect> {
        const REMAINDER_MIN: u32 = 4;

        let mut best_idx = None;
        let mut best_waste = u32::MAX;
        for (i, r) in self.free_list.iter().enumerate() {
            if r.w >= w && r.h >= h {
                let waste = (r.w - w) + (r.h - h);
                if waste < best_waste {
                    best_waste = waste;
                    best_idx = Some(i);
                }
            }
        }
        let idx = best_idx?;
        let r = self.free_list.swap_remove(idx);
        if r.w - w >= REMAINDER_MIN {
            self.free_list.push(Rect {
                x: r.x + w,
                y: r.y,
                w: r.w - w,
                h: r.h,
            });
        }
        Some(Rect {
            x: r.x,
            y: r.y,
            w,
            h: r.h,
        })
    }

    fn evict_one(&mut self) -> bool {
        match self.cache.pop_lru() {
            Some((key, _slot)) => {
                if let Some(rect) = self.rects.remove(&key) {
                    self.free_list.push(rect);
                }
                self.stats.evictions += 1;
                true
            }
            None => false,
        }
    }

    /// Reserves atlas space for a `padded_w x padded_h` glyph and returns its
    /// rect, or inserts a zero-size (no atlas space needed) entry directly for
    /// whitespace glyphs when `padded_w == 0 || padded_h == 0`.
    pub(crate) fn allocate_and_insert(
        &mut self,
        key: GlyphKey,
        padded_w: u32,
        padded_h: u32,
        make_slot: impl FnOnce(u32, u32) -> AtlasSlot,
    ) -> Option<AtlasSlot> {
        if padded_w == 0 || padded_h == 0 {
            let slot = make_slot(0, 0);
            self.cache.put(key, slot);
            return Some(slot);
        }
        let rect = self.allocate(padded_w, padded_h)?;
        let slot = make_slot(rect.x, rect.y);
        self.rects.insert(key, rect);
        self.cache.put(key, slot);
        Some(slot)
    }

    pub(crate) fn stats(&self) -> AtlasStats {
        self.stats
    }

    /// Fraction of the atlas's vertical space claimed by shelves so far, in
    /// `[0, 1]`. A rough occupancy signal (mirrors `atlas.js`'s `fillRatio`) —
    /// it does not account for free-list reclamation within already-claimed
    /// shelves, so it can read "full" while there is still reusable space.
    pub(crate) fn fill_ratio(&self) -> f64 {
        f64::from(self.next_shelf_y) / f64::from(self.atlas_size)
    }

    #[cfg(test)]
    fn cached_len(&self) -> usize {
        self.cache.len()
    }
}

// ── Outline flattening ──────────────────────────────────────────────────────

/// Collects a glyph outline's contours as flattened polylines: quadratic and
/// cubic Bezier segments are subdivided into straight line segments so the
/// rasterizer only ever has to deal with lines.
struct OutlineFlattener {
    contours: Vec<Vec<(f32, f32)>>,
    current: Vec<(f32, f32)>,
    last: (f32, f32),
}

impl OutlineFlattener {
    fn new() -> Self {
        Self {
            contours: Vec::new(),
            current: Vec::new(),
            last: (0.0, 0.0),
        }
    }

    fn finish(mut self) -> Vec<Vec<(f32, f32)>> {
        if !self.current.is_empty() {
            self.contours.push(self.current);
        }
        self.contours
    }
}

impl OutlineBuilder for OutlineFlattener {
    fn move_to(&mut self, x: f32, y: f32) {
        if !self.current.is_empty() {
            self.contours.push(std::mem::take(&mut self.current));
        }
        self.current.push((x, y));
        self.last = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push((x, y));
        self.last = (x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        const STEPS: u32 = 8;
        let (x0, y0) = self.last;
        for i in 1..=STEPS {
            let t = i as f32 / STEPS as f32;
            let mt = 1.0 - t;
            let px = mt * mt * x0 + 2.0 * mt * t * x1 + t * t * x;
            let py = mt * mt * y0 + 2.0 * mt * t * y1 + t * t * y;
            self.current.push((px, py));
        }
        self.last = (x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        const STEPS: u32 = 12;
        let (x0, y0) = self.last;
        for i in 1..=STEPS {
            let t = i as f32 / STEPS as f32;
            let mt = 1.0 - t;
            let px =
                mt * mt * mt * x0 + 3.0 * mt * mt * t * x1 + 3.0 * mt * t * t * x2 + t * t * t * x;
            let py =
                mt * mt * mt * y0 + 3.0 * mt * mt * t * y1 + 3.0 * mt * t * t * y2 + t * t * t * y;
            self.current.push((px, py));
        }
        self.last = (x, y);
    }

    fn close(&mut self) {
        // Contours are treated as implicitly closed by the rasterizer (it wraps
        // the last point back to the first), so there is nothing to record here.
    }
}

// ── Rasterizer ───────────────────────────────────────────────────────────────

/// A rasterized glyph: an R8 coverage bitmap plus the metrics needed to place it.
#[derive(Debug, Clone)]
pub(crate) struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    pub bearing_x: f32,
    pub bearing_y: f32,
    pub advance: f32,
    /// Row-major, top-to-bottom, `width * height` bytes. Empty for whitespace.
    pub coverage: Vec<u8>,
}

/// Rasterizes `glyph_id` from `face` at `font_size_px` into a coverage bitmap.
///
/// Returns `Some` even for glyphs with no outline (space, control chars) — with
/// `width == 0 && height == 0` and an empty `coverage` — so the caller can still
/// look up the advance width without special-casing "no outline" as an error.
/// Returns `None` only if the glyph id itself doesn't exist in the font.
#[allow(
    clippy::cast_possible_truncation,
    reason = "bbox px -> bitmap dimension is a deliberate floor/ceil, glyph bitmaps are always small"
)]
pub(crate) fn rasterize_glyph(
    face: &Face,
    glyph_id: GlyphId,
    font_size_px: f32,
) -> Option<GlyphBitmap> {
    let upem = f32::from(face.units_per_em());
    let scale = font_size_px / upem;
    let advance = face
        .glyph_hor_advance(glyph_id)
        .map(|a| f32::from(a) * scale)
        .unwrap_or(0.0);

    let mut flattener = OutlineFlattener::new();
    let Some(bbox) = face.outline_glyph(glyph_id, &mut flattener) else {
        // No outline: valid for whitespace/control glyphs, not an error.
        return Some(GlyphBitmap {
            width: 0,
            height: 0,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance,
            coverage: Vec::new(),
        });
    };
    let contours = flattener.finish();

    // Font space is Y-up; raster space is Y-down, so flip Y while scaling.
    let min_x = (bbox.x_min as f32 * scale).floor();
    let max_x = (bbox.x_max as f32 * scale).ceil();
    let min_y = (-(bbox.y_max as f32) * scale).floor();
    let max_y = (-(bbox.y_min as f32) * scale).ceil();

    let width = ((max_x - min_x).max(0.0)).ceil() as u32;
    let height = ((max_y - min_y).max(0.0)).ceil() as u32;
    if width == 0 || height == 0 {
        return Some(GlyphBitmap {
            width: 0,
            height: 0,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance,
            coverage: Vec::new(),
        });
    }

    let local: Vec<Vec<(f32, f32)>> = contours
        .iter()
        .map(|c| {
            c.iter()
                .map(|&(x, y)| (x * scale - min_x, -y * scale - min_y))
                .collect()
        })
        .collect();

    let coverage = rasterize_coverage(&local, width, height);

    Some(GlyphBitmap {
        width,
        height,
        bearing_x: min_x,
        bearing_y: min_y,
        advance,
        coverage,
    })
}

/// Scanline non-zero-winding fill with exact horizontal coverage and
/// `SUPERSAMPLE_Y`-way vertical supersampling. `contours` are already in
/// bitmap-local pixel space (origin at the bitmap's top-left).
#[allow(
    clippy::cast_possible_truncation,
    reason = "px/row coverage accumulation -> u8 is a deliberate quantization to 0..=255"
)]
fn rasterize_coverage(contours: &[Vec<(f32, f32)>], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;

    // Flattened directed edges (skip horizontal ones: they never cross a scanline).
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    for contour in contours {
        let n = contour.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (x0, y0) = contour[i];
            let (x1, y1) = contour[(i + 1) % n];
            if y0 != y1 {
                edges.push((x0, y0, x1, y1));
            }
        }
    }

    let mut out = vec![0u8; w * h];
    let mut row = vec![0.0f32; w];
    let row_weight = 1.0 / SUPERSAMPLE_Y as f32;

    for py in 0..h {
        row.iter_mut().for_each(|v| *v = 0.0);

        for sy in 0..SUPERSAMPLE_Y {
            let y = py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE_Y as f32;

            let mut crossings: Vec<(f32, i32)> = Vec::new();
            for &(x0, y0, x1, y1) in &edges {
                let (ylo, yhi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y < ylo || y >= yhi {
                    continue; // half-open range avoids double-counting shared vertices.
                }
                let t = (y - y0) / (y1 - y0);
                let x = x0 + t * (x1 - x0);
                let dir = if y1 > y0 { 1 } else { -1 };
                crossings.push((x, dir));
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));

            let mut winding = 0;
            for i in 0..crossings.len() {
                winding += crossings[i].1;
                if winding != 0 {
                    if let Some(&(x_end, _)) = crossings.get(i + 1) {
                        accumulate_span(&mut row, crossings[i].0, x_end, row_weight);
                    }
                }
            }
        }

        for (x, &coverage) in row.iter().enumerate() {
            out[py * w + x] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }

    out
}

/// Adds `weight` coverage to every pixel column overlapping `[x_start, x_end)`,
/// weighted by the fraction of that column the span actually covers.
#[allow(
    clippy::cast_possible_truncation,
    reason = "x_start/x_end are already clamped to [0, row.len()] above, so floor/ceil as usize cannot exceed row bounds"
)]
fn accumulate_span(row: &mut [f32], x_start: f32, x_end: f32, weight: f32) {
    let width = row.len() as f32;
    let x_start = x_start.clamp(0.0, width);
    let x_end = x_end.clamp(0.0, width);
    if x_end <= x_start {
        return;
    }
    let xs = x_start.floor() as usize;
    let xe = (x_end.ceil() as usize).min(row.len());
    for (x, cell) in row.iter_mut().enumerate().take(xe).skip(xs) {
        let left = x as f32;
        let right = left + 1.0;
        let overlap = (x_end.min(right) - x_start.max(left)).max(0.0);
        *cell += overlap * weight;
    }
}

// ── GPU-backed atlas ─────────────────────────────────────────────────────────

/// R8Unorm GPU texture atlas of rasterized glyph coverage bitmaps, LRU-cached.
pub(crate) struct GlyphAtlas {
    packer: AtlasPacker,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: u32,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("size", &self.size)
            .field("stats", &self.packer.stats())
            .finish_non_exhaustive()
    }
}

impl GlyphAtlas {
    pub(crate) fn new(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("glyph-atlas-view"),
            ..Default::default()
        });
        Self {
            packer: AtlasPacker::new(size),
            texture,
            view,
            size,
        }
    }

    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub(crate) fn stats(&self) -> AtlasStats {
        self.packer.stats()
    }

    /// See `AtlasPacker::fill_ratio`.
    pub(crate) fn fill_ratio(&self) -> f64 {
        self.packer.fill_ratio()
    }

    /// Gets the cached slot for `(glyph_id, font_size_px)`, rasterizing and
    /// uploading it into the GPU atlas on a miss. Returns `None` only when the
    /// glyph id doesn't exist in `face` at all.
    pub(crate) fn get_or_insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        face: &Face,
        glyph_id: GlyphId,
        font_size_px: f32,
    ) -> Option<AtlasSlot> {
        let _ = device; // texture already created in `new`; kept for API symmetry / future re-alloc.
        let key = GlyphKey::new(glyph_id.0, font_size_px);
        if let Some(slot) = self.packer.get(key) {
            return Some(slot);
        }

        let bitmap = rasterize_glyph(face, glyph_id, font_size_px)?;
        let padded_w = if bitmap.width == 0 {
            0
        } else {
            bitmap.width + CELL_PADDING * 2
        };
        let padded_h = if bitmap.height == 0 {
            0
        } else {
            bitmap.height + CELL_PADDING * 2
        };

        let bitmap_ref = &bitmap;
        let atlas_size = self.size;
        let texture = &self.texture;
        self.packer
            .allocate_and_insert(key, padded_w, padded_h, |rx, ry| {
                let atlas_x = if bitmap_ref.width == 0 {
                    0
                } else {
                    rx + CELL_PADDING
                };
                let atlas_y = if bitmap_ref.height == 0 {
                    0
                } else {
                    ry + CELL_PADDING
                };
                if bitmap_ref.width > 0 && bitmap_ref.height > 0 {
                    debug_assert!(atlas_x + bitmap_ref.width <= atlas_size);
                    debug_assert!(atlas_y + bitmap_ref.height <= atlas_size);
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: atlas_x,
                                y: atlas_y,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &bitmap_ref.coverage,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(bitmap_ref.width),
                            rows_per_image: Some(bitmap_ref.height),
                        },
                        wgpu::Extent3d {
                            width: bitmap_ref.width,
                            height: bitmap_ref.height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                AtlasSlot {
                    atlas_x,
                    atlas_y,
                    glyph_w: bitmap_ref.width as f32,
                    glyph_h: bitmap_ref.height as f32,
                    bearing_x: bitmap_ref.bearing_x,
                    bearing_y: bitmap_ref.bearing_y,
                    advance: bitmap_ref.advance,
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Rasterizer: synthetic contours, no font/GPU needed ─────────────────

    #[test]
    fn rasterize_coverage_square_has_ink_inside_and_none_outside() {
        // A 10x10 square, fully inside a 12x12 bitmap (1px margin all around).
        let square = vec![vec![(1.0, 1.0), (11.0, 1.0), (11.0, 11.0), (1.0, 11.0)]];
        let bitmap = rasterize_coverage(&square, 12, 12);

        // Center is fully covered.
        assert_eq!(bitmap[6 * 12 + 6], 255);
        // Every corner (outside the square) is empty.
        assert_eq!(bitmap[0], 0);
        assert_eq!(bitmap[11], 0);
        assert_eq!(bitmap[11 * 12], 0);
        assert_eq!(bitmap[11 * 12 + 11], 0);
    }

    #[test]
    fn rasterize_coverage_hole_via_opposite_winding_is_empty() {
        // Outer CCW square + inner CW square (opposite winding) = a ring. The
        // non-zero winding rule must leave the inner square uncovered.
        let outer = vec![(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
        let inner = vec![(5.0, 5.0), (5.0, 15.0), (15.0, 15.0), (15.0, 5.0)];
        let bitmap = rasterize_coverage(&[outer, inner], 20, 20);

        // Inside the ring (between outer and inner edges): covered.
        assert!(bitmap[2 * 20 + 10] > 200);
        // Inside the hole: empty.
        assert_eq!(bitmap[10 * 20 + 10], 0);
    }

    #[test]
    fn rasterize_glyph_real_font_produces_nonempty_bitmap() {
        // `AHEM` (a standard CSS-testing font: every glyph is a simple square/
        // rectangle) has a real `hhea` table, unlike `SIMPLE_GLYF` — needed
        // because `ttf_parser::Face::parse` requires one (`NoHheaTable`
        // otherwise). Glyph 0 has a real outline (verified against this font).
        let face = Face::parse(font_test_data::AHEM, 0).expect("valid test font");
        let bitmap = rasterize_glyph(&face, GlyphId(0), 32.0).expect("glyph exists");
        assert!(
            bitmap.width > 0 && bitmap.height > 0,
            "expected a real outline"
        );
        assert!(
            bitmap.coverage.iter().any(|&c| c > 0),
            "expected at least one covered pixel"
        );
    }

    // ── Packer: pure CPU bookkeeping, no GPU needed ─────────────────────────

    fn key(id: u16) -> GlyphKey {
        GlyphKey::new(id, 16.0)
    }

    #[test]
    fn packer_hit_rate_tracks_repeated_lookups() {
        let mut packer = AtlasPacker::new(256);
        assert!(packer.get(key(1)).is_none()); // miss
        packer.allocate_and_insert(key(1), 10, 10, |x, y| AtlasSlot {
            atlas_x: x,
            atlas_y: y,
            glyph_w: 8.0,
            glyph_h: 8.0,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance: 10.0,
        });
        assert!(packer.get(key(1)).is_some()); // hit
        assert!(packer.get(key(1)).is_some()); // hit

        let stats = packer.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn packer_evicts_lru_when_atlas_is_full() {
        // A tiny atlas (16x16) so a handful of 10x10-padded glyphs force eviction.
        let mut packer = AtlasPacker::new(16);

        let slot1 = packer
            .allocate_and_insert(key(1), 10, 10, |x, y| AtlasSlot {
                atlas_x: x,
                atlas_y: y,
                glyph_w: 10.0,
                glyph_h: 10.0,
                bearing_x: 0.0,
                bearing_y: 0.0,
                advance: 10.0,
            })
            .expect("first glyph fits in an empty atlas");
        assert_eq!((slot1.atlas_x, slot1.atlas_y), (0, 0));

        // Touch glyph 1 so it is the most-recently-used entry.
        assert!(packer.get(key(1)).is_some());

        // A second, different glyph doesn't fit next to the first one (10+10 >
        // 16 wide) and doesn't fit on a new shelf either (10+10 > 16 tall), so
        // it must evict glyph 1 (the only entry, hence trivially LRU) to fit.
        let slot2 = packer.allocate_and_insert(key(2), 10, 10, |x, y| AtlasSlot {
            atlas_x: x,
            atlas_y: y,
            glyph_w: 10.0,
            glyph_h: 10.0,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance: 10.0,
        });
        assert!(
            slot2.is_some(),
            "second glyph should reuse the evicted slot"
        );
        assert_eq!(packer.stats().evictions, 1);
        assert_eq!(packer.cached_len(), 1); // glyph 1 was evicted, only glyph 2 remains.
        assert!(packer.get(key(1)).is_none()); // confirms eviction, not a packing bug.
    }

    #[test]
    fn packer_whitespace_glyph_needs_no_atlas_space() {
        let mut packer = AtlasPacker::new(256);
        let slot = packer
            .allocate_and_insert(key(1), 0, 0, |_, _| AtlasSlot {
                atlas_x: 0,
                atlas_y: 0,
                glyph_w: 0.0,
                glyph_h: 0.0,
                bearing_x: 0.0,
                bearing_y: 0.0,
                advance: 6.0,
            })
            .expect("whitespace slot always succeeds");
        assert_eq!(slot.glyph_w, 0.0);
        assert_eq!(packer.stats().evictions, 0);
    }
}
