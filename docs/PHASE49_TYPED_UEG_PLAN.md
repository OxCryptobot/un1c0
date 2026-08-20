# Phase 49 typed multi-function UEG plan and Phase 48 memory review

## Objective

Extend the Phase 48 single-function UEG contract into a deterministic multi-function representation with typed statement records, exact source spans, and fail-closed structured diagnostics. Preserve the existing Python-to-Rust golden path while making unsupported syntax observable and non-authorizing.

## Implemented design

Each top-level Python `def` becomes an ordered `LambdaNode`. The parser captures canonical parameter and return annotations, decorators/comments and original source lines, a function-level `SourceSpan`, typed statements, and per-function diagnostics. `Ueg` aggregates diagnostics while preserving source order. The existing target-facing fields remain for compatibility, but typed statements provide an explicit contract for the supported subset.

`StatementKind` currently covers `If`, `Return`, `Print`, `TupleAssign`, and `RangeLoop`. Every other non-empty, non-comment body statement becomes `Unsupported` with the original source text. Each unsupported statement emits an `UEG-UNSUPPORTED-STATEMENT` error diagnostic. Root validation fails when any error exists, and `python_to_rust` returns `// invalid UEG generated` rather than emitting plausible code from unsupported input.

Source spans are derived from original UTF-8 line starts. Byte offsets are sliced against the original source in tests, while line and character-column values are checked at statement boundaries. Function spans exclude the next top-level function and trailing separator blanks. Range-loop detection strips the Python header colon before parsing arguments.

## Phase 48 memory evidence review

The Phase 48 review uses the Phase 47 sustained hot-key benchmark because it is the latest published memory profile. At 32/64/96 producers, peak RSS was 6,548/8,668/10,136 KiB, peak VmPeak was 177,712/244,268/310,652 KiB, and peak threads were 52/84/116. The 32-to-96 change was 1.55x RSS and 1.75x virtual-memory reservation, with 64 additional threads. The most visible operational pressure was admission contention: retries per job rose from 1.42 to 31.34 and end-to-end p95 rose from 3,089 µs to 15,926 µs.

These are process-level allocator-pressure proxies, not allocator traces or GC measurements. Rust has no tracing GC on this path. VmRSS and VmHWM describe resident working-set ceilings, VmPeak includes reservations and mappings, thread count is a proxy for stack/control-block pressure, and retries/p95 describe scheduling and bounded-admission pressure rather than memory causality. The hot-key fixture intentionally reuses one CAS generation, so all but one commit are expected same-generation conflicts. It is suitable for contention bookkeeping but not valid durable-write throughput.

The next memory boundary remains allocator-instrumented mixed-workload profiling: fixed allocator configuration; allocated/free bytes; peak live bytes; fragmentation; cgroup events; cold and warm runs; unique-request, valid-sequential, conflict-storm, forged-evidence, and mixed traffic; and correlation of allocation rate with queue depth and cache misses.

## Validation contract

Phase 49 evidence must include the new typed-UEG integration suite, all existing Phase 48 UEG tests, the CLI golden regression, all-target Rust tests, rustfmt and whitespace checks, skill validation, and explicit restoration of build-generated artifacts. This language-contract phase does not add security/compliance gates, so the published 209-gate metadata remains unchanged.

## Remaining boundaries

`ast_fragment` remains a legacy JSON-like string and target emitters remain heuristic scaffolds. A later phase should replace the fragment with a typed serializable AST, add source-spanned expression nodes and structured diagnostics beyond statements, and wire target-specific semantic validation without claiming that current Go/Zig output is production compiler output.
