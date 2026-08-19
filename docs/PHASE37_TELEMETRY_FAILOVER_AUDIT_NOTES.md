# Phase 37 Secure Telemetry and Failover Audit Notes

## Baseline

The published Phase 36 head is `f039d8c` and the worktree is clean at baseline. The Phase 36 authenticated transport integration suite passes 6 tests. The repository emits only pre-existing warnings in the legacy `src/subagent.rs` and Go walker paths. The updated reusable `agentic-system-engineering` skill already contains the repeatable batch closeout sequence and Phase 36 transport navigation, but its workflow reference should be made explicit and validated as a reusable delivery artifact.

## Observed Phase 36 guarantees

Phase 36 verifies signed, receiver-bound transport envelopes; cluster/resource and payload binding; connection-epoch replay protection; durable witness reservations with crash-boundary injection; exact external-fence protected-write admission; exact operation replay idempotence; and deterministic drop/duplicate transport chaos. Metrics are sanitized and do not persist raw keys, signatures, or complete fencing tokens.

## Phase 37 gaps and design targets

| Severity | Finding | Impact | Remediation |
|---|---|---|---|
| High | Consensus telemetry is represented mostly as benchmark JSON rather than typed signed events with bounded cardinality and monotonic sequence. | Operators cannot distinguish stale, replayed, or cross-region telemetry from current authority evidence. | Add `ConsensusTelemetryEvent`, signed canonical payloads, sequence/epoch binding, bounded labels, and a hash-chained telemetry journal. |
| High | Failover orchestration is distributed across authority calls and benchmarks without an explicit idempotent workflow state machine. | Retries, partial observation, and restart can trigger duplicate promotion attempts or hide missing external-fence steps. | Add typed orchestration phases, retry/backoff policy, durable intent, exact decision binding, and terminal success/failure outcomes. |
| High | Automatic failover lacks a telemetry gate that blocks promotion when witness, transport, fencing, or snapshot evidence is stale. | A controller could promote with incomplete observability or stale evidence. | Require fresh signed health/fence/transport evidence and reject stale or conflicting telemetry before orchestration mutation. |
| Medium | Authenticated receiver and reservation store are tested with examples but not broad structured fuzzing under high connection-epoch churn. | Malformed envelopes, oversized fields, epoch wrap/rollback, and randomized staging states may expose panics or mutation bugs. | Add bounded deterministic fuzz harnesses with thousands of generated cases, invariants, crash-state fixtures, and sanitized counters. |
| Medium | Telemetry cardinality and trace storage bounds are not enforced as a first-class contract. | High-churn regions or hostile labels could exhaust memory or create unbounded evidence. | Enforce label/trace bounds and redact payload content before journaling. |

## Phase 37 invariants

1. Telemetry signatures cover a fixed domain, cluster/resource, producer, region, authority epoch, sequence, event kind, and canonical bounded fields.
2. Telemetry receivers reject unknown producers, wrong cluster/resource, stale epochs, non-monotonic sequences, conflicting same-sequence digests, oversized labels, and control characters before journal mutation.
3. A telemetry journal is append-only, hash-chained, bounded, replayable, and exact duplicate idempotent.
4. Failover orchestration advances only through typed phases and only after fresh required telemetry and exact external-fence admission.
5. Retries are idempotent by operation/decision identity; terminal success cannot be downgraded by later stale evidence.
6. Fuzzing never panics, never accepts malformed input, and never changes state on rejected envelopes or reservation snapshots.
7. Automated failover remains fail-closed when telemetry is absent, stale, conflicting, or outside bounded resource limits.

## Boundaries

Phase 37 local evidence will validate typed telemetry and orchestration contracts plus deterministic fuzz inputs. It will not claim production metrics transport, cross-region clock truth, external alerting, process supervision, cloud DNS/routing control, or real hardware fencing.
