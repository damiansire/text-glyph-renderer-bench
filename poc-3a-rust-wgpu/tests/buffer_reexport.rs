//! Integration test over the re-exported `text-buffer` surface.
//!
//! The piece-table unit tests live in the shared `text-buffer` crate now. This
//! test keeps the named gate (`cargo test -p poc-3a-rust-wgpu`) meaningful by
//! exercising the buffer contract through this crate's public API.

use poc_3a_rust_wgpu::{PieceTable, TextBuffer};

#[test]
fn reexport_reads() {
    let pt = PieceTable::from_bytes(b"line0\nline1\nline2".to_vec());
    assert_eq!(pt.byte_len(), 17);
    assert_eq!(pt.line_count(), 3);
    assert_eq!(pt.line_start_byte(1), 6);
    assert_eq!(pt.bytes_in_range(6..11), b"line1");
}

#[test]
fn reexport_insert_updates_line_count() {
    let mut pt = PieceTable::from_bytes(b"hello".to_vec());
    assert_eq!(pt.line_count(), 1);
    pt.insert(5, "\nworld");
    assert_eq!(pt.line_count(), 2);
    let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
    assert_eq!(content, "hello\nworld");
    assert_eq!(pt.line_start_byte(1), 6);
}

#[test]
fn reexport_delete_keeps_content_consistent() {
    let mut pt = PieceTable::from_bytes(b"alpha\nbeta\ngamma".to_vec());
    pt.delete(6..11); // drop "beta\n"
    let content = String::from_utf8(pt.bytes_in_range(0..pt.byte_len())).unwrap();
    assert_eq!(content, "alpha\ngamma");
    assert_eq!(pt.line_count(), 2);
}
