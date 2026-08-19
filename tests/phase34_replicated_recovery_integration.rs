use ed25519_dalek::SigningKey;
use std::collections::{BTreeMap, BTreeSet};
use tempfile::tempdir;
use un1c0::{
    ChaosDelivery, ChaosFault, DisasterRecoveryConfig, DisasterRecoveryController,
    ExternalFenceAction, ExternalFenceState, FailoverAction, ObserverMembership,
    ObserverMembershipPhase, RegionFailureObservation, ReplicatedRecoveryAction,
    ReplicatedRecoveryAuthority, ReplicatedRecoveryChaosSimulator, ReplicatedRecoveryConfig,
    ReplicatedRecoveryError, ReplicatedRecoverySnapshotStore,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn key_map(entries: &[(&str, &SigningKey)]) -> BTreeMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(id, key)| ((*id).to_string(), key.verifying_key().to_bytes().to_vec()))
        .collect()
}

fn authority() -> (
    ReplicatedRecoveryAuthority,
    SigningKey,
    BTreeMap<String, SigningKey>,
) {
    let authority_key = key(10);
    let observer_keys: BTreeMap<String, SigningKey> = BTreeMap::from([
        ("region-a".into(), key(20)),
        ("region-b".into(), key(21)),
        ("region-c".into(), key(22)),
        ("region-d".into(), key(23)),
    ]);
    let config =
        ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128).unwrap();
    let recovery_config = DisasterRecoveryConfig::new("un1c0-cluster", 3, 100).unwrap();
    let mut controller =
        DisasterRecoveryController::new(recovery_config, "region-a", SNAPSHOT, 1, 1).unwrap();
    for region in ["region-a", "region-b", "region-c"] {
        controller.register_region(region, SNAPSHOT, true).unwrap();
    }
    for (observer_id, signing_key) in &observer_keys {
        if observer_id != "region-d" {
            controller
                .register_trusted_observer(observer_id, &signing_key.verifying_key())
                .unwrap();
        }
    }
    let initial_members = set(&["region-a", "region-b", "region-c"]);
    let initial_keys = key_map(&[
        ("region-a", &observer_keys["region-a"]),
        ("region-b", &observer_keys["region-b"]),
        ("region-c", &observer_keys["region-c"]),
    ]);
    let membership = ObserverMembership::stable(1, initial_members).unwrap();
    let authority = ReplicatedRecoveryAuthority::new(
        config,
        "authority-a",
        authority_key.clone(),
        membership,
        initial_keys,
        controller,
    )
    .unwrap();
    (authority, authority_key, observer_keys)
}

fn transition_to_stable_epoch_two(
    authority: &mut ReplicatedRecoveryAuthority,
    observer_keys: &BTreeMap<String, SigningKey>,
) {
    let new_members = set(&["region-b", "region-c", "region-d"]);
    let new_keys = key_map(&[
        ("region-b", &observer_keys["region-b"]),
        ("region-c", &observer_keys["region-c"]),
        ("region-d", &observer_keys["region-d"]),
    ]);
    let action = authority
        .begin_joint_membership(new_members, new_keys, 2)
        .unwrap();
    let index = match action {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        other => panic!("expected appended joint entry, got {other:?}"),
    };
    authority.acknowledge(index, "region-b").unwrap();
    authority.acknowledge(index, "region-c").unwrap();
    authority.commit_entry(index).unwrap();
    let final_action = authority.finalize_membership().unwrap();
    let final_index = match final_action {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        other => panic!("expected appended final entry, got {other:?}"),
    };
    authority.acknowledge(final_index, "region-b").unwrap();
    authority.acknowledge(final_index, "region-c").unwrap();
    authority.commit_entry(final_index).unwrap();
    assert_eq!(authority.membership().epoch, 2);
    assert_eq!(
        authority.membership().phase,
        ObserverMembershipPhase::Stable
    );
}

fn prepare_recovery(
    authority: &mut ReplicatedRecoveryAuthority,
    observer_keys: &BTreeMap<String, SigningKey>,
) -> un1c0::FailoverProposal {
    authority
        .controller_mut()
        .record_region_failure("region-a", 60, "phase34 recovery test")
        .unwrap();
    for observer_id in ["region-b", "region-c"] {
        let observation = RegionFailureObservation::sign_at_membership_epoch(
            "un1c0-cluster",
            2,
            "region-a",
            observer_id,
            1,
            1,
            60,
            SNAPSHOT,
            "active region unreachable",
            &observer_keys[observer_id],
        )
        .unwrap();
        authority
            .controller_mut()
            .ingest_failure_observation(observation)
            .unwrap();
    }
    match authority
        .prepare_recovery("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => proposal,
        other => panic!("expected promotion preparation, got {other:?}"),
    }
}

