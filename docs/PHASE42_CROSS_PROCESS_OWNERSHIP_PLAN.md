# Phase 42 Cross-Process Ownership and Managed-Volume Recovery Plan

## Objective

Phase 42 extends the Phase 41 quorum-gated CAS protocol from one trusted coordinator process to a bounded cross-process ownership contract. The local implementation will prove ownership lease admission, fencing-epoch monotonicity, owner-bound renewal/release, atomic ownership snapshots, and storage-adapter recovery-state gating. It will not claim to terminate a process or prove cloud-controller behavior.

## Ownership contract

An ownership record binds cluster, resource, logical snapshot, owner identity, process instance, ownership epoch, lease expiry tick, last committed generation/hash, fencing nonce, and record hash. Acquisition uses an atomic create-or-compare-and-swap path. An existing unexpired owner blocks acquisition. An expired or explicitly fenced owner can be replaced only with a strictly higher ownership epoch and a new fencing nonce. Renewal and release require exact owner identity, process instance, epoch, and record hash.

Every CAS write intent carries the active ownership epoch. The coordinator rejects lower epochs, stale record hashes, expired leases, owner identity changes without an epoch transition, and writes attempted by a process that no longer owns the record. A higher epoch fences lower-epoch writers before they can publish a new state.

## Managed-volume recovery contract

A bounded storage adapter reports a typed state: `Prepared`, `Flushed`, `Replicated`, `Recovered`, or `Unknown`. A recovery evidence record binds the logical snapshot, generation/hash, ownership epoch, replica identity, storage adapter identity, flush sequence, replication sequence, observed tick, TTL, and evidence hash. Only fresh evidence from the configured independent replica quorum can admit `Replicated`; `Unknown`, stale, contradictory, or negative evidence is fail closed.

## Acceptance criteria

| Criterion | Required evidence |
|---|---|
| Cross-process ownership | Atomic acquisition, exact owner/process/epoch binding, bounded lease, no duplicate owner |
| Fencing | Strict ownership-epoch monotonicity and lower-epoch write rejection |
| Lease lifecycle | Owner-bound renewal, release, expiry recovery, and stale-record cleanup |
| CAS integration | Write intent carries ownership epoch and expected record hash |
| Recovery state | Typed storage adapter states with `Unknown` and negative states blocking promotion |
| Managed-volume quorum | Distinct replica recovery evidence bound to snapshot/generation/hash and storage sequences |
| Crash recovery | Atomic ownership/evidence snapshots with stale staging cleanup and hash validation |
| Sanitization | No private keys, signatures, payloads, full fencing tokens, or uncontrolled cloud claims |

## Production boundary

The local seam cannot prove scheduler/process termination, fencing hardware, cloud-volume stable-media semantics, controller cache policy, managed-volume replication, replica placement, or cross-region network behavior. Production adapters must connect the typed contracts to an independent supervisor, a real ownership store, a storage-controller API, and an approved failure-domain recovery exercise.
