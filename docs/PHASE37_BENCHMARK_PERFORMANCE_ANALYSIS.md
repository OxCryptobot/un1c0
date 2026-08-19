# Phase 37 Benchmark Performance and Resource Analysis

The analysis uses the published compliance artifact at commit `b2f4b770d901ca4bdf60024d7c082ef89fb8aa82` and direct execution measurements of the release benchmark and nine-test Phase 37 integration binary. The measurement is local and sanitized; it does not represent production cross-region capacity.

## Executive findings

Phase 37 completes its functional benchmark with three admitted telemetry events, a valid journal chain, a committed failover, 4,096 transport fuzz cases, and 4,096 reservation fuzz cases. Both fuzz paths report zero panics and the maximum generated connection epoch reaches 4,096. The reservation harness accepts 653 cases (15.94%) and rejects 3,443 malformed or duplicate cases; the transport harness rejects all generated cases because its mutation schedule intentionally corrupts every post-signing envelope before receiver admission.

The direct release benchmark uses approximately 15.7 MiB peak resident memory, 1.743 s child CPU time, and 2.091 s wall time. The direct release integration binary uses the same approximately 15.7 MiB peak RSS, 0.153 s child CPU time, and 0.192 s wall time for nine tests. The benchmark's internal `elapsed_ms` is 2,060 ms, which is close to—but not identical with—the externally measured 2,091 ms; the difference is expected process-launch and measurement overhead, not a correctness issue.

## Phase 37 functional and fuzz workload

| Workload | Volume | Outcome | Interpretation |
|---|---:|---|---|
| Telemetry admission | 3 events | journal valid; failover committed | Control-plane path is functionally complete, but this is too small for latency characterization |
| Authenticated transport fuzz | 4,096 cases | 0 panics; 4,096 rejected | Malformed-envelope and epoch-churn safety is demonstrated; accepted-path throughput is not measured |
| Witness reservation fuzz | 4,096 cases | 0 panics; 653 accepted; 3,443 rejected | Crash-point and malformed-state paths remain panic-free; persistence cost is included in elapsed time |
| Compliance | 129 gates | 129 passed | No gate failures, but gate status is not a performance metric |

The fuzz reports are safety evidence rather than a balanced workload benchmark. The transport harness's 0/4,096 accepted result is intentional: each generated envelope is signed and then mutated or misbound, so the receiver is being tested primarily for rejection safety. A future throughput benchmark should include a separate valid-envelope control group and report accepted/rejected latency distributions independently.

## Authenticated consensus transport partition metrics

| Scenario | Verified messages | Dropped | Quorum | p95 verification | Verification throughput |
|---|---:|---:|---|---:|---:|
| healthy | 10,000 | 0 | yes | 37.687 µs | 32,474 ops/s |
| majority_partition | 3,600 | 6,400 | yes | 29.645 µs | 33,555 ops/s |
| minority_partition | 1,600 | 8,400 | no | 28.935 µs | 33,433 ops/s |

The healthy five-member path verifies 32,474 envelopes/s at 37.687 µs p95. The majority and minority partition rows show similar local verification cost because they measure in-process signature verification after the partition filter; they do not measure network delay, TLS, disk persistence, or routing. The minority path is correctly marked `quorum_available=false`, so its roughly 33,433 ops/s must not be interpreted as an available service throughput figure.

## Concurrency-eight operation metrics

| Operation | Baseline p95 | Optimized p95 | p95 change | Baseline throughput | Optimized throughput | Throughput change |
|---|---:|---:|---:|---:|---:|---:|
| plan_validate | 0.0005 ms | 0.0005 ms | +6.46% | 3,011,309/s | 2,950,823/s | -2.01% |
| repository_search | 37.2023 ms | 13.4543 ms | -63.83% | 249/s | 923/s | +270.27% |
| provider_route | 0.9877 ms | 0.7976 ms | -19.24% | 31,005/s | 31,841/s | +2.70% |
| checkpoint_save_load | 0.1514 ms | 0.1376 ms | -9.11% | 91,895/s | 89,504/s | -2.60% |
| verification_manifest | 0.0406 ms | 0.0438 ms | +7.94% | 646,600/s | 622,215/s | -3.77% |
| evolution_signature_verify | 0.0485 ms | 0.0621 ms | +28.11% | 122,669/s | 140,071/s | +14.19% |
| canary_report_from_workspace | 0.0794 ms | 0.0906 ms | +14.09% | 256,893/s | 208,755/s | -18.74% |

