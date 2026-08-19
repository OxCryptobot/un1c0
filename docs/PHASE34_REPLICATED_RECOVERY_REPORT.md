# Phase 34 Replicated Recovery Authority Report

## Summary

Phase 34 implements a replicated recovery-authority layer above the Phase 33 disaster-recovery controller. Authority commands are appended to a bounded log with contiguous indices, positive terms, canonical SHA-256 entry hashes, explicit acknowledgements, and commit/applied frontiers. The authority cannot apply a recovery decision until its configured quorum is satisfied.

Observer membership uses a Raft-style joint transition. The old observer set and new observer set are both retained until the joint entry commits with a majority from each set. Final membership is appended only after joint commit and requires a majority of the new set. The joint entry carries the new trusted observer public-key registry and advances the signed observation membership epoch atomically with the authority transition.

Committed recovery entries issue an Ed25519-signed external fencing token. The token is bound to the cluster, resource, owner region, owner term, ownership epoch, observer membership epoch, fencing epoch, authority identity, and recovery-log index. `ExternalFenceState` accepts a newer token, treats the exact token replay as idempotent, rejects stale epochs, and rejects conflicting tokens at the same epoch. The benchmark intentionally records only sanitized outcomes and does not print or persist raw key or signature material.

## Measured evidence

| Evidence | Result |
|---|---:|
| Phase 34 integration tests | 6 passed |
| Phase 30–33 recovery regressions | 40 passed |
| Joint old/new quorum exclusion | Passed |
| Finalization ordering and new-set quorum | Passed |
| Signed external-fence activation and admission | Passed |
| Stale/tampered/rollback fence rejection | Passed |
| Replicated authority snapshot restart | Passed |
| Four-node dynamic partition chaos | Passed |
| Full Rust all-target compile | Passed |

The deterministic benchmark reports four nodes, one dynamic partition step, membership epoch 2, joint and final transition indices 1 and 2, recovery commit index 3, active fence epoch 1, active owner `region-b`, one stale-epoch rejection, one stale-fence rejection, and `safety_passed=true`. The benchmark trace digest is `148c485efb985d777c48b879fb7145081c6b033405c0dd46fe935d40da829f76` for the current deterministic schedule.

## Safety interpretation

The central invariant is that observer membership and recovery authority change together only through a quorum-committed log. A partitioned minority can append no applied authority by itself because joint entries require both old and new majorities, while final entries require the new majority. Recovery commits are ordered after membership finalization and reference the exact pending Phase 33 proposal. The fencing token then creates a verifiable external handoff boundary rather than relying on an unbound region label.

| Control | Local guarantee | Not proven locally |
|---|---|---|
| Joint quorum | Both old and new majorities are required for the transition entry. | Real network transport and crash timing across processes. |
| Log binding | Entry index, term, command hash, and frontiers are validated. | Cross-host replicated storage or distributed compare-and-swap. |
| Membership authority | New observer keys and epoch are applied with the joint entry. | Externally governed observer-registry consensus. |
| Fence token | Signature and resource/authority/generation bindings are verified. | Actual process, socket, DNS, load-balancer, or service-admission enforcement. |
| Chaos schedule | Drop, delay, duplicate, healing, stale epoch, and stale fence paths are replayable. | Cloud-region failure-detector truth and production network behavior. |

## Next phase recommendation

The next best-value phase is to connect this authority layer to authenticated transport and an actual service-admission fence. Each replica should exchange signed log entries and acknowledgements over the repository’s authenticated socket envelope, persist commit frontiers in a replicated store, and require the external token at the write gateway and worker scheduler. Crash injection should cover every boundary between log append, quorum commit, controller commit, token persistence, and external admission.

## References

[1]: ../src/replicated_recovery.rs "Phase 34 replicated recovery implementation"
[2]: ../tests/phase34_replicated_recovery_integration.rs "Phase 34 integration suite"
[3]: ../examples/phase34_replicated_recovery_benchmark.rs "Phase 34 benchmark"
[4]: ../docs/PHASE34_REPLICATED_RECOVERY_PLAN.md "Phase 34 implementation plan"
[5]: ../docs/PHASE34_REPLICATED_RECOVERY_AUDIT_NOTES.md "Phase 34 audit notes"
