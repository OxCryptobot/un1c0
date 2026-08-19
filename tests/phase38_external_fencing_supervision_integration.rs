use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;
use un1c0::external_fencing_supervision::{
    FenceApplicationOutcome, FenceConsumerAcknowledgement, FenceConsumerKind,
    FencingAuthorityHeartbeat, FencingSupervisionError, FencingSupervisionStatus,
    FencingSupervisor, SupervisionKeyRegistry, SupervisionSnapshotStore,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn required_consumers() -> BTreeMap<String, FenceConsumerKind> {
    BTreeMap::from([
        ("gateway-a".to_string(), FenceConsumerKind::WriteGateway),
        (
            "scheduler-a".to_string(),
            FenceConsumerKind::WorkerScheduler,
        ),
    ])
}

fn make_supervisor(
    max_journal_entries: usize,
) -> (FencingSupervisor, SigningKey, SigningKey, SigningKey) {
    let authority_key = key(41);
    let gateway_key = key(42);
    let scheduler_key = key(43);
    let mut supervisor = FencingSupervisor::new(
        "cluster-a",
        "resource-a",
        required_consumers(),
        max_journal_entries,
    )
    .unwrap();
    supervisor
        .register_authority("authority-a", &authority_key.verifying_key())
        .unwrap();
    supervisor
        .register_consumer("gateway-a", &gateway_key.verifying_key())
        .unwrap();
    supervisor
        .register_consumer("scheduler-a", &scheduler_key.verifying_key())
        .unwrap();
    (supervisor, authority_key, gateway_key, scheduler_key)
}

fn heartbeat(
    authority_key: &SigningKey,
    membership_epoch: u64,
    fence_epoch: u64,
    log_index: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    token_byte: char,
) -> FencingAuthorityHeartbeat {
    FencingAuthorityHeartbeat::sign(
        "cluster-a",
        "resource-a",
        "authority-a",
        "region-b",
        membership_epoch,
        fence_epoch,
        log_index,
        &token_byte.to_string().repeat(64),
        &"b".repeat(64),
        observed_tick,
        ttl_ticks,
        authority_key,
    )
    .unwrap()
}

fn acknowledgement(
    consumer_id: &str,
    consumer_kind: FenceConsumerKind,
    consumer_key: &SigningKey,
    fence_epoch: u64,
    observed_tick: u64,
    outcome: FenceApplicationOutcome,
) -> FenceConsumerAcknowledgement {
    FenceConsumerAcknowledgement::sign(
        "cluster-a",
        "resource-a",
        "authority-a",
        consumer_id,
        consumer_kind,
        &"a".repeat(64),
        "region-b",
        3,
        fence_epoch,
        observed_tick,
        50,
        outcome,
        consumer_key,
    )
    .unwrap()
}

fn acknowledgement_in_region(
    consumer_id: &str,
    consumer_kind: FenceConsumerKind,
    consumer_key: &SigningKey,
    owner_region_id: &str,
) -> FenceConsumerAcknowledgement {
    FenceConsumerAcknowledgement::sign(
        "cluster-a",
        "resource-a",
        "authority-a",
        consumer_id,
        consumer_kind,
        &"a".repeat(64),
        owner_region_id,
        3,
        7,
        105,
        50,
        FenceApplicationOutcome::Applied,
        consumer_key,
    )
    .unwrap()
}

#[test]
fn authority_and_consumer_keys_are_pinned_without_rebinding() {
    let authority = key(51);
    let consumer = key(52);
    let mut registry = SupervisionKeyRegistry::new();
    registry
        .register_authority("authority-a", &authority.verifying_key())
        .unwrap();
    registry
        .register_consumer("gateway-a", &consumer.verifying_key())
        .unwrap();
    assert!(registry
        .register_authority("authority-a", &authority.verifying_key())
        .is_ok());
    assert!(matches!(
        registry.register_authority("authority-a", &key(53).verifying_key()),
        Err(FencingSupervisionError::Rejected(_))
    ));
    assert!(matches!(
        registry.register_consumer("gateway-a", &key(54).verifying_key()),
        Err(FencingSupervisionError::Rejected(_))
    ));
}

#[test]
fn stale_authority_heartbeat_blocks_supervision_readiness() {
    let (mut supervisor, authority_key, _, _) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 10, 'a'), 105)
        .unwrap();
    assert_eq!(
        supervisor.evaluate(111).status,
        FencingSupervisionStatus::AuthorityStale
    );
}

#[test]
fn future_dated_authority_evidence_is_rejected() {
    let (mut supervisor, authority_key, _, _) = make_supervisor(8);
    assert!(matches!(
        supervisor.ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 106, 50, 'a'), 105),
        Err(FencingSupervisionError::StaleEvidence(_))
    ));
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::AuthorityMissing
    );
}

