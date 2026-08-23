# Phase 59: deterministic semantic change derivation

## Objective

Phase 58 required callers to provide an explicit changed-function set. That API is safe only when the caller's set is complete and precise. Phase 59 derives the set from Phase 55 per-function fingerprints, rejects declared-set mismatches, and gives callers a zero-work no-op refresh when the new UEG is fingerprint-equivalent to the current session state.

## Contract

`SemanticSession::derive_change_set` compares the session's current per-function keys with the new UEG's per-function keys under the fixed target profile. It returns changed and unchanged indexes, function-count evidence, and previous/current root keys. Function names and order remain a structural boundary; any mismatch returns `StructuralChange` and invalidates the session.

`refresh_auto` uses the derived set. The explicit `refresh` API still accepts a declared set, but it must exactly equal the derived set. A mismatch invalidates the session instead of silently trusting an overbroad or incomplete caller declaration. If the derived set is empty and roots are equal, refresh returns the current valid snapshot with zero affected functions, zero revalidation, and zero cache work.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Exact derivation | One changed function changes exactly one function key and the root |
| Caller integrity | Declared and derived sets must be identical |
| No-op safety | Equal roots return zero-work refresh and preserve the snapshot |
| Parser errors | Blocking UEG diagnostics invalidate even when fingerprints are unchanged |
| Structural edits | Function count, names, or order invalidate before semantic reuse |
| Dependency reuse | Auto-refresh still revalidates the changed leaf and reuses unchanged reverse callers |
| Profile binding | Target/profile drift invalidates the session |
| Authority | No filesystem, process, network, secret, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8/16/32-function call chains, Rust/Go/Zig/Python profiles, 64 samples per row, and a one-function leaf edit. Measure full snapshot capture, fingerprint-derived change-set derivation, and warmed auto-refresh separately at p50/p95/p99. Record changed/unchanged/affected/revalidated counts, error count, and sanitized authority markers. Do not claim that auto-refresh is faster when the changed leaf reaches the entire call chain.
