#[cfg(feature = "benchmark")]
mod benchmark {
    use std::hint::black_box;
    use std::time::Instant;

    use ed25519_dalek::SigningKey;
    use serde::Serialize;
    use tempfile::tempdir;
    use un1c0::emission_diagnostic_service_identity::{
        DurableServiceIdentityOutbox, ServiceIdentityAuthority, ServiceIdentityDescriptor,
        ServiceIdentityRegistry,
    };

    const SAMPLES: usize = 11;
    const BATCH_SIZES: [usize; 3] = [4, 8, 16];

    #[derive(Debug, Serialize)]
    struct Artifact {
        schema_version: u8,
        phase: u8,
        artifact: &'static str,
        samples: usize,
        batch_sizes: Vec<usize>,
        modes: Vec<Row>,
        errors: usize,
        secret_material_recorded: bool,
    }

    #[derive(Debug, Serialize)]
    struct Row {
        mode: &'static str,
        batch: usize,
        samples: usize,
        p50_us: u128,
        p95_us: u128,
        p99_us: u128,
        max_us: u128,
        median_throughput_ops_per_sec: u64,
        submitted_per_trial: usize,
        accepted_per_trial: usize,
        errors: usize,
    }

    fn authority() -> ServiceIdentityAuthority {
        let key = SigningKey::from_bytes(&[81; 32]);
        let identity = ServiceIdentityDescriptor::new("un1c0.local", "bench", "outbox").unwrap();
        let mut registry = ServiceIdentityRegistry::new("svc-outbox-benchmark", identity).unwrap();
        registry
            .register_initial_signer("benchmark-signer", key.verifying_key().to_bytes(), 1)
            .unwrap();
        ServiceIdentityAuthority::new(registry, "benchmark-signer", key, 1).unwrap()
    }

    fn measure(durable_sync: bool, batch: usize) -> (u128, usize, usize, usize) {
        let directory = tempdir().unwrap();
        let outbox = DurableServiceIdentityOutbox::open(directory.path(), batch + 1).unwrap();
        let authority = authority();
        let started = Instant::now();
        let mut accepted = 0;
        let mut errors = 0;
        for index in 0..batch {
            let stream_id = format!("bench-stream-{index}");
            let envelope = authority
                .issue([((index % 251) + 1) as u8; 32], &stream_id, 1, None)
                .unwrap();
            let result = if durable_sync {
                outbox.enqueue(&envelope, authority.registry())
            } else {
                outbox.enqueue_without_sync_for_benchmark(&envelope, authority.registry())
            };
            match result {
                Ok(true) => accepted += 1,
                Ok(false) => {}
                Err(_) => errors += 1,
            }
            black_box(&envelope);
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        (elapsed_us, batch, accepted, errors)
    }

    fn percentile(values: &[u128], numerator: usize, denominator: usize) -> u128 {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let index = ((sorted.len() - 1) * numerator).div_ceil(denominator);
        sorted[index]
    }

    fn median_throughput(values: &[u64]) -> u64 {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    pub fn run() {
        let mut rows = Vec::new();
        let mut total_errors = 0;
        for batch in BATCH_SIZES {
            for (durable_sync, label) in [(true, "durable_sync"), (false, "no_sync_benchmark_only")]
            {
                let mut elapsed = Vec::with_capacity(SAMPLES);
                let mut throughputs = Vec::with_capacity(SAMPLES);
                let mut submitted = None;
                let mut accepted = None;
                let mut row_errors = 0;
                for _ in 0..SAMPLES {
                    let (elapsed_us, submitted_trial, accepted_trial, errors) =
                        measure(durable_sync, batch);
                    elapsed.push(elapsed_us);
                    throughputs
                        .push((submitted_trial as u64 * 1_000_000 / elapsed_us as u64).max(1));
                    submitted.get_or_insert(submitted_trial);
                    accepted.get_or_insert(accepted_trial);
                    if submitted != Some(submitted_trial) || accepted != Some(accepted_trial) {
                        row_errors += 1;
                    }
                    row_errors += errors;
                }
                total_errors += row_errors;
                rows.push(Row {
                    mode: label,
                    batch,
                    samples: SAMPLES,
                    p50_us: percentile(&elapsed, 1, 2),
                    p95_us: percentile(&elapsed, 95, 100),
                    p99_us: percentile(&elapsed, 99, 100),
                    max_us: *elapsed.iter().max().unwrap(),
                    median_throughput_ops_per_sec: median_throughput(&throughputs),
                    submitted_per_trial: submitted.unwrap(),
                    accepted_per_trial: accepted.unwrap(),
                    errors: row_errors,
                });
            }
        }
        let artifact = Artifact {
            schema_version: 1,
            phase: 80,
            artifact: "diagnostic_outbox_sync_comparison",
            samples: SAMPLES,
            batch_sizes: BATCH_SIZES.to_vec(),
            modes: rows,
            errors: total_errors,
            secret_material_recorded: false,
        };
        assert_eq!(artifact.errors, 0, "benchmark must have zero errors");
        assert!(!artifact.secret_material_recorded);
        println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
    }
}

#[cfg(feature = "benchmark")]
fn main() {
    benchmark::run();
}

#[cfg(not(feature = "benchmark"))]
fn main() {
    eprintln!("phase80_outbox_sync_benchmark requires --features benchmark");
}
