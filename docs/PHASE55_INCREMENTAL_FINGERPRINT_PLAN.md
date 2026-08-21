# Phase 55: incremental semantic fingerprint composition

## Objective

Phase 54 showed that prepared semantic-cache hits are sub-microsecond while content fingerprint construction reaches roughly 313 microseconds p50 at 32 functions. Phase 55 reduces changed-input overhead by composing a root semantic key from stable per-profile and per-function digests.

The composer is an optimization boundary only. It does not skip semantic validation, authorize target features, persist trust, or share cache state remotely. A changed function digest changes the composed root key; unchanged function digests remain reusable. Function order and profile changes also change the root key.

## Architecture

`SemanticFingerprint` contains a target-capability profile digest, ordered per-function digests, and a composed root `SemanticCacheKey`. Function digests cover the function name, parameters and annotations, return annotation, exact function span, statement kinds/spans/sources, and recursively typed expression kinds/sources/spans. The profile digest covers the target binding, capability booleans, and ordered operator sets.

`SemanticFingerprint::replace_function` updates one function digest and recomposes the root key without walking the other functions. It is bounded by the existing function vector and returns a typed out-of-range error. The cache exposes the fingerprint seam and continues to validate the full UEG on a miss. A cached report is never accepted solely because a fingerprint was computed.

## Milestones

| Milestone | Outcome | Evidence |
|---|---|---|
| 55.1 | Stable profile/function/root digest contract | Equal inputs produce equal digests; changed function, order, span, or profile changes root key |
| 55.2 | In-place one-function update seam | Unchanged function digests remain equal; changed digest and root key differ |
| 55.3 | Cache integration | Changed root keys miss and re-run semantic validation; warm identical keys hit |
| 55.4 | Performance evidence | Full recomposition versus one-function update p50/p95/p99 at 1–32 functions |
| 55.5 | Reusable guidance and operational limits | Explicit cold/warm/changed-input and local-only boundaries |

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Profile separation | Target and capability changes alter the profile/root digest |
| Function separation | Changing one function changes only its function digest among unchanged peers |
| Root composition | Function order, count, and any function digest change the root key |
| Exact invalidation | Changed UEG validation misses a prior cache entry and returns the changed report |
| Warm reuse | Repeated identical fingerprint/report lookup hits without extra insertion |
| Bounds | Update index is checked; no vector growth or unbounded state occurs |
| Fail closed | Fingerprints never bypass UEG or semantic validation errors |
| Safety | No filesystem, process, network, secret, or cluster mutation |

## Benchmark methodology

Use deterministic 1/2/4/8/16/32-function fixtures, four targets, and 128 samples per row. Measure full fingerprint construction for a changed UEG and in-place replacement of one function digest in a precomputed composer. Report p50/p95/p99 nanoseconds, function count, cache hit/miss/eviction metrics, changed-input diagnostics, and secret/mutation markers. Do not infer production compiler throughput from local timings.

## Explicit non-goals

Phase 55 does not add persistent fingerprints, distributed cache coherence, remote cache trust, generated-code caching, type inference, runtime execution, or new authority. Compliance metadata remains at 209 gates.
