# Phase 77 Worker Fairness, Cancellation, and Tail Profile Report

**Author:** Manus AI

**Status:** Implemented locally and validated against deterministic worker-boundary tests and a sanitized 16-row, 17-trial aggregate tail benchmark.

## Executive summary

Phase 77 hardens the Phase 76 in-memory verification worker foundation with explicit cancellation tokens, typed cancelled outcomes, a bounded global in-flight reservation, and a per-node fairness quota. Cancellation is checked before verification and again after verification; cancelled results retain their ordered job identity but cannot enter aggregate mutation. The network adapter rejects cancelled results before consuming evidence or advancing any replay or aggregate state. Fairness is enforced before queue admission, so a hot node cannot reserve the entire bounded queue while other nodes retain admission capacity.

The tail benchmark covers 1, 2, 4, and 8 workers with 1, 4, 8, and 16 concurrent jobs, aggregating 17 repeated trials per matrix row. All 16 rows completed with zero errors, zero cancelled jobs, zero queue-full rejections, and zero fairness rejections. At 16 jobs, aggregate p95 end-to-end latency falls from **256.032 ms** at one worker to **144.789 ms** at two, **76.487 ms** at four, and **55.920 ms** at eight; median throughput rises from **61.121** to **117.970**, **225.705**, and **306.943 jobs/s**. The dominant remaining tail component is worker service time and host contention, not queue fairness itself. The benchmark is a local capacity proxy, not a production SLA or deployment capacity claim.[1]

## Implemented behavior

### Cancellation

`DiagnosticVerificationCancellationToken` is a cloneable shared atomic flag. `submit_with_cancellation` returns a `DiagnosticVerificationTicket` that can cancel a queued or running job. A worker checks the token before entering attestation verification and after it returns. A cancelled job emits `EmissionDiagnosticWorkerError::Cancelled`, increments `cancelled_jobs`, and preserves the result order. `DiagnosticVerifiedResult::is_cancelled` checks both the token and typed result error.

`MultiNodeDiagnosticReceiver::ingest_worker_result` rejects cancelled results with `VerificationCancelled` before mapping evidence, before current-state verification, and before aggregate mutation. This is a mutation-boundary guarantee: cancellation cannot advance the per-node sequence, source count, or aggregate state even when verification completed concurrently with cancellation.

### Per-node fairness

The pool reserves one global in-flight slot and one per-node slot only after `try_send` succeeds. The default per-node limit is half the queue capacity, clamped to at least one; callers can select an explicit limit up to `MAX_DIAGNOSTIC_NODE_IN_FLIGHT = 64`. A node that reaches its reservation limit receives `FairnessLimit`, while a full global queue receives `QueueFull`. Failed reservation attempts do not consume job IDs or mutation capacity. Reservation is released when the ordered result is dispatched, preserving bounded memory even when results complete out of order.

Fairness is intentionally local. It is not a distributed scheduler, cross-process quota, or service authorization mechanism. Phase 77 must extend it with explicit byte accounting, cancellation reason classes, supersession, and scheduler-level fairness before production transport promotion.

## Tail-latency evidence

| Workers | Jobs | E2E p50 | E2E p95 | E2E p99 | Queue-wait p95 | Service p95 | Throughput |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 16.493 ms | 18.192 ms | 18.192 ms | 0.082 ms | 18.063 ms | 60.474 jobs/s |
| 1 | 4 | 33.021 ms | 53.564 ms | 53.564 ms | 35.204 ms | 17.940 ms | 60.590 jobs/s |
| 1 | 8 | 67.912 ms | 131.804 ms | 131.804 ms | 114.510 ms | 20.068 ms | 58.370 jobs/s |
| 1 | 16 | 130.635 ms | 256.032 ms | 256.032 ms | 238.672 ms | 18.820 ms | 61.121 jobs/s |
| 2 | 4 | 16.737 ms | 34.818 ms | 34.818 ms | 17.435 ms | 18.464 ms | 122.470 jobs/s |
| 2 | 8 | 33.926 ms | 69.995 ms | 69.995 ms | 53.753 ms | 22.598 ms | 118.399 jobs/s |
| 2 | 16 | 68.300 ms | 144.789 ms | 144.789 ms | 124.589 ms | 22.209 ms | 117.970 jobs/s |
| 4 | 4 | 16.624 ms | 18.858 ms | 18.858 ms | 0.072 ms | 17.948 ms | 233.032 jobs/s |
| 4 | 8 | 17.579 ms | 37.419 ms | 37.419 ms | 18.879 ms | 19.178 ms | 228.480 jobs/s |
| 4 | 16 | 35.345 ms | 76.487 ms | 76.487 ms | 59.820 ms | 23.685 ms | 225.705 jobs/s |
| 8 | 4 | 16.925 ms | 19.155 ms | 19.155 ms | 0.181 ms | 17.309 ms | 227.648 jobs/s |
| 8 | 8 | 24.659 ms | 28.641 ms | 28.641 ms | 2.805 ms | 26.185 ms | 296.516 jobs/s |
| 8 | 16 | 35.225 ms | 55.920 ms | 55.920 ms | 34.354 ms | 38.269 ms | 306.943 jobs/s |

