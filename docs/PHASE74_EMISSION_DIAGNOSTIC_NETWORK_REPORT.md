# Phase 74: Emission Diagnostic Network Transport Report

**Author:** Manus AI
**Status:** Implemented and validated locally; commit closeout remains.
**Commit base:** Phase 73 `a5cb569`
**Scope:** Authenticated TCP framing and bounded multi-node diagnostic transport over Phase 73 Ed25519 attestations.

## Executive summary

Phase 74 adds a real length-prefixed TCP transport for Phase 73 emission-diagnostic attestations and a bounded multi-node receiver. The implementation binds each connection to a non-zero node ID, connection ID, explicitly registered Ed25519 public key, and monotonic sequence window. It rejects malformed or oversized frames before allocation, verifies a domain-separated frame digest over connection metadata and canonical attestation JSON, and enforces a 64-frame budget on both send and receive paths. The multi-node receiver allows at most eight registered nodes and performs Phase 73 current-state stream verification before constructing a distributed observation or mutating a per-node aggregator.

The loopback integration suite passes **9/9 tests**, and the sanitized benchmark produces **16/16 frame/concurrency rows with zero errors** across 12 samples per row. The measured bottleneck is not TCP framing: at one frame and one connection, the receive p50 is 131,751 ns while stream verification is 10,702,821 ns; at eight frames and eight connections, receive p50 is 2,187,331 ns while verification is 228,602,592 ns. Verification accounts for approximately 98.9% of the end-to-end p50 in the slowest observed row. This timing boundary includes semantic current-state verification and Ed25519 verification; the benchmark does not yet isolate those operations, so it does not justify attributing the full cost to Ed25519 or domain-separated hashing alone.

## Implemented surface

| Area | Implementation evidence |
|---|---|
| TCP listener | `AuthenticatedDiagnosticListener` binds a caller-selected address, fixes the expected node ID, exposes its local address, and accepts a verified handshake. |
| Handshake | Version, node ID, connection ID, public key, and domain-separated SHA-256 digest are checked; the exact key must exist in the verifier registry. |
| Framing | Four-byte big-endian length prefix; empty, oversized, malformed, unknown-field, and invalid-shape payloads are rejected before unbounded allocation. |
| Frame binding | Node ID, connection ID, sequence, and canonical attestation bytes are bound into a Phase 74 frame digest. |
| Replay window | Send and receive counters start at one and accept only the exact next sequence; replays and gaps do not advance state. |
| Frame budget | Both directions stop after `MAX_NETWORK_FRAMES_PER_NODE = 64`. |
| Multi-node fan-in | `MultiNodeDiagnosticReceiver` supports at most `MAX_NETWORK_NODES = 8` nodes and keeps aggregators isolated by node ID. |
| Verification gate | Receiver-side aggregation calls Phase 73 `verify_stream` against the current snapshot, target profile, and candidate UEG map before creating a verified observation. |
| Malformed-key safety | Network decoding uses a fallible public-key accessor, preventing malformed deserialized key lengths from reaching an `expect` panic. |

## Validation evidence

The focused integration command completed successfully:

```text
cargo test --test phase74_emission_diagnostic_network_integration -- --nocapture
9 passed; 0 failed; 0 ignored
```

The test cases cover valid loopback handshake and attested transfer, multi-node isolation, exact trust-key mismatch, node-identity mismatch, client replay and gap refusal, downstream aggregator replay and gap rejection, frame-limit enforcement, invalid node and connection IDs, stale candidate-state rejection, unregistered nodes, the eight-node registration bound, and zero frame-limit rejection. All socket tests use `127.0.0.1:0`; no external network is contacted.

The example build and benchmark command also completed successfully:

```text
cargo check --example phase74_emission_diagnostic_network_benchmark
cargo run --quiet --example phase74_emission_diagnostic_network_benchmark
```

The artifact records `samples: 12`, frame counts `[1, 2, 4, 8]`, concurrency levels `[1, 2, 4, 8]`, `errors: 0`, `cluster_mutation_performed: false`, and `secret_material_recorded: false`. The exact raw sanitized rows are preserved in `benchmarks/phase74_emission_diagnostic_network.json`.

## Benchmark evidence

All measurements below are in nanoseconds, as emitted by the benchmark artifact. `e2e` includes client connection setup, handshake, send, server receive, and verification completion. `receive` is the server's framed receive critical path across the concurrent handlers. `verify` is the server's current-state Phase 73 stream-verification critical path. The per-connection-frame column divides the verification critical path by frames per connection; it is not a claim about an isolated Ed25519 operation.

