use std::sync::Arc;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::security::{
    AuditLog, AuditSignerStore, DurableRemoteAuditSink, RemoteAuditAcknowledgement,
    RemoteAuditDecision, RemoteAuditEnvelope, SecurityError,
};

fn trusted_store(signer_id: &str, key: &SigningKey) -> AuditSignerStore {
    let mut store = AuditSignerStore::default();
    store
        .trust_public_key(signer_id, &key.verifying_key().to_bytes())
        .unwrap();
    store
}

fn audit_log(directory: &std::path::Path, key: SigningKey) -> AuditLog {
    let signer_id = "node-a-signer";
    AuditLog::open_with_signer(
        directory.join("audit.jsonl"),
        signer_id,
        key.clone(),
        trusted_store(signer_id, &key),
    )
    .unwrap()
}

fn source_and_sink(directory: &std::path::Path) -> (AuditLog, SigningKey, SigningKey) {
    let source_key = SigningKey::from_bytes(&[41u8; 32]);
    let sink_key = SigningKey::from_bytes(&[42u8; 32]);
    (
        audit_log(directory, source_key.clone()),
        source_key,
        sink_key,
    )
}

fn sink_for(
    directory: &std::path::Path,
    source_key: &SigningKey,
    sink_key: &SigningKey,
) -> DurableRemoteAuditSink {
    DurableRemoteAuditSink::open(
        directory.join("outbox"),
        "cluster-a",
        trusted_store("node-a-signer", source_key),
        trusted_store("remote-sink", sink_key),
    )
    .unwrap()
}

fn envelope(log: &AuditLog, key: &SigningKey, event: &str) -> RemoteAuditEnvelope {
    let record = log
        .append(
            event,
            "node-a",
            "state",
            "allow",
            &serde_json::json!({"event": event}),
        )
        .unwrap();
    RemoteAuditEnvelope::from_record("cluster-a", "node-a", "consensus", &record, key).unwrap()
}

#[test]
fn envelope_signature_and_cluster_binding_fail_closed() {
    let directory = tempdir().unwrap();
    let (log, source_key, _sink_key) = source_and_sink(directory.path());
    let mut tampered_envelope = envelope(&log, &source_key, "commit");
    let trusted = trusted_store("node-a-signer", &source_key);
    tampered_envelope.validate("cluster-a", &trusted).unwrap();
    tampered_envelope.record_hash = "a".repeat(64);
    assert!(matches!(
        tampered_envelope.validate("cluster-a", &trusted),
        Err(SecurityError::RemoteAuditRejected(_)) | Err(SecurityError::InvalidSignature)
    ));
    let valid = envelope(&log, &source_key, "snapshot");
    assert!(matches!(
        valid.validate("cluster-b", &trusted),
        Err(SecurityError::RemoteAuditRejected(_))
    ));
}

#[test]
fn enqueue_is_idempotent_and_pending_replays_in_stream_order() {
    let directory = tempdir().unwrap();
    let (log, source_key, sink_key) = source_and_sink(directory.path());
    let first = envelope(&log, &source_key, "first");
    let second = envelope(&log, &source_key, "second");
    let sink = sink_for(directory.path(), &source_key, &sink_key);
    sink.enqueue(&second).unwrap();
    sink.enqueue(&first).unwrap();
    sink.enqueue(&first).unwrap();
    let pending = sink.pending().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].source_sequence, 1);
    assert_eq!(pending[1].source_sequence, 2);
}

#[test]
fn same_stream_sequence_with_different_hash_is_rejected() {
    let first_directory = tempdir().unwrap();
    let second_directory = tempdir().unwrap();
    let (first_log, source_key, sink_key) = source_and_sink(first_directory.path());
    let (second_log, _same_source_key, _same_sink_key) = source_and_sink(second_directory.path());
    let first = envelope(&first_log, &source_key, "first");
    let second_record = second_log
        .append(
            "different",
            "node-a",
            "state",
            "deny",
            &serde_json::json!({"x": 2}),
        )
        .unwrap();
    let second = RemoteAuditEnvelope::from_record(
        "cluster-a",
        "node-a",
        "consensus",
        &second_record,
        &source_key,
    )
    .unwrap();
    assert_eq!(first.source_sequence, second.source_sequence);
    assert_ne!(first.envelope_hash, second.envelope_hash);
    let sink = sink_for(first_directory.path(), &source_key, &sink_key);
    sink.enqueue(&first).unwrap();
    assert!(matches!(
        sink.enqueue(&second),
        Err(SecurityError::RemoteAuditCollision(_))
    ));
}

