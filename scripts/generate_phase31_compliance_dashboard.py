"""Generate an interactive Phase 31 compliance and trace-seal dashboard."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import plotly.graph_objects as go
from plotly.subplots import make_subplots


def load(path: Path):
    return json.loads(path.read_text())


def phase_for_gate(gate: str) -> str:
    phase_map = {
        "phase11": {"phase11_membership_change"},
        "phase15": {"election_timer_safety", "failure_detector_boundaries"},
        "phase16": {"replication_flow_control", "replication_backpressure_boundaries"},
        "phase17": {"remote_audit_ordering", "remote_audit_outbox_durability"},
        "phase18": {"log_compaction_safety", "configuration_bound_snapshots"},
        "phase19": {"durable_compaction_manifests", "compaction_recovery"},
        "phase20": {"snapshot_install_readiness", "snapshot_ack_binding"},
        "phase21": {"snapshot_transfer_metrics", "snapshot_bandwidth_backpressure", "snapshot_cancellation", "snapshot_completion_accounting"},
        "phase22": {"durable_term_vote_persistence", "durable_consensus_state_recovery", "epoch_bound_replay_window", "replay_term_floor"},
        "phase23": {"cross_node_compaction_coordination", "compaction_quorum_admission", "follower_triggered_snapshot_requests", "snapshot_request_binding"},
        "phase24": {"socket_layer_backpressure", "per_peer_socket_quotas", "receive_window_admission", "socket_quota_epoch_reset"},
        "phase25": {"durable_transport_queue", "quota_recovery_after_restart", "atomic_queue_cutover", "queue_epoch_binding"},
        "phase26": {"authenticated_durable_delivery", "socket_boundary_crash_injection", "crash_retry_queue_retention", "authenticated_delivery_ack_order"},
        "phase27": {"replicated_ack_quorum", "cross_host_queue_ownership", "authenticated_ack_binding", "failover_owner_lease"},
        "phase28": {"quorum_loss_fences_delivery", "lease_expiry_fences_delivery", "fence_survives_restart", "ownership_transfer_clears_fence"},
        "phase29": {"authenticated_remote_fence_observation", "remote_fence_owner_term_binding", "remote_fence_idempotent", "remote_fence_misbinding_rejected"},
        "phase30": {"region_topology_deterministic", "asymmetric_partition_replayable", "observer_quorum_admission", "split_brain_commit_exclusion", "stale_owner_fenced_after_heal", "transfer_crash_recovers_safely", "clock_skew_boundary_fail_closed", "multi_region_retry_reaches_quorum"},
        "phase31": {"signed_replay_manifest_required", "replay_schedule_hash_bound", "replay_sequence_tick_bounds_enforced", "trusted_key_cluster_epoch_binding", "tampered_schedule_rejected", "trace_seal_verification"},
    }
    for phase, gates in phase_map.items():
        if gate in gates:
            return phase
    return "baseline"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--metrics", type=Path, required=True)
    parser.add_argument("--audit", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--png", type=Path, required=True)
    args = parser.parse_args()

    metrics = load(args.metrics)
    audit = load(args.audit)
    gates = metrics["gates"]
    gate_names = sorted(gates)
    gate_phase = [phase_for_gate(name) for name in gate_names]
    gate_status = [1 if gates[name] == "passed" else 0 for name in gate_names]

    family_counts = {}
    for phase in gate_phase:
        family_counts[phase] = family_counts.get(phase, 0) + 1
    family_names = sorted(family_counts)
    family_values = [family_counts[name] for name in family_names]

    partition = metrics["authenticated_consensus_partition"]["scenarios"]
    partition_names = [row["name"] for row in partition]
    partition_p95 = [row["verification_p95_us"] for row in partition]
    overhead = metrics["phase31_trace_seal_overhead"]
    overhead_names = ["p50", "p95", "p99", "mean"]
    overhead_values = [overhead["p50_us"], overhead["p95_us"], overhead["p99_us"], overhead["mean_us"]]

    fig = make_subplots(
        rows=2,
        cols=2,
        subplot_titles=(
            f"82-gate status ({audit['passed_gate_count']}/{audit['expected_gate_count']} passed)",
            "Gate count by control family",
            "Authenticated partition verification p95",
            "Trace-seal verification overhead",
        ),
        specs=[[{"type": "heatmap"}, {"type": "bar"}], [{"type": "bar"}, {"type": "bar"}]],
        horizontal_spacing=0.12,
        vertical_spacing=0.18,
    )
    fig.add_trace(
        go.Heatmap(
            z=[gate_status],
            x=gate_names,
            y=["gate"],
            colorscale=[[0, "#b42318"], [1, "#027a48"]],
            zmin=0,
            zmax=1,
            showscale=False,
            hovertemplate="%{x}<br>status: passed<extra></extra>",
        ),
        row=1,
        col=1,
    )
    fig.add_trace(
        go.Bar(x=family_names, y=family_values, marker_color="#3b82f6", hovertemplate="%{x}: %{y} gates<extra></extra>"),
        row=1,
        col=2,
    )
    fig.add_trace(
        go.Bar(x=partition_names, y=partition_p95, marker_color="#7c3aed", hovertemplate="%{x}: %{y:.3f} µs p95<extra></extra>"),
        row=2,
        col=1,
    )
    fig.add_trace(
        go.Bar(x=overhead_names, y=overhead_values, marker_color="#f97316", hovertemplate="%{x}: %{y:.3f} µs<extra></extra>"),
        row=2,
        col=2,
    )
    fig.update_yaxes(title_text="pass (1)", range=[-0.1, 1.1], row=1, col=1)
    fig.update_yaxes(title_text="gates", row=1, col=2)
    fig.update_yaxes(title_text="p95 µs", row=2, col=1)
    fig.update_yaxes(title_text="µs", row=2, col=2)
    fig.update_xaxes(tickangle=-45, row=1, col=1)
    fig.update_layout(
        title="un1c0 Phase 31 Security Compliance and Trace-Seal Dashboard",
        template="plotly_white",
        height=900,
        width=1500,
        margin={"l": 70, "r": 40, "t": 100, "b": 190},
        showlegend=False,
        annotations=[
            {
                "text": "Trace-seal benchmark: in-process Ed25519 + canonical SHA-256 payload; not a network/TLS/cloud benchmark",
                "xref": "paper",
                "yref": "paper",
                "x": 0.02,
                "y": -0.06,
                "showarrow": False,
                "font": {"size": 12, "color": "#475467"},
            }
        ],
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.png.parent.mkdir(parents=True, exist_ok=True)
    fig.write_html(args.output, include_plotlyjs="inline", full_html=True)
    fig.write_image(args.png, scale=2)
    print(json.dumps({"dashboard": str(args.output), "png": str(args.png), "gate_count": len(gates), "passed": sum(gate_status), "trace_seal_p95_us": overhead["p95_us"]}, indent=2))


if __name__ == "__main__":
    main()
