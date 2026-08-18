use std::collections::BTreeSet;

use un1c0::{
    ConsensusError, ConsensusNode, ConsensusRole, ReplicationBatchAck, ReplicationFlowAction,
    ReplicationFlowConfig, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn leader() -> ConsensusNode {
    let cluster = members();
    let mut node = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut follower = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = node.start_election().unwrap();
    let response = follower.handle_vote_request(request).unwrap();
    assert!(node.receive_vote_response(response).unwrap());
    assert_eq!(node.role(), ConsensusRole::Leader);
    node
}

fn configured_leader() -> ConsensusNode {
    let mut node = leader();
    node.configure_replication_flow(ReplicationFlowConfig::new(2, 64 * 1024, 10).unwrap())
        .unwrap();
    node
}

fn append_one(node: &mut ConsensusNode, key: &str, value: &str) {
    node.propose(StateCommand::Set {
        key: key.into(),
        value: value.into(),
    })
    .unwrap();
}

#[test]
fn flow_control_limits_each_peer_to_one_in_flight_batch() {
    let mut node = configured_leader();
    append_one(&mut node, "a", "1");
    append_one(&mut node, "b", "2");
    append_one(&mut node, "c", "3");

    let first_b = node
        .prepare_flow_controlled_replication("node-b", 0)
        .unwrap();
    assert!(matches!(first_b, ReplicationFlowAction::Send(_)));
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-b", 1)
            .unwrap(),
        ReplicationFlowAction::Backpressured {
            retry_at_tick: None
        }
    ));
    let first_c = node
        .prepare_flow_controlled_replication("node-c", 1)
        .unwrap();
    assert!(matches!(first_c, ReplicationFlowAction::Send(_)));
    assert_eq!(
        node.replication_window_status("node-b")
            .unwrap()
            .sent_batches,
        1
    );
    assert_eq!(
        node.replication_window_status("node-c")
            .unwrap()
            .sent_batches,
        1
    );
}

#[test]
fn successful_ack_releases_window_and_preserves_quorum_commit_rules() {
    let mut node = configured_leader();
    append_one(&mut node, "feature", "enabled");
    assert_eq!(node.commit_index(), 0);
    let batch = match node
        .prepare_flow_controlled_replication("node-b", 0)
        .unwrap()
    {
        ReplicationFlowAction::Send(batch) => batch,
        other => panic!("expected batch, got {other:?}"),
    };
    let mut follower = ConsensusNode::new("node-b", members(), 128).unwrap();
    let response = follower
        .handle_append_entries(batch.request.clone())
        .unwrap();
    assert!(node
        .acknowledge_flow_controlled_replication(
            ReplicationBatchAck {
                batch_id: batch.batch_id,
                follower_id: "node-b".into(),
                response,
            },
            1,
        )
        .unwrap());
    assert_eq!(node.commit_index(), 1);
    let status = node.replication_window_status("node-b").unwrap();
    assert_eq!(status.in_flight_batch_id, None);
    assert_eq!(status.last_completed_batch_id, Some(batch.batch_id));
    assert_eq!(status.acknowledged_batches, 1);
    assert_eq!(node.state_value("feature"), Some("enabled"));
}

#[test]
fn failed_ack_uses_backoff_and_is_sendable_at_exact_retry_boundary() {
    let mut node = configured_leader();
    append_one(&mut node, "retry", "yes");
    let batch = match node
        .prepare_flow_controlled_replication("node-b", 5)
        .unwrap()
    {
        ReplicationFlowAction::Send(batch) => batch,
        other => panic!("expected batch, got {other:?}"),
    };
    let response = un1c0::AppendResponse {
        term: node.current_term(),
        follower_id: "node-b".into(),
        success: false,
        match_index: 0,
    };
    assert!(!node
        .acknowledge_flow_controlled_replication(
            ReplicationBatchAck {
                batch_id: batch.batch_id,
                follower_id: "node-b".into(),
                response,
            },
            5,
        )
        .unwrap());
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-b", 14)
            .unwrap(),
        ReplicationFlowAction::Backpressured {
            retry_at_tick: Some(15)
        }
    ));
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-b", 15)
            .unwrap(),
        ReplicationFlowAction::Send(_)
    ));
}

