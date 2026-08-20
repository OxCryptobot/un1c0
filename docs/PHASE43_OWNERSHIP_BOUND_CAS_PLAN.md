# Phase 43 ownership-bound CAS plan

## Objective

Close the Phase 42 ownership-to-CAS time-of-check/time-of-use seam by enforcing the active cross-process ownership permit throughout replicated CAS verification, quorum admission, durable CAS snapshot persistence, and ownership-record advancement.

## Typed transaction contract

`OwnershipBoundCasCoordinator` owns one `CrossProcessOwnershipStore`, one `SingleWriterCasStore`, and one `CasDurabilitySnapshotStore` for the exact cluster, resource, and logical snapshot. `commit_owned` accepts an `OwnershipWritePermit`, a signed `CasWriteRequest`, authenticated replica acknowledgements, and a bounded current tick. The coordinator rejects a writer/owner mismatch, ownership-epoch mismatch, expired/fenced lease, process mismatch, record-hash mismatch, ownership/CAS generation mismatch, stale or conflicting replica evidence, and insufficient quorum before logical state advancement.

The OS-backed ownership lock remains held while the coordinator reloads the ownership record, refreshes the CAS snapshot, verifies the CAS request, checks replica evidence, constructs the receipt, and persists the next CAS snapshot. Only a committed receipt matching the requested next generation/content hash can advance the ownership record. Exact nonce retries return the prior receipt and preserve the existing ownership record hash.

## Acceptance criteria

| Criterion | Required evidence |
|---|---|
| Permit binding | Signed CAS writer ID and epoch equal the active ownership permit |
| Lock-held admission | Ownership lock spans CAS validation and distinct-replica quorum counting |
| State equality | Ownership generation/content hash equals refreshed CAS state before commit |
| Failure preservation | Quorum, stale permit, and replacement-owner failures leave both states unchanged |
| Durable ordering | CAS snapshot is persisted and validated before ownership-record advancement |
| Retry behavior | Exact CAS nonce retry is idempotent and does not advance ownership again |
| Sanitization | Benchmark evidence contains no private keys, signatures, payloads, or raw fencing tokens |
| Compliance | Eight new Phase 43 gates raise the audit total from 169 to 177 |

## Production boundary

The local coordinator proves lock-held ordering and typed adapter-contract consistency. It does not prove scheduler/process termination, distributed lease-service behavior, remote storage-controller barriers, managed-volume replication, network quorum, or independent failure-domain placement. Those capabilities remain externally supervised production adapters and promotion gates.
