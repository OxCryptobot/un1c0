# Phase 17 Remote Audit Ordering and Durable Sink

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** signed remote-audit envelopes, per-stream ordering, idempotent sink acknowledgements, and durable outbox replay
**Author:** Manus AI

## Architectural design

Phase 17 extends the existing Ed25519 hash-chained local audit log with a transport-agnostic remote handoff. `RemoteAuditEnvelope` binds cluster ID, source node ID, stream ID, source sequence, previous hash, record hash, canonical record bytes, signer ID, source public key, envelope hash, and signature. The source signature is verified against the trusted historical signer registry before an envelope enters the outbox or is replayed.

The ordering model is deliberately explicit. Local ordering remains per audit stream and sequence; wall-clock timestamps are not used to infer global order. A remote service may issue an optional order token, but the signed acknowledgement binds that token to the exact envelope hash, source sequence, and decision. A predecessor gap is represented by `AwaitingPredecessor` and remains durable rather than being silently discarded.

## Durable outbox behavior

`DurableRemoteAuditSink` stores each envelope in a create-new JSON file keyed by its envelope hash. Identical enqueue is idempotent. A same-stream/same-sequence envelope with a different hash is rejected as a collision. Pending entries are bounded and replayed deterministically by stream, source sequence, and envelope hash.

`RemoteAuditAcknowledgement` is signed by a trusted remote sink signer and binds cluster, sink ID, envelope hash, source sequence, decision, next expected sequence, and optional order token. `Accepted` and `AlreadyAccepted` remove the outbox entry and synchronize the containing directory. `AwaitingPredecessor`, `RetryableFailure`, and `Rejected` retain the entry. Unknown, forged, mismatched, or malformed acknowledgements fail closed without mutating the outbox.

## Phase 17 feature matrix

| Feature | Implementation | Safety result |
|---|---|---|
| Source authenticity | Ed25519 envelope signature and historical signer authorization | Forged or rebound source identities fail closed |
| Stream ordering | Deterministic pending replay by stream and sequence | Global order is not inferred from timestamps |
| Idempotency | Create-new hash-keyed files with identical-byte replay acceptance | Retries do not duplicate accepted bytes |
| Collision protection | Same stream and sequence cannot map to a different envelope hash | Conflicting history is rejected |
| Gap handling | Signed `AwaitingPredecessor` acknowledgement retains pending data | Missing predecessors are not silently skipped |
| Sink authenticity | Signed remote acknowledgement and trusted sink registry | Forged sink decisions fail closed |
| Durable removal | Accepted ack removes entry and synchronizes directory | Accepted state survives normal restart boundaries |
| Deployment boundary | No sockets, background workers, or sink quorum in local core | Transport and remote service remain explicit responsibilities |

## Validation

The Phase 17 integration suite passes six tests covering envelope signature and cluster binding, idempotent enqueue, deterministic stream-order replay, same-sequence collision rejection, predecessor-gap retention, accepted-entry removal, acknowledgement binding, untrusted sink signatures, and retry retention. The complete compliance validator now reports **26 passed gates**, including `remote_audit_ordering` and `remote_audit_outbox_durability`.

## Production boundaries

Production still requires mTLS, remote sink quorum and order-token allocation, cross-node packet loss and reordering tests, retry scheduling, metrics, key custody, and a durable service-side database. The local file-backed outbox proves the integrity and failure semantics of the handoff boundary; it does not claim remote availability or global ordering capacity.
