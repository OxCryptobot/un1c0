#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase20_snapshot_install_readiness_integration -- --nocapture
printf '%s\n' 'Phase 20 snapshot install-readiness gate passed: bounded transfer lifecycle, exact ack binding, installed-only progress, retry, term, and clock safety'
