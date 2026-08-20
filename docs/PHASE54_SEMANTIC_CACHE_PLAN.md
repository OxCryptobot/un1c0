# Phase 54: bounded semantic-validation cache and performance contracts

## Objective

Phase 53 established deterministic cross-language symbol validation and target capability enforcement. Phase 54 reduces repeated validation overhead in incremental generation without weakening fail-closed semantics. It introduces a bounded, content-addressed cache keyed by the typed UEG structure and target capability profile. Cache hits return cloned validation reports; misses compute the same validator path as the uncached implementation.

The cache is an optimization boundary, not an authority boundary. It does not cache generated code, bypass UEG validity checks, or authorize unsupported target features. Every cache entry remains target/profile-specific and is invalidated by a changed UEG fingerprint or capability profile fingerprint.

## Architectural milestones

| Milestone | Outcome | Required evidence |
|---|---|---|
| 54.1 | Deterministic UEG/profile fingerprints | Same typed UEG and profile yield identical keys; changed spans, sources, symbols, or profile flags miss |
| 54.2 | Bounded thread-safe semantic cache | Capacity validation, hit/miss/eviction metrics, no unbounded growth, concurrent lookup tests |
| 54.3 | Incremental generator cache integration | Cached preflight preserves exact Phase 53 diagnostics and still blocks emitters on errors |
| 54.4 | Warm/cold performance benchmark | p50/p95/p99 nanoseconds, hit ratio, evictions, valid samples, and zero mutation/secret markers |
| 54.5 | Operational boundary documentation | Explicit cache size, memory, staleness, and production-scaling limitations |

## Cache contract

`SemanticValidationCache` has a positive fixed capacity, content-addressed keys, clone-on-read reports, and bounded LRU-style eviction. Metrics include capacity, entries, hits, misses, insertions, and evictions. A cache key includes a SHA-256 digest of all typed UEG function names, parameters, spans, statement sources/kinds, expression sources/kinds/spans, and the target capability profile. The digest is an optimization key only; it is not a signing or authority token.

Cache poisoning is limited by deriving keys from the actual typed UEG and profile object rather than caller-provided labels. A changed UEG or target profile cannot reuse an old report. Reports remain immutable after insertion. A poisoned mutex is recovered without exposing authority or suppressing validation errors.

## Code-generation contract

`IncrementalCodeGenerator::with_semantic_cache` enables cached validation. The uncached constructor remains behaviorally identical. The generation path always runs the existing UEG error gate, then cached or uncached semantic validation, then target rendering. Semantic errors return `GenerationError::SemanticValidation` and no emitter or sink callback runs.

## Performance metrics

Benchmark 1/2/4/8/16/32-function deterministic fixtures across Rust, Go, Zig, and Python profiles. Measure uncached and warmed-cache validation p50/p95/p99/min/max in nanoseconds, expression count, diagnostics, valid samples, cache hits/misses/evictions, and cache capacity. Report speedup only as a local optimization comparison; do not infer production scalability or compiler throughput.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Key stability | Equal typed UEG/profile inputs produce equal fingerprints |
| Key separation | Different target/profile/typed source/span inputs do not collide in tested cases |
| Bounds | Capacity and entry count never exceed configured capacity |
| Eviction | Oldest entry is evicted deterministically after capacity is exceeded |
| Concurrency | Concurrent validation returns equivalent reports and bounded metrics |
| Fail closed | Cached invalid reports still block all target emitters and pooled output |
| Regression | Phase 48–53 suites remain green |
| Safety | No filesystem, process, network, secret, or cluster mutation |

## Explicit non-goals

Phase 54 does not add distributed cache coherence, persistent cache files, remote cache trust, code generation caching, type inference, runtime execution, or authority grants. Compliance metadata remains at 209 gates.
