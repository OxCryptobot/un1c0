use std::collections::BTreeSet;
use un1c0::{ConsensusNode, ConsensusRole, StateCommand};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

#[test]
fn public_consensus_api_replicates_quorum_committed_state() {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 32).unwrap();
    let mut follower = ConsensusNode::new("node-b", cluster, 32).unwrap();

    let vote_request = leader.start_election().unwrap();
    let vote_response = follower.handle_vote_request(vote_request).unwrap();
    assert!(vote_response.granted);
    assert!(leader.receive_vote_response(vote_response).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);

    leader
        .propose(StateCommand::Set {
            key: "agent/consensus".into(),
            value: "quorum-committed".into(),
        })
        .unwrap();
    assert_eq!(leader.commit_index(), 0);

    let append = leader.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(append).unwrap();
    assert!(response.success);
    assert!(leader.acknowledge_append(response).unwrap());
    assert_eq!(leader.commit_index(), 1);
    assert_eq!(leader.state_value("agent/consensus"), Some("quorum-committed"));

    let commit_notice = leader.append_entries_for("node-b").unwrap();
    follower.handle_append_entries(commit_notice).unwrap();
    assert_eq!(follower.commit_index(), 1);
    assert_eq!(follower.state_value("agent/consensus"), Some("quorum-committed"));
    assert_eq!(
        leader.snapshot().unwrap().state_hash,
        follower.snapshot().unwrap().state_hash
    );
}
