use std::collections::BTreeSet;

use un1c0::{
    ConsensusNode, ConsensusRole, ConsensusError, SnapshotAssembler, StateCommand,
    StateDelta, VoteResponse,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn leader() -> ConsensusNode {
    let mut node = ConsensusNode::new("node-a", members(), 128).unwrap();
    node.start_election().unwrap();
    assert!(node
        .receive_vote_response(VoteResponse {
            term: 1,
            voter_id: "node-b".into(),
            granted: true,
        })
        .unwrap());
    assert_eq!(node.role(), ConsensusRole::Leader);
    node
}

#[test]
fn snapshot_chunking_round_trips_out_of_order_and_rejects_corruption() {
    let mut source = leader();
    for index in 0..12 {
        let entry = source
            .propose(StateCommand::Set {
                key: format!("key-{index}"),
                value: format!("value-{index}"),
            })
            .unwrap();
        source
            .acknowledge_append(un1c0::AppendResponse {
                term: 1,
                follower_id: "node-b".into(),
                success: true,
                match_index: entry.index,
            })
            .unwrap();
        source
            .acknowledge_append(un1c0::AppendResponse {
                term: 1,
                follower_id: "node-c".into(),
                success: true,
                match_index: entry.index,
            })
            .unwrap();
    }
    let chunker = source.snapshot_chunker("transfer-13", 64).unwrap();
    assert!(chunker.chunks().len() > 1);

    let mut assembler = SnapshotAssembler::new(chunker.manifest().clone()).unwrap();
    for chunk in chunker.chunks().iter().rev() {
        assembler.accept(chunk.clone()).unwrap();
    }
    let snapshot = assembler.finish().unwrap();
    assert_eq!(snapshot.state.get("key-11").map(String::as_str), Some("value-11"));

    let mut corrupted = chunker.chunk(0).unwrap().clone();
    corrupted.bytes[0] ^= 0x80;
    let mut rejecting = SnapshotAssembler::new(chunker.manifest().clone()).unwrap();
    assert!(matches!(
        rejecting.accept(corrupted),
        Err(ConsensusError::InvalidSnapshotChunk(_))
    ));

    let mut incomplete = SnapshotAssembler::new(chunker.manifest().clone()).unwrap();
    incomplete.accept(chunker.chunk(0).unwrap().clone()).unwrap();
    assert!(matches!(
        incomplete.finish(),
        Err(ConsensusError::SnapshotTransferIncomplete)
    ));
}

#[test]
fn incremental_sync_applies_bounded_deltas_and_prepares_concurrent_catch_up() {
    let mut source = leader();
    source
        .propose(StateCommand::Set {
            key: "alpha".into(),
            value: "one".into(),
        })
        .unwrap();
    source
        .propose(StateCommand::Set {
            key: "beta".into(),
            value: "two".into(),
        })
        .unwrap();

    let delta_for_c = source.incremental_delta_for("node-c").unwrap().unwrap();
    assert_eq!(delta_for_c.base_index, 0);
    assert_eq!(delta_for_c.target_index, 2);
    assert_eq!(delta_for_c.leader_commit, 0);
    let mut follower = ConsensusNode::new("node-c", members(), 128).unwrap();
    follower.apply_incremental_delta(delta_for_c.clone()).unwrap();
    assert_eq!(follower.state_value("alpha"), None);

    let delta_for_b = source.incremental_delta_for("node-b").unwrap().unwrap();
    let plans = source
        .prepare_concurrent_catch_up(&["node-b".into(), "node-c".into()])
        .unwrap();
    assert!(plans.contains_key("node-b"));
    assert!(plans.contains_key("node-c"));
    assert_eq!(delta_for_b.target_index, 2);

    let mut committed_delta = delta_for_c;
    committed_delta.leader_commit = committed_delta.target_index;
    committed_delta.delta_hash = StateDelta::new(
        committed_delta.term,
        committed_delta.base_index,
        committed_delta.leader_commit,
        committed_delta.entries.clone(),
    )
    .unwrap()
    .delta_hash;
    follower.apply_incremental_delta(committed_delta).unwrap();
    assert_eq!(follower.state_value("alpha"), Some("one"));
    assert_eq!(follower.state_value("beta"), Some("two"));

    let mut forged = plans["node-b"].clone();
    forged.delta_hash.replace_range(..2, "ff");
    assert!(matches!(
        follower.apply_incremental_delta(forged),
        Err(ConsensusError::IncrementalSyncConflict(_))
    ));
}
