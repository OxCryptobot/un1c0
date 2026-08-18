#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -x /home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo ]]; then
  export PATH="/home/ubuntu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
fi

cargo test --test phase14_linearizable_reads_integration -- --nocapture

python3 - <<'PY'
import json
from pathlib import Path

path = Path("benchmarks/phase14_read_benchmark.json")
rows = json.loads(path.read_text())
expected_concurrency = [1, 2, 4, 8, 16, 32]
expected_paths = {"lease_fast_path", "quorum_read_index"}
assert len(rows) == len(expected_concurrency) * len(expected_paths), len(rows)
assert {row["path"] for row in rows} == expected_paths
assert sorted({row["concurrency"] for row in rows}) == expected_concurrency
for row in rows:
    expected_operations = row["concurrency"] * 128
    assert row["operations"] == expected_operations, row
    assert row["successful"] == expected_operations, row
    assert row["errors"] == 0, row
    assert 0 <= row["p50_us"] <= row["p95_us"] <= row["p99_us"], row
    assert row["throughput_ops_per_sec"] > 0, row
print(f"Phase 14 benchmark gate passed: {len(rows)} rows, {sum(r['operations'] for r in rows)} reads, zero errors")
PY
