use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use un1c0::cross_process_ownership::{
    OwnershipClaim, OwnershipRecord, OwnershipWritePermit, ZERO_HASH,
};
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::ownership_bound_cas_admission::{
    AdaptiveAdmissionConfig, AdaptiveAdmissionError, AdaptiveOwnershipBoundCasAdmission,
};
use un1c0::ownership_bound_cas_executor::OwnershipBoundCasIntent;
use un1c0::replicated_durability::{
    CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn record_hash(record: &OwnershipRecord) -> String {
    let bytes = serde_json::to_vec(&(
        &record.cluster_id,
        &record.resource_id,
        &record.snapshot_id,
        &record.owner_id,
        &record.process_instance,
        record.ownership_epoch,
        record.lease_expiry_tick,
        record.generation,
        &record.content_hash,
        &record.fencing_nonce,
        record.fenced,
    ))
    .unwrap();
    format!("{:x}", Sha256::digest(bytes))
}

fn fixture() -> (
    tempfile::TempDir,
    OwnershipBoundCasCoordinator,
    SigningKey,
    [SigningKey; 2],
    OwnershipRecord,
) {
    let directory = tempdir().unwrap();
    let owner = key(141);
    let replicas = [key(142), key(143)];
    let mut coordinator = OwnershipBoundCasCoordinator::new(
        directory.path().join("ownership.json"),
        directory.path().join("cas.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
        128,
    )
    .unwrap();
    coordinator
        .register_owner("owner-a", &owner.verifying_key())
        .unwrap();
    coordinator
        .register_replica("replica-a", &replicas[0].verifying_key())
        .unwrap();
    coordinator
        .register_replica("replica-b", &replicas[1].verifying_key())
        .unwrap();
    let record = coordinator
        .acquire(
            OwnershipClaim::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                "owner-a",
                "process-a",
                ZERO_HASH,
                1,
                100_000,
                0,
                &hash('0'),
                "fence-a",
                &owner,
            )
            .unwrap(),
            1,
        )
        .unwrap();
    (directory, coordinator, owner, replicas, record)
}

fn intent(
    record: &OwnershipRecord,
    owner: &SigningKey,
    replicas: &[SigningKey; 2],
    nonce: &str,
    proposed_hash: &str,
) -> OwnershipBoundCasIntent {
    let permit = OwnershipWritePermit {
        owner_id: record.owner_id.clone(),
        process_instance: record.process_instance.clone(),
        ownership_epoch: record.ownership_epoch,
        record_hash: record.record_hash.clone(),
    };
    let request = CasWriteRequest::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "owner-a",
        record.ownership_epoch,
        nonce,
        record.generation,
        &record.content_hash,
        record.generation + 1,
        proposed_hash,
        proposed_hash,
        owner,
    )
    .unwrap();
    let acknowledgements = vec![
        ReplicaDurabilityAcknowledgement::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &request.request_hash,
            request.proposed_generation,
            &request.proposed_hash,
            "replica-a",
            ReplicaDurabilityMode::ReplicatedVolume,
            7,
            100,
            50,
            &replicas[0],
        )
        .unwrap(),
        ReplicaDurabilityAcknowledgement::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &request.request_hash,
            request.proposed_generation,
            &request.proposed_hash,
            "replica-b",
            ReplicaDurabilityMode::ReplicatedVolume,
            7,
            100,
            50,
            &replicas[1],
        )
        .unwrap(),
    ];
    OwnershipBoundCasIntent {
        permit,
        request,
        acknowledgements,
        current_tick: 105,
    }
}

#[test]
fn limiter_rejects_without_consuming_an_intent_id_and_recovers_after_completion() {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let config = AdaptiveAdmissionConfig {
        initial_permits: 1,
        minimum_permits: 1,
        maximum_permits: 2,
        target_service_p95_us: 100_000,
        failure_threshold: 8,
        adjustment_window: 16,
    };
    let mut admission = AdaptiveOwnershipBoundCasAdmission::new(coordinator, 2, 4, config).unwrap();
    let first = admission
        .submit(intent(
            &record,
            &owner,
            &replicas,
            "admission-1",
            &hash('a'),
        ))
        .unwrap();
    assert_eq!(first.intent_id, 1);
    assert!(matches!(
        admission.submit(intent(
            &record,
            &owner,
            &replicas,
            "admission-2",
            &hash('b')
        )),
        Err(AdaptiveAdmissionError::Limited { .. })
    ));
    first.wait().unwrap();
    let third = admission
        .submit(intent(
            &record,
            &owner,
            &replicas,
            "admission-3",
            &hash('c'),
        ))
        .unwrap();
    assert_eq!(third.intent_id, 2);
    let _ = third.wait();
    let metrics = admission.metrics();
    assert_eq!(metrics.admission.limiter_rejections, 1);
    assert_eq!(metrics.verifier.submitted_intents, 2);
    admission.close().unwrap();
}

#[test]
fn failure_window_multiplicatively_decreases_permits() {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let config = AdaptiveAdmissionConfig {
        initial_permits: 4,
        minimum_permits: 1,
        maximum_permits: 8,
        target_service_p95_us: 1,
        failure_threshold: 1,
        adjustment_window: 2,
    };
    let mut admission = AdaptiveOwnershipBoundCasAdmission::new(coordinator, 2, 4, config).unwrap();
    let mut forged = intent(&record, &owner, &replicas, "decrease-1", &hash('a'));
    forged.acknowledgements[0].signature[0] ^= 1;
    let first = admission.submit(forged).unwrap();
    let second = admission
        .submit(intent(&record, &owner, &replicas, "decrease-2", &hash('b')))
        .unwrap();
    assert!(first.wait().is_err());
    assert!(second.wait().is_ok());
    let metrics = admission.metrics();
    assert_eq!(metrics.admission.permits, 2);
    assert_eq!(metrics.admission.total_completions, 2);
    assert_eq!(metrics.admission.total_failures, 1);
    admission.close().unwrap();
}

#[test]
fn parsed_key_and_exact_fact_cache_reuse_is_context_bound() {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let context = coordinator.pre_admission_context().unwrap();
    let candidate = intent(&record, &owner, &replicas, "cache-1", &hash('a'));
    context
        .verify(
            &candidate.request,
            &candidate.acknowledgements,
            candidate.current_tick,
        )
        .unwrap();
    let first = context.cache_metrics();
    assert_eq!(first.cache_hits, 0);
    assert_eq!(first.cache_misses, 3);
    context
        .verify(
            &candidate.request,
            &candidate.acknowledgements,
            candidate.current_tick + 1,
        )
        .unwrap();
    let second = context.cache_metrics();
    assert_eq!(second.cache_hits, 3);
    assert_eq!(second.cache_misses, 3);
    assert!(second.cache_entries >= 3);
}

#[test]
fn cache_does_not_bypass_freshness_or_request_binding() {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let context = coordinator.pre_admission_context().unwrap();
    let candidate = intent(&record, &owner, &replicas, "cache-2", &hash('a'));
    context
        .verify(
            &candidate.request,
            &candidate.acknowledgements,
            candidate.current_tick,
        )
        .unwrap();
    assert!(context
        .verify(
            &candidate.request,
            &candidate.acknowledgements,
            candidate.current_tick + 51,
        )
        .is_err());
}
