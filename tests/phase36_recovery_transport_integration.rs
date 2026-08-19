use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedTransportEnvelope, AuthenticatedTransportReceiver, MultiLeaderConfig,
    MultiLeaderFailoverAuthority, ProtectedWriteAction, ProtectedWriteGateway,
    ProtectedWriteRequest, RecoveryTransportError, RegionalLeader, ReplayDecision,
    ReplicatedRecoveryConfig, ReservationAction, ReservationPersistenceFault,
    TransportChaosDelivery, TransportChaosFault, TransportChaosHarness, TransportKeyRegistry,
    TransportMessageKind, TrustedFencingAuthorityRegistry, WitnessReservationStore, WitnessVote,
    WitnessVoteReservation,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAYLOAD_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn transport_registry(sender_key: &SigningKey) -> TransportKeyRegistry {
    let mut registry = TransportKeyRegistry::new();
    registry
        .register("leader-b", &sender_key.verifying_key())
        .unwrap();
    registry
}

#[test]
fn authenticated_envelope_binds_identity_payload_and_receiver_before_dispatch() {
    let sender_key = key(11);
    let registry = transport_registry(&sender_key);
    let mut receiver = AuthenticatedTransportReceiver::new(
        "witness-1",
        "un1c0-cluster",
        "recovery-resource",
        1,
        registry,
    )
    .unwrap();
    let envelope = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        1,
        1,
        "nonce-1",
        TransportMessageKind::LeaderProposal,
        b"proposal-bytes".to_vec(),
        &sender_key,
    )
    .unwrap();
    let accepted = receiver.receive(envelope.clone()).unwrap();
    assert_eq!(accepted.decision, ReplayDecision::Accepted);
    assert_eq!(accepted.payload, b"proposal-bytes".to_vec());
    let replay = receiver.receive(envelope.clone()).unwrap();
    assert_eq!(replay.decision, ReplayDecision::AlreadySeen);

    let mut tampered = envelope.clone();
    tampered.payload[0] ^= 1;
    assert!(matches!(
        receiver.receive(tampered),
        Err(RecoveryTransportError::EnvelopeRejected(_))
    ));
    let mut wrong_receiver = envelope;
    wrong_receiver.receiver_id = "witness-2".into();
    assert!(matches!(
        receiver.receive(wrong_receiver),
        Err(RecoveryTransportError::EnvelopeRejected(_))
    ));
}

#[test]
fn transport_replay_window_rejects_stale_sequences_and_old_connection_epochs() {
    let sender_key = key(12);
    let registry = transport_registry(&sender_key);
    let mut receiver = AuthenticatedTransportReceiver::new(
        "witness-1",
        "un1c0-cluster",
        "recovery-resource",
        1,
        registry,
    )
    .unwrap();
    let first = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        1,
        1,
        "nonce-a",
        TransportMessageKind::WitnessVote,
        b"vote-a".to_vec(),
        &sender_key,
    )
    .unwrap();
    receiver.receive(first.clone()).unwrap();
    let mut stale_sequence = first.clone();
    stale_sequence.sequence = 1;
    stale_sequence.nonce = "nonce-b".into();
    stale_sequence.payload = b"vote-b".to_vec();
    stale_sequence.payload_hash =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into();
    assert!(matches!(
        receiver.receive(stale_sequence),
        Err(RecoveryTransportError::EnvelopeRejected(_))
    ));

    let next_epoch = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        2,
        1,
        "nonce-c",
        TransportMessageKind::WitnessVote,
        b"vote-c".to_vec(),
        &sender_key,
    )
    .unwrap();
    receiver.receive(next_epoch).unwrap();
    let old_epoch = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        1,
        2,
        "nonce-d",
        TransportMessageKind::WitnessVote,
        b"vote-d".to_vec(),
        &sender_key,
    )
    .unwrap();
    assert!(matches!(
        receiver.receive(old_epoch),
        Err(RecoveryTransportError::ReplayRejected(_))
    ));
}

fn reservation(round_id: u64, digest_char: char) -> WitnessVoteReservation {
    WitnessVoteReservation::new(
        round_id,
        "witness-1",
        &std::iter::repeat(digest_char).take(64).collect::<String>(),
        3,
        9,
    )
    .unwrap()
}

#[test]
fn durable_witness_reservations_are_atomic_idempotent_and_conflict_safe() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("witness-reservations.json");
    let mut store = WitnessReservationStore::new(&path);
    assert_eq!(
        store.reserve(reservation(1, 'a')).unwrap(),
        ReservationAction::Reserved
    );
    assert_eq!(
        store.reserve(reservation(1, 'a')).unwrap(),
        ReservationAction::AlreadyReserved
    );
    assert!(matches!(
        store.reserve(reservation(1, 'b')),
        Err(RecoveryTransportError::ReservationRejected(_))
    ));
    assert_eq!(
        store.reserve(reservation(2, 'b')).unwrap(),
        ReservationAction::Reserved
    );
    assert_eq!(store.reservations().unwrap().len(), 2);
}

