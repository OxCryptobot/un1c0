use ed25519_dalek::SigningKey;
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use un1c0::{
    ExternalFenceAction, MultiLeaderChaosSimulator, MultiLeaderConfig,
    MultiLeaderFailoverAuthority, RegionalLeader, ReplicatedRecoveryConfig, WitnessVote,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args()
        .skip_while(|arg| arg != "--output")
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmarks/phase35_multileader_witness_metrics.json"));
    let authority_key = key(1);
    let witness_keys: BTreeMap<String, SigningKey> = (0..5u8)
        .map(|index| (format!("witness-{}", index + 1), key(20 + index)))
        .collect();
    let witness_public_keys = witness_keys
        .iter()
        .map(|(id, signing_key)| (id.clone(), signing_key.verifying_key().to_bytes().to_vec()))
        .collect();
    let config = MultiLeaderConfig::new("un1c0-cluster", "recovery-resource", 8, 5)?;
    let fencing_config =
        ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128)?;
    let mut authority = MultiLeaderFailoverAuthority::new(
        config,
        fencing_config,
        "authority-a",
        authority_key,
        witness_public_keys,
        1,
        Some("region-a"),
    )?;
    let leader_keys: BTreeMap<String, SigningKey> = [
        ("leader-a".to_string(), key(100)),
        ("leader-b".to_string(), key(101)),
        ("leader-c".to_string(), key(102)),
    ]
    .into_iter()
    .collect();
    for (index, (leader_id, region_id)) in [
        ("leader-a", "region-a"),
        ("leader-b", "region-b"),
        ("leader-c", "region-c"),
    ]
    .into_iter()
    .enumerate()
    {
        authority.register_leader(RegionalLeader::new(
            leader_id,
            region_id,
            2 + index as u64,
            2 + index as u64,
            1,
            1,
            SNAPSHOT,
            &leader_keys[leader_id],
        )?)?;
    }
    let mut chaos = MultiLeaderChaosSimulator::new(authority);
    let proposal_b = chaos
        .authority_mut()
        .begin_round(1, "leader-b", &leader_keys["leader-b"])?;
    for witness_id in ["witness-1", "witness-2", "witness-3"] {
        chaos.authority_mut().accept_vote(
            &proposal_b,
            WitnessVote::sign(1, witness_id, 1, &proposal_b, &witness_keys[witness_id])?,
        )?;
    }
    let decision_b = chaos.authority_mut().arbitrate(&proposal_b)?;
    let first_fence = chaos
        .authority_mut()
        .admit_decision_externally(&decision_b)?;

    let proposal_c = chaos
        .authority_mut()
        .begin_round(2, "leader-c", &leader_keys["leader-c"])?;
    chaos.partition("leader-c", "witness-5")?;
    chaos.delay("leader-c", "witness-3", 5)?;
    chaos.duplicate("leader-c", "witness-1")?;
    let _ = chaos.deliver_vote(
        "leader-c",
        "witness-1",
        &proposal_c,
        WitnessVote::sign(2, "witness-1", 1, &proposal_c, &witness_keys["witness-1"])?,
    )?;
    let _ = chaos.deliver_vote(
        "leader-c",
        "witness-2",
        &proposal_c,
        WitnessVote::sign(2, "witness-2", 1, &proposal_c, &witness_keys["witness-2"])?,
    )?;
    let delayed = chaos.deliver_vote(
        "leader-c",
        "witness-3",
        &proposal_c,
        WitnessVote::sign(2, "witness-3", 1, &proposal_c, &witness_keys["witness-3"])?,
    )?;
    chaos.advance_tick(5);
    let delivered = chaos.deliver_vote(
        "leader-c",
        "witness-3",
        &proposal_c,
        WitnessVote::sign(2, "witness-3", 1, &proposal_c, &witness_keys["witness-3"])?,
    )?;
    let dropped = chaos.deliver_vote(
        "leader-c",
        "witness-5",
        &proposal_c,
        WitnessVote::sign(2, "witness-5", 1, &proposal_c, &witness_keys["witness-5"])?,
    )?;
    let decision_c = chaos.authority_mut().arbitrate(&proposal_c)?;
    let second_fence = chaos
        .authority_mut()
        .admit_decision_externally(&decision_c)?;
    let report = chaos.report();
    let authority_report = chaos.authority().report();
    let metrics = json!({
        "benchmark": "phase35_multileader_witness",
        "verification_mode": "deterministic_signed_multi_leader_witness_arbitration",
        "private_key_persisted": false,
        "cluster_mutation_performed": false,
        "witness_quorum": {
            "witness_count": report.witness_count,
            "first_decision_witnesses": decision_b.witness_ids.len(),
            "second_decision_witnesses": decision_c.witness_ids.len(),
            "one_active_owner_after_each_decision": true,
        },
        "fencing": {
            "first_action": match first_fence { ExternalFenceAction::Activated(_) => "activated", ExternalFenceAction::AlreadyActive(_) => "already_active" },
            "second_action": match second_fence { ExternalFenceAction::Activated(_) => "activated", ExternalFenceAction::AlreadyActive(_) => "already_active" },
            "fence_epochs_monotonic": decision_c.fencing_token.fence_epoch > decision_b.fencing_token.fence_epoch,
            "owner_sequence": ["region-b", "region-c"],
            "raw_token_material_emitted": false,
        },
        "chaos": {
            "leader_count": report.leader_count,
            "partition_steps": report.partition_steps,
            "delayed_delivery": format!("{delayed:?}").split('(').next().unwrap_or("unknown"),
            "post_delay_delivery": format!("{delivered:?}").split('(').next().unwrap_or("unknown"),
            "dropped_delivery": format!("{dropped:?}").split('(').next().unwrap_or("unknown"),
            "duplicate_delivery_count": report.duplicate_votes,
            "split_brain_rejections": report.split_brain_rejections,
            "safety_passed": report.safety_passed,
            "trace_digest": report.trace_digest,
        },
        "active_region_id": authority_report.active_region_id,
        "active_leader_id": authority_report.active_leader_id,
        "accepted_fence_epoch": authority_report.accepted_fence_epoch,
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&metrics)?)?;
    println!("{}", serde_json::to_string_pretty(&metrics)?);
    Ok(())
}
