use serde::Serialize;
use std::fs;
use un1c0::resource_durability::{
    measure_atomic_snapshot_persistence, ResourceBudget, ResourceBudgetDecision,
};

#[derive(Debug, Serialize)]
struct Phase39Benchmark {
    phase: u8,
    persistence: un1c0::resource_durability::SanitizedPersistenceMeasurement,
    budget: ResourceBudgetDecision,
    valid_path_workload: bool,
    secret_material_recorded: bool,
    cluster_mutation_performed: bool,
}

fn main() {
    let root = std::env::temp_dir().join(format!("un1c0-phase39-{}", std::process::id()));
    let payload = vec![b'u'; 4096];
    let measurement = measure_atomic_snapshot_persistence(&root, &payload, 64)
        .expect("bounded persistence measurement should succeed");
    let budget = ResourceBudget {
        max_rss_kib: Some(256 * 1024),
        max_threads: Some(64),
        max_open_fds: Some(256),
    }
    .evaluate(&measurement.resource_after);
    let report = Phase39Benchmark {
        phase: 39,
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
