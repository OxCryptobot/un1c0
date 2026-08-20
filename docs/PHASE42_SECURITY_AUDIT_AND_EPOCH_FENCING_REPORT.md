# Phase 42 security audit and ownership-epoch fencing review

## Executive summary

The committed Phase 42 independent audit reports **169 observed gates, 169 passed gates, zero failures, no recorded secret material, and no cluster mutation**. The eight Phase 42 controls cover signed ownership claims, atomic cross-process locking, monotonic ownership epochs, owner-bound lease lifecycle, stale-staging cleanup, fail-closed managed-volume states, distinct-replica recovery quorum, and hash-bound recovery evidence. The evidence is local and adapter-oriented; it does not claim cloud-controller, scheduler, process-termination, or independent-failure-domain truth.

The review identified one important integration seam: Phase 42 exposed an `OwnershipWritePermit`, but Phase 41 CAS could previously be called after ownership admission had ended. Phase 43 closes that time-of-check/time-of-use gap with `OwnershipBoundCasCoordinator`, which holds the ownership lock through CAS verification and replica quorum admission, persists the CAS snapshot before advancing the ownership record, and rolls back CAS snapshot state when subsequent persistence fails. Phase 43 adds eight controls, raising the published total to **177 passed gates**.

## 1. Phase 42 audit breakdown

| Evidence family | Phase 42 result | Meaning |
|---|---:|---|
| Ownership claim signature | Passed | Canonical Ed25519 claim is checked against the pinned owner key and exact cluster/resource/snapshot identity. |
| Atomic cross-process lock | Passed | Acquisition and lifecycle mutations use an exclusive create-new lock with bounded retries. |
| Ownership epoch fencing | Passed | A replacement owner must use a strictly higher epoch; stale or equal epochs cannot supersede the active record. |
| Owner-bound lease lifecycle | Passed | Renew, release, and write admission require exact owner, process, epoch, and record-hash binding. |
| Stale-staging cleanup | Passed | Stale `.staging` state is removed before loading or writing the ownership record. |
| Managed recovery state | Passed | `Prepared`, `Flushed`, `Unknown`, and `Failed` evidence cannot admit recovered state. |
| Distinct recovery quorum | Passed | Recovery counts unique trusted replica identities rather than duplicate acknowledgements. |
| Recovery evidence hash binding | Passed | Evidence hashes bind canonical signed recovery fields, including generation, content hash, epoch, sequences, freshness, and replica identity. |

The independent artifact also verifies that the metrics commit is an ancestor of the audited repository head, the benchmark concurrency remains eight, and the complete phase set includes Phases 15 through 43. The false values for secret material and cluster mutation are intentional safety evidence, not failed controls.

## 2. Ownership claim verification

`OwnershipClaim::verify` first validates the bounded shape and protocol domain. It then checks exact cluster, resource, and logical snapshot binding, looks up the owner in the pinned registry, rejects public-key rebinding, verifies Ed25519 over a canonical payload, and checks the claim digest. The signed payload includes owner identity, process instance, expected ownership-record hash, requested epoch, lease expiry, generation, content hash, fencing nonce, and public key.

This ordering matters because the claim is treated as untrusted input. No ownership state is loaded as trusted until identifiers, key identity, signature, and digest all pass. The claim’s `expected_record_hash` is the compare-and-swap precondition for the ownership record, preventing a caller from replacing a record it did not observe.

## 3. Atomic ownership and lease lifecycle

`CrossProcessOwnershipStore::with_lock` creates a sibling `.lock` file with `create_new`, retries a bounded number of times when another process holds it, runs the operation while the lock exists, and removes the lock afterward. The record itself is written through a staging file, file `sync_all`, atomic rename, and parent-directory `sync_all`.

Acquisition rejects an active unexpired owner as `Busy`. If the record is expired or fenced, replacement requires a strictly higher `requested_epoch`; a stale or equal epoch returns `StaleEpoch`. The initial claim must use epoch one and the zero record hash. Renewal requires the same owner, process, epoch, and record hash and extends the lease monotonically within a bounded window. Release marks the record fenced and sets the expiry to the current tick. Write admission rejects fenced or expired records and returns an `OwnershipWritePermit` containing the exact owner, process, epoch, and record hash.

## 4. Ownership-epoch fencing

The ownership epoch is a monotonic fencing generation, not merely a version label. Consider an old process A holding epoch 1 while process B obtains epoch 2 after expiry or explicit fencing. Any write from A still carries epoch 1. The active record now contains epoch 2, so the permit or record comparison fails before CAS or payload mutation. Even if A’s old lease clock is wrong or A is partitioned from the coordinator, its lower epoch cannot satisfy the active record’s equality checks.

The protection is layered:

