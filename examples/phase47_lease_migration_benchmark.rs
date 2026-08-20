use ed25519_dalek::SigningKey;
use serde_json::json;
use std::time::Instant;
use tempfile::tempdir;
use un1c0::lease_migration::{
    LeaseMigrationActivation, LeaseMigrationAuthority, LeaseMigrationRelease, LeaseMigrationState,
    LeaseMigrationWitnessAck, LeaseRecord,
};

const ROUNDS: u64 = 128;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn main() {
    let directory = tempdir().expect("temporary benchmark directory");
    let source = key(91);
    let destination = key(92);
    let witnesses = [key(101), key(102), key(103)];
    let mut authority = LeaseMigrationAuthority::new(
        directory.path().join("migration.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
    )
    .expect("authority");
    authority
        .register_owner("owner-a", &source.verifying_key())
        .expect("source owner");
    authority
        .register_owner("owner-b", &destination.verifying_key())
        .expect("destination owner");
    for (index, witness) in witnesses.iter().enumerate() {
        authority
            .register_witness(&format!("witness-{index}"), &witness.verifying_key())
            .expect("witness");
    }
    authority
        .initialize(
            LeaseRecord::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                "region-a",
                "owner-a",
                "process-a",
                1,
                200,
                3,
                &hash('a'),
            )
            .expect("initial lease"),
        )
        .expect("initialize");

    let started = Instant::now();
    let mut current_tick = 10_u64;
    let mut witness_acknowledgements = 0_u64;
    for round in 0..ROUNDS {
        let lease = authority.current_lease().expect("current lease");
        let (
            source_key,
            destination_key,
            destination_region,
            destination_owner,
            destination_process,
        ) = if lease.owner_id == "owner-a" {
            (&source, &destination, "region-b", "owner-b", "process-b")
        } else {
            (&destination, &source, "region-a", "owner-a", "process-a")
        };
        let intent = un1c0::lease_migration::LeaseMigrationIntent::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &lease.region_id,
            &lease.owner_id,
            &lease.process_instance,
            destination_region,
            destination_owner,
            destination_process,
            lease.ownership_epoch,
            &lease.record_hash,
            lease.generation,
            &lease.content_hash,
            &format!("phase47-round-{round}"),
            lease.ownership_epoch + 1,
            current_tick + 100,
            source_key,
        )
        .expect("intent");
        authority
            .begin(intent.clone(), current_tick)
            .expect("begin");
        for (index, witness) in witnesses.iter().take(2).enumerate() {
            let ack = LeaseMigrationWitnessAck::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                &intent.intent_hash,
                &format!("witness-{index}"),
                1,
                current_tick + 1,
                20,
                witness,
            )
            .expect("ack");
            authority
                .accept_witness_ack(ack, current_tick + 1)
                .expect("ack admission");
            witness_acknowledgements += 1;
        }
        authority.prepare(current_tick + 2).expect("prepare");
        let release = LeaseMigrationRelease::sign(
            &intent,
            &lease.owner_id,
            &lease.process_instance,
            lease.ownership_epoch,
            &lease.record_hash,
            current_tick + 3,
            source_key,
        )
        .expect("release");
        authority
            .release_source(release.clone(), current_tick + 3)
            .expect("release admission");
        let destination_lease = LeaseRecord::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            destination_region,
            destination_owner,
            destination_process,
            lease.ownership_epoch + 1,
            current_tick + 200,
            lease.generation,
            &intent.content_hash,
        )
        .expect("destination lease");
        let activation = LeaseMigrationActivation::sign(
            &intent,
            &release,
            destination_region,
            destination_owner,
            destination_process,
            lease.ownership_epoch + 1,
            current_tick + 200,
            lease.generation,
            &intent.content_hash,
            &destination_lease.record_hash,
            current_tick + 4,
            destination_key,
        )
        .expect("activation");
        authority
            .activate_destination(activation, current_tick + 4)
            .expect("activation admission");
        current_tick += 10;
    }
    let wall_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let metrics = authority.metrics();
    println!(
        "{}",
        json!({
            "phase": 47,
            "benchmark": "repeated distributed lease migration handoffs",
            "rounds": ROUNDS,
            "rounds_completed": ROUNDS,
            "witness_acknowledgements": witness_acknowledgements,
            "wall_us": wall_us,
            "throughput_rounds_per_sec": ROUNDS as f64 / (wall_us / 1_000_000.0),
            "final_state": match metrics.state { LeaseMigrationState::Activated => "Activated", _ => "Unexpected" },
            "final_epoch": metrics.last_activation_epoch,
            "secret_material_recorded": false,
            "cluster_mutation_performed": false,
        })
    );
}
