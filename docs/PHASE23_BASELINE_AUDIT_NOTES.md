# Phase 23 Baseline Audit Notes

## Observed facts

The consensus core currently compacts locally through `compact_committed_log`. It validates committed/applied targets, discard and retained-suffix bounds, derives a configuration-bound snapshot, drains the retained prefix, and stores the snapshot in memory. `replication_progress` is tracked per follower, and `replication_catch_up_for` returns `Incremental`, `Snapshot`, or `Idle` based on the follower frontier.

A follower whose append predecessor is inside the compacted prefix returns `ConsensusError::SnapshotRequired`. An incremental delta with a base behind the compacted frontier also returns `SnapshotRequired`. These errors are safe signals but do not currently carry a typed follower request, requested frontier, reason, retry boundary, or correlation identifier.

The existing snapshot transfer state is leader-owned and transport-agnostic. It has exact snapshot/hash/frontier binding, per-follower transfer readiness, byte accounting, bandwidth windows, cancellation, and installed-only progress. The core does not schedule compaction, send messages, persist request intent, or own remote quorum coordination.

## Risks

A leader can compact locally without a typed view of which followers are safe to compact around, causing avoidable snapshot catch-up work or making coordination policy implicit in callers. A follower can detect that incremental replication is impossible but only return an unstructured error, forcing transport callers to infer a snapshot request and potentially losing the precise compacted frontier and configuration binding. Repeated requests can also lack bounded retry and deduplication semantics.

## Phase 23 design direction

Add `CompactionCoordinationConfig`, `CompactionFollowerStatus`, `CompactionCoordinationPlan`, and `CompactionCoordinationAction` to expose a deterministic read-only coordination plan and an approval-controlled compaction decision. Add `SnapshotRequestReason`, `SnapshotRequest`, and `SnapshotRequestAction` for typed follower-triggered requests. Bind requests to cluster/node identity, term, compacted frontier, configuration hash, requested snapshot hash when known, bounded reason text, and exact retry tick. Return requests from explicit follower APIs and preserve existing `SnapshotRequired` errors for backward compatibility.

## Validation requirements

Tests must cover leader-only coordination, stale/unknown follower status, minimum follower frontier admission, joint-configuration majority safety, no mutation on waiting/rejected plans, exact snapshot request binding, follower append and delta request generation, deduplication and retry boundaries, higher-term invalidation, clock uncertainty, and deployment ownership remaining outside the consensus core.