| Boundary | Check | Failure result |
|---|---|---|
| Claim admission | Requested epoch must be greater than the current epoch when replacing a record. | `StaleEpoch` or rejection; record unchanged. |
| Lease operation | Owner, process, epoch, and expected record hash must match. | `RecordMismatch`, `LeaseExpired`, or rejection. |
| Write admission | Record must be unfenced and current tick must be before expiry. | `LeaseExpired` or rejection; no permit. |
| CAS request | Phase 43 requires request writer ID and writer epoch to equal the permit. | `StalePermit`; CAS is not called. |
| Lock-held transaction | The record is reloaded under the ownership lock immediately before CAS verification. | Stale permit or divergence fails closed. |

Epoch fencing therefore prevents stale authority from publishing through a newly elected owner. It does not terminate the stale process; process termination remains an external supervision responsibility.

## 5. Fail-closed typed recovery decisions

`ManagedVolumeRecoveryEvidence::verify` validates the recovery domain, bounded identity fields, snapshot generation and content hash, ownership epoch, replica and adapter identity, sequence and TTL bounds, pinned public key, Ed25519 signature, and evidence digest. `ManagedVolumeRecoveryGate::admit` then requires each evidence item to match the active ownership record exactly.

Evidence in `Prepared`, `Flushed`, `Unknown`, or `Failed` state is rejected as not being fresh replicated state. Future-dated or expired evidence is rejected. Same-replica conflicting hashes return `Conflict`, and fewer than the configured number of distinct replica IDs returns `QuorumUnavailable`. Only a fresh set of distinct trusted replica identities can return `RecoveryDecision { state: Recovered, ... }`. This is fail closed because uncertainty, contradiction, staleness, and negative evidence never get coerced into a successful recovery decision.

The typed state is useful operationally: callers can distinguish a recoverable quorum wait or adapter rejection from an accepted recovered snapshot, while the Rust API still returns an error for unsafe admission rather than guessing. The local implementation does not assert that a storage controller truly flushed stable media; it verifies the signed adapter contract and preserves the boundary as deployment-owned.

## 6. Phase 43 improvement: ownership-bound CAS

The new `OwnershipBoundCasCoordinator` closes the remaining local safety seam. It performs the following sequence while the ownership lock is held:

1. Validate that the permit owner and ownership epoch equal the signed CAS request writer and epoch.
2. Reload and validate the active ownership record under the lock.
3. Refresh the CAS store from its durable snapshot, if present.
4. Require ownership generation and content hash to equal the refreshed CAS state.
5. Verify the signed CAS request and all replica acknowledgements.
6. Require a distinct replica quorum and construct the CAS receipt.
7. Persist the new CAS snapshot.
8. Update the ownership generation and content hash, recompute its record hash, and persist the ownership record.
9. Return the CAS outcome and new ownership-record hash.

If quorum, signature, freshness, stale-permit, or state-equality checks fail, neither logical state advances. If CAS snapshot persistence or later ownership persistence fails, the coordinator restores the in-memory CAS state and attempts to restore the prior durable CAS snapshot. Exact nonce retries return an idempotent receipt only when it still matches the active ownership record; they do not advance ownership again.

## 7. Validation and publication

| Check | Result |
|---|---|
| Phase 42 integration tests | 7 passed |
| Phase 43 integration tests | 5 passed |
| Phase 43 benchmark | 32 ownership-bound commits; final generation 32; quorum 2; sanitized output |
| Complete compliance workflow | Passed, including Helm and Podman Compose mTLS checks |
| Independent compliance audit | 177/177 gates passed |
| Reusable skill validation | Passed |
| Source commit | `27351d95d14983a1988afbf7f37824706c5b5844` |
| Metadata commit | `0212f74dbe2a963e68c684bc5235be9f1c524d44` |
| Remote parity | Local and `origin/main` point to `0212f74dbe2a963e68c684bc5235be9f1c524d44` |

The compiler continues to emit pre-existing warnings for unused translation helpers and an unused `Path` import outside the Phase 43 surface. They do not fail the configured test or compliance gates and were not modified because unrelated worktree changes were intentionally preserved.

## References

[1]: `../benchmarks/security_compliance_audit.json` — committed independent compliance audit artifact.
[2]: `../benchmarks/security_compliance_metrics.json` — committed 177-gate security metrics artifact.
[3]: `../src/cross_process_ownership.rs` — Phase 42 ownership leases, epoch fencing, and recovery quorum implementation.
[4]: `../src/ownership_bound_cas.rs` — Phase 43 lock-held ownership-bound CAS coordinator.
[5]: `../tests/phase42_cross_process_ownership_integration.rs` — Phase 42 integration tests.
[6]: `../tests/phase43_ownership_bound_cas_integration.rs` — Phase 43 integration tests.
[7]: `../docs/PHASE42_CROSS_PROCESS_OWNERSHIP_PLAN.md` — Phase 42 contract and production boundary.
[8]: `../docs/PHASE43_OWNERSHIP_BOUND_CAS_PLAN.md` — Phase 43 contract and acceptance criteria.
