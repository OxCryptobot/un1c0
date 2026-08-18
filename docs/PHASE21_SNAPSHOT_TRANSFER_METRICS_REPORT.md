# Phase 21 Snapshot-Transfer Metrics, Bandwidth Accounting, and Cancellation

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded snapshot-transfer observability and per-follower flow control
**Status:** Implemented and integration-tested

## Executive summary

Phase 21 extends the Phase 20 snapshot install-readiness state machine with typed per-follower transfer metrics, rolling bandwidth windows, exact retry boundaries, complete byte-accounting admission, and explicit cancellation. The implementation remains transport-agnostic: the consensus core returns typed decisions and records bounded state, while callers retain responsibility for chunk transport, scheduling, persistence, authenticated delivery, and metrics export.

The current repository compliance artifact contains **36 passing gates**: the 32 gates present after Phase 20 plus four Phase 21 gates for transfer metrics, bandwidth backpressure, cancellation, and complete install accounting. The full compliance suite passed, the dedicated Phase 21 suite passed four tests, the Phase 20 suite passed four regression tests, and the detailed artifact audit passed with zero findings.

## Typed contracts

| Contract | Responsibility |
|---|---|
| `SnapshotBandwidthConfig` | Bounds per-window bytes and window duration. |
| `SnapshotTransferMetrics` | Exposes active transfer, snapshot size, bytes sent/remaining, window usage/limit, and outcome counters. |
| `SnapshotTransferProgressAction` | Returns `Accepted` progress or `Backpressured` with available bytes and exact retry tick. |
| `SnapshotTransferCancellation` | Returns the cancelled follower, transfer ID, and bounded retry deadline. |
| `SnapshotInstallReadiness::Cancelled` | Makes cancellation visible without conflating it with rejection or installation. |

`set_snapshot_bandwidth_config` validates the configuration before mutation. `record_snapshot_transfer_progress` requires leadership, a known follower, a valid active transfer ID, positive bounded bytes, a trusted monotonic tick, and a request that fits the remaining snapshot frontier. A window resets only when the exact window duration has elapsed. Each follower owns independent counters and window state, so one congested follower cannot consume another follower’s quota.

An `Installed` acknowledgement now fails closed until `bytes_sent` reaches the serialized snapshot size. This prevents an install-ready control message from advancing replication before the data plane has reported complete transfer accounting. Successful installation clears active byte state and advances replication only through the existing Phase 20 installed path.

Cancellation requires leadership, a valid active transfer ID, a bounded non-control reason, and a trusted tick. It clears the active transfer and byte counters, marks the readiness state `Cancelled`, increments the cancellation counter, and sets the standard bounded retry deadline. Cancellation does not advance replication progress and does not itself grant transport, storage, or scheduling authority.

## Integration evidence

| Test | Covered behavior | Result |
|---|---|---|
| `bandwidth_backpressure_is_bounded_and_metrics_are_per_follower` | Isolated follower counters, rolling window exhaustion, exact retry tick, window reset, and independent follower progress | Passed |
| `installed_ack_requires_complete_accounting_and_preserves_progress_boundary` | Installed-before-complete rejection, full byte accounting, staged-to-installed completion, and frontier advancement | Passed |
| `cancellation_releases_active_transfer_but_obeys_exact_retry_boundary` | Active-transfer clearing, cancellation metrics, exact retry boundary, retry release, stale ID, and invalid reason | Passed |
| `cancellation_and_progress_fail_closed_for_clock_regression_and_unknown_followers` | Clock regression, unknown peers, and preservation of active transfer authority after rejected requests | Passed |

The Phase 20 regression suite also passed all four existing tests after being updated to report complete byte accounting before its installed acknowledgement.

## Production boundary

The core deliberately does not persist transfer counters or cancellation intent, own a bandwidth scheduler, open sockets, transmit chunks, stage files, authenticate remote acknowledgements, or export telemetry. Production integration must persist cancellation intent across restart, connect metrics to a durable or loss-tolerant telemetry pipeline, enforce quotas at the authenticated socket/chunk layer, and test cancellation during process loss, network partition, storage failure, and follower restart.

## References

[1]: ../src/consensus.rs "Phase 21 consensus contracts and state machine"
[2]: ../tests/phase21_snapshot_transfer_metrics_integration.rs "Phase 21 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Non-secret security compliance metrics"
[4]: ../benchmarks/security_compliance_audit.json "Detailed security metrics audit"
