#!/usr/bin/env python3
"""Review non-secret socket backpressure and durable queue evidence."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


PHASE24_KEYS = [
    "per_peer_send_isolation",
    "exact_frame_byte_admission",
    "send_quota_release",
    "receive_window_backpressure",
    "authentication_precedes_quota_mutation",
    "epoch_rotation_clears_quota_state",
    "legacy_transport_compatibility",
]
PHASE25_KEYS = [
    "queue_frames_are_hash_bound",
    "exact_quota_recovery_after_restart",
    "atomic_save_and_staging_cleanup",
    "fifo_ack_required",
    "epoch_mismatch_rejected",
    "persistence_failure_rolls_back",
]
PHASE26_KEYS = [
    "authenticated_payload_reverified_before_send",
    "one_active_delivery_per_peer",
    "ack_only_after_flush",
    "restart_retry_after_crash",
    "tampered_payload_fails_closed",
    "durable_delivery_metrics_non_secret",
]
PHASE28_KEYS = [
    "quorum_loss_fences_delivery",
    "lease_expiry_fences_delivery",
    "fence_survives_restart",
    "ownership_transfer_clears_fence",
]
PHASE29_KEYS = [
    "authenticated_remote_fence_observation",
    "remote_fence_owner_term_binding",
    "remote_fence_idempotent",
    "remote_fence_misbinding_rejected",
]
PHASE30_KEYS = [
    "region_topology_is_deterministic",
    "asymmetric_partition_is_replayable",
    "observer_quorum_admission",
    "split_brain_commit_exclusion",
    "stale_owner_is_fenced_after_heal",
    "transfer_crash_recovers_safely",
    "clock_skew_boundary_is_fail_closed",
    "multi_region_retry_reaches_quorum",
]
PHASE31_KEYS = [
    "signed_replay_manifest_required",
    "replay_schedule_hash_bound",
    "replay_sequence_tick_bounds_enforced",
    "trusted_key_cluster_epoch_binding",
    "tampered_schedule_rejected",
    "trace_seal_verification",
]
PHASE32_KEYS = [
    "signed_region_failure_observation_required",
    "distinct_observer_quorum_required",
    "snapshot_hash_binding_required",
    "higher_term_epoch_promotion_required",
    "old_region_fenced_on_commit",
    "single_active_region_invariant",
    "idempotent_failover_evidence",
    "stale_or_conflicting_failover_rejected",
]
PHASE34_KEYS = [
    "joint_observer_quorum_required",
    "joint_to_final_membership_ordering",
    "replicated_recovery_log_hash_bound",
    "external_fencing_token_signature_required",
    "fencing_epoch_monotonicity",
    "stale_external_fence_rejected",
    "replicated_authority_restart_continuity",
    "dynamic_partition_epoch_chaos_safe",
]
PHASE35_KEYS = [
    "multi_leader_proposal_signature_required",
    "witness_quorum_arbitration_required",
    "one_witness_vote_per_round",
    "conflicting_quorum_split_brain_rejected",
    "stale_multi_leader_log_rejected",
    "fencing_token_domain_bound",
    "fencing_authority_registry_pinned",
    "fencing_generation_rollback_rejected",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact", default="benchmarks/security_compliance_metrics.json")
    parser.add_argument("--output", default="benchmarks/socket_backpressure_metrics_review.json")
    args = parser.parse_args()

    artifact_path = Path(args.artifact).resolve()
    output_path = Path(args.output).resolve()
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    phase24 = artifact["phase24_socket_backpressure_quotas"]
    phase25 = artifact["phase25_durable_transport_queues"]
    phase26 = artifact["phase26_authenticated_durable_delivery"]
    phase28 = artifact["phase28_partition_ownership_fencing"]
    phase29 = artifact["phase29_authenticated_remote_fencing"]
    phase30 = artifact["phase30_multi_region_failover"]
    phase31 = artifact["phase31_secure_replay_verification"]
    phase32 = artifact["phase32_disaster_recovery_failover"]
    phase34 = artifact["phase34_replicated_recovery"]
    phase35 = artifact["phase35_multileader_witness"]
    security = artifact["security_notes"]
    failures: list[str] = []

    for key in PHASE24_KEYS:
        if phase24.get(key) is not True:
            failures.append(f"phase24 evidence is not true: {key}")
    for key in PHASE25_KEYS:
        if phase25.get(key) is not True:
            failures.append(f"phase25 evidence is not true: {key}")
    for key in PHASE26_KEYS:
        if phase26.get(key) is not True:
            failures.append(f"phase26 evidence is not true: {key}")
    for key in PHASE28_KEYS:
        if phase28.get(key) is not True:
            failures.append(f"phase28 evidence is not true: {key}")
    for key in PHASE29_KEYS:
        if phase29.get(key) is not True:
            failures.append(f"phase29 evidence is not true: {key}")
    if phase29.get("failure_detector_quorum_authority") != "deployment_boundary":
        failures.append("phase29 failure-detector quorum authority is not deployment-bound")
    for key in PHASE30_KEYS:
        if phase30.get(key) is not True:
            failures.append(f"phase30 evidence is not true: {key}")
    if phase30.get("transport_and_cloud_region_authority") != "deployment_boundary":
        failures.append("phase30 cloud-region authority is not deployment-bound")
    for key in PHASE31_KEYS:
        if phase31.get(key) is not True:
            failures.append(f"phase31 evidence is not true: {key}")
    if phase31.get("production_key_custody_and_transport") != "deployment_boundary":
        failures.append("phase31 key custody and transport are not deployment-bound")
    for key in PHASE32_KEYS:
        if phase32.get(key) is not True:
            failures.append(f"phase32 evidence is not true: {key}")
    if phase32.get("production_cloud_authority") != "deployment_boundary":
        failures.append("phase32 cloud authority is not deployment-bound")
    for key in PHASE34_KEYS:
        if phase34.get(key) is not True:
            failures.append(f"phase34 evidence is not true: {key}")
    if phase34.get("transport_process_fencing_and_external_registry") != "deployment_boundary":
        failures.append("phase34 transport, process fencing, and external registry are not deployment-bound")
    for key in PHASE35_KEYS:
        if phase35.get(key) is not True:
            failures.append(f"phase35 evidence is not true: {key}")
    if phase35.get("transport_failure_detector_and_process_fencing") != "deployment_boundary":
        failures.append("phase35 transport, failure detector, and process fencing are not deployment-bound")
    if phase28.get("network_failure_detector_and_clock_authority") != "deployment_boundary":
        failures.append("phase28 failure detector and clock authority is not deployment-bound")
    if phase26.get("crash_points_retaining_queue") != 4:
        failures.append("phase26 crash-point coverage is not exactly four")
    if phase24.get("socket_threads_and_scheduler") != "deployment_boundary":
        failures.append("socket thread and scheduler ownership is not deployment-bound")
    if phase25.get("socket_queue_threads_and_replication") != "deployment_boundary":
        failures.append("durable queue thread and replication ownership is not deployment-bound")
    if security.get("secret_material_recorded") is not False:
        failures.append("secret-material policy is not false")
    if security.get("cluster_mutation_performed") is not False:
        failures.append("cluster mutation policy is not false")

    result = {
        "artifact": str(artifact_path),
        "gate_count": len(artifact["gates"]),
        "phase24_evidence_checked": PHASE24_KEYS,
        "phase25_evidence_checked": PHASE25_KEYS,
        "phase26_evidence_checked": PHASE26_KEYS,
        "phase26_crash_points_retaining_queue": phase26.get("crash_points_retaining_queue"),
        "phase28_evidence_checked": PHASE28_KEYS,
        "phase29_evidence_checked": PHASE29_KEYS,
        "phase30_evidence_checked": PHASE30_KEYS,
        "phase31_evidence_checked": PHASE31_KEYS,
        "phase32_evidence_checked": PHASE32_KEYS,
        "phase34_evidence_checked": PHASE34_KEYS,
        "phase35_evidence_checked": PHASE35_KEYS,
        "runtime_metric_contract": [
            "in_flight_bytes",
            "receive_window_bytes",
            "admitted_frames",
            "rejected_frames",
            "backpressured_sends",
            "backpressured_receives",
            "durable_queue_frames",
            "durable_queue_bytes",
            "next_queue_sequence",
            "durable_delivery_attempts",
            "durable_delivery_failures",
            "injected_delivery_crashes",
        ],
        "runtime_payloads_persisted_in_artifact": False,
        "runtime_payload_boundary": "deployment_boundary",
        "secret_material_recorded": security.get("secret_material_recorded"),
        "cluster_mutation_performed": security.get("cluster_mutation_performed"),
        "failures": failures,
        "result": "passed" if not failures else "failed",
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
