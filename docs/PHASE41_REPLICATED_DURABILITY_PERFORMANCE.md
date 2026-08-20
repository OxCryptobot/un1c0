# Phase 41 Replicated Durability Performance Report

## Workload

The sanitized benchmark executed **64 sequential single-writer CAS commits** against an in-memory coordinator with two distinct signed replica acknowledgements required per commit. Each request advanced one generation, bound the proposed hash to the payload hash, and used a bounded request nonce. A durable JSON snapshot was then written and loaded through the atomic snapshot store.

## Observed results

| Metric | Observed value | Interpretation |
|---|---:|---|
| CAS attempts / completed commits | 64 / 64 | No accepted-path commit failed |
| Failed commits | 0 | Quorum and CAS invariants held for the workload |
| Final generation | 64 | Every commit advanced exactly one generation |
| Required quorum | 2 replicas | Distinct replica identities were required |
| Commit p95 | 20,835 µs | In-memory signing, verification, digesting, and quorum admission |
| Commit maximum | 21,070 µs | Maximum observed commit interval |
| Total wall time | 1,322,172 µs | Includes request/ack signing and verification |
| Commit throughput | 48.405 commits/s | Sanitized integer milli-commits-per-second converted to commits/s |
| Snapshot round trip | Passed | Atomic save, directory sync, load, identity/hash validation |
| Secret material recorded | false | No keys, signatures, raw payloads, or full tokens emitted |
| Cluster mutation performed | false | Local verification only |

The Phase 41 benchmark is deliberately conservative: each operation performs fresh Ed25519 signing for the writer and two replicas, canonical hashing, signature verification, quorum deduplication, and generation mutation. It is not a network or storage-controller benchmark and does not represent production cross-region throughput.

## Failure-mode coverage

The integration suite contains nine tests covering quorum loss with no state mutation, exact generation/hash CAS mismatch, idempotent request retry, conflicting nonce reuse, same-replica conflicting acknowledgements, future/expired replica evidence, writer-key rebinding, durable snapshot round-trip, snapshot tampering, and bounded receipt state.

The local coordinator verifies all signatures, identity bindings, request/ack digests, freshness, generation continuity, distinct-replica quorum, and receipt integrity before committing. It does not infer that a replica's declared durability mode is true; the acknowledgement is evidence from a trusted adapter and remains subject to deployment-level attestation and supervision.

## Remaining production gaps

The implementation does not observe device queue depth, controller cache policy, forced-unit-access behavior, stable-media barriers, cloud-volume replication acknowledgements, remote filesystem directory ordering, database WAL/commit behavior, or cross-region network latency. It also does not run multiple independent processes against one shared snapshot path. Production must supply a single-writer ownership protocol, a storage adapter that emits trustworthy flush sequences, independent replica placement, process fencing, and a cross-failure-domain recovery exercise.
