# Phase 58: dependency-aware semantic session

## Executive summary

Phase 58 composes the Phase 55 semantic fingerprint, Phase 56 dependency graph/cache, and Phase 57 typed snapshot into a stateful `SemanticSession`. A session fixes its target capability profile, warms per-function validation evidence at start, computes the reverse-dependent closure for each refresh, and replaces its snapshot only after the new aggregate report is valid and exactly bound to the new UEG/profile fingerprint. Any invalid UEG, duplicate function, out-of-range index, profile/target change, structural function-shape change, or invalid semantic report clears the current snapshot. This preserves the fail-closed generation boundary while allowing unchanged callers and unrelated functions to reuse typed reports.

The benchmark is deliberately conservative. It compares full snapshot capture with a warmed leaf refresh over deterministic one-to-32-function Python call chains and four target profiles, using 64 samples per row. In this fixture a changed leaf affects the entire call chain, so the refresh path is not faster than full capture; its value is bounded revalidation and exact evidence reuse, not an unsupported latency claim. At 32 Rust functions, full capture measured 646,982 ns p50 / 685,920 ns p95, while warm refresh measured 1,387,714 ns p50 / 1,701,514 ns p95, with one revalidated function, 31 cache hits, one cache miss, and zero errors.

## Implementation

The session implementation is in [`src/semantic_session.rs`](../src/semantic_session.rs). Its central state is:

```rust
pub struct DependencyAwareSemanticSession {
    profile: TargetCapabilityProfile,
    function_names: Vec<String>,
    validator: DependencyAwareSemanticValidator,
    current_fingerprint: Option<SemanticFingerprint>,
    snapshot: Option<SemanticValidationSnapshot>,
}
```

`start` builds the dependency graph, computes the Phase 55 fingerprint, validates the complete function set through the Phase 56 validator, and creates a Phase 57 snapshot only from a valid aggregate report. `refresh` accepts `(ueg, changed_functions, profile)`, rejects target/profile drift, rejects function-name/order changes, computes the new fingerprint, validates the affected reverse-dependent closure, and atomically replaces the session snapshot only after `SemanticValidationSnapshot::from_validated_report` succeeds.

The new constructor in [`src/semantic_snapshot.rs`](../src/semantic_snapshot.rs) is intentionally crate-private. It does not bypass validation; it verifies `report.is_valid()` before binding the report to the UEG/profile fingerprint. `snapshot_for(target)` additionally checks the requested emitter target, and `IncrementalCodeGenerator::emit_remaining_with_snapshot` performs the final exact-root and profile verification before the first sink call.

## Phase 55 security composition

Phase 55 stores a profile key, one digest per function, and a recomposed root key. During a refresh, the changed function receives a new function digest and the root changes. The Phase 56 validator looks up reports using the pair `(profile_key, function_key)`, so an old report cannot be reused for changed source or a different target profile. Reverse-dependent callers may reuse their own unchanged reports only after the graph has conservatively placed them in the affected closure; the changed function itself must miss and revalidate.

This is fail closed in three ways:

1. **Changed input cannot hit the old function entry.** A source edit changes the function digest, producing a cache miss and a fresh `validate_function_with_profile` call.
2. **Dependency evidence cannot be silently omitted.** A changed leaf causes all transitive reverse callers to be visited. Unrelated functions are not needed for the closure but remain represented by the exact new root fingerprint.
3. **An invalid report cannot become an emission proof.** The aggregate report must contain no error diagnostics before snapshot creation, and the emitter verifies the snapshot against the current UEG and target profile immediately before emission.

The session is local evidence only. It does not grant filesystem, process, network, secret, cluster, signing, or deployment authority.

## Verification evidence

The integration suite is [`tests/phase58_semantic_session_integration.rs`](../tests/phase58_semantic_session_integration.rs). It covers warm changed-leaf reuse, invalid semantic refresh clearing, stale snapshot rejection before sink execution, structural function-set invalidation, target mismatch, profile mismatch, valid refresh emission, and current-root verification. The targeted suite passed **4/4 tests**.

The benchmark source is [`examples/phase58_semantic_session_benchmark.rs`](../examples/phase58_semantic_session_benchmark.rs), the sanitized artifact is [`benchmarks/phase58_semantic_session.json`](../benchmarks/phase58_semantic_session.json), and the chart is [`benchmarks/phase58_semantic_session.png`](../benchmarks/phase58_semantic_session.png). The benchmark records `cluster_mutation_performed: false` and `secret_material_recorded: false` in every row.

## Benchmark results

All rows use 64 samples. Values below are nanoseconds. The Rust rows are shown in full because they are the reference target used in the session integration tests; Go, Zig, and Python were also measured at every function level with the same zero-error and reuse invariants.

| Functions | Full capture p50 | Full capture p95 | Warm refresh p50 | Warm refresh p95 | Affected | Revalidated | Cache hits | Cache misses |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 33,604 | 51,656 | 71,812 | 106,800 | 1 | 1 | 0 | 1 |
| 2 | 53,469 | 74,397 | 117,340 | 157,305 | 2 | 1 | 1 | 1 |
| 4 | 94,636 | 137,183 | 200,933 | 226,572 | 4 | 1 | 3 | 1 |
| 8 | 169,739 | 207,380 | 376,992 | 478,399 | 8 | 1 | 7 | 1 |
| 16 | 330,823 | 352,908 | 708,545 | 793,310 | 16 | 1 | 15 | 1 |
| 32 | 646,982 | 685,920 | 1,387,714 | 1,701,514 | 32 | 1 | 31 | 1 |

The other target profiles remained within the same order of magnitude. At 32 functions, warm-refresh p50 was 1,375,811 ns for Go, 1,377,733 ns for Zig, and 1,377,427 ns for Python; all recorded one miss, 31 hits, one revalidated function, and zero refresh errors.

These are sandbox microbenchmarks, not production throughput. The call-chain fixture intentionally maximizes the affected closure. A graph with many unrelated functions or a shallow dependency cone should show a more favorable relationship between bounded refresh work and full validation; that claim must be measured with a separate fixture rather than inferred here.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase58_semantic_session_integration -- --nocapture
cargo run --example phase58_semantic_session_benchmark > benchmarks/phase58_semantic_session.json
python3 -m json.tool benchmarks/phase58_semantic_session.json >/dev/null
python3 scripts/analyze_phase58_semantic_session.py
```

The full workspace suite and skill validator remain release gates. Warnings in legacy modules are reported separately and are not treated as Phase 58 correctness failures.

## Boundaries and next work

Phase 58 does not persist sessions across processes, merge concurrent edits, coordinate remote caches, or infer changed-function sets from an untrusted caller. A production editor integration should derive changed indexes from a parser-level structural diff, reject ambiguous mappings, and persist only explicitly versioned local evidence. The next architectural step should focus on deterministic edit-to-function mapping and session serialization/versioning, while retaining the exact fingerprint, dependency closure, and pre-emitter verification contracts.
