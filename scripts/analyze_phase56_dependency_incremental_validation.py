import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase56_dependency_incremental_validation.json"
OUTPUT = ROOT / "benchmarks" / "phase56_dependency_incremental_validation.png"
rows = json.loads(INPUT.read_text())
targets = sorted({row["target"] for row in rows})
colors = {"rust": "#d1495b", "go": "#00798c", "zig": "#edae49", "python": "#30638e"}

fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), dpi=160)
for target in targets:
    target_rows = [row for row in rows if row["target"] == target]
    functions = [row["functions"] for row in target_rows]
    color = colors[target]
    axes[0].plot(functions, [row["full_validation_p50_ns"] / 1000 for row in target_rows], marker="o", color=color, label=f"{target} full")
    axes[0].plot(functions, [row["dependency_incremental_p50_ns"] / 1000 for row in target_rows], marker="x", linestyle="--", color=color, label=f"{target} warm incremental")
    axes[1].plot(functions, [row["affected_function_count"] for row in target_rows], marker="o", color=color, label=target)

axes[0].set_title("Validation p50: full vs dependency-aware warm update")
axes[0].set_ylabel("Latency (µs)")
axes[1].set_title("Changed-leaf affected closure")
axes[1].set_ylabel("Affected functions")
for axis in axes:
    axis.set_xlabel("Functions in typed UEG call chain")
    axis.set_xticks([1, 2, 4, 8, 16, 32])
    axis.grid(True, alpha=0.25)
    axis.legend(fontsize=7, ncol=2)
fig.suptitle("Phase 56 dependency-aware semantic validation (64 samples per point)")
fig.tight_layout()
fig.savefig(OUTPUT, bbox_inches="tight")
print(OUTPUT)
