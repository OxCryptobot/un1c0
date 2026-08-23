use std::fs;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use un1c0::emission_diagnostic_service_identity::{
    DurableServiceIdentityOutbox, ServiceIdentityAuthority, ServiceIdentityDescriptor,
    ServiceIdentityEnvelope, ServiceIdentityError, ServiceIdentityRegistry,
};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn authority() -> ServiceIdentityAuthority {
    let first = signing_key(11);
    let identity = ServiceIdentityDescriptor::new("un1c0.local", "agents", "diagnostic").unwrap();
    let mut registry = ServiceIdentityRegistry::new("svc-diagnostic", identity).unwrap();
    registry
        .register_initial_signer("service-signer-a", first.verifying_key().to_bytes(), 1)
        .unwrap();
    ServiceIdentityAuthority::new(registry, "service-signer-a", first, 1).unwrap()
}

fn issue(authority: &ServiceIdentityAuthority, sequence: u64) -> ServiceIdentityEnvelope {
    issue_with_predecessor(authority, sequence, None)
}

fn issue_with_predecessor(
    authority: &ServiceIdentityAuthority,
    sequence: u64,
    predecessor: Option<[u8; 32]>,
) -> ServiceIdentityEnvelope {
    authority
        .issue(
            [sequence as u8; 32],
            "diagnostic-stream",
            sequence,
            predecessor,
        )
        .unwrap()
}

#[test]
fn service_identity_is_separate_from_content_attestation_and_exactly_bound() {
    let authority = authority();
    let envelope = issue(&authority, 1);
    envelope.verify(authority.registry()).unwrap();

    let mut wrong_service = envelope.clone();
    wrong_service.service_id = "other-service".into();
    assert!(matches!(
        wrong_service.verify(authority.registry()),
        Err(ServiceIdentityError::ServiceMismatch)
    ));

    let mut wrong_stream = envelope.clone();
    wrong_stream.stream_id = "other-stream".into();
    assert!(matches!(
        wrong_stream.verify(authority.registry()),
        Err(ServiceIdentityError::InvalidSignature)
    ));

    let mut wrong_evidence = envelope.clone();
    wrong_evidence.evidence_digest = [9; 32];
    assert!(matches!(
        wrong_evidence.verify(authority.registry()),
        Err(ServiceIdentityError::InvalidSignature)
    ));

    let mut wrong_generation = envelope.clone();
    wrong_generation.trust_generation = 2;
    assert!(matches!(
        wrong_generation.verify(authority.registry()),
        Err(ServiceIdentityError::InvalidSignature)
    ));
}

#[test]
fn signer_rotation_and_revocation_are_generation_bound_and_persisted() {
    let directory = tempdir().unwrap();
    let registry_path = directory.path().join("service-registry.json");
    let first = signing_key(11);
    let second = signing_key(22);
    let mut authority = authority();
    let historical = issue(&authority, 1);

    authority
        .rotate_signer("service-signer-b", second.clone(), 2, &registry_path)
        .unwrap();
    assert_eq!(authority.signer_id(), "service-signer-b");
    assert_eq!(
        authority.registry().active_signer_id(),
        Some("service-signer-b")
    );
    assert!(
        authority
            .registry()
            .signer("service-signer-a")
            .unwrap()
            .revoked
    );
    historical.verify(authority.registry()).unwrap();

    let current = issue(&authority, 2);
    current.verify(authority.registry()).unwrap();
    let persisted = ServiceIdentityRegistry::load(&registry_path).unwrap();
    assert_eq!(persisted.active_signer_id(), Some("service-signer-b"));
    assert!(persisted.signer("service-signer-a").unwrap().revoked);

    authority
        .revoke_signer("service-signer-b", &registry_path)
        .unwrap();
    assert!(
        authority
            .registry()
            .signer("service-signer-b")
            .unwrap()
            .revoked
    );
    assert_eq!(authority.registry().active_signer_id(), None);
    assert!(matches!(
        authority.issue([3; 32], "diagnostic-stream", 3, None),
        Err(ServiceIdentityError::RevokedSigner(_))
    ));

    let mut rebound = ServiceIdentityRegistry::load(&registry_path).unwrap();
    assert!(rebound.signer("service-signer-a").unwrap().revoked);
    assert!(rebound.signer("service-signer-b").unwrap().revoked);
    assert!(matches!(
        rebound.register_initial_signer("service-signer-a", first.verifying_key().to_bytes(), 3,),
        Err(ServiceIdentityError::SignerRebinding(_))
    ));
}