#[test]
fn reservation_crash_boundaries_preserve_old_state_and_clean_staging_on_restart() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("witness-reservations.json");
    let mut store = WitnessReservationStore::new(&path);
    store.inject_fault(ReservationPersistenceFault::AfterStage);
    assert!(store.reserve(reservation(1, 'a')).is_err());
    assert!(!path.exists());
    assert!(PathBuf::from(format!("{}.staging", path.display())).exists());
    store.clear_fault();
    store.reserve(reservation(1, 'a')).unwrap();

    store.inject_fault(ReservationPersistenceFault::AfterSyncBeforeRename);
    assert!(store.reserve(reservation(2, 'b')).is_err());
    assert_eq!(store.reservations().unwrap().len(), 1);
    store.clear_fault();
    store.reserve(reservation(2, 'b')).unwrap();
    assert_eq!(store.reservations().unwrap().len(), 2);
}

fn authority_fixture() -> (
    MultiLeaderFailoverAuthority,
    SigningKey,
    BTreeMap<String, SigningKey>,
) {
    let authority_key = key(1);
    let witness_keys: BTreeMap<String, SigningKey> = (0..5u8)
        .map(|index| (format!("witness-{}", index + 1), key(20 + index)))
        .collect();
    let witness_public_keys = witness_keys
        .iter()
        .map(|(id, signing_key)| (id.clone(), signing_key.verifying_key().to_bytes().to_vec()))
        .collect();
    let config = MultiLeaderConfig::new("un1c0-cluster", "recovery-resource", 8, 5).unwrap();
    let fencing_config =
        ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128).unwrap();
    let mut authority = MultiLeaderFailoverAuthority::new(
        config,
        fencing_config,
        "authority-a",
        authority_key.clone(),
        witness_public_keys,
        1,
        Some("region-a"),
    )
    .unwrap();
    let leader_key = key(101);
    authority
        .register_leader(
            RegionalLeader::new("leader-b", "region-b", 2, 2, 1, 1, SNAPSHOT, &leader_key).unwrap(),
        )
        .unwrap();
    let proposal = authority.begin_round(1, "leader-b", &leader_key).unwrap();
    for witness_id in ["witness-1", "witness-2", "witness-3"] {
        authority
            .accept_vote(
                &proposal,
                WitnessVote::sign(1, witness_id, 1, &proposal, &witness_keys[witness_id]).unwrap(),
            )
            .unwrap();
    }
    let decision = authority.arbitrate(&proposal).unwrap();
    authority.admit_decision_externally(&decision).unwrap();
    (authority, authority_key, witness_keys)
}

#[test]
fn protected_write_gateway_requires_exact_external_fence_and_is_idempotent() {
    let (authority, authority_key, _witness_keys) = authority_fixture();
    let decision = authority.committed_decision().unwrap().clone();
    let mut registry = TrustedFencingAuthorityRegistry::new();
    registry
        .register("authority-a", &authority_key.verifying_key())
        .unwrap();
    let request = ProtectedWriteRequest {
        operation_id: "op-1".into(),
        resource_id: "recovery-resource".into(),
        owner_region_id: "region-b".into(),
        payload_hash: PAYLOAD_HASH.into(),
    };
    let mut gateway = ProtectedWriteGateway::new("recovery-resource").unwrap();
    let first = gateway
        .admit_write(
            request.clone(),
            decision.fencing_token.clone(),
            &registry,
            "authority-a",
            "un1c0-cluster",
        )
        .unwrap();
    assert_eq!(first.action, ProtectedWriteAction::Accepted);
    let replay = gateway
        .admit_write(
            request.clone(),
            decision.fencing_token.clone(),
            &registry,
            "authority-a",
            "un1c0-cluster",
        )
        .unwrap();
    assert_eq!(replay.action, ProtectedWriteAction::AlreadyAccepted);
    let mut wrong_request = request;
    wrong_request.payload_hash =
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into();
    assert!(matches!(
        gateway.admit_write(
            wrong_request,
            decision.fencing_token,
            &registry,
            "authority-a",
            "un1c0-cluster",
        ),
        Err(RecoveryTransportError::ProtectedWriteRejected(_))
    ));
    assert_eq!(gateway.report().accepted_operations, 1);
}

#[test]
fn cross_host_transport_chaos_preserves_authenticated_delivery_and_duplicate_idempotence() {
    let sender_key = key(77);
    let mut registry = transport_registry(&sender_key);
    registry
        .register("leader-c", &key(78).verifying_key())
        .unwrap();
    let receiver = AuthenticatedTransportReceiver::new(
        "witness-1",
        "un1c0-cluster",
        "recovery-resource",
        4,
        registry,
    )
    .unwrap();
    let mut chaos = TransportChaosHarness::new(receiver);
    chaos
        .set_fault("leader-b", "witness-1", TransportChaosFault::Drop)
        .unwrap();
    let dropped = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        4,
        1,
        "nonce-drop",
        TransportMessageKind::WitnessVote,
        b"drop".to_vec(),
        &sender_key,
    )
    .unwrap();
    assert_eq!(
        chaos.deliver(dropped).unwrap(),
        TransportChaosDelivery::Dropped
    );
    chaos.heal("leader-b", "witness-1");
    chaos
        .set_fault("leader-b", "witness-1", TransportChaosFault::Duplicate)
        .unwrap();
    let duplicated = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        4,
        2,
        "nonce-duplicate",
        TransportMessageKind::WitnessVote,
        b"duplicate".to_vec(),
        &sender_key,
    )
    .unwrap();
    assert_eq!(
        chaos.deliver(duplicated).unwrap(),
        TransportChaosDelivery::DuplicateDelivered
    );
    let report = chaos.report();
    assert_eq!(report.dropped, 1);
    assert_eq!(report.duplicated, 1);
    assert!(report.safety_passed);
    assert!(!report.trace_digest.is_empty());
}
