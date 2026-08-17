#!/usr/bin/env python3
"""Report exact benchmark and repository-search metrics at concurrency eight."""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

def load(name: str):
    return json.loads((ROOT / name).read_text())

baseline = [row for row in load("agent_benchmark.json") if row["concurrency"] == 8]
optimized = [row for row in load("agent_benchmark_optimized.json") if row["concurrency"] == 8]
profile = next(row for row in load("repository_search_profile.json") if row["concurrency"] == 8)
optimized_by_operation = {row["operation"]: row for row in optimized}

summary = {
    "concurrency": 8,
    "operations": [
        {
            "operation": row["operation"],
            "baseline_p95_ms": row["p95_ns"] / 1_000_000,
            "baseline_p99_ms": row["p99_ns"] / 1_000_000,
            "baseline_throughput_ops_per_sec": row["throughput_ops_per_sec"],
            "optimized_p95_ms": optimized_by_operation[row["operation"]]["p95_ns"] / 1_000_000,
            "optimized_p99_ms": optimized_by_operation[row["operation"]]["p99_ns"] / 1_000_000,
            "optimized_throughput_ops_per_sec": optimized_by_operation[row["operation"]]["throughput_ops_per_sec"],
            "baseline_errors": row["errors"],
            "optimized_errors": optimized_by_operation[row["operation"]]["errors"],
        }
        for row in baseline
    ],
    "repository_search_before_after": profile,
}

print(json.dumps(summary, indent=2, sort_keys=True))
