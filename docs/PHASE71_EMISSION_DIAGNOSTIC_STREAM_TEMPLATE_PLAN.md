# Phase 71: verified diagnostic stream templates

## Objective

Phase 70 supports bounded local diagnostic streams but its original construction path repeated current-state verification and canonical serialization for every equivalent frame. Phase 71 introduces `EmissionDiagnosticStreamTemplate`, a verified immutable report template that reuses one canonical Phase 69 frame encoding while building a bounded stream with up to 32 equivalent frames.

The optimization is deliberately narrow. It removes repeated work only after a report has passed current-envelope verification and canonical serialization. It does not weaken parsing, stream integrity, frame ordering, context checks, or nested verification for untrusted input.

## Bottleneck diagnosis

The Phase 70 stream benchmark used four units, 32 functions, and 64 samples per frame count. At 32 frames, construction p50 was 52,857,586 ns and verification p50 was 52,173,680 ns. The hot path was repeated `EmissionDiagnosticReport::verify_for` plus `to_json` work inside the frame loop. Each equivalent frame carried the same aggregate, roots, statistics, digest, and four typed entries, so recomputing semantic roots and re-encoding identical bytes produced no new evidence.

The optimization target is construction of equivalent observations, not parsing of untrusted streams. Stream parsing must continue to verify every nested frame because serialized input is untrusted and each frame must be checked against the current snapshot.

## Typed contract

`EmissionDiagnosticStreamTemplate::from_report` verifies one report against the current snapshot, target profile, and candidate map, then stores the report, its canonical Phase 69 bytes, and exact aggregate context. `build` requires a non-zero stream ID and frame count from 1 through `MAX_STREAM_FRAMES`, re-verifies the template report once against the caller context, checks the immutable canonical encoding, clones the exact frame bytes into each bounded sequence slot, computes the Phase 70 stream digest, and validates the resulting stream envelope.

The existing `EmissionDiagnosticStream::from_verified_reports` path also reuses the first canonical frame bytes for reports that are exactly equal to the first report. Divergent reports still pass individual current-state verification, exact target/batch/profile/unit-root checks, and canonical serialization.

## Security and correctness invariants

The template is not a trust token, signature, authorization grant, quorum certificate, or distributed observation. It is a local in-memory optimization object. It has no filesystem, process, network, secret, signing, persistence, retry, or cluster authority.

A template cannot be built from an unverified report. A template cannot build an empty or over-limit stream. A template must be re-verified against the current caller context on every build, so using it with stale candidates or a mismatched snapshot fails closed. The cached bytes come only from Phase 69 canonical serialization and are immutable through the public API. Stream frames retain the original report and exact bytes; the resulting stream still rechecks its digest and all bounds.

## Coverage matrix

The Phase 71 coverage must include template creation from a valid report, byte identity with direct Phase 69 serialization, exact equality between template-built and repeated-report streams, deterministic frame counts from 1 through 32, zero and over-limit frame rejection, zero stream-ID rejection, stale candidate rejection at template build time, and existing Phase 70 malformed/canonical/integrity/sequence/context rejection.

The complete suite must include all Phase 67–70 targeted integration tests and the complete all-target Rust suite. The benchmark must report zero errors, the fixed 32-frame workload, 64 samples, p50/p95/p99 values, sanitized authority markers, and both a controlled legacy repeated-work baseline and the optimized template path.

## Benchmark protocol

Use the deterministic Phase 70 fixture: four units, eight functions per unit, 32 functions total, Rust target, and 64 samples. At exactly 32 frames, compare `legacy_repeated_work`—one current-state verification and canonical serialization per frame—against `EmissionDiagnosticStreamTemplate::build`. Also record the current `from_verified_reports` implementation to distinguish template savings from the general report-list path. Preserve JSON in `benchmarks/phase71_emission_diagnostic_stream_template.json`.

Do not infer production scalability from this local benchmark. The expected benefit applies to equivalent in-memory reports. Mixed or divergent reports retain their individual validation path, and untrusted serialized streams retain full nested verification.

## Closeout gates

Phase 71 is complete only when the module is exported, the reusable skill and roadmap are updated, the deterministic 1–32 property test passes, stale-state and bound failures are typed, the benchmark is valid JSON and contains no source/secrets, the skill validator passes, formatting passes, all targeted Phase 67–71 suites pass, the complete all-target suite passes, generated build noise is excluded from the commit, and the local commit is recorded. Remote publication remains a separate GitHub credential boundary.
