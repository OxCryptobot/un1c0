#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase36_recovery_transport_integration -- --nocapture
cargo run --quiet --example phase36_recovery_transport_benchmark -- \
  --output benchmarks/phase36_recovery_transport_metrics.json
! grep -E '"public_key"|"signature"[[:space:]]*:|ExternalFencingToken' \
  benchmarks/phase36_recovery_transport_metrics.json
printf '%s\n' 'Phase 36 authenticated transport gate passed: signed envelopes, receiver binding, connection-epoch replay windows, durable witness reservations, crash-boundary recovery, exact protected-write fencing, and cross-host duplicate/drop chaos'
