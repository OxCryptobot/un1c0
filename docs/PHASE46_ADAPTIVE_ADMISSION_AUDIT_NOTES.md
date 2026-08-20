# Phase 46 adaptive admission audit notes

## Observed Phase 45 facts

The committed Phase 45 benchmark runs 16 jobs per producer with one successful same-generation commit and all remaining intents failing closed. At 1, 2, 4, 8, and 16 producers, throughput is approximately 40.7, 79.9, 159.7, 243.1, and 264.0 intents/s. Verification-service p95 is approximately 22.9, 23.5, 23.4, 33.4, and 45.8 ms. Verification-wait p95 is approximately 61, 42, 61, 8,894, and 33,486 µs. Mutation-service p95 remains approximately 301, 227, 199, 204, and 187 µs. At 16 producers, verification consumes the dominant service budget and the ordered dispatcher accumulates a large pre-admission backlog; the filesystem-backed mutation path is not the dominant tail.

## Verified Phase 45 invariants to preserve

Pre-admission is read-only and advisory. The Phase 43 coordinator remains the authoritative mutation path and must revalidate live ownership, epoch, record hash, CAS generation/hash, quorum, nonce idempotence, and rollback-safe persistence under the ownership lock. Accepted intent IDs must remain contiguous, parallel verification results must dispatch in order, forged evidence must fail before mutation, stale/conflicting intents must fail closed at mutation, queues and retained latency samples must remain bounded, and shutdown/disconnect must return typed failures.

## Phase 46 design proposal

Add a bounded adaptive admission controller that tracks queue depth, verification service p95, and recent verification failure pressure. It admits an intent only when the queue and in-flight budgets permit; otherwise it returns a typed `AdaptiveAdmissionLimited` error without consuming an intent ID. Use additive-increase/multiplicative-decrease worker-pressure hints rather than unbounded thread creation. Expose a deterministic controller snapshot for tests and sanitized benchmark output.

Reduce verification cost without caching authority decisions. Build immutable key material once when the verifier context is constructed, so each request/ack verification reuses parsed Ed25519 verifying keys while still validating the exact public-key bytes against the pinned registry. Add a bounded digest-result cache only for exact canonical request/ack content hashes, keyed by a context fingerprint that changes when cluster/resource/snapshot IDs, key registries, protocol version, or required quorum changes. Cached entries are cryptographic verification facts, not ownership or freshness decisions; freshness, request binding, distinct quorum, and live mutation revalidation must execute on every admission.

## Acceptance criteria

| Area | Required proof |
|---|---|
| Adaptive admission | Queue/in-flight depth, p95 service, and error pressure produce deterministic allow/limit decisions under fixed thresholds |
| No authority bypass | Limited or cached paths never mutate state and never skip Phase 43 lock-held revalidation |
| Cost reduction | Parsed verification keys are reused; cached signature/hash facts are context-bound and bounded |
| Epoch safety | Context fingerprint changes on key/resource/protocol/quorum changes; no stale cache entry survives a context replacement |
| Stress behavior | 16+ producer runs show bounded queue depth and typed limiter decisions, with sanitized metrics |
| Compatibility | Phase 41–45 regressions and full compliance chain remain green |
