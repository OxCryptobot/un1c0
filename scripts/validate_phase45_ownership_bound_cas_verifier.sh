#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase45.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/replicated_durability.rs \
  src/ownership_bound_cas.rs \
  src/ownership_bound_cas_verifier.rs \
  tests/phase45_ownership_bound_cas_verifier_integration.rs \
  examples/phase45_ownership_bound_cas_verifier_benchmark.rs
cargo test --test phase41_replicated_durability_integration \
  --test phase42_cross_process_ownership_integration \
  --test phase43_ownership_bound_cas_integration \
  --test phase44_ownership_bound_cas_executor_integration \
  --test phase45_ownership_bound_cas_verifier_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase45_ownership_bound_cas_verifier_benchmark >"$TMP_DIR/benchmark.json"
python3 - "$TMP_DIR/benchmark.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
assert report["phase"] == 45
assert report["jobs_per_producer"] == 16
assert report["mutation_workers"] == 1
assert report["secret_material_recorded"] is False
assert report["cluster_mutation_performed"] is False
assert [row["producers"] for row in report["results"]] == [1, 2, 4, 8, 16]
for row in report["results"]:
    assert row["jobs"] == row["producers"] * 16
    assert row["successful_commits"] == 1
    assert row["failed_conflicts"] == row["jobs"] - 1
    assert row["verification_queue_full_rejections"] == 0
    assert row["latency_sample_count"] == row["jobs"]
    assert row["latency_sample_count"] <= row["latency_sample_cap"]
    assert row["verification_service_p95_us"] >= row["verification_service_p50_us"]
    assert row["end_to_end_p95_us"] >= row["end_to_end_p50_us"]
    assert row["throughput_intents_per_sec"] >= 0
PY

grep -q 'parallel_pre_admission_workers' scripts/collect_security_compliance_metrics.py
grep -q 'ordered_mutation_dispatch' scripts/collect_security_compliance_metrics.py
grep -q 'phase45_ownership_bound_cas_verifier' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 193' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 45 ownership-bound CAS verifier validation passed.'