#[test]
fn durable_outbox_survives_restart_is_idempotent_and_acknowledges_atomically() {
    let directory = tempdir().unwrap();
    let outbox_path = directory.path().join("outbox");
    let authority = authority();
    let envelope = issue(&authority, 1);
    let outbox = DurableServiceIdentityOutbox::open(&outbox_path, 2).unwrap();

    assert!(outbox.enqueue(&envelope, authority.registry()).unwrap());
    assert!(!outbox.enqueue(&envelope, authority.registry()).unwrap());
    assert_eq!(outbox.pending(authority.registry()).unwrap().len(), 1);

    fs::write(outbox_path.join("crash.tmp"), b"incomplete-write").unwrap();
    drop(outbox);

    let reopened = DurableServiceIdentityOutbox::open(&outbox_path, 2).unwrap();
    let pending = reopened.pending(authority.registry()).unwrap();
    assert_eq!(pending, vec![envelope.clone()]);
    assert_eq!(
        reopened.acknowledge(&envelope, authority.registry()),
        Ok(())
    );
    assert!(reopened.pending(authority.registry()).unwrap().is_empty());
    drop(reopened);

    let recovered = DurableServiceIdentityOutbox::open(&outbox_path, 2).unwrap();
    assert!(recovered.pending(authority.registry()).unwrap().is_empty());
    assert!(outbox_path.join("crash.tmp").exists());
    fs::write(outbox_path.join("corrupt.json"), b"{not-json").unwrap();
    assert!(matches!(
        recovered.pending(authority.registry()),
        Err(ServiceIdentityError::Serialization(_))
    ));
    fs::remove_file(outbox_path.join("corrupt.json")).unwrap();

    let next = issue_with_predecessor(&authority, 2, Some(envelope.envelope_digest().unwrap()));
    assert!(recovered.enqueue(&next, authority.registry()).unwrap());
    let full_outbox = DurableServiceIdentityOutbox::open(directory.path().join("full"), 1).unwrap();
    assert!(full_outbox
        .enqueue(&envelope, authority.registry())
        .unwrap());
    assert!(matches!(
        full_outbox.enqueue(&next, authority.registry()),
        Err(ServiceIdentityError::OutboxFull {
            entries: 1,
            maximum: 1
        })
    ));
    let mut tampered = next.clone();
    tampered.stream_id = "tampered".into();
    assert!(matches!(
        recovered.enqueue(&tampered, authority.registry()),
        Err(ServiceIdentityError::InvalidSignature)
    ));
}

#[test]
fn durable_outbox_capacity_and_revoked_active_signer_fail_closed() {
    let directory = tempdir().unwrap();
    assert!(matches!(
        DurableServiceIdentityOutbox::open(directory.path(), 0),
        Err(ServiceIdentityError::InvalidCapacity)
    ));

    let mut authority = authority();
    let envelope = issue(&authority, 1);
    authority
        .revoke_signer("service-signer-a", directory.path().join("registry.json"))
        .unwrap();
    assert!(matches!(
        DurableServiceIdentityOutbox::open(directory.path().join("outbox"), 1)
            .unwrap()
            .enqueue(&envelope, authority.registry()),
        Ok(true) | Ok(false)
    ));
    assert!(matches!(
        authority.issue([4; 32], "diagnostic-stream", 4, None),
        Err(ServiceIdentityError::RevokedSigner(_))
    ));
}
