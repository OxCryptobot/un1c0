# Phase 51 memory-pressure and lock-free buffer-pool report

## Executive summary

The 128+ producer CAS baseline shows that the dominant scaling signals are not resident bytes alone. From 128 to 192 producers, peak RSS increased from **11332 KiB** to **13964 KiB** (1.23×), VmPeak increased from **377044 KiB** to **509976 KiB** (1.35×), and threads increased from **148** to **212**. The strongest pressure signal is admission retry amplification: retries per job rose from **331.43** to **2159.72** in this run. The benchmark intentionally produces one successful commit and expected same-generation conflicts for every remaining job, so it diagnoses contention/fencing pressure rather than valid-write capacity.

The bounded MPMC pool benchmark retained a **99.90%–99.97%** reuse ratio from 128 to 256 producers, performed only **16–11** fresh allocations, and recorded zero full-queue and oversize drops. This supports the pool’s bounded reuse contract for transient 512-byte buffers, but it does not prove lower RSS in the CAS workload because the pool is currently integrated into code-generation output, not the ownership-intent retry path.

## CAS pressure proxies

| Producers | Peak RSS KiB | VmPeak KiB | Threads | Retries/job | E2E p95 µs | Cache hit ratio | Successes | Expected failures |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 128 | 11332 | 377044 | 148 | 331.43 | 106964 | 99.92% | 1 | 8191 |
| 160 | 12852 | 443600 | 180 | 2589.07 | 28566 | 99.93% | 1 | 10239 |
| 192 | 13964 | 509976 | 212 | 2159.72 | 15228 | 99.95% | 1 | 12287 |

## Pool evidence

| Producers | Operations | Pool reuse | Fresh buffers | Returns | Full drops | Oversize drops | Ops/sec |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 128 | 16384 | 99.90% | 16 | 16384 | 0 | 0 | 15426 |
| 192 | 24576 | 99.96% | 10 | 24576 | 0 | 0 | 63020 |
| 256 | 32768 | 99.97% | 11 | 32768 | 0 | 0 | 41646 |

## Interpretation and next boundary

The thread and VmPeak curves are consistent with native producer-thread stack/address-space reservations and bounded worker infrastructure. The retry curve indicates that producer-side admission loops can dominate scheduling and transient intent cloning under conflict storms. A pooled buffer can reduce repeated transient serialization/output allocations, but it cannot eliminate thread stacks, queue nodes, `OwnershipBoundCasIntent` cloning, cryptographic verification work, or filesystem/CAS state. The next allocator study must instrument allocation bytes and peak live bytes around candidate cloning and admission retries, compare fixed worker pools against one-thread-per-producer, and include unique-request, sequential-valid, mixed-valid/conflicting, and forged-evidence workloads.

The raw artifacts are `benchmarks/phase51_memory_profile_baseline.json` and `benchmarks/phase51_buffer_pool_metrics.json`; the derived analysis is `benchmarks/phase51_memory_pressure_analysis.json`; and the chart is `benchmarks/phase51_memory_pressure.png`. These are local sanitized observations with no secret material and no cluster mutation.
