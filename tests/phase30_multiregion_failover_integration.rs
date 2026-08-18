use un1c0::{
    FailoverTransfer, LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationConfig,
    MultiRegionSimulationError, NodeId,
};

fn simulator(scenario: &str, seed: u64) -> MultiRegionFailoverSimulator {
    MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region(scenario, seed).unwrap(),
    )
    .unwrap()
}

fn transfer_to_region_b() -> FailoverTransfer {
    FailoverTransfer {
        previous_owner_id: NodeId::new("node-a1").unwrap(),
        new_owner_id: NodeId::new("node-b1").unwrap(),
        owner_term: 2,
        ownership_epoch: 2,
    }
}

#[test]
fn region_topology_is_deterministic_and_trace_is_replayable() {
    let mut first = simulator("replayable", 17);
    let mut second = simulator("replayable", 17);
    for current in [&mut first, &mut second] {
        current.partition_regions("region-a", "region-b").unwrap();
        current.partition_regions("region-a", "region-c").unwrap();
        current.attempt_delivery().unwrap();
        current.heal_all_links();
        current.accept_transfer(transfer_to_region_b()).unwrap();
        current.attempt_delivery().unwrap();
    }
    let first_report = first.report();
    let second_report = second.report();
    assert_eq!(first_report, second_report);
    assert!(first_report.safety_passed);
    assert!(first_report.liveness_passed);
    assert!(first_report.committed);
}

#[test]
fn majority_partition_fences_owner_and_retains_queue() {
    let mut simulation = simulator("majority-partition", 18);
    simulation
        .partition_regions("region-a", "region-b")
        .unwrap();
    simulation
        .partition_regions("region-a", "region-c")
        .unwrap();
    assert!(!simulation.attempt_delivery().unwrap());
    let report = simulation.report();
    assert!(report.safety_passed);
    assert!(!report.committed);
    assert!(report.fenced);
    assert_eq!(report.dropped_acknowledgements, 3);
}

#[test]
fn asymmetric_partition_is_replayable_and_healing_requires_transfer() {
    let mut simulation = simulator("asymmetric", 19);
    for peer in ["node-b1", "node-b2", "node-c1"] {
        simulation
            .set_link_fault("node-a1", peer, LinkFault::Drop)
            .unwrap();
    }
    assert!(!simulation.attempt_delivery().unwrap());
    assert!(simulation.report().fenced);
    simulation.heal_all_links();
    assert!(!simulation.attempt_delivery().unwrap());
    assert!(simulation.report().fenced);
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    assert!(simulation.attempt_delivery().unwrap());
    let report = simulation.report();
    assert!(report.safety_passed);
    assert!(report.liveness_passed);
    assert_eq!(report.active_owner_id.as_str(), "node-b1");
}

#[test]
fn transfer_crash_recovery_restores_fence_before_failover() {
    let mut simulation = simulator("transfer-crash", 20);
    simulation
        .partition_regions("region-a", "region-b")
        .unwrap();
    simulation
        .partition_regions("region-a", "region-c")
        .unwrap();
    simulation.attempt_delivery().unwrap();
    let snapshot = simulation.snapshot().unwrap();

    let mut restarted = MultiRegionFailoverSimulator::from_snapshot(snapshot).unwrap();
    assert!(restarted.report().fenced);
    assert!(!restarted.attempt_delivery().unwrap());
    restarted.heal_all_links();
    restarted.accept_transfer(transfer_to_region_b()).unwrap();
    assert!(restarted.attempt_delivery().unwrap());
    let report = restarted.report();
    assert!(report.safety_passed);
    assert!(report.liveness_passed);
}

#[test]
fn stale_owner_transfer_is_rejected_after_healing() {
    let mut simulation = simulator("stale-owner", 21);
    simulation
        .partition_regions("region-a", "region-b")
        .unwrap();
    simulation
        .partition_regions("region-a", "region-c")
        .unwrap();
    simulation.attempt_delivery().unwrap();
    simulation.heal_all_links();
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    let stale = FailoverTransfer {
        previous_owner_id: NodeId::new("node-a1").unwrap(),
        new_owner_id: NodeId::new("node-c1").unwrap(),
        owner_term: 3,
        ownership_epoch: 3,
    };
    assert!(matches!(
        simulation.accept_transfer(stale),
        Err(MultiRegionSimulationError::InvariantViolation(_))
    ));
    assert!(simulation.attempt_delivery().unwrap());
    assert!(simulation.report().safety_passed);
}

#[test]
fn observer_quorum_admission_requires_distinct_reachable_reports() {
    let mut simulation = simulator("observer-quorum", 25);
    assert!(!simulation
        .submit_observer_quorum_loss("node-b1", 2, "observer-one")
        .unwrap());
    assert!(!simulation.report().fenced);
    assert!(simulation
        .submit_observer_quorum_loss("node-c1", 2, "observer-two")
        .unwrap());
    assert!(simulation.report().fenced);
    assert!(simulation.report().safety_passed);
}

#[test]
fn invalid_fence_observation_does_not_enter_simulation() {
    let mut simulation = simulator("invalid-fence", 22);
    assert!(matches!(
        simulation.record_quorum_loss(3, "no loss"),
        Err(MultiRegionSimulationError::InvalidConfiguration(_))
    ));
    assert!(!simulation.report().fenced);
    assert!(!simulation.report().committed);
}

#[test]
fn clock_skew_boundary_fails_closed_without_transfer_mutation() {
    let mut simulation = simulator("clock-skew", 24);
    simulation.set_clock_skew_ticks(3).unwrap();
    assert!(matches!(
        simulation.accept_transfer(transfer_to_region_b()),
        Err(MultiRegionSimulationError::InvariantViolation(_))
    ));
    let report = simulation.report();
    assert!(report.safety_passed);
    assert!(!report.committed);
    assert_eq!(report.active_owner_id.as_str(), "node-a1");

    simulation.set_clock_skew_ticks(0).unwrap();
    simulation.accept_transfer(transfer_to_region_b()).unwrap();
    assert!(simulation.attempt_delivery().unwrap());
    assert!(simulation.report().liveness_passed);
}

#[test]
fn delayed_link_requires_tick_advance_before_quorum_commit() {
    let mut simulation = simulator("delayed", 23);
    simulation
        .set_link_fault("node-a1", "node-b1", LinkFault::Delay { ticks: 4 })
        .unwrap();
    simulation
        .set_link_fault("node-a1", "node-b2", LinkFault::Drop)
        .unwrap();
    simulation
        .set_link_fault("node-a1", "node-c1", LinkFault::Drop)
        .unwrap();
    assert!(!simulation.attempt_delivery().unwrap());
    assert!(!simulation.report().fenced);
    simulation.advance_ticks(3).unwrap();
    assert!(!simulation.report().committed);
    simulation.advance_ticks(1).unwrap();
    assert!(simulation.report().liveness_passed);
}
