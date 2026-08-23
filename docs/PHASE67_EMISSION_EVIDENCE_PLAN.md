# Phase 67: bounded local emission evidence

## Objective

Phase 66 introduced deterministic aggregation for equivalent local emission receipts. Phase 67 adds a small evidence wrapper that makes the aggregate's integrity explicit without promoting repeated observations into distributed trust, quorum, or authorization.

## Contract

`EmissionEvidenceBundle::from_receipts` delegates construction to `EmissionReceiptAggregate::from_receipts`, so empty and divergent observations remain typed failures. It stores the validated aggregate plus a domain-separated SHA-256 digest over the aggregate's canonical fields: target, batch ID, profile key, sorted unit roots, emission statistics, output digest, and observation count.

`verify_for` first recomputes and compares the bundle digest, then delegates to the aggregate's existing current-envelope verifier. Verification therefore remains bound to the current `SemanticSnapshotEnvelope`, exact target/profile identity, complete unit set, current candidate UEG roots, and aggregate statistics. There is no unchecked or stale-state fallback.

## Fail-closed rules

The bundle rejects empty receipt input, divergent receipts, and any current-envelope or candidate-root mismatch through typed errors. A digest mismatch is also typed and aborts before any partial evidence is returned. The API exposes only bounded aggregate fields and a fixed-size digest; it does not include source text, prompts, private keys, signatures, filesystem paths, network metadata, or secret material.

The evidence digest is an integrity check, not an authorization token. Repetition remains an observation count only. This phase adds no persistence, signing, network, filesystem, process, cluster, quorum, trust, or mutation authority.

## Benchmark method

Use deterministic local Rust fixtures containing four units and eight functions per unit. Emit one valid receipt, clone it into 1/2/4/8 equivalent observations, and measure bundle construction separately from exact current-state verification using 64 samples per row. Record p50/p95 nanoseconds, emitted chunks, error count, and sanitized authority markers.
