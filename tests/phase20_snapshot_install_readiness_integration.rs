use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use un1c0::{
    ConfigurationBoundSnapshot, ConsensusError, ConsensusNode, ConsensusRole, LogCompactionConfig,
    ReplicationCatchUpAction, SnapshotInstallAck, SnapshotInstallReadiness, SnapshotTransferAction,
    StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn snapshot_hash(snapshot: &ConfigurationBoundSnapshot) -> String {
    let bytes = serde_json::to_vec(snapshot).unwrap();
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn elected_compacted_leader() -> (ConsensusNode, ConfigurationBoundSnapshot) {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut voter = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response = voter.handle_vote_request(request).unwrap();
    assert!(leader.receive_vote_response(response).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);
    for index in 0..4 {
        leader
            .propose(StateCommand::Set {
                key: format!("key-{index}"),
                value: "value".into(),
            })
            .unwrap();
    }
    let append = leader.append_entries_for("node-b").unwrap();
    let response = voter.handle_append_entries(append).unwrap();
    leader.acknowledge_append(response).unwrap();
    leader
        .propose(StateCommand::Set {
            key: "retained-tail".into(),
            value: "tail".into(),
        })
        .unwrap();
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    let snapshot = leader.compact_committed_log(4).unwrap();
    (leader, snapshot)
}

fn ack(
    transfer_id: &str,
    snapshot: &ConfigurationBoundSnapshot,
    readiness: SnapshotInstallReadiness,
    reason: Option<&str>,
    term: u64,
) -> SnapshotInstallAck {
    SnapshotInstallAck {
        transfer_id: transfer_id.into(),
        follower_id: "node-c".into(),
        term,
        last_included_index: snapshot.last_included_index,
        last_included_term: snapshot.last_included_term,
        snapshot_sha256: snapshot_hash(snapshot),
        configuration_hash: snapshot.configuration_hash.clone(),
        readiness,
        reason: reason.map(str::to_string),
    }
}

#[test]
fn one_snapshot_transfer_is_backpressured_until_installed() {
    let (mut leader, snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-c", 10).unwrap() {
        SnapshotTransferAction::Send {
            transfer_id,
            snapshot: sent,
        } => {
            assert_eq!(sent, snapshot);
            transfer_id
        }
        other => panic!("expected snapshot send, got {other:?}"),
    };
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-c", 10).unwrap(),
        SnapshotTransferAction::Backpressured {
            retry_at_tick: None
        }
    ));
    assert!(!leader
        .acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Validated,
                None,
                1
            ),
            11,
        )
        .unwrap());
    assert!(!leader
        .acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::DurablyStaged,
                None,
                1,
            ),
            12,
        )
        .unwrap());
    assert!(matches!(
        leader.replication_catch_up_for("node-c").unwrap(),
        ReplicationCatchUpAction::Snapshot(_)
    ));
    assert!(leader
        .acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Installed,
                None,
                1
            ),
            13,
        )
        .unwrap());
    let status = leader.snapshot_replication_status("node-c").unwrap();
    assert_eq!(status.readiness, SnapshotInstallReadiness::Installed);
    assert_eq!(status.last_installed_index, 4);
    assert!(status.active_transfer_id.is_none());
}

#[test]
fn rejected_snapshot_uses_exact_retry_boundary() {
    let (mut leader, snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-c", 0).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    assert!(!leader
        .acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Rejected,
                Some("disk full"),
                1,
            ),
            0,
        )
        .unwrap());
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-c", 24).unwrap(),
        SnapshotTransferAction::Backpressured {
            retry_at_tick: Some(25)
        }
    ));
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-c", 25).unwrap(),
        SnapshotTransferAction::Send { .. }
    ));
}

#[test]
fn snapshot_ack_binding_and_readiness_transitions_fail_closed() {
    let (mut leader, snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-c", 0).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    let mut tampered = ack(
        &transfer_id,
        &snapshot,
        SnapshotInstallReadiness::Validated,
        None,
        1,
    );
    tampered.configuration_hash = "0".repeat(64);
    assert!(matches!(
        leader.acknowledge_snapshot_transfer(tampered, 1),
        Err(ConsensusError::InvalidSnapshotAcknowledgement(_))
    ));
    assert!(matches!(
        leader.acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Installed,
                None,
                1
            ),
            2,
        ),
        Err(ConsensusError::InvalidSnapshotAcknowledgement(_))
    ));
    assert!(matches!(
        leader.acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Rejected,
                None,
                1
            ),
            3,
        ),
        Err(ConsensusError::InvalidSnapshotAcknowledgement(_))
    ));
}

#[test]
fn higher_term_or_clock_uncertainty_clears_snapshot_authority() {
    let (mut leader, snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-c", 10).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    assert!(leader
        .acknowledge_snapshot_transfer(
            ack(
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Validated,
                None,
                2
            ),
            11
        )
        .is_ok());
    assert_eq!(leader.role(), ConsensusRole::Follower);
    assert!(leader
        .snapshot_replication_status("node-c")
        .unwrap()
        .active_transfer_id
        .is_none());

    let (mut leader, _) = elected_compacted_leader();
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-c", 10),
        Ok(SnapshotTransferAction::Send { .. })
    ));
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-c", 9),
        Err(ConsensusError::ClockUntrusted)
    ));
}
