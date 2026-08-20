#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase43.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/ownership_bound_cas.rs \
  tests/phase43_ownership_bound_cas_integration.rs \
  examples/phase43_ownership_bound_cas_benchmark.rs
cargo test --test phase42_cross_process_ownership_integration >"$TMP_DIR/phase42.log"
cargo test --test phase43_ownership_bound_cas_integration >"$TMP_DIR/phase43.log"
cargo run --quiet --example phase43_ownership_bound_cas_benchmark >"$TMP_DIR/benchmark.json"
grep -q '"phase":43' "$TMP_DIR/benchmark.json"
grep -q '"ownership_bound_commits":32' "$TMP_DIR/benchmark.json"
grep -q '"final_generation":32' "$TMP_DIR/benchmark.json"
grep -q '"ownership_epoch":1' "$TMP_DIR/benchmark.json"
grep -q '"quorum":2' "$TMP_DIR/benchmark.json"
grep -q '"secret_material_recorded":false' "$TMP_DIR/benchmark.json"
grep -q '"cluster_mutation_performed":false' "$TMP_DIR/benchmark.json"

grep -q 'ownership_bound_cas_permit_required' scripts/collect_security_compliance_metrics.py
grep -q 'cas_request_epoch_exactly_bound' scripts/collect_security_compliance_metrics.py
grep -q 'idempotent_retry_preserves_ownership' scripts/collect_security_compliance_metrics.py
grep -q 'phase43_ownership_bound_cas' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 177' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 43 ownership-bound CAS validation passed.'