#[test]
fn joint_membership_requires_both_old_and_new_majorities() {
    let (mut authority, _, observer_keys) = authority();
    let new_members = set(&["region-b", "region-c", "region-d"]);
    let action = authority
        .begin_joint_membership(
            new_members,
            key_map(&[
                ("region-b", &observer_keys["region-b"]),
                ("region-c", &observer_keys["region-c"]),
                ("region-d", &observer_keys["region-d"]),
            ]),
            2,
        )
        .unwrap();
    let index = match action {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        other => panic!("expected append, got {other:?}"),
    };
    authority.acknowledge(index, "region-a").unwrap();
    authority.acknowledge(index, "region-b").unwrap();
    assert!(matches!(
        authority.commit_entry(index),
        Err(ReplicatedRecoveryError::QuorumUnavailable(_))
    ));
    assert_eq!(authority.commit_index(), 0);

    authority.acknowledge(index, "region-c").unwrap();
    authority.commit_entry(index).unwrap();
    assert_eq!(authority.membership().phase, ObserverMembershipPhase::Joint);
    assert_eq!(authority.membership().epoch, 2);
}

#[test]
fn final_membership_cannot_commit_before_joint_entry_and_requires_new_majority() {
    let (mut authority, _, observer_keys) = authority();
    let new_members = set(&["region-b", "region-c", "region-d"]);
    let joint = authority
        .begin_joint_membership(
            new_members,
            key_map(&[
                ("region-b", &observer_keys["region-b"]),
                ("region-c", &observer_keys["region-c"]),
                ("region-d", &observer_keys["region-d"]),
            ]),
            2,
        )
        .unwrap();
    let joint_index = match joint {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    assert!(matches!(
        authority.finalize_membership(),
        Err(ReplicatedRecoveryError::NoMembershipChange)
    ));
    authority.acknowledge(joint_index, "region-b").unwrap();
    authority.acknowledge(joint_index, "region-c").unwrap();
    authority.commit_entry(joint_index).unwrap();
    let final_action = authority.finalize_membership().unwrap();
    let final_index = match final_action {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    authority.acknowledge(final_index, "region-b").unwrap();
    assert!(matches!(
        authority.commit_entry(final_index),
        Err(ReplicatedRecoveryError::QuorumUnavailable(_))
    ));
    authority.acknowledge(final_index, "region-d").unwrap();
    authority.commit_entry(final_index).unwrap();
    assert_eq!(
        authority.membership().phase,
        ObserverMembershipPhase::Stable
    );
    assert_eq!(
        authority.membership().members,
        set(&["region-b", "region-c", "region-d"])
    );
}

#[test]
fn recovery_commit_issues_signed_external_fencing_token_and_rejects_stale_or_tampered_tokens() {
    let (mut authority, authority_key, observer_keys) = authority();
    transition_to_stable_epoch_two(&mut authority, &observer_keys);
    let proposal = prepare_recovery(&mut authority, &observer_keys);
    let appended = authority.append_recovery_commit(proposal).unwrap();
    let index = match appended {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    authority.acknowledge(index, "region-b").unwrap();
    authority.acknowledge(index, "region-c").unwrap();
    authority.commit_entry(index).unwrap();
    let token = authority.active_fencing_token().unwrap().clone();
    let mut external = ExternalFenceState::new("recovery-resource").unwrap();
    assert!(matches!(
        external.apply(
            token.clone(),
            &authority_key.verifying_key(),
            "un1c0-cluster"
        ),
        Ok(ExternalFenceAction::Activated(_))
    ));
    assert!(matches!(
        external.apply(
            token.clone(),
            &authority_key.verifying_key(),
            "un1c0-cluster"
        ),
        Ok(ExternalFenceAction::AlreadyActive(_))
    ));
    assert!(external
        .admit(&token, &authority_key.verifying_key(), "un1c0-cluster")
        .unwrap());

    let mut tampered = token;
    tampered.owner_region_id = "region-c".into();
    assert!(matches!(
        external.apply(tampered, &authority_key.verifying_key(), "un1c0-cluster"),
        Err(ReplicatedRecoveryError::FencingTokenRejected(_))
    ));
}

#[test]
fn replicated_authority_snapshot_preserves_joint_log_and_committed_fence() {
    let (mut authority, authority_key, observer_keys) = authority();
    transition_to_stable_epoch_two(&mut authority, &observer_keys);
    let proposal = prepare_recovery(&mut authority, &observer_keys);
    let appended = authority.append_recovery_commit(proposal).unwrap();
    let index = match appended {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    authority.acknowledge(index, "region-b").unwrap();
    authority.acknowledge(index, "region-c").unwrap();
    authority.commit_entry(index).unwrap();
    let before = authority.report();
    let directory = tempdir().unwrap();
    let store = ReplicatedRecoverySnapshotStore::new(directory.path().join("authority.json"));
    authority.save_snapshot(&store).unwrap();
    let restored =
        ReplicatedRecoveryAuthority::from_snapshot(store.load().unwrap(), authority_key).unwrap();
    assert_eq!(restored.report(), before);
    assert_eq!(restored.membership().epoch, 2);
    assert_eq!(restored.commit_index(), 3);
    assert!(restored.active_fencing_token().is_some());
}

#[test]
fn extended_dynamic_partition_chaos_preserves_joint_epoch_and_single_fence_authority() {
    let (authority, authority_key, observer_keys) = authority();
    let nodes = set(&["region-a", "region-b", "region-c", "region-d"]);
    let mut chaos = ReplicatedRecoveryChaosSimulator::new(authority, nodes).unwrap();
    let new_members = set(&["region-b", "region-c", "region-d"]);
    let joint = chaos
        .authority_mut()
        .begin_joint_membership(
            new_members,
            key_map(&[
                ("region-b", &observer_keys["region-b"]),
                ("region-c", &observer_keys["region-c"]),
                ("region-d", &observer_keys["region-d"]),
            ]),
            2,
        )
        .unwrap();
    let joint_index = match joint {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    chaos.partition("region-a", "region-d").unwrap();
    chaos
        .inject_fault("region-a", "region-b", ChaosFault::Duplicate)
        .unwrap();
    chaos
        .inject_fault("region-a", "region-c", ChaosFault::Delay { until_tick: 3 })
        .unwrap();
    assert_eq!(
        chaos
            .deliver_ack("region-a", "region-d", joint_index)
            .unwrap(),
        ChaosDelivery::Dropped
    );
    assert_eq!(
        chaos
            .deliver_ack("region-a", "region-b", joint_index)
            .unwrap(),
        ChaosDelivery::DuplicateDelivered
    );
    assert_eq!(
        chaos
            .deliver_ack("region-a", "region-c", joint_index)
            .unwrap(),
        ChaosDelivery::Delayed
    );
    chaos.advance_tick(3);
    chaos
        .deliver_ack("region-a", "region-c", joint_index)
        .unwrap();
    chaos.commit(joint_index).unwrap();

    chaos.heal("region-a", "region-d").unwrap();
    let final_action = chaos.authority_mut().finalize_membership().unwrap();
    let final_index = match final_action {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    chaos
        .deliver_ack("region-a", "region-b", final_index)
        .unwrap();
    chaos
        .deliver_ack("region-a", "region-d", final_index)
        .unwrap();
    chaos.commit(final_index).unwrap();
    chaos.reject_stale_epoch(1);
    chaos.reject_stale_fence();

    let proposal = prepare_recovery(chaos.authority_mut(), &observer_keys);
    let commit = chaos
        .authority_mut()
        .append_recovery_commit(proposal)
        .unwrap();
    let recovery_index = match commit {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    chaos
        .deliver_ack("region-a", "region-b", recovery_index)
        .unwrap();
    chaos
        .deliver_ack("region-a", "region-c", recovery_index)
        .unwrap();
    chaos.commit(recovery_index).unwrap();
    let token = chaos.authority().active_fencing_token().unwrap().clone();
    let mut external = ExternalFenceState::new("recovery-resource").unwrap();
    external
        .apply(token, &authority_key.verifying_key(), "un1c0-cluster")
        .unwrap();
    let report = chaos.report();
    assert_eq!(report.node_count, 4);
    assert_eq!(report.dynamic_partition_steps, 1);
    assert_eq!(report.membership_epochs_seen, vec![2]);
    assert_eq!(report.committed_entries, 3);
    assert_eq!(report.active_fence_epoch, 1);
    assert_eq!(report.active_owner_region_id.as_deref(), Some("region-b"));
    assert_eq!(report.stale_epoch_rejections, 1);
    assert_eq!(report.stale_fence_rejections, 1);
    assert!(report.safety_passed);
    assert!(!report.trace_digest.is_empty());
}

#[test]
fn authority_rejects_conflicting_membership_and_fencing_epoch_rollback() {
    let (mut authority, authority_key, observer_keys) = authority();
    transition_to_stable_epoch_two(&mut authority, &observer_keys);
    let proposal = prepare_recovery(&mut authority, &observer_keys);
    let commit = authority.append_recovery_commit(proposal).unwrap();
    let index = match commit {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => unreachable!(),
    };
    authority.acknowledge(index, "region-b").unwrap();
    authority.acknowledge(index, "region-c").unwrap();
    authority.commit_entry(index).unwrap();
    let token = authority.active_fencing_token().unwrap().clone();
    let mut external = ExternalFenceState::new("recovery-resource").unwrap();
    external
        .apply(
            token.clone(),
            &authority_key.verifying_key(),
            "un1c0-cluster",
        )
        .unwrap();
    let mut rollback = token.clone();
    rollback.fence_epoch = 0;
    assert!(matches!(
        external.apply(rollback, &authority_key.verifying_key(), "un1c0-cluster"),
        Err(ReplicatedRecoveryError::FencingTokenRejected(_))
    ));
}
