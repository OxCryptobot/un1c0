#!/usr/bin/env python3
"""Audit the non-secret security compliance metrics artifact."""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

EXPECTED_GATE_COUNT = 201
EXPECTED_CONCURRENCIES = [1, 2, 4, 8, 16, 32]
INTENTIONAL_FALSE_EVIDENCE = {
    "phase15_election_timers.transport_or_background_threads",
    "phase16_replication_flow_control.transport_or_background_threads",
    "phase37_telemetry_failover.secret_material_recorded",
    "phase37_telemetry_failover.cluster_mutation_performed",
    "phase38_external_fencing_supervision.secret_material_recorded",
    "phase38_external_fencing_supervision.cluster_mutation_performed",
    "phase39_resource_durability.secret_material_recorded",
    "phase39_resource_durability.cluster_mutation_performed",
    "phase40_high_throughput_persistence.secret_material_recorded",
    "phase40_high_throughput_persistence.cluster_mutation_performed",
    "phase41_replicated_durability.secret_material_recorded",
    "phase41_replicated_durability.cluster_mutation_performed",
    "phase42_cross_process_ownership.secret_material_recorded",
    "phase42_cross_process_ownership.cluster_mutation_performed",
    "phase43_ownership_bound_cas.secret_material_recorded",
    "phase43_ownership_bound_cas.cluster_mutation_performed",
    "phase44_ownership_bound_cas_executor.secret_material_recorded",
    "phase44_ownership_bound_cas_executor.cluster_mutation_performed",
    "phase45_ownership_bound_cas_verifier.secret_material_recorded",
    "phase45_ownership_bound_cas_verifier.cluster_mutation_performed",
    "phase46_ownership_bound_cas_admission.secret_material_recorded",
    "phase46_ownership_bound_cas_admission.cluster_mutation_performed",
}

