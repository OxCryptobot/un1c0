#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase31_secure_replay_integration -- --nocapture
printf '%s\n' 'Phase 31 secure replay gate passed: signed manifest, schedule binding, sequence/tick bounds, trusted key/epoch binding, tamper rejection, and trace seal verification'
