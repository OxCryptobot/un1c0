use ed25519_dalek::SigningKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;
use un1c0::{
    AuthenticatedConsensusEnvelope, ConsensusMessage, VoteRequest,
};

const SAMPLES: usize = 2_000;

#[derive(Debug, Serialize)]
struct Scenario {
    name: String,
    cluster_size: usize,
    connected_members: usize,
    messages_attempted: usize,
    messages_verified: usize,
    messages_dropped: usize,
    quorum_available: bool,
    verification_p95_us: f64,
    verification_throughput_ops_per_sec: f64,
}

fn main() {
    let scenarios = [
        ("healthy", 5usize, 5usize),
        ("majority_partition", 5usize, 3usize),
        ("minority_partition", 5usize, 2usize),
    ];
    let report: Vec<Scenario> = scenarios
        .into_iter()
        .map(|(name, cluster_size, connected_members)| {
            measure_scenario(name, cluster_size, connected_members)
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "benchmark": "authenticated_consensus_transport",
            "samples_per_scenario": SAMPLES,
            "scenarios": report,
            "notes": {
                "transport": "in-process Ed25519 envelope verification",
                "partition": "messages crossing the partition are dropped before verification",
                "quorum": "majority of a five-member configuration",
                "production_limit": "not a socket, TLS, kernel, or cross-machine benchmark"
            }
        }))
        .unwrap()
    );
}

fn measure_scenario(name: &str, cluster_size: usize, connected_members: usize) -> Scenario {
    let nodes: Vec<String> = (0..cluster_size).map(|index| format!("node-{index}")).collect();
    let connected: BTreeSet<String> = nodes
        .iter()
        .take(connected_members)
        .cloned()
        .collect();
    let keys: BTreeMap<String, SigningKey> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.clone(), SigningKey::from_bytes(&[(index + 71) as u8; 32])))
        .collect();
    let mut envelopes = Vec::new();
    for sender in &nodes {
        let request = VoteRequest {
            term: 1,
            candidate_id: sender.clone(),
            last_log_index: 0,
            last_log_term: 0,
        };
        let envelope = AuthenticatedConsensusEnvelope::sign(
            sender,
            1,
            &format!("partition-{sender}"),
            ConsensusMessage::VoteRequest(request),
            keys.get(sender).unwrap(),
        )
        .unwrap();
        envelopes.push((sender.clone(), envelope));
    }
    let mut durations = Vec::with_capacity(SAMPLES);
    let mut verified = 0usize;
    let attempted = SAMPLES * nodes.len();
    for sample in 0..SAMPLES {
        for (sender, envelope) in &envelopes {
            let target_connected = connected.contains(sender) && connected.contains(&nodes[sample % nodes.len()]);
            let started = Instant::now();
            if target_connected {
                envelope
                    .verify(sender, &keys.get(sender).unwrap().verifying_key().to_bytes())
                    .unwrap();
                verified += 1;
            }
            durations.push(started.elapsed().as_nanos() as f64 / 1_000.0);
        }
    }
    durations.sort_by(f64::total_cmp);
    let p95_index = ((durations.len() * 95).div_ceil(100)).saturating_sub(1);
    let elapsed_us: f64 = durations.iter().sum();
    Scenario {
        name: name.to_string(),
        cluster_size,
        connected_members,
        messages_attempted: attempted,
        messages_verified: verified,
        messages_dropped: attempted - verified,
        quorum_available: connected_members >= cluster_size / 2 + 1,
        verification_p95_us: durations[p95_index],
        verification_throughput_ops_per_sec: if elapsed_us > 0.0 {
            verified as f64 / (elapsed_us / 1_000_000.0)
        } else {
            0.0
        },
    }
}
