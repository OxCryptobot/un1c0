# Phase 51 optimizer and lock-free buffer-pool plan

## Baseline observations

The Phase 51 high-concurrency baseline extends the published hot-key CAS profile to 128, 160, and 192 producers. At 128/160/192 producers, peak RSS measured 11,332/12,852/13,964 KiB, VmPeak measured 377,044/443,600/509,976 KiB, and peak threads measured 148/180/212. Limiter retries reached 2,715,065/26,512,060/26,538,656, or 331.43/2,589.07/2,159.72 retries per job. Each run intentionally had one successful commit and `jobs - 1` expected same-generation conflicts.

The first bottleneck is thread and virtual-address growth: every producer is a native thread, and the verifier keeps up to 16 workers plus coordinator/sampler threads. The second bottleneck is admission retry amplification: the bounded limiter protects the worker queue but causes large producer-side retry loops and associated intent cloning/scheduling. The third bottleneck is contention-tail variability: 128 producers reached 106,964 microseconds end-to-end p95 in this run, while 160/192 were lower due to run-order and scheduler variance; this is not a monotonic capacity curve. RSS grows more slowly than VmPeak, so resident pressure is not attributable to the retry counter alone.

## Lock-free buffer-pool contract

Use a bounded `crossbeam_queue::ArrayQueue<Vec<u8>>` as an MPMC lock-free pool. `checkout` first reuses a pooled buffer, clears it, and otherwise allocates one buffer with the configured capacity. A returned buffer is retained only when its capacity is within the configured bound; oversized buffers are dropped so the pool cannot retain unbounded memory. `PooledBuffer` returns its buffer on drop, and pool metrics count checkouts, reuse, fresh allocations, returns, dropped oversize buffers, and current availability with atomics.

The pool is intended for transient code-generation output and diagnostic serialization buffers. Phase 51 integrates it into the incremental generator’s aggregate output path and exposes a caller-owned API; it does not replace durable CAS intent ownership, cryptographic evidence, or the admission limiter. The pool is a bounded allocation-reuse mechanism, not a proof of allocator lock freedom for the entire process.

## Cross-target dead-code elimination

Implement a target-neutral `OptimizerPipeline` over the typed UEG. Explicit entry points define roots; call-like identifiers in typed expressions form directed edges to known UEG functions. Unreachable nodes are removed in source order. Preserve-all is the conservative default when no roots are specified. Unknown calls are treated as external symbols and do not cause deletion of the containing function. Invalid UEGs and unknown explicit roots fail closed before mutation.

Optimizer hooks run before and after optimization and can reject the pipeline with a typed error. Optimized UEGs feed the same Rust/Go/Zig/Python incremental emitter, ensuring dead-code decisions are cross-target and not duplicated in individual emitters.

## Measurement boundary

The high-concurrency sampler reports RSS, VmHWM, VmPeak, thread count, retries, cache reuse, and latency tails. It does not identify allocation call sites, fragmentation, cgroup pressure, or GC pauses; Rust has no tracing GC in this path. The expected-conflict fixture is useful for admission and fencing pressure, but not valid-write throughput. Phase 51 therefore reports buffer-pool hit/miss/drop counters separately and preserves a follow-up boundary for allocator instrumentation, cgroup events, unique-request traffic, mixed valid/conflict traffic, and producer worker-pool redesign.
