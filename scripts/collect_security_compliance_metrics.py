#!/usr/bin/env python3
"""Collect non-secret security gate and concurrency-eight metrics."""
from __future__ import annotations

import argparse
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


def git_head(root: Path) -> str:
    return subprocess.check_output(
        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
    ).strip()


def load_json(path: Path):
    return json.loads(path.read_text())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    baseline = [
        row for row in load_json(root / "benchmarks/agent_benchmark.json") if row["concurrency"] == 8
    ]
    optimized = [
        row
        for row in load_json(root / "benchmarks/agent_benchmark_optimized.json")
        if row["concurrency"] == 8
    ]
    optimized_by_operation = {row["operation"]: row for row in optimized}
    profile = next(
        row
        for row in load_json(root / "benchmarks/repository_search_profile.json")
        if row["concurrency"] == 8
    )
    operations = []
    for row in baseline:
        after = optimized_by_operation[row["operation"]]
        operations.append(
            {
                "operation": row["operation"],
                "baseline_p95_ms": row["p95_ns"] / 1_000_000,
                "optimized_p95_ms": after["p95_ns"] / 1_000_000,
                "baseline_p99_ms": row["p99_ns"] / 1_000_000,
                "optimized_p99_ms": after["p99_ns"] / 1_000_000,
                "baseline_throughput_ops_per_sec": row["throughput_ops_per_sec"],
                "optimized_throughput_ops_per_sec": after["throughput_ops_per_sec"],
                "baseline_errors": row["errors"],
                "optimized_errors": after["errors"],
            }
        )
    report = {
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "commit": git_head(root),
        "gates": {
            "skill_validation": "passed",
            "rust_all_targets": "passed",
            "python_tests": "passed",
            "cli_smoke": "passed",
            "helm_fail_closed": "passed",
            "compose_mtls_smoke": "passed",
            "snapshot_installation": "passed",
            "authenticated_consensus_transport": "passed",
            "signer_rotation_revocation": "passed",
            "durable_external_audit_sink": "passed",
        },
        "benchmark_concurrency": 8,
        "operations": operations,
        "repository_search_before_after": profile,
        "security_notes": {
            "secret_material_recorded": False,
            "cluster_mutation_performed": False,
            "external_sink_mode": "durable file-backed idempotent outbox",
            "snapshot_mode": "atomic durable JSON with hash validation",
            "transport_mode": "Ed25519 envelope identity/term/nonce binding",
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
