/**
 * Tests for the shared Electron report-sink contract. Runs with the Node
 * built-in runner (`node --test shared/electron-host/`), no Electron and no GPU
 * needed: these are the pure rules, which is exactly why they can be tested at
 * all now that they are not four copies inside four `main.js`.
 */

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const {
    resolveResultsDir,
    resolveReportPath,
    isWellFormedStats,
    isFromOwnMainFrame,
} = require('./bench-report-sink.js');

test('BENCH_RESULTS_DIR wins over the standalone fallback', () => {
    const dir = resolveResultsDir('/poc/local/results', {
        BENCH_RESULTS_DIR: '/repo/results',
    });
    assert.equal(dir, '/repo/results');
});

test('falls back to the local directory for standalone runs', () => {
    assert.equal(resolveResultsDir('/poc/local/results', {}), '/poc/local/results');
});

test('the report is named after the schema poc_id, in the canonical directory', () => {
    const p = resolveReportPath('1d-webgpu-msdf', '/poc/local/results', {
        BENCH_RESULTS_DIR: '/repo/results',
    });
    assert.equal(p, path.join('/repo/results', '1d-webgpu-msdf_stats.json'));
});

test('malformed stats are rejected before anything is written', () => {
    for (const bad of [null, undefined, 42, 'stats', [], {}, { poc_id: 7 }, { poc_id: '' }]) {
        assert.equal(isWellFormedStats(bad), false, `should reject ${JSON.stringify(bad)}`);
    }
});

test('a report carrying a poc_id is accepted', () => {
    assert.equal(isWellFormedStats({ poc_id: '1d-webgpu-msdf', benchmark: {} }), true);
});

test('a message from a frame other than the main frame is rejected', () => {
    const mainFrame = { id: 'main' };
    const injected = { id: 'injected' };
    assert.equal(
        isFromOwnMainFrame({ senderFrame: injected, sender: { mainFrame } }),
        false,
    );
    assert.equal(
        isFromOwnMainFrame({ senderFrame: mainFrame, sender: { mainFrame } }),
        true,
    );
});
