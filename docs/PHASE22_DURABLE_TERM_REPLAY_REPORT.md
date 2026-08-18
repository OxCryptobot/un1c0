# Phase 22 Durable Term/Vote Persistence and Epoch-Bound Replay

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 22 closes two restart-sensitive consensus gaps. First, `DurableConsensusState` and `DurableConsensusStateStore` preserve the node identity, current term, voted-for identity, replay epoch, and replay term floor through a validated atomic file boundary. Second, authenticated envelopes and replay windows now bind nonce acceptance to a signed replay epoch and a minimum accepted term. A monotonic transport epoch rotation clears old nonce windows explicitly rather than silently resetting them after restart.

The full compliance artifact now contains **40 passing gates**: the prior Phase 21 baseline of 36 plus four Phase 22 gates for durable term/vote persistence, durable state recovery, epoch-bound replay windows, and replay term floors. The dedicated Phase 22 suite passes five integration tests, the existing Phase 12/13 authenticated transport regressions pass, and the deep artifact audit reports zero findings.

## Durable state contract

| Field | Safety purpose |
|---|---|
| `cluster_id` | Prevents loading state from another consensus domain. |
| `node_id` | Prevents applying state to another node identity. |
| `current_term` | Preserves election-term monotonicity across restart. |
| `voted_for` | Preserves same-term vote exclusivity. |
| `replay_epoch` | Identifies the nonce namespace and reset generation. |
| `replay_term_floor` | Rejects authenticated envelopes from terms below the durable replay floor. |
| `state_hash` | Detects tampering or incomplete serialization. |

The store validates canonical state content, enforces a 128 KiB file bound, writes to a create-new staging path, syncs file contents before atomic rename, syncs the containing directory where supported, and removes partial staging during recovery. Node restore rejects cluster/node mismatches, unknown voters, lower terms, lower replay epochs, and lower replay floors before mutating local state.

## Epoch-bound replay contract

`AuthenticatedConsensusEnvelope` now signs `replay_epoch` as part of its payload. Legacy constructors remain available with epoch one for compatibility, while `sign_for_cluster_epoch` creates an explicitly epoch-bound envelope. `ReplayWindow::new_with_epoch` validates the epoch and minimum term, verifies the signed epoch and term floor before nonce mutation, rejects duplicate nonces, and retains bounded insertion-order eviction.

`AuthenticatedSocketTransport::new_with_epoch` initializes every sender window with the durable epoch and term floor. `rotate_replay_epoch` requires a strictly higher epoch, rebuilds all windows before changing the active transport state, and exposes the new epoch and floor to the caller for durable coordination. Socket I/O remains outside the consensus state machine’s authority boundary.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `durable_term_and_vote_state_round_trips_and_recovers_staging` | Atomic save/load and partial staging cleanup | Passed |
| `durable_state_rejects_tampering_identity_and_oversized_payloads` | Hash tampering, identity mismatch, and size bounds | Passed |
| `consensus_node_restores_vote_exclusivity_and_rejects_term_rollback` | Vote persistence, same-term exclusivity, and rollback rejection | Passed |
| `replay_window_binds_epoch_and_term_and_keeps_bounded_nonce_state` | Signature-bound epoch, stale term, duplicate nonce, and eviction | Passed |
| `transport_epoch_rotation_clears_windows_and_is_monotonic` | Epoch rotation, window clearing, and monotonicity | Passed |

## Production boundary

The core does not replicate durable metadata, schedule persistence, own key rotation, open socket threads, or coordinate replay epochs across hosts. Production promotion still requires a durable write-before-ack policy for term/vote transitions, replicated replay-epoch state, coordinated key and epoch rotation, power-loss testing, split-brain testing, and membership-change recovery.

## References

[1]: ../src/consensus.rs "Phase 22 consensus implementation"
[2]: ../tests/phase22_durable_term_replay_integration.rs "Phase 22 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Security compliance metrics artifact"
[4]: ../benchmarks/security_compliance_audit.json "Security metrics audit evidence"
