import AppKit
import os.signpost

// ── HUDView ────────────────────────────────────────────────────────────────────
// A simple overlay view drawn with Core Graphics. Avoids AppKit layout overhead.

final class HUDView: NSView {
    var pocId: String     = "2A"
    var loadMs: Double    = 0
    var lineCount: Int    = 0
    var frameMs: Double   = 0
    var dropped: Int      = 0
    var p95Ms: Double     = 0
    var scrollPx: CGFloat = 0

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.75).cgColor
        layer?.borderWidth = 0.5
        layer?.borderColor = NSColor.white.withAlphaComponent(0.15).cgColor
    }
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let x: CGFloat = 10, lineH: CGFloat = 18
        var y = bounds.height - 28

        func label(_ text: String, value: String, color: NSColor = .white) {
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                .foregroundColor: NSColor(calibratedRed: 0.67, green: 0.67, blue: 0.67, alpha: 1)
            ]
            let vAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .bold),
                .foregroundColor: color
            ]
            let labelStr = NSAttributedString(string: text, attributes: attrs)
            let valStr   = NSAttributedString(string: value, attributes: vAttrs)
            labelStr.draw(at: NSPoint(x: x, y: y))
            let valX = bounds.width - valStr.size().width - 10
            valStr.draw(at: NSPoint(x: valX, y: y))
            y -= lineH
        }

        // Title
        let title = NSAttributedString(string: "PoC \(pocId)", attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .bold),
            .foregroundColor: NSColor.systemBlue,
        ])
        title.draw(at: NSPoint(x: x, y: y))
        y -= lineH + 4

        label("Load time",   value: String(format: "%.1f ms",  loadMs))
        label("Lines",       value: "\(lineCount)")
        label("Frame time",  value: String(format: "%.2f ms",  frameMs))
        label("Scroll",      value: "\(Int(scrollPx)) px")
        label("Dropped",     value: "\(dropped)", color: dropped > 0 ? .systemRed : .systemGreen)
        label("P95",         value: String(format: "%.2f ms",  p95Ms))
    }
}

// ── BenchmarkRunner ───────────────────────────────────────────────────────────

final class BenchmarkRunner {
    private let scrollView: NSScrollView
    private let args: CommandLineArgs
    private weak var hudView: HUDView?

    private var displayLink: CVDisplayLink?
    private var frame: Int = 0
    private var frameTimes: [Double] = []
    private var dropped: Int = 0
    private let budgetMs: Double = 1000.0 / 120.0  // 8.33 ms

    init(scrollView: NSScrollView, args: CommandLineArgs, hudView: HUDView?) {
        self.scrollView = scrollView
        self.args = args
        self.hudView = hudView
        self.frameTimes.reserveCapacity(args.scrollFrames)
    }

    // ── Start ──────────────────────────────────────────────────────────────

    func start() {
        print("Starting benchmark: \(args.scrollFrames) frames @ \(args.scrollPxPerFrame)px/frame")
        setupDisplayLink()
    }

    // ── CVDisplayLink callback ─────────────────────────────────────────────

    private func setupDisplayLink() {
        var link: CVDisplayLink?
        CVDisplayLinkCreateWithActiveCGDisplays(&link)
        guard let link = link else { return }

        let ptr = Unmanaged.passRetained(self).toOpaque()
        CVDisplayLinkSetOutputCallback(link, { _, _, _, _, _, ptr -> CVReturn in
            let runner = Unmanaged<BenchmarkRunner>.fromOpaque(ptr!).takeUnretainedValue()
            DispatchQueue.main.async { runner.tick() }
            return kCVReturnSuccess
        }, ptr)

        CVDisplayLinkStart(link)
        self.displayLink = link
    }

    // ── Per-frame tick ─────────────────────────────────────────────────────

    private func tick() {
        guard frame < args.scrollFrames else {
            finish()
            return
        }

        os_signpost(.begin, log: signpostLog, name: "BenchFrame")
        let t0 = mach_absolute_time()

        // Advance scroll position
        let currentOffset = scrollView.contentView.bounds.origin.y
        let newOffset = currentOffset + args.scrollPxPerFrame
        let maxScroll = (scrollView.documentView?.bounds.height ?? 0) - scrollView.contentView.bounds.height
        let clampedOffset = min(newOffset, max(0, maxScroll))

        // If we've reached the bottom, wrap around
        let finalOffset = clampedOffset < currentOffset + 1 ? 0 : clampedOffset
        scrollView.contentView.scroll(to: NSPoint(x: 0, y: finalOffset))
        scrollView.reflectScrolledClipView(scrollView.contentView)

        let elapsed = machTimeToMs(mach_absolute_time() - t0)
        os_signpost(.end, log: signpostLog, name: "BenchFrame")

        frameTimes.append(elapsed)
        if elapsed > budgetMs { dropped += 1 }

        // HUD update every 60 frames
        if frame % 60 == 0 {
            let sorted = frameTimes.sorted()
            let p95 = sorted.isEmpty ? 0 : sorted[Int(Double(sorted.count) * 0.95)]
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.hudView?.frameMs   = elapsed
                self.hudView?.dropped   = self.dropped
                self.hudView?.p95Ms     = p95
                self.hudView?.scrollPx  = finalOffset
                self.hudView?.needsDisplay = true
            }
        }

        frame += 1
    }

    // ── Results ────────────────────────────────────────────────────────────

    private func finish() {
        if let link = displayLink {
            CVDisplayLinkStop(link)
        }

        let sorted = frameTimes.sorted()
        let n = sorted.count
        let p = { (pct: Double) -> Double in
            guard n > 0 else { return 0 }
            return sorted[Int(Double(n) * pct)]
        }
        let avg = frameTimes.reduce(0, +) / Double(n)

        let result: [String: Any] = [
            "poc_id": "2a-textkit2",
            "file": ["line_count": hudView?.lineCount ?? 0, "load_ms": hudView?.loadMs ?? 0],
            "benchmark": [
                "total_frames":    n,
                "dropped_frames":  dropped,
                "drop_rate_pct":   Double(dropped) / Double(n) * 100,
                "avg_frame_ms":    avg,
                "p50_ms":          p(0.50),
                "p95_ms":          p(0.95),
                "p99_ms":          p(0.99),
                "budget_ms":       budgetMs,
            ]
        ]

        print("\n=== PoC 2A Benchmark Results ===")
        if let data = try? JSONSerialization.data(withJSONObject: result, options: .prettyPrinted),
           let json = String(data: data, encoding: .utf8) {
            print(json)
            writeResults(json: json)
        }
    }

    private func writeResults(json: String) {
        // Write to results/ relative to the executable
        let resultsDir = URL(fileURLWithPath: "results")
        try? FileManager.default.createDirectory(at: resultsDir, withIntermediateDirectories: true)
        let outFile = resultsDir.appendingPathComponent("2a-textkit2_stats.json")
        try? json.write(to: outFile, atomically: true, encoding: .utf8)
        print("Results → \(outFile.path)")
        NSApplication.shared.terminate(nil)
    }
}

private func machTimeToMs(_ machTime: UInt64) -> Double {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    let ns = machTime * UInt64(info.numer) / UInt64(info.denom)
    return Double(ns) / 1_000_000.0
}
