# Phase 32 Quorum Safety Audit Notes

## Scope

This note records the first-pass audit of `DisasterRecoveryController`, the deterministic `MultiRegionFailoverSimulator`, and the current Phase 32/30 integration suites at repository commit `8bf0083`. It separates executable observations from inferred risks and proposed test cases. The local controller intentionally does not claim authority over cloud failure detectors, cross-region transport, routing convergence, managed storage, process fencing, or production key custody.

## Executable baseline

The Phase 30 partition suite passes 9 tests and the Phase 32 controller suite passes 9 tests. The Phase 32 tests establish signed observations, distinct observer admission for a three-member quorum, snapshot binding, higher term/epoch checks, old-region fencing on commit, single-active-region reporting, idempotent observation and commit replay, tamper rejection, stale generation rejection, and self-observation rejection.

The Phase 30 simulator tests establish deterministic trace replay, owner fencing under majority partition, healing without implicit unfencing, restart from a fenced snapshot, stale transfer rejection, distinct reachable observer reports, invalid quorum-loss evidence rejection, clock-skew transfer blocking, and delayed-link progression.

## Observed quorum model

`DisasterRecoveryConfig::required_observers` returns `quorum_size - 1`, because the active region is implicitly one member of the configured cluster quorum. For a three-member quorum, two distinct non-active observers are therefore required before promotion. `ingest_failure_observation` authenticates and binds each observation before inserting it into the observer map. Identical evidence is replay-idempotent; conflicting evidence from the same observer is rejected. `prepare_promotion` then requires the observer threshold, a known healthy unfenced inactive candidate, a strictly higher owner term and ownership epoch, and exact equality among the active snapshot, candidate snapshot, and proposal snapshot. `commit_promotion` only accepts the exact pending proposal, fences and deactivates the previous region, activates the candidate, advances both generations, and clears pending authority.

The simulator independently models directed link drop/delay/duplicate/reorder/corrupt faults, observer reachability, quorum-loss fencing, delayed acknowledgements, snapshot restore, explicit healing, and higher-generation ownership transfer. Its safety report prevents a committed queue from retaining a fence or from lacking an acknowledgement quorum.

## Safety guarantees currently evidenced

| Invariant | Evidence | Boundary |
|---|---|---|
| No promotion before distinct observer quorum | Phase 32 wait test and observer map keyed by observer ID | Observer membership and failure-detector truth remain external |
| Failure evidence is authenticated and state-bound | Ed25519 signature, trusted-key, cluster, observer, active term/epoch, and snapshot checks | Key registry and transport are local/in-process |
| Candidate state is snapshot-consistent | Exact digest equality against active and candidate state | Cross-region snapshot durability is not modeled |
| Generation monotonicity | Both owner term and ownership epoch must strictly increase | No external consensus vote is modeled |
| Old owner is fenced on commit | Previous region becomes inactive, unhealthy, fenced before completion | Runtime/process fencing is not performed |
| At most one active region after successful commit | Region flags and report invariant | No concurrent multi-process controller is modeled |
| Replays do not create duplicate authority | Identical observations and committed proposal are idempotent | Durable event/state recovery is not implemented in this controller |
| Partition healing does not implicitly restore authority | Phase 30 simulator requires explicit higher-generation transfer | Controller does not ingest live network events |

## Potential failure modes not yet tested in Phase 32

