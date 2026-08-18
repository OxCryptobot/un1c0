# UN1C⓪ Agent Kernel

## Purpose

UN1C⓪ is evolving from a source-to-source translator into a **local-first programmable agent runtime**. The Universal Executable Graph (UEG) remains the system's language-neutral intermediate representation, while the agent kernel becomes the control plane that plans work, invokes tools, records evidence, verifies results, and proposes controlled extensions.

The initial kernel deliberately favors **determinism, inspectability, and safe failure** over claims of unrestricted autonomy. An agent may only perform operations represented by typed actions, and every operation produces durable events that can be replayed or inspected.

## Architecture

```text
Goal / Program
      |
      v
+-------------------+      +-------------------+
| Planner boundary  | ---> | Typed Plan        |
| provider-neutral  |      | dependencies      |
+-------------------+      +---------+---------+
                                      |
                                      v
+-------------------+      +-------------------+
| Policy engine     | ---> | Tool registry     |
| capabilities      |      | typed contracts   |
| approvals         |      | deterministic IDs |
+-------------------+      +---------+---------+
                                      |
                                      v
+-------------------+      +-------------------+
| Workspace         | ---> | Execution result  |
| scoped filesystem |      | stdout/stderr     |
| no ambient trust  |      | exit/status       |
+-------------------+      +---------+---------+
                                      |
                                      v
+------------------------------------------------+
| Event journal + checkpoints + memory + metrics |
+------------------------------------------------+
                                      |
                                      v
                         Evolution proposals only
                         after validation and approval
```

## Core contracts

| Contract | Responsibility | Safety property |
|---|---|---|
| `Plan` | Versioned DAG of actions with dependencies | No implicit action ordering; cycle rejection |
| `ToolSpec` | Name, description, input schema, capabilities, timeout | Tool metadata is data, not authority |
| `Policy` | Allow/deny/approval decision for every action | Writes and process execution require explicit capability |
| `Workspace` | Root-scoped file access and command execution | Canonical path containment and bounded output |
| `EventJournal` | Append-only run history | Replayable evidence and crash recovery |
| `MemoryStore` | Bounded session facts with importance and TTL | No unbounded prompt or state growth |
| `EvolutionProposal` | Candidate tool/skill change with hash and tests | Self-evolution is proposed, validated, and approved—not silently applied |
| `ConsentManifest` / `ConsentStore` | Versioned MCP, skill, API, web, and LSP grants | External capabilities fail closed until registered, scoped, and approved |
| `SignedEvolutionProposal` / `EvolutionLedger` | Ed25519-signed proposal lifecycle and durable state | Canary failure rolls back; persistence is atomic and auditable |

## Execution semantics

1. A plan is validated before execution. Validation rejects duplicate IDs, missing dependencies, dependency cycles, empty tool names, and unsupported capabilities.
2. The runtime topologically orders the plan. It executes only actions whose dependencies have succeeded. A failed dependency causes descendants to be skipped rather than run with partial state.
3. Every transition is journaled as an immutable event. The journal is JSON Lines so it can be tailed, copied, diffed, and recovered without a database.
4. The runtime enforces step, output, and wall-clock budgets. A later provider integration can use the same runtime without changing execution semantics.
5. File operations are root-scoped. `read_file` and `write_file` reject traversal, absolute paths, and symlink escapes. `write_file` is atomic.
6. Shell execution is not enabled by default. It requires the `process.exec` capability and an explicit policy approval. The first implementation provides a controlled command runner for allowlisted commands only.
7. Memory is explicit and bounded. Entries have a scope, importance, creation time, and optional TTL. Retrieval is deterministic and records the reason for selection.
8. Evolution is a first-class record. A proposal includes a content hash, changed files, test command, and risk class. The runtime can validate and journal a proposal but cannot auto-apply it without explicit approval.
9. External adapters require a registered `ConsentManifest`. The manifest binds an integration kind, allowed tool names, exact capabilities, payload limits, optional network hosts, and whether approval is mandatory. Revocation removes future authority immediately.
10. Controlled evolution requires an Ed25519 signature over the immutable proposal content and signer identity. The `EvolutionLedger` permits only `draft -> approved -> canary -> applied|rolled_back` transitions and records evidence for every terminal state.

## Provider and planner boundary

The kernel does not embed a particular model provider. A future provider adapter should map a model response into the same `Plan` schema and be treated as an untrusted planner. The runtime, policy engine, workspace, and verification gates remain authoritative. This prevents prompt output from becoming an execution primitive.

