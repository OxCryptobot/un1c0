#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase34_replicated_recovery_integration -- --nocapture
cargo run --quiet --example phase34_replicated_recovery_benchmark -- \
  --output benchmarks/phase34_replicated_recovery_metrics.json
printf '%s\n' 'Phase 34 replicated-recovery gate passed: joint observer quorum, finalization ordering, hash-bound authority log, signed external fencing tokens, monotonic fencing, stale-token rejection, restart continuity, and dynamic partition epoch chaos safety'
