# Phase 56 dependency-aware incremental semantic validation report

## Executive summary

Phase 56 adds a deterministic typed-UEG dependency graph and a bounded per-function semantic-report cache. Function references are extracted from typed identifier expressions, direct dependency edges are inverted into reverse caller edges, and a changed function invalidates itself plus its transitive callers. Unrelated functions remain outside the affected closure. On a cache miss, the existing Phase 53 validator still runs for the complete affected function; a fingerprint never substitutes for semantic validation.

The benchmark used 1/2/4/8/16/32-function call chains, four target profiles, and 64 samples per row. It warmed the base function reports, measured full semantic validation on a changed UEG, and then measured warm dependency-aware updates. At 32 functions, the first changed leaf caused **1 report miss/revalidation and 31 report hits** across an affected closure of 32 functions. However, the local total warm-update p50 was **9.934–15.260 µs**, versus **5.631–8.757 µs** for full validation. This is not a regression claim: the fixture validator is intentionally tiny, so graph traversal and cache lookup overhead dominate. The evidence proves reuse and conservative invalidation, not production throughput improvement.

## Implementation

`DependencyGraph::from_ueg` assigns unique source-order indexes to function names, scans typed statement/expression trees for references to declared UEG functions, and builds both direct `dependencies` and reverse `dependents` sets. Duplicate function names and out-of-range indexes fail closed. `affected_by_changed` uses a bounded breadth-first traversal over reverse edges.

`validate_function_with_profile` reuses the Phase 53 validation rules for one lambda: duplicate parameter detection, source-order local scopes, user/builtin name resolution, target capability enforcement, exact spans, and deterministic diagnostic sorting. `DependencyAwareSemanticValidator` keys cloned function reports by `(profile_key, function_key)`, applies the affected closure, reuses unchanged reports, and revalidates misses. Invalid UEG diagnostics and fingerprint-shape mismatches are rejected before cache access.

## Benchmark results

Values are ranges across Rust, Go, Zig, and Python target rows; each row contains 64 samples. Units are nanoseconds unless shown otherwise.

| Functions | Expressions | Full p50 | Warm incremental p50 | Full p95 | Warm incremental p95 | Affected closure | First-change misses/hits |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 3 | 202–216 ns | 317–332 ns | 233–246 ns | 421–805 ns | 1 | 1 / 0 |
| 2 | 6 | 335–343 ns | 566–578 ns | 378–398 ns | 702–808 ns | 2 | 1 / 1 |
| 4 | 12 | 608–898 ns | 1.122–1.709 µs | 666–1.079 µs | 1.720–2.201 µs | 4 | 1 / 3 |
| 8 | 24 | 1.309–1.949 µs | 2.209–2.301 µs | 1.388–2.324 µs | 2.367–2.975 µs | 8 | 1 / 7 |
| 16 | 48 | 2.674–2.767 µs | 4.657–4.692 µs | 2.791–3.251 µs | 4.905–6.838 µs | 16 | 1 / 15 |
| 32 | 96 | 5.631–8.757 µs | 9.934–15.260 µs | 6.339–15.295 µs | 10.319–19.727 µs | 32 | 1 / 31 |

Across the 24 rows, all changed reports contained zero diagnostics, and every row recorded no secret material or cluster mutation. The highest observed p99 was 33.435 µs for full validation or 33.435 µs among the measured target rows; p99 remains sensitive to sandbox scheduling and should not be generalized beyond this run. The raw artifact is `benchmarks/phase56_dependency_incremental_validation.json`, and the chart is `benchmarks/phase56_dependency_incremental_validation.png`.

## Security and correctness properties

The dependency graph is an optimization index, not an authority boundary. A changed input is still fingerprinted, the UEG is checked for blocking diagnostics, the target profile remains explicit, and every cache miss executes the typed semantic validator. Function reports are cloned on read, bounded by a configured capacity, and keyed by exact profile/function digests. A stale or wrong-shaped fingerprint cannot be used to validate a different UEG.

The integration suite verifies transitive reverse callers, unrelated-function exclusion, duplicate rejection, fingerprint-shape rejection, changed-leaf undefined-name diagnostics, one first-change miss with unchanged caller hits, and stable report aggregation.

## Explicit boundaries

Phase 56 does not claim faster total latency for tiny local fixtures, dependency-aware type inference, semantic ABI checking, persistent cache storage, distributed coherence, remote trust, runtime execution, or any new filesystem/process/network/secret/cluster authority. Phase 57 builds on this boundary by adding a typed local snapshot that binds a valid report to exact UEG/profile fingerprints before emission.
