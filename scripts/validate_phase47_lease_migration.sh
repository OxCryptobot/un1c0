#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
TMP_DIR=$(mktemp -d /tmp/un1c0-phase47.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

TOOLCHAIN=/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin
export PATH="$TOOLCHAIN:/home/ubuntu/.cargo/bin:$PATH"

rustfmt --edition 2021 --check \
  src/lease_migration.rs \
  tests/phase47_lease_migration_integration.rs \
  examples/phase47_memory_profile_benchmark.rs \
  examples/phase47_lease_migration_benchmark.rs

cargo test --lib lease_migration >"$TMP_DIR/lib.log"
cargo test --test phase47_lease_migration_integration >"$TMP_DIR/integration.log"
cargo run --quiet --example phase47_lease_migration_benchmark >"$TMP_DIR/migration.json"
jq -e '
  (.phase == 47) and
  (.rounds_completed == .rounds) and
  (.final_state == "Activated") and
  (.final_epoch > 1) and
  (.secret_material_recorded == false) and
  (.cluster_mutation_performed == false)
' "$TMP_DIR/migration.json" >/dev/null
cargo run --quiet --example phase47_memory_profile_benchmark >"$TMP_DIR/memory.json"
jq -e '
  (.phase == 47) and
  (.results | length == 3) and
  ([.results[] | .producers] == [32, 64, 96]) and
  ([.results[] | .peak_rss_kb > 0 and .peak_hwm_kb > 0 and .peak_vm_peak_kb > 0 and .peak_threads > 0]) and
  ([.results[] | .secret_material_recorded == false and .cluster_mutation_performed == false])
' "$TMP_DIR/memory.json" >/dev/null

python3 scripts/collect_security_compliance_metrics.py --output "$TMP_DIR/compliance.json"
python3 scripts/audit_security_compliance_metrics.py --artifact "$TMP_DIR/compliance.json" >"$TMP_DIR/audit.log"
grep -q 'phase47_lease_migration' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 209' scripts/audit_security_compliance_metrics.py

printf '%s\n' 'Phase 47 lease migration validation passed.'
