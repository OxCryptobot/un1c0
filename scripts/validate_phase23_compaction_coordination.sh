#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase23_compaction_coordination_snapshot_request_integration -- --nocapture
printf '%s\n' 'Phase 23 compaction/snapshot-request gate passed: quorum coordination, no-mutation waiting, follower requests, binding, retry identity, and stale rejection'
