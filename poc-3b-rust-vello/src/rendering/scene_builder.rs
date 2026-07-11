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

use skrifa::instance::LocationRef;
use skrifa::prelude::Size;
use skrifa::raw::FontRef;
use skrifa::MetadataProvider;
use std::sync::Arc;
use vello::kurbo::Affine;
use vello::peniko::{Blob, Color, Fill, FontData};
use vello::{Glyph, Scene};

// ── FontData wrapper ──────────────────────────────────────────────────────────

pub(crate) struct VelloFont {
    pub(crate) data: Arc<Vec<u8>>,
    font_size: f32,
}

impl VelloFont {
    pub(crate) fn load(path: &std::path::Path, size_px: f32) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        // Validate the font *contents* once here (not just that the path
        // exists): a corrupt/malformed font is a recoverable input error, so we
        // surface it as `io::Error` instead of panicking later, per glyph, in
        // the `build_scene` hot loop. `FontRef` borrows `data`, so it is dropped
        // before we move `data` into the struct.
        if FontRef::new(&data).is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid or unsupported font file: {}", path.display()),
            ));
        }
        Ok(Self {
            data: Arc::new(data),
            font_size: size_px,
        })
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

    /// Builds a real Vello `Scene` for the given viewport: for each visible
    /// line, shapes the text with skrifa (charmap + advance widths) and
    /// encodes the resulting glyph run into the scene via
    /// `Scene::draw_glyphs`, which is the current (vello 0.7) API for glyph
    /// geometry — superseding the removed-in-0.2 `GlyphProvider` that the
    /// previous version of this doc comment referenced. Positions are laid
    /// out left-to-right per line with the font's own advance widths; no
    /// line-wrapping or bidi is performed (out of scope for this PoC).
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
        scroll_y: f64,
    ) -> Scene {
        let mut scene = Scene::new();

        // Nothing to traverse for an empty document. Guard before computing
        // `lines.len() - 1`, which would underflow on an empty slice. Also bail
        // when `first_line` is past the end: otherwise `lines[first_line..=..]`
        // would have `start > end` and panic on the slice.
        if first_line >= lines.len() {
            return scene;
        }

        let font_size = Size::new(self.font.font_size);
        // Invariant: `VelloFont::load` already validated these bytes parse as a
        // font (and the data is immutable behind `Arc`), so this cannot fail.
        let font_ref = FontRef::new(&self.font.data).expect("font validated at load");
        let charmap = font_ref.charmap();
        let location = LocationRef::default(); // non-variable font: default axis positions.
        let glyph_metrics = font_ref.glyph_metrics(font_size, location);

        let text_color = Color::from_rgb8(0xC9, 0xD1, 0xD9);
        let font_bytes = self.font.data.clone() as Arc<dyn AsRef<[u8]> + Send + Sync>;
        let font_data = FontData::new(Blob::new(font_bytes), 0);

        let last_line = (first_line + (viewport_h / line_height) as usize + 2).min(lines.len() - 1);

        for (row, line) in lines[first_line..=last_line].iter().enumerate() {
            // Decode lossily so invalid UTF-8 is replaced (U+FFFD) and still
            // counted, rather than silently dropping the whole line.
            let text = String::from_utf8_lossy(line);
            let mut pen_x = 0_f32;
            #[allow(
                clippy::cast_possible_truncation,
                reason = "row index -> pixel y offset, viewport-bounded"
            )]
            let pen_y = (row as f64 * line_height - scroll_y % line_height) as f32;

            let glyphs = text.chars().map(|ch| {
                let gid = charmap.map(ch).unwrap_or_default();
                let advance = glyph_metrics.advance_width(gid).unwrap_or_default();
                let x = pen_x;
                pen_x += advance;
                Glyph {
                    id: gid.to_u32(),
                    x,
                    y: pen_y,
                }
            });

            scene
                .draw_glyphs(&font_data)
                .font_size(self.font.font_size)
                .transform(Affine::IDENTITY)
                .brush(text_color)
                .draw(Fill::NonZero, glyphs);
        }

        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a real (tiny, valid) test font to a temp file and loads it —
    /// `VelloFont::load` reads from a path, so tests need a real file on disk,
    /// not just bytes in memory.
    fn load_test_font() -> VelloFont {
        let path = std::env::temp_dir().join(format!(
            "tglrb-test-font-{}-{:?}.ttf",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, font_test_data::SIMPLE_GLYF).expect("write temp test font");
        let font = VelloFont::load(&path, 13.0).expect("load valid test font");
        let _ = std::fs::remove_file(&path); // best-effort cleanup, not test-critical.
        font
    }

    #[test]
    fn build_scene_encodes_real_glyph_geometry_for_visible_lines() {
        let builder = TextSceneBuilder::new(load_test_font());
        let lines: Vec<&[u8]> = vec![b"hello", b"world"];

        let scene = builder.build_scene(&lines, 0, 100.0, 20.0, 0.0);

        // The whole point of this fix: `build_scene` used to return an empty
        // `Scene` unconditionally (see the CHANGELOG / DECISIONES.md). A real
        // fix must produce at least one glyph run with at least one glyph.
        let glyph_runs = &scene.encoding().resources.glyph_runs;
        assert!(
            !glyph_runs.is_empty(),
            "expected at least one encoded glyph run, scene is still empty"
        );
        let total_glyphs: usize = glyph_runs.iter().map(|run| run.glyphs.len()).sum();
        assert!(
            total_glyphs > 0,
            "glyph runs were encoded but contain zero glyphs"
        );
    }

    #[test]
    fn build_scene_on_empty_document_returns_empty_scene() {
        let builder = TextSceneBuilder::new(load_test_font());
        let lines: Vec<&[u8]> = vec![];

        let scene = builder.build_scene(&lines, 0, 100.0, 20.0, 0.0);

        assert!(scene.encoding().resources.glyph_runs.is_empty());
    }

    #[test]
    fn build_scene_first_line_past_end_returns_empty_scene() {
        let builder = TextSceneBuilder::new(load_test_font());
        let lines: Vec<&[u8]> = vec![b"only line"];

        let scene = builder.build_scene(&lines, 5, 100.0, 20.0, 0.0);

        assert!(scene.encoding().resources.glyph_runs.is_empty());
    }
}
