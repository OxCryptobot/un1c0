#!/usr/bin/env python3
"""Validate the sanitized Phase 80 durable-outbox sync comparison artifact."""

from __future__ import annotations

import json
import sys
from pathlib import Path

EXPECTED_MODES = {"durable_sync", "no_sync_benchmark_only"}
EXPECTED_BATCHES = {4, 8, 16}


def fail(message: str) -> None:
    raise SystemExit(f"validation_failed: {message}")


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: validate_phase80_outbox_sync.py ARTIFACT.json")
    path = Path(sys.argv[1])
    try:
        artifact = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read artifact: {error}")

    if artifact.get("schema_version") != 1:
        fail("schema_version must be 1")
    if artifact.get("phase") != 80:
        fail("phase must be 80")
    if artifact.get("artifact") != "diagnostic_outbox_sync_comparison":
        fail("unexpected artifact type")
    if artifact.get("samples") != 11:
        fail("expected 11 repeated samples")
    if artifact.get("batch_sizes") != [4, 8, 16]:
        fail("unexpected batch-size matrix")
    if artifact.get("errors") != 0:
        fail("artifact reports errors")
    if artifact.get("secret_material_recorded") is not False:
        fail("secret material marker must be false")

    rows = artifact.get("modes")
    if not isinstance(rows, list) or len(rows) != 6:
        fail("expected six mode/batch rows")
    seen = set()
    for row in rows:
        key = (row.get("mode"), row.get("batch"))
        if key in seen:
            fail(f"duplicate row: {key}")
        seen.add(key)
        if row.get("mode") not in EXPECTED_MODES:
            fail(f"unexpected mode: {row.get('mode')}")
        if row.get("batch") not in EXPECTED_BATCHES:
            fail(f"unexpected batch: {row.get('batch')}")
        if row.get("samples") != 11:
            fail(f"row {key} has incomplete sampling")
        if row.get("errors") != 0:
            fail(f"row {key} reports errors")
        if row.get("submitted_per_trial") != row.get("batch"):
            fail(f"row {key} submitted counter mismatch")
        if row.get("accepted_per_trial") != row.get("batch"):
            fail(f"row {key} accepted counter mismatch")
        metrics = [row.get(name) for name in ("p50_us", "p95_us", "p99_us", "max_us")]
        if any(not isinstance(value, int) or value < 0 for value in metrics):
            fail(f"row {key} has invalid latency metrics")
        if metrics != sorted(metrics):
            fail(f"row {key} latency percentiles are not monotonic")
        if not isinstance(row.get("median_throughput_ops_per_sec"), int) or row["median_throughput_ops_per_sec"] <= 0:
            fail(f"row {key} has invalid throughput")

    expected = {(mode, batch) for mode in EXPECTED_MODES for batch in EXPECTED_BATCHES}
    if seen != expected:
        fail(f"matrix mismatch: {sorted(seen)}")
    print(f"phase80_outbox_sync_validation=pass rows={len(rows)} samples_per_row=11 errors=0")


if __name__ == "__main__":
    main()
