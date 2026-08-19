# Phase 34 Replicated Recovery Authority Audit Notes

## Baseline

The repository head is Phase 33 commit `94a7102`, with a clean published baseline and passing Phase 30–33 recovery regressions: 9 Phase 30 tests, 8 Phase 31 tests, 16 Phase 32 tests, and 7 Phase 33 tests. Phase 33 already provides a durable local disaster-recovery controller, hash-bound restart snapshots, monotonic observer-membership epochs, and a deterministic single-controller partition-race arbiter.

## Phase 34 objective

Add a typed replicated recovery-authority layer rather than allowing a local controller snapshot to act as distributed authority. The layer must commit membership transitions and recovery promotions through a bounded, hash-chained-by-entry recovery log with explicit acknowledgements. Joint membership requires a majority of both the old and new observer sets before the transition can commit; final membership requires a majority of the new set after joint commit.

Every committed recovery promotion issues an Ed25519-signed `ExternalFencingToken` bound to cluster, resource, candidate region, owner term, ownership epoch, observer membership epoch, fencing epoch, authority identity, and recovery-log index. An external verifier accepts only a strictly newer fencing epoch, treats same-token replay as idempotent, and rejects stale or conflicting tokens. The token proves authority evidence; it does not itself enforce process, socket, routing, or load-balancer fencing.

## Chaos invariants

| Invariant | Required evidence |
|---|---|
| Joint quorum safety | A joint transition requires a majority of both old and new memberships; one side alone cannot commit. |
| Finalization ordering | Final membership cannot commit before the joint entry commits. |
| Epoch monotonicity | Membership epochs and fencing epochs never decrease; stale observations and tokens fail closed. |
| Log integrity | Entry index, term, command hash, snapshot hash, and commit/applied frontiers are validated before restore. |
| Recovery binding | The committed recovery entry must reference the exact Phase 33 pending proposal and issue a token for the same candidate and log index. |
| External token safety | A verifier admits only the current token hash and owner; same-token replay is idempotent and same-epoch conflicts are rejected. |
| Dynamic partition safety | Drop, delay, duplicate, reorder, and heal schedules are deterministic; no partitioned minority can create a second committed authority. |
| Restart continuity | A replicated-authority snapshot restores joint phase, log acknowledgements, commit frontier, active token, and controller state without private-key persistence. |

## Boundaries

The Phase 34 local implementation does not claim distributed transport, multi-process locking, external observer-registry governance, process fencing, socket admission, routing convergence, cloud-region durability, or failure-detector truth. The chaos harness models delivery faults and acknowledgement quorum behavior deterministically; it is evidence for the authority state machine, not proof of a production network deployment.

## Proposed gates

Phase 34 adds eight evidence gates: `joint_observer_quorum_required`, `joint_to_final_membership_ordering`, `replicated_recovery_log_hash_bound`, `external_fencing_token_signature_required`, `fencing_epoch_monotonicity`, `stale_external_fence_rejected`, `replicated_authority_restart_continuity`, and `dynamic_partition_epoch_chaos_safe`.
