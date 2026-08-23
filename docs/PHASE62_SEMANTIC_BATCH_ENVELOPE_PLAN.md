# Phase 62: versioned semantic-batch envelopes

## Objective

Phase 61 provides atomic multi-file refresh, but a batch has no explicit versioned identity. Phase 62 adds a typed envelope that binds one batch to a profile key, monotonic batch sequence, and ordered unit-update set. The envelope is local semantic evidence; it is not a transport token, filesystem authority, or distributed commit certificate.

## Contract

`SemanticBatchEnvelope` carries a non-zero `batch_id`, a `profile_key`, and one `SemanticEditBatch`. `SemanticBatchSession` stores the fixed profile key and the next accepted batch sequence. `start` initializes the sequence at 1. `refresh_envelope` requires an exact profile-key match and the next expected batch ID; duplicate, stale, skipped, or replayed envelopes fail closed and invalidate all unit sessions.

Successful application advances the expected sequence only after all staged unit refreshes succeed. A failed envelope does not advance the sequence and invalidates the live sessions. The existing Phase 61 all-or-nothing state boundary remains authoritative.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Batch identity | Reject zero IDs and duplicate update identities |
| Profile binding | Reject a profile key that differs from the session profile |
| Sequence | Accept only the next monotonic batch ID; reject replay and gaps |
| Atomicity | Do not expose staged updates when any unit fails |
| Success | Advance the sequence only after all units succeed |
| Rejection | Invalidate all unit snapshots on envelope failure |
| Authority | No filesystem, process, network, secret, signing, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8-unit Rust batches with eight functions per unit, 64 samples per row, and compare versioned-envelope refresh with the Phase 61 raw batch path. Record batch ID, unit count, changed/refreshed units, errors, and sanitized authority markers. The benchmark measures local sequencing and atomic refresh, not transport or distributed consensus.
