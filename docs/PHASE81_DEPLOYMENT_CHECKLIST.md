# Phase 81 Deployment Checklist

**Scope:** Authenticated service channels and durable replay epochs
**Release candidate:** `591ea37`
**Operating rule:** This checklist is a preparation artifact. It does not authorize deployment, cluster mutation, credential changes, or production promotion.

> Do not promote Phase 81 unless every required gate is explicitly evidenced, independently reviewed, and approved. A valid diagnostic or channel signature is not quorum, policy, fencing, consensus, or deployment authority.

## 1. Local implementation gates

| Gate | Evidence | Status |
|---|---|---|
| Source review | `emission_diagnostic_service_channel.rs` reviewed against commit `591ea37`; no network/process/unsafe authority in the local channel primitive. | Pass locally |
| Replay API boundary | `DurableReplayEpochStore::admit` is private; the receiver performs envelope verification before replay-state admission. | Pass locally |
| Canonical identity binding | Sender and receiver canonical identity IDs are persisted, validated at reopen, checked by receiver construction, and included in the state digest. | Pass locally |
| Envelope binding | Channel, sender/receiver services and identities, signer ID/generation, epoch, sequence, nonce, payload hash, and signature are bound and validated. | Pass locally |
| Durable persistence | Temporary-file creation, write, file sync, atomic rename, and directory sync are exercised; persistence errors fail closed. | Pass locally |
| Restart and epoch behavior | Restart reload, stale temporary cleanup, duplicate handling, epoch rollover, and old-epoch rejection are tested. | Pass locally |
| Rotation and revocation | Active signer generation and revocation behavior are tested through the Phase 79 registry. | Pass locally |
| Local security matrix | 13 related integration targets passed 63 tests with zero failures. | Pass locally |
| Full repository suite | `cargo test --all-targets` passed 451 tests with zero failures, ignored tests, or filtered tests in the hardening validation. | Pass locally |

## 2. Required production transport gates

These gates are not satisfied by the transport-agnostic Phase 81 envelope and must be completed before deployment.

| Gate | Required evidence | Release rule |
|---|---|---|
| TLS/mTLS | Live socket implementation with certificate-chain validation, SAN/service-identity checks, minimum protocol/cipher policy, peer authentication, and negative tests for expired, wrong, revoked, and untrusted certificates. | Block if any peer can connect without the expected authenticated channel. |
| Key management | Approved key storage, distribution, rotation, revocation, access control, audit trail, and recovery procedure. Never place private keys in source, logs, images, or ordinary configuration. | Block if signer state cannot be recovered or revoked safely. |
| Service discovery | Allowlisted endpoints and identity-to-service mapping with no ambient network trust. | Block unknown or misbound peers. |
| Replay durability | Production storage with bounded capacity, ownership/access controls, backup/restore evidence, corruption handling, and an explicit epoch recovery protocol. | Block on missing, stale, malformed, or misbound state. |
| Resource limits | Enforced CPU, memory, file, payload, queue, worker, connection, and disk budgets with backpressure and bounded cleanup. | Block before resource exhaustion; never rely on caller cooperation. |
| Readiness/liveness | Distinct liveness and strict readiness probes. Readiness must fail when identity, key registry, replay state, storage, or required policy evidence is unavailable. | Do not route traffic to an unready instance. |
| Observability | Sanitized counters and structured events for authentication failures, replay decisions, epoch changes, persistence failures, queue pressure, and readiness transitions. | No payloads, secrets, keys, signatures, or fencing tokens in telemetry. |

## 3. Staging rollout gates

1. Build from an immutable commit and image digest. Record the source revision, dependency lock state, toolchain, and image provenance.

2. Render deployment manifests with strict production-like values. Fail closed on missing image digests, missing identity/key references, absent resource limits, disabled probes, writable root filesystems where prohibited, excessive capabilities, ambient service-account tokens, broad network policy, or unbounded queues.

3. Use an isolated staging namespace, service identities, certificates, storage paths, ports, and sanitized fixtures. Do not reuse production credentials or persistent state.

4. Exercise valid delivery, wrong service identity, wrong receiver identity, wrong channel, signer rotation, signer revocation, payload tampering, signature tampering, duplicate frames, gaps, stale sequences, old epochs, corrupted replay state, stale temporary state, restart recovery, and persistence failures.

5. Verify that every rejected frame leaves replay, queue, journal, and application state unchanged. Confirm that only durably persisted state is exposed after restart.

6. Exercise resource pressure: maximum payloads, queue saturation, connection churn, worker limits, disk-full or write-failure simulation, and bounded timeout/retry behavior. Confirm backpressure or fail-closed rejection rather than unbounded growth.

7. Run readiness transitions through missing identity, revoked signer, missing replay state, corrupted replay state, unavailable storage, and policy mismatch. Confirm traffic is withheld until all required dependencies are healthy.

8. Run the non-mutating rollout dry run and independent approval workflow from Phase 80. Diagnostic evidence may support the decision but cannot grant deployment authority or bypass consensus, quorum, ownership, fencing, or evolution-ledger gates.

9. Produce sanitized staging evidence, including test summaries, readiness transitions, resource observations, replay decisions, restart results, and rollback rehearsal. Do not include raw payloads, private/public key material, raw signatures, credentials, or full fencing tokens.

## 4. Promotion and rollback controls

| Control | Required action |
|---|---|
| Independent approval | A separately authorized approver signs the exact manifest and staging evidence digests, including signer generation and ordered gate results. |
| No implicit deployment | The report and authorization record must not execute Helm, Docker, SSH, or cluster mutation. Actual deployment is a separately confirmed operation. |
| Canary | Start with an isolated, bounded canary and explicit traffic limits. Monitor authentication, replay, readiness, resource, and persistence metrics. |
| Rollback | Verify a deterministic rollback to the last known-good release and preserve the failing evidence. Rollback must not delete forensic replay state before retention requirements are satisfied. |
| Abort conditions | Abort on any authentication bypass, replay-state ambiguity, identity misbinding, persistence error, readiness false positive, resource-budget breach, secret leakage, unexpected mutation, or disagreement between observed and approved evidence. |
| Post-promotion review | Reconcile deployed digests, identity generations, replay epochs, readiness state, and audit records against the approved manifest. |

## 5. Explicit non-goals for this release candidate

The local Phase 81 implementation is not a production service channel by itself. It does not provide TLS/mTLS, confidentiality, certificate lifecycle management, network admission, service discovery, cluster deployment, production readiness, resource isolation, external quorum, fencing authority, or rollback automation. Until those gates are separately implemented and approved, the correct deployment decision is **do not promote**.

## References

[1]: PHASE81_RELEASE_NOTES.md "Phase 81 release notes"

[2]: PHASE81_AUTHENTICATED_CHANNELS_AND_REPLAY_EPOCHS_REPORT.md "Phase 81 authenticated channels and replay epochs report"

[3]: PHASE76_81_DIAGNOSTIC_STREAMING_INTEGRATION_ROADMAP.md "Phase 76–81 diagnostic streaming integration roadmap"

[4]: ../src/emission_diagnostic_service_channel.rs "Phase 81 authenticated service-channel implementation"
