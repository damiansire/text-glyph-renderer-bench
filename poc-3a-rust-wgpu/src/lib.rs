//! poc-3a-rust-wgpu — TextBuffer abstraction layer
//!
//! The storage backend (piece table + line index) lives in the shared
//! `text-buffer` crate so it is not copy-pasted between PoC 3A and 3B. This
//! crate re-exports it; the PoC-specific code is the (future) wgpu renderer.

pub use text_buffer::{BufferSnapshot, LineIndex, PieceTable, TextBuffer};
