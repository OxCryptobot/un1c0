# Phase 71: verified diagnostic stream templates

## Executive summary

Phase 71 reduces construction overhead for equivalent 32-frame diagnostic streams by introducing `EmissionDiagnosticStreamTemplate`. The template verifies one `EmissionDiagnosticReport`, caches its canonical Phase 69 bytes, and reuses those bytes while building bounded stream frames. It performs one current-state verification per build instead of repeating verification and serialization for every equivalent frame. The optimization is safe only for verified, immutable, byte-identical reports; divergent reports and all untrusted serialized input retain the full validation path.

## Bottleneck and implementation

Phase 70’s 32-frame benchmark recorded construction p50 of **52,857,586 ns**. The frame loop repeatedly called `EmissionDiagnosticReport::verify_for` and `to_json` even though all benchmark frames represented the same aggregate and canonical diagnostic entries. Phase 71 moves that repeated work into `EmissionDiagnosticStreamTemplate::from_report`, which validates once and stores canonical bytes plus exact target, batch, profile, and unit-root context.

`EmissionDiagnosticStreamTemplate::build` rejects zero or excessive frame counts and zero stream IDs, re-verifies the cached report against the caller’s current envelope/profile/candidate map, reuses the immutable canonical bytes, creates contiguous frames 1 through 32, enforces cumulative size, computes the Phase 70 stream digest, and checks the final stream representation. `from_repeated_report` exposes the same optimized path for callers with one verified report and an equivalent observation count. The general `from_verified_reports` path now reuses the first encoded frame for reports exactly equal to the first while retaining individual validation for divergent reports.

## Benchmark results

The benchmark uses the deterministic Phase 70 workload: four units, eight functions per unit, 32 total functions, Rust target, 32 frames, and 64 samples per row. The baseline performs current-state verification and canonical serialization once per frame. The optimized path uses the verified template. The current report-list builder is recorded separately.

| Path | p50 | p95 | p99 |
|---|---:|---:|---:|
| Legacy repeated verification + serialization | 27,341,435 ns | 28,959,089 ns | 29,002,771 ns |
| Optimized verified template | 25,704,371 ns | 26,812,241 ns | 27,144,379 ns |
| Current report-list builder | 26,400,605 ns | 27,986,027 ns | 28,213,991 ns |

Against the controlled legacy baseline, the optimized template reduces p50 by **1,637,064 ns (5.987%)**, p95 by **2,146,848 ns (7.413%)**, and p99 by **1,858,392 ns (6.408%)**. The optimized template is also **2.637%** faster at p50 than the current report-list builder in the same run. All rows use 64 samples, report zero errors, and set `cluster_mutation_performed` and `secret_material_recorded` to false.

The measured gain is intentionally conservative because `build` still performs one current-state verification and must allocate the bounded frame vector and compute the final stream digest. The optimization does not skip validation; it removes only redundant equivalent-frame work. Performance results are local sandbox measurements and are not production scalability claims.

## Coverage evidence

`tests/phase70_emission_diagnostic_stream_integration.rs` now passes **5/5 tests**. The deterministic property test round-trips every frame count from 1 through 32. The template-specific test confirms canonical byte identity with the report, exact equality between template-built and repeated-report streams at 32 frames, and typed stale-candidate rejection. Existing tests continue to cover empty/zero/excessive streams, sequence failures, context drift, nested stale state, integrity tampering, non-canonical JSON, unknown fields, and oversized input.

## Security and authority

The template is a local immutable optimization object, not a signature, bearer token, trust grant, quorum certificate, authorization result, or distributed observation. Every build requires current-envelope verification. Serialized stream parsing still verifies every nested Phase 69 frame and never uses the template as an unchecked deserialization shortcut. No source text, secret, private key, process, socket, filesystem, network, persistence, retry, or cluster authority is introduced.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase70_emission_diagnostic_stream_integration -- --nocapture
cargo run --example phase71_emission_diagnostic_stream_template_benchmark > benchmarks/phase71_emission_diagnostic_stream_template.json
python3 -m json.tool benchmarks/phase71_emission_diagnostic_stream_template.json >/dev/null
```

## Next boundary

Further optimization should first profile report verification and stream digest construction independently. Any cache or batch API must remain bound to exact canonical bytes and current semantic roots, preserve full verification for untrusted input and divergent reports, and remain local and non-authoritative.
