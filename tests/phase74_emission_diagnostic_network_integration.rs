use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
    EmissionDiagnosticAttestationError,
};
use un1c0::emission_diagnostic_network::{
    AuthenticatedDiagnosticConnection, AuthenticatedDiagnosticListener,
    EmissionDiagnosticNetworkError, MultiNodeDiagnosticReceiver, MAX_NETWORK_FRAMES_PER_NODE,
    MAX_NETWORK_NODES,
};
use un1c0::emission_diagnostic_transport::EmissionDiagnosticTransportError;
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
    let lambda = match &base.nodes[0] {
        NodeKind::Lambda(lambda) => lambda,
    };
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

fn diagnostic_stream(fixture: &Fixture, frames: usize) -> un1c0::EmissionDiagnosticStream {
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&fixture.receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    un1c0::EmissionDiagnosticStream::from_repeated_report(
        74,
        &report,
        frames,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn attestation(
    fixture: &Fixture,
    stream: &un1c0::EmissionDiagnosticStream,
    key: &DiagnosticAttestationKey,
    id: u64,
) -> EmissionDiagnosticAttestation {
    key.attest_stream(
        id,
        stream,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
        BTreeMap::from([("environment".to_string(), "loopback".to_string())]),
    )
    .unwrap()
}

fn key(seed: u8) -> DiagnosticAttestationKey {
    DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[seed; 32]))
}

fn verifier_for(key: &DiagnosticAttestationKey) -> Arc<DiagnosticAttestationVerifier> {
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();
    Arc::new(verifier)
}

fn connect_client(
    address: std::net::SocketAddr,
    node_id: u64,
    connection_id: u64,
    public_key: [u8; 32],
) -> Result<AuthenticatedDiagnosticConnection, EmissionDiagnosticNetworkError> {
    AuthenticatedDiagnosticConnection::connect_with_timeout(
        address,
        Duration::from_secs(2),
        node_id,
        connection_id,
        public_key,
        Arc::new(DiagnosticAttestationVerifier::new()),
    )
}

#[test]
fn loopback_handshake_attested_frame_and_multi_node_aggregation_verify() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 2);
    let signing_key = key(7);
    let attested = attestation(&fixture, &stream, &signing_key, 1);
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 41).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&signing_key);
    let server = thread::spawn(move || {
        let mut connection = listener.accept(server_verifier)?;
        connection.receive_attestation()
    });

    let mut client = connect_client(address, 41, 9001, signing_key.public_key()).unwrap();
    client.send_attestation(1, &attested).unwrap();
    let received = server.join().unwrap().unwrap();
    assert_eq!(received, attested);

    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver
        .register_node(41, verifier_for(&signing_key))
        .unwrap();
    receiver
        .ingest_attestation(
            41,
            1,
            &received,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    let summary = receiver.aggregator(41).unwrap().summary();
    assert_eq!(summary.source_count, 1);
    assert_eq!(summary.total_frames, 2);
    assert_eq!(summary.source_sequences.get(&41), Some(&1));
}

#[test]
fn multi_node_registration_keeps_aggregates_isolated() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 1);
    let first_key = key(11);
    let second_key = key(13);
    let first = attestation(&fixture, &stream, &first_key, 11);
    let second = attestation(&fixture, &stream, &second_key, 13);
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver
        .register_node(101, verifier_for(&first_key))
        .unwrap();
    receiver
        .register_node(202, verifier_for(&second_key))
        .unwrap();
    assert_eq!(receiver.registered_nodes(), 2);

    receiver
        .ingest_attestation(
            101,
            1,
            &first,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    receiver
        .ingest_attestation(
            202,
            1,
            &second,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert_eq!(receiver.aggregator(101).unwrap().source_count(), 1);
    assert_eq!(receiver.aggregator(202).unwrap().source_count(), 1);
    assert_eq!(
        receiver.aggregator(101).unwrap().summary().source_sequences[&101],
        1
    );
    assert_eq!(
        receiver.aggregator(202).unwrap().summary().source_sequences[&202],
        1
    );
}

#[test]
fn handshake_identity_and_exact_trust_key_mismatches_fail_closed() {
    let accepted_key = key(17);
    let untrusted_key = key(19);
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 303).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&accepted_key);
    let server = thread::spawn(move || listener.accept(server_verifier).map(|_| ()));
    let _client = connect_client(address, 303, 3001, untrusted_key.public_key()).unwrap();
    assert!(matches!(
        server.join().unwrap(),
        Err(EmissionDiagnosticNetworkError::HandshakeMismatch)
    ));

    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 303).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&accepted_key);
    let accepted_public_key = accepted_key.public_key();
    let server = thread::spawn(move || listener.accept(server_verifier).map(|_| ()));
    let _client = connect_client(address, 304, 3002, accepted_public_key).unwrap();
    assert!(matches!(
        server.join().unwrap(),
        Err(EmissionDiagnosticNetworkError::HandshakeMismatch)
    ));
}

