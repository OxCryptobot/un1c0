# Phase 79 Service Identity and Durable Outbox Report

**Author:** Manus AI
**Status:** Formal Phase 79 service-identity and durable-outbox sub-gates are implemented locally. Rotation, revocation, exact evidence binding, bounded persistence, idempotent enqueue, and crash-restart recovery are validated in local deterministic fixtures. Production service channels, external sink authorization, and deployment promotion remain Phase 81 boundaries.

## Scope and separation contract

Phase 79 introduces an explicit service identity layer rather than treating Phase 73/75 content attestation as service authorization. `ServiceIdentityRegistry` owns a service identifier, a canonical SPIFFE-style identity descriptor, active signer selection, signer public keys, signer generations, and revocation state. `ServiceIdentityEnvelope` binds the independent service identity to an evidence digest, diagnostic stream, source sequence, trust configuration generation, predecessor digest, and signer generation.

> A valid content attestation is necessary evidence for an identity envelope, but it is not itself service authorization.

The identity envelope uses a fixed schema version and a domain-separated Ed25519 signing payload. The verifier obtains the public key from the configured registry rather than trusting a key carried as arbitrary envelope data. It checks service ID, canonical identity ID, signer generation, evidence digest, stream ID, source sequence, trust generation, predecessor shape, and the exact signature before accepting historical identity evidence.

## Signer lifecycle

| Control | Implementation | Fail-closed result |
|---|---|---|
| Initial registration | One initial signer may be registered with a non-zero generation. | Reject duplicate or revoked-ID rebinding. |
| Rotation | Rotation requires the current active signer, a distinct signer ID, a strictly higher generation, and a new public key. | Reject stale, inactive, duplicate, or generation-regressing rotations. |
| Revocation | Revocation marks a signer unusable for new active envelopes and removes it from the active slot. | New issuance fails; historical envelopes remain verifiable for audit replay. |
| Persistence | Registry changes use temporary-file write, `sync_all`, atomic rename, and parent-directory sync. | Failed persistence leaves the in-memory registry unchanged. |
| Identity separation | Service identity signer registry is independent from diagnostic attestation key trust. | Content integrity cannot grant service authorization. |

The historical-versus-active distinction is deliberate. Existing signed identity envelopes remain verifiable against the recorded signer generation after rotation or revocation, while new issuance requires the active non-revoked signer. This permits deterministic audit replay without allowing a revoked signer to issue new authority-bearing evidence.

## Durable outbox

`DurableServiceIdentityOutbox` stores only validated, bounded JSON envelopes under digest-derived filenames. It enforces a configurable maximum entry count, a fixed envelope byte bound, exact stream/source-sequence collision behavior, predecessor binding for sequences after the first, idempotent re-enqueue of identical bytes, and atomic file creation with file and directory synchronization.

Pending entries are reloaded and verified after process restart. Non-JSON temporary files are ignored, while malformed JSON artifacts fail closed instead of being silently discarded. Acknowledgement verifies the envelope, removes only the exact digest-derived file, and synchronizes the directory. The outbox is durable local retry state; it is not proof that an external sink accepted or committed the event.

## Test evidence

The local `tests/phase79_service_identity_integration.rs` suite covers four integration tests.

The final closeout run passed **17 focused tests with zero failures** across the Phase 75 evidence, Phase 79 telemetry, and Phase 79 identity/outbox targets. The complete Rust all-target suite passed **440 tests with zero failures**. `cargo fmt --all -- --check`, the reusable `agentic-system-engineering` skill validator, and `git diff --check` also passed.

| Test | Evidence |
|---|---|
| Identity separation and exact binding | Wrong service, stream, evidence digest, and trust-generation changes fail signature or service binding validation. |
| Rotation/revocation persistence | Old signer becomes revoked, new generation becomes active, registry reload preserves state, historical envelope verification remains possible, and revoked IDs cannot be rebound. |
| Crash-restart outbox recovery | Identical enqueue is idempotent, pending entries survive reopen, temporary non-JSON crash artifacts are ignored, malformed JSON fails closed, and acknowledgement removes the durable entry. |
| Capacity and revoked-active behavior | Invalid capacity and full outbox return typed failures; historical envelopes remain replayable while new issuance after active revocation fails. |

The test fixture uses loopback-free local temporary directories and deterministic Ed25519 keys. No secrets, raw signatures, or full diagnostic payloads are emitted into reports or benchmark artifacts.

## Promotion assessment

| Formal gate | Status | Evidence and remaining boundary |
|---|---|---|
| F79.1 identity separation | **Pass locally** | Independent service identity registry and signed envelope; content attestation cannot substitute for active service authorization. |
| F79.2 rotation/revocation | **Pass locally** | Generation-bound rotation, revocation, atomic registry persistence, reload, historical verification, and no-rebinding tests. |
| F79.3 exact audit binding | **Pass for local identity envelopes** | Evidence digest, identity, stream, source sequence, predecessor, trust generation, and signer generation are signed. External sink authorization and signed acknowledgements remain outside this batch. |
| F79.4 durability | **Pass locally** | Bounded idempotent outbox, atomic create/acknowledge, directory sync, reopen recovery, partial-artifact handling, and corruption rejection. |

Formal production promotion remains blocked until Phase 81 independently authenticates service channels, establishes key-management and replay-epoch controls, proves resource/readiness gates, and completes approval-controlled staging. The local outbox must not be described as a distributed audit sink or delivery acknowledgement mechanism.

## References

[1]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"

[2]: ../tests/phase79_service_identity_integration.rs "Phase 79 service-identity and durable-outbox integration tests"
