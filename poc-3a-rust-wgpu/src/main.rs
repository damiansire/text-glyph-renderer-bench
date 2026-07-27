//! main.rs — CLI test harness for PoC 3A Rust + wgpu + HarfBuzz.
//!
//! Usage:
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt --bench
//!
//! Without --bench: prints file stats and exits.
//!
//! With --bench: runs the REAL GPU render path per "frame" (tgrb-4 — this
//! replaces the earlier line-index-only microbenchmark): resolve the visible
//! line window (line-index lookups + piece-table slicing) -> shape each visible
//! line with HarfBuzz (rustybuzz) -> rasterize/upload any atlas-miss glyphs into
//! the R8Unorm GPU texture atlas (`rendering::atlas`) -> build the instanced
//! glyph-quad buffer -> encode ONE render pass with ONE draw call -> submit to
//! the GPU queue, into an offscreen (headless, no window) render target. So this
//! DOES report `dropped_frames` / `drop_rate_pct` against the 8.33 ms / 120 Hz
//! frame budget: it is now an end-to-end render frame, not a CPU-only slice.
//!
//! GPU availability: `wgpu::Instance::request_adapter` can return no adapter on
//! headless CI (e.g. GitHub Actions `ubuntu-latest` has neither Vulkan nor
//! DX12). When that happens, `--bench` prints a clear message, writes a result
//! JSON with `"gpu_available": false`, and exits 0 — it does NOT panic and does
//! NOT fabricate frame-timing numbers for GPU work that never ran.

mod rendering;

use poc_3a_rust_wgpu::{PieceTable, TextBuffer};
use rendering::atlas::GlyphAtlas;
use rendering::renderer::{
    build_glyph_instances, GlyphRenderer, GpuContext, VIEWPORT_H, VIEWPORT_W,
};

use std::path::{Path, PathBuf};
use std::time::Instant;

/// 120 Hz frame budget, in microseconds. A frame slower than this is a dropped
/// frame against a 120 Hz target.
const FRAME_BUDGET_US: u64 = 8_333;

/// Default corpus font, anchored to the crate directory instead of the process
/// working directory.
///
/// The orchestrator (`shared/metrics/benchmark_runner.py`) launches this binary
/// with `cwd` = repo root and without `--font`, so a CWD-relative default
/// resolved to `<repo>/../shared/fonts/...`, i.e. outside the repo, and the PoC
/// died reading the font on any machine that actually had a GPU adapter.
fn default_font_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../shared/fonts/InterVariable.ttf")
}

// ── CLI args ─────────────────────────────────────────────────────────────────

struct Args {
    file: PathBuf,
    font: PathBuf,
    font_size_px: f32,
    bench: bool,
    scroll_frames: u32,
    scroll_px_per_frame: f64,
    line_height_px: f64,
}

impl Args {
    fn parse() -> Self {
        let mut file = None;
        let mut font = default_font_path();
        let mut font_size_px = 13.0_f32;
        let mut bench = false;
        let mut scroll_frames: u32 = 3600; // 30 s at 120 Hz
        let mut scroll_px = 60.0_f64;
        let mut line_h = 20.0_f64;

        // Small helper so a missing or malformed value for a flag prints a
        // clear diagnostic and exits cleanly instead of panicking with a
        // backtrace (audit P5: args inválidos daban backtrace).
        fn parse_value<T: std::str::FromStr>(flag: &str, raw: Option<String>) -> T {
            let raw = raw.unwrap_or_else(|| {
                eprintln!("error: {flag} requires a value");
                std::process::exit(2);
            });
            raw.parse().unwrap_or_else(|_| {
                eprintln!("error: invalid value '{raw}' for {flag}");
                std::process::exit(2);
            })
        }

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--file" => file = args.next().map(PathBuf::from),
                "--font" => font = args.next().map(PathBuf::from).unwrap_or(font),
                "--font-size" => font_size_px = parse_value("--font-size", args.next()),
                "--bench" => bench = true,
                "--frames" => scroll_frames = parse_value("--frames", args.next()),
                "--scroll-px" => scroll_px = parse_value("--scroll-px", args.next()),
                "--line-height" => line_h = parse_value("--line-height", args.next()),
                "--help" | "-h" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                // A typo silently ignored used to run a bench the caller did
                // not configure; fail loudly instead.
                other => {
                    eprintln!("error: unknown flag '{other}'\n\n{USAGE}");
                    std::process::exit(2);
                }
            }
        }

        Self {
            // A missing --file is a usage error, not a bug: print the usage
            // and exit 2 instead of panicking with a backtrace (`--help`
            // itself used to die on this expect before printing anything).
            file: file.unwrap_or_else(|| {
                eprintln!("error: --file <path> is required\n\n{USAGE}");
                std::process::exit(2);
            }),
            font,
            font_size_px,
            bench,
            scroll_frames,
            scroll_px_per_frame: scroll_px,
            line_height_px: line_h,
        }
    }
}

