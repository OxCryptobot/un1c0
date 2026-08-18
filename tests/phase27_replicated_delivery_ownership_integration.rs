use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedDeliveryAcknowledgement,
    AuthenticatedSocketTransport, ConsensusError, ConsensusMessage, DurableSocketDeliveryAction,
    DurableSocketQueueStore, QueueOwnershipTransfer, ReplicatedDeliveryAcknowledgement,
    ReplicatedDeliveryAction, SocketQuotaConfig, SocketReceiveAction, VoteRequest,
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
) -> AuthenticatedDeliveryAcknowledgement {
    let acknowledgement = ReplicatedDeliveryAcknowledgement::new(
        peer_id,
        sequence,
        frame_digest,
        "node-a",
        "node-b",
        owner_term,
        ownership_epoch,
        5,
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

#[test]
fn authenticated_delivery_waits_for_quorum_and_commits_idempotently() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner.set_local_delivery_signing_key(signing_key(41));
    owner
        .enqueue_durable_frame_with_backpressure(&durable_store, "node-b", &envelope("quorum-1"), 0)
        .unwrap();
    let frame = owner.durable_queue_frame("node-b").unwrap().unwrap();
    let (listener, mut stream) = connect_pair();
    let action = owner
        .deliver_next_durable_frame(
            &durable_store,
            &mut stream,
            "node-b",
            1,
            un1c0::SocketDeliveryCrashPoint::None,
        )
        .unwrap();
    let (_incoming, _) = listener.accept().unwrap();
    assert_eq!(
        action,
        DurableSocketDeliveryAction::WaitingForQuorum {
            sequence: 1,
            acknowledgements: 1,
            required: 2,
        }
    );
    assert_eq!(
        owner.replicated_acknowledgement_count("node-b", 1).unwrap(),
        1
    );

    let duplicate = owner
        .record_authenticated_delivery_ack(
            &durable_store,
            AuthenticatedDeliveryAcknowledgement::sign(
                "cluster-alpha",
                "node-a",
                1,
                ReplicatedDeliveryAcknowledgement::new(
                    "node-b",
                    1,
                    &frame.frame_digest,
                    "node-a",
                    "node-a",
                    1,
                    1,
                    1,
                )
                .unwrap(),
                &signing_key(41),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        duplicate,
        ReplicatedDeliveryAction::WaitingForQuorum {
            sequence: 1,
            acknowledgements: 1,
            required: 2,
        }
    );

    let committed = owner
        .record_authenticated_delivery_ack(
            &durable_store,
            remote_ack("node-b", 1, &frame.frame_digest, 1, 1),
        )
        .unwrap();
    assert_eq!(
        committed,
        ReplicatedDeliveryAction::Committed { sequence: 1 }
    );
    assert_eq!(owner.durable_queue_frame("node-b").unwrap(), None);
}

#[test]
fn replicated_ack_state_survives_restart_before_remote_quorum_arrives() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut owner = transport("node-a");
    owner.set_ack_quorum_size(2).unwrap();
    owner.set_local_delivery_signing_key(signing_key(41));
    owner
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("restart-ack"),
            0,
        )
        .unwrap();
    let frame = owner.durable_queue_frame("node-b").unwrap().unwrap();
    let (listener, mut stream) = connect_pair();
    assert!(matches!(
        owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                1,
                un1c0::SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::WaitingForQuorum { .. }
    ));
    let (_incoming, _) = listener.accept().unwrap();

    let mut restarted = transport("node-a");
    restarted
        .restore_durable_queue_from_store(&durable_store)
        .unwrap();
    restarted.set_ack_quorum_size(2).unwrap();
    assert_eq!(
        restarted
            .replicated_acknowledgement_count("node-b", frame.sequence)
            .unwrap(),
        1
    );
    assert_eq!(
        restarted
            .record_authenticated_delivery_ack(
                &durable_store,
                remote_ack("node-b", frame.sequence, &frame.frame_digest, 1, 1),
            )
            .unwrap(),
        ReplicatedDeliveryAction::Committed { sequence: 1 }
    );
    assert_eq!(restarted.durable_queue_frame("node-b").unwrap(), None);
}

#[test]
fn cross_host_owner_transfer_imports_queue_and_new_owner_delivers() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut source = transport("node-a");
    source
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("failover-1"),
            0,
        )
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
        "ownership-transfer-1",
        ConsensusMessage::QueueOwnershipTransfer(transfer),
        &signing_key(41),
    )
    .unwrap();
    assert_eq!(
        new_owner
            .apply_authenticated_queue_ownership_transfer(&durable_store, &transfer_envelope, 5)
            .unwrap(),
        ReplicatedDeliveryAction::OwnershipTransferred {
            owner_id: "node-b".into(),
            ownership_epoch: 2,
        }
    );
    new_owner.set_local_delivery_signing_key(signing_key(42));
    let (listener, mut stream) = connect_pair();
    assert!(matches!(
        new_owner
            .deliver_next_durable_frame(
                &durable_store,
                &mut stream,
                "node-b",
                6,
                un1c0::SocketDeliveryCrashPoint::None,
            )
            .unwrap(),
        DurableSocketDeliveryAction::Delivered { sequence: 1, .. }
    ));
    let (mut incoming, _) = listener.accept().unwrap();
    let mut receiver = transport("node-b");
    assert!(matches!(
        receiver.receive_with_backpressure(&mut incoming, 6),
        Ok(SocketReceiveAction::Received { .. })
    ));
    assert_eq!(new_owner.durable_queue_frame("node-b").unwrap(), None);
    assert_eq!(
        new_owner.queue_ownership("node-b").unwrap().owner_id,
        "node-b"
    );
}

#[test]
fn stale_or_misbinding_transfer_fails_without_mutating_owner() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let source = transport("node-a");
    source.persist_durable_queue(&durable_store).unwrap();
    let mut new_owner = transport("node-b");
    new_owner
        .restore_replicated_queue_from_store(&durable_store, "node-a")
        .unwrap();
    let before = new_owner.queue_ownership("node-b").unwrap();
    let stale = QueueOwnershipTransfer::new("node-b", "node-a", "node-b", 1, 100, 2).unwrap();
    let envelope = AuthenticatedConsensusEnvelope::sign_for_cluster_epoch(
        "cluster-alpha",
        "node-a",
        1,
        1,
        "ownership-transfer-stale",
        ConsensusMessage::QueueOwnershipTransfer(stale),
        &signing_key(41),
    )
    .unwrap();
    assert!(matches!(
        new_owner.apply_authenticated_queue_ownership_transfer(&durable_store, &envelope, 5),
        Err(ConsensusError::DurableQueueOwnership(_))
    ));
    assert_eq!(new_owner.queue_ownership("node-b").unwrap(), before);
}
