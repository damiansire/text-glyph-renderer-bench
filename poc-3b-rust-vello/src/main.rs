//! main.rs — PoC 3B entry point: Rust + Vello (Compute Shader renderer)
//!
//! Architecture:
//!   TextBuffer = shared `PieceTable` from the `text-buffer` crate, re-exported
//!   by this crate's `lib.rs`. 3A and 3B consume the exact same buffer
//!   implementation and differ only in their renderer (it is NOT re-implemented
//!   here).
//!
//!   Renderer = Vello's `vello::Renderer` backed by wgpu Metal (Apple Silicon).
//!   No texture atlas; curves go to GPU as geometry, Compute Shaders evaluate
//!   coverage at render time.
//!
//! Key difference vs. PoC 3A:
//!   3A: CPU rasterizes glyph → uploads to R8Unorm atlas → GPU samples atlas.
//!   3B: CPU sends Bézier outline → GPU computes coverage pixel-by-pixel.
//!       Zero atlas memory. Perfect quality at all scales.

mod rendering;

use poc_3b_rust_vello::{PieceTable, TextBuffer};
use rendering::scene_builder::{TextSceneBuilder, VelloFont};

use std::path::{Path, PathBuf};
use std::time::Instant;

/// Defaults anchored to the crate directory instead of the process working
/// directory: the orchestrator (`shared/metrics/benchmark_runner.py`) runs this
/// binary with `cwd` = repo root, where a CWD-relative `../shared/...` default
/// resolves outside the repo and the PoC dies loading the font.
fn crate_relative(rest: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rest)
}

struct Args {
    file: PathBuf,
    font: PathBuf,
    bench: bool,
    scroll_frames: u32,
    scroll_px: f64,
    line_height: f64,
    headless: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            file: crate_relative("../shared/test-data/test_100mb.txt"),
            font: crate_relative("../shared/fonts/InterVariable.ttf"),
            bench: false,
            scroll_frames: 3600,
            scroll_px: 60.0,
            line_height: 20.0,
            headless: false,
        };
        // Helper: a missing/malformed flag value prints a clear error and
        // exits instead of panicking with a backtrace (audit P5).
        fn value(flag: &str, raw: Option<String>) -> String {
            raw.unwrap_or_else(|| {
                eprintln!("error: {flag} requires a value");
                std::process::exit(2);
            })
        }
        fn parse_value<T: std::str::FromStr>(flag: &str, raw: Option<String>) -> T {
            let raw = value(flag, raw);
            raw.parse().unwrap_or_else(|_| {
                eprintln!("error: invalid value '{raw}' for {flag}");
                std::process::exit(2);
            })
        }

        let mut argv = std::env::args().skip(1);
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--file" => args.file = value("--file", argv.next()).into(),
                "--font" => args.font = value("--font", argv.next()).into(),
                "--bench" => args.bench = true,
                // Accepted for the orchestrator's CLI contract. 3B only has a
                // headless path (the bench is a CPU scene build, no window
                // exists), so the flag selects nothing; erroring on it would
                // break every documented invocation.
                "--headless" => args.headless = true,
                "--frames" => args.scroll_frames = parse_value("--frames", argv.next()),
                "--scroll-px" => args.scroll_px = parse_value("--scroll-px", argv.next()),
                "--line-height" => args.line_height = parse_value("--line-height", argv.next()),
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
        args
    }
}

const USAGE: &str = "\
poc3b — Rust + Vello CPU scene-build microbenchmark

Usage: poc3b [--file <path>] [--bench] [--frames <n>] [options]

Options:
  --file <path>         Corpus to load (default: shared/test-data/test_100mb.txt)
  --font <path>         Font file (default: shared/fonts/InterVariable.ttf)
  --bench               Run the scene-build microbenchmark and write the report
  --frames <n>          Benchmark iterations (default: 3600)
  --scroll-px <px>      Scroll advance per iteration (default: 60)
  --line-height <px>    Line height in px (default: 20)
  --headless            Accepted for the runner's CLI contract; 3B only has a
                        headless path, so this selects nothing
  --help, -h            Show this help";

