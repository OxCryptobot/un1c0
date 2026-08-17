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

## Evidence

The unit tests cover quorum election, leader replication, stale terms, command limits, and log limits. The public integration test verifies election, follower append, quorum commit, follower commit notification, and identical state hashes across nodes.

## Production boundary

This slice is intentionally not a complete production consensus deployment. It still requires an authenticated transport, election timers, durable log and snapshot storage, backpressure, log compaction, membership-change protocol, metrics, network partition handling, and failure-injection testing before production promotion. Runtime policy and consent manifests remain authoritative for all tools, MCP methods, network access, and secrets; cluster membership does not grant any of those capabilities.
