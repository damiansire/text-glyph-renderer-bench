# PoC Designs by Stack

---

## Web Sandboxed (PoC 1A–1D)

### PoC 1A — Web DOM (Electron) — *Baseline*

- **Role:** Establishes the performance floor. Measures Blink reflow/repaint cost.
- **Stack:** Electron 30+ · Node.js Buffer (`fs.readFile` chunked) · V8 JIT
- **Pipeline:**
  1. Load 100 MB in 64 KB chunks → `Buffer` → decode UTF-8.
  2. `<div contenteditable>` with manual virtualization (IntersectionObserver).
  3. Render ~50 lines in DOM; recycle nodes on scroll (recycling pool).
- **Expected limits:** Layout invalidation on every mutation; GC pressure on JS strings; no direct VRAM access.

### PoC 1B — Canvas 2D API

- **Role:** Tests whether avoiding the DOM removes the layout bottleneck.
- **Stack:** HTML Canvas · `OffscreenCanvas` in Worker · `measureText()` / `fillText()`
- **Pipeline:**
  1. `OffscreenCanvas` in Worker → transfer `ImageBitmap` to main thread each frame.
  2. Virtualization: shape + draw only visible lines in the Worker.
- **Key metric:** Worker→Main latency vs. direct DOM.

### PoC 1C — WebGPU + Dynamic Texture Atlas

- **Stack:** WebGPU API · WGSL shaders · `GPUBuffer` for vertex data
- **Atlas design:**
  - Fixed size: `GPUTexture` RGBA8 2048×2048 (8 MB VRAM).
  - Shelf packing (Next-Fit Decreasing Height) for glyph insertion.
  - LRU eviction with `lastUsedFrame` per slot.
  - Glyph rasterization: CPU rasterizes via offscreen Canvas 2D → `writeTexture()`.
- **GPU pipeline:**
  ```
  Vertex Buffer: [x, y, u, v, color_idx]  (instanced quads)
  Index Buffer:  quad triangles (6 indices × N glyphs)
  Bind Group 0:  atlas_texture, sampler, uniform_buffer (viewport)
  Bind Group 1:  color_palette (storage buffer)
  ```
- **Critical note (UMA vs. sandboxing):** See [macos-optimizations.md](macos-optimizations.md).

### PoC 1D — WebGPU + MSDF (Multi-channel Signed Distance Fields)

- **Role:** Evaluate whether MSDF scale-independence justifies generation cost.
- **Offline process:** `msdfgen` (C++) generates 64×64 px MSDF per glyph, packed into a 2048×2048 3-channel atlas (RGB32F → compressible to BC6H).
- **MSDF shader:**
  ```wgsl
  fn median(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
  }
  
  @fragment
  fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let msd = textureSample(msdf_atlas, s, in.uv).rgb;
    let sd  = median(msd.r, msd.g, msd.b);
    let w   = fwidth(sd);
    let a   = smoothstep(0.5 - w, 0.5 + w, sd);
    return vec4<f32>(in.color.rgb, in.color.a * a);
  }
  ```
- **vs. 1C:** One atlas serves all sizes → zero re-rasterization on zoom. **Drawback:** Artifacts on complex CJK glyphs.

---

## Native macOS — Metal 3 (PoC 2A–2B)

### PoC 2A — TextKit 2 (NSTextView) — *High-level reference*

- **Role:** Establishes the native "comfort ceiling"; any manual PoC should beat it.
- **Stack:** AppKit · TextKit 2 (`NSTextLayoutManager`) · Core Animation
- **Configurations to measure:**
  - `allowsNonContiguousLayout = true` for lazy rendering.
  - `NSTextView` with `CAMetalLayer` backing vs. normal.
- **Instrumentation:** `CADisplayLink` with presentation timestamp.

### PoC 2B — Metal 3 + CoreText + Argument Buffers (Bindless)

Highest complexity and highest potential native performance.

**Subsystem 1: File loading (mmap + UMA)**

```
1. open(path) → fd
2. fstat(fd) → size (100 MB)
3. mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0) → base_ptr
   (MAP_POPULATE is Linux-only; on macOS pre-fault via MADV_WILLNEED below.)
4. madvise(base_ptr, size, MADV_SEQUENTIAL | MADV_WILLNEED)
5. Register base_ptr as MTLBuffer with device.makeBuffer(bytesNoCopy:...)
   → Metal driver reuses the same physical pages without copying.
```

