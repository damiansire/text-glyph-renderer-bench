//! poc-3b-rust-vello — public library surface for benchmarks and integration tests.
//!
//! The text buffer is the shared `text-buffer` crate (same as PoC 3A); this
//! crate re-exports it so 3B differs from 3A only in its renderer.

pub use text_buffer::{BufferSnapshot, LineIndex, PieceTable, TextBuffer};
