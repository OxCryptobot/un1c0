use std::collections::BTreeMap;
use std::fs;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedSocketTransport, ConsensusError, ConsensusMessage,
    ConsensusNode, DurableConsensusState, DurableConsensusStateStore, ReplayWindow, VoteRequest,
};

fn members() -> std::collections::BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn vote_envelope(
    cluster_id: &str,
    sender_id: &str,
    term: u64,
    replay_epoch: u64,
    nonce: &str,
    key: &SigningKey,
) -> AuthenticatedConsensusEnvelope {
    AuthenticatedConsensusEnvelope::sign_for_cluster_epoch(
        cluster_id,
        sender_id,
        term,
        replay_epoch,
        nonce,
        ConsensusMessage::VoteRequest(VoteRequest {
            term,
            candidate_id: sender_id.into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        key,
    )
    .unwrap()
}

#[test]
fn durable_term_and_vote_state_round_trips_and_recovers_staging() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("consensus.state");
    let store = DurableConsensusStateStore::new(&path);
    let state =
        DurableConsensusState::new("cluster-alpha", "node-a", 7, Some("node-b".into()), 3, 7)
            .unwrap();
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), state);

    let staging = path.with_extension("state.tmp");
    fs::write(&staging, b"partial").unwrap();
    assert!(store.recover_staging().unwrap());
    assert!(!staging.exists());
    assert!(!store.recover_staging().unwrap());
}

#[test]
fn durable_state_rejects_tampering_identity_and_oversized_payloads() {
    let mut state =
        DurableConsensusState::new("cluster-alpha", "node-a", 2, Some("node-b".into()), 2, 2)
            .unwrap();
    state.current_term = 3;
    assert!(matches!(
        state.validate(),
        Err(ConsensusError::DurableConsensusState(_))
    ));

    let bad_vote =
        DurableConsensusState::new("cluster-alpha", "node-a", 2, Some("node-b".into()), 2, 2)
            .unwrap();
    let directory = tempdir().unwrap();
    let store = DurableConsensusStateStore::new(directory.path().join("state.json"));
    store.save(&bad_vote).unwrap();
    let mut node = ConsensusNode::new("node-a", members(), 64).unwrap();
    let wrong_cluster = store.load().unwrap();
    assert!(matches!(
        node.restore_durable_consensus_state("cluster-beta", wrong_cluster),
        Err(ConsensusError::DurableConsensusState(_))
    ));

    let oversized = directory.path().join("oversized.json");
    fs::write(&oversized, vec![b'x'; 128 * 1024 + 1]).unwrap();
    assert!(matches!(
        DurableConsensusStateStore::new(oversized).load(),
        Err(ConsensusError::DurableConsensusState(_))
    ));
}

#[test]
fn consensus_node_restores_vote_exclusivity_and_rejects_term_rollback() {
    let cluster = members();
    let mut original = ConsensusNode::new("node-a", cluster.clone(), 64).unwrap();
    let request = original.start_election().unwrap();
    assert_eq!(request.term, 1);
    let durable = original.durable_consensus_state("cluster-alpha").unwrap();

    let mut restored = ConsensusNode::new("node-a", cluster, 64).unwrap();
    restored
        .restore_durable_consensus_state("cluster-alpha", durable.clone())
        .unwrap();
    assert_eq!(restored.current_term(), 1);
    assert_eq!(restored.replay_term_floor(), 1);
    let competing = VoteRequest {
        term: 1,
        candidate_id: "node-b".into(),
        last_log_index: 0,
        last_log_term: 0,
    };
    assert!(!restored.handle_vote_request(competing).unwrap().granted);

    let rollback = DurableConsensusState::new("cluster-alpha", "node-a", 0, None, 1, 1).unwrap();
    assert!(matches!(
        restored.restore_durable_consensus_state("cluster-alpha", rollback),
        Err(ConsensusError::DurableConsensusState(_))
    ));
    assert_eq!(restored.current_term(), 1);
}

#[test]
fn replay_window_binds_epoch_and_term_and_keeps_bounded_nonce_state() {
    let key = signing_key(7);
    let mut window = ReplayWindow::new_with_epoch("cluster-alpha", "node-a", 1, 7, 2).unwrap();
    let first = vote_envelope("cluster-alpha", "node-a", 2, 7, "nonce-1", &key);
    window
        .accept(&first, &key.verifying_key().to_bytes())
        .unwrap();
    assert_eq!(window.len(), 1);
    assert_eq!(window.replay_epoch(), 7);
    assert_eq!(window.min_term(), 2);
    assert!(matches!(
        window.accept(&first, &key.verifying_key().to_bytes()),
        Err(ConsensusError::ReplayDetected)
    ));

    let stale_epoch = vote_envelope("cluster-alpha", "node-a", 2, 6, "nonce-2", &key);
    assert!(matches!(
        window.accept(&stale_epoch, &key.verifying_key().to_bytes()),
        Err(ConsensusError::ReplayEpochMismatch {
            expected: 7,
            received: 6
        })
    ));
    let stale_term = vote_envelope("cluster-alpha", "node-a", 1, 7, "nonce-3", &key);
    assert!(matches!(
        window.accept(&stale_term, &key.verifying_key().to_bytes()),
        Err(ConsensusError::StaleReplayTerm)
    ));

    let second = vote_envelope("cluster-alpha", "node-a", 2, 7, "nonce-4", &key);
    window
        .accept(&second, &key.verifying_key().to_bytes())
        .unwrap();
    assert_eq!(window.len(), 1);
    window
        .accept(&first, &key.verifying_key().to_bytes())
        .unwrap();
    assert_eq!(window.len(), 1);
}

#[test]
fn transport_epoch_rotation_clears_windows_and_is_monotonic() {
    let key_a = signing_key(11);
    let key_b = signing_key(12);
    let mut keys = BTreeMap::new();
    keys.insert("node-a".into(), key_a.verifying_key().to_bytes().to_vec());
    keys.insert("node-b".into(), key_b.verifying_key().to_bytes().to_vec());
    let mut transport =
        AuthenticatedSocketTransport::new_with_epoch("cluster-alpha", "node-b", keys, 8, 4, 3)
            .unwrap();
    assert_eq!(transport.replay_epoch(), 4);
    assert_eq!(transport.replay_term_floor(), 3);
    assert_eq!(transport.replay_window_len("node-a").unwrap(), 0);
    assert!(matches!(
        transport.rotate_replay_epoch(4, 4),
        Err(ConsensusError::DurableConsensusState(_))
    ));
    transport.rotate_replay_epoch(5, 6).unwrap();
    assert_eq!(transport.replay_epoch(), 5);
    assert_eq!(transport.replay_term_floor(), 6);
    assert_eq!(transport.replay_window_len("node-a").unwrap(), 0);
}
