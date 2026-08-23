# Phase 66: emission-receipt aggregation and comparison

## Executive summary

Phase 66 adds a deterministic local comparison boundary for Phase 65 emission receipts. `EmissionReceiptAggregate` accepts only equivalent observations of one semantic emission state and records a bounded observation count. It never combines divergent outputs or treats an aggregate as authorization.

## Implementation

The implementation is in [`src/emission_receipt_aggregate.rs`](../src/emission_receipt_aggregate.rs). Aggregation requires a non-empty slice and compares target, batch ID, profile key, per-unit root map, chunk count, byte count, and output digest against the first receipt. Any mismatch returns a typed error.

`verify_for` checks the aggregate target against the current profile, reconstructs a receipt-shaped value, and delegates to `EmissionReceipt::verify_for`. That repeats current envelope validation, including batch, profile, unit-set, and root-key binding. An old but internally consistent aggregate therefore cannot silently validate against a changed UEG.

## Security and authority

This phase is local, bounded, and read-only. It introduces no persistence, signing, network, filesystem, secret, process, or cluster authority. The aggregate records the count of equivalent observations but does not infer consensus, quorum, or trust from repetition.

## Test evidence

[`tests/phase66_emission_receipt_aggregate_integration.rs`](../tests/phase66_emission_receipt_aggregate_integration.rs) passed **3/3 tests**. Coverage includes equivalent aggregation and current-envelope verification, empty/divergent observation rejection, and target divergence rejection.

## Benchmark results

The benchmark source is [`examples/phase66_emission_receipt_aggregate_benchmark.rs`](../examples/phase66_emission_receipt_aggregate_benchmark.rs), with sanitized rows in [`benchmarks/phase66_emission_receipt_aggregate.json`](../benchmarks/phase66_emission_receipt_aggregate.json). The fixture contains four units and 32 total functions; each row contains 64 samples, zero errors, and false authority markers.

| Observations | Aggregate p50/p95 | Aggregate + verification p50/p95 | Chunks |
|---:|---:|---:|---:|
| 1 | 2,819 / 3,008 ns | 676,747 / 875,698 ns | 32 |
| 2 | 2,819 / 3,008 ns | 676,747 / 875,698 ns | 32 |
| 4 | 2,819 / 3,008 ns | 676,747 / 875,698 ns | 32 |
| 8 | 6,588 / 6,897 ns | 676,844 / 835,352 ns | 32 |

The exact values are preserved in the JSON artifact; local scheduler noise explains small differences between repeated runs. Verification dominates aggregation because it recomputes current semantic fingerprints over all candidate functions.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase66_emission_receipt_aggregate_integration -- --nocapture
cargo run --example phase66_emission_receipt_aggregate_benchmark > benchmarks/phase66_emission_receipt_aggregate.json
python3 -m json.tool benchmarks/phase66_emission_receipt_aggregate.json >/dev/null
```

## Next boundary

The next safe extension is a local diagnostic report over aggregate comparisons. It must preserve exact current-envelope verification and must not turn repeated local observations into distributed trust or authorization.
