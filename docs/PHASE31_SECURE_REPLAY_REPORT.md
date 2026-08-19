# Phase 31 Secure Deterministic Replay Verification

## Summary

Phase 31 hardens the Phase 30 partition simulator against untrusted replay inputs. It introduces a typed `ReplayManifest`, bounded `ReplayFaultStep` schedule, authenticated `ReplayTraceSeal`, and transactional `SecureReplayEngine`. A replay schedule is accepted only after canonical SHA-256 schedule binding, Ed25519 signature verification, trusted-key equality, cluster/signer/replay-epoch/owner-term checks, nonce and identifier validation, strict sequence ordering, monotonic bounded ticks, and final trace-seal verification.

The engine executes against a cloned simulator and commits the clone only after all checks pass. Failed manifest, binding, schedule, signature, sequence/tick, simulation, or trace-seal checks leave the caller’s simulator unchanged.

## Verification sequence

| Stage | Control |
|---|---|
| Shape validation | Bounds scenario, cluster, signer, nonce, event count, tick count, public-key length, signature length, endpoints, sequence, and ticks. |
| Schedule binding | Recomputes SHA-256 over canonical serialized fault steps and rejects any mismatch before fault application. |
| Identity binding | Requires configured trusted public key, expected cluster, expected signer, minimum replay epoch, and minimum owner term. |
| Manifest signature | Verifies Ed25519 over canonical scenario, cluster, signer, epoch, term, seed, nonce, bounds, schedule digest, schedule bytes, and public key. |
| Transactional application | Clones the simulator, advances only to nondecreasing schedule ticks, injects typed directed faults, and never mutates the caller during preparation. |
| Trace seal | Verifies seal identity, epoch, trusted key, signature, event count, and recomputed SHA-256 event digest before committing the clone. |
| Result policy | Requires safety invariants to pass; liveness remains an explicit result because a fenced replay may be safely non-live during partition. |

## Executable evidence

The Phase 31 integration suite contains eight tests covering valid transactional replay, missing manifest signatures, schedule hash tampering, cluster/signer/epoch binding, non-monotonic schedules, trace-seal tampering, wrong trusted keys, and stale replay generations. The benchmark example emits non-secret results showing one valid replay step, one event, a passing safety result, trace digest, accepted signature, rejected tampered schedule, verified trace seal, and `private_key_persisted: false`.

The six Phase 31 gates are `signed_replay_manifest_required`, `replay_schedule_hash_bound`, `replay_sequence_tick_bounds_enforced`, `trusted_key_cluster_epoch_binding`, `tampered_schedule_rejected`, and `trace_seal_verification`. The total compliance count increases from 76 to 82.

## Security boundaries

The implementation authenticates replay inputs and seals local event traces; it does not provide production key custody, remote key registry distribution, TLS transport, cloud-region network authority, or real failure-detector consensus. Those remain explicit deployment boundaries. Metrics contain only public metadata, digests, counts, outcomes, and boundary declarations.

## Reproduction

```bash
scripts/validate_phase31_secure_replay.sh
cargo run --example phase31_secure_replay_benchmark -- --output benchmarks/phase31_secure_replay_metrics.json
```

## References

[1]: ../src/replay.rs "Phase 31 secure replay implementation"
[2]: ../tests/phase31_secure_replay_integration.rs "Phase 31 security integration tests"
[3]: ../examples/phase31_secure_replay_benchmark.rs "Phase 31 benchmark example"
[4]: ../benchmarks/phase31_secure_replay_metrics.json "Phase 31 non-secret benchmark artifact"
[5]: ../scripts/validate_phase31_secure_replay.sh "Phase 31 validation gate"
