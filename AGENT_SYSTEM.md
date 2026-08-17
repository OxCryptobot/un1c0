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

## Non-goals of the first implementation

The first kernel does not claim to provide unrestricted self-learning, autonomous code modification, or safe execution of arbitrary shell commands. Those capabilities require stronger sandboxing, identity, resource isolation, provenance, and evaluation infrastructure. The kernel instead establishes the contracts that make later autonomy safe and measurable.

## Batch implementation extensions

The kernel now includes three additional production seams. `RepositoryIndex` performs deterministic, bounded local indexing of supported source and documentation files, extracts lightweight symbols, hashes files, skips ignored directories and symlinks, and converts ranked matches into bounded provider context items. `CheckpointStore` persists plan-hashed successful action results atomically; `Runtime` can resume from a matching checkpoint, skip completed actions, journal checkpoint saves, and clear the checkpoint only after successful completion. `SubagentCoordinator` schedules isolated workspaces with a bounded parallelism limit, while `MergeGate` requires successful verification and rejects overlapping changed files before integration.

The current batch adds consent-scoped integration adapters for MCP, skills, APIs, web access, and LSP tooling. `ConsentScopedTool` is the common execution boundary; aliases provide protocol-specific naming without weakening the shared manifest, payload, revocation, or approval checks. It also adds `SignedEvolutionProposal` and `EvolutionLedger`, which bind Ed25519 signatures to the proposal hash and enforce approval, canary, apply, and rollback transitions through an atomic JSON ledger. `TrustedSignerStore` now separates cryptographic authentication from authorization by binding signer IDs to explicitly trusted public keys and rejecting unknown, mismatched, or implicitly rebound keys before admission or persisted-record loading. `EvaluationCheck` and `CanaryReport` retain only bounded metadata and SHA-256 digests of command output, bind reports to the active canary run and exact proposal file set, and prevent unverified evidence from applying a proposal. Production report producers should use `CanaryReport::from_workspace`, which hashes bounded regular files after canonical path and symlink checks; caller-controlled textual finalization is intentionally unavailable.

The Compose smoke path now generates disposable certificates into an isolated temporary directory, passes that directory through `CERTS_DIR`, waits for the nginx mTLS endpoint explicitly, and removes the certificate bundle during cleanup. This keeps private keys out of source control and prevents stale ignored keys from invalidating integration results.

The next architecture batch adds a bounded, transport-agnostic consensus core. `ConsensusNode` validates cluster membership, tracks terms and roles, runs quorum-based elections, refuses leader proposals from followers, appends hashed state commands, requires a current-term quorum before applying entries, validates `AppendEntries` predecessor terms and conflicts, and exposes deterministic replicated-state snapshots. Key/value sizes, member count, and log length are bounded; transport authentication, election timers, durable log compaction, membership changes, and production failure injection remain explicit deployment-layer work.
