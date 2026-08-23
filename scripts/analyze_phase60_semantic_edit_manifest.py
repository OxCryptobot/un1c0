#!/usr/bin/env python3
"""Plot sanitized Phase 60 semantic edit-manifest benchmarks."""
from __future__ import annotations

import json
from pathlib import Path

import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "benchmarks" / "phase60_semantic_edit_manifest.json"
OUTPUT = ROOT / "benchmarks" / "phase60_semantic_edit_manifest.png"


def main() -> None:
    rows = json.loads(INPUT.read_text(encoding="utf-8"))
    selected = sorted(
        (row for row in rows if row["target"] == "rust"),
        key=lambda row: row["functions"],
    )
    functions = [row["functions"] for row in selected]
    fig, axes = plt.subplots(1, 2, figsize=(12, 5), constrained_layout=True)
    for axis, percentile, title in zip(
        axes,
        ("p50", "p95"),
        ("Phase 60 p50 latency", "Phase 60 p95 latency"),
        strict=True,
    ):
        for prefix, label, marker in (
            ("full_capture", "full snapshot capture", "o"),
            ("manifest_resolution", "edit-manifest resolution", "s"),
            ("manifest_refresh", "manifest-bound refresh", "x"),
        ):
            axis.plot(
                functions,
                [row[f"{prefix}_{percentile}_ns"] / 1_000 for row in selected],
                marker=marker,
                label=label,
            )
        axis.set_title(title)
        axis.set_xlabel("functions in UEG")
        axis.set_ylabel("microseconds")
        axis.set_xscale("log", base=2)
        axis.grid(True, alpha=0.25)
        axis.legend(fontsize=8)
    fig.suptitle("Phase 60 typed semantic edit manifests — Rust profile")
    fig.savefig(OUTPUT, dpi=160)
    print(OUTPUT)


if __name__ == "__main__":
    main()
