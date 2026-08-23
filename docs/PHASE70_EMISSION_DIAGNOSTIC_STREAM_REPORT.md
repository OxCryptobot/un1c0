# Phase 70: bounded local emission diagnostic stream

## Executive summary

Phase 70 adds `EmissionDiagnosticStream`, a bounded local container for a contiguous sequence of canonical Phase 69 diagnostic envelopes. The stream retains exact target, batch, profile-key, and unit-root context, applies a separate domain-separated SHA-256 digest, and accepts no frame until the nested Phase 69 envelope is current-state verified. Parsing is strict and all-or-nothing: a malformed, stale, reordered, oversized, or tampered frame invalidates the complete result.

## Implementation

The implementation is in [`src/emission_diagnostic_stream.rs`](../src/emission_diagnostic_stream.rs) and is exported through `src/lib.rs`. `from_verified_reports` accepts only reports that pass current-envelope verification, assigns contiguous local sequences starting at one, stores canonical Phase 69 bytes, enforces `MAX_STREAM_FRAMES = 32`, enforces the Phase 69 per-frame limit of 64 KiB, and enforces `MAX_STREAM_BYTES = 256 KiB` across frame bytes.

The stream envelope is a strict version-1 JSON object containing stream ID, target, batch ID, profile key, sorted unit roots, ordered frame records, and a final stream digest. The digest is SHA-256 over a separate Phase 70 domain separator and the canonical stream envelope with its digest field zeroed. `from_json_for` checks the input size before parsing, rejects unknown fields and non-canonical bytes, verifies the stream digest, checks exact current context, validates contiguous sequences, verifies each nested Phase 69 envelope against the supplied current snapshot/profile/candidate map, and returns no partial stream.

The public summary exposes only stream ID, frame count, total frame bytes, first/last sequence, and the fixed-size stream digest. The stream has no file, socket, network, process, secret, signing, quorum, trust, persistence, or authorization authority.

## Coverage evidence

[`tests/phase70_emission_diagnostic_stream_integration.rs`](../tests/phase70_emission_diagnostic_stream_integration.rs) passed **4/4 tests**. The deterministic property-style test round-trips every legal frame count from **1 through 32**, checks sequence boundaries, verifies bounded size, checks source-text absence, and requires exact stream equality after rehydration. Additional tests cover empty/zero/excessive inputs, sequence mutation, target drift, nested stale candidates, stream digest tampering, non-canonical JSON, unknown fields, oversized input, and all-or-nothing error behavior.

## Benchmark results

The benchmark source is [`examples/phase70_emission_diagnostic_stream_benchmark.rs`](../examples/phase70_emission_diagnostic_stream_benchmark.rs), with sanitized rows in [`benchmarks/phase70_emission_diagnostic_stream.json`](../benchmarks/phase70_emission_diagnostic_stream.json). Every row uses four units, eight functions per unit, 32 total functions, 64 samples, Rust target, zero errors, and false authority markers.

| Frames | Total stream bytes | Bytes/frame | Construct p50 / p95 / p99 | Verify p50 / p95 / p99 |
|---:|---:|---:|---:|---:|
| 1 | 5,126 | 1,317 | 2,493,969 / 2,649,527 / 2,653,282 ns | 1,808,639 / 1,979,318 / 2,232,843 ns |
| 2 | 9,353 | 1,317 | 4,126,009 / 5,082,602 / 5,831,004 ns | 3,395,605 / 3,632,280 / 3,670,825 ns |
| 4 | 17,799 | 1,317 | 7,299,612 / 7,936,283 / 8,133,567 ns | 6,716,350 / 7,322,137 / 7,444,148 ns |
| 8 | 34,684 | 1,317 | 13,918,939 / 14,521,621 / 14,793,553 ns | 13,227,629 / 14,222,511 / 15,056,055 ns |
| 16 | 68,484 | 1,317 | 26,695,896 / 30,193,089 / 33,680,805 ns | 25,844,846 / 27,015,118 / 27,191,899 ns |
| 32 | 136,058 | 1,317 | 52,857,586 / 54,508,411 / 54,620,565 ns | 52,173,680 / 53,170,667 / 54,137,112 ns |

The stream size grows approximately linearly because Phase 70 intentionally retains one canonical frame per observation. At 32 frames, the total encoded stream is **136,058 bytes**, approximately **51.8%** of the 256 KiB cumulative ceiling; construction and verification remain below the configured bound. The 1-to-32 p50 change is approximately **21.2×** for construction and **28.8×** for verification, which is expected for nested current-state verification across 32 retained frames rather than fixed-size aggregation.

## Security and authority

The Phase 70 digest is an integrity check over local serialized data, not an authenticity proof. The stream does not infer trust from repeated reports, authorize actions, or establish distributed consensus. Sequence numbers are local ordering constraints only. Nested Phase 69 verification retains target/profile/batch/unit-root and current semantic-state binding. A failed frame invalidates the whole stream; no partial output or best-effort acceptance is exposed.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase70_emission_diagnostic_stream_integration -- --nocapture
cargo run --example phase70_emission_diagnostic_stream_benchmark > benchmarks/phase70_emission_diagnostic_stream.json
python3 -m json.tool benchmarks/phase70_emission_diagnostic_stream.json >/dev/null
```

## Next boundary

A future phase may add a bounded stream comparison or local replay view, but it must preserve frame-level current verification, stream-level integrity, sequence continuity, exact context binding, cumulative limits, and the prohibition on network, persistence, distributed trust, and authorization expansion.
