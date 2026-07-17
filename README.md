# Text Glyph Renderer Bench

Comparative monorepo of high-performance text rendering engines on macOS / Apple Silicon.

## Goal

Empirically measure the performance ceiling of different stacks for massive text rendering (100 MB file, 120 Hz, 8.3 ms frame budget).

## Architecture

See [`docs/architecture.md`](docs/architecture.md) for the full design.

## Included PoCs

The stacks split into two groups: those that **emit stats** today (a
schema-conforming `results/<poc-id>_stats.json`) and those that do **not yet**.
Don't read the second group as part of any comparison.

### Measured stacks (emit stats)

| Folder | Stack | Category | What it measures |
|--------|-------|----------|------------------|
| `poc-1a-web-dom` | Electron + DOM | Web Sandboxed | End-to-end scroll frames vs the 8.33 ms budget. A Node-only baseline path (`benchmarks/synthetic_scroll.js`, no DOM) also emits stats headlessly and is smoke-tested in CI. |
| `poc-1b-canvas2d` | Canvas 2D + OffscreenCanvas | Web Sandboxed | End-to-end scroll frames. Emits stats only inside the Electron GUI (`electron . --benchmark`). |
| `poc-1c-webgpu-atlas` | WebGPU + Texture Atlas | Web Sandboxed | End-to-end scroll frames. Emits stats only inside the Electron GUI + a WebGPU adapter. |
| `poc-2a-textkit2` | TextKit 2 (NSTextView) | Native macOS | End-to-end scroll frames. macOS-only (Swift + TextKit 2). |
| `poc-2b-metal3-coretext` | Metal 3 + CoreText + Arg Buffers | Native macOS | End-to-end scroll frames. macOS-only (Swift + Metal 3). |
| `poc-3a-rust-wgpu` | Rust + wgpu + HarfBuzz | Systems | **Real offscreen GPU render per frame** (shape → rasterize → atlas upload → single instanced draw → GPU submit) vs the 8.33 ms budget, so it *does* report a drop rate. On a GPU-less runner it writes a `gpu_available: false` report instead of fabricating timings. |
| `poc-3b-rust-vello` | Rust + Vello | Systems | **CPU-side scene-build microbenchmark** only — encodes real glyph geometry via `Scene::draw_glyphs`, but performs no on-screen GPU render, so it deliberately reports **no** frame-budget verdict (`n/a` drop rate). |

### Not yet implemented (emit no stats)

| Folder | Stack | Category | Status |
|--------|-------|----------|--------|
| `poc-1d-webgpu-msdf` | WebGPU + MSDF | Web Sandboxed | **Not implemented.** The shaders, `index.html` and loader exist, but the MSDF atlas (`assets/*.png` / `*.bin`) is generated offline and gitignored, so it never renders nor emits stats. The benchmark runner skips it explicitly. **Not counted** in the measured stacks. |

## Initial Setup

### 1. Generate the test file (100 MB, deterministic)

```bash
cd shared/test-data
python3 generate_testfile.py
# → generates test_100mb.txt (~100 MB, seed=42)
```

### 2. Get a test font

`shared/fonts/*.ttf` is gitignored on purpose (fonts aren't ours to redistribute) but
was never documented — PoC 3B defaults to `shared/fonts/InterVariable.ttf` and fails
to even start without it:

```bash
mkdir -p shared/fonts
curl -L -o shared/fonts/InterVariable.ttf \
  https://github.com/rsms/inter/raw/master/docs/font-files/InterVariable.ttf
```

(Any real `.ttf`/`.otf` works — pass a different one with `--font <path>`.)

### 3. Build Rust workspace

```bash
# From the monorepo root
cargo build --release -p poc-3a-rust-wgpu
cargo build --release -p poc-3b-rust-vello
```

### 4. Web PoCs (Node.js / Electron)

```bash
# PoC 1A — Web DOM
cd poc-1a-web-dom && npm install && npm start

# Run synthetic scroll benchmark:
npm run benchmark
```

## System Requirements

- macOS 14+ (Sonoma) · Apple Silicon (M1 or higher)
- Xcode 15+ (for PoCs 2A, 2B)
- Rust 1.92+ (`rustup update`) — MSRV impuesto por vello 0.7 / wgpu 28 (poc-3b)
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

PoC **3B** is a CPU-side microbenchmark that deliberately does **not** report a
frame-budget verdict (it measures Vello scene-build only); its row shows `n/a`
for the drop rate so the table never implies a comparison it did not measure.
PoC **3A**, by contrast, now runs a real offscreen GPU render per frame and
**does** report a drop rate against the 8.33 ms budget — except on a GPU-less
runner, where it writes a `gpu_available: false` report and no timing numbers.

## Statistical Methodology

- **Hardware & single-machine caveat**: every number published in this README
  was measured on a **single machine** (Apple Silicon, macOS 14+ — see System
  Requirements). There is no multi-device sample and no averaging across
  machines, so treat all comparisons as single-machine, single-configuration
  and not a cross-hardware ranking. The cross-platform CI (`bench.yml`) runs the
  Rust PoCs on hosted ubuntu/windows/macOS runners as a *compile-and-run sanity
  check only* — those runners often expose no GPU adapter (`gpu_available:
  false`), so their numbers are not publishable benchmark results.
- **Runs per PoC**: each implemented PoC's `benchmark_runner.py` invocation
  aggregates a full scroll pass over `test_100mb.txt` into a single
  `results/<poc-id>_stats.json` (see `frame_stats.schema.json`) — one
  aggregated report per run, not per-frame raw samples.
- **Percentiles, not just averages**: the schema's `benchmark` block reports
  frame-time percentiles (not a single mean), so a PoC that is fast on average
  but spikes under GC/allocator pressure can't hide behind the mean.
- **Warm-up**: run each PoC's benchmark script twice and discard the first run
  before recording a comparison number — the first pass pays font-parse/shape
  cache-miss cost that a real session wouldn't repeat (see the cold/warm rule
  for Rust benches in `CLAUDE.md`).
- **Thermal throttling**: on Apple Silicon, a benchmark run right after a
  build/compile can read slower purely from thermal state, not from the code
  under test — let the machine idle a minute after `cargo build --release`
  before recording a number you intend to publish in this README.
- **What's NOT yet true**: none of the above (warm-up discipline, thermal
  cooldown) is enforced automatically by `benchmark_runner.py` today — it's a
  manual protocol for whoever runs the benchmark and publishes a number here,
  not a guarantee the JSON reports already encode.
