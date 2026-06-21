/**
 * atlas.js — LRU Texture Atlas (2048×2048) for PoC 1C WebGPU
 *
 * Strategy: Shelf packing + LRU eviction + slot free-list.
 *   - Atlas is divided into shelves of fixed height (= max glyph height on that shelf).
 *   - Glyphs pack left-to-right on a shelf. When a shelf is full, a new shelf starts.
 *   - When the atlas is full, the least-recently-used glyph is evicted and its
 *     slot rectangle returns to a free-list, so a later glyph reuses that space
 *     instead of forcing a whole-atlas rebuild.
 *
 * Key: GlyphId is a string `"${fontFamily}:${codepoint}:${sizePx}"`.
 *
 * All rasterization uses an OffscreenCanvas (CPU) and `GPUQueue.writeTexture()`.
 * On Apple Silicon (UMA), `writeTexture` copies the bytes and re-tiles them into
 * the texture's internal tiled/twiddled (Morton-order) layout. The atlas is
 * `r8unorm` (uncompressed), so this is a copy + swizzle — NOT compression and
 * NOT ASTC. It is still a copy, unlike Metal `makeBuffer(bytesNoCopy:)`.
 */

'use strict';

const ATLAS_SIZE = 2048;   // pixels (square)
const CELL_PADDING = 2;      // px padding around each glyph (prevents bleeding)

class TextureAtlas {
    /**
     * @param {GPUDevice} device
     */
    constructor(device) {
        this._device = device;
        this._size = ATLAS_SIZE;

        // GPU texture (R8Unorm — single channel, 8-bit alpha; saves 4× VRAM vs RGBA8)
        this._texture = device.createTexture({
            label: 'glyph-atlas',
            size: [ATLAS_SIZE, ATLAS_SIZE, 1],
            format: 'r8unorm',
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
        });

        this._view = this._texture.createView({ label: 'glyph-atlas-view' });

        // Shelf state
        this._shelves = [];      // Array<{ nextX, y, height }>
        this._nextShelfY = 0;

        // Free-list of reclaimed slot rectangles (from LRU eviction). A future
        // glyph reuses one of these instead of growing the atlas append-only.
        this._freeList = [];     // Array<{ x, y, w, h }>  (h = shelf row height)
        // Allocation rectangle per cached glyph, needed to free it on eviction.
        this._rects = new Map(); // GlyphKey → { x, y, w, h }

        // Glyph cache: Map<GlyphKey, AtlasSlot>
        // AtlasSlot: { u, v, w, h, offsetX, offsetY, advance }  all in atlas-pixels
        this._cache = new Map();

        // LRU: doubly-linked list of { key, prev, next }
        this._lruHead = null;   // most recently used
        this._lruTail = null;   // least recently used
        this._lruMap = new Map(); // key → node

        // Rasterisation canvas (reused across calls)
        this._rasterCanvas = new OffscreenCanvas(256, 256);
        this._rasterCtx = this._rasterCanvas.getContext('2d', { willReadFrequently: true });

        // Stats
        this.stats = { hits: 0, misses: 0, evictions: 0 };
    }

    get texture() { return this._texture; }
    get view() { return this._view; }
    get size() { return this._size; }

    // ── Public API ────────────────────────────────────────────────────────────

    /**
     * Get or insert a glyph into the atlas.
     * Returns an `AtlasSlot` with UV coordinates in atlas-pixel space.
     * Caller must divide by ATLAS_SIZE to get [0,1] UVs for the shader.
     *
     * @param {string} fontFamily   e.g. "Inter"
     * @param {number} codepoint    Unicode codepoint
     * @param {number} sizePx       font size in CSS pixels
     * @returns {AtlasSlot | null}  null if the glyph is invisible (space, control char)
     */
    get(fontFamily, codepoint, sizePx) {
        const key = `${fontFamily}:${codepoint}:${sizePx}`;

        if (this._cache.has(key)) {
            this.stats.hits++;
            this._lruTouch(key);
            return this._cache.get(key);
        }

        this.stats.misses++;
        return this._rasterizeAndInsert(key, fontFamily, codepoint, sizePx);
    }

    get hitRate() {
        const total = this.stats.hits + this.stats.misses;
        return total === 0 ? 1 : this.stats.hits / total;
    }

    get fillRatio() {
        return this._nextShelfY / ATLAS_SIZE;
    }

    // ── Rasterisation ─────────────────────────────────────────────────────────

