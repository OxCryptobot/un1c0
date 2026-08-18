# Phase 18 Test and Snapshot-Catch-Up Walkthrough

## Compaction fixture

The integration test builds a three-node cluster, elects `node-a`, proposes four commands, replicates them to `node-b`, and then proposes one uncommitted tail command. Commit and applied indexes are therefore 4 while the logical log frontier is 5. `LogCompactionConfig::new(1, 4)` requires at least one retained suffix entry and permits discarding at most four entries.

## Prefix compaction

`compaction_discards_only_applied_prefix_and_preserves_retained_suffix` calls `compact_committed_log(4)`. The test expects last-included index 4, last-included term 1, compacted frontier `(4, 1)`, one retained physical suffix entry, and logical `log_len() == 5`. The key/value state from the discarded prefix remains available. The uncommitted tail is never discarded.

## Snapshot-required boundary

`follower_behind_compacted_prefix_receives_configuration_bound_snapshot` leaves `node-c` at replication progress 0. Because 0 is below the leader’s compacted frontier 4, `replication_catch_up_for("node-c")` must return `ReplicationCatchUpAction::Snapshot` with the exact configuration-bound snapshot. Direct `append_entries_for("node-c")` returns `SnapshotRequired`, proving the leader will not construct an impossible predecessor inside discarded history.

A new `node-c` installs the snapshot and must recover compacted frontier `(4, 1)` and the state from the discarded prefix. A follower at or beyond frontier 4 would instead remain eligible for append or incremental-delta replication.

## No-mutation and binding boundaries

`invalid_compaction_target_and_configuration_tampering_fail_without_mutation` requests target 5 while only target 4 is applied. The call returns `LogCompaction` and leaves frontier `(0, 0)` and all five retained entries unchanged. It then changes membership without recomputing `configuration_hash`; validation returns `InvalidSnapshot`.

`configuration_bound_snapshot_requires_consistent_metadata` supplies invalid state and configuration digests and expects `InvalidSnapshot`. `compaction_configuration_rejects_unsafe_bounds` rejects zero discard and oversized retention before any compaction attempt.

## Phase 19 durable boundary

Phase 19 extends these boundaries to disk. The durable tests stage a snapshot and manifest, verify that staged files are not visible through `load_latest`, commit and reload an identical pair, remove one staged file to simulate a crash, and prove recovery aborts staging while preserving the prior durable pair. A tampered staged digest is removed without promotion, and repeated recovery returns `NoStaging`.
