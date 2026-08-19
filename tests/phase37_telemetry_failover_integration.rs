use ed25519_dalek::SigningKey;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use tempfile::tempdir;
use un1c0::telemetry_failover::{
    fuzz_authenticated_transport_receiver, fuzz_witness_reservation_store, ConsensusTelemetryEvent,
    ConsensusTelemetryKind, FailoverIntentStore, FailoverOrchestrationAction,
    FailoverOrchestrationPhase, FailoverOrchestrator, SecureTelemetryReceiver, TelemetryAdmission,
    TelemetryFailoverError, TelemetryKeyRegistry,
};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn telemetry_event(
    producer: &str,
    key: &SigningKey,
    epoch: u64,
    sequence: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    kind: ConsensusTelemetryKind,
    label_value: &str,
) -> ConsensusTelemetryEvent {
    let mut labels = BTreeMap::new();
    labels.insert("status".to_string(), label_value.to_string());
    let mut metrics = BTreeMap::new();
    metrics.insert("healthy_nodes".to_string(), 3);
    ConsensusTelemetryEvent::sign(
        "cluster-a",
        "resource-a",
        producer,
        "region-a",
        epoch,
        sequence,
        observed_tick,
        ttl_ticks,
        kind,
        labels,
        metrics,
        key,
    )
    .expect("test telemetry event should be valid")
}

fn registry(producer: &str, key: &SigningKey) -> TelemetryKeyRegistry {
    let mut registry = TelemetryKeyRegistry::new();
    registry
        .register(producer, &key.verifying_key())
        .expect("test producer should register");
    registry
}

#[test]
fn signed_telemetry_requires_binding_and_bounded_labels() {
    let key = signing_key(7);
    let registry = registry("producer-a", &key);
    let event = telemetry_event(
        "producer-a",
        &key,
        4,
        1,
        100,
        20,
        ConsensusTelemetryKind::LeaderHealth,
        "healthy",
    );
    event
        .verify(&registry, "cluster-a", "resource-a")
        .expect("valid event should verify");

    let mut too_many_labels = BTreeMap::new();
    for index in 0..17 {
        too_many_labels.insert(format!("label-{index}"), "bounded".to_string());
    }
    let result = ConsensusTelemetryEvent::sign(
        "cluster-a",
        "resource-a",
        "producer-a",
        "region-a",
        4,
        2,
        100,
        20,
        ConsensusTelemetryKind::LeaderHealth,
        too_many_labels,
        BTreeMap::new(),
        &key,
    );
    assert!(matches!(
        result,
        Err(TelemetryFailoverError::TelemetryRejected(_))
    ));

    let mut control_label = BTreeMap::new();
    control_label.insert("status".to_string(), "bad\nvalue".to_string());
    let result = ConsensusTelemetryEvent::sign(
        "cluster-a",
        "resource-a",
        "producer-a",
        "region-a",
        4,
        2,
        100,
        20,
        ConsensusTelemetryKind::LeaderHealth,
        control_label,
        BTreeMap::new(),
        &key,
    );
    assert!(matches!(
        result,
        Err(TelemetryFailoverError::InvalidInput(_))
    ));
}

#[test]
fn telemetry_receiver_is_idempotent_replay_safe_and_hash_chained() {
    let key = signing_key(9);
    let registry = registry("producer-a", &key);
    let mut receiver = SecureTelemetryReceiver::new("cluster-a", "resource-a", 8).unwrap();
    let first = telemetry_event(
        "producer-a",
        &key,
        1,
        1,
        10,
        100,
        ConsensusTelemetryKind::LeaderHealth,
        "healthy",
    );
    assert_eq!(
        receiver.admit(first.clone(), &registry, 11),
        Ok(TelemetryAdmission::Accepted)
    );
    assert_eq!(
        receiver.admit(first, &registry, 11),
        Ok(TelemetryAdmission::AlreadySeen)
    );

    let second = telemetry_event(
        "producer-a",
        &key,
        1,
        2,
        12,
        100,
        ConsensusTelemetryKind::TransportHealth,
        "healthy",
    );
    assert_eq!(
        receiver.admit(second, &registry, 13),
        Ok(TelemetryAdmission::Accepted)
    );
    assert_eq!(receiver.journal().len(), 2);
    assert!(receiver.journal_integrity());
    assert_eq!(receiver.journal()[0].previous_hash, "0".repeat(64));
    assert_eq!(
        receiver.journal()[1].previous_hash,
        receiver.journal()[0].event_hash
    );

    let regressed = telemetry_event(
        "producer-a",
        &key,
        1,
        1,
        14,
        100,
        ConsensusTelemetryKind::SnapshotFreshness,
        "regressed",
    );
    assert!(matches!(
        receiver.admit(regressed, &registry, 15),
        Err(TelemetryFailoverError::ReplayRejected(_))
    ));
    assert_eq!(receiver.report().journal_length, 2);
}

