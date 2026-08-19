use ed25519_dalek::SigningKey;
use std::fs;
use std::path::PathBuf;
use un1c0::{
    DisasterRecoveryConfig, DisasterRecoveryController, FailoverAction, RegionFailureObservation,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn main() {
    let mut output = PathBuf::from("benchmarks/phase32_disaster_recovery_metrics.json");
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--output" {
            output = PathBuf::from(args.next().expect("--output requires a path"));
        } else {
            panic!("unknown argument: {argument}");
        }
    }

    let observer_b = key(52);
    let observer_c = key(53);
    let config = DisasterRecoveryConfig::new("un1c0-cluster", 3, 100).unwrap();
    let mut controller =
        DisasterRecoveryController::new(config, "region-a", SNAPSHOT, 1, 1).unwrap();
    controller
        .register_region("region-a", SNAPSHOT, true)
        .unwrap();
    controller
        .register_region("region-b", SNAPSHOT, true)
        .unwrap();
    controller
        .register_region("region-c", SNAPSHOT, true)
        .unwrap();
    controller
        .register_trusted_observer("region-b", &observer_b.verifying_key())
        .unwrap();
    controller
        .register_trusted_observer("region-c", &observer_c.verifying_key())
        .unwrap();
    controller
        .record_region_failure("region-a", 10, "partition detected")
        .unwrap();

    let observation = |observer_id: &str, key: &SigningKey| {
        RegionFailureObservation::sign(
            "un1c0-cluster",
            "region-a",
            observer_id,
            1,
            1,
            10,
            SNAPSHOT,
            "active region unreachable",
            key,
        )
        .unwrap()
    };
    let first_observer = controller
        .ingest_failure_observation(observation("region-b", &observer_b))
        .unwrap();
    let wait_action = controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap();
    let second_observer = controller
        .ingest_failure_observation(observation("region-c", &observer_c))
        .unwrap();
    let promotion = match controller
        .prepare_promotion("region-b", 2, 2, SNAPSHOT)
        .unwrap()
    {
        FailoverAction::Promote(proposal) => {
            controller.commit_promotion(proposal).unwrap();
            true
        }
        _ => false,
    };
    let report = controller.report();
    let output_report = serde_json::json!({
        "benchmark": "phase32_disaster_recovery",
        "verification_mode": "deterministic_local_ed25519_observer_quorum",
        "wait_action": format!("{wait_action:?}"),
        "first_observer_reached_quorum": first_observer,
        "second_observer_reached_quorum": second_observer,
        "promotion_committed": promotion,
        "active_region": report.active_region_id,
        "owner_term": report.owner_term,
        "ownership_epoch": report.ownership_epoch,
        "observer_count": report.observer_count,
        "required_observers": report.required_observers,
        "safety_passed": report.safety_passed,
        "events": report.events,
        "trace_digest": report.trace_digest,
        "private_key_persisted": false,
        "cluster_mutation_performed": false,
        "production_boundary": "not a cloud-region, managed-storage, DNS, process-fencing, or key-custody benchmark",
    });
    fs::write(
        &output,
        serde_json::to_string_pretty(&output_report).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&output_report).unwrap());
}
