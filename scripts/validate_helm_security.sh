#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CHART_DIR="${ROOT_DIR}/deploy/helm/un1c0"
VALUES_FILE="${CHART_DIR}/values-staging.yaml"
RENDERED=$(mktemp)
ERROR_LOG=$(mktemp)
trap 'rm -f "${RENDERED}" "${ERROR_LOG}"' EXIT

command -v helm >/dev/null 2>&1 || {
  echo "helm is required for chart security validation" >&2
  exit 127
}

if helm template un1c0-staging "${CHART_DIR}" -n un1c0-staging -f "${VALUES_FILE}" >"${RENDERED}" 2>"${ERROR_LOG}"; then
  echo "staging values unexpectedly rendered without required release inputs" >&2
  exit 1
fi
grep -Eq 'required|digest' "${ERROR_LOG}" || {
  echo "untouched staging values failed without an actionable required-input error" >&2
  cat "${ERROR_LOG}" >&2
  exit 1
}

DIGEST="sha256:$(printf '0%.0s' $(seq 1 64))"
helm lint "${CHART_DIR}" \
  -f "${VALUES_FILE}" \
  --set "admin.image.digest=${DIGEST}" \
  --set "nginx.image.digest=${DIGEST}" \
  --set networkPolicy.externalVaultCidr=10.0.0.0/8 \
  --set networkPolicy.nginxIngressCidr=10.0.0.0/8 >/dev/null

helm template un1c0-staging "${CHART_DIR}" \
  -n un1c0-staging \
  -f "${VALUES_FILE}" \
  --set "admin.image.digest=${DIGEST}" \
  --set "nginx.image.digest=${DIGEST}" \
  --set networkPolicy.externalVaultCidr=10.0.0.0/8 \
  --set networkPolicy.nginxIngressCidr=10.0.0.0/8 >"${RENDERED}"

grep -q 'runAsUser: 10001' "${RENDERED}"
grep -q 'runAsUser: 101' "${RENDERED}"
grep -q 'runAsNonRoot: true' "${RENDERED}"
grep -q 'seccompProfile:' "${RENDERED}"
grep -q 'type: RuntimeDefault' "${RENDERED}"
grep -q 'allowPrivilegeEscalation: false' "${RENDERED}"
grep -q 'readOnlyRootFilesystem: true' "${RENDERED}"
grep -q 'automountServiceAccountToken: false' "${RENDERED}"
grep -q 'ssl_verify_client on' "${RENDERED}"
grep -q 'NetworkPolicy' "${RENDERED}"
grep -q 'PodDisruptionBudget' "${RENDERED}"
grep -q 'ghcr.io/oxcryptobot/un1c0/admin-service@sha256:' "${RENDERED}"
grep -q 'nginx@sha256:' "${RENDERED}"
grep -q 'kind: PeerAuthentication' "${RENDERED}"
grep -q 'mode: STRICT' "${RENDERED}"
grep -q 'kind: AuthorizationPolicy' "${RENDERED}"
grep -q 'cluster.local/ns/istio-system/sa/istio-ingressgateway-service-account' "${RENDERED}"
grep -q 'cluster.local/ns/un1c0-staging/sa/un1c0-staging-un1c0-nginx' "${RENDERED}"
grep -q 'name: un1c0-staging-un1c0-admin' "${RENDERED}"
grep -q 'name: un1c0-staging-un1c0-nginx' "${RENDERED}"

MESH_INVALID_VALUES=$(mktemp)
trap 'rm -f "${RENDERED}" "${ERROR_LOG}" "${MESH_INVALID_VALUES}"' EXIT
cat >"${MESH_INVALID_VALUES}" <<'EOF'
mesh:
  enabled: true
  provider: istio
  trustDomain: ""
  mtlsMode: STRICT
  nginxIngressPrincipals: []
  adminIngressPrincipals: []
EOF
if helm template un1c0-staging "${CHART_DIR}" -n un1c0-staging -f "${VALUES_FILE}" -f "${MESH_INVALID_VALUES}" \
  --set "admin.image.digest=${DIGEST}" \
  --set "nginx.image.digest=${DIGEST}" \
  --set networkPolicy.externalVaultCidr=10.0.0.0/8 \
  --set networkPolicy.nginxIngressCidr=10.0.0.0/8 >"${RENDERED}" 2>"${ERROR_LOG}"; then
  echo "mesh-enabled chart unexpectedly rendered without trust-domain and principals" >&2
  exit 1
fi
grep -Eq 'mesh\.(trustDomain|nginxIngressPrincipals|adminIngressPrincipals)' "${ERROR_LOG}" || {
  echo "mesh-enabled failure did not identify the missing zero-trust inputs" >&2
  cat "${ERROR_LOG}" >&2
  exit 1
}

printf '%s\n' 'Helm security validation passed: fail-closed values, immutable images, non-root/read-only pods, probes, mTLS, strict mesh authorization, policies, and disruption budgets.'
