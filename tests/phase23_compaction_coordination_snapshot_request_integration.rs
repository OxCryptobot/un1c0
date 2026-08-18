use std::collections::BTreeSet;

use un1c0::{
    AppendEntries, CompactionCoordinationAction, CompactionCoordinationConfig, ConsensusError,
    ConsensusNode, ConsensusRole, LogCompactionConfig, SnapshotRequestAction,
    SnapshotRequestReason, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn elected_leader() -> (ConsensusNode, ConsensusNode) {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut voter = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response = voter.handle_vote_request(request).unwrap();
    assert!(leader.receive_vote_response(response).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);
    (leader, voter)
}

fn append_set(node: &mut ConsensusNode, key: &str, value: &str) {
    node.propose(StateCommand::Set {
        key: key.into(),
        value: value.into(),
    })
    .unwrap();
}

fn replicate_all(leader: &mut ConsensusNode, follower: &mut ConsensusNode) {
    let request = leader.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(request).unwrap();
    leader.acknowledge_append(response).unwrap();
}

fn build_compacted_leader() -> ConsensusNode {
    let (mut leader, mut follower) = elected_leader();
    for index in 0..4 {
        append_set(&mut leader, &format!("key-{index}"), "value");
    }
    replicate_all(&mut leader, &mut follower);
    append_set(&mut leader, "uncommitted-tail", "tail");
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    leader
}

#[test]
fn compaction_coordination_waits_without_mutating_when_safe_frontier_is_insufficient() {
    let mut leader = build_compacted_leader();
    leader
        .configure_compaction_coordination(CompactionCoordinationConfig::new(0, 2, false).unwrap())
        .unwrap();
    let before = (leader.compacted_log_frontier(), leader.retained_log_len());

    let action = leader.coordinate_compaction(4).unwrap();
    let CompactionCoordinationAction::Waiting { plan } = action else {
        panic!("expected compaction coordination to wait");
    };
    assert!(!plan.ready);
    assert_eq!(plan.required_safe_followers, 2);
    assert_eq!(plan.safe_followers.len(), 1);
    assert_eq!(plan.blocked_followers.len(), 1);
    assert!(plan.validate().is_ok());
    assert_eq!(
        before,
        (leader.compacted_log_frontier(), leader.retained_log_len())
    );
}

#[test]
fn compaction_coordination_admits_remote_quorum_and_returns_bound_snapshot() {
    let mut leader = build_compacted_leader();
    leader
        .configure_compaction_coordination(CompactionCoordinationConfig::new(0, 1, true).unwrap())
        .unwrap();

    let action = leader.coordinate_compaction(4).unwrap();
    let CompactionCoordinationAction::Compacted { plan, snapshot } = action else {
        panic!("expected quorum-safe compaction");
    };
    assert!(plan.ready);
    assert_eq!(plan.target_index, 4);
    assert_eq!(plan.safe_followers.len(), 1);
    assert_eq!(plan.blocked_followers.len(), 1);
    assert_eq!(snapshot.last_included_index, 4);
    assert_eq!(leader.compacted_log_frontier(), (4, 1));
    assert!(snapshot.validate().is_ok());
}

#[test]
fn follower_requests_snapshot_for_compacted_append_and_leader_starts_transfer() {
    let mut leader = build_compacted_leader();
    let mut follower = ConsensusNode::new("node-c", members(), 128).unwrap();
    let snapshot = leader.compact_committed_log(4).unwrap();
    follower
        .install_configuration_bound_snapshot(snapshot)
        .unwrap();

    let append = AppendEntries {
        term: leader.current_term(),
        leader_id: "node-a".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: Vec::new(),
        leader_commit: leader.commit_index(),
    };
    let action = follower
        .snapshot_request_for_append(&append, Some(19))
        .unwrap();
    let SnapshotRequestAction::Request(request) = action else {
        panic!("expected a follower snapshot request");
    };
    assert_eq!(
        request.reason,
        SnapshotRequestReason::AppendPredecessorCompacted
    );
    assert_eq!(request.follower_id, "node-c");
    assert_eq!(request.leader_id, "node-a");
    assert_eq!(request.retry_at_tick, Some(19));
    assert!(request.validate().is_ok());

    let transfer = leader.handle_snapshot_request(request, 19).unwrap();
    assert!(matches!(
        transfer,
        un1c0::SnapshotTransferAction::Send { .. }
    ));
    let status = leader.snapshot_replication_status("node-c").unwrap();
    assert!(status.active_transfer_id.is_some());
}

#[test]
fn follower_requests_snapshot_for_incremental_base_and_request_hash_is_retry_bound() {
    let mut leader = build_compacted_leader();
    let delta = leader
        .incremental_delta_for("node-c")
        .unwrap()
        .expect("leader must have an incremental delta before compaction");
    let snapshot = leader.compact_committed_log(4).unwrap();
    let mut follower = ConsensusNode::new("node-c", members(), 128).unwrap();
    follower
        .install_configuration_bound_snapshot(snapshot)
        .unwrap();

    let first = follower
        .snapshot_request_for_incremental_delta("node-a", &delta, Some(21))
        .unwrap();
    let second = follower
        .snapshot_request_for_incremental_delta("node-a", &delta, Some(22))
        .unwrap();
    let SnapshotRequestAction::Request(first) = first else {
        panic!("expected incremental snapshot request");
    };
    let SnapshotRequestAction::Request(second) = second else {
        panic!("expected incremental snapshot request");
    };
    assert_eq!(first.reason, SnapshotRequestReason::IncrementalBaseBehind);
    assert_ne!(first.request_hash, second.request_hash);
    assert_eq!(first.follower_id, "node-c");
    assert_eq!(first.leader_id, "node-a");
    assert!(first.validate().is_ok());
}

#[test]
fn stale_or_misbinding_snapshot_requests_fail_closed_without_transfer_state() {
    let mut leader = build_compacted_leader();
    let snapshot = leader.compact_committed_log(4).unwrap();
    let mut follower = ConsensusNode::new("node-c", members(), 128).unwrap();
    follower
        .install_configuration_bound_snapshot(snapshot)
        .unwrap();
    let append = AppendEntries {
        term: leader.current_term(),
        leader_id: "node-a".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: Vec::new(),
        leader_commit: leader.commit_index(),
    };
    let SnapshotRequestAction::Request(mut request) =
        follower.snapshot_request_for_append(&append, None).unwrap()
    else {
        panic!("expected request");
    };
    request.term = request.term.saturating_sub(1);
    assert!(matches!(
        leader.handle_snapshot_request(request, 30),
        Err(ConsensusError::SnapshotRequest(_))
    ));
    assert!(leader
        .snapshot_replication_status("node-c")
        .unwrap()
        .active_transfer_id
        .is_none());
}
