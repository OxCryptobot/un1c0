# Phase 44 ownership-bound CAS executor plan

## Objective

Add a bounded execution boundary around the Phase 43 ownership-bound CAS coordinator so concurrent producers cannot race mutable state or bypass ownership-epoch fencing, while preserving FIFO ordering, fail-closed conflicts, deterministic backpressure, and sanitized performance evidence.

## Typed design

`OwnershipBoundCasExecutor` owns one mutable `OwnershipBoundCasCoordinator` inside one worker thread. Callers submit bounded `OwnershipBoundCasIntent` values containing an already validated ownership permit, signed CAS request, replica acknowledgements, and current tick. The executor returns a ticket with a typed wait result. The synchronous queue has a fixed capacity; `try_send` returns `QueueFull` before the intent can mutate state, and disconnected or closed executors return `Shutdown`.

The worker processes accepted jobs in FIFO channel order. It keeps the Phase 43 coordinator as the only mutable CAS authority. Each job records bounded queue-wait, service, and end-to-end latency samples. The sample store is capped at 4,096 entries, and the metrics expose p50, p95, maximum, completion, failure, queue-full, and shutdown counters without storing keys, signatures, canonical payloads, or raw fencing tokens.

## Acceptance criteria

| Criterion | Required evidence |
|---|---|
| Worker ownership | Mutable coordinator is moved into the worker and not shared across producer threads |
| Backpressure | A full bounded queue returns typed `QueueFull` before worker mutation |
| Ordering | Prebuilt valid sequential intents commit in generation order |
| Conflict safety | Concurrent same-generation intents produce one commit and fail closed for the rest |
| Shutdown | New submissions after close return typed `Shutdown` |
| Metrics bounds | Latency samples are capped and p50/p95/max are deterministic over captured samples |
| Stress benchmark | Producer levels 1, 2, 4, 8, and 16 report throughput, queue wait, service, end-to-end latency, conflicts, and rejections |
| Compliance | Eight Phase 44 gates raise the audit total from 177 to 185 |

## Production boundary

The executor measures and constrains a local worker-owned coordinator. It does not provide durable distributed queueing, cross-host worker ownership, scheduler fairness, managed-volume latency isolation, remote backpressure propagation, or independent replica failure-domain proof. Those remain production adapter and deployment responsibilities.
