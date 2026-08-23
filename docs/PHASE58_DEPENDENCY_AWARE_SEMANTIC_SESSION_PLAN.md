# Phase 58: dependency-aware semantic sessions

## Objective

Phase 56 introduced dependency closure and per-function report reuse. Phase 57 introduced exact local semantic snapshots. Phase 58 composes them into a dependency-aware semantic session that can refresh a valid snapshot after a localized UEG change while preserving exact profile/root binding and fail-closed invalidation.

## Contract

A `DependencyAwareSemanticSession` owns a target capability profile, a bounded `DependencyAwareSemanticValidator`, the current UEG fingerprint, and an optional valid `SemanticValidationSnapshot`. `start` validates the complete UEG and captures a valid snapshot. `refresh` receives a new UEG and an explicit changed-function set, validates the dependency-aware affected closure, and creates a new snapshot only when the aggregated report is valid.

The session never treats a digest as a proof. The fingerprint must have the same function shape as the UEG, the dependency graph must reject duplicates and invalid indexes, every cache miss must execute the Phase 53 per-function validator, and invalid reports cannot produce snapshots. A profile or target change requires a new session; it cannot be smuggled through refresh.

## Milestones

| Milestone | Outcome | Evidence |
|---|---|---|
| 58.1 | Session lifecycle | Start and refresh produce typed valid snapshots |
| 58.2 | Dependency-aware refresh | Changed function and reverse callers are revalidated; unrelated functions reuse reports |
| 58.3 | Exact snapshot binding | New snapshot verifies only against its UEG/profile fingerprint |
| 58.4 | Fail closed | Invalid changed UEG, duplicate functions, shape mismatch, and bad indexes reject refresh |
| 58.5 | Performance evidence | Full snapshot capture versus warmed dependency-aware refresh at 1–32 functions |

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Session identity | Target/profile are fixed for the session |
| Dependency closure | Changed leaf reaches transitive reverse callers |
| Reuse | Unchanged affected functions hit the per-function report cache |
| Invalidation | Changed function misses and revalidates |
| Snapshot validity | Only zero-error aggregate reports produce snapshots |
| Staleness | A snapshot rejects changed UEG roots and profile mismatch |
| Bounds | Cache capacity and changed-index traversal remain bounded |
| Authority | No filesystem, process, network, secret, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8/16/32-function call chains, Rust/Go/Zig/Python profiles, and 64 samples per row. Warm the base session, measure full `SemanticValidationSnapshot::capture` on the changed UEG, then measure `refresh` with a changed leaf. Record p50/p95/p99, affected count, revalidated count, cache hits/misses, diagnostics, and sanitized mutation/secret markers. Do not claim production compiler throughput or distributed cache coherence.
