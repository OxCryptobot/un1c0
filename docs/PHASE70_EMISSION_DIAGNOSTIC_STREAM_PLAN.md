# Phase 70: bounded local diagnostic stream plan

## Objective

Phase 69 provides a canonical, integrity-bound JSON envelope for one `EmissionDiagnosticReport`. Phase 70 should add a **bounded local diagnostic stream** that frames a finite sequence of Phase 69 envelopes for local replay, comparison, or inspection without becoming a transport service, persistent journal, trust protocol, or authorization mechanism.

The design should preserve the existing invariant: serialized diagnostic data is untrusted input, and no report or derived comparison is usable until integrity, canonical encoding, exact context, and current semantic-envelope verification all succeed.

## Recommended scope

Create `src/emission_diagnostic_stream.rs` with a typed `EmissionDiagnosticStream` and `EmissionDiagnosticStreamError`. The stream should contain a version, a non-zero stream ID, an exact expected target/profile/batch context, a bounded sequence of frames, and a final stream digest. Each frame should contain a monotonically increasing local sequence number and one canonical Phase 69 envelope. Use a separate domain separator, for example `un1c0/phase70/emission-diagnostic-stream/v1`, so a stream digest cannot be confused with a Phase 67 evidence digest or a Phase 69 envelope digest.

The stream should be constructed only from already verified reports. Construction must enforce one target, one batch, one profile key, one unit-root map, and one serialized-envelope size limit across all frames. It should reject empty streams, duplicate or gapped sequence numbers, mismatched contexts, excessive frame counts, cumulative byte overflow, and cumulative digest mismatch. Parsing must be canonical and integrity-bound, and must parse each frame through `EmissionDiagnosticReport::from_json_for` using the caller-provided current snapshot, profile, and candidate-unit map.

This phase should not add sockets, files, network access, background workers, retries, cluster state, signing keys, quorum logic, distributed trust, or authorization. A stream digest detects accidental or untrusted-data mutation; it is not an authenticity proof and must not be treated as a bearer token.

## Proposed typed contract

| Contract | Required behavior |
|---|---|
| `MAX_STREAM_FRAMES` | Fixed small bound, recommended 32 or 64; reject larger streams before allocation. |
| `MAX_STREAM_BYTES` | Fixed cumulative bound, recommended 256 KiB; reject before parsing or buffering beyond the limit. |
| `MAX_FRAME_BYTES` | Must not exceed `MAX_SERIALIZED_DIAGNOSTIC_BYTES` from Phase 69. |
| Stream ID | Non-zero bounded identifier; no control characters if represented as text. |
| Sequence | Start at zero or one by explicit contract, then strictly contiguous with no replay/gap acceptance. |
| Context | Exact target, batch ID, profile key, unit-root map, and report context across all frames. |
| Frame validation | Canonical Phase 69 bytes, envelope integrity valid, current snapshot valid, candidate roots valid, canonical entries valid. |
| Stream digest | Domain-separated fixed-size digest over canonical stream metadata and frame bytes. |
| Output | Verified reports plus descriptive frame statistics only; no action or authority decision. |

The exact limits should be constants with typed errors, not magic numbers. `MAX_SERIALIZED_DIAGNOSTIC_BYTES = 64 KiB`, `MAX_SERIALIZED_DIAGNOSTIC_UNITS = 256`, `MAX_DIAGNOSTIC_ENTRIES = 4`, and `MAX_DIAGNOSTIC_ENTRY_BYTES = 128` remain inherited Phase 69/diagnostic limits. The stream’s cumulative bound must be lower than any practical memory budget and must be enforced before allocating a vector from untrusted length fields.

## Verification order

The safe order is:

1. Reject input larger than `MAX_STREAM_BYTES` before JSON parsing.
2. Strictly parse a versioned stream envelope with unknown fields denied.
3. Enforce stream ID, frame-count, cumulative-size, and sequence bounds.
4. Recompute the stream digest over canonical stream metadata and exact frame bytes; reject mismatch.
5. Require canonical stream bytes and canonical frame bytes.
6. Check the stream’s target, batch, profile, and unit-root context against the caller’s expected profile and current snapshot.
7. Parse every frame using `EmissionDiagnosticReport::from_json_for`; do not expose a partial vector if any frame fails.
8. Require adjacent frames to have exactly equivalent context and contiguous sequence numbers.
9. Return the complete verified stream only after all frames pass.

