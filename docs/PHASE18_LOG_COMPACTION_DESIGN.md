# Phase 18: Log Compaction and Configuration-Bound Snapshots

## Objective

Add bounded log compaction to the transport-agnostic consensus core without weakening quorum commit, snapshot installation, joint-consensus membership, or follower catch-up semantics. Compaction is admitted only through an explicitly committed frontier and produces a snapshot bound to the active membership configuration.

## Compaction contract

`LogCompactionConfig` defines a bounded minimum retained suffix and maximum discard batch. `ConsensusNode::compact_committed_log` accepts only a target index at or below the applied frontier, at or above the existing snapshot frontier, and within the configured discard bound. It creates a deterministic `ConfigurationBoundSnapshot` containing term, commit index, last-applied index, active configuration phase, old/new membership sets, state hash, and configuration hash before removing only entries that are covered by the snapshot. The newest retained entry remains contiguous after the compaction boundary.

The compaction operation is atomic at the state-machine level: all validation occurs before mutation. The node retains a compacted-prefix marker so subsequent append validation can use the snapshot boundary as the predecessor frontier. A target beyond applied state, a target below the existing frontier, an oversized discard, a malformed configuration, or a mismatched snapshot hash fails without mutating the log or snapshot state.

## Snapshot binding

The existing `ReplicatedSnapshot` remains compatible for legacy callers. Phase 18 adds configuration metadata and a configuration hash to the new snapshot type. Snapshot installation must verify state hash, configuration hash, membership bounds, term/index monotonicity, and the active joint/stable configuration before replacing state. A snapshot from another cluster configuration is rejected even when its key/value state hash matches.

## Replication behavior

A follower whose `next_index` falls inside the compacted prefix must use the configuration-bound snapshot path rather than receiving an impossible predecessor entry. A follower at or beyond the retained suffix continues to use normal append or incremental delta replication. Compaction never changes quorum membership and never marks an entry committed; it only discards already-applied history below the snapshot frontier.

## Failure semantics

| Condition | Result |
|---|---|
| Target above applied index | Reject; no mutation. |
| Target below current compacted frontier | Idempotent no-op only when metadata matches; otherwise reject. |
| Discard exceeds configured batch | Reject; log and snapshot unchanged. |
| State/configuration hash mismatch | Reject snapshot installation. |
| Joint configuration mismatch | Reject; do not collapse membership state. |
| Follower behind compacted prefix | Return snapshot-required action. |
| Process failure during compaction | Original in-memory state remains authoritative until the caller persists the snapshot atomically. |

## Production boundary

The core does not persist files or decide when to schedule compaction. Durable snapshot replacement, fsync, crash recovery, retention policy, and transport delivery remain caller responsibilities. Phase 18 proves deterministic, bounded, configuration-aware compaction decisions in-process.
