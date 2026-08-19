# Phase 37: Secure Consensus Telemetry and Failover Orchestration

## Purpose

Phase 37 adds a local-first control-plane layer for **secure multi-region consensus telemetry** and **automated failover orchestration**. The implementation remains transport-agnostic and deterministic: telemetry is authenticated before admission, bounded before storage, and recorded in an append-only hash-linked journal. Failover promotion is represented by a typed state machine with a durable-intent seam, an explicit freshness gate, and an external-fence boundary before commit.

The phase does not claim that a local process can prove cloud-region health, DNS convergence, process fencing, or storage replication by itself. Those facts remain deployment inputs. The local core proves that only authenticated, fresh, correctly bound, monotonic evidence can influence a failover decision, and that a committed operation cannot be silently replaced or rolled back.

## Contracts

| Contract | Responsibility | Safety property |
|---|---|---|
| `ConsensusTelemetryEvent` | Canonical signed observation with cluster/resource, producer, region, authority epoch, sequence, bounded labels, bounded metrics, TTL, and kind | Ed25519 signature and SHA-256 event digest bind the complete event content |
| `TelemetryKeyRegistry` | Pinned producer identity registry | Unknown producers and key rebinding fail closed |
| `SecureTelemetryReceiver` | Replay-protected admission and bounded hash-linked journal | Regressed epochs/sequences, stale evidence, conflicting sequence identity, and capacity overflow do not mutate the journal |
| `FailoverOrchestrator` | Typed detection, evidence collection, fence admission, and commit lifecycle | Promotion requires fresh same-epoch evidence and an exact external-fence admission |
| `EpochChurnFuzzReport` | Sanitized deterministic fuzz outcome | Fuzzing records counts, bounds, seed-derived trace digest, and panic count only; no keys, signatures, payloads, or fencing material are emitted |

## Typed lifecycle

The orchestrator uses six phases. `Idle` is the initial state. `DetectingFailure` records an explicit detection intent. Valid telemetry moves the instance into `CollectingEvidence`. Once all required evidence is fresh and matches the authority epoch, a failover intent enters `AwaitingFence`. An exact external-fence admission is required before `Committed`. Any conflicting same-kind epoch/sequence observation moves the state to `Failed`, and a committed operation cannot be downgraded or replaced.

```text
Idle
  -> DetectingFailure
  -> CollectingEvidence
  -> AwaitingFence
  -> Committed

CollectingEvidence --missing/stale/conflicting evidence--> Failed or remains gated
AwaitingFence --wrong operation/digest/no fence--> rejected without commit
Committed --same operation and digest--> idempotent AlreadyCommitted
Committed --different operation--> rejected terminally
```

The `FailoverIntent` structure is the durable-intent seam. It contains only the operation identity, decision digest, candidate region, authority epoch, prepared tick, and optional fence-admission tick. The current local implementation keeps the intent in memory because persistence ownership belongs to the recovery authority; callers that require restart continuity must atomically persist and restore the intent through the existing recovery snapshot boundary.

## Telemetry admission invariants

Telemetry admission is fail-closed and ordered as follows:

1. The domain, protocol version, identifiers, TTL, key length, signature length, digest shape, label count, metric count, and control-character constraints are validated.
2. The producer is resolved through a pinned `TelemetryKeyRegistry`; a public-key mismatch is rejected before any state mutation.
3. The event signature is verified over the canonical payload, and the event digest is recomputed over the signed content.
4. Current-tick freshness is checked against the event's observed tick and TTL.
5. Exact event-hash duplicates are idempotent. A lower authority epoch or lower sequence is rejected. A same-epoch, same-sequence different event is a conflict.
6. The journal capacity is checked before append. Accepted entries point to the prior event hash, producing a bounded append-only chain.

The journal uses `BTreeMap` and `BTreeSet` structures for deterministic ordering. Labels and metrics are bounded independently, while the journal has a hard maximum of 4,096 entries. Identifiers and label values are length-bounded and reject control characters. These controls are observable through sanitized report fields rather than raw event bodies.

## Failover gate invariants

`FailoverOrchestrator::begin_failover` cannot prepare an intent until every configured telemetry kind exists, is inside the configured age window, and has the requested authority epoch. A missing or stale event leaves the orchestration in evidence collection and produces no intent. The fence admission method binds the exact operation ID and decision digest to the prepared intent. Commit requires that fence admission and changes the active region only once. Repeating the exact operation and digest is safe; changing either value is rejected.

The gate therefore separates three properties that are often incorrectly conflated: **evidence freshness**, **authority fencing**, and **promotion commit**. Fresh telemetry alone never grants write authority, and an external fence without the exact prepared decision cannot promote a candidate.

## Targeted epoch-churn fuzzing

The phase exposes two deterministic fuzz harnesses. Each harness uses a bounded xorshift stream derived from a caller-provided seed, generates high connection epochs and sequence churn, mutates only local copies of authenticated records, and wraps the admission boundary in `catch_unwind`.

| Harness | Mutations | Required outcome |
|---|---|---|
| `fuzz_authenticated_transport_receiver` | Zero epochs/sequences, oversized payloads, payload-hash changes, signature changes, receiver control characters, unknown senders, repeated and stale epochs | No panic; accepted/rejected counts equal iterations; replay window remains bounded |
| `fuzz_witness_reservation_store` | Zero generations, hash mismatches, malformed digests, control characters, duplicate rounds, and injected pre-stage/after-stage/after-sync crash points | No panic; staging cleanup remains safe; accepted/rejected counts equal iterations; durable store remains bounded |

The harness report contains `iterations`, `accepted`, `rejected`, `panics`, `max_connection_epoch`, and a SHA-256 digest of those counters. It intentionally excludes random inputs, raw payloads, private keys, signatures, and full fencing tokens. The integration suite runs 1,024 iterations per harness; the benchmark example runs 4,096 iterations per harness.

## Evidence and compliance

Phase 37 adds eight gates to the previous 121-gate baseline, producing a 129-gate artifact:

| Gate | Evidence |
|---|---|
| `consensus_telemetry_signature_required` | Signed canonical events verify against the pinned producer registry |
| `telemetry_producer_registry_bound` | Unknown and rebound producer keys fail closed |
| `telemetry_epoch_sequence_monotonic` | Regressed epoch/sequence evidence is rejected without journal mutation |
| `telemetry_journal_hash_chained` | Accepted entries retain the prior digest and pass bounded chain inspection |
| `failover_orchestration_phase_typed` | Detection, evidence, fence, commit, and terminal-state transitions are typed |
| `stale_telemetry_blocks_promotion` | Missing, expired, or wrong-epoch telemetry prevents intent preparation |
| `transport_receiver_epoch_churn_safe` | Transport fuzz reports zero panics under high epoch churn |
| `reservation_store_fuzz_no_panic` | Reservation-store fuzz reports zero panics across persistence crash points |

The collector and independent auditor record only boolean gate outcomes, bounded counts, phase summaries, and sanitized benchmark metadata. The dedicated validator runs focused Phase 36/37 integration suites, the Phase 37 benchmark, formatting checks, and structural evidence checks. The complete compliance validator invokes it after Phase 36 and before the isolated Compose mTLS check.

## Production boundaries

The local module does not own cloud-region failure detection, quorum membership governance, managed key custody, mTLS certificate lifecycle, external fence issuance, process termination, DNS or load-balancer convergence, distributed filesystem guarantees, or durable cross-process intent replication. A production deployment must bind those inputs to the same cluster/resource, authority epoch, operation digest, and external-fence contract before permitting promotion. The local gate is deliberately conservative: uncertainty or stale evidence blocks progress rather than guessing.
