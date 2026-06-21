# Benchmarking, Metrics & Performance Predictions

---

## Instrumentation

### Key-to-Pixel Latency

```
t0: NSEvent / KeyDown timestamp (mach_absolute_time)
t1: TextBuffer.insert() complete
t2: GlyphRun shaped and in vertex buffer
t3: Metal encoder commit
t4: CAMetalLayer.nextDrawable presented (via CADisplayLink)

K2P Latency = t4 - t0
Target: < 8.3ms (1 frame at 120Hz)
```

Each PoC must expose an `EventTimingProbe` that captures these timestamps and exports them to JSON per keypress for post-benchmark analysis.

### Synthetic Deterministic Scroll

```
Autoscroll benchmark:
  Duration:  30 seconds
  Speed:     60 px / frame (at 120 Hz = 7200 px/s)
  File:      test_100mb.txt (known content)

Measurements:
  - Dropped Frames: frames that didn't reach vblank
  - Frame Time Distribution: histogram with P50/P95/P99 percentiles
  - Atlas Hit Rate: glyphs served from cache vs. re-rasterized
  - Peak VRAM: via MTLHeap or os_proc_available_memory()
```

**Deterministic file generator:** use the canonical script, not an inline
snippet. The real generator builds a *mixed* corpus (English/Spanish prose,
Unicode/emoji/CJK/RTL, source-code fragments, long and empty lines) with a
fixed seed so the file is byte-for-byte reproducible:

```bash
python3 shared/test-data/generate_testfile.py            # → test_100mb.txt (100 MB)
python3 shared/test-data/generate_testfile.py --size 10  # → test_10mb.txt  (10 MB)
python3 shared/test-data/generate_testfile.py --verify    # check SHA-256 of the canonical corpus
```

(An earlier inline `random.choices(string.printable, …)` one-liner was wrong:
`string.printable` includes `\n`/`\r`, so it injects line breaks mid-"line" and
the 1.3M-line count does not hold.)

### Instruments.app Integration

Native PoCs (2A, 2B):
```swift
import os.signpost
let log = OSLog(subsystem: "com.poc.textengine", category: .pointsOfInterest)

os_signpost(.begin, log: log, name: "ShapeViewport")
// ... shaping code ...
os_signpost(.end, log: log, name: "ShapeViewport")
```

Rust PoCs (3A, 3B):
```bash
xcrun xctrace record --template "Metal System Trace" \
  --launch -- ./target/release/poc3a --benchmark
```

---

## Performance Predictions (M3/M4)

### Expected Ranking (lower latency = better)

```
PREDICTION: Key-to-Pixel Latency on 100MB text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Rank │ PoC                          │ Est. K2P     │ Rationale
─────┼──────────────────────────────┼──────────────┼───────────────────────────
 🥇  │ 2B: Metal 3 + CoreText       │  1.5 – 3.0ms │ Zero-copy UMA, Arg Buffers
─────┼──────────────────────────────┼──────────────┼───────────────────────────
 🥈  │ 3A: Rust + wgpu + HarfBuzz   │  2.0 – 4.0ms │ Native Metal via wgpu
─────┼──────────────────────────────┼──────────────┼───────────────────────────
 🥉  │ 3B: Rust + Vello             │  3.0 – 6.0ms │ Pure GPU Compute
─────┼──────────────────────────────┼──────────────┼───────────────────────────
  4  │ 2A: TextKit 2                │  3.0 – 6.0ms │ Layout overhead on scroll
─────┼──────────────────────────────┼──────────────┼───────────────────────────
  5  │ 1D: WebGPU + MSDF            │  4.0 – 8.0ms │ IPC overhead + MSDF shader
─────┼──────────────────────────────┼──────────────┼───────────────────────────
  6  │ 1C: WebGPU + Atlas           │  5.0 –10.0ms │ Misses = extra IPC copies
─────┼──────────────────────────────┼──────────────┼───────────────────────────
  7  │ 1B: Canvas 2D                │ 10.0 –20.0ms │ measureText() imprecise
─────┼──────────────────────────────┼──────────────┼───────────────────────────
  8  │ 1A: Web DOM (Electron)       │ 20.0 –60.0ms │ DOM reflow + layout
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### Why PoC 2B Wins

**Metal 3 + CoreText + Piece Table + MSDF Texture Atlas:**

1. **Piece Table over mmap:** With 0 mutations (scroll only), the table has 1 piece → O(1) access.
2. **`makeBuffer(bytesNoCopy:)` (design target, not yet implemented):** the GPU
   *text* buffer would alias the mmap pages — zero copies for the **raw bytes**.
   This is not the rendered output: glyphs are still drawn from a CPU-rasterized
   atlas + vertex buffer. No PoC in this repo implements `bytesNoCopy` today.
3. **Argument Buffers Tier 2:** 1 binding call per frame. Saves 2–5 ms CPU time in dense text (>500 glyphs/frame).
4. **Lazy shaping with GCD:** Main thread only shapes viewport ± margin. Font fallbacks never block the `CADisplayLink` callback.
5. **MSDF Atlas:** Glyphs valid at any scale, no re-rasterization on zoom.

> **Why M3/M4 amplifies the advantage:** UMA zero-copy is **exclusive to Apple Silicon**. On a PC with dedicated VRAM, both Metal and wgpu would need to copy. Only on Apple Silicon does `bytesNoCopy` + mmap achieve truly zero-copy — the bandwidth advantage isn't visible in naive benchmarks until there are thousands of atlas misses per frame.

---

## Appendix: Quick-Decision Table

| Question | Answer |
|----------|--------|
| Data structure for 100MB read-only + scroll? | Piece Table over mmap (1 piece, O(1) access) |
| Data structure for collaborative editor with CRDT? | Rope with nodes storing `\n` count |
| Atlas or Compute for <500 unique glyphs? | Atlas (rasterized once, constant sample cost) |
| Atlas or Compute for variable fonts / >1000 unique glyphs? | Compute Shaders (Vello) |
| MSDF or dynamic atlas? | MSDF if no subpixel hinting; dynamic atlas if small-size hinting is needed |
| Can WebGPU match Metal on Apple Silicon? | No for bulk uploads. IPC imposes copies that native UMA avoids |
| HarfBuzz or CoreText for shaping? | HarfBuzz for full OpenType portability; CoreText for 100% Apple ecosystem |
| GCD or async/await Swift for font fallback? | GCD with explicit `.utility` QoS — more predictable scheduler prioritization |
