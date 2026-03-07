'use strict';

/**
 * virtualizer.js — DOM Recycling Pool Virtualizer
 *
 * Strategy: Only-DOM-touch-visible-lines pattern.
 *   - Maintains a fixed-size pool of DOM `.text-line` elements.
 *   - On scroll, repositions elements via `transform: translateY()` (avoids reflow).
 *   - Pool size = Math.ceil(viewportHeight / lineHeight) + 2 * OVERSCAN.
 *   - No `IntersectionObserver` needed for the recycling itself; a resize
 *     observer handles viewport changes.
 *   - `IntersectionObserver` is used separately on the entire pool container
 *     to pause rendering when the window is occluded (backgroundThrottling: false).
 *
 * This is the WORST CASE baseline: every scroll repositioning goes through
 * the full Blink layout/paint path for possibly-dirty elements.
 *
 * Layout avoidance tricks used:
 *   - All positioning via GPU-composited `transform: translateY` (no `top`).
 *   - `will-change: transform` on the pool container.
 *   - `textContent` assignment (not innerHTML) — avoids HTML parsing overhead.
 *   - Read `clientHeight` once; cache it.
 */

const OVERSCAN = 5; // extra lines above and below viewport

class DOMVirtualizer {
    /**
     * @param {object} opts
     * @param {HTMLElement}   opts.container   — scroll viewport element
     * @param {HTMLElement}   opts.pool        — pool container (position:absolute)
     * @param {string[]}      opts.lines       — pre-split line strings
     * @param {number}        opts.lineHeight  — px height of each line (integer for crisp rendering)
     * @param {FrameMetrics}  opts.metrics     — metrics collector
     */
    constructor({ container, pool, lines, lineHeight, metrics }) {
        this._container = container;
        this._pool = pool;
        this._lines = lines;
        this._lh = lineHeight;
        this._metrics = metrics;
        this._scrollY = 0;
        this._poolNodes = [];
        this._lastFirst = -1; // last rendered first-visible-line index

        // Total virtual height — set on the pool so native scrollbar works in
        // interactive mode (not needed for benchmark but good for UX).
        const totalH = lines.length * lineHeight;
        pool.style.height = totalH + 'px';

        this._viewportH = container.clientHeight || window.innerHeight;
        this._poolSize = Math.ceil(this._viewportH / lineHeight) + 2 * OVERSCAN;

        this._createPool();

        // Handle viewport resize
        const ro = new ResizeObserver(() => {
            this._viewportH = container.clientHeight || window.innerHeight;
            const newSize = Math.ceil(this._viewportH / lineHeight) + 2 * OVERSCAN;
            if (newSize !== this._poolSize) {
                this._poolSize = newSize;
                this._createPool();
                this.render(this._scrollY);
            }
        });
        ro.observe(container);
    }

    get scrollY() { return this._scrollY; }

    // ── Pool creation ─────────────────────────────────────────────────────────

    _createPool() {
        // Remove excess nodes
        while (this._poolNodes.length > this._poolSize) {
            const node = this._poolNodes.pop();
            node.remove();
        }
        // Add missing nodes
        while (this._poolNodes.length < this._poolSize) {
            const el = document.createElement('div');
            el.className = 'text-line';
            el.innerHTML = '<span class="ln"></span><span class="content"></span>';
            this._pool.appendChild(el);
            this._poolNodes.push(el);
        }
        this._lastFirst = -1; // force full redraw
    }

    // ── Core render ───────────────────────────────────────────────────────────

    /**
     * Render the viewport at scroll position `scrollY`.
     * Called by `scrollBy()` and the initial render.
     *
     * @param {number} newScrollY
     */
    render(newScrollY) {
        const t0 = performance.now();

        const lh = this._lh;
        const lines = this._lines;
        const lineCount = lines.length;
        const poolSize = this._poolSize;
        const poolNodes = this._poolNodes;

        // Clamp scroll
        const maxScroll = Math.max(0, lineCount * lh - this._viewportH);
        const clampedY = Math.max(0, Math.min(newScrollY, maxScroll));
        this._scrollY = clampedY;

        const firstLine = Math.max(0, Math.floor(clampedY / lh) - OVERSCAN);
        const lastLine = Math.min(lineCount - 1, firstLine + poolSize - 1);
        const linesVisible = lastLine - firstLine + 1;

        // Check if we need a full redraw or just a repositioning
        const sameFirst = firstLine === this._lastFirst;
        this._lastFirst = firstLine;

        for (let i = 0; i < poolSize; i++) {
            const lineIdx = firstLine + i;
            const node = poolNodes[i];

            if (lineIdx > lastLine) {
                // Hide out-of-range nodes
                node.style.visibility = 'hidden';
                continue;
            }

            node.style.visibility = 'visible';
            // Position via translateY — compositor-only, no layout
            node.style.transform = `translateY(${lineIdx * lh}px)`;

            // Update text content only if the line changed
            if (!sameFirst) {
                const text = lines[lineIdx] || '';
                // Direct property assignment: fastest DOM text update
                node.firstChild.textContent = lineIdx + 1;    // line number span
                node.lastChild.textContent = text;            // content span
            }
        }

        const frameMs = performance.now() - t0;
        this._metrics.recordFrame({
            frameMs,
            scrollY: clampedY,
            linesVisible,
            glyphsRendered: linesVisible * 40, // rough estimate: avg 40 chars/line
        });
    }

    // ── Scroll ────────────────────────────────────────────────────────────────

    scrollBy(deltaY) {
        this.render(this._scrollY + deltaY);
    }

    scrollTo(y) {
        this.render(y);
    }
}

// Export for use in index.html (no module bundler in this PoC)
window.DOMVirtualizer = DOMVirtualizer;
