#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase20_snapshot_install_readiness_integration -- --nocapture
cargo test --test phase21_snapshot_transfer_metrics_integration -- --nocapture
printf '%s\n' 'Phase 21 snapshot-transfer metrics gate passed: per-follower accounting, bounded bandwidth backpressure, complete install accounting, cancellation, retry, and clock safety'
