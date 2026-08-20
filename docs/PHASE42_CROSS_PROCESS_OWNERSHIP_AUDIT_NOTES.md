# Phase 42 Cross-Process Ownership and Managed-Volume Recovery Audit Notes

## Phase 41 Ed25519 verification

`CasWriteRequest::verify` validates the domain and protocol version, bounded identifiers and digests, cluster/resource/snapshot identity, pinned writer public-key equality, Ed25519 public-key shape, signature shape, signature verification over canonical fields, and the request content digest. The signed payload includes writer identity and epoch, request nonce, expected and proposed generations/hashes, payload hash, and public key. `ReplicaDurabilityAcknowledgement::verify` applies the same sequence to a pinned replica key and canonical payload that additionally includes durability mode, flush sequence, observed tick, and TTL.

The verification order is fail closed: malformed evidence, unknown identities, key rebinding, resource misbinding, signature failure, or digest mismatch returns before the coordinator considers quorum or mutates CAS state. Canonical serialization is deterministic through typed serde structs and digest input tuples.

## Distinct-replica quorum enforcement

`SingleWriterCasStore::commit` first verifies the signed writer request, handles exact nonce idempotence/conflict, checks writer epoch and identity continuity, then requires the exact current generation/hash and the next generation. It verifies each acknowledgement against the pinned replica registry, exact request/proposed hashes, freshness bounds, and the active snapshot identity. A `BTreeMap<replica_id, event_hash>` deduplicates same-replica evidence and rejects same-replica conflicting hashes. Commit is admitted only when the number of distinct accepted replica IDs reaches the configured quorum. State and receipt insertion occur only after every check passes.

The receipt persists the request hash, generation, content hash, quorum count, replica-set digest, and receipt hash. Snapshot validation additionally checks receipt hashes and contiguous generations. The local implementation therefore proves cryptographic identity, exact binding, quorum counting, and no-mutation failure behavior for the tested adapter contract.

## Phase 42 gaps

Phase 41 does not coordinate independent processes competing for one shared logical snapshot path. It has no OS-backed lease owner, fencing epoch, compare-and-swap lock record, lease expiry recovery, or cross-process crash observer. It also trusts a signed replica acknowledgement as an adapter assertion; the local code cannot prove that the storage controller flushed stable media or that a managed volume replicated the write across independent failure domains.

Phase 42 should add a bounded cross-process ownership record with owner identity, process instance, ownership epoch, lease expiry, last committed generation/hash, and a fencing nonce. Acquisition must use exclusive create or atomic replacement with an expected owner/epoch hash. Renewal and release must be owner-bound; stale or expired owners must fail before CAS writes. Every write intent must include the active ownership epoch, and a higher epoch must fence lower epochs.

Managed-volume recovery verification should use an explicit adapter trait or test seam that reports `Prepared`, `Flushed`, `Replicated`, `Recovered`, or `Unknown`. Only a trusted `Replicated` acknowledgement from the required independent replica set can satisfy the promotion gate. `Unknown`, stale, contradictory, same-replica, or storage-controller-negative evidence must leave the logical state unchanged. Real cloud-volume behavior, process termination, failure-domain placement, and controller truth remain deployment adapters and must not be simulated as production evidence.
