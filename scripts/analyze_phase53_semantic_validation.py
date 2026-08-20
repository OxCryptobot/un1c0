import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase53_semantic_validation.json"
OUTPUT = ROOT / "benchmarks" / "phase53_semantic_validation.png"

rows = json.loads(INPUT.read_text())
targets = sorted({row["target"] for row in rows})
fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), dpi=160)
colors = {"rust": "#d1495b", "go": "#00798c", "zig": "#edae49", "python": "#30638e"}

for target in targets:
    target_rows = [row for row in rows if row["target"] == target]
    functions = [row["functions"] for row in target_rows]
    p50 = [row["p50_ns"] / 1000 for row in target_rows]
    p95 = [row["p95_ns"] / 1000 for row in target_rows]
    axes[0].plot(functions, p50, marker="o", label=f"{target} p50", color=colors[target])
    axes[1].plot(functions, p95, marker="o", label=f"{target} p95", color=colors[target])

for axis, title in zip(axes, ["Median semantic validation", "p95 semantic validation"]):
    axis.set_title(title)
    axis.set_xlabel("Functions in parsed UEG")
    axis.set_ylabel("Validation latency (µs)")
    axis.set_xticks([1, 2, 4, 8, 16, 32])
    axis.grid(True, alpha=0.25)
    axis.legend(fontsize=8, ncol=2)

fig.suptitle("Phase 53 typed semantic validation scaling (96 samples per point)")
fig.tight_layout()
fig.savefig(OUTPUT, bbox_inches="tight")
print(OUTPUT)