REQUIRED_PHASES = {
    "phase36_recovery_transport": [
        "authenticated_transport_envelope_signature_required",
        "transport_receiver_binding_required",
        "connection_epoch_replay_window_enforced",
        "durable_witness_reservation_hash_bound",
        "reservation_crash_cutover_atomic",
        "protected_write_exact_fence_required",
        "cross_host_chaos_duplicate_idempotent",
        "stale_transport_replay_rejected",
        "kernel_tls_and_distributed_filesystem",
    ],
    "phase37_telemetry_failover": [
        "signed_canonical_events",
        "producer_registry_key_pinning",
        "epoch_sequence_frontier",
        "bounded_labels_and_metrics",
        "append_only_hash_chain",
        "freshness_gate_blocks_promotion",
        "typed_idempotent_orchestration",
        "transport_and_reservation_fuzz_panics",
        "trace_storage_bounded",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase38_external_fencing_supervision": [
        "signed_authority_heartbeat",
        "pinned_authority_and_consumer_keys",
        "monotonic_authority_generations",
        "exact_consumer_token_binding",
        "complete_consumer_coverage_required",
        "stale_authority_blocks_readiness",
        "atomic_hash_bound_snapshot",
        "quarantined_consumer_blocks_readiness",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase39_resource_durability": [
        "authority_owner_region_signed",
        "valid_path_resource_metrics",
        "persistence_bytes_accounted",
        "file_fsync_latency_recorded",
        "directory_fsync_latency_recorded",
        "staging_recovery_bounded",
        "resource_budget_fail_closed",
        "sanitized_durability_evidence",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase40_high_throughput_persistence": [
        "concurrent_persistence_bounds",
        "atomic_order_under_concurrency",
        "unique_staging_target_accounting",
        "stale_staging_recovery_counted",
        "concurrent_completion_accounting",
        "contention_timing_recorded",
        "active_worker_resource_snapshot",
        "concurrent_failure_propagation",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase41_replicated_durability": [
        "cas_writer_signature_required",
        "replica_ack_signature_required",
        "cas_expected_generation_hash_exact",
        "replicated_ack_quorum_required",
        "replica_ack_conflict_rejected",
        "nonce_retry_idempotent",
        "cas_commit_atomic_on_failure",
        "durable_cas_snapshot_hash_bound",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase42_cross_process_ownership": [
        "ownership_claim_signature_required",
        "atomic_cross_process_lock",
        "ownership_epoch_fencing",
        "owner_bound_lease_lifecycle",
        "stale_staging_cleanup",
        "managed_recovery_state_fail_closed",
        "managed_recovery_distinct_quorum",
        "recovery_evidence_hash_bound",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase43_ownership_bound_cas": [
        "ownership_bound_cas_permit_required",
        "cas_request_epoch_exactly_bound",
        "ownership_record_hash_exactly_bound",
        "ownership_lock_held_through_quorum",
        "cas_quorum_failure_preserves_ownership",
        "successful_cas_advances_ownership_record",
        "stale_permit_rejected_before_cas",
        "idempotent_retry_preserves_ownership",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase44_ownership_bound_cas_executor": [
        "bounded_executor_queue",
        "worker_owned_mutation",
        "deterministic_queue_full_backpressure",
        "fifo_intent_ordering",
        "concurrent_conflicts_fail_closed",
        "bounded_latency_samples",
        "shutdown_rejects_new_intents",
        "sanitized_concurrency_metrics",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase45_ownership_bound_cas_verifier": [
        "parallel_pre_admission_workers",
        "bounded_verification_queue",
        "pre_admission_signature_hash_checks",
        "ordered_mutation_dispatch",
        "mutation_revalidation_required",
        "pre_admission_failure_no_mutation",
        "verification_latency_metrics_bounded",
        "verification_stress_sanitized",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase46_ownership_bound_cas_admission": [
        "adaptive_limiter_bounded",
        "limiter_rejection_typed",
        "no_intent_id_on_limited",
        "parsed_verifying_keys_reused",
        "context_fingerprint_bound_cache",
        "freshness_rechecked_on_cache_hit",
        "validation_failure_adapts_capacity",
        "sanitized_admission_metrics",
        "secret_material_recorded",
        "cluster_mutation_performed",
    ],
    "phase35_multileader_witness": [
        "multi_leader_proposal_signature_required",
        "witness_quorum_arbitration_required",
        "one_witness_vote_per_round",
        "conflicting_quorum_split_brain_rejected",
        "stale_multi_leader_log_rejected",
        "fencing_token_domain_bound",
        "fencing_authority_registry_pinned",
        "fencing_generation_rollback_rejected",
        "transport_failure_detector_and_process_fencing",
    ],
    "phase34_replicated_recovery": [
        "joint_observer_quorum_required",
        "joint_to_final_membership_ordering",
        "replicated_recovery_log_hash_bound",
        "external_fencing_token_signature_required",
        "fencing_epoch_monotonicity",
        "stale_external_fence_rejected",
        "replicated_authority_restart_continuity",
        "dynamic_partition_epoch_chaos_safe",
        "transport_process_fencing_and_external_registry",
    ],
    "phase33_durable_recovery": [
        "snapshot_hash_bound",
        "atomic_cutover",
        "partial_staging_cleanup",
        "restart_preserves_pending_authority",
        "observer_membership_epoch_bound",
        "stale_membership_evidence_rejected",
        "concurrent_partition_race_single_commit",
        "durable_storage_and_external_fencing",
    ],
    "phase15_election_timers": [
        "clock_regression",
        "failure_detector_boundary",
        "timer_actions",
        "transport_or_background_threads",
    ],
    "phase16_replication_flow_control": [
        "actions",
        "clock_uncertainty_blocks_sends",
        "exact_retry_boundary_is_sendable",
        "higher_term_clears_windows",
        "one_in_flight_batch_per_peer",
        "transport_or_background_threads",
    ],
    "phase17_remote_audit": [
        "accepted_ack_removes_and_syncs_directory",
        "envelope_signature_binding",
        "gap_and_retry_retention",
        "idempotent_enqueue",
        "per_stream_sequence_order",
        "transport_or_sink_quorum",
    ],
    "phase18_log_compaction": [
        "bounded_discard_and_retention",
        "configuration_hash_binding",
        "invalid_target_no_mutation",
        "logical_frontier_translation",
        "persistent_compaction_scheduler",
        "snapshot_required_for_behind_follower",
    ],
    "phase19_durable_compaction": [
        "fsync_before_atomic_cutover",
        "manifest_snapshot_hash_binding",
        "partial_staging_aborts",
        "prior_snapshot_preserved",
        "recovery_is_idempotent",
        "storage_scheduler",
    ],
    "phase20_snapshot_install_readiness": [
        "ack_hash_and_configuration_binding",
        "durably_staged_does_not_advance_progress",
        "exact_retry_boundary",
        "higher_term_clears_snapshot_state",
        "installed_advances_progress",
        "one_active_transfer_per_follower",
        "validated_does_not_advance_progress",
    ],
    "phase21_snapshot_transfer_metrics": [
        "bandwidth_backpressure_has_exact_retry_tick",
        "bytes_sent_and_remaining_are_monotonic",
        "cancellation_clears_active_transfer",
        "cancellation_has_bounded_retry",
        "clock_uncertainty_blocks_progress",
        "installed_requires_complete_byte_accounting",
        "per_follower_isolation",
        "rolling_window_is_bounded",
        "transport_storage_and_scheduler",
    ],
    "phase22_durable_term_replay": [
        "term_and_vote_atomic_state_hash",
        "staging_recovery_removes_partial_state",
        "term_vote_restore_rejects_rollback",
        "same_term_vote_exclusivity_survives_restore",
        "replay_epoch_is_signature_bound",
        "replay_epoch_rotation_clears_windows",
        "replay_term_floor_rejects_stale_envelopes",
        "bounded_nonce_eviction_is_preserved",
        "persistence_and_socket_authority",
    ],
    "phase23_compaction_coordination_snapshot_requests": [
        "coordination_plan_is_hash_bound",
        "waiting_plan_has_no_mutation",
        "remote_quorum_admission_is_explicit",
        "stable_and_joint_quorum_logic",
        "append_predecessor_requests_snapshot",
        "incremental_base_requests_snapshot",
        "request_retry_tick_is_hash_bound",
        "stale_or_misbinding_requests_fail_closed",
        "network_scheduler_and_compaction_authority",
    ],
    "phase24_socket_backpressure_quotas": [
        "per_peer_send_isolation",
        "exact_frame_byte_admission",
        "send_quota_release",
        "receive_window_backpressure",
        "authentication_precedes_quota_mutation",
        "epoch_rotation_clears_quota_state",
        "legacy_transport_compatibility",
        "socket_threads_and_scheduler",
    ],
    "phase25_durable_transport_queues": [
        "queue_frames_are_hash_bound",
        "exact_quota_recovery_after_restart",
        "atomic_save_and_staging_cleanup",
        "fifo_ack_required",
        "epoch_mismatch_rejected",
        "persistence_failure_rolls_back",
        "socket_queue_threads_and_replication",
    ],
    "phase26_authenticated_durable_delivery": [
        "authenticated_payload_reverified_before_send",
        "one_active_delivery_per_peer",
        "ack_only_after_flush",
        "crash_points_retaining_queue",
        "restart_retry_after_crash",
        "tampered_payload_fails_closed",
        "durable_delivery_metrics_non_secret",
        "socket_thread_and_process_ownership",
    ],
    "phase27_replicated_delivery_ownership": [
        "quorum_ack_required",
        "ack_hash_owner_term_epoch_bound",
        "idempotent_same_sender_ack",
        "conflicting_sender_ack_rejected",
        "ownership_transfer_lease_and_term_bound",
        "cross_host_restore_validates_source_identity",
        "failover_new_owner_can_retry",
        "old_owner_cannot_ack_after_transfer",
        "transport_and_replica_quorum",
    ],
    "phase28_partition_ownership_fencing": [
        "quorum_loss_fences_delivery",
        "lease_expiry_fences_delivery",
        "fence_survives_restart",
        "ownership_transfer_clears_fence",
        "network_failure_detector_and_clock_authority",
    ],
    "phase29_authenticated_remote_fencing": [
        "authenticated_remote_fence_observation",
        "remote_fence_owner_term_binding",
        "remote_fence_idempotent",
        "remote_fence_misbinding_rejected",
        "failure_detector_quorum_authority",
    ],
    "phase30_multi_region_failover": [
        "region_topology_is_deterministic",
        "asymmetric_partition_is_replayable",
        "observer_quorum_admission",
        "split_brain_commit_exclusion",
        "stale_owner_is_fenced_after_heal",
        "transfer_crash_recovers_safely",
        "clock_skew_boundary_is_fail_closed",
        "multi_region_retry_reaches_quorum",
        "transport_and_cloud_region_authority",
    ],
    "phase31_secure_replay_verification": [
        "signed_replay_manifest_required",
        "replay_schedule_hash_bound",
        "replay_sequence_tick_bounds_enforced",
        "trusted_key_cluster_epoch_binding",
        "tampered_schedule_rejected",
        "trace_seal_verification",
        "production_key_custody_and_transport",
    ],
    "phase32_disaster_recovery_failover": [
        "signed_region_failure_observation_required",
        "distinct_observer_quorum_required",
        "snapshot_hash_binding_required",
        "higher_term_epoch_promotion_required",
        "old_region_fenced_on_commit",
        "single_active_region_invariant",
        "idempotent_failover_evidence",
        "stale_or_conflicting_failover_rejected",
        "failure_detection_sequence_required",
        "conflicting_pending_proposal_rejected",
        "terminal_recovery_cycle_protected",
        "committed_proposal_identity_bound",
        "production_cloud_authority",
    ],
}


