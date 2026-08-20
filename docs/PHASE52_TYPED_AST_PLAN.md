# Phase 52: Typed expression nodes and source-span diagnostics

## Objective

Complete the next UEG semantic boundary after Phase 51 by replacing the legacy JSON-like `ast_fragment` string with a typed, serializable function AST. The implementation must preserve source order, retain exact source spans, reject unsupported expressions explicitly, and expose enough structure for target-specific emitter selection without claiming production compiler completeness.

## Contract

`AstFragment` contains the canonical function name, typed parameters, canonical return annotation, typed statements, and the function source span. `TypedExpression` contains an `ExpressionKind`, the trimmed source slice, and a source span. The initial expression matrix includes identifiers, integer/float/string/boolean literals, unary operators, arithmetic/comparison/boolean binary operators, calls, tuples, and explicit unsupported nodes.

All spans use byte offsets into the original UTF-8 source and character-count columns. Nested expressions retain child spans. Serialization uses the crate's existing `serde` contract and is verified with a JSON round trip.

## Fail-closed behavior

Unsupported expression forms produce `UEG-UNSUPPORTED-EXPRESSION` error diagnostics at their exact expression span. Statement-level unsupported syntax remains `UEG-UNSUPPORTED-STATEMENT`. Errors aggregate at both the lambda and UEG roots and cause `Ueg::validate` to fail. The existing Python and incremental target entrypoints reject invalid UEG before invoking emitters.

## Emitter integration

Incremental code-generation chunks expose target-neutral `EmitterHints`: function source span, expression-node count, call-site count, and control-flow count. Hints are derived from the typed AST, not from generated source strings. They are observability and target-selection metadata only; the current Rust/Go/Zig/Python emitters remain bounded heuristic/source-to-source emitters.

## Verification matrix

| Area | Evidence |
|---|---|
| Nested expression shape | `phase52_typed_expression_integration.rs` checks boolean/comparison/arithmetic/call nodes |
| Child spans | Exact source slicing for conditions, calls, range ends, and unsupported expressions |
| Serialization | `serde_json` serialize/deserialize equality for `AstFragment` |
| Range and tuple forms | Typed child expressions preserve source order and offsets |
| Fail-closed diagnostics | Unsupported subscript yields one `UEG-UNSUPPORTED-EXPRESSION` error and blocks generation |
| Emitter hints | Incremental chunk reports deterministic span, call, control-flow, and node counts |
| Regression | Phase 48/49 UEG and Phase 50 codegen suites remain green |

## Explicit boundaries

This phase does not provide complete Python semantics, static type checking, borrow checking, runtime equivalence, optimizer proof, or production compiler output. Those capabilities require separate typed contracts and independent verification phases. Compliance metadata remains at 209 gates because this phase changes the language contract but does not add authority, network, secret, or deployment behavior.