#[test]
fn invalid_batch_size_does_not_mutate_window_state() {
    let mut node = leader();
    node.configure_replication_flow(ReplicationFlowConfig::new(1, 1, 10).unwrap())
        .unwrap();
    append_one(&mut node, "oversized", "value");
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-b", 0),
        Err(ConsensusError::ReplicationFlowControl(_))
    ));
    let status = node.replication_window_status("node-b").unwrap();
    assert_eq!(status.in_flight_batch_id, None);
    assert_eq!(status.sent_batches, 0);
    assert_eq!(status.last_completed_batch_id, None);
}

#[test]
fn stale_or_duplicate_acknowledgements_fail_closed_without_progress_mutation() {
    let mut node = configured_leader();
    append_one(&mut node, "guard", "value");
    let batch = match node
        .prepare_flow_controlled_replication("node-b", 0)
        .unwrap()
    {
        ReplicationFlowAction::Send(batch) => batch,
        other => panic!("expected batch, got {other:?}"),
    };
    let stale = ReplicationBatchAck {
        batch_id: batch.batch_id + 1,
        follower_id: "node-b".into(),
        response: un1c0::AppendResponse {
            term: node.current_term(),
            follower_id: "node-b".into(),
            success: true,
            match_index: 1,
        },
    };
    assert!(matches!(
        node.acknowledge_flow_controlled_replication(stale, 1),
        Err(ConsensusError::ReplicationFlowControl(_))
    ));
    assert_eq!(node.commit_index(), 0);
    let mut follower = ConsensusNode::new("node-b", members(), 128).unwrap();
    let response = follower.handle_append_entries(batch.request).unwrap();
    let ack = ReplicationBatchAck {
        batch_id: batch.batch_id,
        follower_id: "node-b".into(),
        response: response.clone(),
    };
    node.acknowledge_flow_controlled_replication(ack.clone(), 2)
        .unwrap();
    assert!(matches!(
        node.acknowledge_flow_controlled_replication(ack, 3),
        Err(ConsensusError::ReplicationFlowControl(_))
    ));
}

#[test]
fn higher_term_response_steps_down_and_clears_in_flight_windows() {
    let mut node = configured_leader();
    append_one(&mut node, "term", "change");
    let batch = match node
        .prepare_flow_controlled_replication("node-b", 0)
        .unwrap()
    {
        ReplicationFlowAction::Send(batch) => batch,
        other => panic!("expected batch, got {other:?}"),
    };
    let response = un1c0::AppendResponse {
        term: node.current_term() + 1,
        follower_id: "node-b".into(),
        success: false,
        match_index: 0,
    };
    assert!(!node
        .acknowledge_flow_controlled_replication(
            ReplicationBatchAck {
                batch_id: batch.batch_id,
                follower_id: "node-b".into(),
                response,
            },
            1,
        )
        .unwrap());
    assert_eq!(node.role(), ConsensusRole::Follower);
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-b", 2),
        Err(ConsensusError::NotLeader)
    ));
}

#[test]
fn clock_uncertainty_blocks_flow_controlled_sends_until_reanchored() {
    let mut node = configured_leader();
    append_one(&mut node, "clock", "guard");
    node.prepare_flow_controlled_replication("node-b", 10)
        .unwrap();
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-c", 5),
        Err(ConsensusError::ClockUntrusted)
    ));
    node.reanchor_monotonic_clock(5).unwrap();
    assert!(matches!(
        node.prepare_flow_controlled_replication("node-c", 5),
        Ok(ReplicationFlowAction::Send(_))
    ));
}
