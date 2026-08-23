# Phase 81 Release Notes

**Release:** Authenticated service channels and durable replay epochs
**Follow-up commit:** `591ea37` — `fix(ueg): harden replay identity boundaries`
**Status:** Local validation complete; production integration remains gated.

## Summary

Phase 81 hardens the local-first un1c0 diagnostic service-channel foundation. The channel envelope is independently authenticated through the Phase 79 service-identity registry and binds channel, sender and receiver service IDs, canonical identity IDs, signer ID and generation, connection epoch, sequence, nonce, payload hash, and signature. The implementation remains transport-agnostic: it does not claim live TLS/mTLS, certificate distribution, service discovery, cluster readiness, or production deployment.

The follow-up hardening closes two API-boundary risks identified during review. Replay admission is now private to `AuthenticatedServiceChannelReceiver`, so callers cannot persist an envelope through `DurableReplayEpochStore` without first passing the receiver’s authentication and integrity checks. `DurableReplayEpochState` now persists canonical sender and receiver identity IDs and includes them in its domain-separated state digest; reopen and receiver construction reject misbound replay artifacts.

## Included changes

| Area | Delivered behavior |
|---|---|
| Authenticated envelope | Strict schema and identifier bounds; channel, peer, canonical identity, signer generation, epoch, sequence, nonce, payload hash, and Ed25519 signature binding. |
| Active signer policy | New frames require the registry’s active, non-revoked signer and exact generation/key binding. Historical frames remain verifiable only where the surrounding policy permits; active issuance is fail closed. |
| Replay admission | Private store admission behind the verified receiver path; duplicate, stale, gap, wrong-epoch, full-window, and persistence failures do not advance state. |
| Durable replay state | Atomic temporary-file write, file sync, rename, directory sync, strict digest validation, stale temporary cleanup only after committed-state validation, and monotonic epoch rollover. |
| Identity binding | Canonical sender and receiver identity IDs are persisted, validated on reopen, checked by receiver construction, and covered by the replay-state digest. |
| Test evidence | Six Phase 81 integration tests plus the broader authenticated transport/replay matrix. |

## Security and boundary audit

The local audit reviewed the Phase 81 channel, Phase 74 diagnostic network, Phase 72 local diagnostic transport, Phase 36 authenticated recovery transport, Phase 79 identity registry, and the authenticated consensus socket transport. Public receive or ingest methods were traced to determine whether authentication, current-state verification, replay, quota, journal, queue, or aggregate mutation could be bypassed.

| Module | Boundary result |
|---|---|
| `emission_diagnostic_service_channel.rs` | Replay admission is private; receiver verifies shape, identity, active signer/generation, signature, and payload hash before admission. |
| `emission_diagnostic_network.rs` | Handshake and frame identity/integrity checks precede attestation return; multi-node ingestion re-verifies current evidence before journal or aggregate mutation. |
| `emission_diagnostic_transport.rs` | Local diagnostic transport verifies frame integrity and current semantic context; it is explicitly non-authoritative and not a service-authentication substitute. |
| `recovery_transport.rs` | Replay admission is private to the authenticated receiver; trusted-key and receiver/cluster/resource binding precede replay mutation. |
| `consensus.rs` authenticated socket transport | Trusted envelope verification precedes nonce, backpressure, replay, queue, ownership-fence, and delivery state changes; socket admission helpers remain internally bounded. |
| `emission_diagnostic_service_identity.rs` | Service identity is independent from content attestation; active signer authorization, generation binding, rotation, revocation, and atomic registry persistence remain enforced. |

No additional direct public replay-store admission bypass was found in the reviewed service-channel and authenticated transport surfaces. The audit does not turn this local review into a production authorization claim; future channel integrations must preserve the same verified-receiver boundary.

## Validation evidence

The focused security matrix covered 13 integration targets and passed **63 tests with zero failures**. The complete repository all-target suite at the preceding hardening validation passed **451 tests with zero failures, ignored tests, or filtered tests**. Formatting and committed-diff checks passed, and the reusable agentic-system engineering skill validator passed.

| Test target | Passed |
|---|---:|
| Phase 12 transport | 2 |
| Phase 13 transport stress | 1 |
| Phase 22 durable term/replay | 5 |
| Phase 24 socket backpressure | 4 |
| Phase 25 durable transport queue | 4 |
| Phase 27 replicated delivery ownership | 4 |
| Phase 28 partition ownership fencing | 6 |
| Phase 31 secure replay | 8 |
| Phase 36 recovery transport | 6 |
| Phase 72 diagnostic transport | 4 |
| Phase 74 diagnostic network | 9 |
| Phase 79 service identity | 4 |
| Phase 81 service channel | 6 |
| **Total** | **63** |

## Compatibility and operational notes

The `DurableReplayEpochStore::open` constructor now requires canonical sender and receiver identity IDs. Callers constructing replay stores must supply the same identity values used by the authenticated receiver. Existing replay artifacts that lack the new fields are rejected by strict deserialization and must be recreated through an explicitly approved migration or epoch reset; they must not be silently overwritten.

The Phase 80 no-sync outbox path remains benchmark-only. Durable file and directory synchronization remains the default for production-shaped persistence. No raw payloads, keys, signatures, credentials, or fencing tokens are part of the release artifacts.

## Not included and still gated

This release does not include live TLS/mTLS sockets, certificate issuance or rotation, external key-management integration, service discovery, health/readiness endpoints, resource and queue budgets at deployment level, container or Helm rollout artifacts, external cluster tests, production rollback, or promotion approval. These are required before any Phase 81 production rollout.

## References

[1]: PHASE81_AUTHENTICATED_CHANNELS_AND_REPLAY_EPOCHS_REPORT.md "Phase 81 authenticated channel and durable replay report"

[2]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"

[3]: ../src/emission_diagnostic_service_channel.rs "Phase 81 authenticated service-channel implementation"

[4]: ../tests/phase81_service_channel_integration.rs "Phase 81 service-channel integration tests"
