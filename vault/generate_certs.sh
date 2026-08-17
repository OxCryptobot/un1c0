#!/usr/bin/env bash
set -euo pipefail

# Generate a CA, server certificate, and client certificate for local mTLS testing.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CERTS_DIR=${CERTS_DIR:-"$ROOT_DIR/vault/certs"}
umask 077
mkdir -p "$CERTS_DIR"
cd "$CERTS_DIR"

echo "Generating CA key and cert"
openssl genrsa -out ca.key 4096
openssl req -x509 -new -nodes -key ca.key -subj "/CN=LocalVaultCA" -days 3650 -out ca.crt

echo "Generating server key and CSR"
openssl genrsa -out server.key 2048
openssl req -new -key server.key -subj "/CN=admin-service" -out server.csr

cat > v3.ext <<EOF
authorityKeyIdentifier=keyid,issuer
basicConstraints=CA:FALSE
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = admin-service
DNS.2 = localhost
IP.1 = 127.0.0.1
EOF

openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out server.crt -days 365 -sha256 -extfile v3.ext

echo "Generating client key and cert"
openssl genrsa -out client.key 2048
openssl req -new -key client.key -subj "/CN=ci-runner" -out client.csr

cat > v3-client.ext <<EOF
authorityKeyIdentifier=keyid,issuer
basicConstraints=CA:FALSE
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = clientAuth
EOF

openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out client.crt -days 365 -sha256 -extfile v3-client.ext

echo "Generated files in ${CERTS_DIR}: ca.crt ca.key server.crt server.key client.crt client.key"
