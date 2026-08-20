# Phase 46 adaptive admission and verification-cost reduction plan

## Objective

Control high-concurrency verifier pressure without weakening the Phase 43 lock-held mutation authority, and reduce repeated Ed25519 verification cost through immutable parsed-key reuse and context-bound cryptographic facts.

## Architecture

`AdaptiveOwnershipBoundCasAdmission` wraps the Phase 45 bounded verifier pipeline. A controller owns a bounded in-flight permit budget. Submission above the current permit count returns typed `Limited` without consuming a verifier intent ID. Completed tickets release permits and feed a fixed adjustment window. Healthy windows add one permit up to the configured maximum; pre-admission verification failures or service p95 above the configured target halve permits down to the minimum.

The Phase 45 pre-admission context now parses pinned writer and replica keys once when the context is constructed. It maintains a bounded exact-fact cache keyed by context fingerprint, fact type, and content hash. The fingerprint includes cluster/resource/snapshot identity, required quorum, and the pinned key registries. Cache entries record only that a cryptographic fact was verified; they never cache freshness, request/ack binding, distinct-quorum, ownership, CAS generation, nonce, or persistence decisions.

## Acceptance criteria

| Criterion | Required evidence |
|---|---|
| Bounded admission | Minimum, initial, and maximum permits are validated; in-flight admission never exceeds the configured maximum |
| Typed limiting | Full admission returns `Limited` without consuming a verifier intent ID or mutating state |
| Adaptive response | Pre-admission failure pressure halves permits; healthy windows increase permits additively |
| Key cost reduction | Parsed `VerifyingKey` values are reused across worker calls; registry bytes remain exact-match checked |
| Cache safety | Fact keys include a context fingerprint; cache hits still execute shape, resource, freshness, binding, quorum, and live mutation checks |
| Metrics | Limiter rejections, permit state, service p95, cache hits/misses/entries, queue wait, mutation service, and end-to-end metrics remain bounded and sanitized |
| Stress | Producer levels 1, 2, 4, 8, 16, and 32 complete with one successful same-generation transition and fail-closed conflicts for the remainder |
| Compliance | Eight Phase 46 controls raise the total from 193 to 201 gates |

## Production boundary

Phase 46 is a local admission and verification optimization. It does not coordinate limiter state across hosts, establish CPU fairness across processes, provide distributed cache invalidation, or prove managed-volume/replica-domain authority. Production deployments must bind context replacement to key-registry/resource/protocol changes and retain Phase 43 live revalidation.
