# Phase 34 Replicated Recovery Authority and External Fencing

## Objective

Phase 34 moves disaster-recovery promotion authority from a single durable controller into a bounded replicated authority layer. The implementation introduces joint observer-membership transitions, a hash-bound recovery log, exact proposal commit binding, externally verifiable fencing tokens, and a deterministic four-node chaos harness for dynamic partition and epoch-transition races.

## Contracts

| Contract | Responsibility |
|---|---|
| `ObserverMembership` | Represent stable or joint old/new observer sets, monotonic membership epoch, transition index, quorum calculation, and voter union. |
| `RecoveryAuthorityLogEntry` | Bind a contiguous log index and term to a canonical recovery command hash. |
| `ReplicatedRecoveryAuthority` | Append commands, collect acknowledgements, require stable or joint quorum, apply entries in order, preserve Phase 33 controller semantics, and expose reports. |
| `ExternalFencingToken` | Sign cluster, resource, candidate region, owner term, ownership epoch, membership epoch, fencing epoch, authority identity, and log index with Ed25519. |
| `ExternalFenceState` | Verify token signatures and bindings, reject stale/conflicting epochs, admit only the current token hash and owner, and treat exact replay idempotently. |
| `ReplicatedRecoverySnapshotStore` | Persist the authority log, acknowledgements, membership, controller snapshot, active token, and frontiers through bounded atomic staging. |
| `ReplicatedRecoveryChaosSimulator` | Deliver acknowledgements through deterministic drop, delay, duplicate, heal, and clock schedules and emit sanitized trace evidence. |

## Joint-membership sequencing

A stable authority begins a joint transition by appending old membership, new membership, new observer public keys, and the next membership epoch. The entry cannot commit until acknowledgements contain a majority of the old set **and** a majority of the new set. Only after joint commit can final membership be appended, and finalization requires a majority of the new set. The controller’s trusted observer registry rotates only when the joint entry applies, so old-epoch evidence cannot be admitted after the transition.

## Recovery and fencing sequencing

A recovery commit must reference the exact Phase 33 pending failover proposal and a stable authority membership epoch. The authority appends a `CommitRecovery` entry and issues a token whose `log_index` equals the entry index and whose `fence_epoch` is exactly one greater than the last accepted fence epoch. The entry cannot apply without the authority quorum. Applying the entry first commits the controller’s fencing transition; the external verifier then accepts the signed token as the current resource owner. Process, socket, routing, and load-balancer enforcement remain outside this local token verifier.

## Phase 34 gates

| Gate | Required evidence |
|---|---|
| `joint_observer_quorum_required` | One old/new side alone cannot commit a joint transition. |
| `joint_to_final_membership_ordering` | Finalization is rejected before joint commit and requires a new-set majority. |
| `replicated_recovery_log_hash_bound` | Entry hashes, indices, terms, and commit/applied frontiers validate on restore. |
| `external_fencing_token_signature_required` | A signed token binds all authority and resource fields and verifies before admission. |
| `fencing_epoch_monotonicity` | Fence epochs increase exactly once per committed recovery. |
| `stale_external_fence_rejected` | Stale and same-epoch conflicting tokens fail closed; exact replay is idempotent. |
| `replicated_authority_restart_continuity` | Membership, log, commit frontier, active token, and controller trace survive restart. |
| `dynamic_partition_epoch_chaos_safe` | Four-node drop, delay, duplicate, healing, stale epoch, and stale fence schedules preserve one safe authority. |

## Evidence

The integration suite contains six focused tests covering joint quorum exclusion, finalization ordering, signed external fencing, replicated restart continuity, dynamic partition chaos, and token rollback rejection. The benchmark emits sanitized non-secret outcomes only: quorum and finalization indices, membership epoch, token hash presence, fence epoch, owner region, restart booleans, chaos counts, and trace digest.

## Boundaries

This phase does not provide distributed transport, multi-process locking, externally governed observer membership, failure-detector truth, durable remote storage, cloud-region failover, process fencing, socket admission, or routing convergence. The chaos simulator is deterministic local evidence for authority sequencing and partition handling, not production network proof.

## Reproduction

```bash
cargo test --test phase34_replicated_recovery_integration -- --nocapture
cargo run --example phase34_replicated_recovery_benchmark -- --output benchmarks/phase34_replicated_recovery_metrics.json
```

## References

[1]: ../src/replicated_recovery.rs "Phase 34 replicated recovery authority and chaos harness"
[2]: ../tests/phase34_replicated_recovery_integration.rs "Phase 34 integration tests"
[3]: ../examples/phase34_replicated_recovery_benchmark.rs "Phase 34 sanitized benchmark"
[4]: ../docs/PHASE34_REPLICATED_RECOVERY_AUDIT_NOTES.md "Phase 34 audit notes"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus and replication architecture evidence"