## Roadmap

| Phase | Outcome | Dependency |
|---|---|---|
| 1 | Typed plan, policy, workspace, event journal, deterministic runtime | None |
| 2 | Provider adapters and streaming event transport | Phase 1 |
| 3 | Repository index, symbol graph, UEG-aware context retrieval | Phase 1 |
| 4 | Verification loop: tests, diffs, compiler diagnostics, repair plans | Phases 1–3 |
| 5 | Parallel subagents with isolated workspaces and merge gates | Phases 1–4 |
| 6 | MCP/skill adapters with explicit consent and capability manifests | Phases 1–5; implemented in `integration.rs` |
| 7 | Controlled evolution service with signed proposals and regression evaluation | Phases 1–6; lifecycle ledger implemented in `evolution.rs` |
| 8 | Bounded distributed consensus, quorum commit, and deterministic replicated state snapshots | Phases 1–7; transport-agnostic core implemented in `consensus.rs` |
| 9 | Zero-trust service mesh identity/method authorization and cryptographic audit evidence | Phases 1–8; mesh/audit core implemented in `security.rs` and optional Istio resources in Helm |
| 10 | Durable snapshots, authenticated consensus envelopes, signer rotation/revocation, and durable external audit sink | Phases 1–9; implemented in `consensus.rs`, `security.rs`, and Phase 10 integration tests |
| 11 | Production-grade joint-consensus membership changes, dynamic re-voting, crash recovery, and partition evidence | Phases 1–10; implemented in `consensus.rs`, Phase 11 integration tests, and partition benchmark |
| 12 | Authenticated socket transport, cluster configuration IDs, replay windows, and power-loss snapshot recovery | Phases 1–11; implemented in `consensus.rs`, Phase 12 transport tests, and failure-injection tests |
| 13 | Distributed snapshot chunking, streaming transfer, incremental state synchronization, concurrent follower catch-up, and packet-corruption stress testing | Phases 1–12; implemented in `consensus.rs`, Phase 13 snapshot/transport tests, and the expanded compliance gate |
| 14 | Leader-lease read-index optimization, conservative clock-drift safety, and linearizable client read queries | Phases 1–13; implemented in `consensus.rs`, Phase 14 linearizable-read tests, and lease-versus-quorum benchmarks |
| 15 | Bounded election timers, deterministic per-node jitter, heartbeat scheduling, peer failure suspicion, and clock-safe timer actions | Phases 1–14; implemented in `consensus.rs` and Phase 15 election-timer integration tests |
| 16 | Bounded replication batches, per-peer flow control, backpressure, retry scheduling, and typed batch acknowledgements | Phases 1–15; implemented in `consensus.rs`, Phase 16 flow-control tests, and the replication design report |
| 17 | Signed remote-audit envelopes, per-stream ordering, idempotent sink acknowledgements, and durable outbox replay | Phases 1–16; implemented in `security.rs`, Phase 17 remote-audit tests, and the remote-ordering design report |
| 18 | Bounded committed-prefix log compaction, compacted-frontier translation, configuration-bound snapshots, and snapshot-required follower catch-up | Phases 1–17; implemented in `consensus.rs`, Phase 18 compaction tests, and the compaction design report |
| 19 | Durable compaction manifests, staged snapshot cutover, deterministic startup recovery, and prior-snapshot preservation | Phases 1–18; implemented in `consensus.rs`, Phase 19 durable-recovery tests, and the recovery design report |
| 20 | Replicated snapshot acknowledgements, install-readiness lifecycle, one-transfer-per-follower backpressure, and installed-only progress advancement | Phases 1–19; implemented in `consensus.rs`, Phase 20 snapshot-readiness tests, and the install-readiness design report |

## Non-goals of the first implementation

The first kernel does not claim to provide unrestricted self-learning, autonomous code modification, or safe execution of arbitrary shell commands. Those capabilities require stronger sandboxing, identity, resource isolation, provenance, and evaluation infrastructure. The kernel instead establishes the contracts that make later autonomy safe and measurable.

## Batch implementation extensions

The kernel now includes three additional production seams. `RepositoryIndex` performs deterministic, bounded local indexing of supported source and documentation files, extracts lightweight symbols, hashes files, skips ignored directories and symlinks, and converts ranked matches into bounded provider context items. `CheckpointStore` persists plan-hashed successful action results atomically; `Runtime` can resume from a matching checkpoint, skip completed actions, journal checkpoint saves, and clear the checkpoint only after successful completion. `SubagentCoordinator` schedules isolated workspaces with a bounded parallelism limit, while `MergeGate` requires successful verification and rejects overlapping changed files before integration.

