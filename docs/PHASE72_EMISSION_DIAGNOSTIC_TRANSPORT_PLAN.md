# Phase 72: bounded diagnostic transport and distributed aggregation

## Objective

Phase 72 extends the Phase 70 diagnostic stream with a bounded, distributed-shaped fan-in layer and an asynchronous transport abstraction. The implementation is intentionally local and in-memory: it models source identity, ordered observations, bounded queues, wakeable polling, replay/gap rejection, and deterministic aggregate evidence without opening sockets or claiming distributed trust.

## Components

`AsyncDiagnosticTransport` owns a bounded `ArrayQueue` of diagnostic stream frames. `send` validates non-zero source and sequence identifiers, serializes a verified stream through its canonical Phase 70 representation, binds source ID and sequence to a domain-separated transport digest, and admits the frame only when the queue has capacity and is open. `try_receive_for` and `receive_for` decode frames only through Phase 70 current-envelope verification. `poll_receive_for` provides a `Future`-compatible wakeable path without requiring a runtime dependency; send and close wake registered waiters.

`DistributedEmissionAggregator` accepts verified source observations in deterministic local source-map order. It allows at most eight source IDs, at most 256 accumulated frames, and a cumulative bound derived from the stream frame limit. Every source must start at sequence one and advance contiguously. Replays and gaps are typed errors. The aggregate maintains cumulative frame/byte accounting and a domain-separated digest over accepted source IDs, latest sequences, stream digests, and totals.

## Verification and failure ordering

Transport admission checks source/sequence shape, canonical stream serialization, queue lifecycle, and capacity. Receive verifies transport metadata before parsing the stream. Stream parsing retains the Phase 70 order: input size, strict envelope shape, stream digest, canonical bytes, exact context, sequence/frame bounds, and nested current-envelope verification. Aggregation then re-verifies the decoded stream and checks source replay/gap rules and aggregate limits before mutating state.

The aggregator mutates only after every check for an observation succeeds. A rejected replay, gap, stale stream, context mismatch, queue-full send, closed transport, or source/frame/byte overflow leaves prior aggregate state unchanged. Source-map ordering makes equivalent accepted histories produce identical summaries and aggregate digests.

## Authority boundary

Phase 72 does not implement sockets, network I/O, authentication, signatures, durable queues, cluster membership, quorum, leader election, trust, authorization, persistence, retries, processes, filesystem access, secret reads, or external side effects. Source IDs and transport digests are local integrity/order metadata. They are not identities issued by an authority and cannot authorize an action.

## Coverage-first matrix

The integration suite must cover valid send/receive, empty-queue pending behavior, wakeup after send, close wakeup and send rejection, queue capacity, invalid source/sequence IDs, stale candidate rejection, source-count limit, contiguous source sequences, replay rejection, gap rejection, deterministic summary/digest equality, cumulative frame/byte accounting, and all-or-nothing state preservation. Tests must retain the Phase 70 1–32 property coverage.

## Benchmark protocol

Use deterministic local fixtures and 64 samples at one, two, four, and eight source IDs, with four frames per source. Record source count, total frames, p50/p95/p99 end-to-end local transport-plus-aggregation latency, errors, and sanitized authority markers. Do not infer network throughput or distributed production behavior from these measurements.

## SIMD feasibility boundary

Phase 71 measured 5.987% p50 improvement by removing repeated verification and serialization, while the remaining Phase 71 template build still spends time on one semantic verification, frame allocation/cloning, stream JSON assembly, and SHA-256 digesting. SIMD is not a safe first optimization for canonical JSON: generic `serde_json` encoding and SHA-256 already have correctness-sensitive ordering and platform-dependent implementation choices. Any future SIMD work should be isolated behind byte-for-byte golden tests, runtime feature detection, scalar fallback, and benchmark evidence that separates semantic verification, JSON assembly, memory copies, and hashing. Phase 72 therefore optimizes architecture and bounded queue behavior rather than introducing unsafe or architecture-specific byte serialization.

## Closeout gates

Complete the implementation only after exporting the module, adding typed tests and benchmark JSON, updating the roadmap and reusable skill, validating formatting and the skill package, running all Phase 67–72 targeted suites and the complete all-target Rust suite, checking sanitized artifacts, excluding generated build noise, and committing only intended files.
