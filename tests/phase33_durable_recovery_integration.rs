use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;
use un1c0::{
    DisasterRecoveryConfig, DisasterRecoveryController, DisasterRecoveryError, FailoverAction,
    LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationConfig, RegionFailureObservation,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn controller() -> DisasterRecoveryController {
    let config = DisasterRecoveryConfig::new("un1c0-cluster", 3, 100).unwrap();
    let mut controller =
        DisasterRecoveryController::new(config, "region-a", SNAPSHOT, 1, 1).unwrap();
    controller
        .register_region("region-a", SNAPSHOT, true)
        .unwrap();
    controller
        .register_region("region-b", SNAPSHOT, true)
        .unwrap();
    controller
        .register_region("region-c", SNAPSHOT, true)
        .unwrap();
    controller
}

fn register_observers(
    controller: &mut DisasterRecoveryController,
    observer_b: &SigningKey,
    observer_c: &SigningKey,
) {
    controller
        .register_trusted_observer("region-b", &observer_b.verifying_key())
        .unwrap();
    controller
        .register_trusted_observer("region-c", &observer_c.verifying_key())
        .unwrap();
}

fn observation(
    epoch: u64,
    observer_id: &str,
    signing_key: &SigningKey,
    tick: u64,
) -> RegionFailureObservation {
    RegionFailureObservation::sign_at_membership_epoch(
        "un1c0-cluster",
        epoch,
        "region-a",
        observer_id,
        1,
        1,
        tick,
        SNAPSHOT,
        "active region unreachable",
        signing_key,
    )
    .unwrap()
}

fn prepare_controller() -> DisasterRecoveryController {
    let observer_b = key(80);
    let observer_c = key(81);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 30, "partition race")
        .unwrap();
    controller
        .ingest_failure_observation(observation(1, "region-b", &observer_b, 30))
        .unwrap();
    controller
        .ingest_failure_observation(observation(1, "region-c", &observer_c, 30))
        .unwrap();
    controller
}

#[test]
fn durable_snapshot_round_trip_preserves_pending_recovery_and_resumes_commit() {
    let mut controller = prepare_controller();
    let store =
        un1c0::DisasterRecoverySnapshotStore::new(tempdir().unwrap().path().join("recovery.json"));
    let proposal = match controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion, got {other:?}"),
    };
    controller.save_snapshot(&store).unwrap();

    let mut restored = DisasterRecoveryController::load_snapshot(&store).unwrap();
    assert_eq!(restored.report(), controller.report());
    assert_eq!(restored.membership_epoch(), 1);
    assert!(matches!(
        restored.commit_promotion(proposal).unwrap(),
        FailoverAction::Committed(_)
    ));
    assert_eq!(restored.report().active_region_id, "region-b");
    assert!(restored.report().safety_passed);
}

#[test]
fn tampered_snapshot_is_rejected_without_mutating_existing_controller() {
    let mut controller = prepare_controller();
    let before = controller.report();
    let mut snapshot = controller.snapshot().unwrap();
    snapshot.state_hash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into();

    let error = controller.restore_snapshot(snapshot).unwrap_err();

    assert!(matches!(error, DisasterRecoveryError::DurableSnapshot(_)));
    assert_eq!(controller.report(), before);
}

#[test]
fn partial_recovery_snapshot_staging_is_removed_before_restart() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("recovery.json");
    let store = un1c0::DisasterRecoverySnapshotStore::new(&path);
    fs::write(path.with_extension("recovery.tmp"), b"partial").unwrap();

    assert!(store.recover_staging().unwrap());
    assert!(!store.recover_staging().unwrap());
    assert!(!path.with_extension("recovery.tmp").exists());
}

#[test]
fn observer_membership_epoch_rejects_old_quorum_evidence_and_accepts_current_epoch() {
    let old_b = key(82);
    let old_c = key(83);
    let new_b = key(84);
    let new_c = key(85);
    let mut controller = controller();
    register_observers(&mut controller, &old_b, &old_c);
    let mut rotated = BTreeMap::new();
    rotated.insert("region-b".into(), new_b.verifying_key().to_bytes().to_vec());
    rotated.insert("region-c".into(), new_c.verifying_key().to_bytes().to_vec());
    controller.rotate_observer_membership(2, rotated).unwrap();
    controller
        .record_region_failure("region-a", 31, "membership epoch race")
        .unwrap();

    let old_epoch = observation(1, "region-b", &new_b, 31);
    let error = controller
        .ingest_failure_observation(old_epoch)
        .unwrap_err();
    assert_eq!(
        error,
        DisasterRecoveryError::StaleMembershipEpoch {
            expected: 2,
            observed: 1
        }
    );
    assert_eq!(controller.report().observer_count, 0);
    assert!(
        controller
            .ingest_failure_observation(observation(2, "region-b", &new_b, 31))
            .unwrap()
            == false
    );
    assert_eq!(controller.report().membership_epoch, 2);
}

