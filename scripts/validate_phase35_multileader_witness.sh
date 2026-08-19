#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase35_multileader_witness_integration -- --nocapture
cargo run --quiet --example phase35_multileader_witness_benchmark -- \
  --output benchmarks/phase35_multileader_witness_metrics.json
! grep -E '"public_key"|"signature"[[:space:]]*:|ExternalFencingToken' \
  benchmarks/phase35_multileader_witness_metrics.json
printf '%s\n' 'Phase 35 multi-leader gate passed: signed leader proposals, witness quorum arbitration, one-vote-per-round, conflicting-quorum split-brain rejection, stale-log rejection, domain-bound fencing, authority registry pinning, generation rollback rejection, and dynamic cross-region chaos'
