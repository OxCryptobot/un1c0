use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::fs;
use std::time::Instant;
use un1c0::replicated_durability::{
    CasCommitOutcome, CasDurabilitySnapshotStore, CasWriteRequest,
    ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode, SingleWriterCasStore,
};

#[derive(Debug, Serialize)]
struct Phase41Benchmark {
    phase: u8,
    attempts: usize,
    completed_commits: usize,
    failed_commits: usize,
    final_generation: u64,
    quorum_per_commit: usize,
    commit_p95_us: u64,
    commit_max_us: u64,
    wall_time_us: u64,
    commits_per_second_milli: u64,
    durable_snapshot_round_trip: bool,
    secret_material_recorded: bool,
    cluster_mutation_performed: bool,
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn digest_for(generation: usize) -> String {
    format!("{generation:064x}")
}

fn percentile95(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() * 95 + 99) / 100).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)]
}

fn main() {
    let writer = key(41);
    let replica_a = key(42);
    let replica_b = key(43);
    let mut store = SingleWriterCasStore::new("cluster-a", "resource-a", "snapshot-a", 2, 128)
        .expect("CAS store should construct");
    store
        .register_writer("writer-a", &writer.verifying_key())
        .expect("writer should register");
    store
        .register_replica("replica-a", &replica_a.verifying_key())
        .expect("replica A should register");
    store
        .register_replica("replica-b", &replica_b.verifying_key())
        .expect("replica B should register");

    let attempts = 64;
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(attempts);
    let mut completed_commits = 0usize;
    for generation in 0..attempts {
        let operation_started = Instant::now();
        let expected_hash = if generation == 0 {
            "0".repeat(64)
        } else {
            digest_for(generation)
        };
        let proposed_hash = digest_for(generation + 1);
        let request = CasWriteRequest::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            "writer-a",
            1,
            &format!("phase41-nonce-{generation}"),
            generation as u64,
            &expected_hash,
            generation as u64 + 1,
            &proposed_hash,
            &proposed_hash,
            &writer,
        )
        .expect("CAS request should sign");
        let acknowledgements = [
            ReplicaDurabilityAcknowledgement::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                &request.request_hash,
                request.proposed_generation,
                &request.proposed_hash,
                "replica-a",
                ReplicaDurabilityMode::ReplicatedVolume,
                generation as u64 + 1,
                100 + generation as u64,
                50,
                &replica_a,
            )
            .expect("replica A acknowledgement should sign"),
            ReplicaDurabilityAcknowledgement::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                &request.request_hash,
                request.proposed_generation,
                &request.proposed_hash,
                "replica-b",
                ReplicaDurabilityMode::ReplicatedVolume,
                generation as u64 + 1,
                100 + generation as u64,
                50,
                &replica_b,
            )
            .expect("replica B acknowledgement should sign"),
        ];
        match store
            .commit(request, &acknowledgements, 105 + generation as u64)
            .expect("quorum CAS commit should succeed")
        {
            CasCommitOutcome::Committed(_) => completed_commits += 1,
            CasCommitOutcome::Idempotent(_) => {
                panic!("benchmark must not produce an idempotent first commit")
            }
        }
        latencies.push(operation_started.elapsed().as_micros() as u64);
    }

    let root = std::env::temp_dir().join(format!("un1c0-phase41-{}", std::process::id()));
    let path = root.join("cas.snapshot.json");
    let snapshot_store =
        CasDurabilitySnapshotStore::new(&path, "cluster-a", "resource-a", "snapshot-a")
            .expect("snapshot store should construct");
    snapshot_store
        .save(&store.snapshot().expect("snapshot should hash"))
        .expect("snapshot should persist");
    let durable_snapshot_round_trip = snapshot_store
        .load()
        .expect("snapshot should load")
        .is_some();
    let _ = fs::remove_dir_all(root);

    let wall_time_us = started.elapsed().as_micros() as u64;
    let commits_per_second_milli = if wall_time_us == 0 {
        0
    } else {
        ((completed_commits as u128 * 1_000_000_000)
            .checked_div(wall_time_us as u128)
            .unwrap_or_default()
            .min(u64::MAX as u128)) as u64
    };
    let report = Phase41Benchmark {
        phase: 41,
        attempts,
        completed_commits,
        failed_commits: attempts - completed_commits,
        final_generation: store.state().generation,
        quorum_per_commit: 2,
        commit_p95_us: percentile95(&latencies),
        commit_max_us: latencies.iter().copied().max().unwrap_or_default(),
        wall_time_us,
        commits_per_second_milli,
        durable_snapshot_round_trip,
        secret_material_recorded: false,
        cluster_mutation_performed: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report should serialize")
    );
}
