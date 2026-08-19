use ed25519_dalek::SigningKey;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use un1c0::{
    DisasterRecoveryConfig, DisasterRecoveryController, FailoverAction, LinkFault,
    MultiRegionFailoverSimulator, MultiRegionSimulationConfig, RegionFailureObservation,
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
        .unwrap_or_else(|| PathBuf::from("benchmarks/phase33_durable_recovery_metrics.json"));

    let observer_b = key(100);
    let observer_c = key(101);
    let config = DisasterRecoveryConfig::new("un1c0-cluster", 3, 100)?;
    let mut controller = DisasterRecoveryController::new(config, "region-a", SNAPSHOT, 1, 1)?;
    for region in ["region-a", "region-b", "region-c"] {
        controller.register_region(region, SNAPSHOT, true)?;
    }
    controller.register_trusted_observer("region-b", &observer_b.verifying_key())?;
    controller.register_trusted_observer("region-c", &observer_c.verifying_key())?;
    controller.record_region_failure("region-a", 40, "benchmark partition")?;
    let first = RegionFailureObservation::sign(
        "un1c0-cluster",
        "region-a",
        "region-b",
        1,
        1,
        40,
        SNAPSHOT,
        "active region unreachable",
        &observer_b,
    )?;
    let second = RegionFailureObservation::sign(
        "un1c0-cluster",
        "region-a",
        "region-c",
        1,
        1,
        40,
        SNAPSHOT,
        "active region unreachable",
        &observer_c,
    )?;
    let first_reached_quorum = controller.ingest_failure_observation(first)?;
    let second_reached_quorum = controller.ingest_failure_observation(second)?;
    let proposal = match controller.prepare_promotion("region-b", 2, 2, SNAPSHOT)? {
        FailoverAction::Promote(proposal) => proposal,
        action => return Err(format!("unexpected action: {action:?}").into()),
    };
    controller.commit_promotion(proposal)?;
    let recovery_snapshot = controller.snapshot()?;

    let simulator_config =
        MultiRegionSimulationConfig::three_region("phase33-benchmark-race", 3302)?;
    let mut simulator = MultiRegionFailoverSimulator::new(simulator_config)?;
    simulator.partition_regions("region-a", "region-b")?;
    simulator.partition_regions("region-a", "region-c")?;
    simulator.inject_link_fault("node-b1", "node-a1", LinkFault::Delay { ticks: 2 })?;
    simulator.inject_link_fault("node-c1", "node-a1", LinkFault::Duplicate)?;
    let base = simulator.snapshot()?;
    let mut branch_b = MultiRegionFailoverSimulator::from_snapshot(base.clone())?;
    let mut branch_c = MultiRegionFailoverSimulator::from_snapshot(base)?;
    branch_b.accept_transfer(un1c0::FailoverTransfer {
        previous_owner_id: un1c0::NodeId::new("node-a1")?,
        new_owner_id: un1c0::NodeId::new("node-b1")?,
        owner_term: 2,
        ownership_epoch: 2,
    })?;
    branch_c.accept_transfer(un1c0::FailoverTransfer {
        previous_owner_id: un1c0::NodeId::new("node-a1")?,
        new_owner_id: un1c0::NodeId::new("node-c1")?,
        owner_term: 2,
        ownership_epoch: 2,
    })?;
    let branch_b_report = branch_b.report();
    let branch_c_report = branch_c.report();

    let metrics = json!({
        "benchmark": "phase33_durable_recovery",
        "verification_mode": "deterministic_local_atomic_snapshot_and_membership_epoch",
        "private_key_persisted": false,
        "cluster_mutation_performed": false,
        "durable_snapshot": {
            "state_hash_bound": !recovery_snapshot.state_hash.is_empty(),
            "pending_or_committed_identity_preserved": recovery_snapshot.committed_proposal.is_some(),
            "membership_epoch": recovery_snapshot.membership_epoch,
            "events": recovery_snapshot.events.len(),
        },
        "membership_epoch": {
            "current_epoch": recovery_snapshot.membership_epoch,
            "observations_bound_to_epoch": recovery_snapshot.observations.values().all(|observation| observation.membership_epoch == recovery_snapshot.membership_epoch),
        },
        "concurrent_partition_race": {
            "base_trace_digest": simulator.trace_digest(),
            "branch_b_safety_passed": branch_b_report.safety_passed,
            "branch_c_safety_passed": branch_c_report.safety_passed,
            "branches_have_distinct_owners": branch_b_report.active_owner_id != branch_c_report.active_owner_id,
            "arbiter_active_region": controller.report().active_region_id,
            "arbiter_safety_passed": controller.report().safety_passed,
        },
        "quorum": {
            "first_observer_reached_quorum": first_reached_quorum,
            "second_observer_reached_quorum": second_reached_quorum,
            "required_observers": controller.report().required_observers,
            "observer_count": controller.report().observer_count,
        },
        "trace_digest": controller.trace_digest(),
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&metrics)?)?;
    println!("{}", serde_json::to_string_pretty(&metrics)?);
    Ok(())
}
