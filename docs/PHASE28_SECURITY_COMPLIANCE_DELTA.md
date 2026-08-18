# Phase 28 Security and Compliance Delta

## Result

The Phase 27 artifact had **60 passing gates**. Phase 28 adds four executable compliance gates, producing a **64-gate artifact** after the Phase 28 full suite:

| New gate | Evidence |
|---|---|
| `quorum_loss_fences_delivery` | A quorum-loss observation creates a hash-bound fence and returns `OwnershipFenced` before socket write. |
| `lease_expiry_fences_delivery` | A finite expired lease returns `OwnershipFenced` and leaves the queue unchanged. |
| `fence_survives_restart` | Durable restore rehydrates the fence and acknowledgement commits remain blocked. |
| `ownership_transfer_clears_fence` | A valid higher-term/higher-epoch transfer clears the fence atomically; the new owner retries but remains quorum-gated. |

## Audit status

The independent auditor reports 64 expected, 64 observed, and 64 passed. The socket-and-queue review checks the four Phase 28 booleans and verifies that failure-detector and clock authority remain a deployment boundary. No secret material is recorded and no cluster mutation is performed.

## Interpretation

The delta hardens the local execution kernel against unsafe delivery during a partition, but it does not turn a local observation into a distributed truth. Production must provide the authenticated detector, quorum transport, lease renewal authority, clock uncertainty policy, split-brain fencing, and transfer coordination. The fail-closed local contract is intentionally useful even when those external systems are unavailable or report an unsafe state.

## References

[1]: ../benchmarks/security_compliance_metrics.json "Current Phase 28 metrics artifact"
[2]: ../benchmarks/security_compliance_audit.json "Current independent audit"
[3]: ../scripts/review_socket_backpressure_metrics.py "Socket and queue evidence review"
[4]: ../docs/PHASE27_60_GATE_AUDIT_BREAKDOWN.md "Phase 27 60-gate baseline breakdown"
