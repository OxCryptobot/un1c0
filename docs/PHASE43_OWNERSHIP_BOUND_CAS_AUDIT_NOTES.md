# Phase 43 ownership-bound CAS audit notes

## Verified Phase 42 evidence

The committed independent audit reports 169 observed gates, 169 passed gates, no failures, no recorded secret material, and no cluster mutation. The Phase 42 section passes ownership-claim signature verification, atomic cross-process locking, ownership-epoch fencing, owner-bound lease lifecycle, stale-staging cleanup, fail-closed managed recovery state, distinct-replica recovery quorum, and hash-bound recovery evidence.

The Rust implementation validates canonical Ed25519 claims against pinned owner keys and exact cluster/resource/snapshot identity before admission. It serializes filesystem operations behind an atomic create-new lock, persists records using staged write, file sync, atomic rename, and directory sync, and rejects busy, stale, expired, mismatched, or conflicting ownership operations. Recovery evidence is canonical, signed, resource-bound, freshness-bounded, and counted only by distinct trusted replicas; `Prepared`, `Flushed`, `Unknown`, and `Failed` evidence cannot satisfy the recovered state.

## Residual integration gap

`CrossProcessOwnershipStore` exposes an `OwnershipWritePermit`, but Phase 41 `SingleWriterCasStore::commit` does not consume that permit. A caller can therefore validate ownership and invoke CAS as two separate operations, leaving a time-of-check/time-of-use seam in which a lease can expire, be released, or be superseded between admission and commit. The Phase 42 plan names ownership-epoch-bound write intent as a requirement, but the published implementation does not yet enforce the binding at the CAS mutation boundary.

The ownership lock file also has no explicit stale-lock recovery contract after process crash. That remains a liveness seam rather than a safety bypass: lock contention fails closed after bounded retries, but a crashed process can strand the local lock artifact until an operator removes it. This batch prioritizes the higher-leverage safety seam first by making the ownership permit and CAS commit one guarded transaction; stale-lock recovery remains a subsequent production-supervision boundary.

## Phase 43 proposal

Add `OwnershipBoundCasCoordinator`, a typed adapter that holds the cross-process ownership lock for the full CAS verification/quorum/mutation transaction, reloads and validates the ownership record before commit, requires exact owner/process/epoch/record-hash binding, refreshes the CAS snapshot, and rejects any generation/content mismatch before mutation. On quorum or signature failure, neither ownership nor CAS state may advance. On success, the coordinator updates the ownership record's committed generation/hash and returns the CAS receipt plus the new ownership record hash.

Add focused integration coverage for stale-permit rejection, ownership replacement between attempts, quorum failure preserving both states, successful ownership-bound commit, exact retry idempotence, and snapshot/hash binding. Add eight Phase 43 compliance gates and update the reusable engineering reference after validation.

## Production boundary

The coordinator proves local ordering and typed binding only. It does not prove that a remote process died, that a managed-volume controller honored stable-media barriers, or that a distributed lease service and storage quorum share one failure domain. Those remain explicit adapters and promotion gates.
