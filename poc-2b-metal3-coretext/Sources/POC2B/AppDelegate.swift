import AppKit
import MetalKit
import os.signpost

let signpostLog = OSLog(subsystem: "com.poc.2b-metal3-coretext", category: .pointsOfInterest)

@main
struct POC2BApp {
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate2B()
        app.delegate = delegate
        app.run()
    }
}

final class AppDelegate2B: NSObject, NSApplicationDelegate {
    var window: NSWindow!

    func applicationDidFinishLaunching(_ notification: Notification) {
        let args = parse2BArgs()

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1440, height: 900),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered, defer: false)
        window.title = "PoC 2B — Metal 3 + CoreText (Flagship)"
        window.center()

        let vc = PoC2BViewController(args: args)
        window.contentViewController = vc
        window.makeKeyAndOrderFront(nil)

        if args.benchmark {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                vc.startBenchmark()
            }
        }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

// ── ViewController 2B ────────────────────────────────────────────────────────

final class PoC2BViewController: NSViewController {
    private let args: POC2BArgs
    private var mtkView:  MTKView!
    private var renderer: MetalRenderer!
    private var buffer:   PieceTable!
    private var shaper:   ShapingEngine!
    private var atlas:    TextureAtlasSwift!

    // Benchmark state
    private var benchFrames = 0
    private var frameTimes: [UInt64] = []
    private var dropped = 0
    private let budgetNs: UInt64 = 8_333_333  // 8.33ms in ns

    init(args: POC2BArgs) {
        self.args = args
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        mtkView = MTKView(frame: NSRect(x: 0, y: 0, width: 1440, height: 900))
        view = mtkView
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        // ── Load file ──────────────────────────────────────────────────
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            do {
                self.buffer = try PieceTable(path: self.args.file)
                DispatchQueue.main.async { self.setupRenderer() }
            } catch {
                print("Error loading file: \(error)")
            }
        }
    }

    private func setupRenderer() {
        do {
            renderer = try MetalRenderer(mtkView: mtkView)
        } catch {
            print("Metal init failed: \(error)"); return
        }

        atlas  = TextureAtlasSwift(device: mtkView.device!)
        shaper = ShapingEngine()
        shaper.onFallbackReady = { [weak self] _ in self?.mtkView.setNeedsDisplay(self?.mtkView.bounds ?? .zero) }

        renderer.textBuffer    = buffer
        renderer.shapingEngine = shaper
        renderer.atlas         = atlas
        renderer.lineHeight    = args.lineHeight
        renderer.scrollY       = 0
        renderer.isHeadless    = args.benchmark

        renderer.onFrameRendered = { [weak self] stats in
            self?.recordFrame(stats)
        }

        mtkView.delegate = renderer
        print("PoC 2B ready: \(buffer.lineCount) lines")
    }

    // Scroll
    override func scrollWheel(with event: NSEvent) {
        renderer?.scrollY = max(0, (renderer?.scrollY ?? 0) - event.scrollingDeltaY)
    }

    // ── Benchmark ─────────────────────────────────────────────────────────

    func startBenchmark() {
        print("Starting 2B benchmark: \(args.scrollFrames) frames")
        fflush(stdout)
        frameTimes = []
        dropped = 0
        benchFrames = 0
        
        // Force manual draws for headless benchmark since MTKView might not tick
        DispatchQueue.global(qos: .userInteractive).async {
            while self.benchFrames < self.args.scrollFrames {
                DispatchQueue.main.sync {
                    self.mtkView.draw() // Force a draw
                }
            }
        }
    }

    private func recordFrame(_ stats: MetalRenderer.FrameStatsItem) {
        guard args.benchmark else { return }
        frameTimes.append(stats.cpuEncodeNs)
        if stats.cpuEncodeNs > budgetNs { dropped += 1 }
        benchFrames += 1

        if benchFrames % 100 == 0 {
            print("Rendered \(benchFrames)/\(args.scrollFrames) frames")
        }

        // Advance scroll
        renderer.scrollY += args.scrollPxPerFrame
        let maxScroll = CGFloat(buffer.lineCount) * args.lineHeight - (mtkView.bounds.height)
        if renderer.scrollY > maxScroll { renderer.scrollY = 0 }

        if benchFrames >= args.scrollFrames {
            finishBenchmark()
        }
    }

    private func finishBenchmark() {
        let sorted = frameTimes.sorted()
        let n = sorted.count
        let p = { (pct: Double) -> Double in
            Double(sorted[Int(Double(n) * pct)]) / 1_000_000.0
        }
        let result: [String: Any] = [
            "poc_id": "2b-metal3-coretext",
            "file": ["line_count": buffer.lineCount],
            "benchmark": [
                "total_frames": n, "dropped_frames": dropped,
                "drop_rate_pct": Double(dropped) / Double(n) * 100,
                "p50_ms": p(0.50), "p95_ms": p(0.95), "p99_ms": p(0.99),
                "budget_ms": 8.333,
            ]
        ]
        if let data = try? JSONSerialization.data(withJSONObject: result, options: .prettyPrinted),
           let json = String(data: data, encoding: .utf8) {
            print("\n=== PoC 2B Results ===\n\(json)")
            let dir = URL(fileURLWithPath: "results")
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try? json.write(to: dir.appendingPathComponent("2b-metal3-coretext_stats.json"), atomically: true, encoding: .utf8)
        }
        NSApplication.shared.terminate(nil)
    }
}

struct POC2BArgs {
    var file: String
    var benchmark: Bool = false
    var scrollPxPerFrame: CGFloat = 60
    var scrollFrames: Int = 3600
    var lineHeight: CGFloat = 20
}

func parse2BArgs() -> POC2BArgs {
    let argv = CommandLine.arguments.dropFirst()
    var a = POC2BArgs(file: "")
    var i = argv.startIndex
    while i < argv.endIndex {
        switch argv[i] {
        case "--file":        i = argv.index(after: i); a.file = argv[i]
        case "--benchmark":   a.benchmark = true
        case "--scroll-px":   i = argv.index(after: i); a.scrollPxPerFrame = CGFloat(Double(argv[i]) ?? 60)
        case "--frames":      i = argv.index(after: i); a.scrollFrames = Int(argv[i]) ?? 3600
        case "--line-height": i = argv.index(after: i); a.lineHeight = CGFloat(Double(argv[i]) ?? 20)
        default: break
        }
        i = argv.index(after: i)
    }
    if a.file.isEmpty {
        a.file = "../shared/test-data/test_100mb.txt"
    }
    return a
}
