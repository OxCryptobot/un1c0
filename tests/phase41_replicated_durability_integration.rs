use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use tempfile::tempdir;
use un1c0::replicated_durability::{
    CasCommitOutcome, CasDurabilitySnapshotStore, CasWriteRequest,
    ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode, ReplicatedDurabilityError,
    SingleWriterCasStore,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn make_store() -> (SingleWriterCasStore, SigningKey, [SigningKey; 3]) {
    let writer = key(41);
    let replicas = [key(42), key(43), key(44)];
    let mut store =
        SingleWriterCasStore::new("cluster-a", "resource-a", "snapshot-a", 2, 16).unwrap();
    store
        .register_writer("writer-a", &writer.verifying_key())
        .unwrap();
    for (replica_id, replica_key) in ["replica-a", "replica-b", "replica-c"]
        .into_iter()
        .zip(replicas.iter())
    {
        store
            .register_replica(replica_id, &replica_key.verifying_key())
            .unwrap();
    }
    (store, writer, replicas)
}

fn request(
    writer: &SigningKey,
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
        "writer-a",
        1,
        nonce,
        expected_generation,
        expected_hash,
        proposed_generation,
        proposed_hash,
        proposed_hash,
        writer,
    )
    .unwrap()
}

fn acknowledgement(
    request: &CasWriteRequest,
    replica_id: &str,
    replica_key: &SigningKey,
    observed_tick: u64,
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
        observed_tick,
        50,
        replica_key,
    )
    .unwrap()
}

#[test]
fn quorum_gated_cas_commit_updates_state_atomically() {
    let (mut store, writer, replicas) = make_store();
    let request = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0], 100),
        acknowledgement(&request, "replica-b", &replicas[1], 100),
    ];
    let outcome = store.commit(request, &acknowledgements, 105).unwrap();
    let receipt = match outcome {
        CasCommitOutcome::Committed(receipt) => receipt,
        other => panic!("unexpected outcome: {other:?}"),
    };
    assert_eq!(receipt.quorum_count, 2);
    assert_eq!(store.state().generation, 1);
    assert_eq!(store.state().content_hash, hash('a'));
    assert_eq!(store.committed_request_count(), 1);
}

#[test]
fn quorum_loss_fails_closed_without_mutating_cas_state() {
    let (mut store, writer, replicas) = make_store();
    let request = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let only_ack = [acknowledgement(&request, "replica-a", &replicas[0], 100)];
    assert!(matches!(
        store.commit(request, &only_ack, 105),
        Err(ReplicatedDurabilityError::QuorumUnavailable)
    ));
    assert_eq!(store.state().generation, 0);
    assert_eq!(store.state().content_hash, hash('0'));
    assert_eq!(store.committed_request_count(), 0);
}

#[test]
fn stale_expected_generation_is_rejected_without_mutation() {
    let (mut store, writer, replicas) = make_store();
    let first = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let first_acks = [
        acknowledgement(&first, "replica-a", &replicas[0], 100),
        acknowledgement(&first, "replica-b", &replicas[1], 100),
    ];
    store.commit(first, &first_acks, 105).unwrap();
    let stale = request(&writer, "nonce-2", 0, &hash('0'), 1, &hash('b'));
    assert!(matches!(
        store.commit(stale, &[], 105),
        Err(ReplicatedDurabilityError::CasMismatch)
    ));
    assert_eq!(store.state().generation, 1);
    assert_eq!(store.state().content_hash, hash('a'));
}

#[test]
fn exact_request_retry_is_idempotent_but_nonce_reuse_conflicts() {
    let (mut store, writer, replicas) = make_store();
    let first = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let first_acks = [
        acknowledgement(&first, "replica-a", &replicas[0], 100),
        acknowledgement(&first, "replica-b", &replicas[1], 100),
    ];
    let first_receipt = match store.commit(first.clone(), &first_acks, 105).unwrap() {
        CasCommitOutcome::Committed(receipt) => receipt,
        other => panic!("unexpected outcome: {other:?}"),
    };
    let retry = store.commit(first, &[], 105).unwrap();
    assert!(matches!(retry, CasCommitOutcome::Idempotent(receipt) if receipt == first_receipt));

    let conflict = request(&writer, "nonce-1", 1, &hash('a'), 2, &hash('b'));
    assert!(matches!(
        store.commit(conflict, &[], 105),
        Err(ReplicatedDurabilityError::Conflict(_))
    ));
}

#[test]
fn conflicting_same_replica_acknowledgements_fail_closed() {
    let (mut store, writer, replicas) = make_store();
    let request = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0], 100),
        acknowledgement(&request, "replica-a", &replicas[0], 101),
    ];
    assert!(matches!(
        store.commit(request, &acknowledgements, 105),
        Err(ReplicatedDurabilityError::Conflict(_))
    ));
    assert_eq!(store.state().generation, 0);
}

#[test]
fn future_or_expired_replica_evidence_is_rejected() {
    let (mut store, writer, replicas) = make_store();
    let request = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let future = [
        acknowledgement(&request, "replica-a", &replicas[0], 106),
        acknowledgement(&request, "replica-b", &replicas[1], 100),
    ];
    assert!(matches!(
        store.commit(request.clone(), &future, 105),
        Err(ReplicatedDurabilityError::Rejected(_))
    ));
    let expired = [
        acknowledgement(&request, "replica-a", &replicas[0], 1),
        acknowledgement(&request, "replica-b", &replicas[1], 1),
    ];
    assert!(matches!(
        store.commit(request, &expired, 105),
        Err(ReplicatedDurabilityError::Rejected(_))
    ));
    assert_eq!(store.state().generation, 0);
}

#[test]
fn durable_cas_snapshot_round_trips_and_tampering_is_rejected() {
    let (mut store, writer, replicas) = make_store();
    let request = request(&writer, "nonce-1", 0, &hash('0'), 1, &hash('a'));
    let acknowledgements = [
        acknowledgement(&request, "replica-a", &replicas[0], 100),
        acknowledgement(&request, "replica-b", &replicas[1], 100),
    ];
    store.commit(request, &acknowledgements, 105).unwrap();
    let directory = tempdir().unwrap();
    let path = directory.path().join("cas.json");
    let snapshot_store =
        CasDurabilitySnapshotStore::new(&path, "cluster-a", "resource-a", "snapshot-a").unwrap();
    snapshot_store.save(&store.snapshot().unwrap()).unwrap();
    let loaded = snapshot_store.load().unwrap().unwrap();
    let (mut restored, _, _) = make_store();
    restored.restore(loaded).unwrap();
    assert_eq!(restored.state(), store.state());
    assert_eq!(restored.committed_request_count(), 1);

    let mut tampered = std::fs::read(&path).unwrap();
    tampered[0] ^= 1;
    std::fs::write(&path, tampered).unwrap();
    assert!(matches!(
        snapshot_store.load(),
        Err(ReplicatedDurabilityError::PersistenceFailed(_))
            | Err(ReplicatedDurabilityError::Rejected(_))
    ));
}

#[test]
fn writer_key_rebinding_is_rejected() {
    let (mut store, _, _) = make_store();
    assert!(matches!(
        store.register_writer("writer-a", &key(99).verifying_key()),
        Err(ReplicatedDurabilityError::Rejected(_))
    ));
}

#[test]
fn snapshot_contains_only_bounded_request_receipts() {
    let (store, _, _) = make_store();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.committed_requests.len(), 0);
    assert_eq!(snapshot.state_hash.len(), 64);
}

#[allow(dead_code)]
fn _registry_fixture() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::new()
}
