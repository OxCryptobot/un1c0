use std::collections::BTreeMap;
use std::fs;
use std::net::{TcpListener, TcpStream};

use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedSocketTransport, ConsensusError, ConsensusMessage,
    DurableSocketDeliveryAction, DurableSocketQueueState, DurableSocketQueueStore,
    SocketDeliveryCrashPoint, SocketQuotaConfig, SocketReceiveAction, VoteRequest,
};

fn keys() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "node-a".into(),
            SigningKey::from_bytes(&[41u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
        (
            "node-b".into(),
            SigningKey::from_bytes(&[42u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
    ])
}

fn envelope(nonce: &str) -> AuthenticatedConsensusEnvelope {
    AuthenticatedConsensusEnvelope::sign_for_cluster(
        "cluster-alpha",
        "node-a",
        1,
        nonce,
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &SigningKey::from_bytes(&[41u8; 32]),
    )
    .unwrap()
}

fn transport() -> AuthenticatedSocketTransport {
    AuthenticatedSocketTransport::new_with_epoch_and_quota(
        "cluster-alpha",
        "node-a",
        keys(),
        8,
        1,
        1,
        SocketQuotaConfig::new(4096, 4096, 10, 3).unwrap(),
    )
    .unwrap()
}

fn store(directory: &tempfile::TempDir) -> DurableSocketQueueStore {
    DurableSocketQueueStore::new(directory.path().join("socket-queue.json"))
}

#[test]
fn authenticated_delivery_flushes_before_fifo_ack_and_remote_receive_verifies() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let message = envelope("deliver-1");
    let mut sender = transport();
    sender
        .enqueue_durable_frame_with_backpressure(&durable_store, "node-b", &message, 0)
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let action = sender
        .deliver_next_durable_frame(
            &durable_store,
            &mut stream,
            "node-b",
            0,
            SocketDeliveryCrashPoint::None,
        )
        .unwrap();
    assert!(matches!(
        action,
        DurableSocketDeliveryAction::Delivered {
            sequence: 1,
            frame_bytes: bytes
        } if bytes > 0
    ));

    let (mut incoming, _) = listener.accept().unwrap();
    let mut receiver =
        AuthenticatedSocketTransport::new("cluster-alpha", "node-b", keys(), 8).unwrap();
    assert!(matches!(
        receiver.receive_with_backpressure(&mut incoming, 0),
        Ok(SocketReceiveAction::Received { .. })
    ));
    let metrics = sender.socket_peer_metrics("node-b").unwrap();
    assert_eq!(metrics.durable_queue_frames, 0);
    assert_eq!(metrics.in_flight_bytes, 0);
    assert_eq!(metrics.durable_delivery_attempts, 1);
    assert_eq!(metrics.durable_delivery_failures, 0);
}

#[test]
fn every_socket_crash_boundary_retains_queue_for_authenticated_retry() {
    let points = [
        SocketDeliveryCrashPoint::BeforeLengthPrefix,
        SocketDeliveryCrashPoint::AfterLengthPrefix,
        SocketDeliveryCrashPoint::AfterPayloadWrite,
        SocketDeliveryCrashPoint::AfterFlush,
    ];

    for (index, point) in points.into_iter().enumerate() {
        let directory = tempdir().unwrap();
        let durable_store = store(&directory);
        let message = envelope(&format!("crash-{index}"));
        let mut sender = transport();
        sender
            .enqueue_durable_frame_with_backpressure(&durable_store, "node-b", &message, 0)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        assert!(matches!(
            sender
                .deliver_next_durable_frame(
                    &durable_store,
                    &mut stream,
                    "node-b",
                    0,
                    point,
                )
                .unwrap(),
            DurableSocketDeliveryAction::CrashInjected { sequence: 1, point: actual }
                if actual == point
        ));
        drop(stream);
        assert_eq!(
            sender
                .socket_peer_metrics("node-b")
                .unwrap()
                .durable_queue_frames,
            1
        );
        assert_eq!(
            sender
                .socket_peer_metrics("node-b")
                .unwrap()
                .injected_delivery_crashes,
            1
        );

        let mut restarted = transport();
        restarted
            .restore_durable_queue_from_store(&durable_store)
            .unwrap();
        let retry_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut retry_stream = TcpStream::connect(retry_listener.local_addr().unwrap()).unwrap();
        assert!(matches!(
            restarted
                .deliver_next_durable_frame(
                    &durable_store,
                    &mut retry_stream,
                    "node-b",
                    3,
                    SocketDeliveryCrashPoint::None,
                )
                .unwrap(),
            DurableSocketDeliveryAction::Delivered { sequence: 1, .. }
        ));
        let (mut incoming, _) = retry_listener.accept().unwrap();
        let mut receiver =
            AuthenticatedSocketTransport::new("cluster-alpha", "node-b", keys(), 8).unwrap();
        assert!(matches!(
            receiver.receive_with_backpressure(&mut incoming, 3),
            Ok(SocketReceiveAction::Received { .. })
        ));
        assert_eq!(
            restarted
                .socket_peer_metrics("node-b")
                .unwrap()
                .durable_queue_frames,
            0
        );
    }
}

#[test]
fn tampered_authenticated_payload_fails_before_delivery_and_preserves_queue() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut original = transport();
    original
        .enqueue_durable_frame_with_backpressure(
            &durable_store,
            "node-b",
            &envelope("auth-original"),
            0,
        )
        .unwrap();
    let mut state = durable_store.load().unwrap();
    let forged = AuthenticatedConsensusEnvelope::sign_for_cluster(
        "cluster-alpha",
        "node-a",
        1,
        "auth-forged",
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &SigningKey::from_bytes(&[99u8; 32]),
    )
    .unwrap();
    let forged_bytes = serde_json::to_vec(&forged).unwrap();
    let forged_digest = Sha256::digest(&forged_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let frame = &mut state.queued_frames.get_mut("node-b").unwrap()[0];
    frame.frame_bytes = forged_bytes.clone();
    frame.frame_digest = forged_digest;
    state.peer_quotas.get_mut("node-b").unwrap().in_flight_bytes = forged_bytes.len() as u64;
    let rewritten = DurableSocketQueueState::new(
        &state.cluster_id,
        &state.node_id,
        state.replay_epoch,
        state.quota_config,
        state.peer_quotas,
        state.next_queue_sequences,
        state.queued_frames,
    )
    .unwrap();
    durable_store.save(&rewritten).unwrap();

    let mut restarted = transport();
    restarted
        .restore_durable_queue_from_store(&durable_store)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    assert!(matches!(
        restarted.deliver_next_durable_frame(
            &durable_store,
            &mut stream,
            "node-b",
            0,
            SocketDeliveryCrashPoint::None,
        ),
        Err(ConsensusError::Unauthenticated(_))
    ));
    let metrics = restarted.socket_peer_metrics("node-b").unwrap();
    assert_eq!(metrics.durable_queue_frames, 1);
    assert_eq!(metrics.durable_delivery_attempts, 0);
    assert_eq!(metrics.in_flight_bytes, metrics.durable_queue_bytes);
}

#[test]
fn delivery_state_does_not_claim_persistent_socket_thread_ownership() {
    let directory = tempdir().unwrap();
    let durable_store = store(&directory);
    let mut sender = transport();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    assert!(matches!(
        sender.deliver_next_durable_frame(
            &durable_store,
            &mut stream,
            "node-b",
            0,
            SocketDeliveryCrashPoint::None,
        ),
        Ok(DurableSocketDeliveryAction::Idle)
    ));
    assert_eq!(
        sender
            .socket_peer_metrics("node-b")
            .unwrap()
            .durable_queue_frames,
        0
    );
    let _ = fs::remove_dir_all(directory.path());
}