The full artifact also contains one-job rows for 2, 4, and 8 workers. These rows show that adding idle workers does not improve a single job materially; the useful concurrency region begins when work exceeds the worker count. At 16 jobs, eight workers deliver the lowest measured p95 and highest measured throughput in this fixture, but service p95 rises to **38.269 ms**, showing contention and host scheduling effects rather than unlimited scaling.[1]

The benchmark’s `out_of_order_buffered` counter confirms that concurrent completion occurs while ordered release remains intact. For example, the 8-worker/16-job aggregate row records a maximum out-of-order buffer of 12 and still returns all 16 results in submission order across every trial.
No row mutates aggregate state directly; mutation remains the receiver’s ordered boundary.

## Serialization and content-hash optimization findings

Phase 76’s direct profile measured the following 1-to-32-frame p50 growth: payload size **28.048×**, canonical payload serialization **26.303×**, SHA-256 integrity hashing **26.782×**, combined wire serialization and integrity **28.418×**, full verification **4.237×**, and warm cache admission **1.037×**.[2] This is a size-proportional pattern, not evidence that SHA-256 alone is the dominant cost.

The recommended order is therefore: first reuse immutable canonical bytes already proven by Phase 75/76; then measure allocation count, buffer growth, nested report serialization, and byte copies; then reuse a caller-owned or pooled bounded buffer with byte-for-byte golden vectors; and only afterward evaluate SHA-256 implementation specialization. Hashing optimization should be judged on the already assembled byte path and measured in CPU cycles per byte. A SIMD or hardware-accelerated route requires runtime feature detection, scalar fallback, domain-separator equivalence, and tamper/golden-vector tests.

Ed25519 remains the dominant fixed per-attestation cost for small streams, but Phase 77’s worker data shows that queueing and host contention dominate large-batch tails once several workers share the CPU. Signature batching should not be introduced merely because inclusive verification is expensive. It requires an independent cryptographic microbenchmark, a proof that the library/API preserves individual signature validity and failure localization, and a mutation-boundary test proving one bad signature cannot contaminate a batch.

## Promotion gates

| Gate | Required evidence | Current status |
|---|---|---|
| F77.1 cancellation safety | Queued and running cancellation tests; no replay/aggregate advancement; typed metrics | Pass: cancellation regression and receiver boundary pass |
| F77.2 per-node fairness | Exact hot-node rejection, other-node admission, no reservation leak | Pass: two-node fairness regression passes |
| F77.3 global bounds | Queue capacity, global in-flight, per-node limit, and shutdown behavior at boundaries | Pass for queue/per-node boundaries; byte accounting remains next work |
| F77.4 ordered mutation | Controlled out-of-order completion and ordered result release | Pass in existing worker and tail fixtures |
| F77.5 tail evidence | 16 rows at 1/2/4/8 workers and 1/4/8/16 jobs, p50/p95/p99, queue wait, service, throughput, zero errors | Pass: sanitized artifact has 16 rows and zero errors |
| F77.6 production scheduler hardening | Cancellation reasons, supersession, per-node byte quotas, deterministic fairness, and controlled delay injection | Not yet promoted; Phase 77 follow-up |

## Security and production boundaries

Cancellation and fairness are safety controls, not identity or authorization. A cancelled or over-quota job is rejected without changing evidence validity. A valid evidence result remains subject to the verifier trust epoch, current candidate roots, connection identity, replay sequence, node bounds, aggregate bounds, and the owning authority’s mutation decision. Metrics are numeric and redacted; worker queue failure cannot turn a failed verification into acceptance.

The pool remains process-local and in-memory. It does not provide durable work queues, cross-process ownership, service authentication, TLS, durable replay epochs, external key management, or consensus authority. These remain later integration and production gates.

## Validation summary

The reusable engineering skill was extended with the Phase 77 cancellation, fairness, tail-gating, and profiling reference. The Phase 77 worker-tail benchmark generated 16 rows with zero errors and no secret material, and the reusable tail-gate validator passed. The focused Phase 73–76 matrix passed 31 tests with zero failures: 5 Phase 73, 9 Phase 74, 7 Phase 75, and 10 Phase 76 tests. The complete Rust all-target suite then passed **430 tests with zero failures**, and formatting and staged-whitespace checks passed.

## References

[1]: ../benchmarks/phase77_worker_tail.json "Phase 77 sanitized worker tail-latency benchmark"

[2]: ../benchmarks/phase76_serialization_hash.json "Phase 76 sanitized serialization and hashing profile"
