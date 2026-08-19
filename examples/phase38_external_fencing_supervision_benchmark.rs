use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::time::Instant;
use un1c0::external_fencing_supervision::{
    FenceApplicationOutcome, FenceConsumerAcknowledgement, FenceConsumerKind,
    FencingAuthorityHeartbeat, FencingSupervisionStatus, FencingSupervisor,
    SupervisionSnapshotStore,
};

#[derive(Debug, Serialize)]
struct Phase38Benchmark {
    phase: u8,
    authority_heartbeat_admitted: bool,
    consumer_acknowledgements: usize,
    required_consumers: usize,
    ready_status: String,
    stale_status: String,
    snapshot_round_trip: bool,
    journal_integrity: bool,
    elapsed_ms: u128,
    secret_material_recorded: bool,
}

fn main() {
    let started = Instant::now();
    let authority_key = SigningKey::from_bytes(&[38; 32]);
    let gateway_key = SigningKey::from_bytes(&[39; 32]);
    let scheduler_key = SigningKey::from_bytes(&[40; 32]);
    let mut required_consumers = BTreeMap::new();
    required_consumers.insert("gateway-a".to_string(), FenceConsumerKind::WriteGateway);
    required_consumers.insert(
        "scheduler-a".to_string(),
        FenceConsumerKind::WorkerScheduler,
    );
    let mut supervisor =
        FencingSupervisor::new("cluster-a", "resource-a", required_consumers.clone(), 64)
            .expect("supervisor should initialize");
    supervisor
        .register_authority("authority-a", &authority_key.verifying_key())
        .expect("authority should register");
    supervisor
        .register_consumer("gateway-a", &gateway_key.verifying_key())
        .expect("gateway should register");
    supervisor
        .register_consumer("scheduler-a", &scheduler_key.verifying_key())
        .expect("scheduler should register");

    let heartbeat = FencingAuthorityHeartbeat::sign(
        "cluster-a",
        "resource-a",
        "authority-a",
        3,
        7,
        9,
        &"a".repeat(64),
        &"b".repeat(64),
        100,
        100,
        &authority_key,
    )
    .expect("heartbeat should sign");
    supervisor
        .ingest_heartbeat(heartbeat, 105)
        .expect("heartbeat should admit");

    for (consumer_id, kind, key) in [
        ("gateway-a", FenceConsumerKind::WriteGateway, &gateway_key),
        (
            "scheduler-a",
            FenceConsumerKind::WorkerScheduler,
            &scheduler_key,
        ),
    ] {
        let acknowledgement = FenceConsumerAcknowledgement::sign(
            "cluster-a",
            "resource-a",
            "authority-a",
            consumer_id,
            kind,
            &"a".repeat(64),
            "region-b",
            3,
            7,
            105,
            100,
            FenceApplicationOutcome::Applied,
            key,
        )
        .expect("consumer acknowledgement should sign");
        supervisor
            .ingest_consumer_acknowledgement(acknowledgement, 105)
            .expect("consumer acknowledgement should admit");
    }

    let ready = supervisor.evaluate(105);
    let stale = supervisor.evaluate(201);
    let scratch = std::env::temp_dir().join(format!("un1c0-phase38-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("scratch directory should initialize");
    let store = SupervisionSnapshotStore::new(scratch.join("supervision.json"));
    let snapshot = supervisor.snapshot().expect("snapshot should build");
    store.save(&snapshot).expect("snapshot should persist");
    let loaded = store
        .load()
        .expect("snapshot should load")
        .expect("snapshot should exist");
    let mut restored = FencingSupervisor::new("cluster-a", "resource-a", required_consumers, 64)
        .expect("restored supervisor should initialize");
    restored
        .register_authority("authority-a", &authority_key.verifying_key())
        .expect("restored authority should register");
    restored
        .register_consumer("gateway-a", &gateway_key.verifying_key())
        .expect("restored gateway should register");
    restored
        .register_consumer("scheduler-a", &scheduler_key.verifying_key())
        .expect("restored scheduler should register");
    restored.restore(loaded).expect("snapshot should restore");

    let report = Phase38Benchmark {
        phase: 38,
        authority_heartbeat_admitted: ready.membership_epoch == 3,
        consumer_acknowledgements: ready.acknowledged_consumers,
        required_consumers: ready.required_consumers,
        ready_status: format!("{:?}", ready.status),
        stale_status: format!("{:?}", stale.status),
        snapshot_round_trip: restored.evaluate(105).status == FencingSupervisionStatus::Ready,
        journal_integrity: restored.journal_integrity(),
        elapsed_ms: started.elapsed().as_millis(),
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report should serialize")
    );
    let _ = fs::remove_dir_all(scratch);
}
