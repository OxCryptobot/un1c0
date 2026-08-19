# Phase 32 Disaster Recovery and Automated Consensus Failover Report

## Summary

Phase 32 adds a deterministic `DisasterRecoveryController` for multi-region disaster-recovery decisions. It authenticates signed region-failure observations, requires distinct observer quorum, binds evidence and promotion to the active snapshot digest, requires strictly higher owner term and ownership epoch, fences the previous region, and reports a single-active-region safety invariant.

The implementation is intentionally transport-agnostic and local. It does not mutate cloud resources or claim that a local observation is equivalent to a production failure detector.

## State machine

| Phase | Meaning |
|---|---|
| `Stable` | The configured active region is registered and no failure workflow is open. |
| `DetectingFailure` | The active region has a bounded failure observation and fewer than the required distinct observers. |
| `AwaitingObserverQuorum` | The controller has retained failure evidence but will not promote until observer quorum is met. |
| `PromotionPrepared` | A healthy candidate with matching snapshot and higher term/epoch has a pending proposal. |
| `Committed` | The candidate is active, the previous region is inactive/unhealthy/fenced, and the generation has advanced. |

## Security sequencing

`RegionFailureObservation::verify` validates shape, public-key and signature bounds, canonical payload construction, trusted-key equality, cluster identity, observer identity, and Ed25519 signature. `ingest_failure_observation` then binds the observation to the active region, active term, ownership epoch, and snapshot hash, rejects self-observation, and handles identical replays idempotently while rejecting conflicts from the same observer.

`prepare_promotion` requires distinct observer count to meet quorum. It rejects an unknown, unhealthy, fenced, or already-active candidate; requires the candidate snapshot to equal both active and candidate state; and requires both owner term and ownership epoch to increase. `commit_promotion` requires exact pending-proposal equality, marks the previous region inactive, unhealthy, and fenced, activates the candidate, advances the generation, and records an ordered event. Repeated identical commit is idempotent and conflicting proposals fail closed.

## Evidence

The nine-test integration suite covers observer quorum commit, quorum wait without active-region mutation, identical observation idempotence, conflicting evidence rejection, signature tampering, snapshot mismatch, stale term/epoch, self-observation rejection, and committed failover idempotence. The benchmark reports two distinct observers, one quorum wait, active region `region-b`, term `2`, epoch `2`, eight ordered events, a trace digest, safety passed, and no private-key persistence.

The eight new compliance gates are `signed_region_failure_observation_required`, `distinct_observer_quorum_required`, `snapshot_hash_binding_required`, `higher_term_epoch_promotion_required`, `old_region_fenced_on_commit`, `single_active_region_invariant`, `idempotent_failover_evidence`, and `stale_or_conflicting_failover_rejected`. The expected compliance total becomes 90 gates.

## Production boundaries

Production still needs an independently governed failure detector, observer membership and key registry, durable cross-region snapshots, mTLS or equivalent authenticated transport, routing and DNS convergence controls, process-level fencing, rollback and operator approval, managed-storage consistency, and real multi-region chaos validation. The local controller proves sequencing and fail-closed state transitions, not those external authorities.

## References

[1]: ../src/disaster_recovery.rs "Phase 32 controller implementation"
[2]: ../tests/phase32_disaster_recovery_integration.rs "Phase 32 integration evidence"
[3]: ../benchmarks/phase32_disaster_recovery_metrics.json "Phase 32 benchmark artifact"
[4]: ../scripts/validate_phase32_disaster_recovery.sh "Phase 32 validation gate"
