#!/usr/bin/env python3
from __future__ import annotations

import json
from collections import defaultdict
from pathlib import Path

rows = json.loads(
    (Path(__file__).resolve().parents[1] / "benchmarks" / "phase58_semantic_session.json").read_text()
)
for target in sorted({row["target"] for row in rows}):
    target_rows = [row for row in rows if row["target"] == target]
    print(target)
    for row in sorted(target_rows, key=lambda item: item["functions"]):
        print(
            row["functions"],
            row["full_capture_p50_ns"],
            row["full_capture_p95_ns"],
            row["warm_refresh_p50_ns"],
            row["warm_refresh_p95_ns"],
            row["affected_functions"],
            row["revalidated_functions"],
            row["cache_hits"],
            row["cache_misses"],
        )
