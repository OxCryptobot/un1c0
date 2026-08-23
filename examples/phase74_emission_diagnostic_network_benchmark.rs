use std::collections::BTreeMap;
use std::thread;
use std::time::Instant;

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
};
use un1c0::emission_diagnostic_network::{
    AuthenticatedDiagnosticConnection, AuthenticatedDiagnosticListener,
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

const SAMPLES: usize = 12;
const FRAME_COUNTS: [usize; 4] = [1, 2, 4, 8];
const CONCURRENCY_LEVELS: [usize; 4] = [1, 2, 4, 8];

#[derive(Debug, Serialize)]
struct BenchmarkArtifact {
    phase: u8,
    samples: usize,
    target: &'static str,
    frame_counts: &'static [usize; 4],
    concurrency_levels: &'static [usize; 4],
    rows: Vec<Row>,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

#[derive(Debug, Serialize)]
struct Row {
    frames_per_connection: usize,
    concurrent_connections: usize,
    stream_bytes: usize,
    e2e_p50_ns: u128,
    e2e_p95_ns: u128,
    e2e_p99_ns: u128,
    receive_p50_ns: u128,
    receive_p95_ns: u128,
    receive_p99_ns: u128,
    verify_p50_ns: u128,
    verify_p95_ns: u128,
    verify_p99_ns: u128,
    verify_per_connection_frame_p50_ns: u128,
    frames_per_second_p50: u64,
}

#[derive(Debug)]
struct Fixture {
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
    report: EmissionDiagnosticReport,
}

#[derive(Debug, Default)]
struct StageMetrics {
    receive_ns: u128,
    verify_ns: u128,
}

fn main() {
    let fixture = prepared();
    let key = DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[74; 32]));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier
        .register_public_key(key.public_key())
        .expect("register benchmark key");
    let mut rows = Vec::new();
    let mut errors = 0;

    for frames_per_connection in FRAME_COUNTS {
        let stream = un1c0::EmissionDiagnosticStream::from_repeated_report(
            74,
            &fixture.report,
            frames_per_connection,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .expect("build benchmark stream");
        let attestation = key
            .attest_stream(
                frames_per_connection as u64,
                &stream,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                BTreeMap::from([("environment".to_string(), "loopback".to_string())]),
            )
            .expect("attest benchmark stream");
        let actual_stream_bytes = stream.to_json().expect("serialize stream").len();

        for concurrent_connections in CONCURRENCY_LEVELS {
            let mut e2e = Vec::with_capacity(SAMPLES);
            let mut receive = Vec::with_capacity(SAMPLES);
            let mut verify = Vec::with_capacity(SAMPLES);
            let mut errors_for_row = 0;
            for _sample in 0..SAMPLES {
                match run_once(
                    &fixture,
                    &stream,
                    &attestation,
                    &verifier,
                    concurrent_connections,
                    frames_per_connection,
                ) {
                    Ok(metrics) => {
                        e2e.push(metrics.e2e_ns);
                        receive.push(metrics.receive_ns);
                        verify.push(metrics.verify_ns);
                    }
                    Err(_) => errors_for_row += 1,
                }
            }
            errors += errors_for_row;
            if errors_for_row > 0 {
                continue;
            }
            e2e.sort_unstable();
            receive.sort_unstable();
            verify.sort_unstable();
            let e2e_p50 = percentile(&e2e, 0.50);
            let total_frames = (concurrent_connections * frames_per_connection) as u128;
            let frames_per_second_p50 = if e2e_p50 == 0 {
                0
            } else {
                ((total_frames * 1_000_000_000) / e2e_p50) as u64
            };
            rows.push(Row {
                frames_per_connection,
                concurrent_connections,
                stream_bytes: actual_stream_bytes,
                e2e_p50_ns: e2e_p50,
                e2e_p95_ns: percentile(&e2e, 0.95),
                e2e_p99_ns: percentile(&e2e, 0.99),
                receive_p50_ns: percentile(&receive, 0.50),
                receive_p95_ns: percentile(&receive, 0.95),
                receive_p99_ns: percentile(&receive, 0.99),
                verify_p50_ns: percentile(&verify, 0.50),
                verify_p95_ns: percentile(&verify, 0.95),
                verify_p99_ns: percentile(&verify, 0.99),
                verify_per_connection_frame_p50_ns: percentile(&verify, 0.50)
                    / frames_per_connection as u128,
                frames_per_second_p50,
            });
        }
    }

    let artifact = BenchmarkArtifact {
        phase: 74,
        samples: SAMPLES,
        target: fixture.profile.target.label(),
        frame_counts: &FRAME_COUNTS,
        concurrency_levels: &CONCURRENCY_LEVELS,
        rows,
        errors,
        cluster_mutation_performed: false,
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark artifact")
    );
}

#[derive(Debug)]
struct RunMetrics {
    e2e_ns: u128,
    receive_ns: u128,
    verify_ns: u128,
}

fn run_once(
    fixture: &Fixture,
    stream: &un1c0::EmissionDiagnosticStream,
    attestation: &EmissionDiagnosticAttestation,
    verifier: &DiagnosticAttestationVerifier,
    concurrent_connections: usize,
    frames_per_connection: usize,
) -> Result<RunMetrics, String> {
    let node_id = 7400;
    let listener = AuthenticatedDiagnosticListener::bind("127.0.0.1:0", node_id)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let server_verifier = std::sync::Arc::new(verifier.clone());
    let server_stream = stream.clone();
    let server_snapshot = fixture.snapshot.clone();
    let server_profile = fixture.profile.clone();
    let server_candidates = fixture.candidates.clone();
    let server = thread::spawn(move || -> Result<(u128, u128), String> {
        let mut handlers = Vec::with_capacity(concurrent_connections);
        for _ in 0..concurrent_connections {
            let mut connection = listener
                .accept(server_verifier.clone())
                .map_err(|error| error.to_string())?;
            let verifier = server_verifier.clone();
            let stream = server_stream.clone();
            let snapshot = server_snapshot.clone();
            let profile = server_profile.clone();
            let candidates = server_candidates.clone();
            handlers.push(thread::spawn(move || -> Result<StageMetrics, String> {
                let started_receive = Instant::now();
                let mut received = Vec::with_capacity(frames_per_connection);
                for _ in 0..frames_per_connection {
                    received.push(
                        connection
                            .receive_attestation()
                            .map_err(|error| error.to_string())?,
                    );
                }
                let receive_ns = started_receive.elapsed().as_nanos();
                let started_verify = Instant::now();
                for attestation in received {
                    verifier
                        .verify_stream(&attestation, &stream, &snapshot, &profile, &candidates)
                        .map_err(|error| error.to_string())?;
                }
                Ok(StageMetrics {
                    receive_ns,
                    verify_ns: started_verify.elapsed().as_nanos(),
                })
            }));
        }
        let mut receive_ns = 0;
        let mut verify_ns = 0;
        for handler in handlers {
            let metrics = handler
                .join()
                .map_err(|_| "server worker panicked".to_string())??;
            receive_ns = receive_ns.max(metrics.receive_ns);
            verify_ns = verify_ns.max(metrics.verify_ns);
        }
        Ok((receive_ns, verify_ns))
    });

    let started = Instant::now();
    let public_key = attestation.public_key();
    let mut clients = Vec::with_capacity(concurrent_connections);
    for connection_index in 0..concurrent_connections {
        let attestation = attestation.clone();
        clients.push(thread::spawn(move || -> Result<(), String> {
            let mut connection = AuthenticatedDiagnosticConnection::connect_with_timeout(
                address,
                std::time::Duration::from_secs(2),
                node_id,
                10_000 + connection_index as u64,
                public_key,
                std::sync::Arc::new(DiagnosticAttestationVerifier::new()),
            )
            .map_err(|error| error.to_string())?;
            for sequence in 1..=frames_per_connection as u64 {
                connection
                    .send_attestation(sequence, &attestation)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }));
    }
    let mut client_error = None;
    for client in clients {
        match client
            .join()
            .map_err(|_| "client worker panicked".to_string())?
        {
            Ok(()) => {}
            Err(error) => client_error = Some(error),
        }
    }
    let server_result = server
        .join()
        .map_err(|_| "server acceptor panicked".to_string())?;
    if let Some(error) = client_error {
        return Err(format!(
            "client error: {error}; server result: {server_result:?}"
        ));
    }
    let (receive_ns, verify_ns) = server_result?;
    Ok(RunMetrics {
        e2e_ns: started.elapsed().as_nanos(),
        receive_ns,
        verify_ns,
    })
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
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    Fixture {
        snapshot,
        profile,
        candidates,
        report,
    }
}

fn source(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
