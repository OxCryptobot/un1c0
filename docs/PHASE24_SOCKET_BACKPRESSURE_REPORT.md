# Phase 24 Socket-Layer Backpressure and Per-Peer Quotas

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 24 extends authenticated consensus transport with typed socket-layer capacity controls. `SocketQuotaConfig` bounds per-peer in-flight send bytes, receive-window bytes, window duration, and retry backoff. `SocketPeerQuota` maintains isolated counters per trusted peer. `SocketBackpressureAction`, `SocketReceiveAction`, and `SocketTransportMetrics` expose deterministic admission, retry, release, and observation semantics without owning threads, schedulers, durable queues, or cross-process state.

The quota-aware path calculates exact serialized frame bytes before mutation. Send admission returns an exact retry tick when a peer is saturated and releases bytes after either successful or failed socket writes. Receive admission verifies sender identity, cluster, term, signature, replay epoch, and duplicate nonce before changing receive quota state. Monotonic replay-epoch rotation rebuilds both replay windows and quota maps.

The complete compliance artifact increases from **44 to 48 passing gates**. All Phase 24 integration tests, Rust targets, prior regressions, Helm checks, Compose mTLS smoke tests, and the independent metrics audit pass.

## Contract summary

| Contract | Safety behavior |
|---|---|
| `SocketQuotaConfig` | Rejects zero, oversized, or unbounded quota/window values. |
| `SocketPeerQuota` | Separates send in-flight bytes and receive-window bytes per trusted peer. |
| `SocketBackpressureAction` | Returns `Admitted` or `Backpressured` with frame bytes, available bytes, and retry ticks. |
| `SocketReceiveAction` | Returns an admitted envelope or a typed receive backpressure result. |
| `SocketTransportMetrics` | Exposes bounded non-secret counters and active quota state. |
| `send_to_peer_with_backpressure` | Authenticates, serializes, admits exact bytes, writes, and releases bytes. |
| `receive_with_backpressure` | Bounds the frame, verifies/authenticates/replay-checks, then admits receive bytes. |

## Security ordering

Receive-side quota mutation intentionally occurs after frame-length validation, deserialization, trusted-key lookup, cluster and replay-epoch verification, term-floor verification, signature verification, and duplicate-nonce detection. Invalid or replayed frames therefore do not consume a peer’s receive budget. Oversized and unknown-peer paths fail closed without creating quota state.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `per_peer_send_quotas_are_isolated_and_release_exactly` | Per-peer isolation, exact saturation, retry boundary, and release | Passed |
| `receive_window_backpressure_has_exact_retry_and_tick_reset` | Receive-window backpressure and tick reset | Passed |
| `authentication_and_oversized_rejection_do_not_consume_quota` | Security ordering and no-mutation rejection | Passed |
| `quota_aware_wire_send_releases_bytes_and_epoch_rotation_clears_state` | Real wire send, release, and epoch reset | Passed |

## Compatibility and boundaries

The existing `send`, `receive`, and `listen_once` methods remain available for legacy callers. New production callers should use the explicit quota-aware methods and connect typed actions to a real socket queue and retry scheduler. The local quota map is neither durable nor distributed. Production promotion still requires socket-thread queue ownership, cross-process quota replication, durable retry intent, metrics export, authenticated peer scheduling, and network-stress testing.

## References

[1]: ../src/consensus.rs "Phase 24 socket transport implementation"
[2]: ../tests/phase24_socket_backpressure_integration.rs "Phase 24 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Current security compliance metrics"
[4]: ../benchmarks/security_compliance_audit.json "Machine-readable security metrics audit"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture and boundaries"
