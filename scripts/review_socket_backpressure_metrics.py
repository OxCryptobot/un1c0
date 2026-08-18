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
    security = artifact["security_notes"]
    failures: list[str] = []

    for key in PHASE24_KEYS:
        if phase24.get(key) is not True:
            failures.append(f"phase24 evidence is not true: {key}")
    for key in PHASE25_KEYS:
        if phase25.get(key) is not True:
            failures.append(f"phase25 evidence is not true: {key}")
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
