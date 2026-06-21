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

use skrifa::prelude::Size;
use skrifa::raw::FontRef;
use skrifa::MetadataProvider;
use vello::peniko::Color;
use vello::Scene;
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

    /// Emulate the **CPU cost** of building a Vello `Scene` for the given
    /// viewport. **This does NOT build any geometry**: it walks the visible
    /// lines and maps each char to a glyph id (the charmap lookup cost), but it
    /// appends nothing to the scene and returns an **empty** `Scene`. Vello's
    /// `GlyphProvider` was removed in vello 0.2, so the real outline/`GlyphRun`
    /// path is not wired up here — see the PoC status in the README.
    ///
    /// `lines`:       all lines pre-split from the file
    /// `first_line`:  first visible line (0-based)
    /// `viewport_h`:  viewport height in pixels
    /// `line_height`: line height in pixels
    /// `scroll_y`:    vertical scroll offset in pixels
    pub fn build_scene(
        &self,
        lines: &[&[u8]],
        first_line: usize,
        viewport_h: f64,
        line_height: f64,
        scroll_y: f64,
    ) -> Scene {
        let scene = Scene::new();

        // Nothing to traverse for an empty document. Guard before computing
        // `lines.len() - 1`, which would underflow on an empty slice.
        if lines.is_empty() {
            return scene;
        }

        let _font_size = Size::new(self.font.font_size);
        let font_ref  = FontRef::new(&self.font.data).expect("valid font");
        let charmap   = font_ref.charmap();

        let _text_color = Color::from_rgb8(0xC9, 0xD1, 0xD9);

        let last_line = (first_line + (viewport_h / line_height) as usize + 2).min(lines.len() - 1);

        for li in first_line..=last_line {
            let _y   = li as f64 * line_height - scroll_y;
            let line = lines[li];
            // Decode lossily so invalid UTF-8 is replaced (U+FFFD) and still
            // counted, rather than silently dropping the whole line.
            let text = String::from_utf8_lossy(line);

            let mut x = 52.0_f64; // left margin (line number space).

            for ch in text.chars() {
                let _glyph_id = charmap.map(ch).unwrap_or_default();
                // NOTE: a real Vello app would build the path from the skrifa
                // outline and append a GlyphRun to `scene`. Here we only pay the
                // charmap lookup; no geometry is produced (see method docs).

                // Advance (approximate using EM width for now).
                x += self.font.font_size as f64 * 0.6; // monospace em width approximation.
            }
        }

        scene
    }
}
