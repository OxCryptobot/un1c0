# Phase 62: versioned semantic-batch envelopes

## Executive summary

Phase 62 gives atomic multi-file semantic refreshes an explicit local sequence boundary. `SemanticBatchEnvelope` binds a batch to a non-zero batch ID, the exact profile key, and a typed `SemanticEditBatch`. `SemanticBatchSession::refresh_envelope` accepts only the next expected ID. Replays, gaps, profile-key mismatches, and any per-unit semantic failure invalidate all unit sessions.

The envelope is deliberately an evidence container, not a transport credential or distributed commit certificate. It adds deterministic replay and ordering checks without adding filesystem, process, network, secret, signing, or cluster authority.

## Implementation

The implementation extends [`src/semantic_batch.rs`](../src/semantic_batch.rs). `SemanticBatchSession` now stores `profile_key` and `next_batch_id`, initialized to 1. `SemanticBatchEnvelope::new` rejects zero IDs and stores the exact profile key plus a cloneable batch. `refresh_envelope` checks the profile key and sequence before delegating to Phase 61 staged all-or-nothing refresh. The sequence advances only after the delegated batch succeeds.

The acceptance rule is:

> `envelope.profile_key == session.profile_key` and `envelope.batch_id == session.next_batch_id`

Otherwise the session is invalidated. This is intentionally strict: the local session does not infer missing batches, reorder envelopes, or permit replay of an already applied batch.

## Security and correctness

Phase 62 closes a state-replay boundary that Phase 61 did not represent explicitly. A duplicate envelope cannot be accepted after a successful refresh because the expected ID advances. A skipped envelope cannot jump the session state because the expected ID is exact. A profile key from a different target capability profile cannot be applied to the session. A failed unit still invalidates the whole session because the Phase 61 atomicity rule remains underneath the envelope layer.

The envelope does not prove that an edit is authorized. Phase 60 root/profile-bound edit manifests and Phase 59 fingerprint-derived changes remain the semantic authority. The envelope only sequences that already-validated local evidence.

## Test evidence

[`tests/phase62_semantic_batch_envelope_integration.rs`](../tests/phase62_semantic_batch_envelope_integration.rs) passed **3/3 tests**. It covers successful ID 1 acceptance and advancement, replay and sequence-gap invalidation, profile-key mismatch, and zero-ID rejection. Phase 61 batch tests passed **3/3**, while Phase 58–60 compatibility tests remain green.

## Benchmark results

The benchmark source is [`examples/phase62_semantic_batch_envelope_benchmark.rs`](../examples/phase62_semantic_batch_envelope_benchmark.rs), with sanitized rows in [`benchmarks/phase62_semantic_batch_envelope.json`](../benchmarks/phase62_semantic_batch_envelope.json). Each row contains 64 samples, zero errors, `cluster_mutation_performed: false`, and `secret_material_recorded: false`.

| Units | Total functions | Raw batch p50/p95 | Envelope refresh p50/p95 | Envelope ID |
|---:|---:|---:|---:|---:|
| 1 | 8 | 2,924,901 / 3,852,765 ns | 2,852,749 / 3,229,459 ns | 1 |
| 2 | 16 | 5,718,671 / 8,088,412 ns | 5,565,591 / 6,841,243 ns | 1 |
| 4 | 32 | 10,825,558 / 12,034,248 ns | 10,586,719 / 11,146,227 ns | 1 |
| 8 | 64 | 20,791,538 / 22,917,260 ns | 20,202,249 / 20,692,169 ns | 1 |

The envelope path is not presented as a general optimization; the observed difference is within a local microbenchmark whose dominant cost is fresh session construction and dependency-aware refresh. The evidence demonstrates that sequence/profile checks remain bounded around the existing atomic batch path.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase62_semantic_batch_envelope_integration -- --nocapture
cargo run --example phase62_semantic_batch_envelope_benchmark > benchmarks/phase62_semantic_batch_envelope.json
python3 -m json.tool benchmarks/phase62_semantic_batch_envelope.json >/dev/null
```

## Next boundary

Phase 62 does not add persistence, signatures, transport, or distributed ordering. The next safe phase is a typed versioned snapshot envelope that records each unit's current root and profile keys, allowing a consumer to verify that an entire batch view corresponds to one exact semantic state before emission.
