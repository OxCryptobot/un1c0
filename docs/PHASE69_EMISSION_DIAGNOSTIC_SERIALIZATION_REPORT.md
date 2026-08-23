# Phase 69: bounded diagnostic serialization

## Executive summary

Phase 69 adds a canonical, integrity-bound JSON envelope for `EmissionDiagnosticReport`. Serialization exposes only bounded diagnostic evidence; rehydration rejects malformed, non-canonical, oversized, tampered, identity-drifted, and stale data before returning a report. The reconstructed report must pass the existing current-envelope verification path, so serialization does not weaken semantic freshness or target/profile binding.

## Implementation

The implementation is in [`src/emission_diagnostic_serialization.rs`](../src/emission_diagnostic_serialization.rs), with `SemanticCacheKey::from_bytes` and a crate-private aggregate rehydration constructor supporting the typed wire boundary. `to_json` emits a version-1 envelope with target, batch ID, profile key, sorted unit roots, statistics, output digest, observation count, typed entries, and an integrity digest.

The integrity digest is SHA-256 over a domain separator and the canonical envelope bytes with its digest field zeroed. `from_json_for` enforces a 64 KiB input limit, strict unknown-field rejection, canonical JSON bytes, digest equality, non-zero identity/count fields, valid bounded unit IDs, and maximum entry/unit counts. It then reconstructs the aggregate and calls `EmissionDiagnosticReport::from_verified_aggregate`, which rechecks the current snapshot, target profile, complete candidate map, and roots before comparing canonical entries.

## Coverage evidence

[`tests/phase69_emission_diagnostic_serialization_integration.rs`](../tests/phase69_emission_diagnostic_serialization_integration.rs) passed **6/6 tests**. The matrix covers canonical round-trip, deterministic serialization, malformed input, unknown fields, non-canonical JSON, oversized input, integrity tampering, invalid target, invalid unit ID, zero observations, inconsistent observation/entry pairs, and stale candidate-state rejection. The deterministic bounded-count case exercises observation values 1, 2, 4, 8, 16, and 32; values inconsistent with the typed entry are rejected rather than silently normalized.

## Benchmark results

The benchmark source is [`examples/phase69_emission_diagnostic_serialization_benchmark.rs`](../examples/phase69_emission_diagnostic_serialization_benchmark.rs), with sanitized rows in [`benchmarks/phase69_emission_diagnostic_serialization.json`](../benchmarks/phase69_emission_diagnostic_serialization.json). Every row uses four units, eight functions per unit, 32 functions total, 64 samples, zero errors, and false authority markers.

| Observations | Serialized bytes | Serialization p50/p95 | Rehydration p50/p95 |
|---:|---:|---:|---:|
| 1 | 1,317 | 153,769 / 207,756 ns | 895,995 / 1,069,864 ns |
| 2 | 1,319 | 148,284 / 197,814 ns | 902,094 / 1,082,741 ns |
| 4 | 1,323 | 146,250 / 169,175 ns | 888,690 / 1,019,321 ns |
| 8 | 1,327 | 145,811 / 189,562 ns | 903,081 / 1,151,348 ns |

Serialized size grows by only **10 bytes** from one to eight observations because the envelope stores a count, not duplicated receipts. Serialization p50 remains between **145.811 and 153.769 µs**. Verification-gated rehydration p50 remains between **888.690 and 903.081 µs** and is the dominant cost because it recomputes and verifies current semantic state.

## Security and authority

The envelope is local data, bounded to 64 KiB and 256 units, with fixed-size digest fields and typed entries. It does not persist itself, execute commands, access the network, read secrets, sign data, mutate the cluster, or create an authorization decision. Its integrity digest detects serialized-data mutation; it is not a bearer token, signature, quorum certificate, or trust grant.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase69_emission_diagnostic_serialization_integration -- --nocapture
cargo run --example phase69_emission_diagnostic_serialization_benchmark > benchmarks/phase69_emission_diagnostic_serialization.json
python3 -m json.tool benchmarks/phase69_emission_diagnostic_serialization.json >/dev/null
```

## Next boundary

Future work may add a bounded local diagnostic stream or framing protocol, but it must preserve canonical encoding, domain-separated integrity, strict limits, current-envelope re-verification, and the absence of distributed trust or authorization semantics.
