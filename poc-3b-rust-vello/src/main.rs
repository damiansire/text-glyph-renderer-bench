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
    file:            PathBuf,
    font:            PathBuf,
    bench:           bool,
    scroll_frames:   u32,
    scroll_px:       f64,
    line_height:     f64,
    headless:        bool,
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
        let mut argv = std::env::args().skip(1);
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--file"        => args.file   = argv.next().unwrap().into(),
                "--font"        => args.font   = argv.next().unwrap().into(),
                "--bench"       => args.bench  = true,
                "--headless"    => args.headless = true,
                "--frames"      => args.scroll_frames = argv.next().unwrap().parse().unwrap(),
                "--scroll-px"   => args.scroll_px     = argv.next().unwrap().parse().unwrap(),
                "--line-height" => args.line_height   = argv.next().unwrap().parse().unwrap(),
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
    println!("Loaded {} MB, {} lines in {:.1}ms",
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

    // Pre-split lines (Vec of byte slices into the mmap)
    let all_bytes: Vec<u8> = pt.bytes_in_range(0..pt.byte_len());
    let lines: Vec<&[u8]> = all_bytes.split(|&b| b == b'\n').collect();
    let line_count = lines.len();

    const VIEWPORT_H: f64 = 900.0;
    let mut iter_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut scroll_y = 0.0_f64;
    let total_h = line_count as f64 * args.line_height;

    for _iter in 0..args.scroll_frames {
        let t0 = Instant::now();

        let first_line = (scroll_y / args.line_height) as usize;
        // CPU-side scene build for the visible viewport (line traversal +
        // charmap lookups). Builds no geometry and submits nothing to the GPU.
        let _scene = scene_builder.build_scene(
            &lines,
            first_line,
            VIEWPORT_H,
            args.line_height,
            scroll_y,
        );

        scroll_y += args.scroll_px;
        if scroll_y + VIEWPORT_H > total_h { scroll_y = 0.0; }

        let elapsed_us = t0.elapsed().as_micros() as u64;
        iter_times_us.push(elapsed_us);
    }

    // ── Results ─────────────────────────────────────────────────────────────
    // No `dropped_frames` / `drop_rate_pct` / 8.33 ms comparison: this loop
    // only exercises the CPU scene build, not an end-to-end render frame.
    iter_times_us.sort_unstable();
    let n = iter_times_us.len();
    let p = |pct: usize| iter_times_us[n * pct / 100];
    let avg: f64 = iter_times_us.iter().map(|&v| v as f64).sum::<f64>() / n as f64;

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
