#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase38.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/external_fencing_supervision.rs \
  tests/phase38_external_fencing_supervision_integration.rs \
  examples/phase38_external_fencing_supervision_benchmark.rs
cargo test --test phase38_external_fencing_supervision_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase38_external_fencing_supervision_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase": 38' "$TMP_DIR/benchmark.json"
grep -q '"authority_heartbeat_admitted": true' "$TMP_DIR/benchmark.json"
grep -q '"ready_status": "Ready"' "$TMP_DIR/benchmark.json"
grep -q '"stale_status": "AuthorityStale"' "$TMP_DIR/benchmark.json"
grep -q '"snapshot_round_trip": true' "$TMP_DIR/benchmark.json"
grep -q '"journal_integrity": true' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded": false' "$TMP_DIR/benchmark.json"

grep -q 'fencing_authority_heartbeat_signature_required' scripts/collect_security_compliance_metrics.py
grep -q 'fence_consumer_ack_exact_binding' scripts/collect_security_compliance_metrics.py
grep -q 'phase38_external_fencing_supervision' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 177' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 38 external-fencing supervision validation passed.'
