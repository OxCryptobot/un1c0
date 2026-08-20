#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase44.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/ownership_bound_cas_executor.rs \
  tests/phase44_ownership_bound_cas_executor_integration.rs \
  examples/phase44_ownership_bound_cas_executor_benchmark.rs
cargo test --test phase42_cross_process_ownership_integration \
  --test phase43_ownership_bound_cas_integration \
  --test phase44_ownership_bound_cas_executor_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase44_ownership_bound_cas_executor_benchmark >"$TMP_DIR/benchmark.json"
python3 - "$TMP_DIR/benchmark.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
assert report["phase"] == 44
assert report["jobs_per_producer"] == 16
assert report["quorum"] == 2
assert report["secret_material_recorded"] is False
assert report["cluster_mutation_performed"] is False
assert [row["producers"] for row in report["results"]] == [1, 2, 4, 8, 16]
for row in report["results"]:
    assert row["jobs"] == row["producers"] * 16
    assert row["successful_commits"] == 1
    assert row["failed_conflicts"] == row["jobs"] - 1
    assert row["queue_full_rejections"] == 0
    assert row["latency_sample_count"] == row["jobs"]
    assert row["latency_sample_count"] <= row["latency_sample_cap"]
    assert row["end_to_end_p95_us"] >= row["end_to_end_p50_us"]
    assert row["throughput_intents_per_sec"] >= 0
PY

grep -q 'bounded_executor_queue' scripts/collect_security_compliance_metrics.py
grep -q 'worker_owned_mutation' scripts/collect_security_compliance_metrics.py
grep -q 'phase44_ownership_bound_cas_executor' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 185' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 44 ownership-bound CAS executor validation passed.'