| Priority | Failure mode | Why it matters | Proposed test |
|---|---|---|---|
| High | Unknown observer ID | The implementation rejects it, but no Phase 32 test proves no observation mutation and no phase transition | Ingest a correctly signed observation from an unregistered observer; assert `BindingRejected`, observer count 0, active region unchanged, and safety passed |
| High | Wrong cluster binding | Canonical signature verification must reject a valid signature from another cluster before map mutation | Sign with the trusted key but use another cluster ID; assert rejection and unchanged report/trace |
| High | Wrong observer identity or trusted-key rebinding | Prevent a registered observer ID from presenting another observer's key or identity | Sign as observer-c but label observer-b, and register a mismatched key; assert rejection before mutation |
| High | Commit without a prepared proposal | Prevent caller-supplied failover from bypassing quorum and candidate checks | Construct a valid-looking `FailoverProposal` without `prepare_promotion`; assert `StaleProposal` and old region remains active/unfenced state unchanged |
| High | Conflicting or stale proposal after preparation | A second candidate or altered term/epoch/snapshot must not overwrite pending authority | Prepare region-b, then submit altered candidate/term/epoch/snapshot to commit; assert rejection and no fencing/activation mutation |
| High | Promotion after committed state with a different candidate | Idempotence must not become an alternate takeover path | Commit region-b, then prepare or commit region-c; assert rejection and exactly one active region |
| Medium | Candidate region is unhealthy or fenced | `prepare_promotion` has explicit guards but no integration evidence | Mark a registered candidate unhealthy is currently not exposed; add a test fixture/API or document this as an API gap |
| Medium | Empty/duplicate observer evidence under quorum | Replaying an observation after quorum must not inflate count or bypass distinctness | Ingest same observation repeatedly around the quorum boundary and assert count remains bounded and promotion remains deterministic |
| Medium | Observations arriving before `record_region_failure` | The controller currently accepts state-bound observations without checking `failure_tick` is set | Ingest valid observation before local failure detection; decide whether to reject as sequencing violation or explicitly document remote observation as sufficient |
| Medium | Observation tick ordering and stale observation replay | `observed_tick` is bound but not compared with `failure_tick` or prior observer ticks | Test older-than-failure, far-future-within-bound, and same observer later-tick evidence; define acceptance policy and assert it |
| Medium | Partial registration / missing active region | `register_region` permits a controller state with no registered active region until later calls | Attempt promotion or report before active region registration; assert fail-closed behavior or tighten constructor/registration invariant |
| Medium | Term/epoch overflow at maximum values | Strict `+1` is caller-supplied and no overflow test exists | Exercise `u64::MAX` active generation and candidate generation; reject impossible promotion without wraparound |
| Medium | Event-bound exhaustion and trace behavior | `record_event` uses saturating sequence and silently stops appending after the event cap | Drive the event limit or expose a bounded test hook; assert safety/report does not claim complete trace evidence after truncation |
| Medium | Restart/durable recovery of controller state | Phase 30 simulator tests restart, but Phase 32 controller has no snapshot/restore API | Add serialized controller snapshot or explicitly retain this as a deployment boundary with a separate acceptance test at the orchestration layer |
| Medium | Concurrent/competing promotion attempts | A single mutable controller is tested serially; no exact pending-proposal conflict test exists | Prepare one proposal, attempt another candidate, then commit the first; assert no pending replacement and one active region |
| Low | Unicode byte-vs-character bounds | `len()` limits bytes while `chars().any()` checks controls; boundary semantics are not tested | Add identifier/reason tests at byte limit, multibyte limit, control character, and digest casing boundaries |
| Low | Error-path trace immutability | Rejected observations/proposals should not add authority events or mutate trace unexpectedly | Capture report/trace before and after each rejection and assert only explicitly documented events change |

## Initial assessment

The strongest safety property is the ordering of authenticated and state-bound evidence before observation-map mutation, followed by quorum admission and only then proposal validation and fencing. The most important remaining controller-level gap is **sequencing authority**: a valid observation can be ingested even when `record_region_failure` has not been called, because `failure_tick` is stored but never consulted by `ingest_failure_observation`. Whether this is a defect depends on whether remote signed observations are intended to establish failure detection independently; the current reference describes observations as evidence after failure detection, so a test or contract decision is warranted.

The second important gap is **unprepared commit rejection**. The implementation correctly checks `pending_proposal`, but the current suite never proves that a caller cannot directly commit a valid-looking proposal without quorum preparation. The third is **competing/stale proposal behavior after preparation**, which is central to split-brain prevention but currently only indirectly covered by stale term/epoch checks.

## References

[1]: ../src/disaster_recovery.rs "Phase 32 disaster-recovery controller"
[2]: ../tests/phase32_disaster_recovery_integration.rs "Current Phase 32 integration coverage"
[3]: ../src/multiregion.rs "Deterministic multi-region partition simulator"
[4]: ../tests/phase30_multiregion_failover_integration.rs "Phase 30 partition/failover integration coverage"
[5]: ../../skills/agentic-system-engineering/references/phase32-multiregion-disaster-recovery.md "Phase 32 normative safety contract"


## Follow-up implementation

The high-priority controller gaps were addressed in the working batch. `ingest_failure_observation` now requires local failure detection before accepting remote evidence, so a valid signature cannot create observer quorum in the `Stable` phase. `prepare_promotion` preserves an existing pending proposal and rejects a conflicting candidate instead of replacing pending authority. The controller stores the exact committed proposal identity, so idempotent replay is limited to the original proposal rather than any request sharing only the active region and generation. A prepared or committed recovery cycle is terminal for `record_region_failure`, preventing stale evidence from resetting the state machine.

The Phase 32 suite now contains 16 passing tests. The new tests prove unknown-observer and wrong-cluster rejection before mutation, local failure-detection sequencing, unprepared commit rejection, conflicting candidate rejection, terminal-cycle protection, and exact committed-proposal replay identity. The original 9-test baseline remains green, and the Phase 30 9-test partition suite remains the adjacent simulator regression set.

## Remaining gaps

The controller still does not model durable controller snapshots or restart recovery, concurrent multi-process contenders, failure-detector membership changes, observer liveness leases, candidate health transitions after registration, cross-region snapshot transfer, or actual network delivery ordering. The Phase 30 simulator covers restart and partition-healing behavior for its separate queue-ownership state machine, but it does not serialize and restore the Phase 32 controller's observer map, pending proposal, committed proposal, or recovery phase. These should remain explicit production-promotion gates rather than being inferred from the local mutable controller.

The implementation also retains a deliberately conservative one-cycle lifecycle: after `Committed`, a new recovery requires a new controller instance or a future explicit reset protocol. That avoids implicit authority reuse but should be made a typed operational contract before embedding the controller in a long-lived service.
