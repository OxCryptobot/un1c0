use ed25519_dalek::SigningKey;
use std::fs;
use std::path::PathBuf;
use un1c0::{
    LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationConfig, ReplayFaultStep,
    ReplayManifest, SecureReplayEngine,
};

fn key() -> SigningKey {
    SigningKey::from_bytes(&[61; 32])
}

fn simulator() -> MultiRegionFailoverSimulator {
    MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("phase31-replay", 31).unwrap(),
    )
    .unwrap()
}

fn schedule() -> Vec<ReplayFaultStep> {
    vec![ReplayFaultStep {
        sequence: 1,
        tick: 3,
        from: "node-a1".to_string(),
        to: "node-b1".to_string(),
        fault: LinkFault::Drop,
    }]
}

fn main() {
    let mut output = PathBuf::from("benchmarks/phase31_secure_replay_metrics.json");
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--output" {
            output = PathBuf::from(args.next().expect("--output requires a path"));
        } else {
            panic!("unknown argument: {argument}");
        }
    }

    let signing_key = key();
    let manifest = ReplayManifest::new(
        "phase31-replay",
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
        31,
        "phase31-nonce",
        schedule(),
        &signing_key,
    )
    .unwrap();
    let baseline = simulator();
    let seal = SecureReplayEngine::prepare_trace_seal(&baseline, &manifest, &signing_key).unwrap();
    let mut replay_target = baseline.clone();
    let valid = SecureReplayEngine::replay(
        &mut replay_target,
        &manifest,
        &seal,
        &signing_key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap();

    let mut tampered_manifest = manifest.clone();
    tampered_manifest.schedule[0].fault = LinkFault::Corrupt;
    let mut tampered_target = baseline;
    let tampered_rejected = SecureReplayEngine::replay(
        &mut tampered_target,
        &tampered_manifest,
        &seal,
        &signing_key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .is_err();

    let report = serde_json::json!({
        "benchmark": "phase31_secure_replay",
        "verification_mode": "deterministic_local_ed25519_and_sha256",
        "valid_replay": {
            "safety_passed": valid.safety_passed,
            "liveness_passed": valid.liveness_passed,
            "applied_steps": valid.applied_steps,
            "event_count": valid.event_count,
            "trace_digest": valid.trace_digest,
        },
        "security_checks": {
            "signed_manifest_accepted": true,
            "tampered_schedule_rejected": tampered_rejected,
            "trace_seal_verified": true,
            "private_key_persisted": false,
        },
        "production_boundary": "not a production key-custody, transport, or cloud-region benchmark",
    });
    fs::write(
        &output,
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
