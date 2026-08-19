# Phase 32 Quorum Safety and Partition Edge-Case Audit

## Executive conclusion

Phase 32 provides a strong **local fail-closed state-machine contract** for region promotion. A candidate cannot become active until signed failure evidence has been authenticated, bound to the configured cluster and active generation, supplied by distinct trusted observers, matched to the active snapshot, and followed by a strictly higher owner term and ownership epoch. Commit then fences and deactivates the previous region before exposing the candidate as active. The controller reports a single-active-region invariant and records an ordered trace digest. These guarantees are implemented in [`src/disaster_recovery.rs`][1] and exercised by the expanded 16-test Phase 32 suite [2].

The most important limitation is architectural rather than cryptographic: the controller is a single mutable in-process authority. It does not itself establish failure-detector truth, coordinate concurrent controllers, durably recover its observer and proposal state, fence processes or routing, replicate snapshots, or deliver messages across a real partition. The adjacent deterministic simulator covers many of those *scenario shapes* for queue ownership, including partition healing, restart from a fenced snapshot, delayed links, and stale transfers, but it is not a serialized execution of the Phase 32 controller [3] [4].

## Quorum model and safety ordering

For a configured cluster quorum of `Q`, the controller requires `Q - 1` distinct non-active observers because the active region contributes the implicit local member. In the standard three-member configuration, two distinct observers are therefore required. The observer map is keyed by observer ID, so duplicate submissions cannot inflate quorum. The safety-critical order is authentication and binding first, observer-state mutation second, quorum admission third, proposal validation fourth, and fencing/activation last [1] [5].

| Stage | Required condition | State mutation permitted before success? | Failure behavior |
|---|---|---:|---|
| Observation shape | Bounded identifiers, digest, term, epoch, tick, reason, public-key, and signature lengths | No | `InvalidInput` or `SignatureRejected` |
| Trusted identity | Observer ID is registered and public key exactly matches the trusted registry | No | `BindingRejected` |
| Cryptographic binding | Signature covers canonical cluster, region, observer, term, epoch, tick, snapshot, reason, and public key | No | `SignatureRejected` or `BindingRejected` |
| Active-state binding | Observation matches active region, owner term, ownership epoch, and snapshot digest | No | `BindingRejected` |
| Failure sequencing | Local failure detection was recorded for the active region | No | `BindingRejected` |
| Distinct quorum | Observer count reaches `Q - 1` | Observation map only | `AwaitingQuorum`; active region remains unchanged |
| Candidate admission | Candidate is known, healthy, inactive, unfenced, and snapshot-equal | No | `StaleProposal` or `SnapshotHashMismatch` |
| Generation admission | Candidate term and epoch are both strictly greater than active values | No | `StaleProposal` |
| Proposal preparation | No conflicting proposal is already pending | Pending proposal | Conflicting preparation fails closed |
| Commit | Exact pending proposal matches the request | Fencing and activation | Unprepared or altered proposal rejected |
| Replay | Exact committed proposal matches the original proposal identity | No | Only exact replay returns `AlreadyCommitted` |

This ordering prevents a valid signature from becoming authority by itself. The signature proves authenticity and payload integrity; it does not prove that the observer is reachable, that the region is actually unavailable, or that the observer set is an independent quorum. Those facts remain external failure-detector and membership responsibilities.

## Partition scenario analysis

The deterministic Phase 30 simulator provides the closest executable partition model. Under a majority partition that isolates the current owner, delivery fails and a fence is retained without commit. Under asymmetric loss, healing does not remove the fence; a higher-term and higher-epoch transfer is still required. Delayed links require explicit tick advancement before a quorum acknowledgement can commit. Restart restores a fenced snapshot before transfer, and stale transfers after healing are rejected [4]. These are the correct safety shapes for Phase 32 orchestration, but the Phase 32 controller still needs an integration layer that feeds equivalent authenticated observer evidence and durable generation state.

