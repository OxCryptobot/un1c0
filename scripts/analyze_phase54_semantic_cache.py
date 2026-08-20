import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase54_semantic_cache.json"
OUTPUT = ROOT / "benchmarks" / "phase54_semantic_cache.png"
rows = json.loads(INPUT.read_text())
targets = sorted({row["target"] for row in rows})
colors = {"rust": "#d1495b", "go": "#00798c", "zig": "#edae49", "python": "#30638e"}

fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), dpi=160)
for target in targets:
    target_rows = [row for row in rows if row["target"] == target]
    functions = [row["functions"] for row in target_rows]
    axes[0].plot(functions, [row["uncached_p50_ns"] / 1000 for row in target_rows], marker="o", color=colors[target], label=f"{target} uncached")
    axes[0].plot(functions, [row["cached_p50_ns"] / 1000 for row in target_rows], marker="x", linestyle="--", color=colors[target], label=f"{target} cache hit")
    axes[1].plot(functions, [row["key_p50_ns"] / 1000 for row in target_rows], marker="o", color=colors[target], label=f"{target} key derivation")

axes[0].set_title("Validation p50: uncached vs prepared-key hit")
axes[1].set_title("Fingerprint key derivation p50")
for axis in axes:
    axis.set_xlabel("Functions in typed UEG")
    axis.set_ylabel("Latency (µs)")
    axis.set_xticks([1, 2, 4, 8, 16, 32])
    axis.grid(True, alpha=0.25)
    axis.legend(fontsize=7, ncol=2)
fig.suptitle("Phase 54 semantic-cache performance (128 samples per point)")
fig.tight_layout()
fig.savefig(OUTPUT, bbox_inches="tight")
print(OUTPUT)
