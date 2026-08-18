# Phase 14 Read Optimization Benchmark Report

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** leader-lease fast-path reads, quorum-backed read-index reads, linearizable plan execution, and concurrency validation
**Author:** Manus AI
**Benchmark fixture:** deterministic committed three-member in-process cluster with injected monotonic ticks and a contended shared node lock
**Evidence:** `benchmarks/phase14_read_benchmark.json` and the `phase14_read_optimization` section of `benchmarks/security_compliance_metrics.json`

## Executive summary

Phase 14 validates two linearizable client-read paths. The lease fast path serves a query after a previously completed current-term quorum read-index round, while the quorum path performs a fresh bounded read-index acknowledgement before executing the plan. The latest repeatable gate completed **16,128 reads with zero errors** in each path comparison. At concurrency 32, the lease path measured **1,884 µs p95** and **77,360.38 operations per second**, while the quorum path measured **1,752 µs p95** and **71,580.85 operations per second**.

The benchmark confirms correctness, bounded concurrency behavior, and zero-error operation. It does **not** establish a universal latency or throughput advantage for the lease path in this small in-process fixture: scheduler and mutex effects dominate some tail samples. The protocol-level optimization is real because the lease path avoids a fresh read-index round, but production performance must be measured with authenticated transport, independent client workers, realistic network delay, and a server architecture that does not serialize all reads behind one benchmark mutex.

## Latest measurements

| Concurrency | Path | Operations | Errors | p50 (µs) | p95 (µs) | p99 (µs) | Throughput (ops/s) |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | Lease fast path | 128 | 0 | 2 | 3 | 7 | 181,213.79 |
| 1 | Quorum read-index | 128 | 0 | 5 | 9 | 15 | 121,312.87 |
| 2 | Lease fast path | 256 | 0 | 5 | 20 | 196 | 112,471.16 |
| 2 | Quorum read-index | 256 | 0 | 14 | 97 | 248 | 69,052.26 |
| 4 | Lease fast path | 512 | 0 | 13 | 27 | 979 | 74,912.65 |
| 4 | Quorum read-index | 512 | 0 | 13 | 34 | 426 | 75,276.54 |
| 8 | Lease fast path | 1,024 | 0 | 13 | 166 | 2,033 | 71,759.40 |
| 8 | Quorum read-index | 1,024 | 0 | 14 | 211 | 2,103 | 68,638.34 |
| 16 | Lease fast path | 2,048 | 0 | 14 | 758 | 3,170 | 70,699.90 |
| 16 | Quorum read-index | 2,048 | 0 | 14 | 351 | 4,482 | 73,282.82 |
| 32 | Lease fast path | 4,096 | 0 | 14 | 1,884 | 6,322 | 77,360.38 |
| 32 | Quorum read-index | 4,096 | 0 | 14 | 1,752 | 6,334 | 71,580.85 |

## Interpretation

The lease path has lower p50 latency at concurrency 1 and 2 and remains competitive in throughput at higher concurrency, but the p95 comparison is not monotonic. This is expected for a benchmark that serializes access through `Arc<Mutex<ConsensusNode>>`; the result measures the interaction between the protocol path, lock contention, scheduler timing, and allocation rather than a production network service. The correct conclusion is that the implementation is **functionally and operationally ready for the next benchmark layer**, not that this fixture proves a specific production speedup.

A follow-up benchmark should separate protocol execution from client serialization, use a read-only snapshot view for the execution portion, add a real authenticated transport round for the quorum path, record CPU and allocation counters, and run repeated samples with confidence intervals. No WAN or cross-machine capacity claim is made here.

## Safety evidence

The Phase 14 integration suite passes six tests. It rejects lease configurations where clock drift consumes the lease, treats the drift-adjusted expiration boundary as expired, invalidates leases on term and role transitions, refuses follower acknowledgements before the requested commit index, rejects mismatched and stale read responses, rejects duplicate completed requests, refuses to execute a plan from a different term or below the applied frontier, and requires explicit clock re-anchoring after a monotonic-clock regression.

The lease condition is intentionally conservative:

> A lease is safe only when `now_tick + max_clock_drift_ticks < expiration_tick`; equality is expired.

The consensus core uses an injected monotonic tick and never spawns timers or reads wall-clock time. A detected clock regression makes clock safety sticky until the caller explicitly re-anchors the monotonic source. The caller remains responsible for clock-health policy, suspend/resume detection, and choosing the quorum fallback when the monotonic source is uncertain.

## Compliance integration

The full security/compliance validator includes two Phase 14 gates: `leader_lease_read_optimization` and `linearizable_read_consistency`. The committed metrics artifact contains **20 passed gates**, the 12 raw lease/quorum benchmark rows, zero benchmark errors, the commit identifier, and the non-secret production-boundary notes.

## Reproduction

Run the dedicated gate from the repository root:

```bash
scripts/validate_phase14_read_optimization.sh
```

The gate runs `phase14_linearizable_reads_integration`, regenerates the raw JSON, and verifies all 12 expected rows, all requested concurrency levels, 128 operations per worker, zero errors, monotonic percentile ordering, and positive throughput. The complete project gate is:

```bash
scripts/validate_security_compliance.sh
```
