# Phase 67: bounded local emission diagnostic report

## Objective

Phase 66 established deterministic aggregation for equivalent local emission receipts. Phase 67 adds a bounded diagnostic projection that explains the aggregate with typed entries while preserving exact current-envelope verification and avoiding any implication of distributed trust or authorization.

## Contract

`EmissionDiagnosticReport::from_receipts` first constructs an `EmissionReceiptAggregate`, then verifies it against the current `SemanticSnapshotEnvelope`, target profile, and candidate UEG map before creating any entries. The report stores the validated aggregate and exactly four typed entries: observation count, emitted chunk count, emitted byte count, and output digest confirmation.

`verify_for` validates the entry bounds and delegates again to the aggregate's current-envelope verifier. Report generation and later consumption therefore cannot use an unchecked aggregate, stale candidate state, incomplete unit map, target drift, or profile drift as valid diagnostic evidence.

## Bounds and fail-closed behavior

The report caps entries at `MAX_DIAGNOSTIC_ENTRIES` and each encoded entry at `MAX_DIAGNOSTIC_ENTRY_BYTES`. All boundary failures use `EmissionDiagnosticError`. Empty or divergent receipt observations remain `ReceiptAggregateError` values wrapped by the diagnostic error; stale snapshots and candidate-root mismatches also fail before a report is returned.

Entries are typed and bounded. They carry no source text, prompts, model output, private keys, signatures, filesystem paths, network metadata, or secrets. Observation counts describe equivalent local observations only; they are not quorum, consensus, freshness, authorization, or trust.

## Benchmark method

Use a deterministic local Rust fixture containing four units and eight functions per unit. Emit one valid receipt, repeat it as 1/2/4/8 equivalent observations, and measure report generation and subsequent `verify_for` calls with 64 samples per row. Record p50/p95 nanoseconds, entry count, emitted chunks, error count, and sanitized authority markers.
