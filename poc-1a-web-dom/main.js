/**
 * main.js — Electron main process for PoC 1A (Web DOM Baseline)
 *
 * Responsibilities:
 *  1. Create the BrowserWindow with hardware acceleration enabled.
 *  2. Load the renderer HTML page.
 *  3. Pass CLI flags (--benchmark, --file) to the renderer via command-line args.
 *  4. Handle IPC messages: receive FrameStats from renderer, write JSON results.
 *  5. Quit after benchmark completes (if --benchmark flag).
 *
 * Architecture note:
 *  File loading is done in the renderer process via Node.js integration
 *  (contextIsolation: false) to avoid IPC overhead during the hot path.
 *  In production code you'd use contextBridge; here we optimize for
 *  accurate benchmark data with minimal overhead.
 */

const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('path');
const fs = require('fs');
const {
    resolveReportPath,
    isWellFormedStats,
    isFromOwnMainFrame,
} = require('../shared/electron-host/bench-report-sink.js');

// ── CLI flag parsing ──────────────────────────────────────────────────────────

function parseArgs() {
    const argv = process.argv.slice(2);
    const args = {
        benchmark: false,
        headless: false,
        file: path.join(__dirname, '..', 'shared', 'test-data', 'test_100mb.txt'),
        scrollPxPerFrame: 60,
        scrollFrames: 3600, // 30s at 120Hz
        lineHeight: 20,
    };

    for (let i = 0; i < argv.length; i++) {
        switch (argv[i]) {
            case '--benchmark': args.benchmark = true; break;
            case '--headless': args.headless = true; break;
            case '--file': args.file = argv[++i]; break;
            case '--scroll-px': args.scrollPxPerFrame = parseFloat(argv[++i]); break;
            case '--frames': args.scrollFrames = parseInt(argv[++i], 10); break;
            case '--line-height': args.lineHeight = parseFloat(argv[++i]); break;
        }
    }
    return args;
}

const cliArgs = parseArgs();

// ── Window creation ───────────────────────────────────────────────────────────

let mainWindow = null;

function createWindow() {
    mainWindow = new BrowserWindow({
        width: 1440,
        height: 900,
        show: !cliArgs.headless,
        // Hardware acceleration must be ON (default) for ProMotion/120Hz support.
        // Disable only for headless CI where there's no GPU.
        webPreferences: {
            nodeIntegration: true,       // allow require() in renderer (benchmark only)
            contextIsolation: false,     // disable sandbox for direct fs access
            backgroundThrottling: false, // never throttle RAF when benchmarking
        },
    });

    // Pass args to renderer via URL query params (avoids IPC round-trip before load)
    const params = new URLSearchParams({
        file: cliArgs.file,
        benchmark: cliArgs.benchmark ? '1' : '0',
        scrollPx: cliArgs.scrollPxPerFrame,
        scrollFrames: cliArgs.scrollFrames,
        lineHeight: cliArgs.lineHeight,
    });

    const rendererUrl = `file://${path.join(__dirname, 'renderer', 'index.html')}?${params}`;
    mainWindow.loadURL(rendererUrl);

    if (!cliArgs.headless && process.env.NODE_ENV === 'development') {
        mainWindow.webContents.openDevTools({ mode: 'detach' });
    }

    mainWindow.on('closed', () => { mainWindow = null; });
}

// ── IPC: receive benchmark results from renderer ──────────────────────────────

ipcMain.on('benchmark-complete', (event, stats) => {
    if (!isFromOwnMainFrame(event)) return;
    if (!isWellFormedStats(stats)) {
        process.stderr.write('[renderer] ignoring malformed benchmark stats\n');
        return;
    }
    console.log('\n=== PoC 1A Benchmark Results ===');
    console.log(JSON.stringify(stats, null, 2));

    const outFile = resolveReportPath('1a-web-dom', path.join(__dirname, '..', 'results'));
    fs.mkdirSync(path.dirname(outFile), { recursive: true });
    fs.writeFileSync(outFile, JSON.stringify(stats, null, 2));
    console.log(`\nResults written to ${outFile}`);

    if (cliArgs.benchmark) {
        // Auto-quit after benchmark finishes
        setTimeout(() => app.quit(), 500);
    }
});

ipcMain.on('log', (_event, msg) => {
    process.stdout.write(`[renderer] ${msg}\n`);
});

// ── App lifecycle ─────────────────────────────────────────────────────────────

app.whenReady().then(() => {
    // On macOS, enable ProMotion (120Hz) display support
    app.commandLine.appendSwitch('enable-features', 'VaapiVideoDecodeLinuxGL');

    createWindow();

    app.on('activate', () => {
        if (BrowserWindow.getAllWindows().length === 0) createWindow();
    });
});

app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') app.quit();
});
