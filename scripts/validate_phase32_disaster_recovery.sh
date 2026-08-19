#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase32_disaster_recovery_integration -- --nocapture
printf '%s\n' 'Phase 32 disaster-recovery gate passed: signed observations, distinct quorum, snapshot binding, higher-term promotion, fencing, single-active invariant, idempotence, and stale-state rejection'
