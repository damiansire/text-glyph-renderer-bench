//! scene_builder.rs — Builds Vello scenes from a PieceTable viewport.
//!
//! Vello's rendering model:
//!   Instead of rasterizing glyphs to a texture atlas (PoC 3A), Vello sends
//!   Bézier curve data to the GPU as a scene graph.  A series of Compute Shader
//!   passes flatten the curves, compute coverage, and composite the result.
//!   This eliminates the CPU rasterization step entirely — the GPU evaluates
//!   curves on-demand for each output pixel.
//!
//! Scene construction:
//!   For each visible line we build a `vello::glyph::GlyphRun` which encodes
//!   the glyph IDs and positions.  Vello handles font loading, glyph outline
//!   extraction (via skrifa), and GPU submission automatically.

use vello::glyph::{GlyphContext, GlyphProvider};
use vello::kurbo::{Affine, Point};
use vello::peniko::{Brush, Color, Fill};
use vello::{Scene, SceneBuilder};
use skrifa::{FontRef, MetadataProvider};
use skrifa::instance::{Location, NormalizedCoord};
use skrifa::raw::FontData;

use std::sync::Arc;

// ── FontData wrapper ──────────────────────────────────────────────────────────

pub struct VelloFont {
    pub data: Arc<Vec<u8>>,
    font_size: f32,
}

impl VelloFont {
    pub fn load(path: &std::path::Path, size_px: f32) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(Self { data: Arc::new(data), font_size: size_px })
    }
}

// ── SceneBuilder helper ───────────────────────────────────────────────────────

pub struct TextSceneBuilder {
    font: VelloFont,
}

impl TextSceneBuilder {
    pub fn new(font: VelloFont) -> Self {
        Self { font }
    }

    /// Build a Vello `Scene` for the given viewport.
    ///
    /// `lines`:       all lines pre-split from the file
    /// `first_line`:  first visible line (0-based)
    /// `viewport_h`:  viewport height in pixels
    /// `line_height`: line height in pixels
    /// `scroll_y`:    vertical scroll offset in pixels
    ///
    /// Returns a `Scene` ready for submission to `vello::Renderer`.
    pub fn build_scene(
        &self,
        lines: &[&[u8]],
        first_line: usize,
        viewport_h: f64,
        line_height: f64,
        scroll_y: f64,
    ) -> Scene {
        let mut scene = Scene::new();
        let mut sb    = SceneBuilder::for_scene(&mut scene);

        let font_size = vello::glyph::skrifa::instance::Size::new(self.font.font_size);
        let font_data = FontData::new(&self.font.data);
        let font_ref  = FontRef::from_index(font_data, 0).expect("valid font");
        let charmap    = font_ref.charmap();
        let hmetrics   = font_ref.horizontal_metrics(font_data, font_size, &Location::default());
        let glyphs_per_em = font_ref.head().map_or(1000u16, |h| h.units_per_em());
        let scale = self.font.font_size / glyphs_per_em as f32;

        let text_color = Color::rgb8(0xC9, 0xD1, 0xD9);
        let mut ctx    = GlyphContext::new();

        let last_line = (first_line + (viewport_h / line_height) as usize + 2).min(lines.len() - 1);

        for li in first_line..=last_line {
            let y    = li as f64 * line_height - scroll_y;
            let line = lines[li];
            let text = std::str::from_utf8(line).unwrap_or("");

            // Encode glyph run using Vello's GlyphProvider
            let mut provider = ctx.new_provider(&font_ref, None, self.font.font_size, false, Affine::IDENTITY);
            let mut x = 52.0_f64; // left margin (line number space)

            for ch in text.chars() {
                let glyph_id = charmap.map(ch).unwrap_or_default();
                if let Some(glyph) = provider.get(glyph_id, None) {
                    sb.append(&glyph, Some(Affine::translate((x, y))));
                }
                // Advance (approximate using EM width for now)
                x += self.font.font_size as f64 * 0.6; // monospace em width approximation
            }
        }

        // Clear background (Vello composites on transparent; caller fills bg)
        drop(sb);
        scene
    }
}
