#!/usr/bin/env python3
"""Audit the non-secret security compliance metrics artifact."""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

EXPECTED_GATE_COUNT = 44
EXPECTED_CONCURRENCIES = [1, 2, 4, 8, 16, 32]
INTENTIONAL_FALSE_EVIDENCE = {
    "phase15_election_timers.transport_or_background_threads",
    "phase16_replication_flow_control.transport_or_background_threads",
}

REQUIRED_PHASES = {
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
