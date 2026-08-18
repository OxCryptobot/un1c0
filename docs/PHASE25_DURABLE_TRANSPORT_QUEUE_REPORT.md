# Phase 25 Durable Transport Queues and Quota Recovery

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 25 extends Phase 24 socket backpressure with durable per-peer transport queues. `DurableSocketQueueFrame` stores exact serialized frame bytes, a positive FIFO sequence, peer binding, and a SHA-256 digest. `DurableSocketQueueState` binds cluster ID, node ID, replay epoch, quota configuration, peer quota counters, queue sequences, and queue contents to a canonical state hash. `DurableSocketQueueStore` persists this state with bounded JSON, create-new staging, file synchronization, atomic rename, directory synchronization, and explicit partial-staging cleanup.

Durable enqueue admits exact frame bytes against the existing per-peer quota, appends a FIFO frame, advances its sequence, and persists before returning. If persistence fails, the in-memory frame, sequence, and in-flight quota bytes are rolled back. Restart restore validates identity, trusted membership, replay epoch, state hash, frame digests, queue order, quota-byte equality, and global bounds. Acknowledgement is FIFO-only and also rolls back in-memory removal if durable persistence fails.

The compliance artifact increases from **48 to 52 passing gates**. The Phase 24 socket metrics remain represented through per-peer in-flight bytes, receive-window bytes, durable queue depth, durable queue bytes, next queue sequence, admission counts, and backpressure counters.

## Contract summary

| Contract | Safety behavior |
|---|---|
| `DurableSocketQueueFrame` | Binds peer, sequence, exact bytes, and SHA-256 digest under frame bounds. |
| `DurableSocketQueueState` | Binds queue and quota maps to identity, replay epoch, canonical hash, and exact byte totals. |
| `DurableSocketQueueStore` | Uses staged fsync/atomic rename and removes partial staging on recovery. |
| `enqueue_durable_frame_with_backpressure` | Authenticates, serializes, admits, appends, persists, and rolls back on failure. |
| `restore_durable_queue_from_store` | Rejects identity, membership, epoch, hash, digest, ordering, and quota mismatches. |
| `acknowledge_durable_frame` | Removes only the queue head and restores state on persistence failure. |
| `SocketTransportMetrics` | Reports active quota plus durable queue depth, bytes, and next sequence. |

## Restart and rollback invariants

A queue snapshot is usable after restart only when its cluster ID, node ID, peer membership, quota configuration, replay epoch, state hash, frame hashes, sequence order, and in-flight byte totals validate. A state from another replay epoch is rejected rather than silently cleared or merged. Partial staging files are deleted explicitly. Enqueue and acknowledgement both preserve in-memory state when the durable write fails.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `durable_queue_round_trip_recovers_bytes_and_quota_after_restart` | Durable enqueue, metrics persistence, restart restore, FIFO acknowledgement | Passed |
| `durable_queue_rejects_tampering_and_cleans_partial_staging` | Frame digest rejection and staging cleanup | Passed |
| `restart_rejects_epoch_mismatch_without_mutating_transport` | Replay-epoch binding and no-mutation rejection | Passed |
| `durable_queue_backpressures_and_rolls_back_when_store_fails` | Exact quota retry boundary and persistence rollback | Passed |

## Production boundaries

The local durable store does not claim cross-host replication, queue-thread ownership, delivery scheduling, durable retry execution, or distributed quota authority. Production promotion still requires authenticated queue delivery, replicated queue snapshots, process-supervised retry scheduling, crash-injection at the socket boundary, cross-host replay/epoch coordination, and metrics export that excludes payload bytes and secrets.

## References

[1]: ../src/consensus.rs "Phase 25 durable socket queue implementation"
[2]: ../tests/phase25_durable_transport_queue_integration.rs "Phase 25 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Current security metrics artifact"
[4]: ../benchmarks/security_compliance_audit.json "Independent security metrics audit"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture"
