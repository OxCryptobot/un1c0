# Phase 33 Durable Recovery and Observer Epoch Audit Notes

## Baseline

The repository is at published commit `6fd5f56`. The Phase 30 multi-region simulator suite passes 9 tests, the Phase 31 secure replay suite passes 8 tests, and the Phase 32 disaster-recovery suite passes 16 tests. The first baseline command attempted an unauthenticated `git ls-remote`; local tests passed, but remote verification requires the configured deploy key and must not be treated as a repository failure.

## Scope

This batch targets the two highest-priority risks identified by the Phase 32 quorum-safety audit: (1) controller state is not durably snapshotted and restart-recovered, and (2) observer membership and quorum authority are not bound to an explicit membership epoch. It also adds a deterministic distributed race harness that runs competing controller decisions against the same partition schedule and asserts that no race produces two active regions or a stale generation commit.

## Design constraints

The controller remains transport-agnostic and local. Durable recovery must use canonical JSON, SHA-256 state binding, atomic staging and rename, rollback rejection, and cleanup of partial staging. Observer membership epochs must be part of the signed observation payload and the controller's trusted registry. A membership epoch change must invalidate observations from older epochs before they can mutate quorum state. The implementation must not claim real cloud failure-detector, process-fencing, DNS, storage, or network authority.

## Planned invariants

| Invariant | Required behavior |
|---|---|
| Snapshot integrity | Stored snapshot digest equals canonical controller state; malformed or tampered state is rejected before restore |
| Atomic recovery | Partial staging is removed; prior committed snapshot remains authoritative on failed write or restore |
| Generation monotonicity | Restored owner term, ownership epoch, membership epoch, and event frontier cannot roll back |
| Observer membership binding | Every accepted observation carries the current membership epoch and observer key is present in that epoch's registry |
| Membership rotation | Rotating to a higher epoch invalidates old-epoch observations and prevents old quorum evidence from promoting |
| Race exclusion | Concurrent or interleaved candidates from the same active generation cannot both commit; at most one active region remains |
| Restart continuity | Pending and committed proposal identity, evidence digests, recovery phase, and trace frontier survive restart |

## Open implementation questions to resolve from source

1. Whether the existing controller exposes enough state accessors to serialize regions, observer evidence, pending/committed proposals, and events without exposing secrets.
2. Whether `serde` derives can be applied directly to the controller's public contract types or whether a dedicated `RecoverySnapshot` DTO is safer.
3. Whether the current `record_region_failure` API should accept a membership epoch or whether membership changes should be a separate typed method.
4. Whether the deterministic multi-region simulator can host multiple controller instances or whether a dedicated race harness should schedule cloned controllers and compare their proposed actions.

## Validation target

The final batch must pass rustfmt, shell/Python syntax, skill validation, Phase 30–33 focused tests, all-target Rust tests, the security compliance suite, independent 90+ gate audit, `git diff --check`, and remote publication verification using the deploy key without printing credentials.


## Implementation status

The implementation now includes `DisasterRecoverySnapshot`, `DisasterRecoverySnapshotStore`, signed `membership_epoch` binding, monotonic membership rotation, and the deterministic race-arbiter integration path. The Phase 33 suite contains seven passing tests: pending-authority restart, committed replay continuity, tamper rejection, partial-staging cleanup, stale membership rejection, membership rotation reset, and concurrent partition-race arbitration.

A validation correction was required for committed snapshots: observer evidence is historical after a successful promotion, so restored committed state accepts observations bound to the exact previous region and a strictly lower owner term/ownership epoch while still requiring the current membership epoch, cluster, observer key, and snapshot digest. It rejects evidence that is neither current-cycle nor committed-history bound.
