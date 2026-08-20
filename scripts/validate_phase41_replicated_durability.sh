#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase41.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/replicated_durability.rs \
  tests/phase41_replicated_durability_integration.rs \
  examples/phase41_replicated_durability_benchmark.rs
cargo test --test phase41_replicated_durability_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase41_replicated_durability_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 41' "$TMP_DIR/benchmark.json"
grep -q '"attempts": 64' "$TMP_DIR/benchmark.json"
grep -q '"completed_commits": 64' "$TMP_DIR/benchmark.json"
grep -q '"failed_commits": 0' "$TMP_DIR/benchmark.json"
grep -q '"final_generation": 64' "$TMP_DIR/benchmark.json"
grep -q '"quorum_per_commit": 2' "$TMP_DIR/benchmark.json"
grep -q '"durable_snapshot_round_trip": true' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"
grep -q '"cluster_mutation_performed": false' "$TMP_DIR/benchmark.json"

grep -q 'cas_writer_signature_required' scripts/collect_security_compliance_metrics.py
grep -q 'replicated_ack_quorum_required' scripts/collect_security_compliance_metrics.py
grep -q 'phase41_replicated_durability' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 193' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 41 replicated-durability validation passed.'
