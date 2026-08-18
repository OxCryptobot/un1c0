# Phase 14: Leader-Lease Read Index and Linearizable Client Reads

## Design objective

Reduce the cost of linearizable reads without weakening the consensus commit boundary. A client read may return a value only after the node proves that its applied state is at least the read index. A leader may use a bounded lease fast path only while its term, quorum-observed lease, and clock-drift safety window remain valid. Otherwise it must fall back to a quorum-backed read-index round.

## Clock model and lease bound

The implementation uses an injected monotonic logical tick, not wall-clock time. A `LeaderLeaseConfig` defines a positive lease duration and a non-negative maximum clock-drift bound. The usable lease horizon is `lease_duration - max_clock_drift`; configurations where drift consumes the entire lease are rejected. The leader records the lease expiration tick only after a current-term quorum read acknowledgement. A lease is valid at tick `t` only when `t + max_clock_drift < expiration_tick`; equality is treated as expired. This conservative inequality prevents a fast local clock from serving a read after a peer could have observed a newer leader.

The lease is invalidated on term changes, step-down, membership changes, and any local tick that crosses the safe horizon. The core does not spawn timers or read the system clock. Callers supply monotonic ticks from a trusted clock source and must advance them conservatively across suspend/resume or clock-source discontinuity.

## Read-index protocol

A leader creates a `ReadIndexRequest` containing a request ID, current term, leader ID, and the leader's committed index. Followers accept it only from a configured member and only when the request term is current or newer. They acknowledge the request only if their local commit index is at least the requested read index. The leader counts distinct current-term acknowledgements using the active membership quorum rule, including the leader's own acknowledgement. A response from a higher term forces the leader to step down. A stale, future, duplicate, or under-committed response cannot complete the round.

The completed read index is the quorum-observed commit index. The leader may serve the query only after applying through that index. The protocol remains transport-agnostic: callers deliver typed messages through the existing authenticated transport and must not confuse an uncommitted local append with a committed read boundary.

## Linearizable client query contract

`LinearizableReadRequest` binds a bounded request ID, key, and monotonic tick. `LinearizableReadPlan` records the request ID, key, read index, term, and whether the lease fast path was used. The leader prepares a plan through the lease fast path when safe, or returns a quorum read-index request when a round is required. After quorum completion, the caller executes the plan against the state machine. The plan is rejected if the node is no longer the same leader term, if its applied index is below the read index, or if the lease has expired.

Followers and non-leaders do not serve linearizable client reads from local state. Missing keys return `None` only after the consistency proof succeeds. Request IDs are bounded and duplicate completion is rejected by the caller-owned round state; the consensus core never treats a read request as a state mutation.

## Validation and benchmark gates

Tests cover lease acceptance and expiry, maximum drift rejection, strict-boundary expiry, term-change invalidation, step-down invalidation, membership-change invalidation, stale/future/duplicate/under-committed acknowledgements, quorum completion, follower rejection, applied-index gating, and read-after-write behavior. Benchmarks use deterministic in-process fixtures and injected ticks, compare lease-fast-path and quorum read-index paths, and report p50/p95/p99 latency, throughput, errors, and concurrency from 1 through 32 workers. Results are local evidence only and are not WAN scalability claims.

## Production boundaries

The phase does not provide a physical clock-synchronization service, election timers, failure detection, disk-backed read-round recovery, or an external client session layer. Production callers must use a monotonic clock with suspend/resume handling, transport authentication, bounded request retention, cancellation, metrics, and a policy that falls back to quorum reads whenever clock health is uncertain.
