//! poc-3b-rust-vello — public library surface for benchmarks and integration tests.

pub mod buffer;

pub use buffer::{
    BufferSnapshot,
    line_index::LineIndex,
    piece_table::PieceTable,
    TextBuffer,
};
