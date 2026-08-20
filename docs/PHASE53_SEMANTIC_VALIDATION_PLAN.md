# Phase 53: typed semantic validation and target capability contracts

## Architectural objective

Phase 53 turns the Phase 52 typed expression tree into an explicit semantic preflight boundary. The parser remains syntax-oriented and bounded; a new validator checks symbol references, duplicate parameters, target capability requirements, and unsupported typed nodes before any target emitter runs. The validator is deterministic, side-effect-free, and returns structured diagnostics with exact source spans.

This phase does not execute programs, infer full Python types, prove runtime equivalence, or mutate the repository. It supplies the contract that future type inference, effect analysis, and compiler backends can consume safely.

## Milestones

| Milestone | Outcome | Evidence |
|---|---|---|
| 53.1 | Typed semantic diagnostic model with stable codes, severity, target, and span | Unit/integration tests for deterministic ordering and exact source slices |
| 53.2 | Function-local symbol validation for parameters, assignment targets, loop targets, and referenced identifiers | Tests for duplicate parameters, undefined references, and local definitions |
| 53.3 | Target capability profiles for Rust, Go, Zig, and Python | Matrix tests for supported expression/operator/statement features |
| 53.4 | Fail-closed code-generation preflight | Invalid target capability or semantic diagnostics prevent emitter invocation |
| 53.5 | Deterministic validation benchmark | 1/2/4/8/16/32-function fixtures, p50/p95/p99, diagnostics, and zero mutation |
| 53.6 | Reusable workflow update | Phase 53 reference and roadmap row; compliance metadata remains unchanged |

## Contract

`SemanticDiagnostic` contains a stable code, message, severity, target label, and exact `SourceSpan`. `SemanticValidationReport` contains the target, function count, expression count, diagnostic count, and ordered diagnostics. `TargetCapabilityProfile` declares supported expression kinds, operators, and statement kinds for each target binding. Profiles are data, not authority; they cannot grant filesystem, process, network, secret, or deployment capabilities.

The validator builds a per-function symbol table from parameters and assignment/loop targets. A reference is accepted when it is a parameter, a local target defined earlier in source order, or an explicitly recognized builtin. Function-call names are checked against the same function set or recognized builtins. Unknown references and duplicate parameters are errors. Unsupported typed AST nodes are errors. Diagnostics are sorted by source span and stable code before return.

## Code-generation gate

`IncrementalCodeGenerator` runs the semantic preflight after the existing UEG error gate and before `render_node`. `GenerationError::SemanticValidation` carries the target and ordered report. No emitter is invoked when semantic preflight fails. Existing cursor, sink, pooled-buffer, target-preamble, and DCE contracts remain unchanged.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Determinism | Same UEG and target produce byte-for-byte identical reports |
| Scope | Parameters and earlier assignment/loop targets resolve; later or unknown names fail |
| Calls | User-defined functions and approved builtins resolve; unknown callees fail |
| Operators | Capability profiles accept only declared operators |
| Target variation | Rust/Go/Zig/Python profiles are explicit and testable |
| Fail closed | Semantic errors block all target emitters and pooled generation |
| Safety | No filesystem, process, network, secret, or cluster mutation |
| Performance | Benchmark reports p50/p95/p99 and diagnostic counts across 1–32 functions |

## Benchmark boundary

The benchmark measures local validator overhead only. It must not be presented as compiler throughput, runtime performance, or production scalability. Phase 51 pool evidence remains separate: the pool reduces transient output-buffer allocation pressure but does not attribute CAS intent cloning, thread stacks, cryptographic verification, or filesystem state.

## Explicit non-goals

Phase 53 does not add a type checker, effect system, borrow checker, runtime interpreter, remote provider, autonomous mutation, or deployment behavior. Compliance metadata remains at 209 gates because the phase adds a read-only semantic validation contract and no new authority surface.
