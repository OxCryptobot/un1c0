# Phase 67: bounded local emission evidence

## Executive summary

Phase 67 adds `EmissionEvidenceBundle`, a small read-only wrapper over the Phase 66 emission-receipt aggregate. It makes aggregate integrity explicit with a domain-separated SHA-256 digest while retaining the existing exact current-envelope verification gate. Empty or divergent observations cannot produce a bundle, and a stale candidate UEG cannot be accepted merely because an old aggregate remains internally consistent.

## Implementation

The implementation is in [`src/emission_evidence.rs`](../src/emission_evidence.rs). `from_receipts` delegates first to `EmissionReceiptAggregate::from_receipts`, preserving its non-empty and exact-equivalence contract. The bundle then stores the validated aggregate and a fixed `[u8; 32]` digest over canonical target, batch, profile, sorted unit-root, statistics, output-digest, and observation-count fields.

`verify_for` recomputes the digest and returns a typed `DigestMismatch` error if the wrapper no longer matches its aggregate. It then delegates to `EmissionReceiptAggregate::verify_for`, which rechecks the current semantic snapshot envelope, target/profile identity, batch binding, complete unit set, and current candidate roots. No unchecked fallback or partial diagnostic result is exposed.

## Security and authority

The bundle is local, bounded, in-memory, and read-only. It exposes only a validated aggregate and a fixed-size integrity digest. It contains no source text, prompts, model output, private keys, signatures, bearer tokens, filesystem handles, network metadata, process control, persistence, quorum logic, trust inference, authorization, or cluster mutation.

The observation count is descriptive evidence about equivalent local receipts only. Repetition does not become consensus, distributed freshness, or permission to act. The digest provides integrity detection, not authorization.

## Test evidence

[`tests/phase67_emission_evidence_integration.rs`](../tests/phase67_emission_evidence_integration.rs) passed **3/3 tests**. Coverage includes successful exact verification with two equivalent observations, rejection of divergent receipt production before bundle creation, and rejection of stale candidate state during verification.

## Benchmark results

The benchmark source is [`examples/phase67_emission_evidence_benchmark.rs`](../examples/phase67_emission_evidence_benchmark.rs), with sanitized rows in [`benchmarks/phase67_emission_evidence.json`](../benchmarks/phase67_emission_evidence.json). The deterministic fixture contains four units, eight functions per unit, 32 total functions, and 32 emitted chunks. Each row contains 64 samples, zero errors, and false authority markers.

| Observations | Bundle construction p50/p95 | Current-state verification p50/p95 | Chunks |
|---:|---:|---:|---:|
| 1 | 20,982 / 26,188 ns | 682,672 / 732,158 ns | 32 |
| 2 | 16,405 / 22,536 ns | 679,921 / 728,769 ns | 32 |
| 4 | 17,587 / 19,487 ns | 684,412 / 952,548 ns | 32 |
| 8 | 19,918 / 22,538 ns | 680,857 / 740,948 ns | 32 |

The exact values are preserved in the JSON artifact. Local scheduler noise explains small differences between rows. Verification dominates because it recomputes current semantic fingerprints across all candidate functions; the wrapper's digest pass remains small and bounded.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase67_emission_evidence_integration -- --nocapture
cargo run --example phase67_emission_evidence_benchmark > benchmarks/phase67_emission_evidence.json
python3 -m json.tool benchmarks/phase67_emission_evidence.json >/dev/null
```

## Validation boundary

Phase 67 is complete when the reusable skill validator, `cargo fmt --all -- --check`, and `cargo test --all-targets` pass. Publication remains a separate GitHub-authentication boundary; local commits must be preserved if the remote rejects the available credentials.

## Next boundary

A later phase may add a richer local diagnostic projection over this bundle, but it must remain bounded and read-only, preserve digest and current-envelope verification, and avoid turning local evidence into authorization or distributed trust.
