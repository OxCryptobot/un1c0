# Phase 64: snapshot-bound multi-unit emission

## Executive summary

Phase 64 connects the Phase 63 multi-unit semantic snapshot envelope to a typed emission boundary. `SnapshotBoundBatchEmitter` verifies the exact batch ID, unit set, profile key, and per-unit root keys before invoking any emitter or sink. Stale candidate UEGs are rejected before the first sink call, target mismatches return typed errors, and sink failures preserve unit identity.

The emitter is intentionally local and bounded. It does not persist snapshots, sign evidence, transmit data, or mutate a cluster.

## Implementation

The implementation is in [`src/snapshot_emission.rs`](../src/snapshot_emission.rs), exported from [`src/lib.rs`](../src/lib.rs). The emitter receives a `SemanticSnapshotEnvelope`, batch ID, target capability profile, and a map of unit IDs to candidate UEGs.

It first checks that `profile.target` matches the configured emitter target. It then delegates exact multi-unit verification to `SemanticSnapshotEnvelope::verify_for`. Only after that succeeds does it construct one `IncrementalCodeGenerator` per unit and call `emit_remaining_with_snapshot` using the retained validated per-unit snapshot. Every sink callback includes the unit ID.

## Fail-closed behavior

The sink is never invoked when the candidate unit set, batch ID, profile, or root fingerprints are stale. A sink error is wrapped as `SnapshotEmissionError::Sink` and retains the unit identity; generator failures are wrapped as `SnapshotEmissionError::Unit`. There is no unchecked fallback to `emit_remaining`.

The accepted state is:

> exact envelope batch ID + exact unit set + exact target profile + exact per-unit semantic roots

## Test evidence

[`tests/phase64_snapshot_bound_emission_integration.rs`](../tests/phase64_snapshot_bound_emission_integration.rs) passed **3/3 tests**. Coverage includes successful two-function emission, stale candidate rejection before sink invocation, target mismatch, and typed sink failure.

## Benchmark results

The benchmark source is [`examples/phase64_snapshot_bound_emission_benchmark.rs`](../examples/phase64_snapshot_bound_emission_benchmark.rs), with sanitized rows in [`benchmarks/phase64_snapshot_bound_emission.json`](../benchmarks/phase64_snapshot_bound_emission.json). Each row contains 64 samples, zero errors, eight functions per unit, and false authority markers.

| Units | Total functions | Verification p50/p95 | Emission p50/p95 | Chunks |
|---:|---:|---:|---:|---:|
| 1 | 8 | 224,190 / 274,314 ns | 491,565 / 558,051 ns | 8 |
| 2 | 16 | 335,294 / 365,629 ns | 726,989 / 840,428 ns | 16 |
| 4 | 32 | 669,598 / 829,575 ns | 1,469,956 / 2,199,486 ns | 32 |
| 8 | 64 | 1,354,832 / 1,877,404 ns | 2,893,464 / 3,053,530 ns | 64 |

Emission includes full candidate verification and code generation for every unit. These are local sandbox measurements and must not be interpreted as production throughput.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase64_snapshot_bound_emission_integration -- --nocapture
cargo run --example phase64_snapshot_bound_emission_benchmark > benchmarks/phase64_snapshot_bound_emission.json
python3 -m json.tool benchmarks/phase64_snapshot_bound_emission.json >/dev/null
```

## Next boundary

Phase 64 gates local emission. It does not create a durable or remotely verifiable artifact. The next safe boundary is a typed emission receipt that records the exact envelope keys, emitted chunk counts, and output digest for local audit without introducing secret-bearing signing or transport authority.
