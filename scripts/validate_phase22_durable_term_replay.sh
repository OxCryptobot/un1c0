#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase22_durable_term_replay_integration -- --nocapture
printf '%s\n' 'Phase 22 durable term/replay gate passed: atomic term-vote state, staging recovery, rollback protection, epoch-bound envelopes, term floors, and bounded replay windows'
