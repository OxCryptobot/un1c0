import json
from pathlib import Path

import matplotlib.pyplot as plt

artifact = Path("benchmarks/phase44_ownership_bound_cas_executor_metrics.json")
output = Path("benchmarks/phase44_executor_scaling.png")
report = json.loads(artifact.read_text(encoding="utf-8"))
rows = report["results"]
producers = [row["producers"] for row in rows]
throughput = [row["throughput_intents_per_sec"] for row in rows]
queue_p95 = [row["queue_wait_p95_us"] for row in rows]
end_to_end_p95 = [row["end_to_end_p95_us"] for row in rows]

plt.style.use("seaborn-v0_8-whitegrid")
fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), dpi=180)
fig.suptitle("Phase 44 bounded ownership-bound CAS executor scaling", fontsize=14, fontweight="bold")

axes[0].plot(producers, throughput, marker="o", linewidth=2.2, color="#1f5f8b")
axes[0].set_title("Contention workload throughput")
axes[0].set_xlabel("Concurrent producers")
axes[0].set_ylabel("Completed intents / second")
axes[0].set_xscale("log", base=2)
axes[0].set_xticks(producers)
axes[0].set_xticklabels([str(value) for value in producers])

axes[1].plot(producers, queue_p95, marker="o", linewidth=2.2, label="Queue wait p95", color="#c55a11")
axes[1].plot(producers, end_to_end_p95, marker="o", linewidth=2.2, label="End-to-end p95", color="#7f3c8d")
axes[1].set_title("Tail latency under contention")
axes[1].set_xlabel("Concurrent producers")
axes[1].set_ylabel("Microseconds")
axes[1].set_xscale("log", base=2)
axes[1].set_xticks(producers)
axes[1].set_xticklabels([str(value) for value in producers])
axes[1].legend(frameon=True)

fig.text(
    0.01,
    0.01,
    "Sanitized local benchmark; one worker owns mutable CAS state; same-generation conflicts fail closed.",
    fontsize=8,
)
fig.tight_layout(rect=(0, 0.04, 1, 0.95))
output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(output, bbox_inches="tight")
