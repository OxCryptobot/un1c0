# Phase 28 Partition-Aware Queue Ownership Fencing

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 27 established authenticated queue ownership leases and cross-host ownership transfer, but its local delivery path did not have a durable, typed response to quorum loss or lease expiry before a socket write. Phase 28 closes that execution-boundary gap without claiming to implement a network failure detector. It adds a hash-bound `QueueOwnershipFence`, persists the fence with the durable queue state, returns an `OwnershipFenced` action before frame deserialization or socket write, and rejects acknowledgement commits while the fence is active.

A fence is sticky. Network recovery alone does not clear it. Only a validated higher-term and higher-epoch ownership transfer clears the fence atomically with stale acknowledgement evidence and active-delivery state. The new owner may retry the retained FIFO frame, but it remains pending until its configured authenticated acknowledgement quorum is satisfied.

## Partition and failover behavior

| Situation | Local behavior | Safety result |
|---|---|---|
| Owner reports reachable members below quorum | Persist a fence bound to peer, owner, term, epoch, tick, counts, and reason. | Future delivery returns `OwnershipFenced` without mutating the queue. |
| Lease reaches expiry | Delivery returns `OwnershipFenced` with bounded retry timing. | No new socket write is attempted under an expired lease. |
| Process restarts while fenced | Durable restore validates and restores the fence. | The restarted owner remains fail-closed. |
| New owner receives valid higher-term transfer | Replace ownership, clear fence/acks/active delivery, persist atomically. | Failover can retry retained work without inheriting stale authority. |
| New owner retries during a partition | Authenticated delivery can write, but post-flush removal still waits for the configured quorum. | Retry does not become a single-node commit. |
| Network connectivity returns | No automatic un-fence occurs. | A coordinator must provide a validated transfer or explicit future lease protocol. |

## Code contracts

`QueueOwnershipFence` validates bounded member counts, positive owner term and epoch, a non-empty control-character-free reason, and a canonical SHA-256 content hash. Durable queue state includes an optional fence map and hashes it for all new snapshots. Pre-Phase-28 snapshots with the legacy state hash remain loadable when they contain no fence evidence; new snapshots use the expanded hash.

`record_ownership_quorum_loss` is intentionally an execution-kernel seam. It requires the local node to own the queue and requires `reachable_members < ack_quorum_size`. The caller remains responsible for obtaining an authenticated quorum-loss observation. `deliver_next_durable_frame` checks fences, current ownership, and finite lease expiry before reading the queued envelope. `record_authenticated_delivery_ack` and `acknowledge_durable_frame` reject active fences. Ownership transfer removes the fence before persistence and restores it if persistence fails.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `quorum_loss_fences_delivery_before_socket_write` | Quorum loss creates a durable fence and prevents delivery mutation | Passed |
| `lease_expiry_fences_delivery_without_mutating_queue` | Finite lease expiry blocks delivery and retains the queue | Passed |
| `ownership_fence_survives_restart_and_blocks_acknowledgement` | Fence restore and acknowledgement rejection after restart | Passed |
| `ownership_transfer_clears_fence_and_allows_new_owner_retry` | Higher-term transfer clears fence; new owner retries but waits for quorum | Passed |

## Production boundary

The implementation does not infer quorum loss from a socket timeout, synchronize lease clocks across machines, renew leases, elect an owner, or prevent a real split brain. A production deployment must supply authenticated failure-detector evidence, monotonic lease authority, quorum-backed transfer intent, network-partition tests, and process-level fencing. The local contract is deliberately fail-closed when those external authorities report unsafe conditions.

## References

[1]: ../src/consensus.rs "Phase 28 ownership-fencing implementation"
[2]: ../tests/phase28_partition_ownership_fencing_integration.rs "Phase 28 integration tests"
[3]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture and production boundary"
[4]: ../docs/PHASE27_REPLICATED_DELIVERY_OWNERSHIP_REPORT.md "Phase 27 ownership and replicated acknowledgement report"
