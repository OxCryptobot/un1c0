# Audit notes

## Repository baseline

- The repository is a Rust CLI plus a small Python UEG package. Its documented mission is source-to-source language translation through a seven-node UEG, not an agent runtime.
- `cargo test --all-features` passes 14 Rust tests on stable Rust 1.97.1, but emits six warnings for unused Go walker code. `pytest -q` exits with code 5 because the repository contains no pytest tests; Python dependencies were absent until installed locally.
- The CLI currently uses `expect`/`unwrap`, reads arbitrary input paths directly, and hard-codes a simplistic entropy gate. It has a Rust-to-Python stub and several target/language paths that are explicitly scaffolds or regex-based.
- `ueg/core.py` imports Z3 and BLAKE3, but its validation constructs tautological constraints, accepts `entropy_cert` without checking it, and `fib_ueg()` embeds `mock-proof`; the lowerer is a stub.
- The repository contains strong claims of 100% translation coverage and production readiness that are not supported by the current test surface or implementation breadth.

## External reference findings

- OpenHands' current SDK documentation describes a production architecture with separate core SDK, tools, workspace, and agent-server packages. The SDK centers on typed agents, conversations, LLM/provider interfaces, tools, events, workspaces, skills, context condensation, security, persistence, metrics, and tracing. It supports a local in-process mode and a sandboxed/remote mode with swappable workspace implementations.
- The MCP specification explicitly treats tools as arbitrary code execution and requires explicit user consent, authorization, clear tool descriptions, privacy controls, and input validation. Tool metadata must be treated as untrusted unless it comes from a trusted server.

## Implication for un1c0

The highest-leverage change is not adding more translation cells. It is introducing a typed, event-sourced agent kernel with a provider-neutral planner boundary, capability-scoped tools, workspace abstraction, durable checkpoints, observable runs, and controlled evolution proposals. The existing UEG should become one executable artifact/IR within that kernel rather than the entire product boundary.

## Sources

1. OpenHands SDK architecture: https://docs.openhands.dev/sdk/arch/overview
2. Model Context Protocol specification, Security and Trust & Safety: https://modelcontextprotocol.io/specification/2026-07-28
