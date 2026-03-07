'use strict';

/**
 * metrics.js — FrameMetrics probe for PoC 1A (Web DOM)
 *
 * Captures:
 *   - Frame wall-clock time (performance.now() delta)
 *   - Dropped frames (> 8.33ms budget at 120Hz)
 *   - Scroll offset, lines visible
 *   - Running percentiles (P50, P95, P99) via histogram approximation
 *   - Final JSON report matching shared/metrics/frame_stats.schema.json
 *
 * Implementation note:
 *   `PerformanceObserver` for `frame` entries is NOT available in Electron's
 *   renderer (it's a Chrome DevTools feature, not a stable web API).
 *   We use `performance.now()` deltas around the render call instead.
 *
 *   For GPU timing in PoCs 1C/1D, WebGPU `GPUQuerySet` timestamp queries
 *   will replace the CPU-side approximation here.
 */

class FrameMetrics {
    /**
     * @param {string} pocId  — e.g. '1a-web-dom'
     */
    constructor(pocId) {
        this._pocId = pocId;
        this._frames = [];   // array of frame time in ms (for percentile computation)
        this._dropped = 0;
        this._lastFrameMs = 0;
        this._linesVisible = 0;
        this._frameIndex = 0;
        this._budgetMs = 1000 / 120; // 8.333ms at 120Hz
        this._startTs = performance.now();

        // Key-to-pixel tracking (for interactive mode)
        this._pendingKeyTs = null;
        document.addEventListener('keydown', (e) => {
            this._pendingKeyTs = e.timeStamp;
        }, { capture: true });
    }

    /**
     * Called by the virtualizer after each render pass.
     * @param {object} opts
     * @param {number} opts.frameMs       — CPU render time in ms
     * @param {number} opts.scrollY       — current scroll offset in px
     * @param {number} opts.linesVisible  — number of lines rendered
     * @param {number} opts.glyphsRendered — estimated glyph count
     */
    recordFrame({ frameMs, scrollY, linesVisible, glyphsRendered = 0 }) {
        this._lastFrameMs = frameMs;
        this._linesVisible = linesVisible;
        this._frameIndex++;

        const dropped = frameMs > this._budgetMs;
        if (dropped) this._dropped++;

        // Store frame times (keep last 10k to bound memory)
        if (this._frames.length >= 10_000) {
            this._frames.shift();
        }
        this._frames.push(frameMs);

        // Key-to-pixel: if there's a pending key event, estimate K2P
        let k2p_ns = null;
        if (this._pendingKeyTs !== null) {
            k2p_ns = Math.round((performance.now() - this._pendingKeyTs) * 1e6);
            this._pendingKeyTs = null;
        }

        // Emit to DevTools timeline (visible in Electron DevTools Performance tab)
        if (performance.mark) {
            performance.mark(`frame-${this._frameIndex}`);
        }

        return {
            poc_id: this._pocId,
            timestamp_ms: performance.now() - this._startTs,
            frame_index: this._frameIndex,
            cpu_encode_ns: Math.round(frameMs * 1e6),
            gpu_render_ns: 0,         // unavailable for DOM rendering
            key_to_pixel_ns: k2p_ns,
            atlas_hit_rate: null,     // N/A for DOM
            atlas_fill_ratio: null,
            glyphs_rendered: glyphsRendered,
            lines_visible: linesVisible,
            scroll_offset_px: scrollY,
            dropped_frame: dropped,
            frame_time_ms: frameMs,
        };
    }

    // ── Live snapshot (for HUD updates) ──────────────────────────────────────

    snapshot() {
        const sorted = [...this._frames].sort((a, b) => a - b);
        const n = sorted.length;
        const p95 = n > 0 ? sorted[Math.floor(n * 0.95)] : 0;
        return {
            lastFrameMs: this._lastFrameMs,
            droppedFrames: this._dropped,
            linesVisible: this._linesVisible,
            p95Ms: p95,
        };
    }

    // ── Final report ──────────────────────────────────────────────────────────

    /**
     * Compute final benchmark report.
     * @param {number} load_ms    — file load time in ms
     * @param {number} split_ms   — line split time in ms
     * @param {number} lineCount  — total lines in the file
     */
    report(load_ms, split_ms, lineCount) {
        const sorted = [...this._frames].sort((a, b) => a - b);
        const n = sorted.length;

        const percentile = (p) => n > 0 ? sorted[Math.floor(n * p / 100)] : 0;
        const avg = n > 0 ? this._frames.reduce((a, b) => a + b, 0) / n : 0;
        const dropRate = n > 0 ? (this._dropped / n) * 100 : 0;

        return {
            poc_id: this._pocId,
            meta: {
                platform: navigator.platform,
                userAgent: navigator.userAgent,
                devicePixelRatio: window.devicePixelRatio,
                innerWidth: window.innerWidth,
                innerHeight: window.innerHeight,
                timestamp: new Date().toISOString(),
            },
            file: {
                line_count: lineCount,
                load_ms: Math.round(load_ms * 100) / 100,
                split_ms: Math.round(split_ms * 100) / 100,
            },
            benchmark: {
                total_frames: n,
                dropped_frames: this._dropped,
                drop_rate_pct: Math.round(dropRate * 100) / 100,
                avg_frame_ms: Math.round(avg * 1000) / 1000,
                p50_ms: Math.round(percentile(50) * 1000) / 1000,
                p95_ms: Math.round(percentile(95) * 1000) / 1000,
                p99_ms: Math.round(percentile(99) * 1000) / 1000,
                p999_ms: Math.round(percentile(99.9) * 1000) / 1000,
                budget_ms: this._budgetMs,
            },
        };
    }
}

window.FrameMetrics = FrameMetrics;
