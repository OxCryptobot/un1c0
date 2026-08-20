import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase45_ownership_bound_cas_verifier_metrics.json"
OUTPUT = ROOT / "benchmarks" / "phase45_latency_breakdown.png"

with INPUT.open(encoding="utf-8") as handle:
    report = json.load(handle)
rows = report["results"]
producers = [row["producers"] for row in rows]
wait = [row["verification_wait_p95_us"] / 1000 for row in rows]
verification = [row["verification_service_p95_us"] / 1000 for row in rows]
mutation = [row["mutation_service_p95_us"] / 1000 for row in rows]
end_to_end = [row["end_to_end_p95_us"] / 1000 for row in rows]

plt.style.use("seaborn-v0_8-whitegrid")
fig, axis = plt.subplots(figsize=(10, 5.8), dpi=160)
axis.plot(producers, wait, marker="o", linewidth=2.2, label="verification wait p95")
axis.plot(producers, verification, marker="o", linewidth=2.2, label="verification service p95")
axis.plot(producers, mutation, marker="o", linewidth=2.2, label="mutation service p95")
axis.plot(producers, end_to_end, marker="o", linewidth=2.6, label="end-to-end p95")
axis.set_title("Phase 45 verifier latency breakdown")
axis.set_xlabel("Producer count")
axis.set_ylabel("p95 latency (ms)")
axis.set_xticks(producers)
axis.legend(loc="upper left", frameon=True)
axis.annotate(
    "16 producers: verification wait dominates tail",
    xy=(16, wait[-1]),
    xytext=(9.5, max(end_to_end) * 0.7),
    arrowprops={"arrowstyle": "->", "color": "#334155"},
    fontsize=9,
)
fig.tight_layout()
fig.savefig(OUTPUT, bbox_inches="tight")
print(OUTPUT)
