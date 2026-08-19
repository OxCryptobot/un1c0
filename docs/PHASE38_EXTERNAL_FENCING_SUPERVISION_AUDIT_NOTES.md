# Phase 38 Audit Notes: External Fencing Authority and Production Supervision

## Scope and observed baseline

The Phase 37 benchmark demonstrates bounded local safety and relative execution behavior, not production failover capacity. The direct release benchmark completed 4,096 transport and 4,096 reservation fuzz cases with zero panics, used approximately 15.7 MiB peak RSS, and completed in approximately 2.09 seconds. The published artifact records 129/129 compliance gates, but it does not yet record disk I/O latency, network bytes, thread count, allocator pressure, or per-consumer fence-application latency. These omissions are material for production supervision.

The repository already implements the cryptographic and monotonic core of external fencing. `ExternalFencingToken` binds the domain, cluster, resource, owner region, owner term, ownership epoch, membership epoch, fencing epoch, authority ID, and recovery-log index before Ed25519 verification (`src/replicated_recovery.rs`). `ExternalFenceState` rejects authority-key rebinding, generation rollback, older fencing epochs, and same-epoch conflicting tokens; exact token replay is idempotent. `TrustedFencingAuthorityRegistry` rejects unknown authorities and implicit key rebinding. Phase 37's `FailoverOrchestrator` separately requires fresh telemetry and an exact operation/decision digest before external-fence admission.

> **Critical boundary:** a signed fencing token is evidence, not enforcement. Production safety exists only when every write-capable consumer applies the token before accepting work.

## Required production topology

| Component | Production responsibility | Must not be delegated to |
|---|---|---|
| Replicated fencing authority | Durable quorum, membership epoch, monotonic owner/fence generations, proposal identity, signed token issuance, and restart continuity | The candidate region or a single application process |
| Key custody service | Protect authority signing keys, approve rotation/revocation, audit signer use, and prevent private-key export | Agent nodes, telemetry collectors, or public configuration |
| External write gateway | Validate exact cluster/resource/token binding and reject writes from stale owners before mutation | Application-level best effort or post-write audit |
| Worker/scheduler controller | Stop or quarantine queued work under a stale fence and require a new exact token before dispatch | Queue consumers that cannot prove current ownership |
| Socket/queue ownership layer | Apply the fence to delivery ownership, acknowledgements, leases, and retries | Transport-only authentication without ownership state |
| Routing layer | Remove or isolate stale-region endpoints before serving writes | DNS TTL expiry alone or a passive health check |
| Process-fence agent | Terminate, isolate, or revoke stale owner processes and confirm completion | The fenced process's own self-report |
| Independent supervisor | Compare authority heartbeat, consumer acknowledgements, clock health, and resource budgets; block promotion on uncertainty | The same process that is being promoted |

The authority and independent supervisor must not share a failure domain with the candidate owner. If the candidate can suppress, delay, or forge its own supervision evidence, the evidence is not an authority input. A production deployment should therefore place authority replicas, key custody, consumer adapters, and the supervisor across failure domains with separate credentials and explicit network policy.

## Authority setup and lifecycle

### 1. Bootstrap a pinned authority identity

Create an authority ID and public key through an approved key-custody workflow. Register the public key in a versioned `TrustedFencingAuthorityRegistry` before admitting any token. The registry must be distributed through an authenticated configuration or membership transition, not an ad hoc environment-variable replacement. A key change requires an explicit rotation protocol with overlapping verification, a higher membership epoch, and an auditable revocation point. Never persist or log private signing material.

### 2. Establish a durable quorum

Run at least the configured observer majority across independent failure domains. Persist the authority membership, public keys, recovery log, acknowledgements, applied/commit frontiers, controller snapshot, active token, and event trace through the existing atomic snapshot pattern. The authority must restore only after validating state hashes, entry hashes, token bindings, membership epoch, and identity. A replica that cannot prove continuity must remain non-authoritative.

### 3. Issue monotonic tokens

The authority must issue a token only from a committed recovery decision. `fence_epoch` advances exactly once per committed fence, while owner term, ownership epoch, membership epoch, and log index remain bound to the decision. A token for the same exact decision may be replayed idempotently; a lower generation, same-generation conflict, mismatched authority, or mismatched resource must be rejected before state mutation.

### 4. Deliver tokens to consumers

Every write-capable consumer receives the token through an authenticated channel and stores only the minimum required evidence: token hash, authority ID, cluster/resource, owner region, generations, and acceptance time. The consumer verifies the signature and exact bindings locally, then atomically updates its enforcement state before acknowledging application. A consumer acknowledgement must identify the consumer, exact token hash, accepted fence epoch, applied action, and monotonic observation tick. A missing acknowledgement is not equivalent to success.

## Supervision contract

The next implementation batch should add two signed evidence families:

