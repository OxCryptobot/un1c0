# UN1C⓪ Agentic System Diagnostic Report

**Author:** Manus AI  
**Repository:** `OxCryptobot/un1c0` (resolved upstream remote: `Un1c0-AI/un1c0`)  
**Assessment date:** 2026-08-16  
**Assessment type:** Code-grounded architecture and implementation diagnostic

## Executive assessment

UN1C⓪ has a promising language-neutral representation and a working Rust translation CLI, but the repository is not yet an agentic language system. Its current center of gravity is source-to-source translation through a small UEG model; it has no durable agent loop, provider boundary, capability-scoped tool runtime, workspace abstraction, event journal, checkpoint/resume protocol, or measurable self-improvement loop. The existing documentation also overstates readiness: the code contains explicit stubs, regex-based walkers, a Rust-to-Python placeholder, and a Python test command that previously found no tests.

The highest-leverage move is therefore **not** adding more translation cells first. It is adding a trustworthy execution kernel around UEG. The delivered patch establishes that foundation: typed dependency-checked plans, provider-neutral planner contracts, explicit capability manifests, default-deny policy, root-contained workspaces, atomic writes, bounded output, timeout-bounded tool execution, append-only JSONL events with monotonic sequence numbers, deterministic memory retrieval, and hashed evolution proposals that remain unapproved until explicitly accepted. This is a foundation for autonomy, not a claim that unrestricted self-learning is already safe or complete.

## Baseline and evidence

| Area | Observed baseline | Significance |
|---|---|---|
| Core product | Rust CLI plus Python UEG package | The repository is a compiler/translator foundation, not yet an agent runtime |
| Rust tests | 20 tests passed after the patch, including 7 new kernel tests | Translation compatibility was preserved while adding the kernel |
| Python tests | 3 tests now pass; before the patch, `pytest -q` exited with code 5 because no tests were collected | The documented Python validation path was incomplete |
| Compiler warnings | Six existing warnings for unused Go walker code | The current translation surface still contains unfinished paths |
| Agent capabilities | No event-sourced run model, policy engine, workspace boundary, or memory/evolution contract before this patch | These are prerequisites for safe autonomous behavior |
| Security posture | CLI uses direct filesystem reads and several `expect`/`unwrap` calls; arbitrary shell/network tool integration is absent | The absence of tools reduces capability but does not constitute a complete safety model |

## Code-grounded findings

| Priority | Finding | Severity | Root cause | Failure scenario | Corrective direction |
|---:|---|---|---|---|---|
| 1 | No agent execution kernel | Critical | The architecture stops at translation and lowering | A model can generate a plan, but there is no typed state machine to validate, execute, resume, or audit it | Introduce the plan, policy, workspace, tool, and event contracts delivered in `src/agentic.rs` |
| 2 | Tool authority is not modeled | Critical | Existing code has no capability manifest or consent boundary | A future MCP/API tool could receive more authority than the user intended | Require every action to declare the exact capabilities its tool requires; deny undeclared or unapproved capabilities |
| 3 | No durable execution evidence | High | No journal or checkpoint abstraction exists | A crash loses the causal history of edits and tool calls, making replay and diagnosis impossible | Append immutable JSONL `RunEvent` records and recover sequence state from the journal |
| 4 | Workspace access is ambient | High | The translator accepts an arbitrary input path and the code has no scoped workspace abstraction | A future file tool follows `../` or a symlink outside the project | Canonicalize and contain all reads/writes under a workspace root; reject traversal and symlink escapes |
| 5 | Self-evolution is undefined and unsafe by omission | High | The repository claims autonomy but has no proposal, test, approval, or provenance contract | An agent could silently change its own tools or prompts and regress behavior without evidence | Represent evolution as hashed proposals with changed files, test command, risk, and explicit approval |
| 6 | Planner/provider boundary is absent | High | No model-neutral planner interface exists | Provider-specific prompt logic becomes coupled to execution and makes fallback/evaluation difficult | Add a `Planner` trait whose output is only a validated `Plan`; keep runtime authority below the model boundary |
| 7 | Entropy gate semantics were inconsistent | High | Python divided by an observed-alphabet value and used an unreachable `> 1.05` threshold; Rust used `> 0.92` | The Python gate could fail to reject the inputs it claimed to detect and mis-score repetitive text | Normalize entropy to `0.0..1.0`, use the shared `0.92` policy threshold, and describe the score as a risk signal rather than proof |
| 8 | Proof validation is not yet proof-carrying execution | High | `ueg/core.py` creates tautological Z3 constraints and includes `mock-proof` in the sample path | A report can say “proof validated” without checking a source-specific proof obligation | Replace sample proofs with traceable obligations, solver inputs, proof hashes, and negative tests |
| 9 | Translation fidelity is narrow despite broad matrix claims | High | Several walkers are scaffolds or regex-based and the README claims full matrix coverage | Complex syntax, effects, types, and control flow can silently lower to comments or invalid code | Add parser-completeness metrics, per-language golden cases, compiler validation, and explicit unsupported-node diagnostics |
| 10 | Error handling is panic-oriented | Medium | CLI code uses `expect` and `unwrap` at file, parser, and proof boundaries | A malformed file or missing dependency terminates with poor diagnostics instead of structured recovery | Introduce typed CLI errors, stable exit codes, contextual diagnostics, and recovery tests |

