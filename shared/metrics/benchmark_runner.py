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
    - Node.js 20+   (for PoCs 1A–1D)
    - Xcode 15+     (for PoCs 2A, 2B: swift build)
    - Rust 1.78+    (for PoCs 3A, 3B: cargo build)
    - npm packages installed in each poc-1* directory
    - `jsonschema` (pip install jsonschema): the report gate is fail-closed, so
      without it the run aborts instead of publishing unvalidated numbers.

Output:
    results/comparison.csv
    results/comparison.md

Exit codes:
    0 = every requested PoC produced a valid row.
    1 = partial: at least one requested PoC failed to produce its row.
    2 = nothing ran (no rows at all), or a precondition is missing.
"""

import argparse
import csv
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# The report gate lives in exactly one module; the runner consumes it instead of
# reimplementing it (an earlier duplicate here was fail-open and contradicted it).
sys.path.insert(0, str(Path(__file__).parent))
from validate_report import validate_report  # noqa: E402

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
        # Audit P1/P2: single canonical artifact named after the schema poc_id.
        "result_file": RESULTS_DIR / "1a-web-dom_stats.json",
        # The schema enum id is declared here, not derived from the folder name:
        # deriving it needed a static exception table for 3a/3b, i.e. a third
        # place where the id could drift away from the schema.
        "schema_id": "1a-web-dom",
        "category": "Web",
    },
    "1b": {
        "label": "PoC 1B — Canvas 2D",
        "cwd":   ROOT_DIR / "poc-1b-canvas2d",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1b-canvas2d_stats.json",
        "schema_id": "1b-canvas2d",
        "category": "Web",
    },
    "1c": {
        "label": "PoC 1C — WebGPU Atlas",
        "cwd":   ROOT_DIR / "poc-1c-webgpu-atlas",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1c-webgpu-atlas_stats.json",
        "schema_id": "1c-webgpu-atlas",
        "category": "Web",
    },
    "1d": {
        "label": "PoC 1D — WebGPU MSDF",
        "cwd":   ROOT_DIR / "poc-1d-webgpu-msdf",
        "cmd":   ["npm", "run", "benchmark", "--", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "1d-webgpu-msdf_stats.json",
        "schema_id": "1d-webgpu-msdf",
        "category": "Web",
        # Audit P4: 1D has no index.html and no charset; it never renders nor
        # emits stats. Marked not-implemented so the orchestrator skips it
        # instead of reporting a spurious "result file not found".
        "not_implemented": "missing index.html + MSDF charset (see README)",
    },
    "2a": {
        "label": "PoC 2A — TextKit 2 (Swift)",
        "cwd":   ROOT_DIR / "poc-2a-textkit2",
        "cmd":   ["swift", "run", "-c", "release", "POC2A",
                  "--benchmark", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "2a-textkit2_stats.json",
        "schema_id": "2a-textkit2",
        "category": "Native macOS",
    },
    "2b": {
        "label": "PoC 2B — Metal 3 + CoreText",
        "cwd":   ROOT_DIR / "poc-2b-metal3-coretext",
        "cmd":   ["swift", "run", "-c", "release", "POC2B",
                  "--benchmark", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "2b-metal3-coretext_stats.json",
        "schema_id": "2b-metal3-coretext",
        "category": "Native macOS",
    },
    "3a": {
        "label": "PoC 3A — Rust + wgpu + HarfBuzz",
        "cwd":   ROOT_DIR,
        "cmd":   ["cargo", "run", "--release", "-p", "poc-3a-rust-wgpu", "--",
                  "--bench", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "3a-rust-wgpu_stats.json",
        "schema_id": "3a-rust-wgpu",
        "category": "Systems (Rust)",
    },
    "3b": {
        "label": "PoC 3B — Rust + Vello",
        "cwd":   ROOT_DIR,
        "cmd":   ["cargo", "run", "--release", "-p", "poc-3b-rust-vello", "--",
                  "--bench", "--headless", "--file", str(TEST_FILE)],
        "result_file": RESULTS_DIR / "3b-rust-vello_stats.json",
        "schema_id": "3b-rust-vello",
        "category": "Systems (Rust)",
    },
}

# ── Runner ───────────────────────────────────────────────────────────────────

def run_poc(poc_id: str, info: dict) -> dict | None:
    print(f"\n{'='*60}")
    print(f"Running {info['label']}...")
    print(f"{'='*60}")

    # Audit P4: skip PoCs explicitly declared not-implemented instead of
    # running them and reporting a spurious "result file not found".
    if info.get("not_implemented"):
        print(f"  SKIPPED: not implemented — {info['not_implemented']}")
        return None

    t0 = time.monotonic()
    try:
        # Avoid Electron environment contamination.
        env = os.environ.copy()
        env.pop("ELECTRON_RUN_AS_NODE", None)
        # Audit P1: every PoC writes its *_stats.json into ONE canonical
        # directory (absolute ROOT/results), regardless of its own cwd. Each
        # PoC reads BENCH_RESULTS_DIR; without this the Web PoCs wrote into
        # poc-X/results (relative to their cwd) and the runner never found them.
        env["BENCH_RESULTS_DIR"] = str(RESULTS_DIR)

        subprocess.run(
            info["cmd"],
            cwd=str(info["cwd"]),
            env=env,
            timeout=300,  # 5 minutes max per PoC
            check=True,
            capture_output=False,
        )
        elapsed = time.monotonic() - t0
        print(f"  Completed in {elapsed:.1f}s")

        # Read result JSON.
        result_file = info["result_file"]
        if result_file.exists():
            # Audit P2: validate against the executable data contract. A PoC
            # that emits a non-conforming report is a hard failure of the
            # comparison, not a silent pass. The gate is the shared module, so
            # its fail-closed semantics (missing `jsonschema` = failure) apply
            # here too.
            ok, msg = validate_report(result_file, info["schema_id"])
            if not ok:
                print(f"  SCHEMA ERROR: {msg}")
                return None
            with open(result_file, encoding="utf-8") as f:
                return json.load(f)
        else:
            print(f"  ERROR: result file not found: {result_file}")
            return None
    except subprocess.TimeoutExpired:
        print("  TIMEOUT after 300s")
        return None
    except subprocess.CalledProcessError as e:
        print(f"  ERROR: exit code {e.returncode}")
        return None
    except FileNotFoundError as e:
        print(f"  SKIPPED: command not found — {e}")
        return None


def extract_metrics(poc_id: str, data: dict) -> dict:
    """Extract comparable metrics from any PoC result JSON.

    Audit P1/P6: PoCs emit two distinct shapes. End-to-end PoCs (1a/1b/1c and
    now 3a — which runs a real offscreen GPU render per frame, tgrb-4) emit a
    `benchmark` block with percentiles already in ms. PoC 3b is a CPU
    microbenchmark that deliberately does NOT report a frame-budget verdict; it
    emits a `scene_build` block in µs. We normalise both into the same row,
    converting µs→ms, and leave drop_rate as 'n/a' for the microbenchmark so the
    table never implies a frame-budget comparison it did not measure. The legacy
    3a `line_index_traversal` shape (pre-tgrb-4) is still accepted below for
    backward-compatibility with older reports.
    """
    label    = POCS[poc_id]["label"]
    category = POCS[poc_id]["category"]

    def us_to_ms(v):
        return round(v / 1000.0, 3) if isinstance(v, (int, float)) else "?"

    # ── Rust microbenchmarks: µs blocks, no frame-budget verdict ──────────
    if "line_index_traversal" in data:  # 3a
        mb = data["line_index_traversal"]
        return {
            "poc_id": poc_id, "label": label, "category": category,
            "line_count": data.get("file_lines", "?"),
            "load_ms": data.get("load_ms", "?"),
            "total_frames": mb.get("total_iters", "?"),
            "dropped_frames": "n/a", "drop_rate_pct": "n/a",
            "p50_ms": us_to_ms(mb.get("p50_us")),
            "p95_ms": us_to_ms(mb.get("p95_us")),
            "p99_ms": us_to_ms(mb.get("p99_us")),
            "budget_ms": 8.333,
        }
    if "scene_build" in data:  # 3b
        mb = data["scene_build"]
        file_info = data.get("file", {})
        return {
            "poc_id": poc_id, "label": label, "category": category,
            "line_count": file_info.get("line_count", "?"),
            "load_ms": file_info.get("load_ms", "?"),
            "total_frames": mb.get("total_iters", "?"),
            "dropped_frames": "n/a", "drop_rate_pct": "n/a",
            # 3b already exposes p*_ms numbers alongside the µs values.
            "p50_ms": mb.get("p50_ms", us_to_ms(mb.get("p50_us"))),
            "p95_ms": mb.get("p95_ms", us_to_ms(mb.get("p95_us"))),
            "p99_ms": mb.get("p99_ms", us_to_ms(mb.get("p99_us"))),
            "budget_ms": 8.333,
        }

    # ── End-to-end PoCs: `benchmark` block, percentiles already in ms ─────
    bench = data.get("benchmark", {})
    file_info = data.get("file", {})
    return {
        "poc_id":          poc_id,
        "label":           label,
        "category":        category,
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
        "Test file: 100 MB, seed=42 | Frame budget: 8.33 ms (120 Hz) | Scroll: 60px/frame",
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
        sys.exit(2)

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    # Run selected PoCs. A PoC that was asked for and did not produce a valid
    # row is a failure of the comparison, not a footnote: the exit code has to
    # say so, otherwise a cron/CI job cannot tell "the 8 ran" from "the 8 died"
    # while a stale comparison.csv sits on disk looking current.
    rows = []
    failed: list[str] = []
    for poc_id in args.poc:
        if poc_id not in POCS:
            print(f"ERROR: unknown PoC '{poc_id}'")
            failed.append(poc_id)
            continue
        data = run_poc(poc_id, POCS[poc_id])
        if data:
            rows.append(extract_metrics(poc_id, data))
        elif not POCS[poc_id].get("not_implemented"):
            # A declared not-implemented PoC is a deliberate gap, not a failure.
            failed.append(poc_id)

    if not rows:
        print("\nNo results to aggregate.")
        if failed:
            print(f"FAILED PoCs: {', '.join(failed)}")
        sys.exit(2)

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
    print(open(md_path, encoding="utf-8").read())

    if failed:
        print(f"\nPARTIAL RUN: {len(failed)} PoC(s) produced no valid row: "
              f"{', '.join(failed)}")
        sys.exit(1)
