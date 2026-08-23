use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_serialization::{
    EmissionDiagnosticSerializationError, MAX_SERIALIZED_DIAGNOSTIC_BYTES,
};
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn source(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn unit() -> SemanticUnitId {
    SemanticUnitId::new("workspace/unit.ueg").expect("valid unit")
}

fn leaf_range(ueg: &Ueg) -> SemanticEditRange {
    let NodeKind::Lambda(lambda) = &ueg.nodes[0];
    SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap()
}

fn prepared() -> (
    EmissionDiagnosticReport,
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let id = unit();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session.manifest_for(&id, vec![leaf_range(&base)]).unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: id.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&batch_envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(id, changed)]);
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    (report, snapshot, profile, candidates)
}

fn value_from_report(report: &EmissionDiagnosticReport) -> Value {
    serde_json::from_slice(&report.to_json().unwrap()).unwrap()
}

fn reseal(bytes: Vec<u8>, needle: &str, replacement: &str) -> Vec<u8> {
    let text = String::from_utf8(bytes)
        .unwrap()
        .replace(needle, replacement);
    reseal_text(text)
}

fn reseal_text(text: String) -> Vec<u8> {
    let marker = ",\"integrity_digest\":";
    let digest_start = text.find(marker).unwrap() + marker.len();
    let digest_end = text[digest_start..].find(']').unwrap() + digest_start + 1;
    let zero_digest = serde_json::to_string(&vec![0u8; 32]).unwrap();
    let payload = format!(
        "{}{}{}",
        &text[..digest_start],
        zero_digest,
        &text[digest_end..]
    );
    let mut hasher = Sha256::new();
    hasher.update(b"un1c0/phase69/emission-diagnostic/v1");
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize().to_vec();
    let digest_json = serde_json::to_string(&digest).unwrap();
    format!(
        "{}{}{}",
        &text[..digest_start],
        digest_json,
        &text[digest_end..]
    )
    .into_bytes()
}

#[test]
fn canonical_round_trip_rehydrates_only_after_current_verification() {
    let (report, snapshot, profile, candidates) = prepared();
    let bytes = report.to_json().unwrap();
    assert!(bytes.len() < MAX_SERIALIZED_DIAGNOSTIC_BYTES);
    assert!(!String::from_utf8_lossy(&bytes).contains("value + 2"));
    let restored =
        EmissionDiagnosticReport::from_json_for(&bytes, &snapshot, &profile, &candidates).unwrap();
    assert_eq!(restored, report);
}

#[test]
fn inconsistent_observation_counts_are_rejected_without_payload_growth() {
    let (report, snapshot, profile, candidates) = prepared();
    for observations in [1usize, 2, 4, 8, 16, 32] {
        let bytes = reseal(
            report.to_json().unwrap(),
            "\"observations\":1",
            &format!("\"observations\":{observations}"),
        );
        let restored =
            EmissionDiagnosticReport::from_json_for(&bytes, &snapshot, &profile, &candidates);
        if observations == 1 {
            assert!(restored.is_ok());
        } else {
            assert!(matches!(
                restored,
                Err(EmissionDiagnosticSerializationError::NonCanonicalEntries)
            ));
        }
    }
}

#[test]
fn parser_enforces_unit_and_entry_count_limits() {
    let (report, snapshot, profile, candidates) = prepared();
    let text = String::from_utf8(report.to_json().unwrap()).unwrap();
    let root = serde_json::to_string(&vec![0u8; 32]).unwrap();
    let mut extras = String::new();
    for index in 0..=256 {
        extras.push_str(&format!(",\"workspace/zextra_{index:03}\":{root}"));
    }
    let unit_text = text.replace("},\"chunks_emitted\":", &(extras + "},\"chunks_emitted\":"));
    let unit_bytes = reseal_text(unit_text);
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&unit_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::TooManyUnits { .. })
    ));

    let entry = serde_json::to_string(&report.entries()[0]).unwrap();
    let entry_text = text.replace(
        "],\"integrity_digest\":",
        &format!(",{entry}],\"integrity_digest\":"),
    );
    let entry_bytes = reseal_text(entry_text);
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&entry_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::InvalidEntryCount { .. })
    ));
}

