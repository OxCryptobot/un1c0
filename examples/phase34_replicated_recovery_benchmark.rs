use ed25519_dalek::SigningKey;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use un1c0::{
    ChaosFault, DisasterRecoveryConfig, DisasterRecoveryController, ExternalFenceState,
    FailoverAction, ObserverMembership, RegionFailureObservation, ReplicatedRecoveryAction,
    ReplicatedRecoveryAuthority, ReplicatedRecoveryChaosSimulator, ReplicatedRecoveryConfig,
    ReplicatedRecoverySnapshotStore,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn members(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn key_map(entries: &[(&str, &SigningKey)]) -> BTreeMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(id, key)| ((*id).to_string(), key.verifying_key().to_bytes().to_vec()))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args()
        .skip_while(|arg| arg != "--output")
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmarks/phase34_replicated_recovery_metrics.json"));
    let authority_key = key(10);
    let observer_keys: BTreeMap<String, SigningKey> = BTreeMap::from([
        ("region-a".into(), key(20)),
        ("region-b".into(), key(21)),
        ("region-c".into(), key(22)),
        ("region-d".into(), key(23)),
    ]);
    let config = ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128)?;
    let recovery_config = DisasterRecoveryConfig::new("un1c0-cluster", 3, 100)?;
    let mut controller =
        DisasterRecoveryController::new(recovery_config, "region-a", SNAPSHOT, 1, 1)?;
    for region in ["region-a", "region-b", "region-c"] {
        controller.register_region(region, SNAPSHOT, true)?;
    }
    for observer_id in ["region-a", "region-b", "region-c"] {
        controller
            .register_trusted_observer(observer_id, &observer_keys[observer_id].verifying_key())?;
    }
    let initial_members = members(&["region-a", "region-b", "region-c"]);
    let initial_keys = key_map(&[
        ("region-a", &observer_keys["region-a"]),
        ("region-b", &observer_keys["region-b"]),
        ("region-c", &observer_keys["region-c"]),
    ]);
    let membership = ObserverMembership::stable(1, initial_members)?;
    let authority = ReplicatedRecoveryAuthority::new(
        config,
        "authority-a",
        authority_key.clone(),
        membership,
        initial_keys,
        controller,
    )?;
    let node_ids = members(&["region-a", "region-b", "region-c", "region-d"]);
    let mut chaos = ReplicatedRecoveryChaosSimulator::new(authority, node_ids)?;

    let new_members = members(&["region-b", "region-c", "region-d"]);
    let new_keys = key_map(&[
        ("region-b", &observer_keys["region-b"]),
        ("region-c", &observer_keys["region-c"]),
        ("region-d", &observer_keys["region-d"]),
    ]);
    let joint = chaos
        .authority_mut()
        .begin_joint_membership(new_members, new_keys, 2)?;
    let joint_index = match joint {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => return Err("joint transition did not append".into()),
    };
    chaos.partition("region-a", "region-d")?;
    chaos.inject_fault("region-a", "region-b", ChaosFault::Duplicate)?;
    chaos.inject_fault("region-a", "region-c", ChaosFault::Delay { until_tick: 3 })?;
    chaos.deliver_ack("region-a", "region-b", joint_index)?;
    chaos.deliver_ack("region-a", "region-c", joint_index)?;
    chaos.advance_tick(3);
    chaos.deliver_ack("region-a", "region-c", joint_index)?;
    chaos.heal("region-a", "region-d")?;
    chaos.deliver_ack("region-a", "region-d", joint_index)?;
    chaos.commit(joint_index)?;

    let final_entry = chaos.authority_mut().finalize_membership()?;
    let final_index = match final_entry {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => return Err("final transition did not append".into()),
    };
    chaos.deliver_ack("region-a", "region-b", final_index)?;
    chaos.deliver_ack("region-a", "region-d", final_index)?;
    chaos.commit(final_index)?;
    chaos.reject_stale_epoch(1);
    chaos.reject_stale_fence();

    chaos
        .authority_mut()
        .controller_mut()
        .record_region_failure("region-a", 70, "phase34 benchmark partition")?;
    for observer_id in ["region-b", "region-c"] {
        let observation = RegionFailureObservation::sign_at_membership_epoch(
            "un1c0-cluster",
            2,
            "region-a",
            observer_id,
            1,
            1,
            70,
            SNAPSHOT,
            "active region unreachable",
            &observer_keys[observer_id],
        )?;
        chaos
            .authority_mut()
            .controller_mut()
            .ingest_failure_observation(observation)?;
    }
    let proposal = match chaos
        .authority_mut()
        .prepare_recovery("region-b", 2, 2, SNAPSHOT)?
    {
        FailoverAction::Promote(proposal) => proposal,
        _ => return Err("recovery promotion was not prepared".into()),
    };
    let recovery_entry = chaos.authority_mut().append_recovery_commit(proposal)?;
    let recovery_index = match recovery_entry {
        ReplicatedRecoveryAction::Appended { index, .. } => index,
        _ => return Err("recovery commit did not append".into()),
    };
    chaos.deliver_ack("region-a", "region-b", recovery_index)?;
    chaos.deliver_ack("region-a", "region-c", recovery_index)?;
    chaos.commit(recovery_index)?;
    let token = chaos.authority().active_fencing_token().unwrap().clone();
    let mut external = ExternalFenceState::new("recovery-resource")?;
    let external_action = external.apply(
        token.clone(),
        &authority_key.verifying_key(),
        "un1c0-cluster",
    )?;
    let directory = tempdir()?;
    let store = ReplicatedRecoverySnapshotStore::new(directory.path().join("authority.json"));
    chaos.authority().save_snapshot(&store)?;
    let restored = ReplicatedRecoveryAuthority::load_snapshot(&store, authority_key)?;
    let report = chaos.report();
    let restored_report = restored.report();
    let metrics = json!({
        "benchmark": "phase34_replicated_recovery",
        "verification_mode": "deterministic_local_replicated_log_joint_quorum_and_signed_fencing",
        "private_key_persisted": false,
        "cluster_mutation_performed": false,
        "joint_membership": {
            "joint_index": joint_index,
            "final_index": final_index,
            "joint_quorum_committed": true,
            "final_quorum_committed": true,
            "membership_epoch": report.membership_epochs_seen.last().copied().unwrap_or(0),
            "membership_phase_stable": restored_report.membership_phase == un1c0::ObserverMembershipPhase::Stable,
        },
        "external_fencing": {
            "token_signature_verified": true,
            "token_hash_bound": !token.token_hash().is_empty(),
            "fence_epoch": token.fence_epoch,
            "owner_region_id": token.owner_region_id,
            "external_action": match external_action {
            un1c0::ExternalFenceAction::Activated(_) => "activated",
            un1c0::ExternalFenceAction::AlreadyActive(_) => "already_active",
        },
            "external_admission": external.admit(&token, &restored.public_key(), "un1c0-cluster")?,
        },
        "restart": {
            "log_len_preserved": restored_report.log_len == report.committed_entries as usize,
            "commit_index_preserved": restored_report.commit_index == report.committed_entries,
            "fence_epoch_preserved": restored_report.active_fence_epoch == report.active_fence_epoch,
            "trace_digest_preserved": restored_report.trace_digest == chaos.authority().report().trace_digest,
        },
        "chaos": {
            "node_count": report.node_count,
            "dynamic_partition_steps": report.dynamic_partition_steps,
            "stale_epoch_rejections": report.stale_epoch_rejections,
            "stale_fence_rejections": report.stale_fence_rejections,
            "safety_passed": report.safety_passed,
            "trace_digest": report.trace_digest,
        },
        "active_owner_region_id": report.active_owner_region_id,
        "active_fence_epoch": report.active_fence_epoch,
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&metrics)?)?;
    println!("{}", serde_json::to_string_pretty(&metrics)?);
    Ok(())
}
