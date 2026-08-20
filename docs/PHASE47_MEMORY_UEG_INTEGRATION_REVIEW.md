# Phase 47 memory-pressure and UEG integration review

## Scope and baseline

This review covers the published Phase 47 memory artifacts and the five preserved working-tree changes in `src/targets/go.rs`, `src/targets/zig.rs`, `src/types.rs`, `src/ueg_python.rs`, and `src/walker.rs`. The repository is at `709e6001ff348ee046fbfde8b9cacb6d3c1ae594`, and `origin/main` is synchronized with that commit. The working tree contains only the five named unstaged UEG files after build-artifact cleanup.

The five files were compared against `HEAD` after formatting both versions with the stable Rust formatter. Every file is **semantic-equivalent after rustfmt**. Therefore, the unstaged changes are formatting normalization, not uncommitted behavioral work. Their underlying UEG behavior is already present in the published main branch. This distinction is important: integrating them as a semantic feature would create a misleading commit boundary and would not add runtime capability.

## Allocator-pressure proxies

The Phase 47 memory benchmark creates one bounded `OwnershipBoundCasCoordinator`, wraps it in adaptive admission with at most 16 verifier workers and a queue capacity of 128, starts 32/64/96 producer threads, and submits 64 intents per producer. Every producer clones the same hot-key intent fixture. A separate sampler reads `/proc/self/status` every 10 ms and retains maxima for `VmRSS`, `VmHWM`, `VmPeak`, and `Threads`. Atomic counters collect limiter retries, successes, and failures; verifier metrics collect queue wait, verification service, mutation service, end-to-end samples, and cache hits/misses.

| Proxy | What it measures | What it can indicate | What it cannot prove |
|---|---|---|---|
| `VmRSS` peak | Resident pages observed by the process | Working-set growth and retained resident pressure | Which subsystem allocated the pages, fragmentation, cgroup reclaim, or leak causality |
| `VmHWM` peak | Kernel-recorded resident high-water mark | A conservative historical resident ceiling | Allocation lifetime, allocator-bin reuse, or whether the peak was a short-lived burst |
| `VmPeak` peak | Maximum virtual address-space size | Thread-stack reservations, allocator arenas, mappings, and address-space expansion | Physical memory consumption or committed pages |
| `Threads` peak | Concurrent OS threads | Stack/control-block overhead and scheduler pressure | Per-thread stack size, CPU saturation, or thread leaks after teardown |
| Limiter retries | Typed `Limited` admission failures retried by producers | Admission contention and retry/bookkeeping pressure | Memory leakage, cryptographic failure, or protocol failure |
| Cache hits/misses | Reuse of exact context-bound verification facts | Cryptographic work avoided for repeated hot-key facts | General cache effectiveness for unique hashes or mixed workloads |
| p95 latency | Queue/service/mutation/end-to-end tail behavior | Contention, scheduling, and bounded pipeline pressure | Memory causality or production capacity |

The 32-to-96 producer rows show peak RSS increasing from **6,548 KiB to 10,136 KiB**, a **3,588 KiB** increase and **1.55×** factor. Peak `VmPeak` increases from **177,712 KiB to 310,652 KiB**, a **132,940 KiB** increase and **1.75×** factor. Peak threads increase from **52 to 116**, exactly **64 additional threads**, which tracks the producer-level increase and makes thread stacks/control structures a plausible contributor to virtual-memory growth. The resident series grows more slowly than the address-space series, so the measured pressure is not evidence that the process consumed the full virtual-memory reservation.

The `VmHWM - VmRSS` gap narrows from **224 KiB** at 32 producers to **168 KiB** at 96 producers. This is consistent with sampling and workload timing, but it is not a fragmentation measurement. The sampler may miss short-lived peaks despite the final sample, and `VmHWM` itself is a kernel high-water statistic rather than an allocator trace.

## Expected-conflict limitation

