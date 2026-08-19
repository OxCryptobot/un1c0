#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"
if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:/home/ubuntu/.cargo/bin:/tmp/helm-un1c0/linux-amd64:${PATH}"
else
  export PATH="/home/ubuntu/.cargo/bin:/tmp/helm-un1c0/linux-amd64:${PATH}"
fi
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

printf '%s\n' '== Phase 13 snapshot streaming and network stress =='
cargo test --test phase13_snapshot_sync_integration >"$TMP_DIR/phase13-snapshot.log"
cargo test --test phase13_transport_stress_integration >"$TMP_DIR/phase13-stress.log"
cargo run --quiet --release --bin un1c0-consensus-bench > benchmarks/consensus_partition_metrics.json
printf '%s\n' '== Phase 14 leader leases and linearizable reads =='
scripts/validate_phase14_read_optimization.sh >"$TMP_DIR/phase14-reads.log"
printf '%s\n' '== Phase 15 election timers and failure detectors =='
scripts/validate_phase15_election_timers.sh >"$TMP_DIR/phase15-timers.log"
printf '%s\n' '== Phase 16 replication flow control =='
scripts/validate_phase16_replication_flow_control.sh >"$TMP_DIR/phase16-flow-control.log"
printf '%s\n' '== Phase 17 remote audit ordering and outbox =='
scripts/validate_phase17_remote_audit.sh >"$TMP_DIR/phase17-remote-audit.log"
printf '%s\n' '== Phase 18 log compaction and configuration-bound snapshots =='
scripts/validate_phase18_log_compaction.sh >"$TMP_DIR/phase18-log-compaction.log"
printf '%s\n' '== Phase 19 durable compaction and recovery =='
scripts/validate_phase19_durable_compaction.sh >"$TMP_DIR/phase19-durable-compaction.log"
printf '%s\n' '== Phase 20 snapshot install readiness =='
scripts/validate_phase20_snapshot_install_readiness.sh >"$TMP_DIR/phase20-snapshot-readiness.log"
printf '%s\n' '== Phase 21 snapshot transfer metrics and cancellation =='
scripts/validate_phase21_snapshot_transfer_metrics.sh >"$TMP_DIR/phase21-snapshot-transfer-metrics.log"
printf '%s\n' '== Phase 22 durable term and epoch-bound replay =='
scripts/validate_phase22_durable_term_replay.sh >"$TMP_DIR/phase22-durable-term-replay.log"
printf '%s\n' '== Phase 23 compaction coordination and snapshot requests =='
scripts/validate_phase23_compaction_coordination.sh >"$TMP_DIR/phase23-compaction-coordination.log"
printf '%s\n' '== Phase 24 socket backpressure and per-peer quotas =='
scripts/validate_phase24_socket_backpressure.sh >"$TMP_DIR/phase24-socket-backpressure.log"
printf '%s\n' '== Phase 25 durable transport queues and quota recovery =='
scripts/validate_phase25_durable_transport_queue.sh >"$TMP_DIR/phase25-durable-transport-queue.log"
printf '%s\n' '== Phase 26 authenticated durable delivery and crash injection =='
scripts/validate_phase26_authenticated_delivery.sh >"$TMP_DIR/phase26-authenticated-delivery.log"
printf '%s\n' '== Phase 27 replicated delivery acknowledgements and ownership =='
scripts/validate_phase27_replicated_delivery_ownership.sh >"$TMP_DIR/phase27-replicated-delivery-ownership.log"
printf '%s\n' '== Phase 28 partition-aware ownership fencing =='
scripts/validate_phase28_partition_ownership_fencing.sh >"$TMP_DIR/phase28-partition-ownership-fencing.log"
printf '%s\n' '== Phase 29 authenticated remote ownership fencing =='
scripts/validate_phase29_authenticated_remote_fencing.sh >"$TMP_DIR/phase29-authenticated-remote-fencing.log"
printf '%s\n' '== Phase 30 deterministic multi-region failover =='
scripts/validate_phase30_multiregion_failover.sh >"$TMP_DIR/phase30-multiregion-failover.log"
printf '%s\n' '== Phase 31 secure deterministic replay =='
scripts/validate_phase31_secure_replay.sh >"$TMP_DIR/phase31-secure-replay.log"
printf '%s\n' '== Phase 32 disaster recovery and automated consensus failover =='
scripts/validate_phase32_disaster_recovery.sh >"$TMP_DIR/phase32-disaster-recovery.log"
printf '%s\n' '== Phase 33 durable recovery and observer-membership epochs =='
scripts/validate_phase33_durable_recovery.sh >"$TMP_DIR/phase33-durable-recovery.log"

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
