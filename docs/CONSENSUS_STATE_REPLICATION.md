# Consensus and State Replication

## Design boundary

`src/consensus.rs` implements a bounded, transport-agnostic state machine. It does not open sockets, spawn election timers, or grant network/tool authority. A caller may carry its typed messages over an approved transport while the core enforces membership, term, log, quorum, and state-machine invariants.

## Core behavior

| Concern | Contract |
|---|---|
| Membership | Node IDs are bounded and control-character-free; every sender and candidate must belong to the configured cluster. |
| Election | A candidate increments its term, votes for itself, and becomes leader only after a quorum of valid votes. Higher terms force follower state. |
| Proposal | Only a leader may append a typed `Set` or `Delete` command. Keys, values, member count, batch size, and log length are bounded. |
| Integrity | Every log entry carries a SHA-256 digest of canonical serialized command content. Hash or index mismatches fail closed. |
| Commit | A leader applies only current-term entries acknowledged by a quorum. A local append alone does not commit state. |
| Replication | `AppendEntries` validates previous index/term, rejects stale terms, truncates conflicting suffixes, appends bounded entries, and advances followers only to the leader's commit index. |
| Snapshot | Replicated state snapshots include term, commit index, last-applied index, state, and a deterministic state hash for equality checks. |
| Membership change | `ConfigurationJoint` carries old and new sets, requires a double majority, blocks concurrent changes, and precedes `ConfigurationFinal`. |
| Dynamic re-voting | Elections and commit acknowledgements use the active set; joint mode requires a majority in both old and new sets. |
| Crash recovery | Snapshot staging can be explicitly recovered after a process abort before rename; invalid installs leave node state unchanged. |
| Authenticated benchmark | Deterministic Ed25519 envelope benchmark reports verified/dropped messages, p95 verification time, throughput, and quorum availability under partitions. |
| Socket transport | Length-prefixed TCP frames are bounded before allocation and carry typed authenticated envelopes. |
| Cluster/replay binding | Envelopes bind cluster ID and sender ID; receivers use trusted keys and insertion-ordered bounded replay windows. |
| Power-loss recovery | A process-abort fixture leaves a partial staging file, then explicit recovery removes it before atomic rewrite. |
| Timer and failure detection | Injected-tick `ElectionTimerAction` plans provide bounded elections, leader heartbeats, deterministic jitter, peer suspicion, and clock-safe fail-closed behavior without background threads. |
| Replication flow control | `ReplicationFlowAction` provides bounded `Send`, `Backpressured`, or `Idle` work with one in-flight batch per follower, exact retry boundaries, and independent peer windows. |
| Remote audit ordering | Signed remote-audit envelopes bind source stream/sequence and record hashes; durable outbox replay is deterministic, idempotent, and retains gaps or retryable acknowledgements. |

## Evidence

The unit tests cover quorum election, leader replication, stale terms, command limits, and log limits. The public integration test verifies election, follower append, quorum commit, follower commit notification, and identical state hashes across nodes. Phase 11 integration tests verify double-majority joint consensus, final membership adoption by existing and late nodes, dynamic re-voting, single-flight bounds, process-boundary crash recovery, invalid snapshot rollback, and the authenticated partition benchmark. Phase 15 integration tests verify bounded election deadlines, heartbeat cadence, deterministic timer actions, exact failure-detector expiry, unknown/self peer rejection, clock-regression blocking, and explicit re-anchoring. Phase 16 integration tests verify bounded batch bytes and entries, one in-flight batch per peer, independent peer windows, exact retry-boundary eligibility, successful release, failed-ack backoff, higher-term invalidation, no partial mutation on rejected batches, and clock-uncertainty blocking. Phase 17 integration tests verify source-envelope signatures, cluster binding, deterministic stream ordering, idempotent enqueue, same-sequence collision rejection, predecessor-gap retention, signed sink-ack binding, accepted-entry removal, and retry retention.

## Production boundary

This slice is intentionally not a complete production consensus deployment. It still requires mTLS or mesh confidentiality and peer authentication around the TCP frame layer, durable replay epochs across restart, durable log compaction, membership configuration backup/restore, transport-level backpressure and bandwidth quotas, durable remote sink quorum and order-token allocation, durable retry state, metrics export, real scheduler jitter, durable term/vote persistence, and cross-machine partition, failure-detector, and remote-sink testing before production promotion. The local partition benchmark measures in-process Ed25519 verification and drop filtering, not network latency or kernel behavior. Runtime policy and consent manifests remain authoritative for all tools, MCP methods, network access, and secrets; cluster membership does not grant any of those capabilities.
