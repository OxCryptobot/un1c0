# Phase 56: dependency-aware incremental semantic validation

## Objective

Phase 55 reduced semantic-cache fingerprint cost by composing stable per-function digests. Phase 56 uses the typed UEG call references to build a deterministic function dependency graph and reuse unchanged per-function semantic reports. A changed function invalidates itself and the transitive reverse-dependent closure; unrelated functions are not admitted into the affected set.

## Architecture

`DependencyGraph` maps unique function names to source-order indexes, records direct dependencies from typed identifier references, and builds reverse `dependents` sets. `affected_by_changed` performs a bounded breadth-first traversal over reverse edges. Duplicate function names and out-of-range indexes fail closed.

`validate_function_with_profile` validates one function using the complete UEG function namespace and the existing Phase 53 symbol/capability rules. It preserves source-order local definitions, exact diagnostic spans, and deterministic diagnostic ordering.

`DependencyAwareSemanticValidator` owns a bounded LRU-like per-function report cache keyed by `(profile_key, function_key)`. On a changed-input request it computes the affected closure, looks up each affected function, and executes full per-function validation only on misses. It aggregates cloned reports in deterministic order. Fingerprints and report caches never bypass invalid-UEG rejection or target capability enforcement.

## Milestones

| Milestone | Outcome | Evidence |
|---|---|---|
| 56.1 | Deterministic typed call dependency graph | Direct dependencies, reverse callers, duplicate rejection |
| 56.2 | Conservative invalidation closure | Changed leaf reaches all transitive callers; unrelated nodes excluded |
| 56.3 | Per-function report cache | Unchanged callers hit; changed function misses and revalidates |
| 56.4 | Fail-closed changed-input validation | Undefined names and invalid UEG diagnostics remain blocking |
| 56.5 | Performance evidence | Full validation versus warm dependency-aware updates at 1–32 functions |

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Identity | Unique function names; duplicate declarations rejected |
| Dependency extraction | Typed identifier references resolve only to declared UEG functions |
| Invalidation | Reverse-dependent closure includes changed node and transitive callers |
| Isolation | Unrelated functions remain outside the affected closure |
| Cache | Profile/function digest pair keys reports; reports are cloned on read |
| Fail closed | Invalid UEG and semantic errors remain blocking; no report-only bypass |
| Bounds | Indexes, cache capacity, and traversal state remain bounded |
| Determinism | Source-order indexes and diagnostic sorting are stable |
| Authority | No filesystem, process, network, secret, or cluster authority |

## Benchmark methodology

Use deterministic 1/2/4/8/16/32-function call chains, four target profiles, and 64 samples per row. Warm the per-function cache with the base UEG, measure full semantic validation on the changed UEG, then measure warm dependency-aware updates using a changed leaf. Record p50/p95/p99 nanoseconds, affected and revalidated counts, first-change cache hits/misses, diagnostics, and secret/mutation markers. The benchmark excludes parsing and fingerprint construction and does not claim production compiler throughput.
