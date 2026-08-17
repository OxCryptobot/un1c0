# Repository Search Profile and Optimization

## Scope

This report profiles the repository-search hotspot identified in the un1c0 architecture benchmark and evaluates a bounded content-snapshot optimization. The comparison uses the same deterministic fixture, query, result bounds, sample count, and concurrency levels as the committed baseline benchmark. The baseline is `benchmarks/agent_benchmark.json`; the optimized run is `benchmarks/agent_benchmark_optimized.json`.

The optimization stores bounded UTF-8 file content already read during index construction, with a configurable `IndexConfig.max_cached_bytes` budget of 32 MiB by default. It also precomputes a deterministic `(path, line) → symbol name` lookup map. Deserialized indexes keep portable metadata and safely fall back to filesystem reads when the private cache is absent; a zero-byte cache test preserves search semantics.

## Results

| Concurrency | Baseline p95 | Optimized p95 | p95 reduction | Baseline throughput | Optimized throughput | Throughput gain |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 4.176 ms | 0.580 ms | 86.1% | 263 ops/s | 1,890 ops/s | 618.8% |
| 2 | 16.624 ms | 0.999 ms | 94.0% | 216 ops/s | 2,596 ops/s | 1,099.9% |
| 4 | 17.923 ms | 6.096 ms | 66.0% | 319 ops/s | 1,155 ops/s | 262.6% |
| 8 | 37.202 ms | 13.454 ms | 63.8% | 249 ops/s | 923 ops/s | 270.3% |

Both runs recorded zero errors at every concurrency. The optimization reduces the concurrency-eight p95 from 37.2 ms to 13.5 ms and increases measured throughput from 249 to 923 operations per second. The remaining tail is consistent with CPU-side token scoring, line scanning, candidate construction, sorting, and lock contention in the benchmark harness rather than repeated file opens alone.

## Implementation and safety

The cache is populated only from regular, non-symlink files that already satisfy the existing extension and maximum-file-size filters. Cache population is bounded by `max_cached_bytes`; files beyond the remaining budget are not cached and continue through the safe filesystem fallback. The cache is private, skipped during serialization, and backed by immutable reference-counted maps so concurrent searches do not mutate shared state. Search ranking, language filters, maximum results, maximum context bytes, symlink exclusion, and deterministic ordering remain unchanged.

The regression suite covers default cache population, zero-budget fallback, deterministic indexing, hard result/context bounds, symlink exclusion, and large-file exclusion. The interactive dashboard combines baseline benchmark metrics with the before/after repository-search profile and is available at [`benchmarks/benchmark_dashboard.html`](../benchmarks/benchmark_dashboard.html).

## Production interpretation

The result justifies the cache optimization as a high-value local improvement, but it is not a production SLO. The fixture is small and local, and the benchmark uses a deterministic query rather than a representative workload distribution. Before setting concurrency or capacity targets, repeat the measurement on staging hardware with repositories that vary in file count, average file size, language mix, cache warmness, storage class, query selectivity, and concurrent agent sessions. Observe resident memory as the cache budget changes; a higher cache limit trades read latency for memory pressure.

The next profiling step should separate cold-index build cost from warm-search cost and add a cache hit/miss counter to runtime telemetry. If warm search remains dominated by line scanning and sorting after real-repository validation, consider a bounded token or line index rather than increasing the content cache without measurement.
