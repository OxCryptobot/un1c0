use ed25519_dalek::SigningKey;
use std::time::Instant;
use tempfile::tempdir;
use un1c0::ownership_bound_cas::OwnershipBoundCasCoordinator;
use un1c0::replicated_durability::{
    CasCommitOutcome, CasWriteRequest, ReplicaDurabilityAcknowledgement, ReplicaDurabilityMode,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hash(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn main() {
    let directory = tempdir().expect("temporary benchmark directory");
    let owner = key(91);
    let replicas = [key(92), key(93)];
    let mut coordinator = OwnershipBoundCasCoordinator::new(
        directory.path().join("ownership.json"),
        directory.path().join("cas.json"),
        "cluster-a",
        "resource-a",
        "snapshot-a",
        2,
        64,
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
            un1c0::cross_process_ownership::OwnershipClaim::sign(
                "cluster-a",
                "resource-a",
                "snapshot-a",
                "owner-a",
                "process-a",
                un1c0::cross_process_ownership::ZERO_HASH,
                1,
                10_000,
                0,
                &hash('0'),
                "fence-a",
                &owner,
            )
            .expect("initial claim"),
            1,
        )
        .expect("initial ownership");
    assert_eq!(record.ownership_epoch, 1);

    let started = Instant::now();
    let mut commits = 0usize;
    for cycle in 0..32u64 {
        let current = coordinator
            .current_owner()
            .expect("owner load")
            .expect("owner");
        let permit = coordinator
            .admit_write(
                "owner-a",
                "process-a",
                current.ownership_epoch,
                &current.record_hash,
                100 + cycle,
            )
            .expect("write permit");
        let hex_digit = b"0123456789abcdef"[(cycle as usize) % 16] as char;
        let proposed_hash = hash(hex_digit);
        let request = CasWriteRequest::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            "owner-a",
            current.ownership_epoch,
            &format!("nonce-{cycle}"),
            current.generation,
            &current.content_hash,
            current.generation + 1,
            &proposed_hash,
            &proposed_hash,
            &owner,
        )
        .expect("CAS request");
        let acknowledgements = [
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
                100 + cycle,
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
                100 + cycle,
                50,
                &replicas[1],
            )
            .expect("acknowledgement"),
        ];
        let receipt = coordinator
            .commit_owned(permit, request, &acknowledgements, 105 + cycle)
            .expect("ownership-bound commit");
        assert!(matches!(receipt.outcome, CasCommitOutcome::Committed(_)));
        commits += 1;
    }
    let elapsed_us = started.elapsed().as_secs_f64() * 1_000_000.0;
    let final_owner = coordinator
        .current_owner()
        .expect("final owner load")
        .expect("final owner");
    assert_eq!(final_owner.generation, 32);
    assert_eq!(coordinator.cas_state().generation, 32);
    println!(
        "{{\"phase\":43,\"ownership_bound_commits\":{},\"final_generation\":{},\"ownership_epoch\":{},\"quorum\":2,\"elapsed_us\":{:.3},\"secret_material_recorded\":false,\"cluster_mutation_performed\":false,\"production_boundary\":\"local ownership lock and replicated CAS adapter contract only\"}}",
        commits, final_owner.generation, final_owner.ownership_epoch, elapsed_us
    );
}
