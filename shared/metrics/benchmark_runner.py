#!/usr/bin/env python3
"""
benchmark_runner.py — Shared orchestrator that runs all PoC benchmarks
and aggregates results into a single comparison CSV + Markdown table.

Usage:
    python3 shared/metrics/benchmark_runner.py
    python3 shared/metrics/benchmark_runner.py --poc 1a 2b 3a
    python3 shared/metrics/benchmark_runner.py --file path/to/test.txt

Prerequisites:
    - test_100mb.txt generated (run: python3 shared/test-data/generate_testfile.py)
    - Node.js 20+  (for PoCs 1A–1D)
    - Xcode 15+    (for PoCs 2A, 2B: swift build)
    - Rust 1.78+   (for PoCs 3A, 3B: cargo build)
    - npm packages installed in each poc-1* directory

Output:
    results/comparison.csv
    results/comparison.md
"""

import subprocess
import json
import os
import sys
import csv
import time
import argparse
from pathlib import Path

# ── Paths ────────────────────────────────────────────────────────────────────

ROOT_DIR     = Path(__file__).parent.parent.parent  # monorepo root
RESULTS_DIR  = ROOT_DIR / "results"
TEST_FILE    = ROOT_DIR / "shared" / "test-data" / "test_100mb.txt"

# ── PoC registry ─────────────────────────────────────────────────────────────

