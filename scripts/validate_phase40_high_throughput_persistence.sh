#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase40.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/resource_durability.rs \
  tests/phase40_high_throughput_persistence_integration.rs \
  examples/phase40_high_throughput_persistence_benchmark.rs
cargo test --test phase39_resource_durability_integration --test phase40_high_throughput_persistence_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase40_high_throughput_persistence_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 40' "$TMP_DIR/benchmark.json"
grep -q '"operation_count": 128' "$TMP_DIR/benchmark.json"
grep -q '"completed_operations": 128' "$TMP_DIR/benchmark.json"
grep -q '"failed_operations": 0' "$TMP_DIR/benchmark.json"
grep -q '"unique_target_count": 128' "$TMP_DIR/benchmark.json"
grep -q '"staging_recovery_scans": 4' "$TMP_DIR/benchmark.json"
grep -q '"throughput_milli_ops_per_sec":' "$TMP_DIR/benchmark.json"
grep -q '"resource_during_workers"' "$TMP_DIR/benchmark.json"
grep -q '"within_budget": true' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"
grep -q '"cluster_mutation_performed": false' "$TMP_DIR/benchmark.json"

grep -q 'concurrent_persistence_bounds' scripts/collect_security_compliance_metrics.py
grep -q 'active_worker_resource_snapshot' scripts/collect_security_compliance_metrics.py
grep -q 'phase40_high_throughput_persistence' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 169' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 40 high-throughput persistence validation passed.'
