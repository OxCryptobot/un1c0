# Phase 61: atomic multi-file semantic edit batches

## Objective

Phase 60 binds one source-byte edit manifest to one semantic session. Phase 61 extends that contract across multiple independently fingerprinted UEG units while keeping the batch local, typed, and fail closed. A batch must either refresh all declared units successfully or invalidate every session in the batch.

## Contract

`SemanticUnitId` is a bounded, control-character-free, relative identity. It rejects absolute paths, empty segments, backslash traversal, and `..` components without reading the filesystem. `SemanticBatchSession::start` creates one fixed-profile semantic session per unit and rejects duplicate identities.

`SemanticEditBatch` sorts updates by unit identity and rejects empty or duplicate batches. `refresh_batch` checks the fixed profile, clones all sessions into a staging map, applies each Phase 60 manifest-bound refresh to the staged sessions, and commits the staged map only if every unit succeeds. Any unknown unit, stale manifest, semantic error, structural error, or profile drift invalidates all live sessions and commits nothing.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Unit identity | Reject absolute, traversal, control-character, empty, or overlong identities |
| Membership | Reject duplicate starts, duplicate updates, and unknown units |
| Profile | Reject batch profile drift and invalidate all units |
| Atomicity | A later unit failure cannot expose an earlier staged refresh |
| Valid batch | Refresh changed and unchanged units together under one fixed profile |
| Snapshot | Preserve per-unit valid snapshots only after all updates succeed |
| Authority | No filesystem, process, network, secret, signing, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8-unit batches with eight functions per unit, one leaf edit per unit, 64 samples, and the Rust profile. Compare atomic batch refresh with sequential per-unit refresh at p50/p95. Record total functions, changed/refreshed units, errors, and sanitized authority markers. Because the current benchmark constructs fresh sessions per sample and validates a worst-case call chain, it is a correctness-oriented local microbenchmark rather than a production throughput claim.
