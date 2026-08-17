# Evolution Ledger Persistence and Recovery Review

## Scope

This review covers the persistence boundary in [`src/evolution.rs`](../src/evolution.rs), especially `EvolutionLedger::transition` and `persist_locked`, plus the public integration coverage in [`tests/evolution_ledger_integration.rs`](../tests/evolution_ledger_integration.rs).

## Atomic transition sequence

| Stage | Behavior | Recovery guarantee |
|---|---|---|
| Load | `EvolutionLedger::open_with_trusted_signers` parses the JSON map and verifies every persisted signature, signer binding, proposal ID, and state invariant before returning a ledger. | Corrupt, forged, unknown-signer, or state-inconsistent records fail closed during open. |
| Mutate | `transition` clones the original record, applies the requested state change, then re-verifies the signed proposal and validates the resulting record. | Invalid transitions restore the original in-memory record before returning the error. |
| Stage | `persist_locked` serializes the complete record map, creates a unique `json.tmp-<timestamp>-<pid>` file with `create_new`, writes all bytes, and calls `sync_all` on the file. | Partial writes remain in the temporary file and cannot replace the live ledger. |
| Commit | The fully written temporary file is atomically renamed over the ledger path. A best-effort parent-directory `sync_all` follows the rename. | Readers see either the previous complete file or the new complete file at the rename boundary. |
| Failure recovery | If staging or rename fails, the temporary file is removed. If persistence returns an error, `transition` reinserts the cloned original record into the in-memory map. | A failed approval, canary, apply, or rollback does not leave an optimistic state in memory. |

The persistence implementation uses a per-process suffix in addition to the millisecond timestamp to avoid temporary-name collisions between rapid transitions in one process. It deliberately does not log serialized proposals, signatures, source contents, or credentials.

## Rollback semantics

A failed evaluation check produces a structured `CanaryReport` with a nonzero exit code and is persisted as `ProposalState::RolledBack`; it is not treated as a successful application. A mismatched canary run, changed-file set, path escape, symlink, malformed digest, or contradictory pass/exit-code pair is rejected before state finalization. The separate `rollback` method requires a non-empty reason and only permits rollback from approved, canary, or applied states.

## Evidence

The public integration suite verifies valid Ed25519 signatures, forged-signature rejection, unknown and mismatched trusted signers, persistence through `Draft → Approved → Canary → Applied`, persistence of failed canaries as `RolledBack`, and recovery after an atomic persistence failure. The failure-recovery test replaces the live ledger path with a directory, forces rename failure, confirms the record remains `Draft`, and confirms no `evolution.json.tmp-*` file remains.

This is a local filesystem durability contract, not a distributed consensus protocol. Production promotion still requires an approval-controlled deployment process, durable storage guarantees, backup/restore policy, and operational monitoring for persistence failures.
