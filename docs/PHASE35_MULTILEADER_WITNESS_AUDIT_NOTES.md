# Phase 35 Multi-Leader and Witness-Arbitration Audit Notes

## Baseline

The published Phase 34 head is `e0218d7`, synchronized with `origin/main` and clean. The Phase 30–34 recovery regression surface passes 46 tests in total: 9 Phase 30, 8 Phase 31, 16 Phase 32, 7 Phase 33, and 6 Phase 34.

## Observed Phase 34 fencing behavior

`ExternalFencingToken::verify` validates identifier shape, positive generations, key/signature lengths, cluster/resource binding, signer public-key equality, and Ed25519 over a canonical JSON payload. `ExternalFenceState::apply` enforces strictly increasing `fence_epoch`, exact replay idempotence, and same-epoch conflict rejection. The Phase 34 authority binds the token to the recovery-log index, candidate region, membership epoch, and next fence epoch before applying controller promotion.

## Security risks to close in Phase 35

| Severity | Finding | Impact | Phase 35 remediation |
|---|---|---|---|
| High | The legacy `ExternalFenceState::apply` accepts a caller-supplied key and has no required authority registry or expected authority ID before first activation. | A caller that supplies an untrusted authority key can potentially admit a validly signed token from an unauthorized authority. | Add a trusted-authority registry path, bind authority ID to the registered key, retain accepted authority identity, and make registry-backed admission the multi-leader path. |
| High | External fence state checks fence epoch but does not require membership epoch, owner term, or ownership epoch to be monotonic. | A newer fence epoch could carry older authority generations and undermine ordering across failover rounds. | Track accepted membership/term/ownership frontiers and reject regression before mutation. |
| Medium | The canonical token payload has no explicit protocol/domain version. | Cross-protocol signature reuse is harder to detect during future schema evolution. | Add a fixed domain separator/version to the canonical payload and cover it with Ed25519. |
| High | Phase 34 has one authority object; it does not model independent regional leaders producing concurrent proposals. | Split-brain safety is tested only through one arbiter, not competing leader identities and witness votes. | Add signed leader proposals, one-vote-per-round witness nodes, quorum arbitration, explicit conflicting-quorum rejection, and stale-log rejection. |
| High | No automated witness election/arbitration exists. | A partition can leave multiple locally prepared leaders without a deterministic external decision. | Require a majority of distinct witness votes for exactly one proposal; reject duplicate/conflicting witness votes. |
| Medium | The external state does not bind an accepted authority identity to the current resource owner. | A different authority could supersede a prior owner if it presents a higher fence epoch. | Add trusted authority registry and reject authority changes unless an explicit authority transition is modeled. |

## Phase 35 invariants

1. A leader proposal is signed by the registered regional leader key and binds cluster, resource, leader, region, owner term, ownership epoch, membership epoch, log index, snapshot hash, and arbitration round.
2. Each witness signs at most one proposal digest per round and its vote binds witness ID, witness membership epoch, round ID, and proposal digest.
3. A decision requires a majority of distinct witnesses and exactly one proposal with quorum. Ambiguous or conflicting quorum evidence fails closed.
4. A candidate whose replicated log index is behind the coordinator’s committed index cannot win, even with witness votes.
5. External fencing admission uses an authority registry, a fixed token domain separator, and monotonic membership/term/ownership/fence frontiers.
6. Exact token replay is idempotent; same-fence-epoch conflicts, authority-key changes, cross-resource tokens, signer mismatches, and all generation regressions are rejected without state mutation.
7. A committed decision fences every prior active region before the new owner is externally admitted; no two active owner identities are returned by the coordinator.

## Boundaries

The Phase 35 local harness will not claim real network transport, process fencing, DNS/load-balancer convergence, cloud failure-detector truth, independent durable witness storage, or cross-machine key custody. It will provide deterministic evidence for signed leader proposals, one-vote witness behavior, quorum arbitration, stale-log rejection, and fail-closed external fence validation.
