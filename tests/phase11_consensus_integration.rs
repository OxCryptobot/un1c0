use std::collections::BTreeSet;

use un1c0::{
    ConfigurationPhase, ConsensusNode, StateCommand,
};

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

fn elected_leader() -> ConsensusNode {
    let members = set(&["node-a", "node-b", "node-c"]);
    let mut leader = ConsensusNode::new("node-a", members.clone(), 64).unwrap();
    let mut voter = ConsensusNode::new("node-b", members, 64).unwrap();
    let request = leader.start_election().unwrap();
    let response = voter.handle_vote_request(request).unwrap();
    assert!(leader.receive_vote_response(response).unwrap());
    leader
}

#[test]
fn joint_consensus_requires_double_majority_and_finalizes_new_membership() {
    let old_members = set(&["node-a", "node-b", "node-c"]);
    let new_members = set(&["node-a", "node-b", "node-c", "node-d"]);
    let mut leader = elected_leader();
    let mut follower_b = ConsensusNode::new("node-b", old_members.clone(), 64).unwrap();
    let mut follower_d = ConsensusNode::new("node-d", new_members.clone(), 64).unwrap();

    let joint = leader.begin_membership_change(new_members.clone()).unwrap();
    assert!(matches!(joint.command, StateCommand::ConfigurationJoint { .. }));
    assert_eq!(leader.configuration_phase(), ConfigurationPhase::Joint);
    assert!(matches!(
        leader.finalize_membership_change(),
        Err(un1c0::ConsensusError::InvalidMembershipChange(_))
    ));

    let joint_for_b = leader.append_entries_for("node-b").unwrap();
    let response_b = follower_b.handle_append_entries(joint_for_b).unwrap();
    assert!(!leader.acknowledge_append(response_b).unwrap());

    let joint_for_d = leader.append_entries_for("node-d").unwrap();
    let response_d = follower_d.handle_append_entries(joint_for_d).unwrap();
    assert!(leader.acknowledge_append(response_d).unwrap());
    assert_eq!(leader.commit_index(), joint.index);

    follower_b
        .handle_append_entries(leader.append_entries_for("node-b").unwrap())
        .unwrap();
    assert_eq!(follower_b.configuration_phase(), ConfigurationPhase::Joint);
    assert_eq!(follower_b.members(), new_members);
    assert_eq!(follower_b.previous_members(), Some(old_members.clone()));

    let final_entry = leader.finalize_membership_change().unwrap();
    assert!(matches!(
        final_entry.command,
        StateCommand::ConfigurationFinal { .. }
    ));
    let final_for_b = leader.append_entries_for("node-b").unwrap();
    let response_b = follower_b.handle_append_entries(final_for_b).unwrap();
    assert!(!leader.acknowledge_append(response_b).unwrap());
    let final_for_d = leader.append_entries_for("node-d").unwrap();
    let response_d = follower_d.handle_append_entries(final_for_d).unwrap();
    assert!(leader.acknowledge_append(response_d).unwrap());
    assert_eq!(leader.configuration_phase(), ConfigurationPhase::Stable);
    assert_eq!(leader.members(), new_members);

    follower_b
        .handle_append_entries(leader.append_entries_for("node-b").unwrap())
        .unwrap();
    follower_d
        .handle_append_entries(leader.append_entries_for("node-d").unwrap())
        .unwrap();
    assert_eq!(follower_b.configuration_phase(), ConfigurationPhase::Stable);
    assert_eq!(follower_d.configuration_phase(), ConfigurationPhase::Stable);
    assert_eq!(follower_b.members(), new_members);
    assert_eq!(follower_d.members(), new_members);

    let request = leader.start_election().unwrap();
    let vote_b = follower_b.handle_vote_request(request.clone()).unwrap();
    assert!(!leader.receive_vote_response(vote_b).unwrap());
    let vote_d = follower_d.handle_vote_request(request).unwrap();
    assert!(leader.receive_vote_response(vote_d).unwrap());
}

#[test]
fn membership_changes_are_bounded_and_single_flight() {
    let mut leader = elected_leader();
    assert!(matches!(
        leader.begin_membership_change(set(&["node-a", "node-b", "node-c"])),
        Err(un1c0::ConsensusError::InvalidMembershipChange(_))
    ));
    leader.begin_membership_change(set(&["node-a", "node-b", "node-c", "node-d"]))
        .unwrap();
    assert!(matches!(
        leader.begin_membership_change(set(&["node-a", "node-b", "node-c", "node-e"])),
        Err(un1c0::ConsensusError::MembershipChangeInProgress)
    ));
}
