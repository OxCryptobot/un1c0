#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase30_multiregion_failover_integration -- --nocapture
cargo run --quiet --example phase30_failover_benchmark -- --output benchmarks/phase30_multiregion_failover_metrics.json >/dev/null
printf '%s\n' 'Phase 30 multi-region failover gate passed: deterministic topology, replayable partitions, safety invariants, clock fencing, crash recovery, and quorum retry'
