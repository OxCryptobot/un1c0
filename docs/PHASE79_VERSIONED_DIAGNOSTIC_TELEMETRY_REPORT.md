# Phase 79 Versioned Diagnostic Telemetry Report

**Author:** Manus AI
**Status:** F78.0–F78.4 telemetry, canonical-byte, collector, and journal-ordering gates are implemented and validated locally. Formal Phase 79 identity/audit work remains pending.

## Scope decision

The requested Phase 79 work began at the boundary between the Phase 78 observability plan and the formal Phase 79 identity/audit plan. This batch completes the remaining Phase 78 telemetry gates: strict schema/redaction, non-authoritative collector failure, and deterministic journal ordering. It does not claim that service identity, signed external audit evidence, or durable outbox delivery has been implemented.

> Telemetry remains observational. Its serialization, validation, collection, or persistence status cannot change verification acceptance, evidence validity, replay state, aggregate mutation, or authority decisions.

## Implemented schema

`DiagnosticInstrumentationSnapshot` now has fallible versioned JSON helpers backed by a strict `DiagnosticTelemetryEnvelope`.

| Property | Contract |
|---|---|
| Schema version | Fixed version `1`; unsupported versions are rejected. |
| Event type | Fixed `diagnostic_instrumentation_snapshot`; other event types are rejected. |
| Unknown fields | `deny_unknown_fields` applies to the envelope, snapshot, samples, stage timings, and counters. |
| Numeric bounds | Maximum 512 samples; frame counts must be 1–32; stream bytes must not exceed the stream limit. |
| Encoding | Compact deterministic JSON; non-canonical input is rejected on decode. |
| Redaction | Explicit numeric fields and three approved outcome strings only; no arbitrary metadata or payload fields. |
| Failure semantics | Schema errors are typed and observational; they do not alter verification results or state mutation. |

The public `to_versioned_json` method validates before emission and enforces a 2 MiB envelope bound. The public `from_versioned_json` method checks input size, strict structure, schema version, event type, canonical encoding, and sample bounds before returning a snapshot. Dropped samples remain represented by the bounded `dropped_samples` counter rather than being silently fabricated or treated as verification failures.

## Automated redaction evidence

The scanner at `scripts/scan_diagnostic_telemetry_redaction.py` walks the generated telemetry JSON, rejects keys outside the explicit allowlist, rejects raw-sensitive field-name patterns, accepts only the fixed event/outcome strings, and checks the versioned envelope and sample bound. The generated artifact at `benchmarks/phase79_diagnostic_telemetry.json` contains two samples and passes with `redaction_scan=pass`; no raw source, prompts, tokens, credentials, keys, signatures, or canonical diagnostic bytes are present.[1]

This scan is intentionally narrower than a general repository secret scanner: it verifies the diagnostic telemetry serialization boundary rather than searching unrelated project code or historical artifacts. The scan should remain a required release check for every telemetry artifact emitted by later phases.

## Test evidence

The new `tests/phase79_diagnostic_telemetry_integration.rs` covers the promoted schema boundary.

| Test area | Evidence |
|---|---|
| Round trip | Versioned envelope serializes and deserializes byte-for-byte with the expected schema and event identifiers. |
| Allowlist | Recursive key scan confirms only approved telemetry fields occur. |
| Strictness | Unknown top-level and nested fields are rejected. |
| Version/event binding | Wrong schema versions and event types are rejected. |
| Canonicality | Appended non-canonical whitespace is rejected. |
| Bounds | Excess samples and invalid frame counts are rejected. |
| Non-authority | Dropped samples remain an observation and do not invalidate the snapshot. |
| Collector overflow | A full bounded collector returns a typed queue-full error while verified receiver acceptance and aggregate state remain unchanged. |
| Journal ordering | Verified evidence is preflighted, journaled with deterministic sequence/hash-chain fields, and only then admitted to the aggregate; full-journal and rejected-verification paths leave state unchanged. |

The existing Phase 75 instrumentation tests continue to pass, including accepted/rejected outcomes, disabled instrumentation, bounded sample collection, and canonical-byte reuse attribution. The expanded focused Phase 75 plus F78.3/F78.4 regression run passed **13 tests with zero failures**. The complete Rust all-target suite passed **436 tests with zero failures**; formatter, redaction-scan, reusable-skill validation, and whitespace checks also passed.

## Remaining Phase 78 gates

| Gate | Status | Decision |
|---|---|---|
| F78.0 canonical-byte reuse | **Pass** | Completed in Phase 78 with byte-identical golden-vector tests and sanitized frame-count profiling. |
| F78.1 schema stability | **Pass** | Versioned strict envelope, canonical round trip, unknown-field rejection, event/version binding, and bounds are implemented and tested. |
| F78.2 redaction | **Pass for sanitized telemetry artifacts** | Allowlist-based scanner passes on the generated artifact. Repeat the scan for every future telemetry artifact and before release. |
| F78.3 non-authority | **Pass** | Bounded collector capacity, schema collection failure, and queue overflow are typed; `ingest_verified_with_telemetry` ignores collector failure while preserving acceptance and aggregate mutation. |
| F78.4 journal ordering | **Pass** | `DiagnosticObservationJournal` uses deterministic sequence/hash-chain entries; receiver order is verify → aggregate preflight → journal append → authorized mutation, with full-journal and rejected-verification no-mutation tests. |

## Formal Phase 79 prerequisite review

| Prerequisite | Status | Required before formal identity/audit promotion |
|---|---|---|
| Content-versus-service identity separation | **Ready as a prerequisite; identity layer not implemented** | Add a separately governed service identity envelope; never treat the Phase 73/75 attestation key as service authorization. |
| Independent signer rotation and revocation | **Pending** | Add generation-bound identity keys, revocation checks, old-identity rejection, and non-retroactive validation tests. |
| Exact audit binding | **Pending** | Bind each audit record to service identity, evidence digest, stream, sequence, predecessor, and trust configuration generation. |
| Durable outbox and replay-safe acknowledgement | **Pending** | Add atomic persistence, crash/restart recovery, idempotent enqueue, signed acknowledgements, and no-ack-before-durable-commit tests. |
| F78.3 collector non-authority | **Pass** | Bounded collector and schema failures are ignored at the authority boundary; accepted verification and aggregate mutation remain correct. |
| F78.4 deterministic journal ordering | **Pass** | Process-local journal append precedes authorized aggregate mutation after verification and preflight; full/rejected paths do not mutate. |

## Formal Phase 79 boundary

The formal Phase 79 identity/audit work remains unimplemented. It must separate service identity from Phase 73/75 content integrity, introduce independent key rotation/revocation, bind audit records to exact evidence and sequence context, and prove durable outbox recovery before any external sink is treated as authoritative.[2] The completed F78.4 journal is process-local diagnostic ordering evidence, not an external identity or audit authority.

## References

[1]: ../benchmarks/phase79_diagnostic_telemetry.json "Phase 79 sanitized versioned diagnostic telemetry artifact"

[2]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"
