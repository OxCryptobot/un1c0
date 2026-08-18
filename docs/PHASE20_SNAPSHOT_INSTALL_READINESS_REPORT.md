# Phase 20 Snapshot Install Readiness

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** replicated snapshot acknowledgements, bounded transfer lifecycle, and installed-only follower progress
**Author:** Manus AI

## Implementation

Phase 20 adds `SnapshotInstallAck`, `SnapshotInstallReadiness`, `SnapshotTransferAction`, and `SnapshotReplicationStatus` to the consensus module. A leader creates one bounded snapshot transfer per follower when Phase 18 catch-up selects the configuration-bound snapshot. The transfer binds the follower, term, last-included index and term, serialized snapshot digest, and configuration hash.

The readiness lifecycle is `Unknown -> Receiving -> Validated -> DurablyStaged -> Installed`. `Validated` proves the follower accepted the exact configuration-bound snapshot. `DurablyStaged` proves the Phase 19 durable snapshot/manifest boundary completed. Neither state advances replication progress. Only `Installed` clears the active transfer and advances follower progress to the last-included index.

Rejected acknowledgements require a bounded reason, clear the active transfer, and become retryable at `now_tick + retry_backoff_ticks`. The exact retry tick is eligible for a new send. A second request while a transfer is active returns `Backpressured`. Higher terms force step-down and clear active snapshot authority. Monotonic clock uncertainty blocks new transfer preparation.

## Evidence matrix

| Control | Evidence |
|---|---|
| One active transfer per follower | Second preparation returns `Backpressured`. |
| Exact snapshot binding | Tampered configuration hash is rejected before state mutation. |
| No premature progress | Validated and durably-staged acknowledgements leave catch-up at snapshot mode. |
| Installed-only progress | Installed acknowledgement clears active transfer and records frontier 4. |
| Retry boundary | Rejected at tick 0 is backpressured at tick 24 and sendable at tick 25. |
| Rejection reason | Rejected acknowledgement without a reason fails validation. |
| Higher-term safety | Higher-term acknowledgement steps the leader down and clears snapshot state. |
| Clock safety | Backward tick returns `ClockUntrusted` and blocks transfer preparation. |

## Production boundary

The consensus core performs no network transfer, file I/O, background scheduling, authentication, or remote quorum. Callers must connect these typed actions to authenticated chunk transport and the Phase 19 durable compaction store. Production still requires retry persistence, bandwidth quotas, transfer cancellation, crash recovery across transport and storage boundaries, and cross-host snapshot-install testing.
