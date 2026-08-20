# Phase 50 UEG performance and memory-pressure report

## Executive summary

The deterministic Phase 50 benchmark compares one, two, four, eight, sixteen, and thirty-two typed UEG functions. Parse p95 rises from **397 µs** for one function to **9881 µs** for thirty-two functions, while normalized parse p95 per function falls from **397 µs** to **308 µs**. This indicates roughly linear total work with improving amortization, not an observed super-linear parser cliff, within the tested range. Incremental target generation remains bounded by emitted chunks and bytes; the Go and Zig bindings carry more target-specific formatting work than Rust and Python in this fixture.

The Phase 47 high-concurrency memory profile remains a pressure-proxy study. At 96 producers it measured **10136 KiB** peak RSS, **310652 KiB** VmPeak, **116** threads, **31.34** limiter retries per job, and **15926 µs** end-to-end p95. The profile records no GC pauses and no allocator attribution; Rust has no tracing GC in this path. Because the fixture intentionally creates same-generation conflicts, its failed outcomes measure contention bookkeeping rather than valid durable-write throughput.

## Parser comparison

| Functions | Source bytes | Parse p50 (µs) | Parse p95 (µs) | Parse p95/function (µs) | Parse max (µs) |
|---:|---:|---:|---:|---:|---:|
| 1 | 215 | 282 | 397 | 397 | 619 |
| 2 | 430 | 617 | 900 | 450 | 992 |
| 4 | 860 | 1141 | 1725 | 431 | 1932 |
| 8 | 1720 | 2198 | 2758 | 344 | 2929 |
| 16 | 3446 | 4283 | 5032 | 314 | 5382 |
| 32 | 6902 | 8422 | 9881 | 308 | 12441 |

## Target generation comparison

| Functions | Target | Chunks | Bytes | Generation p50 (µs) | Generation p95 (µs) | Generation max (µs) |
|---:|---|---:|---:|---:|---:|---:|
| 1 | rust | 1 | 249 | 6 | 7 | 18 |
| 1 | go | 1 | 234 | 26 | 57 | 119 |
| 1 | zig | 1 | 269 | 27 | 30 | 39 |
| 1 | python | 1 | 214 | 5 | 6 | 8 |
| 2 | rust | 2 | 498 | 23 | 28 | 42 |
| 2 | go | 2 | 440 | 90 | 107 | 127 |
| 2 | zig | 2 | 509 | 95 | 112 | 122 |
| 2 | python | 2 | 429 | 18 | 18 | 32 |
| 4 | rust | 4 | 996 | 25 | 33 | 48 |
| 4 | go | 4 | 852 | 106 | 120 | 137 |
| 4 | zig | 4 | 989 | 113 | 134 | 239 |
| 4 | python | 4 | 859 | 21 | 27 | 41 |
| 8 | rust | 8 | 1992 | 53 | 72 | 98 |
| 8 | go | 8 | 1676 | 213 | 285 | 379 |
| 8 | zig | 8 | 1949 | 227 | 308 | 373 |
| 8 | python | 8 | 1719 | 44 | 59 | 70 |
| 16 | rust | 16 | 3990 | 109 | 137 | 193 |
| 16 | go | 16 | 3330 | 430 | 549 | 697 |
| 16 | zig | 16 | 3875 | 454 | 661 | 777 |
| 16 | python | 16 | 3445 | 144 | 166 | 184 |
| 32 | rust | 32 | 7990 | 216 | 232 | 311 |
| 32 | go | 32 | 6642 | 859 | 950 | 973 |
| 32 | zig | 32 | 7731 | 911 | 1087 | 1264 |
| 32 | python | 32 | 6901 | 172 | 188 | 242 |

## Allocator-pressure proxy review

The 32-to-96 producer memory profile increased peak RSS by **3588 KiB** (1.55×), VmPeak by **132940 KiB** (1.75×), and threads by **64**. Cache hit ratio remained between **99.69%** and **99.90%**, but total limiter retries increased from **2906** to **192577** and end-to-end p95 increased from **3089 µs** to **15926 µs**.

These observations support a bounded-contention interpretation: RSS remained modest relative to virtual-memory and thread growth, while retries, queue/wait/service tails, and expected-conflict completion dominated degradation. They do not prove that retries allocate memory, identify allocator bins, measure fragmentation, or establish a leak. The next measurement should add allocator instrumentation, cgroup memory events, fixed allocator settings, and unique-request/mixed-validity workloads.

## Artifacts and limitations

The raw benchmark is [`benchmarks/phase50_ueg_codegen_metrics.json`](../benchmarks/phase50_ueg_codegen_metrics.json), derived analysis is [`benchmarks/phase50_ueg_codegen_analysis.json`](../benchmarks/phase50_ueg_codegen_analysis.json), and the chart is [`benchmarks/phase50_ueg_codegen_performance.png`](../benchmarks/phase50_ueg_codegen_performance.png). The source memory profile is [`benchmarks/phase47_memory_profile_metrics.json`](../benchmarks/phase47_memory_profile_metrics.json). These are local deterministic observations, not production capacity claims. The benchmark uses a fixed source fixture, warm process, bounded iteration count, and no cluster mutation or secret material.
