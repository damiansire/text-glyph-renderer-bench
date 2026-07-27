#!/usr/bin/env python3
"""test_smoke.py — One smoke test per PoC that emits stats.

Each test drives a PoC's benchmark on a tiny corpus and asserts it writes a
report that conforms to `frame_stats.schema.json` with the right `poc_id`
(the "does it run and emit the expected metrics schema?" gate).

Coverage boundary — read this before assuming a green run means all 8 stacks
were exercised locally:

  Runnable headlessly (executed here / in `ci.yml`):
    - 1a-web-dom : Node standalone path (`synthetic_scroll.js`), no Electron.
    - 3a-rust-wgpu / 3b-rust-vello : `cargo run -- --bench`, opt-in via
      SMOKE_RUST=1 (compiling wgpu/vello is slow); ALSO run for real on three
      OSes by `.github/workflows/bench.yml`, which validates their stats with
      the same `validate_report.py`.

  No headless stats path (skipped here, with reason):
    - 1b-canvas2d / 1c-webgpu-atlas : benchmark is `electron . --benchmark`,
      i.e. it only emits inside an Electron GUI window; there is no Node path.
    - 2a-textkit2 / 2b-metal3-coretext : Swift + TextKit 2 / Metal 3, macOS-only.

  Emits no stats by design:
    - 1d-webgpu-msdf : declared `not_implemented` in the runner (needs the
      offline-generated MSDF atlas); the runner skips it instead of expecting a
      report. Asserted below so that contract can't silently rot.

Run:  python -m unittest shared/metrics/test_smoke.py -v
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from validate_report import validate_report  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent.parent
GEN = ROOT / "shared" / "test-data" / "generate_testfile.py"

# Populated by setUpModule.
_TMP: tempfile.TemporaryDirectory | None = None
_CORPUS: Path | None = None
_RESULTS: Path | None = None


def setUpModule() -> None:
    global _TMP, _CORPUS, _RESULTS
    _TMP = tempfile.TemporaryDirectory(prefix="tgrb-smoke-")
    base = Path(_TMP.name)
    _CORPUS = base / "tiny.txt"
    _RESULTS = base / "results"
    _RESULTS.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [sys.executable, str(GEN), "--size", "1", "--output", str(_CORPUS)],
        check=True,
        capture_output=True,
    )


def tearDownModule() -> None:
    if _TMP is not None:
        _TMP.cleanup()


def _bench_env() -> dict[str, str]:
    env = os.environ.copy()
    env["BENCH_RESULTS_DIR"] = str(_RESULTS)
    env.pop("ELECTRON_RUN_AS_NODE", None)
    return env


def _write_report(name: str, payload: dict) -> Path:
    import json

    path = Path(_TMP.name) / name  # type: ignore[union-attr]
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


# Minimal reports in the exact shape each family of PoCs emits. They are the
# fixtures for "what the contract must accept", the counterpart of the
# bare-poc_id report the contract must reject.
_END_TO_END = {
    "poc_id": "1a-web-dom",
    "file": {"line_count": 100, "load_ms": 3.2},
    "benchmark": {
        "total_frames": 200, "dropped_frames": 1, "drop_rate_pct": 0.5,
        "p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 3.0, "budget_ms": 8.333,
    },
}
_SCENE_BUILD = {
    "poc_id": "3b-rust-vello",
    "measures": "cpu-scene-build-only",
    "file": {"line_count": 100, "load_ms": 1},
    "scene_build": {"total_iters": 50, "p50_us": 10, "p95_us": 20, "p99_us": 30},
}
_NO_GPU = {
    "poc_id": "3a-rust-wgpu",
    "measures": "real-gpu-render",
    "gpu_available": False,
    "file": {"line_count": 100, "load_ms": 0},
}


class SchemaContract(unittest.TestCase):
    def test_schema_is_valid_draft7(self) -> None:
        import json

        import jsonschema

        schema = json.loads(
            (ROOT / "shared" / "metrics" / "frame_stats.schema.json").read_text("utf-8")
        )
        jsonschema.Draft7Validator.check_schema(schema)

    def test_report_without_any_metric_is_rejected(self) -> None:
        # The gate's whole claim is "a bench ran only if it emitted metrics".
        # A bare poc_id used to pass it, which made the claim false.
        path = _write_report("bare.json", {"poc_id": "3a-rust-wgpu"})
        ok, msg = validate_report(path, "3a-rust-wgpu")
        self.assertFalse(ok, f"a report with no metrics must be rejected, got: {msg}")

    def test_report_with_empty_benchmark_block_is_rejected(self) -> None:
        path = _write_report(
            "empty-bench.json",
            {"poc_id": "1a-web-dom", "file": {"line_count": 1}, "benchmark": {}},
        )
        ok, msg = validate_report(path, "1a-web-dom")
        self.assertFalse(ok, f"an empty benchmark block must be rejected, got: {msg}")

    def test_each_real_report_shape_is_accepted(self) -> None:
        for name, payload in (
            ("end-to-end", _END_TO_END),
            ("scene-build", _SCENE_BUILD),
            ("no-gpu", _NO_GPU),
        ):
            with self.subTest(shape=name):
                path = _write_report(f"{name}.json", payload)
                ok, msg = validate_report(path, payload["poc_id"])
                self.assertTrue(ok, msg)


class ReportGateIsSingleImplementation(unittest.TestCase):
    """The docstring of validate_report.py promises exactly one implementation
    of the contract check. This asserts it instead of trusting the prose."""

    def test_runner_reuses_the_shared_gate(self) -> None:
        import benchmark_runner
        import validate_report as gate

        self.assertIs(benchmark_runner.validate_report, gate.validate_report)

    def test_every_poc_declares_its_schema_id(self) -> None:
        import json

        import benchmark_runner

        schema = json.loads(
            (ROOT / "shared" / "metrics" / "frame_stats.schema.json").read_text("utf-8")
        )
        valid_ids = set(schema["properties"]["poc_id"]["enum"])
        for key, info in benchmark_runner.POCS.items():
            with self.subTest(poc=key):
                self.assertIn(info["schema_id"], valid_ids)


class WebPocsSmoke(unittest.TestCase):
    @unittest.skipUnless(shutil.which("node"), "node not installed")
    def test_1a_web_dom_runs_headless_and_conforms(self) -> None:
        subprocess.run(
            [
                "node",
                str(ROOT / "poc-1a-web-dom" / "benchmarks" / "synthetic_scroll.js"),
                str(_CORPUS),
                "60",
                "200",
            ],
            cwd=str(ROOT),
            env=_bench_env(),
            check=True,
            capture_output=True,
        )
        ok, msg = validate_report(_RESULTS / "1a-web-dom_stats.json", "1a-web-dom")
        self.assertTrue(ok, msg)

    def test_1b_canvas2d_has_no_headless_stats_path(self) -> None:
        self.skipTest(
            "1b-canvas2d emits stats only inside Electron (electron . --benchmark); "
            "no Node/headless path — run manually or on a GUI runner."
        )

    def test_1c_webgpu_atlas_has_no_headless_stats_path(self) -> None:
        self.skipTest(
            "1c-webgpu-atlas emits stats only inside Electron + a WebGPU adapter; "
            "no Node/headless path — run manually or on a GUI runner."
        )


class NativeMacOsPocsSmoke(unittest.TestCase):
    def test_2a_textkit2_is_macos_only(self) -> None:
        self.skipTest("2a-textkit2 is Swift + TextKit 2 (macOS-only); no cross-platform path.")

    def test_2b_metal3_coretext_is_macos_only(self) -> None:
        self.skipTest("2b-metal3-coretext is Swift + Metal 3 (macOS-only); no cross-platform path.")


class RustPocsSmoke(unittest.TestCase):
    """Opt-in: compiling wgpu/vello is slow, so gate on SMOKE_RUST=1. These same
    PoCs are run for real on ubuntu/windows/macos by bench.yml, which validates
    their reports with validate_report.py — so CI already gates them even when
    this local opt-in is off."""

    def _run_cargo_bench(self, crate: str, poc_id: str, extra: list[str]) -> None:
        if os.environ.get("SMOKE_RUST") != "1":
            self.skipTest("set SMOKE_RUST=1 to run the Rust cargo smoke (slow build); "
                          "bench.yml gates these on CI.")
        if not shutil.which("cargo"):
            self.skipTest("cargo not installed")
        subprocess.run(
            ["cargo", "run", "--release", "-p", crate, "--",
             "--file", str(_CORPUS), "--bench", "--frames", "100", *extra],
            cwd=str(ROOT),
            env=_bench_env(),
            check=True,
        )
        ok, msg = validate_report(_RESULTS / f"{poc_id}_stats.json", poc_id)
        self.assertTrue(ok, msg)

    def test_3a_rust_wgpu(self) -> None:
        # No GPU adapter (headless CI) is fine: 3a writes a conforming
        # gpu_available:false report and exits 0.
        self._run_cargo_bench("poc-3a-rust-wgpu", "3a-rust-wgpu", [])

    def test_3b_rust_vello(self) -> None:
        self._run_cargo_bench(
            "poc-3b-rust-vello", "3b-rust-vello",
            ["--headless", "--font", str(ROOT / "shared" / "fonts" / "InterVariable.ttf")],
        )


class NotImplementedContract(unittest.TestCase):
    def test_1d_webgpu_msdf_is_declared_not_implemented(self) -> None:
        # 1d intentionally emits no stats; the runner must keep skipping it
        # instead of reporting a spurious "result file not found".
        import benchmark_runner

        self.assertIn("not_implemented", benchmark_runner.POCS["1d"])


if __name__ == "__main__":
    unittest.main()
