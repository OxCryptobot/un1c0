# Phase 20: Replicated Snapshot Acknowledgements and Install Readiness

## Objective

Close the replication boundary between Phase 18 snapshot-required catch-up and Phase 19 durable local cutover. Leaders need a bounded, authenticated-friendly snapshot transfer lifecycle that records which follower has validated, durably staged, and safely installed a configuration-bound snapshot. A follower must not report progress at the snapshot frontier before durable installation is complete.

## Contracts

`SnapshotInstallReadiness` describes a follower’s lifecycle: `Unknown`, `Receiving`, `Validated`, `DurablyStaged`, `Installed`, or `Rejected`. `SnapshotInstallAck` binds follower ID, transfer ID, term, last-included index/term, configuration hash, snapshot hash, readiness state, and an optional rejection reason. `SnapshotReplicationState` tracks one active transfer per follower, retry deadline, last acknowledged frontier, and bounded counters.

## Leader behavior

When `replication_catch_up_for` selects a snapshot, the leader creates a bounded transfer ID and records an active per-follower transfer. A follower acknowledgement is accepted only if it matches the active transfer, current term, exact snapshot hashes, exact configuration hash, and accepted membership. `Validated` and `DurablyStaged` acknowledgements advance readiness but do not advance replication progress. Only `Installed` advances follower replication progress to the snapshot’s last-included index. Rejected or stale acknowledgements release the transfer with a bounded retry deadline.

## Follower behavior

The follower validates the configuration-bound snapshot before reporting `Validated`. After the Phase 19 durable store stages and fsyncs the snapshot/manifest pair, the caller reports `DurablyStaged`. After atomic cutover and local state installation, the caller reports `Installed`. The consensus core does not perform transport, file I/O, or background scheduling; it only validates the typed lifecycle and returns the next action.

## Safety invariants

| Invariant | Required behavior |
|---|---|
| One transfer per follower | A second transfer is backpressured until the active transfer is completed or retried. |
| Exact binding | Transfer ID, term, frontier, snapshot hash, and configuration hash must match. |
| No premature progress | `Validated` and `DurablyStaged` never advance replication progress. |
| Monotonic install | An installed frontier cannot move backward. |
| Membership safety | Unknown or removed followers are rejected. |
| Retry safety | Rejected or stale acknowledgements set bounded retry state without corrupting progress. |
| Crash boundary | Durable staging is reported separately from installed state. |

## Production boundary

Transport authentication, chunk delivery, durable file operations, retry scheduling, and remote quorum remain caller responsibilities. Phase 20 adds typed state and testable leader/follower transitions without claiming a full remote snapshot service.
