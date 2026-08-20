# Phase 48 typed UEG contract plan

## Objective

Make the UEG annotation and parser boundary deterministic and testable without yet expanding the representation into a full multi-function typed AST. The phase establishes a canonical annotation contract, fail-closed no-function behavior, and function-boundary preservation while keeping existing CLI golden output stable.

## Implemented contract

`types::normalize_annotation` recursively canonicalizes primitive names, tuple-like forms, and nested generic parameters. The function is tested for idempotence. Unsupported and malformed forms remain stable rather than being guessed into a different type. `walker::python_to_ueg` now routes typed parameter and return annotations through this shared normalizer, returns an empty invalid UEG when no function is present, and excludes the next top-level function from the current function’s preserved original body. The binary crate declares the shared `types` module so the CLI and library compile against one annotation implementation.

## Test matrix

| Contract | Coverage |
|---|---|
| Primitive annotations | Empty, `int`, `float`, `str`, `bool`, `None`, already-canonical forms |
| Nested annotations | `Vec`, `List`, `HashMap`, `Map`, `Dict`, nested generic arguments, tuple-like forms |
| Stability | Unsupported custom forms, malformed delimiters, repeated normalization |
| Parser identity | Function name, canonical parameter and return annotations, deterministic metadata fragment |
| Source preservation | Decorators, comments, original header/body, exclusion of the next top-level function |
| Lowered subset | `if`/early return, `print`, and preserved body behavior |
| Invalid input | Source without a function produces an empty invalid UEG |
| Compatibility | Existing Python round-trip, Go/Zig scaffold, emitted-source, and CLI golden tests |

## Commit boundaries

The five formatter-normalized UEG helper changes are already published separately as `f403552d2ee53e7ba7e7ebddf0ce1f6b1b641cb6` with message `chore(ueg): rustfmt translation helpers`. The Phase 48 semantic parser and test changes must be published in a later source commit. No Phase 47 compliance gate count changes are required because this phase adds language-contract tests rather than security or deployment controls.

## Remaining boundary

`ast_fragment` remains a legacy JSON-like string and the statement lowering path remains a bounded heuristic subset. The next semantic phase should introduce typed statement nodes, source spans, structured diagnostics, and multi-function UEG storage before wiring richer Go/Zig semantics or CLI target routing.
