# Phase 33 Durable Disaster Recovery and Observer-Membership Epochs

## Objective

Phase 33 extends the local disaster-recovery controller with canonical durable snapshots, atomic restart recovery, monotonic observer-membership epochs, and deterministic concurrent partition-race evidence. The implementation preserves the Phase 32 fail-closed promotion order and does not claim cloud-region, process-fencing, routing, storage, transport, or failure-detector authority.

## Implementation sequence

| Stage | Contract | Failure result |
|---|---|---|
| Snapshot DTO | Serialize controller state, trusted public keys, observer evidence, pending/committed proposal identity, event trace, and state hash | Reject malformed, oversized, or hash-mismatched state |
| Atomic persistence | Write bounded JSON to sibling staging file, `sync_all`, rename atomically, and best-effort sync the parent directory | Remove staging on failure; preserve the previous committed file |
| Restart restore | Validate every binding before assigning restored state to a controller | Existing controller remains unchanged on restore failure |
| Membership epoch | Include observer-membership epoch in canonical signed observations and controller reports | Reject stale epoch before observer-map mutation |
| Membership rotation | Require a strictly higher epoch, validate all observer IDs and Ed25519 public keys, clear old evidence and pending authority | Reject non-monotonic rotation, undersized membership, or rotation during prepared/committed recovery |
| Partition race | Clone a deterministic simulator base state into competing branches and feed candidate actions to one controller arbiter | Branch divergence cannot create two commits; arbiter accepts one exact proposal |

## Phase 33 gates

The compliance total increases from 90 to 97 with these seven correctness gates:

| Gate | Required evidence |
|---|---|
| `durable_recovery_snapshot_hash_bound` | Canonical snapshot state hash is verified before restore. |
| `atomic_recovery_cutover` | Staging write, sync, atomic rename, and no-mutation-on-failure behavior are tested. |
| `partial_staging_cleanup` | Interrupted staging is removed before restart. |
| `restart_preserves_pending_authority` | Pending or committed proposal identity survives save/load and resumes safely. |
| `observer_membership_epoch_bound` | Signed observations include and verify the active membership epoch. |
| `stale_membership_evidence_rejected` | Old-epoch evidence fails before observer-map mutation. |
| `concurrent_partition_race_single_commit` | Deterministic competing branches are resolved by one arbiter with one active region. |

## Test and benchmark evidence

The Phase 33 integration suite contains seven tests for pending-authority restart, committed replay continuity, snapshot tampering, partial staging cleanup, old-epoch rejection, membership-rotation reset, and concurrent partition races. The benchmark emits only non-secret outcomes: snapshot hash binding, committed-proposal continuity, membership epoch, branch safety, branch owner divergence, arbiter-selected active region, quorum counts, and trace digests.

## Production boundaries

The local store does not provide multi-process locking, distributed compare-and-swap, cross-host observer registry authority, failure-detector truth, process or routing fencing, cloud-region durability, DNS convergence, cross-region snapshot replication, or private-key custody. A production implementation requires an externally governed membership transition, durable replicated recovery log, fencing tokens enforced by service admission and routing, and staged chaos validation across independent machines and regions.

## Reproduction

```bash
cargo test --test phase33_durable_recovery_integration -- --nocapture
cargo run --example phase33_durable_recovery_benchmark -- --output benchmarks/phase33_durable_recovery_metrics.json
```

## References

[1]: ../src/disaster_recovery.rs "Phase 33 durable recovery controller, snapshot DTO, store, and membership epochs"
[2]: ../tests/phase33_durable_recovery_integration.rs "Phase 33 durable recovery and partition-race integration tests"
[3]: ../examples/phase33_durable_recovery_benchmark.rs "Phase 33 non-secret benchmark example"
[4]: ../benchmarks/phase33_durable_recovery_metrics.json "Phase 33 benchmark artifact"
[5]: ../docs/PHASE32_QUORUM_SAFETY_AUDIT_REPORT.md "Phase 32 quorum-safety audit and prioritized next phase"
