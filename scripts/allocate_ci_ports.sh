#!/usr/bin/env bash
set -Eeuo pipefail

python3 - <<'PY'
import os
import socket

names = ("VAULT_PORT", "ADMIN_PORT", "NGINX_PORT")
ports = []
sockets = []
try:
    for name in names:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.bind(("127.0.0.1", 0))
        sockets.append(sock)
        ports.append((name, sock.getsockname()[1]))
finally:
    for sock in sockets:
        sock.close()

for name, port in ports:
    print(f"{name}={port}")

run_id = os.environ.get("GITHUB_RUN_ID", "local")
attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
print(f"COMPOSE_PROJECT_NAME=un1c0-e2e-{run_id}-{attempt}")
PY
