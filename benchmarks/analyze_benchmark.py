#!/usr/bin/env python3
"""Analyze deterministic un1c0 benchmark output and render stakeholder plots."""
from __future__ import annotations

import csv
import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parent
rows = json.loads((ROOT / "agent_benchmark.json").read_text())

summary_path = ROOT / "agent_benchmark_summary.csv"
with summary_path.open("w", newline="") as handle:
    writer = csv.DictWriter(
        handle,
        lineterminator="\n",
        fieldnames=[
            "operation",
            "concurrency",
            "samples",
            "errors",
            "elapsed_ms",
            "throughput_ops_per_sec",
            "p50_ns",
            "p95_ns",
            "p99_ns",
        ],
    )
    writer.writeheader()
    writer.writerows(rows)

operations = sorted({row["operation"] for row in rows})
concurrencies = sorted({row["concurrency"] for row in rows})

plt.figure(figsize=(12, 7))
for operation in operations:
    series = [
        next(row for row in rows if row["operation"] == operation and row["concurrency"] == concurrency)
        for concurrency in concurrencies
    ]
    plt.plot(concurrencies, [row["p95_ns"] / 1_000 for row in series], marker="o", label=operation)
plt.xlabel("Worker concurrency")
plt.ylabel("p95 latency (µs)")
plt.title("un1c0 architecture p95 latency under controlled concurrency")
plt.grid(True, alpha=0.25)
plt.legend(fontsize=8, ncol=2)
plt.tight_layout()
plt.savefig(ROOT / "latency_p95_by_concurrency.png", dpi=160)
plt.close()

plt.figure(figsize=(12, 7))
for operation in operations:
    series = [
        next(row for row in rows if row["operation"] == operation and row["concurrency"] == concurrency)
        for concurrency in concurrencies
    ]
    plt.plot(concurrencies, [row["throughput_ops_per_sec"] for row in series], marker="o", label=operation)
plt.xlabel("Worker concurrency")
plt.ylabel("Throughput (operations/sec)")
plt.title("un1c0 architecture throughput under controlled concurrency")
plt.grid(True, alpha=0.25)
plt.yscale("log")
plt.legend(fontsize=8, ncol=2)
plt.tight_layout()
plt.savefig(ROOT / "throughput_by_concurrency.png", dpi=160)
plt.close()

baseline = {row["operation"]: row for row in rows if row["concurrency"] == concurrencies[0]}
peak = {row["operation"]: row for row in rows if row["concurrency"] == concurrencies[-1]}
report = {
    "operations": operations,
    "concurrencies": concurrencies,
    "baseline_concurrency": concurrencies[0],
    "peak_concurrency": concurrencies[-1],
    "zero_error_rows": all(row["errors"] == 0 for row in rows),
    "baseline": baseline,
    "peak": peak,
}
(ROOT / "benchmark_analysis.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({"rows": len(rows), "operations": operations, "concurrencies": concurrencies}, indent=2))
