use ed25519_dalek::SigningKey;
use serde_json::json;
use tempfile::tempdir;
use un1c0::{
    AuditLog, AuditSignerStore, MeshIdentity, MeshPolicy, MeshRequest, SecurityError,
    ZeroTrustMesh,
};

fn identities() -> (MeshIdentity, MeshIdentity) {
    (
        MeshIdentity::new("cluster.local", "agent", "runtime").unwrap(),
        MeshIdentity::new("cluster.local", "control", "admin").unwrap(),
    )
}

fn audit(path: &std::path::Path) -> (AuditLog, SigningKey) {
    let signing_key = SigningKey::from_bytes(&[17u8; 32]);
    let mut trusted = AuditSignerStore::default();
    trusted
        .trust_public_key("operator:security", &signing_key.verifying_key().to_bytes())
        .unwrap();
    (
        AuditLog::open_with_signer(path, "operator:security", signing_key.clone(), trusted)
            .unwrap(),
        signing_key,
    )
}

fn request(source: MeshIdentity, destination: MeshIdentity, fingerprint: &str) -> MeshRequest {
    MeshRequest {
        request_id: "security-request-1".into(),
        audience: destination.spiffe_id(),
        source,
        destination,
        method: "consensus.append".into(),
        peer_certificate_sha256: fingerprint.into(),
    }
}

#[test]
fn public_security_api_requires_identity_certificate_peer_and_method_policy() {
    let (source, destination) = identities();
    let fingerprint = "c".repeat(64);
    let policy = MeshPolicy::new("cluster.local")
        .unwrap()
        .allow_peer(&source, &destination)
        .unwrap()
        .allow_method(&destination, "consensus.append")
        .unwrap()
        .trust_certificate(&source, &fingerprint)
        .unwrap();
    let mesh = ZeroTrustMesh::new(policy);
    let directory = tempdir().unwrap();
    let (audit_log, _) = audit(&directory.path().join("audit.jsonl"));

    let allowed = mesh
        .authorize_and_audit(
            &request(source.clone(), destination.clone(), &fingerprint),
            &audit_log,
        )
        .unwrap();
    assert!(allowed.allowed);

    let mut denied = request(source, destination, &fingerprint);
    denied.method = "consensus.unapproved".into();
    let decision = mesh.authorize_and_audit(&denied, &audit_log).unwrap();
    assert!(!decision.allowed);
    assert_eq!(std::fs::read_to_string(audit_log.path()).unwrap().lines().count(), 2);
}

#[test]
fn public_audit_api_rejects_tampered_chain() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let (audit_log, signing_key) = audit(&path);
    audit_log
        .append(
            "mesh_authorization",
            "spiffe://cluster.local/ns/agent/sa/runtime",
            "spiffe://cluster.local/ns/control/sa/admin",
            "allow",
            &json!({"request_id":"security-request-1"}),
        )
        .unwrap();
    let mut trusted = AuditSignerStore::default();
    trusted
        .trust_public_key("operator:security", &signing_key.verifying_key().to_bytes())
        .unwrap();
    let mut content = std::fs::read_to_string(&path).unwrap();
    content = content.replacen("mesh_authorization", "mesh_tampered", 1);
    std::fs::write(&path, content).unwrap();
    assert!(matches!(
        AuditLog::open_with_signer(&path, "operator:security", signing_key, trusted),
        Err(SecurityError::ChainInvalid(_)) | Err(SecurityError::InvalidSignature)
    ));
}