def git_head(root: Path) -> str:
    return subprocess.check_output(
        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
    ).strip()


def is_ancestor(root: Path, commit: str, head: str) -> bool:
    return subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", commit, head],
        check=False,
    ).returncode == 0


def check(condition: bool, failures: list[str], message: str) -> None:
    if not condition:
        failures.append(message)


def audit(root: Path, artifact: Path) -> dict:
    report = json.loads(artifact.read_text())
    failures: list[str] = []
    gates = report.get("gates", {})
    check(len(gates) == EXPECTED_GATE_COUNT, failures, f"expected {EXPECTED_GATE_COUNT} gates, found {len(gates)}")
    check(bool(gates), failures, "gate map is empty")
    check(all(value == "passed" for value in gates.values()), failures, "one or more gates are not marked passed")
    check(report.get("benchmark_concurrency") == 8, failures, "benchmark concurrency must remain 8")
    current_head = git_head(root)
    metrics_commit = report.get("commit")
    check(
        isinstance(metrics_commit, str) and is_ancestor(root, metrics_commit, current_head),
        failures,
        "metrics commit is neither the current repository HEAD nor an ancestor",
    )

    for phase, required_keys in REQUIRED_PHASES.items():
        section = report.get(phase)
        check(isinstance(section, dict), failures, f"missing phase evidence section: {phase}")
        if not isinstance(section, dict):
            continue
        for key in required_keys:
            check(key in section, failures, f"missing {phase}.{key}")
        for key, value in section.items():
            if isinstance(value, bool) and not value:
                check(
                    f"{phase}.{key}" in INTENTIONAL_FALSE_EVIDENCE,
                    failures,
                    f"unexpected false boolean evidence in {phase}.{key}",
                )

    operations = report.get("operations", [])
    check(len(operations) > 0, failures, "operation benchmark evidence is empty")
    for row in operations:
        check(row.get("baseline_errors") == 0, failures, f"baseline operation errors: {row.get('operation')}")
        check(row.get("optimized_errors") == 0, failures, f"optimized operation errors: {row.get('operation')}")
        check(row.get("optimized_p95_ms", 0) >= 0, failures, f"negative optimized p95: {row.get('operation')}")
        check(row.get("optimized_throughput_ops_per_sec", 0) >= 0, failures, f"negative optimized throughput: {row.get('operation')}")

    profile = report.get("repository_search_before_after", {})
    for key in ("baseline_errors", "optimized_errors"):
        check(profile.get(key) == 0, failures, f"repository search {key} is non-zero")
    for key in ("baseline_p95_ms", "optimized_p95_ms", "baseline_throughput_ops_per_sec", "optimized_throughput_ops_per_sec"):
        check(isinstance(profile.get(key), (int, float)) and profile[key] >= 0, failures, f"invalid repository search metric: {key}")

    reads = report.get("phase14_read_optimization", [])
    check(len(reads) == 12, failures, f"expected 12 Phase 14 read rows, found {len(reads)}")
    if reads:
        check(sorted({row.get("concurrency") for row in reads}) == EXPECTED_CONCURRENCIES, failures, "Phase 14 concurrency set is not 1,2,4,8,16,32")
        check({row.get("path") for row in reads} == {"lease_fast_path", "quorum_read_index"}, failures, "Phase 14 paths are incomplete")
        check(all(row.get("errors") == 0 for row in reads), failures, "Phase 14 contains benchmark errors")

    notes = report.get("security_notes", {})
    check(notes.get("secret_material_recorded") is False, failures, "secret material flag is not false")
    check(notes.get("cluster_mutation_performed") is False, failures, "cluster mutation flag is not false")
    check(notes.get("transport_mode") == "Ed25519 envelope identity/term/nonce binding", failures, "unexpected transport security note")
    check(notes.get("telemetry_mode") == "signed bounded hash-chained journal with freshness gate", failures, "unexpected telemetry security note")
    check(notes.get("fuzz_mode") == "deterministic epoch churn with sanitized counters", failures, "unexpected fuzz security note")
    check(notes.get("fencing_supervision_mode") == "signed authority heartbeat plus exact consumer acknowledgements", failures, "unexpected fencing supervision security note")
    check(notes.get("resource_durability_mode") == "bounded local fsync, atomic rename, and process-resource instrumentation", failures, "unexpected resource durability security note")
    check(notes.get("high_throughput_persistence_mode") == "bounded concurrent unique staging with atomic rename and active-worker resource capture", failures, "unexpected high-throughput persistence security note")
    check(notes.get("replicated_durability_mode") == "signed replica acknowledgements with quorum-gated single-writer compare-and-swap", failures, "unexpected replicated durability security note")
    check(notes.get("cross_process_ownership_mode") == "atomic ownership lease epochs with hash-bound managed-volume recovery quorum", failures, "unexpected cross-process ownership security note")
    check(notes.get("ownership_bound_cas_mode") == "ownership permit held through replicated CAS quorum and record advancement", failures, "unexpected ownership-bound CAS security note")
    check(notes.get("ownership_bound_cas_executor_mode") == "bounded worker-owned FIFO executor with sanitized contention metrics", failures, "unexpected ownership-bound CAS executor security note")
    check(notes.get("ownership_bound_cas_verifier_mode") == "parallel read-only pre-admission verification with ordered lock-held mutation revalidation", failures, "unexpected ownership-bound CAS verifier security note")
    check(notes.get("ownership_bound_cas_admission_mode") == "bounded adaptive admission with context-bound parsed-key verification facts and live freshness checks", failures, "unexpected ownership-bound CAS admission security note")

    return {
        "artifact": str(artifact),
        "commit": report.get("commit"),
        "commit_is_current_or_ancestor": isinstance(metrics_commit, str)
        and is_ancestor(root, metrics_commit, current_head),
        "expected_gate_count": EXPECTED_GATE_COUNT,
        "observed_gate_count": len(gates),
        "passed_gate_count": sum(value == "passed" for value in gates.values()),
        "phase_sections_audited": sorted(REQUIRED_PHASES),
        "phase14_rows": len(reads),
        "benchmark_concurrency": report.get("benchmark_concurrency"),
        "secret_material_recorded": notes.get("secret_material_recorded"),
        "cluster_mutation_performed": notes.get("cluster_mutation_performed"),
        "result": "passed" if not failures else "failed",
        "failures": failures,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--artifact", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    artifact = (args.artifact or root / "benchmarks/security_compliance_metrics.json").resolve()
    result = audit(root, artifact)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    if result["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