    _rasterizeAndInsert(key, fontFamily, codepoint, sizePx) {
        const char = String.fromCodePoint(codepoint);
        const font = `${sizePx}px ${fontFamily}, monospace`;

        // Measure
        const ctx = this._rasterCtx;
        ctx.font = font;
        const metrics = ctx.measureText(char);
        const w = Math.ceil(metrics.width);
        const ascent = Math.ceil(metrics.actualBoundingBoxAscent);
        const descent = Math.ceil(metrics.actualBoundingBoxDescent);
        const h = ascent + descent;

        if (w <= 0 || h <= 0) return null; // control char / space

        const pw = w + CELL_PADDING * 2;
        const ph = h + CELL_PADDING * 2;

        // Find or create a shelf with enough space
        const slot = this._allocate(pw, ph);
        if (!slot) return null;

        // Remember the rectangle this glyph occupies (the slot's full row
        // height) so `_evictLRU` can return it to the free-list later.
        this._rects.set(key, { x: slot.atlasX, y: slot.atlasY, w: pw, h: slot.shelfHeight });

        // Rasterize to OffscreenCanvas
        this._rasterCanvas.width = pw;
        this._rasterCanvas.height = ph;
        ctx.clearRect(0, 0, pw, ph);
        ctx.fillStyle = '#fff';  // white glyph on black bg (atlas is R8Unorm)
        ctx.font = font;
        ctx.textBaseline = 'alphabetic';
        ctx.fillText(char, CELL_PADDING, CELL_PADDING + ascent);

        const imageData = ctx.getImageData(0, 0, pw, ph);
        // Extract alpha channel only (R8Unorm atlas)
        const r8 = new Uint8Array(pw * ph);
        for (let i = 0; i < r8.length; i++) {
            r8[i] = imageData.data[i * 4 + 3]; // alpha channel
        }

        // Upload to GPU atlas texture
        this._device.queue.writeTexture(
            { texture: this._texture, origin: [slot.atlasX, slot.atlasY, 0] },
            r8,
            { bytesPerRow: pw, rowsPerImage: ph },
            [pw, ph, 1]
        );

        const atlasSlot = {
            // UV coords in atlas pixels (divide by ATLAS_SIZE for shader)
            u: slot.atlasX + CELL_PADDING,
            v: slot.atlasY + CELL_PADDING,
            w: w,
            h: h,
            offsetX: -metrics.actualBoundingBoxLeft,
            offsetY: -ascent,
            advance: metrics.width,
        };

        this._cache.set(key, atlasSlot);
        this._lruInsert(key);
        return atlasSlot;
    }

    // ── Shelf packing ─────────────────────────────────────────────────────────

    _allocate(w, h) {
        for (;;) {
            // 1. Reuse a reclaimed slot if one fits (real free-list).
            const reused = this._takeFromFreeList(w, h);
            if (reused) return reused;

            // 2. Fit on an existing shelf.
            for (const shelf of this._shelves) {
                if (shelf.height >= h && shelf.nextX + w <= ATLAS_SIZE) {
                    const x = shelf.nextX;
                    shelf.nextX += w;
                    return { atlasX: x, atlasY: shelf.y, shelfHeight: shelf.height };
                }
            }

            // 3. Start a new shelf if there is vertical room.
            if (this._nextShelfY + h <= ATLAS_SIZE) {
                const y = this._nextShelfY;
                this._shelves.push({ nextX: w, y, height: h });
                this._nextShelfY += h;
                return { atlasX: 0, atlasY: y, shelfHeight: h };
            }

            // 4. Atlas full: evict the LRU glyph (which returns its slot to the
            //    free-list) and loop to retry. No periodic whole-atlas rebuild,
            //    no deep recursion — just reclaim until the glyph fits or there
            //    is nothing left to evict.
            if (!this._evictLRU()) return null;
        }
    }

    /**
     * Best-fit search of the free-list for a rectangle that holds a `w`×`h`
     * glyph (the slot's row height must be ≥ h). On a hit the slot is removed
     * and any unused width to its right is pushed back so it stays reusable.
     *
     * @returns {{ atlasX: number, atlasY: number, shelfHeight: number } | null}
     */
    _takeFromFreeList(w, h) {
        let bestIdx = -1;
        let bestWaste = Infinity;
        for (let i = 0; i < this._freeList.length; i++) {
            const r = this._freeList[i];
            if (r.w >= w && r.h >= h) {
                const waste = (r.w - w) + (r.h - h);
                if (waste < bestWaste) { bestWaste = waste; bestIdx = i; }
            }
        }
        if (bestIdx < 0) return null;

        const r = this._freeList.splice(bestIdx, 1)[0];
        // Split off the leftover width so it can be reused later (skip slivers).
        const REMAINDER_MIN = 4; // px
        if (r.w - w >= REMAINDER_MIN) {
            this._freeList.push({ x: r.x + w, y: r.y, w: r.w - w, h: r.h });
        }
        return { atlasX: r.x, atlasY: r.y, shelfHeight: r.h };
    }

    // ── LRU management ────────────────────────────────────────────────────────

    _lruInsert(key) {
        const node = { key, prev: null, next: this._lruHead };
        if (this._lruHead) this._lruHead.prev = node;
        this._lruHead = node;
        if (!this._lruTail) this._lruTail = node;
        this._lruMap.set(key, node);
    }

    _lruTouch(key) {
        const node = this._lruMap.get(key);
        if (!node || node === this._lruHead) return;
        // Detach
        if (node.prev) node.prev.next = node.next;
        if (node.next) node.next.prev = node.prev;
        if (node === this._lruTail) this._lruTail = node.prev;
        // Move to head
        node.prev = null;
        node.next = this._lruHead;
        if (this._lruHead) this._lruHead.prev = node;
        this._lruHead = node;
    }

    _evictLRU() {
        if (!this._lruTail) return false;
        const key = this._lruTail.key;
        this._cache.delete(key);
        this._lruMap.delete(key);
        if (this._lruTail.prev) this._lruTail.prev.next = null;
        this._lruTail = this._lruTail.prev;
        if (!this._lruTail) this._lruHead = null;

        // Reclaim the slot: return its rectangle to the free-list so a later
        // glyph can reuse the space. This is what stops a single miss against a
        // full atlas from cascading into ~512 evictions + a whole-atlas rebuild.
        const rect = this._rects.get(key);
        if (rect) {
            this._rects.delete(key);
            this._freeList.push(rect);
        }

        this.stats.evictions++;
        return true;
    }

    destroy() {
        this._texture.destroy();
    }
}

window.TextureAtlas = TextureAtlas;
