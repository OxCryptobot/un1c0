# Phase 67 diagnostic benchmark analysis

## Scope and artifact structure

The benchmark artifact is [`benchmarks/phase67_emission_diagnostic.json`](../benchmarks/phase67_emission_diagnostic.json). It contains four rows, one for each equivalent-local-observation count of 1, 2, 4, and 8. Every row uses the same deterministic Rust fixture: four semantic units, eight functions per unit, 32 functions total, 32 emitted chunks, four typed diagnostic entries, and 64 timing samples. The benchmark records report-generation latency separately from later `verify_for` latency.

The companion executable is [`examples/phase67_emission_diagnostic_benchmark.rs`](../examples/phase67_emission_diagnostic_benchmark.rs). It emits one valid snapshot-bound receipt, repeats that receipt to form equivalent observations, constructs the report, and then verifies the resulting report against the current semantic snapshot. The JSON is sanitized: it contains no source text, keys, signatures, tokens, prompts, or raw sink payloads.

## Recorded measurements

| Equivalent observations | Report p50 | Report p95 | `verify_for` p50 | `verify_for` p95 | Entries | Chunks | Errors |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 669.762 µs | 723.668 µs | 664.892 µs | 722.544 µs | 4 | 32 | 0 |
| 2 | 669.658 µs | 693.950 µs | 667.747 µs | 713.301 µs | 4 | 32 | 0 |
| 4 | 671.823 µs | 715.430 µs | 669.145 µs | 896.389 µs | 4 | 32 | 0 |
| 8 | 676.473 µs | 806.316 µs | 667.987 µs | 729.573 µs | 4 | 32 | 0 |

The underlying artifact stores nanoseconds. Values above are converted to microseconds only for readability; no measurements were re-estimated.

## Derived observations

The mean report-generation p50 across rows is **671.929 µs**, with a minimum of **669.658 µs** and maximum of **676.473 µs**. The mean report p95 is **734.841 µs**, ranging from **693.950 µs** to **806.316 µs**. The report p50 changes by only **+6.711 µs** from one to eight observations, or approximately **1.0%** relative to the one-observation row. This is expected because the report has a fixed four-entry projection and the aggregate stores a count rather than duplicating every receipt.

The mean `verify_for` p50 is **667.443 µs**, ranging from **664.892 µs** to **669.145 µs**. Its one-to-eight-observation change is **+3.095 µs**, approximately **0.5%**. The p50 verification-to-report ratios are **0.993×**, **0.997×**, **0.996×**, and **0.987×** for 1, 2, 4, and 8 observations respectively. In this fixture, report construction and explicit verification have similar cost because both execute the exact semantic-envelope verification path; the wrapper's typed-entry assembly is small compared with root/fingerprint recomputation.

The widest verification p95 spread occurs at four observations: **227.244 µs** between p50 and p95, versus **57.652 µs**, **45.554 µs**, and **61.586 µs** at 1, 2, and 8 observations. That isolated spread is consistent with local scheduler or runtime noise in a 64-sample sandbox run; it is not evidence of a monotonic observation-count cost. The report p95 spread grows from **53.906 µs** at one observation to **129.843 µs** at eight observations, while remaining below 1 ms in the recorded fixture.

## Interpretation and limits

The result demonstrates bounded local behavior for this implementation and fixture: the number of diagnostic entries remains fixed at four, emitted chunks remain fixed at 32, and all rows report zero errors. It does not demonstrate production throughput, distributed consensus, authorization, or network performance. The benchmark uses an in-process local sink and does not exercise filesystem, network, process, cluster, persistence, secret, or signing authority.

The observation-count sweep tests that equivalent local observations do not cause unbounded report growth. It does not prove arbitrary-scale performance, because the aggregate API intentionally retains one canonical observation plus a count. Larger UEGs, more units, different targets, cache states, OS scheduling, compiler versions, and concurrent callers require separate measurements.

The chart [`docs/PHASE67_emission_diagnostic_benchmark.png`](./PHASE67_emission_diagnostic_benchmark.png) plots the four recorded p50/p95 series. The p95 point at four observations is visible as the only pronounced verification-tail excursion; the p50 lines remain nearly flat.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo run --example phase67_emission_diagnostic_benchmark > benchmarks/phase67_emission_diagnostic.json
python3 -m json.tool benchmarks/phase67_emission_diagnostic.json >/dev/null
```

The benchmark is intentionally repeatable but not bit-for-bit stable across runs: scheduler noise can change latency samples. Treat the committed JSON as the recorded evidence for this batch, and compare future runs using the same fixture, sample count, runtime, and target profile.

## Sanitized authority markers

Every row records `errors: 0`, `cluster_mutation_performed: false`, and `secret_material_recorded: false`. These are explicit boundaries in the artifact, not inferred claims about an external deployment.
