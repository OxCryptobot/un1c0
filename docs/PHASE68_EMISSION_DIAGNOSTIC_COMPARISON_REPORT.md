# Phase 68: bounded local diagnostic comparison

## Executive summary

Phase 68 adds a typed local comparison boundary for two Phase 67 emission diagnostic reports. The comparison verifies both reports against the same current semantic snapshot before calculating deltas, then requires equal target, batch, profile, and unit-root context. It exposes only signed observation/chunk/byte deltas and output-digest equality; it does not infer distributed trust, consensus, freshness, authorization, or permission to act.

## Implementation

The implementation is in [`src/emission_diagnostic_comparison.rs`](../src/emission_diagnostic_comparison.rs). `EmissionDiagnosticComparison::compare` performs `before.verify_for(...)` followed by `after.verify_for(...)`. Any entry-bound, target-bound, profile-bound, batch-bound, unit-set, or candidate-root failure is returned as a typed `Before` or `After` error before a delta exists.

After both reports pass current-envelope verification, the comparison checks target, batch ID, profile key, and unit-root-map equality. The resulting `EmissionDiagnosticDelta` uses `i128` arithmetic for bounded counters and includes `digest_equal` to distinguish identical output identity from changed output identity.

## Test evidence

[`tests/phase68_emission_diagnostic_comparison_integration.rs`](../tests/phase68_emission_diagnostic_comparison_integration.rs) passed **3/3 tests**. Coverage includes exact comparison of one versus four equivalent observations, signed observation delta of three, zero chunk/byte deltas, digest equality, stale-candidate rejection before calculation, and profile-drift rejection.

## Benchmark results

The benchmark source is [`examples/phase68_emission_diagnostic_comparison_benchmark.rs`](../examples/phase68_emission_diagnostic_comparison_benchmark.rs), with sanitized rows in [`benchmarks/phase68_emission_diagnostic_comparison.json`](../benchmarks/phase68_emission_diagnostic_comparison.json). Every row uses four units, eight functions per unit, 32 functions total, 64 samples, zero errors, and false authority markers. The baseline is a one-observation report; the comparison target is 1/2/4/8 observations.

| Before | After | Comparison p50 | Comparison p95 | Observation delta | Chunk delta | Byte delta | Digest equal |
|---:|---:|---:|---:|---:|---:|---:|:---:|
| 1 | 1 | 1,326,377 ns | 1,429,962 ns | 0 | 0 | 0 | true |
| 1 | 2 | 1,332,176 ns | 1,449,612 ns | 1 | 0 | 0 | true |
| 1 | 4 | 1,330,031 ns | 1,774,440 ns | 3 | 0 | 0 | true |
| 1 | 8 | 1,326,615 ns | 1,438,288 ns | 7 | 0 | 0 | true |

The p50 range is **1,326,615–1,332,176 ns**, a spread of **5,561 ns** or approximately **0.4%** of the minimum. The highest p95 is the four-observation row at **1,774,440 ns**, an isolated tail increase in the local 64-sample run. Comparison cost is dominated by re-verifying both reports against the current semantic envelope, not by delta arithmetic.

## Security and authority

The comparison is local, bounded, in-memory, and read-only. It does not persist observations, contact a network, execute a process, read secrets, sign data, mutate a cluster, or create an authorization decision. Repeated observations remain a descriptive count, and `digest_equal` is an equality result rather than a trust assertion.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase68_emission_diagnostic_comparison_integration -- --nocapture
cargo run --example phase68_emission_diagnostic_comparison_benchmark > benchmarks/phase68_emission_diagnostic_comparison.json
python3 -m json.tool benchmarks/phase68_emission_diagnostic_comparison.json >/dev/null
```

## Next boundary

A future phase may add a structured local diagnostic stream or bounded serialization, but it must retain dual current-envelope verification, exact context binding, fixed-size fields, and the prohibition on distributed trust or authorization inference.
