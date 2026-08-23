# Phase 75 Specification: P0 Stage Instrumentation and P1 Immutable Verified-Evidence Reuse

**Author:** Manus AI
**Status:** Draft technical implementation specification; no implementation changes are included in this document.
**Parent milestone:** Phase 74 commit `53a3462`
**Primary surfaces:** `emission_diagnostic_stream.rs`, `emission_diagnostic_attestation.rs`, `emission_diagnostic_network.rs`, `semantic_snapshot_envelope.rs`, `emission_receipt.rs`, and `incremental_semantic.rs`.

## 1. Purpose and decision summary

Phase 74 established bounded length-prefixed TCP streaming for Phase 73 diagnostic attestations. The next implementation must answer two different questions without conflating them. **P0** must measure where verification time is spent. **P1** must remove repeated work only where a typed immutable evidence object proves that the work was already completed against the exact current semantic state and trusted attestation context.

The Phase 74 artifact demonstrates a CPU-dominated verification path: for one frame and one connection, end-to-end p50 is 11.470 ms, receive p50 is 0.132 ms, and complete verification p50 is 10.703 ms. For eight frames and eight connections, the corresponding values are 233.688 ms, 2.187 ms, and 228.603 ms.[1] The current verification timer is inclusive: it contains semantic snapshot/fingerprint checks, nested report and receipt verification, canonical serialization, SHA-256 content hashing, public-key parsing, signing-payload construction, and Ed25519 verification. The artifact therefore does **not** measure Ed25519 as an isolated component.

The implementation decision is to add instrumentation first, preserve the existing verification path as the correctness oracle, and then introduce two non-forgeable evidence levels:

| Evidence level | Meaning | Consumers |
|---|---|---|
| `CanonicalDiagnosticEvidence` | The stream has passed structural validation, current snapshot/profile/UEG verification, canonical-byte equality, and content-digest checks. | Attestation verification, repeated local consumers, benchmark harness. |
| `VerifiedDiagnosticEvidence` | The canonical evidence has additionally passed exact content-type/hash checks, explicit trusted-key lookup, signing-payload validation, and Ed25519 verification under a specific trust-registry epoch. | Network receiver, per-node aggregator, audit/health adapters. |

P1 must not make a signature alone authoritative. Network sequence, node/connection identity, current semantic state, aggregate ordering, authorization, quorum, and durable handoff remain separate contracts.

## 2. Current call-chain and overhead attribution

The present path has several nested verification layers. `DiagnosticAttestationVerifier::verify_stream` calls `EmissionDiagnosticStream::verify_for`, then recomputes the stream content hash, rebuilds a `VerifyingKey`, constructs the canonical signing payload, and performs Ed25519 verification.[2] `EmissionDiagnosticStream::verify_for` checks stream context, loops over every frame, verifies each nested report, serializes each report again, recomputes byte totals, recomputes the stream digest, and calls `to_json` again.[3]

Each nested report delegates to receipt-aggregate and receipt verification. Receipt verification rechecks the snapshot envelope, rebuilds expected unit roots, and recomputes expected chunk counts.[4] Snapshot-envelope verification recomputes a semantic fingerprint for every candidate UEG and compares profile/root keys with the frozen envelope.[5] The dependency-aware semantic module already provides a safer future reuse model based on per-function fingerprints and reverse-dependent closure.[6]

The following table is the correct current attribution boundary:

| Component | Current evidence | What can be concluded now |
|---|---:|---|
| TCP receive and frame parsing | 0.132 ms p50 at 1/1; 2.187 ms p50 at 8/8; maximum observed receive p99 9.722 ms | Framing is not the first optimization target for this fixture. |
| Complete stream verification | 10.703 ms p50 at 1/1; 228.603 ms p50 at 8/8 | The verification pipeline is dominant and scales with frame count and concurrency. |
| Semantic freshness and nested report/receipt verification | Included in the complete verification timer | Likely a major component because every frame repeats nested checks, but not separately quantified. |
| Canonical report/stream serialization | Included in stream verification and content-hash helpers | Repeated and visible in code; exact share is not quantified. |
| SHA-256 domain/content/frame hashing | Included in the same timers | Bounded and likely smaller than semantic checks for this fixture, but not directly measured. |
| Ed25519 public-key parsing | `VerifyingKey::from_bytes` occurs inside every `verify_common` call | A concrete repeated operation; exact share is unmeasured. |
| Ed25519 signature verification | Final `VerifyingKey::verify` call | Required cryptographic cost; exact share is unmeasured and must not be inferred from the inclusive timer. |

The observed data supports optimization of repeated semantic and canonical work before cryptographic micro-optimization. It does not support the claim that Ed25519 is slow or fast in isolation. P0 must make that claim measurable.

A useful prior comparison is Phase 73's stream-versus-aggregate measurement. At eight observations, stream verification is 18.243 ms p50 versus 15.226 ms for aggregate verification, a 3.017 ms difference. Stream attestation is 9.334 ms versus 6.071 ms for aggregate attestation, a 3.263 ms difference. The gap grows with observation count, which is consistent with stream-specific frame traversal, canonical serialization, and digest work layered around the shared semantic/aggregate contract. It is not a pure serialization measurement, but it is stronger evidence for repeated stream-wrapper work than the Phase 74 inclusive timer alone.

| Phase 73 observations | Stream verify p50 | Aggregate verify p50 | Stream-only delta | Stream attest p50 | Aggregate attest p50 |
|---:|---:|---:|---:|---:|---:|
| 1 | 10.659 ms | 10.150 ms | 0.509 ms | 1.582 ms | 1.137 ms |
| 2 | 11.756 ms | 10.879 ms | 0.877 ms | 2.686 ms | 1.853 ms |
| 4 | 14.000 ms | 12.211 ms | 1.790 ms | 4.927 ms | 3.277 ms |
| 8 | 18.243 ms | 15.226 ms | 3.017 ms | 9.334 ms | 6.071 ms |

## 3. Specific overhead attribution: Ed25519 versus serialization and hashing

The current architecture performs one Ed25519 verification per attestation, not one signature verification per stream frame. This matters for attribution. Between one and eight observations in the Phase 73 artifact, stream-verification p50 increases from 10.659 ms to 18.243 ms, a 7.584 ms increase, while the attestation still carries one signature. The incremental increase must therefore come from repeated stream/report verification, canonical work, content hashing, or size-dependent contention; it cannot be attributed solely to an additional seven Ed25519 calls. The signature cost is still present in every row, but its absolute share is unknown.

| Suspected contributor | Evidence-backed statement | Attribution confidence before P0 |
|---|---|---|
| Ed25519 verification | One final `VerifyingKey::verify` call occurs per attestation. It is required, but its isolated p50/p95/p99 is not present in Phase 73 or Phase 74 artifacts. | Low for percentage; high that it is a required fixed-per-attestation cost. |
| Public-key parsing | `VerifyingKey::from_bytes` is reconstructed inside each shared verification call. P1 can move this to explicit key registration. | High that the work repeats; low for its percentage. |
| Signing-payload construction | Canonical payload JSON and domain prefix are rebuilt for each verification. | High that the work repeats; low for its percentage. |
| Canonical report/stream serialization | Every frame is reserialized by stream verification, and the stream is serialized again for digest/integrity checks. Phase 73 stream-versus-aggregate verification deltas grow from 0.509 ms at one observation to 3.017 ms at eight. | Medium as a stream-wrapper contributor; the delta is not a pure serialization timer. |
| SHA-256 content/frame hashing | Stream content and network frame digests hash bounded canonical bytes. Work grows with serialized size, but no isolated hash timer exists. | Medium that it is size-dependent; low for percentage. |
| Semantic freshness and nested receipt/report validation | Snapshot envelope and nested receipt/aggregate checks are called before the signature check and repeat per frame. | High as a repeated pipeline contributor; low for exact percentage until timed separately. |
| TCP receive/framing | Phase 74 receive p50 is 0.132 ms at 1/1 and 2.187 ms at 8/8, versus verification p50 of 10.703 ms and 228.603 ms. | High that it is not the first-order bottleneck in this fixture. |

