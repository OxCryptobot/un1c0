use ed25519_dalek::SigningKey;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use un1c0::{
    MultiRegionFailoverSimulator, MultiRegionSimulationConfig, ReplayManifest, ReplayTraceSeal,
};

fn main() {
    let mut output = PathBuf::from("benchmarks/phase31_trace_seal_overhead.json");
    let mut iterations = 2_000usize;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--output" => output = PathBuf::from(args.next().expect("--output requires a path")),
            "--iterations" => {
                iterations = args
                    .next()
                    .expect("--iterations requires a value")
                    .parse()
                    .expect("iterations must be numeric")
            }
            _ => panic!("unknown argument: {argument}"),
        }
    }
    assert!(iterations > 0 && iterations <= 100_000);

    let signing_key = SigningKey::from_bytes(&[71; 32]);
    let simulator = MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("phase31-seal-overhead", 311).unwrap(),
    )
    .unwrap();
    let manifest = ReplayManifest::new(
        "phase31-seal-overhead",
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
        311,
        "phase31-seal-overhead-nonce",
        Vec::new(),
        &signing_key,
    )
    .unwrap();
    let seal = ReplayTraceSeal::sign_for(&manifest, &simulator, &signing_key).unwrap();
    let trusted_key = signing_key.verifying_key();

    for _ in 0..100 {
        seal.verify(&manifest, &trusted_key).unwrap();
    }
    let mut samples_ns = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        seal.verify(&manifest, &trusted_key).unwrap();
        samples_ns.push(started.elapsed().as_nanos() as f64);
    }
    samples_ns.sort_by(|left, right| left.partial_cmp(right).unwrap());
    let percentile = |fraction: f64| {
        let index = ((samples_ns.len() - 1) as f64 * fraction).round() as usize;
        samples_ns[index]
    };
    let sum: f64 = samples_ns.iter().sum();
    let report = serde_json::json!({
        "benchmark": "phase31_trace_seal_verification",
        "verification_mode": "in_process_ed25519_and_canonical_sha256_payload",
        "iterations": iterations,
        "p50_us": percentile(0.50) / 1_000.0,
        "p95_us": percentile(0.95) / 1_000.0,
        "p99_us": percentile(0.99) / 1_000.0,
        "mean_us": sum / samples_ns.len() as f64 / 1_000.0,
        "verification_errors": 0,
        "private_key_persisted": false,
        "production_boundary": "not a network, TLS, cloud-region, or key-custody benchmark",
    });
    fs::write(
        &output,
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
