# Phase 17: Remote Audit Ordering and Durable Sink Semantics

## Objective

Extend the existing signed audit chain with a transport-agnostic remote sink contract that preserves local durability, idempotent retry, signer authenticity, and explicit cross-node ordering semantics. Phase 17 does not open sockets or invent a distributed database; it defines the authenticated envelopes and deterministic sink decisions that an approved remote service can implement.

## Ordering model

Each `AuditRecord` remains locally ordered by its per-log sequence and hash chain. A `RemoteAuditEnvelope` adds a cluster ID, source node ID, stream ID, source sequence, previous record hash, record hash, signer identity, canonical record bytes, and envelope hash. A remote sink may accept an envelope only when its source stream is the next expected sequence, or when it is an exact idempotent replay of an already accepted record. A gap is returned as `AwaitingPredecessor`, not silently buffered without a bound.

Cross-node global ordering is not inferred from wall-clock timestamps. The remote service must choose an explicit order token or stream partition; this core exposes source stream and sequence only. A `GlobalAuditOrderToken` is accepted only when issued by the remote service and bound to the exact envelope hash.

## Authentication and integrity

The envelope hash covers cluster ID, source node, stream, source sequence, previous hash, record hash, signer ID, and canonical record bytes. The source Ed25519 signature is verified against the trusted historical signer registry before remote admission. Cluster and source identities are validated and cannot contain controls or path separators. The remote acknowledgement binds the envelope hash, source sequence, decision, and next expected sequence; it is itself signed by a trusted remote sink key.

## Durable outbox behavior

The existing local JSONL audit file remains the source of truth. A `DurableRemoteAuditSink` stores bounded outbox entries keyed by envelope hash using `create_new` and fsync. `enqueue` is idempotent for identical bytes and rejects hash collisions. `acknowledge` removes an outbox entry only after a valid remote acknowledgement proves acceptance or exact duplicate acceptance. Gap, retryable failure, and rejected decisions retain the outbox entry. `replay_pending` returns entries in source-stream/sequence order and never skips a lower sequence in the same stream.

## Failure semantics

| Condition | Result |
|---|---|
| Exact accepted replay | Idempotent success; outbox may be cleared. |
| Same sequence with different record hash | Fail closed; chain collision error. |
| Missing predecessor | `AwaitingPredecessor`; retain outbox. |
| Wrong cluster/source/stream | Reject; no mutation. |
| Invalid source or remote signature | Reject; no mutation. |
| Remote acknowledgement for unknown envelope | Reject; no mutation. |
| Higher remote order token with wrong envelope hash | Reject; no mutation. |
| Process crash before outbox removal | Replay discovers and resubmits the durable entry. |

## Production boundary

The transport, remote service quorum, global order-token allocator, key custody, TLS/mTLS channel, retry scheduling, and cross-host loss/reordering tests remain deployment responsibilities. The local implementation must make those boundaries explicit and testable without claiming that a file-backed outbox is a distributed audit service.