> **UMA key (and its limit):** On Apple Silicon, CPU and GPU share the same
> physical DRAM. `makeBuffer(bytesNoCopy:)` creates a buffer descriptor pointing
> to the already-mapped pages, so the GPU can read the **raw text bytes** without
> a `memcpy`. This is **not** "zero-copy end-to-end rendering": the GPU still
> renders glyphs from a CPU-rasterized atlas and a CPU-built vertex buffer; the
> `bytesNoCopy` mmap is only the source of the *bytes*, not of the *pixels*.
> **Status:** this subsystem is a design target — no PoC in this repo implements
> the `bytesNoCopy` path today.

**Subsystem 2: SIMD newline scan (Accelerate/NEON)**

```swift
func buildLineIndex(buffer: UnsafeRawPointer, length: Int) -> [Int] {
    // Compare 16 bytes vs. vector 0x0A0A...0A with NEON vceqq_u8
    // then vmaxvq_u8 to detect hit, extract positions with vst1q_u8
}
```

Result: a `[Int]` (or `uint32` array for offsets ≤4GB) with every `\n` position. Built **once** after load, queried with O(log N) binary search.

**Subsystem 3: Argument Buffers (Bindless Rendering)**

```swift
// Tier 2 Argument Buffer (requires Apple Silicon)
struct GlyphDrawArguments {
    var atlasTexture: MTLResourceID
    var sampler: MTLResourceID
    var glyphUVBuffer: MTLResourceID
    var transformBuffer: MTLResourceID
}

encoder.setVertexBuffer(argumentBuffer, offset: 0, index: 0)
encoder.useResources([atlasTexture, uvBuffer, transformBuffer], usage: .read)
encoder.drawIndexedPrimitives(...)
// → 0 per-glyph binding overhead
```

**Subsystem 4: Shaping + Fallback + GCD**

```swift
func shapeViewport(firstLine: Int, lastLine: Int) {
    let margin = 50
    let range = max(0, firstLine - margin)...(lastLine + margin)
    for lineIdx in range {
        let run = shapingCache.get(lineIdx) ?? {
            let shaped = CoreText.shape(line: lineIdx, font: primaryFont)
            shapingCache.insert(lineIdx, shaped)
            return shaped
        }()
        for glyph in run.glyphs where glyph.id == .notdef {
            enqueueAsyncFallback(codepoint: glyph.codepoint)
        }
    }
}

let utilityQueue = DispatchQueue(label: "text.fallback", qos: .utility, attributes: .concurrent)
func enqueueAsyncFallback(codepoint: Unicode.Scalar) {
    utilityQueue.async {
        let fallbackFont = CTFontCreateForString(...)
        let shaped = CoreText.shape(codepoint: codepoint, font: fallbackFont)
        DispatchQueue.main.async {
            atlas.insert(glyph: shaped)
            setNeedsDisplay()
        }
    }
}
```

---

## Systems — Rust (PoC 3A–3B)

### PoC 3A — Rust + wgpu + HarfBuzz

- **wgpu** is a safe abstraction over Metal on macOS. In release builds it compiles directly to Metal with negligible overhead (≤2% vs. native Metal per wgpu-core benchmarks).
- **HarfBuzz** via `harfbuzz-sys`: full shaping with OpenType ligatures, GPOS, GSUB.
- **Mmap in Rust:**
  ```rust
  let file = File::open(path)?;
  let mmap = unsafe { MmapOptions::new().populate().map(&file)? };
  // mmap: Deref<Target=[u8]> — zero-copy slice
  ```
- **Pipeline:**
  1. `mmap` → SIMD `\n` scan via `std::arch::aarch64` / `memchr` crate.
  2. HarfBuzz shape → `GlyphBuffer` → vertex data.
  3. `wgpu::Buffer` (mapped at creation) → upload vertex data.
  4. Render pass with texture atlas identical to PoC 1C but on native Metal.

### PoC 3B — Rust + Vello

- **Vello** is a GPU-first vector renderer based on Compute Shaders (not fragment shaders).
- **Philosophy:** Instead of rasterizing glyphs to a texture atlas, it **evaluates Bézier curves directly on the GPU** using the Euler Spiral / flatten→strip algorithm from Linebender.
- **Caveats:** Vello is maturing (2024–2025). Requires `wgpu` with `MAPPABLE_PRIMARY_BUFFERS`. Per-glyph throughput may be lower than atlas in dense text areas, but has no VRAM atlas limit and scales infinitely without artifacts.