The current batch adds consent-scoped integration adapters for MCP, skills, APIs, web access, and LSP tooling. `ConsentScopedTool` is the common execution boundary; aliases provide protocol-specific naming without weakening the shared manifest, payload, revocation, or approval checks. It also adds `SignedEvolutionProposal` and `EvolutionLedger`, which bind Ed25519 signatures to the proposal hash and enforce approval, canary, apply, and rollback transitions through an atomic JSON ledger. `TrustedSignerStore` now separates cryptographic authentication from authorization by binding signer IDs to explicitly trusted public keys and rejecting unknown, mismatched, or implicitly rebound keys before admission or persisted-record loading. `EvaluationCheck` and `CanaryReport` retain only bounded metadata and SHA-256 digests of command output, bind reports to the active canary run and exact proposal file set, and prevent unverified evidence from applying a proposal. Production report producers should use `CanaryReport::from_workspace`, which hashes bounded regular files after canonical path and symlink checks; caller-controlled textual finalization is intentionally unavailable.

The Compose smoke path now generates disposable certificates into an isolated temporary directory, passes that directory through `CERTS_DIR`, waits for the nginx mTLS endpoint explicitly, and removes the certificate bundle during cleanup. This keeps private keys out of source control and prevents stale ignored keys from invalidating integration results.

The next architecture batch adds a bounded, transport-agnostic consensus core. `ConsensusNode` validates cluster membership, tracks terms and roles, runs quorum-based elections, refuses leader proposals from followers, appends hashed state commands, requires a current-term quorum before applying entries, validates `AppendEntries` predecessor terms and conflicts, and exposes deterministic replicated-state snapshots. Key/value sizes, member count, and log length are bounded; election timers, durable log compaction, membership changes, and production failure injection remain explicit deployment-layer work.

Phase 10 adds durable JSON snapshots with hash validation and atomic replacement, snapshot install checks, and `AuthenticatedConsensusEnvelope`, which binds an Ed25519 signature to sender ID, current term, bounded nonce, message bytes, and trusted public-key identity. `AuditSignerStore` now supports atomic persistence, one-way rotation, revocation, and historical verification. `DurableFileAuditSink` persists immutable content-addressed records with `create_new`, fsync, idempotent retry, and chain verification; `AuditLog::flush_sink` provides at-least-once outbox recovery when the external sink is temporarily unavailable. Production promotion still requires a quorum-aware transport replay/anti-replay window, external key-management policy, durable snapshot backup/restore, and an approved remote audit service.

Phase 11 adds a bounded joint-consensus protocol. A `ConfigurationJoint` log entry carries both old and new member sets and requires a double majority for elections and commits. `ConfigurationFinal` is admitted only after the joint entry commits; follower application updates membership deterministically, blocks concurrent changes, and supports late-node adoption. A dedicated failure-injector aborts after partial snapshot staging so startup cleanup and atomic rewrite are exercised across a process boundary. The partition benchmark records authenticated-envelope p95/throughput, dropped traffic, and quorum availability for healthy, majority, and minority five-node components.

Phase 12 adds a real loopback TCP transport for length-prefixed `AuthenticatedConsensusEnvelope` frames. Envelopes bind a cluster configuration ID, sender ID, term, nonce, message bytes, and public key. Receivers use trusted-key lookup, bounded frame allocation, sender-local transport identity, and insertion-ordered replay windows; duplicate nonces, cluster mismatches, oversized frames, key rebinding, and impersonation fail closed. A process-abort power-loss fixture stages a partial snapshot before rename and proves cleanup plus atomic rewrite on recovery. Production still requires mTLS/mesh confidentiality and peer authentication, durable replay epochs, timeouts, backpressure, and cross-machine fault tests.

Phase 13 adds bounded `SnapshotManifest`/`SnapshotChunk` streaming with manifest, chunk, state, and metadata hashes; out-of-order assembly with identical retransmission tolerance; authenticated `StateDelta` incremental synchronization; and immutable-state concurrent follower catch-up planning. Packet stress tests run concurrent valid senders plus forged-signature and truncated frames, classifying rejected traffic without panic. Production still requires resumable disk-backed staging, per-peer flow control, cross-host packet loss/reordering tests, and per-chunk transport envelopes.

