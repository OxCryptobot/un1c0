use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Instant;
use un1c0::telemetry_failover::{
    fuzz_authenticated_transport_receiver, fuzz_witness_reservation_store, ConsensusTelemetryEvent,
    ConsensusTelemetryKind, FailoverOrchestrationAction, FailoverOrchestrator,
    SecureTelemetryReceiver, TelemetryKeyRegistry,
};

#[derive(Debug, Serialize)]
struct Phase37Benchmark {
    phase: u8,
    telemetry_events_admitted: usize,
    journal_hash_chain_valid: bool,
    failover_phase: String,
    active_region: Option<String>,
    transport_fuzz: un1c0::EpochChurnFuzzReport,
    reservation_fuzz: un1c0::EpochChurnFuzzReport,
    elapsed_ms: u128,
    secret_material_recorded: bool,
}

fn event(key: &SigningKey, kind: ConsensusTelemetryKind, sequence: u64) -> ConsensusTelemetryEvent {
    let mut labels = BTreeMap::new();
    labels.insert("source".into(), "phase37-benchmark".into());
    let mut metrics = BTreeMap::new();
    metrics.insert("healthy_nodes".into(), 3);
    ConsensusTelemetryEvent::sign(
        "cluster-a",
        "resource-a",
        "producer-a",
        "region-a",
        7,
        sequence,
        100,
        100,
        kind,
        labels,
        metrics,
        key,
    )
    .expect("benchmark event should be valid")
}

fn main() {
    let started = Instant::now();
    let key = SigningKey::from_bytes(&[37; 32]);
    let mut registry = TelemetryKeyRegistry::new();
    registry
        .register("producer-a", &key.verifying_key())
        .expect("benchmark producer should register");
    let mut receiver = SecureTelemetryReceiver::new("cluster-a", "resource-a", 64)
        .expect("receiver should initialize");
    let required = BTreeSet::from([
        ConsensusTelemetryKind::LeaderHealth,
        ConsensusTelemetryKind::WitnessQuorum,
        ConsensusTelemetryKind::TransportHealth,
    ]);
    let mut orchestrator =
        FailoverOrchestrator::new("cluster-a", "resource-a", required, 100, Some("region-a"))
            .expect("orchestrator should initialize");
    orchestrator
        .detect_failure("phase37-op")
        .expect("detection should succeed");
    for (sequence, kind) in [
        (1, ConsensusTelemetryKind::LeaderHealth),
        (2, ConsensusTelemetryKind::WitnessQuorum),
        (3, ConsensusTelemetryKind::TransportHealth),
    ] {
        let telemetry = event(&key, kind, sequence);
        receiver
            .admit(telemetry.clone(), &registry, 105)
            .expect("receiver admission should succeed");
        orchestrator
            .ingest(telemetry, &registry, 105)
            .expect("orchestrator admission should succeed");
    }
    let decision_digest = "d".repeat(64);
    assert_eq!(
        orchestrator.begin_failover("phase37-op", &decision_digest, "region-b", 7, 105),
        Ok(FailoverOrchestrationAction::AwaitingExternalFence)
    );
    orchestrator
        .admit_external_fence("phase37-op", &decision_digest, 106)
        .expect("external fence should be admitted");
    orchestrator
        .commit("phase37-op", &decision_digest)
        .expect("failover should commit");

    let scratch = std::env::temp_dir().join(format!("un1c0-phase37-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("scratch directory should initialize");
    let reservation_report =
        fuzz_witness_reservation_store(scratch.join("witness-reservations.json"), 0x37, 4_096);
    let transport_report = fuzz_authenticated_transport_receiver(0x73, 4_096);
    let report = Phase37Benchmark {
        phase: 37,
        telemetry_events_admitted: receiver.report().accepted_events,
        journal_hash_chain_valid: receiver.journal_integrity(),
        failover_phase: format!("{:?}", orchestrator.report().phase),
        active_region: orchestrator.report().active_region_id,
        transport_fuzz: transport_report,
        reservation_fuzz: reservation_report,
        elapsed_ms: started.elapsed().as_millis(),
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report should serialize")
    );
    let _ = fs::remove_dir_all(scratch);
}