const USAGE: &str = "\
poc3a — Rust + wgpu + HarfBuzz GPU render benchmark

Usage: poc3a --file <path> [--bench] [--frames <n>] [options]

Options:
  --file <path>         Corpus to load (required)
  --font <path>         Font file (default: shared/fonts/InterVariable.ttf)
  --font-size <px>      Font size in px (default: 13)
  --bench               Run the GPU render benchmark and write the report
  --frames <n>          Benchmark frames (default: 3600)
  --scroll-px <px>      Scroll advance per frame (default: 60)
  --line-height <px>    Line height in px (default: 20)
  --help, -h            Show this help";

// ── Benchmark: real GPU render per synthetic-scroll frame ──────────────────

struct GpuBenchStats {
    total_iters: u32,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    avg_glyphs_per_iter: f64,
    dropped_frames: u32,
    drop_rate_pct: f64,
    atlas_hit_rate: f64,
    atlas_evictions: u64,
    atlas_fill_ratio: f64,
}

/// Real end-to-end frame benchmark: resolves the visible line window, shapes it
/// with HarfBuzz, rasterizes/uploads atlas misses, builds the instance buffer,
/// and submits one render pass to the GPU queue, once per synthetic scroll step.
#[allow(
    clippy::cast_possible_truncation,
    clippy::too_many_arguments,
    reason = "bench scaffolding: scroll->line index is a deliberate floor, elapsed micros fit in u64; \
              the arg count mirrors the GPU objects a real frame needs (device/queue/faces/atlas/renderer/args)"
)]
fn run_gpu_render_bench(
    pt: &mut PieceTable,
    ctx: &GpuContext,
    ttf_face: &ttf_parser::Face,
    rb_face: &rustybuzz::Face,
    atlas: &mut GlyphAtlas,
    renderer: &GlyphRenderer,
    args: &Args,
) -> GpuBenchStats {
    const LEFT_MARGIN_PX: f32 = 52.0; // room for a line-number gutter, matches poc-1c.
    const TEXT_COLOR_RGBA: u32 = 0xC9D1D9FF;

    let line_height_f32 = args.line_height_px as f32;
    let viewport_h_f64 = f64::from(VIEWPORT_H);
    let visible_lines = (viewport_h_f64 / args.line_height_px) as usize + 2;

    let line_count = pt.line_count();
    let total_height_px = line_count as f64 * args.line_height_px;
    let mut scroll_y: f64 = 0.0;

    let mut iter_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut total_glyphs: u64 = 0;

    // Reusable buffers: allocated once, cleared per frame (mirrors 3B's memory
    // discipline — see its F6 note — steady-state allocates nothing proportional
    // to the file size, only to the viewport).
    let mut window_bytes: Vec<u8> = Vec::new();
    let mut window_spans: Vec<(usize, usize)> = Vec::with_capacity(visible_lines);

    for _iter in 0..args.scroll_frames {
        let t_start = Instant::now();

        let first_visible_line = (scroll_y / args.line_height_px) as usize;
        let last_visible_line =
            (first_visible_line + visible_lines).min(line_count.saturating_sub(1));

        window_bytes.clear();
        window_spans.clear();
        for line_idx in first_visible_line..=last_visible_line {
            let line_start = pt.line_start_byte(line_idx);
            let line_end = if line_idx + 1 < line_count {
                pt.line_start_byte(line_idx + 1)
            } else {
                pt.byte_len()
            };
            let span_start = window_bytes.len();
            pt.slice_pieces(line_start..line_end, |slice, _offset| {
                window_bytes.extend_from_slice(slice);
                true
            });
            window_spans.push((span_start, window_bytes.len()));
        }
        // Decode lossily (matches 3B's `build_scene`): invalid UTF-8 is replaced
        // (U+FFFD) rather than silently dropping the line.
        let lines_owned: Vec<String> = window_spans
            .iter()
            .map(|&(s, e)| String::from_utf8_lossy(&window_bytes[s..e]).into_owned())
            .collect();
        let lines: Vec<&str> = lines_owned.iter().map(String::as_str).collect();

        // ── Shape + rasterize/atlas-upload + build instances + GPU submit ──
        let instances = build_glyph_instances(
            &ctx.device,
            &ctx.queue,
            ttf_face,
            rb_face,
            atlas,
            &lines,
            line_height_f32,
            args.font_size_px,
            LEFT_MARGIN_PX,
            TEXT_COLOR_RGBA,
        );
        total_glyphs += instances.len() as u64;
        renderer.render_frame(&ctx.device, &ctx.queue, &instances);

        scroll_y += args.scroll_px_per_frame;
        if scroll_y + viewport_h_f64 > total_height_px {
            scroll_y = 0.0; // wrap-around for the benchmark
        }

        let elapsed_us = t_start.elapsed().as_micros() as u64;
        iter_times_us.push(elapsed_us);
    }

    // ── Percentiles + frame-budget verdict ─────────────────────────────────
    // Guard the empty case (`--frames 0`): see original P5 fix, same reasoning.
    iter_times_us.sort_unstable();
    let n = iter_times_us.len();
    let pct = |p: usize| {
        if n == 0 {
            0
        } else {
            iter_times_us[(n * p / 100).min(n - 1)]
        }
    };
    let dropped_frames = iter_times_us
        .iter()
        .filter(|&&t| t > FRAME_BUDGET_US)
        .count() as u32;
    let drop_rate_pct = if args.scroll_frames == 0 {
        0.0
    } else {
        f64::from(dropped_frames) / f64::from(args.scroll_frames) * 100.0
    };
    let avg_glyphs_per_iter = if args.scroll_frames == 0 {
        0.0
    } else {
        total_glyphs as f64 / f64::from(args.scroll_frames)
    };

    let atlas_stats = atlas.stats();
    GpuBenchStats {
        total_iters: args.scroll_frames,
        p50_us: pct(50),
        p95_us: pct(95),
        p99_us: pct(99),
        avg_glyphs_per_iter,
        dropped_frames,
        drop_rate_pct,
        atlas_hit_rate: atlas_stats.hit_rate(),
        atlas_evictions: atlas_stats.evictions,
        atlas_fill_ratio: atlas.fill_ratio(),
    }
}

