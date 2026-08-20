#!/usr/bin/env python3
import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase47_memory_profile_metrics.json"
OUTPUT_JSON = ROOT / "benchmarks" / "phase47_memory_profile_analysis.json"
OUTPUT_PNG = ROOT / "benchmarks" / "phase47_memory_profile.png"

with INPUT.open() as handle:
    artifact = json.load(handle)

rows = artifact["results"]
if not rows:
    raise SystemExit("benchmark has no result rows")

base = rows[0]
analysis_rows = []
for row in rows:
    jobs = row["jobs"]
    analysis_rows.append(
        {
            "producers": row["producers"],
            "jobs": jobs,
            "peak_rss_kb": row["peak_rss_kb"],
            "peak_hwm_kb": row["peak_hwm_kb"],
            "peak_vm_peak_kb": row["peak_vm_peak_kb"],
            "peak_threads": row["peak_threads"],
            "rss_per_producer_kb": row["peak_rss_kb"] / row["producers"],
            "hwm_per_producer_kb": row["peak_hwm_kb"] / row["producers"],
            "vm_peak_per_producer_kb": row["peak_vm_peak_kb"] / row["producers"],
            "limiter_retries_per_job": row["limiter_retries"] / jobs,
            "verification_cache_hit_ratio": row["verifier"]["verification_cache_hits"]
            / max(1, row["verifier"]["verification_cache_hits"] + row["verifier"]["verification_cache_misses"]),
            "verification_service_p95_us": row["verifier"]["verification_service_p95_us"],
            "end_to_end_p95_us": row["verifier"]["end_to_end_p95_us"],
            "successful_commits": row["successful_commits"],
            "failed_outcomes": row["failed_outcomes"],
        }
    )

last = rows[-1]
summary = {
    "measurement_scope": "local sanitized Rust process benchmark; not a production capacity claim",
    "gc_interpretation": "Rust has no tracing GC in this path; allocator pressure is inferred from RSS/high-water, virtual-memory reservation, thread count, and bounded retry/bookkeeping activity",
    "producer_levels": [row["producers"] for row in rows],
    "rss_growth_32_to_96_kb": last["peak_rss_kb"] - base["peak_rss_kb"],
    "hwm_growth_32_to_96_kb": last["peak_hwm_kb"] - base["peak_hwm_kb"],
    "vm_peak_growth_32_to_96_kb": last["peak_vm_peak_kb"] - base["peak_vm_peak_kb"],
    "thread_growth_32_to_96": last["peak_threads"] - base["peak_threads"],
    "rss_growth_factor_32_to_96": last["peak_rss_kb"] / base["peak_rss_kb"],
    "hwm_growth_factor_32_to_96": last["peak_hwm_kb"] / base["peak_hwm_kb"],
    "vm_peak_growth_factor_32_to_96": last["peak_vm_peak_kb"] / base["peak_vm_peak_kb"],
    "cache_hit_ratio_at_96": analysis_rows[-1]["verification_cache_hit_ratio"],
    "limiter_retries_per_job_at_96": analysis_rows[-1]["limiter_retries_per_job"],
    "successful_commits_at_96": last["successful_commits"],
    "failed_outcomes_at_96": last["failed_outcomes"],
    "caveat": "The hot-key benchmark intentionally reuses one CAS generation, so after the first valid commit subsequent outcomes are expected conflicts; failure count is not an allocator or protocol-crash count.",
}

result = {"summary": summary, "rows": analysis_rows}
with OUTPUT_JSON.open("w") as handle:
    json.dump(result, handle, indent=2)
    handle.write("\n")

producers = [row["producers"] for row in analysis_rows]
fig, axes = plt.subplots(1, 3, figsize=(14, 4.5), constrained_layout=True)
fig.suptitle("Phase 47 sustained producer memory and contention profile", fontsize=14, fontweight="bold")
axes[0].plot(producers, [row["peak_rss_kb"] for row in analysis_rows], marker="o", label="Peak RSS")
axes[0].plot(producers, [row["peak_hwm_kb"] for row in analysis_rows], marker="o", label="Peak RSS high-water")
axes[0].set_title("Resident memory")
axes[0].set_xlabel("Producer threads")
axes[0].set_ylabel("KiB")
axes[0].legend(frameon=False)
axes[0].grid(alpha=0.25)
axes[1].plot(producers, [row["peak_vm_peak_kb"] for row in analysis_rows], marker="o", color="#9b59b6", label="VmPeak")
axes[1].plot(producers, [row["peak_threads"] for row in analysis_rows], marker="o", color="#e67e22", label="Peak threads")
axes[1].set_title("Address-space and thread pressure")
axes[1].set_xlabel("Producer threads")
axes[1].set_ylabel("KiB / threads")
axes[1].legend(frameon=False)
axes[1].grid(alpha=0.25)
axes[2].plot(producers, [row["limiter_retries_per_job"] for row in analysis_rows], marker="o", color="#c0392b", label="Limiter retries/job")
axes[2].plot(producers, [row["verification_cache_hit_ratio"] for row in analysis_rows], marker="o", color="#16a085", label="Cache hit ratio")
axes[2].set_title("Bookkeeping and cache behavior")
axes[2].set_xlabel("Producer threads")
axes[2].set_ylabel("Ratio / retries per job")
axes[2].legend(frameon=False)
axes[2].grid(alpha=0.25)
fig.savefig(OUTPUT_PNG, dpi=160)
print(json.dumps({"analysis": str(OUTPUT_JSON), "plot": str(OUTPUT_PNG), "summary": summary}, indent=2))
