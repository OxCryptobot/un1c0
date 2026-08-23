# Phase 66: emission-receipt aggregation and comparison

## Objective

Phase 65 produced deterministic local emission receipts. Phase 66 adds an aggregate comparison boundary for repeated observations of the same emission state without promoting local receipts to remote authority.

## Contract

`EmissionReceiptAggregate::from_receipts` accepts a non-empty slice and requires every observation to match the first receipt on target, batch ID, profile key, complete unit-root map, chunk count, byte count, and output digest. It stores one canonical observation plus the count of equivalent observations.

`verify_for` reconstructs a receipt-shaped view and delegates to the existing exact envelope verifier. It therefore rechecks the current semantic snapshot, target/profile, batch ID, unit set, and candidate roots instead of trusting the aggregate as a standalone authorization.

## Fail-closed rules

Empty inputs, target drift, batch drift, profile drift, unit-root drift, statistics drift, digest drift, and current-envelope verification failures are typed errors. The aggregate is never created from partial or divergent observations.

## Benchmark method

Use a fixed four-unit, 32-function Rust fixture and compare aggregation alone with aggregate construction plus current-state verification over 1/2/4/8 equivalent observations. Record p50/p95, 64 samples per row, chunk count, errors, and sanitized authority markers.
