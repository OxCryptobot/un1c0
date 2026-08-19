#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase33_durable_recovery_integration -- --nocapture
cargo run --quiet --example phase33_durable_recovery_benchmark -- \
  --output benchmarks/phase33_durable_recovery_metrics.json
printf '%s\n' 'Phase 33 durable-recovery gate passed: hash-bound snapshots, atomic restart recovery, partial-staging cleanup, observer-membership epochs, stale-epoch rejection, and concurrent partition-race single-commit arbitration'
