# Security Compliance Report

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded consensus/state replication, joint-consensus membership changes, dynamic re-voting, crash recovery, authenticated socket transport, cluster configuration IDs, replay windows, zero-trust mesh authorization, cryptographic audit logging, durable snapshots, authenticated consensus envelopes, signer lifecycle, Helm fail-closed rendering, and isolated mTLS validation
**Author:** Manus AI

## Executive assessment

The current batch adds a transport-agnostic zero-trust authorization layer and an Ed25519-signed, SHA-256 hash-chained audit sink while preserving the existing runtime, consent, Helm, and mTLS boundaries. The implementation fails closed for unknown trust-domain identities, certificate fingerprints, peer relations, audiences, methods, signer keys, malformed chains, oversized metadata, missing mesh inputs, mutable image tags, missing network CIDRs, and missing staging secrets.

The repository’s Raft-style core remains intentionally bounded and deterministic. It provides quorum elections, current-term commit rules, command-integrity hashes, bounded replication batches, and state snapshots, but its `ReplicatedSnapshot` is an in-memory transport object rather than a durable snapshot-install protocol. Production promotion therefore remains gated on authenticated message transport, durable log/snapshot storage, key rotation and revocation, service-mesh control-plane availability, and failure-injection evidence.

## Control matrix

| Control | Implementation | Evidence | Status |
|---|---|---|---|
| Consensus membership | Bounded member IDs and unknown-member rejection | [`src/consensus.rs`](../src/consensus.rs) | Implemented |
| Quorum commit | `floor(n/2)+1` quorum; leader applies only current-term acknowledged entries | [`src/consensus.rs`](../src/consensus.rs) | Implemented |
| Snapshot integrity | Snapshot includes term, commit index, last-applied index, state, and SHA-256 state hash | [`src/consensus.rs`](../src/consensus.rs) | Implemented |
| Snapshot durability | Unique temporary file, write/fsync, atomic rename, hash validation, stale-snapshot rejection, and install path | [`src/consensus.rs`](../src/consensus.rs) | Implemented; backup/restore operations remain deployment gates |
| Consensus transport identity | Ed25519 envelope binds sender ID, term, bounded nonce, message, and trusted public key | [`src/consensus.rs`](../src/consensus.rs) | Implemented; replay window remains deployment gate |
| Membership transition | Joint configuration carries old/new sets, requires double majority, and finalizes only after commit | [`src/consensus.rs`](../src/consensus.rs), [`tests/phase11_consensus_integration.rs`](../tests/phase11_consensus_integration.rs) | Implemented |
| Crash recovery | Process-abort staging cleanup, atomic snapshot rewrite, invalid-install rollback | [`tests/failure_injection_integration.rs`](../tests/failure_injection_integration.rs) | Passed |
| Partition evidence | Healthy/majority/minority metrics report drops, verification p95, throughput, and quorum availability | [`benchmarks/consensus_partition_metrics.json`](../benchmarks/consensus_partition_metrics.json) | Passed; in-process benchmark only |
| Socket framing | Bounded length-prefixed TCP frames with typed envelope deserialization | [`src/consensus.rs`](../src/consensus.rs), [`tests/phase12_transport_integration.rs`](../tests/phase12_transport_integration.rs) | Passed |
| Cluster/replay binding | Trusted sender key lookup, cluster-ID binding, sender-local transport identity, insertion-ordered replay window | [`src/consensus.rs`](../src/consensus.rs), [`tests/phase12_transport_integration.rs`](../tests/phase12_transport_integration.rs) | Passed |
| Power-loss recovery | Process abort before snapshot rename, staging cleanup, atomic rewrite, invalid-install rollback | [`docs/PHASE12_TRANSPORT_RECOVERY_REPORT.md`](PHASE12_TRANSPORT_RECOVERY_REPORT.md) | Passed |
| Mesh identity | Trust-domain, namespace, service-account, SPIFFE-style identity validation | [`src/security.rs`](../src/security.rs) | Implemented |
| Mesh authorization | Audience, certificate fingerprint, peer relation, and method allowlist checks | [`src/security.rs`](../src/security.rs) | Implemented |
| In-cluster mTLS | Istio `PeerAuthentication` with `STRICT` mode when explicitly enabled | [`deploy/helm/un1c0/templates/mesh.yaml`](../deploy/helm/un1c0/templates/mesh.yaml) | Implemented as optional chart resources |
| Mesh policy | Istio `AuthorizationPolicy` and control-plane egress ports 15012/15017 | [`deploy/helm/un1c0/templates/mesh.yaml`](../deploy/helm/un1c0/templates/mesh.yaml), [`deploy/helm/un1c0/templates/policies.yaml`](../deploy/helm/un1c0/templates/policies.yaml) | Implemented |
| Audit authenticity | Ed25519 signature over canonical record payload and trusted signer binding | [`src/security.rs`](../src/security.rs) | Implemented |
| Audit tamper evidence | Contiguous sequence, previous hash, current hash, signature, and public-key verification | [`src/security.rs`](../src/security.rs) | Implemented |
| Audit privacy | Metadata is stored as a bounded SHA-256 digest; raw request payloads are not persisted | [`src/security.rs`](../src/security.rs) | Implemented |
| Signer lifecycle | Atomic registry persistence, one-way rotation, revocation, historical verification, and active-signer enforcement | [`src/security.rs`](../src/security.rs) | Implemented |
| External audit sink | Immutable content-addressed records, create-new writes, fsync, idempotent retries, chain verification, and flush recovery | [`src/security.rs`](../src/security.rs) | Implemented locally; remote sink remains deployment gate |
| Helm fail-closed | Untouched staging values fail; mutable tags, missing digests/CIDRs, and missing mesh inputs are rejected | [`scripts/validate_helm_security.sh`](../scripts/validate_helm_security.sh) | Passed |
| Edge mTLS | Disposable CA/server/client certificates, `ssl_verify_client on`, health and Prometheus checks | [`scripts/validate_compose_smoke.sh`](../scripts/validate_compose_smoke.sh), [`vault/nginx/mutual_tls.conf`](../vault/nginx/mutual_tls.conf) | Passed |