#[test]
fn serialization_is_deterministic_and_malformed_input_is_typed() {
    let (report, snapshot, profile, candidates) = prepared();
    let first = report.to_json().unwrap();
    let second = report.to_json().unwrap();
    assert_eq!(first, second);
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(b"not-json", &snapshot, &profile, &candidates,),
        Err(EmissionDiagnosticSerializationError::Json(_))
    ));
}

#[test]
fn parser_rejects_integrity_tampering_before_rehydration() {
    let (report, snapshot, profile, candidates) = prepared();
    let mut tampered = report.to_json().unwrap();
    let index = tampered
        .iter()
        .position(|byte| *byte == b'1')
        .expect("serialized envelope contains a numeric byte");
    tampered[index] = b'2';
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&tampered, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::IntegrityMismatch)
            | Err(EmissionDiagnosticSerializationError::Json(_))
    ));
}

#[test]
fn parser_rejects_noncanonical_unknown_and_oversized_envelopes() {
    let (report, snapshot, profile, candidates) = prepared();
    let bytes = report.to_json().unwrap();
    let pretty = serde_json::to_vec_pretty(&value_from_report(&report)).unwrap();
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&pretty, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::Report(_))
            | Err(EmissionDiagnosticSerializationError::Json(_))
    ));

    let mut unknown = value_from_report(&report);
    unknown["unexpected"] = Value::Bool(true);
    let unknown_bytes = serde_json::to_vec(&unknown).unwrap();
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&unknown_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::Json(_))
    ));

    let oversized = vec![b'x'; MAX_SERIALIZED_DIAGNOSTIC_BYTES + 1];
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&oversized, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::EnvelopeTooLarge { .. })
    ));

    assert!(!bytes.is_empty());
}

#[test]
fn parser_rejects_identity_drift_invalid_ids_zero_observations_and_stale_state() {
    let (report, snapshot, profile, candidates) = prepared();

    let target_bytes = reseal(
        report.to_json().unwrap(),
        "\"target\":\"rust\"",
        "\"target\":\"zig\"",
    );
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&target_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::InvalidTarget)
    ));

    let batch_bytes = reseal(
        report.to_json().unwrap(),
        "\"batch_id\":1",
        "\"batch_id\":2",
    );
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&batch_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::Report(_))
    ));

    let text = String::from_utf8(report.to_json().unwrap()).unwrap();
    let roots_start = text.find("\"unit_roots\":").unwrap();
    let chunks_start = text.find(",\"chunks_emitted\":").unwrap();
    let missing_units = reseal_text(format!(
        "{}\"unit_roots\":{{}}{}",
        &text[..roots_start],
        &text[chunks_start..]
    ));
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&missing_units, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::InvalidEnvelope)
            | Err(EmissionDiagnosticSerializationError::Report(_))
    ));

    let invalid_id_bytes = reseal(report.to_json().unwrap(), "workspace/unit.ueg", "../escape");
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(
            &invalid_id_bytes,
            &snapshot,
            &profile,
            &candidates
        ),
        Err(EmissionDiagnosticSerializationError::Report(_))
            | Err(EmissionDiagnosticSerializationError::InvalidUnitId)
    ));

    let zero_bytes = reseal(
        report.to_json().unwrap(),
        "\"observations\":1",
        "\"observations\":0",
    );
    assert!(matches!(
        EmissionDiagnosticReport::from_json_for(&zero_bytes, &snapshot, &profile, &candidates),
        Err(EmissionDiagnosticSerializationError::InvalidObservationCount)
    ));

    let stale = BTreeMap::from([(unit(), parse(&source("value + 9")))]);
    let error = EmissionDiagnosticReport::from_json_for(
        &report.to_json().unwrap(),
        &snapshot,
        &profile,
        &stale,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        EmissionDiagnosticSerializationError::Report(_)
    ));
}
