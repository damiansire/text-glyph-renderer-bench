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
mod buffer;

use buffer::{PieceTable, TextBuffer};
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("PoC 3B — Rust + Vello");
    println!("File: {}", args.file.display());
    println!("Font: {}", args.font.display());

    // ── Load text file ─────────────────────────────────────────────────────
    let t_load = Instant::now();
    let pt = PieceTable::from_file(&args.file)?;
    println!("Loaded {} MB, {} lines in {:.1}ms",
        pt.byte_len() / 1_048_576,
        pt.line_count(),
        t_load.elapsed().as_secs_f64() * 1000.0
    );

    // ── Load font ──────────────────────────────────────────────────────────
    let font = VelloFont::load(&args.font, 13.0)?;
    let scene_builder = TextSceneBuilder::new(font);

    if !args.bench {
        println!("Use --bench to run the scroll benchmark.");
        println!("Use --headless for CI mode (no window).");
        return Ok(());
    }

    // ── Headless scroll benchmark (no GPU window) ──────────────────────────
    // In a full implementation this would open a winit window and use the
    // Vello renderer.  For the CLI benchmark we simulate the scene building
    // cost (CPU side: curve extraction per frame) without GPU submission.

    println!("\nRunning headless scene-build benchmark ({} frames)...", args.scroll_frames);

    // Pre-split lines (Vec of byte slices into the mmap)
    let all_bytes: Vec<u8> = pt.bytes_in_range(0..pt.byte_len());
    let lines: Vec<&[u8]> = all_bytes.split(|&b| b == b'\n').collect();
    let line_count = lines.len();

    let FRAME_BUDGET_US: u64 = 8333;
    let mut frame_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut dropped = 0u32;
    let mut scroll_y = 0.0_f64;
    let total_h = line_count as f64 * args.line_height;

    for _frame in 0..args.scroll_frames {
        let t0 = Instant::now();

        let first_line = (scroll_y / args.line_height) as usize;
        // Build Vello scene (CPU curve extraction + scene encoding)
        let _scene = scene_builder.build_scene(
            &lines,
            first_line.min(line_count.saturating_sub(1)),
            900.0,
            args.line_height,
            scroll_y,
        );

        scroll_y += args.scroll_px;
        if scroll_y + 900.0 > total_h { scroll_y = 0.0; }

        let elapsed_us = t0.elapsed().as_micros() as u64;
        frame_times_us.push(elapsed_us);
        if elapsed_us > FRAME_BUDGET_US { dropped += 1; }
    }

    // ── Results ─────────────────────────────────────────────────────────────
    frame_times_us.sort_unstable();
    let n = frame_times_us.len();
    let p = |pct: usize| frame_times_us[n * pct / 100];
    let avg: f64 = frame_times_us.iter().map(|&v| v as f64).sum::<f64>() / n as f64;

    let result = serde_json::json!({
        "poc_id": "3b-rust-vello",
        "file": { "line_count": line_count, "load_ms": t_load.elapsed().as_millis() },
        "benchmark": {
            "total_frames":   args.scroll_frames,
            "dropped_frames": dropped,
            "drop_rate_pct":  dropped as f64 / args.scroll_frames as f64 * 100.0,
            "p50_us": p(50), "p95_us": p(95), "p99_us": p(99),
            "p50_ms": p(50) as f64 / 1000.0,
            "p95_ms": p(95) as f64 / 1000.0,
            "p99_ms": p(99) as f64 / 1000.0,
            "avg_us": avg,
            "budget_ms": 8.333,
            "note": "headless: CPU scene build only; GPU submission excluded from timing",
        }
    });

    std::fs::create_dir_all("results")?;
    let out = "results/3b-rust-vello_stats.json";
    std::fs::write(out, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults → {}", out);

    Ok(())
}
