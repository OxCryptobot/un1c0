use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::lease_migration::{
    LeaseMigrationActivation, LeaseMigrationAuthority, LeaseMigrationError, LeaseMigrationIntent,
    LeaseMigrationState, LeaseMigrationWitnessAck, LeaseRecord,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn setup() -> (
    tempfile::TempDir,
    LeaseMigrationAuthority,
    SigningKey,
    SigningKey,
    [SigningKey; 3],
) {
    let directory = tempdir().expect("temporary directory");
    let source = key(61);
    let destination = key(62);
    let witnesses = [key(71), key(72), key(73)];
    let mut authority = LeaseMigrationAuthority::new(
        directory.path().join("migration.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
    )
    .expect("authority");
    authority
        .register_owner("owner-a", &source.verifying_key())
        .expect("source owner");
    authority
        .register_owner("owner-b", &destination.verifying_key())
        .expect("destination owner");
    for (index, witness) in witnesses.iter().enumerate() {
        authority
            .register_witness(&format!("witness-{index}"), &witness.verifying_key())
            .expect("witness");
    }
    authority
        .initialize(
            LeaseRecord::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                "region-a",
                "owner-a",
                "process-a",
                7,
                200,
                3,
                &hash('a'),
            )
            .expect("initial lease"),
        )
        .expect("initialize");
    (directory, authority, source, destination, witnesses)
}

fn intent(
    authority: &LeaseMigrationAuthority,
    source: &SigningKey,
    nonce: &str,
    destination_region: &str,
) -> LeaseMigrationIntent {
    let lease = authority.current_lease().expect("lease");
    LeaseMigrationIntent::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        &lease.region_id,
        &lease.owner_id,
        &lease.process_instance,
        destination_region,
        "owner-b",
        "process-b",
        lease.ownership_epoch,
        &lease.record_hash,
        lease.generation,
        &lease.content_hash,
        nonce,
        lease.ownership_epoch + 1,
        150,
        source,
    )
    .expect("intent")
}

fn ack(
    intent: &LeaseMigrationIntent,
    witness_id: &str,
    witness: &SigningKey,
    observed_tick: u64,
) -> LeaseMigrationWitnessAck {
    LeaseMigrationWitnessAck::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        &intent.intent_hash,
        witness_id,
        1,
        observed_tick,
        20,
        witness,
    )
    .expect("ack")
}

#[test]
fn valid_handoff_has_a_single_active_destination_and_monotonic_epoch() {
    let (_directory, mut authority, source, destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-valid", "region-b");
    authority.begin(migration.clone(), 10).expect("drain");
    authority
        .accept_witness_ack(ack(&migration, "witness-0", &witnesses[0], 11), 11)
        .expect("ack0");
    authority
        .accept_witness_ack(ack(&migration, "witness-1", &witnesses[1], 11), 11)
        .expect("ack1");
    authority.prepare(12).expect("prepare");
    let release = un1c0::lease_migration::LeaseMigrationRelease::sign(
        &migration,
        "owner-a",
        "process-a",
        7,
        &migration.current_record_hash,
        13,
        &source,
    )
    .expect("release");
    authority
        .release_source(release.clone(), 13)
        .expect("release");
    let destination_lease = LeaseRecord::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "region-b",
        "owner-b",
        "process-b",
        8,
        240,
        3,
        &migration.content_hash,
    )
    .expect("destination lease");
    let activation = LeaseMigrationActivation::sign(
        &migration,
        &release,
        "region-b",
        "owner-b",
        "process-b",
        8,
        240,
        3,
        &migration.content_hash,
        &destination_lease.record_hash,
        14,
        &destination,
    )
    .expect("activation");
    let lease = authority
        .activate_destination(activation.clone(), 14)
        .expect("activate");
    assert_eq!(lease.region_id, "region-b");
    assert_eq!(lease.ownership_epoch, 8);
    assert_eq!(authority.state(), LeaseMigrationState::Activated);
    assert_eq!(
        authority
            .activate_destination(activation, 14)
            .expect("idempotent activation"),
        lease
    );
}

