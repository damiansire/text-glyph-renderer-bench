/**
 * main.js — Electron main process for PoC 1B (Canvas 2D)
 */
const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('path');
const fs = require('fs');

function parseArgs() {
    const argv = process.argv.slice(2);
    const a = {
        benchmark: false,
        file: path.join(__dirname, '..', '..', 'shared', 'test-data', 'test_100mb.txt'),
        scrollPx: 60, scrollFrames: 3600, lineHeight: 20,
    };
    for (let i = 0; i < argv.length; i++) {
        switch (argv[i]) {
            case '--benchmark': a.benchmark = true; break;
            case '--file': a.file = argv[++i]; break;
            case '--scroll-px': a.scrollPx = parseFloat(argv[++i]); break;
            case '--frames': a.scrollFrames = parseInt(argv[++i], 10); break;
            case '--line-height': a.lineHeight = parseFloat(argv[++i]); break;
        }
    }
    return a;
}

const cli = parseArgs();

app.whenReady().then(() => {
    const win = new BrowserWindow({
        width: 1440, height: 900,
        webPreferences: {
            nodeIntegration: true, contextIsolation: false, backgroundThrottling: false,
        },
    });
    const p = new URLSearchParams({
        file: cli.file, benchmark: cli.benchmark ? '1' : '0',
        scrollPx: cli.scrollPx, scrollFrames: cli.scrollFrames, lineHeight: cli.lineHeight,
    });
    win.loadURL(`file://${path.join(__dirname, '..', 'index.html')}?${p}`);
});

ipcMain.on('benchmark-complete', (_e, stats) => {
    console.log(JSON.stringify(stats, null, 2));
    const dir = path.join(__dirname, '..', 'results');
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, '1b-canvas2d_stats.json'), JSON.stringify(stats, null, 2));
    if (cli.benchmark) setTimeout(() => app.quit(), 500);
});

ipcMain.on('log', (_e, msg) => process.stdout.write(`[1B] ${msg}\n`));
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });
