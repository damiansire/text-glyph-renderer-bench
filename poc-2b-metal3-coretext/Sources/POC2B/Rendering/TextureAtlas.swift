import Metal
import CoreText
import AppKit

// ── TextureAtlasSwift ─────────────────────────────────────────────────────────
// LRU + Shelf-packing atlas for the Metal 3 renderer.
// Mirrors the JS TextureAtlas in poc-1c but uses MTLTexture shared storage (UMA).

struct GlyphAtlasSlot {
    var atlasX:  Int
    var atlasY:  Int
    var width:   Int
    var height:  Int
    var offsetX: CGFloat
    var offsetY: CGFloat
    var advance: CGFloat
}

final class TextureAtlasSwift {
    private let device:   MTLDevice
    let texture:          MTLTexture
    private let size:     Int = 2048
    private let padding:  Int = 2

    private var shelves: [(nextX: Int, y: Int, height: Int)] = []
    private var nextShelfY = 0

    // Key: (glyphID, fontID) → AtlasSlot
    private var cache:   [UInt64: GlyphAtlasSlot] = [:]

    private(set) var hitCount:  Int = 0
    private(set) var missCount: Int = 0

    /// AUDIT FIX (P9): number of times the atlas was full and had to clear and
    /// rebuild, discarding the whole cache mid-measurement. The proper fix is a
    /// multi-texture (paged) atlas; until that lands, this counter lets the
    /// measurement harness detect and discard runs where a reset occurred so a
    /// reset does not silently corrupt steady-state numbers. See `allocate`.
    private(set) var resetCount: Int = 0

    /// Number of atlas textures (pages) currently in use. Hoy es siempre `1`:
    /// el atlas usa una sola textura y, al llenarse, hace clear-and-rebuild
    /// (ver `allocate`). Cuando se implemente el atlas paginado este getter
    /// pasara a `pages.count`. Se expone ya para que el harness de medicion
    /// pueda registrar/asertar el numero de paginas de un run.
    /// Plan completo de paginado: `docs/atlas-paging-plan.md` (P9).
    /// NO VERIFICADO: requiere toolchain Metal (macOS) para compilar.
    var pageCount: Int { 1 }

    var hitRate: Float { missCount == 0 ? 1 : Float(hitCount) / Float(hitCount + missCount) }

    // ── Init ──────────────────────────────────────────────────────────────

    init(device: MTLDevice) {
        self.device = device
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r8Unorm, width: size, height: size, mipmapped: false)
        desc.usage       = [.shaderRead]
        desc.storageMode = .shared  // CPU-writable + GPU-readable via UMA (no copy)
        texture = device.makeTexture(descriptor: desc)!
        texture.label = "GlyphAtlas-2B"
    }

    // ── Lookup / insert ───────────────────────────────────────────────────

    func slot(forGlyph glyph: CGGlyph, font: CTFont) -> GlyphAtlasSlot? {
        let key = makeKey(glyph: glyph, font: font)
        if let s = cache[key] {
            hitCount += 1
            return s
        }
        missCount += 1
        return rasterizeAndInsert(glyph: glyph, font: font, key: key)
    }

    // ── Rasterization ─────────────────────────────────────────────────────

    private func rasterizeAndInsert(glyph: CGGlyph, font: CTFont, key: UInt64) -> GlyphAtlasSlot? {
        var g = glyph
        var bbox = CGRect.zero
        CTFontGetBoundingRectsForGlyphs(font, .default, &g, &bbox, 1)

        let w = Int(ceil(bbox.width))  + padding * 2
        let h = Int(ceil(bbox.height)) + padding * 2
        guard w > 0 && h > 0 else { return nil }

        guard let slot = allocate(w: w, h: h) else { return nil }

        // Rasterize using CoreGraphics into a CPU buffer
        var bytes = [UInt8](repeating: 0, count: w * h)
        let colorSpace = CGColorSpaceCreateDeviceGray()
        guard let ctx = CGContext(
            data: &bytes,
            width: w, height: h,
            bitsPerComponent: 8, bytesPerRow: w,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.none.rawValue
        ) else { return nil }

        ctx.setFillColor(gray: 1, alpha: 1)
        var origin = CGPoint(x: CGFloat(padding) - bbox.origin.x,
                             y: CGFloat(padding) - bbox.origin.y)
        CTFontDrawGlyphs(font, &g, &origin, 1, ctx)

        // Upload to atlas texture (MTLTexture.replace — direct CPU write, UMA)
        texture.replace(
            region: MTLRegionMake2D(slot.atlasX, slot.atlasY, w, h),
            mipmapLevel: 0,
            withBytes: bytes,
            bytesPerRow: w
        )

        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(font, .default, &g, &adv, 1)

        let atlasSlot = GlyphAtlasSlot(
            atlasX:  slot.atlasX + padding,
            atlasY:  slot.atlasY + padding,
            width:   w - padding * 2,
            height:  h - padding * 2,
            offsetX: bbox.origin.x,
            offsetY: bbox.origin.y,
            advance: adv.width
        )
        cache[key] = atlasSlot
        return atlasSlot
    }

    // ── Shelf packing ─────────────────────────────────────────────────────

    private func allocate(w: Int, h: Int) -> (atlasX: Int, atlasY: Int)? {
        guard w <= size && h <= size else { return nil }
        for i in 0..<shelves.indices.count {
            var shelf = shelves[i]
            if shelf.height >= h && shelf.nextX + w <= size {
                let x = shelf.nextX
                shelf.nextX += w
                shelves[i] = shelf
                return (x, shelf.y)
            }
        }
        if nextShelfY + h > size {
            // Full: clear and rebuild (simplified; production would use free-list).
            // AUDIT NOTE (P9): the correct fix is a multi-texture (paged) atlas;
            // that requires new infrastructure across the renderer (bindless
            // argument buffer + shader UV/texture indexing) that cannot be
            // verified without compiling Metal, so it is DEFERRED.
            // TODO(P9): reemplazar este clear-and-rebuild por "abrir una pagina
            // nueva y seguir" (sin tocar el cache existente). Plan detallado y
            // cambios necesarios en el renderer: docs/atlas-paging-plan.md.
            // Mientras tanto registramos el reset para que el harness descarte
            // los runs en los que ocurre un wipe de cache a mitad de medicion.
            resetCount += 1
            shelves = []; nextShelfY = 0; cache = [:]
            return allocate(w: w, h: h)
        }
        let y = nextShelfY
        shelves.append((nextX: w, y: y, height: h))
        nextShelfY += h
        return (0, y)
    }

    // ── Key ───────────────────────────────────────────────────────────────

    private func makeKey(glyph: CGGlyph, font: CTFont) -> UInt64 {
        let fontID = UInt64(UInt(bitPattern: ObjectIdentifier(font)))
        return (UInt64(glyph) << 32) | (fontID & 0xFFFFFFFF)
    }
}
