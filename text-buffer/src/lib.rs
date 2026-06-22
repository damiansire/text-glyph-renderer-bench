//! text-buffer — `TextBuffer` trait + zero-copy piece table over mmap.
//!
//! Shared storage layer for the Rust PoCs (`poc-3a-rust-wgpu`,
//! `poc-3b-rust-vello`). The PoCs differ only in their renderer; the text
//! buffer lives here so a fix lands once instead of being copy-pasted into
//! both crates (which had already started to diverge).

// Every public item in this crate must be documented (audit P11). The crate is
// the shared contract consumed by both Rust PoCs, so an undocumented public API
// is a real gap, not a style nit.
#![deny(missing_docs)]

pub mod line_index;
pub mod piece_table;

use std::ops::Range;

pub use line_index::LineIndex;
pub use piece_table::PieceTable;

/// Snapshot ref-counted immutable view into the buffer.
/// Cheap to *clone* (O(1) Arc bump); used by the renderer to avoid holding a
/// mutable borrow on the buffer while the GPU is consuming vertex data.
///
/// Note: *creating* the snapshot (`PieceTable::snapshot()`) currently
/// materialises the logical content once (O(N) copy). The zero-copy
/// single-piece fast-path is not implemented — see `snapshot()`.
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

    /// Like [`bytes_in_range`], but appends the bytes into a caller-owned
    /// `Vec<u8>` instead of allocating a fresh one per call.
    ///
    /// This is an **ergonomics** helper, not a performance fix: the Fase 2
    /// measurement found the per-call allocation of `bytes_in_range`
    /// (~34 ns over a 8.3 ms frame budget — 0.0004 %) to be negligible. It
    /// exists for callers that already need a contiguous buffer across many
    /// calls (e.g. a per-frame scratch buffer) and want to amortise the
    /// allocation by reusing it. The buffer is **not** cleared first, so the
    /// caller controls whether to append or `clear()` beforehand.
    ///
    /// Default impl is provided in terms of `slice_pieces`; backends may
    /// override for a tighter loop.
    ///
    /// [`bytes_in_range`]: TextBuffer::bytes_in_range
    fn bytes_in_range_into(&self, range: Range<usize>, out: &mut Vec<u8>) {
        out.reserve(range.len());
        self.slice_pieces(range, |slice, _| {
            out.extend_from_slice(slice);
            true
        });
    }

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
    /// O(N): the current implementation always materialises the content into
    /// a fresh allocation. (A true O(1) single-piece fast-path that shares the
    /// mmap is not implemented yet.)
    ///
    /// The snapshot is a point in time: mutating the buffer afterwards does not
    /// change a snapshot already taken.
    ///
    /// # Examples
    ///
    /// ```
    /// use text_buffer::{PieceTable, TextBuffer};
    ///
    /// let mut pt = PieceTable::from_bytes(b"hello".to_vec()).unwrap();
    /// let snap = pt.snapshot();
    /// pt.insert(5, " world");
    /// assert_eq!(snap.as_ref().as_ref(), b"hello");
    /// assert_eq!(pt.bytes_in_range(0..pt.byte_len()), b"hello world");
    /// ```
    fn snapshot(&self) -> BufferSnapshot;
}
