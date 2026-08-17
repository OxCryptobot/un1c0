# Security Compliance Report

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** bounded consensus/state replication, zero-trust mesh authorization, cryptographic audit logging, Helm fail-closed rendering, and isolated mTLS validation
**Author:** Manus AI

## Executive assessment

The current batch adds a transport-agnostic zero-trust authorization layer and an Ed25519-signed, SHA-256 hash-chained audit sink while preserving the existing runtime, consent, Helm, and mTLS boundaries. The implementation fails closed for unknown trust-domain identities, certificate fingerprints, peer relations, audiences, methods, signer keys, malformed chains, oversized metadata, missing mesh inputs, mutable image tags, missing network CIDRs, and missing staging secrets.

The repository’s Raft-style core remains intentionally bounded and deterministic. It provides quorum elections, current-term commit rules, command-integrity hashes, bounded replication batches, and state snapshots, but its `ReplicatedSnapshot` is an in-memory transport object rather than a durable snapshot-install protocol. Production promotion therefore remains gated on authenticated message transport, durable log/snapshot storage, key rotation and revocation, service-mesh control-plane availability, and failure-injection evidence.

## Control matrix

| Control | Implementation | Evidence | Status |
|---|---|---|---|
| Consensus membership | Bounded member IDs and unknown-member rejection | [`src/consensus.rs`](../src/consensus.rs) | Implemented |
| Quorum commit | `floor(n/2)+1` quorum; leader applies only current-term acknowledged entries | [`src/consensus.rs`](../src/consensus.rs) | Implemented |
| Snapshot integrity | Snapshot includes term, commit index, last-applied index, state, and SHA-256 state hash | [`src/consensus.rs`](../src/consensus.rs) | Implemented in-memory |
| Mesh identity | Trust-domain, namespace, service-account, SPIFFE-style identity validation | [`src/security.rs`](../src/security.rs) | Implemented |
| Mesh authorization | Audience, certificate fingerprint, peer relation, and method allowlist checks | [`src/security.rs`](../src/security.rs) | Implemented |
| In-cluster mTLS | Istio `PeerAuthentication` with `STRICT` mode when explicitly enabled | [`deploy/helm/un1c0/templates/mesh.yaml`](../deploy/helm/un1c0/templates/mesh.yaml) | Implemented as optional chart resources |
| Mesh policy | Istio `AuthorizationPolicy` and control-plane egress ports 15012/15017 | [`deploy/helm/un1c0/templates/mesh.yaml`](../deploy/helm/un1c0/templates/mesh.yaml), [`deploy/helm/un1c0/templates/policies.yaml`](../deploy/helm/un1c0/templates/policies.yaml) | Implemented |
| Audit authenticity | Ed25519 signature over canonical record payload and trusted signer binding | [`src/security.rs`](../src/security.rs) | Implemented |
| Audit tamper evidence | Contiguous sequence, previous hash, current hash, signature, and public-key verification | [`src/security.rs`](../src/security.rs) | Implemented |
| Audit privacy | Metadata is stored as a bounded SHA-256 digest; raw request payloads are not persisted | [`src/security.rs`](../src/security.rs) | Implemented |
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

The local audit file is a signed hash chain, not a distributed audit-consensus protocol. Production requires an approved durable sink, a signer rotation/revocation process, retention and deletion policy, clock-skew policy, cross-node ordering semantics, and alerting for chain verification failures. The mesh chart emits Istio resources but does not prove that an Istio control plane or admission webhook is installed; a staging server-side dry run and authenticated rollout remain required.

The consensus snapshot remains in-memory and does not compact or persist the log. Before production promotion, add durable snapshot files with atomic replacement, snapshot-install messages, membership configuration binding, replay/recovery tests, and a transport authentication test that binds message identity to the zero-trust mesh policy.

## References

[1]: ../src/consensus.rs "Bounded consensus and replicated-state implementation"
[2]: ../src/security.rs "Zero-trust mesh and cryptographic audit implementation"
[3]: ../scripts/validate_helm_security.sh "Helm fail-closed security validation"
[4]: ../scripts/validate_compose_smoke.sh "Isolated Compose mTLS smoke validation"
[5]: ../deploy/helm/un1c0/templates/mesh.yaml "Istio mesh resources"
[6]: ../vault/nginx/mutual_tls.conf "NGINX edge mTLS configuration"
