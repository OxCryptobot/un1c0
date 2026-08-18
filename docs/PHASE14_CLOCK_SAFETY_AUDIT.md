# Phase 14 Clock Safety Audit and Phase 15 Handoff

## Scope

This audit reviews `ConsensusNode` leader-lease safety, sticky monotonic-clock uncertainty, and explicit re-anchoring in `src/consensus.rs`. It also records the next hardening slice: bounded in-process election timers and failure-detector state.

## Observed Phase 14 design

`LeaderLeaseConfig` bounds `lease_ticks` and rejects any configuration where `max_clock_drift_ticks >= lease_ticks`. A lease is installed only after a current-term read-index quorum and is usable only when `now_tick + max_clock_drift_ticks < expiration_tick`. The strict inequality makes the drift-adjusted boundary fail closed.

`observe_tick` stores the last supplied tick. If a caller supplies a lower tick, the node immediately clears the lease and sets `clock_uncertain = true`; the lower tick becomes the new observed anchor, but uncertainty remains sticky. `lease_is_valid` checks both the role and the sticky uncertainty bit, so a lower tick cannot silently extend a lease. `install_lease` refuses to install a new lease while uncertainty is set.

`reanchor_monotonic_clock` is the explicit recovery boundary. It invalidates any existing lease, updates the observed tick, clears uncertainty, and allows a lower tick only when uncertainty was already recorded. A backward jump presented for the first time through re-anchoring is rejected with `ClockUntrusted`, preventing callers from using the method as an implicit clock rollback bypass.

All lease-invalidating state transitions reviewed in this slice include elections, higher-term responses, step-down, append handling, snapshot installation, and membership transitions. Read plans separately validate leader role, exact term, lease validity for the fast path, commit frontier, applied frontier, and duplicate request retention.

## Findings

| Finding | Severity | Assessment |
|---|---|---|
| Drift-adjusted lease boundary is strict | High-value safety control | Correct; equality is expired and overflow cannot make a lease valid. |
| Backward tick clears lease immediately | High-value safety control | Correct; the uncertainty bit prevents automatic lease reuse. |
| Re-anchoring is explicit and lease-invalidating | High-value safety control | Correct; callers must establish a trusted monotonic origin. |
| No wall-clock or timer thread in consensus core | Design boundary | Correct; callers own clock source and lifecycle. |
| Lease state is not persisted across restart | Production boundary | Safe by default because restart begins without a lease; production may add an epoch-bound clock-health record. |
| Clock uncertainty is a single boolean | Maintainability gap | Acceptable for the bounded slice; a future production layer should carry an epoch/reason and telemetry. |
| Election timers are not yet implemented | Next-phase gap | Phase 15 should use the same monotonic tick and fail closed on clock uncertainty. |

## Phase 15 implementation direction

Add a bounded, transport-agnostic election timer and failure detector. The node should expose an injected-tick `tick(now_tick)` method rather than spawn a background thread. Followers and candidates start an election only after a deterministic, per-node timeout window; leaders emit heartbeat work at a bounded interval. Peer heartbeat acknowledgements should be recorded with bounded membership checks, and a peer becomes suspect only after the configured failure interval. A clock regression or uncertain clock must block timer-driven elections and require explicit re-anchoring first.

The timer slice must remain separate from transport and should return typed actions for the caller to deliver. Tests should cover heartbeat cadence, follower timeout, deterministic timeout separation by node identity, higher-term reset, clock regression blocking, explicit re-anchoring, peer suspicion expiry, unknown-member rejection, and bounded membership configuration.