## What was implemented

The patch creates a practical foundation instead of adding another speculative prompt layer.

| Delivered component | Implementation | Result |
|---|---|---|
| Typed plans | `Plan` and `Action` with JSON serialization, dependency validation, cycle rejection, step/output budgets | Model output is data that can be rejected before execution |
| Tool contracts | `ToolSpec`, `Tool`, `ToolRegistry`, and a built-in manifest | Tools expose capabilities and input schemas without receiving authority implicitly |
| Policy | Restricted and developer policies with explicit approval for writes | Read-only is the default; writes are guarded per action |
| Workspace | Canonical root, traversal rejection, symlink rejection, bounded reads, atomic writes, bounded file listing | File operations are scoped and recoverable |
| Runtime | Dependency-aware execution, blocked descendants, timeout-bounded tool calls, structured results | Partial failure is explicit rather than silently propagated |
| Journal | Append-only JSONL events with recovered monotonic sequence numbers | Runs can be inspected, tailed, and reconstructed after a crash |
| Planner boundary | `Planner` trait plus deterministic starter planner | Future LLM providers can propose plans without controlling execution directly |
| Memory | Bounded session entries with importance, TTL, deterministic retrieval | Memory is explicit and bounded rather than unbounded prompt stuffing |
| Controlled evolution | `EvolutionProposal` with content hash, changed files, test command, risk, and approval flag | “Self-evolution” is observable and gated rather than silent mutation |
| CLI | New `un1c0-agent` binary with `plan`, `run`, and `tools` commands | The kernel is executable and easy to integrate into later provider adapters |
| Evaluation | Rust kernel tests, Python entropy tests, and a checked-in JSON plan example | The foundation is exercised rather than documented only |

## Architecture gaps that remain

The delivered kernel should be considered **Phase 1 of a larger system**. It does not yet provide the model adapter, repository index, AST/symbol graph, compiler/test verifier, parallel subagents, MCP transport, browser/API connectors, or a production sandbox for arbitrary processes. Those components must be added behind the existing contracts.

OpenHands’ public SDK architecture is a useful reference because it separates core agent behavior, tools, workspaces, and agent-server deployment while keeping local and sandboxed modes swappable.[1] MCP’s specification is equally important for future integrations: it treats tools as arbitrary code execution and calls for explicit consent, authorization, privacy controls, and validation.[2] UN1C⓪ should adopt those principles without copying a provider’s product surface.

> “Tools represent arbitrary code execution and must be treated with appropriate caution.” — Model Context Protocol specification [2]

## Prioritized improvement backlog

### Immediate next ten improvements

