# Phase 68: bounded local diagnostic comparison

## Objective

Phase 67 added bounded typed diagnostic reports over verified emission aggregates. Phase 68 adds a local comparison boundary for two reports so callers can inspect observation, chunk, byte, and digest deltas without treating the comparison as distributed trust or authorization.

## Contract

`EmissionDiagnosticComparison::compare` verifies both the `before` and `after` reports against the same current `SemanticSnapshotEnvelope`, target profile, and complete candidate-unit map before calculating any delta. It then requires matching target, batch ID, profile key, and unit-root context.

The returned `EmissionDiagnosticDelta` contains signed integer deltas for observations, chunks, and bytes, plus a boolean output-digest equality result. The comparison is descriptive only: it does not infer quorum, consensus, causality, freshness, authorization, or permission to act.

## Fail-closed rules

A stale report or profile/target mismatch returns a typed error before any result is produced. An internally inconsistent context is rejected even if both inputs are structurally valid. No comparison path bypasses Phase 67 entry-bound validation or Phase 66 current-envelope re-verification.

## Benchmark method

Use the deterministic four-unit, 32-function Rust fixture from Phase 67. Compare a one-observation report with 1/2/4/8-observation reports. Record 64 samples per row, p50/p95 nanoseconds, signed deltas, digest equality, errors, and sanitized authority markers. The benchmark must remain local, read-only, and in-process.
