#!/usr/bin/env python3
"""Compare repository-search benchmark rows before and after content caching."""
from __future__ import annotations

import json
import sys
from pathlib import Path


def load_rows(path: Path) -> dict[int, dict]:
    rows = json.loads(path.read_text())
    return {
        row["concurrency"]: row
        for row in rows
        if row["operation"] == "repository_search"
    }


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: profile_repository_search.py BASELINE_JSON OPTIMIZED_JSON")
    baseline = load_rows(Path(sys.argv[1]))
    optimized = load_rows(Path(sys.argv[2]))
    report = []
    for concurrency in sorted(set(baseline) & set(optimized)):
        before = baseline[concurrency]
        after = optimized[concurrency]
        report.append(
            {
                "concurrency": concurrency,
                "baseline_p95_ms": before["p95_ns"] / 1_000_000,
                "optimized_p95_ms": after["p95_ns"] / 1_000_000,
                "p95_reduction_pct": (1 - after["p95_ns"] / before["p95_ns"]) * 100,
                "baseline_throughput_ops_per_sec": before["throughput_ops_per_sec"],
                "optimized_throughput_ops_per_sec": after["throughput_ops_per_sec"],
                "throughput_gain_pct": (after["throughput_ops_per_sec"] / before["throughput_ops_per_sec"] - 1) * 100,
                "baseline_errors": before["errors"],
                "optimized_errors": after["errors"],
            }
        )
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
