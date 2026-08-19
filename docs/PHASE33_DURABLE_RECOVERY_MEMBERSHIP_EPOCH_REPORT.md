# Phase 33 Durable Recovery and Observer-Membership Epoch Report

## Summary

Phase 33 implements the next high-value recovery slice identified by the Phase 32 quorum-safety audit. `DisasterRecoverySnapshot` captures the controller's recovery authority as canonical JSON with a SHA-256 state hash. `DisasterRecoverySnapshotStore` uses bounded sibling staging, `create_new`, `sync_all`, atomic rename, staging cleanup, and validation-before-restore. A controller can save and load pending or committed proposal identity without losing observer evidence or recovery phase.

`RegionFailureObservation` now carries `membership_epoch` inside the signed canonical payload. The controller exposes the active membership epoch and supports strictly monotonic observer registry rotation. Rotation validates public keys, rejects undersized registries, clears stale evidence and pending authority, and refuses to run during prepared or committed recovery. Observations signed for an old epoch fail before observer-map mutation.

A deterministic concurrent partition-race integration test clones a simulator base snapshot into two competing branches with asymmetric partition faults, delayed and duplicated links, and higher-generation transfers. Both branches remain individually safe but diverge in proposed owner. The Phase 32 controller then acts as the sole arbiter: it accepts one exact prepared proposal, rejects the competing candidate, commits one region, fences the old region, and reports safety passed.

## Evidence

| Evidence | Result |
|---|---:|
| Phase 33 integration tests | 7 passed |
| Phase 32 regression tests | 16 passed |
| Phase 30 partition regression tests | 9 passed |
| Phase 31 replay regression tests | 8 passed |
| Durable pending-proposal restart | Passed |
| Tampered snapshot no-mutation restore | Passed |
| Partial staging cleanup | Passed |
| Observer epoch rotation and stale evidence rejection | Passed |
| Concurrent partition branch race and one-commit arbitration | Passed |

The seventh test additionally proves that a committed controller snapshot restores the exact original proposal identity and returns `AlreadyCommitted` only for that exact replay. The benchmark artifact is deterministic and non-secret. It records snapshot hash binding, committed-proposal continuity, membership epoch, branch safety, branch owner divergence, arbiter-selected active region, observer quorum counts, and trace digests. It does not persist private signing keys or mutate a cluster.

## Safety model

The phase preserves the Phase 32 ordering: authenticate and bind evidence before mutation, require distinct observer quorum, require exact snapshot equality and strictly higher owner term/ownership epoch, preserve a pending proposal against conflicting replacement, fence the old region before activation, and restrict committed replay to the original proposal identity. Phase 33 adds a second authority dimension: observer membership epoch. A signature from a trusted key is insufficient if it belongs to an earlier observer registry epoch.

| Invariant | Implementation evidence | Remaining boundary |
|---|---|---|
| Snapshot hash integrity | Canonical content hash excludes `state_hash`; load validates before restore | No replicated remote storage |
| Atomic restart recovery | Staging write, sync, rename, cleanup, bounded load | No multi-process lock or distributed CAS |
| Pending authority continuity | Snapshot includes observations, pending proposal, phase, events, and committed identity | No external recovery log |
| Membership epoch monotonicity | Rotation requires a higher epoch and validates all public keys | Registry transition is local, not joint consensus |
| Stale evidence exclusion | Observation epoch must equal controller epoch before insertion | No real transport freshness or lease |
| Partition-race single commit | Competing simulator branches are resolved by one controller arbiter | No process or routing fence is enforced |

## Recommended next phase

The next best-value phase is a **replicated recovery authority and external fencing token**. It should place the snapshot and membership epoch behind a consensus-backed compare-and-swap log, add joint observer-membership transitions, and bind every promotion to a fencing token enforced by service admission, socket ownership, and routing. Recovery should be tested across two controller processes with crash injection between proposal preparation and commit. The local snapshot store should remain the atomic persistence primitive, but not the distributed authority.

## References

[1]: ../src/disaster_recovery.rs "Phase 33 implementation"
[2]: ../tests/phase33_durable_recovery_integration.rs "Phase 33 integration suite"
[3]: ../tests/phase32_disaster_recovery_integration.rs "Phase 32 regression suite"
[4]: ../src/multiregion.rs "Deterministic multi-region simulator"
[5]: ../docs/PHASE32_QUORUM_SAFETY_AUDIT_REPORT.md "Phase 32 audit and next-phase recommendations"
