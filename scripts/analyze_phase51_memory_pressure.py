#!/usr/bin/env python3
"""Analyze Phase 51 high-concurrency pressure and lock-free pool evidence."""
from __future__ import annotations

import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
BASELINE_PATH = ROOT / "benchmarks" / "phase51_memory_profile_baseline.json"
POOL_PATH = ROOT / "benchmarks" / "phase51_buffer_pool_metrics.json"
ANALYSIS_PATH = ROOT / "benchmarks" / "phase51_memory_pressure_analysis.json"
CHART_PATH = ROOT / "benchmarks" / "phase51_memory_pressure.png"
REPORT_PATH = ROOT / "docs" / "PHASE51_MEMORY_PRESSURE_REPORT.md"


def main() -> int:
    baseline = json.loads(BASELINE_PATH.read_text())
    pool = json.loads(POOL_PATH.read_text())
    baseline_rows = []
    for row in baseline["results"]:
        verifier = row["verifier"]
        baseline_rows.append(
            {
                "producers": row["producers"],
                "jobs": row["jobs"],
                "peak_rss_kb": row["peak_rss_kb"],
                "peak_hwm_kb": row["peak_hwm_kb"],
                "peak_vm_peak_kb": row["peak_vm_peak_kb"],
                "peak_threads": row["peak_threads"],
                "limiter_retries": row["limiter_retries"],
                "limiter_retries_per_job": row["limiter_retries"] / row["jobs"],
                "end_to_end_p95_us": verifier["end_to_end_p95_us"],
                "verification_wait_p95_us": verifier["verification_wait_p95_us"],
                "cache_hit_ratio": verifier["verification_cache_hits"]
                / max(1, verifier["verification_cache_hits"] + verifier["verification_cache_misses"]),
                "successful_commits": row["successful_commits"],
                "failed_outcomes": row["failed_outcomes"],
            }
        )
    pool_rows = []
    for row in pool["results"]:
        metrics = row["pool"]
        pool_rows.append(
            {
                "producers": row["producers"],
                "operations": row["operations"],
                "operations_per_sec": row["operations_per_sec"],
                "peak_rss_kb": row["peak_rss_kb"],
                "peak_hwm_kb": row["peak_hwm_kb"],
                "peak_vm_peak_kb": row["peak_vm_peak_kb"],
                "peak_threads": row["peak_threads"],
                "reused": metrics["reused"],
                "fresh_allocations": metrics["fresh_allocations"],
                "returns": metrics["returns"],
                "dropped_full": metrics["dropped_full"],
                "dropped_oversize": metrics["dropped_oversize"],
                "reuse_ratio": metrics["reused"] / max(1, metrics["checkouts"]),
            }
        )
    analysis = {
        "phase": 51,
        "baseline": baseline_rows,
        "pool": pool_rows,
        "interpretation": {
            "cas_expected_conflict_fixture": True,
            "cas_allocator_attribution": False,
            "pool_lock_free_queue": True,
            "pool_bounded_capacity": pool["pool_capacity"],
            "pool_bounded_slots": pool["pool_slots"],
            "pool_dropped_buffers": sum(row["dropped_full"] + row["dropped_oversize"] for row in pool_rows),
            "pool_reuse_ratio_min": min(row["reuse_ratio"] for row in pool_rows),
            "rss_growth_128_to_192": baseline_rows[-1]["peak_rss_kb"] / baseline_rows[0]["peak_rss_kb"],
            "vm_peak_growth_128_to_192": baseline_rows[-1]["peak_vm_peak_kb"] / baseline_rows[0]["peak_vm_peak_kb"],
            "thread_growth_128_to_192": baseline_rows[-1]["peak_threads"] - baseline_rows[0]["peak_threads"],
        },
    }
    ANALYSIS_PATH.write_text(json.dumps(analysis, indent=2) + "\n")

    x = [row["producers"] for row in baseline_rows]
    plt.style.use("seaborn-v0_8-whitegrid")
    fig, axes = plt.subplots(1, 2, figsize=(12, 5), dpi=160)
    axes[0].plot(x, [row["peak_rss_kb"] for row in baseline_rows], marker="o", label="RSS KiB")
    axes[0].plot(x, [row["peak_vm_peak_kb"] / 10 for row in baseline_rows], marker="s", label="VmPeak / 10 KiB")
    axes[0].plot(x, [row["peak_threads"] * 100 for row in baseline_rows], marker="^", label="Threads × 100")
    axes[0].set_title("CAS pressure proxies")
    axes[0].set_xlabel("Producers")
    axes[0].set_ylabel("Scaled process metric")
    axes[0].legend()
    pool_x = [row["producers"] for row in pool_rows]
    axes[1].plot(pool_x, [row["reuse_ratio"] * 100 for row in pool_rows], marker="o", label="Pool reuse %")
    axes[1].plot(pool_x, [row["fresh_allocations"] for row in pool_rows], marker="s", label="Fresh buffers")
    axes[1].plot(pool_x, [row["operations_per_sec"] / 1000 for row in pool_rows], marker="^", label="Ops/sec ÷ 1,000")
    axes[1].set_title("Lock-free pool evidence")
    axes[1].set_xlabel("Producers")
    axes[1].set_ylabel("Reuse / count / scaled throughput")
    axes[1].legend()
    fig.suptitle("Phase 51 memory-pressure and buffer-pool analysis", fontsize=14)
    fig.tight_layout()
    fig.savefig(CHART_PATH, bbox_inches="tight")
    plt.close(fig)

    b0, b2 = baseline_rows[0], baseline_rows[-1]
    p0, p2 = pool_rows[0], pool_rows[-1]
    report = f"""# Phase 51 memory-pressure and lock-free buffer-pool report

## Executive summary

The 128+ producer CAS baseline shows that the dominant scaling signals are not resident bytes alone. From 128 to 192 producers, peak RSS increased from **{b0['peak_rss_kb']} KiB** to **{b2['peak_rss_kb']} KiB** ({b2['peak_rss_kb'] / b0['peak_rss_kb']:.2f}×), VmPeak increased from **{b0['peak_vm_peak_kb']} KiB** to **{b2['peak_vm_peak_kb']} KiB** ({b2['peak_vm_peak_kb'] / b0['peak_vm_peak_kb']:.2f}×), and threads increased from **{b0['peak_threads']}** to **{b2['peak_threads']}**. The strongest pressure signal is admission retry amplification: retries per job rose from **{b0['limiter_retries_per_job']:.2f}** to **{b2['limiter_retries_per_job']:.2f}** in this run. The benchmark intentionally produces one successful commit and expected same-generation conflicts for every remaining job, so it diagnoses contention/fencing pressure rather than valid-write capacity.

The bounded MPMC pool benchmark retained a **{p0['reuse_ratio']:.2%}–{p2['reuse_ratio']:.2%}** reuse ratio from 128 to 256 producers, performed only **{p0['fresh_allocations']}–{p2['fresh_allocations']}** fresh allocations, and recorded zero full-queue and oversize drops. This supports the pool’s bounded reuse contract for transient 512-byte buffers, but it does not prove lower RSS in the CAS workload because the pool is currently integrated into code-generation output, not the ownership-intent retry path.

## CAS pressure proxies

| Producers | Peak RSS KiB | VmPeak KiB | Threads | Retries/job | E2E p95 µs | Cache hit ratio | Successes | Expected failures |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for row in baseline_rows:
        report += f"| {row['producers']} | {row['peak_rss_kb']} | {row['peak_vm_peak_kb']} | {row['peak_threads']} | {row['limiter_retries_per_job']:.2f} | {row['end_to_end_p95_us']} | {row['cache_hit_ratio']:.2%} | {row['successful_commits']} | {row['failed_outcomes']} |\n"
    report += """
