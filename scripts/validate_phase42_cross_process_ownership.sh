#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase42.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/cross_process_ownership.rs \
  tests/phase42_cross_process_ownership_integration.rs \
  examples/phase42_cross_process_ownership_benchmark.rs
cargo test --test phase42_cross_process_ownership_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase42_cross_process_ownership_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 42' "$TMP_DIR/benchmark.json"
grep -q '"ownership_cycles": 32' "$TMP_DIR/benchmark.json"
grep -q '"acquisitions": 32' "$TMP_DIR/benchmark.json"
grep -q '"lease_write_permits": 32' "$TMP_DIR/benchmark.json"
grep -q '"recovery_evidence_count": 2' "$TMP_DIR/benchmark.json"
grep -q '"recovery_state": "Recovered"' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"
grep -q '"cluster_mutation_performed": false' "$TMP_DIR/benchmark.json"

grep -q 'ownership_claim_signature_required' scripts/collect_security_compliance_metrics.py
grep -q 'managed_recovery_distinct_quorum' scripts/collect_security_compliance_metrics.py
grep -q 'phase42_cross_process_ownership' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 169' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 42 cross-process ownership validation passed.'
