# Phase 73: emission diagnostic cryptographic attestation

## Executive summary

Phase 73 adds typed Ed25519 attestation for canonical Phase 70 diagnostic streams and verified Phase 72 distributed-shaped aggregates. The layer signs a domain-separated canonical content hash, stores the public key and signature in a bounded JSON object, and verifies only against explicitly registered public keys. Stream attestation creation and verification retain current-envelope verification. Aggregate attestation creation and current-state verification use the new aggregate invariant checker before content hashing.

The implementation is intentionally not a network or key-management system. Signing keys are held in memory by a local `DiagnosticAttestationKey`. `DiagnosticAttestationVerifier` is a bounded explicit registration set, not a certificate authority. A valid signature proves that a holder of the corresponding private key signed a specific content hash; it does not establish identity, trust, quorum, or authorization.

## Typed contract

`EmissionDiagnosticAttestation` is a version-1, unknown-field-rejecting JSON object with a non-zero local attestation ID, stream/aggregate content type, fixed 32-byte content hash, fixed 32-byte public key represented as a bounded JSON byte vector, fixed 64-byte Ed25519 signature represented as a bounded JSON byte vector, and a sorted bounded metadata map. The serialized object is limited to 32 KiB, metadata to eight entries, keys to 64 bytes, and values to 256 bytes.

The content hash is SHA-256 over `un1c0/phase73/emission-diagnostic-content/v1`, a one-byte content-type tag, and the canonical Phase 70 stream bytes or canonical Phase 72 aggregate summary. The signature payload is domain-separated and canonical, and signs the attestation’s version, ID, content type, hash, public key, and metadata. Stream attestation verifies the stream against the caller’s current snapshot/profile/candidates before signing. Aggregate attestation verifies all aggregate observations and accounting before signing.

## Failure ordering

Parsing rejects oversized input, malformed JSON, unknown fields, unsupported version, zero ID, invalid fixed-size key/signature vectors, metadata overflows, and non-canonical bytes before returning an attestation. Stream verification then checks current semantic state before content-hash and signature checks. Aggregate verification checks aggregate current-state invariants and exact aggregate content before cryptographic verification. Wrong content types, unknown public keys, content-hash changes, signature changes, stale candidates, empty aggregates, and trust-store overflow are typed failures.

## Benchmark evidence

The benchmark uses a deterministic one-unit/two-function fixture, 64 samples per row, a derived test-only signing key, zero errors, and no recorded secret material. The stream rows use equivalent stream observation counts 1/2/4/8. Aggregate rows contain one source whose stream contains the selected number of frames.

| Observations | Stream bytes | Stream sign p50/p95/p99 | Stream verify p50/p95/p99 | Aggregate sign p50/p95/p99 | Aggregate verify p50/p95/p99 |
|---:|---:|---|---|---|---|
| 1 | 3,439 | 1,582,445 / 1,722,221 / 1,743,667 ns | 10,659,234 / 10,815,585 / 10,912,092 ns | 1,136,543 / 1,184,316 / 1,241,016 ns | 10,149,892 / 12,619,181 / 12,870,003 ns |
| 2 | 6,373 | 2,686,327 / 2,721,610 / 2,730,629 ns | 11,756,079 / 11,894,383 / 11,977,422 ns | 1,853,245 / 1,979,823 / 2,187,487 ns | 10,879,150 / 11,002,138 / 11,116,484 ns |
| 4 | 12,256 | 4,927,227 / 5,098,449 / 5,133,927 ns | 14,000,393 / 14,162,319 / 14,206,959 ns | 3,277,172 / 3,408,389 / 3,426,666 ns | 12,210,812 / 12,482,930 / 12,739,839 ns |
| 8 | 24,032 | 9,334,126 / 9,486,162 / 9,540,395 ns | 18,242,833 / 18,497,635 / 19,186,257 ns | 6,070,610 / 6,177,658 / 6,214,934 ns | 15,226,230 / 16,080,082 / 16,571,475 ns |

Stream signing p50 grows from **1.582 ms to 9.334 ms** as stream bytes grow from 3,439 to 24,032 bytes because current-state verification and canonical stream hashing remain part of attestation creation. Stream verification p50 grows from **10.659 ms to 18.243 ms**, dominated by current-envelope re-verification rather than Ed25519 itself. Aggregate signing p50 grows from **1.137 ms to 6.071 ms** because aggregate attestation rechecks the selected stream state before hashing the summary. Aggregate verification p50 grows from **10.150 ms to 15.226 ms** for the same reason. The benchmark measures local code only and does not measure network, key storage, certificate, or production service behavior.

## Coverage evidence

The Phase 73 integration suite contains **5 tests**, all passing in the validated run. It covers stream and aggregate canonical round trips, deterministic signatures, explicit trust registration and revocation, stream/aggregate content separation, content-hash tampering, signature tampering, wrong content types, stale candidate rejection, zero IDs, unsupported versions, unknown fields, non-canonical JSON, metadata entry/key/value limits, empty aggregate rejection, and trusted-key cardinality limits.

The suite also retains the Phase 72 aggregate current-state boundary. The new `DistributedEmissionAggregator::verify_for` recomputes bounded frame/byte totals and the aggregate digest after re-verifying each nested stream, rejecting inconsistent internal state before attestation is accepted.

## Authority boundary

No network sockets, filesystem writes, durable key store, secret reads, process execution, cluster membership, quorum, certificate authority, token issuance, authorization, or remote trust discovery was added. Private signing material is not serialized by the attestation type or benchmark artifact. Public-key registration is explicit and bounded, and revocation removes future verification authority from that local verifier instance.

## Validation and closeout

The required closeout is the reusable-skill validator, formatting check, Phase 67–73 targeted integration suites, JSON artifact validation, and complete all-target Rust tests. The commit must include only the Phase 73 module, export, tests, benchmark, JSON artifact, report, roadmap row, and reusable reference. Generated build outputs and unrelated pre-existing worktree changes must remain unstaged.

## References

1. [`PHASE73_EMISSION_DIAGNOSTIC_ATTESTATION_PLAN.md`](PHASE73_EMISSION_DIAGNOSTIC_ATTESTATION_PLAN.md) — Phase 73 design and security boundary.
2. [`emission_diagnostic_attestation.rs`](../src/emission_diagnostic_attestation.rs) — typed implementation.
3. [`phase73_emission_diagnostic_attestation_integration.rs`](../tests/phase73_emission_diagnostic_attestation_integration.rs) — integration and fail-closed coverage.
4. [`phase73_emission_diagnostic_attestation.json`](../benchmarks/phase73_emission_diagnostic_attestation.json) — sanitized benchmark artifact.
5. [`phase73-emission-diagnostic-attestation.md`](../../skills/agentic-system-engineering/references/phase73-emission-diagnostic-attestation.md) — reusable engineering guidance.
