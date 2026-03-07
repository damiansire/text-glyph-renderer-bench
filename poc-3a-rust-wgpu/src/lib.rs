//! poc-3a-rust-wgpu — TextBuffer abstraction layer
//!
//! Defines the core `TextBuffer` trait that all storage backends implement,
//! plus re-exports the `PieceTable` and `LineIndex` implementations.

pub mod buffer;

pub use buffer::{
    line_index::LineIndex,
    piece_table::{BufferSnapshot, PieceTable},
    TextBuffer,
};
