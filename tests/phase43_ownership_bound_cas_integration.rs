use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::ownership_bound_cas::{OwnershipBoundCasCoordinator, OwnershipBoundCasError};
use un1c0::replicated_durability::{
    CasCommitOutcome, CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn coordinator() -> (
    tempfile::TempDir,
    OwnershipBoundCasCoordinator,
    SigningKey,
    [SigningKey; 3],
) {
    let directory = tempdir().unwrap();
    let owner = key(81);
    let replicas = [key(82), key(83), key(84)];
    let ownership_path = directory.path().join("ownership.json");
    let cas_path = directory.path().join("cas.json");
    let mut coordinator = OwnershipBoundCasCoordinator::new(
        ownership_path,
        cas_path,
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
        16,
    )
    .unwrap();
    coordinator
        .register_owner("owner-a", &owner.verifying_key())
        .unwrap();
    for (replica_id, replica_key) in ["replica-a", "replica-b", "replica-c"]
        .into_iter()
        .zip(replicas.iter())
    {
        coordinator
            .register_replica(replica_id, &replica_key.verifying_key())
            .unwrap();
    }
    (directory, coordinator, owner, replicas)
}

fn claim(
    owner: &SigningKey,
    epoch: u64,
    expected_record_hash: &str,
    expiry_tick: u64,
    generation: u64,
    content_hash: &str,
) -> un1c0::cross_process_ownership::OwnershipClaim {
    un1c0::cross_process_ownership::OwnershipClaim::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "owner-a",
        "process-a",
        expected_record_hash,
        epoch,
        expiry_tick,
        generation,
        content_hash,
        &format!("fence-{epoch}"),
        owner,
    )
    .unwrap()
}

fn request(
    owner: &SigningKey,
    epoch: u64,
    nonce: &str,
    expected_generation: u64,
    expected_hash: &str,
    proposed_generation: u64,
    proposed_hash: &str,
) -> CasWriteRequest {
    CasWriteRequest::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "owner-a",
        epoch,
        nonce,
        expected_generation,
        expected_hash,
        proposed_generation,
        proposed_hash,
        proposed_hash,
        owner,
    )
    .unwrap()
}

fn acknowledgement(
    request: &CasWriteRequest,
    replica_id: &str,
    replica_key: &SigningKey,
) -> ReplicaDurabilityAcknowledgement {
    ReplicaDurabilityAcknowledgement::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        &request.request_hash,
        request.proposed_generation,
        &request.proposed_hash,
        replica_id,
        ReplicaDurabilityMode::ReplicatedVolume,
        7,
        100,
        50,
        replica_key,
    )
    .unwrap()
}

fn acquire_first(
    coordinator: &OwnershipBoundCasCoordinator,
    owner: &SigningKey,
) -> un1c0::cross_process_ownership::OwnershipRecord {
    coordinator
        .acquire(
            claim(
                owner,
                1,
                un1c0::cross_process_ownership::ZERO_HASH,
                200,
                0,
                &hash('0'),
            ),
            1,
        )
        .unwrap()
}

#[test]
fn successful_commit_updates_ownership_and_cas_under_one_guard() {
    let (_directory, mut coordinator, owner, replicas) = coordinator();
    let record = acquire_first(&coordinator, &owner);
    let permit = coordinator
        .admit_write("owner-a", "process-a", 1, &record.record_hash, 10)
        .unwrap();
    let request = request(&owner, 1, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0]),
        acknowledgement(&request, "replica-b", &replicas[1]),
    ];
    let receipt = coordinator
        .commit_owned(permit, request, &acknowledgements, 105)
        .unwrap();
    assert_eq!(receipt.generation, 1);
    assert_eq!(coordinator.cas_state().generation, 1);
    assert_eq!(coordinator.cas_state().content_hash, hash('a'));
    let updated = coordinator.current_owner().unwrap().unwrap();
    assert_eq!(updated.generation, 1);
    assert_eq!(updated.content_hash, hash('a'));
    assert_eq!(updated.record_hash, receipt.ownership_record_hash);
}

