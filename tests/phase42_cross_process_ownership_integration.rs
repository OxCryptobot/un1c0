use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::cross_process_ownership::{
    CrossProcessOwnershipStore, ManagedVolumeRecoveryEvidence, ManagedVolumeRecoveryGate,
    ManagedVolumeRecoveryState, OwnershipClaim, OwnershipError, OwnershipRecord,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn store_fixture() -> (
    tempfile::TempDir,
    CrossProcessOwnershipStore,
    SigningKey,
    SigningKey,
) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("ownership.json");
    let owner_a = key(51);
    let owner_b = key(52);
    let mut store =
        CrossProcessOwnershipStore::new(&path, "cluster-a", "resource-a", "snapshot-a").unwrap();
    store
        .register_owner("owner-a", &owner_a.verifying_key())
        .unwrap();
    store
        .register_owner("owner-b", &owner_b.verifying_key())
        .unwrap();
    (directory, store, owner_a, owner_b)
}

fn claim(
    signing_key: &SigningKey,
    owner_id: &str,
    process_instance: &str,
    expected_record_hash: &str,
    epoch: u64,
    expiry: u64,
    generation: u64,
    content_hash: &str,
    nonce: &str,
) -> OwnershipClaim {
    OwnershipClaim::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        owner_id,
        process_instance,
        expected_record_hash,
        epoch,
        expiry,
        generation,
        content_hash,
        nonce,
        signing_key,
    )
    .unwrap()
}

fn recovery_evidence(
    record: &OwnershipRecord,
    replica_id: &str,
    replica_key: &SigningKey,
    state: ManagedVolumeRecoveryState,
    observed_tick: u64,
) -> ManagedVolumeRecoveryEvidence {
    ManagedVolumeRecoveryEvidence::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        record.generation,
        &record.content_hash,
        record.ownership_epoch,
        replica_id,
        &format!("adapter-{replica_id}"),
        state,
        10,
        20,
        observed_tick,
        50,
        replica_key,
    )
    .unwrap()
}

#[test]
fn first_owner_acquisition_is_atomic_and_second_owner_is_blocked() {
    let (_directory, store, owner_a, owner_b) = store_fixture();
    let first_claim = claim(
        &owner_a,
        "owner-a",
        "process-a",
        un1c0::cross_process_ownership::ZERO_HASH,
        1,
        20,
        0,
        &hash('0'),
        "fence-a",
    );
    let first = store.acquire(first_claim, 1).unwrap();
    assert_eq!(first.ownership_epoch, 1);
    assert!(!first.fenced);

    let second_claim = claim(
        &owner_b,
        "owner-b",
        "process-b",
        &first.record_hash,
        2,
        40,
        1,
        &hash('a'),
        "fence-b",
    );
    assert!(matches!(
        store.acquire(second_claim, 10),
        Err(OwnershipError::Busy)
    ));
}

#[test]
fn expired_owner_can_be_replaced_only_by_a_higher_epoch() {
    let (_directory, store, owner_a, owner_b) = store_fixture();
    let first = store
        .acquire(
            claim(
                &owner_a,
                "owner-a",
                "process-a",
                un1c0::cross_process_ownership::ZERO_HASH,
                1,
                20,
                0,
                &hash('0'),
                "fence-a",
            ),
            1,
        )
        .unwrap();
    let stale = claim(
        &owner_b,
        "owner-b",
        "process-b",
        &first.record_hash,
        1,
        40,
        1,
        &hash('b'),
        "fence-b",
    );
    assert!(matches!(
        store.acquire(stale, 20),
        Err(OwnershipError::StaleEpoch)
    ));
    let replacement = store
        .acquire(
            claim(
                &owner_b,
                "owner-b",
                "process-b",
                &first.record_hash,
                2,
                40,
                1,
                &hash('b'),
                "fence-b",
            ),
            20,
        )
        .unwrap();
    assert_eq!(replacement.owner_id, "owner-b");
    assert_eq!(replacement.ownership_epoch, 2);
}

#[test]
fn renewal_release_and_write_permit_are_owner_bound() {
    let (_directory, store, owner_a, _owner_b) = store_fixture();
    let record = store
        .acquire(
            claim(
                &owner_a,
                "owner-a",
                "process-a",
                un1c0::cross_process_ownership::ZERO_HASH,
                1,
                20,
                0,
                &hash('0'),
                "fence-a",
            ),
            1,
        )
        .unwrap();
    assert!(store
        .renew("owner-a", "wrong-process", 1, &record.record_hash, 30, 10,)
        .is_err());
    let renewed = store
        .renew("owner-a", "process-a", 1, &record.record_hash, 30, 10)
        .unwrap();
    let permit = store
        .admit_write("owner-a", "process-a", 1, &renewed.record_hash, 11)
        .unwrap();
    assert_eq!(permit.ownership_epoch, 1);
    let released = store
        .release("owner-a", "process-a", 1, &renewed.record_hash, 12)
        .unwrap();
    assert!(released.fenced);
    assert!(matches!(
        store.admit_write("owner-a", "process-a", 1, &released.record_hash, 12),
        Err(OwnershipError::LeaseExpired)
    ));
}

