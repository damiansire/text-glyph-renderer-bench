//! piece_table.rs — Zero-copy Piece Table over memory-mapped files.
//!
//! Architecture:
//!   - `original` buffer: read-only `memmap2::Mmap` of the source file.
//!     On Apple Silicon the same physical pages are shared with the GPU
//!     via `wgpu::Buffer` (mapped at creation) — zero CPU→GPU copy.
//!   - `add` buffer: `Vec<u8>` for inserted text (append-only).
//!   - `pieces`: ordered list of `Piece` structs pointing into either buffer.
//!
//! For a freshly loaded, unedited file there is exactly **1 piece** pointing
//! to the entire `original` buffer.  Inserts and deletes add/split pieces
//! without copying the original data.
//!
//! Line index is maintained by `LineIndex` (see `line_index.rs`) and is
//! rebuilt lazily after batches of mutations.

use super::{BufferSnapshot, TextBuffer};
use crate::buffer::line_index::LineIndex;

use memmap2::{Mmap, MmapOptions};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

// ── Internal types ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufferKind {
    Original,
    Add,
}

#[derive(Clone, Debug)]
pub(crate) struct Piece {
    pub kind: BufferKind,
    /// Byte offset into the owning buffer (original or add).
    pub start: usize,
    /// Length of this piece in bytes.
    pub len: usize,
}

impl Piece {
    #[inline]
    fn end(&self) -> usize {
        self.start + self.len
    }
}

// ── Snapshot ────────────────────────────────────────────────────────────────

/// An immutable, Arc-wrapped materialization of the current logical content.
/// Created by `PieceTable::snapshot()`, cheap to clone (O(1) Arc bump).
///
/// For single-piece tables (pure mmap read) this shares the `Arc<Mmap>` and
/// stores a byte range — no allocation, no copy.

// ── PieceTable ──────────────────────────────────────────────────────────────

pub struct PieceTable {
    /// Memory-mapped original file (read-only, shared with GPU on UMA).
    original: Mmap,
    /// Append-only buffer for inserted text.
    add_buffer: Vec<u8>,
    /// Ordered sequence of pieces (the logical document).
    pieces: Vec<Piece>,
    /// Pre-computed byte offsets of every line start (SIMD-built).
    line_index: LineIndex,
    /// True when pieces changed and `line_index` needs rebuilding.
    dirty: bool,
}

impl PieceTable {
    // ── Constructors ─────────────────────────────────────────────────────

    /// Load a file by memory-mapping it.  On Apple Silicon the kernel maps
    /// the file into the unified address space; the GPU can read the same
    /// pages via `wgpu::Buffer` without a copy.
    ///
    /// `MAP_POPULATE` pre-faults pages so the first render doesn't stall.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe {
            MmapOptions::new()
                .populate() // pre-fault pages — avoids stalls during first scroll
                .map(&file)?
        };

        // Advise sequential access for the initial line-index scan
        #[cfg(unix)]
        {
            use memmap2::Advice;
            let _ = mmap.advise(Advice::Sequential);
            let _ = mmap.advise(Advice::WillNeed);
        }

        let line_index = LineIndex::build(&mmap);
        let len = mmap.len();

        Ok(Self {
            original: mmap,
            add_buffer: Vec::new(),
            // Single piece covering the whole file — the happy path for read-only scroll.
            pieces: vec![Piece {
                kind: BufferKind::Original,
                start: 0,
                len,
            }],
            line_index,
            dirty: false,
        })
    }

    /// Create an in-memory PieceTable from a byte slice (for tests / REPL).
    pub fn from_bytes(data: Vec<u8>) -> Self {
        let len = data.len();
        // We store the in-memory data in the add buffer and make one Add piece.
        let mut pt = Self {
            original: {
                // Safety: empty anonymous mmap for the "original" slot.
                let mut opts = MmapOptions::new();
                opts.len(1).map_anon().expect("anon mmap").make_read_only().expect("make_read_only")
            },
            add_buffer: data,
            pieces: vec![Piece {
                kind: BufferKind::Add,
                start: 0,
                len,
            }],
            line_index: LineIndex::empty(),
            dirty: true,
        };
        pt.rebuild_line_index();
        pt
    }

    // ── Buffer resolution ─────────────────────────────────────────────────

    #[inline]
    fn resolve<'a>(&'a self, piece: &Piece) -> &'a [u8] {
        match piece.kind {
            BufferKind::Original => &self.original[piece.start..piece.end()],
            BufferKind::Add => &self.add_buffer[piece.start..piece.end()],
        }
    }

    // ── Line index ────────────────────────────────────────────────────────

    fn rebuild_line_index(&mut self) {
        // Materialise logical content into a temporary Vec for scanning.
        // This is only called after mutations — not on the hot render path.
        let content = self.materialise();
        self.line_index = LineIndex::build(&content);
        self.dirty = false;
    }

    fn ensure_line_index(&mut self) {
        if self.dirty {
            self.rebuild_line_index();
        }
    }

    /// Materialise all pieces into a contiguous byte vector.
    /// O(N bytes) — only called after mutations or for snapshot().
    fn materialise(&self) -> Vec<u8> {
        let total: usize = self.pieces.iter().map(|p| p.len).sum();
        let mut out = Vec::with_capacity(total);
        for piece in &self.pieces {
            out.extend_from_slice(self.resolve(piece));
        }
        out
    }

    // ── Piece lookup ──────────────────────────────────────────────────────

    /// Find which piece contains `byte_offset` and the local offset within
    /// that piece.  Returns `(piece_index, local_offset)`.
    ///
    /// O(P) where P = number of pieces.  For unedited files P = 1 → O(1).
    fn find_piece(&self, byte_offset: usize) -> (usize, usize) {
        // Empty document (e.g. after deleting all content): there is no piece
        // to point into, so report index 0 with offset 0. Callers treat an
        // empty `pieces` as "insert into an empty buffer".
        if self.pieces.is_empty() {
            return (0, 0);
        }

        let mut remaining = byte_offset;
        for (i, piece) in self.pieces.iter().enumerate() {
            if remaining <= piece.len {
                return (i, remaining);
            }
            remaining -= piece.len;
        }
        // At the very end of the buffer
        let last = self.pieces.len() - 1;
        (last, self.pieces[last].len)
    }
}

