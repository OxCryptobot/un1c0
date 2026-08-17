use std::collections::BTreeSet;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::{
    AppendEntries, AuthenticatedConsensusEnvelope, AuditLog, AuditSignerStore,
    ConsensusError, ConsensusMessage, ConsensusNode, DurableFileAuditSink,
    DurableSnapshotStore, ReplicatedSnapshot, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn leader() -> ConsensusNode {
    ConsensusNode::new("node-a", members(), 32).unwrap()
}

#[test]
fn durable_snapshot_round_trip_install_and_stale_rejection() {
    let mut source = leader();
    let mut follower = ConsensusNode::new("node-b", members(), 32).unwrap();
    let election = source.start_election().unwrap();
    let vote = follower.handle_vote_request(election).unwrap();
    assert!(source.receive_vote_response(vote).unwrap());
    source
        .propose(StateCommand::Set {
            key: "state/replication".into(),
            value: "durable".into(),
        })
        .unwrap();
    let append = source.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(append).unwrap();
    assert!(source.acknowledge_append(response).unwrap());
    follower
        .handle_append_entries(source.append_entries_for("node-b").unwrap())
        .unwrap();
    let snapshot = source.snapshot().unwrap();
    let directory = tempdir().unwrap();
    let store = DurableSnapshotStore::new(directory.path().join("state.snapshot.json"));
    store.save(&snapshot).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded, snapshot);

    let mut restored = ConsensusNode::new("node-c", members(), 32).unwrap();
    restored.install_snapshot(loaded.clone()).unwrap();
    assert_eq!(restored.state_value("state/replication"), Some("durable"));
    assert_eq!(restored.snapshot().unwrap().state_hash, loaded.state_hash);
    assert!(matches!(
        restored.install_snapshot(ReplicatedSnapshot {
            term: loaded.term,
            commit_index: loaded.commit_index.saturating_sub(1),
            last_applied: loaded.last_applied.saturating_sub(1),
            state: loaded.state.clone(),
            state_hash: loaded.state_hash.clone(),
        }),
        Err(ConsensusError::InvalidSnapshot(_))
    ));
}

#[test]
fn authenticated_consensus_envelope_binds_sender_term_nonce_and_key() {
    let key = SigningKey::from_bytes(&[31u8; 32]);
    let message = ConsensusMessage::AppendEntries(AppendEntries {
        term: 7,
        leader_id: "node-a".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![],
        leader_commit: 0,
    });
    let mut envelope = AuthenticatedConsensusEnvelope::sign(
        "node-a",
        7,
        "nonce-7",
        message,
        &key,
    )
    .unwrap();
    envelope
        .verify("node-a", &key.verifying_key().to_bytes())
        .unwrap();
    envelope.nonce = "tampered".into();
    assert!(matches!(
        envelope.verify("node-a", &key.verifying_key().to_bytes()),
        Err(ConsensusError::Unauthenticated(_))
    ));
    assert!(matches!(
        AuthenticatedConsensusEnvelope::sign(
            "node-a",
            8,
            "nonce-8",
            ConsensusMessage::AppendEntries(AppendEntries {
                term: 7,
                leader_id: "node-a".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![],
                leader_commit: 0,
            }),
            &key,
        ),
        Err(ConsensusError::Unauthenticated(_))
    ));
}

#[test]
fn signer_rotation_revocation_and_external_sink_are_durable_and_idempotent() {
    let directory = tempdir().unwrap();
    let local_path = directory.path().join("audit.jsonl");
    let registry_path = directory.path().join("signers.json");
    let sink = Arc::new(DurableFileAuditSink::open(directory.path().join("external")).unwrap());
    let key_v1 = SigningKey::from_bytes(&[41u8; 32]);
    let key_v2 = SigningKey::from_bytes(&[42u8; 32]);
    let mut trusted = AuditSignerStore::default();
    trusted
        .trust_public_key("operator:v1", &key_v1.verifying_key().to_bytes())
        .unwrap();
    let audit = AuditLog::open_with_signer_and_sink(
        &local_path,
        "operator:v1",
        key_v1.clone(),
        trusted,
        Some(sink.clone()),
    )
    .unwrap();
    audit
        .append("security_check", "node-a", "mesh", "allow", &serde_json::json!({"n":1}))
        .unwrap();
    audit
        .rotate_signer("operator:v2", key_v2.clone(), &registry_path)
        .unwrap();
    audit
        .append("security_check", "node-a", "mesh", "allow", &serde_json::json!({"n":2}))
        .unwrap();
    let persisted = AuditSignerStore::load(&registry_path).unwrap();
    assert!(persisted.is_revoked("operator:v1"));
    assert!(!persisted.is_revoked("operator:v2"));
    assert_eq!(sink.verify_chain(&persisted).unwrap(), 2);
    assert_eq!(audit.flush_sink().unwrap(), 2);

    audit
        .revoke_signer("operator:v2", &registry_path)
        .unwrap();
    assert!(matches!(
        audit.append("security_check", "node-a", "mesh", "deny", &serde_json::json!({"n":3})),
        Err(un1c0::SecurityError::SignerRevoked(_))
    ));
    let revoked_registry = AuditSignerStore::load(&registry_path).unwrap();
    assert!(matches!(
        AuditLog::open_with_signer(&local_path, "operator:v2", key_v2, revoked_registry),
        Err(un1c0::SecurityError::SignerRevoked(_))
    ));
}
