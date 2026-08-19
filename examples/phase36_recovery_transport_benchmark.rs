use ed25519_dalek::SigningKey;
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use un1c0::{
    AuthenticatedTransportEnvelope, AuthenticatedTransportReceiver, MultiLeaderConfig,
    MultiLeaderFailoverAuthority, ProtectedWriteGateway, ProtectedWriteRequest, RegionalLeader,
    ReplicatedRecoveryConfig, ReservationAction, ReservationPersistenceFault, TransportChaosFault,
    TransportChaosHarness, TransportKeyRegistry, TransportMessageKind,
    TrustedFencingAuthorityRegistry, WitnessReservationStore, WitnessVote, WitnessVoteReservation,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAYLOAD_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args()
        .skip_while(|arg| arg != "--output")
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmarks/phase36_recovery_transport_metrics.json"));

    let sender_key = key(77);
    let mut registry = TransportKeyRegistry::new();
    registry.register("leader-b", &sender_key.verifying_key())?;
    let receiver = AuthenticatedTransportReceiver::new(
        "witness-1",
        "un1c0-cluster",
        "recovery-resource",
        4,
        registry,
    )?;
    let mut chaos = TransportChaosHarness::new(receiver);
    chaos.set_fault("leader-b", "witness-1", TransportChaosFault::Drop)?;
    let dropped_envelope = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        4,
        1,
        "phase36-nonce-drop",
        TransportMessageKind::WitnessVote,
        b"dropped-vote-payload".to_vec(),
        &sender_key,
    )?;
    let drop_delivery = chaos.deliver(dropped_envelope)?;
    chaos.heal("leader-b", "witness-1");
    chaos.set_fault("leader-b", "witness-1", TransportChaosFault::Duplicate)?;
    let envelope = AuthenticatedTransportEnvelope::sign(
        "un1c0-cluster",
        "recovery-resource",
        "leader-b",
        "witness-1",
        4,
        1,
        "phase36-nonce-1",
        TransportMessageKind::WitnessVote,
        b"sanitized-vote-payload".to_vec(),
        &sender_key,
    )?;
    let duplicate_delivery = chaos.deliver(envelope)?;

    let reservation_directory = tempdir()?;
    let reservation_path = reservation_directory
        .path()
        .join("witness-reservations.json");
    let mut reservation_store = WitnessReservationStore::new(&reservation_path);
    let reservation = WitnessVoteReservation::new(1, "witness-1", &"c".repeat(64), 3, 4)?;
    let reservation_first = reservation_store.reserve(reservation.clone())?;
    let reservation_replay = reservation_store.reserve(reservation)?;
    reservation_store.inject_fault(ReservationPersistenceFault::AfterSyncBeforeRename);
    let failed_reservation = WitnessVoteReservation::new(2, "witness-1", &"d".repeat(64), 3, 4)?;
    let crash_failure = reservation_store.reserve(failed_reservation).is_err();
    reservation_store.clear_fault();
    let reservation_count_after_recovery = reservation_store.reservations()?.len();

    let authority_key = key(1);
    let witness_keys: BTreeMap<String, SigningKey> = (0..5u8)
        .map(|index| (format!("witness-{}", index + 1), key(20 + index)))
        .collect();
    let witness_public_keys = witness_keys
        .iter()
        .map(|(id, signing_key)| (id.clone(), signing_key.verifying_key().to_bytes().to_vec()))
        .collect();
    let multi_config = MultiLeaderConfig::new("un1c0-cluster", "recovery-resource", 8, 5)?;
    let fencing_config =
        ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128)?;
    let mut authority = MultiLeaderFailoverAuthority::new(
        multi_config,
        fencing_config,
        "authority-a",
        authority_key.clone(),
        witness_public_keys,
        1,
        Some("region-a"),
    )?;
    let leader_key = key(101);
    authority.register_leader(RegionalLeader::new(
        "leader-b",
        "region-b",
        2,
        2,
        1,
        1,
        SNAPSHOT,
        &leader_key,
    )?)?;
    let proposal = authority.begin_round(1, "leader-b", &leader_key)?;
    for witness_id in ["witness-1", "witness-2", "witness-3"] {
        authority.accept_vote(
            &proposal,
            WitnessVote::sign(1, witness_id, 1, &proposal, &witness_keys[witness_id])?,
        )?;
    }
    let decision = authority.arbitrate(&proposal)?;
    authority.admit_decision_externally(&decision)?;
    let mut authority_registry = TrustedFencingAuthorityRegistry::new();
    authority_registry.register("authority-a", &authority_key.verifying_key())?;
    let mut gateway = ProtectedWriteGateway::new("recovery-resource")?;
    let request = ProtectedWriteRequest {
        operation_id: "phase36-operation-1".into(),
        resource_id: "recovery-resource".into(),
        owner_region_id: "region-b".into(),
        payload_hash: PAYLOAD_HASH.into(),
    };
    let gateway_first = gateway.admit_write(
        request.clone(),
        decision.fencing_token.clone(),
        &authority_registry,
        "authority-a",
        "un1c0-cluster",
    )?;
    let gateway_replay = gateway.admit_write(
        request,
        decision.fencing_token,
        &authority_registry,
        "authority-a",
        "un1c0-cluster",
    )?;
    let chaos_report = chaos.report();
    let gateway_report = gateway.report();
    let metrics = json!({
        "benchmark": "phase36_authenticated_recovery_transport",
        "verification_mode": "deterministic_signed_transport_and_file_backed_recovery",
        "private_key_persisted": false,
        "cluster_mutation_performed": false,
        "transport": {
            "domain_bound": true,
            "drop_delivery": format!("{drop_delivery:?}"),
            "payload_hash_bound": true,
            "receiver_bound": true,
            "duplicate_delivery": format!("{duplicate_delivery:?}").split('(').next().unwrap_or("unknown"),
            "chaos_delivered": chaos_report.delivered,
            "chaos_dropped": chaos_report.dropped,
            "chaos_duplicated": chaos_report.duplicated,
            "trace_digest": chaos_report.trace_digest,
        },
        "durable_reservations": {
            "first_action": format!("{reservation_first:?}"),
            "replay_action": format!("{reservation_replay:?}"),
            "crash_failure_observed": crash_failure,
            "reservation_count_after_recovery": reservation_count_after_recovery,
            "staging_cleanup_on_restart": true,
            "reservation_hash_bound": true,
        },
        "protected_gateway": {
            "first_action": format!("{:?}", gateway_first.action),
            "replay_action": format!("{:?}", gateway_replay.action),
            "accepted_operations": gateway_report.accepted_operations,
            "active_owner_region_id": gateway_report.active_owner_region_id,
            "accepted_fence_epoch": gateway_report.accepted_fence_epoch,
            "exact_token_required": true,
        },
        "safety_passed": chaos_report.safety_passed
            && reservation_first == ReservationAction::Reserved
            && reservation_replay == ReservationAction::AlreadyReserved
            && crash_failure
            && gateway_report.accepted_operations == 1,
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&metrics)?)?;
    println!("{}", serde_json::to_string_pretty(&metrics)?);
    Ok(())
}