Repository search is the clear optimization win: p95 falls 63.83% from 37.202 ms to 13.454 ms while throughput rises 270.27% from 249 to 923 ops/s, with zero errors in both paths. Provider routing improves modestly. The remaining operations trade small p95 or throughput regressions for stronger instrumentation or verification behavior; these should be monitored with a regression budget rather than optimized blindly. All rows report zero errors.

## Read-path scaling context

| Concurrency | Lease p95 | Quorum p95 | Lease throughput | Quorum throughput | Observation |
|---:|---:|---:|---:|---:|---|
| 1 | 2 µs | 5 µs | 84,130/s | 154,703/s | lease p95 lower |
| 2 | 13 µs | 12 µs | 111,788/s | 104,580/s | quorum p95 lower |
| 4 | 20 µs | 104 µs | 143,827/s | 90,202/s | lease p95 lower |
| 8 | 193 µs | 430 µs | 98,288/s | 88,003/s | lease p95 lower |
| 16 | 844 µs | 553 µs | 87,022/s | 86,174/s | quorum p95 lower |
| 32 | 1414 µs | 1350 µs | 89,059/s | 88,894/s | quorum p95 lower |

At concurrency 8, the lease path has 193 µs p95 versus 430 µs for quorum reads and approximately 11.7% higher throughput. At concurrency 16 and 32, p95 tails converge and the quorum path is slightly better on p95; both paths remain error-free. The p99 tail reaches 5.340 ms for leases and 5.066 ms for quorum reads at concurrency 32, which is a signal to instrument scheduler contention, clock uncertainty, and queueing before claiming linear scalability.

## Resource utilization

| Process | Wall time | Child user CPU | Child system CPU | Peak RSS | Output |
|---|---:|---:|---:|---:|---:|
| Phase 37 release benchmark | 2.091 s | 1.743 s | 0.137 s | 15.68 MiB | 729 bytes |
| Phase 37 release integration binary | 0.192 s | 0.122 s | 0.031 s | 15.68 MiB | 692 bytes |

The benchmark is CPU-dominant: approximately 1.879 s of combined user and system CPU over 2.091 s wall time, or roughly 90% single-process CPU occupancy. The test binary uses approximately 0.153 s CPU over 0.192 s wall time. Peak RSS is low and stable for this bounded local workload, but it includes process-level allocation and not a production multi-region agent fleet. No disk-byte, fsync-latency, network-byte, thread-count, allocator, or per-operation CPU metrics are captured yet.

## Production measurement gaps and next instrumentation batch

| Gap | Why it matters | Recommended next control |
|---|---|---|
| Valid-path telemetry/transport latency | Current fuzz workload is rejection-heavy | Add valid-envelope control samples and p50/p95/p99 admission histograms |
| Persistence resource cost | Reservation and intent stores use fsync/atomic rename | Count bytes written, fsync duration, staging retries, and recovery scans |
| Supervisor CPU/memory limits | RSS is process-wide and single-run | Add bounded per-component budgets and periodic resource snapshots |
| External authority liveness | Local signatures do not prove authority availability | Add signed authority heartbeats with TTL and monotonic fencing epochs |
| Consumer enforcement | A token is evidence, not enforcement | Require signed acknowledgements from write gateway, scheduler, socket owner, routing, and process-fence consumers |
| Cross-region timing | In-process ticks do not prove clock safety | Bind observations to a trusted clock-health source and fail closed on uncertainty |

## Interpretation boundary

> These measurements prove bounded local safety and relative implementation behavior. They do not establish production capacity, cross-region network performance, cloud-region failure truth, managed storage durability, DNS convergence, process termination, mTLS lifecycle correctness, or external-fence enforcement.

The next architecture batch should therefore implement the **external fencing supervision seam**: a bounded, signed authority-heartbeat contract; registered consumer acknowledgements bound to the exact resource and fence epoch; a supervisor decision that blocks promotion when authority evidence or required consumer enforcement is stale; and sanitized resource-budget telemetry. The enforcement adapters remain deployment-specific, but the local contract can make missing supervision evidence visible and fail closed.
