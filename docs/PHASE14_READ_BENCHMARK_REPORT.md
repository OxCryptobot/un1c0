# Phase 14 Read Optimization Benchmark Report

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** leader-lease fast-path reads, quorum-backed read-index reads, linearizable plan execution, and concurrency validation
**Author:** Manus AI
**Benchmark fixture:** deterministic committed three-member in-process cluster with injected monotonic ticks and a contended shared node lock
**Evidence:** `benchmarks/phase14_read_benchmark.json` and the `phase14_read_optimization` section of `benchmarks/security_compliance_metrics.json`

## Executive summary

Phase 14 validates two linearizable client-read paths. The lease fast path serves a query after a previously completed current-term quorum read-index round, while the quorum path performs a fresh bounded read-index acknowledgement before executing the plan. The latest repeatable gate completed **16,128 reads with zero errors** in each path comparison. At concurrency 32, the lease path measured **1,571 µs p95** and **78,846.78 operations per second**, while the quorum path measured **1,799 µs p95** and **82,339.48 operations per second**.

The benchmark confirms correctness, bounded concurrency behavior, and zero-error operation. It does **not** establish a universal latency or throughput advantage for the lease path in this small in-process fixture: scheduler and mutex effects dominate some tail samples. The protocol-level optimization is real because the lease path avoids a fresh read-index round, but production performance must be measured with authenticated transport, independent client workers, realistic network delay, and a server architecture that does not serialize all reads behind one benchmark mutex.

## Latest measurements

| Concurrency | Path | Operations | Errors | p50 (µs) | p95 (µs) | p99 (µs) | Throughput (ops/s) |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | Lease fast path | 128 | 0 | 2 | 2 | 5 | 146,090.20 |
| 1 | Quorum read-index | 128 | 0 | 5 | 5 | 6 | 164,411.36 |
| 2 | Lease fast path | 256 | 0 | 12 | 30 | 167 | 87,238.21 |
| 2 | Quorum read-index | 256 | 0 | 12 | 13 | 25 | 93,903.81 |
| 4 | Lease fast path | 512 | 0 | 13 | 87 | 723 | 68,358.81 |
| 4 | Quorum read-index | 512 | 0 | 13 | 51 | 960 | 72,733.24 |
| 8 | Lease fast path | 1,024 | 0 | 13 | 186 | 1,728 | 77,323.85 |
| 8 | Quorum read-index | 1,024 | 0 | 13 | 168 | 1,951 | 74,141.33 |
| 16 | Lease fast path | 2,048 | 0 | 13 | 387 | 3,237 | 78,599.30 |
| 16 | Quorum read-index | 2,048 | 0 | 13 | 437 | 3,457 | 74,987.80 |
| 32 | Lease fast path | 4,096 | 0 | 13 | 1,571 | 5,982 | 78,846.78 |
| 32 | Quorum read-index | 4,096 | 0 | 13 | 1,799 | 5,132 | 82,339.48 |

## Interpretation

At concurrency 1 the quorum path is faster in this sample. At concurrency 2 and 4 the paths remain near parity, while concurrency 8 and 16 slightly favor the lease path in throughput. At concurrency 32 the lease path has lower p95 and approximately **0.96×** the quorum throughput. This confirms that the lease path removes protocol work without guaranteeing a universal throughput or tail-latency advantage under a shared in-process mutex.

The lease path avoids creating and completing a fresh read-index round, so its protocol work is lower by construction. The benchmark serializes calls through `Arc<Mutex<ConsensusNode>>`, which can dominate tail latency and obscure the protocol-level advantage. The next performance layer should separate immutable read execution from mutable consensus bookkeeping, use independent client workers, measure authenticated transport, record CPU and allocation counters, and repeat each point for confidence intervals. No WAN or cross-machine capacity claim is made here.

## Safety evidence

The Phase 14 integration suite passes six tests. It rejects lease configurations where clock drift consumes the lease, treats the drift-adjusted expiration boundary as expired, invalidates leases on term and role transitions, refuses follower acknowledgements before the requested commit index, rejects mismatched and stale read responses, rejects duplicate completed requests, refuses to execute a plan from a different term or below the applied frontier, and requires explicit clock re-anchoring after a monotonic-clock regression.

The lease condition is intentionally conservative:

> A lease is safe only when `now_tick + max_clock_drift_ticks < expiration_tick`; equality is expired.

The consensus core uses an injected monotonic tick and never spawns timers or reads wall-clock time. A detected clock regression makes clock safety sticky until the caller explicitly re-anchors the monotonic source. The caller remains responsible for clock-health policy, suspend/resume detection, and choosing the quorum fallback when the monotonic source is uncertain.

## Compliance integration

The full security/compliance validator now includes Phase 15 timer gates, Phase 16 replication-flow-control gates, and Phase 17 remote-audit gates, reporting **26 passed gates**. The committed metrics artifact contains the 12 raw Phase 14 lease/quorum rows, zero benchmark errors, Phase 15 and Phase 16 safety records, Phase 17 remote-audit evidence, the commit identifier, and the non-secret production-boundary notes.

## Reproduction

Run the dedicated Phase 14 gate:

```bash
scripts/validate_phase14_read_optimization.sh
```

Run the detailed analysis generator:

```bash
python3 scripts/analyze_phase14_benchmark.py
```

Run the complete project gate, including Phases 15, 16, and 17:

```bash
scripts/validate_security_compliance.sh
```