The fixture intentionally constructs every request against one ownership record and one logical CAS generation. The first valid intent can commit; subsequent intents still expect the old generation and therefore encounter the coordinator’s expected same-generation conflict path. The measured outcomes are exactly **1 successful commit and 2,047/4,095/6,143 failed outcomes** for 32/64/96 producers. Those counts are `jobs - 1`, which is the expected shape of the fixture, not evidence of crashes, memory corruption, or allocator failure.

This design is useful for stressing admission, verification cache reuse, queueing, ticket completion, conflict fencing, and bounded failure bookkeeping. It is not a valid durable-write throughput benchmark because almost all requests are intentionally rejected at the logical CAS conflict boundary. It also does not represent a mixed workload containing valid sequential commits, same-generation conflict storms, forged evidence, and unique-request verification.

The limiter retry count is the strongest pressure signal in the fixture: **2,906**, **23,514**, and **192,577** retries at 32/64/96 producers, or approximately **1.42**, **5.74**, and **31.34 retries per job**. The admission controller increments this counter only when `try_acquire` sees `in_flight >= permits`; the producer then sleeps 50 microseconds and retries. These retries are explicit backpressure behavior, not hidden allocation events. They imply increasing scheduling and coordination overhead at 96 producers, which aligns with end-to-end p95 increasing from **3,089 µs** to **15,926 µs**, but they do not establish a memory leak.

The verification-fact cache is highly effective for this hot-key fixture: cache hits/misses are 6,125/19, 12,269/19, and 18,414/18. The 96-producer hit ratio is **99.90%**. This result should not be generalized to unique-request traffic, where unique request hashes intentionally miss the cache and pay fresh signature/hash work. Cache reuse and allocator pressure are related only indirectly; the benchmark does not attribute bytes to cache entries or cryptographic objects.

## GC and allocator limitations

Rust has no tracing garbage collector in this execution path. Consequently, a GC-pressure claim would require a different runtime or explicit allocator instrumentation. The report correctly treats RSS/high-water, virtual-memory reservation, thread count, retry bookkeeping, cache behavior, and latency tails as **allocator-pressure proxies**, not GC pause or collection measurements.

The benchmark does not record allocation/free counts, allocated bytes, peak live bytes, allocator-bin occupancy, fragmentation, cgroup memory pressure, page faults, kernel reclaim, thread-stack sizes, or per-subsystem ownership of memory. The 10 ms sampling cadence can miss transient allocations. `VmPeak` can grow because of reservations and mappings without proportional physical commitment. p95 latency can worsen from queue contention even if memory remains stable. A follow-up should use a fixed allocator configuration and capture allocation/free counts and bytes, cold versus warm runs, unique and mixed workloads, queue depth, cache miss rate, and cgroup memory events.

## Review of the five UEG changes

| File | Current role | Diff after rustfmt normalization | Integration assessment |
|---|---|---|---|
| `src/targets/go.rs` | Emits Go scaffolds from UEG lambdas; maps normalized annotations, slices, options, and maps; rewrites simple bodies | No semantic difference | Safe to isolate as formatting-only cleanup. Future semantic work should add Go golden/compile tests rather than mix with this diff. |
| `src/targets/zig.rs` | Emits Zig scaffolds; maps normalized annotations and rewrites `let`, returns, assignments, ranges, and `println!` | No semantic difference | Same as Go. Current output is a heuristic scaffold, not a complete Zig semantic backend. |
| `src/types.rs` | Shared annotation parser and normalizer for primitive, tuple-like, and generic forms | No semantic difference | This is the shared contract and deserves a dedicated normalization matrix before any semantic backend changes. |
| `src/ueg_python.rs` | Lowers UEG back to Python, preferring preserved `orig_body` lines and falling back to heuristic body conversion | No semantic difference | Highest semantic-risk surface in the current architecture because round-trip behavior depends on indentation and header detection, even though this particular working-tree diff is formatting-only. |
| `src/walker.rs` | Parses a single Python function into UEG, captures decorators/comments/original lines, creates a JSON-like fragment, and heuristically lowers body statements | No semantic difference | Highest integration priority for future semantic work: parser boundaries, multi-function behavior, tuple/range rewrites, and `orig_body` capture feed every downstream target. |