#[test]
fn stale_telemetry_blocks_failover_promotion() {
    let key = signing_key(11);
    let registry = registry("producer-a", &key);
    let required = BTreeSet::from([
        ConsensusTelemetryKind::LeaderHealth,
        ConsensusTelemetryKind::WitnessQuorum,
    ]);
    let mut orchestrator =
        FailoverOrchestrator::new("cluster-a", "resource-a", required, 10, Some("region-a"))
            .unwrap();
    orchestrator.detect_failure("op-stale").unwrap();
    let stale = telemetry_event(
        "producer-a",
        &key,
        5,
        1,
        1,
        2,
        ConsensusTelemetryKind::LeaderHealth,
        "healthy",
    );
    assert!(matches!(
        orchestrator.ingest(stale, &registry, 20),
        Err(TelemetryFailoverError::StaleTelemetry(_))
    ));
    assert_eq!(
        orchestrator.report().phase,
        FailoverOrchestrationPhase::CollectingEvidence
    );
    assert!(matches!(
        orchestrator.begin_failover("op-stale", &"a".repeat(64), "region-b", 5, 20),
        Err(TelemetryFailoverError::OrchestrationRejected(_))
    ));
}

#[test]
fn failover_is_typed_fenced_and_exactly_idempotent() {
    let key = signing_key(13);
    let registry = registry("producer-a", &key);
    let required = BTreeSet::from([
        ConsensusTelemetryKind::LeaderHealth,
        ConsensusTelemetryKind::WitnessQuorum,
    ]);
    let mut orchestrator =
        FailoverOrchestrator::new("cluster-a", "resource-a", required, 50, Some("region-a"))
            .unwrap();
    orchestrator.detect_failure("op-37").unwrap();
    orchestrator
        .ingest(
            telemetry_event(
                "producer-a",
                &key,
                5,
                1,
                100,
                50,
                ConsensusTelemetryKind::LeaderHealth,
                "failed",
            ),
            &registry,
            105,
        )
        .unwrap();
    orchestrator
        .ingest(
            telemetry_event(
                "producer-a",
                &key,
                5,
                2,
                101,
                50,
                ConsensusTelemetryKind::WitnessQuorum,
                "available",
            ),
            &registry,
            105,
        )
        .unwrap();
    let digest = "b".repeat(64);
    assert_eq!(
        orchestrator.begin_failover("op-37", &digest, "region-b", 5, 105),
        Ok(FailoverOrchestrationAction::AwaitingExternalFence)
    );
    assert_eq!(
        orchestrator.begin_failover("op-37", &digest, "region-b", 5, 105),
        Ok(FailoverOrchestrationAction::AlreadyPrepared)
    );
    assert_eq!(
        orchestrator.admit_external_fence("op-37", &digest, 106),
        Ok(FailoverOrchestrationAction::FenceAdmitted)
    );
    assert_eq!(
        orchestrator.admit_external_fence("op-37", &digest, 107),
        Ok(FailoverOrchestrationAction::AlreadyFenced)
    );
    assert_eq!(
        orchestrator.commit("op-37", &digest),
        Ok(FailoverOrchestrationAction::Committed)
    );
    assert_eq!(
        orchestrator.commit("op-37", &digest),
        Ok(FailoverOrchestrationAction::AlreadyCommitted)
    );
    let report = orchestrator.report();
    assert_eq!(report.phase, FailoverOrchestrationPhase::Committed);
    assert_eq!(report.active_region_id.as_deref(), Some("region-b"));
    assert_eq!(report.committed_operation_id.as_deref(), Some("op-37"));
    assert!(report.safety_passed);
}

#[test]
fn conflicting_same_epoch_sequence_fails_closed() {
    let key = signing_key(17);
    let registry = registry("producer-a", &key);
    let required = BTreeSet::from([ConsensusTelemetryKind::LeaderHealth]);
    let mut orchestrator =
        FailoverOrchestrator::new("cluster-a", "resource-a", required, 50, Some("region-a"))
            .unwrap();
    orchestrator.detect_failure("op-conflict").unwrap();
    orchestrator
        .ingest(
            telemetry_event(
                "producer-a",
                &key,
                8,
                1,
                10,
                50,
                ConsensusTelemetryKind::LeaderHealth,
                "healthy",
            ),
            &registry,
            11,
        )
        .unwrap();
    let conflict = telemetry_event(
        "producer-a",
        &key,
        8,
        1,
        10,
        50,
        ConsensusTelemetryKind::LeaderHealth,
        "failed",
    );
    assert!(matches!(
        orchestrator.ingest(conflict, &registry, 11),
        Err(TelemetryFailoverError::TelemetryConflict(_))
    ));
    assert_eq!(
        orchestrator.report().phase,
        FailoverOrchestrationPhase::Failed
    );
    assert!(orchestrator.detect_failure("op-next").is_err());
}

#[test]
fn epoch_churn_fuzz_harnesses_never_panic_and_stay_bounded() {
    let transport = fuzz_authenticated_transport_receiver(0x37, 1_024);
    assert_eq!(transport.iterations, 1_024);
    assert_eq!(transport.panics, 0);
    assert!(transport.max_connection_epoch > 1);
    assert!(transport.safety_passed);
    assert_eq!(transport.trace_digest.len(), 64);

    let directory = tempdir().unwrap();
    let reservation =
        fuzz_witness_reservation_store(directory.path().join("reservations.json"), 0x73, 1_024);
    assert_eq!(reservation.iterations, 1_024);
    assert_eq!(reservation.panics, 0);
    assert!(reservation.max_connection_epoch > 1);
    assert!(reservation.safety_passed);
    assert_eq!(reservation.trace_digest.len(), 64);
}

