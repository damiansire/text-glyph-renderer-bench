//! rendering — Real GPU render path for PoC 3A (Rust + wgpu + rustybuzz).
//!
//! Architecture (see `atlas.rs` / `renderer.rs` doc comments for detail):
//!   CPU rasterizes each glyph outline once (scanline non-zero-winding fill over
//!   line segments flattened from `ttf-parser` outlines) into an R8Unorm coverage
//!   bitmap, uploads it into an LRU-cached GPU texture atlas, then draws one
//!   instanced textured quad per visible glyph in a single draw call. This is the
//!   CPU-rasterize-to-atlas architecture (mirrors `poc-1c-webgpu-atlas`), distinct
//!   from PoC 3B's GPU-vector-curves (Vello) approach.

pub(crate) mod atlas;
pub(crate) mod renderer;
