use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;
use un1c0::cross_process_ownership::{OwnershipClaim, OwnershipRecord, ZERO_HASH};
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::ownership_bound_cas_executor::OwnershipBoundCasIntent;
use un1c0::ownership_bound_cas_verifier::{
    OwnershipBoundCasVerifierError, OwnershipBoundCasVerifierPipeline,
};
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
    let owner = key(121);
    let replicas = [key(122), key(123)];
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
    cycle: u64,
    nonce: &str,
    proposed_hash: &str,
) -> OwnershipBoundCasIntent {
    let permit = un1c0::cross_process_ownership::OwnershipWritePermit {
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
            100 + cycle,
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
            100 + cycle,
            50,
            &replicas[1],
        )
        .unwrap(),
    ];
    OwnershipBoundCasIntent {
        permit,
        request,
        acknowledgements,
        current_tick: 105 + cycle,
    }
}

fn sequential_intents(
    initial: &OwnershipRecord,
    owner: &SigningKey,
    replicas: &[SigningKey; 2],
    count: u64,
) -> Vec<OwnershipBoundCasIntent> {
    let mut shadow = initial.clone();
    let mut intents = Vec::new();
    for cycle in 0..count {
        let proposed_hash = hash(b"0123456789abcdef"[(cycle as usize) % 16] as char);
        intents.push(intent(
            &shadow,
            owner,
            replicas,
            cycle,
            &format!("verifier-nonce-{cycle}"),
            &proposed_hash,
        ));
        shadow.generation += 1;
        shadow.content_hash = proposed_hash;
        shadow.record_hash = record_hash(&shadow);
    }
    intents
}

#[test]
fn parallel_pre_admission_feeds_valid_intents_to_single_mutation_worker() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let intents = sequential_intents(&initial, &owner, &replicas, 8);
    let mut pipeline = OwnershipBoundCasVerifierPipeline::new(coordinator, 4, 8).unwrap();
    let tickets = intents
        .into_iter()
        .map(|intent| pipeline.submit(intent).unwrap())
        .collect::<Vec<_>>();
    let receipts = tickets
        .into_iter()
        .map(|ticket| ticket.wait().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 8);
    assert_eq!(receipts[0].generation, 1);
    assert_eq!(receipts.last().unwrap().generation, 8);
    let metrics = pipeline.metrics();
    assert_eq!(metrics.submitted_intents, 8);
    assert_eq!(metrics.pre_admitted_intents, 8);
    assert_eq!(metrics.pre_admission_failures, 0);
    assert_eq!(metrics.completed_intents, 8);
    assert_eq!(metrics.failed_intents, 0);
    assert_eq!(metrics.latency_sample_count, 8);
    pipeline.close().unwrap();
}

#[test]
fn forged_replica_evidence_fails_before_mutation_and_is_typed() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let mut forged = intent(&initial, &owner, &replicas, 0, "forged-ack", &hash('a'));
    forged.acknowledgements[0].signature[0] ^= 1;
    let mut pipeline = OwnershipBoundCasVerifierPipeline::new(coordinator, 2, 4).unwrap();
    let result = pipeline.submit(forged).unwrap().wait();
    assert!(matches!(
        result,
        Err(OwnershipBoundCasVerifierError::PreAdmission(_))
    ));
    let metrics = pipeline.metrics();
    assert_eq!(metrics.pre_admitted_intents, 0);
    assert_eq!(metrics.pre_admission_failures, 1);
    assert_eq!(metrics.completed_intents, 0);
    pipeline.close().unwrap();
}

