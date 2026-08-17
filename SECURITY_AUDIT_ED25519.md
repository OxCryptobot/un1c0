# Ed25519 Proposal Signing and Canary Evidence Security Audit

## Scope

This audit covers `src/evolution.rs`, focusing on signed proposal identity, trust-anchor handling, persisted-ledger validation, canary report integrity, state transitions, and atomic persistence. Findings are based on the implementation at commit `d520a13` and the current regression coverage.

## Observed strengths

`SignedEvolutionProposal::verify` recomputes the proposal content hash, checks the canonical `evo-<hash-prefix>` identifier, enforces Ed25519 key and signature lengths, reconstructs the signing payload, and verifies the signature. `EvolutionLedger::open` validates every persisted record before loading it, and transition failures restore the original in-memory record after validation or persistence errors. `CanaryReport` bounds check count, run identifiers, changed-file count, relative paths, and digest shape. Terminal transitions are gated on the active canary run and all checks passing.

## Findings

| ID | Severity | Finding | Impact | Required treatment |
|---|---|---|---|---|
| E-01 | High | The signed proposal embeds its own public key, and `EvolutionLedger` accepts any key that verifies the signature. There is no trusted-signer registry or key-rotation/revocation boundary. | Any actor able to write a valid ledger record can introduce a new signing key and approve an evolution proposal. Ed25519 proves possession, not authorization. | Add an explicit trusted-signer store and require signer identity plus public-key match before proposal admission and persisted-record loading. |
| E-02 | High | The legacy `finalize_canary(id, passed, evidence)` API accepts caller-supplied pass/fail state and arbitrary evidence text. | A caller can mark a canary applied without a structured report or machine-verifiable check set. | Remove or restrict the legacy path and require a structured, run-bound report with a deterministic evidence digest. |
| E-03 | Medium | `CanaryReport::new` validates changed-file hash syntax but does not compute hashes from the verified workspace or bind them to a verification result. | A malicious or faulty producer can self-attest arbitrary 64-character digests. | Add a trusted report constructor that hashes bounded file bytes from a declared workspace and rejects symlink/path escapes. |
| E-04 | Medium | `EvaluationCheck` permits `passed=true` with a nonzero exit code and has no semantic status/exit-code consistency rule. | A malformed report can claim success while carrying contradictory process evidence. | Enforce `passed == (exit_code == Some(0))` for process-backed checks, or model unavailable/timeout states explicitly. |
| E-05 | Low | Signer IDs, run IDs, check names, and evidence fields are length-bounded but not normalized for control characters. | Logs and audit displays may be ambiguous or vulnerable to newline/control-character injection. | Reject control characters and normalize identifiers at trust boundaries. |

## Implemented hardening

The current working batch implements the planned controls. `TrustedSignerStore` binds signer IDs to fixed Ed25519 public keys and rejects unknown signers, mismatched keys, and implicit key rebinding. `EvolutionLedger::open_with_trusted_signers` validates persisted records against that trust boundary before loading them, while `propose` and every transition re-check authorization. The caller-controlled legacy finalization method was removed; application now requires a structured report whose run ID and exact changed-file set match the signed proposal.

`CanaryReport::from_workspace` canonicalizes a declared root, rejects absolute paths, traversal, symlinks, non-regular files, workspace escapes, and oversized files, and computes SHA-256 hashes from the actual bounded file bytes. `EvaluationCheck::from_output` and report validation reject oversized output, control-character identifiers, malformed digests, and contradictory pass/exit-code combinations. Regression coverage now exercises forged signatures, unknown and mismatched signers, unsafe paths, missing workspace files, contradictory checks, replayed canary run IDs, failed reports, and successful persistence/reopen.

## Residual findings after hardening

| ID | Status | Remaining concern |
|---|---|---|
| E-01 | Mitigated in-process | The trusted-key store is supplied by the host process and is not yet backed by a separately authenticated key-distribution or revocation service. |
| E-02 | Mitigated | Structured finalization is now the only application path; deployment code must still ensure reports originate from the trusted verifier adapter. |
| E-03 | Mitigated for workspace-derived reports | Callers can still construct the low-level `CanaryReport::new` API with self-attested digests; production adapters should use `from_workspace`. |
| E-04 | Mitigated | Process-backed checks enforce pass/exit-code consistency; unavailable and timeout states remain represented by the higher-level verification result contract. |
| E-05 | Mitigated | Identifier controls are rejected at signing and report construction boundaries. |


## Residual design boundary

Ed25519 signatures authenticate a payload but do not establish organizational authorization, freshness, or recovery policy. Production deployment still needs an external trust-store distribution process, key rotation, revocation, operator approval, and an append-only audit sink. The ledger remains a local fail-closed state machine, not a complete supply-chain transparency service.
