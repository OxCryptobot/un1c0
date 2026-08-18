# Phase 15 Election Timers and Failure Detectors

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded injected-tick election timers, deterministic jitter, leader heartbeat plans, peer failure suspicion, and clock-safe timer actions
**Author:** Manus AI

## Implementation summary

Phase 15 extends the transport-agnostic consensus core with `ElectionTimerConfig`, `ElectionTimerAction`, and `HeartbeatPlan`. The core does not spawn timer threads, sleep, open sockets, or deliver messages. Callers supply monotonic ticks, invoke `tick(now_tick)`, and transport the returned `StartElection` or `SendHeartbeats` work through the existing authenticated boundary.

Election timeout, deterministic jitter, heartbeat interval, and failure-detector interval are bounded. The heartbeat interval must be shorter than the election timeout, and the failure detector must cover the election timeout. Per-node/per-term jitter is derived from a stable SHA-256 input, keeping test behavior reproducible while reducing synchronized election deadlines across nodes.

## Timer behavior

Followers and candidates initialize a deadline on the first tick and start a new election only when the deadline is reached. Starting the election increments the term and resets the next deadline. Leaders emit an initial heartbeat immediately and subsequently at each heartbeat interval, with a heartbeat plan containing the current term, leader identity, and accepted peer IDs.

Peer heartbeat observations accept only configured members other than the local node. A peer becomes suspect when `now_tick - last_heartbeat >= failure_detector_ticks`; exact-boundary expiry is fail closed. Unknown and self peer IDs are rejected.

## Clock safety

Phase 15 reuses the Phase 14 sticky clock-uncertainty boundary. A backward tick clears lease state, marks clock safety uncertain, blocks timer actions, and blocks failure-detector decisions. Explicit monotonic re-anchoring clears uncertainty, invalidates leases, resets the election deadline, and clears old peer heartbeat observations. The re-anchoring path cannot silently authorize a first-time backward jump while the clock is trusted.

## Validation

The dedicated Phase 15 integration suite passes five tests covering bounded follower election deadlines, leader heartbeat cadence, peer heartbeat deadline reset, exact failure-detector expiry, unknown/self peer rejection, unsafe timer configurations, clock-regression blocking, and explicit re-anchoring. The complete repository compliance validator now includes two additional Phase 15 gates: `election_timer_safety` and `failure_detector_boundaries`.

## Production boundaries

The local slice does not claim distributed timing accuracy. Production still requires real scheduler jitter, authenticated heartbeat transport, durable term/vote persistence, failure and timer metrics, cancellation, backpressure, suspend/resume handling, and cross-machine fault injection. The core intentionally returns typed work rather than assuming transport or background execution authority.
