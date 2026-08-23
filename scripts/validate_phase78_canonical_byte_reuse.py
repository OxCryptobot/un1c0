"""Validate the sanitized Phase 78 immutable canonical-byte reuse artifact."""

from __future__ import annotations

import json
import sys
from pathlib import Path

EXPECTED_FRAMES = [1, 2, 4, 8, 16, 32]


def main() -> int:
    artifact_path = (
        Path(sys.argv[1])
        if len(sys.argv) > 1
        else Path("benchmarks/phase78_canonical_byte_reuse.json")
    )
    data = json.loads(artifact_path.read_text())
    rows = data.get("rows", [])
    assert data.get("schema_version") == 1
    assert data.get("phase") == 78
    assert data.get("artifact") == "diagnostic_canonical_byte_reuse"
    assert data.get("samples") == 32
    assert data.get("frame_counts") == EXPECTED_FRAMES
    assert data.get("secret_material_recorded") is False
    assert data.get("errors") == 0
    assert [row["frames"] for row in rows] == EXPECTED_FRAMES
    for row in rows:
        assert row["payload_bytes"] > 0
        assert row["stream_bytes"] > 0
        assert row["canonical_payload_reuse_p50_ns"] <= row["canonical_payload_reuse_p95_ns"] <= row["canonical_payload_reuse_p99_ns"]
        assert row["canonical_json_reuse_p50_ns"] <= row["canonical_json_reuse_p95_ns"] <= row["canonical_json_reuse_p99_ns"]
        assert row["sha256_integrity_p50_ns"] <= row["sha256_integrity_p95_ns"] <= row["sha256_integrity_p99_ns"]
        assert row["full_verification_p50_ns"] <= row["full_verification_p95_ns"] <= row["full_verification_p99_ns"]
        assert row["warm_cache_admission_p50_ns"] <= row["warm_cache_admission_p95_ns"] <= row["warm_cache_admission_p99_ns"]
        assert row["sampled_canonical_stream_serialize_ns"] >= 0
        assert row["sampled_canonical_bytes_reuse_ns"] > 0
        assert row["sampled_content_hash_ns"] > 0
    print(
        "phase78_canonical_byte_reuse_gate=pass "
        f"rows={len(rows)} samples={data['samples']} errors={data['errors']} "
        f"frames={EXPECTED_FRAMES} secret_material_recorded={data['secret_material_recorded']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
