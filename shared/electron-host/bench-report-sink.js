/**
 * bench-report-sink.js — the report-sink contract shared by the four Electron
 * PoCs (1A/1B/1C/1D).
 *
 * Why this exists: each PoC had its own copy of the `benchmark-complete`
 * handler, and the copies diverged. Three audit fixes (canonical output
 * directory, sender-frame filter, malformed-stats guard) landed in 1A/1B/1C and
 * were never applied to 1D, whose reports would have been written where the
 * orchestrator does not look. These three rules are the contract, so they live
 * once and are unit-testable without an Electron window.
 */

'use strict';

const path = require('path');

/**
 * Resolve where a PoC must write its `*_stats.json`.
 *
 * The orchestrator passes an absolute `BENCH_RESULTS_DIR` (the canonical
 * `<repo>/results`) precisely because each PoC runs with a different cwd; a PoC
 * that ignores it drops out of the comparison with a "result file not found".
 *
 * @param {string} fallbackDir — directory used for standalone runs.
 * @param {NodeJS.ProcessEnv} [env]
 * @returns {string}
 */
function resolveResultsDir(fallbackDir, env = process.env) {
    return env.BENCH_RESULTS_DIR || fallbackDir;
}

/**
 * The shape guard for stats arriving over IPC: the only barrier between an
 * arbitrary IPC payload and a file written to disk.
 *
 * @param {unknown} stats
 * @returns {boolean}
 */
function isWellFormedStats(stats) {
    return (
        !!stats &&
        typeof stats === 'object' &&
        !Array.isArray(stats) &&
        typeof stats.poc_id === 'string' &&
        stats.poc_id.length > 0
    );
}

/**
 * Accept a `benchmark-complete` message only when it comes from the PoC's own
 * main frame.
 *
 * @param {{senderFrame?: unknown, sender?: {mainFrame?: unknown}}} event
 * @returns {boolean}
 */
function isFromOwnMainFrame(event) {
    if (!event || !event.senderFrame) {
        // No identifiable frame: keep the historical behaviour of the three
        // PoCs that already shipped this guard (accept, and let the shape guard
        // above decide). Tightening it to a rejection is a separate change: it
        // can drop the one legitimate report of a run whose frame was already
        // torn down.
        return true;
    }
    return event.senderFrame === (event.sender && event.sender.mainFrame);
}

/**
 * Full path of a PoC's canonical report artifact.
 *
 * @param {string} pocId — the schema id, e.g. `1d-webgpu-msdf`.
 * @param {string} fallbackDir
 * @param {NodeJS.ProcessEnv} [env]
 * @returns {string}
 */
function resolveReportPath(pocId, fallbackDir, env = process.env) {
    return path.join(resolveResultsDir(fallbackDir, env), `${pocId}_stats.json`);
}

module.exports = {
    resolveResultsDir,
    resolveReportPath,
    isWellFormedStats,
    isFromOwnMainFrame,
};
