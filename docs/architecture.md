# Architecture — Text Glyph Renderer Bench
### macOS / Apple Silicon — Technical Reference

> **Target platform:** macOS 15+ · Apple Silicon (M3/M4) · ProMotion 120 Hz  
> **Frame budget:** 8.3 ms at 120 Hz · Zero-copy UMA · Memory-Mapped I/O

---

## Documents

| File | Contents |
|------|----------|
| [abstractions.md](abstractions.md) | Common interfaces: `TextBuffer`, `ShapingEngine`, `Renderer` |
| [poc-designs.md](poc-designs.md) | Design details for each PoC (Web, Native macOS, Rust) |
| [data-structures.md](data-structures.md) | Duel A — Rope vs. Piece Table |
| [rasterization.md](rasterization.md) | Duel B — Texture Atlas vs. Compute Shaders |
| [macos-optimizations.md](macos-optimizations.md) | UMA, SIMD scan, frame budget, LRU atlas, WebGPU vs. Metal 3 |
| [benchmarking.md](benchmarking.md) | Metrics, instrumentation, performance predictions, decision table |

---

## Monorepo Structure

```
text-glyph-renderer-bench/
├── README.md
├── docs/
│   ├── architecture.md          ← This file (index)
│   ├── abstractions.md
│   ├── poc-designs.md
│   ├── data-structures.md
│   ├── rasterization.md
│   ├── macos-optimizations.md
│   ├── benchmarking.md
│   └── results/
│       └── .gitkeep
│
├── shared/
│   ├── test-data/
│   │   ├── generate_testfile.py
│   │   └── test_100mb.txt        ← (gitignored)
│   ├── metrics/
│   │   ├── frame_stats.schema.json
│   │   └── benchmark_runner.py
│   └── fonts/
│       ├── InterVariable.ttf
│       └── NotoSansCJK-Regular.ttf
│
├── poc-1a-web-dom/
├── poc-1b-canvas2d/
├── poc-1c-webgpu-atlas/
├── poc-1d-webgpu-msdf/
├── poc-2a-textkit2/
├── poc-2b-metal3-coretext/
├── poc-3a-rust-wgpu/
├── poc-3b-rust-vello/
└── Cargo.toml                   ← Workspace root (poc-3a + poc-3b)
```
