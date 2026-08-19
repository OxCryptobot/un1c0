# Phase 36 Authenticated Recovery Transport Report

## Executive summary

Phase 36 moves the Phase 35 multi-leader witness model across a typed process boundary. `AuthenticatedTransportEnvelope` signs sender and receiver identity, cluster/resource, connection epoch, sequence, nonce, message kind, payload digest, and public key. The receiver verifies the envelope before changing replay state, rejects stale connection epochs, and treats exact duplicate delivery as idempotent.

Witness vote reservations are now durable, hash-bound, bounded, staged, fsynced, atomically renamed, and directory-synced. Deterministic fault injection covers failures before stage, after stage, and after sync before rename. Restart cleanup removes stale staging and preserves the previous valid snapshot. `ProtectedWriteGateway` adds a typed write-admission boundary that requires the exact trusted external fencing token before accepting a protected operation.

## Evidence

| Evidence | Result |
|---|---:|
| Phase 36 integration tests | 6 passed |
| Authenticated envelope signature and payload binding | Passed |
| Receiver, cluster, and resource binding | Passed |
| Duplicate and stale-sequence replay behavior | Passed |
| Connection-epoch transition and stale-epoch rejection | Passed |
| Durable reservation exact replay and conflict rejection | Passed |
| Pre-stage, post-stage, and post-sync crash recovery | Passed |
| Protected-write exact-fence admission | Passed |
| Protected-write operation replay idempotence | Passed |
| Cross-host drop and duplicate chaos | Passed |

The deterministic benchmark reports signed receiver-bound transport, one dropped envelope, one duplicate delivery, a durable reservation replay, an injected crash with recovery, one accepted protected operation, exact operation replay, active owner `region-b`, accepted fence epoch `1`, and `safety_passed=true`. No raw public keys, signatures, private keys, or full fencing tokens are emitted.

## Security analysis

| Control | Phase 36 behavior | Residual boundary |
|---|---|---|
| Envelope authentication | Ed25519 signs a fixed transport domain and all routing/replay/payload fields. | Real network confidentiality and certificate rotation are external. |
| Replay protection | Sender sequence and connection epoch are verified before dispatch; exact duplicate is idempotent. | Multi-host replay-window replication is not yet implemented. |
| Durable reservation | Reservation state is hash-bound and atomically cut over with fsync and restart cleanup. | Distributed filesystem guarantees and independent witness stores are external. |
| Gateway fencing | Every protected write requires trusted authority, exact resource/owner binding, and accepted fence state. | Only the typed local gateway is enforced; production handlers must integrate it. |
| Crash ordering | Faults at pre-stage, post-stage, and post-sync boundaries preserve old or new valid state. | OS power-loss and hardware storage behavior are not simulated. |
| Cross-host chaos | Directed drop and duplicate delivery are deterministic and replayable. | Kernel scheduling, TCP behavior, TLS, and real process crashes remain unmodeled. |

## Failure modes addressed

A forged envelope fails before payload dispatch because the sender key, canonical signature, and payload hash are checked. A valid envelope for another receiver fails direction binding. A delayed envelope from an old connection epoch fails replay admission after restart. A witness cannot lose its vote reservation merely because a process restarts; the hash-bound reservation is restored before new reservations are accepted. A protected operation cannot bypass the external fence state, and an operation ID cannot be rebound to a different payload.

## Recommended next phase

The next high-value slice is authenticated multi-process integration with independent witness stores and a real loopback transport. It should add process-level crash/restart orchestration, persistent replay windows, gateway middleware integration, and cross-host reservation replication while preserving the local fail-closed contracts.

## References

[1]: ../src/recovery_transport.rs "Phase 36 authenticated transport and persistence implementation"
[2]: ../tests/phase36_recovery_transport_integration.rs "Phase 36 integration tests"
[3]: ../examples/phase36_recovery_transport_benchmark.rs "Phase 36 sanitized benchmark"
[4]: ../docs/PHASE36_AUTHENTICATED_TRANSPORT_PLAN.md "Phase 36 implementation plan"
[5]: ../docs/PHASE36_AUTHENTICATED_TRANSPORT_AUDIT_NOTES.md "Phase 36 baseline audit notes"
[6]: ../src/replicated_recovery.rs "Phase 34 external fencing state"
