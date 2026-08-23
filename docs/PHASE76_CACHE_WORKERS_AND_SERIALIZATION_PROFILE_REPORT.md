# Phase 76 Cache, Workers, and Serialization Profile Report

**Author:** Manus AI

**Status:** Implemented locally and validated against deterministic loopback fixtures.

## Executive summary

Phase 76 adds two complementary paths to the authenticated diagnostic pipeline. The first is a bounded `DiagnosticEvidenceCache` that stores private, immutable verified evidence under a domain-separated key binding the canonical attestation bytes, stream identity and digest, batch/profile context, exact unit roots, and verifier trust epoch. The second is a bounded `DiagnosticVerificationWorkerPool` that accepts typed jobs through a nonblocking `sync_channel`, verifies them concurrently, buffers out-of-order completions, and returns results only in submission order. Final aggregate admission continues through the existing current-state, trust, connection, sequence, and transport bounds.

The profiling artifact shows that serialization and hashing scale with canonical payload size, while Ed25519 remains a large fixed per-attestation cost in the current sandbox. For one through 32 frames, canonical payload serialization increases from **0.151 ms to 3.980 ms p50**, direct SHA-256 integrity hashing from **0.066 ms to 1.771 ms p50**, and the combined wire serialization/integrity path from **0.346 ms to 9.823 ms p50**. Sampled Ed25519 verification remains approximately **7.8–8.6 ms**. Full verification rises from **9.204 ms to 38.995 ms p50**. Warm evidence admission remains approximately **0.077–0.087 ms p50**, but its current path still recomputes candidate fingerprints and constructs a complete cache key; it is therefore an evidence-reuse baseline, not the final Phase 76 optimization ceiling.[1]

## Implemented contracts

### Bounded immutable evidence cache

`DiagnosticEvidenceCache` enforces a positive entry capacity, a positive byte budget, and a hard maximum of 1,024 entries. It uses deterministic recency ordering and tracks entries, bytes, hits, misses, insertions, evictions, and invalidations. Oversized evidence is not inserted. The cache owns `Arc<VerifiedDiagnosticEvidence>` values, so repeated consumers share the immutable canonical stream bytes without exposing mutable state.

The cache key is generated from a domain-separated SHA-256 input containing canonical attestation JSON, stream digest and ID, batch and profile context, exact stream and envelope unit roots, target label, and trust epoch. A cache hit is usable only after `verify_evidence_current` confirms the verifier’s trust epoch and exact registered key and `matches_current_candidates` recomputes the current candidate fingerprints. If either check fails, the entry is invalidated and the full verification path runs. A cache hit can never bypass replay windows, connection identity, node bounds, sequence checks, or aggregate admission.

### Bounded asynchronous verification

`DiagnosticVerificationWorkerPool` validates worker and queue bounds at construction, uses a bounded `sync_channel`, and returns `QueueFull` immediately rather than blocking or allocating unbounded work. Workers share immutable job inputs and the cache. Each job retains node ID, connection ID, sequence, attestation, stream, snapshot envelope, target profile, candidates, verifier, cache, and instrumentation. Completion results are buffered in a `BTreeMap` and released only at the next expected job ID. `MultiNodeDiagnosticReceiver::ingest_worker_result` converts the ordered result into the existing verified-admission path, which rechecks current state before mutation.

The pool supports an optional start barrier solely for deterministic boundary tests. It is not a production scheduler or a substitute for cancellation and shutdown policy. Dropping or closing the pool closes the sender, joins all worker threads, and reports worker panics rather than silently claiming completion.

## Profiling evidence

| Frames | Payload bytes | Canonical payload p50 | SHA-256 p50 | Wire + integrity p50 | Semantic fingerprint p50 | Full verification p50 | Warm cache admission p50 |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 3,414 | 0.151 ms | 0.066 ms | 0.346 ms | 0.028 ms | 9.204 ms | 0.084 ms |
| 2 | 6,392 | 0.253 ms | 0.119 ms | 0.666 ms | 0.028 ms | 10.066 ms | 0.077 ms |
| 4 | 12,348 | 0.526 ms | 0.248 ms | 1.276 ms | 0.038 ms | 12.436 ms | 0.084 ms |
| 8 | 24,260 | 1.104 ms | 0.523 ms | 2.491 ms | 0.028 ms | 16.070 ms | 0.077 ms |
| 16 | 48,091 | 1.961 ms | 0.902 ms | 4.858 ms | 0.029 ms | 24.798 ms | 0.077 ms |
| 32 | 95,755 | 3.980 ms | 1.771 ms | 9.823 ms | 0.029 ms | 38.995 ms | 0.087 ms |

