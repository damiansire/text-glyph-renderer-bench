# Common Abstraction Layer

> **Principle:** Any data structure must be pluggable into any rendering backend — three orthogonal interfaces.

---

## `TextBuffer`

Manages text storage and mutation. Both Rope and Piece Table implement this contract.

```
TextBuffer {
  // Load without copies from fd+mmap
  load_mmap(fd: FileDescriptor, len: usize) -> Self

  // Zero-copy byte range access
  slice(byte_start: usize, byte_end: usize) -> &[u8]

  // Mutations
  insert(at: usize, text: &str) -> ()
  delete(range: Range<usize>) -> ()

  // Line iterators (for SIMD scan)
  line_offsets() -> &[usize]          // pre-computed \n jump slice
  line_at_byte(byte: usize) -> usize  // binary search O(log n)

  // Immutable snapshots for the renderer
  snapshot() -> BufferSnapshot         // ref-counted, O(1)
}
```

---

## `ShapingEngine`

Abstracts HarfBuzz (Rust), CoreText (Swift/C), and libshaping web (JS).

```
ShapingEngine {
  shape(text: &str, font: FontId, size: f32, features: &[Feature]) -> GlyphRun

  // Async font fallback
  resolve_fallback(codepoint: u32, qos: QoSClass) -> Future<FontId>

  // LRU cache by (text_hash, font_id, size)
  invalidate_cache(font_id: FontId) -> ()
}

GlyphRun {
  glyphs: Vec<GlyphId>,
  advances: Vec<f32>,     // in font design units
  clusters: Vec<u32>,     // glyph → UTF-16 cluster mapping
  bbox: Rect,
}
```

---

## `Renderer`

The rendering frontend. All PoCs implement this trait.

```
Renderer {
  begin_frame(viewport: Rect, scroll_y: f64) -> FrameContext
  submit_glyphs(ctx: &FrameContext, runs: &[PositionedRun]) -> ()
  end_frame(ctx: FrameContext) -> PresentationTimestamp
  last_frame_stats() -> FrameStats
}

PositionedRun {
  run: GlyphRun,
  origin: Vec2,       // baseline origin in viewport coordinates
  color: ColorRgba,
}

FrameStats {
  cpu_encode_ns: u64,
  gpu_render_ns: u64,
  atlas_fill_ratio: f32,
  dropped_frames: u32,
}
```
