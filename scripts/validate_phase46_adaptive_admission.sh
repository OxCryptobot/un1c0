#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:${PATH}"
fi
TMP_DIR=$(mktemp -d /tmp/un1c0-phase46.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

rustfmt --edition 2021 --check \
  src/replicated_durability.rs \
  src/ownership_bound_cas.rs \
  src/ownership_bound_cas_verifier.rs \
  src/ownership_bound_cas_admission.rs \
  tests/phase46_adaptive_admission_integration.rs \
  examples/phase46_adaptive_admission_benchmark.rs
cargo test --test phase41_replicated_durability_integration \
  --test phase42_cross_process_ownership_integration \
  --test phase43_ownership_bound_cas_integration \
  --test phase44_ownership_bound_cas_executor_integration \
  --test phase45_ownership_bound_cas_verifier_integration \
  --test phase46_adaptive_admission_integration >"$TMP_DIR/tests.log"
cargo run --quiet --example phase46_adaptive_admission_benchmark >"$TMP_DIR/benchmark.json"
python3 - "$TMP_DIR/benchmark.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
assert report["phase"] == 46
assert report["mutation_workers"] == 1
assert report["secret_material_recorded"] is False
assert report["cluster_mutation_performed"] is False
assert [row["producers"] for row in report["results"]] == [1, 2, 4, 8, 16, 32]
for row in report["results"]:
    assert row["jobs"] == row["producers"] * 8
    assert row["successful_commits"] == 1
    assert row["failed_conflicts"] == row["jobs"] - 1
    assert row["adaptive"]["limiter_rejections"] >= 0
    assert row["adaptive"]["permits"] >= row["adaptive"]["minimum_permits"]
    assert row["adaptive"]["permits"] <= row["adaptive"]["maximum_permits"]
    assert row["adaptive"]["service_sample_count"] <= row["adaptive"]["service_sample_cap"]
    cache_hits = row["verifier"]["verification_cache_hits"]
    cache_misses = row["verifier"]["verification_cache_misses"]
    assert cache_misses >= 3
    assert cache_hits > 0
    assert cache_hits + cache_misses == row["jobs"] * 3
    assert row["verifier"]["verification_cache_entries"] <= 2048
    assert row["verifier"]["latency_sample_count"] <= row["verifier"]["latency_sample_cap"]
    assert row["throughput_intents_per_sec"] >= 0
PY

grep -q 'adaptive_limiter_bounded' scripts/collect_security_compliance_metrics.py
grep -q 'context_fingerprint_bound_cache' scripts/collect_security_compliance_metrics.py
grep -q 'phase46_ownership_bound_cas_admission' scripts/audit_security_compliance_metrics.py
grep -q 'EXPECTED_GATE_COUNT = 201' scripts/audit_security_compliance_metrics.py
printf '%s\n' 'Phase 46 adaptive admission validation passed.'