This leads to a precise optimization order: first eliminate repeated semantic/canonical work with immutable evidence; second remove key parsing and signing-payload rebuilding; third measure whether hashing is material; only then evaluate Ed25519 batching or alternate implementations. Any statement such as “Ed25519 consumes 98.9%” is invalid because 98.9% is the share of the inclusive verification timer in one Phase 74 row, not an isolated cryptographic measurement.

## 4. P0 instrumentation specification

### 4.1 Instrumentation boundary

Add a dedicated internal module, preferably `src/emission_diagnostic_instrumentation.rs`, with a no-op default and an explicitly enabled collector. Instrumentation must be observational only. It must not change acceptance decisions, sequence advancement, trust registration, aggregation mutation, or error ordering.

The hot path should create a bounded operation-local sample and publish it once at operation completion. Do not acquire a global mutex for every stage. The default build may use a no-op sink or fixed-size atomic counters; an enabled benchmark sink may use a bounded queue drained by a collector. If a queue is full, increment a dropped-sample counter and continue the verification decision without blocking or changing the result.

A proposed typed contract is:

```rust
pub struct DiagnosticVerificationSample {
    pub schema_version: u8,
    pub frame_count: u16,
    pub stream_bytes: u32,
    pub outcome: VerificationOutcome,
    pub stages: DiagnosticStageTimings,
    pub counters: DiagnosticStageCounters,
}

pub struct DiagnosticStageTimings {
    pub transport_receive_ns: u64,
    pub transport_frame_integrity_ns: u64,
    pub stream_shape_ns: u64,
    pub snapshot_fingerprint_ns: u64,
    pub nested_report_verify_ns: u64,
    pub canonical_report_serialize_ns: u64,
    pub canonical_stream_serialize_ns: u64,
    pub content_hash_ns: u64,
    pub attestation_shape_ns: u64,
    pub trust_lookup_ns: u64,
    pub public_key_parse_ns: u64,
    pub signing_payload_serialize_ns: u64,
    pub ed25519_verify_ns: u64,
    pub aggregate_admission_ns: u64,
    pub end_to_end_ns: u64,
}

pub enum VerificationOutcome {
    Accepted,
    Rejected,
}
```

The exact Rust names may change, but the contract must remain bounded, serializable, and free of source text, key bytes, signature bytes, raw attestation payloads, or tokens. `u16`, `u32`, and `u64` fields must saturate or return a typed overflow result; they must never wrap into an apparently valid measurement.

### 4.2 Stage taxonomy and timer semantics

The implementation must document whether each timer is **exclusive** or **inclusive**. The recommended approach is exclusive leaf timings plus separately recorded inclusive totals. The following stages must be measured:

| Stage | Instrument at | Required meaning |
|---|---|---|
| `transport_receive` | `read_frame` and JSON decode boundary | Time spent reading the length-prefixed payload and decoding its bounded envelope. |
| `transport_frame_integrity` | network frame digest validation | Time for Phase 74 metadata/canonical-attestation frame-digest verification. |
| `stream_shape` | stream structural checks | Version, IDs, frame count, sequence, size, context, and canonical stream envelope checks. |
| `snapshot_fingerprint` | `SemanticSnapshotEnvelope::verify_for` | Candidate fingerprint recomputation and snapshot/profile/root comparison. |
| `nested_report_verify` | `EmissionDiagnosticReport::verify_for` through receipt/aggregate verification | Current-state and receipt/aggregate consistency work, excluding separately timed serialization and hashes where possible. |
| `canonical_report_serialize` | report `to_json` calls | Canonical report encoding and bounded-size checks. |
| `canonical_stream_serialize` | stream `to_json` or equivalent canonical wire assembly | Canonical stream encoding and bounded-size checks. |
| `content_hash` | stream/aggregate content hash helper | Domain-separated SHA-256 over already-canonical content bytes. |
| `trust_lookup` | explicit verifier registry lookup | Exact public-key membership lookup, excluding key parsing. |
| `public_key_parse` | `VerifyingKey::from_bytes` | Parse and validate the 32-byte public key. P1 should make this a zero-or-near-zero hot-path cost by parsing at registration. |
| `signing_payload_serialize` | canonical signing-payload construction | Canonical payload construction and domain-prefix assembly. |
| `ed25519_verify` | `VerifyingKey::verify` only | The isolated signature verification operation over an already prepared payload. |
| `aggregate_admission` | verified observation construction and per-node aggregator admission | Sequence, bounds, and all-or-nothing aggregation work after cryptographic verification. |
| `end_to_end` | public operation boundary | Full operation latency for correlation with the Phase 74 artifact. |

If a stage cannot be isolated without duplicating work, record the limitation explicitly and use an inclusive parent timer rather than reporting a fabricated leaf number. Stage sums should be checked against the parent timer with an explicit `unattributed_ns` field.

### 4.3 Counters and dimensions

The collector must record bounded counters for accepted and rejected operations, typed rejection categories, frame count, stream-byte bucket, canonical serialization bytes, SHA-256 invocation count, public-key parse count, signature verification count, trust lookups, stale-snapshot failures, replay/gap failures, frame-integrity failures, and dropped telemetry samples. A benchmark row must include frame count, connection concurrency, cold/warm mode, and whether evidence reuse was enabled.

Do not use raw node IDs or public keys as unconstrained metric labels. Use fixed outcome enums and bounded numeric buckets. If an approved node label is required later, use a bounded configured identifier and reject labels outside the configured registry. Metrics and logs must never contain private keys, public keys, signatures, raw canonical payloads, source text, prompts, tokens, or full diagnostic contents.

### 4.4 P0 benchmark matrix

Retain the Phase 74 matrix of frame counts `1/2/4/8` and concurrent connections `1/2/4/8`. Add these isolated operations:

| Benchmark mode | Preparation | Timed work |
|---|---|---|
| Receive-only | Prebind listener and use a known bounded payload. | TCP read and length-prefix handling only. |
| Frame-integrity-only | Precompute canonical attestation JSON and frame metadata. | Phase 74 frame digest calculation and comparison. |
| Semantic freshness | Reuse a fixed snapshot and candidate UEG map. | Snapshot-envelope fingerprint and root/profile checks. |
| Nested report verification | Use an already parsed report. | Report, aggregate, receipt, and current-state verification. |
| Canonical serialization | Use valid immutable report and stream objects. | Report serialization, stream serialization, and byte-size checks separately. |
| Hash-only | Use already prepared canonical bytes. | Content-domain SHA-256, frame-domain SHA-256, and signing-payload hash if present. |
| Key parse-only | Use fixed 32-byte public-key arrays, never emitted. | `VerifyingKey::from_bytes` only. |
| Ed25519-only | Preparse the verifying key and prebuild the exact signing payload. | `VerifyingKey::verify` only. |
| Full cold verification | Reconstruct all current objects per sample. | Existing complete path. |
| Full warm verification | Reuse P1 evidence and parsed key material where permitted. | Optimized path with identical acceptance semantics. |
| Loopback end-to-end | Start the Phase 74 listener and bounded client. | Handshake, send, receive, verification, and completion. |

Use the same deterministic fixture for baseline and optimized runs, collect at least p50/p95/p99, and retain zero-error status. For p99 stability, the harness may increase the sample count beyond Phase 74's twelve samples, but it must report the exact count and must not mix cold and warm samples. Use `black_box` or an equivalent guard so isolated cryptographic work is not optimized away.

