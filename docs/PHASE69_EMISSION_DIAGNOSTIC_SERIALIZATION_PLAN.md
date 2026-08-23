# Phase 69: bounded diagnostic serialization

## Objective

Phase 68 compares two current-verified local diagnostic reports. Phase 69 adds a bounded canonical JSON envelope so a diagnostic report can be transported or stored as data without embedding execution authority, while preserving integrity and current-envelope verification.

## Contract

`EmissionDiagnosticReport::to_json` emits a versioned envelope containing only the target label, non-zero batch ID, profile key, unit-root map, aggregate statistics, output digest, observation count, typed entries, and a domain-separated SHA-256 integrity digest. The serialized size is capped at 64 KiB and unit count at 256.

`EmissionDiagnosticReport::from_json_for` rejects oversized or malformed data, unknown fields, non-canonical JSON, invalid IDs, zero observations, integrity mismatches, and non-canonical entries. It rehydrates the aggregate only through a crate-private constructor and immediately delegates to `from_verified_aggregate`, which requires the current snapshot, target profile, complete candidate-unit map, and exact roots to verify.

## Fail-closed rules

No parsed envelope is returned before integrity and current-envelope checks pass. Serialized data cannot carry source text, prompts, model output, private keys, signatures, filesystem paths, network metadata, process directives, or secrets. The envelope is an evidence representation, not an authorization token or distributed-trust object.

## Coverage matrix

The test suite covers canonical round trips, deterministic output, malformed JSON, unknown fields, pretty/non-canonical JSON, integrity tampering, invalid target, invalid unit IDs, zero observations, inconsistent entries, oversized input, stale candidate state, and current profile binding.

## Benchmark method

Use four units and eight functions per unit with 1/2/4/8 equivalent observations. Record serialized bytes, canonical serialization p50/p95, verification-gated rehydration p50/p95, errors, and sanitized authority markers over 64 samples per row.
