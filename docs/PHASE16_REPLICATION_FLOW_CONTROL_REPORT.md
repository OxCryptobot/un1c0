# Phase 16 Replication Flow Control

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded replication batches, per-peer in-flight windows, backpressure, retry backoff, and authenticated batch acknowledgements
**Author:** Manus AI

## Architectural design

Phase 16 adds a bounded flow-control layer around the existing `AppendEntries` replication protocol. `ReplicationFlowConfig` limits entries per batch, serialized batch bytes, and retry backoff. `ReplicationBatch` binds a positive batch ID, current term, leader identity, follower identity, and a bounded append request. `ReplicationBatchAck` binds the batch ID and follower identity to the existing `AppendResponse`.

The consensus core returns typed `ReplicationFlowAction` values: `Idle` when a follower is caught up, `Backpressured` when a peer already has an in-flight batch or is inside retry backoff, and `Send(ReplicationBatch)` when bounded work can be issued. The caller remains responsible for scheduling and carrying the batch over the existing authenticated transport. The new message variants are term-extractable through the same authenticated envelope boundary and do not open sockets or spawn workers.

## Implementation behavior

Each accepted follower has an independent `PeerReplicationFlow`. At most one batch is in flight per follower. Batch validation occurs before flow state mutation, preventing an oversized or malformed batch from consuming a batch ID or creating phantom in-flight state. A successful acknowledgement releases the window, records the completed batch, and delegates replication progress and quorum commit to the existing `acknowledge_append` path. A failed acknowledgement releases the window and sets a bounded retry deadline.

The exact retry deadline is eligible for a new send. Unknown, duplicate, mismatched, and stale batch acknowledgements fail closed without advancing replication progress. A higher-term response steps the node down and clears every peer window. Membership rebuilds prune removed peers and initialize accepted followers independently. Local append and send admission never advance commit; only the existing quorum rule does.

## Phase 16 feature matrix

| Feature | Contract | Evidence |
|---|---|---|
| Entry bound | `max_entries_per_batch` is positive and no larger than the existing batch bound | Configuration and oversized-batch tests |
| Serialized-byte bound | Batch JSON must fit within the configured bounded byte window | No-partial-mutation test |
| Per-peer backpressure | One in-flight batch per follower | Independent window test |
| Retry backoff | Failed acknowledgement blocks until, but not beyond, the exact retry tick | Exact-boundary test |
| Successful release | Matching successful acknowledgement clears in-flight state and records completion | Follower round-trip test |
| Stale-leader safety | Higher-term acknowledgement clears flow state and forces follower role | Higher-term test |
| Clock safety | Clock uncertainty blocks new sends until explicit re-anchoring | Clock-regression test |

## Validation

The Phase 16 integration suite contains seven tests. It covers one-window-per-peer behavior, independent peer windows, successful follower acknowledgement and quorum commit, failed acknowledgement retry backoff, exact retry-boundary eligibility, invalid batch no-mutation, duplicate and mismatched acknowledgements, higher-term invalidation, unknown/self membership rejection, and clock-uncertainty blocking.

## Production boundaries

Production still requires transport-level connection flow control, cancellation, peer bandwidth quotas, durable retry state, adaptive windows, per-peer metrics, packet loss and reordering tests, and integration with authenticated socket backpressure. This in-process slice provides bounded safety contracts and does not claim WAN capacity.
