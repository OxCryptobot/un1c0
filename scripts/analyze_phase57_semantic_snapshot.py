import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase57_semantic_snapshot.json"
OUTPUT = ROOT / "benchmarks" / "phase57_semantic_snapshot.png"
rows = json.loads(INPUT.read_text())
targets = sorted({row["target"] for row in rows})
colors = {"rust": "#d1495b", "go": "#00798c", "zig": "#edae49", "python": "#30638e"}

fig, axis = plt.subplots(figsize=(8.5, 4.8), dpi=160)
for target in targets:
    target_rows = [row for row in rows if row["target"] == target]
    functions = [row["functions"] for row in target_rows]
    color = colors[target]
    axis.plot(functions, [row["capture_p50_ns"] / 1000 for row in target_rows], marker="o", color=color, label=f"{target} capture")
    axis.plot(functions, [row["verify_p50_ns"] / 1000 for row in target_rows], marker="x", linestyle="--", color=color, label=f"{target} verify")
axis.set_title("Phase 57 semantic snapshot capture versus verification")
axis.set_xlabel("Functions in typed UEG call chain")
axis.set_ylabel("p50 latency (µs)")
axis.set_xticks([1, 2, 4, 8, 16, 32])
axis.grid(True, alpha=0.25)
axis.legend(fontsize=8, ncol=2)
fig.tight_layout()
fig.savefig(OUTPUT, bbox_inches="tight")
print(OUTPUT)