/// Build the run report in the canonical `frame_stats.schema.json` shape.
///
/// Takes `load_ms` as an already-measured value on purpose: the metric it
/// publishes is the corpus load, so it must not be derivable from anything this
/// function can still observe (that is exactly how the loop time ended up
/// reported as load time).
///
/// No `dropped_frames` / `drop_rate_pct` / 8.33 ms comparison: this loop only
/// exercises the CPU scene build, not an end-to-end render frame. Sorts
/// `iter_times_us` in place and guards the empty case (`--frames 0`), where the
/// percentile indexing would panic and `avg` divide by zero (audit P5).
fn build_report(
    line_count: usize,
    load_ms: u128,
    total_iters: u32,
    iter_times_us: &mut [u64],
) -> serde_json::Value {
    iter_times_us.sort_unstable();
    let n = iter_times_us.len();
    let p = |pct: usize| {
        if n == 0 {
            0
        } else {
            iter_times_us[(n * pct / 100).min(n - 1)]
        }
    };
    #[allow(
        clippy::cast_precision_loss,
        reason = "iteration counts and microsecond timings are far below f64's exact-integer range"
    )]
    let avg: f64 = if n == 0 {
        0.0
    } else {
        iter_times_us.iter().map(|&v| v as f64).sum::<f64>() / n as f64
    };

    serde_json::json!({
        "poc_id": "3b-rust-vello",
        "measures": "cpu-scene-build-only (real glyph geometry, no raster/gpu submission)",
        "file": { "line_count": line_count, "load_ms": load_ms },
        "scene_build": {
            "total_iters": total_iters,
            "p50_us": p(50), "p95_us": p(95), "p99_us": p(99),
            "p50_ms": p(50) as f64 / 1000.0,
            "p95_ms": p(95) as f64 / 1000.0,
            "p99_ms": p(99) as f64 / 1000.0,
            "avg_us": avg,
            "note": "headless: CPU scene build with real glyph geometry (draw_glyphs); excludes GPU submission",
        }
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "bench scaffolding: scroll->line index is a deliberate floor and elapsed micros fit in u64"
)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("PoC 3B — Rust + Vello");
    println!("File: {}", args.file.display());

    // ── Load text file ─────────────────────────────────────────────────────
    // The elapsed time is frozen HERE. Reading `t_load.elapsed()` later (down in
    // the report) measured load + the whole benchmark loop and published it as
    // `file.load_ms`: 3233 ms for a 1 MB mmap, three orders of magnitude off.
    let t_load = Instant::now();
    let pt = PieceTable::from_file(&args.file)?;
    let load_ms = t_load.elapsed().as_millis();
    println!(
        "Loaded {} MB, {} lines in {}ms",
        pt.byte_len() / 1_048_576,
        pt.line_count(),
        load_ms
    );

    if !args.bench {
        println!("Use --bench to run the scene-build microbenchmark.");
        return Ok(());
    }

    // ── Font + scene builder ───────────────────────────────────────────────
    // `build_scene` needs a valid font (skrifa `FontRef`), so the font must be
    // present.  If it is missing we fail loudly here instead of silently
    // running an empty loop that measures nothing.
    let font = VelloFont::load(&args.font, 13.0)?;
    let scene_builder = TextSceneBuilder::new(font);

    // ── Headless scene-build microbenchmark (no GPU window) ────────────────
    // IMPORTANT — what this measures: per iteration it runs the CPU-side scene
    // build for the visible viewport (shaping + real glyph geometry encoded via
    // `Scene::draw_glyphs`, see `build_scene` docs). It does NOT submit to the
    // GPU (no window, no render pass) — this is scene *construction* cost only.
    // Therefore it does NOT report `dropped_frames` / `drop_rate` against the
    // 8.33 ms frame budget: doing so would over-sell a CPU microbenchmark as an
    // end-to-end render budget. Re-introduce the frame-budget verdict only once
    // this PoC actually submits the built scene to the GPU and presents a frame.
    println!(
        "\nRunning headless scene-build microbench: {} iters ...",
        args.scroll_frames
    );
    println!(
        "  (measures CPU scene build — real glyph geometry, no raster/GPU submission,\n   so no frame-budget drop rate is reported)"
    );

    // F6 (memory): do NOT materialise the whole ~100 MB file into a contiguous
    // `Vec<u8>` up front (the old `bytes_in_range(0..byte_len())` doubled the
    // resident set: mmap pages + a full heap copy, locally negating the crate's
    // zero-copy premise). Instead we mirror PoC 3A: only the *visible* line
    // window is resolved per frame, straight off the piece table via
    // `line_start_byte` + `slice_pieces` (zero-copy borrows into the mmap),
    // copied into a small reusable per-line buffer bounded by the viewport
    // (~tens of lines), never the whole document.
    const VIEWPORT_H: f64 = 900.0;
    // A couple of extra lines of margin so partially visible rows still count.
    let visible_lines = (VIEWPORT_H / args.line_height) as usize + 2;

    let line_count = pt.line_count();
    let mut iter_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut scroll_y = 0.0_f64;
    let total_h = line_count as f64 * args.line_height;

    // Reusable buffers: the line byte-store and the `&[u8]` view slice are
    // allocated once and cleared per frame, so the steady state allocates
    // nothing proportional to the file size.
    let mut window_bytes: Vec<u8> = Vec::new();
    let mut window_spans: Vec<(usize, usize)> = Vec::with_capacity(visible_lines);

    for _iter in 0..args.scroll_frames {
        let t0 = Instant::now();

        let first_line = (scroll_y / args.line_height) as usize;
        let last_line = (first_line + visible_lines).min(line_count.saturating_sub(1));

        // Resolve only the visible window off the piece table (zero-copy reads
        // into the mmap), appending each line into the reusable buffer and
        // recording its (start, end) span. The total copied per frame is the
        // size of the viewport, not of the file.
        window_bytes.clear();
        window_spans.clear();
        for line_idx in first_line..=last_line {
            let line_start = pt.line_start_byte(line_idx);
            let line_end = if line_idx + 1 < line_count {
                pt.line_start_byte(line_idx + 1)
            } else {
                pt.byte_len()
            };
            let span_start = window_bytes.len();
            pt.slice_pieces(line_start..line_end, |slice, _| {
                window_bytes.extend_from_slice(slice);
                true
            });
            window_spans.push((span_start, window_bytes.len()));
        }
        // Materialise the borrowed `&[u8]` views now that `window_bytes` is
        // stable for this frame (the spans index into it).
        let lines: Vec<&[u8]> = window_spans
            .iter()
            .map(|&(s, e)| &window_bytes[s..e])
            .collect();

        // CPU-side scene build for the visible viewport (shaping + real glyph
        // geometry via `Scene::draw_glyphs`). Submits nothing to the GPU.
        // `first_line` is 0 because `lines` already starts at the first visible
        // line (the window was sliced for this frame).
        let _scene = scene_builder.build_scene(&lines, 0, VIEWPORT_H, args.line_height, scroll_y);

        scroll_y += args.scroll_px;
        if scroll_y + VIEWPORT_H > total_h {
            scroll_y = 0.0;
        }

        let elapsed_us = t0.elapsed().as_micros() as u64;
        iter_times_us.push(elapsed_us);
    }

    // ── Results ─────────────────────────────────────────────────────────────
    let result = build_report(line_count, load_ms, args.scroll_frames, &mut iter_times_us);

    // Audit P1: write into the directory the orchestrator points us at
    // (BENCH_RESULTS_DIR = absolute ROOT/results); fall back to a local
    // `results/` for standalone runs.
    let out_dir = std::env::var("BENCH_RESULTS_DIR").unwrap_or_else(|_| "results".to_string());
    std::fs::create_dir_all(&out_dir)?;
    let out = format!("{out_dir}/3b-rust-vello_stats.json");
    std::fs::write(&out, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults → {}", out);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_report, crate_relative};

    /// Regression for the metric that was three orders of magnitude wrong:
    /// `file.load_ms` published the whole benchmark loop instead of the corpus
    /// load. The report must carry the load measurement it was given, no matter
    /// how expensive the iterations were.
    #[test]
    fn report_load_ms_is_the_load_measurement_not_the_loop() {
        let mut iters = vec![54_000_u64; 50]; // 50 iterations of ~54 ms each.
        let report = build_report(17_955, 7, 50, &mut iters);

        let load_ms = report["file"]["load_ms"].as_u64().expect("load_ms");
        assert_eq!(load_ms, 7, "load_ms must be the measured load time");

        let loop_ms: u64 = iters.iter().sum::<u64>() / 1000;
        assert!(
            load_ms < loop_ms,
            "load_ms ({load_ms}) cannot exceed the loop time ({loop_ms}); \
             that is the symptom of timing the loop instead of the load"
        );
    }

    #[test]
    fn report_handles_zero_iterations_without_panicking() {
        let mut iters: Vec<u64> = Vec::new();
        let report = build_report(0, 0, 0, &mut iters);
        assert_eq!(report["scene_build"]["p99_us"].as_u64(), Some(0));
        assert_eq!(report["scene_build"]["avg_us"].as_f64(), Some(0.0));
    }

    /// The orchestrator runs this binary from the repo root without `--font`,
    /// so the defaults must not depend on the working directory.
    #[test]
    fn default_font_resolves_to_a_real_file() {
        let path = crate_relative("../shared/fonts/InterVariable.ttf");
        assert!(
            path.is_file(),
            "default font path does not resolve to a file: {}",
            path.display()
        );
    }
}
