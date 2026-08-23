# Phase 65: typed snapshot-bound emission receipts

## Objective

Phase 64 gates multi-unit code generation on an exact semantic snapshot envelope. Phase 65 records a deterministic, typed receipt of that successful emission so local callers can audit which batch/profile/unit roots produced which output statistics and digest.

## Contract

`EmissionReceipt` binds the emitter target, applied batch ID, envelope profile key, complete per-unit root-key map, generation counts, byte count, and a domain-separated SHA-256 digest of the delivered chunks. The digest is computed in the deterministic `BTreeMap` unit order and includes unit identity, node index, and emitted code bytes only after the sink accepts each chunk.

`emit_with_receipt` returns a receipt only after the complete snapshot-bound emission succeeds. A sink or generator error returns no receipt. `verify_for` checks target, batch ID, profile key, exact unit set, and per-unit root keys against a `SemanticSnapshotEnvelope`; it never treats a mismatched receipt as advisory evidence.

## Security and authority boundaries

The receipt is local evidence, not a signature, secret, transport token, filesystem commit record, or distributed consensus certificate. It stores no source paths beyond bounded typed unit IDs and no private key or credential material. It must not be used to authorize a later emission without repeating envelope verification.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Receipt creation | No receipt on any generator or sink error |
| Target | Receipt target equals emitter/profile target |
| Batch | Receipt batch ID equals verified envelope batch ID |
| Profile | Receipt profile key equals envelope profile key |
| Units | Receipt unit-root map equals envelope unit-root map exactly |
| Digest | Digest changes when accepted emitted bytes or ordering changes |
| Stats | Counts and byte totals describe the delivered chunks |
| Authority | No signing, persistence, process, network, secret, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8-unit Rust batches with eight functions per unit and 64 samples per row. Compare Phase 64 emission with Phase 65 emission-plus-receipt. Record p50/p95, chunk counts, zero errors, and sanitized authority markers. Do not infer production throughput from local sandbox measurements.
