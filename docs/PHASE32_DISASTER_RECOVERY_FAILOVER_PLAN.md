# Phase 32 Multi-Region Disaster Recovery and Automated Consensus Failover

## Objective

Phase 32 introduces a local, deterministic controller for disaster-recovery decisions across regions. It accepts authenticated failure observations from distinct trusted observers, requires an observer quorum, validates a candidate’s snapshot and generation, prepares a higher-term/higher-epoch promotion, fences the previous region, and commits exactly one active region. The controller is a verification harness and state-machine contract; it is not a cloud control-plane mutator.

## Implementation sequence

| Stage | Contract and security behavior | Failure result |
|---|---|---|
| Configuration | Bound cluster ID, quorum size, and failover tick budget. | Reject invalid or unbounded configuration. |
| Region registration | Register unique region IDs with exact snapshot digest, health, active, and fenced state. | Reject duplicates, invalid IDs, or invalid digests. |
| Failure detection | Enter `DetectingFailure` only for the active region and a bounded reason/tick. | Reject non-active targets, control characters, oversized reasons, or excessive ticks. |
| Observation authentication | Verify trusted observer key, cluster, observer identity, region, owner term, ownership epoch, tick, snapshot hash, canonical payload, and Ed25519 signature. | Reject unknown, self, stale, tampered, or misbound evidence before state mutation. |
| Quorum admission | Count distinct observer IDs only; replay of identical evidence is idempotent. | Return `AwaitingQuorum` without changing the active region until required observers are present. |
| Promotion preparation | Require healthy, non-active, non-fenced candidate with equal snapshot hash and strictly higher term and epoch. | Reject stale proposal, snapshot mismatch, or invalid candidate. |
| Promotion commit | Require the exact pending proposal, mark the prior region unhealthy/inactive/fenced, activate the candidate, and advance term/epoch atomically in the controller state. | Reject unprepared, stale, mismatched, or replayed conflicting proposals. |
| Safety report | Verify at most one active region, active region is not fenced, generations are positive, and emit an ordered SHA-256 event trace digest. | Report `safety_passed=false` if an invariant is violated. |

## Phase 32 security gates

The compliance total increases from 82 to 90 with these eight correctness gates:

| Gate | Required evidence |
|---|---|
| `signed_region_failure_observation_required` | Forged, malformed, or unknown-key failure observations fail before mutation. |
| `distinct_observer_quorum_required` | One observer cannot promote; two distinct trusted observers satisfy the configured quorum. |
| `snapshot_hash_binding_required` | Observations and promotion proposals bind to the active and candidate snapshot digest. |
| `higher_term_epoch_promotion_required` | Equal or lower owner term/epoch proposals fail closed. |
| `old_region_fenced_on_commit` | A committed promotion marks the old region inactive, unhealthy, and fenced. |
| `single_active_region_invariant` | The report and integration test prove no more than one active region. |
| `idempotent_failover_evidence` | Identical observations and repeated identical commit requests do not create new authority. |
| `stale_or_conflicting_failover_rejected` | Conflicting observer evidence, self-observation, unknown observers, and stale proposals are rejected. |

## Test and benchmark evidence

The integration suite contains sixteen tests for full quorum promotion, quorum wait without mutation, identical observation idempotence, unknown and wrong-cluster observer rejection, local failure-detection sequencing, conflicting observation rejection, signature tampering, snapshot mismatch, stale term/epoch, unprepared commit rejection, conflicting prepared-promotion rejection, terminal recovery-cycle protection, exact committed-proposal replay identity, self-observation rejection, and committed failover idempotence. The benchmark emits non-secret evidence showing one quorum wait, two distinct observers, promotion to `region-b`, owner term `2`, ownership epoch `2`, old-region fencing through the safety report, eight ordered events, a trace digest, and no private-key persistence.

## Production boundaries

The local controller does not establish failure-detector truth, cloud-region health, DNS or load-balancer convergence, managed-storage replication, process fencing, transport confidentiality, key custody, or real cross-region consensus. A production promotion requires an external failure-detector and membership authority, durable multi-region snapshot storage, mTLS/mesh transport, fencing at the process and routing layers, operator-approved rollback, and staged chaos validation.

## Reproduction

```bash
scripts/validate_phase32_disaster_recovery.sh
cargo run --example phase32_disaster_recovery_benchmark -- --output benchmarks/phase32_disaster_recovery_metrics.json
```

## References

[1]: ../src/disaster_recovery.rs "Phase 32 disaster-recovery controller"
[2]: ../tests/phase32_disaster_recovery_integration.rs "Phase 32 integration tests"
[3]: ../examples/phase32_disaster_recovery_benchmark.rs "Phase 32 benchmark example"
[4]: ../benchmarks/phase32_disaster_recovery_metrics.json "Phase 32 non-secret benchmark artifact"
[5]: ../scripts/validate_phase32_disaster_recovery.sh "Phase 32 validation gate"
