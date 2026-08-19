# Phase 35 Cross-Region Multi-Leader Failover and Witness Arbitration

## Objective

Phase 35 extends the Phase 34 replicated recovery authority into a cross-region multi-leader failover layer. Regional leaders can prepare signed candidate promotions concurrently, while independent witness nodes arbitrate exactly one proposal per round. The winning decision creates an externally verifiable fencing token, but the new region becomes active only after trusted external fence admission.

## Contracts

| Contract | Responsibility |
|---|---|
| `RegionalLeader` | Bind leader identity, region, term, ownership epoch, membership epoch, replicated log index, snapshot hash, and public key. |
| `LeaderFailoverProposal` | Sign a domain-separated candidate proposal that covers cluster, resource, round, leader, region, generations, log index, snapshot hash, and signer key. |
| `WitnessVote` | Sign one witness decision for one proposal digest in one round, binding witness identity, witness membership epoch, and proposal digest. |
| `MultiLeaderFailoverAuthority` | Verify registered leaders, collect distinct witness votes, reject conflicting quorum evidence, select one decision, issue a signed fencing token, and enforce monotonic rounds and log state. |
| `TrustedFencingAuthorityRegistry` | Pin authority IDs to Ed25519 public keys and reject implicit key rebinding. |
| `ExternalFenceState` | Verify token domain, signer, resource, authority, membership, term, ownership, and fence generations before external activation. |
| `MultiLeaderChaosSimulator` | Exercise drop, delay, duplicate, heal, and post-delay witness delivery across regions while recording sanitized trace evidence. |

## Arbitration sequencing

A leader proposal is accepted only after signature verification against the registered leader key and validation against current membership epoch, candidate region, leader generations, snapshot, and committed log index. A witness can vote for one digest in one round. Duplicate votes are idempotent; an attempt to vote for a second digest increments the split-brain rejection path and fails closed.

A decision requires a majority of distinct witnesses. If more than one proposal reaches quorum in a round, arbitration rejects the round. A newer round cannot be arbitrated until the previous decision has been externally fenced. Replaying the exact current decision returns the same decision and token rather than advancing the fence epoch.

## Fencing security

The external token uses a fixed domain separator and protocol version. The signed payload includes cluster, resource, candidate owner, owner term, ownership epoch, membership epoch, fence epoch, authority ID, log index, and authority public key. `ExternalFenceState` pins the first accepted authority identity and public key, rejects implicit authority changes, enforces monotonic membership/term/ownership/fence frontiers, treats exact token replay idempotently, and rejects same-epoch conflicts. The multi-leader coordinator accepts only the exact current decision and updates active owner state after external admission.

## Phase 35 gates

| Gate | Evidence |
|---|---|
| `multi_leader_proposal_signature_required` | Registered leader signatures and payload bindings verify. |
| `witness_quorum_arbitration_required` | A majority of distinct witnesses is required before decision. |
| `one_witness_vote_per_round` | Duplicate same-digest vote is idempotent; second digest fails closed. |
| `conflicting_quorum_split_brain_rejected` | More than one quorum or conflicting decision is rejected. |
| `stale_multi_leader_log_rejected` | Leaders behind the committed log cannot propose or win. |
| `fencing_token_domain_bound` | Domain/version and all resource generations are signed and checked. |
| `fencing_authority_registry_pinned` | Unknown authority and key rebinding are rejected. |
| `fencing_generation_rollback_rejected` | Membership, term, ownership, and fence regressions fail without mutation. |

## Boundaries

The local implementation does not claim real cross-region transport, witness process durability, cloud failure-detector truth, process or socket fencing, DNS/load-balancer convergence, external registry governance, or multi-machine key custody. The chaos harness is deterministic evidence for proposal/vote ordering and fail-closed split-brain behavior.

## Reproduction

```bash
cargo test --test phase35_multileader_witness_integration -- --nocapture
cargo run --example phase35_multileader_witness_benchmark -- --output benchmarks/phase35_multileader_witness_metrics.json
```

## References

[1]: ../src/multileader_recovery.rs "Phase 35 multi-leader recovery authority"
[2]: ../src/replicated_recovery.rs "Phase 34 external fencing and trusted authority registry"
[3]: ../tests/phase35_multileader_witness_integration.rs "Phase 35 adversarial integration tests"
[4]: ../examples/phase35_multileader_witness_benchmark.rs "Phase 35 sanitized benchmark"
[5]: ../docs/PHASE35_MULTILEADER_WITNESS_AUDIT_NOTES.md "Phase 35 audit notes"
