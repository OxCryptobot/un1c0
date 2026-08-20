# Phase 50 incremental code generation and emitter-binding plan

## Objective

Expose the typed multi-function UEG through a deterministic incremental generation pipeline. The generator must emit one function node at a time, preserve source order, support bounded sink delivery, and bind the same UEG contract to Rust, Go, Zig, and Python targets without claiming full target-compiler semantics.

## Contract

`TargetBinding` identifies Rust, Go, Zig, and Python. Each binding supplies a target label, optional file preamble, and a node renderer. `IncrementalCodeGenerator` owns a monotonic node cursor. `next_chunk` emits at most one `GeneratedChunk`; repeated calls are idempotent at end-of-input. `emit_remaining` sends only not-yet-emitted chunks to a caller-owned bounded sink and returns sanitized chunk/byte counters. Invalid UEG diagnostics are checked before emitter invocation, and unsupported statements fail closed.

Go and Zig emitters retain their existing target preambles; incremental generation strips those preambles from each node chunk and adds the preamble exactly once to the complete output. Rust emits function chunks directly. Python uses preserved original function source when available to avoid duplicate headers in multi-function incremental output, with the existing lowerer retained as a compatibility fallback.

## Benchmark matrix

Compare deterministic single-function and multi-function Python sources at 1, 2, 4, 8, 16, and 32 functions. Measure warm parser latency p50/p95/max, total and per-function parse latency, source bytes, parsed node count, and incremental generation latency for each target. Report local sandbox observations only; do not infer production capacity.

## Phase 49 memory-pressure review boundary

The latest published hot-key profile measured peak RSS 6,548/8,668/10,136 KiB and peak VmPeak 177,712/244,268/310,652 KiB at 32/64/96 producers. The profile’s 99.69–99.90% cache hit ratios indicate hot-key fact reuse, while limiter retries rose from 2,906 to 192,577 total and end-to-end p95 rose from 3,089 to 15,926 microseconds. These are process RSS, address-space, thread, retry, and latency proxies; Rust has no tracing GC in this path, and the fixture’s expected same-generation conflicts prevent interpreting it as valid-write throughput or proof of an allocator leak.

Phase 50 adds parser/codegen scaling evidence but does not claim allocator attribution. The next allocator boundary remains fixed-allocator mixed-workload profiling with allocation/free bytes, peak live bytes, fragmentation, cgroup events, cold/warm runs, unique-request traffic, valid sequential commits, conflict storms, forged evidence, and queue/cache correlation.

## Production boundaries

The incremental generator is a local deterministic pipeline. It does not own distributed scheduling, durable target artifacts, compiler execution, sandboxing, or target-specific type checking. Those responsibilities remain with the verification loop and deployment authority.
