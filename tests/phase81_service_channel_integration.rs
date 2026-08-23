use std::fs;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::emission_diagnostic_service_channel::{
    AuthenticatedServiceChannelReceiver, DurableReplayEpochStore, ServiceChannelError,
    ServiceChannelReadiness, ServiceChannelReplayDecision, ServiceChannelResourceBudget,
    ServiceChannelSender,
};
use un1c0::emission_diagnostic_service_identity::{
    ServiceIdentityDescriptor, ServiceIdentityRegistry,
};

const CHANNEL_ID: &str = "diagnostic-channel";
const RECEIVER_SERVICE_ID: &str = "svc-receiver";
const RECEIVER_IDENTITY_ID: &str = "spiffe://un1c0.local/ns/receiver/sa/endpoint";

fn sender_fixture(seed: u8) -> (ServiceIdentityRegistry, SigningKey) {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let identity = ServiceIdentityDescriptor::new("un1c0.local", "sender", "diagnostic").unwrap();
    let mut registry = ServiceIdentityRegistry::new("svc-sender", identity).unwrap();
    registry
        .register_initial_signer("sender-signer", key.verifying_key().to_bytes(), 1)
        .unwrap();
    (registry, key)
}

fn sender(registry: ServiceIdentityRegistry, key: SigningKey, epoch: u64) -> ServiceChannelSender {
    ServiceChannelSender::new(
        registry,
        "sender-signer",
        key,
        CHANNEL_ID,
        RECEIVER_SERVICE_ID,
        RECEIVER_IDENTITY_ID,
        epoch,
    )
    .unwrap()
}

fn receiver(
    path: &std::path::Path,
    registry: ServiceIdentityRegistry,
    epoch: u64,
) -> AuthenticatedServiceChannelReceiver {
    receiver_with_budget(
        path,
        registry,
        epoch,
        ServiceChannelResourceBudget::default(),
    )
}

fn receiver_with_budget(
    path: &std::path::Path,
    registry: ServiceIdentityRegistry,
    epoch: u64,
    budget: ServiceChannelResourceBudget,
) -> AuthenticatedServiceChannelReceiver {
    let store = DurableReplayEpochStore::open_with_budget(
        path,
        CHANNEL_ID,
        registry.service_id(),
        &registry.identity().canonical_id(),
        RECEIVER_SERVICE_ID,
        RECEIVER_IDENTITY_ID,
        epoch,
        budget,
    )
    .unwrap();
    AuthenticatedServiceChannelReceiver::new(
        CHANNEL_ID,
        RECEIVER_SERVICE_ID,
        RECEIVER_IDENTITY_ID,
        registry,
        store,
    )
    .unwrap()
}

#[test]
fn authenticated_channel_binds_independent_service_identities_and_payload_integrity() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, key) = sender_fixture(81);
    let mut tx = sender(registry.clone(), key, 1);
    let mut rx = receiver(&replay_path, registry, 1);
    let envelope = tx.send(b"verified-observation".to_vec(), [1; 16]).unwrap();

    let accepted = rx.receive(envelope.clone()).unwrap();
    assert_eq!(accepted.decision, ServiceChannelReplayDecision::Accepted);
    assert_eq!(accepted.payload, b"verified-observation");
    assert_eq!(rx.replay_state().highest_sequence, 1);

    let mut wrong_receiver = envelope.clone();
    wrong_receiver.receiver_service_id = "svc-other".into();
    assert!(matches!(
        rx.receive(wrong_receiver),
        Err(ServiceChannelError::ServiceMismatch("receiver service"))
    ));

    let mut tampered = envelope;
    tampered.payload[0] ^= 1;
    assert!(matches!(
        rx.receive(tampered),
        Err(ServiceChannelError::InvalidPayload)
    ));
    assert_eq!(rx.replay_state().highest_sequence, 1);
}

#[test]
fn replay_state_binds_canonical_sender_and_receiver_identities() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, _) = sender_fixture(87);
    let sender_identity_id = registry.identity().canonical_id();
    let store = DurableReplayEpochStore::open(
        &replay_path,
        CHANNEL_ID,
        registry.service_id(),
        &sender_identity_id,
        RECEIVER_SERVICE_ID,
        RECEIVER_IDENTITY_ID,
        1,
    )
    .unwrap();
    assert_eq!(store.state().sender_identity_id, sender_identity_id);
    assert_eq!(store.state().receiver_identity_id, RECEIVER_IDENTITY_ID);
    drop(store);

    assert!(matches!(
        DurableReplayEpochStore::open(
            &replay_path,
            CHANNEL_ID,
            registry.service_id(),
            "spiffe://un1c0.local/ns/sender/sa/other",
            RECEIVER_SERVICE_ID,
            RECEIVER_IDENTITY_ID,
            1,
        ),
        Err(ServiceChannelError::ServiceMismatch("replay state"))
    ));
    assert!(matches!(
        DurableReplayEpochStore::open(
            &replay_path,
            CHANNEL_ID,
            registry.service_id(),
            &sender_identity_id,
            RECEIVER_SERVICE_ID,
            "spiffe://un1c0.local/ns/receiver/sa/other",
            1,
        ),
        Err(ServiceChannelError::ServiceMismatch("replay state"))
    ));
}

