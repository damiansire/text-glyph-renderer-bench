'use strict';
/**
 * worker.js — OffscreenCanvas Worker for PoC 1B (Canvas 2D)
 *
 * Lives in a dedicated Worker thread.  The main thread transfers an
 * OffscreenCanvas to this worker via `postMessage` with a Transferable.
 *
 * Responsibilities:
 *   1. Receive the OffscreenCanvas + the full lines array.
 *   2. On each `render` command, draw the visible slice of lines.
 *   3. Report frame timing back to the main thread via `postMessage`.
 *
 * Shaping strategy:
 *   - Uses Canvas 2D `ctx.measureText()` for advance widths (no full OpenType shaping).
 *   - This deliberately limits this PoC: no ligatures, no kerning, no bidi.
 *   - The goal is to measure Canvas throughput, not shaping quality.
 *
 * ImageBitmap transfer:
 *   - We do NOT use `transferToImageBitmap()` + postMessage here because
 *     OffscreenCanvas commit() is more efficient — it avoids an extra copy.
 *   - `canvas.commit()` pushes the current frame to the main thread compositor
 *     without any IPC overhead (the backing store is shared).
 */

let canvas = null;
let ctx = null;
let lines = [];
let lineH = 20;

// Styling constants (match the DOM PoC for visual consistency)
const BG_COLOR = '#1a1b1e';
const TEXT_COLOR = '#c9d1d9';
const LN_COLOR = '#4a5568';
const FONT = '13px "JetBrains Mono", "Fira Code", monospace';
const LN_WIDTH = 48;   // px reserved for line numbers
const PADDING = 8;    // px left/right content padding

self.onmessage = function (e) {
    const { type, data } = e.data;

    switch (type) {
        // ── init: receive OffscreenCanvas + all lines ─────────────────────────
        case 'init': {
            canvas = data.canvas;   // transferred OffscreenCanvas
            lines = data.lines;
            lineH = data.lineHeight;
            ctx = canvas.getContext('2d', {
                alpha: false,          // opaque — avoids alpha blending overhead
                desynchronized: true,  // low-latency mode (skip compositor sync where possible)
            });
            ctx.font = FONT;
            self.postMessage({ type: 'ready', lineCount: lines.length });
            break;
        }

        // ── resize: viewport changed ──────────────────────────────────────────
        case 'resize': {
            canvas.width = data.width;
            canvas.height = data.height;
            // Re-set font after resize (canvas state resets)
            ctx.font = FONT;
            break;
        }

        // ── render: draw a frame ─────────────────────────────────────────────
        case 'render': {
            const { scrollY, viewportW, viewportH } = data;
            const t0 = performance.now();

            const firstLine = Math.max(0, Math.floor(scrollY / lineH));
            const visCount = Math.ceil(viewportH / lineH) + 2; // +2 for partial lines
            const lastLine = Math.min(lines.length - 1, firstLine + visCount);

            // ── Clear background ────────────────────────────────────────────────
            ctx.fillStyle = BG_COLOR;
            ctx.fillRect(0, 0, viewportW, viewportH);

            // ── Draw line numbers + text ─────────────────────────────────────────
            ctx.font = FONT;  // ensure consistent after any resize
            ctx.textBaseline = 'top';

            for (let i = firstLine; i <= lastLine; i++) {
                const y = (i - firstLine) * lineH; // relative to viewport top

                // Line number
                ctx.fillStyle = LN_COLOR;
                ctx.textAlign = 'right';
                ctx.fillText(String(i + 1), LN_WIDTH - PADDING, y + 2);

                // Line content
                ctx.fillStyle = TEXT_COLOR;
                ctx.textAlign = 'left';
                const text = lines[i] || '';
                // Clip long lines to avoid measuring/drawing off-screen chars
                ctx.fillText(text, LN_WIDTH + PADDING, y + 2, viewportW - LN_WIDTH - PADDING * 2);
            }

            // ── Commit (compositor-path copy, no IPC serialization) ──────────────
            // `canvas.commit()` is the OffscreenCanvas-specific call that sends the
            // current frame to the page compositor without going through postMessage.
            if (typeof canvas.commit === 'function') {
                canvas.commit();
            }

            const frameMs = performance.now() - t0;

            self.postMessage({
                type: 'frame-done',
                frameMs,
                firstLine,
                linesVisible: lastLine - firstLine + 1,
            });
            break;
        }

        default:
            console.warn('[worker] unknown message type:', type);
    }
};
