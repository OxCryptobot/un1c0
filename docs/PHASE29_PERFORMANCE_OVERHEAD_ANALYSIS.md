# Phase 29 Performance and Compliance Overhead Analysis

## Scope and evidence

The performance snapshot analyzed here is the **64-gate Phase 28 baseline artifact** and records `benchmark_concurrency: 8`; Phase 29 adds four correctness gates but does not add new timing rows. Its operation samples compare baseline and optimized local paths with zero recorded errors. The artifact does not measure the wall-clock cost of running all 64 or 68 compliance gates in parallel or in CI; it measures representative subsystem operations and separate concurrency sweeps. Therefore, the numbers below are directional subsystem overhead evidence, not a production capacity claim.

## Operation-level overhead

| Operation | p95 change | p99 change | Throughput change | Interpretation |
|---|---:|---:|---:|---|
| `plan_validate` | +6.46% | +16.23% | −2.01% | Small validation overhead at multi-million-operation throughput. |
| `repository_search` | −63.83% | −60.91% | +270.27% | The optimized filesystem/search path is a material improvement, not overhead. |
| `provider_route` | −19.24% | −40.23% | +2.70% | Routing optimization improves tail latency with nearly flat throughput. |
| `checkpoint_save_load` | −9.11% | +13.85% | −2.60% | Median/tail behavior is mixed; persistence remains sensitive to tail variance. |
| `verification_manifest` | +7.94% | +12.80% | −3.77% | Content verification adds a small measurable cost. |
| `evolution_signature_verify` | +28.11% | +4.59% | +14.19% | p95 rises while aggregate throughput improves; the distribution is not represented by a single average. |
| `canary_report_from_workspace` | +14.09% | +15.76% | −18.74% | The largest throughput regression in the listed evolution/audit operations; worth profiling before high-frequency canary use. |

All operation rows report zero baseline and optimized errors. The security-sensitive additions therefore show bounded local costs in the available samples, but the artifact does not include CPU, memory, filesystem I/O, or concurrent process measurements.

## Authenticated partition benchmark

| Scenario | Quorum | Dropped | Verification p95 | Verification throughput |
|---|---|---:|---:|---:|
| `healthy` | Available | 0.00% | 34.392 µs | 33,524.649 ops/s |
| `majority_partition` | Available | 64.00% | 29.815 µs | 33,059.922 ops/s |
| `minority_partition` | Unavailable | 84.00% | 30.166 µs | 32,435.325 ops/s |

The majority partition retains quorum; the minority partition does not. Messages are dropped before in-process Ed25519 verification, so the benchmark characterizes local authentication/drop filtering rather than real TCP, TLS, kernel, cross-machine, or split-brain recovery overhead.

## Phase 14 high-concurrency read sweep

The artifact retains lease-fast-path and quorum-read-index rows at concurrency 1, 2, 4, 8, 16, and 32, all with zero errors. Relative to the lease path, quorum-read-index throughput is lower by 9.66%, 8.22%, 1.38%, 9.08%, and 4.30% at concurrency 1, 2, 4, 8, and 16, respectively; at concurrency 32 it is 3.15% higher in this sample. p95 latency is noisy: quorum is 100% higher at concurrency 1, 9.52% lower at 2, 62.70% lower at 4, 38.92% lower at 8, 4.36% higher at 16, and 1.11% higher at 32. The non-monotonic values indicate a local benchmark distribution rather than a guaranteed scaling law.

## Compliance-load interpretation

The 64-gate baseline audit, and the current 68-gate audit, are primarily correctness and evidence suites. Structural gates such as hash binding, replay rejection, state rollback, ownership binding, and deployment-boundary checks do not each have independent latency samples in the JSON.
 The strongest directly observed performance signals are therefore:

| Signal | Finding |
|---|---|
| Correctness under measured load | All listed operation and Phase 14 rows report zero errors. |
| Validation cost | `plan_validate` p95 increases 6.46% and throughput decreases 2.01%; manifest verification p95 increases 7.94% and throughput decreases 3.77%. |
| Evolution/audit cost | Workspace canary report throughput decreases 18.74%; signature verification throughput increases 14.19% despite a 28.11% p95 increase. |
| Partition authentication cost | Verification p95 remains near 30–34 µs across healthy, majority, and minority scenarios, but the work is in-process. |
| High-concurrency consistency | The Phase 14 sweep has zero errors through concurrency 32, with noisy p95 and modest quorum-throughput differences. |

## Recommended next benchmark

To measure actual “high gate validation load,” add a deterministic suite-level benchmark that runs the exact 68-gate validator in isolated repetitions, records cold/warm startup, wall-clock duration, CPU time, peak RSS, filesystem bytes, per-gate durations, failure rates, and concurrency scaling. Keep the benchmark separate from the compliance pass/fail artifact so performance noise cannot turn a correctness gate into a false pass.

## References

[1]: ../benchmarks/security_compliance_metrics.json "Current 64-gate security metrics artifact"
[2]: ../benchmarks/security_compliance_audit.json "Current independent audit"
[3]: ../scripts/collect_security_compliance_metrics.py "Metrics collector"
[4]: ../docs/PHASE28_SECURITY_COMPLIANCE_DELTA.md "Phase 28 60-to-64 gate delta"