| Frames/connection | Connections | Stream bytes | E2E p50 | E2E p95 | E2E p99 | Verify p50 | Verify/connection-frame p50 | Frames/s p50 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 3,435 | 11,470,400 | 11,896,274 | 12,432,263 | 10,702,821 | 10,702,821 | 87 |
| 1 | 8 | 3,435 | 20,518,793 | 21,754,677 | 23,013,210 | 17,939,270 | 17,939,270 | 389 |
| 2 | 1 | 6,380 | 24,451,247 | 24,744,660 | 24,830,203 | 23,426,099 | 11,713,049 | 81 |
| 2 | 8 | 6,380 | 42,245,170 | 44,387,430 | 46,153,594 | 37,414,923 | 18,707,461 | 378 |
| 4 | 1 | 12,254 | 56,917,813 | 57,394,825 | 60,666,037 | 55,777,765 | 13,944,441 | 70 |
| 4 | 8 | 12,254 | 91,848,822 | 97,923,118 | 101,523,767 | 87,755,099 | 21,938,774 | 348 |
| 8 | 1 | 24,027 | 156,785,023 | 174,144,508 | 191,781,274 | 155,002,094 | 19,375,261 | 51 |
| 8 | 8 | 24,027 | 233,688,091 | 239,352,190 | 243,010,124 | 228,602,592 | 28,575,324 | 273 |

The complete artifact includes all intermediate concurrency rows. At one frame per connection, increasing concurrency from one to eight raises p50 throughput from 87 to 389 frames/s. At eight frames per connection, the corresponding increase is from 51 to 273 frames/s. Throughput improves with concurrency, but the critical-path verification cost and end-to-end tail rise as more signature-bearing streams compete for CPU. The maximum observed verification p99 is 240,624,560 ns, while the maximum receive p99 is 9,722,031 ns, demonstrating that framing and socket reads are materially smaller than the current verification boundary in this local fixture.

## Ed25519 and hashing bottleneck analysis

Phase 74 exposes the first real network boundary, but its current benchmark intentionally measures a complete verification contract rather than an unsafe microbenchmark. The `verify` timer includes canonical stream/current-envelope verification, content recomputation, and Ed25519 signature verification. Therefore, the measured 98.9% verification share should be read as **verification-pipeline dominance**, not as proof that Ed25519 itself accounts for 98.9% of runtime.

The network frame digest adds a SHA-256 pass over the canonical attestation JSON and the fixed connection metadata. The receive p99 remains below 10 ms in the observed rows, while verification p99 reaches approximately 241 ms. This makes frame-domain hashing and length-prefix parsing unlikely to be the first optimization target for this fixture. The correct next profiling step is to add separate timers around canonical-byte reuse, semantic snapshot verification, content-hash recomputation, Ed25519 verification, and frame-domain hashing, then compare scalar, cached, and batched paths with byte-for-byte golden vectors. No SIMD or batching claim is made in this phase.

The benchmark also shows a concurrency boundary. Eight concurrent connections increase aggregate throughput, but the eight-frame/eight-connection row reaches 243,010,124 ns at p99. This is consistent with CPU contention around repeated full stream verification. A future phase may introduce bounded verification workers or verified canonical-byte reuse, but it must preserve explicit trust registration, current-state checks, replay ordering, and all-or-nothing aggregation.

## Security boundary and remaining production work

Phase 74 is a bounded authenticated transport layer, not a production service-mesh identity system. It does not provide TLS, certificate authority behavior, peer discovery, key distribution, durable queues, retry delivery, durable acknowledgements, cluster membership, quorum, authorization, or persistent trust state. A valid Ed25519 signature remains integrity evidence for the signed canonical content, and a handshake key match remains evidence only that the peer presented a registered key.

The TCP implementation currently uses blocking `std::net` I/O and caller-managed connection lifecycle. Production deployment would require an explicit asynchronous runtime or supervised worker model, read/write deadlines, connection admission policy, observability with redaction, durable handoff semantics, certificate or transport confidentiality decisions, and an operational key-rotation procedure. Those capabilities should be added in later, separately gated phases rather than inferred from this loopback implementation.

## Closeout status

Phase 74 source, tests, benchmark, and documentation are complete locally. The remaining closeout sequence is to run the full repository validation, validate the reusable skill, stage only the Phase 74 paths plus the intended Phase 73 API hardening, commit with the Phase 74 message, and report the exact local/remote boundary. Existing unrelated worktree modifications and previously created presentation files must remain unstaged.

## References

[1]: ../src/emission_diagnostic_network.rs "Phase 74 authenticated diagnostic network implementation"
[2]: ../src/emission_diagnostic_attestation.rs "Phase 73 attestation and verifier implementation"
[3]: ../src/emission_diagnostic_transport.rs "Phase 72 bounded diagnostic transport and aggregation"
[4]: ../benchmarks/phase74_emission_diagnostic_network.json "Phase 74 sanitized benchmark artifact"
[5]: ../benchmarks/phase73_emission_diagnostic_attestation.json "Phase 73 attestation benchmark artifact"
[6]: ../tests/phase74_emission_diagnostic_network_integration.rs "Phase 74 loopback integration tests"