| Partition scenario | Safety expectation | Current evidence | Residual concern |
|---|---|---|---|
| Active region isolated from both remote regions | No promotion from one local view; active authority is fenced or remains awaiting evidence | Phase 30 majority-partition fencing; Phase 32 quorum wait | Controller does not receive real link reachability or fence authority |
| One observer reachable, one observer partitioned | No promotion; active region and generation unchanged | Phase 32 `AwaitingQuorum` test | No live transport to prove observer reachability and independence |
| Asymmetric link loss | Healing alone must not revive old authority | Phase 30 asymmetric-healing test | Phase 32 controller has no healing event or old-owner message path |
| Delayed observer report | No early promotion; later valid evidence may progress | Phase 30 delayed-link test | Phase 32 observation tick is bound but not modeled as a delivery schedule |
| Duplicate observer report | Count remains distinct and bounded | Phase 32 idempotence test | No cross-process deduplication or durable replay window |
| Conflicting same-observer report | No mutation or quorum inflation | Phase 32 conflict test | Error is local; remote conflict resolution remains external |
| Competing candidate promotions | One pending proposal cannot be replaced by another | New Phase 32 conflict-preparation test | No concurrent controllers or atomic distributed compare-and-swap |
| Old owner sends traffic after promotion | Old authority must remain fenced | Phase 30 stale-owner simulator test; Phase 32 old-region flag | No process, socket, DNS, or routing fence is executed |
| Controller restarts after evidence or preparation | State must recover without losing quorum or accepting stale promotion | Not implemented for Phase 32 controller | Durable controller snapshot/recovery is a high-priority gap |
| Observer membership changes during failure | Old observer quorum must not silently authorize a new configuration | Not implemented | Requires joint membership or external observer registry epoch |

The decisive split-brain invariant is therefore **generation plus fencing**, not quorum count alone. A quorum of observers can authorize a proposal only when its observation and snapshot are bound to the active generation, and the candidate must advance both term and ownership epoch. The old region must then be fenced by an authority that is stronger than a local boolean. Without that external fence, the local controller can report safe state but cannot prevent a stale process from continuing to serve traffic.

## Coverage review

The original Phase 32 suite had nine tests. It covered the happy path and the eight compliance-gate themes, but it did not prove several important negative paths. The follow-up batch added seven tests and corresponding implementation guards, bringing the suite to 16 passing tests while preserving the original behavior [2].

| Coverage area | Before | After | Assessment |
|---|---:|---:|---|
| Full observer quorum promotion | Yes | Yes | Strong local evidence |
| Quorum wait without active mutation | Yes | Yes | Strong local evidence |
| Signature tampering | Yes | Yes | Strong cryptographic rejection evidence |
| Snapshot and generation binding | Yes | Yes | Strong local binding evidence |
| Duplicate and conflicting observer evidence | Yes | Yes | Strong single-observer evidence |
| Unknown observer | No | Yes | Added no-mutation rejection test |
| Wrong cluster | No | Yes | Added canonical-binding rejection test |
| Observation before local failure detection | No | Yes | Added sequencing guard and test |
| Commit without preparation | No | Yes | Added quorum/fencing bypass test |
| Conflicting candidate after preparation | No | Yes | Added pending-authority preservation test |
| New failure after prepared/committed cycle | No | Yes | Added terminal-cycle guard and test |
| Altered replay after commit | Partial | Yes | Exact original proposal identity is now required |
| Controller restart/recovery | No | No | Remains a high-priority gap |
| Concurrent controller/process race | No | No | Remains a production-boundary gap |
| Real partition transport and fencing | No | No | Remains a deployment boundary |

The new sequencing guard is intentionally conservative. A remote signed observation is not permitted to create a recovery workflow before the local controller has entered `DetectingFailure`. This prevents evidence ingestion from bypassing the explicit state-machine transition. If future orchestration requires remote evidence to initiate detection independently, that must be represented as a separate typed action with its own quorum, freshness, and authority contract rather than weakening `ingest_failure_observation`.

