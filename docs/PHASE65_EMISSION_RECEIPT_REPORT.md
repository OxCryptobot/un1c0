# Phase 65: typed snapshot-bound emission receipts

## Executive summary

Phase 65 records a deterministic typed receipt for successful Phase 64 multi-unit emission. `EmissionReceipt` binds the target, applied batch ID, envelope profile key, complete per-unit root map, emitted chunk/byte counts, and a domain-separated SHA-256 digest of the chunks accepted by the sink.

The receipt is local audit evidence. It is not a signature, secret-bearing credential, transport token, filesystem commit record, or distributed consensus certificate.

## Implementation

The implementation is in [`src/emission_receipt.rs`](../src/emission_receipt.rs), with a read-only byte accessor on `SemanticCacheKey` in [`src/semantic_cache.rs`](../src/semantic_cache.rs). `ReceiptBoundBatchEmitter::emit_with_receipt` repeats Phase 64 envelope verification before generation, hashes accepted chunks in deterministic `BTreeMap` unit order, and returns a receipt only after all units complete successfully.

The canonical digest domain is `un1c0/phase65/emission-receipt/v1`. It includes the target label, batch ID, envelope profile key, and for each accepted chunk the bounded unit-identity length and bytes, node index, code length, and code bytes. A sink rejection occurs before the chunk is hashed.

`EmissionReceipt::verify_for` repeats the semantic envelope verification and checks receipt target, batch ID, profile key, complete unit-root map, and expected chunk count. It returns typed errors rather than treating mismatches as advisory.

## Security and correctness

Phase 65 closes the audit gap after successful local generation: a caller can retain evidence of exactly which semantic state and target produced the output statistics. It does not authorize future emission; every future emission must verify a current envelope again. No private keys, signatures, source files, process handles, network state, or cluster authority are included.

## Test evidence

[`tests/phase65_emission_receipt_integration.rs`](../tests/phase65_emission_receipt_integration.rs) passed **3/3 tests**. Coverage includes successful receipt creation and exact verification, batch/target mismatch rejection, and typed no-receipt behavior on sink failure.

## Benchmark results

The benchmark source is [`examples/phase65_emission_receipt_benchmark.rs`](../examples/phase65_emission_receipt_benchmark.rs), with sanitized data in [`benchmarks/phase65_emission_receipt.json`](../benchmarks/phase65_emission_receipt.json). Each row contains 64 samples, zero errors, and false authority markers.

| Units | Total functions | Phase 64 emission p50/p95 | Phase 65 receipt emission p50/p95 | Chunks |
|---:|---:|---:|---:|---:|
| 1 | 8 | 418,534 / 650,232 ns | 690,443 / 1,262,293 ns | 8 |
| 2 | 16 | 778,253 / 1,081,378 ns | 1,182,743 / 1,643,424 ns | 16 |
| 4 | 32 | 1,493,255 / 2,112,640 ns | 2,313,411 / 3,186,685 ns | 32 |
| 8 | 64 | 2,970,353 / 3,246,511 ns | 4,526,581 / 5,020,281 ns | 64 |

Receipt generation adds deterministic hashing and receipt construction to the existing verification and emission path. These are local sandbox measurements, not production throughput claims.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase65_emission_receipt_integration -- --nocapture
cargo run --example phase65_emission_receipt_benchmark > benchmarks/phase65_emission_receipt.json
python3 -m json.tool benchmarks/phase65_emission_receipt.json >/dev/null
```

## Next boundary

Phase 65 does not persist or sign receipts. The next safe extension is a bounded receipt comparison/aggregation API for local diagnostics that preserves exact envelope binding and never upgrades local receipts into remote authority.
