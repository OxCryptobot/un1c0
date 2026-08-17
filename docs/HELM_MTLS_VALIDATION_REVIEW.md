# Helm Fail-Closed and Isolated mTLS Validation Review

## Helm staging gate

The script [`scripts/validate_helm_security.sh`](../scripts/validate_helm_security.sh) first renders the untouched staging values and requires that rendering fail with an actionable `required` or `digest` error. This prevents placeholder release inputs from being mistaken for a deployable environment. It then renders with deterministic test overrides: SHA-256 image digests, an external Vault CIDR, and an ingress CIDR.

The rendered manifest assertions cover the main production boundary: admin UID 10001, NGINX UID 101, non-root execution, RuntimeDefault seccomp, no privilege escalation, read-only roots, disabled service-account token automount, mTLS client verification, NetworkPolicies, PodDisruptionBudgets, and digest-pinned admin/NGINX images. The chart values keep the runtime Secret, mTLS Secret, Vault address, image digests, and CIDRs explicit rather than silently inventing them.

This is a client-side fail-closed rendering test. It does not contact a Kubernetes API, perform a server-side dry run, create Secrets, or mutate a namespace. Real staging promotion still requires an authorized context, real image digests, Vault configuration, Secrets, CIDRs, and explicit approval.

## Isolated Compose and mTLS smoke

[`scripts/validate_compose_smoke.sh`](../scripts/validate_compose_smoke.sh) selects Docker or Podman, allocates per-run ports through `allocate_ci_ports.sh`, assigns a unique Compose project name, generates certificates into a disposable `CERTS_DIR` under `umask 077`, validates Compose configuration, and starts the Vault, admin, and NGINX services. Bounded probes wait for Vault health, admin health/readiness, and the actual client-certificate HTTPS endpoint.

The NGINX configuration requires `ssl_verify_client on`, validates client certificates against the generated CA, rate-limits by client subject, and proxies directly to the `admin-service` Compose DNS name. The smoke test then verifies the mTLS health endpoint and Prometheus metric `un1c0_admin_status 1`. An unconditional trap removes containers, volumes, orphans, temporary certificates, and private-key material even when a probe fails.

The smoke path validates local integration and security wiring rather than production availability. It does not establish a Kubernetes network boundary, rotate long-lived credentials, or replace a real Vault policy review.
