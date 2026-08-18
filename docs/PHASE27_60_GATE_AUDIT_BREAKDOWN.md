# Phase 27 60-Gate Security and Compliance Audit Breakdown

**Scope:** The published Phase 27 artifact before Phase 28 changes.
**Audit result:** 60 expected, 60 observed, 60 passed; no secret material recorded; no cluster mutation performed.

## How to read the audit

The machine-readable artifact records each gate as `passed`. The independent auditor verifies the expected count, every gate value, phase evidence fields, benchmark shape, commit ancestry, secret policy, and deployment-boundary claims. The 60-gate inventory below is the exact gate set from the Phase 27 artifact, not a replacement for executable tests.

## Gate inventory

| Family | Gates | What the family establishes |
|---|---|---|
| Baseline execution and delivery | `skill_validation`, `rust_all_targets`, `python_tests`, `cli_smoke`, `helm_fail_closed`, `compose_mtls_smoke` | The reusable workflow, Rust targets, Python suite, CLI, Helm fail-closed policies, and isolated Compose/mTLS smoke path execute successfully. |
| Consensus and authenticated transport | `snapshot_installation`, `authenticated_consensus_transport`, `authenticated_socket_transport`, `replay_window_and_cluster_binding`, `authenticated_partition_benchmark`, `phase11_membership_change`, `failure_injection_snapshot_recovery`, `network_stress_packet_corruption` | Snapshot installation, authenticated envelopes, bounded socket framing, cluster/replay identity, membership transitions, crash recovery, and packet-corruption handling are covered. |
| Read consistency and timing | `leader_lease_read_optimization`, `linearizable_read_consistency`, `election_timer_safety`, `failure_detector_boundaries` | Lease fast paths, quorum read-index behavior, bounded election timers, heartbeat cadence, failure suspicion, and clock-safe boundaries are tested. |
| Replication flow and remote audit | `replication_flow_control`, `replication_backpressure_boundaries`, `remote_audit_ordering`, `remote_audit_outbox_durability` | Per-follower windows, retry boundaries, backpressure, signed stream ordering, durable outbox replay, gap retention, and acknowledgement handling are validated. |
| Compaction and snapshots | `log_compaction_safety`, `configuration_bound_snapshots`, `durable_compaction_manifests`, `compaction_recovery`, `snapshot_install_readiness`, `snapshot_ack_binding`, `snapshot_transfer_metrics`, `snapshot_bandwidth_backpressure`, `snapshot_cancellation`, `snapshot_completion_accounting`, `snapshot_chunk_streaming`, `incremental_state_sync`, `follower_triggered_snapshot_requests`, `cross_node_compaction_coordination`, `compaction_quorum_admission`, `snapshot_request_binding` | Compaction frontiers, configuration hashes, atomic manifests, snapshot lifecycle, exact byte accounting, bandwidth windows, cancellation, chunk assembly, incremental catch-up, follower requests, and remote quorum admission fail closed. |
| Durable state and replay | `durable_term_vote_persistence`, `durable_consensus_state_recovery`, `epoch_bound_replay_window`, `replay_term_floor`, `queue_epoch_binding` | Durable term/vote state, canonical hashes, rollback rejection, signed replay epochs, stale-term floors, and queue epoch identity survive restart safely. |
| Socket quotas and durable queues | `socket_layer_backpressure`, `per_peer_socket_quotas`, `receive_window_admission`, `socket_quota_epoch_reset`, `durable_transport_queue`, `quota_recovery_after_restart`, `atomic_queue_cutover`, `queue_epoch_binding`, `socket_boundary_crash_injection`, `crash_retry_queue_retention` | Serialized-byte admission, isolated send/receive quotas, epoch reset, atomic queue snapshots, restart recovery, partial-write retention, and retry after crash are bounded and testable. |
| Authenticated durable delivery | `authenticated_durable_delivery`, `authenticated_delivery_ack_order`, `authenticated_ack_binding`, `replicated_ack_quorum`, `cross_host_queue_ownership`, `failover_owner_lease`, `crash_retry_queue_retention` | Payloads are re-verified before send, post-flush acknowledgement is required, acknowledgement hashes bind frame/owner/term/epoch, quorum prevents single-node commit, and ownership transfer supports failover without stale authority. |
| Evolution and audit custody | `signer_rotation_revocation`, `durable_external_audit_sink` | Signer lifecycle, revocation, durable external audit outbox behavior, idempotence, and recovery are covered. |

The family table explains the control surfaces. The canonical inventory below lists each of the **60 unique Phase 27 gate names exactly once**, in the same lexical order used by the JSON artifact:

