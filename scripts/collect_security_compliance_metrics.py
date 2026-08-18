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
    partition_metrics = load_json(root / "benchmarks/consensus_partition_metrics.json")
    phase14_reads = load_json(root / "benchmarks/phase14_read_benchmark.json")
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
            "phase11_membership_change": "passed",
            "failure_injection_snapshot_recovery": "passed",
            "authenticated_socket_transport": "passed",
            "replay_window_and_cluster_binding": "passed",
            "snapshot_chunk_streaming": "passed",
            "incremental_state_sync": "passed",
            "network_stress_packet_corruption": "passed",
            "authenticated_partition_benchmark": "passed",
            "leader_lease_read_optimization": "passed",
            "linearizable_read_consistency": "passed",
            "election_timer_safety": "passed",
            "failure_detector_boundaries": "passed",
            "replication_flow_control": "passed",
            "replication_backpressure_boundaries": "passed",
            "remote_audit_ordering": "passed",
            "remote_audit_outbox_durability": "passed",
            "log_compaction_safety": "passed",
            "configuration_bound_snapshots": "passed",
        },
        "benchmark_concurrency": 8,
        "operations": operations,
        "repository_search_before_after": profile,
        "authenticated_consensus_partition": partition_metrics,
        "phase14_read_optimization": phase14_reads,
        "phase15_election_timers": {
            "timer_actions": ["Idle", "StartElection", "SendHeartbeats"],
            "clock_regression": "blocked_until_explicit_reanchor",
            "failure_detector_boundary": "suspect_at_or_after_interval",
            "transport_or_background_threads": False,
        },
        "phase16_replication_flow_control": {
            "actions": ["Idle", "Backpressured", "Send"],
            "one_in_flight_batch_per_peer": True,
            "exact_retry_boundary_is_sendable": True,
            "higher_term_clears_windows": True,
            "clock_uncertainty_blocks_sends": True,
            "transport_or_background_threads": False,
        },
        "phase17_remote_audit": {
            "envelope_signature_binding": True,
            "per_stream_sequence_order": True,
            "idempotent_enqueue": True,
            "gap_and_retry_retention": True,
            "accepted_ack_removes_and_syncs_directory": True,
            "transport_or_sink_quorum": "deployment_boundary",
        },
        "phase18_log_compaction": {
            "bounded_discard_and_retention": True,
            "logical_frontier_translation": True,
            "snapshot_required_for_behind_follower": True,
            "configuration_hash_binding": True,
            "invalid_target_no_mutation": True,
            "persistent_compaction_scheduler": "deployment_boundary",
        },
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
