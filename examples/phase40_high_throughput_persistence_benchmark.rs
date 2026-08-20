use serde::Serialize;
use std::fs;
use un1c0::resource_durability::{
    measure_concurrent_snapshot_persistence, ResourceBudget, ResourceBudgetDecision,
};

#[derive(Debug, Serialize)]
struct Phase40Benchmark {
    phase: u8,
    persistence: un1c0::resource_durability::SanitizedConcurrentPersistenceMeasurement,
    budget: ResourceBudgetDecision,
    valid_path_workload: bool,
    secret_material_recorded: bool,
    cluster_mutation_performed: bool,
}

fn main() {
    let root = std::env::temp_dir().join(format!("un1c0-phase40-{}", std::process::id()));
    let payload = vec![b'p'; 4096];
    let measurement = measure_concurrent_snapshot_persistence(&root, &payload, 4, 32, true)
        .expect("bounded concurrent persistence measurement should succeed");
    let budget = ResourceBudget {
        max_rss_kib: Some(256 * 1024),
        max_threads: Some(64),
        max_open_fds: Some(256),
    }
    .evaluate(&measurement.resource_after);
    let report = Phase40Benchmark {
        phase: 40,
        persistence: measurement.sanitized(),
        budget,
        valid_path_workload: true,
        secret_material_recorded: false,
        cluster_mutation_performed: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report should serialize")
    );
    let _ = fs::remove_dir_all(root);
}
