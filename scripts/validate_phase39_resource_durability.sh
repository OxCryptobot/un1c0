#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase39.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/resource_durability.rs \
  tests/phase39_resource_durability_integration.rs \
  examples/phase39_resource_durability_benchmark.rs
cargo test --test phase38_external_fencing_supervision_integration --test phase39_resource_durability_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase39_resource_durability_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 39' "$TMP_DIR/benchmark.json"
grep -q '"valid_path_workload": true' "$TMP_DIR/benchmark.json"
grep -q '"within_budget": true' "$TMP_DIR/benchmark.json"
grep -q '"bytes_written": 262144' "$TMP_DIR/benchmark.json"
grep -q '"staging_retries": 0' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"
grep -q '"cluster_mutation_performed": false' "$TMP_DIR/benchmark.json"

grep -q 'authority_owner_region_signed' scripts/collect_security_compliance_metrics.py
grep -q 'file_fsync_latency_recorded' scripts/collect_security_compliance_metrics.py
grep -q 'phase39_resource_durability' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 161' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 39 resource and durability validation passed.'