#[test]
fn replay_epoch_state_survives_restart_and_old_epoch_is_rejected_after_rollover() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, key) = sender_fixture(82);
    let mut tx = sender(registry.clone(), key.clone(), 1);
    let mut rx = receiver(&replay_path, registry.clone(), 1);
    let first = tx.send(b"one".to_vec(), [1; 16]).unwrap();
    rx.receive(first.clone()).unwrap();
    drop(rx);

    fs::write(directory.path().join(".replay.json.tmp"), b"incomplete").unwrap();
    let mut restarted = receiver(&replay_path, registry.clone(), 1);
    let duplicate = restarted.receive(first.clone()).unwrap();
    assert_eq!(
        duplicate.decision,
        ServiceChannelReplayDecision::AlreadySeen
    );

    let second = tx.send(b"two".to_vec(), [2; 16]).unwrap();
    restarted.receive(second).unwrap();
    restarted.advance_epoch(2).unwrap();
    assert_eq!(restarted.replay_state().connection_epoch, 2);

    let stale = first;
    assert!(matches!(
        restarted.receive(stale),
        Err(ServiceChannelError::EpochMismatch {
            expected: 2,
            actual: 1
        })
    ));

    let mut next_tx = sender(registry.clone(), key, 2);
    let next = next_tx.send(b"epoch-two".to_vec(), [3; 16]).unwrap();
    assert_eq!(
        restarted.receive(next).unwrap().decision,
        ServiceChannelReplayDecision::Accepted
    );
    assert_eq!(restarted.replay_state().highest_sequence, 1);
}

#[test]
fn gaps_and_tampering_do_not_advance_durable_replay_state() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, key) = sender_fixture(83);
    let mut tx = sender(registry.clone(), key, 1);
    let mut rx = receiver(&replay_path, registry, 1);
    let first = tx.send(b"first".to_vec(), [4; 16]).unwrap();
    let second = tx.send(b"second".to_vec(), [5; 16]).unwrap();

    assert!(matches!(
        rx.receive(second),
        Err(ServiceChannelError::Gap {
            expected: 1,
            actual: 2
        })
    ));
    assert_eq!(rx.replay_state().highest_sequence, 0);

    let mut tampered = first.clone();
    tampered.signature[0] ^= 1;
    assert!(matches!(
        rx.receive(tampered),
        Err(ServiceChannelError::InvalidSignature)
    ));
    assert_eq!(rx.replay_state().highest_sequence, 0);

    rx.receive(first).unwrap();
    assert_eq!(rx.replay_state().highest_sequence, 1);
}

#[test]
fn revoked_channel_signer_cannot_authenticate_new_frames_and_rotation_requires_new_epoch() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (mut registry, old_key) = sender_fixture(84);
    let mut old_tx = sender(registry.clone(), old_key.clone(), 1);
    let mut rx = receiver(&replay_path, registry.clone(), 1);
    let historical = old_tx.send(b"before-rotation".to_vec(), [6; 16]).unwrap();
    rx.receive(historical).unwrap();

    let new_key = SigningKey::from_bytes(&[85; 32]);
    registry
        .rotate_signer(
            "sender-signer",
            "sender-signer-v2",
            new_key.verifying_key().to_bytes(),
            2,
        )
        .unwrap();
    drop(rx);
    let mut rx = receiver(&replay_path, registry.clone(), 1);
    assert!(matches!(
        rx.receive(old_tx.send(b"old-signer".to_vec(), [7; 16]).unwrap()),
        Err(ServiceChannelError::Signer(
            un1c0::emission_diagnostic_service_identity::ServiceIdentityError::RevokedSigner(_)
        ))
    ));

    rx.advance_epoch(2).unwrap();
    let mut new_tx = ServiceChannelSender::new(
        registry.clone(),
        "sender-signer-v2",
        new_key,
        CHANNEL_ID,
        RECEIVER_SERVICE_ID,
        RECEIVER_IDENTITY_ID,
        2,
    )
    .unwrap();
    let accepted = rx
        .receive(new_tx.send(b"after-rotation".to_vec(), [8; 16]).unwrap())
        .unwrap();
    assert_eq!(accepted.decision, ServiceChannelReplayDecision::Accepted);
}

