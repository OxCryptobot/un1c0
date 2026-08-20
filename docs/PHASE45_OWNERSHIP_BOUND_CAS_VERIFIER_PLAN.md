# Phase 45 ownership-bound CAS verifier plan

## Objective

Move read-only, CPU-heavy admission checks off the Phase 44 serialized mutation worker without weakening ownership-epoch fencing, exact permit binding, CAS quorum requirements, or rollback-safe persistence ordering.

## Pipeline architecture

`OwnershipBoundCasVerifierPipeline` owns a bounded verification queue, a bounded pool of read-only verifier workers, an ordered dispatcher, and one mutation worker. The verifier context contains only cloned cluster/resource/snapshot identifiers, required quorum, and pinned writer/replica public keys. It cannot mutate ownership or CAS state.

Each accepted intent receives a monotonic pipeline ID. Verifier workers run request signature/hash checks, replica signature/hash checks, request/ack binding, freshness, duplicate-conflict detection, and distinct quorum checks. Results may complete out of order, so the dispatcher buffers them in a bounded `BTreeMap` and forwards only the next accepted ID to the mutation queue. This prevents parallel verification from reordering state transitions.

The mutation worker receives only verified evidence and calls the Phase 43 `commit_owned` path. Phase 43 revalidates the live permit, ownership record, current CAS state, quorum, idempotence, and persistence ordering while holding the cross-process ownership lock. Pre-admission is therefore an optimization and early rejection path, never an authority substitute.

## Acceptance criteria

| Criterion | Required evidence |
|---|---|
| Parallel read-only verification | Configured worker count is bounded to 1–32 and workers receive cloned verification context only |
| Bounded resources | Verification, result, and mutation queues have fixed capacity; oversized intents fail before admission |
| Ordering | Accepted intent IDs dispatch in order even when verification completion order differs |
| Early rejection | Forged signatures/hashes fail as typed pre-admission errors and do not reach mutation |
| Authoritative revalidation | Valid but stale/conflicting intents can pass pre-admission and fail under Phase 43 lock-held mutation checks |
| Contention safety | Concurrent same-generation stress produces exactly one commit and fail-closed conflicts for the remainder |
| Metrics | Verification wait/service, mutation service, end-to-end p50/p95/max, completion, failure, queue-full, and bounded sample counters are sanitized |
| Compliance | Eight Phase 45 controls raise the total from 185 to 193 gates |

## Production boundary

Phase 45 provides a local bounded verification pool and ordered mutation dispatcher. It does not provide durable distributed verification queues, cross-host worker ownership, scheduler fairness, managed-volume latency isolation, remote queue backpressure, or independent replica-failure-domain proof. Production adapters must retain live lock-held revalidation and add deployment-level failure injection before claiming distributed performance or durability.
