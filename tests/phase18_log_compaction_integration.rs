use std::collections::BTreeSet;

use un1c0::{
    ConfigurationBoundSnapshot, ConsensusError, ConsensusNode, ConsensusRole, LogCompactionConfig,
    ReplicationCatchUpAction, StateCommand,
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

fn replicate_all(leader: &mut ConsensusNode, follower: &mut ConsensusNode) {
    let request = leader.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(request).unwrap();
    leader.acknowledge_append(response).unwrap();
}

fn append_set(node: &mut ConsensusNode, key: &str, value: &str) {
    node.propose(StateCommand::Set {
        key: key.into(),
        value: value.into(),
    })
    .unwrap();
}

#[test]
fn compaction_discards_only_applied_prefix_and_preserves_retained_suffix() {
    let (mut leader, mut follower) = elected_leader();
    for index in 0..4 {
        append_set(&mut leader, &format!("key-{index}"), "value");
    }
    replicate_all(&mut leader, &mut follower);
    append_set(&mut leader, "uncommitted", "tail");
    assert_eq!(leader.commit_index(), 4);
    assert_eq!(leader.log_len(), 5);

    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    let snapshot = leader.compact_committed_log(4).unwrap();
    assert_eq!(snapshot.last_included_index, 4);
    assert_eq!(snapshot.last_included_term, 1);
    assert_eq!(leader.compacted_log_frontier(), (4, 1));
    assert_eq!(leader.retained_log_len(), 1);
    assert_eq!(leader.log_len(), 5);
    assert_eq!(leader.state_value("key-3"), Some("value"));
}

#[test]
fn follower_behind_compacted_prefix_receives_configuration_bound_snapshot() {
    let (mut leader, mut follower) = elected_leader();
    for index in 0..4 {
        append_set(&mut leader, &format!("key-{index}"), "value");
    }
    replicate_all(&mut leader, &mut follower);
    append_set(&mut leader, "uncommitted", "tail");
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    let snapshot = leader.compact_committed_log(4).unwrap();

    assert!(matches!(
        leader.replication_catch_up_for("node-c").unwrap(),
        ReplicationCatchUpAction::Snapshot(ref received) if received == &snapshot
    ));
    assert!(matches!(
        leader.append_entries_for("node-c"),
        Err(ConsensusError::SnapshotRequired(_))
    ));

    let mut late_follower = ConsensusNode::new("node-c", members(), 128).unwrap();
    late_follower
        .install_configuration_bound_snapshot(snapshot.clone())
        .unwrap();
    assert_eq!(late_follower.compacted_log_frontier(), (4, 1));
    assert_eq!(late_follower.state_value("key-3"), Some("value"));
}

#[test]
fn invalid_compaction_target_and_configuration_tampering_fail_without_mutation() {
    let (mut leader, mut follower) = elected_leader();
    for index in 0..4 {
        append_set(&mut leader, &format!("key-{index}"), "value");
    }
    replicate_all(&mut leader, &mut follower);
    append_set(&mut leader, "uncommitted", "tail");
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    assert!(matches!(
        leader.compact_committed_log(5),
        Err(ConsensusError::LogCompaction(_))
    ));
    assert_eq!(leader.compacted_log_frontier(), (0, 0));
    assert_eq!(leader.retained_log_len(), 5);

    let snapshot = leader.configuration_bound_snapshot().unwrap();
    let mut tampered = snapshot.clone();
    tampered.members.insert("forged-node".into());
    assert!(matches!(
        tampered.validate(),
        Err(ConsensusError::InvalidSnapshot(_))
    ));
}

#[test]
fn compaction_configuration_rejects_unsafe_bounds() {
    assert!(matches!(
        LogCompactionConfig::new(1, 0),
        Err(ConsensusError::LogCompaction(_))
    ));
    assert!(matches!(
        LogCompactionConfig::new(usize::MAX, 1),
        Err(ConsensusError::LogCompaction(_))
    ));
}

#[test]
fn configuration_bound_snapshot_requires_consistent_metadata() {
    let snapshot = ConfigurationBoundSnapshot {
        term: 1,
        last_included_index: 2,
        last_included_term: 1,
        commit_index: 2,
        last_applied: 2,
        state: Default::default(),
        state_hash: "0".repeat(64),
        configuration_phase: un1c0::ConfigurationPhase::Stable,
        members: members(),
        previous_members: None,
        configuration_hash: "0".repeat(64),
    };
    assert!(matches!(
        snapshot.validate(),
        Err(ConsensusError::InvalidSnapshot(_))
    ));
}
