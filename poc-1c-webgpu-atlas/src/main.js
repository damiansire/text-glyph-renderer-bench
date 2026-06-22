/**
 * main.js — Electron main process for PoC 1C (WebGPU Atlas)
 * Enables --enable-unsafe-webgpu for Electron 30's WebGPU support.
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

// WebGPU requires these flags in Electron 30
app.commandLine.appendSwitch('enable-unsafe-webgpu');
app.commandLine.appendSwitch('enable-features', 'Vulkan,UseSkiaRenderer');

app.whenReady().then(() => {
    const win = new BrowserWindow({
        width: 1440, height: 900,
        webPreferences: {
            // DEUDA CONSCIENTE (audit: electron-sin-aislamiento): este harness de
            // benchmark local lee el corpus de 100 MB con `fs` sincrono desde el
            // renderer, por eso usa nodeIntegration. No carga contenido remoto ni
            // entradas no confiables. Si esto creciera a app real, migrar a
            // preload + contextBridge con contextIsolation:true y mover el I/O al
            // proceso main.
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
    fs.writeFileSync(path.join(dir, '1c-webgpu-atlas_stats.json'), JSON.stringify(stats, null, 2));
    if (cli.benchmark) setTimeout(() => app.quit(), 500);
});

ipcMain.on('log', (_e, msg) => process.stdout.write(`[1C] ${msg}\n`));
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });
