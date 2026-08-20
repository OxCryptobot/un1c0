#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase37.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check src/telemetry_failover.rs tests/phase37_telemetry_failover_integration.rs examples/phase37_telemetry_failover_benchmark.rs
cargo test --test phase36_recovery_transport_integration --test phase37_telemetry_failover_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase37_telemetry_failover_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 37' "$TMP_DIR/benchmark.json"
grep -q '"journal_hash_chain_valid": true' "$TMP_DIR/benchmark.json"
grep -q '"failover_phase": "Committed"' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"
[[ $(grep -c '"safety_passed": true' "$TMP_DIR/benchmark.json") -ge 2 ]]

grep -q 'consensus_telemetry_signature_required' scripts/collect_security_compliance_metrics.py
grep -q 'reservation_store_fuzz_no_panic' scripts/collect_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 193' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 37 telemetry, failover, and epoch-churn fuzz validation passed.'
