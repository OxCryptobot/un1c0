# Phase 18 Log Compaction and Configuration-Bound Snapshots

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded committed-prefix compaction, logical frontier translation, configuration-bound snapshots, and snapshot-required follower catch-up
**Author:** Manus AI

## Architectural design

Phase 18 adds explicit log compaction to the transport-agnostic consensus core without changing quorum authority. `LogCompactionConfig` bounds the minimum retained suffix and maximum discard batch. `compact_committed_log` admits only a target that is both committed and applied, advances the existing compacted frontier, exists in the retained log, and leaves the configured suffix. All checks occur before the prefix is drained.

The node tracks `log_base_index` and `log_base_term`. Logical log indexes are translated through `entry_at` and `last_log_index`; the implementation no longer assumes that a vector position is the logical Raft index after compaction. Append predecessor validation accepts the compacted boundary term. Incremental deltas reject bases behind the boundary with `SnapshotRequired` instead of attempting to apply impossible history.

## Configuration-bound snapshots

`ConfigurationBoundSnapshot` binds last-included index and term, commit and applied frontiers, state and state hash, active configuration phase, current membership, previous membership, and a deterministic configuration hash. Installation validates all metadata before mutation. A state hash match is insufficient when membership metadata has been altered.

`ReplicationCatchUpAction` reports `Incremental`, `Snapshot`, or `Idle`. A follower behind the compacted prefix receives the latest configuration-bound snapshot. A follower at the retained suffix continues through incremental delta or append replication. Compaction itself never advances commit and never changes membership.

## Phase 18 feature matrix

| Feature | Evidence |
|---|---|
| Applied/committed target admission | Compaction test rejects target 5 when only target 4 is applied. |
| Bounded discard and retention | Configuration limits and retained-suffix assertions pass. |
| Logical index translation | Four entries are discarded while logical log length remains 5 and frontier becomes `(4, 1)`. |
| Snapshot-required catch-up | A follower at progress 0 receives `ReplicationCatchUpAction::Snapshot`. |
| Safe append boundary | Direct append for a follower behind the compacted prefix returns `SnapshotRequired`. |
| Configuration binding | Tampered membership fails configuration-hash validation. |
| No partial mutation | Rejected target leaves frontier and retained log unchanged. |
| Late follower installation | A new follower installs the configuration-bound snapshot and recovers state/frontier. |

## Validation

The Phase 18 integration suite passes five tests. The complete compliance validator now includes `log_compaction_safety`, `configuration_bound_snapshots`, `durable_compaction_manifests`, and `compaction_recovery` as first-class gates, reporting **30 passed gates** in the Phase 19 run. The existing Phase 13–17 suites remain part of the complete run.

## Production boundaries

Phase 18’s consensus core does not schedule compaction or own persistence. Phase 19 adds an explicit file-backed durability boundary, but production still requires storage quotas, backup retention, encryption-at-rest policy, fsync telemetry, process-crash recovery, durable membership backup, transport delivery, and cross-node compaction/catch-up testing. The implementation proves bounded deterministic decisions and configuration-aware safety without claiming a distributed storage service.
