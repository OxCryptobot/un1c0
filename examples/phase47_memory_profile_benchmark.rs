use ed25519_dalek::SigningKey;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
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

const JOBS_PER_PRODUCER: u64 = 64;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Default)]
struct ProcSample {
    rss_kb: u64,
    hwm_kb: u64,
    vm_peak_kb: u64,
    threads: u64,
}

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
    let owner = key(171);
    let replicas = [key(172), key(173)];
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
) -> OwnershipBoundCasIntent {
    let permit = OwnershipWritePermit {
        owner_id: record.owner_id.clone(),
        process_instance: record.process_instance.clone(),
        ownership_epoch: record.ownership_epoch,
        record_hash: record.record_hash.clone(),
    };
    let proposed_hash = hash('a');
    let request = CasWriteRequest::sign(
        "cluster-a",
        "resource-a",
        "snapshot-a",
        "owner-a",
        record.ownership_epoch,
        "phase47-memory-hot-key",
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
            100,
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
            100,
            50,
            &replicas[1],
        )
        .expect("acknowledgement"),
    ];
    OwnershipBoundCasIntent {
        permit,
        request,
        acknowledgements,
        current_tick: 105,
    }
}

fn proc_sample() -> ProcSample {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut sample = ProcSample::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let value = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        match name {
            "VmRSS:" => sample.rss_kb = value,
            "VmHWM:" => sample.hwm_kb = value,
            "VmPeak:" => sample.vm_peak_kb = value,
            "Threads:" => sample.threads = value,
            _ => {}
        }
    }
    sample
}

fn run_level(producers: usize) -> serde_json::Value {
    let (_directory, coordinator, owner, replicas, record) = fixture();
    let workers = producers.min(16).max(1);
    let config = AdaptiveAdmissionConfig {
        initial_permits: 16.min(workers.max(1)),
        minimum_permits: 1,
        maximum_permits: 32,
        target_service_p95_us: 50_000,
        failure_threshold: 8,
        adjustment_window: 32,
    };
    let admission = Arc::new(
        AdaptiveOwnershipBoundCasAdmission::new(coordinator, workers, 128, config)
            .expect("adaptive admission"),
    );
    let start_barrier = Arc::new(Barrier::new(producers + 1));
    let running = Arc::new(AtomicBool::new(true));
    let peak_rss_kb = Arc::new(AtomicU64::new(0));
    let peak_hwm_kb = Arc::new(AtomicU64::new(0));
    let peak_vm_peak_kb = Arc::new(AtomicU64::new(0));
    let peak_threads = Arc::new(AtomicU64::new(0));
    let sampler_running = Arc::clone(&running);
    let sampler_peak_rss = Arc::clone(&peak_rss_kb);
    let sampler_peak_hwm = Arc::clone(&peak_hwm_kb);
    let sampler_peak_vm = Arc::clone(&peak_vm_peak_kb);
    let sampler_peak_threads = Arc::clone(&peak_threads);
    let sampler = thread::spawn(move || {
        while sampler_running.load(Ordering::Acquire) {
            let sample = proc_sample();
            sampler_peak_rss.fetch_max(sample.rss_kb, Ordering::Relaxed);
            sampler_peak_hwm.fetch_max(sample.hwm_kb, Ordering::Relaxed);
            sampler_peak_vm.fetch_max(sample.vm_peak_kb, Ordering::Relaxed);
            sampler_peak_threads.fetch_max(sample.threads, Ordering::Relaxed);
            thread::sleep(SAMPLE_INTERVAL);
        }
        let sample = proc_sample();
        sampler_peak_rss.fetch_max(sample.rss_kb, Ordering::Relaxed);
        sampler_peak_hwm.fetch_max(sample.hwm_kb, Ordering::Relaxed);
        sampler_peak_vm.fetch_max(sample.vm_peak_kb, Ordering::Relaxed);
        sampler_peak_threads.fetch_max(sample.threads, Ordering::Relaxed);
    });
    let successes = Arc::new(AtomicU64::new(0));
    let failures = Arc::new(AtomicU64::new(0));
    let limiter_retries = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let mut producers_join = Vec::with_capacity(producers);
    for _producer_id in 0..producers {
        let admission = Arc::clone(&admission);
        let barrier = Arc::clone(&start_barrier);
        let owner = owner.clone();
        let replicas = replicas.clone();
        let record = record.clone();
        let successes = Arc::clone(&successes);
        let failures = Arc::clone(&failures);
        let limiter_retries = Arc::clone(&limiter_retries);
        producers_join.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..JOBS_PER_PRODUCER {
                let candidate = intent(&record, &owner, &replicas);
                loop {
                    match admission.submit(candidate.clone()) {
                        Ok(ticket) => {
                            if ticket.wait().is_ok() {
                                successes.fetch_add(1, Ordering::Relaxed);
                            } else {
                                failures.fetch_add(1, Ordering::Relaxed);
                            }
                            break;
                        }
                        Err(AdaptiveAdmissionError::Limited { .. }) => {
                            limiter_retries.fetch_add(1, Ordering::Relaxed);
                            thread::sleep(Duration::from_micros(50));
                        }
                        Err(_) => {
                            failures.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        }));
    }
    start_barrier.wait();
    for handle in producers_join {
        handle.join().expect("producer thread");
    }
    let wall_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    running.store(false, Ordering::Release);
    sampler.join().expect("memory sampler");
    let metrics = admission.metrics();
    json!({
        "producers": producers,
        "jobs": producers as u64 * JOBS_PER_PRODUCER,
        "verification_workers": workers,
        "successful_commits": successes.load(Ordering::Relaxed),
        "failed_outcomes": failures.load(Ordering::Relaxed),
        "limiter_retries": limiter_retries.load(Ordering::Relaxed),
        "wall_us": wall_us,
        "throughput_intents_per_sec": (producers as f64 * JOBS_PER_PRODUCER as f64) / (wall_us / 1_000_000.0),
        "peak_rss_kb": peak_rss_kb.load(Ordering::Relaxed),
        "peak_hwm_kb": peak_hwm_kb.load(Ordering::Relaxed),
        "peak_vm_peak_kb": peak_vm_peak_kb.load(Ordering::Relaxed),
        "peak_threads": peak_threads.load(Ordering::Relaxed),
        "verifier": metrics.verifier,
        "adaptive": metrics.admission,
        "secret_material_recorded": false,
        "cluster_mutation_performed": false,
    })
}

fn main() {
    let results = [32_usize, 64, 96]
        .into_iter()
        .map(run_level)
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "phase": 47,
            "workload": "sustained hot-key ownership-bound CAS contention with process memory sampling",
            "jobs_per_producer": JOBS_PER_PRODUCER,
            "sampling_interval_ms": SAMPLE_INTERVAL.as_millis(),
            "allocator_note": "Rust process RSS/high-water and allocator churn proxies; no tracing GC is present in this path",
            "results": results,
            "secret_material_recorded": false,
            "cluster_mutation_performed": false,
        })
    );
}
