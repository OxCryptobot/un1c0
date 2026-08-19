use ed25519_dalek::SigningKey;
use un1c0::{
    DisasterRecoveryConfig, DisasterRecoveryController, DisasterRecoveryError, FailoverAction,
    FailoverProposal, RecoveryPhase, RegionFailureObservation,
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

fn observation(observer_id: &str, signing_key: &SigningKey, tick: u64) -> RegionFailureObservation {
    RegionFailureObservation::sign(
        "un1c0-cluster",
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

#[test]
fn observer_quorum_commits_higher_term_failover_and_fences_old_region() {
    let observer_b = key(32);
    let observer_c = key(33);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 10, "partition detected")
        .unwrap();
    assert!(!controller
        .ingest_failure_observation(observation("region-b", &observer_b, 10))
        .unwrap());
    assert!(controller
        .ingest_failure_observation(observation("region-c", &observer_c, 10))
        .unwrap());

    let proposal = match controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion, got {other:?}"),
    };
    assert!(matches!(
        controller.commit_promotion(proposal).unwrap(),
        FailoverAction::Committed(_)
    ));
    let report = controller.report();
    assert_eq!(report.active_region_id, "region-b");
    assert_eq!(report.phase, RecoveryPhase::Committed);
    assert!(report.safety_passed);
    assert!(controller.region("region-a").unwrap().fenced);
    assert!(!controller.region("region-a").unwrap().active);
    assert!(controller.region("region-b").unwrap().active);
}

#[test]
fn failover_waits_for_distinct_observer_quorum_without_mutating_active_region() {
    let observer_b = key(34);
    let observer_c = key(35);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 11, "partition detected")
        .unwrap();
    assert!(!controller
        .ingest_failure_observation(observation("region-b", &observer_b, 11))
        .unwrap());
    let action = controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap();
    assert_eq!(
        action,
        FailoverAction::AwaitingQuorum {
            observed: 1,
            required: 2
        }
    );
    assert_eq!(controller.report().active_region_id, "region-a");
    assert_eq!(
        controller.report().phase,
        RecoveryPhase::AwaitingObserverQuorum
    );
}

#[test]
fn identical_failure_observation_is_idempotent() {
    let observer_b = key(36);
    let observer_c = key(37);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 12, "partition detected")
        .unwrap();
    let first = observation("region-b", &observer_b, 12);
    assert!(!controller
        .ingest_failure_observation(first.clone())
        .unwrap());
    assert!(!controller.ingest_failure_observation(first).unwrap());
    assert_eq!(controller.report().observer_count, 1);
}

#[test]
fn conflicting_observation_from_same_observer_fails_closed() {
    let observer_b = key(38);
    let observer_c = key(39);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 13, "partition detected")
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-b", &observer_b, 13))
        .unwrap();
    let conflicting = RegionFailureObservation::sign(
        "un1c0-cluster",
        "region-a",
        "region-b",
        1,
        1,
        14,
        SNAPSHOT,
        "different observation",
        &observer_b,
    )
    .unwrap();
    let error = controller
        .ingest_failure_observation(conflicting)
        .unwrap_err();
    assert!(matches!(
        error,
        DisasterRecoveryError::InvariantViolation(_)
    ));
}

#[test]
fn tampered_failure_observation_signature_is_rejected() {
    let observer_b = key(40);
    let observer_c = key(41);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 14, "partition detected")
        .unwrap();
    let mut tampered = observation("region-b", &observer_b, 14);
    tampered.signature[0] ^= 1;
    let error = controller.ingest_failure_observation(tampered).unwrap_err();
    assert!(matches!(error, DisasterRecoveryError::SignatureRejected(_)));
    assert_eq!(controller.report().observer_count, 0);
}

#[test]
fn snapshot_hash_mismatch_rejects_observation_and_promotion() {
    let observer_b = key(42);
    let observer_c = key(43);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 15, "partition detected")
        .unwrap();
    let mismatched = RegionFailureObservation::sign(
        "un1c0-cluster",
        "region-a",
        "region-b",
        1,
        1,
        15,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "partition detected",
        &observer_b,
    )
    .unwrap();
    let error = controller
        .ingest_failure_observation(mismatched)
        .unwrap_err();
    assert!(matches!(error, DisasterRecoveryError::BindingRejected(_)));
    controller
        .ingest_failure_observation(observation("region-b", &observer_b, 15))
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-c", &observer_c, 15))
        .unwrap();
    let error = controller
        .prepare_promotion(
            "region-b",
            2,
            2,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap_err();
    assert_eq!(error, DisasterRecoveryError::SnapshotHashMismatch);
}

#[test]
fn stale_term_or_epoch_cannot_promote_candidate() {
    let observer_b = key(44);
    let observer_c = key(45);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 16, "partition detected")
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-b", &observer_b, 16))
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-c", &observer_c, 16))
        .unwrap();
    let error = controller
        .prepare_promotion("region-b", 1, 2, SNAPSHOT)
        .unwrap_err();
    assert!(matches!(error, DisasterRecoveryError::StaleProposal(_)));
}

#[test]
fn self_observation_cannot_supply_quorum() {
    let observer_b = key(46);
    let mut controller = controller();
    controller
        .register_trusted_observer("region-a", &observer_b.verifying_key())
        .unwrap();
    controller
        .record_region_failure("region-a", 17, "partition detected")
        .unwrap();
    let self_observation = observation("region-a", &observer_b, 17);
    let error = controller
        .ingest_failure_observation(self_observation)
        .unwrap_err();
    assert!(matches!(error, DisasterRecoveryError::BindingRejected(_)));
}

#[test]
fn committed_failover_is_idempotent_and_never_has_two_active_regions() {
    let observer_b = key(47);
    let observer_c = key(48);
    let mut controller = controller();
    register_observers(&mut controller, &observer_b, &observer_c);
    controller
        .record_region_failure("region-a", 18, "partition detected")
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-b", &observer_b, 18))
        .unwrap();
    controller
        .ingest_failure_observation(observation("region-c", &observer_c, 18))
        .unwrap();
    let proposal = match controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion, got {other:?}"),
    };
    controller.commit_promotion(proposal.clone()).unwrap();
    assert_eq!(
        controller.commit_promotion(proposal).unwrap(),
        FailoverAction::AlreadyCommitted(FailoverProposal {
            previous_region_id: "region-a".into(),
            candidate_region_id: "region-b".into(),
            owner_term: 2,
            ownership_epoch: 2,
            snapshot_hash: SNAPSHOT.into(),
        })
    );
    let active_count = ["region-a", "region-b", "region-c"]
        .iter()
        .filter(|region| controller.region(region).unwrap().active)
        .count();
    assert_eq!(active_count, 1);
}
