#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase28_partition_ownership_fencing_integration -- --nocapture
printf '%s\n' 'Phase 28 partition ownership fencing gate passed: quorum-loss fencing, lease-expiry fencing, restart durability, and transfer fence clearing'
