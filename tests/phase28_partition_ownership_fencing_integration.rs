use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedDeliveryAcknowledgement,
    AuthenticatedSocketTransport, ConsensusError, ConsensusMessage, DurableQueueOwnership,
    DurableSocketDeliveryAction, DurableSocketQueueState, DurableSocketQueueStore,
    QueueOwnershipFence, QueueOwnershipTransfer, ReplicatedDeliveryAcknowledgement,
    ReplicatedDeliveryAction, SocketDeliveryCrashPoint, SocketQuotaConfig, VoteRequest,
};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn keys() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "node-a".into(),
            signing_key(41).verifying_key().to_bytes().to_vec(),
        ),
        (
            "node-b".into(),
            signing_key(42).verifying_key().to_bytes().to_vec(),
        ),
    ])
}

fn transport(node_id: &str) -> AuthenticatedSocketTransport {
    AuthenticatedSocketTransport::new_with_epoch_and_quota(
        "cluster-alpha",
        node_id,
        keys(),
        8,
        1,
        1,
        SocketQuotaConfig::new(4096, 4096, 10, 3).unwrap(),
    )
    .unwrap()
}

fn envelope(nonce: &str) -> AuthenticatedConsensusEnvelope {
    AuthenticatedConsensusEnvelope::sign_for_cluster_epoch(
        "cluster-alpha",
        "node-a",
        1,
        1,
        nonce,
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &signing_key(41),
    )
    .unwrap()
}

fn store(directory: &tempfile::TempDir) -> DurableSocketQueueStore {
    DurableSocketQueueStore::new(directory.path().join("queue.json"))
}

fn connect_pair() -> (TcpListener, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    (listener, stream)
}

fn remote_ack(
    peer_id: &str,
    sequence: u64,
    frame_digest: &str,
    owner_term: u64,
    ownership_epoch: u64,
    tick: u64,
) -> AuthenticatedDeliveryAcknowledgement {
    let acknowledgement = ReplicatedDeliveryAcknowledgement::new(
        peer_id,
        sequence,
        frame_digest,
        "node-a",
        "node-b",
        owner_term,
        ownership_epoch,
        tick,
    )
    .unwrap();
    AuthenticatedDeliveryAcknowledgement::sign(
        "cluster-alpha",
        "node-b",
        1,
        acknowledgement,
        &signing_key(42),
    )
    .unwrap()
}

fn remote_fence(
    peer_id: &str,
    owner_id: &str,
    owner_term: u64,
    ownership_epoch: u64,
    observed_tick: u64,
    reachable_members: usize,
    required_members: usize,
    reason: &str,
    sender_id: &str,
    nonce: &str,
) -> AuthenticatedConsensusEnvelope {
    let fence = QueueOwnershipFence::new(
        peer_id,
        owner_id,
        owner_term,
        ownership_epoch,
        observed_tick,
        reachable_members,
        required_members,
        reason,
    )
    .unwrap();
    AuthenticatedConsensusEnvelope::sign_for_cluster_epoch(
        "cluster-alpha",
        sender_id,
        owner_term,
        1,
        nonce,
        ConsensusMessage::QueueOwnershipFence(fence),
        &signing_key(42),
    )
    .unwrap()
}

fn with_lease(
    transport: &AuthenticatedSocketTransport,
    lease_expiry_tick: u64,
) -> DurableSocketQueueState {
    let state = transport.durable_queue_state().unwrap();
    let mut ownership = state.ownership.clone();
    ownership.insert(
        "node-b".into(),
        DurableQueueOwnership::new("node-b", "node-a", 1, lease_expiry_tick, 1).unwrap(),
    );
    DurableSocketQueueState::new_with_replication_and_fences(
        &state.cluster_id,
        &state.node_id,
        state.replay_epoch,
        state.quota_config,
        state.peer_quotas,
        state.next_queue_sequences,
        state.queued_frames,
        ownership,
        state.replicated_acknowledgements,
        state.ownership_fences,
        state.ack_quorum_size,
    )
    .unwrap()
}

#[test]
fn quorum_loss_fences_delivery_before_socket_write() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner
        .enqueue_durable_frame_with_backpressure(&durable_store, "node-b", &envelope("fence-1"), 0)
        .unwrap();

    assert_eq!(
        owner
            .record_ownership_quorum_loss(
                &durable_store,
                "node-b",
                7,
                1,
                "partition lost acknowledgement quorum",
            )
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced {
            peer_id: "node-b".into(),
            owner_id: "node-a".into(),
            ownership_epoch: 1,
            retry_at_tick: 7,
        }
    );

    let (_listener, mut stream) = connect_pair();
    assert_eq!(
        owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                8,
                SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced {
            peer_id: "node-b".into(),
            owner_id: "node-a".into(),
            ownership_epoch: 1,
            retry_at_tick: 11,
        }
    );
    assert!(owner.durable_queue_frame("node-b").unwrap().is_some());
}

#[test]
fn lease_expiry_fences_delivery_without_mutating_queue() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner
        .enqueue_durable_frame_with_backpressure(&durable_store, "node-b", &envelope("expiry-1"), 0)
        .unwrap();
    let expired_state = with_lease(&owner, 5);
    owner.restore_durable_queue(expired_state).unwrap();

    let (_listener, mut stream) = connect_pair();
    assert_eq!(
        owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                5,
                SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced {
            peer_id: "node-b".into(),
            owner_id: "node-a".into(),
            ownership_epoch: 1,
            retry_at_tick: 8,
        }
    );
    assert!(owner.durable_queue_frame("node-b").unwrap().is_some());
}