The direct microbenchmarks are intentionally separate. `canonical_payload_bytes` measures JSON assembly of the stream integrity payload with a zero digest. `canonical_payload_digest` measures the domain-separated SHA-256 pass over already assembled bytes. `to_json` measures the existing wire path, including payload reconstruction, digest validation, and final wire serialization. The P0 evidence sample measures nested semantic/report verification, canonical stream serialization, content hashing, and Ed25519 in the full attestation path. This separation avoids labeling a composite timer as a cryptographic primitive.

| Frames | Ed25519 share of full p50 | Canonical stream share | Content-hash share | Semantic fingerprint p50 | Unattributed share |
|---:|---:|---:|---:|---:|---:|
| 1 | 84.95% | 5.18% | 1.57% | 0.028 ms | 4.24% |
| 2 | 78.56% | 9.02% | 2.62% | 0.028 ms | 6.01% |
| 4 | 63.90% | 13.87% | 3.88% | 0.038 ms | 9.55% |
| 8 | 49.83% | 21.22% | 6.17% | 0.028 ms | 14.85% |
| 16 | 31.69% | 28.81% | 8.07% | 0.029 ms | 19.06% |
| 32 | 20.58% | 34.67% | 9.51% | 0.029 ms | 21.90% |

The share table is directional rather than additive: stage timers are nested in parts of the verification call chain, and the full timer includes allocation, metadata checks, attestation payload construction, report serialization, repeated stream checks, and other work. The data supports two different optimization priorities. For short streams, Ed25519 dominates the measured critical path; signature batching, key reuse, or a faster verified implementation should be considered only after a cryptographic-only benchmark validates the opportunity. For long streams, canonical serialization and hashing grow with payload bytes and become the highest-leverage non-cryptographic targets. The direct canonical serializer scales about **26.3×** from one to 32 frames, direct SHA-256 about **26.8×**, combined wire serialization/integrity about **28.4×**, and payload size about **28.1×**.[1]

## Test evidence

The Phase 76 integration suite covers malformed cache configuration, metadata and stream identity key separation, deterministic entry and byte bounds, stale candidate rejection before aggregate mutation, cache hit/miss counters, trust-epoch revocation invalidation, bounded worker construction, deterministic queue-full rejection, ordered multi-worker completion, and receiver-side current-state rejection of a worker result. The required Phase 73, Phase 74, and Phase 75 suites remain separate regression gates.

The tests intentionally do not claim that the worker pool is a durable queue, that diagnostic attestations are service identity, or that cache hits establish authorization. The cache is process-local and bounded. The worker pool is an in-memory scheduling layer. Production transport, durable replay epochs, service authentication, key custody, cancellation semantics, and rollout policy remain later-phase boundaries.

## Remaining optimization work

The next optimization should avoid recomputing the complete attestation JSON key on every cache lookup. A connection-local immutable attestation identity digest can be created after canonical shape validation, while the cache key still retains the complete context and trust epoch. The candidate-root check should be measured independently from the cache-key path; if current root keys are already available from an immutable snapshot, use those keys instead of recomputing every UEG fingerprint.

Canonical serialization should be profiled for allocation count, buffer growth, map traversal, and repeated nested report encoding before introducing a buffer pool or custom serializer. Any custom path must preserve byte-for-byte golden vectors and the existing domain separator. SHA-256 optimization should be considered only after confirming whether the workload is CPU-bound on hashing or dominated by repeated JSON allocation and copying. Runtime feature detection and scalar fallback are required for any SIMD implementation.

The worker pool should next gain explicit cancellation tokens, per-node quotas, queue-byte accounting, and a mutation barrier that records stale-result rejection. Out-of-order completion tests should use controlled worker delays rather than relying on scheduler timing. Tail benchmarks should compare one, two, four, and eight workers at controlled concurrency and report queue wait separately from service time.

## Security and production boundary

Phase 76 remains fail closed. Cache insertion follows complete verification; stale candidates, trust-epoch changes, unknown keys, malformed attestations, and cache-bound mismatches do not produce reusable evidence. Worker completion order cannot change aggregate order. Observability is bounded and redacted. No raw keys, signatures, tokens, source text, or full canonical payloads are present in the benchmark artifact.

The implementation is not a production network-security claim. TLS or equivalent authenticated confidentiality, service identity, durable replay epochs, external key management, cancellation and resource governance, staging rollout, and explicit approval remain Phase 79–81 work as described in the integration roadmap.[2]

## Validation summary

The Phase 76 cache/worker integration suite passed **8/8 tests**, the profiling benchmark produced six frame-count rows with zero errors, the reusable-skill validator passed, and the complete Rust all-target suite passed with zero failures. Existing unrelated worktree changes are not part of the Phase 76 patch.

## References

[1]: ../benchmarks/phase76_serialization_hash.json "Phase 76 sanitized serialization and hashing profile"

[2]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Diagnostic streaming Phase 76–81 integration roadmap"

[3]: PHASE75_P0_P1_INSTRUMENTATION_AND_VERIFIED_EVIDENCE_SPEC.md "Phase 75 P0/P1 implementation specification"