#[test]
fn source_drain_precedes_prepare_and_activation_requires_release() {
    let (_directory, mut authority, source, destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-order", "region-b");
    let release = un1c0::lease_migration::LeaseMigrationRelease::sign(
        &migration,
        "owner-a",
        "process-a",
        7,
        &migration.current_record_hash,
        13,
        &source,
    )
    .expect("release");
    let activation = LeaseMigrationActivation::sign(
        &migration,
        &release,
        "region-b",
        "owner-b",
        "process-b",
        8,
        240,
        3,
        &migration.content_hash,
        &hash('b'),
        14,
        &destination,
    )
    .expect("activation evidence");
    assert_eq!(
        authority.activate_destination(activation, 14),
        Err(LeaseMigrationError::InvalidState(
            "migration is not started".into()
        ))
    );
    assert_eq!(
        authority.prepare(12),
        Err(LeaseMigrationError::InvalidState(
            "migration is not started".into()
        ))
    );
    authority.begin(migration.clone(), 10).expect("drain");
    authority
        .accept_witness_ack(ack(&migration, "witness-0", &witnesses[0], 11), 11)
        .expect("ack0");
    assert_eq!(
        authority.prepare(12),
        Err(LeaseMigrationError::QuorumUnavailable)
    );
}

#[test]
fn forged_misbound_and_stale_evidence_fails_before_state_change() {
    let (_directory, mut authority, source, _destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-forged", "region-b");
    let mut forged = migration.clone();
    forged.destination_region = "region-c".into();
    assert!(authority.begin(forged, 10).is_err());
    assert_eq!(authority.state(), LeaseMigrationState::Stable);
    authority.begin(migration.clone(), 10).expect("drain");
    let stale = ack(&migration, "witness-0", &witnesses[0], 11);
    assert_eq!(
        authority.accept_witness_ack(stale, 200),
        Err(LeaseMigrationError::StaleEvidence)
    );
    assert_eq!(authority.state(), LeaseMigrationState::Draining);
}

#[test]
fn duplicate_witness_is_idempotent_but_changed_vote_conflicts() {
    let (_directory, mut authority, source, _destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-vote", "region-b");
    authority.begin(migration.clone(), 10).expect("drain");
    let first = ack(&migration, "witness-0", &witnesses[0], 11);
    authority
        .accept_witness_ack(first.clone(), 11)
        .expect("first ack");
    authority
        .accept_witness_ack(first, 11)
        .expect("duplicate ack");
    let changed = ack(&migration, "witness-0", &witnesses[0], 12);
    assert_eq!(
        authority.accept_witness_ack(changed, 12),
        Err(LeaseMigrationError::Conflict(
            "witness changed its vote for this migration".into()
        ))
    );
    assert_eq!(authority.metrics().witness_count, 1);
}

#[test]
fn competing_migration_digest_and_nonce_replay_are_rejected() {
    let (_directory, mut authority, source, _destination, _witnesses) = setup();
    let first = intent(&authority, &source, "nonce-race", "region-b");
    let second = intent(&authority, &source, "nonce-race-2", "region-c");
    authority.begin(first.clone(), 10).expect("first migration");
    assert_eq!(
        authority.begin(second, 10),
        Err(LeaseMigrationError::Conflict(
            "another migration is already in progress".into()
        ))
    );
    assert_eq!(
        authority.begin(first, 10).expect("exact replay"),
        LeaseMigrationState::Draining
    );
}

#[test]
fn snapshot_recovery_is_hash_bound_and_removes_stale_stage() {
    let (directory, mut authority, source, _destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-recovery", "region-b");
    authority.begin(migration.clone(), 10).expect("drain");
    authority
        .accept_witness_ack(ack(&migration, "witness-0", &witnesses[0], 11), 11)
        .expect("ack");
    authority.persist().expect("persist");
    std::fs::write(directory.path().join("migration.staging"), b"stale").expect("stage");
    let mut restarted = LeaseMigrationAuthority::new(
        directory.path().join("migration.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
    )
    .expect("restart");
    restarted
        .register_owner("owner-a", &source.verifying_key())
        .expect("source key");
    restarted
        .register_witness("witness-0", &witnesses[0].verifying_key())
        .expect("witness key");
    restarted.restore_persisted().expect("restore");
    assert!(!directory.path().join("migration.staging").exists());
    assert_eq!(restarted.state(), LeaseMigrationState::Draining);
    let mut tampered = restarted.snapshot().expect("snapshot");
    tampered.last_activation_epoch += 1;
    assert!(restarted.restore(tampered).is_err());
}

#[test]
fn strict_epoch_fencing_rejects_non_monotonic_activation() {
    let (_directory, mut authority, source, destination, witnesses) = setup();
    let migration = intent(&authority, &source, "nonce-epoch", "region-b");
    authority.begin(migration.clone(), 10).expect("drain");
    authority
        .accept_witness_ack(ack(&migration, "witness-0", &witnesses[0], 11), 11)
        .expect("ack0");
    authority
        .accept_witness_ack(ack(&migration, "witness-1", &witnesses[1], 11), 11)
        .expect("ack1");
    authority.prepare(12).expect("prepare");
    let release = un1c0::lease_migration::LeaseMigrationRelease::sign(
        &migration,
        "owner-a",
        "process-a",
        7,
        &migration.current_record_hash,
        13,
        &source,
    )
    .expect("release");
    authority
        .release_source(release.clone(), 13)
        .expect("release");
    assert!(LeaseMigrationActivation::sign(
        &migration,
        &release,
        "region-b",
        "owner-b",
        "process-b",
        7,
        240,
        3,
        &migration.content_hash,
        &hash('b'),
        14,
        &destination,
    )
    .is_err());
}
