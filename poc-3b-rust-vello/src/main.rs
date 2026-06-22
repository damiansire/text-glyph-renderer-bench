//! main.rs — PoC 3B entry point: Rust + Vello (Compute Shader renderer)
//!
//! Architecture:
//!   TextBuffer = shared PieceTable (from poc-3a-rust-wgpu, re-implemented here
//!   for standalone operation — in the workspace it could be a shared crate).
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

use std::path::PathBuf;
use std::time::Instant;

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
            file: PathBuf::from("../shared/test-data/test_100mb.txt"),
            font: PathBuf::from("../shared/fonts/InterVariable.ttf"),
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
                "--headless" => args.headless = true,
                "--frames" => args.scroll_frames = parse_value("--frames", argv.next()),
                "--scroll-px" => args.scroll_px = parse_value("--scroll-px", argv.next()),
                "--line-height" => args.line_height = parse_value("--line-height", argv.next()),
                _ => {}
            }
        }
        args
    }
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
    let t_load = Instant::now();
    let pt = PieceTable::from_file(&args.file)?;
    println!(
        "Loaded {} MB, {} lines in {:.1}ms",
        pt.byte_len() / 1_048_576,
        pt.line_count(),
        t_load.elapsed().as_secs_f64() * 1000.0
    );

    if !args.bench {
        println!("Use --bench to run the scene-build microbenchmark.");
        println!("Use --headless for CI mode (no window).");
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
    // build for the visible viewport (line traversal + charmap lookups). It
    // builds NO geometry and does NO GPU submission (see `build_scene` docs and
    // the PoC status in the README). Therefore it does NOT report
    // `dropped_frames` / `drop_rate` against the 8.33 ms frame budget: doing so
    // would over-sell a CPU microbenchmark as an end-to-end render budget.
    // Re-introduce the frame-budget verdict only once real geometry + GPU land.
    println!(
        "\nRunning headless scene-build microbench: {} iters ...",
        args.scroll_frames
    );
    println!(
        "  (measures CPU scene build only — NO geometry/raster/GPU,\n   so no frame-budget drop rate is reported)"
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

        // CPU-side scene build for the visible viewport (line traversal +
        // charmap lookups). Builds no geometry and submits nothing to the GPU.
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
    // No `dropped_frames` / `drop_rate_pct` / 8.33 ms comparison: this loop
    // only exercises the CPU scene build, not an end-to-end render frame.
    // Guard the empty case (`--frames 0`): empty `iter_times_us` would make the
    // percentile indexing panic and `avg` divide by zero (audit P5).
    iter_times_us.sort_unstable();
    let n = iter_times_us.len();
    let p = |pct: usize| {
        if n == 0 {
            0
        } else {
            iter_times_us[(n * pct / 100).min(n - 1)]
        }
    };
    let avg: f64 = if n == 0 {
        0.0
    } else {
        iter_times_us.iter().map(|&v| v as f64).sum::<f64>() / n as f64
    };

    let result = serde_json::json!({
        "poc_id": "3b-rust-vello",
        "measures": "cpu-scene-build-only (no geometry/raster/gpu)",
        "file": { "line_count": line_count, "load_ms": t_load.elapsed().as_millis() },
        "scene_build": {
            "total_iters": args.scroll_frames,
            "p50_us": p(50), "p95_us": p(95), "p99_us": p(99),
            "p50_ms": p(50) as f64 / 1000.0,
            "p95_ms": p(95) as f64 / 1000.0,
            "p99_ms": p(99) as f64 / 1000.0,
            "avg_us": avg,
            "note": "headless: CPU scene build only; builds no geometry and excludes GPU submission",
        }
    });

    std::fs::create_dir_all("results")?;
    let out = "results/3b-rust-vello_stats.json";
    std::fs::write(out, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults → {}", out);

    Ok(())
}
