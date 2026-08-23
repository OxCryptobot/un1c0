use serde_json::Value;
use un1c0::emission_diagnostic_instrumentation::{
    DiagnosticInstrumentation, DiagnosticInstrumentationSnapshot, DiagnosticTelemetryError,
    VerificationOutcome, DIAGNOSTIC_INSTRUMENTATION_VERSION, DIAGNOSTIC_TELEMETRY_EVENT_TYPE,
    DIAGNOSTIC_TELEMETRY_SCHEMA_VERSION, MAX_DIAGNOSTIC_TELEMETRY_SAMPLES,
};

fn snapshot_with_sample() -> DiagnosticInstrumentationSnapshot {
    let instrumentation = DiagnosticInstrumentation::enabled(4);
    instrumentation
        .recorder(4, 1024)
        .finish(VerificationOutcome::Accepted);
    instrumentation.snapshot()
}

fn walk_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                keys.push(key.clone());
                walk_keys(value, keys);
            }
        }
        Value::Array(values) => {
            for value in values {
                walk_keys(value, keys);
            }
        }
        _ => {}
    }
}

#[test]
fn versioned_telemetry_round_trips_with_allowlisted_redacted_fields() {
    let snapshot = snapshot_with_sample();
    let bytes = snapshot.to_versioned_json().unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["schema_version"], DIAGNOSTIC_TELEMETRY_SCHEMA_VERSION);
    assert_eq!(value["event_type"], DIAGNOSTIC_TELEMETRY_EVENT_TYPE);
    assert_eq!(
        value["snapshot"]["samples"][0]["schema_version"],
        DIAGNOSTIC_INSTRUMENTATION_VERSION
    );

    let mut keys = Vec::new();
    walk_keys(&value, &mut keys);
    let allowed = [
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
        "accepted_operations",
        "rejected_operations",
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
        "dropped_samples",
    ];
    assert!(keys.iter().all(|key| allowed.contains(&key.as_str())));

    let restored = DiagnosticInstrumentationSnapshot::from_versioned_json(&bytes).unwrap();
    assert_eq!(restored, snapshot);
}

#[test]
fn versioned_telemetry_rejects_unknown_version_and_noncanonical_input() {
    let snapshot = snapshot_with_sample();
    let bytes = snapshot.to_versioned_json().unwrap();

    let mut unknown: Value = serde_json::from_slice(&bytes).unwrap();
    unknown["unexpected"] = Value::Bool(true);
    let unknown_bytes = serde_json::to_vec(&unknown).unwrap();
    assert!(matches!(
        DiagnosticInstrumentationSnapshot::from_versioned_json(&unknown_bytes),
        Err(DiagnosticTelemetryError::Json(_))
    ));

    let mut nested_unknown: Value = serde_json::from_slice(&bytes).unwrap();
    nested_unknown["snapshot"]["samples"][0]["stages"]["raw_payload"] = Value::from("blocked");
    let nested_unknown_bytes = serde_json::to_vec(&nested_unknown).unwrap();
    assert!(matches!(
        DiagnosticInstrumentationSnapshot::from_versioned_json(&nested_unknown_bytes),
        Err(DiagnosticTelemetryError::Json(_))
    ));

    let mut wrong_event: Value = serde_json::from_slice(&bytes).unwrap();
    wrong_event["event_type"] = Value::from("other_event");
    let wrong_event_bytes = serde_json::to_vec(&wrong_event).unwrap();
    assert!(matches!(
        DiagnosticInstrumentationSnapshot::from_versioned_json(&wrong_event_bytes),
        Err(DiagnosticTelemetryError::UnexpectedEventType)
    ));

    let mut wrong_version: Value = serde_json::from_slice(&bytes).unwrap();
    wrong_version["schema_version"] = Value::from(99);
    let wrong_version_bytes = serde_json::to_vec(&wrong_version).unwrap();
    assert!(matches!(
        DiagnosticInstrumentationSnapshot::from_versioned_json(&wrong_version_bytes),
        Err(DiagnosticTelemetryError::UnsupportedSchemaVersion { version: 99 })
    ));

    let mut noncanonical = bytes.clone();
    noncanonical.push(b' ');
    assert!(matches!(
        DiagnosticInstrumentationSnapshot::from_versioned_json(&noncanonical),
        Err(DiagnosticTelemetryError::NonCanonicalEncoding)
    ));
}

#[test]
fn versioned_telemetry_enforces_sample_bounds_and_preserves_dropped_sample_observation() {
    let snapshot = snapshot_with_sample();
    let sample = snapshot.samples[0].clone();
    let mut too_many = snapshot.clone();
    too_many.samples = vec![sample.clone(); MAX_DIAGNOSTIC_TELEMETRY_SAMPLES + 1];
    assert!(matches!(
        too_many.to_versioned_json(),
        Err(DiagnosticTelemetryError::TooManySamples { .. })
    ));

    let mut invalid_frame = snapshot.clone();
    invalid_frame.samples[0].frame_count = 0;
    assert!(matches!(
        invalid_frame.to_versioned_json(),
        Err(DiagnosticTelemetryError::InvalidFrameCount { index: 0, count: 0 })
    ));

    let instrumentation = DiagnosticInstrumentation::enabled(1);
    instrumentation
        .recorder(1, 128)
        .finish(VerificationOutcome::Accepted);
    instrumentation
        .recorder(1, 128)
        .finish(VerificationOutcome::Rejected);
    let dropped = instrumentation.snapshot();
    assert_eq!(dropped.completed_operations, 2);
    assert_eq!(dropped.dropped_samples, 1);
    assert!(dropped.to_versioned_json().is_ok());
}
