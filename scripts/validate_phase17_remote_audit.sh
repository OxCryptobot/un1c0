#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase17_remote_audit_integration -- --nocapture
printf '%s\n' 'Phase 17 remote-audit gate passed: authenticated envelopes, stream ordering, idempotent outbox, gap retention, and signed sink acknowledgements'
