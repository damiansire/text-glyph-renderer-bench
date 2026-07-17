#!/usr/bin/env python3
"""validate_report.py — Single, reusable gate for a PoC's *_stats.json.

A PoC "emits stats" only if it writes a report that conforms to
`frame_stats.schema.json` and carries the expected `poc_id`. Both the smoke
suite (`test_smoke.py`) and CI (`bench.yml`) call this so there is exactly one
implementation of the contract check.

Usage (CLI, for CI):
    python3 shared/metrics/validate_report.py results/3a-rust-wgpu_stats.json 3a-rust-wgpu

Exit code 0 = conforms; 1 = missing file / invalid JSON / schema violation /
wrong poc_id. Importable too: `validate_report(path, expected_id)` returns a
`(ok: bool, message: str)` tuple and never raises for the expected failures.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCHEMA_FILE = Path(__file__).with_name("frame_stats.schema.json")


def validate_report(path: str | Path, expected_id: str | None = None) -> tuple[bool, str]:
    """Validate one stats file against the schema (and optionally its poc_id).

    Returns (ok, message). Requires the `jsonschema` package; if it is missing
    the check is a hard failure (a gate that can't run is not a passing gate).
    """
    path = Path(path)
    if not path.exists():
        return False, f"stats file not found: {path}"

    try:
        import jsonschema
    except ImportError:
        return False, "`jsonschema` not installed (pip install jsonschema)"

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        return False, f"{path.name}: not valid JSON — {e}"

    schema = json.loads(SCHEMA_FILE.read_text(encoding="utf-8"))
    try:
        jsonschema.validate(instance=data, schema=schema)
    except jsonschema.ValidationError as e:
        return False, f"{path.name}: schema violation — {e.message}"

    if expected_id is not None and data.get("poc_id") != expected_id:
        return False, (
            f"{path.name}: poc_id {data.get('poc_id')!r} != expected {expected_id!r}"
        )

    return True, f"{path.name}: conforms to BenchmarkReport (poc_id={data.get('poc_id')!r})"


def main(argv: list[str]) -> int:
    if not 2 <= len(argv) <= 3:
        print("usage: validate_report.py <stats.json> [expected_poc_id]", file=sys.stderr)
        return 2
    expected = argv[2] if len(argv) == 3 else None
    ok, msg = validate_report(argv[1], expected)
    print(("OK: " if ok else "FAIL: ") + msg)
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
