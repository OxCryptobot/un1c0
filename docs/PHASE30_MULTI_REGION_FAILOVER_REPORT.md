# Phase 30 Deterministic Multi-Region Failover Testing

## Summary

Phase 30 adds a dependency-light deterministic simulator for multi-region queue ownership and failover behavior. The simulator models three regions and five nodes, directed link faults, durable snapshots, quorum-loss fences, distinct observer reports, higher-term ownership transfer, clock-skew fencing, replayable event traces, and safety/liveness reports. It intentionally runs locally and does not mutate Kubernetes, cloud networking, DNS, managed storage, or production credentials.

The first implementation includes nine integration tests and a machine-readable benchmark example. The tests cover deterministic trace replay, minority-region fencing, asymmetric partition healing, durable crash recovery, stale transfer rejection, invalid fence rejection, clock-skew fail-closed transfer, observer quorum admission, and delayed-link behavior.

## Simulator contracts

| Contract | Implementation |
|---|---|
| Deterministic topology | `MultiRegionSimulationConfig::three_region` creates regions `region-a`, `region-b`, and `region-c`, five named nodes, a quorum of three, and bounded ticks. A seed and scenario ID are persisted in reports. |
| Directed transport faults | `LinkFault` supports `Healthy`, `Drop`, `Delay`, `Duplicate`, `Reorder`, and `Corrupt`. `partition_regions` applies symmetric region cuts; `set_link_fault` permits asymmetric directed faults. |
| Durable state | `MultiRegionSnapshot` persists owner, term, epoch, clock-skew observation, fence, queue sequence, acknowledgements, observer reports, links, pending acknowledgements, and event trace state. |
| Fence admission | Local quorum loss creates a bounded fence. Distinct observer reports must reach quorum before `submit_observer_quorum_loss` records a fence. |
| Failover | `accept_transfer` requires a changed owner, higher term, higher epoch, known node, and acceptable clock uncertainty. It clears fence, acknowledgement, observer, and pending-delivery state. |
| Replayability | Events are sequenced and hashed with SHA-256. Same scenario ID and seed produce the same report and trace digest. |
| Safety/liveness | Reports distinguish `safety_passed` from `liveness_passed`. No-quorum fencing with retained work is a safety success even when liveness is intentionally false. |

## Executable evidence

The dedicated test command runs `tests/phase30_multiregion_failover_integration.rs`. The benchmark example runs five deterministic scenarios and writes `benchmarks/phase30_multiregion_failover_metrics.json`.

| Scenario | Expected result |
|---|---|
| `majority_partition` | Three dropped acknowledgements, active fence, no commit, safety true, liveness false. |
| `heal_and_failover` | Higher-term transfer to `node-b1`, fence cleared, quorum-gated retry commits, safety and liveness true. |
| `asymmetric_partition` | Directed owner-to-peer loss fences the old owner; healing plus transfer commits under the new owner. |
| `observer_quorum` | First observer report is insufficient; second distinct reachable observer admits the fence. |
| `clock_skew_boundary` | Transfer is blocked while skew exceeds the bound; after re-anchoring, transfer and quorum commit succeed. |

The integration suite additionally covers snapshot restoration, stale-owner transfer rejection, invalid quorum-loss observations, and delayed-link tick progression.

## Security and compliance boundaries

The simulator is intentionally explicit about what it proves. It can prove deterministic local state transitions, fail-closed fence behavior, higher-term transfer ordering, durable snapshot restoration, and invariant outcomes under the modeled faults. It cannot prove cloud-region isolation, kernel packet behavior, TLS termination, DNS or load-balancer convergence, managed-database replication, real process fencing, or production observer membership. Those remain deployment and staging-chaos boundaries.

## Proposed Phase 30 gates

The compliance delta adds eight gates: `region_topology_is_deterministic`, `asymmetric_partition_is_replayable`, `observer_quorum_admission`, `split_brain_commit_exclusion`, `stale_owner_is_fenced_after_heal`, `transfer_crash_recovers_safely`, `clock_skew_boundary_is_fail_closed`, and `multi_region_retry_reaches_quorum`. The machine-readable report also records the simulator artifact’s production boundary and scenario reports.

## Reproduction

```bash
scripts/validate_phase30_multiregion_failover.sh
cargo run --example phase30_failover_benchmark -- --output benchmarks/phase30_multiregion_failover_metrics.json
```

## References

[1]: ../src/multiregion.rs "Deterministic multi-region failover simulator"
[2]: ../tests/phase30_multiregion_failover_integration.rs "Phase 30 integration tests"
[3]: ../examples/phase30_failover_benchmark.rs "Phase 30 benchmark example"
[4]: ../benchmarks/phase30_multiregion_failover_metrics.json "Generated Phase 30 scenario artifact"
[5]: ../scripts/validate_phase30_multiregion_failover.sh "Dedicated Phase 30 validation gate"
