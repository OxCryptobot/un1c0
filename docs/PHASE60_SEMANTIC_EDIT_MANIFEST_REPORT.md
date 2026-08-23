# Phase 60: typed semantic edit manifests

## Executive summary

Phase 60 adds a typed bridge from source-byte edits to dependency-aware UEG refresh. A `SemanticEditManifest` binds edit ranges to the current session's exact profile and root keys. The session maps each range to one and only one function source span, derives actual semantic changes from fingerprints, and refuses to refresh when the edit manifest is ambiguous, stale, incomplete, or inconsistent with the new UEG.

The feature preserves the local-first authority boundary. It does not read files, execute processes, access networks, inspect secrets, sign proposals, or mutate clusters. It only accepts caller-provided ranges as untrusted evidence and confirms them against typed UEG spans and Phase 55 fingerprints before invoking Phase 56 validation and Phase 57/58 snapshot replacement.

## Implementation

The implementation is in [`src/semantic_session.rs`](../src/semantic_session.rs). `SemanticEditRange` validates byte order. `SemanticEditManifest` sorts ranges, rejects overlap, and stores `base_root`, `profile_key`, and the immutable range list. The session creates a manifest with `manifest_for_edits` while it still owns a valid fingerprint.

The critical path is `derive_edit_resolution`: it performs Phase 59 fingerprint derivation, verifies profile/root binding against the pre-edit session state, maps each range to `LambdaNode.source_span`, and requires exactly one match per range. It then requires all fingerprint-derived changed functions to be included in the mapped edit set. `refresh_from_edit_manifest` passes only the derived set into the existing dependency-aware refresh method.

Representative API shape:

```rust
pub struct SemanticEditManifest {
    base_root: SemanticCacheKey,
    profile_key: SemanticCacheKey,
    ranges: Vec<SemanticEditRange>,
}

pub fn refresh_from_edit_manifest(
    &mut self,
    ueg: &Ueg,
    profile: &TargetCapabilityProfile,
    manifest: &SemanticEditManifest,
) -> Result<DependencyAwareRefresh, SemanticSessionError>
```

## Security and correctness

The edit manifest is not trusted merely because it is well-formed. A reversed or overlapping range is rejected at construction. A manifest from another session or an earlier UEG is rejected by exact root binding. A profile mismatch is rejected before span mapping. A range with no matching function is unmapped; a range spanning multiple functions is ambiguous. A changed function outside the mapped set is rejected rather than silently validated through an incomplete edit declaration.

The fail-closed chain is:

> fixed profile → blocking-diagnostic rejection → structural graph binding → current root/profile binding → exact one-function span mapping → fingerprint-derived semantic-change completeness → dependency-aware validation → snapshot replacement

This preserves Phase 55's security property: per-function digests, not source-text claims, decide which semantic inputs changed. Phase 60 adds a typed source-edit explanation without weakening that final authority.

## Test evidence

The integration suite is [`tests/phase60_semantic_edit_manifest_integration.rs`](../tests/phase60_semantic_edit_manifest_integration.rs). It covers valid leaf mapping and conservative caller refresh, root/profile binding failure, ambiguous span rejection, changed-outside-manifest rejection, reversed-range rejection, and overlap rejection. The suite passed **4/4 tests**.

Phase 58 compatibility passed **4/4 tests**, and Phase 59 derivation passed **5/5 tests** after the Phase 60 changes. The reusable skill validator passed, `cargo fmt --all -- --check` passed, and the complete Rust `cargo test --all-targets` suite passed with status 0.

## Benchmark results

The benchmark source is [`examples/phase60_semantic_edit_manifest_benchmark.rs`](../examples/phase60_semantic_edit_manifest_benchmark.rs). The sanitized artifact is [`benchmarks/phase60_semantic_edit_manifest.json`](../benchmarks/phase60_semantic_edit_manifest.json), and the visual chart is [`benchmarks/phase60_semantic_edit_manifest.png`](../benchmarks/phase60_semantic_edit_manifest.png). Every row contains 64 samples, zero errors, `cluster_mutation_performed: false`, and `secret_material_recorded: false`.

The measured Rust rows are nanoseconds:

| Functions | Full capture p50/p95 | Manifest resolution p50/p95 | Manifest refresh p50/p95 | Changed | Mapped | Affected | Revalidated |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 34,321 / 64,880 | 35,863 / 48,872 | 230,111 / 304,139 | 1 | 1 | 1 | 1 |
| 2 | 53,645 / 72,086 | 56,604 / 78,382 | 368,448 / 450,202 | 1 | 1 | 2 | 1 |
| 4 | 90,991 / 132,353 | 97,948 / 134,270 | 621,620 / 779,071 | 1 | 1 | 4 | 1 |
| 8 | 170,800 / 194,430 | 179,479 / 208,343 | 1,155,483 / 1,382,237 | 1 | 1 | 8 | 1 |
| 16 | 338,881 / 381,307 | 358,323 / 403,783 | 2,292,780 / 3,111,125 | 1 | 1 | 16 | 1 |
| 32 | 643,630 / 720,110 | 688,064 / 918,000 | 4,635,962 / 5,819,048 | 1 | 1 | 32 | 1 |

The chart confirms that manifest resolution remains close to full capture while manifest-bound refresh is the dominant series in this worst-case call chain. The refresh measurement includes constructing a fresh session for each sample, so it is an end-to-end conservative path rather than a steady-state warm-session microbenchmark. These numbers are sandbox measurements and are not production capacity claims.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase60_semantic_edit_manifest_integration -- --nocapture
cargo test --test phase58_semantic_session_integration --test phase59_semantic_change_derivation_integration -- --nocapture
cargo run --example phase60_semantic_edit_manifest_benchmark > benchmarks/phase60_semantic_edit_manifest.json
python3 -m json.tool benchmarks/phase60_semantic_edit_manifest.json >/dev/null
python3 scripts/analyze_phase60_semantic_edit_manifest.py
```

## Boundary and next phase

Phase 60 does not parse editor protocol events, handle multi-file transactions, or infer edits from untrusted text diffs. The next safe extension is a typed multi-file edit batch with per-file UEG/profile identities and atomic session invalidation when any file cannot be mapped exactly.
