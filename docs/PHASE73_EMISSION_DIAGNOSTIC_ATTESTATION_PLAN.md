# Phase 73: cryptographic attestation for emission diagnostic streams

## Objective

Phase 72 established a bounded local async transport and distributed-shaped aggregator. Phase 73 adds the first real authority boundary: typed cryptographic attestation of a verified diagnostic stream or aggregate, using Ed25519 signatures over canonical content hashes. This is not a network transport phase. It is a local signing and verification layer that can later be used as the trust anchor for real network streaming.

## Why attestation before streaming

Real network streaming requires an authenticated envelope, a replay-window lifecycle, connection identity, and a trust model. Introducing all of those in a single phase risks conflating transport mechanics with cryptographic trust. Phase 73 isolates the trust primitive: a caller with a registered signing key can produce a typed `EmissionDiagnosticAttestation` over a verified stream or aggregate. A verifier with the same public key can confirm the attestation without re-running the full semantic verification pipeline.

Phase 74 or later can then add a real network transport that carries attestations as authenticated envelopes, with connection-epoch replay windows, backpressure, and durable handoff.

## Typed contract

`EmissionDiagnosticAttestation` contains a version, a non-zero attestation ID, the attested content type (stream or aggregate), a domain-separated canonical content hash, an Ed25519 public key, an Ed25519 signature over the canonical hash, and a bounded metadata map. It does not contain source text, private keys, secrets, or authorization grants.

`DiagnosticAttestationKey` is a local in-memory signing key. It holds only the key pair in memory; it does not write to disk, network, or a key store. It is not a certificate authority, trust anchor, or cluster identity.

`DiagnosticAttestationVerifier` holds a bounded set of registered public keys. It verifies that the attestation's public key is registered, the signature is valid over the canonical hash, and the content hash matches the stream or aggregate being checked. It does not grant authorization, issue tokens, or expand trust beyond the registered key set.

## Domain separation and canonical content

The content hash is SHA-256 over a domain prefix, the content type byte, and the canonical Phase 70 stream bytes or Phase 72 aggregate digest. The domain prefix is `un1c0/phase73/emission-diagnostic-attestation/v1`. This prevents a stream attestation from being replayed as an aggregate attestation or vice versa.

The signing payload is SHA-256 over the domain prefix and the content hash. Ed25519 signs the 32-byte payload hash, not the raw stream bytes, to keep the signature operation bounded and deterministic.

## Security and authority boundaries

Phase 73 introduces no network sockets, filesystem writes, cluster membership, quorum, authorization decisions, certificate issuance, token generation, key distribution, or secret storage. The signing key is in-memory only. The verifier accepts only explicitly registered public keys. A valid attestation proves that the holder of the corresponding private key signed a specific canonical content hash; it does not prove identity, authorize an action, or grant access to any resource.

Attestation IDs are local non-zero integers, not distributed sequence numbers. They carry no ordering semantics beyond the local context in which they are created.

## Coverage matrix

The integration suite must cover valid attestation creation and verification for both stream and aggregate content, exact content-hash binding, signature rejection for tampered hashes, unknown-key rejection, wrong-type rejection, zero-ID rejection, version mismatch rejection, empty verifier rejection, bounded metadata limits, and no partial verification result after any failure. It must also verify that a stream attestation cannot be used to verify an aggregate and vice versa.

## Benchmark protocol

Use deterministic fixtures and 64 samples at observation counts 1/2/4/8. Record attestation creation p50/p95/p99, verification p50/p95/p99, content-hash size, errors, and sanitized authority markers. Expect sub-millisecond signing and verification because Ed25519 is fast and the payload is 32 bytes. Do not infer production key-management or network latency from these measurements.

## Closeout gates

Export the module, add typed tests and benchmark JSON, update the roadmap and reusable skill, validate formatting and the skill package, run all Phase 67–73 targeted suites and the complete all-target Rust suite, check sanitized artifacts, exclude generated build noise, and commit only intended files. Confirm that no private key, secret, or token appears in any committed file, log, or benchmark artifact.

## Next boundary after Phase 73

Phase 74 should add a real network transport that carries Phase 73 attestations as authenticated envelopes. It must define connection identity, replay-window lifecycle, backpressure, and durable handoff semantics. None of those properties should be inferred from Phase 73's local in-memory key pair or attestation ID.
