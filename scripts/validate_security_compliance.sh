#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
export PATH="/home/ubuntu/.cargo/bin:/tmp/helm-un1c0/linux-amd64:${PATH}"
OUTPUT=${SECURITY_COMPLIANCE_OUTPUT:-"$ROOT_DIR/benchmarks/security_compliance_metrics.json"}
TMP_DIR=$(mktemp -d /tmp/un1c0-compliance.XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

printf '%s\n' '== reusable skill =='
python3 /home/ubuntu/skills/skill-creator/scripts/quick_validate.py agentic-system-engineering >"$TMP_DIR/skill.log"

printf '%s\n' '== syntax and Rust/Python tests =='
bash -n scripts/*.sh vault/*.sh
cargo test --all-targets >"$TMP_DIR/cargo.log"
PYTHONPATH=. python3 -m pytest -q >"$TMP_DIR/pytest.log"
python3 -m py_compile benchmarks/*.py scripts/*.py vault/admin_service/app.py

printf '%s\n' '== CLI smoke =='
cargo run --quiet --bin un1c0-agent -- plan "security compliance validation" >"$TMP_DIR/cli.log"
cargo run --quiet --bin un1c0-agent -- tools >>"$TMP_DIR/cli.log"
grep -q 'list_files' "$TMP_DIR/cli.log"

printf '%s\n' '== Helm fail-closed =='
scripts/validate_helm_security.sh >"$TMP_DIR/helm.log"

printf '%s\n' '== focused Phase 10 integration =='
cargo test --test phase10_security_integration >"$TMP_DIR/phase10.log"

printf '%s\n' '== Phase 11 membership and failure injection =='
cargo test --test phase11_consensus_integration >"$TMP_DIR/phase11.log"
cargo test --test failure_injection_integration >"$TMP_DIR/failure-injection.log"

printf '%s\n' '== Phase 12 authenticated socket transport =='
cargo test --test phase12_transport_integration >"$TMP_DIR/phase12.log"
cargo run --quiet --release --bin un1c0-consensus-bench > benchmarks/consensus_partition_metrics.json

printf '%s\n' '== isolated Compose mTLS =='
CONTAINER_RUNTIME=${CONTAINER_RUNTIME:-podman} PODMAN_SUDO=${PODMAN_SUDO:-1} \
  scripts/validate_compose_smoke.sh >"$TMP_DIR/compose.log"

printf '%s\n' '== metrics report =='
python3 scripts/collect_security_compliance_metrics.py --output "$OUTPUT"

printf '%s\n' '== compliance summary =='
tail -1 "$TMP_DIR/pytest.log"
tail -1 "$TMP_DIR/helm.log"
tail -1 "$TMP_DIR/compose.log"
printf 'Security compliance validation passed; metrics=%s\n' "$OUTPUT"