#[test]
fn readiness_fails_closed_without_an_active_signer() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (mut registry, key) = sender_fixture(88);
    let mut tx = sender(registry.clone(), key, 1);
    registry.revoke_signer("sender-signer").unwrap();
    let mut rx = receiver(&replay_path, registry, 1);
    assert_eq!(rx.readiness(), ServiceChannelReadiness::NoActiveSigner);
    assert!(matches!(
        rx.require_ready(),
        Err(ServiceChannelError::NotReady("active signer"))
    ));
    let envelope = tx.send(b"blocked".to_vec(), [9; 16]).unwrap();
    assert!(matches!(
        rx.receive(envelope),
        Err(ServiceChannelError::NotReady("active signer"))
    ));
    assert_eq!(rx.replay_state().highest_sequence, 0);
}

#[test]
fn resource_budget_rejects_oversized_payload_without_advancing_replay() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, key) = sender_fixture(89);
    let mut tx = sender(registry.clone(), key, 1);
    let mut budget = ServiceChannelResourceBudget::default();
    budget.max_payload_bytes = 3;
    let mut rx = receiver_with_budget(&replay_path, registry, 1, budget);
    assert_eq!(rx.readiness(), ServiceChannelReadiness::Ready);
    let envelope = tx.send(b"four".to_vec(), [10; 16]).unwrap();
    assert!(matches!(
        rx.receive(envelope),
        Err(ServiceChannelError::ResourceLimit("payload bytes"))
    ));
    assert_eq!(rx.replay_state().highest_sequence, 0);
}

#[test]
fn resource_budget_rejects_replay_window_exhaustion_without_state_advance() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, key) = sender_fixture(90);
    let mut tx = sender(registry.clone(), key, 1);
    let mut budget = ServiceChannelResourceBudget::default();
    budget.max_seen_envelope_hashes = 1;
    let mut rx = receiver_with_budget(&replay_path, registry, 1, budget);
    rx.receive(tx.send(b"first".to_vec(), [11; 16]).unwrap())
        .unwrap();
    let second = tx.send(b"second".to_vec(), [12; 16]).unwrap();
    assert!(matches!(
        rx.receive(second),
        Err(ServiceChannelError::ReplayWindowFull)
    ));
    assert_eq!(rx.replay_state().highest_sequence, 1);
}

#[test]
fn invalid_or_oversized_replay_budgets_fail_closed_before_state_load() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, _) = sender_fixture(91);
    let mut invalid = ServiceChannelResourceBudget::default();
    invalid.max_seen_envelope_hashes = 0;
    assert!(matches!(
        DurableReplayEpochStore::open_with_budget(
            &replay_path,
            CHANNEL_ID,
            registry.service_id(),
            &registry.identity().canonical_id(),
            RECEIVER_SERVICE_ID,
            RECEIVER_IDENTITY_ID,
            1,
            invalid,
        ),
        Err(ServiceChannelError::InvalidResourceBudget("replay window"))
    ));

    let mut tiny = ServiceChannelResourceBudget::default();
    tiny.max_replay_state_bytes = 1;
    fs::write(&replay_path, [0u8; 2]).unwrap();
    assert!(matches!(
        DurableReplayEpochStore::open_with_budget(
            &replay_path,
            CHANNEL_ID,
            registry.service_id(),
            &registry.identity().canonical_id(),
            RECEIVER_SERVICE_ID,
            RECEIVER_IDENTITY_ID,
            1,
            tiny,
        ),
        Err(ServiceChannelError::ResourceLimit("replay state bytes"))
    ));
}

#[test]
fn corrupted_replay_epoch_artifact_fails_closed() {
    let directory = tempdir().unwrap();
    let replay_path = directory.path().join("replay.json");
    let (registry, _) = sender_fixture(86);
    let _ = receiver(&replay_path, registry.clone(), 1);
    fs::write(&replay_path, b"{}").unwrap();
    assert!(matches!(
        DurableReplayEpochStore::open(
            &replay_path,
            CHANNEL_ID,
            registry.service_id(),
            &registry.identity().canonical_id(),
            RECEIVER_SERVICE_ID,
            RECEIVER_IDENTITY_ID,
            1,
        ),
        Err(ServiceChannelError::Serialization(_))
    ));
}