The current smoke tests are insufficient for semantic integration. `tests/ueg_roundtrip.rs` checks only that the output contains `def fib(`. `tests/emitted_sources.rs` writes Go/Zig files and checks only scaffold markers such as `func`, `package main`, `pub fn`, or `const std`. `tests/targets_scaffold.rs` checks only that outputs are non-empty. These tests passed on the preserved working tree, but they do not prove behavior preservation, valid target syntax, multi-function capture, decorator fidelity, nested generic normalization, or control-flow correctness.

## Recommended integration plan

### Step 1: Resolve the formatting-only state

Do not silently mix the five unstaged files into the next semantic architecture commit. The preferred choice is to restore them if no formatting-only commit is desired. If repository style requires normalized formatting, create a separate `chore(ueg): rustfmt translation helpers` commit containing only the five files, after the focused tests and full suite pass. This commit should state explicitly that it is behavior-neutral and should not alter compliance-gate counts.

### Step 2: Add a UEG contract test matrix

Before semantic backend work, add tests for `normalize_annotation` covering primitives, nested `Vec`/`List`, `Option`, `HashMap`/`Map`/`Dict`, tuple-like annotations, mixed delimiters, empty annotations, and malformed or unbalanced delimiters. Add parser tests for one function, multiple functions, decorators, comments, blank lines, annotations, return types, tuple assignment, `range`, `return`, `print`, and unsupported statements.

### Step 3: Strengthen round-trip and target golden tests

Replace scaffold-only assertions with exact or normalized golden fixtures. Python round-trip tests should verify decorators, function header, parameter annotations, indentation, blank lines, docstrings, and body semantics. Go and Zig tests should validate generated signatures, type mappings, loop boundaries, assignment declarations, return syntax, print conversion, and unsupported-statement markers. Run `gofmt` and `zig fmt` when available, and treat unavailable toolchains as an explicit evidence state rather than silently claiming compilation.

### Step 4: Separate syntax preservation from semantic lowering

Keep `orig_body` as a round-trip preservation field, but introduce a typed body representation for statements that are intended to lower semantically. Do not make raw string heuristics the authority for control flow. Preserve unsupported constructs as typed diagnostics or explicit TODO nodes with source spans. Make `ast_fragment` a typed, canonical serializable structure instead of an unescaped JSON-like string.

### Step 5: Define the next semantic phase

The next best-in-class UEG phase should be **Phase 48: typed multi-function UEG and target-contract hardening**. Its first slice should support multiple top-level functions, typed annotations, source spans, decorators/comments as metadata, and a normalized statement subset. Its gates should cover parser determinism, round-trip preservation, target output validity, unsupported syntax fail-closed behavior, and no mutation of unrelated source files. Only after those gates pass should Go/Zig target expansion or CLI wiring be attempted.

### Step 6: Commit and publish with bounded scope

Use one optional formatting-only commit for the five current files, followed by one semantic Phase 48 source commit and one metadata-only compliance commit if new gates are added. Preserve the five-file diff boundary, run rustfmt, focused UEG tests, all-target Rust tests, the complete compliance suite, `git diff --check`, and remote-head verification. Do not modify the Phase 47 compliance artifacts unless Phase 48 introduces and validates new gates.

## Decision

The five unstaged files are **not pending semantic integration**; they are formatting-only drift over already-published UEG behavior. The safe immediate action is to keep them out of the Phase 48 semantic commit. The high-value next implementation is not another formatting pass, but a typed UEG contract and regression matrix that converts the current heuristic translator into a measurable, fail-closed multi-function language subsystem.
