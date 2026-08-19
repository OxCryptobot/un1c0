# Phase 35 Multi-Leader Failover and Fencing Security Report

## Executive summary

Phase 35 implements a deterministic cross-region multi-leader failover authority with automated witness arbitration. Three regional leaders can produce independently signed candidate proposals, while five witnesses vote on proposal digests. A proposal wins only with a majority of distinct witnesses, and a witness cannot vote for two digests in one arbitration round. The authority rejects conflicting quorum evidence, stale replicated-log state, wrong signer bindings, and a second decision before the previous decision has completed external fencing.

The Ed25519 external-fencing audit found and closed four important risks in the Phase 34 boundary: missing protocol domain separation, unpinned authority identity, generation rollback acceptance, and active-owner mutation before external fence admission. Phase 35 adds a signed token domain/version, trusted authority registry, authority-ID pinning, monotonic membership/term/ownership/fence checks, exact current-decision admission, and replay-safe arbitration. The local evidence still does not prove real process, socket, network, DNS, cloud-region, or failure-detector enforcement.

## Measured evidence

| Evidence | Result |
|---|---:|
| Phase 35 integration tests | 8 passed |
| Phase 34 regression tests | 6 passed |
| Signed multi-leader proposal verification | Passed |
| Witness quorum arbitration | Passed |
| One-vote-per-round conflict rejection | Passed |
| Stale log and wrong signer rejection | Passed |
| Domain tampering and authority rebinding rejection | Passed |
| Generation rollback rejection with no state mutation | Passed |
| Previous-round external-fence ordering | Passed |
| Dynamic three-leader witness chaos | Passed |

The deterministic benchmark reports three leaders, five witnesses, two sequential fenced owners (`region-b` then `region-c`), three witnesses for each decision, one partition step, one dropped vote, one delayed vote, one post-delay delivery, one duplicate delivery, monotonic fence epochs, and `safety_passed=true`. The active owner is `region-c` at accepted fence epoch 2. The benchmark records only boolean outcomes, owner labels, counters, hashes, and trace digests; raw key, signature, and token fields are excluded.

## Ed25519 security audit

| Control | Phase 35 behavior | Residual boundary |
|---|---|---|
| Domain separation | Token payload includes `un1c0/external-fencing-token/v1` and protocol version 1. | Future protocol rotation still requires an explicit compatibility policy. |
| Canonical binding | Cluster, resource, owner, terms, epochs, authority, log index, public key, and signature are covered. | Canonical serialization must remain stable across non-Rust implementations. |
| Signer authorization | Registry maps authority ID to one public key and rejects key rebinding. | Registry replication and operator governance are external. |
| Generation ordering | Membership, owner term, ownership epoch, and fence epoch regressions are rejected before mutation. | Cross-machine monotonic storage is not implemented locally. |
| Replay | Exact token replay is idempotent; same-epoch conflicting tokens fail. | External consumers must persist the same state atomically. |
| Decision binding | Only the exact current authority decision can be externally admitted. | A real gateway must enforce the token on every protected operation. |

## Split-brain analysis

A split-brain candidate can be prepared by multiple leaders, but a witness can cast at most one vote per round. If two candidate digests receive conflicting witness support, the authority rejects the second vote at that witness and rejects any round with more than one quorum. A leader whose log index is behind the committed authority cannot enter or win a round. After one decision is committed, the next round is blocked until the previous token has been externally admitted. This ordering prevents the authority from advancing its fence epoch while the prior owner handoff is still only local evidence.

The local harness models directed drop, delay, duplicate, and healing faults. It does not execute multiple OS processes with independent durable stores, and therefore cannot demonstrate the impossibility of split-brain behavior under a compromised witness registry, a faulty failure detector, key theft, or a network that violates the simulator’s delivery model.

## Recommended next phase

The next high-value phase is authenticated multi-process transport for leader proposals and witness votes. It should persist witness vote reservations and external-fence state in independent stores, inject crashes between vote acceptance and durable reservation, require gateway-level token checks on every write, and run cross-host chaos against real process and socket boundaries.

## References

[1]: ../src/multileader_recovery.rs "Phase 35 multi-leader authority and chaos simulator"
[2]: ../src/replicated_recovery.rs "Phase 34 fencing token and trusted authority implementation"
[3]: ../tests/phase35_multileader_witness_integration.rs "Phase 35 security integration suite"
[4]: ../examples/phase35_multileader_witness_benchmark.rs "Phase 35 benchmark"
[5]: ../docs/PHASE35_MULTILEADER_WITNESS_PLAN.md "Phase 35 implementation plan"
[6]: ../docs/PHASE35_MULTILEADER_WITNESS_AUDIT_NOTES.md "Phase 35 audit notes"