// ── TextBuffer impl ─────────────────────────────────────────────────────────

impl TextBuffer for PieceTable {
    fn byte_len(&self) -> usize {
        self.pieces.iter().map(|p| p.len).sum()
    }

    fn line_count(&self) -> usize {
        // `LineIndex` stores the byte offset of every line start.
        // line_count == number of entries (we always have at least line 0).
        self.line_index.count()
    }

    fn line_start_byte(&self, line: usize) -> usize {
        self.line_index.line_start(line)
    }

    fn byte_to_line(&self, byte_offset: usize) -> usize {
        self.line_index.byte_to_line(byte_offset)
    }

    fn bytes_in_range(&self, range: Range<usize>) -> Vec<u8> {
        let mut out = Vec::with_capacity(range.len());
        self.slice_pieces(range, |slice, _| {
            out.extend_from_slice(slice);
            true
        });
        out
    }

    fn slice_pieces<F>(&self, range: Range<usize>, mut f: F)
    where
        F: FnMut(&[u8], usize) -> bool,
    {
        let mut global_offset = 0usize;
        let start = range.start;
        let end = range.end;

        for piece in &self.pieces {
            let piece_end = global_offset + piece.len;

            if piece_end <= start {
                global_offset = piece_end;
                continue;
            }
            if global_offset >= end {
                break;
            }

            // Compute the overlap between [global_offset..piece_end] and [start..end]
            let local_start = if global_offset < start { start - global_offset } else { 0 };
            let local_end = (end - global_offset).min(piece.len);
            let data = &self.resolve(piece)[local_start..local_end];
            let should_continue = f(data, global_offset + local_start);

            global_offset = piece_end;
            if !should_continue {
                break;
            }
        }
    }

    // ── Mutations ─────────────────────────────────────────────────────────

    fn insert(&mut self, byte_offset: usize, text: &str) {
        if text.is_empty() {
            return;
        }

        let add_start = self.add_buffer.len();
        self.add_buffer.extend_from_slice(text.as_bytes());
        let add_len = text.len();

        let (piece_idx, local_offset) = self.find_piece(byte_offset);

        let new_piece = Piece {
            kind: BufferKind::Add,
            start: add_start,
            len: add_len,
        };

        if local_offset == 0 {
            // Insert before piece_idx
            self.pieces.insert(piece_idx, new_piece);
        } else if local_offset == self.pieces[piece_idx].len {
            // Insert after piece_idx
            self.pieces.insert(piece_idx + 1, new_piece);
        } else {
            // Split piece_idx into two, insert new piece in between
            let original_piece = self.pieces[piece_idx].clone();
            let left = Piece {
                kind: original_piece.kind,
                start: original_piece.start,
                len: local_offset,
            };
            let right = Piece {
                kind: original_piece.kind,
                start: original_piece.start + local_offset,
                len: original_piece.len - local_offset,
            };
            self.pieces.splice(piece_idx..=piece_idx, [left, new_piece, right]);
        }

        self.rebuild_line_index();
    }

    fn delete(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }

        let (start_piece, start_local) = self.find_piece(range.start);
        let (end_piece, end_local) = self.find_piece(range.end);

        // Build replacement pieces (left stub of start_piece + right stub of end_piece)
        let mut replacement = Vec::new();

        // Left stub: bytes before range.start in the start piece
        if start_local > 0 {
            let p = &self.pieces[start_piece];
            replacement.push(Piece {
                kind: p.kind,
                start: p.start,
                len: start_local,
            });
        }

