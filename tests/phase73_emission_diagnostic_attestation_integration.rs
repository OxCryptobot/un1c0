use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use serde_json::Value;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
    EmissionDiagnosticAttestationContent, EmissionDiagnosticAttestationError,
    MAX_ATTESTATION_METADATA_ENTRIES, MAX_ATTESTATION_METADATA_KEY_BYTES,
    MAX_ATTESTATION_METADATA_VALUE_BYTES, MAX_TRUSTED_ATTESTATION_KEYS,
};
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_transport::{
    AsyncDiagnosticTransport, DistributedEmissionAggregator,
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

fn stream(fixture: &Fixture, frames: usize) -> EmissionDiagnosticStream {
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&fixture.receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    EmissionDiagnosticStream::from_repeated_report(
        73,
        &report,
        frames,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn aggregate(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
) -> DistributedEmissionAggregator {
    let transport = AsyncDiagnosticTransport::new(1).unwrap();
    transport.send(5, 1, stream).unwrap();
    let observation = transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    let mut aggregate = DistributedEmissionAggregator::new();
    aggregate
        .ingest(
            observation,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    aggregate
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn metadata() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("environment".to_string(), "test".to_string()),
        ("purpose".to_string(), "phase73".to_string()),
    ])
}

fn mutate_json(
    attestation: &EmissionDiagnosticAttestation,
    field: &str,
    value: Value,
) -> EmissionDiagnosticAttestation {
    let mut json = serde_json::from_slice::<Value>(&attestation.to_json().unwrap()).unwrap();
    json[field] = value;
    let mutated: EmissionDiagnosticAttestation =
        serde_json::from_value(json).expect("valid attestation shape");
    EmissionDiagnosticAttestation::from_json(&serde_json::to_vec(&mutated).unwrap()).unwrap()
}

fn mutate_json_bytes(
    attestation: &EmissionDiagnosticAttestation,
    field: &str,
    value: Value,
) -> Vec<u8> {
    let mut json = serde_json::from_slice::<Value>(&attestation.to_json().unwrap()).unwrap();
    json[field] = value;
    serde_json::to_vec(&json).unwrap()
}

#[test]
fn stream_and_aggregate_attestations_verify_after_canonical_round_trip() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture, 4);
    let aggregate = aggregate(&fixture, &diagnostic_stream);
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(7));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();

    let stream_attestation = key
        .attest_stream(
            1,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();
    assert_eq!(
        stream_attestation.content_type(),
        EmissionDiagnosticAttestationContent::Stream
    );
    assert_eq!(stream_attestation.version(), 1);
    assert_eq!(stream_attestation.attestation_id(), 1);
    assert_eq!(verifier.trusted_key_count(), 1);
    verifier
        .verify_stream(
            &stream_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    let restored_stream_attestation =
        EmissionDiagnosticAttestation::from_json(&stream_attestation.to_json().unwrap()).unwrap();
    assert_eq!(restored_stream_attestation, stream_attestation);
    verifier
        .verify_stream(
            &restored_stream_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();

    let aggregate_attestation = key
        .attest_aggregate(
            2,
            &aggregate,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();
    assert_eq!(
        aggregate_attestation.content_type(),
        EmissionDiagnosticAttestationContent::Aggregate
    );
    verifier
        .verify_aggregate_for(
            &aggregate_attestation,
            &aggregate,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert_ne!(
        stream_attestation.content_hash(),
        aggregate_attestation.content_hash()
    );
}

#[test]
fn signatures_are_deterministic_and_trust_registration_is_explicit() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture, 1);
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(11));
    let first = key
        .attest_stream(
            9,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();
    let second = key
        .attest_stream(
            9,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();
    assert_eq!(first, second);

    let mut untrusted = DiagnosticAttestationVerifier::new();
    assert!(matches!(
        untrusted.verify_stream(
            &first,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticAttestationError::UnknownPublicKey)
    ));
    untrusted.register_public_key(key.public_key()).unwrap();
    untrusted
        .verify_stream(
            &first,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert!(untrusted.revoke_public_key(&key.public_key()));
    assert_eq!(untrusted.trusted_key_count(), 0);
    assert!(matches!(
        untrusted.verify_stream(
            &first,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticAttestationError::UnknownPublicKey)
    ));
}

#[test]
fn tampering_wrong_type_and_stale_state_fail_closed() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture, 2);
    let aggregate = aggregate(&fixture, &diagnostic_stream);
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(13));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();
    let attestation = key
        .attest_stream(
            3,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();

    let tampered_hash = mutate_json(&attestation, "content_hash", Value::from(vec![1u8; 32]));
    assert!(matches!(
        verifier.verify_stream(
            &tampered_hash,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticAttestationError::ContentMismatch)
    ));

    let tampered_signature = mutate_json(&attestation, "signature", Value::from(vec![2u8; 64]));
    assert!(matches!(
        verifier.verify_stream(
            &tampered_signature,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticAttestationError::InvalidSignature)
    ));

    let wrong_type = mutate_json(&attestation, "content_type", Value::from("aggregate"));
    assert!(matches!(
        verifier.verify_stream(
            &wrong_type,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticAttestationError::WrongContentType { .. })
    ));
    assert!(matches!(
        verifier.verify_aggregate(&attestation, &aggregate.summary()),
        Err(EmissionDiagnosticAttestationError::WrongContentType { .. })
    ));

    let stale = BTreeMap::from([(
        SemanticUnitId::new("workspace/unit.ueg").unwrap(),
        parse(&source("value + 9")),
    )]);
    assert!(matches!(
        verifier.verify_stream(
            &attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &stale,
        ),
        Err(EmissionDiagnosticAttestationError::Stream(_))
    ));
}

#[test]
fn bounds_versions_ids_and_canonical_json_fail_closed() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture, 1);
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(17));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();
    let attestation = key
        .attest_stream(
            5,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        )
        .unwrap();

    assert!(matches!(
        key.attest_stream(
            0,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            metadata(),
        ),
        Err(EmissionDiagnosticAttestationError::InvalidAttestationId)
    ));
    assert!(matches!(
        key.attest_stream(
            6,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            (0..=MAX_ATTESTATION_METADATA_ENTRIES)
                .map(|index| (format!("k{index}"), "v".to_string()))
                .collect(),
        ),
        Err(EmissionDiagnosticAttestationError::MetadataTooLarge { .. })
    ));
    assert!(matches!(
        key.attest_stream(
            7,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::from([(
                "k".repeat(MAX_ATTESTATION_METADATA_KEY_BYTES + 1),
                "v".to_string()
            )]),
        ),
        Err(EmissionDiagnosticAttestationError::MetadataKeyTooLarge { .. })
    ));
    assert!(matches!(
        key.attest_stream(
            8,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::from([(
                "k".to_string(),
                "v".repeat(MAX_ATTESTATION_METADATA_VALUE_BYTES + 1)
            )]),
        ),
        Err(EmissionDiagnosticAttestationError::MetadataValueTooLarge { .. })
    ));

    let version_bytes = mutate_json_bytes(&attestation, "version", Value::from(2));
    assert!(matches!(
        EmissionDiagnosticAttestation::from_json(&version_bytes),
        Err(EmissionDiagnosticAttestationError::InvalidVersion(2))
    ));
    let pretty = serde_json::to_vec_pretty(
        &serde_json::from_slice::<Value>(&attestation.to_json().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        EmissionDiagnosticAttestation::from_json(&pretty),
        Err(EmissionDiagnosticAttestationError::NonCanonical)
    ));
    let mut unknown = serde_json::from_slice::<Value>(&attestation.to_json().unwrap()).unwrap();
    unknown["unexpected"] = Value::Bool(true);
    assert!(matches!(
        EmissionDiagnosticAttestation::from_json(&serde_json::to_vec(&unknown).unwrap()),
        Err(EmissionDiagnosticAttestationError::Json(_))
    ));
}

#[test]
fn trust_store_is_bounded_and_empty_aggregates_cannot_be_attested() {
    let fixture = prepared();
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(19));
    let empty = DistributedEmissionAggregator::new();
    assert!(matches!(
        key.attest_aggregate(
            1,
            &empty,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::new(),
        ),
        Err(EmissionDiagnosticAttestationError::EmptyAggregate)
    ));

    let mut verifier = DiagnosticAttestationVerifier::new();
    for seed in 0..MAX_TRUSTED_ATTESTATION_KEYS as u8 {
        verifier
            .register_public_key(
                signing_key(seed.wrapping_add(30))
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap();
    }
    assert_eq!(verifier.trusted_key_count(), MAX_TRUSTED_ATTESTATION_KEYS);
    assert!(matches!(
        verifier.register_public_key(signing_key(250).verifying_key().to_bytes()),
        Err(EmissionDiagnosticAttestationError::TooManyTrustedKeys { .. })
    ));
}