#[test]
fn receiver_rejects_stale_epochs_without_mutating_journal() {
    let key = signing_key(19);
    let registry = registry("producer-a", &key);
    let mut receiver = SecureTelemetryReceiver::new("cluster-a", "resource-a", 4).unwrap();
    let current = telemetry_event(
        "producer-a",
        &key,
        9,
        3,
        30,
        50,
        ConsensusTelemetryKind::LeaderHealth,
        "healthy",
    );
    receiver.admit(current, &registry, 31).unwrap();
    let stale = telemetry_event(
        "producer-a",
        &key,
        8,
        99,
        32,
        50,
        ConsensusTelemetryKind::WitnessQuorum,
        "stale",
    );
    assert!(matches!(
        receiver.admit(stale, &registry, 33),
        Err(TelemetryFailoverError::ReplayRejected(_))
    ));
    assert_eq!(receiver.report().journal_length, 1);
    assert_eq!(receiver.report().rejected_events, 1);
}

#[test]
fn durable_failover_intent_round_trips_and_rolls_back_on_write_failure() {
    let key = signing_key(29);
    let registry = registry("producer-a", &key);
    let required = BTreeSet::from([ConsensusTelemetryKind::LeaderHealth]);
    let directory = tempdir().unwrap();
    let store = FailoverIntentStore::new(directory.path().join("intent.json"));
    let mut orchestrator = FailoverOrchestrator::new(
        "cluster-a",
        "resource-a",
        required.clone(),
        50,
        Some("region-a"),
    )
    .unwrap();
    orchestrator.detect_failure("op-durable").unwrap();
    orchestrator
        .ingest(
            telemetry_event(
                "producer-a",
                &key,
                12,
                1,
                50,
                50,
                ConsensusTelemetryKind::LeaderHealth,
                "failed",
            ),
            &registry,
            51,
        )
        .unwrap();
    let digest = "e".repeat(64);
    assert_eq!(
        orchestrator.begin_failover_with_store(&store, "op-durable", &digest, "region-b", 12, 51,),
        Ok(FailoverOrchestrationAction::AwaitingExternalFence)
    );
    let intent = store.load().unwrap().expect("intent should persist");
    let mut restored =
        FailoverOrchestrator::new("cluster-a", "resource-a", required, 50, Some("region-a"))
            .unwrap();
    restored.restore_intent(intent).unwrap();
    restored
        .admit_external_fence("op-durable", &digest, 52)
        .unwrap();
    restored.commit("op-durable", &digest).unwrap();
    assert_eq!(
        restored.report().active_region_id.as_deref(),
        Some("region-b")
    );

    let blocked_parent = directory.path().join("blocked-parent");
    fs::write(&blocked_parent, b"not a directory").unwrap();
    let blocked_store = FailoverIntentStore::new(blocked_parent.join("intent.json"));
    let mut rollback = FailoverOrchestrator::new(
        "cluster-a",
        "resource-a",
        BTreeSet::from([ConsensusTelemetryKind::LeaderHealth]),
        50,
        Some("region-a"),
    )
    .unwrap();
    rollback.detect_failure("op-rollback").unwrap();
    rollback
        .ingest(
            telemetry_event(
                "producer-a",
                &key,
                12,
                2,
                50,
                50,
                ConsensusTelemetryKind::LeaderHealth,
                "failed",
            ),
            &registry,
            51,
        )
        .unwrap();
    assert!(matches!(
        rollback.begin_failover_with_store(
            &blocked_store,
            "op-rollback",
            &digest,
            "region-b",
            12,
            51
        ),
        Err(TelemetryFailoverError::PersistenceFailed(_))
    ));
    assert_eq!(
        rollback.report().phase,
        FailoverOrchestrationPhase::CollectingEvidence
    );
}

#[test]
fn telemetry_journal_capacity_is_fail_closed() {
    let key = signing_key(23);
    let registry = registry("producer-a", &key);
    let mut receiver = SecureTelemetryReceiver::new("cluster-a", "resource-a", 1).unwrap();
    let first = telemetry_event(
        "producer-a",
        &key,
        1,
        1,
        1,
        100,
        ConsensusTelemetryKind::LeaderHealth,
        "healthy",
    );
    receiver.admit(first, &registry, 2).unwrap();
    let second = telemetry_event(
        "producer-a",
        &key,
        1,
        2,
        2,
        100,
        ConsensusTelemetryKind::WitnessQuorum,
        "available",
    );
    assert!(matches!(
        receiver.admit(second, &registry, 3),
        Err(TelemetryFailoverError::TelemetryRejected(_))
    ));
    assert_eq!(receiver.report().journal_length, 1);
    assert!(receiver.report().safety_passed);
}
