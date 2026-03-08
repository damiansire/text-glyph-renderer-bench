import AppKit

import os.signpost

// ── Signpost log for Instruments.app profiling ─────────────────────────────────
let signpostLog = OSLog(subsystem: "com.poc.2a-textkit2", category: .pointsOfInterest)

// ── ViewController ──────────────────────────────────────────────────────────────

final class ViewController: NSViewController {
    private let args: CommandLineArgs

    // TextKit 2 stack
    private var textStorage:       NSTextStorage!
    private var textLayoutManager: NSTextLayoutManager!
    private var textContainer:     NSTextContentStorage!
    private var scrollView:        NSScrollView!
    private var textView:          NSTextView!

    // Metrics
    private var benchmark: BenchmarkRunner?

    // Loading indicator
    private var progressLabel: NSTextField?

    init(args: CommandLineArgs) {
        self.args = args
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    // ── View life cycle ───────────────────────────────────────────────────────

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 1440, height: 900))
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor(calibratedRed: 0.102, green: 0.106, blue: 0.118, alpha: 1).cgColor
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        // ── Loading label ──────────────────────────────────────────────────
        let label = NSTextField(labelWithString: "Cargando archivo…")
        label.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        label.textColor = .systemBlue
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        progressLabel = label

        // ── Load file async ────────────────────────────────────────────────
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.loadFile()
        }
    }

    // ── File loading ──────────────────────────────────────────────────────────

    private func loadFile() {
        let t0 = mach_absolute_time()
        os_signpost(.begin, log: signpostLog, name: "LoadFile")

        guard let content = try? String(contentsOfFile: args.file, encoding: .utf8) else {
            DispatchQueue.main.async { [weak self] in
                self?.progressLabel?.stringValue = "Error: no se pudo cargar \(self?.args.file ?? "")"
            }
            return
        }

        os_signpost(.end, log: signpostLog, name: "LoadFile")
        let elapsed = machTimeToMs(mach_absolute_time() - t0)
        print("File loaded: \(content.count) chars in \(elapsed)ms")

        DispatchQueue.main.async { [weak self] in
            self?.setupTextView(content: content, loadMs: elapsed)
        }
    }

    // ── TextKit 2 setup ───────────────────────────────────────────────────────

    private func setupTextView(content: String, loadMs: Double) {
        os_signpost(.begin, log: signpostLog, name: "SetupTextView")
        progressLabel?.removeFromSuperview()
        progressLabel = nil

        // ── TextKit 2 stack ────────────────────────────────────────────────
        textStorage = NSTextStorage(string: content)

        // NSTextLayoutManager is the TextKit 2 layout engine (vs NSLayoutManager in TK1)
        textLayoutManager = NSTextLayoutManager()

        // Non-contiguous layout: only layout visible ranges → critical for 100MB files
        textLayoutManager.textViewportLayoutController.delegate = self

        let textContainer = NSTextContainer(
            size: CGSize(width: view.bounds.width - 60, height: .greatestFiniteMagnitude)
        )
        textContainer.widthTracksTextView = true
        textLayoutManager.textContainer = textContainer

        let contentStorage = NSTextContentStorage()
        contentStorage.addTextLayoutManager(textLayoutManager)

        // Attach storage to content storage
        let attrStr = NSMutableAttributedString(string: content, attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
            .foregroundColor: NSColor(calibratedRed: 0.788, green: 0.820, blue: 0.851, alpha: 1),
        ])
        contentStorage.performEditingTransaction { contentStorage.attributedString = attrStr }

        // ── NSScrollView + NSTextView ──────────────────────────────────────
        scrollView = NSScrollView(frame: view.bounds)
        scrollView.autoresizingMask = [.width, .height]
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.drawsBackground = false
        scrollView.contentView.drawsBackground = false

        textView = NSTextView(frame: view.bounds, textContainer: textContainer)
        textView.backgroundColor = NSColor(calibratedRed: 0.102, green: 0.106, blue: 0.118, alpha: 1)
        textView.textColor = NSColor(calibratedRed: 0.788, green: 0.820, blue: 0.851, alpha: 1)
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = true
        textView.isVerticallyResizable = true
        textView.autoresizingMask = .width
        textView.minSize = NSSize(width: 0, height: view.bounds.height)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.containerSize = NSSize(width: view.bounds.width, height: .greatestFiniteMagnitude)

        scrollView.documentView = textView
        view.addSubview(scrollView)

        os_signpost(.end, log: signpostLog, name: "SetupTextView")
        print("TextKit 2 setup complete. LoadMs: \(loadMs)")

        // Display HUD
        setupHUD(loadMs: loadMs, lineCount: content.components(separatedBy: "\n").count)
    }

    // ── HUD ───────────────────────────────────────────────────────────────────

    private var hudView: HUDView?

    private func setupHUD(loadMs: Double, lineCount: Int) {
        let hud = HUDView(frame: NSRect(x: view.bounds.width - 230, y: view.bounds.height - 180, width: 220, height: 170))
        hud.autoresizingMask = [.minXMargin, .minYMargin]
        hud.pocId = "2A — TextKit 2"
        hud.loadMs = loadMs
        hud.lineCount = lineCount
        view.addSubview(hud)
        hudView = hud
    }

    // ── Benchmark ─────────────────────────────────────────────────────────────

    func startBenchmark() {
        guard let scrollView = scrollView else { return }
        let runner = BenchmarkRunner(
            scrollView: scrollView,
            args: args,
            hudView: hudView
        )
        self.benchmark = runner
        runner.start()
    }
}

// ── NSTextViewportLayoutControllerDelegate ──────────────────────────────────

extension ViewController: NSTextViewportLayoutControllerDelegate {
    func viewportBounds(for textViewportLayoutController: NSTextViewportLayoutController) -> CGRect {
        // Return the currently visible rect + a margin for pre-layout
        guard let scrollView = scrollView else { return .zero }
        var rect = scrollView.contentView.bounds
        rect = rect.insetBy(dx: 0, dy: -args.lineHeight * 50) // 50 line pre-roll margin
        return rect
    }

    func textViewportLayoutController(
        _ textViewportLayoutController: NSTextViewportLayoutController,
        configureRenderingSurfaceFor textLayoutFragment: NSTextLayoutFragment
    ) {
        // No custom surface needed — NSTextView handles rendering
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

private func machTimeToMs(_ machTime: UInt64) -> Double {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    let ns = machTime * UInt64(info.numer) / UInt64(info.denom)
    return Double(ns) / 1_000_000.0
}