## Pool evidence

| Producers | Operations | Pool reuse | Fresh buffers | Returns | Full drops | Oversize drops | Ops/sec |
|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for row in pool_rows:
        report += f"| {row['producers']} | {row['operations']} | {row['reuse_ratio']:.2%} | {row['fresh_allocations']} | {row['returns']} | {row['dropped_full']} | {row['dropped_oversize']} | {row['operations_per_sec']:.0f} |\n"
    report += """
## Interpretation and next boundary

The thread and VmPeak curves are consistent with native producer-thread stack/address-space reservations and bounded worker infrastructure. The retry curve indicates that producer-side admission loops can dominate scheduling and transient intent cloning under conflict storms. A pooled buffer can reduce repeated transient serialization/output allocations, but it cannot eliminate thread stacks, queue nodes, `OwnershipBoundCasIntent` cloning, cryptographic verification work, or filesystem/CAS state. The next allocator study must instrument allocation bytes and peak live bytes around candidate cloning and admission retries, compare fixed worker pools against one-thread-per-producer, and include unique-request, sequential-valid, mixed-valid/conflicting, and forged-evidence workloads.

The raw artifacts are `benchmarks/phase51_memory_profile_baseline.json` and `benchmarks/phase51_buffer_pool_metrics.json`; the derived analysis is `benchmarks/phase51_memory_pressure_analysis.json`; and the chart is `benchmarks/phase51_memory_pressure.png`. These are local sanitized observations with no secret material and no cluster mutation.
"""
    REPORT_PATH.write_text(report)
    print(json.dumps({"analysis": str(ANALYSIS_PATH), "chart": str(CHART_PATH), "report": str(REPORT_PATH)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