P0 must compare instrumentation-disabled and instrumentation-enabled runs. The enabled path must demonstrate its own overhead with the same fixture and matrix. An acceptable initial target is that enabled instrumentation remain within a small single-digit percentage of the no-instrumentation p50/p95 for local benchmarks; the report must state measured overhead rather than assume the target was met.

## 5. P1 immutable verified-evidence specification

### 5.1 Evidence construction

Add a private-field, cloneable, immutable evidence type. A recommended contract is:

```rust
pub struct CanonicalDiagnosticEvidence {
    canonical_stream_bytes: Arc<[u8]>,
    stream_digest: [u8; 32],
    content_hash: [u8; 32],
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
    frame_count: u16,
    total_frame_bytes: u32,
}

pub struct VerifiedDiagnosticEvidence {
    canonical: Arc<CanonicalDiagnosticEvidence>,
    attestation_id: u64,
    content_type: EmissionDiagnosticAttestationContent,
    attestation_digest: [u8; 32],
    trusted_key_id: [u8; 32],
    trust_epoch: u64,
}
```

The type must be constructed only by an internal verification constructor. The constructor must:

1. Validate stream shape, bounds, target, batch, profile, unit set, frame sequences, nested report context, canonical report bytes, and stream bytes.
2. Call current snapshot verification against the exact `SemanticSnapshotEnvelope`, profile, and candidate UEG map.
3. Recompute and compare the stream digest and content hash.
4. Store the already-validated canonical stream bytes in an immutable `Arc<[u8]>`; do not expose mutable slices.
5. Verify the attestation's exact content type and content hash against the canonical evidence.
6. Use an explicitly registered parsed `VerifyingKey` and verify the exact domain-separated signing payload.
7. Capture the verifier's monotonically increasing `trust_epoch` after the trusted-key lookup and before returning evidence.

The constructor must return a typed error and no partially valid object. If any stage fails, no evidence object is returned and no transport sequence or aggregate state advances.

### 5.2 Consumer APIs

Keep the existing full verification APIs as correctness-preserving compatibility paths. Add evidence-aware APIs rather than silently weakening the old ones:

```rust
impl DiagnosticAttestationVerifier {
    pub fn verify_stream_evidence(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        evidence: &CanonicalDiagnosticEvidence,
    ) -> Result<VerifiedDiagnosticEvidence, EmissionDiagnosticAttestationError>;
}

impl MultiNodeDiagnosticReceiver {
    pub fn ingest_verified(
        &mut self,
        node_id: u64,
        connection_id: u64,
        sequence: u64,
        evidence: VerifiedDiagnosticEvidence,
    ) -> Result<(), EmissionDiagnosticNetworkError>;
}
```

`ingest_verified` must still enforce the network node/connection/sequence contract. Evidence reuse does not advance a replay window, prove that a frame arrived on a particular connection, or bypass per-node aggregate limits. The network adapter may continue to expose a raw receive method for compatibility, but the preferred Phase 75 path should create evidence before aggregation.

The evidence-aware attestation path should check that the attestation digest, ID, content type, trusted key identity, and trust epoch are exactly the values covered by the evidence. It must not accept a different signature-bearing object merely because it references the same stream content hash. This prevents signature substitution and preserves attestation-level auditability.

### 5.3 Parsed public-key registry

Change the verifier's internal registry from a set-like map of `[u8; 32] -> ()` to a map that stores the parsed `VerifyingKey` and an internal trust epoch. Registration must parse and validate the key once. A successful new registration or revocation increments the epoch; idempotent registration may leave the epoch unchanged if and only if the stored parsed key is identical. Revoke must remove the key and invalidate evidence cached under the prior epoch.

