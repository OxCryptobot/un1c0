use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use un1c0::lock_free_buffer_pool::LockFreeBufferPool;

const ROUNDS: usize = 128;
const LEVELS: [usize; 3] = [128, 192, 256];
const SAMPLE_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Default)]
struct ProcSample {
    rss_kb: u64,
    hwm_kb: u64,
    vm_peak_kb: u64,
    threads: u64,
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
    let pool = Arc::new(LockFreeBufferPool::new(512, 64).expect("pool"));
    let barrier = Arc::new(Barrier::new(producers + 1));
    let running = Arc::new(AtomicBool::new(true));
    let peak_rss = Arc::new(AtomicU64::new(0));
    let peak_hwm = Arc::new(AtomicU64::new(0));
    let peak_vm = Arc::new(AtomicU64::new(0));
    let peak_threads = Arc::new(AtomicU64::new(0));
    let sampler = {
        let running = Arc::clone(&running);
        let peak_rss = Arc::clone(&peak_rss);
        let peak_hwm = Arc::clone(&peak_hwm);
        let peak_vm = Arc::clone(&peak_vm);
        let peak_threads = Arc::clone(&peak_threads);
        thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                let sample = proc_sample();
                peak_rss.fetch_max(sample.rss_kb, Ordering::Relaxed);
                peak_hwm.fetch_max(sample.hwm_kb, Ordering::Relaxed);
                peak_vm.fetch_max(sample.vm_peak_kb, Ordering::Relaxed);
                peak_threads.fetch_max(sample.threads, Ordering::Relaxed);
                thread::sleep(SAMPLE_INTERVAL);
            }
        })
    };
    let started = Instant::now();
    let mut handles = Vec::with_capacity(producers);
    for _ in 0..producers {
        let pool = Arc::clone(&pool);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let payload = [b'p'; 256];
            barrier.wait();
            for _ in 0..ROUNDS {
                let mut buffer = pool.checkout();
                buffer.extend_from_slice(&payload);
                assert_eq!(buffer.len(), payload.len());
            }
        }));
    }
    barrier.wait();
    for handle in handles {
        handle.join().expect("pool producer");
    }
    let wall_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    running.store(false, Ordering::Release);
    sampler.join().expect("sampler");
    let sample = proc_sample();
    peak_rss.fetch_max(sample.rss_kb, Ordering::Relaxed);
    peak_hwm.fetch_max(sample.hwm_kb, Ordering::Relaxed);
    peak_vm.fetch_max(sample.vm_peak_kb, Ordering::Relaxed);
    peak_threads.fetch_max(sample.threads, Ordering::Relaxed);
    let metrics = pool.metrics();
    json!({
        "producers": producers,
        "rounds_per_producer": ROUNDS,
        "operations": producers * ROUNDS,
        "wall_us": wall_us,
        "operations_per_sec": (producers * ROUNDS) as f64 / (wall_us / 1_000_000.0),
        "peak_rss_kb": peak_rss.load(Ordering::Relaxed),
        "peak_hwm_kb": peak_hwm.load(Ordering::Relaxed),
        "peak_vm_peak_kb": peak_vm.load(Ordering::Relaxed),
        "peak_threads": peak_threads.load(Ordering::Relaxed),
        "pool": metrics,
        "secret_material_recorded": false,
        "cluster_mutation_performed": false,
    })
}

fn main() {
    let results = LEVELS.into_iter().map(run_level).collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "phase": 51,
            "pool_capacity": 512,
            "pool_slots": 64,
            "workload": "bounded lock-free pooled buffers under sustained 128+ producer concurrency",
            "results": results,
            "allocator_note": "RSS/high-water/VmPeak/thread and pool reuse counters are pressure proxies; no tracing GC or allocator attribution",
            "secret_material_recorded": false,
            "cluster_mutation_performed": false,
        })
    );
}
