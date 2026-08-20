# Phase 44 concurrency audit notes

## Observed Phase 43 baseline

The published Phase 43 benchmark executes 32 ownership-bound CAS commits serially through one mutable coordinator. Its latest sanitized run reported 32 commits, final generation 32, ownership epoch 1, quorum 2, and elapsed time of approximately 774,814 microseconds. That is an end-to-end serial baseline of roughly 24.2 milliseconds per committed transition and roughly 41 commits per second for the local filesystem/signature/snapshot path. The benchmark does not report p50/p95/p99 latency, queue wait, worker contention, rejection rate, or concurrency scaling, so it cannot support a high-concurrency claim.

## Bottleneck diagnosis

`OwnershipBoundCasCoordinator` contains mutable CAS state and holds a filesystem ownership lock through verification, replica quorum admission, CAS snapshot persistence, and ownership-record persistence. This ordering is the required safety boundary, but the current benchmark has no bounded submission or worker orchestration around it. Calling the mutable coordinator from multiple threads would be unsafe without an explicit serialization boundary, and parallel callers using the same generation would be rejected as stale or conflicting. The next high-leverage slice is therefore a bounded ownership-bound intent executor: one guarded commit authority, a bounded queue, deterministic backpressure, worker-owned execution, per-intent latency metrics, and typed outcomes.

## Proposed Phase 44

Add `OwnershipBoundCasExecutor` around the Phase 43 coordinator. The executor should accept bounded signed commit intents, reject queue overflow before mutation, process intents through one coordinator owner, preserve Phase 43 lock-held invariants, and expose sanitized p50/p95/max queue-wait and end-to-end latency, throughput, completion, failure, and backpressure counters. It must not expose the mutable coordinator across threads. Integration tests must cover FIFO ordering, queue-full rejection, exact nonce idempotence, stale-generation failure without state mutation, worker shutdown, and bounded metric retention. A concurrency benchmark should exercise fixed levels such as 1, 2, 4, 8, and 16 producers against a bounded queue and report successful commits, rejected intents, errors, p95 latency, throughput, and final generation without keys, signatures, or payloads.

## Production boundary

This phase will measure a local executor and filesystem-backed coordinator only. It will not claim distributed queue durability, cross-host worker ownership, remote scheduler fairness, managed-volume latency, or production replica-domain independence. Those remain deployment adapters and promotion gates.
