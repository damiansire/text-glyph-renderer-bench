# Duel B — Rasterization: Texture Atlas vs. Compute Shaders

---

## Texture Atlas (PoC 1C, 2B, 3A)

```
GPU Frame Pipeline:
  CPU: rasterize missing glyphs → writeTexture() → atlas 2048×2048
  CPU: build vertex buffer (x,y,u,v per glyph)
  GPU: 1 instanced draw call → sample atlas → output pixel

VRAM: 2048×2048×4 bytes = 16 MB (RGBA8) or 8 MB (R8 grayscale)
Miss latency: CPU rasterization + UMA copy (on Apple Silicon)
```

**Bottlenecks:**
- **Cache thrashing:** 2048² atlas holds ≈1000–4000 glyphs depending on size. Highly varied text (many fonts/sizes) causes frequent evictions → re-rasterization.
- **Subpixel rendering:** Requires storing separate versions per fractional position (×3 VRAM).
- **Strength:** A single indirect draw call serves N glyphs → extremely efficient when the atlas is warm.

---

## Compute Shaders / GPU-Side Rasterization (PoC 3B — Vello)

```
GPU Frame Pipeline:
  CPU: send glyph Bézier curves (from font) to GPU buffer
  GPU Compute 1: flatten curves → line segments (Euler Spiral approx.)
  GPU Compute 2: tile-based scan convert → coverage mask
  GPU Fragment: composite with color

VRAM: only curve buffers (<<1 MB for visible text)
Latency: fully on GPU, no CPU rasterization stall
```

**Advantages:**
- **No atlas limit:** Each glyph is evaluated live → infinite font/size variety.
- **Resolution-independent:** No pre-rasterized pixels → perfect Retina 3x quality.
- **Ideal for static scroll text:** curves are sent once, Compute re-evaluates only visible pixels.

**Disadvantages:**
- Higher per-glyph cost: flatten+rasterize pipeline is more expensive than a simple `textureSample()`.
- For very dense text (thousands of identical glyphs on screen), atlas is unbeatable.

---

## Verdict

The crossover point is around **~800 unique visible glyphs**:
- Below: **Atlas wins** (sampling is cheaper than evaluating curves).
- Above / with font variety: **Compute wins** (atlas fragments, evictions destroy throughput).

For a standard text editor with 1–3 fixed fonts: **MSDF Atlas** (PoC 1D / 2B) is the correct architecture. For a document renderer with variable fonts: **Compute** (Vello).