#[test]
fn ownership_fence_survives_restart_and_blocks_acknowledgement() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner.set_local_delivery_signing_key(signing_key(41));
    owner
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("restart-fence"),
            0,
        )
        .unwrap();
    let frame = owner.durable_queue_frame("node-b").unwrap().unwrap();
    owner
        .record_ownership_quorum_loss(
            &durable_store,
            "node-b",
            9,
            1,
            "partition fence must survive restart",
        )
        .unwrap();

    let mut restarted = transport("node-a");
    restarted
        .restore_durable_queue_from_store(&durable_store)
        .unwrap();
    assert!(restarted.ownership_fence("node-b").unwrap().is_some());
    assert!(matches!(
        restarted.record_authenticated_delivery_ack(
            &durable_store,
            remote_ack("node-b", frame.sequence, &frame.frame_digest, 1, 1, 10),
        ),
        Err(ConsensusError::DurableQueueOwnership(_))
    ));
    assert!(restarted.durable_queue_frame("node-b").unwrap().is_some());
}

#[test]
fn authenticated_remote_fence_observation_is_idempotent_and_blocks_delivery() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("remote-fence"),
            0,
        )
        .unwrap();
    let observation = remote_fence(
        "node-b",
        "node-a",
        1,
        1,
        12,
        1,
        2,
        "remote observer reports quorum loss",
        "node-b",
        "remote-fence-1",
    );
    assert_eq!(
        owner
            .apply_authenticated_queue_ownership_fence(&durable_store, &observation)
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced {
            peer_id: "node-b".into(),
            owner_id: "node-a".into(),
            ownership_epoch: 1,
            retry_at_tick: 12,
        }
    );
    let first_hash = owner.durable_queue_state().unwrap().state_hash;
    assert_eq!(
        owner
            .apply_authenticated_queue_ownership_fence(&durable_store, &observation)
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced {
            peer_id: "node-b".into(),
            owner_id: "node-a".into(),
            ownership_epoch: 1,
            retry_at_tick: 12,
        }
    );
    assert_eq!(owner.durable_queue_state().unwrap().state_hash, first_hash);

    let (_listener, mut stream) = connect_pair();
    assert!(matches!(
        owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                13,
                SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::OwnershipFenced { .. }
    ));
    assert!(owner.durable_queue_frame("node-b").unwrap().is_some());
}

#[test]
fn tampered_or_misbinding_remote_fence_fails_without_mutation() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("remote-fence-reject"),
            0,
        )
        .unwrap();
    let before = owner.durable_queue_state().unwrap().state_hash;

    let mut tampered = remote_fence(
        "node-b",
        "node-a",
        1,
        1,
        12,
        1,
        2,
        "tampered signature",
        "node-b",
        "remote-fence-tampered",
    );
    tampered.signature[0] ^= 1;
    assert!(matches!(
        owner.apply_authenticated_queue_ownership_fence(&durable_store, &tampered),
        Err(ConsensusError::Unauthenticated(_))
    ));

    let misbound = remote_fence(
        "node-b",
        "node-b",
        1,
        1,
        12,
        1,
        2,
        "wrong owner binding",
        "node-b",
        "remote-fence-misbinding",
    );
    assert!(matches!(
        owner.apply_authenticated_queue_ownership_fence(&durable_store, &misbound),
        Err(ConsensusError::DurableQueueOwnership(_))
    ));
    assert_eq!(owner.durable_queue_state().unwrap().state_hash, before);
    assert!(owner.ownership_fence("node-b").unwrap().is_none());
}

#[test]
fn ownership_transfer_clears_fence_and_allows_new_owner_retry() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut source = transport("node-a");
    source.set_ack_quorum_size(2).unwrap();
    source
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("failover-fence"),
            0,
        )
        .unwrap();
    source
        .record_ownership_quorum_loss(&durable_store, "node-b", 7, 1, "old owner lost quorum")
        .unwrap();

    let mut new_owner = transport("node-b");
    new_owner
        .restore_replicated_queue_from_store(&durable_store, "node-a")
        .unwrap();
    let transfer = QueueOwnershipTransfer::new("node-b", "node-a", "node-b", 2, 100, 2).unwrap();
    let transfer_envelope = AuthenticatedConsensusEnvelope::sign_for_cluster_epoch(
        "cluster-alpha",
        "node-a",
        2,
        1,
        "ownership-transfer-fence-clear",
        ConsensusMessage::QueueOwnershipTransfer(transfer),
        &signing_key(41),
    )
    .unwrap();
    assert_eq!(
        new_owner
            .apply_authenticated_queue_ownership_transfer(&durable_store, &transfer_envelope, 8)
            .unwrap(),
        ReplicatedDeliveryAction::OwnershipTransferred {
            owner_id: "node-b".into(),
            ownership_epoch: 2,
        }
    );
    assert!(new_owner.ownership_fence("node-b").unwrap().is_none());

    new_owner.set_local_delivery_signing_key(signing_key(42));
    let (listener, mut stream) = connect_pair();
    assert_eq!(
        new_owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                9,
                SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::WaitingForQuorum {
            sequence: 1,
            acknowledgements: 1,
            required: 2,
        }
    );
    let _ = listener.accept().unwrap();
    assert!(new_owner.durable_queue_frame("node-b").unwrap().is_some());
}
