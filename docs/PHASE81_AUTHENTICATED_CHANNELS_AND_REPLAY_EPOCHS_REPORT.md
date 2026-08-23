# Phase 81 Authenticated Service Channels and Durable Replay Epochs

**Author:** Manus AI
**Status:** Phase 81 local authenticated service-channel and durable replay-epoch primitives are implemented and tested. The channel envelope is transport-agnostic and fail closed; TLS, certificate distribution, external cluster integration, readiness, and live production rollout remain separate deployment gates.

## Service-channel authentication

`AuthenticatedServiceChannelEnvelope` is independent from diagnostic content attestation. Its domain-separated signing payload binds the channel ID, sender and receiver service IDs, canonical identity IDs, signer ID and generation, connection epoch, contiguous sequence, nonce, and payload hash. The receiver resolves the sender key from the configured `ServiceIdentityRegistry`; it does not trust a public key supplied by the envelope.

Before replay state or application payload exposure, the receiver validates the schema, bounded identifiers, canonical identity paths, positive signer generation, positive connection epoch and sequence, payload size and hash, exact signature length, receiver binding, active signer generation, and Ed25519 signature. Replay admission is private to the authenticated receiver path, preventing callers from persisting an unverified envelope. A valid envelope authenticates the service channel; it does not authorize consensus, policy, deployment, or aggregate mutation.

## Durable replay epoch state

`DurableReplayEpochStore` persists channel bindings, canonical sender and receiver identity IDs, connection epoch, highest contiguous sequence, a bounded set of seen envelope hashes, and a domain-separated state digest covering every binding. State transitions use a deterministic temporary file, file synchronization, atomic rename, and containing-directory synchronization. On restart, a stale temporary artifact is removed only when the committed state exists and validates. Missing, malformed, misbound, or digest-inconsistent committed state fails closed.

Admission is ordered as authentication and payload-integrity verification, exact current-epoch validation, deterministic duplicate detection, contiguous sequence checking, bounded replay-window admission, durable state persistence, and only then payload exposure. A gap, stale sequence, old epoch, tampered payload/signature, full window, or persistence failure cannot advance replay state. Epoch rollover is monotonic and atomically resets the sequence window.

| Gate | Result |
|---|---|
| Independent service identity | Implemented locally; separate from content attestation |
| Payload and receiver binding | Implemented and tested |
| Replay duplicate/gap/stale rejection | Implemented and tested |
| Durable restart recovery | Implemented and tested with stale temporary artifact cleanup |
| Epoch rollover | Implemented and tested; old epoch rejected |
| Signer rotation/revocation | Implemented and tested against the Phase 79 registry |
| External TLS/certificates/cluster | Pending separate production integration |

## Validation evidence

The focused Phase 81 target now contains six local tests covering valid authenticated delivery, receiver and payload tampering, restart reload, duplicate delivery, stale temporary recovery, epoch rollover, gap rejection, signer revocation, corrupted replay state, and canonical identity binding. The follow-up focused target passed 6 tests with zero failures; the complete `cargo test --all-targets` run passed 451 tests with zero failures, ignored tests, or filtered tests after hardening.

Only sanitized local fixtures are used. No raw payload, key material, signature, credential, or full fencing token is emitted into logs or benchmark artifacts.

## Production boundary

This phase supplies the cryptographic channel and replay-state kernel. It does not claim TLS, confidentiality, network certificate issuance, service discovery, cluster readiness, resource budgets, health probes, staging deployment, rollback, or approval-controlled production promotion. Those controls remain required before a live Phase 81 rollout.

## References

[1]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"

[2]: ../tests/phase81_service_channel_integration.rs "Phase 81 authenticated service-channel integration tests"

[3]: ../src/emission_diagnostic_service_channel.rs "Phase 81 authenticated service-channel and replay implementation"
