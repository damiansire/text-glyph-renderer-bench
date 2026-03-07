//! buffer module for PoC 3B — re-exports PieceTable and TextBuffer from 3A logic.
//! (In a real workspace shared crate this would be a `use poc_3a_rust_wgpu::...`)

pub mod piece_table;
pub mod line_index;

pub use line_index::LineIndex;
pub use piece_table::PieceTable;

use std::ops::Range;
use std::sync::Arc;

pub type BufferSnapshot = Arc<dyn AsRef<[u8]> + Send + Sync>;

pub trait TextBuffer: Send + Sync {
    fn byte_len(&self) -> usize;
    fn line_count(&self) -> usize;
    fn line_start_byte(&self, line: usize) -> usize;
    fn byte_to_line(&self, byte_offset: usize) -> usize;
    fn bytes_in_range(&self, range: Range<usize>) -> Vec<u8>;
    fn slice_pieces<F>(&self, range: Range<usize>, f: F) where F: FnMut(&[u8], usize) -> bool;
    fn insert(&mut self, byte_offset: usize, text: &str);
    fn delete(&mut self, range: Range<usize>);
    fn snapshot(&self) -> BufferSnapshot;
}
