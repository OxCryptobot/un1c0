#!/usr/bin/env python3
"""Render detailed p95-latency and throughput scaling charts from agent_benchmark.json."""
from __future__ import annotations

import json
from pathlib import Path

import matplotlib.pyplot as plt

plt.rcParams["axes.unicode_minus"] = False

ROOT = Path(__file__).resolve().parent
rows = json.loads((ROOT / "agent_benchmark.json").read_text())
operations = sorted({row["operation"] for row in rows})
concurrencies = sorted({row["concurrency"] for row in rows})
colors = plt.get_cmap("tab10").colors

fig, axes = plt.subplots(2, 1, figsize=(15, 12), sharex=True)
fig.subplots_adjust(left=0.08, right=0.80, top=0.92, bottom=0.10, hspace=0.30)
fig.suptitle("un1c0 performance scaling: p95 latency and throughput", fontsize=16, fontweight="bold")

for index, operation in enumerate(operations):
    series = sorted(
        [row for row in rows if row["operation"] == operation],
        key=lambda row: row["concurrency"],
    )
    color = colors[index % len(colors)]
    x = [row["concurrency"] for row in series]
    p95_ms = [row["p95_ns"] / 1_000_000 for row in series]
    throughput = [row["throughput_ops_per_sec"] for row in series]
    axes[0].plot(x, p95_ms, marker="o", linewidth=2, color=color, label=operation)
    axes[1].plot(x, throughput, marker="o", linewidth=2, color=color, label=operation)

axes[0].set_title("Tail latency under increasing worker concurrency")
axes[0].set_ylabel("p95 latency (ms)")
axes[0].set_yscale("log")
axes[0].grid(True, which="both", alpha=0.25)
axes[0].legend(loc="upper left", bbox_to_anchor=(1.01, 1.0), fontsize=9)

axes[1].set_title("Throughput scaling under increasing worker concurrency")
axes[1].set_xlabel("Worker concurrency")
axes[1].set_ylabel("Throughput (operations/sec)")
axes[1].set_yscale("log")
axes[1].set_xticks(concurrencies)
axes[1].grid(True, which="both", alpha=0.25)

for ax in axes:
    ax.set_xlim(min(concurrencies) * 0.85, max(concurrencies) * 1.15)

fig.text(
    0.08,
    0.025,
    "2,000 samples per operation and concurrency; deterministic local fixtures; zero recorded errors; log axes preserve cross-module scale.",
    fontsize=9,
)
fig.savefig(ROOT / "performance_scaling_analysis.png", dpi=180, bbox_inches="tight")
plt.close(fig)

analysis = {}
for operation in operations:
    series = sorted(
        [row for row in rows if row["operation"] == operation],
        key=lambda row: row["concurrency"],
    )
    baseline = series[0]
    peak = series[-1]
    analysis[operation] = {
        "p95_latency_ms": {str(row["concurrency"]): row["p95_ns"] / 1_000_000 for row in series},
        "throughput_ops_per_sec": {str(row["concurrency"]): row["throughput_ops_per_sec"] for row in series},
        "p95_latency_growth_x": peak["p95_ns"] / max(baseline["p95_ns"], 1),
        "throughput_growth_x": peak["throughput_ops_per_sec"] / max(baseline["throughput_ops_per_sec"], 1e-9),
        "error_count": sum(row["errors"] for row in series),
    }
(ROOT / "performance_scaling_analysis.json").write_text(json.dumps(analysis, indent=2) + "\n")
print(json.dumps({"operations": operations, "concurrencies": concurrencies, "output": "performance_scaling_analysis.png"}, indent=2))
