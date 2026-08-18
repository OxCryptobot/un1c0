use std::fs;
use std::path::PathBuf;
use un1c0::{
    FailoverTransfer, LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationConfig, NodeId,
};

fn transfer_to_region_b() -> FailoverTransfer {
    FailoverTransfer {
        previous_owner_id: NodeId::new("node-a1").unwrap(),
        new_owner_id: NodeId::new("node-b1").unwrap(),
        owner_term: 2,
        ownership_epoch: 2,
    }
}

fn run_majority_partition() -> serde_json::Value {
    let mut simulation = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("majority-partition", 30).unwrap(),
    )
    .unwrap();
    simulation
        .partition_regions("region-a", "region-b")
        .unwrap();
    simulation
        .partition_regions("region-a", "region-c")
        .unwrap();
    let delivery_committed = simulation.attempt_delivery().unwrap();
    serde_json::json!({
        "name": "majority_partition",
        "delivery_committed": delivery_committed,
        "report": simulation.report(),
        "expected": {"safety_passed": true, "liveness_passed": false, "fenced": true}
    })
}

fn run_heal_and_failover() -> serde_json::Value {
    let mut simulation = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("heal-failover", 31).unwrap(),
    )
    .unwrap();
    simulation
        .partition_regions("region-a", "region-b")
        .unwrap();
    simulation
        .partition_regions("region-a", "region-c")
        .unwrap();
    let fenced_delivery = simulation.attempt_delivery().unwrap();
    simulation.heal_all_links();
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    let committed_after_heal = simulation.attempt_delivery().unwrap();
    serde_json::json!({
        "name": "heal_and_failover",
        "fenced_delivery": fenced_delivery,
        "committed_after_heal": committed_after_heal,
        "report": simulation.report(),
        "expected": {"safety_passed": true, "liveness_passed": true, "committed": true}
    })
}

fn run_observer_quorum() -> serde_json::Value {
    let mut simulation = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("observer-quorum", 33).unwrap(),
    )
    .unwrap();
    let first = simulation
        .submit_observer_quorum_loss("node-b1", 2, "observer-one")
        .unwrap();
    let second = simulation
        .submit_observer_quorum_loss("node-c1", 2, "observer-two")
        .unwrap();
    serde_json::json!({
        "name": "observer_quorum",
        "first_report_admitted": first,
        "quorum_report_admitted": second,
        "report": simulation.report(),
        "expected": {"safety_passed": true, "fenced": true}
    })
}

fn run_clock_skew_boundary() -> serde_json::Value {
    let mut simulation = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("clock-skew", 34).unwrap(),
    )
    .unwrap();
    simulation.set_clock_skew_ticks(3).unwrap();
    let blocked = simulation.accept_transfer(transfer_to_region_b()).is_err();
    simulation.set_clock_skew_ticks(0).unwrap();
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    let recovered = simulation.attempt_delivery().unwrap();
    serde_json::json!({
        "name": "clock_skew_boundary",
        "transfer_blocked": blocked,
        "recovered_after_reanchor": recovered,
        "report": simulation.report(),
        "expected": {"safety_passed": true, "committed": true}
    })
}

fn run_asymmetric_partition() -> serde_json::Value {
    let mut simulation = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("asymmetric-partition", 32).unwrap(),
    )
    .unwrap();
    for peer in ["node-b1", "node-b2", "node-c1"] {
        simulation
            .set_link_fault("node-a1", peer, LinkFault::Drop)
            .unwrap();
    }
    let first_attempt = simulation.attempt_delivery().unwrap();
    let first_digest = simulation.trace_digest();
    simulation.heal_all_links();
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    let recovered = simulation.attempt_delivery().unwrap();
    serde_json::json!({
        "name": "asymmetric_partition",
        "first_attempt_committed": first_attempt,
        "recovered_after_heal": recovered,
        "first_trace_digest": first_digest,
        "report": simulation.report(),
        "expected": {"safety_passed": true, "liveness_passed": true, "committed": true}
    })
}

fn main() {
    let mut output = PathBuf::from("benchmarks/phase30_multiregion_failover_metrics.json");
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--output" {
            output = PathBuf::from(args.next().expect("--output requires a path"));
        } else {
            panic!("unknown argument: {argument}");
        }
    }
    let scenarios = vec![
        run_majority_partition(),
        run_heal_and_failover(),
        run_asymmetric_partition(),
        run_observer_quorum(),
        run_clock_skew_boundary(),
    ];
    let report = serde_json::json!({
        "benchmark": "phase30_multiregion_failover",
        "simulator": "deterministic_local_fault_injection",
        "seed_set": [30, 31, 32, 33, 34],
        "scenario_count": scenarios.len(),
        "scenarios": scenarios,
        "production_boundary": "not a cloud-region, kernel, DNS, load-balancer, or managed-storage benchmark",
    });
    fs::write(
        &output,
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
