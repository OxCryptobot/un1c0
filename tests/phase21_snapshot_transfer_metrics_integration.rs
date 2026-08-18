use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use un1c0::{
    ConfigurationBoundSnapshot, ConsensusError, ConsensusNode, ConsensusRole, LogCompactionConfig,
    SnapshotBandwidthConfig, SnapshotInstallAck, SnapshotInstallReadiness, SnapshotTransferAction,
    SnapshotTransferProgressAction, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c", "node-d", "node-e"]
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
    let mut voter_b = ConsensusNode::new("node-b", cluster.clone(), 128).unwrap();
    let mut voter_c = ConsensusNode::new("node-c", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response_b = voter_b.handle_vote_request(request.clone()).unwrap();
    let response_c = voter_c.handle_vote_request(request).unwrap();
    assert!(!leader.receive_vote_response(response_b).unwrap());
    assert!(leader.receive_vote_response(response_c).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);
    for index in 0..4 {
        leader
            .propose(StateCommand::Set {
                key: format!("key-{index}"),
                value: format!("value-{index}-with-metrics"),
            })
            .unwrap();
    }
    let append_b = leader.append_entries_for("node-b").unwrap();
    let append_c = leader.append_entries_for("node-c").unwrap();
    leader
        .acknowledge_append(voter_b.handle_append_entries(append_b).unwrap())
        .unwrap();
    leader
        .acknowledge_append(voter_c.handle_append_entries(append_c).unwrap())
        .unwrap();
    leader
        .propose(StateCommand::Set {
            key: "retained-tail".into(),
            value: "tail-for-phase21".into(),
        })
        .unwrap();
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    let snapshot = leader.compact_committed_log(4).unwrap();
    (leader, snapshot)
}

fn ack(
    follower_id: &str,
    transfer_id: &str,
    snapshot: &ConfigurationBoundSnapshot,
    readiness: SnapshotInstallReadiness,
    reason: Option<&str>,
    term: u64,
) -> SnapshotInstallAck {
    SnapshotInstallAck {
        transfer_id: transfer_id.into(),
        follower_id: follower_id.into(),
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
fn bandwidth_backpressure_is_bounded_and_metrics_are_per_follower() {
    let (mut leader, snapshot) = elected_compacted_leader();
    leader
        .set_snapshot_bandwidth_config(SnapshotBandwidthConfig::new(100, 10).unwrap())
        .unwrap();
    let transfer_id = match leader.prepare_snapshot_transfer("node-d", 0).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap().len() as u64;
    assert!(snapshot_bytes > 100);
    assert!(matches!(
        leader.record_snapshot_transfer_progress("node-d", &transfer_id, 80, 0),
        Ok(SnapshotTransferProgressAction::Accepted {
            bytes_sent: 80,
            bytes_remaining,
        }) if bytes_remaining == snapshot_bytes - 80
    ));
    assert!(matches!(
        leader.record_snapshot_transfer_progress("node-d", &transfer_id, 30, 0),
        Ok(SnapshotTransferProgressAction::Backpressured {
            retry_at_tick: 10,
            available_bytes: 20,
        })
    ));
    assert!(matches!(
        leader.record_snapshot_transfer_progress("node-d", &transfer_id, 30, 10),
        Ok(SnapshotTransferProgressAction::Accepted {
            bytes_sent: 110,
            ..
        })
    ));
    let d_metrics = leader.snapshot_transfer_metrics("node-d").unwrap();
    assert_eq!(d_metrics.bytes_sent, 110);
    assert_eq!(d_metrics.bandwidth_window_bytes, 30);
    assert_eq!(d_metrics.bandwidth_window_start_tick, Some(10));
    assert_eq!(d_metrics.sent_transfers, 1);

    let other_transfer_id = match leader.prepare_snapshot_transfer("node-e", 10).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected independent snapshot send, got {other:?}"),
    };
    assert!(matches!(
        leader.record_snapshot_transfer_progress("node-e", &other_transfer_id, 40, 10),
        Ok(SnapshotTransferProgressAction::Accepted { bytes_sent: 40, .. })
    ));
    assert_eq!(
        leader
            .snapshot_transfer_metrics("node-e")
            .unwrap()
            .bytes_sent,
        40
    );
    assert_eq!(
        leader
            .snapshot_transfer_metrics("node-d")
            .unwrap()
            .bytes_sent,
        110
    );
}

#[test]
fn installed_ack_requires_complete_accounting_and_preserves_progress_boundary() {
    let (mut leader, snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-d", 0).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    assert!(
        leader
            .acknowledge_snapshot_transfer(
                ack(
                    "node-d",
                    &transfer_id,
                    &snapshot,
                    SnapshotInstallReadiness::Validated,
                    None,
                    1,
                ),
                1,
            )
            .unwrap()
            == false
    );
    assert!(matches!(
        leader.acknowledge_snapshot_transfer(
            ack(
                "node-d",
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Installed,
                None,
                1,
            ),
            2,
        ),
        Err(ConsensusError::InvalidSnapshotAcknowledgement(_))
    ));
    assert_eq!(
        leader
            .snapshot_replication_status("node-d")
            .unwrap()
            .last_installed_index,
        0
    );
    let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap().len() as u64;
    leader
        .record_snapshot_transfer_progress("node-d", &transfer_id, snapshot_bytes, 3)
        .unwrap();
    leader
        .acknowledge_snapshot_transfer(
            ack(
                "node-d",
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::DurablyStaged,
                None,
                1,
            ),
            4,
        )
        .unwrap();
    assert!(leader
        .acknowledge_snapshot_transfer(
            ack(
                "node-d",
                &transfer_id,
                &snapshot,
                SnapshotInstallReadiness::Installed,
                None,
                1,
            ),
            5,
        )
        .unwrap());
    let status = leader.snapshot_replication_status("node-d").unwrap();
    assert_eq!(status.last_installed_index, 4);
    assert_eq!(status.metrics.bytes_remaining, 0);
    assert_eq!(status.acknowledged_transfers, 1);
}

#[test]
fn cancellation_releases_active_transfer_but_obeys_exact_retry_boundary() {
    let (mut leader, _snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-d", 0).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    let cancellation = leader
        .cancel_snapshot_transfer("node-d", &transfer_id, 0, "follower disconnected")
        .unwrap();
    assert_eq!(cancellation.retry_at_tick, 25);
    let status = leader.snapshot_replication_status("node-d").unwrap();
    assert_eq!(status.readiness, SnapshotInstallReadiness::Cancelled);
    assert!(status.active_transfer_id.is_none());
    assert_eq!(status.cancelled_transfers, 1);
    assert_eq!(status.metrics.snapshot_bytes, 0);
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-d", 24).unwrap(),
        SnapshotTransferAction::Backpressured {
            retry_at_tick: Some(25)
        }
    ));
    assert!(matches!(
        leader.prepare_snapshot_transfer("node-d", 25).unwrap(),
        SnapshotTransferAction::Send { .. }
    ));
    assert!(matches!(
        leader.cancel_snapshot_transfer("node-d", "wrong-transfer", 25, "cleanup"),
        Err(ConsensusError::SnapshotCancellation(_))
    ));
    assert!(matches!(
        leader.cancel_snapshot_transfer("node-d", &transfer_id, 25, "bad\nreason"),
        Err(ConsensusError::SnapshotCancellation(_))
    ));
}

#[test]
fn cancellation_and_progress_fail_closed_for_clock_regression_and_unknown_followers() {
    let (mut leader, _snapshot) = elected_compacted_leader();
    let transfer_id = match leader.prepare_snapshot_transfer("node-d", 10).unwrap() {
        SnapshotTransferAction::Send { transfer_id, .. } => transfer_id,
        other => panic!("expected snapshot send, got {other:?}"),
    };
    assert!(matches!(
        leader.record_snapshot_transfer_progress("node-d", &transfer_id, 1, 9),
        Err(ConsensusError::ClockUntrusted)
    ));
    assert!(matches!(
        leader.cancel_snapshot_transfer("node-x", &transfer_id, 11, "unknown"),
        Err(ConsensusError::InvalidPeer(_))
    ));
    assert!(matches!(
        leader.snapshot_transfer_metrics("node-x"),
        Err(ConsensusError::InvalidPeer(_))
    ));
    assert_eq!(
        leader
            .snapshot_replication_status("node-d")
            .unwrap()
            .active_transfer_id,
        Some(transfer_id)
    );
}
