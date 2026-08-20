use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;
use un1c0::cross_process_ownership::{OwnershipClaim, OwnershipRecord, ZERO_HASH};
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::ownership_bound_cas_executor::{
    OwnershipBoundCasExecutor, OwnershipBoundCasExecutorError, OwnershipBoundCasIntent,
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
    let owner = key(101);
    let replicas = [key(102), key(103)];
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
            &format!("executor-nonce-{cycle}"),
            &proposed_hash,
        ));
        shadow.generation += 1;
        shadow.content_hash = proposed_hash;
        shadow.record_hash = record_hash(&shadow);
    }
    intents
}

#[test]
fn concurrent_producers_serialize_conflicts_and_report_latency() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let intents = (0..16u64)
        .map(|cycle| {
            let proposed_hash = hash(b"0123456789abcdef"[(cycle as usize) % 16] as char);
            intent(
                &initial,
                &owner,
                &replicas,
                cycle,
                &format!("conflict-nonce-{cycle}"),
                &proposed_hash,
            )
        })
        .collect::<Vec<_>>();
    let executor = Arc::new(OwnershipBoundCasExecutor::new(coordinator, 32).unwrap());
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for producer in 0..4 {
        let executor = Arc::clone(&executor);
        let barrier = Arc::clone(&barrier);
        let batch = intents[producer * 4..producer * 4 + 4].to_vec();
        handles.push(thread::spawn(move || {
            barrier.wait();
            batch
                .into_iter()
                .map(|intent| executor.submit(intent).unwrap().wait().unwrap())
                .collect::<Vec<_>>()
        }));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.extend(handle.join().unwrap());
    }
    assert_eq!(outcomes.len(), 16);
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        15
    );
    let metrics = executor.metrics();
    assert_eq!(metrics.accepted_intents, 16);
    assert_eq!(metrics.completed_intents, 16);
    assert_eq!(metrics.failed_intents, 15);
    assert_eq!(metrics.latency_sample_count, 16);
    assert!(metrics.end_to_end_p95_us >= metrics.end_to_end_p50_us);
    let mut executor = Arc::try_unwrap(executor).unwrap();
    executor.close().unwrap();
}

#[test]
fn queue_full_is_deterministic_before_worker_release() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let gate = Arc::new(Barrier::new(2));
    let mut executor =
        OwnershipBoundCasExecutor::new_with_worker_start_gate(coordinator, 1, Arc::clone(&gate))
            .unwrap();
    let first = executor.submit(sequential_intents(&initial, &owner, &replicas, 1)[0].clone());
    assert!(first.is_ok());
    let second = executor.submit(sequential_intents(&initial, &owner, &replicas, 1)[0].clone());
    assert!(matches!(
        second,
        Err(OwnershipBoundCasExecutorError::QueueFull)
    ));
    assert_eq!(executor.metrics().queue_full_rejections, 1);
    gate.wait();
    first.unwrap().wait().unwrap().unwrap();
    executor.close().unwrap();
}

#[test]
fn stale_generation_failure_does_not_advance_state() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let first = intent(&initial, &owner, &replicas, 0, "stale-first", &hash('a'));
    let second = intent(&initial, &owner, &replicas, 1, "stale-second", &hash('b'));
    let mut executor = OwnershipBoundCasExecutor::new(coordinator, 4).unwrap();
    let first_ticket = executor.submit(first).unwrap();
    let second_ticket = executor.submit(second).unwrap();
    first_ticket.wait().unwrap().unwrap();
    assert!(second_ticket.wait().unwrap().is_err());
    assert_eq!(executor.metrics().failed_intents, 1);
    executor.close().unwrap();
}

#[test]
fn shutdown_rejects_new_intents_and_records_the_boundary() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let mut executor = OwnershipBoundCasExecutor::new(coordinator, 2).unwrap();
    executor.close().unwrap();
    let intent = sequential_intents(&initial, &owner, &replicas, 1)[0].clone();
    assert!(matches!(
        executor.submit(intent),
        Err(OwnershipBoundCasExecutorError::Shutdown)
    ));
    assert_eq!(executor.metrics().shutdown_rejections, 1);
}

#[test]
fn fifo_worker_processes_prebuilt_valid_intents_in_generation_order() {
    let (_directory, coordinator, owner, replicas, initial) = fixture();
    let intents = sequential_intents(&initial, &owner, &replicas, 8);
    let mut executor = OwnershipBoundCasExecutor::new(coordinator, 8).unwrap();
    let tickets = intents
        .into_iter()
        .map(|intent| executor.submit(intent).unwrap())
        .collect::<Vec<_>>();
    let receipts = tickets
        .into_iter()
        .map(|ticket| ticket.wait().unwrap().unwrap())
        .collect::<Vec<_>>();
    let generations = receipts
        .iter()
        .map(|receipt| receipt.generation)
        .collect::<Vec<_>>();
    assert_eq!(generations, (1..=8).collect::<Vec<_>>());
    assert_eq!(executor.metrics().failed_intents, 0);
    executor.close().unwrap();
}