Phase 14 adds `LeaderLeaseConfig`, conservative monotonic-tick lease validity, typed `ReadIndexRequest`/`ReadIndexResponse` quorum rounds, and `LinearizableReadPlan` execution. The lease fast path is admitted only after a current-term quorum acknowledgement and only while `now_tick + max_clock_drift_ticks < expiration_tick`; term changes, step-down, membership transitions, snapshot installation, append heartbeats, clock regression, and uncertainty invalidate it. Followers acknowledge only after their local commit index reaches the requested read index, and client plans fail closed when the leader term or applied frontier no longer matches. Deterministic integration benchmarks compare lease and quorum paths at concurrency 1, 2, 4, 8, 16, and 32. Production still requires trusted clock-health signals, suspend/resume handling, cancellation, metrics, durable request retention, and automatic fallback to quorum reads when clock safety is uncertain.

Phase 15 adds a transport-agnostic `ElectionTimerConfig`, injected-tick `tick` actions, deterministic per-node/per-term election jitter, bounded leader heartbeat plans, peer heartbeat observations, and exact-boundary failure suspicion. Followers and candidates start elections only at their deadlines; leaders emit heartbeats at bounded intervals; unknown and self peer IDs fail closed. Clock regression blocks timer actions until explicit monotonic re-anchoring, which resets deadlines and clears stale peer observations. The consensus core still spawns no timers, threads, or sockets; callers own scheduling and authenticated message delivery.

Phase 16 adds `ReplicationFlowConfig`, typed `ReplicationBatch`/`ReplicationBatchAck` messages, and independent per-follower flow windows. The leader returns `Idle`, `Backpressured`, or bounded `Send` actions, admits only one batch per peer, validates serialized byte and entry bounds before mutating window state, and applies retry backoff at exact tick boundaries. Successful acknowledgements release windows and delegate quorum progress to the existing append path; failed, duplicate, mismatched, or higher-term acknowledgements fail closed, with higher terms clearing all windows and forcing step-down. Membership rebuilds prune removed peers, and clock uncertainty blocks new sends.

Phase 17 adds `RemoteAuditEnvelope`, `RemoteAuditAcknowledgement`, `RemoteAuditDecision`, and `DurableRemoteAuditSink`. Source envelopes bind cluster, source node, stream, sequence, predecessor hash, record hash, canonical bytes, signer identity, and Ed25519 signature. The outbox uses create-new files keyed by envelope hash, rejects same-stream sequence collisions, replays in deterministic stream/sequence order, retains predecessor gaps and retryable decisions, and removes accepted entries only after a signed sink acknowledgement plus directory synchronization. Global ordering is not inferred from timestamps; remote services must issue explicit order tokens bound to exact envelope hashes. Transport, sink quorum, and key custody remain deployment-layer responsibilities.

Phase 18 adds `LogCompactionConfig`, compacted logical frontiers, `ConfigurationBoundSnapshot`, and `ReplicationCatchUpAction`. Compaction admits only committed/applied targets within bounded discard and retained-suffix limits, validates all conditions before draining the prefix, and preserves the boundary term for predecessor checks. Append and incremental-delta paths translate logical indexes through the compacted frontier; followers behind it receive a snapshot-required result. Configuration-bound snapshots hash state and the active stable/joint membership metadata, and installation rejects tampered or mismatched configuration state.

Phase 19 adds `CompactionManifest`, `DurableCompactionStore`, and deterministic `CompactionRecoveryOutcome` states. Staged snapshot and manifest files are validated and fsynced before atomic rename, the manifest lifecycle is committed only after snapshot promotion, and directory synchronization completes the cutover. Startup recovery finalizes a complete matching pair, aborts malformed or partial staging, preserves the previous durable pair, and is idempotent. The local core still does not schedule compaction or own transport/storage policy.

Phase 20 adds `SnapshotInstallAck`, `SnapshotInstallReadiness`, `SnapshotTransferAction`, and `SnapshotReplicationStatus`. A leader permits one active snapshot transfer per follower, binds acknowledgements to the exact transfer, term, frontier, serialized snapshot hash, and configuration hash, and returns bounded backpressure or retry actions. `Validated` and `DurablyStaged` never advance replication progress; only `Installed` does. Rejected or stale acknowledgements fail closed, and higher terms, clock uncertainty, step-down, and membership changes clear active snapshot authority.

