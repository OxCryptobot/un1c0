#!/usr/bin/env bash
set -euo pipefail

# Initialize a local Vault dev server for PoC
# Run: docker compose -f vault/docker-compose.yml up -d
# Then run this script to enable KV v2 and create a sample secret and policy.

VAULT_ADDR=${VAULT_ADDR:-http://127.0.0.1:8200}
VAULT_TOKEN=${VAULT_TOKEN:-root-token}
OUTPUT_ENV_FILE=${OUTPUT_ENV_FILE:-}

write_secret_file() {
  local name="$1" value="$2"
  if [ -n "$OUTPUT_ENV_FILE" ]; then
    umask 077
    printf '%s=%s\n' "$name" "$value" >> "$OUTPUT_ENV_FILE"
  fi
  if [ "${GITHUB_ACTIONS:-false}" = "true" ] && [ -n "$value" ]; then
    echo "::add-mask::$value"
  fi
}

echo "Using VAULT_ADDR=$VAULT_ADDR"

export VAULT_ADDR
export VAULT_TOKEN

echo "Enabling KV v2 at path 'kv'"
curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -d '{"type":"kv-v2"}' $VAULT_ADDR/v1/sys/mounts/kv || true

echo "Writing example master key to kv/data/master_key"
MASTER_KEY=$(openssl rand -hex 32)
EXPIRY=$(date -u -d "+1 day" +"%Y-%m-%dT%H:%M:%SZ")
curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -H "Content-Type: application/json" \
  -d "{\"data\":{\"key\":\"$MASTER_KEY\",\"expiry\":\"$EXPIRY\"}}" \
  $VAULT_ADDR/v1/kv/data/master_key

write_secret_file MASTER_KEY "$MASTER_KEY"
echo "Created master key in Vault at kv/data/master_key; the value is intentionally not printed."

echo "Creating a policy 'read-master-key' that allows reading kv/data/master_key"
cat > /tmp/read-master-key.hcl <<'HCL'
path "kv/data/master_key" {
  capabilities = ["read"]
}
HCL

POLICY_JSON=$(jq -Rs '{policy: .}' /tmp/read-master-key.hcl)
curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -H "Content-Type: application/json" -d "$POLICY_JSON" $VAULT_ADDR/v1/sys/policies/acl/read-master-key || true

echo "Creating token with 'read-master-key' policy"
READ_TOKEN=$(curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -d '{"policies":["read-master-key"],"ttl":"1h"}' $VAULT_ADDR/v1/auth/token/create | jq -r '.auth.client_token') || true
write_secret_file READ_TOKEN "$READ_TOKEN"
echo "Created a short-lived token for reading the master key; the value is intentionally not printed."

echo "Creating an AppRole 'master-key-approle' for GitHub Actions PoC"
cat > /tmp/approle-payload.json <<'JSON'
{"policies": ["read-master-key"], "token_ttl": "1h", "token_max_ttl": "2h"}
JSON

echo "Enabling AppRole auth method at path 'approle' (idempotent)"
curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -d '{"type":"approle"}' $VAULT_ADDR/v1/sys/auth/approle || true

curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" -H "Content-Type: application/json" -d @/tmp/approle-payload.json $VAULT_ADDR/v1/auth/approle/role/master-key-approle || true

ROLE_ID=$(curl -sS -H "X-Vault-Token: $VAULT_TOKEN" $VAULT_ADDR/v1/auth/approle/role/master-key-approle/role-id | jq -r '.data.role_id') || true
SECRET_ID=$(curl -sS -X POST -H "X-Vault-Token: $VAULT_TOKEN" $VAULT_ADDR/v1/auth/approle/role/master-key-approle/secret-id | jq -r '.data.secret_id') || true

write_secret_file ROLE_ID "$ROLE_ID"
write_secret_file SECRET_ID "$SECRET_ID"
echo "AppRole created; role and secret IDs are intentionally not printed."

echo "Initialization complete. Use the created token or AppRole credentials to read the master key via Vault API."
