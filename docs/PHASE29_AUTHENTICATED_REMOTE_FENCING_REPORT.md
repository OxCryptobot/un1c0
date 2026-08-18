# Phase 29 Authenticated Remote Queue-Fence Observations

## Summary

Phase 29 extends local partition fencing with an authenticated evidence-ingestion path. A trusted peer can send a signed `ConsensusMessage::QueueOwnershipFence` observation. The current owner verifies the cluster, sender identity, Ed25519 signature, replay epoch, message term, active owner term, ownership epoch, and acknowledgement quorum threshold before persisting the fence.

This is an authenticated observation channel, not a complete distributed failure detector. A valid signature proves which trusted peer made the claim; it does not prove that the peer’s network sample is globally correct. Failure-detector quorum aggregation, observer membership policy, lease-clock authority, and split-brain prevention remain explicit deployment boundaries.

## Contract and behavior

| Condition | Result |
|---|---|
| Trusted peer signs a fence bound to the active owner lease | Fence is persisted and delivery returns `OwnershipFenced`. |
| Same fence hash is received again | No state mutation; the existing fenced action is returned. |
| Newer observation tick is received | Newer fence replaces the older observation and is persisted. |
| Older observation tick is received | Existing newer fence remains authoritative. |
| Different fence hash at the same tick | Rejected as a conflicting observation without mutation. |
| Owner term, epoch, or quorum threshold mismatches | Rejected before durable state mutation. |
| Signature, sender, cluster, replay epoch, or message term is invalid | Rejected before durable state mutation. |

The receiver requires the fence owner to be the local transport node. It then loads the queue ownership lease and requires exact owner-term, ownership-epoch, and quorum-threshold equality. Persistence failure restores the previous in-memory fence state.

## Integration evidence

The Phase 28 integration file now contains six tests, including two Phase 29 scenarios:

| Test | Evidence |
|---|---|
| `authenticated_remote_fence_observation_is_idempotent_and_blocks_delivery` | A signed peer observation is accepted, the state hash remains unchanged on duplicate application, and delivery remains fenced before socket write. |
| `tampered_or_misbinding_remote_fence_fails_without_mutation` | A modified signature and an owner-misbinding report are rejected while the queue state hash and fence map remain unchanged. |
| `quorum_loss_fences_delivery_before_socket_write` | Local quorum-loss fencing retains the queue and prevents delivery. |
| `lease_expiry_fences_delivery_without_mutating_queue` | Lease expiry fails closed without queue mutation. |
| `ownership_fence_survives_restart_and_blocks_acknowledgement` | Durable fence state survives restart and blocks replicated acknowledgement. |
| `ownership_transfer_clears_fence_and_allows_new_owner_retry` | Valid higher-term/higher-epoch transfer clears the fence; new-owner retry remains quorum-gated. |

## Security boundary

The message is authenticated through the existing consensus envelope, but the code does not yet aggregate multiple observer reports into a quorum-certified failure-detector decision. Nor does it prove that an old owner has been physically stopped. Production promotion still requires authenticated observer membership, quorum evidence aggregation, monotonic lease clocks, lease renewal, process fencing, and cross-host partition tests.

## References

[1]: ../src/consensus.rs "Phase 29 authenticated remote fence implementation"
[2]: ../tests/phase28_partition_ownership_fencing_integration.rs "Phase 28 and Phase 29 integration evidence"
[3]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture and production boundary"
[4]: ../docs/PHASE28_PARTITION_OWNERSHIP_FENCING_REPORT.md "Phase 28 local fencing report"
