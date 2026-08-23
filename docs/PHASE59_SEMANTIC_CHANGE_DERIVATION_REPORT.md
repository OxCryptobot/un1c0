# Phase 59: deterministic semantic change derivation

## Executive summary

Phase 59 removes a trust gap in the Phase 58 session API. Instead of treating a caller-provided changed-function set as authoritative, the session now derives changes from the Phase 55 per-function fingerprint vector under its fixed target profile. The explicit refresh API rejects any declared set that differs from the derived set and clears the session. `refresh_auto` uses the derived set directly. An unchanged UEG produces a zero-work refresh that preserves the valid snapshot, while blocking parser diagnostics invalidate the session even when the semantic node fingerprints are unchanged.

The implementation is intentionally local and bounded. Structural function-name/order changes remain a hard boundary. The dependency-aware validator still controls reverse-dependent closure, per-function cache reuse, and semantic diagnostics. Phase 59 only improves the integrity of the change-set boundary; it does not grant filesystem, process, network, secret, cluster, signing, or deployment authority.

## Implementation

The implementation is in [`src/semantic_session.rs`](../src/semantic_session.rs). The new `SemanticChangeSet` carries exact changed and unchanged indexes, previous/current function counts, and previous/current root keys:

```rust
pub struct SemanticChangeSet {
    pub changed_functions: BTreeSet<usize>,
    pub unchanged_functions: BTreeSet<usize>,
    pub previous_function_count: usize,
    pub current_function_count: usize,
    pub previous_root: SemanticCacheKey,
    pub current_root: SemanticCacheKey,
}
```

`derive_change_set` first checks profile identity, blocking UEG diagnostics, non-empty function structure, unique function graph shape, and fingerprint vector length. It then compares each per-function key in order. Any error invalidates the session. `refresh_auto` feeds the derived set into the existing Phase 58 refresh path.

The explicit `refresh` path derives the current set before semantic validation. If the caller declares `{1}` while the fingerprint shows `{0}`, it returns `ChangedSetMismatch` and clears the snapshot. This prevents an incomplete declaration from suppressing changed-function validation or an overbroad declaration from causing nondeterministic work.

The no-op path is also fail closed. It returns zero affected functions, zero revalidated functions, and zero cache hits/misses only when the current root is identical and there are no blocking UEG diagnostics. A parser error cannot hide behind unchanged semantic nodes.

## Security properties

Phase 55 per-function digests remain the source of truth for changed-input detection. A changed function receives a new function key, which changes the composed root. The Phase 59 derived set is computed from the key vector itself, not from source-text heuristics or a caller assertion. The profile key remains fixed by the session and is checked before derivation.

The implementation preserves the fail-closed chain:

> profile binding → structural graph binding → blocking-diagnostic rejection → per-function digest comparison → exact declared-set equality → dependency-aware semantic validation → valid snapshot capture → pre-emitter snapshot verification

This ordering prevents a stale valid snapshot from surviving structural edits, parser errors, profile drift, or incomplete caller evidence. It also means that an unchanged root is not treated as a general compiler certificate; it is only a local semantic-session equality result under the exact profile and UEG shape.

## Regression evidence

The Phase 59 integration suite is [`tests/phase59_semantic_change_derivation_integration.rs`](../tests/phase59_semantic_change_derivation_integration.rs). It contains five tests covering exact leaf derivation and transitive caller refresh, declared-set mismatch invalidation, unchanged zero-work refresh, blocking-diagnostic invalidation under an unchanged fingerprint, and structural invalidation. The suite passed **5/5 tests**.

The prior Phase 58 suite remains the compatibility gate and continues to pass **4/4 tests**. The full workspace suite was previously run successfully after Phase 58; Phase 59 should rerun the same complete suite before publication.

## Benchmark results

The benchmark source is [`examples/phase59_semantic_change_derivation_benchmark.rs`](../examples/phase59_semantic_change_derivation_benchmark.rs), the sanitized artifact is [`benchmarks/phase59_semantic_change_derivation.json`](../benchmarks/phase59_semantic_change_derivation.json), and the chart is [`benchmarks/phase59_semantic_change_derivation.png`](../benchmarks/phase59_semantic_change_derivation.png). Each row uses 64 samples, zero refresh errors, `cluster_mutation_performed: false`, and `secret_material_recorded: false`.

The following Rust rows are measured in nanoseconds:

| Functions | Full capture p50/p95/p99 | Derive p50/p95/p99 | Auto-refresh p50/p95/p99 | Changed | Unchanged | Affected | Revalidated |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 33,612 / 57,588 / 66,139 | 34,158 / 40,392 / 57,362 | 140,319 / 207,883 / 305,360 | 1 | 0 | 1 | 1 |
| 2 | 53,137 / 79,307 / 97,612 | 55,082 / 82,399 / 89,880 | 232,180 / 522,471 / 592,153 | 1 | 1 | 2 | 1 |
| 4 | 90,506 / 131,573 / 264,575 | 93,361 / 121,611 / 122,345 | 411,386 / 880,819 / 898,936 | 1 | 3 | 4 | 1 |
| 8 | 164,898 / 191,420 / 193,497 | 204,528 / 378,663 / 545,483 | 719,744 / 846,154 / 953,408 | 1 | 7 | 8 | 1 |
| 16 | 326,156 / 435,985 / 447,535 | 345,058 / 411,574 / 440,797 | 1,376,474 / 1,523,950 / 1,767,231 | 1 | 15 | 16 | 1 |
| 32 | 644,265 / 708,287 / 784,265 | 691,147 / 1,040,536 / 1,122,347 | 2,667,889 / 2,863,497 / 2,953,659 | 1 | 31 | 32 | 1 |

The call-chain fixture intentionally makes the changed leaf affect every caller, so auto-refresh is slower than full capture in this local microbenchmark. The useful Phase 59 result is not an unsupported speedup claim: it is exact derivation, one revalidated function, conservative affected closure, and zero stale-evidence acceptance. A shallow dependency cone or a UEG with many unrelated functions should be measured separately before making a performance claim.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase59_semantic_change_derivation_integration -- --nocapture
cargo test --test phase58_semantic_session_integration -- --nocapture
cargo run --example phase59_semantic_change_derivation_benchmark > benchmarks/phase59_semantic_change_derivation.json
python3 -m json.tool benchmarks/phase59_semantic_change_derivation.json >/dev/null
python3 scripts/analyze_phase59_semantic_change_derivation.py
```

## Boundaries and next phase

Phase 59 does not infer semantic changes from arbitrary text diffs, serialize sessions across processes, merge concurrent edits, or provide a proof of target-emitter correctness. The next safe extension is a typed parser-to-session edit manifest that maps source spans to function indexes and refuses ambiguous mappings, while retaining fingerprint equality as the final authority.
