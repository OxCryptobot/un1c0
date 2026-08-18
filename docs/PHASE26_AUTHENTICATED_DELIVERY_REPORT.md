# Phase 26 Authenticated Durable Queue Delivery

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 26 connects durable Phase 25 queue frames to the authenticated socket boundary. Before any write, the transport deserializes the persisted bytes and re-verifies the authenticated envelope against the local trusted key, cluster ID, replay epoch, and term floor. A peer can have only one active delivery sequence. The queue frame remains durable until the length prefix, payload, and flush all complete successfully; only then is the FIFO head acknowledged and removed.

The delivery API exposes `Idle`, `Backpressured`, `Delivered`, and `CrashInjected` outcomes. Deterministic crash points cover the interval before the length prefix, after the prefix, after payload write, and after flush. Every injected crash clears transient active-delivery state but retains the durable frame and quota bytes, allowing a restarted transport to retry the same sequence. Tampered queued payloads fail closed before any socket write.

The compliance artifact increases from **52 to 56 passing gates**. The Phase 25 review remains independently represented through all prior gate and evidence checks.

## Delivery state machine

| Stage | Allowed effect | Recovery behavior |
|---|---|---|
| `Idle` | No queue frame exists for the peer. | No socket access or queue mutation. |
| `Active` | One peer sequence is marked transiently active; attempt counter increments. | Active marker is cleared on error or process restart. |
| Before prefix | No bytes written. | Queue and quota remain unchanged. |
| After prefix | Partial frame may exist on the socket. | Queue remains; retry sends the complete frame. |
| After payload | Full bytes may exist without flush confirmation. | Queue remains; retry is required. |
| After flush | Delivery is complete but not yet acknowledged. | Acknowledgement removes only the FIFO head. |
| `Delivered` | Durable acknowledgement succeeds. | Queue bytes and in-flight quota decrease. |

## Security invariants

Persisted frame digests are not sufficient authorization. The implementation re-deserializes and re-verifies the envelope before sending, so a frame whose bytes are altered and rehashed with an untrusted signature still fails closed. Delivery attempts never log payload bytes or signing material. Authentication, replay epoch, term floor, and trusted identity remain authoritative.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `authenticated_delivery_flushes_before_fifo_ack_and_remote_receive_verifies` | Authenticated wire send, receiver verification, post-flush removal | Passed |
| `every_socket_crash_boundary_retains_queue_for_authenticated_retry` | Four crash points, restart restore, same-sequence retry | Passed |
| `tampered_authenticated_payload_fails_before_delivery_and_preserves_queue` | Untrusted persisted payload rejection before write | Passed |
| `delivery_state_does_not_claim_persistent_socket_thread_ownership` | Empty queue behavior and explicit deployment boundary | Passed |

## Production boundaries

The local implementation provides deterministic fault injection and crash-safe queue semantics but does not claim to supervise real processes, control kernel socket buffers, replicate queues across hosts, or schedule durable retries. Production promotion requires process-level crash injection, authenticated queue replication, durable retry scheduling, network partition tests, and operational metrics export.

## References

[1]: ../src/consensus.rs "Phase 26 authenticated durable delivery implementation"
[2]: ../tests/phase26_authenticated_durable_delivery_integration.rs "Phase 26 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Current security metrics artifact"
[4]: ../benchmarks/security_compliance_audit.json "Independent metrics audit"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture"
