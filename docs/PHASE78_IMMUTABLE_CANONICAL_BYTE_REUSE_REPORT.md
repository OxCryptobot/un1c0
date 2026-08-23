# Phase 78 Immutable Canonical-Byte Reuse Report

**Author:** Manus AI
**Status:** Implemented locally and validated; telemetry and event-journal promotion gates remain pending.

## Executive summary

Phase 78 implements the recommended non-cryptographic optimization identified by the Phase 76 serialization/hash profile: construct canonical stream bytes once, retain them immutably, and reuse them only after the stream has passed the existing bounded current-state verification and integrity checks. `EmissionDiagnosticStream` now retains a zero-digest payload and full-wire canonical JSON in private `Arc<[u8]>` fields. Programmatic and deserialized streams populate these fields only after frame encoding, context binding, integrity hashing, canonical re-encoding, and size checks succeed.

Repeated verification now reuses immutable frame encodings and the cached payload rather than reserializing every nested report and rebuilding the stream payload. `to_json` returns a clone of the validated cached full-wire bytes after rechecking the payload digest. Verified evidence shares the stream’s immutable canonical JSON allocation rather than cloning it into a second buffer. The optimization does **not** bypass semantic freshness, candidate-root checks, trusted-key lookup, trust epochs, Ed25519 verification, replay windows, node limits, or aggregate mutation boundaries.

The deterministic Phase 78 artifact covers frame counts 1, 2, 4, 8, 16, and 32 with 32 samples per row, zero errors, and `secret_material_recorded=false`.[1] The focused Phase 70/75 regression set passed 13 tests, including byte-identical construction/deserialization/evidence assertions. Phase 78 is therefore implemented as a local immutable-byte optimization; redacted telemetry schema checks, collector-failure behavior, and deterministic journal ordering remain the next promotion gates.

## Implementation contract

The optimization has four tightly bounded pieces.

| Component | Phase 78 behavior | Security condition |
|---|---|---|
| Canonical payload | Cache the stream envelope with its digest field zeroed in `Arc<[u8]>`. | Recompute the domain-separated SHA-256 digest before returning or hashing cached bytes. |
| Full-wire JSON | Cache canonical stream JSON containing the verified stream digest. | Populate only after canonical re-encoding and maximum-size checks. |
| Nested frames | Reuse each frame’s already validated canonical report encoding. | Keep sequence, frame-size, context, and current-state checks active. |
| Verified evidence | Share the stream’s immutable canonical JSON allocation. | Construct evidence only after full current-state and attestation verification. |

The stream type remains logically immutable: its byte caches and frame fields are private, its public accessors expose read-only slices or owned clones, and every constructor ends in a single canonical finalization path. Deserialization retains the exact input bytes only after integrity, canonicality, frame, context, and current-state validation. An unavailable or inconsistent cache returns a typed error instead of silently rebuilding or accepting unverified bytes.

## Measured evidence

The sanitized artifact records cached payload access, cached full-wire access, SHA-256 over preassembled payload bytes, full verification, warm cache admission, and redacted stage timings. Times below are p50 values in nanoseconds from the same deterministic local fixture; they are optimization evidence, not production capacity claims.[1]

| Frames | Payload bytes | Cached payload p50 | Cached JSON p50 | SHA-256 p50 | Full verification p50 | Warm admission p50 | Reuse stage p50 |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 3,414 | 67,297 | 71,744 | 66,400 | 8,055,123 | 76,270 | 771 |
| 2 | 6,392 | 121,045 | 120,958 | 118,443 | 8,367,503 | 85,204 | 691 |
| 4 | 12,348 | 236,172 | 365,306 | 229,198 | 9,318,256 | 85,927 | 1,203 |
| 8 | 24,260 | 513,325 | 490,855 | 502,020 | 9,447,464 | 81,490 | 1,309 |
| 16 | 48,091 | 975,580 | 1,026,295 | 898,622 | 10,436,645 | 77,972 | 3,345 |
| 32 | 95,755 | 1,803,236 | 1,822,435 | 1,760,138 | 13,919,552 | 77,443 | 6,034 |

