# Phase 63: typed multi-unit semantic snapshot envelopes

## Objective

Phase 62 sequences semantic batches but does not package the resulting per-unit semantic state for a later emission boundary. Phase 63 adds a typed snapshot envelope containing the exact profile key, applied batch ID, and root key for every unit in the valid batch session.

## Contract

`SemanticSnapshotEnvelope::capture` accepts only a valid `SemanticBatchSession` and a non-zero batch ID that has already been applied. It records the complete unit identity set and each unit's current snapshot root. `verify_for` requires the same batch ID, the exact same unit set, and candidate UEGs whose per-unit profile/root fingerprints match the envelope. Missing units, unexpected units, profile drift, root drift, or an invalid session fail closed.

The envelope stores no source text, secret material, filesystem path authority, process handles, transport state, or distributed consensus claim. It is a typed local evidence object for pre-emitter verification.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Capture | Reject zero IDs, invalidated sessions, and empty unit state |
| Applied state | Reject envelopes claiming a batch ID not yet applied |
| Identity | Reject missing or unexpected unit identities |
| Profile | Reject profile-key drift in any unit |
| Root | Reject UEG changes in any unit |
| Batch binding | Reject envelope verification under another batch ID |
| Authority | No filesystem, process, network, secret, signing, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8-unit Rust batches with eight functions per unit, capture a successful Phase 62 batch state, and run 64 samples per row. Compare envelope capture with envelope verification. Record unit count, total functions, batch ID, errors, and sanitized authority markers. Do not infer production capacity from local microbenchmarks.
