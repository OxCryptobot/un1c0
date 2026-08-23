# Phase 67: bounded local emission diagnostic report

## Executive summary

Phase 67 adds `EmissionDiagnosticReport`, a bounded read-only projection over the Phase 66 receipt aggregate. The report is created only after aggregate construction and exact current-envelope verification. It exposes four typed entries—observation count, chunk count, bytes emitted, and output-digest confirmation—while retaining typed failures for empty/divergent observations, stale candidate UEGs, target drift, and profile drift.

## Implementation

The implementation is in [`src/emission_diagnostic.rs`](../src/emission_diagnostic.rs). `from_receipts` delegates aggregate construction to `EmissionReceiptAggregate::from_receipts`, then calls `verify_for` against the current `SemanticSnapshotEnvelope`, `TargetCapabilityProfile`, and complete candidate-unit map before constructing entries.

`verify_for` rechecks entry bounds and delegates to `EmissionReceiptAggregate::verify_for`. This preserves the Phase 66 current-envelope gate: target, batch, profile, complete unit set, and current roots must still match. The report cannot be generated from a merely self-consistent stale aggregate.

## Typed bounded entries

| Entry | Meaning | Bound |
|---|---|---:|
| `ObservationCount` | Number of equivalent local receipts | `usize` |
| `ChunkCount` | Accepted emitted chunks in the aggregate | `usize` |
| `BytesEmitted` | Accepted emitted bytes in the aggregate | `usize` |
| `DigestConfirmed` | Output digest captured by the receipts | 32 bytes |

The implementation caps the report at four entries and each encoded entry at 128 bytes. These are structural bounds, not a trust or authorization mechanism.

## Security and authority

The report is local, in-memory, bounded, and read-only. It carries no source text, prompts, model output, private keys, signatures, bearer tokens, filesystem handles, network metadata, process control, persistence, quorum logic, trust inference, authorization, or cluster mutation. Observation repetition remains a descriptive local count. The digest entry confirms receipt output identity; it does not authorize an action.

## Test evidence

[`tests/phase67_emission_diagnostic_integration.rs`](../tests/phase67_emission_diagnostic_integration.rs) passed **3/3 tests**. Coverage includes valid report generation with the maximum four typed entries, empty-input rejection, target-bound emission rejection, stale candidate-state rejection, and target-profile drift rejection. The companion evidence-wrapper tests in [`tests/phase67_emission_evidence_integration.rs`](../tests/phase67_emission_evidence_integration.rs) also passed **3/3 tests**.

## Benchmark results

The benchmark source is [`examples/phase67_emission_diagnostic_benchmark.rs`](../examples/phase67_emission_diagnostic_benchmark.rs), with sanitized rows in [`benchmarks/phase67_emission_diagnostic.json`](../benchmarks/phase67_emission_diagnostic.json). The deterministic fixture contains four units, eight functions per unit, 32 total functions, and 32 emitted chunks. Each row contains 64 samples, four diagnostic entries, zero errors, and false authority markers.

| Observations | Report generation p50/p95 | `verify_for` p50/p95 | Entries | Chunks |
|---:|---:|---:|---:|---:|
| 1 | 669,762 / 723,668 ns | 664,892 / 722,544 ns | 4 | 32 |
| 2 | 669,658 / 693,950 ns | 667,747 / 713,301 ns | 4 | 32 |
| 4 | 671,823 / 715,430 ns | 669,145 / 896,389 ns | 4 | 32 |
| 8 | 676,473 / 806,316 ns | 667,987 / 729,573 ns | 4 | 32 |

The exact values are preserved in the JSON artifact. Verification dominates because every report generation and verification path rechecks current semantic fingerprints across the candidate functions. Local scheduler noise explains the isolated p95 variation at four observations.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase67_emission_diagnostic_integration -- --nocapture
cargo test --test phase67_emission_evidence_integration -- --nocapture
cargo run --example phase67_emission_diagnostic_benchmark > benchmarks/phase67_emission_diagnostic.json
python3 -m json.tool benchmarks/phase67_emission_diagnostic.json >/dev/null
```

## Validation boundary

Phase 67 is complete when the reusable skill validator, `cargo fmt --all -- --check`, and `cargo test --all-targets` pass. Publication is a separate GitHub-authentication boundary; local commits must be preserved if the remote rejects the available credentials.

## Next boundary

A future phase may add a richer diagnostic taxonomy or local comparison view, but it must remain bounded and read-only, require aggregate and current-envelope verification, and never convert local observations into distributed trust or authorization.
