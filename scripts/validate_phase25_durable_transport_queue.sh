#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase25_durable_transport_queue_integration -- --nocapture
printf '%s\n' 'Phase 25 durable transport queue gate passed: atomic persistence, restart quota recovery, FIFO acknowledgement, epoch binding, and rollback'
