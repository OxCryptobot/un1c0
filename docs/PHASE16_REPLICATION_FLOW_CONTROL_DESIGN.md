# Phase 16: Replication Backpressure and Per-Peer Flow Control

## Objective

Prevent a fast leader or slow follower from creating unbounded replication work. Phase 16 adds a bounded, transport-agnostic flow-control layer around the existing `AppendEntries` protocol. The consensus core remains responsible for message validity, log consistency, quorum commit, and state application; the caller remains responsible for authenticated transport delivery and scheduling.

## Architecture

`ReplicationFlowConfig` bounds per-peer batch bytes, entries, retry backoff, and in-flight work. A leader maintains a separate `PeerReplicationFlow` for every accepted follower. At most one `ReplicationBatch` is in flight per follower in this bounded slice. The batch binds a monotonically allocated `batch_id`, leader term, leader identity, follower identity, and the bounded `AppendEntries` request. A `ReplicationBatchAck` binds the batch ID to the follower’s existing `AppendResponse`.

The wire additions are typed `ConsensusMessage::ReplicationBatch` and `ConsensusMessage::ReplicationBatchAck` variants. They are carried through the existing authenticated envelope; the flow-control layer does not open sockets or bypass sender, cluster, term, replay, or consent policy.

## Send path

`prepare_flow_controlled_replication(follower_id, now_tick)` first validates leader role and accepted membership. It returns `Idle` when the follower is caught up, `Backpressured` when a batch is already in flight or retry backoff has not elapsed, and `Send(ReplicationBatch)` when bounded work is available. The batch selector respects both entry and serialized-byte limits and always fails closed if even one entry cannot fit.

The leader records the batch only after the complete request has passed validation. A caller must not retry a sent batch by creating a new ID while the original remains in flight. The `ReplicationWindowStatus` exposes bounded diagnostic counters without exposing secrets or source contents.

## Acknowledgement path

`acknowledge_flow_controlled_replication` validates the batch ID, follower identity, response term, and active in-flight window before delegating the existing quorum-progress update. A successful acknowledgement releases the window and advances replication progress. A failed acknowledgement releases the window but sets a bounded retry deadline. Higher terms step the node down and clear flow state. Unknown, duplicate, mismatched, or stale batch acknowledgements fail closed without mutating replication progress.

## Safety and liveness boundaries

Backpressure is explicit rather than silent: the caller receives a typed status and can schedule a later attempt. A per-peer window prevents one slow follower from blocking unrelated peers. Retry backoff prevents tight failure loops. Membership changes rebuild accepted peer state, and step-down clears in-flight batches so an old leader cannot continue replication after losing authority. The existing quorum commit rule remains authoritative; a batch being sent or acknowledged never makes an entry committed by itself.

## Validation plan

Tests cover byte and entry bounds, one-batch-per-peer backpressure, independent peer windows, idle caught-up peers, failed-ack retry backoff, successful-ack release, unknown and duplicate batch IDs, higher-term step-down, membership rejection, exact retry-boundary behavior, and the invariant that local append plus an in-flight batch does not advance commit without quorum.

## Production boundary

Production still requires transport-level connection flow control, cancellation, peer bandwidth quotas, durable retry state, metrics export, adaptive windows, cross-node packet loss/reordering, and integration with socket backpressure. This bounded in-process slice deliberately avoids background workers and does not claim WAN throughput capacity.
