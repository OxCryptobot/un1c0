"""Scan versioned diagnostic telemetry artifacts for schema and redaction violations."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ALLOWED_KEYS = {
    "schema_version",
    "event_type",
    "snapshot",
    "enabled",
    "completed_operations",
    "accepted_operations",
    "rejected_operations",
    "dropped_samples",
    "counters",
    "samples",
    "frame_count",
    "stream_bytes",
    "outcome",
    "stages",
    "unattributed_ns",
    "end_to_end_ns",
    "transport_receive_ns",
    "transport_frame_integrity_ns",
    "stream_shape_ns",
    "snapshot_fingerprint_ns",
    "nested_report_verify_ns",
    "canonical_report_serialize_ns",
    "canonical_stream_serialize_ns",
    "canonical_bytes_reuse_ns",
    "content_hash_ns",
    "attestation_shape_ns",
    "trust_lookup_ns",
    "public_key_parse_ns",
    "signing_payload_serialize_ns",
    "ed25519_verify_ns",
    "aggregate_admission_ns",
    "evidence_cache_lookup_ns",
    "evidence_cache_insert_ns",
    "trust_lookups",
    "public_key_parses",
    "signature_verifications",
    "content_hashes",
    "frame_integrity_checks",
    "stale_snapshot_rejections",
    "replay_gap_rejections",
    "evidence_cache_hits",
    "evidence_cache_misses",
    "evidence_cache_invalidations",
}
FORBIDDEN = re.compile(
    r"private[_-]?(key|material|bytes)|secret[_-]?(value|material|bytes)|"
    r"token[_-]?(value|text|bytes)|signature[_-]?(value|text|bytes)|"
    r"public[_-]?key[_-]?(value|text|bytes|material)|source[_-]?(text|bytes)|"
    r"prompt[_-]?(text|content|bytes)|raw[_-]?(payload|diagnostic|source)|"
    r"credential|password|api[_-]?key",
    re.IGNORECASE,
)
ALLOWED_STRINGS = {
    "diagnostic_instrumentation_snapshot",
    "accepted",
    "rejected",
}


def walk(value: object, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            assert key in ALLOWED_KEYS, f"{path}: unexpected telemetry key {key!r}"
            assert not FORBIDDEN.search(key), f"{path}: sensitive telemetry key {key!r}"
            walk(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk(child, f"{path}[{index}]")
    elif isinstance(value, str):
        assert value in ALLOWED_STRINGS, f"{path}: unexpected telemetry string"


def scan(path: Path) -> None:
    data = json.loads(path.read_text())
    assert data["schema_version"] == 1
    assert data["event_type"] == "diagnostic_instrumentation_snapshot"
    walk(data)
    samples = data["snapshot"]["samples"]
    assert len(samples) <= 512
    print(f"redaction_scan=pass path={path} samples={len(samples)}")


def main() -> int:
    paths = [Path(argument) for argument in sys.argv[1:]]
    if not paths:
        paths = [Path("benchmarks/phase79_diagnostic_telemetry.json")]
    for path in paths:
        scan(path)
    print(f"redaction_scan_total={len(paths)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