The exact committed-proposal identity is also material. Previously, an `AlreadyCommitted` response could be inferred from the active candidate region and current term/epoch without proving that the replayed request matched the original proposal's previous region and snapshot identity. The controller now stores the committed proposal and permits idempotent replay only when the complete proposal matches. A committed or prepared cycle is terminal for `record_region_failure`, which prevents stale observations from resetting the recovery phase or leaving old evidence attached to a new cycle.

## Findings and priorities

| Severity | Finding | Likelihood | Impact | Recommended next phase |
|---|---|---:|---:|---|
| High | Phase 32 controller state is not durably snapshotted or recovered | Medium | Restart can lose observer quorum, pending proposal, committed identity, and terminal phase | Add hash-bound controller snapshots with atomic recovery and crash-injection tests |
| High | Process/routing fencing is represented only as local region flags | High in real partitions | A stale region could continue serving despite a local safe report | Integrate external fencing tokens, service admission, and routing epoch checks |
| High | Failure-detector and observer membership authority are outside the controller | Medium | Correlated or compromised observers could form a false quorum | Add observer registry epochs, independent-domain policy, and authenticated quorum authority |
| High | No concurrent-controller race model | Medium | Two controllers could prepare different candidates from the same generation | Add an external compare-and-swap/lease authority or a replicated recovery log |
| Medium | Observation freshness is only bounded, not policy-checked against a recovery generation or expiry window | Medium | Delayed evidence could be replayed within the global tick bound | Add signed failure-generation IDs, freshness windows, and observer evidence expiry |
| Medium | Candidate health and snapshot state are registered once and do not evolve | Medium | Promotion may rely on stale health or snapshot metadata | Bind promotion to a current signed candidate readiness report |
| Medium | The controller uses a one-cycle terminal lifecycle | Low | Long-lived services need explicit reinitialization semantics | Add a typed `Reset`/`NewGeneration` protocol with durable audit and approval |
| Low | Event-cap exhaustion is not exercised | Low | Trace digest may omit later events if the bounded event list saturates | Add a bounded event overflow action and test the report contract |

## Recommended next batch

The next best-value engineering batch is **durable, externally fenced recovery authority**. It should add a serializable controller snapshot containing the cluster identity, active region, active generation, snapshot digest, recovery phase, failure tick, observer evidence digests, pending proposal, committed proposal, and event frontier. Recovery must validate the snapshot hash, remove partial staging, reject rollback generations, and restore the terminal state before accepting new evidence. The test suite should inject crashes before staging, after staging, before rename, after rename, and after event append.

The following batch should connect the controller to an explicit observer-membership epoch and a compare-and-swap recovery lease. Promotion should require an authenticated quorum certificate that binds observer identities, independent failure domains, failure-generation ID, active generation, candidate snapshot, candidate generation, and fencing token. A stale controller must be rejected before it can prepare or commit. This is the point at which the local controller becomes a safe component of a distributed recovery protocol rather than a standalone in-process decision engine.

## Validation status

At the start of this batch, the Phase 30 partition suite passed 9 tests and the Phase 32 suite passed 9 tests at commit `8bf0083`. After the hardening changes, the Phase 32 suite passes 16 tests. The final publication batch must still run the complete all-target Rust suite, focused Phase 30 regressions, Python and shell syntax checks, the skill validator, the complete security compliance validator, and `git diff --check` before commit and push.

## References

[1]: ../src/disaster_recovery.rs "Phase 32 disaster-recovery controller and safety transitions"
[2]: ../tests/phase32_disaster_recovery_integration.rs "Phase 32 integration tests, including the follow-up safety batch"
[3]: ../src/multiregion.rs "Deterministic multi-region partition and failover simulator"
[4]: ../tests/phase30_multiregion_failover_integration.rs "Phase 30 partition, healing, restart, delay, and stale-transfer tests"
[5]: ../../skills/agentic-system-engineering/references/phase32-multiregion-disaster-recovery.md "Normative Phase 32 safety contract"
[6]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture and production boundaries"
