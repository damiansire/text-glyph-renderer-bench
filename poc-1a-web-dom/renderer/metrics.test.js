'use strict';

/**
 * metrics.test.js — unit tests for the FrameMetrics counting logic.
 *
 * Runs under plain `node --test` (no Electron, no DOM): the drop-rate math is
 * pure and used to be wrong exactly where no test looked — past 10k frames the
 * whole-run dropped counter was divided by the 10k percentile window, so
 * `drop_rate_pct` exceeded 100 and the schema gate rejected the report.
 */

const { test } = require('node:test');
const assert = require('node:assert');

const { FrameMetrics } = require('./metrics.js');

const frame = (frameMs) => ({ frameMs, scrollY: 0, linesVisible: 10 });

test('drop rate uses the whole run, not the 10k percentile window', () => {
    const m = new FrameMetrics('1a-web-dom');
    for (let i = 0; i < 15000; i++) m.recordFrame(frame(20)); // over budget
    for (let i = 0; i < 5000; i++) m.recordFrame(frame(1));   // under budget
    const r = m.report(0, 0, 1).benchmark;
    assert.strictEqual(r.total_frames, 20000);
    assert.strictEqual(r.dropped_frames, 15000);
    assert.strictEqual(r.drop_rate_pct, 75);
});

test('drop rate never exceeds the schema maximum of 100', () => {
    const m = new FrameMetrics('1a-web-dom');
    // 20k dropped frames over a 10k window used to report 200 %.
    for (let i = 0; i < 20000; i++) m.recordFrame(frame(20));
    const r = m.report(0, 0, 1).benchmark;
    assert.strictEqual(r.total_frames, 20000);
    assert.strictEqual(r.dropped_frames, 20000);
    assert.strictEqual(r.drop_rate_pct, 100);
});

test('the percentile window is declared next to the percentiles', () => {
    const m = new FrameMetrics('1a-web-dom');
    for (let i = 0; i < 12000; i++) m.recordFrame(frame(2));
    const r = m.report(0, 0, 1).benchmark;
    // Percentiles describe only the last 10k frames; the report says so
    // instead of implying they cover all 12k.
    assert.strictEqual(r.percentile_window_frames, 10000);
    assert.strictEqual(r.total_frames, 12000);
});

test('an empty run reports zeros, not NaN', () => {
    const m = new FrameMetrics('1a-web-dom');
    const r = m.report(0, 0, 0).benchmark;
    assert.strictEqual(r.total_frames, 0);
    assert.strictEqual(r.drop_rate_pct, 0);
    assert.strictEqual(r.p50_ms, 0);
});
