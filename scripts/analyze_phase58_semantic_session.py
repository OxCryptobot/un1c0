#!/usr/bin/env python3
"""Plot sanitized Phase 58 semantic-session benchmark data."""
from __future__ import annotations

import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase58_semantic_session.json"
OUTPUT = ROOT / "benchmarks" / "phase58_semantic_session.png"


def main() -> None:
    rows = json.loads(INPUT.read_text(encoding="utf-8"))
    targets = sorted({row["target"] for row in rows})
    fig, axes = plt.subplots(1, 2, figsize=(13, 5), constrained_layout=True)
    for target in targets:
        target_rows = sorted(
            (row for row in rows if row["target"] == target),
            key=lambda row: row["functions"],
        )
        functions = [row["functions"] for row in target_rows]
        axes[0].plot(
            functions,
            [row["full_capture_p50_ns"] / 1_000 for row in target_rows],
            marker="o",
            label=f"{target} full capture",
        )
        axes[0].plot(
            functions,
            [row["warm_refresh_p50_ns"] / 1_000 for row in target_rows],
            marker="x",
            linestyle="--",
            label=f"{target} warm refresh",
        )
        axes[1].plot(
            functions,
            [row["full_capture_p95_ns"] / 1_000 for row in target_rows],
            marker="o",
            label=f"{target} full capture",
        )
        axes[1].plot(
            functions,
            [row["warm_refresh_p95_ns"] / 1_000 for row in target_rows],
            marker="x",
            linestyle="--",
            label=f"{target} warm refresh",
        )

    for axis, title in zip(
        axes,
        ("p50 latency", "p95 latency"),
        strict=True,
    ):
        axis.set_title(title)
        axis.set_xlabel("functions in UEG")
        axis.set_ylabel("microseconds")
        axis.set_xscale("log", base=2)
        axis.grid(True, alpha=0.25)
        axis.legend(fontsize=7, ncol=2)
    fig.suptitle("Phase 58 dependency-aware semantic session")
    fig.savefig(OUTPUT, dpi=160)
    print(OUTPUT)


if __name__ == "__main__":
    main()