#[test]
fn replay_and_gap_sequences_are_rejected_before_wire_mutation() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 1);
    let signing_key = key(23);
    let attested = attestation(&fixture, &stream, &signing_key, 23);
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 404).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&signing_key);
    let public_key = signing_key.public_key();
    let server = thread::spawn(move || {
        let mut connection = listener.accept(server_verifier)?;
        connection.receive_attestation()
    });
    let mut client = connect_client(address, 404, 4001, public_key).unwrap();
    assert!(matches!(
        client.send_attestation(2, &attested),
        Err(EmissionDiagnosticNetworkError::Gap {
            expected: 1,
            actual: 2
        })
    ));
    client.send_attestation(1, &attested).unwrap();
    assert!(matches!(
        client.send_attestation(1, &attested),
        Err(EmissionDiagnosticNetworkError::Replay {
            expected: 2,
            actual: 1
        })
    ));
    let received = server.join().unwrap().unwrap();
    assert_eq!(received, attested);

    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver
        .register_node(404, verifier_for(&signing_key))
        .unwrap();
    receiver
        .ingest_attestation(
            404,
            1,
            &received,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert!(matches!(
        receiver.ingest_attestation(
            404,
            1,
            &received,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticNetworkError::Transport(
            EmissionDiagnosticTransportError::Replay {
                source_id: 404,
                expected: 2,
                actual: 1
            }
        ))
    ));
    assert!(matches!(
        receiver.ingest_attestation(
            404,
            3,
            &received,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticNetworkError::Transport(
            EmissionDiagnosticTransportError::Gap {
                source_id: 404,
                expected: 2,
                actual: 3
            }
        ))
    ));
}

#[test]
fn connection_frame_limits_and_identifier_bounds_are_enforced() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 1);
    let signing_key = key(29);
    let attested = attestation(&fixture, &stream, &signing_key, 29);
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 505).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&signing_key);
    let public_key = signing_key.public_key();
    let server = thread::spawn(move || {
        let mut connection = listener.accept(server_verifier)?;
        for _ in 0..MAX_NETWORK_FRAMES_PER_NODE {
            connection.receive_attestation()?;
        }
        connection.receive_attestation()
    });
    let mut client = connect_client(address, 505, 5001, public_key).unwrap();
    for sequence in 1..=MAX_NETWORK_FRAMES_PER_NODE as u64 {
        client.send_attestation(sequence, &attested).unwrap();
    }
    assert!(matches!(
        client.send_attestation((MAX_NETWORK_FRAMES_PER_NODE + 1) as u64, &attested),
        Err(EmissionDiagnosticNetworkError::FrameLimit { .. })
    ));
    assert!(matches!(
        server.join().unwrap(),
        Err(EmissionDiagnosticNetworkError::FrameLimit {
            count: 65,
            maximum: MAX_NETWORK_FRAMES_PER_NODE
        })
    ));

    assert!(matches!(
        AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 0),
        Err(EmissionDiagnosticNetworkError::InvalidNodeId)
    ));
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 606).unwrap();
    let address = listener.local_addr().unwrap();
    assert!(matches!(
        connect_client(address, 0, 6001, signing_key.public_key()),
        Err(EmissionDiagnosticNetworkError::InvalidNodeId)
    ));
    assert!(matches!(
        connect_client(address, 606, 0, signing_key.public_key()),
        Err(EmissionDiagnosticNetworkError::InvalidConnectionId)
    ));
}

#[test]
fn stale_candidate_state_is_rejected_after_network_attestation_verification() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 1);
    let signing_key = key(31);
    let attested = attestation(&fixture, &stream, &signing_key, 31);
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", 707).unwrap();
    let address = listener.local_addr().unwrap();
    let server_verifier = verifier_for(&signing_key);
    let public_key = signing_key.public_key();
    let server = thread::spawn(move || {
        let mut connection = listener.accept(server_verifier)?;
        connection.receive_attestation()
    });
    let mut client = connect_client(address, 707, 7001, public_key).unwrap();
    client.send_attestation(1, &attested).unwrap();
    let received = server.join().unwrap().unwrap();

    let stale_candidates = BTreeMap::from([(
        SemanticUnitId::new("workspace/unit.ueg").unwrap(),
        parse(&source("value + 99")),
    )]);
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver
        .register_node(707, verifier_for(&signing_key))
        .unwrap();
    assert!(matches!(
        receiver.ingest_attestation(
            707,
            1,
            &received,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &stale_candidates,
        ),
        Err(EmissionDiagnosticNetworkError::Attestation(
            EmissionDiagnosticAttestationError::Stream(_)
        ))
    ));
    assert_eq!(receiver.aggregator(707).unwrap().source_count(), 0);
}

#[test]
fn network_receiver_rejects_unregistered_nodes() {
    let fixture = prepared();
    let stream = diagnostic_stream(&fixture, 1);
    let signing_key = key(37);
    let attested = attestation(&fixture, &stream, &signing_key, 37);
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    assert!(matches!(
        receiver.ingest_attestation(
            999,
            1,
            &attested,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticNetworkError::UnexpectedNode {
            expected: 0,
            actual: 999
        })
    ));
}

#[test]
fn network_receiver_node_registration_is_bounded() {
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    for node_id in 1..=MAX_NETWORK_NODES as u64 {
        receiver
            .register_node(node_id, Arc::new(DiagnosticAttestationVerifier::new()))
            .unwrap();
    }
    assert_eq!(receiver.registered_nodes(), MAX_NETWORK_NODES);
    match receiver.register_node(999, Arc::new(DiagnosticAttestationVerifier::new())) {
        Err(EmissionDiagnosticNetworkError::NodeLimit { count, maximum }) => {
            assert_eq!(count, MAX_NETWORK_NODES + 1);
            assert_eq!(maximum, MAX_NETWORK_NODES);
        }
        other => panic!("unexpected node-limit result: {other:?}"),
    }
}

#[test]
fn network_module_never_accepts_zero_frame_limit() {
    assert!(matches!(
        AuthenticatedDiagnosticListener::bind_with_limit("127.0.0.1:0", 1, 0),
        Err(EmissionDiagnosticNetworkError::FrameTooLarge {
            bytes: 0,
            maximum: _
        })
    ));
}