#[test]
fn stale_conflicting_intents_pass_precheck_but_fail_under_mutation_revalidation() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let first = intent(&initial, &owner, &replicas, 0, "conflict-a", &hash('a'));
    let second = intent(&initial, &owner, &replicas, 1, "conflict-b", &hash('b'));
    let mut pipeline = OwnershipBoundCasVerifierPipeline::new(coordinator, 4, 4).unwrap();
    let first_ticket = pipeline.submit(first).unwrap();
    let second_ticket = pipeline.submit(second).unwrap();
    let first_result = first_ticket.wait();
    let second_result = second_ticket.wait();
    assert!(first_result.is_ok() ^ second_result.is_ok());
    let failed = if first_result.is_err() {
        first_result.unwrap_err()
    } else {
        second_result.unwrap_err()
    };
    assert!(matches!(
        failed,
        OwnershipBoundCasVerifierError::Mutation(_)
    ));
    let metrics = pipeline.metrics();
    assert_eq!(metrics.pre_admitted_intents, 2);
    assert_eq!(metrics.pre_admission_failures, 0);
    assert_eq!(metrics.completed_intents, 2);
    assert_eq!(metrics.failed_intents, 1);
    pipeline.close().unwrap();
}

#[test]
fn verification_queue_full_is_deterministic_before_worker_release() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let gate = Arc::new(Barrier::new(2));
    let mut pipeline = OwnershipBoundCasVerifierPipeline::new_with_verifier_start_gate(
        coordinator,
        1,
        1,
        Arc::clone(&gate),
    )
    .unwrap();
    let first = pipeline.submit(sequential_intents(&initial, &owner, &replicas, 1)[0].clone());
    assert!(first.is_ok());
    let second = pipeline.submit(sequential_intents(&initial, &owner, &replicas, 1)[0].clone());
    assert!(matches!(
        second,
        Err(OwnershipBoundCasVerifierError::VerificationQueueFull)
    ));
    assert_eq!(pipeline.metrics().verification_queue_full_rejections, 1);
    gate.wait();
    first.unwrap().wait().unwrap();
    pipeline.close().unwrap();
}

#[test]
fn concurrent_producers_keep_pre_admission_bounded_and_fail_closed() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let intents = (0..32u64)
        .map(|cycle| {
            intent(
                &initial,
                &owner,
                &replicas,
                cycle,
                &format!("stress-{cycle}"),
                &hash(b"0123456789abcdef"[(cycle as usize) % 16] as char),
            )
        })
        .collect::<Vec<_>>();
    let pipeline = Arc::new(OwnershipBoundCasVerifierPipeline::new(coordinator, 4, 32).unwrap());
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for producer in 0..4 {
        let pipeline = Arc::clone(&pipeline);
        let barrier = Arc::clone(&barrier);
        let batch = intents[producer * 8..producer * 8 + 8].to_vec();
        handles.push(thread::spawn(move || {
            barrier.wait();
            batch
                .into_iter()
                .map(|intent| pipeline.submit(intent).unwrap().wait())
                .collect::<Vec<_>>()
        }));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.extend(handle.join().unwrap());
    }
    assert_eq!(outcomes.len(), 32);
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 31);
    let metrics = pipeline.metrics();
    assert_eq!(metrics.submitted_intents, 32);
    assert_eq!(metrics.pre_admitted_intents, 32);
    assert_eq!(metrics.pre_admission_failures, 0);
    assert_eq!(metrics.completed_intents, 32);
    assert_eq!(metrics.failed_intents, 31);
    assert!(metrics.latency_sample_count <= metrics.latency_sample_cap);
    let mut pipeline = Arc::try_unwrap(pipeline).unwrap();
    pipeline.close().unwrap();
}

#[test]
fn shutdown_rejects_new_verification_intents() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let mut pipeline = OwnershipBoundCasVerifierPipeline::new(coordinator, 2, 2).unwrap();
    pipeline.close().unwrap();
    let intent = sequential_intents(&initial, &owner, &replicas, 1)[0].clone();
    assert!(matches!(
        pipeline.submit(intent),
        Err(OwnershipBoundCasVerifierError::Shutdown)
    ));
    assert_eq!(pipeline.metrics().shutdown_rejections, 1);
}
