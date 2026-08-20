use ed25519_dalek::SigningKey;
use serde_json::json;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use tempfile::tempdir;
use un1c0::cross_process_ownership::{
    OwnershipClaim, OwnershipRecord, OwnershipWritePermit, ZERO_HASH,
};
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::ownership_bound_cas_executor::{
    OwnershipBoundCasExecutor, OwnershipBoundCasExecutorMetrics, OwnershipBoundCasIntent,
};
use un1c0::replicated_durability::{
    CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode,
};

const JOBS_PER_PRODUCER: u64 = 16;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn fixture() -> (
    tempfile::TempDir,
    OwnershipBoundCasCoordinator,
    SigningKey,
    [SigningKey; 2],
    OwnershipRecord,
) {
    let directory = tempdir().expect("temporary benchmark directory");
    let owner = key(111);
    let replicas = [key(112), key(113)];
    let mut coordinator = OwnershipBoundCasCoordinator::new(
        directory.path().join("ownership.json"),
        directory.path().join("cas.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
        256,
    )
    .expect("coordinator");
    coordinator
        .register_owner("owner-a", &owner.verifying_key())
        .expect("owner registration");
    coordinator
        .register_replica("replica-a", &replicas[0].verifying_key())
        .expect("replica registration");
    coordinator
        .register_replica("replica-b", &replicas[1].verifying_key())
        .expect("replica registration");
    let record = coordinator
        .acquire(
            OwnershipClaim::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                "owner-a",
                "process-a",
                ZERO_HASH,
                1,
                100_000,
                0,
                &hash('0'),
                "fence-a",
                &owner,
            )
            .expect("initial claim"),
            1,
        )
        .expect("initial ownership");
    (directory, coordinator, owner, replicas, record)
}

fn intent(
    record: &OwnershipRecord,
    owner: &SigningKey,
    replicas: &[SigningKey; 2],
    id: u64,
) -> OwnershipBoundCasIntent {
    let permit = OwnershipWritePermit {
        owner_id: record.owner_id.clone(),
        process_instance: record.process_instance.clone(),
        ownership_epoch: record.ownership_epoch,
        record_hash: record.record_hash.clone(),
    };
    let proposed_hash = hash(b"0123456789abcdef"[(id as usize) % 16] as char);
    let request = CasWriteRequest::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "owner-a",
        record.ownership_epoch,
        &format!("benchmark-conflict-{id}"),
        record.generation,
        &record.content_hash,
        record.generation + 1,
        &proposed_hash,
        &proposed_hash,
        owner,
    )
    .expect("CAS request");
    let acknowledgements = vec![
        ReplicaDurabilityAcknowledgement::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &request.request_hash,
            request.proposed_generation,
            &request.proposed_hash,
            "replica-a",
            ReplicaDurabilityMode::ReplicatedVolume,
            7,
            100 + id,
            50,
            &replicas[0],
        )
        .expect("acknowledgement"),
        ReplicaDurabilityAcknowledgement::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &request.request_hash,
            request.proposed_generation,
            &request.proposed_hash,
            "replica-b",
            ReplicaDurabilityMode::ReplicatedVolume,
            7,
            100 + id,
            50,
            &replicas[1],
        )
        .expect("acknowledgement"),
    ];
    OwnershipBoundCasIntent {
        permit,
        request,
        acknowledgements,
        current_tick: 105 + id,
    }
}

fn run_level(producers: u64) -> serde_json::Value {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let queue_capacity = (producers * JOBS_PER_PRODUCER + 1) as usize;
    let executor =
        Arc::new(OwnershipBoundCasExecutor::new(coordinator, queue_capacity).expect("executor"));
    let barrier = Arc::new(Barrier::new(producers as usize));
    let mut handles = Vec::new();
    for producer in 0..producers {
        let executor = Arc::clone(&executor);
        let barrier = Arc::clone(&barrier);
        let owner = owner.clone();
        let replicas = replicas.clone();
        let record = record.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            let mut outcomes = Vec::new();
            for offset in 0..JOBS_PER_PRODUCER {
                let id = producer * JOBS_PER_PRODUCER + offset;
                let ticket = executor
                    .submit(intent(&record, &owner, &replicas, id))
                    .expect("bounded queue admission");
                outcomes.push(ticket.wait().expect("worker response"));
            }
            outcomes
        }));
    }
    let started = Instant::now();
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.extend(handle.join().expect("producer"));
    }
    let wall_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let metrics: OwnershipBoundCasExecutorMetrics = executor.metrics();
    assert_eq!(
        outcomes.len(),
        producers as usize * JOBS_PER_PRODUCER as usize
    );
    assert_eq!(metrics.accepted_intents, outcomes.len() as u64);
    assert_eq!(metrics.completed_intents, outcomes.len() as u64);
    assert_eq!(metrics.queue_full_rejections, 0);
    assert_eq!(metrics.failed_intents, metrics.completed_intents - 1);
    let throughput = if wall_us > 0.0 {
        metrics.completed_intents as f64 / (wall_us / 1_000_000.0)
    } else {
        0.0
    };
    let mut executor = Arc::try_unwrap(executor).expect("executor ownership");
    executor.close().expect("executor close");
    json!({
        "producers": producers,
        "jobs": metrics.completed_intents,
        "successful_commits": metrics.completed_intents - metrics.failed_intents,
        "failed_conflicts": metrics.failed_intents,
        "queue_full_rejections": metrics.queue_full_rejections,
        "wall_us": wall_us,
        "throughput_intents_per_sec": throughput,
        "queue_wait_p50_us": metrics.queue_wait_p50_us,
        "queue_wait_p95_us": metrics.queue_wait_p95_us,
        "queue_wait_max_us": metrics.queue_wait_max_us,
        "service_p50_us": metrics.service_p50_us,
        "service_p95_us": metrics.service_p95_us,
        "service_max_us": metrics.service_max_us,
        "end_to_end_p50_us": metrics.end_to_end_p50_us,
        "end_to_end_p95_us": metrics.end_to_end_p95_us,
        "end_to_end_max_us": metrics.end_to_end_max_us,
        "latency_sample_count": metrics.latency_sample_count,
        "latency_sample_cap": metrics.latency_sample_cap,
    })
}

fn main() {
    let results = [1_u64, 2, 4, 8, 16]
        .into_iter()
        .map(run_level)
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "phase": 44,
            "workload": "bounded concurrent same-generation ownership-bound CAS contention",
            "jobs_per_producer": JOBS_PER_PRODUCER,
            "quorum": 2,
            "results": results,
            "secret_material_recorded": false,
            "cluster_mutation_performed": false,
            "production_boundary": "local worker-owned coordinator, bounded queue, and filesystem-backed evidence only",
        })
    );
}
