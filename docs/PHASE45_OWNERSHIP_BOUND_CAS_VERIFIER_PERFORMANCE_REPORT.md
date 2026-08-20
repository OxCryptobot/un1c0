# Phase 45 ownership-bound CAS verifier performance and coverage report

## Executive summary

The Phase 44 suite established a single worker-owned ownership-bound CAS executor with five integration tests and a conflict-heavy stress benchmark. Its main limitation was that signature, hash, freshness, and quorum checks remained on the serialized mutation worker. Phase 45 introduces a bounded read-only verification pool, an ordered dispatcher, and one authoritative mutation worker that retains the Phase 43 lock-held revalidation boundary.

The Phase 45 benchmark uses the same-generation conflict workload as Phase 44: one intent can commit and all other intents must fail closed. It therefore measures intent-processing throughput, not successful durable-write throughput. Across 1, 2, 4, 8, and 16 producers, the pipeline processed approximately 40.7, 79.9, 159.7, 243.1, and 264.0 intents per second. Verification service p95 ranged from approximately 22.9 to 45.8 milliseconds, while mutation service p95 remained approximately 187–301 microseconds. At 8 and 16 producers, verification queue wait and end-to-end p95 became the dominant tail-latency costs. This demonstrates that the new split is correctly isolating the short mutation path while exposing signature-verification CPU pressure for future tuning.

## Phase 44 coverage review

The Phase 44 integration suite covered five material boundaries. The concurrent-producer case admitted sixteen same-generation intents and asserted one successful transition with fifteen fail-closed conflicts while recording bounded latency metrics. The queue-full test used a one-slot queue and a worker-start barrier to prove deterministic rejection before mutation. The stale-generation test confirmed that a second prebuilt request could not advance state after the first commit. The shutdown test asserted typed rejection after close. The FIFO test submitted eight prebuilt valid sequential intents and required generation order from one through eight.

The Phase 44 benchmark extended this to producer levels 1, 2, 4, 8, and 16, recording queue wait, service, end-to-end p50/p95/max, throughput, completion, conflict, and rejection counters. Its workload intentionally used same-generation conflicts, so the single successful commit at each level was a safety invariant rather than a throughput target. Phase 45 preserves these tests and adds forged-evidence rejection, pre-admission metrics, ordered dispatch, mutation-time stale/conflict revalidation, verifier queue backpressure, and a thirty-two-intent multi-producer stress path.

## Phase 45 measured results

| Producers | Verifier workers | Jobs | Successful commits | Conflicts | Throughput (intents/s) | Verification service p95 (µs) | Mutation service p95 (µs) | End-to-end p95 (µs) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 16 | 1 | 15 | 40.7 | 22,897 | 301 | 23,266 |
| 2 | 2 | 32 | 1 | 31 | 79.9 | 23,487 | 227 | 24,205 |
| 4 | 4 | 64 | 1 | 63 | 159.7 | 23,397 | 199 | 24,598 |
| 8 | 8 | 128 | 1 | 127 | 243.1 | 33,414 | 204 | 41,655 |
| 16 | 8 | 256 | 1 | 255 | 264.0 | 45,788 | 187 | 77,304 |

The mutation path is materially shorter than the verification path in this workload. That is expected because each intent performs one writer signature verification plus two replica signature verifications before reaching the filesystem-backed CAS/ownership path. Increasing verifier workers improves aggregate processing up to the tested saturation point, but the 16-producer run is limited by eight verifier workers, CPU scheduling, and ordered-dispatch backlog. The result is useful evidence for Phase 46 rather than a production scalability claim.

## Safety model

| Boundary | Phase 45 behavior |
|---|---|
| Pre-admission context | Cloned identifiers, quorum, and pinned public keys; read-only verification only |
| Verification checks | Request/ack shape, signatures, hashes, resource binding, freshness, duplicate conflict, and distinct quorum |
| Ordering | Monotonic IDs assigned only after queue admission; bounded ordered dispatcher prevents result reordering |
| Early rejection | Forged evidence returns typed `PreAdmission` failure and never reaches mutation |
| Live authority | Phase 43 `commit_owned` reacquires the ownership lock and revalidates the current permit, epoch, record hash, CAS state, quorum, idempotence, and persistence ordering |
| Conflict handling | Intents that pass pre-admission but become stale fail as typed mutation errors without advancing state |
| Resource bounds | Verification worker count is capped at 32; acknowledgement vectors and all queues are bounded; latency samples are capped at 4,096 |
| Evidence hygiene | Benchmarks record aggregate counters and timings only; no private keys, signatures, raw payloads, or cluster mutation are recorded |

## Recommended Phase 46

The next high-leverage phase is **verification-cost reduction and adaptive admission**. The Phase 45 data shows that the serialized mutation p95 is below one millisecond while verification p95 is tens of milliseconds. Phase 46 should add bounded immutable verification artifacts—such as validated public-key handles and request/ack canonical digest caches keyed by exact content hash—without caching authority decisions across ownership epochs. It should also add adaptive worker and queue budgets, explicit CPU saturation metrics, and separate benchmarks for valid sequential commits, same-generation conflicts, forged evidence, and mixed workloads.

Any cache must be invalidated on key-registry changes, resource binding changes, protocol-version changes, and ownership/CAS epoch transitions. The mutation worker must continue to revalidate all security-critical state under the lock. Phase 46 should not trade verification latency for stale authority or unbounded memory.

## References

[1]: `../tests/phase44_ownership_bound_cas_executor_integration.rs` — Phase 44 executor integration and concurrency stress tests.
[2]: `../examples/phase44_ownership_bound_cas_executor_benchmark.rs` — Phase 44 sanitized high-concurrency benchmark.
[3]: `../tests/phase45_ownership_bound_cas_verifier_integration.rs` — Phase 45 verifier integration and stress tests.
[4]: `../benchmarks/phase45_ownership_bound_cas_verifier_metrics.json` — Phase 45 sanitized benchmark artifact.
[5]: `../src/ownership_bound_cas_verifier.rs` — bounded parallel verifier and ordered dispatcher implementation.
[6]: `../docs/PHASE45_OWNERSHIP_BOUND_CAS_VERIFIER_PLAN.md` — Phase 45 design and acceptance criteria.