The exact public-key lookup must remain mandatory. A non-empty registry is not sufficient. The key identity in the attestation must match the registered key used for verification, and a malformed remote key must remain a typed error rather than a panic. The public JSON shape of `EmissionDiagnosticAttestation` should not be changed solely to add a cache field.

### 5.4 Canonical signing-payload reuse

Do not add a serialized cache field to the public attestation JSON. Instead, construct an internal verification material object from the validated attestation. It may retain the canonical signing payload, parsed key reference, signature bytes, content hash, attestation ID, content type, and trust epoch. The material must be discarded or invalidated when any attestation field changes.

The cache key must cover the complete canonical attestation bytes or an equally strong digest of all fields, not only `content_hash`. It must include attestation ID, content type, public-key identity, metadata, signature, version, and trust epoch. Reusing a payload for an attestation with different metadata or signature is forbidden even when the stream content hash is unchanged.

### 5.5 Snapshot-bound evidence reuse

A local evidence cache may be introduced after the immutable type is working. Its key must contain the complete canonical attestation identity, stream digest, batch ID, profile key, exact unit-root map, and a trust-registry epoch. Use a bounded capacity with explicit hits, misses, evictions, and invalidations. Never reuse evidence across a changed candidate UEG root, changed profile, changed unit set, changed batch, changed attestation bytes, revoked key, or trust-epoch change.

Evidence cache lookup must occur only after bounded input-shape checks. A cache hit may skip repeated semantic/canonical work, but the network layer must still enforce the current connection's node, connection ID, sequence, frame budget, and replay window. A cache hit must not be treated as a new attestation or new aggregate observation until the transport ordering contract accepts it.

## 6. Security and correctness invariants

The following invariants are non-negotiable:

| Invariant | Required test |
|---|---|
| Evidence is immutable after construction. | Attempted mutation is impossible through the public API; clones retain byte-for-byte identity. |
| Evidence is current-state bound. | Changed UEG, changed profile, changed batch, missing unit, and unexpected unit all reject. |
| Evidence is canonical-byte bound. | Reordered JSON, altered nested report bytes, altered stream digest, and altered frame encoding reject. |
| Evidence is attestation bound. | Wrong type, wrong content hash, changed metadata, changed ID, changed signature, unknown key, and revoked key reject. |
| Evidence reuse is trust-epoch bound. | Evidence created before registration/revocation changes is not admitted under a different epoch. |
| Evidence reuse is transport-independent. | Reusing equivalent evidence on a new connection still requires a valid handshake and next sequence. |
| Verification failure is atomic. | No sequence counter, queue, aggregator total, or cache success marker advances on failure. |
| Observability is non-authoritative. | Dropped telemetry, collector failure, or disabled instrumentation does not change acceptance. |
| Sensitive data is absent from metrics. | Serialized samples contain no source, public-key bytes, signatures, private-key data, tokens, or raw diagnostic payloads. |
| Existing APIs remain fail closed. | Phase 73 and Phase 74 integration suites continue to pass unchanged. |

## 7. Performance experiment and expected conclusions

The first P0 run should report both absolute values and normalized shares. For each row, calculate verification share as `verify_p50 / e2e_p50`, non-verification overhead as `e2e_p50 - verify_p50`, and p99/p50 tail inflation. The existing artifact shows verification shares from approximately 87.4% to 98.9% across the matrix, with a maximum observed verify p99 of 240.625 ms and receive p99 of 9.722 ms.[1]

The key attribution experiment is a controlled four-way decomposition:

| Experiment | What it answers |
|---|---|
| Prebuilt payload + prebuilt `VerifyingKey` + Ed25519 only | Is the cryptographic primitive itself material at this payload size? |
| Key parsing only | How much repeated verifier construction costs before P1 registry reuse? |
| Canonical bytes + SHA-256 only | How much hashing costs when JSON allocation and semantic checks are excluded? |
| Current-state semantic and nested report path with crypto excluded | How much of the inclusive timer is semantic freshness and receipt/report validation? |