| Evidence | Required fields | Gate |
|---|---|---|
| `FencingAuthorityHeartbeat` | Domain/version, cluster/resource, authority ID, membership epoch, latest fence epoch, committed log index/hash, observed tick, TTL, signer key, signature, digest | Authority is trusted, fresh, and monotonic |
| `FenceConsumerAcknowledgement` | Domain/version, cluster/resource, authority ID, consumer ID/type, token hash, fence epoch, owner region, applied action, observed tick, TTL, consumer key, signature, digest | Required consumers have applied the exact current token |

The supervisor should expose typed decisions such as `Ready`, `AuthorityStale`, `ConsumerCoverageInsufficient`, `ClockUncertain`, `GenerationRegression`, and `Quarantined`. It must evaluate all required consumers before allowing a promotion. The decision is advisory to the deployment controller but fail-closed: no `Ready` result is emitted when authority evidence, consumer coverage, clock health, or resource budgets are stale or contradictory.

The supervisor must be independent from the authority token issuer and the candidate process. It should use a monotonic local clock, enforce bounded journal/cardinality limits, persist its last accepted authority and consumer frontiers atomically, and expose sanitized metrics only. Resource telemetry must include supervisor loop duration, signature-verification latency, journal bytes, persistence/fsync latency, pending consumer count, stale-evidence age, and memory/CPU budget status.

## Supervision timing rules

Choose heartbeat and TTL values from observed p99 latency plus bounded clock uncertainty, not from nominal averages. The TTL must exceed the worst-case authenticated delivery, persistence, and supervisor-evaluation budget by a documented margin. A supervisor must treat clock regression, missing clock-health evidence, authority epoch regression, fence-epoch regression, and consumer acknowledgement regression as unsafe. It must not extend a token's validity locally when the authority heartbeat is stale.

Use separate budgets for the authority, supervisor, and consumers. A local Phase 37 RSS result is a reference for test sizing only; it is not a production limit. Production budgets must be validated under key rotation, authority restart, partition, fsync delay, consumer backlog, and simultaneous failover attempts. When a budget is exceeded, the supervisor should enter `Quarantined` and preserve the last safe state rather than silently dropping evidence.

## Consumer enforcement protocol

A production write admission should follow this order:

1. Resolve the resource's current token hash and trusted authority key.
2. Verify token signature, cluster/resource, authority ID, owner term, ownership epoch, membership epoch, fence epoch, and log index.
3. Compare the token against the consumer's accepted generations and reject rollback or same-epoch conflict.
4. Confirm the local region/process/lease is the token owner and that the token has not expired according to trusted clock health.
5. Atomically apply the consumer enforcement state before admitting a write, queue delivery, socket acknowledgement, route, or process lease.
6. Emit a signed acknowledgement containing the exact token hash and applied action.
7. On restart, restore the enforcement state and refuse writes until state continuity and current authority evidence verify.

The protocol must be implemented at every write path. A single protected gateway does not fence background schedulers, direct database writers, socket owners, DNS/routing, or already-running processes unless those consumers independently enforce the same token.

## Failure and recovery runbook

| Condition | Required supervisor action | Recovery requirement |
|---|---|---|
| Authority heartbeat stale | Block promotion and new write authority; retain last safe token | Restore authenticated heartbeat at the same or higher membership/fence epoch |
| Authority key mismatch/rebinding | Quarantine authority and reject tokens | Execute explicit registry rotation and revocation procedure |
| Consumer acknowledgement missing | Keep consumer out of `Ready`; isolate its writes or route | Reconcile exact token hash and obtain a fresh signed acknowledgement |
| Consumer reports conflicting token | Quarantine the consumer and block promotion | Resolve authority/consumer state from durable log and restore monotonic generations |
| Clock regression/uncertainty | Fail closed for expiry and promotion decisions | Re-anchor to a trusted monotonic clock-health source |
| Authority quorum loss | Do not issue or apply a new promotion token | Restore quorum or perform an approved manual recovery procedure |
| Candidate process ignores fence | Invoke independent process-fence mechanism; do not rely on self-termination | Confirm process isolation/termination from an external observer |
| Supervisor resource exhaustion | Enter `Quarantined`, preserve last safe state, emit bounded alert | Restore capacity and replay durable evidence before re-admission |
| Restart with incomplete state | Refuse writes and promotion | Clean staging, validate snapshot/hash/identity, then rejoin at a safe epoch |

## Deployment gates before production

A promotion should remain blocked until the deployment proves, in a non-production rehearsal, that: authority restart preserves token generations; membership/key rotation is explicit and monotonic; every write-capable consumer rejects stale and misbound tokens; consumer acknowledgements are signed and exact; process fencing works when the target is uncooperative; routing and queue delivery converge; clock uncertainty blocks promotion; supervisor and authority survive partitions; and audit evidence contains only bounded non-secret digests.

The local repository can provide deterministic contracts, signatures, replay protection, atomic snapshots, and fail-closed decisions. It cannot prove cloud-region failure truth, managed storage durability, DNS convergence, mTLS lifecycle, process termination, hardware fencing, or service admission outside the adapters. Those remain explicit deployment boundaries and must be validated with environment-specific integration tests and approval-controlled rollout gates.
