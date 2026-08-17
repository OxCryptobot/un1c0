#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

runtime="${CONTAINER_RUNTIME:-}"
if [[ -z "$runtime" ]]; then
  if command -v docker >/dev/null 2>&1; then
    runtime=docker
  elif command -v podman >/dev/null 2>&1; then
    runtime=podman
  else
    echo "No Docker or Podman runtime is installed." >&2
    exit 2
  fi
fi

if [[ "$runtime" == "docker" ]]; then
  command -v docker >/dev/null 2>&1 || { echo "docker is not installed" >&2; exit 2; }
  compose=(docker compose)
elif [[ "$runtime" == "podman" ]]; then
  command -v podman >/dev/null 2>&1 || { echo "podman is not installed" >&2; exit 2; }
  if command -v podman-compose >/dev/null 2>&1; then
    compose=(podman-compose)
  else
    echo "podman-compose is not installed" >&2
    exit 2
  fi
else
  echo "Unsupported CONTAINER_RUNTIME=$runtime; use docker or podman" >&2
  exit 2
fi

if [[ "${PODMAN_SUDO:-0}" == "1" && "$runtime" == "podman" ]]; then
  compose=(sudo -E "${compose[@]}")
fi

eval "$(./scripts/allocate_ci_ports.sh)"
export VAULT_PORT ADMIN_PORT NGINX_PORT COMPOSE_PROJECT_NAME
export VAULT_ADDR=http://vault:8200
compose+=( -p "$COMPOSE_PROJECT_NAME" -f vault/docker-compose.yml )

cleanup() {
  set +e
  "${compose[@]}" down --volumes --remove-orphans >/tmp/un1c0-compose-smoke-cleanup.log 2>&1
}
trap cleanup EXIT INT TERM

"${compose[@]}" config >/tmp/un1c0-compose-smoke-config.yml
VAULT_PORT="$VAULT_PORT" ADMIN_PORT="$ADMIN_PORT" NGINX_PORT="$NGINX_PORT" "${compose[@]}" up --build -d

wait_http() {
  local url="$1"
  for _ in $(seq 1 60); do
    if curl --silent --show-error --fail --max-time 3 "$url" >/dev/null; then return 0; fi
    sleep 2
  done
  echo "Timed out waiting for $url" >&2
  return 1
}

wait_http "http://127.0.0.1:${VAULT_PORT}/v1/sys/health"
wait_http "http://127.0.0.1:${ADMIN_PORT}/health"
wait_http "http://127.0.0.1:${ADMIN_PORT}/ready"

curl --silent --show-error --fail --max-time 10 \
  -k --cert vault/certs/client.crt --key vault/certs/client.key \
  "https://127.0.0.1:${NGINX_PORT}/health" >/tmp/un1c0-compose-smoke-health.json
curl --silent --show-error --fail --max-time 10 \
  -k --cert vault/certs/client.crt --key vault/certs/client.key \
  "https://127.0.0.1:${NGINX_PORT}/metrics/prometheus" >/tmp/un1c0-compose-smoke-metrics.txt

grep -q '^un1c0_admin_status 1$' /tmp/un1c0-compose-smoke-metrics.txt
printf 'Compose smoke passed: runtime=%s project=%s vault=%s admin=%s nginx=%s\n' \
  "$runtime" "$COMPOSE_PROJECT_NAME" "$VAULT_PORT" "$ADMIN_PORT" "$NGINX_PORT"
