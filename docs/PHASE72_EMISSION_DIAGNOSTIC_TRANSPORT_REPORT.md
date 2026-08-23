# Phase 72: bounded diagnostic transport and distributed aggregation

## Executive summary

Phase 72 adds a process-local asynchronous transport and a deterministic distributed-shaped diagnostic aggregator. The transport provides bounded queue admission, wakeable `Future` polling, close semantics, source/sequence metadata, and a domain-separated frame-integrity digest. The aggregator provides bounded source fan-in, contiguous per-source sequences, replay/gap rejection, cumulative frame/byte accounting, and a deterministic aggregate digest. All received streams continue through Phase 70 parsing and current-envelope verification before aggregation.

The implementation intentionally stops short of real distributed transport. There are no sockets, network calls, authentication credentials, signatures, durable queues, cluster authority, quorum rules, or authorization decisions. The new layer is a safe local contract that can be used for later transport integration without conflating local integrity with distributed trust.

## Implementation details

`AsyncDiagnosticTransport` uses a bounded `ArrayQueue`. A send validates source and sequence IDs, obtains the canonical Phase 70 stream bytes, binds source ID and sequence to a domain-separated SHA-256 digest, and rejects closed or full queues. Receive validates the transport digest before invoking `EmissionDiagnosticStream::from_json_for`, which retains strict canonicality, stream-integrity, context, bounds, and nested current-state checks. The polling API registers at most 64 deduplicated waiters and wakes them on send or close.

`DistributedEmissionAggregator` admits at most eight sources and 256 accumulated stream frames. Each source begins at sequence one; lower sequences are replay errors and higher sequences are gap errors. It verifies the complete stream before checking sequence and aggregate limits, then updates state only after all checks succeed. Its summary exposes only bounded counts, source sequence metadata, byte totals, and a domain-separated aggregate digest.

## Phase 72 benchmark

The benchmark uses 64 samples per row, one deterministic unit with two functions, four frames per source, and 1/2/4/8 sources. Each sample sends canonical streams through the bounded queue, receives and re-verifies them, and ingests them into the aggregator.

| Sources | Total frames | p50 | p95 | p99 |
|---:|---:|---:|---:|---:|
| 1 | 4 | 10,967,929 ns | 11,671,581 ns | 11,857,978 ns |
| 2 | 8 | 21,951,233 ns | 22,582,840 ns | 23,103,909 ns |
| 4 | 16 | 43,427,999 ns | 43,827,606 ns | 44,061,038 ns |
| 8 | 32 | 86,674,631 ns | 88,592,306 ns | 89,912,766 ns |

The p50 path scales approximately linearly with source count because each source contributes four complete stream frames and every received frame is rehydrated through current-state verification. The eight-source row reaches 32 total frames, exactly one-eighth of the 256-frame aggregate ceiling, and reports zero errors. These are local sandbox measurements, not distributed network throughput or production latency claims.

## Phase 71 bottleneck and SIMD analysis

Phase 71’s controlled 32-frame benchmark reduced p50 construction from 27,341,435 ns for repeated verification/serialization to 25,704,371 ns with the verified template, a 5.987% reduction. The remaining optimized work includes one current-state semantic verification, allocation/cloning of bounded frame storage, stream JSON assembly, and SHA-256 hashing.

Canonical JSON serialization is not an appropriate first target for ad hoc SIMD. Field order, escaping, numeric formatting, digest-field zeroing, and exact byte equality are correctness contracts. A vectorized path that changes whitespace, escaping, ordering, or numeric output would break canonicality. SHA-256 may benefit from optimized platform implementations, but replacing the existing digest path would require runtime feature detection, scalar fallback, byte-for-byte golden vectors, and separate benchmarks. Phase 72 therefore makes no architecture-specific serialization change. The recommended next profile is to separate semantic verification, `serde_json` assembly, frame byte copies, and hashing before considering SIMD.

## Coverage evidence

`tests/phase72_emission_diagnostic_transport_integration.rs` passes **4/4 tests**. Coverage includes valid async send/receive, pending polling and wakeup, close behavior, queue capacity, invalid identifiers, stale candidate failure, bounded source count, contiguous sequence admission, replay and gap rejection, deterministic summary/digest equality, and retained Phase 70 nested verification. The transport digest binds metadata before stream parsing; rejected observations are not committed to aggregate state.

## Authority boundary

Source IDs, sequence numbers, transport digests, and aggregate digests are local data. They do not prove source identity, authenticate a remote peer, establish trust, form a quorum, authorize an action, or mutate a cluster. The implementation is in-memory and read-only with respect to external systems. No source text, secrets, private keys, process execution, filesystem access, persistence, network I/O, or external side effects are introduced.

## Reproduction

```bash
cd /home/ubuntu/un1c0
source "$HOME/.cargo/env"
cargo test --test phase72_emission_diagnostic_transport_integration -- --nocapture
cargo run --example phase72_emission_diagnostic_transport_benchmark > benchmarks/phase72_emission_diagnostic_transport.json
python3 -m json.tool benchmarks/phase72_emission_diagnostic_transport.json >/dev/null
```

## Next boundary

A later phase may add explicit per-source transport capabilities or durable handoff, but only after defining an authenticated envelope, replay-window lifecycle, persistence rollback behavior, and an external authority boundary. Those features must not be inferred from this local aggregate digest.
