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

import argparse
import csv
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# ── Paths ────────────────────────────────────────────────────────────────────

ROOT_DIR     = Path(__file__).parent.parent.parent  # monorepo root
RESULTS_DIR  = ROOT_DIR / "results"
TEST_FILE    = ROOT_DIR / "shared" / "test-data" / "test_100mb.txt"
SCHEMA_FILE  = ROOT_DIR / "shared" / "metrics" / "frame_stats.schema.json"

# ── PoC registry ─────────────────────────────────────────────────────────────

POCS = {
    "1a": {
        "label": "PoC 1A — Web DOM (Electron)",
        "cwd":   ROOT_DIR / "poc-1a-web-dom",
        "cmd":   ["node", "benchmarks/synthetic_scroll.js", str(TEST_FILE)],
        # Audit P1/P2: single canonical artifact named after the schema poc_id.
        "result_file": RESULTS_DIR / "1a-web-dom_stats.json",
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
            with open(result_file) as f:
                data = json.load(f)
            # Audit P2: validate against the executable data contract. A PoC
            # that emits a non-conforming report is a hard failure of the
            # comparison, not a silent pass.
            if not validate_report(poc_id, data, result_file):
                return None
            return data
        else:
            print(f"  WARNING: result file not found: {result_file}")
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


# ── Schema validation (audit P2) ─────────────────────────────────────────────

_SCHEMA_CACHE: dict | None = None


def _load_schema() -> dict | None:
    """Load and cache the BenchmarkReport JSON schema. Returns None if the
    `jsonschema` package or the schema file is unavailable (validation is then
    skipped with a warning rather than blocking the run)."""
    global _SCHEMA_CACHE
    if _SCHEMA_CACHE is not None:
        return _SCHEMA_CACHE
    if not SCHEMA_FILE.exists():
        print(f"  WARNING: schema not found at {SCHEMA_FILE}; skipping validation")
        return None
    with open(SCHEMA_FILE) as f:
        _SCHEMA_CACHE = json.load(f)
    return _SCHEMA_CACHE


def validate_report(poc_id: str, data: dict, src: Path) -> bool:
    """Validate a PoC's *_stats.json against frame_stats.schema.json and check
    that its poc_id is the expected one. Returns True if valid (or if the
    validator is unavailable)."""
    try:
        import jsonschema
    except ImportError:
        print("  WARNING: `jsonschema` not installed; skipping schema validation"
              " (pip install jsonschema)")
        return True

    schema = _load_schema()
    if schema is None:
        return True

    try:
        jsonschema.validate(instance=data, schema=schema)
    except jsonschema.ValidationError as e:
        print(f"  SCHEMA ERROR in {src.name}: {e.message}")
        return False

    # The enum check above guarantees poc_id is one of the 8 valid ids; also
    # assert it is the one we expected for this slot.
    if data.get("poc_id") != poc_id_to_schema_id(poc_id):
        print(f"  SCHEMA ERROR in {src.name}: poc_id "
              f"{data.get('poc_id')!r} != expected "
              f"{poc_id_to_schema_id(poc_id)!r}")
        return False
    return True


def poc_id_to_schema_id(poc_id: str) -> str:
    """Map the runner's short key (e.g. '1a') to the schema enum id
    (e.g. '1a-web-dom')."""
    folder = POCS[poc_id]["cwd"].name
    # cwd folder is e.g. "poc-1a-web-dom"; the schema id strips the "poc-" prefix.
    # For 3a/3b the cwd is ROOT, so fall back to a static table.
    static = {
        "3a": "3a-rust-wgpu",
        "3b": "3b-rust-vello",
    }
    if poc_id in static:
        return static[poc_id]
    return folder[len("poc-"):] if folder.startswith("poc-") else folder


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