#[test]
fn membership_rotation_clears_pending_evidence_and_rejects_non_monotonic_epoch() {
    let observer_b = key(86);
    let observer_c = key(87);
    let replacement_b = key(88);
    let replacement_c = key(89);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 32, "membership transition")
        .unwrap();
    controller
        .ingest_failure_observation(observation(1, "region-b", &observer_b, 32))
        .unwrap();
    let mut rotated = BTreeMap::new();
    rotated.insert(
        "region-b".into(),
        replacement_b.verifying_key().to_bytes().to_vec(),
    );
    rotated.insert(
        "region-c".into(),
        replacement_c.verifying_key().to_bytes().to_vec(),
    );
    controller.rotate_observer_membership(2, rotated).unwrap();

    let error = controller
        .rotate_observer_membership(2, BTreeMap::new())
        .unwrap_err();

    assert!(matches!(
        error,
        DisasterRecoveryError::StaleMembershipEpoch { .. }
    ));
    assert_eq!(controller.report().observer_count, 0);
    assert_eq!(controller.report().membership_epoch, 2);
}

#[test]
fn concurrent_partition_race_is_replayable_and_controller_arbiter_allows_one_commit() {
    let config =
        MultiRegionSimulationConfig::three_region("phase33-concurrent-race", 3301).unwrap();
    let mut simulator = MultiRegionFailoverSimulator::new(config).unwrap();
    simulator.partition_regions("region-a", "region-b").unwrap();
    simulator.partition_regions("region-a", "region-c").unwrap();
    simulator
        .inject_link_fault("node-b1", "node-a1", LinkFault::Delay { ticks: 2 })
        .unwrap();
    simulator
        .inject_link_fault("node-c1", "node-a1", LinkFault::Duplicate)
        .unwrap();
    let base = simulator.snapshot().unwrap();

    let mut branch_b = MultiRegionFailoverSimulator::from_snapshot(base.clone()).unwrap();
    let mut branch_c = MultiRegionFailoverSimulator::from_snapshot(base.clone()).unwrap();
    branch_b
        .accept_transfer(un1c0::FailoverTransfer {
            previous_owner_id: un1c0::NodeId::new("node-a1").unwrap(),
            new_owner_id: un1c0::NodeId::new("node-b1").unwrap(),
            owner_term: 2,
            ownership_epoch: 2,
        })
        .unwrap();
    branch_c
        .accept_transfer(un1c0::FailoverTransfer {
            previous_owner_id: un1c0::NodeId::new("node-a1").unwrap(),
            new_owner_id: un1c0::NodeId::new("node-c1").unwrap(),
            owner_term: 2,
            ownership_epoch: 2,
        })
        .unwrap();
    assert!(branch_b.report().safety_passed);
    assert!(branch_c.report().safety_passed);
    assert_ne!(
        branch_b.report().active_owner_id,
        branch_c.report().active_owner_id
    );

    let observer_b = key(90);
    let observer_c = key(91);
    let mut arbiter = controller();
    register_observers(&mut arbiter, &observer_b, &observer_c);
    arbiter
        .record_region_failure("region-a", 33, "concurrent partition race")
        .unwrap();
    arbiter
        .ingest_failure_observation(observation(1, "region-b", &observer_b, 33))
        .unwrap();
    arbiter
        .ingest_failure_observation(observation(1, "region-c", &observer_c, 33))
        .unwrap();
    let proposal = match arbiter
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion, got {other:?}"),
    };
    let conflict = arbiter
        .prepare_promotion("region-c", 2, 2, SNAPSHOT)
        .unwrap_err();
    assert!(matches!(conflict, DisasterRecoveryError::StaleProposal(_)));
    arbiter.commit_promotion(proposal).unwrap();
    assert_eq!(arbiter.report().active_region_id, "region-b");
    assert!(arbiter.report().safety_passed);
}

#[test]
fn durable_snapshot_round_trip_preserves_committed_replay_identity() {
    let mut controller = prepare_controller();
    let directory = tempdir().unwrap();
    let store = un1c0::DisasterRecoverySnapshotStore::new(directory.path().join("committed.json"));
    let proposal = match controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion, got {other:?}"),
    };
    controller.commit_promotion(proposal.clone()).unwrap();
    controller.save_snapshot(&store).unwrap();

    let mut restored = DisasterRecoveryController::load_snapshot(&store).unwrap();

    assert_eq!(
        restored.commit_promotion(proposal.clone()).unwrap(),
        FailoverAction::AlreadyCommitted(proposal)
    );
    assert_eq!(restored.report().active_region_id, "region-b");
    assert_eq!(restored.report().observer_count, 2);
    assert!(restored.report().safety_passed);
}
