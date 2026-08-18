use std::collections::BTreeMap;
use std::fs;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedSocketTransport, ConsensusError, ConsensusMessage,
    DurableSocketQueueStore, SocketBackpressureAction, SocketQuotaConfig, SocketTransportMetrics,
    VoteRequest,
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

fn config(max_in_flight_bytes: u64) -> SocketQuotaConfig {
    SocketQuotaConfig::new(max_in_flight_bytes, 4096, 10, 3).unwrap()
}

fn transport(quota: SocketQuotaConfig, epoch: u64) -> AuthenticatedSocketTransport {
    AuthenticatedSocketTransport::new_with_epoch_and_quota(
        "cluster-alpha",
        "node-a",
        keys(),
        8,
        epoch,
        1,
        quota,
    )
    .unwrap()
}

fn queue_metrics(transport: &AuthenticatedSocketTransport) -> SocketTransportMetrics {
    transport.socket_peer_metrics("node-b").unwrap()
}

#[test]
fn durable_queue_round_trip_recovers_bytes_and_quota_after_restart() {
    let directory = tempdir().unwrap();
    let store = DurableSocketQueueStore::new(directory.path().join("socket-queue.json"));
    let message = envelope("durable-1");
    let frame_bytes = serde_json::to_vec(&message).unwrap().len() as u64;
    let quota = config(frame_bytes + 32);
    let mut first = transport(quota, 1);

    assert!(matches!(
        first
            .enqueue_durable_frame_with_backpressure(&store, "node-b", &message, 4)
            .unwrap(),
        SocketBackpressureAction::Admitted { frame_bytes: bytes, .. } if bytes == frame_bytes
    ));
    let first_metrics = queue_metrics(&first);
    assert_eq!(first_metrics.durable_queue_frames, 1);
    assert_eq!(first_metrics.durable_queue_bytes, frame_bytes);
    assert_eq!(first_metrics.in_flight_bytes, frame_bytes);

    let mut restarted = transport(quota, 1);
    restarted.restore_durable_queue_from_store(&store).unwrap();
    assert_eq!(queue_metrics(&restarted), first_metrics);
    let queued = restarted.durable_queue_frame("node-b").unwrap().unwrap();
    assert_eq!(queued.sequence, 1);
    assert_eq!(queued.frame_bytes.len() as u64, frame_bytes);

    restarted
        .acknowledge_durable_frame(&store, "node-b", queued.sequence)
        .unwrap();
    let after_ack = queue_metrics(&restarted);
    assert_eq!(after_ack.durable_queue_frames, 0);
    assert_eq!(after_ack.durable_queue_bytes, 0);
    assert_eq!(after_ack.in_flight_bytes, 0);
    assert!(store.load().unwrap().queued_frames["node-b"].is_empty());
}

#[test]
fn durable_queue_rejects_tampering_and_cleans_partial_staging() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("socket-queue.json");
    let store = DurableSocketQueueStore::new(&path);
    let mut transport = transport(config(4096), 1);
    transport
        .enqueue_durable_frame_with_backpressure(&store, "node-b", &envelope("tamper-1"), 0)
        .unwrap();

    let mut tampered = store.load().unwrap();
    tampered.queued_frames.get_mut("node-b").unwrap()[0].frame_bytes[0] ^= 1;
    fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert!(matches!(
        store.load(),
        Err(ConsensusError::SocketQuota(message)) if message.contains("digest mismatch")
    ));

    let staging = path.with_extension("queue.tmp");
    fs::write(&staging, b"partial").unwrap();
    assert!(store.recover_staging().unwrap());
    assert!(!staging.exists());
    assert!(!store.recover_staging().unwrap());
}

#[test]
fn restart_rejects_epoch_mismatch_without_mutating_transport() {
    let directory = tempdir().unwrap();
    let store = DurableSocketQueueStore::new(directory.path().join("socket-queue.json"));
    let quota = config(4096);
    let mut original = transport(quota, 1);
    original
        .enqueue_durable_frame_with_backpressure(&store, "node-b", &envelope("epoch-1"), 0)
        .unwrap();

    let mut rotated = transport(quota, 2);
    let error = rotated
        .restore_durable_queue_from_store(&store)
        .unwrap_err();
    assert!(matches!(
        error,
        ConsensusError::ReplayEpochMismatch {
            expected: 2,
            received: 1
        }
    ));
    let metrics = queue_metrics(&rotated);
    assert_eq!(metrics.durable_queue_frames, 0);
    assert_eq!(metrics.in_flight_bytes, 0);
}

#[test]
fn durable_queue_backpressures_and_rolls_back_when_store_fails() {
    let directory = tempdir().unwrap();
    let message = envelope("quota-1");
    let frame_bytes = serde_json::to_vec(&message).unwrap().len() as u64;
    let store = DurableSocketQueueStore::new(directory.path().join("socket-queue.json"));
    let mut quota_transport = transport(config(frame_bytes), 1);
    quota_transport
        .enqueue_durable_frame_with_backpressure(&store, "node-b", &message, 7)
        .unwrap();
    assert!(matches!(
        quota_transport
            .enqueue_durable_frame_with_backpressure(&store, "node-b", &envelope("quota-2"), 7)
            .unwrap(),
        SocketBackpressureAction::Backpressured {
            retry_at_tick: 10,
            available_bytes: 0
        }
    ));

    let failing_path = directory.path().join("queue-directory");
    fs::create_dir(&failing_path).unwrap();
    let failing_store = DurableSocketQueueStore::new(&failing_path);
    let mut rollback_transport = transport(config(4096), 1);
    let error = rollback_transport
        .enqueue_durable_frame_with_backpressure(
            &failing_store,
            "node-b",
            &envelope("rollback-1"),
            0,
        )
        .unwrap_err();
    assert!(matches!(error, ConsensusError::SocketQuota(_)));
    let metrics = queue_metrics(&rollback_transport);
    assert_eq!(metrics.durable_queue_frames, 0);
    assert_eq!(metrics.in_flight_bytes, 0);
}
