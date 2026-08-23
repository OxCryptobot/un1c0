# Phase 57: typed semantic validation snapshots

## Objective

Phase 56 reuses per-function semantic reports but still requires a caller to hold the exact UEG/profile relationship. Phase 57 introduces a typed `SemanticValidationSnapshot` that binds a valid semantic report to a `SemanticFingerprint` and target binding. Code generation can consume the snapshot only after recomputing and comparing the current UEG/profile fingerprints.

## Contract

`SemanticValidationSnapshot::capture` runs the complete Phase 53 semantic validator and refuses to create a snapshot for invalid UEG or target diagnostics. A snapshot stores the target, exact profile/function/root fingerprint, and a valid cloned report.

`verify_for` recomputes the current fingerprint, rejects profile or UEG changes, and rejects an invalid stored report. `IncrementalCodeGenerator::emit_remaining_with_snapshot` performs this verification before invoking an emitter or sink. The existing `next_chunk`, `emit_remaining`, and cached generation APIs remain unchanged and continue to run their normal validation gates.

## Milestones

| Milestone | Outcome | Evidence |
|---|---|---|
| 57.1 | Typed valid-report snapshot | Invalid semantic reports cannot be captured |
| 57.2 | Exact UEG/profile binding | Source, span, expression, order, target, and capability changes reject snapshots |
| 57.3 | Pre-emitter gate | Stale snapshots fail before sink callbacks or generated chunks |
| 57.4 | Compatibility | Existing generation APIs and target bindings remain unchanged |
| 57.5 | Performance evidence | Capture versus same-input verification across 1–32 functions |

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Validity | Capture requires zero error diagnostics |
| UEG identity | Any fingerprint-changing UEG mutation rejects the snapshot |
| Profile identity | Target or capability-profile changes reject the snapshot |
| Execution order | Verification occurs before emitter and sink execution |
| Fail closed | Snapshot errors do not fall back to unchecked emission |
| Compatibility | Existing incremental generation regressions remain green |
| Authority | Snapshot adds no filesystem, process, network, secret, or cluster authority |

## Explicit non-goals

Phase 57 does not persist snapshots, distribute them, sign them, establish remote trust, or replace the semantic validator. A snapshot is local in-memory evidence for a specific UEG/profile pair, not an authorization token or compiler proof.
