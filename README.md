# Text Glyph Renderer Bench

Comparative monorepo of high-performance text rendering engines on macOS / Apple Silicon.

## Goal

Empirically measure the performance ceiling of different stacks for massive text rendering (100 MB file, 120 Hz, 8.3 ms frame budget).

## Architecture

See [`docs/architecture.md`](docs/architecture.md) for the full design.

## Included PoCs

| Folder | Stack | Category | Status |
|--------|-------|----------|--------|
| `poc-1a-web-dom` | Electron + DOM | Web Sandboxed | — |
| `poc-1b-canvas2d` | Canvas 2D + OffscreenCanvas | Web Sandboxed | — |
| `poc-1c-webgpu-atlas` | WebGPU + Texture Atlas | Web Sandboxed | — |
| `poc-1d-webgpu-msdf` | WebGPU + MSDF | Web Sandboxed | **not implemented** — missing `index.html` + MSDF charset; never renders nor emits stats. Skipped by the benchmark runner. |
| `poc-2a-textkit2` | TextKit 2 (NSTextView) | Native macOS | — |
| `poc-2b-metal3-coretext` | Metal 3 + CoreText + Arg Buffers | Native macOS | — |
| `poc-3a-rust-wgpu` | Rust + wgpu + HarfBuzz | Systems | buffer/line-index only (no GPU render yet) |
| `poc-3b-rust-vello` | Rust + Vello | Systems | **render NOT implemented** — `build_scene` returns an empty `Scene` (no geometry) |

## Initial Setup

### 1. Generate the test file (100 MB, deterministic)

```bash
cd shared/test-data
python3 generate_testfile.py
# → generates test_100mb.txt (~100 MB, seed=42)
```

### 2. Build Rust workspace

```bash
# From the monorepo root
cargo build --release -p poc-3a-rust-wgpu
cargo build --release -p poc-3b-rust-vello
```

### 3. Web PoCs (Node.js / Electron)

```bash
# PoC 1A — Web DOM
cd poc-1a-web-dom && npm install && npm start

# Run synthetic scroll benchmark:
npm run benchmark
```

## System Requirements

- macOS 14+ (Sonoma) · Apple Silicon (M1 or higher)
- Xcode 15+ (for PoCs 2A, 2B)
- Rust 1.78+ (`rustup update`)
- Node.js 20+ · npm 10+
- Python 3.11+

## Metrics Structure

Each implemented PoC writes an **aggregated** `results/<poc-id>_stats.json`
report — one object per run, not one record per frame — that conforms to the
`BenchmarkReport` schema in `shared/metrics/frame_stats.schema.json`. The schema
requires a `poc_id` from a closed enum of the 8 stacks and validates the
`benchmark` block (percentiles as numbers, in ms).

`shared/metrics/benchmark_runner.py` validates every `*_stats.json` against that
schema as it aggregates them: a non-conforming report is a hard failure, not a
silent pass. All PoCs write into a single canonical directory (`results/`),
which the runner passes to each via the `BENCH_RESULTS_DIR` environment variable.

The Rust PoCs (3A/3B) are microbenchmarks that deliberately do **not** report a
frame-budget verdict (they measure line-index traversal / CPU scene build only);
their rows show `n/a` for the drop rate so the table never implies a comparison
they did not measure.