POCS = {
    "1a": {
        "label": "PoC 1A — Web DOM (Electron)",
        "cwd":   ROOT_DIR / "poc-1a-web-dom",
        "cmd":   ["node", "benchmarks/synthetic_scroll.js", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1a-web-dom-nodejs_stats.json",
        "category": "Web",
    },
    "1b": {
        "label": "PoC 1B — Canvas 2D",
        "cwd":   ROOT_DIR / "poc-1b-canvas2d",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1b-canvas2d_stats.json",
        "category": "Web",
    },
    "1c": {
        "label": "PoC 1C — WebGPU Atlas",
        "cwd":   ROOT_DIR / "poc-1c-webgpu-atlas",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1c-webgpu-atlas_stats.json",
        "category": "Web",
    },
    "1d": {
        "label": "PoC 1D — WebGPU MSDF",
        "cwd":   ROOT_DIR / "poc-1d-webgpu-msdf",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1d-webgpu-msdf_stats.json",
        "category": "Web",
    },
    "2a": {
        "label": "PoC 2A — TextKit 2 (Swift)",
        "cwd":   ROOT_DIR / "poc-2a-textkit2",
        "cmd":   ["swift", "run", "-c", "release", "POC2A",
                  "--benchmark", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "2a-textkit2_stats.json",
        "category": "Native macOS",
    },
    "2b": {
        "label": "PoC 2B — Metal 3 + CoreText",
        "cwd":   ROOT_DIR / "poc-2b-metal3-coretext",
        "cmd":   ["swift", "run", "-c", "release", "POC2B",
                  "--benchmark", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "2b-metal3-coretext_stats.json",
        "category": "Native macOS",
    },
    "3a": {
        "label": "PoC 3A — Rust + wgpu + HarfBuzz",
        "cwd":   ROOT_DIR,
        "cmd":   ["cargo", "run", "--release", "-p", "poc-3a-rust-wgpu", "--",
                  "--bench", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "3a-rust-wgpu_stats.json",
        "category": "Systems (Rust)",
    },
    "3b": {
        "label": "PoC 3B — Rust + Vello",
        "cwd":   ROOT_DIR,
        "cmd":   ["cargo", "run", "--release", "-p", "poc-3b-rust-vello", "--",
                  "--bench", "--headless", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "3b-rust-vello_stats.json",
        "category": "Systems (Rust)",
    },
}

# ── Runner ───────────────────────────────────────────────────────────────────

def run_poc(poc_id: str, info: dict) -> dict | None:
    print(f"\n{'='*60}")
    print(f"Running {info['label']}...")
    print(f"{'='*60}")
    t0 = time.monotonic()
    try:
        # Avoid Electron environment contamination
        env = os.environ.copy()
        env.pop("ELECTRON_RUN_AS_NODE", None)

        result = subprocess.run(
            info["cmd"],
            cwd=str(info["cwd"]),
            env=env,
            timeout=300,  # 5 minutes max per PoC
            check=True,
            capture_output=False,
        )
        elapsed = time.monotonic() - t0
        print(f"  Completed in {elapsed:.1f}s")

        # Read result JSON
        result_file = info["result_file"]
        if result_file.exists():
            with open(result_file) as f:
                return json.load(f)
        else:
            print(f"  WARNING: result file not found: {result_file}")
            return None
    except subprocess.TimeoutExpired:
        print(f"  TIMEOUT after 300s")
        return None
    except subprocess.CalledProcessError as e:
        print(f"  ERROR: exit code {e.returncode}")
        return None
    except FileNotFoundError as e:
        print(f"  SKIPPED: command not found — {e}")
        return None


def extract_metrics(poc_id: str, data: dict) -> dict:
    """Extract comparable metrics from any PoC result JSON."""
    bench = data.get("benchmark", {})
    file_info = data.get("file", {})
    return {
        "poc_id":          poc_id,
        "label":           POCS[poc_id]["label"],
        "category":        POCS[poc_id]["category"],
        "line_count":      file_info.get("line_count", "?"),
        "load_ms":         file_info.get("load_ms", "?"),
        "total_frames":    bench.get("total_frames", "?"),
        "dropped_frames":  bench.get("dropped_frames", "?"),
        "drop_rate_pct":   bench.get("drop_rate_pct", "?"),
        "p50_ms":          bench.get("p50_ms", "?"),
        "p95_ms":          bench.get("p95_ms", "?"),
        "p99_ms":          bench.get("p99_ms", "?"),
        "budget_ms":       8.333,
    }


def write_markdown(rows: list[dict], out_path: Path) -> None:
    headers = ["PoC", "Category", "Load ms", "P50 ms", "P95 ms", "P99 ms", "Dropped", "Drop %"]
    lines = [
        "# Text Engine PoC — Benchmark Comparison",
        f"Generated: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
        f"Test file: 100 MB, seed=42 | Frame budget: 8.33 ms (120 Hz) | Scroll: 60px/frame",
        "",
        "| " + " | ".join(headers) + " |",
        "| " + " | ".join(["---"] * len(headers)) + " |",
    ]
    for r in rows:
        def f(v): return f"{v:.2f}" if isinstance(v, float) else str(v)
        lines.append(
            f"| {r['label']} | {r['category']} | {f(r['load_ms'])} | "
            f"{f(r['p50_ms'])} | {f(r['p95_ms'])} | {f(r['p99_ms'])} | "
            f"{r['dropped_frames']} | {f(r['drop_rate_pct'])} |"
        )
    lines += ["", "> Budget: 8.33ms · Dropped = frames > budget"]
    out_path.write_text("\n".join(lines))


def write_csv(rows: list[dict], out_path: Path) -> None:
    if not rows:
        return
    with open(out_path, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)


# ── Main ──────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Run all text-engine-poc benchmarks.")
    parser.add_argument("--poc", nargs="+", default=list(POCS.keys()),
                        help="Which PoCs to run (default: all)")
    parser.add_argument("--file", default=str(TEST_FILE), help="Test file path")
    args = parser.parse_args()

    # Override test file in all commands if specified
    if args.file != str(TEST_FILE):
        abs_file = str(Path(args.file).resolve())
        for k in POCS:
            POCS[k]["cmd"] = [
                c.replace(str(TEST_FILE), abs_file) if str(TEST_FILE) in c else c
                for c in POCS[k]["cmd"]
            ]
            
    # Check test file exists
    test_file = Path(args.file).resolve()
    if not test_file.exists():
        print(f"ERROR: test file not found: {test_file}")
        print("Generate it with:")
        print("  python3 shared/test-data/generate_testfile.py")
        sys.exit(1)

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    # Run selected PoCs
    rows = []
    for poc_id in args.poc:
        if poc_id not in POCS:
            print(f"WARNING: unknown PoC '{poc_id}', skipping")
            continue
        data = run_poc(poc_id, POCS[poc_id])
        if data:
            rows.append(extract_metrics(poc_id, data))

    if not rows:
        print("\nNo results to aggregate.")
        sys.exit(0)

    # Write outputs
    csv_path = RESULTS_DIR / "comparison.csv"
    md_path  = RESULTS_DIR / "comparison.md"
    write_csv(rows, csv_path)
    write_markdown(rows, md_path)

    print(f"\n{'='*60}")
    print("BENCHMARK COMPLETE")
    print(f"  CSV: {csv_path}")
    print(f"  MD:  {md_path}")
    print(f"{'='*60}")
    print(open(md_path).read())
