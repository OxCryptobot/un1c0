# Phase 60: typed semantic edit manifests

## Objective

Phase 59 derives semantic changes from fingerprints, but a source editor still needs a typed bridge from byte edits to UEG function indexes. Phase 60 introduces a local-only edit manifest that binds a set of source-byte ranges to the session's current profile key and root key. The session resolves each range against exactly one UEG function span before allowing dependency-aware refresh.

## Contract

`SemanticEditRange` is a validated half-open byte range. A `SemanticEditManifest` sorts ranges, rejects invalid ordering and overlap, and stores the exact base root/profile keys from the current session. `manifest_for_edits` creates the manifest without granting any authority.

`derive_edit_resolution` first performs Phase 59 fingerprint derivation, then checks the manifest's profile and base-root binding. Each range must overlap exactly one function source span. Zero matches are unmapped; multiple matches are ambiguous. The derived semantic-change set must be a subset of the functions named by the manifest. A semantic change outside the mapped set invalidates the session.

`refresh_from_edit_manifest` uses the resolved derived change set and retains Phase 56 reverse-dependent validation, Phase 57 snapshot binding, and Phase 58 target/profile identity. Any error clears the current snapshot.

## Verification matrix

| Boundary | Required assertion |
|---|---|
| Range syntax | Reject reversed ranges and overlapping edits |
| Base binding | Reject manifests created from another session/root |
| Profile binding | Reject profile drift before edit mapping |
| Span mapping | Reject zero-match and multi-function ranges |
| Semantic completeness | Reject changed functions outside mapped edit functions |
| Valid refresh | Map a leaf edit, revalidate the leaf, and preserve reverse-dependent closure |
| Authority | No filesystem, process, network, secret, signing, or cluster authority |

## Benchmark method

Use deterministic 1/2/4/8/16/32-function Python call chains, one leaf range, Rust/Go/Zig/Python profiles, and 64 samples per row. Compare full snapshot capture, edit-manifest resolution, and manifest-bound refresh at p50/p95. Record range count, changed/mapped/affected/revalidated counts, error count, and sanitized authority markers. The call-chain fixture is intentionally conservative: a leaf edit reaches every caller, so refresh cost must not be presented as a general speedup.