#[test]
fn exact_consumer_coverage_is_required_for_ready() {
    let (mut supervisor, authority_key, gateway_key, scheduler_key) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::MissingConsumer
    );
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "gateway-a",
                FenceConsumerKind::WriteGateway,
                &gateway_key,
                7,
                105,
                FenceApplicationOutcome::Applied,
            ),
            105,
        )
        .unwrap();
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::MissingConsumer
    );
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "scheduler-a",
                FenceConsumerKind::WorkerScheduler,
                &scheduler_key,
                7,
                105,
                FenceApplicationOutcome::Applied,
            ),
            105,
        )
        .unwrap();
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::Ready
    );
    assert!(supervisor.journal_integrity());
}

#[test]
fn acknowledgement_owner_region_must_match_authority() {
    let (mut supervisor, authority_key, gateway_key, _) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    let mismatched = acknowledgement_in_region(
        "gateway-a",
        FenceConsumerKind::WriteGateway,
        &gateway_key,
        "region-a",
    );
    assert!(matches!(
        supervisor.ingest_consumer_acknowledgement(mismatched, 105),
        Err(FencingSupervisionError::Rejected(_))
    ));
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::MissingConsumer
    );
}

#[test]
fn acknowledgements_must_bind_to_current_fence_generation() {
    let (mut supervisor, authority_key, gateway_key, _) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    let stale = acknowledgement(
        "gateway-a",
        FenceConsumerKind::WriteGateway,
        &gateway_key,
        6,
        105,
        FenceApplicationOutcome::Applied,
    );
    assert!(matches!(
        supervisor.ingest_consumer_acknowledgement(stale, 105),
        Err(FencingSupervisionError::Rejected(_))
    ));
}

#[test]
fn conflicting_authority_generation_quarantines_supervisor() {
    let (mut supervisor, authority_key, _, _) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    let conflicting = heartbeat(&authority_key, 3, 7, 9, 101, 50, 'b');
    assert!(matches!(
        supervisor.ingest_heartbeat(conflicting, 105),
        Err(FencingSupervisionError::Conflict(_))
    ));
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::Quarantined
    );
}

#[test]
fn quarantined_consumer_blocks_readiness() {
    let (mut supervisor, authority_key, gateway_key, scheduler_key) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "gateway-a",
                FenceConsumerKind::WriteGateway,
                &gateway_key,
                7,
                105,
                FenceApplicationOutcome::Quarantined,
            ),
            105,
        )
        .unwrap();
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "scheduler-a",
                FenceConsumerKind::WorkerScheduler,
                &scheduler_key,
                7,
                105,
                FenceApplicationOutcome::Applied,
            ),
            105,
        )
        .unwrap();
    assert_eq!(
        supervisor.evaluate(105).status,
        FencingSupervisionStatus::ConsumerQuarantined
    );
}

#[test]
fn supervision_snapshot_round_trips_and_tampering_is_rejected() {
    let (mut supervisor, authority_key, gateway_key, scheduler_key) = make_supervisor(8);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "gateway-a",
                FenceConsumerKind::WriteGateway,
                &gateway_key,
                7,
                105,
                FenceApplicationOutcome::Applied,
            ),
            105,
        )
        .unwrap();
    supervisor
        .ingest_consumer_acknowledgement(
            acknowledgement(
                "scheduler-a",
                FenceConsumerKind::WorkerScheduler,
                &scheduler_key,
                7,
                105,
                FenceApplicationOutcome::Applied,
            ),
            105,
        )
        .unwrap();
    let directory = tempdir().unwrap();
    let path = directory.path().join("supervision.json");
    let store = SupervisionSnapshotStore::new(&path);
    store.save(&supervisor.snapshot().unwrap()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    let (mut restored, restored_authority, restored_gateway, restored_scheduler) =
        make_supervisor(8);
    restored
        .restore(loaded)
        .expect("restored snapshot should verify against pinned keys");
    assert_eq!(
        restored.evaluate(105).status,
        FencingSupervisionStatus::Ready
    );
    drop((restored_authority, restored_gateway, restored_scheduler));

    let mut tampered = fs::read(&path).unwrap();
    tampered[0] ^= 0x01;
    fs::write(&path, tampered).unwrap();
    assert!(matches!(
        store.load(),
        Err(FencingSupervisionError::PersistenceFailed(_))
            | Err(FencingSupervisionError::Rejected(_))
    ));
}

#[test]
fn journal_capacity_failure_rolls_back_consumer_acknowledgement() {
    let (mut supervisor, authority_key, gateway_key, _) = make_supervisor(1);
    supervisor
        .ingest_heartbeat(heartbeat(&authority_key, 3, 7, 9, 100, 50, 'a'), 105)
        .unwrap();
    let result = supervisor.ingest_consumer_acknowledgement(
        acknowledgement(
            "gateway-a",
            FenceConsumerKind::WriteGateway,
            &gateway_key,
            7,
            105,
            FenceApplicationOutcome::Applied,
        ),
        105,
    );
    assert!(matches!(result, Err(FencingSupervisionError::Rejected(_))));
    let report = supervisor.evaluate(105);
    assert_eq!(report.acknowledged_consumers, 0);
    assert_eq!(report.journal_entries, 1);
    assert_eq!(report.status, FencingSupervisionStatus::MissingConsumer);
}