## Raft configuration and snapshot mechanics

| Parameter | Value | Interpretation |
|---|---:|---|
| Maximum members | 256 | Bounds membership validation and quorum bookkeeping. |
| Maximum append batch | 256 entries | Bounds one replication message. |
| Maximum state key | 4 KiB | Prevents unbounded command identifiers. |
| Maximum state value | 64 KiB | Prevents oversized replicated payloads. |
| Maximum log length | 100,000 entries | Bounds in-memory log growth per node. |
| Quorum | `members / 2 + 1` | Requires a strict majority for election and commit. |
| Snapshot state hash | SHA-256 | Provides deterministic equality evidence across replicas. |
| Snapshot persistence | None in this slice | Requires a durable storage/install protocol before production. |

A leader can append locally without committing. An entry becomes applicable only after a current-term quorum acknowledges its index. Followers validate the previous index and term, truncate conflicting suffixes, append only hash-valid entries, and apply no more than the leader’s committed index. The snapshot contains `term`, `commit_index`, `last_applied`, the ordered state map, and `state_hash`; it does not yet include a snapshot ID, membership configuration, chunking, compression, durable file path, or install/recovery message.

## Security validation evidence

The safe validation suite passed shell syntax checks for repository scripts, Helm fail-closed rendering, Rust all-target tests, Python tests, and isolated rootful Podman Compose/mTLS smoke. Secret-mutating Vault initialization, OIDC configuration, accessor cleanup, and master-key rotation scripts were intentionally not invoked because they require live credentials and operational authorization. The break-glass checker was also not invoked because it requires a user-provided token; no token was requested or printed.

The updated Helm gate additionally verifies strict Istio mTLS, both AuthorizationPolicy resources, explicit gateway and NGINX principals, distinct admin/NGINX service accounts, sidecar injection, control-plane discovery ports, and a negative render when mesh trust-domain/principal inputs are empty. The Compose path uses isolated ports, a unique project, disposable certificates under `umask 077`, bounded health probes, client-certificate HTTPS validation, Prometheus verification, and unconditional cleanup.

## Residual risks and required promotion gates

The local audit file remains the authoritative outbox and is not itself a remote distributed audit service. Phase 10 adds durable sink segments, idempotent retry, registry rotation/revocation, and local recovery. Production still requires a remote append-only sink, retention/deletion policy, clock-skew policy, cross-node ordering semantics, signer distribution, and alerting for chain verification failures. The mesh chart emits Istio resources but does not prove that an Istio control plane or admission webhook is installed; a staging server-side dry run and authenticated rollout remain required.

Phase 10 adds durable JSON snapshots and install checks, but the consensus log is not yet compacted around an installable membership/configuration record. The authenticated envelope binds identity, term, nonce, and message signature; a production transport still needs a replay window, cluster/configuration ID, key rotation distribution, and connection-level authorization bound to mesh identity.

## Metrics report

The generated [`benchmarks/security_compliance_metrics.json`](../benchmarks/security_compliance_metrics.json) records all fifteen local gates as passed, no secret material or cluster mutation, exact concurrency-eight benchmark evidence, and authenticated partition scenarios. Repository search improved from **37.202256 ms p95 / 249.185679 ops/s** to **13.454338 ms p95 / 922.652107 ops/s**, a **63.8346% p95 reduction** and **270.2669% throughput gain**, with zero errors in both runs. The authenticated partition benchmark reports 29.825 µs healthy verification p95, 29.644 µs majority-partition p95 with quorum available, and 29.565 µs minority-partition p95 with quorum unavailable. The metrics report was generated by [`scripts/collect_security_compliance_metrics.py`](../scripts/collect_security_compliance_metrics.py) and the complete gate is reproducible through [`scripts/validate_security_compliance.sh`](../scripts/validate_security_compliance.sh).

## References

[1]: ../src/consensus.rs "Bounded consensus and replicated-state implementation"
[2]: ../src/security.rs "Zero-trust mesh and cryptographic audit implementation"
[3]: ../scripts/validate_helm_security.sh "Helm fail-closed security validation"
[4]: ../scripts/validate_compose_smoke.sh "Isolated Compose mTLS smoke validation"
[5]: ../deploy/helm/un1c0/templates/mesh.yaml "Istio mesh resources"
[6]: ../vault/nginx/mutual_tls.conf "NGINX edge mTLS configuration"
