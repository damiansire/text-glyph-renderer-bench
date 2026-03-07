//! main.rs — CLI test harness for PoC 3A Rust + wgpu + HarfBuzz.
//!
//! Usage:
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt --bench
//!
//! Without --bench: prints file stats and exits.
//! With --bench: runs the synthetic scroll benchmark and emits JSON stats.

use poc_3a_rust_wgpu::{LineIndex, PieceTable, TextBuffer};

use std::path::PathBuf;
use std::time::Instant;

// ── CLI args (manual, no external dependency for now) ──────────────────────

struct Args {
    file: PathBuf,
    bench: bool,
    scroll_frames: u32,
    scroll_px_per_frame: f64,
    line_height_px: f64,
}

impl Args {
    fn parse() -> Self {
        let mut file = None;
        let mut bench = false;
        let mut scroll_frames: u32 = 3600; // 30 s at 120 Hz
        let mut scroll_px = 60.0_f64;
        let mut line_h = 20.0_f64;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--file" => file = args.next().map(PathBuf::from),
                "--bench" => bench = true,
                "--frames" => scroll_frames = args.next().unwrap().parse().unwrap(),
                "--scroll-px" => scroll_px = args.next().unwrap().parse().unwrap(),
                "--line-height" => line_h = args.next().unwrap().parse().unwrap(),
                _ => {}
            }
        }

        Self {
            file: file.expect("--file <path> is required"),
            bench,
            scroll_frames,
            scroll_px_per_frame: scroll_px,
            line_height_px: line_h,
        }
    }
}

// ── Benchmark: synthetic scroll ────────────────────────────────────────────

struct ScrollStats {
    total_frames: u32,
    dropped_frames: u32,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    avg_lines_shaped: f64,
}

fn run_scroll_benchmark(pt: &mut PieceTable, args: &Args) -> ScrollStats {
    const FRAME_BUDGET_US: u64 = 8333; // 1/120 s in µs
    const VIEWPORT_LINES: usize = 50;
    const MARGIN_LINES: usize = 50;

    let line_count = pt.line_count();
    let total_height_px = line_count as f64 * args.line_height_px;
    let mut scroll_y: f64 = 0.0;

    let mut frame_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut total_lines_shaped: u64 = 0;
    let mut dropped = 0u32;

    for _frame in 0..args.scroll_frames {
        let t_start = Instant::now();

        // ── Compute visible line range ──────────────────────────────────
        let first_visible_line = (scroll_y / args.line_height_px) as usize;
        let last_visible_line =
            (first_visible_line + VIEWPORT_LINES + MARGIN_LINES).min(line_count.saturating_sub(1));

        // ── Simulate lazy shaping: access line byte ranges ──────────────
        let mut lines_shaped = 0usize;
        for line_idx in first_visible_line..=last_visible_line {
            let line_start = pt.line_start_byte(line_idx);
            let line_end = if line_idx + 1 < line_count {
                pt.line_start_byte(line_idx + 1)
            } else {
                pt.byte_len()
            };

            // Zero-copy iteration over pieces for this line range
            pt.slice_pieces(line_start..line_end, |_slice, _offset| {
                // In the real renderer this slice goes to HarfBuzz for shaping.
                // Here we just count to simulate the work unit.
                true
            });
            lines_shaped += 1;
        }
        total_lines_shaped += lines_shaped as u64;

        // ── Advance scroll position ────────────────────────────────────
        scroll_y += args.scroll_px_per_frame;
        if scroll_y + (VIEWPORT_LINES as f64 * args.line_height_px) > total_height_px {
            scroll_y = 0.0; // wrap-around for the benchmark
        }

        let elapsed_us = t_start.elapsed().as_micros() as u64;
        frame_times_us.push(elapsed_us);
        if elapsed_us > FRAME_BUDGET_US {
            dropped += 1;
        }
    }

    // ── Percentiles ────────────────────────────────────────────────────
    frame_times_us.sort_unstable();
    let n = frame_times_us.len();
    let p50 = frame_times_us[n * 50 / 100];
    let p95 = frame_times_us[n * 95 / 100];
    let p99 = frame_times_us[n * 99 / 100];
    let avg_lines = total_lines_shaped as f64 / args.scroll_frames as f64;

    ScrollStats {
        total_frames: args.scroll_frames,
        dropped_frames: dropped,
        p50_us: p50,
        p95_us: p95,
        p99_us: p99,
        avg_lines_shaped: avg_lines,
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
        // Just print stats and exit
        println!("\nLine 0   start byte: {}", pt.line_start_byte(0));
        println!("Line 100 start byte: {}", pt.line_start_byte(100.min(pt.line_count() - 1)));
        println!("Byte 4096 → line {}", pt.byte_to_line(4096.min(pt.byte_len() - 1)));
        println!("\nDone. Run with --bench for scroll benchmark.");
        return Ok(());
    }

    // ── Synthetic scroll benchmark ─────────────────────────────────────
    println!(
        "\nRunning scroll benchmark: {} frames, {:.0}px/frame, {:.0}px line height",
        args.scroll_frames, args.scroll_px_per_frame, args.line_height_px
    );
    let stats = run_scroll_benchmark(&mut pt, &args);

    // ── Output results as JSON ─────────────────────────────────────────
    let result = serde_json::json!({
        "poc_id": "3a-rust-wgpu",
        "file_bytes": pt.byte_len(),
        "file_lines": pt.line_count(),
        "load_ms": load_ms,
        "benchmark": {
            "total_frames": stats.total_frames,
            "dropped_frames": stats.dropped_frames,
            "drop_rate_pct": stats.dropped_frames as f64 / stats.total_frames as f64 * 100.0,
            "p50_us": stats.p50_us,
            "p95_us": stats.p95_us,
            "p99_us": stats.p99_us,
            "avg_lines_per_frame": stats.avg_lines_shaped,
        }
    });

    let out_path = format!("results/3a-rust-wgpu_stats.json");
    std::fs::create_dir_all("results")?;
    std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults written to {}", out_path);

    Ok(())
}
