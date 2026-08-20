# Phase 47 memory and allocator-pressure profile

## Executive summary

A sustained hot-key ownership-bound CAS workload was measured at **32, 64, and 96 producer threads**, with 64 submitted intents per producer. The final run remained within bounded process resources: peak RSS increased from **6,548 KiB** at 32 producers to **10,136 KiB** at 96 producers, while peak RSS high-water increased from **6,772 KiB** to **10,304 KiB**. Peak virtual-memory reservation increased from **177,712 KiB** to **310,652 KiB**, and peak threads increased from **52** to **116**. These are local sandbox observations, not production capacity claims.

The dominant pressure signal was bounded contention bookkeeping rather than resident-memory exhaustion. The 96-producer run achieved a **99.90% verification-fact cache hit ratio** and recorded **31.34 limiter retries per submitted job**. Because the hot-key fixture intentionally reused one CAS generation, one commit succeeded and the remaining 6,143 outcomes were expected generation conflicts; they are not interpreted as crashes, panics, or allocator failures. Rust has no tracing garbage collector in this path, so “GC pressure” is reported as allocator churn and retention proxies rather than pause time.

## Measurements

| Producers | Jobs | Peak RSS (KiB) | RSS high-water (KiB) | Peak VmPeak (KiB) | Peak threads | Cache hit ratio | Limiter retries/job | End-to-end p95 (µs) |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 32 | 2,048 | 6,548 | 6,772 | 177,712 | 52 | 99.69% | 1.42 | 3,089 |
| 64 | 4,096 | 8,668 | 8,860 | 244,268 | 84 | 99.85% | 5.74 | 5,557 |
| 96 | 6,144 | 10,136 | 10,304 | 310,652 | 116 | 99.90% | 31.34 | 15,926 |

The raw sanitized measurements are stored in [`benchmarks/phase47_memory_profile_metrics.json`](../benchmarks/phase47_memory_profile_metrics.json), the derived analysis is in [`benchmarks/phase47_memory_profile_analysis.json`](../benchmarks/phase47_memory_profile_analysis.json), and the chart is [`benchmarks/phase47_memory_profile.png`](../benchmarks/phase47_memory_profile.png).

## Interpretation

From 32 to 96 producers, peak RSS increased by **3,588 KiB**, a **1.55×** factor, while peak virtual-memory reservation increased by **132,940 KiB**, a **1.75×** factor. Peak threads increased by **64**, matching the additional producer threads plus sampler/worker scheduling behavior observed in the benchmark process. The measurements suggest that the bounded queues, cloned intents, ticket channels, cache entries, and metric samples remain small in resident memory at this workload size, while thread stacks and allocator address-space reservation are more visible in the virtual-memory series.

The cache hit ratio demonstrates that the Phase 46 context-bound cryptographic fact cache is effective for repeated hot-key traffic. It does not imply that unique-request traffic has the same profile: unique hashes intentionally miss the cache and incur fresh cryptographic work. The retry rate at 96 producers is the clearest operational pressure indicator in this fixture. It reflects admission contention and retry object churn; it is not a proof of memory leakage.

## Limitations and next measurement boundary

The benchmark samples Linux `/proc/self/status` every 10 ms and therefore reports process-level RSS, high-water RSS, virtual-memory peak, and thread count. It does not count allocations, identify allocator bins, measure fragmentation, capture cgroup memory events, or observe kernel reclaim. Since Rust does not use tracing GC in this path, no GC pause or collection rate can be inferred. An allocator-instrumented follow-up should use a fixed allocator configuration, capture allocation/free counts and bytes, run cold and warm workloads separately, include unique-request and mixed valid/conflict traffic, and compare per-intent allocation rate with queue depth and cache miss rate.

The workload also intentionally reuses one generation to create a conflict storm. It is therefore appropriate for admission and bookkeeping pressure, but not for valid durable-write throughput. The separate repeated lease-migration benchmark completes 128 fenced handoffs with 256 witness acknowledgements, finishes in `Activated`, and reaches final ownership epoch 129; its sanitized artifact is [`benchmarks/phase47_lease_migration_metrics.json`](../benchmarks/phase47_lease_migration_metrics.json).

## Reproducibility

Run the memory benchmark with `cargo run --quiet --example phase47_memory_profile_benchmark`, then regenerate the derived JSON and chart with `MPLBACKEND=Agg python3 scripts/analyze_phase47_memory_profile.py`. Run the lease-migration benchmark with `cargo run --quiet --example phase47_lease_migration_benchmark`. Both examples emit only bounded counters, state labels, epochs, durations, and explicit false secret/mutation indicators.
