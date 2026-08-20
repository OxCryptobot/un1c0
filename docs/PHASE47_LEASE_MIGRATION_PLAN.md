# Phase 47: distributed multi-region lease migration

## Objective

Add a local-first, cryptographically verifiable lease-migration state machine for moving a resource between regions without allowing the destination to become active before the source is fenced. The module is deliberately an authority/evidence kernel: it does not claim to provide cloud failure detection, process termination, DNS convergence, database fencing, or independent-region durability.

## Protocol state machine

| State | Admission rule | Mutation allowed |
|---|---|---|
| `Stable` | A signed current lease is present and unexpired. | Source writes only while the lease is current. |
| `Draining` | A source-signed migration intent is bound to the current lease, destination, state hash, nonce, and bounded expiry. | New source writes are blocked; exact intent replay is idempotent. |
| `Prepared` | A distinct witness quorum signs the exact migration digest, and the source lease is still the current fenced candidate. | Destination may stage state but cannot serve writes. |
| `Released` | The source signs a release bound to the migration digest and its current ownership epoch. | Source is fenced; destination remains inactive until activation evidence is complete. |
| `Activated` | A destination activation is signed by the destination, uses a strictly higher ownership epoch, includes the release and quorum evidence, and is admitted once. | Destination writes are allowed under the new lease. |
| `Aborted` | The migration is expired or explicitly rejected before release. | Source can resume only through a new lease-bound operation; no destination activation is possible. |

## Signed evidence

`LeaseMigrationIntent` covers a fixed domain/version, cluster, resource, snapshot, source region/owner/process, destination region/owner/process, current ownership epoch, current record hash, generation, content hash, migration nonce, requested destination epoch, expiry tick, and signer key. `LeaseMigrationWitnessAck` covers the exact intent digest, witness ID, witness membership epoch, observed tick, TTL, and signer key. `LeaseMigrationRelease` covers the exact intent digest, source record hash, source epoch, release tick, and signer key. `LeaseMigrationActivation` covers the exact intent digest, release digest, destination record hash, strictly higher destination epoch, destination lease expiry, and signer key.

All signed payloads are domain-separated and canonical JSON encoded. Registries pin owner, witness, and region keys; rebinding is rejected. Hashes, identifiers, signatures, nonces, evidence counts, and TTLs are bounded. Raw key material, signatures, and full fencing tokens are never emitted in metrics or benchmark output.

## Split-brain invariants

1. **Single active region.** `Activated` is admitted only after a valid source release and a distinct witness quorum for the exact intent. A competing destination cannot activate from a different digest in the same round.
2. **Strict fencing order.** The source must be `Draining` before witness preparation, and `Released` before destination activation. A higher destination epoch does not bypass source release.
3. **Monotonic epochs.** Destination activation must use an epoch strictly greater than the source epoch and any previously observed activation epoch.
4. **Exact binding.** Cluster, resource, snapshot, source, destination, owner/process identities, generation, content hash, current record hash, nonce, and expiry are checked before state mutation.
5. **Quorum distinctness.** The witness quorum counts unique trusted witness identities only; duplicate acknowledgements do not increase quorum, and a witness cannot approve two different digests in one round.
6. **Replay safety.** Exact intent, acknowledgement, release, and activation replay is idempotent. Reuse of a nonce or migration digest with changed content is rejected without mutation.
7. **Freshness.** Expired intents, acknowledgements, releases, and activations are rejected. Clock uncertainty cannot extend validity.
8. **Durable recovery.** A bounded hash-bound snapshot is written with sibling staging, sync-before-rename, directory sync, stale-stage cleanup, and validate-before-restore. Restart cannot resurrect a pre-release source or post-release destination without complete evidence.

## Typed API shape

`LeaseMigrationAuthority` owns a bounded state and registries. Its public operations are `begin`, `accept_witness_ack`, `prepare`, `release_source`, `activate_destination`, `abort`, `snapshot`, and `restore`. Errors distinguish invalid input, unknown signer, stale evidence, conflict, quorum unavailable, source-not-drained, release missing, epoch regression, replay mismatch, persistence failure, and state transition violations.

## Testing and evidence plan

The Phase 47 integration suite must cover a valid handoff, exact replay, forged and misbound evidence, duplicate and conflicting witness votes, stale/expired evidence, destination activation before release, epoch rollback, competing migration digest, snapshot tampering and restart recovery, and a deterministic partition-style race where two destinations prepare but only one exact digest can activate. The benchmark must report sanitized state-transition counters and trace digests only.

The eight compliance gates are: signed intent binding; witness quorum distinctness; source-drain enforcement; release-before-activation ordering; strict destination epoch increase; replay/conflict handling; expiry and stale evidence rejection; and durable snapshot recovery/partition safety.
