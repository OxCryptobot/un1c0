# Phase 63: typed multi-unit semantic snapshot envelopes

## Executive summary

Phase 63 packages the result of a successful Phase 62 semantic batch into a typed local snapshot envelope. The envelope records the applied batch ID, the fixed profile key, and the exact root key for every UEG unit. A consumer can verify a candidate unit map only when the batch ID, unit set, profile keys, and root keys all match.

This creates a precise pre-emitter boundary for multi-file semantic state without adding transport or persistence authority. The envelope is evidence for local verification, not a signature, network token, filesystem transaction, or distributed commit certificate.

## Implementation

The implementation is in [`src/semantic_snapshot_envelope.rs`](../src/semantic_snapshot_envelope.rs), exported by [`src/lib.rs`](../src/lib.rs). `SemanticSnapshotEnvelope::capture` accepts only a valid `SemanticBatchSession` and an ID for the most recently applied batch. It records each unit's profile and root key from the session's validated snapshot.

`verify_for` enforces the exact batch ID and exact unit identity set, then recomputes each candidate UEG's `SemanticFingerprint` under the supplied profile. It rejects profile-key drift and root-key drift for any unit. Missing, unexpected, empty, or invalidated state is never treated as equivalent.

The core acceptance rule is:

> applied batch ID + exact unit set + exact profile key + exact per-unit root keys

## Security and correctness

Phase 63 closes the gap between “a batch refresh succeeded” and “a later consumer can prove which multi-unit semantic state it is using.” A candidate cannot omit a unit, add an untracked unit, reuse a root under a different profile, or reuse a root after a UEG change. Capture also refuses a batch ID that has not yet been applied and refuses invalidated sessions.

The envelope remains local and bounded. It stores typed keys and identities only; it does not store source text, private keys, signatures, process handles, network addresses, filesystem authority, or cluster state.

## Test evidence

[`tests/phase63_semantic_snapshot_envelope_integration.rs`](../tests/phase63_semantic_snapshot_envelope_integration.rs) passed **3/3 tests**. Coverage includes successful exact-state capture and verification, batch-ID mismatch, empty and unexpected unit-set rejection, root drift rejection, and capture rejection before application or after invalidation.

## Benchmark results

The benchmark source is [`examples/phase63_semantic_snapshot_envelope_benchmark.rs`](../examples/phase63_semantic_snapshot_envelope_benchmark.rs), with sanitized data in [`benchmarks/phase63_snapshot_envelope.json`](../benchmarks/phase63_snapshot_envelope.json). Each row contains 64 samples, zero errors, `cluster_mutation_performed: false`, and `secret_material_recorded: false`.

| Units | Total functions | Capture p50/p95 | Verification p50/p95 | Batch ID |
|---:|---:|---:|---:|---:|
| 1 | 8 | 765 / 1,093 ns | 163,948 / 191,900 ns | 1 |
| 2 | 16 | 1,195 / 1,356 ns | 345,780 / 434,976 ns | 1 |
| 4 | 32 | 2,266 / 2,344 ns | 691,260 / 804,592 ns | 1 |
| 8 | 64 | 4,943 / 7,390 ns | 1,360,113 / 1,537,502 ns | 1 |

Capture is a compact map extraction. Verification is intentionally more expensive because it recomputes fingerprints for every candidate UEG and checks the full unit set. These are local sandbox measurements, not production capacity claims.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase63_semantic_snapshot_envelope_integration -- --nocapture
cargo run --example phase63_semantic_snapshot_envelope_benchmark > benchmarks/phase63_snapshot_envelope.json
python3 -m json.tool benchmarks/phase63_snapshot_envelope.json >/dev/null
```

## Next boundary

Phase 63 does not persist envelopes, sign them, transmit them, or apply them across processes. The next safe extension is an explicit versioned snapshot-consumer API that gates code generation on one verified multi-unit envelope and returns typed stale-state errors rather than allowing unchecked fallback.
