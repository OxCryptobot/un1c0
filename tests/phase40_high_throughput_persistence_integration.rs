use tempfile::tempdir;
use un1c0::resource_durability::{
    measure_concurrent_snapshot_persistence, ResourceDurabilityError,
};

#[test]
fn concurrent_persistence_completes_all_operations_and_recovers_staging() {
    let directory = tempdir().unwrap();
    let measurement =
        measure_concurrent_snapshot_persistence(directory.path(), b"phase-40-payload", 4, 8, true)
            .unwrap();
    assert_eq!(measurement.operation_count, 32);
    assert_eq!(measurement.completed_operations, 32);
    assert_eq!(measurement.failed_operations, 0);
    assert_eq!(measurement.unique_target_count, 32);
    assert_eq!(measurement.staging_recovery_scans, 4);
    assert!(measurement.stale_staging_seeded);
    assert!(measurement.wall_time_us > 0);
    assert!(measurement.throughput_milli_ops_per_sec > 0);
    assert!(measurement.total_max_us >= measurement.total_p95_us);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn concurrent_measurement_is_sanitized_and_resource_bounded() {
    let directory = tempdir().unwrap();
    let measurement =
        measure_concurrent_snapshot_persistence(directory.path(), &[7_u8; 1024], 2, 4, false)
            .unwrap();
    let sanitized = measurement.sanitized();
    assert_eq!(sanitized.completed_operations, 8);
    assert_eq!(sanitized.unique_target_count, 8);
    assert!(!sanitized.secret_material_recorded);
    assert!(!sanitized.cluster_mutation_performed);
    assert!(sanitized.resource_before.threads.is_some());
    assert!(sanitized.resource_during_workers.threads.is_some());
    assert!(sanitized.resource_during_workers.open_fds.is_some());
    assert!(sanitized.resource_after.open_fds.is_some());
}

#[test]
fn concurrent_bounds_fail_before_filesystem_mutation() {
    let directory = tempdir().unwrap();
    assert!(matches!(
        measure_concurrent_snapshot_persistence(directory.path(), b"x", 0, 1, false),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert!(matches!(
        measure_concurrent_snapshot_persistence(directory.path(), b"x", 33, 1, false),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert!(matches!(
        measure_concurrent_snapshot_persistence(directory.path(), b"x", 1, 513, false),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert!(matches!(
        measure_concurrent_snapshot_persistence(directory.path(), b"x", 32, 129, false),
        Err(ResourceDurabilityError::InvalidInput(_))
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn concurrent_measurement_propagates_filesystem_failure() {
    let directory = tempdir().unwrap();
    let root_file = directory.path().join("not-a-directory");
    std::fs::write(&root_file, b"file").unwrap();
    assert!(matches!(
        measure_concurrent_snapshot_persistence(&root_file, b"x", 2, 2, true),
        Err(ResourceDurabilityError::PersistenceFailed(_))
    ));
}

#[test]
fn sequential_concurrent_runs_leave_no_staging_or_run_directories() {
    let directory = tempdir().unwrap();
    for _ in 0..3 {
        let measurement =
            measure_concurrent_snapshot_persistence(directory.path(), b"repeatable", 3, 3, true)
                .unwrap();
        assert_eq!(measurement.completed_operations, 9);
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