#[test]
fn quorum_failure_preserves_both_ownership_and_cas_state() {
    let (_directory, mut coordinator, owner, replicas) = coordinator();
    let record = acquire_first(&coordinator, &owner);
    let permit = coordinator
        .admit_write("owner-a", "process-a", 1, &record.record_hash, 10)
        .unwrap();
    let request = request(&owner, 1, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let only_ack = [acknowledgement(&request, "replica-a", &replicas[0])];
    assert!(matches!(
        coordinator.commit_owned(permit, request, &only_ack, 105),
        Err(OwnershipBoundCasError::Cas(
            un1c0::replicated_durability::ReplicatedDurabilityError::QuorumUnavailable
        ))
    ));
    let unchanged = coordinator.current_owner().unwrap().unwrap();
    assert_eq!(unchanged.generation, 0);
    assert_eq!(unchanged.content_hash, hash('0'));
    assert_eq!(coordinator.cas_state().generation, 0);
}

#[test]
fn stale_request_epoch_cannot_use_a_current_ownership_permit() {
    let (_directory, mut coordinator, owner, replicas) = coordinator();
    let record = acquire_first(&coordinator, &owner);
    let permit = coordinator
        .admit_write("owner-a", "process-a", 1, &record.record_hash, 10)
        .unwrap();
    let request = request(&owner, 2, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0]),
        acknowledgement(&request, "replica-b", &replicas[1]),
    ];
    assert!(matches!(
        coordinator.commit_owned(permit, request, &acknowledgements, 105),
        Err(OwnershipBoundCasError::StalePermit(_))
    ));
    assert_eq!(coordinator.cas_state().generation, 0);
}

#[test]
fn released_owner_cannot_reuse_a_permit_after_higher_epoch_replacement() {
    let (_directory, mut coordinator, owner, replicas) = coordinator();
    let record = acquire_first(&coordinator, &owner);
    let permit = coordinator
        .admit_write("owner-a", "process-a", 1, &record.record_hash, 10)
        .unwrap();
    let released = coordinator
        .release("owner-a", "process-a", 1, &record.record_hash, 20)
        .unwrap();
    let replacement = coordinator
        .acquire(
            claim(&owner, 2, &released.record_hash, 200, 0, &hash('0')),
            20,
        )
        .unwrap();
    assert_eq!(replacement.ownership_epoch, 2);
    let request = request(&owner, 1, "nonce-old", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0]),
        acknowledgement(&request, "replica-b", &replicas[1]),
    ];
    assert!(matches!(
        coordinator.commit_owned(permit, request, &acknowledgements, 25),
        Err(OwnershipBoundCasError::Ownership(_))
    ));
    assert_eq!(coordinator.cas_state().generation, 0);
}

#[test]
fn exact_retry_is_idempotent_without_advancing_ownership_again() {
    let (_directory, mut coordinator, owner, replicas) = coordinator();
    let record = acquire_first(&coordinator, &owner);
    let permit = coordinator
        .admit_write("owner-a", "process-a", 1, &record.record_hash, 10)
        .unwrap();
    let request = request(&owner, 1, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0]),
        acknowledgement(&request, "replica-b", &replicas[1]),
    ];
    coordinator
        .commit_owned(permit, request.clone(), &acknowledgements, 105)
        .unwrap();
    let updated = coordinator.current_owner().unwrap().unwrap();
    let retry_permit = coordinator
        .admit_write("owner-a", "process-a", 1, &updated.record_hash, 110)
        .unwrap();
    let retry = coordinator
        .commit_owned(retry_permit, request, &[], 110)
        .unwrap();
    assert!(matches!(retry.outcome, CasCommitOutcome::Idempotent(_)));
    let after = coordinator.current_owner().unwrap().unwrap();
    assert_eq!(after.record_hash, updated.record_hash);
    assert_eq!(after.generation, 1);
}