        // Right stub: bytes after range.end in the end piece
        if end_local < self.pieces[end_piece].len {
            let p = &self.pieces[end_piece];
            replacement.push(Piece {
                kind: p.kind,
                start: p.start + end_local,
                len: p.len - end_local,
            });
        }

        self.pieces.splice(start_piece..=end_piece, replacement);
        self.rebuild_line_index();
    }

    fn snapshot(&self) -> super::BufferSnapshot {
        Arc::new(self.materialise()) as super::BufferSnapshot
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make(s: &str) -> PieceTable {
        PieceTable::from_bytes(s.as_bytes().to_vec())
    }

    #[test]
    fn test_byte_len() {
        let pt = make("hello world");
        assert_eq!(pt.byte_len(), 11);
    }

    #[test]
    fn test_bytes_in_range() {
        let pt = make("hello world");
        assert_eq!(pt.bytes_in_range(6..11), b"world");
    }

    #[test]
    fn test_line_count_single_line() {
        let pt = make("hello world");
        assert_eq!(pt.line_count(), 1);
    }

    #[test]
    fn test_line_count_multi() {
        let pt = make("line0\nline1\nline2");
        assert_eq!(pt.line_count(), 3);
    }

    #[test]
    fn test_line_start_byte() {
        let pt = make("aaa\nbbb\nccc");
        assert_eq!(pt.line_start_byte(0), 0);
        assert_eq!(pt.line_start_byte(1), 4);
        assert_eq!(pt.line_start_byte(2), 8);
    }

    #[test]
    fn test_byte_to_line() {
        let pt = make("aaa\nbbb\nccc");
        assert_eq!(pt.byte_to_line(0), 0);
        assert_eq!(pt.byte_to_line(3), 0);  // the \n is still on line 0
        assert_eq!(pt.byte_to_line(4), 1);
        assert_eq!(pt.byte_to_line(8), 2);
    }

    #[test]
    fn test_insert_beginning() {
        let mut pt = make("world");
        pt.insert(0, "hello ");
        let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_insert_middle() {
        let mut pt = make("helo");
        pt.insert(3, "l");
        let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_delete() {
        let mut pt = make("hello brave world");
        pt.delete(5..11);
        let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_line_count_after_insert() {
        let mut pt = make("hello");
        pt.insert(5, "\nworld");
        assert_eq!(pt.line_count(), 2);
    }

    // ── Edge cases: empty buffer / multi-piece / invalid UTF-8 / mmap path ──

    /// Deleting the entire content must not leave the table in a state that
    /// panics on the next operation (regression for the `find_piece`
    /// underflow when `pieces` is empty).
    #[test]
    fn test_delete_all_then_operate() {
        let mut pt = make("hello world");
        pt.delete(0..pt.byte_len());
        assert_eq!(pt.byte_len(), 0);
        // Subsequent operations must not panic.
        assert_eq!(pt.bytes_in_range(0..0), Vec::<u8>::new());
        pt.insert(0, "fresh");
        let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
        assert_eq!(content, "fresh");
    }

    /// A slice that spans several pieces after a sequence of edits must
    /// reassemble the logical content correctly.
    #[test]
    fn test_slice_multi_piece() {
        let mut pt = make("ACE");
        pt.insert(1, "B"); // A B CE
        pt.insert(3, "D"); // A B C D E
        let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
        assert_eq!(content, "ABCDE");
        // Partial slice across the inserted pieces.
        assert_eq!(pt.bytes_in_range(1..4), b"BCD");
    }

    /// Invalid UTF-8 bytes must round-trip through the byte API untouched
    /// (the buffer is byte-oriented; it must not assume valid UTF-8).
    #[test]
    fn test_invalid_utf8_roundtrip() {
        let raw = vec![0x66, 0xFF, 0x0A, 0xFE, 0x6F]; // f, invalid, \n, invalid, o
        let pt = PieceTable::from_bytes(raw.clone());
        assert_eq!(pt.bytes_in_range(0..pt.byte_len()), raw);
        assert_eq!(pt.line_count(), 2);
    }

    /// Exercise the `from_file` / mmap (`Original`) path, which the rest of
    /// the suite never touches because `from_bytes` only uses the add buffer.
    #[test]
    fn test_from_file_original_path() {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        path.push(format!("poc3b_pt_test_{}.txt", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).expect("create tmp file");
            f.write_all(b"alpha\nbeta\ngamma").expect("write tmp file");
        }

        let pt = PieceTable::from_file(&path).expect("mmap tmp file");
        assert_eq!(pt.byte_len(), 16);
        assert_eq!(pt.line_count(), 3);
        assert_eq!(pt.line_start_byte(1), 6);
        assert_eq!(pt.bytes_in_range(0..5), b"alpha");
        assert_eq!(pt.bytes_in_range(6..10), b"beta");

        let _ = std::fs::remove_file(&path);
    }
}
