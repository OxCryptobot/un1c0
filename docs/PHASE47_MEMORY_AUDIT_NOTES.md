# Phase 47 memory audit notes and lease-migration proposal

## Observed Phase 46 baseline

The published repository is at commit `1e87325380390434b921de9c16b0113ca9a31084`, and local `main` matches `origin/main`. The Phase 46 benchmark covers 1, 2, 4, 8, 16, and 32 producers with eight hot-key replay jobs per producer. At 32 producers it records 256 submitted jobs, 11 final adaptive permits, 234 limiter rejections, 750 cache hits, 18 cache misses, approximately 0.172 ms verifier-service p95, approximately 0.353 ms end-to-end p95, and approximately 1,083.9 intent submissions per second. These are local sanitized measurements, not distributed production capacity.

## Memory questions

The benchmark retains one ticket per admitted item until the collection loop drains it, so the end-to-end allocation profile includes ticket/channel bookkeeping, cloned intent payloads, acknowledgements, cache-key strings, bounded metric samples, filesystem staging buffers, and Rust allocator reuse. The first measurements must distinguish process RSS/high-water from live heap retention and must capture peak thread count, file descriptors, and benchmark wall time. Rust has no tracing GC in this path; “GC pressure” is therefore interpreted as allocator churn, temporary allocation rate, and reclamation/retention behavior rather than a garbage-collector pause.

## Phase 47 proposal

Design a distributed multi-region lease migration protocol that treats lease movement as a fenced, signed, quorum-authorized transfer rather than a local owner swap. The source region must enter a drain state, publish a signed migration intent bound to resource, source/destination regions, current ownership epoch, record hash, migration nonce, and expiry. A distinct regional witness quorum must acknowledge the intent. The destination may prepare but cannot activate until it receives a signed source release, a higher ownership epoch, a fresh destination lease, and quorum evidence that excludes the source’s active epoch. The source must reject new mutations after drain, and the destination must reject stale, replayed, misbound, or incomplete evidence.

## Safety targets

The protocol must preserve single-active-region ownership, monotonic ownership epochs, one migration decision per nonce, source fencing before destination activation, distinct witness quorum, hash-bound state transfer, bounded evidence, atomic durable migration state, and fail-closed recovery after restart or partition. It must explicitly distinguish local protocol simulation from real cloud lease controllers, process termination, network partitions, independent failure domains, and storage authority.

## Measured sustained-concurrency profile

The sanitized workload ran 64 hot-key CAS intents per producer at 32, 64, and 96 producer threads. Peak RSS rose from 6,660 KiB at 32 producers to 10,228 KiB at 96 producers, a 3,396 KiB increase and 1.51× factor. RSS high-water rose from 6,860 KiB to 10,300 KiB. Peak virtual-memory reservation rose from 177,704 KiB to 310,648 KiB, a 1.75× factor, while peak threads rose from 52 to 116. The chart is saved as `benchmarks/phase47_memory_profile.png`; machine-readable analysis is saved as `benchmarks/phase47_memory_profile_analysis.json`.

The hot-key cache remained highly effective, with a measured 0.9994 hit ratio at 96 producers. The main pressure signal was not resident-memory exhaustion but bounded admission retry/bookkeeping: 171.29 limiter retries per submitted job at 96 producers. The workload intentionally reused one CAS generation, so 6,143 expected conflict outcomes after the first valid commit are not treated as crashes or allocator failures. Because Rust has no tracing garbage collector in this path, “GC pressure” is reported as allocator churn and retention proxies: RSS/high-water, virtual-memory reservation, thread growth, temporary retry objects, cloned intents, and bounded queue/cache/metric state. The benchmark does not expose allocation counts or fragmentation; an allocator-instrumented run would be required for that claim.