| Rank | Improvement | Expected benefit | Difficulty |
|---:|---|---|---|
| 1 | Add a provider adapter that converts structured model output into `Plan` JSON only | Enables real LLM planning without bypassing runtime policy | Medium |
| 2 | Add a verifier tool for `cargo test`, language compiler checks, and golden-output comparison | Turns “generated” into “verified” | Medium |
| 3 | Replace panic-oriented CLI paths with typed errors and stable diagnostics | Improves reliability and automation | Medium |
| 4 | Add repository indexing for files, symbols, imports, and UEG fragments | Reduces context waste and enables targeted retrieval | High |
| 5 | Add persisted checkpoint/resume semantics keyed by run and action IDs | Makes long-running work recoverable | Medium |
| 6 | Add diff-only edit tools with preconditions and post-write hashes | Prevents blind overwrites and supports rollback | Medium |
| 7 | Add structured reflection events and failure taxonomy | Makes repair loops measurable | Medium |
| 8 | Add isolated subagent workspaces and explicit merge gates | Enables parallel work without shared-state corruption | High |
| 9 | Add MCP/skill adapters with consent prompts and capability manifests | Expands the tool ecosystem without ambient trust | High |
| 10 | Add benchmark fixtures for planning, context selection, editing, security, latency, and token use | Prevents “best in class” from becoming an unverifiable slogan | High |

### Capability gaps versus a modern coding agent

The following capabilities are absent or incomplete and should be treated as product work, not prompt polish: repository-wide symbol indexing; incremental context retrieval; compiler/test feedback loops; patch preconditions; automatic rollback; model fallback and rate-limit handling; streaming events; session resume; subagent scheduling; merge conflict resolution; MCP consent UI; secret redaction; process sandboxing; network egress policy; prompt-injection detection; trace/span export; cost and token telemetry; model-quality regression tests; and signed evolution artifacts.

## Seven-phase roadmap

| Phase | Scope | Exit criteria | Dependencies | Estimate |
|---|---|---|---|---:|
| 1. Safety and correctness | Typed plans, policy, scoped workspace, event journal, bounded execution | All action transitions are validated, logged, and tested | None | 1–2 weeks |
| 2. Provider integration | Structured-output model adapter, retries, streaming, fallback routing | Providers can propose plans but cannot bypass runtime gates | Phase 1 | 2–3 weeks |
| 3. Repository intelligence | Incremental file/symbol/import/UEG index and retrieval | Context selection is measurable and reproducible | Phase 1 | 3–5 weeks |
| 4. Verification loop | Build/test/lint/diff verification, failure taxonomy, repair plans | Generated changes are accepted only with evidence | Phases 1–3 | 3–4 weeks |
| 5. Autonomy and subagents | Isolated workspaces, parallel plans, merge/checkpoint protocol | Parallel work is deterministic, bounded, and recoverable | Phases 1–4 | 4–6 weeks |
| 6. Ecosystem connectors | MCP, skills, APIs, browser/web connectors, consent UX | Every connector has a manifest, policy, and audit trail | Phases 1–5 | 4–6 weeks |
| 7. Controlled evolution | Signed proposals, sandboxed evaluation, canary rollout, rollback | The system can improve only through tested, reviewable artifacts | Phases 1–6 | 4–8 weeks |

## Competitive positioning

UN1C⓪ should not claim parity with Claude Code, Cursor, Codex CLI, Kimi Code, Aider, or OpenHands based on translation-matrix breadth alone. The relevant competitive unit is a **reliable work loop**: understand the repository, plan, edit, run verification, recover from failure, explain evidence, and resume after interruption. The current patch moves UN1C⓪ toward that unit by adding the execution kernel. The next differentiation opportunity is to make UEG the verifiable intermediate representation for agent plans and code changes, with proof/effect metadata attached to the same durable event stream.

## Final recommendation

Keep the UEG translator, but stop treating translation-cell count as the main progress metric. Make every future feature implement one of the kernel contracts, add a benchmark and failure case, and require an evidence-bearing event trail. The phrase “self-learning and self-evolving” should mean **bounded memory plus validated evolution proposals with rollback**, not unconstrained self-modification. That interpretation is both more defensible and more likely to produce a system that can outperform less disciplined agents in real repositories.

## References

[1]: https://docs.openhands.dev/sdk/arch/overview "OpenHands Software Agent SDK: Architecture Overview"
[2]: https://modelcontextprotocol.io/specification/2026-07-28 "Model Context Protocol Specification: Security and Trust & Safety"