The parser should not perform partial acceptance, best-effort recovery, implicit sorting, duplicate elimination, or unchecked fallback. A failed frame invalidates the complete stream result.

## Coverage matrix

The implementation should include unit tests for digest determinism, domain separation, empty-stream rejection, non-zero IDs, frame-count and cumulative-byte limits, sequence start/continuity, duplicate and gap rejection, frame-order preservation, context mismatch, mixed target/profile/batch/unit roots, stale snapshot and stale candidate rejection, nested Phase 69 integrity mismatch, nested non-canonical JSON, stream-level tampering, unknown fields, malformed JSON, and zero-error complete round trips.

Integration tests should build streams from one, two, four, eight, and 32 equivalent observations. They should verify that a complete stream round trips exactly, that no returned result exists after any frame fails, that stream comparison remains descriptive, and that the stream never invokes a sink, filesystem, process, network, signing, or cluster operation. Add deterministic property-style loops over legal frame counts and malformed sequence positions. Add tests that mutate the final frame, first frame, middle frame, sequence number, context, and digest separately so each failure is attributable to a typed error.

The stream API should expose a bounded summary such as frame count, total bytes, first and last sequence, and digest equality. It should not expose source text or accept caller-supplied pass/fail claims. If Phase 70 adds stream-to-stream comparison, both complete streams must be verified against the same current snapshot before deltas are calculated, matching the Phase 68 dual-verification rule.

## Benchmark design

Reuse the Phase 69 deterministic fixture: four units, eight functions per unit, 32 functions total, Rust target, and 64 samples per row. Benchmark stream construction and verification at frame counts 1/2/4/8/16/32. Record p50, p95, p99, total serialized bytes, per-frame bytes, errors, and false authority markers. Separate cold construction from warm verification if possible. Do not overwrite the Phase 69 JSON artifact; create `benchmarks/phase70_emission_diagnostic_stream.json` and a report with the exact commit and runtime.

Recommended acceptance checks are zero errors, no partial stream results, cumulative size always within the configured limit, and linear or near-linear growth in total bytes. A stream benchmark is expected to scale with frame count because it intentionally retains frame bytes; this differs from Phase 67/68 aggregate/report behavior, which stores counts and fixed-size summaries.

## Phase 69 baseline review

Phase 69 recorded 64 samples for each of four rows, with four units, eight functions per unit, 32 total functions, zero errors, target `rust`, and both authority markers false. Serialized sizes were 1,317, 1,319, 1,323, and 1,327 bytes for 1/2/4/8 observations. Serialization p50 values were 153,769, 148,284, 146,250, and 145,811 ns; p95 values were 207,756, 197,814, 169,175, and 189,562 ns. Verification-gated rehydration p50 values were 895,995, 902,094, 888,690, and 903,081 ns; p95 values were 1,069,864, 1,082,741, 1,019,321, and 1,151,348 ns.

From one to eight observations, serialized size increased by 10 bytes, or approximately 0.759%. Serialization p50 decreased by approximately 5.175% in this local sample, while rehydration p50 increased by approximately 0.791%. The largest observed rehydration p95 was 1,151,348 ns. The largest envelope used approximately 2.025% of the 64 KiB byte ceiling, so Phase 70 must not interpret the baseline as proof that the limit is safe for arbitrary unit identifiers or frame counts. The 64 KiB envelope limit is the effective byte ceiling; the 256-unit limit is an independent structural ceiling.

## Phase 70 closeout gates

The phase is complete only when the new module is exported, the roadmap and reusable skill are updated, all typed errors have integration coverage, benchmark JSON is sanitized, no raw source or secret material appears in serialized artifacts, the skill validator passes, formatting passes, all Phase 67–70 targeted tests pass, the complete Rust all-target suite passes, and the local commit contains no generated build noise. Publication remains a separate GitHub credential boundary.
