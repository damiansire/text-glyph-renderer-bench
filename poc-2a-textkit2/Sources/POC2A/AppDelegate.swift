import AppKit

@main
struct POC2AApp {
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.run()
    }
}

// ── AppDelegate ───────────────────────────────────────────────────────────────

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var viewController: ViewController!

    /// Parse CLI arguments for benchmark mode.
    func applicationDidFinishLaunching(_ notification: Notification) {
        let args = CommandLineArgs.parse()

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1440, height: 900),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "PoC 2A — TextKit 2 (NSTextView)"
        window.center()

        viewController = ViewController(args: args)
        window.contentViewController = viewController
        window.makeKeyAndOrderFront(nil)

        if args.benchmark {
            // Start benchmark after a short settle delay
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.viewController.startBenchmark()
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

// ── CLI argument parsing ───────────────────────────────────────────────────────

struct CommandLineArgs {
    var file: String = ""
    var benchmark: Bool = false
    var scrollPxPerFrame: CGFloat = 60
    var scrollFrames: Int = 3600
    var lineHeight: CGFloat = 20

    static func parse() -> Self {
        var args = Self()
        let argv = CommandLine.arguments.dropFirst()
        var i = argv.startIndex
        while i < argv.endIndex {
            switch argv[i] {
            case "--file":         i = argv.index(after: i); args.file = argv[i]
            case "--benchmark":    args.benchmark = true
            case "--scroll-px":    i = argv.index(after: i); args.scrollPxPerFrame = CGFloat(Double(argv[i]) ?? 60)
            case "--frames":       i = argv.index(after: i); args.scrollFrames = Int(argv[i]) ?? 3600
            case "--line-height":  i = argv.index(after: i); args.lineHeight = CGFloat(Double(argv[i]) ?? 20)
            default: break
            }
            i = argv.index(after: i)
        }
        if args.file.isEmpty {
            // Default to shared test file
            let here = URL(fileURLWithPath: #file).deletingLastPathComponent()
            args.file = here
                .deletingLastPathComponent()         // POC2A/
                .deletingLastPathComponent()         // Sources/
                .deletingLastPathComponent()         // poc-2a-textkit2/
                .appendingPathComponent("shared/test-data/test_100mb.txt")
                .path
        }
        return args
    }
}
