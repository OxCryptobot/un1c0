#!/usr/bin/env python3
"""Compare Phase 58–60 Rust 32-function semantic-session benchmarks."""
from __future__ import annotations

import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
OUTPUT_JSON = ROOT / "benchmarks" / "phase58_60_semantic_comparison.json"
OUTPUT_PNG = ROOT / "benchmarks" / "phase58_60_semantic_comparison.png"


def find_row(path: Path, *, functions: int = 32, target: str = "rust") -> dict:
    rows = json.loads(path.read_text(encoding="utf-8"))
    matches = [row for row in rows if row.get("functions") == functions and row.get("target") == target]
    if len(matches) != 1:
        raise RuntimeError(f"expected one {target}/{functions} row in {path}, found {len(matches)}")
    return matches[0]


def main() -> None:
    p58 = find_row(ROOT / "benchmarks" / "phase58_semantic_session.json")
    p59 = find_row(ROOT / "benchmarks" / "phase59_semantic_change_derivation.json")
    p60 = find_row(ROOT / "benchmarks" / "phase60_semantic_edit_manifest.json")
    rows = [
        {
            "phase": 58,
            "incremental_operation": "warm dependency-aware refresh",
            "full_capture_p50_ns": p58["full_capture_p50_ns"],
            "full_capture_p95_ns": p58["full_capture_p95_ns"],
            "incremental_p50_ns": p58["warm_refresh_p50_ns"],
            "incremental_p95_ns": p58["warm_refresh_p95_ns"],
            "functions": 32,
            "target": "rust",
            "source": "benchmarks/phase58_semantic_session.json",
            "errors": p58["refresh_errors"],
        },
        {
            "phase": 59,
            "incremental_operation": "fingerprint-derived auto-refresh",
            "full_capture_p50_ns": p59["full_capture_p50_ns"],
            "full_capture_p95_ns": p59["full_capture_p95_ns"],
            "incremental_p50_ns": p59["auto_refresh_p50_ns"],
            "incremental_p95_ns": p59["auto_refresh_p95_ns"],
            "functions": 32,
            "target": "rust",
            "source": "benchmarks/phase59_semantic_change_derivation.json",
            "errors": p59["refresh_errors"],
        },
        {
            "phase": 60,
            "incremental_operation": "typed edit-manifest refresh",
            "full_capture_p50_ns": p60["full_capture_p50_ns"],
            "full_capture_p95_ns": p60["full_capture_p95_ns"],
            "incremental_p50_ns": p60["manifest_refresh_p50_ns"],
            "incremental_p95_ns": p60["manifest_refresh_p95_ns"],
            "functions": 32,
            "target": "rust",
            "source": "benchmarks/phase60_semantic_edit_manifest.json",
            "errors": p60["errors"],
        },
    ]
    if any(row["errors"] != 0 for row in rows):
        raise RuntimeError("comparison contains a benchmark error")
    OUTPUT_JSON.write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")

    phases = [str(row["phase"]) for row in rows]
    x = list(range(len(rows)))
    fig, axes = plt.subplots(1, 2, figsize=(12, 5), constrained_layout=True)
    for axis, percentile, title in zip(
        axes,
        ("p50", "p95"),
        ("32-function Rust p50 latency", "32-function Rust p95 latency"),
        strict=True,
    ):
        full = [row[f"full_capture_{percentile}_ns"] / 1_000 for row in rows]
        incremental = [row[f"incremental_{percentile}_ns"] / 1_000 for row in rows]
        axis.plot(x, full, marker="o", label="full snapshot capture")
        axis.plot(x, incremental, marker="x", label="phase-specific incremental path")
        axis.set_xticks(x, phases)
        axis.set_xlabel("phase")
        axis.set_ylabel("microseconds")
        axis.set_title(title)
        axis.grid(True, alpha=0.25)
        axis.legend(fontsize=8)
    fig.suptitle("Phase 58–60 semantic performance comparison")
    fig.savefig(OUTPUT_PNG, dpi=160)
    print(OUTPUT_JSON)
    print(OUTPUT_PNG)


if __name__ == "__main__":
    main()