// ── main ───────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("PoC 3A — Rust + wgpu + HarfBuzz");
    println!("File: {}", args.file.display());

    // ── Load file via mmap ─────────────────────────────────────────────
    let t_load = Instant::now();
    let mut pt = PieceTable::from_file(&args.file)?;
    let load_ms = t_load.elapsed().as_millis();

    println!(
        "Loaded {} MB in {}ms  ({} lines, {} bytes)",
        pt.byte_len() / 1_048_576,
        load_ms,
        pt.line_count(),
        pt.byte_len(),
    );

    if !args.bench {
        // Just print stats and exit. Use `saturating_sub(1)` so an empty file
        // (line_count/byte_len == 0) does not underflow `usize` (audit P5).
        println!("\nLine 0   start byte: {}", pt.line_start_byte(0));
        println!(
            "Line 100 start byte: {}",
            pt.line_start_byte(100.min(pt.line_count().saturating_sub(1)))
        );
        println!(
            "Byte 4096 → line {}",
            pt.byte_to_line(4096.min(pt.byte_len().saturating_sub(1)))
        );
        println!("\nDone. Run with --bench for the GPU render benchmark.");
        return Ok(());
    }

    let out_dir = std::env::var("BENCH_RESULTS_DIR").unwrap_or_else(|_| "results".to_string());
    std::fs::create_dir_all(&out_dir)?;
    let out_path = format!("{out_dir}/3a-rust-wgpu_stats.json");

    // ── GPU init (headless, no window/winit) ────────────────────────────
    let Some(ctx) = GpuContext::new_headless() else {
        // No adapter: e.g. a display-less CI runner (GitHub Actions
        // `ubuntu-latest` typically has neither Vulkan nor DX12). This is NOT
        // an error — degrade gracefully instead of panicking or faking numbers.
        println!(
            "\nNo GPU adapter available — skipping GPU render benchmark (this \
             environment likely has no GPU, e.g. a headless CI runner)."
        );
        let result = serde_json::json!({
            "poc_id": "3a-rust-wgpu",
            "measures": "real-gpu-render (shape+rasterize+atlas-upload+gpu-submit)",
            "gpu_available": false,
            // Canonical `file` block of frame_stats.schema.json. These used to
            // be flat top-level keys, which the aggregator (reading
            // `data["file"]`) silently turned into "?" in the comparison table.
            "file": {
                "line_count": pt.line_count(),
                "load_ms": load_ms,
                "byte_count": pt.byte_len(),
            },
        });
        std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        println!("\nResults written to {}", out_path);
        return Ok(());
    };
    println!(
        "\nGPU adapter: {} (backend={:?})",
        ctx.adapter_name, ctx.backend
    );

    // ── Font + shaping/rasterization faces ──────────────────────────────
    // `build_glyph_instances` needs both a `ttf_parser::Face` (outline
    // rasterization) and a `rustybuzz::Face` (shaping) over the SAME font
    // bytes; both borrow `font_bytes`, so it must outlive them.
    let font_bytes = std::fs::read(&args.font)
        .map_err(|e| format!("failed to read font '{}': {e}", args.font.display()))?;
    let ttf_face = ttf_parser::Face::parse(&font_bytes, 0)
        .map_err(|e| format!("invalid font '{}': {e}", args.font.display()))?;
    let rb_face = rustybuzz::Face::from_face(
        ttf_parser::Face::parse(&font_bytes, 0).expect("already validated above"),
    );

    let mut atlas = GlyphAtlas::new(&ctx.device, rendering::atlas::ATLAS_SIZE);
    let renderer = GlyphRenderer::new(&ctx.device, &atlas, VIEWPORT_W, VIEWPORT_H);

    // ── Real GPU render benchmark ────────────────────────────────────────
    println!(
        "\nRunning GPU render benchmark: {} iters, {:.0}px/iter, {:.0}px line height, {:.1}px font\n  \
         (measures shape + rasterize + atlas-upload + GPU submit per frame, against the {}us / 120Hz budget)",
        args.scroll_frames, args.scroll_px_per_frame, args.line_height_px, args.font_size_px, FRAME_BUDGET_US
    );
    let stats = run_gpu_render_bench(
        &mut pt, &ctx, &ttf_face, &rb_face, &mut atlas, &renderer, &args,
    );

    // ── Untimed readback smoke-check ─────────────────────────────────────
    // Not part of the timed benchmark loop (reading back every frame would be
    // its own, unrelated cost) — one extra render + CPU readback afterwards,
    // confirming the offscreen frame actually contains rendered glyph ink and
    // not just the cleared background. Stronger evidence than trusting
    // `dropped_frames`/timings alone that the GPU path really ran.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "line_height_px is a CLI-supplied UI dimension (default 20.0), f64->f32 here loses no meaningful precision"
    )]
    let line_height_f32 = args.line_height_px as f32;
    let smoke_instances = build_glyph_instances(
        &ctx.device,
        &ctx.queue,
        &ttf_face,
        &rb_face,
        &mut atlas,
        &["PoC 3A smoke test: real GPU glyph rendering"],
        line_height_f32,
        args.font_size_px,
        52.0,
        0xC9D1D9FF,
    );
    renderer.render_frame(&ctx.device, &ctx.queue, &smoke_instances);
    let pixels = renderer.read_color_texture_rgba(&ctx.device, &ctx.queue);
    const BACKGROUND: [i32; 3] = [26, 27, 30]; // clear color (0.102, 0.106, 0.118) as u8.
    let ink_pixels = pixels
        .chunks_exact(4)
        .filter(|px| {
            (i32::from(px[0]) - BACKGROUND[0]).abs() > 8
                || (i32::from(px[1]) - BACKGROUND[1]).abs() > 8
                || (i32::from(px[2]) - BACKGROUND[2]).abs() > 8
        })
        .count();
    println!(
        "Smoke check: {ink_pixels} non-background pixels in the readback \
         (confirms the GPU frame contains real glyph ink)."
    );
    // A readback with zero ink means the GPU path produced no glyphs, i.e. the
    // frame timings below measured an empty render. Printing that and exiting 0
    // published the numbers anyway; it has to be a gate.
    if ink_pixels == 0 {
        eprintln!(
            "error: the GPU readback contains no glyph ink — the render path \
             produced an empty frame, so the timings would be meaningless."
        );
        std::process::exit(1);
    }

    // ── Output results as JSON (schema: shared/metrics/frame_stats.schema.json) ─
    let result = serde_json::json!({
        "poc_id": "3a-rust-wgpu",
        "measures": "real-gpu-render (shape+rasterize+atlas-upload+gpu-submit)",
        "gpu_available": true,
        "gpu_backend": format!("{:?}", ctx.backend),
        "gpu_adapter": ctx.adapter_name,
        // Canonical `file` block of frame_stats.schema.json (same shape 1A/3B
        // emit); the aggregator reads load/line metrics from here.
        "file": {
            "line_count": pt.line_count(),
            "load_ms": load_ms,
            "byte_count": pt.byte_len(),
        },
        // Evidence that the timed frames rendered real ink, carried in the
        // artifact instead of only in stdout.
        "ink_pixels": ink_pixels,
        "benchmark": {
            "total_frames": stats.total_iters,
            "dropped_frames": stats.dropped_frames,
            "drop_rate_pct": stats.drop_rate_pct,
            "p50_ms": stats.p50_us as f64 / 1000.0,
            "p95_ms": stats.p95_us as f64 / 1000.0,
            "p99_ms": stats.p99_us as f64 / 1000.0,
            "budget_ms": FRAME_BUDGET_US as f64 / 1000.0,
            "avg_glyphs_per_frame": stats.avg_glyphs_per_iter,
        },
        "atlas": {
            "hit_rate": stats.atlas_hit_rate,
            "evictions": stats.atlas_evictions,
            "fill_ratio": stats.atlas_fill_ratio,
        }
    });

    // Audit P1: write into the directory the orchestrator points us at
    // (BENCH_RESULTS_DIR = absolute ROOT/results); fall back to a local
    // `results/` for standalone runs.
    std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults written to {}", out_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::default_font_path;

    /// The orchestrator invokes this binary from the repo root without
    /// `--font`, so the default must resolve to the real font regardless of the
    /// working directory. The previous CWD-relative default pointed outside the
    /// repo and made the PoC exit non-zero on every machine with a GPU.
    #[test]
    fn default_font_path_resolves_to_a_real_file() {
        let path = default_font_path();
        assert!(
            path.is_file(),
            "default font path does not resolve to a file: {}",
            path.display()
        );
    }
}
