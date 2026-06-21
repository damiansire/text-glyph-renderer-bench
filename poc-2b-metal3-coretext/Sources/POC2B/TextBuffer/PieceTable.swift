import Foundation
import AppKit

// ── TextBuffer protocol (mirrors the Rust trait) ────────────────────────────

protocol TextBuffer: AnyObject {
    var byteLen: Int { get }
    var lineCount: Int { get }
    func lineStartByte(_ line: Int) -> Int
    func byteToLine(_ offset: Int) -> Int
    func bytesInRange(_ range: Range<Int>) -> Data
}

// ── PieceTable ────────────────────────────────────────────────────────────────

final class PieceTable: TextBuffer {
    // Memory-mapped original file (read-only)
    private let originalMmap: UnsafeRawPointer
    private let originalLen:  Int
    private let fileHandle:   FileHandle
    private var addBuffer:    Data = Data()

    private struct Piece {
        enum BufferKind { case original, add }
        var kind:  BufferKind
        var start: Int
        var len:   Int
        var end:   Int { start + len }
    }
    private var pieces: [Piece] = []

    /// Byte offset of the start of each line (built by a scalar byte scan for `\n`).
    private var lineOffsets: [Int] = []

    // ── Init ──────────────────────────────────────────────────────────────

    init(path: String) throws {
        let url = URL(fileURLWithPath: path)
        fileHandle = try FileHandle(forReadingFrom: url)

        let size = try FileManager.default.attributesOfItem(atPath: path)[.size] as! Int

        // mmap the file read-only 
        let ptr = mmap(nil, size, PROT_READ, MAP_PRIVATE, fileHandle.fileDescriptor, 0)
        guard ptr != MAP_FAILED else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }

        // Advise sequential access for the initial line-index scan
        madvise(ptr, size, MADV_SEQUENTIAL)

        originalMmap = UnsafeRawPointer(ptr!)
        originalLen  = size
        pieces       = [Piece(kind: .original, start: 0, len: size)]

        lineOffsets = buildLineIndex()
        print("PieceTable: \(size) bytes, \(lineOffsets.count) lines")
    }

    deinit {
        munmap(UnsafeMutableRawPointer(mutating: originalMmap), originalLen)
        try? fileHandle.close()
    }

    // ── TextBuffer ─────────────────────────────────────────────────────────

    var byteLen: Int { pieces.reduce(0) { $0 + $1.len } }
    var lineCount: Int { lineOffsets.count }

    func lineStartByte(_ line: Int) -> Int { lineOffsets[line] }

    func byteToLine(_ offset: Int) -> Int {
        // Binary search on lineOffsets
        var lo = 0, hi = lineOffsets.count - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if lineOffsets[mid] <= offset { lo = mid } else { hi = mid - 1 }
        }
        return lo
    }

    func bytesInRange(_ range: Range<Int>) -> Data {
        var result = Data(capacity: range.count)
        slicePieces(range) { slice, _ in result.append(contentsOf: slice); return true }
        return result
    }

    // ── Zero-copy slice iteration ───────────────────────────────────────────

    /// Iterate over raw byte slices for the given range without copying.
    func slicePieces(_ range: Range<Int>, _ body: (UnsafeRawBufferPointer, Int) -> Bool) {
        var globalOffset = 0
        for piece in pieces {
            let pieceEnd = globalOffset + piece.len
            if pieceEnd <= range.lowerBound { globalOffset = pieceEnd; continue }
            if globalOffset >= range.upperBound { break }

            let localStart = max(0, range.lowerBound - globalOffset)
            let localEnd   = min(piece.len, range.upperBound - globalOffset)
            let count      = localEnd - localStart
            let callerOffset = globalOffset + localStart

            let shouldContinue: Bool
            switch piece.kind {
            case .original:
                // The mmap is a stable, long-lived mapping: its base pointer
                // does not move, so deriving a sub-pointer here is safe.
                let ptr = originalMmap.advanced(by: piece.start + localStart)
                let buf = UnsafeRawBufferPointer(start: ptr, count: count)
                shouldContinue = body(buf, callerOffset)
            case .add:
                // `Data.withUnsafeBytes` only guarantees the pointer is valid
                // *inside* the closure — `Data` may relocate its storage, so the
                // base address must never escape. Invoke `body` within the
                // closure, rebasing to this piece's sub-range.
                let pieceStart = piece.start + localStart
                shouldContinue = addBuffer.withUnsafeBytes { raw -> Bool in
                    let sub = UnsafeRawBufferPointer(rebasing: raw[pieceStart ..< pieceStart + count])
                    return body(sub, callerOffset)
                }
            }
            if !shouldContinue { break }
            globalOffset = pieceEnd
        }
    }

    // ── Mutations ───────────────────────────────────────────────────────────

    func insert(at byte: Int, text: String) {
        guard let data = text.data(using: .utf8) else { return }
        let addStart = addBuffer.count
        addBuffer.append(data)
        let (pieceIdx, localOffset) = findPiece(byte)
        let newPiece = Piece(kind: .add, start: addStart, len: data.count)
        if localOffset == 0 {
            pieces.insert(newPiece, at: pieceIdx)
        } else if localOffset == pieces[pieceIdx].len {
            pieces.insert(newPiece, at: pieceIdx + 1)
        } else {
            let orig = pieces[pieceIdx]
            let left  = Piece(kind: orig.kind, start: orig.start, len: localOffset)
            let right = Piece(kind: orig.kind, start: orig.start + localOffset, len: orig.len - localOffset)
            pieces.replaceSubrange(pieceIdx...pieceIdx, with: [left, newPiece, right])
        }
        lineOffsets = buildLineIndex()  // rebuild after mutation
    }

    // ── Line index ──────────────────────────────────────────────────────────

    private func buildLineIndex() -> [Int] {
        // For single-piece tables (pure mmap, no mutations) we can scan
        // the mmap directly using UInt8 comparison — faster than copying to Data.
        var offsets = [0]  // line 0 starts at byte 0
        if pieces.count == 1 && pieces[0].kind == .original {
            // Direct scalar scan over the mmap for `\n` (0x0A), byte by byte.
            // NOTE: not vectorized — no vDSP/SIMD/memchr here; see lineOffsets.
            let buf = UnsafeRawBufferPointer(start: originalMmap, count: originalLen)
            for (i, byte) in buf.enumerated() where byte == 0x0A {
                offsets.append(i + 1)
            }
        } else {
            // Multi-piece: materialise and scan
            let data = bytesInRange(0..<byteLen)
            data.withUnsafeBytes { ptr in
                for i in 0..<data.count where ptr.load(fromByteOffset: i, as: UInt8.self) == 0x0A {
                    offsets.append(i + 1)
                }
            }
        }
        return offsets
    }

    private func findPiece(_ byte: Int) -> (Int, Int) {
        var remaining = byte
        for (i, p) in pieces.enumerated() {
            if remaining <= p.len { return (i, remaining) }
            remaining -= p.len
        }
        return (pieces.count - 1, pieces.last?.len ?? 0)
    }
}
