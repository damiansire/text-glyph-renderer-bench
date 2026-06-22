import Foundation
import CoreText
import AppKit
import os.signpost

// ── GlyphRun ──────────────────────────────────────────────────────────────────

struct GlyphRun {
    var glyphIDs:  [CGGlyph]
    var advances:  [CGSize]
    var positions: [CGPoint]   // baseline positions relative to line origin
    var font:      CTFont
    var totalWidth: CGFloat
}

// ── AtlasSlot ─────────────────────────────────────────────────────────────────

struct AtlasSlot {
    var atlasX: Int
    var atlasY: Int
    var width:  Int
    var height: Int
    var offsetX: CGFloat
    var offsetY: CGFloat
    var advance: CGFloat
}

// ── ShapingEngine ─────────────────────────────────────────────────────────────

final class ShapingEngine {
    private let primaryFont: CTFont
    private var shapingCache: [Int: [GlyphRun]] = [:]  // line → shaped runs
    private let maxCacheLines = 2000

    /// Utility QoS queue for async font fallback.
    /// GCD `.utility` priority: below User Interactive, above Background.
    /// This matches the architecture doc's GCD Async Fallback design.
    private let fallbackQueue = DispatchQueue(
        label: "com.poc.2b.font-fallback",
        qos: .utility,
        attributes: .concurrent
    )

    /// Callback invoked on main thread when a fallback glyph is ready.
    var onFallbackReady: ((Int) -> Void)?  // lineIdx

    /// AUDIT FIX (P3): lines with a fallback task currently in flight.
    /// Prevents re-enqueueing a fallback while one is already live for the line.
    private var fallbackInFlight: Set<Int> = []

    /// AUDIT FIX (P3): lines whose .notdef could not be resolved by fallback.
    /// These reach a stable cached state (notdef included) and are never
    /// re-enqueued, so steady-state measurement is not flooded with re-shaping.
    private var fallbackFailed: Set<Int> = []

    init(fontName: String = "JetBrains Mono", size: CGFloat = 13) {
        let descriptor = CTFontDescriptorCreateWithNameAndSize(fontName as CFString, size)
        primaryFont = CTFontCreateWithFontDescriptor(descriptor, size, nil)
    }

    // ── Lazy shaping ─────────────────────────────────────────────────────────

    /// Shape lines in [firstLine, lastLine+margin] range.
    /// Returns immediately; missing glyph fallbacks are resolved asynchronously.
    func shapeViewport(buffer: TextBuffer, firstLine: Int, lastLine: Int, margin: Int = 50) {
        let start = max(0, firstLine - margin)
        let end   = min(buffer.lineCount - 1, lastLine + margin)

        for lineIdx in start...end {
            guard shapingCache[lineIdx] == nil else { continue }

            os_signpost(.begin, log: signpostLog, name: "ShapeLine")
            let runs = shapeLine(buffer: buffer, lineIdx: lineIdx)
            shapingCache[lineIdx] = runs
            os_signpost(.end, log: signpostLog, name: "ShapeLine")

            // Check for fallback-needed glyphs
            checkFallback(runs: runs, lineIdx: lineIdx)
        }

        // Evict oldest entries if cache is too large
        if shapingCache.count > maxCacheLines {
            let keysToRemove = shapingCache.keys.sorted()
                .prefix(shapingCache.count - maxCacheLines)
            keysToRemove.forEach { shapingCache.removeValue(forKey: $0) }
        }
    }

    func runs(forLine line: Int) -> [GlyphRun]? { shapingCache[line] }

    // ── Internal shaping ──────────────────────────────────────────────────────

    private func shapeLine(buffer: TextBuffer, lineIdx: Int) -> [GlyphRun] {
        let lineStart = buffer.lineStartByte(lineIdx)
        let lineEnd: Int
        if lineIdx + 1 < buffer.lineCount {
            lineEnd = buffer.lineStartByte(lineIdx + 1) - 1  // exclude trailing \n
        } else {
            lineEnd = buffer.byteLen
        }
        guard lineStart < lineEnd else { return [] }

        let lineData = buffer.bytesInRange(lineStart..<lineEnd)
        guard let lineStr = String(data: lineData, encoding: .utf8) else { return [] }

        // Use CoreText for shaping (handles Unicode, bidi, ligatures)
        let attrStr = CFAttributedStringCreate(nil, lineStr as CFString, [
            kCTFontAttributeName: primaryFont,
        ] as CFDictionary)!

        let line = CTLineCreateWithAttributedString(attrStr)
        let runs = CTLineGetGlyphRuns(line) as! [CTRun]

        return runs.map { ctRun in
            let count = CTRunGetGlyphCount(ctRun)
            var glyphs    = [CGGlyph](repeating: 0, count: count)
            var advances  = [CGSize](repeating: .zero, count: count)
            var positions = [CGPoint](repeating: .zero, count: count)

            CTRunGetGlyphs(ctRun, CFRangeMake(0, count), &glyphs)
            CTRunGetAdvances(ctRun, CFRangeMake(0, count), &advances)
            CTRunGetPositions(ctRun, CFRangeMake(0, count), &positions)

            let attrs  = CTRunGetAttributes(ctRun) as! [NSAttributedString.Key: Any]
            let font   = attrs[.font] as! CTFont
            let totalW = advances.reduce(0) { $0 + $1.width }

            return GlyphRun(glyphIDs: glyphs, advances: advances,
                            positions: positions, font: font, totalWidth: totalW)
        }
    }

    // ── Font fallback (async) ─────────────────────────────────────────────────

    private func checkFallback(runs: [GlyphRun], lineIdx: Int) {
        // AUDIT FIX (P3): never re-enqueue a fallback for a line that already
        // has a task in flight or has permanently failed to resolve. Without
        // this guard, any unresolved .notdef makes the line re-shape every
        // frame (cache nil -> full re-shape -> same notdef -> re-enqueue),
        // flooding .utility tasks and ruining steady-state measurement.
        guard !fallbackInFlight.contains(lineIdx),
              !fallbackFailed.contains(lineIdx) else { return }

        for run in runs {
            for glyph in run.glyphIDs where glyph == 0 {
                // notdef glyph: schedule fallback lookup on utility queue.
                fallbackInFlight.insert(lineIdx)
                fallbackQueue.async { [weak self] in
                    guard let self = self else { return }
                    // In a real impl: CTFontCreateForString to find the fallback
                    // font, then re-shape and detect whether the fallback font
                    // actually produced glyphs != 0.
                    // For the PoC we simulate the async work; the simulation
                    // cannot resolve the notdef, so `resolved` is always false.
                    Thread.sleep(forTimeInterval: 0.001) // simulate lookup.
                    let resolved = false
                    DispatchQueue.main.async {
                        self.fallbackInFlight.remove(lineIdx)
                        if resolved {
                            // Only invalidate when a DIFFERENT font yielded
                            // glyphs != 0; otherwise we'd loop forever.
                            self.shapingCache.removeValue(forKey: lineIdx)
                            self.onFallbackReady?(lineIdx)
                        } else {
                            // Unresolvable notdef: keep the cached shaping (with
                            // the notdef) and never reschedule for this line.
                            self.fallbackFailed.insert(lineIdx)
                        }
                    }
                }
                return  // only one fallback request per line.
            }
        }
    }
}
