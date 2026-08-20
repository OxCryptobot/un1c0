#!/usr/bin/env python3
"""Analyze Phase 50 UEG parsing, incremental generation, and Phase 47 pressure evidence."""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
CODEGEN_PATH = ROOT / "benchmarks" / "phase50_ueg_codegen_metrics.json"
MEMORY_PATH = ROOT / "benchmarks" / "phase47_memory_profile_metrics.json"
ANALYSIS_PATH = ROOT / "benchmarks" / "phase50_ueg_codegen_analysis.json"
CHART_PATH = ROOT / "benchmarks" / "phase50_ueg_codegen_performance.png"
REPORT_PATH = ROOT / "docs" / "PHASE50_PERFORMANCE_MEMORY_REPORT.md"


def percentile_ratio(value: float, baseline: float) -> float:
    return value / baseline if baseline else math.inf


def main() -> int:
    codegen = json.loads(CODEGEN_PATH.read_text())
    memory = json.loads(MEMORY_PATH.read_text())
    rows = codegen["function_counts"]
    baseline = rows[0]["parser"]["parse_p95_us"]

    parse_rows = []
    generation_rows = []
    for row in rows:
        parser = row["parser"]
        parse_rows.append(
            {
                "functions": row["functions"],
                "source_bytes": row["source_bytes"],
                "parse_p50_us": parser["parse_p50_us"],
                "parse_p95_us": parser["parse_p95_us"],
                "parse_max_us": parser["parse_max_us"],
                "parse_per_function_p95_us": parser["parse_per_function_p95_us"],
                "p95_vs_single_function": percentile_ratio(parser["parse_p95_us"], baseline),
                "nodes": parser["parsed_nodes"],
            }
        )
        for target in row["generation"]:
            generation_rows.append(
                {
                    "functions": row["functions"],
                    "target": target["target"],
                    "bytes": target["bytes"],
                    "chunks": target["chunks"],
                    "generation_p50_us": target["generation_p50_us"],
                    "generation_p95_us": target["generation_p95_us"],
                    "generation_max_us": target["generation_max_us"],
                }
            )

    memory_rows = memory["results"]
    memory_summary = []
    for row in memory_rows:
        verifier = row["verifier"]
        memory_summary.append(
            {
                "producers": row["producers"],
                "peak_rss_kb": row["peak_rss_kb"],
                "peak_hwm_kb": row["peak_hwm_kb"],
                "peak_vm_peak_kb": row["peak_vm_peak_kb"],
                "peak_threads": row["peak_threads"],
                "limiter_retries": row["limiter_retries"],
                "jobs": row["jobs"],
                "limiter_retries_per_job": row["limiter_retries"] / row["jobs"],
                "cache_hit_ratio": verifier["verification_cache_hits"]
                / max(1, verifier["verification_cache_hits"] + verifier["verification_cache_misses"]),
                "end_to_end_p95_us": verifier["end_to_end_p95_us"],
                "failed_outcomes": row["failed_outcomes"],
                "successful_commits": row["successful_commits"],
            }
        )

    analysis = {
        "phase": 50,
        "source_artifact": str(CODEGEN_PATH.relative_to(ROOT)),
        "memory_artifact": str(MEMORY_PATH.relative_to(ROOT)),
        "parser_scaling": parse_rows,
        "target_generation_scaling": generation_rows,
        "memory_pressure_review": {
            "rows": memory_summary,
            "rss_growth_factor_32_to_96": memory_rows[-1]["peak_rss_kb"] / memory_rows[0]["peak_rss_kb"],
            "vm_peak_growth_factor_32_to_96": memory_rows[-1]["peak_vm_peak_kb"] / memory_rows[0]["peak_vm_peak_kb"],
            "thread_growth_32_to_96": memory_rows[-1]["peak_threads"] - memory_rows[0]["peak_threads"],
            "expected_conflict_fixture": True,
            "gc_pause_measurement": False,
            "allocator_attribution": False,
        },
    }
    ANALYSIS_PATH.write_text(json.dumps(analysis, indent=2) + "\n")

    functions = [row["functions"] for row in rows]
    plt.style.use("seaborn-v0_8-whitegrid")
    fig, axes = plt.subplots(1, 2, figsize=(12, 5), dpi=160)
    axes[0].plot(functions, [row["parser"]["parse_p95_us"] for row in rows], marker="o", label="parse p95")
    axes[0].plot(functions, [row["parser"]["parse_per_function_p95_us"] for row in rows], marker="s", label="parse p95 / function")
    axes[0].set_title("UEG parsing scaling")
    axes[0].set_xlabel("Functions in source")
    axes[0].set_ylabel("Microseconds")
    axes[0].set_xscale("log", base=2)
    axes[0].legend()

    for target in sorted({row["target"] for row in generation_rows}):
        target_rows = [row for row in generation_rows if row["target"] == target]
        axes[1].plot(
            [row["functions"] for row in target_rows],
            [row["generation_p95_us"] for row in target_rows],
            marker="o",
            label=target,
        )
    axes[1].set_title("Incremental target generation scaling")
    axes[1].set_xlabel("Functions in source")
    axes[1].set_ylabel("Generation p95 (microseconds)")
    axes[1].set_xscale("log", base=2)
    axes[1].legend()
    fig.suptitle("Phase 50 UEG performance benchmark", fontsize=14)
    fig.tight_layout()
    fig.savefig(CHART_PATH, bbox_inches="tight")
    plt.close(fig)

    largest = parse_rows[-1]
    memory_last = memory_summary[-1]
    report = f"""# Phase 50 UEG performance and memory-pressure report

## Executive summary

The deterministic Phase 50 benchmark compares one, two, four, eight, sixteen, and thirty-two typed UEG functions. Parse p95 rises from **{parse_rows[0]['parse_p95_us']} µs** for one function to **{largest['parse_p95_us']} µs** for thirty-two functions, while normalized parse p95 per function falls from **{parse_rows[0]['parse_per_function_p95_us']} µs** to **{largest['parse_per_function_p95_us']} µs**. This indicates roughly linear total work with improving amortization, not an observed super-linear parser cliff, within the tested range. Incremental target generation remains bounded by emitted chunks and bytes; the Go and Zig bindings carry more target-specific formatting work than Rust and Python in this fixture.

The Phase 47 high-concurrency memory profile remains a pressure-proxy study. At 96 producers it measured **{memory_last['peak_rss_kb']} KiB** peak RSS, **{memory_last['peak_vm_peak_kb']} KiB** VmPeak, **{memory_last['peak_threads']}** threads, **{memory_last['limiter_retries_per_job']:.2f}** limiter retries per job, and **{memory_last['end_to_end_p95_us']} µs** end-to-end p95. The profile records no GC pauses and no allocator attribution; Rust has no tracing GC in this path. Because the fixture intentionally creates same-generation conflicts, its failed outcomes measure contention bookkeeping rather than valid durable-write throughput.

## Parser comparison

| Functions | Source bytes | Parse p50 (µs) | Parse p95 (µs) | Parse p95/function (µs) | Parse max (µs) |
|---:|---:|---:|---:|---:|---:|
"""
    for row in parse_rows:
        report += f"| {row['functions']} | {row['source_bytes']} | {row['parse_p50_us']} | {row['parse_p95_us']} | {row['parse_per_function_p95_us']} | {row['parse_max_us']} |\n"

    report += """
## Target generation comparison

| Functions | Target | Chunks | Bytes | Generation p50 (µs) | Generation p95 (µs) | Generation max (µs) |
|---:|---|---:|---:|---:|---:|---:|
"""
    for row in generation_rows:
        report += f"| {row['functions']} | {row['target']} | {row['chunks']} | {row['bytes']} | {row['generation_p50_us']} | {row['generation_p95_us']} | {row['generation_max_us']} |\n"

    report += f"""
## Allocator-pressure proxy review

The 32-to-96 producer memory profile increased peak RSS by **{memory_rows[-1]['peak_rss_kb'] - memory_rows[0]['peak_rss_kb']} KiB** ({memory_rows[-1]['peak_rss_kb'] / memory_rows[0]['peak_rss_kb']:.2f}×), VmPeak by **{memory_rows[-1]['peak_vm_peak_kb'] - memory_rows[0]['peak_vm_peak_kb']} KiB** ({memory_rows[-1]['peak_vm_peak_kb'] / memory_rows[0]['peak_vm_peak_kb']:.2f}×), and threads by **{memory_rows[-1]['peak_threads'] - memory_rows[0]['peak_threads']}**. Cache hit ratio remained between **{memory_summary[0]['cache_hit_ratio']:.2%}** and **{memory_summary[-1]['cache_hit_ratio']:.2%}**, but total limiter retries increased from **{memory_rows[0]['limiter_retries']}** to **{memory_rows[-1]['limiter_retries']}** and end-to-end p95 increased from **{memory_rows[0]['verifier']['end_to_end_p95_us']} µs** to **{memory_rows[-1]['verifier']['end_to_end_p95_us']} µs**.

These observations support a bounded-contention interpretation: RSS remained modest relative to virtual-memory and thread growth, while retries, queue/wait/service tails, and expected-conflict completion dominated degradation. They do not prove that retries allocate memory, identify allocator bins, measure fragmentation, or establish a leak. The next measurement should add allocator instrumentation, cgroup memory events, fixed allocator settings, and unique-request/mixed-validity workloads.

## Artifacts and limitations

The raw benchmark is [`benchmarks/phase50_ueg_codegen_metrics.json`](../benchmarks/phase50_ueg_codegen_metrics.json), derived analysis is [`benchmarks/phase50_ueg_codegen_analysis.json`](../benchmarks/phase50_ueg_codegen_analysis.json), and the chart is [`benchmarks/phase50_ueg_codegen_performance.png`](../benchmarks/phase50_ueg_codegen_performance.png). The source memory profile is [`benchmarks/phase47_memory_profile_metrics.json`](../benchmarks/phase47_memory_profile_metrics.json). These are local deterministic observations, not production capacity claims. The benchmark uses a fixed source fixture, warm process, bounded iteration count, and no cluster mutation or secret material.
"""
    REPORT_PATH.write_text(report)
    print(json.dumps({"analysis": str(ANALYSIS_PATH), "chart": str(CHART_PATH), "report": str(REPORT_PATH)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
