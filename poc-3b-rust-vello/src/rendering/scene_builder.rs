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

pub(crate) struct VelloFont {
    pub(crate) data: Arc<Vec<u8>>,
    font_size: f32,
}

impl VelloFont {
    pub(crate) fn load(path: &std::path::Path, size_px: f32) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(Self { data: Arc::new(data), font_size: size_px })
    }
}

// ── SceneBuilder helper ───────────────────────────────────────────────────────

pub(crate) struct TextSceneBuilder {
    font: VelloFont,
}

impl TextSceneBuilder {
    pub(crate) fn new(font: VelloFont) -> Self {
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "viewport height / line height -> visible line count is a deliberate floor"
    )]
    pub(crate) fn build_scene(
        &self,
        lines: &[&[u8]],
        first_line: usize,
        viewport_h: f64,
        line_height: f64,
        _scroll_y: f64,
    ) -> Scene {
        let scene = Scene::new();

        // Nothing to traverse for an empty document. Guard before computing
        // `lines.len() - 1`, which would underflow on an empty slice. Also bail
        // when `first_line` is past the end: otherwise `lines[first_line..=..]`
        // would have `start > end` and panic on the slice.
        if first_line >= lines.len() {
            return scene;
        }

        let _font_size = Size::new(self.font.font_size);
        let font_ref  = FontRef::new(&self.font.data).expect("valid font");
        let charmap   = font_ref.charmap();

        let _text_color = Color::from_rgb8(0xC9, 0xD1, 0xD9);

        let last_line = (first_line + (viewport_h / line_height) as usize + 2).min(lines.len() - 1);

        // As the method docs state, the only per-glyph cost emulated here is the
        // charmap lookup; no geometry (positions/outlines) is produced.
        for line in &lines[first_line..=last_line] {
            // Decode lossily so invalid UTF-8 is replaced (U+FFFD) and still
            // counted, rather than silently dropping the whole line.
            let text = String::from_utf8_lossy(line);
            for ch in text.chars() {
                let _glyph_id = charmap.map(ch).unwrap_or_default();
            }
        }

        scene
    }
}
