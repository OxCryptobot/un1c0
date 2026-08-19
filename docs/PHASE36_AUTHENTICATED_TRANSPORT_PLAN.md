# Phase 36 Authenticated Recovery Transport and Durable Witness Reservations

## Objective

Phase 36 closes the most important Phase 35 production-boundary gap: proposals and witness votes must survive process boundaries, replay protection must survive connection restarts, and protected writes must be rejected unless the exact accepted external fencing token is present.

## Contracts

| Contract | Responsibility |
|---|---|
| `AuthenticatedTransportEnvelope` | Sign domain, version, cluster, resource, sender, receiver, connection epoch, sequence, nonce, message kind, payload hash, and signer key. |
| `TransportKeyRegistry` | Pin process/leader IDs to Ed25519 public keys and reject implicit rebinding. |
| `AuthenticatedTransportReceiver` | Verify identity/direction/payload before dispatch and apply the connection-epoch replay window. |
| `TransportReplayWindow` | Reject stale epochs and sequences while treating exact duplicate envelopes idempotently. |
| `WitnessVoteReservation` | Hash-bind witness, round, proposal digest, membership epoch, and connection epoch. |
| `WitnessReservationStore` | Persist reservations with staged JSON, fsync, atomic rename, directory sync, hash validation, restart cleanup, and injected crash boundaries. |
| `ProtectedWriteGateway` | Require registry-backed exact external fencing admission before accepting a resource operation and make exact operation replay idempotent. |
| `TransportChaosHarness` | Exercise directed drop, delay, duplicate, heal, and replay behavior across host identities. |

## Ordering invariants

The receiver verifies the envelope’s signature, expected receiver, cluster/resource binding, payload hash, sender registry key, and protocol domain before applying replay-window state. Connection epochs are monotonic; an epoch transition clears the previous sequence window, while delayed envelopes from an older epoch are rejected.

Witness reservations are durable before they are considered accepted. A crash before stage leaves no new authority; a crash after stage or after sync leaves only temporary evidence that restart cleanup removes before loading the prior valid snapshot. Exact reservation replay is idempotent, while a different proposal digest for the same witness, round, and membership epoch is rejected.

The protected-write gateway validates operation ID, resource, owner region, payload hash, trusted authority ID, and exact external token before adding the operation to its accepted set. Same operation/request replay returns `AlreadyAccepted`; the same operation ID with a different request is rejected without mutation.

## Phase 36 gates

| Gate | Evidence |
|---|---|
| `authenticated_transport_envelope_signature_required` | Signed envelope and trusted-key verification pass. |
| `transport_receiver_binding_required` | Wrong receiver and cluster/resource envelopes fail closed. |
| `connection_epoch_replay_window_enforced` | Duplicate, stale sequence, and epoch transition behavior pass. |
| `durable_witness_reservation_hash_bound` | Reservation hash, bounded state, and exact replay pass. |
| `reservation_crash_cutover_atomic` | Pre-stage, post-stage, and post-sync faults preserve valid state and clean staging. |
| `protected_write_exact_fence_required` | Gateway requires the exact registry-backed fencing token. |
| `cross_host_chaos_duplicate_idempotent` | Directed drop and duplicate host delivery remain deterministic and idempotent. |
| `stale_transport_replay_rejected` | Old connection epochs cannot dispatch after restart. |

## Validation

```bash
cargo test --test phase36_recovery_transport_integration -- --nocapture
cargo run --example phase36_recovery_transport_benchmark -- --output benchmarks/phase36_recovery_transport_metrics.json
```

## Production boundaries

The local implementation does not claim kernel TLS, mTLS certificate lifecycle, OS process isolation, distributed filesystem semantics, cross-machine clock authority, hardware fencing, or a real write gateway outside the typed adapter. These remain deployment gates for a production rollout.

## References

[1]: ../src/recovery_transport.rs "Phase 36 authenticated recovery transport"
[2]: ../tests/phase36_recovery_transport_integration.rs "Phase 36 transport and durability integration suite"
[3]: ../examples/phase36_recovery_transport_benchmark.rs "Phase 36 sanitized benchmark"
[4]: ../docs/PHASE36_AUTHENTICATED_TRANSPORT_AUDIT_NOTES.md "Phase 36 audit notes"
[5]: ../src/multileader_recovery.rs "Phase 35 multi-leader authority"
