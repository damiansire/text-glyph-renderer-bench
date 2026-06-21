//! line_index.rs — SIMD-accelerated line start offset table.
//!
//! Uses `memchr::memchr_iter` which on aarch64 (Apple Silicon) compiles to
//! NEON `vceqq_u8` + `vmaxvq_u8` vectorized scan — 16 bytes per cycle.
//!
//! The result is a `Vec<u32>` of byte offsets for the start of each line:
//!   index[0] = 0          (line 0 always starts at byte 0)
//!   index[k] = offset of byte immediately after the (k-1)'th `\n`
//!
//! This table is built **once** during file load and never mutated during
//! scroll.  After text mutations it is rebuilt by `PieceTable` before the
//! next render frame that requires line information.

use memchr::memchr_iter;

/// Pre-computed line start offset table.
///
/// Uses `u32` offsets — sufficient for files up to 4 GB.
/// For files > 4 GB, promote to `u64`.
#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    /// Byte offset of the start of each line (0-based).
    /// `offsets[0]` is always `0`.  `offsets.len() == line_count`.
    offsets: Vec<u32>,
}

impl LineIndex {
    /// Narrow a byte position into the `u32` used to store line offsets.
    ///
    /// The index addresses files up to 4 GB by design (see the struct docs), so
    /// every position fits in `u32`. The invariant is asserted in debug builds
    /// and the truncation in release is deliberate — this is the
    /// `clippy::cast_possible_truncation` site the repo calls out as highest ROI,
    /// now guarded at the source instead of left as a silent `as u32`.
    #[inline]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "line offsets are capped at 4 GB by design; the debug_assert guards the invariant"
    )]
    fn narrow(byte_pos: usize) -> u32 {
        debug_assert!(
            byte_pos <= u32::MAX as usize,
            "line offset {byte_pos} exceeds u32::MAX (file larger than 4 GB)"
        );
        byte_pos as u32
    }

    // ── Construction ─────────────────────────────────────────────────────

    /// Build a `LineIndex` by SIMD-scanning `data` for `\n` bytes.
    ///
    /// Complexity: O(N / 16) with NEON vectorization on aarch64.
    ///
    /// On a M-series chip scanning 100 MB: ~6.25 M cycles / ~2 ms at 3 GHz.
    pub fn build(data: &[u8]) -> Self {
        // Pre-allocate: estimate 1 line per 45 bytes (reasonable for mixed text).
        let estimated = (data.len() / 45).max(64);
        let mut offsets: Vec<u32> = Vec::with_capacity(estimated);

        // Line 0 always starts at byte 0.
        offsets.push(0);

        // `memchr_iter` uses NEON on aarch64, SSE2/AVX2 on x86_64.
        // Each found `\n` at position `i` means line `k+1` starts at `i+1`.
        for newline_pos in memchr_iter(b'\n', data) {
            let next_line_start = Self::narrow(newline_pos + 1);
            offsets.push(next_line_start);
        }

        Self { offsets }
    }

    /// Empty index (used for zero-length buffers or before the first build).
    pub fn empty() -> Self {
        Self { offsets: vec![0] }
    }

    // ── Queries ───────────────────────────────────────────────────────────

    /// Total number of lines.
    #[inline]
    pub fn count(&self) -> usize {
        self.offsets.len()
    }

    /// Byte offset where `line` starts (0-based line number).
    ///
    /// # Panics
    /// Panics if `line >= count()`.
    #[inline]
    pub fn line_start(&self, line: usize) -> usize {
        self.offsets[line] as usize
    }

    /// Byte offset one past the last byte of `line` (i.e., start of next line,
    /// or end of buffer for the last line).
    ///
    /// Useful for slicing: `&data[line_start(n)..line_end(n, buf_len)]`
    #[inline]
    pub fn line_end(&self, line: usize, buf_len: usize) -> usize {
        if line + 1 < self.offsets.len() {
            self.offsets[line + 1] as usize
        } else {
            buf_len
        }
    }

    /// Returns the 0-based line number that contains `byte_offset`.
    ///
    /// Uses binary search: O(log L) where L = number of lines.
    /// For a 100 MB file with ~1.3 M lines: ~21 comparisons.
    #[inline]
    pub fn byte_to_line(&self, byte_offset: usize) -> usize {
        let target = Self::narrow(byte_offset);
        match self.offsets.binary_search(&target) {
            // Exact match: this byte IS the start of a line.
            Ok(line) => line,
            // Not found: the insertion point is the line AFTER the one we want.
            Err(next_line) => next_line.saturating_sub(1),
        }
    }

    /// Returns a slice of line start offsets for lines in `[first_line, last_line)`.
    ///
    /// Used by the renderer to iterate over a viewport range without allocating.
    #[inline]
    pub fn line_range_offsets(&self, first_line: usize, last_line: usize) -> &[u32] {
        let end = last_line.min(self.offsets.len());
        &self.offsets[first_line..end]
    }

    /// Return the (first_line, last_line) range that covers the byte range
    /// `[byte_start, byte_end)`.  Used for viewport culling: find which lines
    /// are visible given the scroll offset and viewport height in bytes.
    pub fn lines_for_byte_range(&self, byte_start: usize, byte_end: usize) -> (usize, usize) {
        let first = self.byte_to_line(byte_start);
        let last = self.byte_to_line(byte_end.saturating_sub(1)).min(self.offsets.len() - 1);
        (first, last)
    }

    // ── Incremental maintenance ─────────────────────────────────────────────
    //
    // Edits patch the offset table in place instead of rescanning the whole
    // document. Cost is O(L) in the number of lines (to shift the offsets after
    // the edit) plus O(edit length) for the newline scan — never an O(N) memcpy
    // + rescan of the entire buffer on each keystroke.

    /// Update the index for an insertion of `inserted` at byte position `at`.
    ///
    /// Line starts strictly after `at` shift right by `inserted.len()`; the
    /// newlines contained in `inserted` splice in as new line starts. A line
    /// start exactly at `at` stays put — the inserted bytes land before it.
    pub(crate) fn record_insert(&mut self, at: usize, inserted: &[u8]) {
        let ins_len = inserted.len();
        if ins_len == 0 {
            return;
        }

        // Everything at a byte position > `at` moves right by `ins_len`.
        let split = self.offsets.partition_point(|&o| (o as usize) <= at);
        for o in &mut self.offsets[split..] {
            *o = Self::narrow(*o as usize + ins_len);
        }

        // Each '\n' at position `p` within `inserted` begins a line at
        // `at + p + 1`. These all sort between the untouched prefix and the
        // shifted suffix, so they splice in at `split`.
        let new_starts: Vec<u32> = memchr_iter(b'\n', inserted)
            .map(|p| Self::narrow(at + p + 1))
            .collect();
        if !new_starts.is_empty() {
            self.offsets.splice(split..split, new_starts);
        }
    }

    /// Update the index for a deletion of the byte range `[start, end)`.
    ///
    /// Line starts whose newline fell inside the range disappear; the survivors
    /// after the range shift left by the deleted length. `offsets[0]` (value 0)
    /// is never inside `(start, end]`, so line 0 is always preserved.
    pub(crate) fn record_delete(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let del_len = end - start;

        // A line start with value in `(start, end]` corresponds to a '\n' that
        // lived inside the deleted bytes, so it is gone.
        self.offsets.retain(|&o| {
            let v = o as usize;
            v <= start || v > end
        });

        // Survivors located after the deleted range move left by `del_len`.
        for o in &mut self.offsets {
            let v = *o as usize;
            if v > end {
                *o = Self::narrow(v - del_len);
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(s: &str) -> LineIndex {
        LineIndex::build(s.as_bytes())
    }

    #[test]
    fn single_line_no_newline() {
        let li = idx("hello world");
        assert_eq!(li.count(), 1);
        assert_eq!(li.line_start(0), 0);
        assert_eq!(li.line_end(0, 11), 11);
        assert_eq!(li.byte_to_line(5), 0);
    }

    #[test]
    fn three_lines() {
        let li = idx("aaa\nbbb\nccc");
        //                  ^3  ^7
        assert_eq!(li.count(), 3);
        assert_eq!(li.line_start(0), 0);
        assert_eq!(li.line_start(1), 4);
        assert_eq!(li.line_start(2), 8);
    }

    #[test]
    fn byte_to_line_basic() {
        let li = idx("aaa\nbbb\nccc");
        assert_eq!(li.byte_to_line(0), 0);
        assert_eq!(li.byte_to_line(2), 0);
        assert_eq!(li.byte_to_line(3), 0); // the \n itself is on line 0
        assert_eq!(li.byte_to_line(4), 1);
        assert_eq!(li.byte_to_line(7), 1);
        assert_eq!(li.byte_to_line(8), 2);
        assert_eq!(li.byte_to_line(10), 2);
    }

    #[test]
    fn trailing_newline() {
        let li = idx("abc\n");
        // "abc\n" → line 0 starts at 0, line 1 starts at 4 (empty line)
        assert_eq!(li.count(), 2);
        assert_eq!(li.line_start(1), 4);
    }

    #[test]
    fn empty_lines() {
        let li = idx("\n\n\n");
        assert_eq!(li.count(), 4);
        assert_eq!(li.line_start(0), 0);
        assert_eq!(li.line_start(1), 1);
        assert_eq!(li.line_start(2), 2);
        assert_eq!(li.line_start(3), 3);
    }

    #[test]
    fn line_range_offsets() {
        let li = idx("a\nb\nc\nd\n");
        let range = li.line_range_offsets(1, 3);
        assert_eq!(range, &[2u32, 4u32]);
    }

    #[test]
    fn lines_for_byte_range() {
        let li = idx("aaa\nbbb\nccc\nddd");
        // bytes 4..12 covers "bbb\nccc\n"
        let (first, last) = li.lines_for_byte_range(4, 12);
        assert_eq!(first, 1);
        assert_eq!(last, 2);
    }

    #[test]
    fn large_simulated_build() {
        // Simulate a 1000-line buffer and verify consistency
        let content: String = (0..1000).map(|i| format!("line {:04}\n", i)).collect();
        let li = LineIndex::build(content.as_bytes());
        assert_eq!(li.count(), 1001); // 1000 lines + trailing empty line after last \n
        assert_eq!(li.line_start(0), 0);
        assert_eq!(li.line_start(1), 10); // "line 0000\n" = 10 bytes
        assert_eq!(li.byte_to_line(10), 1);
    }

    // ── Incremental maintenance: must match a full rebuild ──────────────────

    /// Apply `record_insert` and confirm the patched table is identical to a
    /// fresh build of the post-edit bytes.
    fn check_insert(initial: &str, at: usize, ins: &str) {
        let mut li = LineIndex::build(initial.as_bytes());
        li.record_insert(at, ins.as_bytes());

        let mut bytes = initial.as_bytes().to_vec();
        bytes.splice(at..at, ins.bytes());
        let expected = LineIndex::build(&bytes);

        assert_eq!(
            li.offsets, expected.offsets,
            "record_insert({at}, {ins:?}) over {initial:?} diverged from a full rebuild"
        );
    }

    /// Apply `record_delete` and confirm the patched table is identical to a
    /// fresh build of the post-edit bytes.
    fn check_delete(initial: &str, start: usize, end: usize) {
        let mut li = LineIndex::build(initial.as_bytes());
        li.record_delete(start, end);

        let mut bytes = initial.as_bytes().to_vec();
        bytes.drain(start..end);
        let expected = LineIndex::build(&bytes);

        assert_eq!(
            li.offsets, expected.offsets,
            "record_delete({start}..{end}) over {initial:?} diverged from a full rebuild"
        );
    }

    #[test]
    fn record_insert_matches_rebuild() {
        check_insert("world", 0, "hello ");   // before line 0, no newline
        check_insert("hello", 5, "\nworld");  // append newline + text
        check_insert("ab\ncd", 0, "X");       // insert at 0, shifts later starts
        check_insert("ab\ncd", 3, "X");       // exactly at a line start (stays put)
        check_insert("ab\ncd", 2, "X\nY");    // newline inside inserted text
        check_insert("a\nb\nc", 2, "XY");     // multi-line, mid, no newline
        check_insert("abcd", 2, "X\nY\nZ");   // several newlines inserted
        check_insert("", 0, "fresh\nstart");  // into an empty buffer
        check_insert("a\nb\nc\nd", 4, "\n");  // lone newline at a boundary
    }

    #[test]
    fn record_delete_matches_rebuild() {
        check_delete("a\nb\nc", 2, 4);        // drop "b\n" (one newline)
        check_delete("a\nb\nc\nd", 2, 4);     // drop one newline, shift the rest
        check_delete("abcd\nefgh", 2, 7);     // delete spanning a newline
        check_delete("a\nbcd\ne", 3, 4);      // delete within a line
        check_delete("ab\ncd", 2, 3);         // delete just the newline
        check_delete("hello world", 0, 11);   // delete everything
        check_delete("a\nb\nc\n", 5, 6);      // delete the trailing newline
        check_delete("a\nb\nc", 0, 2);        // delete first line incl. newline
    }

    #[test]
    fn record_edits_sequence_matches_rebuild() {
        // A sequence of incremental edits must match a rebuild of the final bytes.
        let mut li = LineIndex::build(b"alpha\nbeta\ngamma");
        let mut bytes = b"alpha\nbeta\ngamma".to_vec();

        li.record_insert(6, b"X\n"); // -> "alpha\nX\nbeta\ngamma"
        bytes.splice(6..6, *b"X\n");

        li.record_delete(0, 3); // drop "alp"
        bytes.drain(0..3);

        let n = bytes.len();
        li.record_insert(n, b"\n"); // trailing newline
        bytes.splice(n..n, *b"\n");

        let expected = LineIndex::build(&bytes);
        assert_eq!(li.offsets, expected.offsets);
        assert_eq!(li.count(), expected.count());
    }
}