Phase 21 adds `SnapshotBandwidthConfig`, `SnapshotTransferMetrics`, `SnapshotTransferProgressAction`, and `SnapshotTransferCancellation`. Each follower has isolated byte counters and a rolling bounded bandwidth window. Progress admits only positive bounded chunks, never exceeds the exact snapshot byte frontier, and returns an exact retry tick when the per-follower window is exhausted. An installed acknowledgement is rejected until byte accounting reaches the complete snapshot size. Explicit cancellation clears active transfer authority, records a bounded retry deadline and cancellation counter, and preserves the existing one-transfer backpressure invariant. Metrics expose active transfer, bytes sent/remaining, window usage/limit, transfer outcomes, and cancellation counts without owning transport, storage, scheduling, or persistence.

Phase 22 adds `DurableConsensusState`, `DurableConsensusStateStore`, epoch-aware authenticated envelopes, and epoch-bound replay windows. Durable state binds cluster ID, node ID, current term, voted-for identity, replay epoch, replay term floor, and a canonical SHA-256 state hash. The file store validates bounded JSON, fsyncs before atomic rename, removes partial staging, and rejects identity or term/replay rollback on restore. Authenticated envelopes bind replay epoch into the Ed25519 payload; transport windows reject stale epochs and terms, evict only bounded oldest nonces, and clear all nonce state only through a monotonic epoch rotation. Persistence and socket delivery remain explicit caller boundaries.

Phase 23 adds `CompactionCoordinationConfig`, `CompactionFollowerStatus`, `CompactionCoordinationPlan`, `CompactionCoordinationAction`, `SnapshotRequest`, and `SnapshotRequestAction`. Leaders expose a hash-bound compaction plan with per-follower lag, stable/joint remote-quorum requirements, and an explicit waiting state that cannot mutate the log. Compaction executes only after the plan is ready. Followers can emit typed requests when an append predecessor or incremental delta base is inside their compacted prefix; requests bind follower/leader identity, term, reason, frontier knowledge, optional snapshot hash, and retry tick. Leaders validate the request and delegate to existing bounded snapshot transfer state. Network delivery, scheduling, durable request intent, and compaction authority remain caller boundaries.

Phase 24 adds `SocketQuotaConfig`, `SocketPeerQuota`, `SocketBackpressureAction`, `SocketReceiveAction`, and `SocketTransportMetrics`. Authenticated socket transport now admits serialized frame bytes against isolated per-peer in-flight send quotas and receive-window quotas, returns deterministic retry ticks under backpressure, releases send bytes after successful or failed writes, and resets quota state on monotonic replay-epoch rotation. Authentication and duplicate-replay checks precede receive quota mutation. Legacy send/receive APIs remain compatible; quota-aware callers use the explicit backpressure methods. Socket thread ownership, scheduling, queue persistence, and cross-process quota replication remain deployment boundaries.

Phase 25 adds `DurableSocketQueueFrame`, `DurableSocketQueueState`, and `DurableSocketQueueStore`. Queue frames are hash-bound, FIFO ordered, identity/epoch bound, and bounded by per-peer and global byte/entry limits. Enqueue persists the queue and quota state with create-new staging, fsync, atomic rename, directory sync, and rollback on persistence failure. Restart restore validates state hashes, peer membership, quota/frame byte equality, and replay epoch; acknowledgements remove only the queue head. Partial staging is explicitly removed. Queue threads, delivery scheduling, durable retry execution, and cross-process replication remain deployment boundaries.

Phase 26 adds `SocketDeliveryCrashPoint` and `DurableSocketDeliveryAction`. A queued envelope is deserialized and re-verified against the trusted local identity, replay epoch, and term floor before delivery. One active delivery per peer is allowed. The queue is retained through injected crashes before the length prefix, after the prefix, after payload write, and after flush; only a fully flushed frame can be acknowledged and removed. Restart retries the retained frame, while tampered queued payloads fail closed before any socket write. Delivery counters remain non-secret and socket thread/process ownership remains a deployment boundary.

The current security batch adds a transport-agnostic `ZeroTrustMesh` policy engine and `AuditLog`. Mesh requests must bind a trust-domain identity, audience, certificate SHA-256 fingerprint, source-to-destination relation, and method allowlist; decisions can be recorded as bounded metadata digests in an Ed25519-signed, SHA-256 hash-chained append-only log. Helm optionally emits strict Istio `PeerAuthentication`, explicit `AuthorizationPolicy` allowlists, per-component service accounts, sidecar injection labels, and control-plane egress rules. Phase 10 closes the local signer lifecycle and sink-recovery gaps; server-side mesh rollout, remote audit service durability, and operational key custody remain promotion gates.
