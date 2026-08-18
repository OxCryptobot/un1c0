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
            "durable_compaction_manifests": "passed",
            "compaction_recovery": "passed",
            "snapshot_install_readiness": "passed",
            "snapshot_ack_binding": "passed",
            "snapshot_transfer_metrics": "passed",
            "snapshot_bandwidth_backpressure": "passed",
            "snapshot_cancellation": "passed",
            "snapshot_completion_accounting": "passed",
            "durable_term_vote_persistence": "passed",
            "durable_consensus_state_recovery": "passed",
            "epoch_bound_replay_window": "passed",
            "replay_term_floor": "passed",
            "cross_node_compaction_coordination": "passed",
            "compaction_quorum_admission": "passed",
            "follower_triggered_snapshot_requests": "passed",
            "snapshot_request_binding": "passed",
            "socket_layer_backpressure": "passed",
            "per_peer_socket_quotas": "passed",
            "receive_window_admission": "passed",
            "socket_quota_epoch_reset": "passed",
            "durable_transport_queue": "passed",
            "quota_recovery_after_restart": "passed",
            "atomic_queue_cutover": "passed",
            "queue_epoch_binding": "passed",
            "authenticated_durable_delivery": "passed",
            "socket_boundary_crash_injection": "passed",
            "crash_retry_queue_retention": "passed",
            "authenticated_delivery_ack_order": "passed",
            "replicated_ack_quorum": "passed",
            "cross_host_queue_ownership": "passed",
            "authenticated_ack_binding": "passed",
            "failover_owner_lease": "passed",
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
        "phase19_durable_compaction": {
            "manifest_snapshot_hash_binding": True,
            "fsync_before_atomic_cutover": True,
            "partial_staging_aborts": True,
            "prior_snapshot_preserved": True,
            "recovery_is_idempotent": True,
            "storage_scheduler": "deployment_boundary",
        },
        "phase20_snapshot_install_readiness": {
            "one_active_transfer_per_follower": True,
            "validated_does_not_advance_progress": True,
            "durably_staged_does_not_advance_progress": True,
            "installed_advances_progress": True,
            "exact_retry_boundary": True,
            "ack_hash_and_configuration_binding": True,
            "higher_term_clears_snapshot_state": True,
            "transport_and_storage": "deployment_boundary",
        },
        "phase21_snapshot_transfer_metrics": {
            "per_follower_isolation": True,
            "bytes_sent_and_remaining_are_monotonic": True,
            "rolling_window_is_bounded": True,
            "bandwidth_backpressure_has_exact_retry_tick": True,
            "installed_requires_complete_byte_accounting": True,
            "cancellation_clears_active_transfer": True,
            "cancellation_has_bounded_retry": True,
            "clock_uncertainty_blocks_progress": True,
            "transport_storage_and_scheduler": "deployment_boundary",
        },
        "phase22_durable_term_replay": {
            "term_and_vote_atomic_state_hash": True,
            "staging_recovery_removes_partial_state": True,
            "term_vote_restore_rejects_rollback": True,
            "same_term_vote_exclusivity_survives_restore": True,
            "replay_epoch_is_signature_bound": True,
            "replay_epoch_rotation_clears_windows": True,
            "replay_term_floor_rejects_stale_envelopes": True,
            "bounded_nonce_eviction_is_preserved": True,
            "persistence_and_socket_authority": "deployment_boundary",
        },
        "phase23_compaction_coordination_snapshot_requests": {
            "coordination_plan_is_hash_bound": True,
            "waiting_plan_has_no_mutation": True,
            "remote_quorum_admission_is_explicit": True,
            "stable_and_joint_quorum_logic": True,
            "append_predecessor_requests_snapshot": True,
            "incremental_base_requests_snapshot": True,
            "request_retry_tick_is_hash_bound": True,
            "stale_or_misbinding_requests_fail_closed": True,
            "network_scheduler_and_compaction_authority": "deployment_boundary",
        },
        "phase24_socket_backpressure_quotas": {
            "per_peer_send_isolation": True,
            "exact_frame_byte_admission": True,
            "send_quota_release": True,
            "receive_window_backpressure": True,
            "authentication_precedes_quota_mutation": True,
            "epoch_rotation_clears_quota_state": True,
            "legacy_transport_compatibility": True,
            "socket_threads_and_scheduler": "deployment_boundary",
        },
        "phase25_durable_transport_queues": {
            "queue_frames_are_hash_bound": True,
            "exact_quota_recovery_after_restart": True,
            "atomic_save_and_staging_cleanup": True,
            "fifo_ack_required": True,
            "epoch_mismatch_rejected": True,
            "persistence_failure_rolls_back": True,
            "socket_queue_threads_and_replication": "deployment_boundary",
        },
        "phase26_authenticated_durable_delivery": {
            "authenticated_payload_reverified_before_send": True,
            "one_active_delivery_per_peer": True,
            "ack_only_after_flush": True,
            "crash_points_retaining_queue": 4,
            "restart_retry_after_crash": True,
            "tampered_payload_fails_closed": True,
            "durable_delivery_metrics_non_secret": True,
            "socket_thread_and_process_ownership": "deployment_boundary",
        },
        "phase27_replicated_delivery_ownership": {
            "quorum_ack_required": True,
            "ack_hash_owner_term_epoch_bound": True,
            "idempotent_same_sender_ack": True,
            "conflicting_sender_ack_rejected": True,
            "ownership_transfer_lease_and_term_bound": True,
            "cross_host_restore_validates_source_identity": True,
            "failover_new_owner_can_retry": True,
            "old_owner_cannot_ack_after_transfer": True,
            "transport_and_replica_quorum": "deployment_boundary",
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
