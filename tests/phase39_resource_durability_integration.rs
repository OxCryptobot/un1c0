use tempfile::tempdir;
use un1c0::resource_durability::{
    capture_resource_snapshot, measure_atomic_snapshot_persistence, ResourceBudget,
    ResourceDurabilityError,
};

#[test]
fn resource_snapshot_exposes_only_bounded_process_dimensions() {
    let snapshot = capture_resource_snapshot();
    if let Some(rss_kib) = snapshot.rss_kib {
        assert!(rss_kib > 0);
    }
    if let Some(threads) = snapshot.threads {
        assert!(threads > 0);
    }
    if let Some(open_fds) = snapshot.open_fds {
        assert!(open_fds > 0);
    }
}

#[test]
fn resource_budget_fails_closed_when_a_limit_is_exceeded() {
    let snapshot = capture_resource_snapshot();
    let budget = ResourceBudget {
        max_rss_kib: Some(1),
        max_threads: Some(1),
        max_open_fds: Some(1),
    };
    let decision = budget.evaluate(&snapshot);
    assert!(!decision.within_budget);
    assert!(!decision.violations.is_empty());
}

#[test]
fn atomic_persistence_measurement_records_bytes_syncs_and_resources() {
    let directory = tempdir().unwrap();
    let measurement =
        measure_atomic_snapshot_persistence(directory.path(), b"bounded supervision snapshot", 8)
            .unwrap();
    assert_eq!(measurement.operation_count, 8);
    assert_eq!(
        measurement.bytes_written,
        8 * b"bounded supervision snapshot".len() as u64
    );
    assert!(measurement.total_p95_us > 0);
    assert!(measurement.total_max_us >= measurement.total_p95_us);
    assert!(measurement.resource_before.threads.is_some());
    assert!(measurement.resource_after.threads.is_some());
    let sanitized = measurement.sanitized();
    assert!(!sanitized.secret_material_recorded);
    assert!(!sanitized.cluster_mutation_performed);
}

#[test]
fn stale_staging_is_counted_and_removed_before_atomic_write() {
    let directory = tempdir().unwrap();
    std::fs::write(
        directory.path().join("supervision.snapshot.staging"),
        b"stale",
    )
    .unwrap();
    let measurement =
        measure_atomic_snapshot_persistence(directory.path(), b"snapshot", 1).unwrap();
    assert_eq!(measurement.staging_recovery_scans, 1);
    assert!(!directory
        .path()
        .join("supervision.snapshot.staging")
        .exists());
    assert!(directory.path().join("supervision.snapshot").exists());
}

#[test]
fn oversized_payload_is_rejected_before_filesystem_mutation() {
    let directory = tempdir().unwrap();
    let payload = vec![0_u8; 4 * 1024 * 1024 + 1];
    assert!(matches!(
        measure_atomic_snapshot_persistence(directory.path(), &payload, 1),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn zero_or_excessive_operation_counts_are_rejected() {
    let directory = tempdir().unwrap();
    assert!(matches!(
        measure_atomic_snapshot_persistence(directory.path(), b"snapshot", 0),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert!(matches!(
        measure_atomic_snapshot_persistence(directory.path(), b"snapshot", 513),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
}
