# Phase 36 Authenticated Transport Audit Notes

## Baseline

The published repository head is `8b6e381`, synchronized with `origin/main`, and the worktree is clean. The Phase 35 witness/fencing integration suite passes 8 tests. The repository emits only pre-existing warnings in `src/subagent.rs` and the legacy Go walker binary path during the baseline test command.

## Observed Phase 35 behavior

Phase 35 signs leader failover proposals and witness votes in-process. The multi-leader authority verifies registered leader and witness keys, enforces one witness digest per round, requires a witness majority, rejects conflicting quorum evidence, blocks newer arbitration until prior external fencing, and admits only the exact current decision through a trusted fencing registry. The external token state is hash-bound and monotonic across authority, membership, term, ownership, and fence generations.

## Phase 36 risks

| Severity | Finding | Impact | Phase 36 remediation |
|---|---|---|---|
| High | Proposals and witness votes are passed as typed in-memory values rather than authenticated process-to-process envelopes. | Cross-process identity, replay, nonce, and channel binding are not exercised. | Add domain-separated authenticated envelopes with sender, receiver, cluster, connection epoch, sequence, nonce, payload digest, and Ed25519 signature. |
| High | Witness vote reservation is only in memory. | A witness restart can vote twice in one round or lose a reservation before durable acknowledgement. | Add atomic file-backed reservation state with hash binding, create-new staging, replay idempotence, and no mutation on rejected restore. |
| High | External-fence admission is modeled by a local state object but not enforced at the protected write gateway. | A downstream operation could ignore the accepted token or use stale owner state. | Add a gateway admission contract requiring an accepted token for every write, with resource/owner/fence checks before mutation. |
| Medium | Transport envelope replay protection is not bound to a connection epoch and receiver identity. | Delayed messages from a prior process incarnation may be accepted after restart. | Require monotonic connection epochs and bounded per-sender sequence/nonce windows. |
| Medium | Crash boundaries between reservation persistence and vote acknowledgement are not modeled. | Acknowledge-before-durable or durable-before-apply ordering bugs could reintroduce split-brain votes. | Add deterministic crash injection at pre-write, staged, fsynced, renamed, and acknowledgement boundaries. |
| Medium | Cross-host ownership is represented in the authority but not in a transport/queue handoff simulation. | A stale host may continue sending a valid-looking vote after ownership transfer. | Bind envelopes to sender host/leader identity and accepted authority epoch, then test stale-host rejection after transfer. |

## Phase 36 invariants

1. Every proposal/vote envelope is signed over a fixed domain, cluster, resource, sender, receiver, connection epoch, sequence, nonce, payload digest, and signer key.
2. The receiver verifies identity, direction, cluster/resource, epoch, sequence, nonce, payload hash, signature, and replay window before payload dispatch.
3. Durable witness reservations are atomic, hash-bound, bounded, idempotent for exact replay, and fail closed on conflicting same-round reservations.
4. No acknowledgement is emitted before the reservation is durable; restart restores reservations before accepting new votes.
5. A protected write gateway requires the exact currently accepted external fencing token and rejects stale, wrong-resource, wrong-owner, wrong-authority, and unverified tokens before mutation.
6. A connection-epoch transition invalidates old envelopes and reservations from prior host incarnations unless an explicit recovery protocol rebinds them.
7. Crash injection at every persistence boundary leaves either the old valid reservation or the new fully validated reservation, never a partial authority state.

## Boundaries

The Phase 36 local implementation will prove envelope and persistence contracts with deterministic file-backed fixtures. It will not claim kernel-level transport security, real TLS/mTLS deployment, distributed filesystem semantics, cloud-region durability, hardware fencing, or gateway enforcement outside the provided typed adapter.
