#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase27_replicated_delivery_ownership_integration -- --nocapture
printf '%s\n' 'Phase 27 replicated delivery gate passed: quorum acknowledgements, hash binding, cross-host ownership, failover retry, and stale-transfer rejection'
