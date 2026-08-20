use ed25519_dalek::SigningKey;
use serde_json::json;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use un1c0::cross_process_ownership::{
    OwnershipClaim, OwnershipRecord, OwnershipWritePermit, ZERO_HASH,
};
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::ownership_bound_cas_admission::{
    AdaptiveAdmissionConfig, AdaptiveAdmissionError, AdaptiveOwnershipBoundCasAdmission,
};
use un1c0::ownership_bound_cas_executor::OwnershipBoundCasIntent;
use un1c0::replicated_durability::{
    CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode,
};

const JOBS_PER_PRODUCER: u64 = 8;
const MAX_LIMITER_RETRIES: usize = 20_000;

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
    let owner = key(151);
    let replicas = [key(152), key(153)];
    let mut coordinator = OwnershipBoundCasCoordinator::new(
        directory.path().join("ownership.json"),
        directory.path().join("cas.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
        512,
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
        &format!("phase46-adaptive-{id}"),
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
    let jobs = producers * JOBS_PER_PRODUCER;
    let worker_count = producers.min(16) as usize;
    let config = AdaptiveAdmissionConfig {
        initial_permits: 8.min(worker_count.max(1)),
        minimum_permits: 1,
        maximum_permits: 32,
        target_service_p95_us: 50_000,
        failure_threshold: 8,
        adjustment_window: 16,
    };
    let mut admission = AdaptiveOwnershipBoundCasAdmission::new(
        coordinator,
        worker_count,
        jobs as usize + 1,
        config,
    )
    .expect("adaptive admission");
    let started = Instant::now();
    let mut tickets = Vec::new();
    let mut limiter_retries = 0_u64;
    let mut successful = 0_u64;
    let mut failed = 0_u64;
    for _ in 0..jobs {
        let candidate = intent(&record, &owner, &replicas, 0);
        loop {
            match admission.submit(candidate.clone()) {
                Ok(ticket) => {
                    tickets.push(ticket);
                    break;
                }
                Err(AdaptiveAdmissionError::Limited { .. }) => {
                    limiter_retries = limiter_retries.saturating_add(1);
                    if limiter_retries as usize > MAX_LIMITER_RETRIES * jobs as usize {
                        panic!("adaptive limiter exceeded bounded retry budget");
                    }
                    if let Some(ticket) = tickets.pop() {
                        if ticket.wait().is_ok() {
                            successful = successful.saturating_add(1);
                        } else {
                            failed = failed.saturating_add(1);
                        }
                    } else {
                        thread::sleep(Duration::from_micros(50));
                    }
                }
                Err(error) => panic!("unexpected adaptive admission error: {error}"),
            }
        }
    }
    for ticket in tickets {
        if ticket.wait().is_ok() {
            successful = successful.saturating_add(1);
        } else {
            failed = failed.saturating_add(1);
        }
    }
    let wall_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let metrics = admission.metrics();
    assert_eq!(successful, 1);
    assert_eq!(failed, jobs - 1);
    assert_eq!(metrics.verifier.submitted_intents, jobs);
    assert_eq!(metrics.verifier.pre_admitted_intents, jobs);
    assert_eq!(metrics.verifier.completed_intents, jobs);
    assert_eq!(metrics.verifier.failed_intents, jobs - 1);
    let throughput = if wall_us > 0.0 {
        jobs as f64 / (wall_us / 1_000_000.0)
    } else {
        0.0
    };
    let output = json!({
        "producers": producers,
        "verification_workers": worker_count,
        "jobs": jobs,
        "successful_commits": successful,
        "failed_conflicts": failed,
        "limiter_retries": limiter_retries,
        "wall_us": wall_us,
        "throughput_intents_per_sec": throughput,
        "adaptive": metrics.admission,
        "verifier": metrics.verifier,
    });
    admission.close().expect("admission close");
    output
}

fn main() {
    let results = [1_u64, 2, 4, 8, 16, 32]
        .into_iter()
        .map(run_level)
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "phase": 46,
            "workload": "adaptive bounded admission with parsed-key verification and same-generation CAS contention",
            "mutation_workers": 1,
            "secret_material_recorded": false,
            "cluster_mutation_performed": false,
            "production_boundary": "local adaptive admission, immutable verification keys, bounded fact cache, and filesystem-backed mutation worker only",
            "results": results,
        })
    );
}
