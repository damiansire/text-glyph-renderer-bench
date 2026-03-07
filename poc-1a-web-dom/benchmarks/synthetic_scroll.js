'use strict';

/**
 * synthetic_scroll.js — Programmatic scroll benchmark driver for PoC 1A.
 *
 * Can be run two ways:
 *   1. Embedded: loaded from index.html when ?benchmark=1 is in the URL.
 *      (The main benchmark path — uses requestAnimationFrame for accurate timing.)
 *   2. Standalone: node benchmarks/synthetic_scroll.js  (CLI stats without Electron)
 *      This mode measures pure JS line-index math and string operations,
 *      without any DOM involvement, to separate JS overhead from DOM overhead.
 *
 * Output: JSON matching frame_stats.schema.json written to results/
 */

// ── Standalone Node.js path ──────────────────────────────────────────────────

if (typeof window === 'undefined') {
    // Running in Node (not Electron renderer)
    const fs = require('fs');
    const path = require('path');

    const FILE_PATH = process.argv[2]
        || path.join(__dirname, '..', '..', 'shared', 'test-data', 'test_100mb.txt');

    const SCROLL_PX_PER_FRAME = parseFloat(process.argv[3]) || 60;
    const FRAMES = parseInt(process.argv[4], 10) || 3600;
    const LINE_H = parseFloat(process.argv[5]) || 20;
    const FRAME_BUDGET_US = 8333; // 1/120s in µs

    console.log(`PoC 1A — Node.js baseline (no DOM)`);
    console.log(`File: ${FILE_PATH}`);
    console.log(`Frames: ${FRAMES}  ·  ${SCROLL_PX_PER_FRAME}px/frame  ·  ${LINE_H}px line height\n`);

    // Load
    const t0 = process.hrtime.bigint();
    const content = fs.readFileSync(FILE_PATH, 'utf8');
    const t1 = process.hrtime.bigint();
    const load_ms = Number(t1 - t0) / 1e6;

    // Split
    const t2 = process.hrtime.bigint();
    const lines = content.split('\n');
    const t3 = process.hrtime.bigint();
    const split_ms = Number(t3 - t2) / 1e6;

    console.log(`Lines: ${lines.length}  ·  Load: ${load_ms.toFixed(1)}ms  ·  Split: ${split_ms.toFixed(1)}ms`);

    // Simulate viewport rendering math
    const VIEWPORT_LINES = 50;
    const OVERSCAN = 5;
    const totalH = lines.length * LINE_H;
    let scrollY = 0;
    const frameTimes_ns = [];
    let dropped = 0;

    for (let frame = 0; frame < FRAMES; frame++) {
        const fStart = process.hrtime.bigint();

        const first = Math.max(0, Math.floor(scrollY / LINE_H) - OVERSCAN);
        const last = Math.min(lines.length - 1, first + VIEWPORT_LINES + OVERSCAN * 2 - 1);

        // Simulate textContent assignment (measure string access cost)
        let charCount = 0;
        for (let i = first; i <= last; i++) {
            charCount += (lines[i] || '').length;
        }
        // Dummy use to prevent dead-code elimination
        if (charCount < 0) throw new Error('unreachable');

        scrollY += SCROLL_PX_PER_FRAME;
        if (scrollY + VIEWPORT_LINES * LINE_H > totalH) scrollY = 0;

        const elapsed_ns = Number(process.hrtime.bigint() - fStart);
        frameTimes_ns.push(elapsed_ns);
        if (elapsed_ns > FRAME_BUDGET_US * 1000) dropped++;
    }

    frameTimes_ns.sort((a, b) => a - b);
    const n = frameTimes_ns.length;
    const p = (pct) => frameTimes_ns[Math.floor(n * pct / 100)] / 1e6;

    const result = {
        poc_id: '1a-web-dom-nodejs',
        meta: { note: 'Node.js only — no DOM, no Electron' },
        file: { line_count: lines.length, load_ms, split_ms },
        benchmark: {
            total_frames: n,
            dropped_frames: dropped,
            drop_rate_pct: (dropped / n * 100).toFixed(2),
            p50_ms: p(50).toFixed(3),
            p95_ms: p(95).toFixed(3),
            p99_ms: p(99).toFixed(3),
            budget_ms: FRAME_BUDGET_US / 1000,
        },
    };

    console.log(JSON.stringify(result, null, 2));

    const outDir = 'results';
    fs.mkdirSync(outDir, { recursive: true });
    const outFile = `${outDir}/1a-web-dom-nodejs_stats.json`;
    fs.writeFileSync(outFile, JSON.stringify(result, null, 2));
    console.log(`\nResults → ${outFile}`);

    process.exit(0);
}

// ── Electron renderer path ───────────────────────────────────────────────────
// (Loaded by index.html; the actual benchmark loop lives in index.html's
//  inline script using requestAnimationFrame for accurate wall-clock timing.)
// This file is a no-op when loaded in the renderer.
