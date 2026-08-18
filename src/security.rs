//! Zero-trust service-mesh authorization and cryptographic audit logging.
//!
//! The mesh layer is transport-agnostic: an authenticated mTLS sidecar or
//! gateway supplies a peer identity and certificate fingerprint, while this
//! module decides whether the request is authorized. The audit layer records
//! only bounded metadata digests and signs a tamper-evident hash chain.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MAX_AUDIT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_AUDIT_EVENTS: usize = 100_000;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REMOTE_AUDIT_BYTES: usize = 16 * 1024 * 1024;
const MAX_REMOTE_AUDIT_TOKEN_BYTES: usize = 256;
const MAX_REMOTE_AUDIT_PENDING: usize = 100_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SecurityError {
    #[error("invalid zero-trust identity: {0}")]
    InvalidIdentity(String),
    #[error("invalid zero-trust policy: {0}")]
    InvalidPolicy(String),
    #[error("invalid mesh request: {0}")]
    InvalidRequest(String),
    #[error("mesh request denied: {0}")]
    AccessDenied(String),
    #[error("audit signer is not trusted: {0}")]
    UntrustedSigner(String),
    #[error("audit signer is revoked: {0}")]
    SignerRevoked(String),
    #[error("durable audit sink persistence failed: {0}")]
    SinkPersistence(String),
    #[error("audit signature verification failed")]
    InvalidSignature,
    #[error("audit chain verification failed: {0}")]
    ChainInvalid(String),
    #[error("audit persistence failed: {0}")]
    Persistence(String),
    #[error("audit serialization failed: {0}")]
    Serialization(String),
    #[error("audit metadata exceeds the configured bound")]
    MetadataTooLarge,
    #[error("remote audit ordering gap: {0}")]
    RemoteAuditGap(String),
    #[error("remote audit envelope rejected: {0}")]
    RemoteAuditRejected(String),
    #[error("remote audit envelope collision: {0}")]
    RemoteAuditCollision(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct MeshIdentity {
    pub trust_domain: String,
    pub namespace: String,
    pub service_account: String,
}

impl MeshIdentity {
    pub fn new(
        trust_domain: &str,
        namespace: &str,
        service_account: &str,
    ) -> Result<Self, SecurityError> {
        for (label, value) in [
            ("trust domain", trust_domain),
            ("namespace", namespace),
            ("service account", service_account),
        ] {
            validate_segment(value, label)?;
        }
        Ok(Self {
            trust_domain: trust_domain.to_string(),
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
        })
    }

    pub fn spiffe_id(&self) -> String {
        format!(
            "spiffe://{}/ns/{}/sa/{}",
            self.trust_domain, self.namespace, self.service_account
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPolicy {
    pub trust_domain: String,
    pub allowed_peers: BTreeMap<String, BTreeSet<String>>,
    pub allowed_methods: BTreeMap<String, BTreeSet<String>>,
    pub trusted_certificates: BTreeMap<String, BTreeSet<String>>,
}

impl MeshPolicy {
    pub fn new(trust_domain: &str) -> Result<Self, SecurityError> {
        validate_segment(trust_domain, "trust domain")?;
        Ok(Self {
            trust_domain: trust_domain.to_string(),
            allowed_peers: BTreeMap::new(),
            allowed_methods: BTreeMap::new(),
            trusted_certificates: BTreeMap::new(),
        })
    }

    pub fn allow_peer(
        mut self,
        source: &MeshIdentity,
        destination: &MeshIdentity,
    ) -> Result<Self, SecurityError> {
        self.validate_identity(source)?;
        self.validate_identity(destination)?;
        self.allowed_peers
            .entry(source.spiffe_id())
            .or_default()
            .insert(destination.spiffe_id());
        Ok(self)
    }

    pub fn allow_method(
        mut self,
        destination: &MeshIdentity,
        method: &str,
    ) -> Result<Self, SecurityError> {
        self.validate_identity(destination)?;
        validate_identifier(method, "mesh method")?;
        self.allowed_methods
            .entry(destination.spiffe_id())
            .or_default()
            .insert(method.to_string());
        Ok(self)
    }

    pub fn trust_certificate(
        mut self,
        identity: &MeshIdentity,
        fingerprint: &str,
    ) -> Result<Self, SecurityError> {
        self.validate_identity(identity)?;
        validate_fingerprint(fingerprint)?;
        self.trusted_certificates
            .entry(identity.spiffe_id())
            .or_default()
            .insert(fingerprint.to_ascii_lowercase());
        Ok(self)
    }

    fn validate_identity(&self, identity: &MeshIdentity) -> Result<(), SecurityError> {
        if identity.trust_domain != self.trust_domain {
            return Err(SecurityError::InvalidPolicy(format!(
                "identity '{}' is outside trust domain '{}'",
                identity.spiffe_id(),
                self.trust_domain
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshRequest {
    pub request_id: String,
    pub source: MeshIdentity,
    pub destination: MeshIdentity,
    pub audience: String,
    pub method: String,
    pub peer_certificate_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshDecision {
    pub allowed: bool,
    pub reason: String,
    pub source: String,
    pub destination: String,
    pub method: String,
}

#[derive(Debug, Clone)]
pub struct ZeroTrustMesh {
    policy: MeshPolicy,
}

impl ZeroTrustMesh {
    pub fn new(policy: MeshPolicy) -> Self {
        Self { policy }
    }

    pub fn authorize(&self, request: &MeshRequest) -> Result<MeshDecision, SecurityError> {
        validate_identifier(&request.request_id, "mesh request id")?;
        validate_identifier(&request.audience, "mesh audience")?;
        validate_identifier(&request.method, "mesh method")?;
        validate_fingerprint(&request.peer_certificate_sha256)?;
        self.policy.validate_identity(&request.source)?;
        self.policy.validate_identity(&request.destination)?;
        let source = request.source.spiffe_id();
        let destination = request.destination.spiffe_id();
        let denied = |reason: &str| MeshDecision {
            allowed: false,
            reason: reason.to_string(),
            source: source.clone(),
            destination: destination.clone(),
            method: request.method.clone(),
        };
        if request.audience != destination {
            return Ok(denied("audience does not match destination identity"));
        }
        if !self
            .policy
            .trusted_certificates
            .get(&source)
            .is_some_and(|fingerprints| {
                fingerprints.contains(&request.peer_certificate_sha256.to_ascii_lowercase())
            })
        {
            return Ok(denied("peer certificate fingerprint is not trusted"));
        }
        if !self
            .policy
            .allowed_peers
            .get(&source)
            .is_some_and(|destinations| destinations.contains(&destination))
        {
            return Ok(denied("source-to-destination peer relation is not allowed"));
        }
        if !self
            .policy
            .allowed_methods
            .get(&destination)
            .is_some_and(|methods| methods.contains(&request.method))
        {
            return Ok(denied("method is not allowed for destination"));
        }
        Ok(MeshDecision {
            allowed: true,
            reason: "mTLS identity, audience, peer, certificate, and method policy verified".into(),
            source,
            destination,
            method: request.method.clone(),
        })
    }

    pub fn authorize_and_audit(
        &self,
        request: &MeshRequest,
        audit: &AuditLog,
    ) -> Result<MeshDecision, SecurityError> {
        let decision = self.authorize(request)?;
        let metadata = serde_json::json!({
            "request_id": request.request_id,
            "audience": request.audience,
            "peer_certificate_sha256": request.peer_certificate_sha256,
            "decision_reason": decision.reason,
        });
        audit.append(
            "mesh_authorization",
            &decision.source,
            &decision.destination,
            if decision.allowed { "allow" } else { "deny" },
            &metadata,
        )?;
        Ok(decision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditSignerState {
    pub public_key: [u8; 32],
    pub revoked: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditSignerStore {
    keys: BTreeMap<String, AuditSignerState>,
}

impl AuditSignerStore {
    pub fn trust_public_key(
        &mut self,
        signer_id: &str,
        public_key: &[u8],
    ) -> Result<(), SecurityError> {
        validate_identifier(signer_id, "audit signer id")?;
        let key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        if let Some(existing) = self.keys.get(signer_id) {
            if existing.public_key != key {
                return Err(SecurityError::UntrustedSigner(format!(
                    "signer '{}' cannot be rebound",
                    signer_id
                )));
            }
            if existing.revoked {
                return Err(SecurityError::SignerRevoked(signer_id.to_string()));
            }
        } else {
            self.keys.insert(
                signer_id.to_string(),
                AuditSignerState {
                    public_key: key,
                    revoked: false,
                },
            );
        }
        Ok(())
    }

    pub fn revoke(&mut self, signer_id: &str) -> Result<(), SecurityError> {
        validate_identifier(signer_id, "audit signer id")?;
        let signer = self
            .keys
            .get_mut(signer_id)
            .ok_or_else(|| SecurityError::UntrustedSigner(signer_id.to_string()))?;
        signer.revoked = true;
        Ok(())
    }

    pub fn rotate(
        &mut self,
        old_signer_id: &str,
        new_signer_id: &str,
        new_public_key: &[u8],
    ) -> Result<(), SecurityError> {
        validate_identifier(old_signer_id, "old audit signer id")?;
        validate_identifier(new_signer_id, "new audit signer id")?;
        if old_signer_id == new_signer_id {
            return Err(SecurityError::UntrustedSigner(
                "signer rotation requires distinct signer IDs".into(),
            ));
        }
        let new_key: [u8; 32] = new_public_key
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        let old = self
            .keys
            .get(old_signer_id)
            .ok_or_else(|| SecurityError::UntrustedSigner(old_signer_id.to_string()))?;
        if old.revoked {
            return Err(SecurityError::SignerRevoked(old_signer_id.to_string()));
        }
        if self.keys.contains_key(new_signer_id) {
            return Err(SecurityError::UntrustedSigner(format!(
                "signer '{}' already exists",
                new_signer_id
            )));
        }
        self.keys
            .get_mut(old_signer_id)
            .expect("checked above")
            .revoked = true;
        self.keys.insert(
            new_signer_id.to_string(),
            AuditSignerState {
                public_key: new_key,
                revoked: false,
            },
        );
        Ok(())
    }

    pub fn is_revoked(&self, signer_id: &str) -> bool {
        self.keys
            .get(signer_id)
            .is_some_and(|signer| signer.revoked)
    }

    fn authorize_historical(
        &self,
        signer_id: &str,
        public_key: &[u8],
    ) -> Result<(), SecurityError> {
        let trusted = self
            .keys
            .get(signer_id)
            .ok_or_else(|| SecurityError::UntrustedSigner(signer_id.to_string()))?;
        if trusted.public_key.as_slice() != public_key {
            return Err(SecurityError::UntrustedSigner(format!(
                "public key mismatch for '{}'",
                signer_id
            )));
        }
        Ok(())
    }

    fn authorize_active(&self, signer_id: &str, public_key: &[u8]) -> Result<(), SecurityError> {
        self.authorize_historical(signer_id, public_key)?;
        if self.is_revoked(signer_id) {
            return Err(SecurityError::SignerRevoked(signer_id.to_string()));
        }
        Ok(())
    }

    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), SecurityError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        let temporary = parent.join(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("signers")
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| SecurityError::Persistence(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, SecurityError> {
        let bytes =
            fs::read(path).map_err(|error| SecurityError::Persistence(error.to_string()))?;
        let store: Self = serde_json::from_slice(&bytes)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        for (signer_id, signer) in &store.keys {
            validate_identifier(signer_id, "audit signer id")?;
            if signer.public_key.len() != 32 {
                return Err(SecurityError::InvalidSignature);
            }
        }
        Ok(store)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditRecord {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub event_type: String,
    pub actor: String,
    pub resource: String,
    pub outcome: String,
    pub metadata_sha256: String,
    pub previous_hash: String,
    pub record_hash: String,
    pub signer_id: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl AuditRecord {
    fn signing_payload(&self) -> Result<Vec<u8>, SecurityError> {
        serde_json::to_vec(&(
            self.sequence,
            self.timestamp_ms,
            &self.event_type,
            &self.actor,
            &self.resource,
            &self.outcome,
            &self.metadata_sha256,
            &self.previous_hash,
            &self.signer_id,
            &self.public_key,
        ))
        .map_err(|error| SecurityError::Serialization(error.to_string()))
    }

    pub fn verify(&self, trusted: &AuditSignerStore) -> Result<(), SecurityError> {
        validate_identifier(&self.signer_id, "audit signer id")?;
        validate_identifier(&self.event_type, "audit event type")?;
        validate_identifier(&self.actor, "audit actor")?;
        validate_identifier(&self.resource, "audit resource")?;
        validate_identifier(&self.outcome, "audit outcome")?;
        if !is_hex_digest(&self.metadata_sha256)
            || (!self.previous_hash.is_empty() && !is_hex_digest(&self.previous_hash))
            || !is_hex_digest(&self.record_hash)
        {
            return Err(SecurityError::ChainInvalid(
                "audit record contains a malformed digest".into(),
            ));
        }
        trusted.authorize_historical(&self.signer_id, &self.public_key)?;
        let payload = self.signing_payload()?;
        let expected_hash = hex_digest(&payload);
        if expected_hash != self.record_hash {
            return Err(SecurityError::ChainInvalid(format!(
                "record hash mismatch at sequence {}",
                self.sequence
            )));
        }
        let public_key: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        let verifying_key =
            VerifyingKey::from_bytes(&public_key).map_err(|_| SecurityError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        verifying_key
            .verify(&payload, &signature)
            .map_err(|_| SecurityError::InvalidSignature)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAuditEnvelope {
    pub cluster_id: String,
    pub source_node_id: String,
    pub stream_id: String,
    pub source_sequence: u64,
    pub previous_hash: String,
    pub record_hash: String,
    pub signer_id: String,
    pub record_bytes: Vec<u8>,
    pub envelope_hash: String,
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

impl RemoteAuditEnvelope {
    pub fn from_record(
        cluster_id: &str,
        source_node_id: &str,
        stream_id: &str,
        record: &AuditRecord,
        signing_key: &SigningKey,
    ) -> Result<Self, SecurityError> {
        validate_identifier(cluster_id, "remote audit cluster id")?;
        validate_identifier(source_node_id, "remote audit source node id")?;
        validate_identifier(stream_id, "remote audit stream id")?;
        if record.sequence == 0 || record.record_hash.is_empty() {
            return Err(SecurityError::RemoteAuditRejected(
                "record must have a positive sequence and hash".into(),
            ));
        }
        let record_bytes = serde_json::to_vec(record)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        if record_bytes.len() > MAX_REMOTE_AUDIT_BYTES {
            return Err(SecurityError::RemoteAuditRejected(
                "record bytes exceed remote audit bound".into(),
            ));
        }
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut envelope = Self {
            cluster_id: cluster_id.to_string(),
            source_node_id: source_node_id.to_string(),
            stream_id: stream_id.to_string(),
            source_sequence: record.sequence,
            previous_hash: record.previous_hash.clone(),
            record_hash: record.record_hash.clone(),
            signer_id: record.signer_id.clone(),
            record_bytes,
            envelope_hash: String::new(),
            signature: Vec::new(),
            public_key,
        };
        let payload = envelope.signing_payload()?;
        envelope.envelope_hash = hex_digest(&payload);
        envelope.signature = signing_key.sign(&payload).to_bytes().to_vec();
        Ok(envelope)
    }

    fn signing_payload(&self) -> Result<Vec<u8>, SecurityError> {
        serde_json::to_vec(&(
            &self.cluster_id,
            &self.source_node_id,
            &self.stream_id,
            self.source_sequence,
            &self.previous_hash,
            &self.record_hash,
            &self.signer_id,
            &self.record_bytes,
            &self.public_key,
        ))
        .map_err(|error| SecurityError::Serialization(error.to_string()))
    }

    pub fn validate(
        &self,
        cluster_id: &str,
        trusted_signers: &AuditSignerStore,
    ) -> Result<(), SecurityError> {
        validate_identifier(cluster_id, "remote audit cluster id")?;
        if self.cluster_id != cluster_id {
            return Err(SecurityError::RemoteAuditRejected(
                "remote audit cluster mismatch".into(),
            ));
        }
        validate_identifier(&self.source_node_id, "remote audit source node id")?;
        validate_identifier(&self.stream_id, "remote audit stream id")?;
        validate_identifier(&self.signer_id, "remote audit signer id")?;
        if self.source_sequence == 0
            || !is_hex_digest(&self.previous_hash) && !self.previous_hash.is_empty()
            || !is_hex_digest(&self.record_hash)
            || self.record_bytes.len() > MAX_REMOTE_AUDIT_BYTES
        {
            return Err(SecurityError::RemoteAuditRejected(
                "remote audit envelope fields are malformed".into(),
            ));
        }
        trusted_signers.authorize_historical(&self.signer_id, &self.public_key)?;
        let payload = self.signing_payload()?;
        if hex_digest(&payload) != self.envelope_hash {
            return Err(SecurityError::RemoteAuditRejected(
                "remote audit envelope hash mismatch".into(),
            ));
        }
        let public_key: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| SecurityError::InvalidSignature)?
            .verify(&payload, &Signature::from_bytes(&signature))
            .map_err(|_| SecurityError::InvalidSignature)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteAuditDecision {
    Accepted,
    AlreadyAccepted,
    AwaitingPredecessor,
    RetryableFailure,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAuditAcknowledgement {
    pub cluster_id: String,
    pub sink_id: String,
    pub envelope_hash: String,
    pub source_sequence: u64,
    pub decision: RemoteAuditDecision,
    pub next_expected_sequence: u64,
    pub order_token: Option<String>,
    pub acknowledgement_hash: String,
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

impl RemoteAuditAcknowledgement {
    pub fn new(
        cluster_id: &str,
        sink_id: &str,
        envelope: &RemoteAuditEnvelope,
        decision: RemoteAuditDecision,
        next_expected_sequence: u64,
        order_token: Option<&str>,
        signing_key: &SigningKey,
    ) -> Result<Self, SecurityError> {
        validate_identifier(cluster_id, "remote audit cluster id")?;
        validate_identifier(sink_id, "remote audit sink id")?;
        if let Some(token) = order_token {
            if token.is_empty()
                || token.len() > MAX_REMOTE_AUDIT_TOKEN_BYTES
                || token.chars().any(char::is_control)
            {
                return Err(SecurityError::RemoteAuditRejected(
                    "remote audit order token is out of bounds".into(),
                ));
            }
        }
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut acknowledgement = Self {
            cluster_id: cluster_id.to_string(),
            sink_id: sink_id.to_string(),
            envelope_hash: envelope.envelope_hash.clone(),
            source_sequence: envelope.source_sequence,
            decision,
            next_expected_sequence,
            order_token: order_token.map(str::to_string),
            acknowledgement_hash: String::new(),
            signature: Vec::new(),
            public_key,
        };
        let payload = acknowledgement.signing_payload()?;
        acknowledgement.acknowledgement_hash = hex_digest(&payload);
        acknowledgement.signature = signing_key.sign(&payload).to_bytes().to_vec();
        Ok(acknowledgement)
    }

    fn signing_payload(&self) -> Result<Vec<u8>, SecurityError> {
        serde_json::to_vec(&(
            &self.cluster_id,
            &self.sink_id,
            &self.envelope_hash,
            self.source_sequence,
            &self.decision,
            self.next_expected_sequence,
            &self.order_token,
            &self.public_key,
        ))
        .map_err(|error| SecurityError::Serialization(error.to_string()))
    }

    pub fn validate(
        &self,
        cluster_id: &str,
        trusted_sink: &AuditSignerStore,
        envelope: &RemoteAuditEnvelope,
    ) -> Result<(), SecurityError> {
        if self.cluster_id != cluster_id
            || self.envelope_hash != envelope.envelope_hash
            || self.source_sequence != envelope.source_sequence
        {
            return Err(SecurityError::RemoteAuditRejected(
                "remote acknowledgement does not bind to envelope".into(),
            ));
        }
        validate_identifier(&self.sink_id, "remote audit sink id")?;
        if self.next_expected_sequence == 0
            || self.order_token.as_ref().is_some_and(|token| {
                token.is_empty()
                    || token.len() > MAX_REMOTE_AUDIT_TOKEN_BYTES
                    || token.chars().any(char::is_control)
            })
        {
            return Err(SecurityError::RemoteAuditRejected(
                "remote acknowledgement fields are malformed".into(),
            ));
        }
        trusted_sink.authorize_historical(&self.sink_id, &self.public_key)?;
        let payload = self.signing_payload()?;
        if hex_digest(&payload) != self.acknowledgement_hash {
            return Err(SecurityError::RemoteAuditRejected(
                "remote acknowledgement hash mismatch".into(),
            ));
        }
        let public_key: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::InvalidSignature)?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| SecurityError::InvalidSignature)?
            .verify(&payload, &Signature::from_bytes(&signature))
            .map_err(|_| SecurityError::InvalidSignature)
    }
}

#[derive(Clone)]
pub struct DurableRemoteAuditSink {
    directory: PathBuf,
    cluster_id: String,
    trusted_signers: AuditSignerStore,
    trusted_sink_signers: AuditSignerStore,
}

impl DurableRemoteAuditSink {
    pub fn open(
        directory: impl AsRef<Path>,
        cluster_id: &str,
        trusted_signers: AuditSignerStore,
        trusted_sink_signers: AuditSignerStore,
    ) -> Result<Self, SecurityError> {
        validate_identifier(cluster_id, "remote audit cluster id")?;
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory)
            .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
        Ok(Self {
            directory,
            cluster_id: cluster_id.to_string(),
            trusted_signers,
            trusted_sink_signers,
        })
    }

    pub fn enqueue(&self, envelope: &RemoteAuditEnvelope) -> Result<(), SecurityError> {
        envelope.validate(&self.cluster_id, &self.trusted_signers)?;
        let pending_envelopes = self.pending()?;
        if pending_envelopes.len() >= MAX_REMOTE_AUDIT_PENDING {
            return Err(SecurityError::RemoteAuditRejected(
                "remote audit outbox exceeds the pending-entry bound".into(),
            ));
        }
        for pending in pending_envelopes {
            if pending.stream_id == envelope.stream_id
                && pending.source_sequence == envelope.source_sequence
                && pending.envelope_hash != envelope.envelope_hash
            {
                return Err(SecurityError::RemoteAuditCollision(
                    "same stream sequence has a different envelope hash".into(),
                ));
            }
        }
        let path = self
            .directory
            .join(format!("{}.json", envelope.envelope_hash));
        let bytes = serde_json::to_vec(envelope)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        if bytes.len() > MAX_REMOTE_AUDIT_BYTES {
            return Err(SecurityError::RemoteAuditRejected(
                "outbox entry exceeds remote audit bound".into(),
            ));
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read(&path)
                    .map_err(|read_error| SecurityError::SinkPersistence(read_error.to_string()))?;
                if existing == bytes {
                    Ok(())
                } else {
                    Err(SecurityError::RemoteAuditCollision(
                        "envelope hash collides with different bytes".into(),
                    ))
                }
            }
            Err(error) => Err(SecurityError::SinkPersistence(error.to_string())),
        }
    }

    pub fn pending(&self) -> Result<Vec<RemoteAuditEnvelope>, SecurityError> {
        let mut envelopes = Vec::new();
        for entry in fs::read_dir(&self.directory)
            .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?
        {
            let entry = entry.map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
            if !entry
                .file_type()
                .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?
                .is_file()
            {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let envelope: RemoteAuditEnvelope = serde_json::from_slice(
                &fs::read(&path)
                    .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?,
            )
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
            envelope.validate(&self.cluster_id, &self.trusted_signers)?;
            envelopes.push(envelope);
        }
        envelopes.sort_by(|left, right| {
            left.stream_id
                .cmp(&right.stream_id)
                .then(left.source_sequence.cmp(&right.source_sequence))
                .then(left.envelope_hash.cmp(&right.envelope_hash))
        });
        Ok(envelopes)
    }

    pub fn replay_pending(&self) -> Result<Vec<RemoteAuditEnvelope>, SecurityError> {
        self.pending()
    }

    pub fn acknowledge(
        &self,
        acknowledgement: &RemoteAuditAcknowledgement,
    ) -> Result<bool, SecurityError> {
        let envelope = self
            .pending()?
            .into_iter()
            .find(|envelope| envelope.envelope_hash == acknowledgement.envelope_hash)
            .ok_or_else(|| {
                SecurityError::RemoteAuditRejected("acknowledgement has no pending envelope".into())
            })?;
        acknowledgement.validate(&self.cluster_id, &self.trusted_sink_signers, &envelope)?;
        match acknowledgement.decision {
            RemoteAuditDecision::Accepted | RemoteAuditDecision::AlreadyAccepted => {
                let path = self
                    .directory
                    .join(format!("{}.json", envelope.envelope_hash));
                fs::remove_file(&path)
                    .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
                if let Ok(directory) = OpenOptions::new().read(true).open(&self.directory) {
                    directory
                        .sync_all()
                        .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
                }
                Ok(true)
            }
            RemoteAuditDecision::AwaitingPredecessor
            | RemoteAuditDecision::RetryableFailure
            | RemoteAuditDecision::Rejected => Ok(false),
        }
    }
}

#[derive(Debug, Clone)]
struct AuditState {
    next_sequence: u64,
    previous_hash: String,
}

pub trait AuditSink: Send + Sync {
    fn persist(&self, record: &AuditRecord) -> Result<(), SecurityError>;
}

#[derive(Clone)]
pub struct DurableFileAuditSink {
    directory: PathBuf,
}

impl DurableFileAuditSink {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, SecurityError> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory)
            .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn verify_chain(&self, trusted: &AuditSignerStore) -> Result<usize, SecurityError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.directory)
            .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?
        {
            let entry = entry.map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
            if entry
                .file_type()
                .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?
                .is_file()
            {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let record: AuditRecord = serde_json::from_slice(
                    &fs::read(&path)
                        .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?,
                )
                .map_err(|error| SecurityError::Serialization(error.to_string()))?;
                records.push(record);
            }
        }
        records.sort_by_key(|record| record.sequence);
        let mut previous_hash = String::new();
        for (expected_sequence, record) in records.iter().enumerate() {
            let expected_sequence = expected_sequence as u64 + 1;
            if record.sequence != expected_sequence || record.previous_hash != previous_hash {
                return Err(SecurityError::ChainInvalid(
                    "external audit sink sequence or previous hash mismatch".into(),
                ));
            }
            record.verify(trusted)?;
            previous_hash = record.record_hash.clone();
        }
        Ok(records.len())
    }
}

impl AuditSink for DurableFileAuditSink {
    fn persist(&self, record: &AuditRecord) -> Result<(), SecurityError> {
        let filename = format!("{:020}-{}.json", record.sequence, record.record_hash);
        let path = self.directory.join(filename);
        let bytes = serde_json::to_vec(record)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| SecurityError::SinkPersistence(error.to_string()))?;
                if let Some(parent) = self.directory.parent() {
                    if let Ok(directory_file) = OpenOptions::new().read(true).open(parent) {
                        let _ = directory_file.sync_all();
                    }
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read(&path)
                    .map_err(|read_error| SecurityError::SinkPersistence(read_error.to_string()))?;
                if existing == bytes {
                    Ok(())
                } else {
                    Err(SecurityError::SinkPersistence(
                        "external audit sink record collision".into(),
                    ))
                }
            }
            Err(error) => Err(SecurityError::SinkPersistence(error.to_string())),
        }
    }
}

#[derive(Clone)]
pub struct AuditLog {
    path: PathBuf,
    signer_id: Arc<Mutex<String>>,
    signing_key: Arc<Mutex<SigningKey>>,
    trusted_signers: Arc<Mutex<AuditSignerStore>>,
    sink: Option<Arc<dyn AuditSink>>,
    state: Arc<Mutex<AuditState>>,
}

impl AuditLog {
    pub fn open_with_signer(
        path: impl AsRef<Path>,
        signer_id: &str,
        signing_key: SigningKey,
        trusted_signers: AuditSignerStore,
    ) -> Result<Self, SecurityError> {
        Self::open_with_signer_and_sink(path, signer_id, signing_key, trusted_signers, None)
    }

    pub fn open_with_signer_and_sink(
        path: impl AsRef<Path>,
        signer_id: &str,
        signing_key: SigningKey,
        trusted_signers: AuditSignerStore,
        sink: Option<Arc<dyn AuditSink>>,
    ) -> Result<Self, SecurityError> {
        validate_identifier(signer_id, "audit signer id")?;
        let path = path.as_ref().to_path_buf();
        let public_key = signing_key.verifying_key().to_bytes();
        trusted_signers.authorize_active(signer_id, &public_key)?;
        let mut next_sequence = 1u64;
        let mut previous_hash = String::new();
        if path.exists() {
            let metadata = fs::metadata(&path)
                .map_err(|error| SecurityError::Persistence(error.to_string()))?;
            if metadata.len() > MAX_AUDIT_BYTES {
                return Err(SecurityError::ChainInvalid(
                    "audit log exceeds the 16 MiB bound".into(),
                ));
            }
            let file = fs::File::open(&path)
                .map_err(|error| SecurityError::Persistence(error.to_string()))?;
            for (line_number, line) in BufReader::new(file).lines().enumerate() {
                if line_number >= MAX_AUDIT_EVENTS {
                    return Err(SecurityError::ChainInvalid(
                        "audit log exceeds the event bound".into(),
                    ));
                }
                let line = line.map_err(|error| SecurityError::Persistence(error.to_string()))?;
                let record: AuditRecord = serde_json::from_str(&line)
                    .map_err(|error| SecurityError::Serialization(error.to_string()))?;
                if record.sequence != next_sequence {
                    return Err(SecurityError::ChainInvalid(format!(
                        "expected sequence {}, received {}",
                        next_sequence, record.sequence
                    )));
                }
                if record.previous_hash != previous_hash {
                    return Err(SecurityError::ChainInvalid(format!(
                        "previous hash mismatch at sequence {}",
                        record.sequence
                    )));
                }
                record.verify(&trusted_signers)?;
                previous_hash = record.record_hash;
                next_sequence = next_sequence
                    .checked_add(1)
                    .ok_or_else(|| SecurityError::ChainInvalid("sequence overflow".into()))?;
            }
        }
        Ok(Self {
            path,
            signer_id: Arc::new(Mutex::new(signer_id.to_string())),
            signing_key: Arc::new(Mutex::new(signing_key)),
            trusted_signers: Arc::new(Mutex::new(trusted_signers)),
            sink,
            state: Arc::new(Mutex::new(AuditState {
                next_sequence,
                previous_hash,
            })),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(
        &self,
        event_type: &str,
        actor: &str,
        resource: &str,
        outcome: &str,
        metadata: &Value,
    ) -> Result<AuditRecord, SecurityError> {
        validate_identifier(event_type, "audit event type")?;
        validate_identifier(actor, "audit actor")?;
        validate_identifier(resource, "audit resource")?;
        validate_identifier(outcome, "audit outcome")?;
        let metadata_bytes = serde_json::to_vec(metadata)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        if metadata_bytes.len() > MAX_METADATA_BYTES {
            return Err(SecurityError::MetadataTooLarge);
        }
        let metadata_sha256 = hex_digest(&metadata_bytes);
        let mut state = self
            .state
            .lock()
            .map_err(|_| SecurityError::Persistence("audit state lock poisoned".into()))?;
        let signer_id = self
            .signer_id
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer lock poisoned".into()))?
            .clone();
        let signing_key = self
            .signing_key
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signing-key lock poisoned".into()))?;
        let trusted_signers = self
            .trusted_signers
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer-store lock poisoned".into()))?;
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        trusted_signers.authorize_active(&signer_id, &public_key)?;
        let mut record = AuditRecord {
            sequence: state.next_sequence,
            timestamp_ms: now_ms(),
            event_type: event_type.to_string(),
            actor: actor.to_string(),
            resource: resource.to_string(),
            outcome: outcome.to_string(),
            metadata_sha256,
            previous_hash: state.previous_hash.clone(),
            record_hash: String::new(),
            signer_id,
            public_key,
            signature: Vec::new(),
        };
        let payload = record.signing_payload()?;
        record.record_hash = hex_digest(&payload);
        record.signature = signing_key.sign(&payload).to_bytes().to_vec();
        record.verify(&trusted_signers)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        }
        let line = serde_json::to_string(&record)
            .map_err(|error| SecurityError::Serialization(error.to_string()))?;
        if let Ok(metadata) = fs::metadata(&self.path) {
            if metadata.len().saturating_add(line.len() as u64 + 1) > MAX_AUDIT_BYTES {
                return Err(SecurityError::Persistence(
                    "audit log would exceed the 16 MiB bound".into(),
                ));
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        writeln!(file, "{}", line)
            .and_then(|_| file.sync_data())
            .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        state.previous_hash = record.record_hash.clone();
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| SecurityError::ChainInvalid("sequence overflow".into()))?;
        drop(trusted_signers);
        drop(signing_key);
        if let Some(sink) = &self.sink {
            sink.persist(&record)?;
        }
        Ok(record)
    }

    pub fn rotate_signer(
        &self,
        new_signer_id: &str,
        new_signing_key: SigningKey,
        registry_path: impl AsRef<Path>,
    ) -> Result<(), SecurityError> {
        let _state = self
            .state
            .lock()
            .map_err(|_| SecurityError::Persistence("audit state lock poisoned".into()))?;
        let mut signer_id = self
            .signer_id
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer lock poisoned".into()))?;
        let mut signing_key = self
            .signing_key
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signing-key lock poisoned".into()))?;
        let mut trusted = self
            .trusted_signers
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer-store lock poisoned".into()))?;
        let original = trusted.clone();
        trusted.rotate(
            &signer_id,
            new_signer_id,
            &new_signing_key.verifying_key().to_bytes(),
        )?;
        if let Err(error) = trusted.save_atomic(registry_path) {
            *trusted = original;
            return Err(error);
        }
        *signer_id = new_signer_id.to_string();
        *signing_key = new_signing_key;
        Ok(())
    }

    pub fn revoke_signer(
        &self,
        signer_id: &str,
        registry_path: impl AsRef<Path>,
    ) -> Result<(), SecurityError> {
        let _state = self
            .state
            .lock()
            .map_err(|_| SecurityError::Persistence("audit state lock poisoned".into()))?;
        let mut trusted = self
            .trusted_signers
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer-store lock poisoned".into()))?;
        let original = trusted.clone();
        trusted.revoke(signer_id)?;
        if let Err(error) = trusted.save_atomic(registry_path) {
            *trusted = original;
            return Err(error);
        }
        Ok(())
    }

    pub fn signer_store(&self) -> Result<AuditSignerStore, SecurityError> {
        self.trusted_signers
            .lock()
            .map(|store| store.clone())
            .map_err(|_| SecurityError::Persistence("audit signer-store lock poisoned".into()))
    }

    pub fn flush_sink(&self) -> Result<usize, SecurityError> {
        let sink = self
            .sink
            .as_ref()
            .ok_or_else(|| SecurityError::SinkPersistence("no external sink configured".into()))?
            .clone();
        let trusted = self
            .trusted_signers
            .lock()
            .map_err(|_| SecurityError::Persistence("audit signer-store lock poisoned".into()))?;
        let file = fs::File::open(&self.path)
            .map_err(|error| SecurityError::Persistence(error.to_string()))?;
        let mut count = 0;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| SecurityError::Persistence(error.to_string()))?;
            let record: AuditRecord = serde_json::from_str(&line)
                .map_err(|error| SecurityError::Serialization(error.to_string()))?;
            record.verify(&trusted)?;
            sink.persist(&record)?;
            count += 1;
        }
        Ok(count)
    }
}

fn validate_segment(value: &str, label: &str) -> Result<(), SecurityError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character == '/')
    {
        return Err(SecurityError::InvalidIdentity(format!(
            "{} must be 1 to {} bytes and contain no controls or slashes",
            label, MAX_IDENTIFIER_BYTES
        )));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), SecurityError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(SecurityError::InvalidRequest(format!(
            "{} must be 1 to {} bytes and contain no control characters",
            label, MAX_IDENTIFIER_BYTES
        )));
    }
    Ok(())
}

fn validate_fingerprint(value: &str) -> Result<(), SecurityError> {
    if value.len() != 64 || !is_hex_digest(value) {
        return Err(SecurityError::InvalidRequest(
            "peer certificate fingerprint must be a 64-character SHA-256 hex digest".into(),
        ));
    }
    Ok(())
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn identities() -> (MeshIdentity, MeshIdentity) {
        (
            MeshIdentity::new("cluster.local", "agent", "runtime").unwrap(),
            MeshIdentity::new("cluster.local", "control", "admin").unwrap(),
        )
    }

    fn mesh_request(
        source: MeshIdentity,
        destination: MeshIdentity,
        fingerprint: &str,
    ) -> MeshRequest {
        MeshRequest {
            request_id: "req-1".into(),
            audience: destination.spiffe_id(),
            source,
            destination,
            method: "state.replicate".into(),
            peer_certificate_sha256: fingerprint.into(),
        }
    }

    fn audit_setup(path: &Path) -> (AuditLog, SigningKey) {
        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let mut trusted = AuditSignerStore::default();
        trusted
            .trust_public_key("operator:mesh", &signing_key.verifying_key().to_bytes())
            .unwrap();
        (
            AuditLog::open_with_signer(path, "operator:mesh", signing_key.clone(), trusted)
                .unwrap(),
            signing_key,
        )
    }

    #[test]
    fn zero_trust_mesh_fails_closed_and_audits_allow_and_deny() {
        let (source, destination) = identities();
        let fingerprint = "a".repeat(64);
        let policy = MeshPolicy::new("cluster.local")
            .unwrap()
            .allow_peer(&source, &destination)
            .unwrap()
            .allow_method(&destination, "state.replicate")
            .unwrap()
            .trust_certificate(&source, &fingerprint)
            .unwrap();
        let mesh = ZeroTrustMesh::new(policy);
        let directory = tempdir().unwrap();
        let (audit, _) = audit_setup(&directory.path().join("audit.jsonl"));
        let allowed = mesh
            .authorize_and_audit(
                &mesh_request(source.clone(), destination.clone(), &fingerprint),
                &audit,
            )
            .unwrap();
        assert!(allowed.allowed);
        let mut denied_request = mesh_request(source, destination, &fingerprint);
        denied_request.audience = "spiffe://cluster.local/ns/other/sa/service".into();
        let denied = mesh.authorize_and_audit(&denied_request, &audit).unwrap();
        assert!(!denied.allowed);
        assert_eq!(
            std::fs::read_to_string(audit.path())
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn audit_log_reopens_chain_and_detects_tampering() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("audit.jsonl");
        let (audit, signing_key) = audit_setup(&path);
        audit
            .append(
                "consensus_commit",
                "node-a",
                "state/feature",
                "allow",
                &serde_json::json!({"index": 1}),
            )
            .unwrap();
        audit
            .append(
                "consensus_snapshot",
                "node-a",
                "state",
                "allow",
                &serde_json::json!({"index": 1}),
            )
            .unwrap();
        let mut trusted = AuditSignerStore::default();
        trusted
            .trust_public_key("operator:mesh", &signing_key.verifying_key().to_bytes())
            .unwrap();
        let reopened = AuditLog::open_with_signer(
            &path,
            "operator:mesh",
            signing_key.clone(),
            trusted.clone(),
        )
        .unwrap();
        let third = reopened
            .append(
                "consensus_commit",
                "node-a",
                "state/feature",
                "allow",
                &serde_json::json!({"index": 2}),
            )
            .unwrap();
        assert_eq!(third.sequence, 3);

        let mut tampered = std::fs::read_to_string(&path).unwrap();
        tampered = tampered.replacen("consensus_snapshot", "tampered_snapshot", 1);
        std::fs::write(&path, tampered).unwrap();
        assert!(matches!(
            AuditLog::open_with_signer(&path, "operator:mesh", signing_key, trusted),
            Err(SecurityError::ChainInvalid(_)) | Err(SecurityError::InvalidSignature)
        ));
    }

    #[test]
    fn mesh_rejects_untrusted_certificate_and_method() {
        let (source, destination) = identities();
        let fingerprint = "b".repeat(64);
        let policy = MeshPolicy::new("cluster.local")
            .unwrap()
            .allow_peer(&source, &destination)
            .unwrap()
            .allow_method(&destination, "state.replicate")
            .unwrap()
            .trust_certificate(&source, &"a".repeat(64))
            .unwrap();
        let mesh = ZeroTrustMesh::new(policy);
        let mut request = mesh_request(source, destination, &fingerprint);
        let denied = mesh.authorize(&request).unwrap();
        assert!(!denied.allowed);
        request.peer_certificate_sha256 = "a".repeat(64);
        request.method = "admin.delete".into();
        let denied_method = mesh.authorize(&request).unwrap();
        assert!(!denied_method.allowed);
    }
}
