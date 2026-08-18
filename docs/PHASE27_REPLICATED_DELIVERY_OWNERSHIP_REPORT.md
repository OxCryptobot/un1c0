# Phase 27 Replicated Authenticated Delivery and Cross-Host Queue Ownership

**Project:** un1c0 local-first AI-programmable agent runtime
**Status:** Implemented and integration-tested

## Executive summary

Phase 27 extends the Phase 26 durable socket queue from local post-flush acknowledgement to a quorum-aware replicated delivery contract. A flushed frame remains in the FIFO queue until authenticated acknowledgements reach the configured quorum. Each acknowledgement binds the queue peer, sequence, exact frame digest, owner identity, owner term, ownership epoch, acknowledgement sender, replay epoch, and canonical hash. Same-sender duplicates are idempotent; a conflicting hash from the same sender is rejected.

Durable queue snapshots now persist ownership leases and per-sequence acknowledgements. A new trusted owner may import a source-bound snapshot through an explicit cross-host restore path, then apply a higher-term and higher-epoch ownership transfer after the previous lease is expired or superseded. Successful transfer clears stale acknowledgement evidence and transient active-delivery authority. The new owner can verify and deliver the retained payload, including payloads originally signed by the previous owner.

The compliance artifact increases from **56 to 60 passing gates**. Transport delivery, replica quorum scheduling, process supervision, and cross-host network authority remain explicit deployment boundaries.

## State and acknowledgement invariants

| Invariant | Enforcement |
|---|---|
| Exact frame binding | Acknowledgement sequence and digest must match the FIFO queue head. |
| Authenticated sender | Ed25519 envelope sender, trusted key, cluster, replay epoch, and term floor are verified before mutation. |
| Owner binding | Acknowledgement owner and ownership epoch must match the active lease. |
| Quorum | A frame is committed only after the configured number of distinct trusted senders acknowledge it. |
| Idempotence | Same sender and same acknowledgement hash produce no additional state. |
| Collision resistance | Same sender with a different hash fails closed. |
| FIFO commit | Only the retained queue head can be removed. |
| Atomicity | Persistence failure restores the queue, quota, ownership, and acknowledgement state. |

## Cross-host ownership

Ownership is a hash-bound lease containing the queue peer, owner identity, owner term, expiry tick, and monotonically increasing ownership epoch. A transfer is itself carried through an authenticated consensus envelope. The sender must be the previous owner, the recipient must be the local new owner, the owner term must not regress, and the ownership epoch must increase. A transfer during an authoritative unexpired lease is rejected unless superseded by a higher owner term.

A new owner restores the complete source snapshot only after validating the cluster, source identity, replay epoch, trusted membership, quota state, frame hashes, ownership hashes, acknowledgement hashes, and quorum bounds. Once the transfer persists, old acknowledgement evidence is cleared. The new owner can retry the retained queue head and produce a fresh authenticated acknowledgement under the new lease.

## Integration evidence

| Test | Coverage | Result |
|---|---|---|
| `authenticated_delivery_waits_for_quorum_and_commits_idempotently` | Quorum waiting, local/remote acknowledgements, duplicate idempotence, commit | Passed |
| `replicated_ack_state_survives_restart_before_remote_quorum_arrives` | Durable acknowledgement persistence and restart completion | Passed |
| `cross_host_owner_transfer_imports_queue_and_new_owner_delivers` | Source-bound restore, authenticated transfer, new-owner delivery | Passed |
| `stale_or_misbinding_transfer_fails_without_mutating_owner` | Stale lease rejection and no-mutation behavior | Passed |

## Production boundaries

The local contracts do not claim a real cross-host replication channel, network quorum scheduler, process supervisor, replicated log authority, or failure detector. Production promotion requires authenticated transport for acknowledgement envelopes, durable membership and owner-term coordination, lease-clock safety across hosts, partition and split-brain tests, process crash injection, durable retry scheduling, and operational metric export.

## References

[1]: ../src/consensus.rs "Phase 27 replicated delivery and ownership implementation"
[2]: ../tests/phase27_replicated_delivery_ownership_integration.rs "Phase 27 integration tests"
[3]: ../benchmarks/security_compliance_metrics.json "Current security metrics artifact"
[4]: ../benchmarks/security_compliance_audit.json "Independent security metrics audit"
[5]: ../docs/CONSENSUS_STATE_REPLICATION.md "Consensus replication architecture"
