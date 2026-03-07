//! buffer module — TextBuffer trait + implementations

pub mod line_index;
pub mod piece_table;

use std::ops::Range;

/// Snapshot ref-counted immutable view into the buffer.
/// Cheap to clone (O(1)); used by the renderer to avoid holding a mutable
/// borrow on the buffer while the GPU is consuming vertex data.
pub type BufferSnapshot = std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>;

/// Common interface for all text storage backends (Piece Table, Rope, …).
///
/// Implementations must be `Send + Sync` so they can be shared across
/// the render thread and the shaping thread pool.
pub trait TextBuffer: Send + Sync {
    // ── Metadata ──────────────────────────────────────────────────────────

    /// Total number of bytes in the logical buffer.
    fn byte_len(&self) -> usize;

    /// Total number of lines (number of `\n` characters + 1).
    fn line_count(&self) -> usize;

    // ── Line ↔ byte offset conversion ────────────────────────────────────

    /// Returns the byte offset of the start of `line` (0-based).
    /// Panics if `line >= line_count()`.
    fn line_start_byte(&self, line: usize) -> usize;

    /// Returns the line number that contains `byte_offset`.
    /// Uses binary search on the pre-computed line index (O(log N)).
    fn byte_to_line(&self, byte_offset: usize) -> usize;

    // ── Content access ────────────────────────────────────────────────────

    /// Returns a `Vec<u8>` containing the bytes in the given byte range.
    ///
    /// For Piece Table over mmap the range may span multiple pieces;
    /// this method assembles them with a single allocation.
    /// For a zero-piece / single-piece table (pure read of mmap) this is
    /// a simple `slice.to_vec()` — one copy, unavoidable at the API level.
    ///
    /// **Prefer `slice_piece`** for zero-copy reads when the caller can
    /// process data piece by piece.
    fn bytes_in_range(&self, range: Range<usize>) -> Vec<u8>;

    /// Iterate over (data_slice, piece_offset) tuples that together cover
    /// the given byte range, without copying.  The closure returns `false`
    /// to stop early.
    fn slice_pieces<F>(&self, range: Range<usize>, f: F)
    where
        F: FnMut(&[u8], usize) -> bool;

    // ── Mutations ─────────────────────────────────────────────────────────

    /// Insert UTF-8 `text` at `byte_offset`.
    fn insert(&mut self, byte_offset: usize, text: &str);

    /// Delete the bytes in `range`.
    fn delete(&mut self, range: Range<usize>);

    // ── Snapshot ──────────────────────────────────────────────────────────

    /// Return an immutable Arc snapshot of the current logical content.
    /// O(1) unless the implementation needs to materialise the content.
    fn snapshot(&self) -> BufferSnapshot;
}
