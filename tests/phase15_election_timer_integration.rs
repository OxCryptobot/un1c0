use std::collections::BTreeSet;

use un1c0::{
    ConsensusError, ConsensusNode, ConsensusRole, ElectionTimerAction, ElectionTimerConfig,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn elected_leader() -> ConsensusNode {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut follower = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response = follower.handle_vote_request(request).unwrap();
    assert!(response.granted);
    assert!(leader.receive_vote_response(response).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);
    leader
}

fn timer_config() -> ElectionTimerConfig {
    ElectionTimerConfig::new(10, 0, 3, 20).unwrap()
}

#[test]
fn follower_starts_a_bounded_election_only_after_deadline() {
    let mut node = ConsensusNode::new("node-a", members(), 128).unwrap();
    node.configure_election_timers(timer_config()).unwrap();
    assert!(matches!(node.tick(0).unwrap(), ElectionTimerAction::Idle));
    assert_eq!(node.election_deadline_tick(), Some(10));
    assert!(matches!(node.tick(9).unwrap(), ElectionTimerAction::Idle));
    let action = node.tick(10).unwrap();
    assert!(matches!(action, ElectionTimerAction::StartElection(_)));
    assert_eq!(node.role(), ConsensusRole::Candidate);
    assert_eq!(node.current_term(), 1);
    assert_eq!(node.election_deadline_tick(), Some(20));
}

#[test]
fn leader_emits_heartbeats_at_bounded_intervals() {
    let mut leader = elected_leader();
    leader.configure_election_timers(timer_config()).unwrap();
    let first = leader.tick(0).unwrap();
    match first {
        ElectionTimerAction::SendHeartbeats(plan) => {
            assert_eq!(plan.term, leader.current_term());
            assert_eq!(plan.leader_id, "node-a");
            assert_eq!(
                plan.peer_ids,
                ["node-b", "node-c"].into_iter().map(String::from).collect()
            );
        }
        other => panic!("expected initial heartbeat, got {other:?}"),
    }
    assert!(matches!(leader.tick(2).unwrap(), ElectionTimerAction::Idle));
    assert!(matches!(
        leader.tick(3).unwrap(),
        ElectionTimerAction::SendHeartbeats(_)
    ));
}

#[test]
fn peer_heartbeats_reset_deadlines_and_failure_detector_is_bounded() {
    let mut node = ConsensusNode::new("node-b", members(), 128).unwrap();
    node.configure_election_timers(timer_config()).unwrap();
    node.tick(0).unwrap();
    assert_eq!(node.election_deadline_tick(), Some(10));
    node.record_peer_heartbeat("node-a", 4).unwrap();
    assert_eq!(node.election_deadline_tick(), Some(14));
    assert!(!node.peer_is_suspect("node-a", 23).unwrap());
    assert!(node.peer_is_suspect("node-a", 24).unwrap());
    assert!(matches!(
        node.record_peer_heartbeat("node-b", 25),
        Err(ConsensusError::InvalidPeer(_))
    ));
    assert!(matches!(
        node.peer_is_suspect("unknown", 25),
        Err(ConsensusError::InvalidPeer(_))
    ));
}

#[test]
fn clock_uncertainty_blocks_timer_actions_until_explicit_reanchor() {
    let mut node = ConsensusNode::new("node-a", members(), 128).unwrap();
    node.configure_election_timers(timer_config()).unwrap();
    node.tick(10).unwrap();
    assert!(node.clock_is_trusted());
    assert!(matches!(node.tick(5), Err(ConsensusError::ClockUntrusted)));
    assert!(!node.clock_is_trusted());
    assert!(matches!(node.tick(6), Err(ConsensusError::ClockUntrusted)));
    node.reanchor_monotonic_clock(6).unwrap();
    assert!(node.clock_is_trusted());
    assert!(matches!(node.tick(6).unwrap(), ElectionTimerAction::Idle));
    assert!(matches!(
        node.reanchor_monotonic_clock(5),
        Err(ConsensusError::ClockUntrusted)
    ));
}

#[test]
fn timer_configuration_rejects_unsafe_intervals() {
    assert!(matches!(
        ElectionTimerConfig::new(0, 0, 1, 2),
        Err(ConsensusError::InvalidElectionTimer(_))
    ));
    assert!(matches!(
        ElectionTimerConfig::new(10, 0, 10, 20),
        Err(ConsensusError::InvalidElectionTimer(_))
    ));
    assert!(matches!(
        ElectionTimerConfig::new(10, 0, 3, 9),
        Err(ConsensusError::InvalidElectionTimer(_))
    ));
}