#[test]
fn awaiting_predecessor_retains_outbox_and_acceptance_removes_it() {
    let directory = tempdir().unwrap();
    let (log, source_key, sink_key) = source_and_sink(directory.path());
    let record = log
        .append(
            "commit",
            "node-a",
            "state",
            "allow",
            &serde_json::json!({"x": 1}),
        )
        .unwrap();
    let envelope =
        RemoteAuditEnvelope::from_record("cluster-a", "node-a", "consensus", &record, &source_key)
            .unwrap();
    let sink = sink_for(directory.path(), &source_key, &sink_key);
    sink.enqueue(&envelope).unwrap();
    let gap = RemoteAuditAcknowledgement::new(
        "cluster-a",
        "remote-sink",
        &envelope,
        RemoteAuditDecision::AwaitingPredecessor,
        1,
        None,
        &sink_key,
    )
    .unwrap();
    assert!(!sink.acknowledge(&gap).unwrap());
    assert_eq!(sink.pending().unwrap().len(), 1);
    let accepted = RemoteAuditAcknowledgement::new(
        "cluster-a",
        "remote-sink",
        &envelope,
        RemoteAuditDecision::Accepted,
        2,
        Some("global-order-1"),
        &sink_key,
    )
    .unwrap();
    assert!(sink.acknowledge(&accepted).unwrap());
    assert!(sink.pending().unwrap().is_empty());
}

#[test]
fn acknowledgement_binding_and_sink_signature_are_verified() {
    let directory = tempdir().unwrap();
    let (log, source_key, sink_key) = source_and_sink(directory.path());
    let envelope = envelope(&log, &source_key, "commit");
    let sink = sink_for(directory.path(), &source_key, &sink_key);
    sink.enqueue(&envelope).unwrap();
    let mut acknowledgement = RemoteAuditAcknowledgement::new(
        "cluster-a",
        "remote-sink",
        &envelope,
        RemoteAuditDecision::Accepted,
        2,
        None,
        &sink_key,
    )
    .unwrap();
    acknowledgement.envelope_hash = "b".repeat(64);
    assert!(matches!(
        sink.acknowledge(&acknowledgement),
        Err(SecurityError::RemoteAuditRejected(_))
    ));
    assert_eq!(sink.pending().unwrap().len(), 1);

    let forged_key = SigningKey::from_bytes(&[43u8; 32]);
    let forged = RemoteAuditAcknowledgement::new(
        "cluster-a",
        "remote-sink",
        &envelope,
        RemoteAuditDecision::Accepted,
        2,
        None,
        &forged_key,
    )
    .unwrap();
    assert!(matches!(
        sink.acknowledge(&forged),
        Err(SecurityError::UntrustedSigner(_))
    ));
}

#[test]
fn retryable_decision_retains_pending_envelope_for_replay() {
    let directory = tempdir().unwrap();
    let (log, source_key, sink_key) = source_and_sink(directory.path());
    let envelope = envelope(&log, &source_key, "retry");
    let sink = Arc::new(sink_for(directory.path(), &source_key, &sink_key));
    sink.enqueue(&envelope).unwrap();
    let retry = RemoteAuditAcknowledgement::new(
        "cluster-a",
        "remote-sink",
        &envelope,
        RemoteAuditDecision::RetryableFailure,
        1,
        None,
        &sink_key,
    )
    .unwrap();
    assert!(!sink.acknowledge(&retry).unwrap());
    assert_eq!(sink.pending().unwrap().len(), 1);
}