The artifact’s p95 and p99 values are retained in the raw JSON and validated for monotonic ordering. At 32 frames, cached canonical JSON access is approximately 1.82 ms p50, while full verification remains approximately 13.92 ms p50 and warm evidence admission approximately 0.077 ms p50. The redacted canonical-byte reuse stage is approximately 6.03 microseconds p50 in the same row. This separation supports the intended optimization order: eliminate redundant assembly first, then independently evaluate hashing and cryptographic costs.[1] The measured values are local observations and should not be interpreted as a service-level objective.

## Regression and fail-closed coverage

The Phase 78 test extends the Phase 75 evidence fixture and asserts that the stream’s cached canonical JSON, owned `to_json` output, deserialized round-trip, and verified-evidence canonical bytes are byte-identical. It also confirms that the cached payload digest equals the stream digest and that instrumentation records zero redundant canonical report serialization while recording canonical-byte reuse.

Existing Phase 70 coverage continues to exercise deterministic frame counts through 32, stale context rejection, sequence and size limits, integrity mismatch, non-canonical input, and all-or-nothing parsing. Existing Phase 75 coverage continues to exercise stale candidates, trust-epoch revocation, immutable evidence, and no aggregate mutation after rejection. These boundaries remain active after the optimization; only redundant serialization is removed.

## Roadmap status

| Phase 78 gate | Status | Evidence |
|---|---|---|
| F78.0 immutable canonical-byte reuse | **Pass** | 13 focused Phase 70/75 tests; 6-row × 32-sample sanitized artifact; zero errors; byte-identical golden-vector assertions. |
| F78.1 versioned telemetry schema | Pending | Existing numeric instrumentation remains bounded; schema/deserialization promotion tests are not part of this batch. |
| F78.2 redaction scan | Pending | No raw bytes are added to telemetry; automated repository-wide redaction scan remains required. |
| F78.3 non-authoritative collector failure | Pending | Collector-failure and dropped-sample mutation-invariance tests remain required. |
| F78.4 journal ordering | Pending | Event-journal integration and deterministic ordering evidence remain required. |

The architecture roadmap and `AGENT_SYSTEM.md` now distinguish the implemented immutable-byte optimization from the still-pending observability and journal work.[2] Phase 79 remains blocked on explicit service identity separation and durable audit evidence; Phase 80 remains blocked on typed policy integration; Phase 81 remains blocked on production transport and rollout evidence.

## Security and production boundaries

Canonical-byte reuse is an integrity-preserving optimization, not a trust, identity, authorization, quorum, or persistence mechanism. A valid cached byte sequence cannot authorize an action, establish a remote service identity, bypass a trust epoch, advance a replay window, or mutate an aggregate without the existing receiver and authority gates. Raw canonical bytes, source material, signatures, public keys, prompts, tokens, and full diagnostics are not written to metrics, logs, or the sanitized benchmark artifact.

The current implementation remains process-local and in-memory. Future pooled serializers, SIMD hashing, cross-process sharing, or durable byte caches require separate designs with unchanged domain separators, byte-for-byte golden vectors, bounded memory, scalar fallback, tamper rejection, failure localization, and independent p50/p95/p99 measurements.

## Validation summary

The Phase 78 artifact validator passed with six rows, 32 samples per row, zero errors, complete frame coverage from 1 through 32, and no recorded secret material. The focused Phase 69–77 regression matrix passed **44 tests with zero failures** across six integration targets. The complete Rust all-target suite passed **431 tests with zero failures**, and formatter, skill, artifact, and whitespace checks passed.

## References

[1]: ../benchmarks/phase78_canonical_byte_reuse.json "Phase 78 sanitized immutable canonical-byte reuse benchmark"

[2]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"
