# Helm Production-Readiness Audit

**Chart:** `deploy/helm/un1c0`

**Values under review:** `deploy/helm/un1c0/values-staging.yaml`

**Validation mode:** Helm 3.17.3, checksum-verified local client, client-side lint and template rendering. No Kubernetes API or staging cluster was contacted.

## Executive verdict

The chart now has a strong fail-closed security baseline, but it is **not deployable to production or staging unchanged**. The staging values intentionally contain placeholder image digests, an example Vault address, and empty network-policy CIDRs. Helm rendering fails until those values are supplied. This is the desired behavior for a staging baseline: missing trust and routing inputs cannot silently become a live release.

The rendered chart passes the static security checks below when supplied with test digests and explicit CIDRs. A real production-readiness approval still requires registry-verified image digests, real Vault and secret references, a confirmed staging namespace/context, a server-side Helm dry run, and bounded end-to-end rollout and mTLS verification.

## Values walk-through

| Values area | Current setting | Assessment |
|---|---|---|
| Admin replicas | `2` | Pass. Supports rolling updates and the PDB. |
| Admin image | Empty tag plus required SHA-256 digest placeholder | Pass by design. The helper rejects non-`sha256:<64 lowercase hex>` values. Replace the placeholder with the digest produced by the signed release workflow. |
| Admin runtime secret | `un1c0-runtime-secrets` with `VAULT_TOKEN` and `ADMIN_API_KEY` keys | Pass. Secrets are referenced, not embedded. The Secret must be provisioned through the approved secret-management path. |
| Vault address | `https://vault.example.invalid` | Blocker until replaced with the real staging Vault endpoint and certificate trust configuration. |
| nginx replicas | `2` | Pass. Supports rolling updates, PDB, and mTLS gateway availability. |
| nginx image | Empty tag plus required SHA-256 digest placeholder | Pass by design. Replace with a registry-verified digest; do not use the placeholder. |
| mTLS secret | `un1c0-mtls` with `server.crt`, `server.key`, and `ca.crt` | Pass as a secret contract. The secret must be created externally, with private key material never committed. |
| Network policy CIDRs | Empty `externalVaultCidr` and `nginxIngressCidr` | Blocker by design. Rendering requires explicit CIDRs to prevent broad external egress or ingress. |
| Vault mode | `enabled: false` | Pass for staging when Vault is external. Do not enable an in-cluster development Vault for production. |

## Rendered security controls

| Control | Result | Evidence and interpretation |
|---|---|---|
| Immutable images | Pass when inputs are supplied | Helpers reject tags and malformed digests and render `repository@sha256:...`. |
| Non-root admin | Pass | Admin image uses UID/GID `10001`; pod context now explicitly sets `runAsUser`, `runAsGroup`, and `fsGroup` to `10001`. |
| Non-root nginx | Pass | nginx pod context explicitly sets UID/GID/fsGroup `101`, avoiding reliance on image metadata. |
| Seccomp | Pass | Both component pod contexts use `RuntimeDefault`. |
| Privilege escalation | Pass | Both containers set `allowPrivilegeEscalation: false`. |
| Filesystem | Pass | Both containers set `readOnlyRootFilesystem: true`; writable paths are bounded memory-backed `emptyDir` mounts. |
| Linux capabilities | Pass | Both containers drop all capabilities. nginx no longer adds CHOWN/SETUID/SETGID/DAC_READ_SEARCH because the explicit non-root runtime does not require them. |
| Service-account token | Pass | The ServiceAccount and both pods set `automountServiceAccountToken: false`. |
| Secrets | Pass with external provisioning gate | Runtime and mTLS material are referenced through Secret objects; no secret values appear in the chart. |
| TLS client authentication | Pass in configuration | nginx renders `ssl_verify_client on` and mounts the CA, server certificate, and private key from the mTLS Secret. |
| Health and readiness | Pass with E2E gate | Admin has `/ready` readiness and `/health` liveness. nginx has TCP readiness and `nginx -t` liveness; a post-rollout client-certificate probe remains required. |
| Network isolation | Pass with CIDR gate | Admin egress to Vault requires `externalVaultCidr`; nginx ingress requires `nginxIngressCidr`; internal nginx-to-admin and DNS paths are bounded by NetworkPolicy. |
| Rollout safety | Pass | Two replicas, `maxUnavailable: 0`, `maxSurge: 1`, and PDB `minAvailable: 1` are rendered for both components. |
| Resource budgets | Pass | CPU and memory requests/limits are explicit for both deployments. |
| Cluster mutation | Not performed | No Kubernetes context or API target was available; only local Helm lint/template validation ran. |

## Required release gates

Before production or staging approval, replace all placeholders, verify the image digests against the release attestation/SBOM, provision the runtime and mTLS Secrets through the approved secret manager, set the actual external Vault address and CIDRs, and connect an authorized staging context and namespace. Then run `helm upgrade --install --dry-run=server`, obtain explicit mutation approval, wait for both rollouts, verify pod security and events, probe admin health/readiness, test nginx with a real client certificate, verify Prometheus metrics, and run the existing end-to-end fixture path.

A successful client-side render is not deployment evidence. The current audit therefore assigns **Static Security Baseline: PASS**, **Configuration Completeness: BLOCKED until placeholders are replaced**, and **Staging Integration: NOT RUN because no authorized cluster target is connected**.