| 1–15 | 16–30 | 31–45 | 46–60 |
|---|---|---|---|
| `atomic_queue_cutover` | `durable_compaction_manifests` | `log_compaction_safety` | `rust_all_targets` |
| `authenticated_ack_binding` | `durable_consensus_state_recovery` | `network_stress_packet_corruption` | `signer_rotation_revocation` |
| `authenticated_consensus_transport` | `durable_external_audit_sink` | `per_peer_socket_quotas` | `skill_validation` |
| `authenticated_delivery_ack_order` | `durable_term_vote_persistence` | `phase11_membership_change` | `snapshot_ack_binding` |
| `authenticated_durable_delivery` | `durable_transport_queue` | `python_tests` | `snapshot_bandwidth_backpressure` |
| `authenticated_partition_benchmark` | `election_timer_safety` | `queue_epoch_binding` | `snapshot_cancellation` |
| `authenticated_socket_transport` | `epoch_bound_replay_window` | `quota_recovery_after_restart` | `snapshot_chunk_streaming` |
| `cli_smoke` | `failover_owner_lease` | `receive_window_admission` | `snapshot_completion_accounting` |
| `compaction_quorum_admission` | `failure_detector_boundaries` | `remote_audit_ordering` | `snapshot_install_readiness` |
| `compaction_recovery` | `failure_injection_snapshot_recovery` | `remote_audit_outbox_durability` | `snapshot_installation` |
| `compose_mtls_smoke` | `follower_triggered_snapshot_requests` | `replay_term_floor` | `snapshot_request_binding` |
| `configuration_bound_snapshots` | `helm_fail_closed` | `replay_window_and_cluster_binding` | `snapshot_transfer_metrics` |
| `crash_retry_queue_retention` | `incremental_state_sync` | `replicated_ack_quorum` | `socket_boundary_crash_injection` |
| `cross_host_queue_ownership` | `leader_lease_read_optimization` | `replication_backpressure_boundaries` | `socket_layer_backpressure` |
| `cross_node_compaction_coordination` | `linearizable_read_consistency` | `replication_flow_control` | `socket_quota_epoch_reset` |

> The machine-readable metrics artifact and independent audit JSON remain authoritative. The table is a human-readable index of the exact 60 names.

## Phase 27-specific evidence

Phase 27 added the following nine evidence fields, of which eight are executable booleans and one is an explicit deployment boundary:

| Evidence | Meaning |
|---|---|
| `quorum_ack_required` | A flushed frame remains queued until the configured distinct-sender quorum is reached. |
| `ack_hash_owner_term_epoch_bound` | Acknowledgement content is bound to peer, sequence, frame digest, owner, term, and ownership epoch. |
| `idempotent_same_sender_ack` | Replaying the same sender/hash does not inflate quorum state. |
| `conflicting_sender_ack_rejected` | A sender cannot replace its prior acknowledgement with a different hash. |
| `ownership_transfer_lease_and_term_bound` | Transfers require correct sender/destination, higher term/epoch, and lease authority checks. |
| `cross_host_restore_validates_source_identity` | Imported queue state must identify the persisted source node explicitly. |
| `failover_new_owner_can_retry` | The new owner can retry the retained FIFO frame after valid transfer. |
| `old_owner_cannot_ack_after_transfer` | A node that no longer owns the queue cannot commit the frame locally. |
| `transport_and_replica_quorum` | Real cross-host transport and replica-quorum scheduling remain a deployment boundary. |

## Partition benchmark qualification

The artifact’s authenticated partition benchmark models a five-member configuration. Healthy connectivity verifies 10,000 messages; a three-member majority partition verifies 3,600 and drops 6,400 while retaining quorum; a two-member minority partition verifies 1,600, drops 8,400, and has no quorum. The benchmark drops messages before verification and measures in-process Ed25519 work. It is not a socket, TLS, kernel, or cross-machine network benchmark. Phase 28 adds local durable ownership fencing but does not retroactively change this Phase 27 artifact.

## References

[1]: https://github.com/OxCryptobot/un1c0/blob/f5ce43b2b0cf8a021c307a9afb5c78fac84d957e/benchmarks/security_compliance_metrics.json "Published Phase 27 security metrics at commit f5ce43b"
[2]: https://github.com/OxCryptobot/un1c0/blob/f5ce43b2b0cf8a021c307a9afb5c78fac84d957e/benchmarks/security_compliance_audit.json "Independent Phase 27 metrics audit at commit f5ce43b"
[3]: https://github.com/OxCryptobot/un1c0/blob/main/scripts/collect_security_compliance_metrics.py "Compliance metrics collector"
[4]: https://github.com/OxCryptobot/un1c0/blob/main/scripts/audit_security_compliance_metrics.py "Independent metrics auditor"
[5]: https://github.com/OxCryptobot/un1c0/blob/main/tests/phase27_replicated_delivery_ownership_integration.rs "Phase 27 integration evidence"
[6]: https://github.com/OxCryptobot/un1c0/blob/main/docs/PHASE27_REPLICATED_DELIVERY_OWNERSHIP_REPORT.md "Phase 27 implementation report"
