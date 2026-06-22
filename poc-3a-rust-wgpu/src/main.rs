//! main.rs — CLI test harness for PoC 3A Rust + wgpu + HarfBuzz.
//!
//! Usage:
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt
//!   cargo run --release -p poc-3a-rust-wgpu -- --file path/to/test_100mb.txt --bench
//!
//! Without --bench: prints file stats and exits.
//! With --bench: runs a **line-index traversal microbenchmark**.
//!
//! IMPORTANT — what this measures: per "frame" it only resolves the visible
//! line range (line-index lookups + piece-table pointer slicing). There is NO
//! shaping, NO rasterization and NO GPU work in this loop. Therefore it does
//! NOT report `dropped_frames` / `drop_rate` against the 8.33 ms frame budget:
//! doing so would over-sell a microbenchmark as an end-to-end render budget.
//! Re-introduce the frame-budget verdict only once real shaping + raster land.

use poc_3a_rust_wgpu::{PieceTable, TextBuffer};

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
                "--bench" => bench = true,
                "--frames" => scroll_frames = parse_value("--frames", args.next()),
                "--scroll-px" => scroll_px = parse_value("--scroll-px", args.next()),
                "--line-height" => line_h = parse_value("--line-height", args.next()),
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

struct TraversalStats {
    total_iters: u32,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    avg_lines_visited: f64,
}

/// Line-index traversal microbenchmark.
///
/// Per iteration this resolves the visible line range and walks the
/// piece-table slices for those lines. It measures ONLY line-index lookups +
/// pointer arithmetic — there is no shaping, no rasterization, no GPU work.
/// Consequently it does NOT compute a frame-budget drop rate (see module docs).
#[allow(
    clippy::cast_possible_truncation,
    reason = "bench scaffolding: scroll->line index is a deliberate floor and elapsed micros fit in u64"
)]
fn run_traversal_microbench(pt: &mut PieceTable, args: &Args) -> TraversalStats {
    const VIEWPORT_LINES: usize = 50;
    const MARGIN_LINES: usize = 50;

    let line_count = pt.line_count();
    let total_height_px = line_count as f64 * args.line_height_px;
    let mut scroll_y: f64 = 0.0;

    let mut iter_times_us: Vec<u64> = Vec::with_capacity(args.scroll_frames as usize);
    let mut total_lines_visited: u64 = 0;

    for _iter in 0..args.scroll_frames {
        let t_start = Instant::now();

        // ── Compute visible line range ──────────────────────────────────
        let first_visible_line = (scroll_y / args.line_height_px) as usize;
        let last_visible_line =
            (first_visible_line + VIEWPORT_LINES + MARGIN_LINES).min(line_count.saturating_sub(1));

        // ── Traverse line byte ranges (line-index + piece slicing only) ──
        let mut lines_visited = 0usize;
        for line_idx in first_visible_line..=last_visible_line {
            let line_start = pt.line_start_byte(line_idx);
            let line_end = if line_idx + 1 < line_count {
                pt.line_start_byte(line_idx + 1)
            } else {
                pt.byte_len()
            };

            // Zero-copy iteration over pieces for this line range.
            // NOTE: the real renderer would hand this slice to HarfBuzz for
            // shaping; that cost is NOT modelled here.
            pt.slice_pieces(line_start..line_end, |_slice, _offset| true);
            lines_visited += 1;
        }
        total_lines_visited += lines_visited as u64;

        // ── Advance scroll position ────────────────────────────────────
        scroll_y += args.scroll_px_per_frame;
        if scroll_y + (VIEWPORT_LINES as f64 * args.line_height_px) > total_height_px {
            scroll_y = 0.0; // wrap-around for the benchmark
        }

        let elapsed_us = t_start.elapsed().as_micros() as u64;
        iter_times_us.push(elapsed_us);
    }

    // ── Percentiles ────────────────────────────────────────────────────
    // Guard the empty case (`--frames 0`): an empty `iter_times_us` would make
    // the percentile indexing panic and `avg_lines` divide by zero (audit P5).
    iter_times_us.sort_unstable();
    let n = iter_times_us.len();
    let pct = |p: usize| {
        if n == 0 {
            0
        } else {
            // Clamp the index to the last element so p99 of a tiny sample is
            // in-bounds.
            iter_times_us[(n * p / 100).min(n - 1)]
        }
    };
    let p50 = pct(50);
    let p95 = pct(95);
    let p99 = pct(99);
    let avg_lines = if args.scroll_frames == 0 {
        0.0
    } else {
        total_lines_visited as f64 / args.scroll_frames as f64
    };

    TraversalStats {
        total_iters: args.scroll_frames,
        p50_us: p50,
        p95_us: p95,
        p99_us: p99,
        avg_lines_visited: avg_lines,
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
        println!("\nDone. Run with --bench for scroll benchmark.");
        return Ok(());
    }

    // ── Line-index traversal microbenchmark ────────────────────────────
    println!(
        "\nRunning line-index traversal microbench: {} iters, {:.0}px/iter, {:.0}px line height",
        args.scroll_frames, args.scroll_px_per_frame, args.line_height_px
    );
    println!(
        "  (measures line-index lookups + piece slicing only — NO shaping/raster/GPU,\n   so no frame-budget drop rate is reported)"
    );
    let stats = run_traversal_microbench(&mut pt, &args);

    // ── Output results as JSON ─────────────────────────────────────────
    // No `dropped_frames` / `drop_rate_pct` / 8.33 ms comparison: this loop
    // only exercises line-index traversal, not an end-to-end render frame.
    let result = serde_json::json!({
        "poc_id": "3a-rust-wgpu",
        "measures": "line-index-traversal-only (no shaping/raster/gpu)",
        "file_bytes": pt.byte_len(),
        "file_lines": pt.line_count(),
        "load_ms": load_ms,
        "line_index_traversal": {
            "total_iters": stats.total_iters,
            "p50_us": stats.p50_us,
            "p95_us": stats.p95_us,
            "p99_us": stats.p99_us,
            "avg_lines_per_iter": stats.avg_lines_visited,
        }
    });

    let out_path = "results/3a-rust-wgpu_stats.json";
    std::fs::create_dir_all("results")?;
    std::fs::write(out_path, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nResults written to {}", out_path);

    Ok(())
}