The current evidence predicts that P1 canonical-byte and semantic-evidence reuse will have the largest end-to-end impact because the same verified stream is checked once per received attestation and each stream contains repeated equivalent frames. It does not predict the exact percentage improvement. That must be measured with cold/warm rows and correctness gates. Parsed-key reuse and signing-payload reuse are low-risk micro-optimizations that should be measured independently; they should not be described as the primary bottleneck until P0 supplies their actual share.

## 8. Implementation order and integration with later phases

| Step | Deliverable | Exit gate |
|---|---|---|
| P0-A | Instrumentation module, no-op sink, bounded sample schema, stage timers, and counters. | Existing behavior unchanged; instrumentation overhead and redaction tested. |
| P0-B | Isolated benchmark operations and expanded Phase 74 matrix. | Ed25519, key parsing, serialization, hashing, semantic, and network values reported separately. |
| P1-A | `CanonicalDiagnosticEvidence` with current-state and canonical-byte verification. | Golden bytes, stale-state rejection, tamper rejection, and no partial mutation. |
| P1-B | Parsed-key registry, trust epoch, `verify_stream_evidence`, and `VerifiedDiagnosticEvidence`. | Exact-key trust, revocation invalidation, signature substitution rejection, and Phase 73 compatibility. |
| P1-C | Evidence-aware multi-node ingestion and bounded cache metrics. | Transport ordering still required; cache hits never bypass replay, node binding, or aggregate limits. |
| Phase 76 | Fixed-size asynchronous verification workers between network receive and aggregation. | Bounded queue/memory, cancellation, backpressure, source ordering, and deterministic failure propagation. |
| Phase 77 | Production-shaped metrics and separate listener liveness/readiness. | Redacted counters/histograms, saturation signals, stale-state readiness failure, and no authority expansion. |
| Phase 78 | Durable handoff and replay epochs using existing queue/audit patterns. | Atomic persistence, re-verification on retry, epoch fencing, idempotent acknowledgement, and crash recovery. |
| Phase 79 | Zero-trust service identity and signed external audit evidence. | Explicit key-to-service binding, rotation/revocation, authorization separate from attestation, and bounded audit events. |
| Phase 80 | Consensus, failover, and controlled-evolution policy integration. | Diagnostic evidence is a typed input only; current term/epoch, quorum, fencing, and ledger gates remain authoritative. |
| Phase 81 | Production transport hardening and multi-host validation. | TLS or mesh decision, deadlines, quotas, loss/reorder/duplication tests, deployment probes, and approval-controlled rollout. |

## 9. Non-goals

P0 and P1 must not add TLS, certificate authority behavior, peer discovery, durable queue ownership, quorum inference, leader promotion, proposal application, or unrestricted logging. They must not remove current-state verification, canonical JSON checks, domain separation, exact public-key registration, or replay/gap enforcement. They must not make the semantic core depend on network connectivity or make observability a prerequisite for correctness.

## References

[1]: ../benchmarks/phase74_emission_diagnostic_network.json "Phase 74 sanitized loopback benchmark artifact"
[2]: ../src/emission_diagnostic_attestation.rs "Phase 73 attestation verification implementation"
[3]: ../src/emission_diagnostic_stream.rs "Phase 70–71 stream verification and canonical serialization"
[4]: ../src/emission_receipt.rs "Receipt verification and nested snapshot checks"
[5]: ../src/semantic_snapshot_envelope.rs "Snapshot-bound fingerprint verification"
[6]: ../src/incremental_semantic.rs "Dependency-aware semantic validation and per-function reuse"
[7]: ../src/emission_diagnostic_network.rs "Phase 74 TCP framing, frame integrity, and connection windows"
[8]: ../docs/PHASE74_OPTIMIZATION_AND_INTEGRATION_ROADMAP.md "Phase 74 optimization and subsequent-phase roadmap"
