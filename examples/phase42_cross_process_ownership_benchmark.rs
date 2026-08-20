use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::fs;
use std::time::Instant;
use un1c0::cross_process_ownership::{
    CrossProcessOwnershipStore, ManagedVolumeRecoveryEvidence, ManagedVolumeRecoveryGate,
    ManagedVolumeRecoveryState, OwnershipClaim,
};

#[derive(Debug, Serialize)]
struct Phase42Benchmark {
    phase: u8,
    ownership_cycles: usize,
    acquisitions: usize,
    releases: usize,
    lease_write_permits: usize,
    recovery_evidence_count: usize,
    recovery_state: ManagedVolumeRecoveryState,
    acquisition_p95_us: u64,
    acquisition_max_us: u64,
    wall_time_us: u64,
    acquisitions_per_second_milli: u64,
    secret_material_recorded: bool,
    cluster_mutation_performed: bool,
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(generation: usize) -> String {
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
    let directory = std::env::temp_dir().join(format!("un1c0-phase42-{}", std::process::id()));
    let path = directory.join("ownership.json");
    let owner = key(81);
    let replica_a = key(82);
    let replica_b = key(83);
    let mut store = CrossProcessOwnershipStore::new(&path, "cluster-a", "resource-a", "snapshot-a")
        .expect("ownership store should construct");
    store
        .register_owner("owner-a", &owner.verifying_key())
        .expect("owner should register");

    let ownership_cycles = 32;
    let started = Instant::now();
    let mut acquisition_latencies = Vec::with_capacity(ownership_cycles);
    let mut record_hash = un1c0::cross_process_ownership::ZERO_HASH.to_string();
    let mut record = None;
    let mut acquisitions = 0usize;
    let mut releases = 0usize;
    let mut lease_write_permits = 0usize;
    for cycle in 0..ownership_cycles {
        let epoch = cycle as u64 + 1;
        let operation_started = Instant::now();
        let acquired = store
            .acquire(
                OwnershipClaim::sign(
                    "cluster-a",
                    "resource-a",
                    "snapshot-a",
                    "owner-a",
                    "process-a",
                    &record_hash,
                    epoch,
                    100 + epoch,
                    cycle as u64,
                    &hash(cycle),
                    &format!("phase42-fence-{epoch}"),
                    &owner,
                )
                .expect("ownership claim should sign"),
                90 + epoch,
            )
            .expect("ownership acquisition should succeed");
        acquisitions += 1;
        let permit = store
            .admit_write(
                "owner-a",
                "process-a",
                epoch,
                &acquired.record_hash,
                91 + epoch,
            )
            .expect("active owner should receive a write permit");
        assert_eq!(permit.ownership_epoch, epoch);
        lease_write_permits += 1;
        acquisition_latencies.push(operation_started.elapsed().as_micros() as u64);
        if cycle + 1 < ownership_cycles {
            let released = store
                .release(
                    "owner-a",
                    "process-a",
                    epoch,
                    &acquired.record_hash,
                    95 + epoch,
                )
                .expect("owner release should succeed");
            record_hash = released.record_hash;
            releases += 1;
        } else {
            record = Some(acquired);
        }
    }

    let record = record.expect("final ownership record should exist");
    let mut recovery_gate =
        ManagedVolumeRecoveryGate::new("cluster-a", "resource-a", "snapshot-a", 2)
            .expect("recovery gate should construct");
    recovery_gate
        .register_replica("replica-a", &replica_a.verifying_key())
        .expect("replica A should register");
    recovery_gate
        .register_replica("replica-b", &replica_b.verifying_key())
        .expect("replica B should register");
    let evidence = [
        ManagedVolumeRecoveryEvidence::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            record.generation,
            &record.content_hash,
            record.ownership_epoch,
            "replica-a",
            "adapter-a",
            ManagedVolumeRecoveryState::Replicated,
            11,
            21,
            100,
            50,
            &replica_a,
        )
        .expect("replica A evidence should sign"),
        ManagedVolumeRecoveryEvidence::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            record.generation,
            &record.content_hash,
            record.ownership_epoch,
            "replica-b",
            "adapter-b",
            ManagedVolumeRecoveryState::Recovered,
            11,
            21,
            100,
            50,
            &replica_b,
        )
        .expect("replica B evidence should sign"),
    ];
    let decision = recovery_gate
        .admit(&record, &evidence, 105)
        .expect("recovery quorum should admit");
    let wall_time_us = started.elapsed().as_micros() as u64;
    let acquisitions_per_second_milli = if wall_time_us == 0 {
        0
    } else {
        ((acquisitions as u128 * 1_000_000_000)
            .checked_div(wall_time_us as u128)
            .unwrap_or_default()
            .min(u64::MAX as u128)) as u64
    };
    let report = Phase42Benchmark {
        phase: 42,
        ownership_cycles,
        acquisitions,
        releases,
        lease_write_permits,
        recovery_evidence_count: decision.evidence_count,
        recovery_state: decision.state,
        acquisition_p95_us: percentile95(&acquisition_latencies),
        acquisition_max_us: acquisition_latencies
            .iter()
            .copied()
            .max()
            .unwrap_or_default(),
        wall_time_us,
        acquisitions_per_second_milli,
        secret_material_recorded: false,
        cluster_mutation_performed: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("benchmark report should serialize")
    );
    let _ = fs::remove_dir_all(directory);
}
