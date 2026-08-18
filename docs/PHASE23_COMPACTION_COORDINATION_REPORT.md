# Phase 23 Cross-Node Compaction Coordination and Follower Snapshot Requests

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 23 turns implicit compaction and snapshot-recovery signals into typed, auditable contracts. Leaders now produce a hash-bound `CompactionCoordinationPlan` that reports target frontier, configuration hash, per-follower lag, safe and blocked followers, and stable or joint remote-quorum requirements. A waiting plan has no mutation; only a ready plan invokes the existing bounded compaction operation.

Followers now expose typed `SnapshotRequest` actions when an append predecessor or incremental delta base falls within compacted history. Requests bind follower and leader identity, term, reason, known frontier, optional snapshot digest, and retry tick. Leaders validate the request and delegate to the existing per-follower snapshot transfer state, preserving Phase 20 readiness and Phase 21 bandwidth/cancellation controls.

The compliance artifact increases from **40 to 44 passing gates**. The complete validator passes Rust, Python, CLI, Helm, Compose mTLS, and Phase 23 integration checks. The deep audit reports **44/44 gates passed** with zero findings.

## Coordination contract

| Contract | Safety behavior |
|---|---|
| `CompactionCoordinationConfig` | Bounds follower lag and minimum safe followers; optionally requires quorum admission. |
| `CompactionFollowerStatus` | Binds follower ID, match index, target index, lag, and safety classification. |
| `CompactionCoordinationPlan` | Hash-binds target, configuration, follower statuses, quorum requirements, and readiness. |
| `CompactionCoordinationAction::Waiting` | Reports insufficient safety without mutating the retained log. |
| `CompactionCoordinationAction::Compacted` | Carries the validated plan and resulting configuration-bound snapshot. |

For stable configurations, the required remote threshold is the leader’s quorum minus the local leader. For joint configurations, the plan uses the maximum remote quorum required by the current and previous membership sets. This is a deterministic local admission contract, not a distributed lock or a replacement for replicated coordination.

## Follower-triggered request contract

`SnapshotRequest` supports `CompactedFrontier`, `IncrementalBaseBehind`, and `AppendPredecessorCompacted` reasons. It carries a bounded request ID, follower/leader IDs, term, known snapshot frontier, optional configuration and serialized snapshot hashes, retry tick, and a canonical request hash. Retry timing is part of the content hash, so a changed retry boundary produces a distinct request identity.

The follower APIs return `SnapshotRequestAction::None` when the request is not needed and `Request` when compacted history prevents incremental progress. The leader handler rejects stale terms, unknown followers, and requests bound to another leader before touching transfer state. Valid requests delegate to existing snapshot preparation, which retains one-transfer backpressure, exact snapshot binding, bandwidth accounting, installed-only progress, and cancellation semantics.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `compaction_coordination_waits_without_mutating_when_safe_frontier_is_insufficient` | Waiting plan, blocked follower, and no log mutation | Passed |
| `compaction_coordination_admits_remote_quorum_and_returns_bound_snapshot` | Quorum-safe compaction and snapshot binding | Passed |
| `follower_requests_snapshot_for_compacted_append_and_leader_starts_transfer` | Append predecessor request and leader transfer delegation | Passed |
| `follower_requests_snapshot_for_incremental_base_and_request_hash_is_retry_bound` | Incremental-base request and retry identity | Passed |
| `stale_or_misbinding_snapshot_requests_fail_closed_without_transfer_state` | Stale term rejection and no transfer mutation | Passed |

## Production boundary

The consensus core does not own cross-node message delivery, distributed locks, durable request intent, request deduplication, compaction scheduling, or storage. Production promotion requires authenticated coordination messages, durable request/retry state, configuration-change coordination, snapshot source availability, and failure testing across compaction, restart, partition, and follower catch-up.

## References

[1]: ../src/consensus.rs "Phase 23 consensus implementation"
[2]: ../tests/phase23_compaction_coordination_snapshot_request_integration.rs "Phase 23 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Security compliance metrics artifact"
[4]: ../benchmarks/security_compliance_audit.json "Security metrics audit evidence"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture and boundaries"
