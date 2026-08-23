# Phase 61: atomic multi-file semantic edit batches

## Executive summary

Phase 61 extends the local-first semantic session from one UEG unit to a typed multi-unit batch. Each unit has a bounded relative identity and its own Phase 60 edit manifest. The batch binds every unit to one fixed target/profile configuration, stages all refreshes on cloned sessions, and commits the staged state only when every update succeeds. A failure in any unit invalidates all live sessions, preventing a partially refreshed multi-file semantic state from being emitted.

## Implementation

The implementation is in [`src/semantic_batch.rs`](../src/semantic_batch.rs), exported from [`src/lib.rs`](../src/lib.rs). `SemanticUnitId` is deliberately not a filesystem path resolver: it only validates an identity string and rejects absolute paths, traversal components, empty segments, control characters, and overlong values. `SemanticBatchSession` owns a bounded map of unit identities to existing `DependencyAwareSemanticSession` values.

`refresh_batch` first verifies the fixed profile. It then clones all sessions into a staging map. Each `SemanticEditUpdate` invokes Phase 60 `refresh_from_edit_manifest` on the staged unit. If all updates succeed, the staged map replaces the live state. If any update fails, the live state is invalidated and the staged partial result is discarded.

```rust
pub fn refresh_batch(
    &mut self,
    batch: &SemanticEditBatch,
    profile: &TargetCapabilityProfile,
) -> Result<SemanticBatchRefresh, SemanticBatchError>
```

## Security and correctness

The batch layer does not add authority. It does not open files from a unit identity, execute commands, access networks, read secrets, sign data, or mutate clusters. Its only responsibility is state-coordination around already-validated local UEG and edit-manifest inputs.

The atomicity boundary is explicit:

> fixed profile → typed unit membership → per-unit root/profile-bound manifest resolution → per-unit fingerprint-derived change completeness → dependency-aware validation → all-unit success → live-state replacement

A stale manifest for the second unit invalidates the entire batch even if the first unit would have succeeded. This avoids the most dangerous multi-file failure mode: an apparently valid snapshot set in which some units represent the new edit and others retain or expose uncommitted state.

## Test evidence

The integration suite is [`tests/phase61_semantic_batch_integration.rs`](../tests/phase61_semantic_batch_integration.rs). It covers successful refresh of changed and unchanged units together, whole-batch invalidation when a later unit has a stale manifest, duplicate/path-safe identity checks, and empty-batch rejection. The suite passed **3/3 tests**.

Phase 58 and Phase 59 compatibility suites remain part of the closeout gate. Phase 60 remains the single-unit manifest compatibility layer.

## Benchmark results

The benchmark source is [`examples/phase61_semantic_batch_benchmark.rs`](../examples/phase61_semantic_batch_benchmark.rs); the sanitized artifact is [`benchmarks/phase61_semantic_batch.json`](../benchmarks/phase61_semantic_batch.json). Each row uses 64 samples, zero errors, `cluster_mutation_performed: false`, and `secret_material_recorded: false`.

| Units | Total functions | Atomic batch p50/p95 | Sequential p50/p95 | Changed | Refreshed |
|---:|---:|---:|---:|---:|---:|
| 1 | 8 | 1,240,629 / 1,362,654 ns | 1,164,411 / 1,560,227 ns | 1 | 1 |
| 2 | 16 | 2,484,025 / 2,683,640 ns | 2,317,225 / 2,490,753 ns | 2 | 2 |
| 4 | 32 | 4,816,489 / 5,018,438 ns | 4,587,748 / 6,183,277 ns | 4 | 4 |
| 8 | 64 | 9,598,476 / 9,924,698 ns | 9,156,646 / 9,442,148 ns | 8 | 8 |

The batch path is not claimed to be faster in this benchmark. It measures atomic coordination plus fresh session construction and worst-case dependency chains. Its value is the all-or-nothing state boundary and deterministic unit attribution. A production comparison should separately measure warmed sessions, unrelated units, partial failures, and parallel scheduling only after a bounded scheduler contract exists.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase61_semantic_batch_integration -- --nocapture
cargo run --example phase61_semantic_batch_benchmark > benchmarks/phase61_semantic_batch.json
python3 -m json.tool benchmarks/phase61_semantic_batch.json >/dev/null
```

## Boundary and next phase

Phase 61 does not implement cross-process persistence, multi-file parser orchestration, parallel execution, filesystem transaction commits, or editor protocol transport. The next safe extension is a typed multi-file UEG snapshot envelope with per-unit root/profile hashes and explicit versioned batch IDs, still without granting filesystem or process authority.
