# Phase 22 Baseline Audit Notes

## Observed facts

The current `ConsensusNode` keeps `current_term` and `voted_for` only in memory. Election start increments the term and votes for the local node; higher-term requests and responses update the term, clear the vote, and step down. A granted same-term vote records the candidate ID. Snapshot installation and higher-term replication paths also mutate the term and vote state.

`AuthenticatedConsensusEnvelope` binds cluster ID, sender ID, term, nonce, message, public key, and Ed25519 signature. `ReplayWindow` validates the envelope and stores a bounded insertion sequence keyed only by nonce. Once the map exceeds its configured size, the oldest insertion sequence is removed. The window has no durable epoch, term binding, restart recovery, or explicit reset API.

`AuthenticatedSocketTransport` creates one replay window per trusted sender and consumes envelopes through the receive path. It currently prevents duplicate nonces while the process remains alive, but a restart reconstructs empty replay windows. Envelope terms are validated against their message but are not tied to a persisted receiver epoch or a durable replay frontier.

`DurableSnapshotStore` and `DurableCompactionStore` already demonstrate validated JSON, bounded files, temporary files, fsync, atomic rename, cleanup, and recovery patterns. Those patterns can be reused for durable consensus metadata without moving network or scheduling authority into the consensus core.

## Risks

An unclean process restart can lose the latest term and vote, allowing a node to reuse a term or vote for multiple candidates in the same term. A replayed authenticated envelope can be accepted after restart because the in-memory nonce window is empty. A durable replay epoch must therefore be advanced atomically with the receiver’s replay state and bound into the acceptance decision.

## Phase 22 design direction

Add a validated `DurableConsensusState` containing cluster ID, node ID, current term, optional voted-for identity, replay epoch, and a state hash. Add an atomic file-backed store with load, save, and staging recovery. Add an `EpochBoundReplayWindow` that binds cluster, sender, receiver epoch, term floor, and nonce window state; reject stale epochs and stale envelope terms before recording a nonce. The transport should expose explicit persistence/epoch controls while retaining socket I/O ownership outside the consensus state machine.

## Validation requirements

Tests must cover initial state creation, atomic save/load, malformed and mismatched state rejection, partial staging cleanup, persistence failure without in-memory mutation, monotonic term advancement, same-term vote exclusivity, higher-term reset, epoch mismatch rejection, stale-term replay rejection, duplicate nonce rejection, bounded eviction, and restart-safe replay behavior.
