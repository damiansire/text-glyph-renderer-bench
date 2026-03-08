# macOS-Specific Optimizations & WebGPU vs. Metal 3

---

## Memory-Mapped Files + Zero-Copy UMA

```
NVMe Disk ──DMA──► DRAM Pages ──UMA──► GPU L2 Cache
                       ↑                    ↑
                 mmap base_ptr       MTLBuffer (bytesNoCopy)
                 (CPU reads)         (GPU reads)

⚡ ZERO copies between CPU and GPU. Same physical pages are
   accessible from both processors via Unified Memory.
```

**Optimal mmap configuration:**
```c
// MAP_PRIVATE: copy-on-write → mutations don't affect the file
// MAP_POPULATE: pre-faulting → avoids page faults during render
// MADV_SEQUENTIAL: TLB prefetcher optimized for linear reads
void* ptr = mmap(NULL, size, PROT_READ,
                 MAP_PRIVATE | MAP_POPULATE, fd, 0);
madvise(ptr, size, MADV_SEQUENTIAL | MADV_WILLNEED);
```

---

## SIMD Newline Scan (Accelerate / NEON ARMv8.2)

Scanning `\n` positions over 100 MB is the most expensive operation on initial load. NEON processes **16 bytes per instruction**:

```
Naive (scalar):   100MB / 1 byte/cycle   = ~100M cycles
NEON (vectorized): 100MB / 16 bytes/cycle = ~6.25M cycles  (16× speedup)
```

In Rust, the `memchr` crate uses NEON automatically on aarch64 and is the reference for this scan.

---

## Frame Budget (120 Hz = 8.33 ms)

```
Per-frame budget:
┌─────────────────────────────────────────────────────────┐
│  CPU Input Processing        ~0.5 ms                    │
│  Line index lookup (binary)  ~0.1 ms                    │
│  Lazy shaping (viewport±50)  ~1.5 ms  (warm cache)      │
│  Atlas update (miss)         ~0.5 ms  (typical)         │
│  Vertex buffer build         ~0.8 ms                    │
│  Metal encoding              ~0.3 ms  (Arg Buffers)     │
│  GPU render (fragment)       ~2.0 ms                    │
│  Display compositor          ~1.5 ms                    │
│  SAFETY MARGIN               ~1.13 ms                   │
└─────────────────────────────────────────────────────────┘
Total: 8.33 ms ✓
```

---

## Texture Atlas LRU Cache

```
Atlas LRU Eviction (2048×2048):
  Capacity:   ~4096 glyphs at 32×32px (RGBA8)
  Structure:  HashMap<GlyphKey, AtlasSlot> + DoublyLinkedList<AtlasSlot>
  GlyphKey:   (font_id: u32, glyph_id: u16, size_px: u8, subpixel: u2)

  On new glyph insert:
  1. Free slot available? → use it (shelf packing).
  2. No free slot → evict LRU (tail of linked list).
  3. Rasterize → writeTexture() to assigned slot.
  4. Move slot to head.

  On glyph use:
  1. HashMap lookup → O(1)
  2. Move to head → O(1)
```

---

## WebGPU vs. Metal 3 — Sandboxing Cost

WebGPU on macOS (Chrome/Safari) runs in an isolated **GPU Process** via `XPC`, separate from the renderer and main browser processes:

```
Web Application
    │
    ▼ (IPC / SharedMemory)
Browser Renderer Process
    │
    ▼ (IPC / MachMessage / XPC)
GPU Process (dawn_wire / wgpu-native)
    │
    ▼ (Metal API calls)
Metal Driver (kernel)
    │
    ▼
GPU Hardware
```

**Hidden copies identified:**

| Operation | Native Metal 3 | WebGPU (Chrome Dawn) |
|-----------|---------------|---------------------|
| `writeBuffer()` with CPU data | `memcpy` to shared MTLBuffer (UMA, 0 extra overhead) | SharedMemory IPC → staging buffer → GPU buffer (1–2 copies) |
| `writeTexture()` (atlas update) | `memcpy` to MTLTexture (UMA, direct pointer) | Serialize image → IPC → deserialize → MTLTexture (2+ copies) |
| `mapAsync()` readback | MTLBuffer.contents() (0 copies, UMA) | IPC roundtrip + copy to ArrayBuffer |
| mmap as GPUBuffer | `makeBuffer(bytesNoCopy:)` → 0 copies | **IMPOSSIBLE:** WebGPU spec has no `bytesNoCopy`. Always copies. |

### The Fundamental mmap Problem

Metal 3's biggest advantage on Apple Silicon — `makeBuffer(bytesNoCopy:)` over mmap pages — is **completely inaccessible from WebGPU**. `GPUBuffer` is an opaque resource owned by the GPU Process; there is no mechanism to say "this buffer IS this application process memory."

### Overhead Estimate

- Atlas upload (1 glyph miss, ~4KB): WebGPU ≈ 50–150 µs extra vs. Metal 3 ≈ 5 µs.
- Per frame with 10 glyph misses: ≈500 µs–1.5 ms overhead from IPC/copies alone in WebGPU.
