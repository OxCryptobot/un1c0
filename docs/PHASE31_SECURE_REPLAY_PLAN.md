# Phase 31 Secure Deterministic Replay Plan

## Objective

Make simulated network-partition and failover traces safe to replay as untrusted inputs. The replay engine must authenticate the schedule, bind it to the intended scenario and consensus generation, apply faults transactionally, and verify the final event trace before committing simulator state.

## Implementation slices

| Slice | Required behavior | Evidence |
|---|---|---|
| Typed manifest | Bound scenario, cluster, signer, replay epoch, owner term, seed, nonce, event count, tick count, schedule, public key, and signature. | Manifest constructor and shape-validation tests. |
| Canonical schedule | Serialize the typed schedule deterministically and bind its SHA-256 digest into the signed payload. | Schedule mutation produces `ScheduleHashMismatch`. |
| Trusted identity | Require exact trusted public key, cluster ID, signer ID, minimum replay epoch, and minimum owner term. | Wrong key, cluster, signer, and stale generation tests. |
| Fault application | Advance only to nondecreasing ticks and apply typed directed link faults through the simulator API. | Valid replay and sequence/tick boundary tests. |
| Transactional state | Replay into a cloned simulator and publish only after all verification stages pass. | Trace-seal failure and missing-signature no-mutation tests. |
| Trace seal | Sign and verify event digest plus count with the same identity and replay epoch. | Trace-seal tamper test and benchmark artifact. |
| Compliance integration | Add six non-secret gates, dedicated validator, full-suite invocation, independent audit, and socket/queue review. | 82-gate metrics, audit, and review artifacts. |

## Gate requirements

The six correctness gates are `signed_replay_manifest_required`, `replay_schedule_hash_bound`, `replay_sequence_tick_bounds_enforced`, `trusted_key_cluster_epoch_binding`, `tampered_schedule_rejected`, and `trace_seal_verification`. All must have executable integration evidence before publication. The report must also state that production key custody, key-registry distribution, authenticated remote transport, and cloud-region replay authority are not proven locally.

## Validation sequence

Run the focused Phase 31 test first. Run the Phase 30 regression suite next. Generate the non-secret Phase 31 benchmark artifact. Run the complete compliance validator, independent 82-gate audit, socket/queue review, Rust formatting and all-target tests, shell/Python syntax checks, skill validation, whitespace checks, and staged-diff review. Remove build artifacts, publish source and evidence, regenerate metrics against the published commit, and publish a metadata-only follow-up commit.

## Reusable process

The repeatable workflow is: inspect and persist a baseline; define bounded typed contracts; implement the smallest safe slice; test failure and rollback paths; extend compliance evidence; update architecture and reusable skill references; validate in layers; publish implementation first; refresh metadata second; and report exact commits, boundaries, and artifacts. This sequence is now captured in the reusable `agentic-system-engineering` skill.
