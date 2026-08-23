#!/usr/bin/env python3
"""Validate the sanitized Phase 77 diagnostic-worker tail artifact."""

from __future__ import annotations

import json
import sys
from pathlib import Path

EXPECTED_WORKERS = {1, 2, 4, 8}
EXPECTED_JOBS = {1, 4, 8, 16}
EXPECTED_ROWS = len(EXPECTED_WORKERS) * len(EXPECTED_JOBS)
EXPECTED_SAMPLES = 17


def main() -> int:
    artifact_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("benchmarks/phase77_worker_tail.json")
    data = json.loads(artifact_path.read_text())
    rows = data.get("rows", [])
    assert data.get("schema_version") == 1
    assert data.get("phase") == 77
    assert data.get("artifact") == "diagnostic_worker_tail_latency"
    assert data.get("secret_material_recorded") is False
    assert data.get("errors") == 0
    assert len(rows) == EXPECTED_ROWS

    observed = {(row["worker_count"], row["job_count"]) for row in rows}
    assert observed == {(workers, jobs) for workers in EXPECTED_WORKERS for jobs in EXPECTED_JOBS}
    for row in rows:
        assert row["errors"] == 0
        assert row["sample_count"] == EXPECTED_SAMPLES
        assert row["submitted_jobs"] == row["job_count"]
        assert row["completed_jobs"] == row["job_count"]
        assert row["failed_jobs"] == 0
        assert row["cancelled_jobs"] == 0
        assert row["queue_full_rejections"] == 0
        assert row["fairness_rejections"] == 0
        assert row["end_to_end_p95_us"] >= row["end_to_end_p50_us"]
        assert row["end_to_end_p99_us"] >= row["end_to_end_p95_us"]
        assert row["end_to_end_max_us"] >= row["end_to_end_p99_us"]
        assert row["throughput_jobs_per_sec"] > 0

    print(
        "phase77_tail_gate=pass "
        f"rows={len(rows)} errors={data['errors']} "
        f"workers={sorted(EXPECTED_WORKERS)} jobs={sorted(EXPECTED_JOBS)} "
        f"secret_material_recorded={data['secret_material_recorded']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
