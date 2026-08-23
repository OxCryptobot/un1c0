use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_serialization::MAX_SERIALIZED_DIAGNOSTIC_BYTES;
use un1c0::emission_diagnostic_stream::{
    EmissionDiagnosticStream, EmissionDiagnosticStreamError, MAX_STREAM_BYTES, MAX_STREAM_FRAMES,
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

const STREAM_DOMAIN: &[u8] = b"un1c0/phase70/emission-diagnostic-stream/v1";

struct Fixture {
    receipt: un1c0::EmissionReceipt,
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
}

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

fn prepared() -> Fixture {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit = SemanticUnitId::new("workspace/unit.ueg").unwrap();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
    let NodeKind::Lambda(lambda) = &base.nodes[0];
    let range =
        SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap();
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit.clone(),
            ueg: base,
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session.manifest_for(&unit, vec![range]).unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: unit.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&batch_envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(unit, changed)]);
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    Fixture {
        receipt,
        snapshot,
        profile,
        candidates,
    }
}

fn report(fixture: &Fixture) -> EmissionDiagnosticReport {
    EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&fixture.receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn stream(fixture: &Fixture, frame_count: usize) -> EmissionDiagnosticStream {
    let report = report(fixture);
    let reports = vec![report; frame_count];
    EmissionDiagnosticStream::from_verified_reports(
        70,
        &reports,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn reseal_stream(text: String) -> Vec<u8> {
    let marker = ",\"stream_digest\":";
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
    hasher.update(STREAM_DOMAIN);
    hasher.update(payload.as_bytes());
    let digest_json = serde_json::to_string(&hasher.finalize().to_vec()).unwrap();
    format!(
        "{}{}{}",
        &text[..digest_start],
        digest_json,
        &text[digest_end..]
    )
    .into_bytes()
}

#[test]
fn deterministic_property_round_trips_frame_counts_through_32() {
    let fixture = prepared();
    for frame_count in 1..=MAX_STREAM_FRAMES {
        let stream = stream(&fixture, frame_count);
        let summary = stream.summary();
        assert_eq!(summary.frame_count, frame_count);
        assert_eq!(summary.first_sequence, 1);
        assert_eq!(summary.last_sequence, frame_count as u64);
        assert!(summary.total_frame_bytes <= MAX_STREAM_BYTES);
        let bytes = stream.to_json().unwrap();
        assert!(bytes.len() <= MAX_STREAM_BYTES);
        assert!(!String::from_utf8_lossy(&bytes).contains("value + 2"));
        let restored = EmissionDiagnosticStream::from_json_for(
            &bytes,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
        assert_eq!(restored, stream);
    }
}

#[test]
fn verified_template_matches_repeated_report_stream_and_rejects_stale_state() {
    let fixture = prepared();
    let report = report(&fixture);
    let template = un1c0::EmissionDiagnosticStreamTemplate::from_report(
        &report,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    assert_eq!(
        template.encoded_bytes(),
        report.to_json().unwrap().as_slice()
    );
    let templated = template
        .build(
            70,
            MAX_STREAM_FRAMES,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    let repeated = EmissionDiagnosticStream::from_repeated_report(
        70,
        &report,
        MAX_STREAM_FRAMES,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    assert_eq!(templated, repeated);

    let stale = BTreeMap::from([(
        SemanticUnitId::new("workspace/unit.ueg").unwrap(),
        parse(&source("value + 9")),
    )]);
    assert!(matches!(
        template.build(
            70,
            MAX_STREAM_FRAMES,
            &fixture.snapshot,
            &fixture.profile,
            &stale,
        ),
        Err(EmissionDiagnosticStreamError::Nested { .. })
    ));
}

#[test]
fn rejects_empty_zero_and_excessive_streams() {
    let fixture = prepared();
    let report = report(&fixture);
    assert!(matches!(
        EmissionDiagnosticStream::from_verified_reports(
            70,
            &[],
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::Empty)
    ));
    assert!(matches!(
        EmissionDiagnosticStream::from_verified_reports(
            0,
            std::slice::from_ref(&report),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::InvalidStreamId)
    ));
    let reports = vec![report; MAX_STREAM_FRAMES + 1];
    assert!(matches!(
        EmissionDiagnosticStream::from_verified_reports(
            70,
            &reports,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::TooManyFrames { .. })
    ));
}

#[test]
fn rejects_sequence_context_and_nested_stale_state_without_partial_output() {
    let fixture = prepared();
    let bytes = stream(&fixture, 4).to_json().unwrap();

    let sequence = reseal_stream(
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("\"sequence\":2", "\"sequence\":3"),
    );
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &sequence,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::SequenceMismatch { .. })
    ));

    let context = reseal_stream(String::from_utf8(bytes.clone()).unwrap().replacen(
        "\"target\":\"rust\"",
        "\"target\":\"zig\"",
        1,
    ));
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &context,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::InvalidTarget)
    ));

    let stale = BTreeMap::from([(
        SemanticUnitId::new("workspace/unit.ueg").unwrap(),
        parse(&source("value + 9")),
    )]);
    let nested = EmissionDiagnosticStream::from_json_for(
        &bytes,
        &fixture.snapshot,
        &fixture.profile,
        &stale,
    )
    .unwrap_err();
    assert!(matches!(
        nested,
        EmissionDiagnosticStreamError::Nested { .. }
    ));
}

#[test]
fn rejects_integrity_canonical_unknown_and_size_violations() {
    let fixture = prepared();
    let bytes = stream(&fixture, 1).to_json().unwrap();
    let mut tampered_value = serde_json::from_slice::<Value>(&bytes).unwrap();
    let digest = tampered_value["stream_digest"].as_array_mut().unwrap();
    let first_digest_byte = digest[0].as_u64().unwrap();
    digest[0] = Value::from((first_digest_byte + 1) % 256);
    let tampered = serde_json::to_vec(&tampered_value).unwrap();
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &tampered,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::IntegrityMismatch)
            | Err(EmissionDiagnosticStreamError::Json(_))
    ));

    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(&bytes).unwrap()).unwrap();
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &pretty,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::NonCanonical)
    ));

    let mut unknown = serde_json::from_slice::<Value>(&bytes).unwrap();
    unknown["unexpected"] = Value::Bool(true);
    let unknown_bytes = serde_json::to_vec(&unknown).unwrap();
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &unknown_bytes,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::Json(_))
    ));

    let oversized = vec![b'x'; MAX_STREAM_BYTES + 1];
    assert!(matches!(
        EmissionDiagnosticStream::from_json_for(
            &oversized,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticStreamError::StreamTooLarge { .. })
    ));
    assert!(MAX_SERIALIZED_DIAGNOSTIC_BYTES < MAX_STREAM_BYTES);
}