#[test]
fn stale_staging_is_removed_and_corrupt_record_is_rejected() {
    let (directory, store, owner_a, _owner_b) = store_fixture();
    let staging = directory.path().join("ownership.staging");
    std::fs::write(&staging, b"stale").unwrap();
    assert!(store.current().unwrap().is_none());
    assert!(!staging.exists());
    std::fs::write(
        directory.path().join("ownership.json"),
        br#"{"cluster_id":"cluster-a","resource_id":"resource-a"}"#,
    )
    .unwrap();
    assert!(matches!(
        store.current(),
        Err(OwnershipError::PersistenceFailed(_)) | Err(OwnershipError::Rejected(_))
    ));
    let _ = owner_a;
}

#[test]
fn recovery_quorum_requires_distinct_fresh_replicated_evidence() {
    let (_directory, store, owner_a, _owner_b) = store_fixture();
    let record = store
        .acquire(
            claim(
                &owner_a,
                "owner-a",
                "process-a",
                un1c0::cross_process_ownership::ZERO_HASH,
                1,
                20,
                1,
                &hash('a'),
                "fence-a",
            ),
            1,
        )
        .unwrap();
    let replica_a = key(61);
    let replica_b = key(62);
    let replica_c = key(63);
    let mut gate =
        ManagedVolumeRecoveryGate::new("cluster-a", "resource-a", "snapshot-a", 2).unwrap();
    gate.register_replica("replica-a", &replica_a.verifying_key())
        .unwrap();
    gate.register_replica("replica-b", &replica_b.verifying_key())
        .unwrap();
    gate.register_replica("replica-c", &replica_c.verifying_key())
        .unwrap();
    let decision = gate
        .admit(
            &record,
            &[
                recovery_evidence(
                    &record,
                    "replica-a",
                    &replica_a,
                    ManagedVolumeRecoveryState::Replicated,
                    100,
                ),
                recovery_evidence(
                    &record,
                    "replica-b",
                    &replica_b,
                    ManagedVolumeRecoveryState::Recovered,
                    100,
                ),
            ],
            105,
        )
        .unwrap();
    assert_eq!(decision.state, ManagedVolumeRecoveryState::Recovered);
    assert_eq!(decision.evidence_count, 2);
}

#[test]
fn unknown_failed_stale_and_conflicting_recovery_evidence_fail_closed() {
    let (_directory, store, owner_a, _owner_b) = store_fixture();
    let record = store
        .acquire(
            claim(
                &owner_a,
                "owner-a",
                "process-a",
                un1c0::cross_process_ownership::ZERO_HASH,
                1,
                20,
                1,
                &hash('a'),
                "fence-a",
            ),
            1,
        )
        .unwrap();
    let replica_a = key(71);
    let replica_b = key(72);
    let mut gate =
        ManagedVolumeRecoveryGate::new("cluster-a", "resource-a", "snapshot-a", 2).unwrap();
    gate.register_replica("replica-a", &replica_a.verifying_key())
        .unwrap();
    gate.register_replica("replica-b", &replica_b.verifying_key())
        .unwrap();
    let failed = [
        recovery_evidence(
            &record,
            "replica-a",
            &replica_a,
            ManagedVolumeRecoveryState::Failed,
            100,
        ),
        recovery_evidence(
            &record,
            "replica-b",
            &replica_b,
            ManagedVolumeRecoveryState::Replicated,
            100,
        ),
    ];
    assert!(matches!(
        gate.admit(&record, &failed, 105),
        Err(OwnershipError::Rejected(_))
    ));
    let stale = [
        recovery_evidence(
            &record,
            "replica-a",
            &replica_a,
            ManagedVolumeRecoveryState::Replicated,
            1,
        ),
        recovery_evidence(
            &record,
            "replica-b",
            &replica_b,
            ManagedVolumeRecoveryState::Replicated,
            1,
        ),
    ];
    assert!(matches!(
        gate.admit(&record, &stale, 105),
        Err(OwnershipError::Rejected(_))
    ));
    let one = recovery_evidence(
        &record,
        "replica-a",
        &replica_a,
        ManagedVolumeRecoveryState::Replicated,
        100,
    );
    let conflicting = recovery_evidence(
        &record,
        "replica-a",
        &replica_a,
        ManagedVolumeRecoveryState::Replicated,
        101,
    );
    assert!(matches!(
        gate.admit(&record, &[one, conflicting], 105),
        Err(OwnershipError::Conflict(_))
    ));
    let one = recovery_evidence(
        &record,
        "replica-a",
        &replica_a,
        ManagedVolumeRecoveryState::Replicated,
        100,
    );
    assert!(matches!(
        gate.admit(&record, &[one], 105),
        Err(OwnershipError::QuorumUnavailable)
    ));
}

#[test]
fn ownership_snapshot_is_bounded_and_sanitized() {
    let (_directory, store, owner_a, _owner_b) = store_fixture();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.cluster_id, "cluster-a");
    assert_eq!(snapshot.resource_id, "resource-a");
    assert_eq!(snapshot.snapshot_hash.len(), 64);
    let _ = owner_a;
}
